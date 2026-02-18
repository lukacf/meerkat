use crate::error::{MobError, MobResult};
use crate::model::{
    CollectionPolicy, ConditionContext, FailureLedgerEntry, FlowContext, FlowSpec, FlowStepSpec,
    FlowId, MeerkatId, MeerkatIdentity, MeerkatInstance, MeerkatInstanceStatus, MobActivationRequest,
    MobActivationResponse, MobEventCategory, MobEventKind, MobReconcileRequest,
    MobId, MobReconcileResult, MobRun, MobRunFilter, MobRunStatus, MobSpec, NewMobEvent,
    MobSpecRecord, PolicyMode, PollEventsResponse, RoleId, RoleSpec, RunId, SchemaPolicy,
    SpecUpdateMode,
    StepId, StepLedgerEntry, StepOutput, StepRunStatus, TimeoutPolicy, UnavailablePolicy,
};
use crate::resolver::{ResolverContext, ResolverRegistry};
use crate::runtime_service::MobRuntimeService;
use crate::spec::{ApplySpecRequest, SpecValidator};
use crate::store::{MobEventStore, MobRunStore, MobSpecStore};
use chrono::Utc;
use futures::{FutureExt, StreamExt, stream::FuturesUnordered};
use indexmap::IndexMap;
use meerkat::{
    AgentToolDispatcher, CommsCommand, CommsRuntime, CreateSessionRequest, InputStreamMode,
    PeerMeta, SendReceipt, SessionBuildOptions, SessionId, SessionService,
    ToolGatewayBuilder,
};
use meerkat_core::agent::CommsRuntime as CoreCommsRuntimeTrait;
use meerkat_core::comms::TrustedPeerSpec;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::future::Future;
use std::path::PathBuf;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use uuid::Uuid;

mod conditions;
mod events;
mod flow;
mod lifecycle;
mod supervisor;
mod topology;
mod tooling;

use conditions::evaluate_condition;
use events::MobEventBuilder;
use topology::{evaluate_topology, role_pair_allowed};

#[derive(Default, Clone)]
pub struct RustToolBundleRegistry {
    bundles: Arc<RwLock<HashMap<String, Arc<dyn AgentToolDispatcher>>>>,
}

impl RustToolBundleRegistry {
    pub async fn register(
        &self,
        bundle_id: impl Into<String>,
        dispatcher: Arc<dyn AgentToolDispatcher>,
    ) {
        self.bundles.write().await.insert(bundle_id.into(), dispatcher);
    }

    pub async fn get(&self, bundle_id: &str) -> Option<Arc<dyn AgentToolDispatcher>> {
        self.bundles.read().await.get(bundle_id).cloned()
    }
}

#[derive(Clone)]
struct ManagedMeerkat {
    pub role: RoleId,
    pub meerkat_id: MeerkatId,
    pub session_id: SessionId,
    pub comms_name: String,
    pub last_activity_at: chrono::DateTime<Utc>,
    pub status: MeerkatInstanceStatus,
}

#[derive(Clone)]
struct DispatchTarget {
    pub role: RoleId,
    pub meerkat_id: MeerkatId,
    pub comms_name: String,
}

#[derive(Clone)]
pub struct MobRuntime {
    realm_id: String,
    default_model: String,
    context_root: Option<PathBuf>,
    user_config_root: Option<PathBuf>,
    _runtime_root: PathBuf,
    session_service: Arc<dyn SessionService>,
    spec_store: Arc<dyn MobSpecStore>,
    run_store: Arc<dyn MobRunStore>,
    event_store: Arc<dyn MobEventStore>,
    validator: Arc<SpecValidator>,
    resolver_registry: ResolverRegistry,
    rust_bundles: RustToolBundleRegistry,
    managed_meerkats: Arc<RwLock<HashMap<MobId, HashMap<String, ManagedMeerkat>>>>,
    spawn_locks: Arc<RwLock<HashMap<MobId, Arc<Mutex<()>>>>>,
    run_tasks: Arc<RwLock<HashMap<RunId, JoinHandle<()>>>>,
    run_status_cache: Arc<RwLock<HashMap<RunId, MobRunStatus>>>,
    inflight_dispatches: Arc<RwLock<HashMap<String, chrono::DateTime<Utc>>>>,
    supervisors: Arc<RwLock<HashMap<MobId, Arc<CommsRuntime>>>>,
    reset_epochs: Arc<RwLock<HashMap<MobId, u64>>>,
}

pub struct MobRuntimeBuilder {
    realm_id: String,
    default_model: String,
    context_root: Option<PathBuf>,
    user_config_root: Option<PathBuf>,
    runtime_root: Option<PathBuf>,
    session_service: Arc<dyn SessionService>,
    spec_store: Arc<dyn MobSpecStore>,
    run_store: Arc<dyn MobRunStore>,
    event_store: Arc<dyn MobEventStore>,
    validator: Arc<SpecValidator>,
    resolver_registry: ResolverRegistry,
    rust_bundles: RustToolBundleRegistry,
}

impl MobRuntimeBuilder {
    pub fn new(
        realm_id: impl Into<String>,
        session_service: Arc<dyn SessionService>,
        spec_store: Arc<dyn MobSpecStore>,
        run_store: Arc<dyn MobRunStore>,
        event_store: Arc<dyn MobEventStore>,
    ) -> Self {
        Self {
            realm_id: realm_id.into(),
            default_model: "claude-sonnet-4-5".to_string(),
            context_root: None,
            user_config_root: None,
            runtime_root: None,
            session_service,
            spec_store,
            run_store,
            event_store,
            validator: Arc::new(SpecValidator::new()),
            resolver_registry: ResolverRegistry::default(),
            rust_bundles: RustToolBundleRegistry::default(),
        }
    }

    pub fn context_root(mut self, context_root: PathBuf) -> Self {
        self.context_root = Some(context_root);
        self
    }

    pub fn default_model(mut self, default_model: impl Into<String>) -> Self {
        self.default_model = default_model.into();
        self
    }

    pub fn user_config_root(mut self, user_config_root: PathBuf) -> Self {
        self.user_config_root = Some(user_config_root);
        self
    }

    pub fn runtime_root(mut self, runtime_root: PathBuf) -> Self {
        self.runtime_root = Some(runtime_root);
        self
    }

    pub fn validator(mut self, validator: Arc<SpecValidator>) -> Self {
        self.validator = validator;
        self
    }

    pub fn resolver_registry(mut self, resolver_registry: ResolverRegistry) -> Self {
        self.resolver_registry = resolver_registry;
        self
    }

    pub fn rust_tool_bundles(mut self, rust_bundles: RustToolBundleRegistry) -> Self {
        self.rust_bundles = rust_bundles;
        self
    }

    pub fn build(self) -> MobResult<MobRuntime> {
        let runtime_root = self.runtime_root.ok_or(MobError::RuntimeRootMissing)?;
        Ok(MobRuntime {
            realm_id: self.realm_id,
            default_model: self.default_model,
            context_root: self.context_root,
            user_config_root: self.user_config_root,
            _runtime_root: runtime_root,
            session_service: self.session_service,
            spec_store: self.spec_store,
            run_store: self.run_store,
            event_store: self.event_store,
            validator: self.validator,
            resolver_registry: self.resolver_registry,
            rust_bundles: self.rust_bundles,
            managed_meerkats: Arc::new(RwLock::new(HashMap::new())),
            spawn_locks: Arc::new(RwLock::new(HashMap::new())),
            run_tasks: Arc::new(RwLock::new(HashMap::new())),
            run_status_cache: Arc::new(RwLock::new(HashMap::new())),
            inflight_dispatches: Arc::new(RwLock::new(HashMap::new())),
            supervisors: Arc::new(RwLock::new(HashMap::new())),
            reset_epochs: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    pub fn build_service(self) -> MobResult<MobRuntimeService> {
        Ok(MobRuntimeService::new(self.build()?))
    }
}

impl MobRuntime {
    pub(crate) async fn apply_spec(&self, request: ApplySpecRequest) -> MobResult<MobSpecRecord> {
        let mut request = request;
        let update_mode = request.update_mode;
        if request.context.context_root.is_none() {
            request.context.context_root = self.context_root.clone();
        }

        let mut specs = self.validator.validate_toml(&request).await?;
        if specs.len() != 1 {
            return Err(MobError::SpecValidation(
                "apply_spec expects exactly one mob spec".to_string(),
            ));
        }

        let mut record = specs.remove(0);
        let existing = self.spec_store.get_spec(record.mob_id.as_ref()).await?;
        if let Some(current) = existing
            && request.expected_revision.is_none()
        {
            record.spec.revision = current.revision + 1;
        }

        self.spec_store
            .put_spec(&record.mob_id, record.spec.clone(), request.expected_revision)
            .await?;

        self.emit_event(
            self.event(
                record.mob_id.clone(),
                MobEventCategory::Lifecycle,
                MobEventKind::SpecApplied,
            )
                .payload(json!({
                    "revision": record.spec.revision,
                    "update_mode": update_mode,
                }))
                .build(),
        )
        .await?;

        match update_mode {
            SpecUpdateMode::DrainReplace => {}
            SpecUpdateMode::HotReload => {
                self.emit_event(
                    self.event(
                        record.mob_id.clone(),
                        MobEventCategory::Supervisor,
                        MobEventKind::Warning,
                    )
                        .payload(json!({
                            "warning": "hot_reload requested; v1 behavior is drain_replace",
                        }))
                        .build(),
                )
                .await?;
            }
            SpecUpdateMode::ForceReset => {
                self.force_reset_mob(&record.mob_id).await?;
            }
        }

        Ok(record)
    }

    pub(crate) async fn get_spec(&self, mob_id: &str) -> MobResult<Option<MobSpecRecord>> {
        let mob_id = MobId::from(mob_id);
        let spec = self.spec_store.get_spec(mob_id.as_ref()).await?;
        Ok(spec.map(|spec| MobSpecRecord { mob_id, spec }))
    }

    pub(crate) async fn list_specs(&self) -> MobResult<Vec<MobSpecRecord>> {
        self.spec_store.list_specs().await
    }

    pub(crate) async fn delete_spec(&self, mob_id: &str) -> MobResult<()> {
        self.spec_store.delete_spec(mob_id).await?;
        self.emit_event(
            self.event(
                MobId::from(mob_id),
                MobEventCategory::Lifecycle,
                MobEventKind::SpecDeleted,
            )
                .payload(json!({}))
                .build(),
        )
        .await?;
        Ok(())
    }

    pub(crate) async fn activate(&self, request: MobActivationRequest) -> MobResult<MobActivationResponse> {
        let spec = self
            .spec_store
            .get_spec(request.mob_id.as_ref())
            .await?
            .ok_or_else(|| MobError::SpecNotFound {
                mob_id: request.mob_id.to_string(),
            })?;

        if !spec.flows.contains_key(request.flow_id.as_ref()) {
            return Err(MobError::FlowNotFound {
                flow_id: request.flow_id.to_string(),
            });
        }

        let run_id = RunId::from(Uuid::now_v7().to_string());
        let run = MobRun::new(
            run_id.clone(),
            request.mob_id.clone(),
            request.flow_id.clone(),
            spec.revision,
            request.payload.clone(),
        );
        self.run_store.create_run(run).await?;
        self.cache_run_status(run_id.clone(), MobRunStatus::Pending).await;

        self.emit_event(
            self.event(request.mob_id.clone(), MobEventCategory::Flow, MobEventKind::RunActivated)
                .run_id(run_id.clone())
                .flow_id(request.flow_id.clone())
                .payload(json!({"dry_run": request.dry_run}))
                .build(),
        )
        .await?;

        let run_id_for_task = run_id.clone();
        let spec_revision = spec.revision;
        let expected_epoch = self.current_reset_epoch(&request.mob_id).await;
        let pinned_spec = Arc::new(spec);
        let runtime = self.clone();
        self.spawn_tracked_run_task(run_id.clone(), async move {
            let outcome = AssertUnwindSafe(runtime.execute_run(
                run_id_for_task.clone(),
                request,
                pinned_spec,
                expected_epoch,
            ))
            .catch_unwind()
            .await;

            match outcome {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    tracing::error!(run_id = %run_id_for_task, "mob run execution failed: {err}");
                }
                Err(_) => {
                    tracing::error!(run_id = %run_id_for_task, "mob run panicked");
                }
            }
        })
        .await;

        Ok(MobActivationResponse {
            run_id,
            status: MobRunStatus::Pending,
            spec_revision,
        })
    }

    pub(crate) async fn get_run(&self, run_id: &str) -> MobResult<Option<MobRun>> {
        self.run_store.get_run(run_id).await
    }

    pub(crate) async fn list_runs(&self, filter: MobRunFilter) -> MobResult<Vec<MobRun>> {
        self.run_store.list_runs(filter).await
    }

    pub(crate) async fn cancel_run(&self, run_id: &str) -> MobResult<()> {
        let changed_running = self
            .run_store
            .cas_run_status(run_id, MobRunStatus::Running, MobRunStatus::Canceled)
            .await?;
        let changed_pending = if !changed_running {
            self
                .run_store
                .cas_run_status(run_id, MobRunStatus::Pending, MobRunStatus::Canceled)
                .await?
        } else {
            false
        };

        let run = self
            .run_store
            .get_run(run_id)
            .await?
            .ok_or_else(|| MobError::RunNotFound {
                run_id: run_id.to_string(),
            })?;

        if !changed_running && !changed_pending && run.status != MobRunStatus::Canceled {
            return Ok(());
        }
        self.cache_run_status(RunId::from(run_id), MobRunStatus::Canceled)
            .await;

        self.emit_event(
            self.event(run.mob_id, MobEventCategory::Flow, MobEventKind::RunCanceled)
                .run_id(run_id)
                .flow_id(run.flow_id)
                .payload(json!({}))
                .build(),
        )
        .await?;

        Ok(())
    }

    pub(crate) async fn list_meerkats(&self, mob_id: &str) -> MobResult<Vec<MeerkatInstance>> {
        let mob_id = MobId::from(mob_id);
        if self.spec_store.get_spec(mob_id.as_ref()).await?.is_none() {
            return Ok(Vec::new());
        }

        let supervisor = self.supervisor_runtime(&mob_id).await?;
        let peers = supervisor.peers().await;
        let supervisor_name = self.supervisor_name_from_mob(&mob_id);
        let managed = self.managed_meerkats.read().await;
        let managed_map = managed.get(&mob_id);

        let mut out = Vec::new();
        for peer in peers {
            if peer.name.as_ref() == supervisor_name {
                continue;
            }
            if peer.meta.labels.get("mob_id") != Some(&mob_id.to_string()) {
                continue;
            }
            let Some(role) = peer.meta.labels.get("role") else {
                continue;
            };
            let Some(meerkat_id) = peer.meta.labels.get("meerkat_id") else {
                continue;
            };
            let key = format!("{role}/{meerkat_id}");
            let managed_instance = managed_map.and_then(|map| map.get(&key));
            out.push(MeerkatInstance {
                mob_id: mob_id.clone(),
                role: RoleId::from(role.as_str()),
                meerkat_id: MeerkatId::from(meerkat_id.as_str()),
                session_id: managed_instance
                    .map(|instance| instance.session_id.to_string())
                    .unwrap_or_default(),
                comms_name: peer.name.to_string(),
                labels: peer.meta.labels.clone().into_iter().collect(),
                last_activity_at: managed_instance
                    .map(|instance| instance.last_activity_at)
                    .unwrap_or_else(Utc::now),
                status: managed_instance
                    .map(|instance| instance.status)
                    .unwrap_or(MeerkatInstanceStatus::Running),
            });
        }
        out.sort_by(|a, b| a.comms_name.cmp(&b.comms_name));
        Ok(out)
    }

    pub(crate) async fn reconcile(&self, request: MobReconcileRequest) -> MobResult<MobReconcileResult> {
        let spec = self
            .spec_store
            .get_spec(request.mob_id.as_ref())
            .await?
            .ok_or_else(|| MobError::SpecNotFound {
                mob_id: request.mob_id.to_string(),
            })?;

        let mut desired = HashMap::<String, (String, MeerkatIdentity)>::new();

        for (role_name, role) in &spec.roles {
            let identities = self
                .resolve_role_meerkats(&request.mob_id, &spec, role_name, role, Value::Null)
                .await?;
            for identity in identities {
                let key = format!("{}/{}", identity.role, identity.meerkat_id);
                desired.insert(key, (role_name.clone(), identity));
            }
        }

        let existing_keys: Vec<String> = {
            let managed = self.managed_meerkats.read().await;
            managed
                .get(&request.mob_id)
                .map(|map| map.keys().cloned().collect())
                .unwrap_or_default()
        };

        let mut result = MobReconcileResult::default();

        for key in &existing_keys {
            if !desired.contains_key(key) {
                if matches!(request.mode, crate::model::ReconcileMode::Apply) {
                    self.retire_meerkat(&request.mob_id, key).await?;
                    result.retired.push(key.clone());
                }
            } else {
                result.unchanged.push(key.clone());
            }
        }

        for (key, (role_name, identity)) in desired {
            if !existing_keys.contains(&key) {
                if matches!(request.mode, crate::model::ReconcileMode::Apply) {
                    let role = spec.roles.get(role_name.as_str()).ok_or_else(|| {
                        MobError::SpecValidation(format!(
                            "missing role '{}' during reconcile",
                            role_name
                        ))
                    })?;
                    if should_spawn_on_reconcile(role) {
                        let spawn_lock = self.spawn_lock(&request.mob_id).await;
                        let _spawn_guard = spawn_lock.lock().await;
                        let already_exists = {
                            let managed = self.managed_meerkats.read().await;
                            managed
                                .get(&request.mob_id)
                                .and_then(|m| m.get(&key))
                                .is_some()
                        };
                        if !already_exists {
                            self.spawn_meerkat(&request.mob_id, &spec, role_name.as_str(), &identity)
                                .await?;
                            result.spawned.push(key);
                        } else {
                            result.unchanged.push(key);
                        }
                    } else {
                        result.unchanged.push(key);
                    }
                } else {
                    result.spawned.push(key);
                }
            }
        }

        self.emit_event(
            self.event(
                request.mob_id,
                MobEventCategory::Lifecycle,
                MobEventKind::ReconcileApplied,
            )
            .payload(json!({
                "spawned": result.spawned,
                "retired": result.retired,
                "unchanged": result.unchanged,
            }))
            .build(),
        )
        .await?;

        Ok(result)
    }

    pub(crate) async fn poll_events(
        &self,
        cursor: Option<u64>,
        limit: Option<usize>,
    ) -> MobResult<PollEventsResponse> {
        let (next_cursor, events) = self.event_store.poll_events(cursor, limit).await?;
        Ok(PollEventsResponse {
            next_cursor,
            events,
        })
    }

    pub(crate) async fn capabilities(&self) -> MobResult<Value> {
        Ok(json!({
            "plugin": "meerkat-mob",
            "version": env!("CARGO_PKG_VERSION"),
            "delivery": ["library", "mcp"],
            "strict_mode": "best_effort_operational",
            "flow_model": "parallel_dag",
            "dispatch_modes": ["fan_out", "one_to_one"],
            "reserved_dispatch_modes": ["fan_in"],
        }))
    }

    pub(crate) async fn emit_event(&self, event: NewMobEvent) -> MobResult<()> {
        let _ = self.event_store.append_event(event).await?;
        Ok(())
    }

    pub(crate) async fn shutdown(&self) -> MobResult<()> {
        let drained: Vec<(RunId, JoinHandle<()>)> = {
            let mut tasks = self.run_tasks.write().await;
            tasks.drain().collect()
        };

        for (_, handle) in &drained {
            handle.abort();
        }

        for (run_id, handle) in drained {
            match handle.await {
                Ok(()) => {}
                Err(err) if err.is_cancelled() => {}
                Err(err) => {
                    return Err(MobError::Internal(format!(
                        "run task '{run_id}' join failed: {err}"
                    )));
                }
            }
        }

        self.supervisors.write().await.clear();
        self.run_status_cache.write().await.clear();
        Ok(())
    }
}

impl MobRuntime {
    fn event(&self, mob_id: MobId, category: MobEventCategory, kind: MobEventKind) -> MobEventBuilder {
        MobEventBuilder::new(mob_id, category, kind)
    }

    async fn spawn_tracked_run_task<F>(&self, run_id: RunId, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let run_tasks = self.run_tasks.clone();
        let run_id_for_cleanup = run_id.clone();
        let handle = tokio::spawn(async move {
            future.await;
            run_tasks.write().await.remove(&run_id_for_cleanup);
        });
        self.run_tasks.write().await.insert(run_id, handle);
    }

    #[cfg(test)]
    async fn active_run_task_count(&self) -> usize {
        self.run_tasks.read().await.len()
    }

    async fn cache_run_status(&self, run_id: RunId, status: MobRunStatus) {
        self.run_status_cache.write().await.insert(run_id, status);
    }

    async fn cached_run_status(&self, run_id: &RunId) -> Option<MobRunStatus> {
        self.run_status_cache.read().await.get(run_id).copied()
    }

    async fn record_dispatch_start(&self, logical_key: &str) {
        self.inflight_dispatches
            .write()
            .await
            .insert(logical_key.to_string(), Utc::now());
    }

    async fn record_dispatch_finish(&self, logical_key: &str) {
        self.inflight_dispatches.write().await.remove(logical_key);
    }

    async fn inflight_dispatch_count(&self) -> usize {
        self.inflight_dispatches.read().await.len()
    }

}

#[derive(Debug)]
struct StepExecutionResult {
    status: StepRunStatus,
    output: Value,
}

struct StepExecutionInput<'a> {
    mob_id: MobId,
    spec: &'a MobSpec,
    flow_id: &'a FlowId,
    run_id: &'a RunId,
    step: FlowStepSpec,
    dependency_outputs: BTreeMap<StepId, Value>,
    activation_payload: Value,
    expected_epoch: u64,
}

struct DispatchTargetInput<'a> {
    mob_id: MobId,
    run_id: &'a RunId,
    flow_id: &'a FlowId,
    step: &'a FlowStepSpec,
    supervisor: Arc<CommsRuntime>,
    target: DispatchTarget,
    intent: String,
    payload: Value,
    spec: &'a MobSpec,
    expected_epoch: u64,
}

struct CompileRoleSessionInput<'a> {
    mob_id: &'a MobId,
    spec: &'a MobSpec,
    role_name: &'a str,
    role: &'a RoleSpec,
    labels: &'a BTreeMap<String, String>,
    comms_name: &'a str,
    namespace: String,
}

#[derive(Default)]
struct RoleTooling {
    enable_builtins: bool,
    enable_shell: bool,
    enable_subagents: bool,
    enable_memory: bool,
    external_tools: Option<Arc<dyn AgentToolDispatcher>>,
}

fn should_spawn_on_reconcile(role: &RoleSpec) -> bool {
    match role.spawn_strategy {
        crate::model::SpawnStrategy::Eager => true,
        crate::model::SpawnStrategy::Lazy => false,
        crate::model::SpawnStrategy::Hybrid => {
            matches!(role.cardinality, crate::model::CardinalityKind::Singleton)
        }
    }
}

#[must_use]
fn should_stop_collection(
    policy: &CollectionPolicy,
    target_count: usize,
    success_count: usize,
    timeout_failures: usize,
    non_timeout_failures: usize,
    pending_count: usize,
) -> bool {
    if pending_count == 0 {
        return true;
    }

    match policy {
        CollectionPolicy::All => {
            success_count == target_count || timeout_failures > 0 || non_timeout_failures > 0
        }
        CollectionPolicy::Quorum { n } => {
            if success_count >= *n {
                return true;
            }
            success_count + pending_count < *n
        }
        CollectionPolicy::Any => success_count >= 1,
    }
}

#[must_use]
fn classify_step_status(
    policy: &CollectionPolicy,
    on_timeout: TimeoutPolicy,
    target_count: usize,
    success_count: usize,
    timeout_failures: usize,
    non_timeout_failures: usize,
) -> StepRunStatus {
    if target_count == 0 {
        return StepRunStatus::Failed;
    }

    let timeout_status = if matches!(on_timeout, TimeoutPolicy::Partial) {
        StepRunStatus::Partial
    } else {
        StepRunStatus::Failed
    };

    match policy {
        CollectionPolicy::All => {
            if success_count == target_count {
                StepRunStatus::Completed
            } else if non_timeout_failures > 0 {
                StepRunStatus::Failed
            } else if timeout_failures > 0 {
                timeout_status
            } else {
                StepRunStatus::Failed
            }
        }
        CollectionPolicy::Quorum { n } => {
            if success_count >= *n {
                StepRunStatus::Completed
            } else if non_timeout_failures > 0 {
                StepRunStatus::Failed
            } else if timeout_failures > 0 {
                timeout_status
            } else {
                StepRunStatus::Failed
            }
        }
        CollectionPolicy::Any => {
            if success_count >= 1 {
                StepRunStatus::Completed
            } else if non_timeout_failures > 0 {
                StepRunStatus::Failed
            } else if timeout_failures > 0 {
                timeout_status
            } else {
                StepRunStatus::Failed
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use crate::model::{ConditionContext, ConditionExpr, TopologyDomainSpec, TopologyRule};
    use crate::store::{InMemoryMobEventStore, InMemoryMobRunStore, InMemoryMobSpecStore};
    use meerkat::Session;
    use serde_json::json;

    #[tokio::test]
    async fn topology_matching_respects_rules() {
        let domain = TopologyDomainSpec {
            mode: PolicyMode::Strict,
            rules: vec![TopologyRule {
                from_roles: vec!["supervisor".to_string()],
                to_roles: vec!["reviewer".to_string()],
                kinds: vec!["peer_request".to_string()],
                intents: vec!["delegate".to_string()],
            }],
        };

        assert!(evaluate_topology(
            &domain,
            "supervisor",
            "reviewer",
            "peer_request",
            "delegate",
        ));

        assert!(!evaluate_topology(
            &domain,
            "supervisor",
            "reviewer",
            "peer_request",
            "status",
        ));
    }

    #[tokio::test]
    async fn condition_dsl_evaluates_paths() {
        let outputs = BTreeMap::from([(
            StepId::from("dispatch"),
            crate::model::StepOutput {
                status: StepRunStatus::Partial,
                count: 3,
                output: json!({"count": 3, "status": "partial"}),
            },
        )]);
        let context = ConditionContext {
            activation: json!({"priority": "high"}),
            steps: outputs,
        };

        assert!(evaluate_condition(
            &ConditionExpr::Eq {
                left: "step.dispatch.status".to_string(),
                right: json!("partial"),
            },
            &context,
        ));

        assert!(evaluate_condition(
            &ConditionExpr::Gt {
                left: "step.dispatch.count".to_string(),
                right: json!(2),
            },
            &context,
        ));
    }

    #[tokio::test]
    async fn builder_creates_runtime() {
        let runtime = MobRuntimeBuilder::new(
            "realm-a",
            Arc::new(MockSessionService),
            Arc::new(InMemoryMobSpecStore::default()),
            Arc::new(InMemoryMobRunStore::default()),
            Arc::new(InMemoryMobEventStore::default()),
        )
        .runtime_root(PathBuf::from("/tmp"))
        .build();

        assert!(runtime.is_ok());
    }

    #[tokio::test]
    async fn tracked_run_task_registers_and_cleans_up() {
        let runtime = MobRuntimeBuilder::new(
            "realm-a",
            Arc::new(MockSessionService),
            Arc::new(InMemoryMobSpecStore::default()),
            Arc::new(InMemoryMobRunStore::default()),
            Arc::new(InMemoryMobEventStore::default()),
        )
        .runtime_root(PathBuf::from("/tmp"))
        .build()
        .unwrap();

        runtime
            .spawn_tracked_run_task(RunId::from("run-test"), async {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            })
            .await;
        assert_eq!(runtime.active_run_task_count().await, 1);

        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        assert_eq!(runtime.active_run_task_count().await, 0);
    }

    #[tokio::test]
    async fn shutdown_aborts_and_clears_run_tasks() {
        let runtime = MobRuntimeBuilder::new(
            "realm-a",
            Arc::new(MockSessionService),
            Arc::new(InMemoryMobSpecStore::default()),
            Arc::new(InMemoryMobRunStore::default()),
            Arc::new(InMemoryMobEventStore::default()),
        )
        .runtime_root(PathBuf::from("/tmp"))
        .build()
        .unwrap();

        runtime
            .spawn_tracked_run_task(RunId::from("run-shutdown"), async {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            })
            .await;
        assert_eq!(runtime.active_run_task_count().await, 1);

        runtime.shutdown().await.unwrap();
        assert_eq!(runtime.active_run_task_count().await, 0);
    }

    #[tokio::test]
    async fn inflight_dispatch_registry_tracks_start_finish() {
        let runtime = MobRuntimeBuilder::new(
            "realm-a",
            Arc::new(MockSessionService),
            Arc::new(InMemoryMobSpecStore::default()),
            Arc::new(InMemoryMobRunStore::default()),
            Arc::new(InMemoryMobEventStore::default()),
        )
        .runtime_root(PathBuf::from("/tmp"))
        .build()
        .unwrap();

        runtime.record_dispatch_start("run:s1:m1").await;
        assert_eq!(runtime.inflight_dispatch_count().await, 1);
        runtime.record_dispatch_finish("run:s1:m1").await;
        assert_eq!(runtime.inflight_dispatch_count().await, 0);
    }

    #[derive(Default)]
    struct MockSessionService;

    #[async_trait]
    impl SessionService for MockSessionService {
        async fn create_session(
            &self,
            _req: CreateSessionRequest,
        ) -> Result<meerkat::RunResult, meerkat::SessionError> {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            Ok(meerkat::RunResult {
                text: String::new(),
                session_id: Session::new().id().clone(),
                turns: 0,
                tool_calls: 0,
                usage: meerkat::Usage::default(),
                structured_output: None,
                schema_warnings: None,
            })
        }

        async fn start_turn(
            &self,
            _id: &SessionId,
            _req: meerkat::StartTurnRequest,
        ) -> Result<meerkat::RunResult, meerkat::SessionError> {
            Ok(meerkat::RunResult {
                text: String::new(),
                session_id: Session::new().id().clone(),
                turns: 0,
                tool_calls: 0,
                usage: meerkat::Usage::default(),
                structured_output: None,
                schema_warnings: None,
            })
        }

        async fn interrupt(&self, _id: &SessionId) -> Result<(), meerkat::SessionError> {
            Ok(())
        }

        async fn read(&self, _id: &SessionId) -> Result<meerkat::SessionView, meerkat::SessionError> {
            Err(meerkat::SessionError::NotFound {
                id: Session::new().id().clone(),
            })
        }

        async fn list(
            &self,
            _query: meerkat::SessionQuery,
        ) -> Result<Vec<meerkat::SessionSummary>, meerkat::SessionError> {
            Ok(Vec::new())
        }

        async fn archive(&self, _id: &SessionId) -> Result<(), meerkat::SessionError> {
            Ok(())
        }
    }
}

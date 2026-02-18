use crate::error::{MobError, MobResult};
use crate::model::{
    CollectionPolicy, FailureLedgerEntry, FlowSpec, FlowStepSpec, MeerkatIdentity,
    MeerkatInstance, MeerkatInstanceStatus, MobActivationRequest, MobActivationResponse, MobEvent,
    MobEventCategory, MobEventKind, MobReconcileRequest, MobReconcileResult, MobRun,
    MobRunFilter, MobRunStatus, MobSpec, NewMobEvent, PolicyMode, PollEventsResponse, RoleSpec,
    SchemaPolicy, SpecUpdateMode, StepLedgerEntry, StepRunStatus, TimeoutPolicy, UnavailablePolicy,
};
use crate::resolver::{ResolverContext, ResolverRegistry};
use crate::service::MobService;
use crate::spec::{ApplySpecRequest, SpecValidator};
use crate::store::{MobEventStore, MobRunStore, MobSpecStore};
use async_trait::async_trait;
use chrono::Utc;
use futures::{FutureExt, StreamExt, stream::FuturesUnordered};
use indexmap::IndexMap;
use meerkat::{
    AgentFactory, AgentToolDispatcher, CommsCommand, CommsRuntime, CreateSessionRequest,
    InputStreamMode, PeerMeta, SendReceipt, Session, SessionBuildOptions, SessionId,
    SessionService, ToolGatewayBuilder,
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
mod topology;

use conditions::evaluate_condition;
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
    pub role: String,
    pub meerkat_id: String,
    pub session_id: SessionId,
    pub comms_name: String,
    pub labels: BTreeMap<String, String>,
    pub last_activity_at: chrono::DateTime<Utc>,
    pub status: MeerkatInstanceStatus,
}

#[derive(Clone)]
pub struct MobRuntime {
    realm_id: String,
    default_model: String,
    context_root: Option<PathBuf>,
    user_config_root: Option<PathBuf>,
    _runtime_root: PathBuf,
    session_service: Arc<dyn SessionService>,
    _agent_factory: AgentFactory,
    spec_store: Arc<dyn MobSpecStore>,
    run_store: Arc<dyn MobRunStore>,
    event_store: Arc<dyn MobEventStore>,
    validator: Arc<SpecValidator>,
    resolver_registry: ResolverRegistry,
    rust_bundles: RustToolBundleRegistry,
    managed_meerkats: Arc<RwLock<HashMap<String, HashMap<String, ManagedMeerkat>>>>,
    spawn_locks: Arc<RwLock<HashMap<String, Arc<Mutex<()>>>>>,
    run_tasks: Arc<RwLock<HashMap<String, JoinHandle<()>>>>,
    supervisors: Arc<RwLock<HashMap<String, Arc<CommsRuntime>>>>,
    reset_epochs: Arc<RwLock<HashMap<String, u64>>>,
}

pub struct MobRuntimeBuilder {
    realm_id: String,
    default_model: String,
    context_root: Option<PathBuf>,
    user_config_root: Option<PathBuf>,
    runtime_root: Option<PathBuf>,
    session_service: Arc<dyn SessionService>,
    agent_factory: AgentFactory,
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
        agent_factory: AgentFactory,
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
            agent_factory,
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
            _agent_factory: self.agent_factory,
            spec_store: self.spec_store,
            run_store: self.run_store,
            event_store: self.event_store,
            validator: self.validator,
            resolver_registry: self.resolver_registry,
            rust_bundles: self.rust_bundles,
            managed_meerkats: Arc::new(RwLock::new(HashMap::new())),
            spawn_locks: Arc::new(RwLock::new(HashMap::new())),
            run_tasks: Arc::new(RwLock::new(HashMap::new())),
            supervisors: Arc::new(RwLock::new(HashMap::new())),
            reset_epochs: Arc::new(RwLock::new(HashMap::new())),
        })
    }
}

#[async_trait]
impl MobService for MobRuntime {
    async fn apply_spec(&self, request: ApplySpecRequest) -> MobResult<MobSpec> {
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

        let mut spec = specs.remove(0);
        let existing = self.spec_store.get_spec(&spec.mob_id).await?;
        if let Some(current) = existing
            && request.expected_revision.is_none()
        {
            spec.revision = current.revision + 1;
        }

        self.spec_store
            .put_spec(spec.clone(), request.expected_revision)
            .await?;

        self.emit_event(MobEvent {
            cursor: 0,
            timestamp: Utc::now(),
            category: MobEventCategory::Lifecycle,
            mob_id: spec.mob_id.clone(),
            run_id: None,
            flow_id: None,
            step_id: None,
            meerkat_id: None,
            kind: MobEventKind::SpecApplied,
            payload: json!({
                "revision": spec.revision,
                "update_mode": update_mode,
            }),
        })
        .await?;

        match update_mode {
            SpecUpdateMode::DrainReplace => {}
            SpecUpdateMode::HotReload => {
                self.emit_event(MobEvent {
                    cursor: 0,
                    timestamp: Utc::now(),
                    category: MobEventCategory::Supervisor,
                    mob_id: spec.mob_id.clone(),
                    run_id: None,
                    flow_id: None,
                    step_id: None,
                    meerkat_id: None,
                    kind: MobEventKind::Warning,
                    payload: json!({
                        "warning": "hot_reload requested; v1 behavior is drain_replace",
                    }),
                })
                .await?;
            }
            SpecUpdateMode::ForceReset => {
                self.force_reset_mob(&spec.mob_id).await?;
            }
        }

        Ok(spec)
    }

    async fn get_spec(&self, mob_id: &str) -> MobResult<Option<MobSpec>> {
        self.spec_store.get_spec(mob_id).await
    }

    async fn list_specs(&self) -> MobResult<Vec<MobSpec>> {
        self.spec_store.list_specs().await
    }

    async fn delete_spec(&self, mob_id: &str) -> MobResult<()> {
        self.spec_store.delete_spec(mob_id).await?;
        self.emit_event(MobEvent {
            cursor: 0,
            timestamp: Utc::now(),
            category: MobEventCategory::Lifecycle,
            mob_id: mob_id.to_string(),
            run_id: None,
            flow_id: None,
            step_id: None,
            meerkat_id: None,
            kind: MobEventKind::SpecDeleted,
            payload: json!({}),
        })
        .await?;
        Ok(())
    }

    async fn activate(&self, request: MobActivationRequest) -> MobResult<MobActivationResponse> {
        let spec = self
            .spec_store
            .get_spec(&request.mob_id)
            .await?
            .ok_or_else(|| MobError::SpecNotFound {
                mob_id: request.mob_id.clone(),
            })?;

        if !spec.flows.contains_key(&request.flow_id) {
            return Err(MobError::FlowNotFound {
                flow_id: request.flow_id,
            });
        }

        let run_id = Uuid::now_v7().to_string();
        let run = MobRun::new(
            run_id.clone(),
            request.mob_id.clone(),
            request.flow_id.clone(),
            spec.revision,
            request.payload.clone(),
        );
        self.run_store.create_run(run).await?;

        self.emit_event(MobEvent {
            cursor: 0,
            timestamp: Utc::now(),
            category: MobEventCategory::Flow,
            mob_id: request.mob_id.clone(),
            run_id: Some(run_id.clone()),
            flow_id: Some(request.flow_id.clone()),
            step_id: None,
            meerkat_id: None,
            kind: MobEventKind::RunActivated,
            payload: json!({"dry_run": request.dry_run}),
        })
        .await?;

        let run_id_for_task = run_id.clone();
        let expected_epoch = self.current_reset_epoch(&request.mob_id).await;
        let pinned_spec = spec.clone();
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
            spec_revision: spec.revision,
        })
    }

    async fn get_run(&self, run_id: &str) -> MobResult<Option<MobRun>> {
        self.run_store.get_run(run_id).await
    }

    async fn list_runs(&self, filter: MobRunFilter) -> MobResult<Vec<MobRun>> {
        self.run_store.list_runs(filter).await
    }

    async fn cancel_run(&self, run_id: &str) -> MobResult<()> {
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

        self.emit_event(MobEvent {
            cursor: 0,
            timestamp: Utc::now(),
            category: MobEventCategory::Flow,
            mob_id: run.mob_id,
            run_id: Some(run_id.to_string()),
            flow_id: Some(run.flow_id),
            step_id: None,
            meerkat_id: None,
            kind: MobEventKind::RunCanceled,
            payload: json!({}),
        })
        .await?;

        Ok(())
    }

    async fn list_meerkats(&self, mob_id: &str) -> MobResult<Vec<MeerkatInstance>> {
        let managed = self.managed_meerkats.read().await;
        let Some(map) = managed.get(mob_id) else {
            return Ok(Vec::new());
        };

        let mut out = Vec::new();
        for instance in map.values() {
            out.push(MeerkatInstance {
                mob_id: mob_id.to_string(),
                role: instance.role.clone(),
                meerkat_id: instance.meerkat_id.clone(),
                session_id: instance.session_id.to_string(),
                comms_name: instance.comms_name.clone(),
                labels: instance.labels.clone(),
                last_activity_at: instance.last_activity_at,
                status: instance.status,
            });
        }
        out.sort_by(|a, b| a.comms_name.cmp(&b.comms_name));
        Ok(out)
    }

    async fn reconcile(&self, request: MobReconcileRequest) -> MobResult<MobReconcileResult> {
        let spec = self
            .spec_store
            .get_spec(&request.mob_id)
            .await?
            .ok_or_else(|| MobError::SpecNotFound {
                mob_id: request.mob_id.clone(),
            })?;

        let mut desired = HashMap::<String, (String, MeerkatIdentity)>::new();

        for (role_name, role) in &spec.roles {
            let identities = self
                .resolve_role_meerkats(&spec, role_name, role, Value::Null)
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
                        let spawn_lock = self.spawn_lock(&spec.mob_id).await;
                        let _spawn_guard = spawn_lock.lock().await;
                        let already_exists = {
                            let managed = self.managed_meerkats.read().await;
                            managed
                                .get(&spec.mob_id)
                                .and_then(|m| m.get(&key))
                                .is_some()
                        };
                        if !already_exists {
                            self.spawn_meerkat(&spec, role_name.as_str(), &identity).await?;
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

        self.emit_event(MobEvent {
            cursor: 0,
            timestamp: Utc::now(),
            category: MobEventCategory::Lifecycle,
            mob_id: request.mob_id,
            run_id: None,
            flow_id: None,
            step_id: None,
            meerkat_id: None,
            kind: MobEventKind::ReconcileApplied,
            payload: json!({
                "spawned": result.spawned,
                "retired": result.retired,
                "unchanged": result.unchanged,
            }),
        })
        .await?;

        Ok(result)
    }

    async fn poll_events(
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

    async fn capabilities(&self) -> MobResult<Value> {
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

    async fn emit_event(&self, event: MobEvent) -> MobResult<()> {
        let _ = self
            .event_store
            .append_event(NewMobEvent {
                timestamp: event.timestamp,
                category: event.category,
                mob_id: event.mob_id,
                run_id: event.run_id,
                flow_id: event.flow_id,
                step_id: event.step_id,
                meerkat_id: event.meerkat_id,
                kind: event.kind,
                payload: event.payload,
            })
            .await?;
        Ok(())
    }
}

impl MobRuntime {
    async fn execute_run(
        &self,
        run_id: String,
        request: MobActivationRequest,
        spec: MobSpec,
        expected_epoch: u64,
    ) -> MobResult<()> {
        let flow = spec
            .flows
            .get(&request.flow_id)
            .cloned()
            .ok_or_else(|| MobError::FlowNotFound {
                flow_id: request.flow_id.clone(),
            })?;

        let _ = self
            .run_store
            .cas_run_status(&run_id, MobRunStatus::Pending, MobRunStatus::Running)
            .await?;

        if request.dry_run {
            let _ = self
                .run_store
                .cas_run_status(&run_id, MobRunStatus::Running, MobRunStatus::Completed)
                .await?;
            self.emit_event(MobEvent {
                cursor: 0,
                timestamp: Utc::now(),
                category: MobEventCategory::Flow,
                mob_id: spec.mob_id.clone(),
                run_id: Some(run_id.clone()),
                flow_id: Some(request.flow_id.clone()),
                step_id: None,
                meerkat_id: None,
                kind: MobEventKind::RunCompleted,
                payload: json!({
                    "status": MobRunStatus::Completed,
                    "dry_run": true,
                }),
            })
            .await?;
            return Ok(());
        }

        let _ = self
            .reconcile(MobReconcileRequest {
                mob_id: request.mob_id.clone(),
                mode: crate::model::ReconcileMode::Apply,
            })
            .await?;

        let run_result = self
            .execute_flow(
                &spec,
                request.flow_id.as_str(),
                &flow,
                &run_id,
                request.payload.clone(),
                expected_epoch,
            )
            .await;

        match run_result {
            Ok(has_failures) => {
                if self.is_run_canceled(&run_id).await? {
                    return Ok(());
                }

                let target_status = if has_failures {
                    MobRunStatus::Failed
                } else {
                    MobRunStatus::Completed
                };

                let _ = self
                    .run_store
                    .cas_run_status(&run_id, MobRunStatus::Running, target_status)
                    .await?;

                self.emit_event(MobEvent {
                    cursor: 0,
                    timestamp: Utc::now(),
                    category: MobEventCategory::Flow,
                    mob_id: spec.mob_id.clone(),
                    run_id: Some(run_id.clone()),
                    flow_id: Some(request.flow_id.clone()),
                    step_id: None,
                    meerkat_id: None,
                    kind: if target_status == MobRunStatus::Completed {
                        MobEventKind::RunCompleted
                    } else {
                        MobEventKind::RunFailed
                    },
                    payload: json!({"status": target_status}),
                })
                .await?;

                if target_status == MobRunStatus::Failed {
                    self.supervisor_escalate(
                        &spec,
                        &run_id,
                        request.flow_id.as_str(),
                        "run finished with failed steps",
                        json!({"status": "failed"}),
                    )
                    .await?;
                }
            }
            Err(MobError::RunCanceled { .. } | MobError::ResetBarrier { .. }) => {
                let _ = self
                    .run_store
                    .cas_run_status(&run_id, MobRunStatus::Running, MobRunStatus::Canceled)
                    .await?;
            }
            Err(err) => {
                if self.is_run_canceled(&run_id).await? {
                    return Ok(());
                }

                let _ = self
                    .run_store
                    .cas_run_status(&run_id, MobRunStatus::Running, MobRunStatus::Failed)
                    .await?;
                self.run_store
                    .append_failure_entry(
                        &run_id,
                        FailureLedgerEntry {
                            timestamp: Utc::now(),
                            step_id: None,
                            target_meerkat: None,
                            error: err.to_string(),
                        },
                    )
                    .await?;

                self.emit_event(MobEvent {
                    cursor: 0,
                    timestamp: Utc::now(),
                    category: MobEventCategory::Flow,
                    mob_id: spec.mob_id.clone(),
                    run_id: Some(run_id.clone()),
                    flow_id: Some(request.flow_id.clone()),
                    step_id: None,
                    meerkat_id: None,
                    kind: MobEventKind::RunFailed,
                    payload: json!({"error": err.to_string()}),
                })
                .await?;

                self.supervisor_escalate(
                    &spec,
                    &run_id,
                    request.flow_id.as_str(),
                    "run execution error",
                    json!({"error": err.to_string()}),
                )
                .await?;
            }
        }

        Ok(())
    }

    async fn supervisor_escalate(
        &self,
        spec: &MobSpec,
        run_id: &str,
        flow_id: &str,
        reason: &str,
        details: Value,
    ) -> MobResult<()> {
        self.emit_event(MobEvent {
            cursor: 0,
            timestamp: Utc::now(),
            category: MobEventCategory::Supervisor,
            mob_id: spec.mob_id.clone(),
            run_id: Some(run_id.to_string()),
            flow_id: Some(flow_id.to_string()),
            step_id: None,
            meerkat_id: None,
            kind: MobEventKind::SupervisorEscalation,
            payload: json!({
                "reason": reason,
                "details": details.clone(),
            }),
        })
        .await?;

        let Some(notify_role) = spec.supervisor.notify_role.as_deref() else {
            return Ok(());
        };

        let targets = self
            .resolve_step_targets(spec, notify_role, "*", Value::Null)
            .await?;
        if targets.is_empty() {
            return Ok(());
        }

        let supervisor = self.supervisor_runtime(&spec.mob_id).await?;
        let params = json!({
            "mob_id": spec.mob_id.clone(),
            "run_id": run_id,
            "flow_id": flow_id,
            "reason": reason,
            "details": details,
        });

        for target in targets {
            let command = CommsCommand::PeerRequest {
                to: meerkat::PeerName::new(target.comms_name.clone())
                    .map_err(MobError::Comms)?,
                intent: "mob.supervisor.escalation".to_string(),
                params: params.clone(),
                stream: InputStreamMode::None,
            };
            let _ = supervisor
                .send(command)
                .await
                .map_err(|err| MobError::Comms(err.to_string()))?;
        }

        Ok(())
    }

    async fn current_reset_epoch(&self, mob_id: &str) -> u64 {
        let epochs = self.reset_epochs.read().await;
        epochs.get(mob_id).copied().unwrap_or(0)
    }

    async fn bump_reset_epoch(&self, mob_id: &str) -> u64 {
        let mut epochs = self.reset_epochs.write().await;
        let next = epochs.get(mob_id).copied().unwrap_or(0) + 1;
        epochs.insert(mob_id.to_string(), next);
        next
    }

    async fn is_run_canceled(&self, run_id: &str) -> MobResult<bool> {
        let Some(run) = self.run_store.get_run(run_id).await? else {
            return Ok(false);
        };
        Ok(run.status == MobRunStatus::Canceled)
    }

    async fn ensure_run_active(
        &self,
        run_id: &str,
        mob_id: &str,
        expected_epoch: u64,
    ) -> MobResult<()> {
        let current_epoch = self.current_reset_epoch(mob_id).await;
        if current_epoch != expected_epoch {
            return Err(MobError::ResetBarrier {
                mob_id: mob_id.to_string(),
                expected: expected_epoch,
                current: current_epoch,
            });
        }

        let run = self
            .run_store
            .get_run(run_id)
            .await?
            .ok_or_else(|| MobError::RunNotFound {
                run_id: run_id.to_string(),
            })?;
        if run.status == MobRunStatus::Canceled {
            return Err(MobError::RunCanceled {
                run_id: run_id.to_string(),
            });
        }
        Ok(())
    }

    async fn force_reset_mob(&self, mob_id: &str) -> MobResult<()> {
        let epoch = self.bump_reset_epoch(mob_id).await;

        self.emit_event(MobEvent {
            cursor: 0,
            timestamp: Utc::now(),
            category: MobEventCategory::Supervisor,
            mob_id: mob_id.to_string(),
            run_id: None,
            flow_id: None,
            step_id: None,
            meerkat_id: None,
            kind: MobEventKind::ForceResetTransition,
            payload: json!({"phase": "quiesce", "epoch": epoch}),
        })
        .await?;

        self.emit_event(MobEvent {
            cursor: 0,
            timestamp: Utc::now(),
            category: MobEventCategory::Supervisor,
            mob_id: mob_id.to_string(),
            run_id: None,
            flow_id: None,
            step_id: None,
            meerkat_id: None,
            kind: MobEventKind::ForceResetTransition,
            payload: json!({"phase": "drain_cancel", "epoch": epoch}),
        })
        .await?;

        let runs = self
            .run_store
            .list_runs(MobRunFilter {
                mob_id: Some(mob_id.to_string()),
                limit: Some(10_000),
                ..MobRunFilter::default()
            })
            .await?;
        for run in runs {
            if matches!(run.status, MobRunStatus::Pending | MobRunStatus::Running) {
                let _ = self.cancel_run(&run.run_id).await;
            }
        }

        self.emit_event(MobEvent {
            cursor: 0,
            timestamp: Utc::now(),
            category: MobEventCategory::Supervisor,
            mob_id: mob_id.to_string(),
            run_id: None,
            flow_id: None,
            step_id: None,
            meerkat_id: None,
            kind: MobEventKind::ForceResetTransition,
            payload: json!({"phase": "archive", "epoch": epoch}),
        })
        .await?;

        let keys: Vec<String> = {
            let managed = self.managed_meerkats.read().await;
            managed
                .get(mob_id)
                .map(|items| items.keys().cloned().collect())
                .unwrap_or_default()
        };
        for key in keys {
            let _ = self.retire_meerkat(mob_id, &key).await;
        }

        let reconcile = self
            .reconcile(MobReconcileRequest {
                mob_id: mob_id.to_string(),
                mode: crate::model::ReconcileMode::Apply,
            })
            .await?;

        self.emit_event(MobEvent {
            cursor: 0,
            timestamp: Utc::now(),
            category: MobEventCategory::Supervisor,
            mob_id: mob_id.to_string(),
            run_id: None,
            flow_id: None,
            step_id: None,
            meerkat_id: None,
            kind: MobEventKind::ForceResetTransition,
            payload: json!({
                "phase": "restart",
                "epoch": epoch,
                "spawned": reconcile.spawned,
                "retired": reconcile.retired,
            }),
        })
        .await?;

        Ok(())
    }

    async fn execute_flow(
        &self,
        spec: &MobSpec,
        flow_id: &str,
        flow: &FlowSpec,
        run_id: &str,
        activation_payload: Value,
        expected_epoch: u64,
    ) -> MobResult<bool> {
        let step_by_id: IndexMap<String, FlowStepSpec> = flow
            .steps
            .iter()
            .map(|step| (step.step_id.clone(), step.clone()))
            .collect();

        let declaration_order: Vec<String> =
            flow.steps.iter().map(|step| step.step_id.clone()).collect();
        let mut completed = HashSet::<String>::new();
        let mut queued = HashSet::<String>::new();
        let mut ready = VecDeque::<String>::new();
        let mut outputs = HashMap::<String, Value>::new();
        let mut has_failures = false;
        let mut running = FuturesUnordered::new();

        for step in &flow.steps {
            if step.depends_on.is_empty() {
                ready.push_back(step.step_id.clone());
                queued.insert(step.step_id.clone());
            }
        }

        while completed.len() < flow.steps.len() {
            self.ensure_run_active(run_id, &spec.mob_id, expected_epoch)
                .await?;

            while running.len() < spec.limits.max_concurrent_ready_steps && !ready.is_empty() {
                if let Some(step_id) = ready.pop_front() {
                    let step = step_by_id
                        .get(&step_id)
                        .ok_or_else(|| MobError::Internal(format!("missing step '{step_id}'")))?
                        .clone();
                    let mut dependency_outputs = HashMap::new();
                    for dep in &step.depends_on {
                        if let Some(value) = outputs.get(dep) {
                            dependency_outputs.insert(dep.clone(), value.clone());
                        }
                    }
                    let activation_payload = activation_payload.clone();
                    let step_id_for_task = step_id.clone();
                    running.push(async move {
                        let result = self
                            .execute_step(
                                spec,
                                flow_id,
                                run_id,
                                step,
                                dependency_outputs,
                                activation_payload,
                                expected_epoch,
                            )
                            .await;
                        (step_id_for_task, result)
                    });
                }
            }

            let Some((step_id, step_result)) = running.next().await else {
                break;
            };
            let step_result = step_result?;

            outputs.insert(step_id.clone(), step_result.output.clone());
            self.run_store
                .put_step_output(run_id, &step_id, step_result.output)
                .await?;

            let status = step_result.status;
            if matches!(status, StepRunStatus::Failed) {
                has_failures = true;
            }

            let mut run = self
                .run_store
                .get_run(run_id)
                .await?
                .ok_or_else(|| MobError::RunNotFound {
                    run_id: run_id.to_string(),
                })?;
            run.step_statuses.insert(step_id.clone(), status);
            self.run_store.put_run(run).await?;

            completed.insert(step_id);

            for step_id in &declaration_order {
                if completed.contains(step_id) || queued.contains(step_id) {
                    continue;
                }

                let Some(step) = step_by_id.get(step_id) else {
                    continue;
                };

                if step.depends_on.iter().all(|dep| completed.contains(dep)) {
                    ready.push_back(step_id.clone());
                    queued.insert(step_id.clone());
                }
            }
        }

        if completed.len() != flow.steps.len() {
            return Err(MobError::Internal(
                "flow execution ended with incomplete steps".to_string(),
            ));
        }

        Ok(has_failures)
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute_step(
        &self,
        spec: &MobSpec,
        flow_id: &str,
        run_id: &str,
        step: FlowStepSpec,
        dependency_outputs: HashMap<String, Value>,
        activation_payload: Value,
        expected_epoch: u64,
    ) -> MobResult<StepExecutionResult> {
        if let Some(condition) = &step.condition {
            let condition_true =
                evaluate_condition(condition, &dependency_outputs, &activation_payload);
            if !condition_true {
                self.emit_event(MobEvent {
                    cursor: 0,
                    timestamp: Utc::now(),
                    category: MobEventCategory::Flow,
                    mob_id: spec.mob_id.clone(),
                    run_id: Some(run_id.to_string()),
                    flow_id: Some(flow_id.to_string()),
                    step_id: Some(step.step_id.clone()),
                    meerkat_id: None,
                    kind: MobEventKind::StepPartial,
                    payload: json!({"skipped": true}),
                })
                .await?;

                return Ok(StepExecutionResult {
                    status: StepRunStatus::Skipped,
                    output: json!({"skipped": true}),
                });
            }
        }

        self.ensure_run_active(run_id, &spec.mob_id, expected_epoch)
            .await?;

        let mut targets = self
            .resolve_step_targets(
                spec,
                &step.targets.role,
                &step.targets.meerkat_id,
                activation_payload.clone(),
            )
            .await?;
        targets.sort_by(|a, b| a.comms_name.cmp(&b.comms_name));

        if matches!(step.dispatch_mode, crate::model::DispatchMode::OneToOne) {
            targets = targets.into_iter().take(1).collect();
        }

        if targets.is_empty() {
            return Ok(StepExecutionResult {
                status: StepRunStatus::Failed,
                output: json!({"error": "no targets resolved"}),
            });
        }

        let supervisor = self
            .supervisor_runtime(&spec.mob_id)
            .await?;

        let from_role = "supervisor";
        let mut dispatch_futures = FuturesUnordered::new();
        let mut dispatched_targets = 0usize;

        for target in targets {
            let intent = step.intent.as_deref().unwrap_or("delegate").to_string();
            let policy = &spec.topology.flow_dispatched;
            let allowed = evaluate_topology(
                policy,
                from_role,
                &target.role,
                "peer_request",
                intent.as_str(),
            );

            if !allowed {
                let violation = json!({
                    "from_role": from_role,
                    "to_role": target.role,
                    "intent": intent,
                    "target": target.comms_name,
                });
                self.emit_event(MobEvent {
                    cursor: 0,
                    timestamp: Utc::now(),
                    category: MobEventCategory::Topology,
                    mob_id: spec.mob_id.clone(),
                    run_id: Some(run_id.to_string()),
                    flow_id: Some(flow_id.to_string()),
                    step_id: Some(step.step_id.clone()),
                    meerkat_id: Some(target.meerkat_id.clone()),
                    kind: MobEventKind::TopologyViolation,
                    payload: violation,
                })
                .await?;

                if matches!(policy.mode, PolicyMode::Strict) {
                    self.run_store
                        .append_failure_entry(
                            run_id,
                            FailureLedgerEntry {
                                timestamp: Utc::now(),
                                step_id: Some(step.step_id.clone()),
                                target_meerkat: Some(target.comms_name.clone()),
                                error: "blocked by strict flow_dispatched topology policy"
                                    .to_string(),
                            },
                        )
                        .await?;
                    continue;
                }
            }

            dispatched_targets += 1;
            let target_meerkat_id = target.meerkat_id.clone();
            let target_comms_name = target.comms_name.clone();
            let step_for_dispatch = step.clone();
            let supervisor_for_dispatch = supervisor.clone();
            let payload = json!({
                "mob_id": spec.mob_id,
                "run_id": run_id,
                "flow_id": flow_id,
                "step_id": step.step_id.clone(),
                "attempt": 1,
                "hop": 1,
                "logical_op_key": format!("{}:{}:{}", run_id, step.step_id, target.meerkat_id),
                "payload": activation_payload.clone(),
                "depends_on_outputs": dependency_outputs.clone(),
            });

            dispatch_futures.push(async move {
                let result = self
                    .dispatch_to_target(
                        run_id,
                        flow_id,
                        &step_for_dispatch,
                        supervisor_for_dispatch,
                        target,
                        intent,
                        payload,
                        spec,
                        expected_epoch,
                    )
                    .await;
                (target_meerkat_id, target_comms_name, result)
            });
        }

        if dispatched_targets == 0 {
            return Ok(StepExecutionResult {
                status: StepRunStatus::Failed,
                output: json!({"error": "all targets blocked by topology policy"}),
            });
        }

        let mut success_values = Vec::new();
        let mut failure_values = Vec::new();
        let mut timeout_failures = 0usize;
        let mut non_timeout_failures = 0usize;

        while let Some((meerkat_id, comms_name, item)) = dispatch_futures.next().await {
            match item {
                Ok(value) => success_values.push(value),
                Err(err) => {
                    let timeout = matches!(err, MobError::DispatchTimeout { .. });
                    if timeout {
                        timeout_failures += 1;
                    } else {
                        non_timeout_failures += 1;
                    }
                    failure_values.push(json!({
                        "meerkat_id": meerkat_id,
                        "target": comms_name,
                        "timeout": timeout,
                        "error": err.to_string(),
                    }));
                }
            }

            self.ensure_run_active(run_id, &spec.mob_id, expected_epoch)
                .await?;

            if should_stop_collection(
                &step.collection_policy,
                dispatched_targets,
                success_values.len(),
                timeout_failures,
                non_timeout_failures,
                dispatch_futures.len(),
            ) {
                break;
            }
        }
        drop(dispatch_futures);

        let success_count = success_values.len();
        let failure_count = timeout_failures + non_timeout_failures;
        let status = classify_step_status(
            &step.collection_policy,
            step.on_timeout,
            dispatched_targets,
            success_count,
            timeout_failures,
            non_timeout_failures,
        );

        let output = json!({
            "step_id": step.step_id,
            "status": status,
            "success_count": success_count,
            "failure_count": failure_count,
            "timeout_failure_count": timeout_failures,
            "non_timeout_failure_count": non_timeout_failures,
            "successes": success_values,
            "failures": failure_values,
        });

        match step.schema_policy {
            SchemaPolicy::WarnOnly | SchemaPolicy::RetryThenWarn => {
                if let Some(schema_ref) = &step.expected_schema_ref
                    && let Some(schema) = spec.schemas.get(schema_ref)
                {
                    let mut mismatches = Vec::new();
                    for success in output
                        .get("successes")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default()
                    {
                        let candidate = success
                            .get("result")
                            .cloned()
                            .unwrap_or_else(|| success.clone());
                        if !jsonschema::is_valid(schema, &candidate) {
                            mismatches.push(json!({
                                "target": success.get("target"),
                                "interaction_id": success.get("interaction_id"),
                            }));
                        }
                    }

                    if !mismatches.is_empty() {
                        self.emit_event(MobEvent {
                            cursor: 0,
                            timestamp: Utc::now(),
                            category: MobEventCategory::Supervisor,
                            mob_id: spec.mob_id.clone(),
                            run_id: Some(run_id.to_string()),
                            flow_id: Some(flow_id.to_string()),
                            step_id: Some(step.step_id.clone()),
                            meerkat_id: None,
                            kind: MobEventKind::Warning,
                            payload: json!({
                                "warning": "mob peer response schema mismatch",
                                "mismatches": mismatches,
                            }),
                        })
                        .await?;
                    }
                }
            }
            SchemaPolicy::RetryThenFail => {
                if let Some(schema_ref) = &step.expected_schema_ref
                    && let Some(schema) = spec.schemas.get(schema_ref)
                {
                    let mut mismatches = Vec::new();
                    for success in output
                        .get("successes")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default()
                    {
                        let candidate = success
                            .get("result")
                            .cloned()
                            .unwrap_or_else(|| success.clone());
                        if !jsonschema::is_valid(schema, &candidate) {
                            mismatches.push(json!({
                                "target": success.get("target"),
                                "interaction_id": success.get("interaction_id"),
                            }));
                        }
                    }

                    if !mismatches.is_empty() {
                        return Err(MobError::SchemaValidation(format!(
                            "step '{}' peer responses failed schema '{}': {}",
                            step.step_id,
                            schema_ref,
                            json!(mismatches)
                        )));
                    }
                }
            }
        }

        Ok(StepExecutionResult { status, output })
    }

    #[allow(clippy::too_many_arguments)]
    async fn dispatch_to_target(
        &self,
        run_id: &str,
        flow_id: &str,
        step: &FlowStepSpec,
        supervisor: Arc<CommsRuntime>,
        target: ManagedMeerkat,
        intent: String,
        payload: Value,
        spec: &MobSpec,
        expected_epoch: u64,
    ) -> MobResult<Value> {
        let timeout_ms = step.timeout_ms.unwrap_or(spec.limits.default_step_timeout_ms);
        let max_attempts = 2u32;
        for attempt in 1..=max_attempts {
            self.ensure_run_active(run_id, &spec.mob_id, expected_epoch)
                .await?;
            self.emit_event(MobEvent {
                cursor: 0,
                timestamp: Utc::now(),
                category: MobEventCategory::Dispatch,
                mob_id: spec.mob_id.clone(),
                run_id: Some(run_id.to_string()),
                flow_id: Some(flow_id.to_string()),
                step_id: Some(step.step_id.clone()),
                meerkat_id: Some(target.meerkat_id.clone()),
                kind: MobEventKind::StepStarted,
                payload: json!({
                    "target": target.comms_name,
                    "intent": intent,
                    "attempt": attempt,
                }),
            })
            .await?;

            let mut attempt_payload = payload.clone();
            if let Some(map) = attempt_payload.as_object_mut() {
                map.insert("attempt".to_string(), json!(attempt));
            }

            let command = CommsCommand::PeerRequest {
                to: meerkat::PeerName::new(target.comms_name.clone())
                    .map_err(MobError::Comms)?,
                intent: intent.clone(),
                params: attempt_payload,
                stream: InputStreamMode::ReserveInteraction,
            };

            let (receipt, mut stream) = match supervisor.send_and_stream(command.clone()).await {
                Ok(result) => result,
                Err(_) => {
                    let receipt = supervisor
                        .send(command)
                        .await
                        .map_err(|err| MobError::Comms(err.to_string()))?;
                    let stream = match &receipt {
                        SendReceipt::PeerRequestSent { interaction_id, .. } => supervisor
                            .stream(meerkat::StreamScope::Interaction(interaction_id.to_owned()))
                            .map_err(|err| MobError::Comms(err.to_string()))?,
                        _ => {
                            return Err(MobError::Comms(
                                "unexpected receipt for peer request".to_string(),
                            ));
                        }
                    };
                    (receipt, stream)
                }
            };

            let interaction_id = match receipt {
                SendReceipt::PeerRequestSent { interaction_id, .. } => interaction_id,
                _ => {
                    return Err(MobError::Comms(
                        "unexpected send receipt for peer request".to_string(),
                    ));
                }
            };

            let target_for_response = target.comms_name.clone();
            let meerkat_id_for_response = target.meerkat_id.clone();
            let wait = async move {
                while let Some(event) = stream.next().await {
                    match event {
                        meerkat::AgentEvent::InteractionComplete {
                            interaction_id: completed_id,
                            result,
                        } if completed_id == interaction_id => {
                            return Ok(json!({
                                "result": result,
                                "interaction_id": interaction_id,
                                "target": target_for_response,
                                "meerkat_id": meerkat_id_for_response,
                            }));
                        }
                        meerkat::AgentEvent::InteractionFailed {
                            interaction_id: failed_id,
                            error,
                        } if failed_id == interaction_id => {
                            return Err(MobError::Comms(error));
                        }
                        _ => {}
                    }
                }
                Err(MobError::Comms(
                    "interaction stream closed before terminal event".to_string(),
                ))
            };

            let result = tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), wait)
                .await
                .map_err(|_| MobError::DispatchTimeout { timeout_ms })?;

            match result {
                Ok(value) => {
                    self.ensure_run_active(run_id, &spec.mob_id, expected_epoch)
                        .await?;
                    let logical_key = format!("{}:{}:{}", run_id, step.step_id, target.meerkat_id);
                    let inserted = self
                        .run_store
                        .append_step_entry_if_absent(
                            run_id,
                            &logical_key,
                            StepLedgerEntry {
                                timestamp: Utc::now(),
                                step_id: step.step_id.clone(),
                                target_meerkat: target.meerkat_id.clone(),
                                logical_key: logical_key.clone(),
                                attempt,
                                status: StepRunStatus::Completed,
                                detail: value.clone(),
                            },
                        )
                        .await?;
                    if !inserted {
                        return Ok(
                            json!({
                                "attempt": attempt,
                                "target": target.comms_name,
                                "meerkat_id": target.meerkat_id,
                                "result": value.get("result").cloned().unwrap_or(Value::Null),
                                "interaction_id": value.get("interaction_id").cloned(),
                                "duplicate_ignored": true,
                            }),
                        );
                    }
                    self.emit_event(MobEvent {
                        cursor: 0,
                        timestamp: Utc::now(),
                        category: MobEventCategory::Dispatch,
                        mob_id: spec.mob_id.clone(),
                        run_id: Some(run_id.to_string()),
                        flow_id: Some(flow_id.to_string()),
                        step_id: Some(step.step_id.clone()),
                        meerkat_id: Some(target.meerkat_id.clone()),
                        kind: MobEventKind::StepCompleted,
                        payload: json!({
                            "attempt": attempt,
                            "response": value.clone(),
                        }),
                    })
                    .await?;
                    let result_payload = value.get("result").cloned().unwrap_or(Value::Null);
                    let interaction_id = value.get("interaction_id").cloned();
                    return Ok(
                        json!({
                            "attempt": attempt,
                            "target": target.comms_name,
                            "meerkat_id": target.meerkat_id,
                            "result": result_payload,
                            "interaction_id": interaction_id,
                        }),
                    );
                }
                Err(err) => {
                    let retryable =
                        matches!(err, MobError::DispatchTimeout { .. } | MobError::Comms(_));
                    if attempt < max_attempts && retryable {
                        tokio::time::sleep(std::time::Duration::from_millis(
                            spec.supervisor.retry_backoff_ms,
                        ))
                        .await;
                        continue;
                    }

                    self.run_store
                        .append_failure_entry(
                            run_id,
                            FailureLedgerEntry {
                                timestamp: Utc::now(),
                                step_id: Some(step.step_id.clone()),
                                target_meerkat: Some(target.comms_name.clone()),
                                error: format!("attempt {attempt}: {err}"),
                            },
                        )
                        .await?;

                    self.emit_event(MobEvent {
                        cursor: 0,
                        timestamp: Utc::now(),
                        category: MobEventCategory::Dispatch,
                        mob_id: spec.mob_id.clone(),
                        run_id: Some(run_id.to_string()),
                        flow_id: Some(flow_id.to_string()),
                        step_id: Some(step.step_id.clone()),
                        meerkat_id: Some(target.meerkat_id.clone()),
                        kind: MobEventKind::StepFailed,
                        payload: json!({"error": err.to_string(), "attempt": attempt}),
                    })
                    .await?;

                    return Err(err);
                }
            }
        }

        Err(MobError::Internal(
            "dispatch attempt loop exited unexpectedly".to_string(),
        ))
    }

    async fn resolve_step_targets(
        &self,
        spec: &MobSpec,
        role: &str,
        meerkat_selector: &str,
        activation_payload: Value,
    ) -> MobResult<Vec<ManagedMeerkat>> {
        let collect = |map: &HashMap<String, ManagedMeerkat>| -> Vec<ManagedMeerkat> {
            map.values()
                .filter(|instance| instance.role == role)
                .filter(|instance| {
                    meerkat_selector == "*" || instance.meerkat_id == meerkat_selector
                })
                .cloned()
                .collect()
        };

        let mut out = {
            let managed = self.managed_meerkats.read().await;
            managed
                .get(&spec.mob_id)
                .map(collect)
                .unwrap_or_default()
        };

        if !out.is_empty() {
            return Ok(out);
        }

        let Some(role_spec) = spec.roles.get(role) else {
            return Ok(Vec::new());
        };
        if matches!(role_spec.spawn_strategy, crate::model::SpawnStrategy::Eager) {
            return Ok(Vec::new());
        }

        let identities = self
            .resolve_role_meerkats(spec, role, role_spec, activation_payload)
            .await?;
        let spawn_lock = self.spawn_lock(&spec.mob_id).await;
        let _spawn_guard = spawn_lock.lock().await;
        for identity in identities {
            if meerkat_selector != "*" && identity.meerkat_id != meerkat_selector {
                continue;
            }
            let key = format!("{}/{}", role, identity.meerkat_id);
            let already_exists = {
                let managed = self.managed_meerkats.read().await;
                managed
                    .get(&spec.mob_id)
                    .and_then(|map| map.get(&key))
                    .is_some()
            };
            if already_exists {
                continue;
            }
            self.spawn_meerkat(spec, role, &identity).await?;
        }

        out = {
            let managed = self.managed_meerkats.read().await;
            managed
                .get(&spec.mob_id)
                .map(collect)
                .unwrap_or_default()
        };
        Ok(out)
    }

    async fn resolve_role_meerkats(
        &self,
        spec: &MobSpec,
        role_name: &str,
        role: &RoleSpec,
        activation_payload: Value,
    ) -> MobResult<Vec<MeerkatIdentity>> {
        match role.cardinality {
            crate::model::CardinalityKind::Singleton => Ok(vec![MeerkatIdentity {
                meerkat_id: "singleton".to_string(),
                role: role_name.to_string(),
                labels: role.labels.clone(),
                attributes: BTreeMap::new(),
            }]),
            crate::model::CardinalityKind::PerKey | crate::model::CardinalityKind::PerMeerkat => {
                let resolver_id = role
                    .resolver
                    .as_ref()
                    .ok_or_else(|| MobError::ResolverNotFound {
                        resolver_id: "<missing>".to_string(),
                    })?;

                let resolver = self.resolver_registry.get(resolver_id).ok_or_else(|| {
                    MobError::ResolverNotFound {
                        resolver_id: resolver_id.clone(),
                    }
                })?;

                let resolver_spec = spec.resolvers.get(resolver_id).cloned();
                let context = ResolverContext {
                    mob_id: spec.mob_id.clone(),
                    role: role_name.to_string(),
                    resolver_id: resolver_id.clone(),
                    resolver_spec,
                    spec: Some(spec.clone()),
                    activation_payload,
                };

                let mut resolved = resolver.list_meerkats(&context).await?;
                for identity in &mut resolved {
                    if identity.role.is_empty() {
                        identity.role = role_name.to_string();
                    }
                    if identity.labels.is_empty() {
                        identity.labels = role.labels.clone();
                    }
                }
                Ok(resolved)
            }
        }
    }

    async fn spawn_meerkat(
        &self,
        spec: &MobSpec,
        role_name: &str,
        identity: &MeerkatIdentity,
    ) -> MobResult<()> {
        let role = spec
            .roles
            .get(role_name)
            .ok_or_else(|| MobError::SpecValidation(format!("unknown role '{role_name}'")))?;

        let namespace = self.namespace_for(spec);
        let comms_name = format!(
            "{}/{}/{}/{}",
            self.realm_id, spec.mob_id, role_name, identity.meerkat_id
        );

        let session = Session::new();
        let session_id = session.id().clone();

        let role_tools = self.resolve_role_tooling(spec, role).await?;

        let mut labels = identity.labels.clone();
        labels.insert("mob_id".to_string(), spec.mob_id.clone());
        labels.insert("role".to_string(), role_name.to_string());
        labels.insert("meerkat_id".to_string(), identity.meerkat_id.clone());

        let peer_meta = labels
            .iter()
            .fold(PeerMeta::default(), |meta, (key, value)| {
                meta.with_label(key.clone(), value.clone())
            })
            .with_description(format!(
                "Mob meerkat '{}' role '{}'",
                identity.meerkat_id, role_name
            ));

        let request = CreateSessionRequest {
            model: role
                .model
                .clone()
                .unwrap_or_else(|| self.default_model.clone()),
            prompt: String::new(),
            system_prompt: Some(role.prompt.clone()),
            max_tokens: None,
            event_tx: None,
            host_mode: true,
            skill_references: None,
            build: Some(SessionBuildOptions {
                comms_name: Some(comms_name.clone()),
                peer_meta: Some(peer_meta),
                resume_session: Some(session),
                external_tools: role_tools.external_tools,
                override_builtins: Some(role_tools.enable_builtins),
                override_shell: Some(role_tools.enable_shell),
                override_subagents: Some(role_tools.enable_subagents),
                override_memory: Some(role_tools.enable_memory),
                preload_skills: Some(
                    role.preload_skills
                        .iter()
                        .map(|value| meerkat_core::skills::SkillId(value.clone()))
                        .collect(),
                ),
                realm_id: Some(namespace),
                ..SessionBuildOptions::default()
            }),
        };

        let session_service = self.session_service.clone();
        tokio::spawn(async move {
            if let Err(err) = session_service.create_session(request).await {
                tracing::error!("failed to start mob meerkat session: {err}");
            }
        });

        self.wait_for_session_comms(&session_id).await?;
        {
            let mut managed = self.managed_meerkats.write().await;
            let mob_map = managed.entry(spec.mob_id.clone()).or_default();
            let key = format!("{role_name}/{}", identity.meerkat_id);
            mob_map.insert(
                key,
                ManagedMeerkat {
                    role: role_name.to_string(),
                    meerkat_id: identity.meerkat_id.clone(),
                    session_id: session_id.clone(),
                    comms_name: comms_name.clone(),
                    labels,
                    last_activity_at: Utc::now(),
                    status: MeerkatInstanceStatus::Running,
                },
            );
        }
        self.refresh_peer_trust(spec).await?;

        self.emit_event(MobEvent {
            cursor: 0,
            timestamp: Utc::now(),
            category: MobEventCategory::Lifecycle,
            mob_id: spec.mob_id.clone(),
            run_id: None,
            flow_id: None,
            step_id: None,
            meerkat_id: Some(identity.meerkat_id.clone()),
            kind: MobEventKind::MeerkatSpawned,
            payload: json!({"role": role_name, "comms_name": comms_name}),
        })
        .await?;

        Ok(())
    }

    async fn wait_for_session_comms(&self, session_id: &SessionId) -> MobResult<()> {
        let mut attempts = 0usize;
        while attempts < 50 {
            if self.session_service.comms_runtime(session_id).await.is_some() {
                return Ok(());
            }
            attempts += 1;
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        Err(MobError::Comms(format!(
            "session '{}' comms runtime unavailable",
            session_id
        )))
    }

    async fn refresh_peer_trust(&self, spec: &MobSpec) -> MobResult<()> {
        let supervisor = self.supervisor_runtime(&spec.mob_id).await?;
        let supervisor_name = self.supervisor_name(spec);
        let supervisor_peer_id = CoreCommsRuntimeTrait::public_key(&*supervisor)
            .ok_or_else(|| MobError::Comms("supervisor public key unavailable".to_string()))?;

        let entries: Vec<ManagedMeerkat> = {
            let managed = self.managed_meerkats.read().await;
            managed
                .get(&spec.mob_id)
                .map(|map| map.values().cloned().collect())
                .unwrap_or_default()
        };

        let mut live = Vec::with_capacity(entries.len());
        for instance in &entries {
            let runtime = self
                .session_service
                .comms_runtime(&instance.session_id)
                .await
                .ok_or_else(|| {
                    MobError::Comms(format!(
                        "missing comms runtime for session {}",
                        instance.session_id
                    ))
                })?;
            let peer_id = CoreCommsRuntimeTrait::public_key(&*runtime)
                .ok_or_else(|| MobError::Comms("peer public key unavailable".to_string()))?;
            live.push((instance.clone(), runtime, peer_id));
        }

        // Supervisor trusts all currently live managed meerkats (convergent replace).
        let mut supervisor_peers = Vec::with_capacity(live.len());
        for (instance, _, peer_id) in &live {
            supervisor_peers.push(
                TrustedPeerSpec::new(
                    instance.comms_name.clone(),
                    peer_id.clone(),
                    format!("inproc://{}", instance.comms_name),
                )
                .map_err(MobError::Comms)?,
            );
        }
        supervisor
            .replace_trusted_peers(supervisor_peers)
            .await
            .map_err(|err| MobError::Comms(err.to_string()))?;

        // Each managed meerkat trusts supervisor and ad_hoc-allowed peers.
        for (source, source_runtime, _) in &live {
            let mut desired = vec![
                TrustedPeerSpec::new(
                    supervisor_name.clone(),
                    supervisor_peer_id.clone(),
                    format!("inproc://{supervisor_name}"),
                )
                .map_err(MobError::Comms)?,
            ];

            for (target, _, target_peer_id) in &live {
                if source.comms_name == target.comms_name {
                    continue;
                }

                if matches!(spec.topology.ad_hoc.mode, PolicyMode::Strict)
                    && !role_pair_allowed(&spec.topology.ad_hoc, &source.role, &target.role)
                {
                    self.emit_event(MobEvent {
                        cursor: 0,
                        timestamp: Utc::now(),
                        category: MobEventCategory::Topology,
                        mob_id: spec.mob_id.clone(),
                        run_id: None,
                        flow_id: None,
                        step_id: None,
                        meerkat_id: Some(source.meerkat_id.clone()),
                        kind: MobEventKind::TopologyViolation,
                        payload: json!({
                            "domain": "ad_hoc",
                            "from_role": source.role.clone(),
                            "to_role": target.role.clone(),
                            "from": source.comms_name.clone(),
                            "to": target.comms_name.clone(),
                            "message": "blocked trusted-peer edge by strict ad_hoc policy",
                        }),
                    })
                    .await?;
                    continue;
                }

                desired.push(
                    TrustedPeerSpec::new(
                        target.comms_name.clone(),
                        target_peer_id.clone(),
                        format!("inproc://{}", target.comms_name),
                    )
                    .map_err(MobError::Comms)?,
                );
            }

            source_runtime
                .replace_trusted_peers(desired)
                .await
                .map_err(|err| MobError::Comms(err.to_string()))?;
        }

        Ok(())
    }

    async fn retire_meerkat(&self, mob_id: &str, key: &str) -> MobResult<()> {
        let instance = {
            let managed = self.managed_meerkats.read().await;
            let Some(map) = managed.get(mob_id) else {
                return Ok(());
            };
            map.get(key).cloned()
        };

        if let Some(instance) = instance {
            self.session_service.archive(&instance.session_id).await?;
            {
                let mut managed = self.managed_meerkats.write().await;
                if let Some(map) = managed.get_mut(mob_id) {
                    map.remove(key);
                }
            }
            if let Some(spec) = self.spec_store.get_spec(mob_id).await? {
                self.refresh_peer_trust(&spec).await?;
            }

            self.emit_event(MobEvent {
                cursor: 0,
                timestamp: Utc::now(),
                category: MobEventCategory::Lifecycle,
                mob_id: mob_id.to_string(),
                run_id: None,
                flow_id: None,
                step_id: None,
                meerkat_id: Some(instance.meerkat_id),
                kind: MobEventKind::MeerkatRetired,
                payload: json!({"key": key}),
            })
            .await?;
        }

        Ok(())
    }

    async fn supervisor_runtime(&self, mob_id: &str) -> MobResult<Arc<CommsRuntime>> {
        if let Some(runtime) = self.supervisors.read().await.get(mob_id).cloned() {
            return Ok(runtime);
        }

        let namespace = self.namespace_for_mob(mob_id);
        let name = self.supervisor_name_from_mob(mob_id);

        let runtime = CommsRuntime::inproc_only_scoped(&name, Some(namespace))
            .map_err(|err| MobError::Comms(err.to_string()))?;
        runtime.set_peer_meta(
            PeerMeta::default()
                .with_description("Mob supervisor runtime")
                .with_label("mob_id", mob_id)
                .with_label("role", "supervisor"),
        );

        let runtime = Arc::new(runtime);
        self.supervisors
            .write()
            .await
            .insert(mob_id.to_string(), runtime.clone());
        Ok(runtime)
    }

    fn namespace_for(&self, spec: &MobSpec) -> String {
        format!("{}/{}", self.realm_id, spec.mob_id)
    }

    fn namespace_for_mob(&self, mob_id: &str) -> String {
        format!("{}/{}", self.realm_id, mob_id)
    }

    fn supervisor_name(&self, spec: &MobSpec) -> String {
        self.supervisor_name_from_mob(&spec.mob_id)
    }

    fn supervisor_name_from_mob(&self, mob_id: &str) -> String {
        format!("{}/{}/supervisor", self.realm_id, mob_id)
    }

    async fn spawn_lock(&self, mob_id: &str) -> Arc<Mutex<()>> {
        if let Some(lock) = self.spawn_locks.read().await.get(mob_id).cloned() {
            return lock;
        }
        let mut locks = self.spawn_locks.write().await;
        locks
            .entry(mob_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    async fn spawn_tracked_run_task<F>(&self, run_id: String, future: F)
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

    async fn resolve_role_tooling(
        &self,
        spec: &MobSpec,
        role: &RoleSpec,
    ) -> MobResult<RoleTooling> {
        let mut enable_builtins = false;
        let mut enable_shell = false;
        let mut enable_subagents = false;
        let mut enable_memory = false;

        let mut external_dispatchers: Vec<Arc<dyn AgentToolDispatcher>> = Vec::new();

        for bundle_id in &role.tool_bundles {
            let bundle = spec.tool_bundles.get(bundle_id).ok_or_else(|| {
                MobError::ToolBundle(format!("unknown tool bundle '{bundle_id}'"))
            })?;

            match bundle {
                crate::model::ToolBundleSpec::Builtin {
                    enable_builtins: b,
                    enable_shell: s,
                    enable_subagents: sa,
                    enable_memory: m,
                } => {
                    enable_builtins |= *b;
                    enable_shell |= *s;
                    enable_subagents |= *sa;
                    enable_memory |= *m;
                }
                crate::model::ToolBundleSpec::Mcp {
                    servers,
                    unavailable,
                } => {
                    match self.build_mcp_dispatcher(servers).await {
                        Ok(dispatcher) => external_dispatchers.push(dispatcher),
                        Err(err) => {
                            if matches!(unavailable, UnavailablePolicy::FailActivation) {
                                return Err(err);
                            }
                            self.emit_event(MobEvent {
                                cursor: 0,
                                timestamp: Utc::now(),
                                category: MobEventCategory::Supervisor,
                                mob_id: spec.mob_id.clone(),
                                run_id: None,
                                flow_id: None,
                                step_id: None,
                                meerkat_id: None,
                                kind: MobEventKind::Warning,
                                payload: json!({"warning": err.to_string()}),
                            })
                            .await?;
                        }
                    }
                }
                crate::model::ToolBundleSpec::RustBundle {
                    bundle_id,
                    unavailable,
                } => match self.rust_bundles.get(bundle_id).await {
                    Some(dispatcher) => external_dispatchers.push(dispatcher),
                    None => {
                        if matches!(unavailable, UnavailablePolicy::FailActivation) {
                            return Err(MobError::ToolBundle(format!(
                                "missing rust tool bundle '{bundle_id}'"
                            )));
                        }
                        self.emit_event(MobEvent {
                            cursor: 0,
                            timestamp: Utc::now(),
                            category: MobEventCategory::Supervisor,
                            mob_id: spec.mob_id.clone(),
                            run_id: None,
                            flow_id: None,
                            step_id: None,
                            meerkat_id: None,
                            kind: MobEventKind::Warning,
                            payload: json!({"warning": format!("missing rust tool bundle '{bundle_id}'")}),
                        })
                        .await?;
                    }
                },
            }
        }

        let external_tools = if external_dispatchers.is_empty() {
            None
        } else if external_dispatchers.len() == 1 {
            external_dispatchers.into_iter().next()
        } else {
            let mut builder = ToolGatewayBuilder::new();
            for dispatcher in external_dispatchers {
                builder = builder.add_dispatcher(dispatcher);
            }
            let gateway = builder
                .build()
                .map_err(|err| MobError::ToolBundle(err.to_string()))?;
            Some(Arc::new(gateway) as Arc<dyn AgentToolDispatcher>)
        };

        Ok(RoleTooling {
            enable_builtins,
            enable_shell,
            enable_subagents,
            enable_memory,
            external_tools,
        })
    }

    async fn build_mcp_dispatcher(
        &self,
        server_names: &[String],
    ) -> MobResult<Arc<dyn AgentToolDispatcher>> {
        let servers = meerkat_core::McpConfig::load_with_scopes_from_roots(
            self.context_root.as_deref(),
            self.user_config_root.as_deref(),
        )
        .await
        .map_err(|err| MobError::ToolBundle(err.to_string()))?;

        let mut server_by_name = HashMap::new();
        for item in servers {
            server_by_name.insert(item.server.name.clone(), item.server);
        }

        let mut router = meerkat::McpRouter::new();
        for name in server_names {
            let server = server_by_name.get(name).cloned().ok_or_else(|| {
                MobError::ToolBundle(format!("MCP server '{name}' not found in realm config"))
            })?;
            router
                .add_server(server)
                .await
                .map_err(|err| MobError::ToolBundle(err.to_string()))?;
        }

        Ok(Arc::new(router))
    }
}

impl MobRuntime {
}

#[derive(Debug)]
struct StepExecutionResult {
    status: StepRunStatus,
    output: Value,
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
    use crate::model::{ConditionExpr, TopologyDomainSpec, TopologyRule};
    use crate::store::{InMemoryMobEventStore, InMemoryMobRunStore, InMemoryMobSpecStore};
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
        let outputs = HashMap::from([(
            "dispatch".to_string(),
            json!({"count": 3, "status": "partial"}),
        )]);

        assert!(evaluate_condition(
            &ConditionExpr::Eq {
                left: "step.dispatch.status".to_string(),
                right: json!("partial"),
            },
            &outputs,
            &json!({"priority": "high"}),
        ));

        assert!(evaluate_condition(
            &ConditionExpr::Gt {
                left: "step.dispatch.count".to_string(),
                right: json!(2),
            },
            &outputs,
            &json!({}),
        ));
    }

    #[tokio::test]
    async fn builder_creates_runtime() {
        let runtime = MobRuntimeBuilder::new(
            "realm-a",
            Arc::new(MockSessionService),
            AgentFactory::new("/tmp"),
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
            AgentFactory::new("/tmp"),
            Arc::new(InMemoryMobSpecStore::default()),
            Arc::new(InMemoryMobRunStore::default()),
            Arc::new(InMemoryMobEventStore::default()),
        )
        .runtime_root(PathBuf::from("/tmp"))
        .build()
        .unwrap();

        runtime
            .spawn_tracked_run_task("run-test".to_string(), async {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            })
            .await;
        assert_eq!(runtime.active_run_task_count().await, 1);

        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        assert_eq!(runtime.active_run_task_count().await, 0);
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

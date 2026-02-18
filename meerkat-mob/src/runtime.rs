//! MobRuntime — the core orchestration engine.
//!
//! [`MobRuntime`] implements [`MobService`] by wiring together:
//! - A comms supervisor identity for dispatch/collection
//! - A DAG scheduler for step ordering and concurrency
//! - Comms-native PeerRequest/PeerResponse for dispatch and collection
//! - A run state machine with CAS transitions
//!
//! The runtime does NOT create real agent sessions (no AgentFactory dependency).
//! Instead, it dispatches work via PeerRequest to meerkats that are expected
//! to be registered in the InprocRegistry under the mob's namespace.

use async_trait::async_trait;
use chrono::Utc;
use meerkat_comms::CommsRuntime;
use meerkat_comms::InprocRegistry;
use meerkat_core::PeerMeta;
use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
use meerkat_core::comms::{
    CommsCommand, InputStreamMode, PeerName, SendReceipt, TrustedPeerSpec,
};
use meerkat_core::interaction::{InteractionContent, ResponseStatus};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};
use uuid::Uuid;

use crate::error::MobError;
use crate::event::{MobEvent, MobEventKind, RetentionCategory};
use crate::resolver::{MeerkatIdentity, MeerkatResolver, ResolverContext};
use crate::run::{
    FailureLedgerEntry, MobRun, RunStatus, StepEntryStatus, StepLedgerEntry,
};
use crate::service::{
    ActivateRequest, ActivateResult, ApplySpecRequest, ListMeerkatsRequest, ListRunsRequest,
    ListSpecsRequest, MobService, PollEventsRequest, ReconcileMode, ReconcileRequest,
    ReconcileResult,
};
use crate::spec::{
    CardinalitySpec, CollectionMode, FlowSpec, FlowStepSpec, MobSpec, TimeoutBehavior,
};
use crate::store::{MobEventStore, MobRunStore, MobSpecStore};
use crate::topology::{TopologyDecision, TopologyDomain, TopologyEvaluator};
use crate::validate::{ValidateOptions, validate_spec};

// ---------------------------------------------------------------------------
// MobRuntime configuration
// ---------------------------------------------------------------------------

/// Configuration for a [`MobRuntime`].
#[derive(Debug, Clone)]
pub struct MobRuntimeConfig {
    /// Realm identifier for namespace isolation.
    pub realm_id: String,
    /// Mob identifier.
    pub mob_id: String,
    /// Validation options for spec apply.
    pub validate_opts: ValidateOptions,
}

// ---------------------------------------------------------------------------
// DAG Scheduler
// ---------------------------------------------------------------------------

/// Determines which steps in a flow are ready to execute, given a set of
/// already-completed step IDs.
pub struct DagScheduler<'a> {
    flow: &'a FlowSpec,
}

impl<'a> DagScheduler<'a> {
    /// Create a new scheduler for the given flow.
    pub fn new(flow: &'a FlowSpec) -> Self {
        Self { flow }
    }

    /// Return step IDs whose dependencies are all satisfied.
    ///
    /// Steps are returned in declaration order (FIFO when overflow occurs).
    /// Steps that are already completed or in the `in_progress` set are excluded.
    pub fn ready_steps(
        &self,
        completed: &HashSet<String>,
        in_progress: &HashSet<String>,
    ) -> Vec<&'a str> {
        self.flow
            .steps
            .iter()
            .filter(|step| {
                // Not already done or running
                !completed.contains(&step.step_id) && !in_progress.contains(&step.step_id)
            })
            .filter(|step| {
                // All dependencies satisfied
                step.depends_on
                    .iter()
                    .all(|dep| completed.contains(dep))
            })
            .map(|step| step.step_id.as_str())
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Role compiler (CHOKE-MOB-002)
// ---------------------------------------------------------------------------

/// Compiled build configuration derived from a [`RoleSpec`].
///
/// This is the mob-level analog of `AgentBuildConfig` — it captures the
/// fields that would be passed to `AgentFactory::build_agent` when spawning
/// real meerkat sessions.
#[derive(Debug, Clone)]
pub struct CompiledRoleConfig {
    /// The model to use (from role spec or realm default).
    pub model: Option<String>,
    /// The resolved system prompt.
    pub system_prompt: Option<String>,
    /// Whether to run in host mode.
    pub host_mode: bool,
    /// Computed comms name.
    pub comms_name: String,
    /// Peer metadata labels.
    pub peer_meta: PeerMeta,
    /// Preloaded skills.
    pub preload_skills: Vec<String>,
    /// Tool configuration compiled from tool bundles.
    pub tools: CompiledToolConfig,
}

/// Tool configuration compiled from role spec tool bundles and policies.
#[derive(Debug, Clone, Default)]
pub struct CompiledToolConfig {
    /// Enable built-in tools.
    pub enable_builtins: bool,
    /// Enable shell access.
    pub enable_shell: bool,
    /// Enable sub-agent spawning.
    pub enable_subagents: bool,
    /// Enable semantic memory.
    pub enable_memory: bool,
    /// Enable comms tools.
    pub enable_comms: bool,
    /// MCP server names to connect.
    pub mcp_servers: Vec<String>,
    /// Rust bundle IDs to load.
    pub rust_bundles: Vec<String>,
    /// Tool allow list (empty = all allowed).
    pub tool_allow: Vec<String>,
    /// Tool deny list (applied after allow).
    pub tool_deny: Vec<String>,
    /// Degradation warnings for unavailable bundles.
    pub degradation_warnings: Vec<String>,
}

/// Compile a role spec into a build configuration for a specific meerkat instance.
///
/// When `role.model` is `None`, `default_model` is used as fallback.
pub fn compile_role_config(
    role: &crate::spec::RoleSpec,
    spec: &MobSpec,
    realm_id: &str,
    meerkat_id: &str,
    default_model: &str,
) -> CompiledRoleConfig {
    use crate::spec::{OnUnavailable, ToolBundleKind};

    // Resolve prompt: inline takes precedence, then ref
    let system_prompt = role
        .prompt_inline
        .clone()
        .or_else(|| {
            role.prompt_ref.as_ref().and_then(|pr| {
                crate::validate::resolve_prompt_ref(pr, &spec.prompts, None)
            })
        });

    let comms_name = format!("{}/{}/{}/{}", realm_id, spec.mob_id, role.role, meerkat_id);

    let peer_meta = PeerMeta::default()
        .with_label("mob_id", &spec.mob_id)
        .with_label("role", &role.role)
        .with_label("meerkat_id", meerkat_id);

    // Compile tool configuration from bundles
    // Comms is always enabled for mob meerkats
    let mut tools = CompiledToolConfig {
        enable_comms: true,
        ..CompiledToolConfig::default()
    };

    for bundle_name in &role.tool_bundles {
        if let Some(bundle) = spec.tool_bundles.get(bundle_name) {
            match &bundle.kind {
                ToolBundleKind::Builtin => {
                    tools.enable_builtins = true;
                    tools.enable_shell = true;
                    tools.enable_subagents = true;
                    tools.enable_memory = true;
                }
                ToolBundleKind::Mcp { server } => {
                    tools.mcp_servers.push(server.clone());
                }
                ToolBundleKind::RustBundle { bundle_id } => {
                    tools.rust_bundles.push(bundle_id.clone());
                }
            }
        } else {
            // Bundle not found in spec
            match spec
                .tool_bundles
                .get(bundle_name)
                .map(|b| b.on_unavailable)
                .unwrap_or(OnUnavailable::FailActivation)
            {
                OnUnavailable::FailActivation => {
                    // This should have been caught during validation,
                    // but record a degradation warning if it slips through
                    tools.degradation_warnings.push(format!(
                        "tool bundle '{}' not found (fail_activation)",
                        bundle_name
                    ));
                }
                OnUnavailable::DegradeWarn => {
                    tools.degradation_warnings.push(format!(
                        "tool bundle '{}' unavailable, degrading gracefully",
                        bundle_name
                    ));
                }
            }
        }
    }

    // Apply tool policy allow/deny filters
    if let Some(ref policy) = role.tool_policy {
        tools.tool_allow.clone_from(&policy.allow);
        tools.tool_deny.clone_from(&policy.deny);
    }

    CompiledRoleConfig {
        model: Some(role.model.clone().unwrap_or_else(|| default_model.to_string())),
        system_prompt,
        host_mode: true,
        comms_name,
        peer_meta,
        preload_skills: role.preload_skills.clone(),
        tools,
    }
}

// ---------------------------------------------------------------------------
// Condition evaluation (REQ-MOB-038)
// ---------------------------------------------------------------------------

/// Evaluate a condition against a JSON context.
///
/// The context is a JSON object with keys like `activation.*` and
/// `step.<id>.*`. Returns `true` if the condition is satisfied.
pub fn evaluate_condition(condition: &crate::spec::Condition, context: &serde_json::Value) -> bool {
    use crate::spec::Condition;

    match condition {
        Condition::All(subs) => subs.iter().all(|c| evaluate_condition(c, context)),
        Condition::Any(subs) => subs.iter().any(|c| evaluate_condition(c, context)),
        Condition::Not(sub) => !evaluate_condition(sub, context),
        Condition::Eq { path, value } => resolve_path(context, path)
            .is_some_and(|v| v == value),
        Condition::InSet { path, values } => resolve_path(context, path)
            .is_some_and(|v| values.contains(v)),
        Condition::Gt { path, value } => resolve_path(context, path)
            .is_some_and(|v| {
                match (v.as_f64(), value.as_f64()) {
                    (Some(a), Some(b)) => a > b,
                    _ => false,
                }
            }),
    }
}

/// Resolve a dot-separated path in a JSON value.
fn resolve_path<'a>(value: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let mut current = value;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}

/// Build the context object for condition evaluation.
///
/// Contains `activation.*` from activation params and `step.<id>.*` from
/// completed step outputs.
fn build_condition_context(
    activation_params: &serde_json::Value,
    step_outputs: &BTreeMap<String, serde_json::Value>,
) -> serde_json::Value {
    let mut ctx = serde_json::Map::new();
    ctx.insert("activation".to_string(), activation_params.clone());

    let mut steps = serde_json::Map::new();
    for (step_id, output) in step_outputs {
        let mut step_info = serde_json::Map::new();
        step_info.insert("status".to_string(), serde_json::json!("completed"));
        step_info.insert("output".to_string(), output.clone());
        steps.insert(step_id.clone(), serde_json::Value::Object(step_info));
    }
    ctx.insert("step".to_string(), serde_json::Value::Object(steps));

    serde_json::Value::Object(ctx)
}

// ---------------------------------------------------------------------------
// Dispatch result tracking
// ---------------------------------------------------------------------------

/// Tracks a single dispatched request awaiting a response.
#[derive(Debug, Clone)]
struct DispatchedRequest {
    step_id: String,
    target_meerkat_id: String,
    envelope_id: Uuid,
    dispatched_at: chrono::DateTime<Utc>,
}

/// Collected response from a dispatched request.
#[derive(Debug, Clone)]
struct CollectedResponse {
    target_meerkat_id: String,
    status: ResponseStatus,
    result: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Response router
// ---------------------------------------------------------------------------

/// Routes incoming PeerResponses to the correct step collector.
///
/// The response router runs as a background task that drains the supervisor's
/// inbox and dispatches responses to step-specific channels based on
/// `in_reply_to` matching dispatched `envelope_id`s.
struct ResponseRouter {
    /// Map from dispatched envelope_id to step_id + target_meerkat_id.
    dispatch_map: Arc<Mutex<HashMap<Uuid, DispatchedRequest>>>,
    /// Map from step_id to response sender channel.
    step_channels: Arc<Mutex<HashMap<String, tokio::sync::mpsc::Sender<CollectedResponse>>>>,
}

impl ResponseRouter {
    fn new() -> Self {
        Self {
            dispatch_map: Arc::new(Mutex::new(HashMap::new())),
            step_channels: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Register a dispatched request for response routing.
    async fn register_dispatch(&self, req: DispatchedRequest) {
        let mut map = self.dispatch_map.lock().await;
        map.insert(req.envelope_id, req);
    }

    /// Register a step's response channel.
    async fn register_step_channel(
        &self,
        step_id: String,
        tx: tokio::sync::mpsc::Sender<CollectedResponse>,
    ) {
        let mut channels = self.step_channels.lock().await;
        channels.insert(step_id, tx);
    }

    /// Remove a step's channel (when step is done).
    async fn remove_step_channel(&self, step_id: &str) {
        let mut channels = self.step_channels.lock().await;
        channels.remove(step_id);
    }

    /// Route a response to the correct step channel.
    async fn route_response(&self, in_reply_to: Uuid, status: ResponseStatus, result: serde_json::Value) {
        let map = self.dispatch_map.lock().await;
        if let Some(req) = map.get(&in_reply_to) {
            let channels = self.step_channels.lock().await;
            if let Some(tx) = channels.get(&req.step_id) {
                let _ = tx.send(CollectedResponse {
                    target_meerkat_id: req.target_meerkat_id.clone(),
                    status,
                    result,
                }).await;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// MobRuntime
// ---------------------------------------------------------------------------

/// The mob orchestration runtime.
///
/// Holds references to stores and a supervisor comms identity. Implements
/// the full [`MobService`] trait for spec management, flow activation,
/// run lifecycle, and event polling.
pub struct MobRuntime {
    config: MobRuntimeConfig,
    spec_store: Arc<dyn MobSpecStore>,
    run_store: Arc<dyn MobRunStore>,
    event_store: Arc<dyn MobEventStore>,
    /// The supervisor's comms identity in namespace `{realm_id}/{mob_id}`.
    supervisor: Arc<CommsRuntime>,
    /// Optional resolver registry for reconcile operations.
    resolvers: Arc<Mutex<HashMap<String, Arc<dyn MeerkatResolver>>>>,
}

impl MobRuntime {
    /// Create a new MobRuntime.
    ///
    /// Creates a CommsRuntime as the mob supervisor identity in namespace
    /// `{realm_id}/{mob_id}`.
    pub fn new(
        config: MobRuntimeConfig,
        spec_store: Arc<dyn MobSpecStore>,
        run_store: Arc<dyn MobRunStore>,
        event_store: Arc<dyn MobEventStore>,
    ) -> Result<Self, MobError> {
        let namespace = format!("{}/{}", config.realm_id, config.mob_id);
        let supervisor_name = format!("supervisor-{}", Uuid::new_v4().simple());
        let supervisor =
            CommsRuntime::inproc_only_scoped(&supervisor_name, Some(namespace))
                .map_err(|e| MobError::NamespaceError(e.to_string()))?;

        // Set peer meta for the supervisor
        supervisor.set_peer_meta(
            PeerMeta::default()
                .with_label("mob_id", &config.mob_id)
                .with_label("role", "supervisor"),
        );

        Ok(Self {
            config,
            spec_store,
            run_store,
            event_store,
            supervisor: Arc::new(supervisor),
            resolvers: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Access the supervisor's CommsRuntime (for trust setup in tests).
    pub fn supervisor(&self) -> &CommsRuntime {
        &self.supervisor
    }

    /// Register a resolver for reconcile operations.
    pub async fn register_resolver(
        &self,
        name: String,
        resolver: Arc<dyn MeerkatResolver>,
    ) {
        let mut resolvers = self.resolvers.lock().await;
        resolvers.insert(name, resolver);
    }

    /// Namespace for this mob's inproc registry.
    fn namespace(&self) -> String {
        format!("{}/{}", self.config.realm_id, self.config.mob_id)
    }

    /// Resolve target meerkat IDs for a step's target selector.
    fn resolve_targets(&self, _spec: &MobSpec, step: &FlowStepSpec) -> Vec<String> {
        let role = &step.targets.role;
        match &step.targets.meerkat_id {
            // Specific meerkat
            Some(id) if id != "*" => vec![id.clone()],
            // All meerkats in role (fan_out to all registered in namespace)
            Some(_) | None => {
                let namespace = self.namespace();
                let peers = InprocRegistry::global().peers_in_namespace(&namespace);
                peers
                    .iter()
                    .filter(|p| {
                        p.meta.labels.get("role").map(|r| r.as_str()) == Some(role)
                    })
                    .map(|p| {
                        p.meta
                            .labels
                            .get("meerkat_id")
                            .cloned()
                            .unwrap_or_else(|| p.name.clone())
                    })
                    .collect()
            }
        }
    }

    /// Dispatch a step to its targets via PeerRequest.
    async fn dispatch_step(
        &self,
        step: &FlowStepSpec,
        targets: &[String],
        run_id: &str,
        flow_id: &str,
        payload: &serde_json::Value,
        attempt: u32,
    ) -> Result<Vec<DispatchedRequest>, MobError> {
        let mut dispatched = Vec::new();

        for target_id in targets {
            // Find the peer name for this target in the namespace
            let namespace = self.namespace();
            let peers = InprocRegistry::global().peers_in_namespace(&namespace);
            let peer = peers.iter().find(|p| {
                p.meta.labels.get("meerkat_id").map(|id| id.as_str()) == Some(target_id)
            });

            let peer_name_str = match peer {
                Some(p) => p.name.clone(),
                None => {
                    return Err(MobError::DispatchFailed {
                        step_id: step.step_id.clone(),
                        reason: format!("target meerkat not found: {target_id}"),
                    });
                }
            };

            // Establish trust with this peer
            let peer_info = peers.iter().find(|p| p.name == peer_name_str);
            if let Some(info) = peer_info {
                let spec = TrustedPeerSpec::new(
                    &peer_name_str,
                    info.pubkey.to_peer_id(),
                    format!("inproc://{peer_name_str}"),
                )
                .map_err(|e| MobError::DispatchFailed {
                    step_id: step.step_id.clone(),
                    reason: e,
                })?;
                CoreCommsRuntime::add_trusted_peer(&*self.supervisor, spec)
                    .await
                    .map_err(MobError::CommsError)?;
            }

            let params = serde_json::json!({
                "mob_id": self.config.mob_id,
                "run_id": run_id,
                "flow_id": flow_id,
                "step_id": step.step_id,
                "attempt": attempt,
                "hop": 1,
                "logical_op_key": format!("{}/{}/{}", run_id, step.step_id, target_id),
                "payload": payload,
            });

            let cmd = CommsCommand::PeerRequest {
                to: PeerName::new(peer_name_str)
                    .map_err(|e| MobError::DispatchFailed {
                        step_id: step.step_id.clone(),
                        reason: e,
                    })?,
                intent: "delegate".to_string(),
                params,
                stream: InputStreamMode::None,
            };

            let receipt = CoreCommsRuntime::send(&*self.supervisor, cmd)
                .await
                .map_err(MobError::CommsError)?;

            match receipt {
                SendReceipt::PeerRequestSent {
                    envelope_id,
                    interaction_id: _,
                    ..
                } => {
                    dispatched.push(DispatchedRequest {
                        step_id: step.step_id.clone(),
                        target_meerkat_id: target_id.clone(),
                        envelope_id,
                        dispatched_at: Utc::now(),
                    });
                }
                other => {
                    return Err(MobError::DispatchFailed {
                        step_id: step.step_id.clone(),
                        reason: format!("unexpected receipt: {other:?}"),
                    });
                }
            }
        }

        Ok(dispatched)
    }

    /// Execute a complete flow: DAG scheduling, step dispatch, collection.
    ///
    /// Uses a centralized inbox drain loop to avoid losing responses when
    /// multiple steps run concurrently.
    async fn execute_flow(
        self: &Arc<Self>,
        spec: &MobSpec,
        flow: &FlowSpec,
        run_id: &str,
        activation_params: &serde_json::Value,
    ) -> Result<(), MobError> {
        let mut completed: HashSet<String> = HashSet::new();
        let mut step_outputs: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        let max_concurrent = spec.limits.max_concurrent_ready_steps;
        let semaphore = Arc::new(Semaphore::new(max_concurrent));

        // Create the response router
        let router = Arc::new(ResponseRouter::new());

        // Start inbox drain loop
        let drain_supervisor = Arc::clone(&self.supervisor);
        let drain_router = Arc::clone(&router);
        let drain_handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;

                let interactions =
                    CoreCommsRuntime::drain_inbox_interactions(&*drain_supervisor).await;

                for interaction in &interactions {
                    if let InteractionContent::Response {
                        in_reply_to,
                        status,
                        result,
                    } = &interaction.content
                    {
                        drain_router
                            .route_response(in_reply_to.0, *status, result.clone())
                            .await;
                    }
                }
            }
        });

        let result = self
            .execute_flow_inner(spec, flow, run_id, activation_params, &semaphore, &router, &mut completed, &mut step_outputs)
            .await;

        // Stop the drain loop
        drain_handle.abort();

        result
    }

    /// Inner flow execution loop (separated for clarity).
    #[allow(clippy::too_many_arguments)]
    async fn execute_flow_inner(
        self: &Arc<Self>,
        spec: &MobSpec,
        flow: &FlowSpec,
        run_id: &str,
        activation_params: &serde_json::Value,
        semaphore: &Arc<Semaphore>,
        router: &Arc<ResponseRouter>,
        completed: &mut HashSet<String>,
        step_outputs: &mut BTreeMap<String, serde_json::Value>,
    ) -> Result<(), MobError> {
        loop {
            let scheduler = DagScheduler::new(flow);
            let ready = scheduler.ready_steps(completed, &HashSet::new());

            if ready.is_empty() {
                if completed.len() == flow.steps.len() {
                    break;
                }
                return Err(MobError::Internal(
                    "DAG scheduler deadlock: no ready steps but not all complete".to_string(),
                ));
            }

            // Prepare step execution data, evaluating conditions
            let mut step_tasks = Vec::new();
            for step_id in ready {
                let step = flow
                    .steps
                    .iter()
                    .find(|s| s.step_id == step_id)
                    .ok_or_else(|| {
                        MobError::Internal(format!("step not found: {step_id}"))
                    })?
                    .clone();

                // Evaluate condition (REQ-MOB-038)
                if let Some(ref condition) = step.condition {
                    let context = build_condition_context(activation_params, step_outputs);
                    if !evaluate_condition(condition, &context) {
                        // Condition false: skip step
                        self.run_store
                            .append_step_entry(
                                run_id,
                                StepLedgerEntry {
                                    step_id: step.step_id.clone(),
                                    target_meerkat_id: "".to_string(),
                                    status: StepEntryStatus::Skipped,
                                    attempt: 0,
                                    dispatched_at: None,
                                    completed_at: Some(Utc::now()),
                                    result: None,
                                    error: Some("condition evaluated to false".to_string()),
                                },
                            )
                            .await?;
                        completed.insert(step.step_id.clone());
                        continue;
                    }
                }

                // Build upstream payload
                let mut upstream_payloads = BTreeMap::new();
                for dep_id in &step.depends_on {
                    if let Some(output) = step_outputs.get(dep_id) {
                        upstream_payloads.insert(dep_id.clone(), output.clone());
                    }
                }

                step_tasks.push((step, upstream_payloads));
            }

            // If all ready steps were skipped by conditions, loop again
            if step_tasks.is_empty() && completed.len() < flow.steps.len() {
                continue;
            }

            // Run ready steps concurrently
            let mut handles = Vec::new();
            for (step, upstream_payloads) in step_tasks {
                let runtime = Arc::clone(self);
                let spec_clone = spec.clone();
                let run_id = run_id.to_string();
                let flow_id = flow.flow_id.clone();
                let sem = Arc::clone(semaphore);
                let rtr = Arc::clone(router);

                handles.push(tokio::spawn(async move {
                    let _permit = match sem.acquire().await {
                        Ok(p) => p,
                        Err(_) => {
                            return (
                                step.step_id.clone(),
                                Err(MobError::Internal(
                                    "step semaphore closed".to_string(),
                                )),
                            );
                        }
                    };
                    let result = runtime
                        .execute_step_with_router(
                            &step,
                            &spec_clone,
                            &run_id,
                            &flow_id,
                            &upstream_payloads,
                            &rtr,
                        )
                        .await;
                    (step.step_id.clone(), result)
                }));
            }

            // Await all concurrent steps
            for handle in handles {
                let (step_id, result) = handle.await.map_err(|e| {
                    MobError::Internal(format!("step task panicked: {e}"))
                })?;
                match result {
                    Ok(outputs) => {
                        let merged = serde_json::to_value(&outputs)
                            .unwrap_or(serde_json::Value::Null);
                        step_outputs.insert(step_id.clone(), merged);
                        completed.insert(step_id);
                    }
                    Err(e) => {
                        return Err(e);
                    }
                }
            }
        }

        Ok(())
    }

    /// Execute a single step using the shared response router.
    async fn execute_step_with_router(
        &self,
        step: &FlowStepSpec,
        spec: &MobSpec,
        run_id: &str,
        flow_id: &str,
        upstream_payloads: &BTreeMap<String, serde_json::Value>,
        router: &ResponseRouter,
    ) -> Result<BTreeMap<String, serde_json::Value>, MobError> {
        let targets = self.resolve_targets(spec, step);

        // Evaluate topology policy before dispatch
        let evaluator = TopologyEvaluator::new(&spec.topology);
        let mut allowed_targets = Vec::new();
        for target_id in &targets {
            let decision = evaluator.evaluate(
                "supervisor",
                &step.targets.role,
                "delegate",
                TopologyDomain::FlowDispatched,
            );
            match &decision {
                TopologyDecision::Allow => {
                    allowed_targets.push(target_id.clone());
                }
                TopologyDecision::Warn {
                    from_role,
                    to_role,
                    description,
                } => {
                    // Advisory: emit warning event but proceed
                    self.emit_event(MobEvent {
                        cursor: 0,
                        timestamp: Utc::now(),
                        mob_id: self.config.mob_id.clone(),
                        run_id: Some(run_id.to_string()),
                        flow_id: Some(flow_id.to_string()),
                        step_id: Some(step.step_id.clone()),
                        retention: RetentionCategory::Ops,
                        kind: MobEventKind::TopologyViolation {
                            from_role: from_role.clone(),
                            to_role: to_role.clone(),
                            description: description.clone(),
                        },
                    })
                    .await;
                    allowed_targets.push(target_id.clone());
                }
                TopologyDecision::Block {
                    from_role,
                    to_role,
                } => {
                    // Strict: block and record failure
                    self.emit_event(MobEvent {
                        cursor: 0,
                        timestamp: Utc::now(),
                        mob_id: self.config.mob_id.clone(),
                        run_id: Some(run_id.to_string()),
                        flow_id: Some(flow_id.to_string()),
                        step_id: Some(step.step_id.clone()),
                        retention: RetentionCategory::Ops,
                        kind: MobEventKind::TopologyBlocked {
                            from_role: from_role.clone(),
                            to_role: to_role.clone(),
                        },
                    })
                    .await;
                    // Record in failure ledger
                    self.run_store
                        .append_failure_entry(
                            run_id,
                            FailureLedgerEntry {
                                step_id: step.step_id.clone(),
                                target_meerkat_id: target_id.clone(),
                                attempt: 1,
                                error: format!(
                                    "topology blocked: {} -> {}",
                                    from_role, to_role
                                ),
                                failed_at: Utc::now(),
                            },
                        )
                        .await?;
                }
            }
        }

        if allowed_targets.is_empty() {
            return Err(MobError::DispatchFailed {
                step_id: step.step_id.clone(),
                reason: "all targets blocked by topology policy".to_string(),
            });
        }

        let payload = serde_json::to_value(upstream_payloads)
            .unwrap_or(serde_json::Value::Null);

        // Emit StepStarted event
        self.emit_event(MobEvent {
            cursor: 0,
            timestamp: Utc::now(),
            mob_id: self.config.mob_id.clone(),
            run_id: Some(run_id.to_string()),
            flow_id: Some(flow_id.to_string()),
            step_id: Some(step.step_id.clone()),
            retention: RetentionCategory::Ops,
            kind: MobEventKind::StepStarted {
                target_count: allowed_targets.len(),
            },
        })
        .await;

        // Retry loop (REQ-MOB-037)
        let max_attempts = step.retry.attempts.max(1);
        let mut attempt: u32 = 0;
        let supervisor = crate::supervisor::MobSupervisor::new(&spec.supervisor);

        loop {
            attempt += 1;

            let (tx, mut rx) = tokio::sync::mpsc::channel::<CollectedResponse>(allowed_targets.len() + 1);
            router.register_step_channel(step.step_id.clone(), tx).await;

            let dispatched = self
                .dispatch_step(step, &allowed_targets, run_id, flow_id, &payload, attempt)
                .await?;

            for req in &dispatched {
                router.register_dispatch(req.clone()).await;
            }

            // Record dispatched ledger entries
            for req in &dispatched {
                self.run_store
                    .append_step_entry(run_id, StepLedgerEntry {
                        step_id: step.step_id.clone(),
                        target_meerkat_id: req.target_meerkat_id.clone(),
                        status: StepEntryStatus::Dispatched,
                        attempt,
                        dispatched_at: Some(req.dispatched_at),
                        completed_at: None,
                        result: None,
                        error: None,
                    })
                    .await?;

                self.emit_event(MobEvent {
                    cursor: 0,
                    timestamp: Utc::now(),
                    mob_id: self.config.mob_id.clone(),
                    run_id: Some(run_id.to_string()),
                    flow_id: Some(flow_id.to_string()),
                    step_id: Some(step.step_id.clone()),
                    retention: RetentionCategory::Debug,
                    kind: MobEventKind::DispatchSent {
                        target_meerkat_id: req.target_meerkat_id.clone(),
                    },
                })
                .await;
            }

            // Collect responses with timeout + supervisor checks (REQ-MOB-060)
            let timeout = std::time::Duration::from_millis(step.timeout_ms);
            let required = match step.collection_policy.mode {
                CollectionMode::All => dispatched.len(),
                CollectionMode::Quorum(n) => n as usize,
                CollectionMode::Any => 1,
            };

            let mut responses = Vec::new();
            let deadline = tokio::time::Instant::now() + timeout;

            while responses.len() < required {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    break;
                }
                // Check supervisor periodically during collection
                let check_interval = std::time::Duration::from_millis(100);
                let wait_time = remaining.min(check_interval);

                match tokio::time::timeout(wait_time, rx.recv()).await {
                    Ok(Some(resp)) => {
                        responses.push(resp);
                    }
                    Ok(None) => break,
                    Err(_) => {
                        // Supervisor check on timeout tick
                        if let Some(run) = self.run_store.get(run_id).await.ok().flatten() {
                            let escalations = supervisor.check_run(&run, Utc::now());
                            for event in supervisor.escalation_events(
                                &escalations, &self.config.mob_id, run_id, Some(flow_id),
                            ) {
                                self.emit_event(event).await;
                            }
                        }
                    }
                }
            }

            router.remove_step_channel(&step.step_id).await;

            // Process collected responses
            let mut step_results = BTreeMap::new();
            let mut has_failures = false;
            for resp in &responses {
                let entry_status = match resp.status {
                    ResponseStatus::Completed => StepEntryStatus::Completed,
                    ResponseStatus::Failed => {
                        has_failures = true;
                        StepEntryStatus::Failed
                    }
                    ResponseStatus::Accepted => StepEntryStatus::Dispatched,
                };
                self.run_store
                    .append_step_entry(run_id, StepLedgerEntry {
                        step_id: step.step_id.clone(),
                        target_meerkat_id: resp.target_meerkat_id.clone(),
                        status: entry_status,
                        attempt,
                        dispatched_at: None,
                        completed_at: Some(Utc::now()),
                        result: Some(resp.result.clone()),
                        error: None,
                    })
                    .await?;

                self.emit_event(MobEvent {
                    cursor: 0,
                    timestamp: Utc::now(),
                    mob_id: self.config.mob_id.clone(),
                    run_id: Some(run_id.to_string()),
                    flow_id: Some(flow_id.to_string()),
                    step_id: Some(step.step_id.clone()),
                    retention: RetentionCategory::Debug,
                    kind: MobEventKind::ResponseCollected {
                        target_meerkat_id: resp.target_meerkat_id.clone(),
                    },
                })
                .await;

                step_results.insert(resp.target_meerkat_id.clone(), resp.result.clone());
            }

            // Schema validation on collected results (REQ-MOB-100)
            if let Some(ref schema_ref) = step.expected_schema_ref
                && let Some(schema_val) = spec.schemas.get(schema_ref)
                && let Ok(validator) = jsonschema::Validator::new(schema_val)
            {
                    for (target_id, result_val) in &step_results {
                        if let Err(error) = validator.validate(result_val) {
                            let msg = format!(
                                "schema validation failed for target {target_id}: {error}"
                            );
                            match step.schema_policy {
                                crate::spec::SchemaPolicy::RetryThenFail
                                | crate::spec::SchemaPolicy::RetryThenWarn => {
                                    has_failures = true;
                                }
                                crate::spec::SchemaPolicy::WarnOnly => {}
                            }
                            self.emit_event(MobEvent {
                                cursor: 0,
                                timestamp: Utc::now(),
                                mob_id: self.config.mob_id.clone(),
                                run_id: Some(run_id.to_string()),
                                flow_id: Some(flow_id.to_string()),
                                step_id: Some(step.step_id.clone()),
                                retention: RetentionCategory::Ops,
                                kind: MobEventKind::DegradationWarning { message: msg },
                            })
                            .await;
                        }
                    }
            }

            // Record timeouts
            let collected_ids: HashSet<&str> = responses
                .iter()
                .map(|r| r.target_meerkat_id.as_str())
                .collect();
            for req in &dispatched {
                if !collected_ids.contains(req.target_meerkat_id.as_str()) {
                    self.run_store
                        .append_step_entry(run_id, StepLedgerEntry {
                            step_id: step.step_id.clone(),
                            target_meerkat_id: req.target_meerkat_id.clone(),
                            status: StepEntryStatus::TimedOut,
                            attempt,
                            dispatched_at: Some(req.dispatched_at),
                            completed_at: None,
                            result: None,
                            error: Some("collection timeout".to_string()),
                        })
                        .await?;

                    self.run_store
                        .append_failure_entry(run_id, FailureLedgerEntry {
                            step_id: step.step_id.clone(),
                            target_meerkat_id: req.target_meerkat_id.clone(),
                            attempt,
                            error: "collection timeout".to_string(),
                            failed_at: Utc::now(),
                        })
                        .await?;
                }
            }

            // Check if collection policy is met
            let policy_met = responses.len() >= required && !has_failures;

            if policy_met {
                self.emit_event(MobEvent {
                    cursor: 0,
                    timestamp: Utc::now(),
                    mob_id: self.config.mob_id.clone(),
                    run_id: Some(run_id.to_string()),
                    flow_id: Some(flow_id.to_string()),
                    step_id: Some(step.step_id.clone()),
                    retention: RetentionCategory::Ops,
                    kind: MobEventKind::StepCompleted { collected: responses.len() },
                })
                .await;
                return Ok(step_results);
            }

            // Not met — can we retry?
            if attempt < max_attempts {
                // Record failure for this attempt
                self.run_store
                    .append_failure_entry(run_id, FailureLedgerEntry {
                        step_id: step.step_id.clone(),
                        target_meerkat_id: "".to_string(),
                        attempt,
                        error: format!(
                            "attempt {attempt} failed: {}/{required} responses, has_failures={has_failures}",
                            responses.len()
                        ),
                        failed_at: Utc::now(),
                    })
                    .await?;

                // Exponential backoff
                let backoff = std::cmp::min(
                    (step.retry.backoff_ms as f64 * step.retry.multiplier.powi(attempt as i32 - 1)) as u64,
                    step.retry.max_backoff_ms,
                );
                tokio::time::sleep(std::time::Duration::from_millis(backoff)).await;
                continue;
            }

            // Out of retries — apply timeout behavior
            if responses.len() >= required {
                // We have enough responses but had failures (schema etc.)
                // For WarnOnly schema policy, accept the results
                self.emit_event(MobEvent {
                    cursor: 0,
                    timestamp: Utc::now(),
                    mob_id: self.config.mob_id.clone(),
                    run_id: Some(run_id.to_string()),
                    flow_id: Some(flow_id.to_string()),
                    step_id: Some(step.step_id.clone()),
                    retention: RetentionCategory::Ops,
                    kind: MobEventKind::StepCompleted { collected: responses.len() },
                })
                .await;
                return Ok(step_results);
            }

            match step.collection_policy.timeout_behavior {
                TimeoutBehavior::Partial => {
                    self.emit_event(MobEvent {
                        cursor: 0,
                        timestamp: Utc::now(),
                        mob_id: self.config.mob_id.clone(),
                        run_id: Some(run_id.to_string()),
                        flow_id: Some(flow_id.to_string()),
                        step_id: Some(step.step_id.clone()),
                        retention: RetentionCategory::Ops,
                        kind: MobEventKind::StepTimedOut {
                            collected: responses.len(),
                            expected: required,
                        },
                    })
                    .await;
                    return Ok(step_results);
                }
                TimeoutBehavior::Fail => {
                    self.emit_event(MobEvent {
                        cursor: 0,
                        timestamp: Utc::now(),
                        mob_id: self.config.mob_id.clone(),
                        run_id: Some(run_id.to_string()),
                        flow_id: Some(flow_id.to_string()),
                        step_id: Some(step.step_id.clone()),
                        retention: RetentionCategory::Ops,
                        kind: MobEventKind::StepFailed {
                            reason: format!(
                                "collection timeout: {}/{required} responses",
                                responses.len()
                            ),
                        },
                    })
                    .await;
                    return Err(MobError::CollectionTimeout {
                        step_id: step.step_id.clone(),
                        collected: responses.len(),
                        expected: required,
                    });
                }
            }
        } // end retry loop
    }

    /// Emit a mob event to the event store.
    async fn emit_event(&self, event: MobEvent) {
        // Best-effort: log errors but don't propagate
        if let Err(e) = self.event_store.append(event).await {
            tracing::warn!("failed to emit mob event: {e}");
        }
    }

    /// Create an Arc clone for spawned tasks.
    fn to_arc(&self) -> Arc<Self> {
        Arc::new(Self {
            config: self.config.clone(),
            spec_store: Arc::clone(&self.spec_store),
            run_store: Arc::clone(&self.run_store),
            event_store: Arc::clone(&self.event_store),
            supervisor: Arc::clone(&self.supervisor),
            resolvers: Arc::clone(&self.resolvers),
        })
    }
}

// ---------------------------------------------------------------------------
// MobService implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl MobService for MobRuntime {
    async fn apply_spec(&self, req: ApplySpecRequest) -> Result<MobSpec, MobError> {
        // Validate
        let diags = validate_spec(&req.spec, &self.config.validate_opts);
        if !diags.is_empty() {
            return Err(MobError::SpecValidation { diagnostics: diags });
        }
        // Store
        self.spec_store.put(req.spec).await
    }

    async fn get_spec(&self, mob_id: &str) -> Result<MobSpec, MobError> {
        self.spec_store
            .get(mob_id)
            .await?
            .ok_or_else(|| MobError::SpecNotFound {
                mob_id: mob_id.to_string(),
            })
    }

    async fn list_specs(&self, req: ListSpecsRequest) -> Result<Vec<MobSpec>, MobError> {
        let mut specs = self.spec_store.list().await?;
        if let Some(limit) = req.limit {
            specs.truncate(limit);
        }
        Ok(specs)
    }

    async fn delete_spec(&self, mob_id: &str) -> Result<(), MobError> {
        let deleted = self.spec_store.delete(mob_id).await?;
        if !deleted {
            return Err(MobError::SpecNotFound {
                mob_id: mob_id.to_string(),
            });
        }
        Ok(())
    }

    async fn activate(&self, req: ActivateRequest) -> Result<ActivateResult, MobError> {
        // Look up spec
        let spec = self.get_spec(&req.mob_id).await?;

        // Find the flow
        let flow = spec
            .flows
            .iter()
            .find(|f| f.flow_id == req.flow_id)
            .ok_or_else(|| MobError::Internal(format!("flow not found: {}", req.flow_id)))?
            .clone();

        // Create run
        let run_id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let run = MobRun {
            run_id: run_id.clone(),
            mob_id: req.mob_id.clone(),
            flow_id: req.flow_id.clone(),
            spec_revision: spec.spec_revision,
            status: RunStatus::Pending,
            step_ledger: Vec::new(),
            failure_ledger: Vec::new(),
            created_at: now,
            updated_at: now,
        };
        self.run_store.create(run).await?;

        let result = ActivateResult {
            run_id: run_id.clone(),
            status: RunStatus::Pending,
            spec_revision: spec.spec_revision,
        };

        // Emit RunStarted event
        self.emit_event(MobEvent {
            cursor: 0,
            timestamp: Utc::now(),
            mob_id: req.mob_id.clone(),
            run_id: Some(run_id.clone()),
            flow_id: Some(req.flow_id.clone()),
            step_id: None,
            retention: RetentionCategory::Audit,
            kind: MobEventKind::RunStarted,
        })
        .await;

        // Transition to Running
        self.run_store
            .cas_status(&run_id, RunStatus::Pending, RunStatus::Running)
            .await?;

        // Spawn flow execution
        let self_arc = self.to_arc();
        let run_id_owned = run_id.clone();
        let mob_id_owned = req.mob_id.clone();
        let flow_id_owned = req.flow_id.clone();
        let spec_owned = spec.clone();
        let activation_params = req.params.clone();
        let run_store = Arc::clone(&self.run_store);
        let event_store = Arc::clone(&self.event_store);

        tokio::spawn(async move {
            let flow_result = self_arc
                .execute_flow(&spec_owned, &flow, &run_id_owned, &activation_params)
                .await;

            match flow_result {
                Ok(()) => {
                    let _ = run_store
                        .cas_status(&run_id_owned, RunStatus::Running, RunStatus::Completed)
                        .await;
                    let _ = event_store
                        .append(MobEvent {
                            cursor: 0,
                            timestamp: Utc::now(),
                            mob_id: mob_id_owned,
                            run_id: Some(run_id_owned.clone()),
                            flow_id: Some(flow_id_owned),
                            step_id: None,
                            retention: RetentionCategory::Audit,
                            kind: MobEventKind::RunCompleted,
                        })
                        .await;
                }
                Err(e) => {
                    let _ = run_store
                        .cas_status(&run_id_owned, RunStatus::Running, RunStatus::Failed)
                        .await;
                    let _ = event_store
                        .append(MobEvent {
                            cursor: 0,
                            timestamp: Utc::now(),
                            mob_id: mob_id_owned,
                            run_id: Some(run_id_owned.clone()),
                            flow_id: Some(flow_id_owned),
                            step_id: None,
                            retention: RetentionCategory::Audit,
                            kind: MobEventKind::RunFailed {
                                reason: e.to_string(),
                            },
                        })
                        .await;
                }
            }
        });

        Ok(result)
    }

    async fn get_run(&self, run_id: &str) -> Result<MobRun, MobError> {
        self.run_store
            .get(run_id)
            .await?
            .ok_or_else(|| MobError::RunNotFound {
                run_id: run_id.to_string(),
            })
    }

    async fn list_runs(&self, req: ListRunsRequest) -> Result<Vec<MobRun>, MobError> {
        let mut runs = self
            .run_store
            .list(req.mob_id.as_deref(), req.status)
            .await?;
        if let Some(limit) = req.limit {
            runs.truncate(limit);
        }
        Ok(runs)
    }

    async fn cancel_run(&self, run_id: &str) -> Result<MobRun, MobError> {
        let run = self.get_run(run_id).await?;
        match run.status {
            RunStatus::Pending => {
                self.run_store
                    .cas_status(run_id, RunStatus::Pending, RunStatus::Cancelled)
                    .await?;
            }
            RunStatus::Running => {
                self.run_store
                    .cas_status(run_id, RunStatus::Running, RunStatus::Cancelled)
                    .await?;
            }
            _ => {
                return Err(MobError::InvalidTransition {
                    from: run.status,
                    to: RunStatus::Cancelled,
                });
            }
        }
        self.get_run(run_id).await
    }

    async fn list_meerkats(
        &self,
        req: ListMeerkatsRequest,
    ) -> Result<Vec<MeerkatIdentity>, MobError> {
        let namespace = format!("{}/{}", self.config.realm_id, req.mob_id);
        let peers = InprocRegistry::global().peers_in_namespace(&namespace);
        let identities: Vec<MeerkatIdentity> = peers
            .iter()
            .filter(|p| {
                req.role.as_ref().is_none_or(|role| {
                    p.meta.labels.get("role").map(|r| r.as_str()) == Some(role)
                })
            })
            .map(|p| MeerkatIdentity {
                meerkat_id: p
                    .meta
                    .labels
                    .get("meerkat_id")
                    .cloned()
                    .unwrap_or_else(|| p.name.clone()),
                role: p
                    .meta
                    .labels
                    .get("role")
                    .cloned()
                    .unwrap_or_default(),
                labels: p.meta.labels.clone(),
                attributes: serde_json::Value::Object(serde_json::Map::new()),
            })
            .collect();
        Ok(identities)
    }

    async fn reconcile(&self, req: ReconcileRequest) -> Result<ReconcileResult, MobError> {
        let spec = self.get_spec(&req.mob_id).await?;
        let namespace = format!("{}/{}", self.config.realm_id, req.mob_id);

        let mut total_spawned = Vec::new();
        let mut total_retired = Vec::new();
        let mut total_unchanged: usize = 0;

        for role in &spec.roles {
            // Determine desired meerkats for this role
            let desired: Vec<MeerkatIdentity> = match &role.cardinality {
                CardinalitySpec::Singleton => {
                    vec![MeerkatIdentity {
                        meerkat_id: role.role.clone(),
                        role: role.role.clone(),
                        labels: BTreeMap::new(),
                        attributes: serde_json::Value::Object(serde_json::Map::new()),
                    }]
                }
                CardinalitySpec::PerKey { .. } => {
                    // per_key meerkats are created at activation time based on
                    // params; reconcile treats the current set as desired
                    Vec::new()
                }
                CardinalitySpec::PerMeerkat { resolver } => {
                    let resolvers = self.resolvers.lock().await;
                    if let Some(res) = resolvers.get(resolver) {
                        let ctx = ResolverContext {
                            mob_id: req.mob_id.clone(),
                            role: role.role.clone(),
                            resolver_name: resolver.clone(),
                        };
                        res.list_meerkats(&ctx).await?
                    } else {
                        return Err(MobError::ResolverError(format!(
                            "resolver '{}' not registered",
                            resolver
                        )));
                    }
                }
            };

            // Get actual meerkats in namespace with this role
            let peers = InprocRegistry::global().peers_in_namespace(&namespace);
            let actual_ids: HashSet<String> = peers
                .iter()
                .filter(|p| {
                    p.meta.labels.get("role").map(|r| r.as_str()) == Some(&role.role)
                })
                .filter_map(|p| p.meta.labels.get("meerkat_id").cloned())
                .collect();

            let desired_ids: HashSet<String> =
                desired.iter().map(|d| d.meerkat_id.clone()).collect();

            // Compute diff
            let to_spawn: Vec<&str> = desired_ids
                .iter()
                .filter(|id| !actual_ids.contains(*id))
                .map(|id| id.as_str())
                .collect();
            let to_retire: Vec<&str> = actual_ids
                .iter()
                .filter(|id| !desired_ids.contains(*id))
                .map(|id| id.as_str())
                .collect();
            let unchanged = actual_ids.intersection(&desired_ids).count();

            total_unchanged += unchanged;

            if req.mode == ReconcileMode::Apply {
                // Spawn missing meerkats
                for meerkat_id in &to_spawn {
                    let name = format!(
                        "{}-{}-{}",
                        role.role,
                        meerkat_id,
                        Uuid::new_v4().simple()
                    );
                    let rt = CommsRuntime::inproc_only_scoped(
                        &name,
                        Some(namespace.clone()),
                    )
                    .map_err(|e| MobError::NamespaceError(e.to_string()))?;

                    rt.set_peer_meta(
                        PeerMeta::default()
                            .with_label("mob_id", &req.mob_id)
                            .with_label("role", &role.role)
                            .with_label("meerkat_id", *meerkat_id),
                    );

                    // Establish mutual trust with supervisor
                    let rt_name = rt.participant_name().to_string();
                    let supervisor_name =
                        self.supervisor.participant_name().to_string();

                    let spec_rt = TrustedPeerSpec::new(
                        &supervisor_name,
                        self.supervisor.public_key().to_peer_id(),
                        format!("inproc://{supervisor_name}"),
                    )
                    .map_err(MobError::NamespaceError)?;
                    CoreCommsRuntime::add_trusted_peer(&rt, spec_rt)
                        .await
                        .map_err(MobError::CommsError)?;

                    let spec_sup = TrustedPeerSpec::new(
                        &rt_name,
                        rt.public_key().to_peer_id(),
                        format!("inproc://{rt_name}"),
                    )
                    .map_err(MobError::NamespaceError)?;
                    CoreCommsRuntime::add_trusted_peer(&*self.supervisor, spec_sup)
                        .await
                        .map_err(MobError::CommsError)?;

                    // Keep the runtime alive by leaking it (in a real system,
                    // it would be held by the session runtime)
                    std::mem::forget(rt);

                    total_spawned.push(meerkat_id.to_string());
                }

                // Retire orphaned meerkats
                for meerkat_id in &to_retire {
                    // Find and unregister the peer by pubkey
                    let peer = peers.iter().find(|p| {
                        p.meta.labels.get("meerkat_id").map(|id| id.as_str())
                            == Some(meerkat_id)
                    });
                    if let Some(p) = peer {
                        InprocRegistry::global()
                            .unregister_in_namespace(&namespace, &p.pubkey);
                    }
                    total_retired.push(meerkat_id.to_string());
                }
            } else {
                // report_only: just record what would happen
                for meerkat_id in &to_spawn {
                    total_spawned.push(meerkat_id.to_string());
                }
                for meerkat_id in &to_retire {
                    total_retired.push(meerkat_id.to_string());
                }
            }
        }

        // Emit ReconcileReport event
        self.emit_event(MobEvent {
            cursor: 0,
            timestamp: Utc::now(),
            mob_id: req.mob_id.clone(),
            run_id: None,
            flow_id: None,
            step_id: None,
            retention: RetentionCategory::Ops,
            kind: MobEventKind::ReconcileReport {
                spawned: total_spawned.len(),
                retired: total_retired.len(),
                unchanged: total_unchanged,
            },
        })
        .await;

        Ok(ReconcileResult {
            spawned: total_spawned.len(),
            retired: total_retired.len(),
            unchanged: total_unchanged,
            spawned_ids: total_spawned,
            retired_ids: total_retired,
        })
    }

    async fn poll_events(&self, req: PollEventsRequest) -> Result<Vec<MobEvent>, MobError> {
        self.event_store
            .poll(&req.mob_id, req.after_cursor, req.limit)
            .await
    }
}

// ---------------------------------------------------------------------------
// Mock meerkat for testing
// ---------------------------------------------------------------------------

/// Spawn a mock meerkat that listens for PeerRequests and auto-responds.
///
/// The mock meerkat registers in the InprocRegistry under the given namespace
/// with the specified role and meerkat_id labels. It listens for incoming
/// PeerRequests and responds with `ResponseStatus::Completed` and a result
/// payload containing `{"mock": true, "meerkat_id": "<id>"}`.
///
/// Returns the CommsRuntime for the mock meerkat and a JoinHandle for cleanup.
#[cfg(any(test, feature = "test-helpers"))]
pub fn spawn_mock_meerkat(
    namespace: &str,
    name: &str,
    role: &str,
    meerkat_id: &str,
    _supervisor: &CommsRuntime,
) -> (Arc<CommsRuntime>, tokio::task::JoinHandle<()>) {
    let runtime = CommsRuntime::inproc_only_scoped(name, Some(namespace.to_string()))
        .expect("create mock meerkat comms");

    // Set peer meta
    runtime.set_peer_meta(
        PeerMeta::default()
            .with_label("mob_id", namespace.rsplit('/').next().unwrap_or(""))
            .with_label("role", role)
            .with_label("meerkat_id", meerkat_id),
    );

    let runtime_arc = Arc::new(runtime);
    let responder = Arc::clone(&runtime_arc);
    let meerkat_id_owned = meerkat_id.to_string();

    let handle = tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;

            let interactions =
                CoreCommsRuntime::drain_inbox_interactions(&*responder).await;

            for interaction in &interactions {
                if let InteractionContent::Request { intent, params } = &interaction.content {
                    // Brief delay to simulate processing
                    tokio::time::sleep(std::time::Duration::from_millis(2)).await;

                    // Send PeerResponse
                    let response_cmd = CommsCommand::PeerResponse {
                        to: PeerName::new(interaction.from.clone())
                            .expect("valid peer name"),
                        in_reply_to: interaction.id,
                        status: meerkat_core::interaction::ResponseStatus::Completed,
                        result: serde_json::json!({
                            "mock": true,
                            "meerkat_id": meerkat_id_owned,
                            "received_intent": intent,
                            "received_step_id": params.get("step_id"),
                        }),
                    };

                    let _ = CoreCommsRuntime::send(&*responder, response_cmd).await;
                }
            }
        }
    });

    (runtime_arc, handle)
}

/// Spawn a mock meerkat that does NOT respond (simulates timeout).
///
/// Like [`spawn_mock_meerkat`] but the responder task simply drains the inbox
/// without sending replies. Useful for testing quorum timeout scenarios.
#[cfg(any(test, feature = "test-helpers"))]
pub fn spawn_silent_mock_meerkat(
    namespace: &str,
    name: &str,
    role: &str,
    meerkat_id: &str,
    _supervisor: &CommsRuntime,
) -> (Arc<CommsRuntime>, tokio::task::JoinHandle<()>) {
    let runtime = CommsRuntime::inproc_only_scoped(name, Some(namespace.to_string()))
        .expect("create silent mock meerkat comms");

    runtime.set_peer_meta(
        PeerMeta::default()
            .with_label("mob_id", namespace.rsplit('/').next().unwrap_or(""))
            .with_label("role", role)
            .with_label("meerkat_id", meerkat_id),
    );

    let runtime_arc = Arc::new(runtime);
    let responder = Arc::clone(&runtime_arc);

    let handle = tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            // Drain inbox but do NOT respond
            let _interactions =
                CoreCommsRuntime::drain_inbox_interactions(&*responder).await;
        }
    });

    (runtime_arc, handle)
}

/// Spawn a mock meerkat that fails the first N attempts, then succeeds.
///
/// Used for testing retry logic (REQ-MOB-037).
#[cfg(any(test, feature = "test-helpers"))]
pub fn spawn_fail_then_succeed_mock(
    namespace: &str,
    name: &str,
    role: &str,
    meerkat_id: &str,
    _supervisor: &CommsRuntime,
    fail_count: u32,
) -> (Arc<CommsRuntime>, tokio::task::JoinHandle<()>) {
    let runtime = CommsRuntime::inproc_only_scoped(name, Some(namespace.to_string()))
        .expect("create fail-then-succeed mock comms");

    runtime.set_peer_meta(
        PeerMeta::default()
            .with_label("mob_id", namespace.rsplit('/').next().unwrap_or(""))
            .with_label("role", role)
            .with_label("meerkat_id", meerkat_id),
    );

    let runtime_arc = Arc::new(runtime);
    let responder = Arc::clone(&runtime_arc);
    let meerkat_id_owned = meerkat_id.to_string();
    let fail_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(fail_count));

    let handle = tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;

            let interactions =
                CoreCommsRuntime::drain_inbox_interactions(&*responder).await;

            for interaction in &interactions {
                if let InteractionContent::Request { .. } = &interaction.content {
                    tokio::time::sleep(std::time::Duration::from_millis(2)).await;

                    let remaining = fail_count.load(std::sync::atomic::Ordering::SeqCst);
                    let (status, result) = if remaining > 0 {
                        fail_count.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                        (
                            ResponseStatus::Failed,
                            serde_json::json!({"error": "simulated failure", "meerkat_id": meerkat_id_owned}),
                        )
                    } else {
                        (
                            ResponseStatus::Completed,
                            serde_json::json!({"mock": true, "meerkat_id": meerkat_id_owned, "retry_success": true}),
                        )
                    };

                    let response_cmd = CommsCommand::PeerResponse {
                        to: PeerName::new(interaction.from.clone())
                            .expect("valid peer name"),
                        in_reply_to: interaction.id,
                        status,
                        result,
                    };

                    let _ = CoreCommsRuntime::send(&*responder, response_cmd).await;
                }
            }
        }
    });

    (runtime_arc, handle)
}

/// Establish mutual trust between the supervisor and a mock meerkat.
#[cfg(any(test, feature = "test-helpers"))]
pub async fn establish_trust(supervisor: &CommsRuntime, meerkat: &CommsRuntime) {
    let meerkat_name = meerkat.participant_name().to_string();
    let supervisor_name = supervisor.participant_name().to_string();

    // Supervisor trusts meerkat
    let spec = TrustedPeerSpec::new(
        &meerkat_name,
        meerkat.public_key().to_peer_id(),
        format!("inproc://{meerkat_name}"),
    )
    .expect("valid peer spec");
    CoreCommsRuntime::add_trusted_peer(supervisor, spec)
        .await
        .expect("add trusted peer supervisor->meerkat");

    // Meerkat trusts supervisor
    let spec = TrustedPeerSpec::new(
        &supervisor_name,
        supervisor.public_key().to_peer_id(),
        format!("inproc://{supervisor_name}"),
    )
    .expect("valid peer spec");
    CoreCommsRuntime::add_trusted_peer(meerkat, spec)
        .await
        .expect("add trusted peer meerkat->supervisor");
}

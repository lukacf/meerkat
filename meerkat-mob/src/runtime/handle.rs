use super::*;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use meerkat_core::types::{HandlingMode, RenderMetadata};
use serde::Serialize;

/// Point-in-time snapshot of a member's execution state.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct MemberExecutionSnapshot {
    /// Current lifecycle status.
    pub status: MemberExecutionStatus,
    /// Preview of the last assistant output (if any).
    pub output_preview: Option<String>,
    /// Error description (if the member errored).
    pub error: Option<String>,
    /// Cumulative token usage.
    pub tokens_used: u64,
    /// Whether the member has reached a terminal state.
    pub is_final: bool,
}

/// Execution status for a mob member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MemberExecutionStatus {
    /// Member is active and potentially running.
    Active,
    /// Member is in the process of retiring.
    Retiring,
    /// Member has completed (session archived or not found).
    Completed,
    /// Member is not in the roster.
    Unknown,
}

/// Options for helper convenience spawns.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct HelperOptions {
    /// Profile to use. If None, requires a default profile in the definition.
    pub profile_name: Option<ProfileName>,
    /// Runtime mode override.
    pub runtime_mode: Option<crate::MobRuntimeMode>,
    /// Backend override.
    pub backend: Option<MobBackendKind>,
    /// Tool access policy for the helper.
    pub tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
}

/// Result from a helper spawn-and-wait operation.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct HelperResult {
    /// The member's final output text.
    pub output: Option<String>,
    /// Total tokens used by the helper.
    pub tokens_used: u64,
    /// The session ID that was used.
    pub session_id: Option<meerkat_core::types::SessionId>,
}

// ---------------------------------------------------------------------------
// MobHandle
// ---------------------------------------------------------------------------

/// Clone-cheap, thread-safe handle for interacting with a running mob.
///
/// All mutation commands are sent through an mpsc channel to the actor.
/// Read-only operations (roster, state) bypass the actor and read from
/// shared `Arc` state directly.
#[derive(Clone)]
pub struct MobHandle {
    pub(super) command_tx: mpsc::Sender<MobCommand>,
    pub(super) roster: Arc<RwLock<Roster>>,
    pub(super) task_board: Arc<RwLock<TaskBoard>>,
    pub(super) definition: Arc<MobDefinition>,
    pub(super) state: Arc<AtomicU8>,
    pub(super) events: Arc<dyn MobEventStore>,
    pub(super) mcp_servers: Arc<tokio::sync::Mutex<BTreeMap<String, actor::McpServerEntry>>>,
    pub(super) flow_streams:
        Arc<tokio::sync::Mutex<BTreeMap<RunId, mpsc::Sender<meerkat_core::ScopedAgentEvent>>>>,
    pub(super) session_service: Arc<dyn MobSessionService>,
}

/// Clone-cheap, capability-bearing handle for interacting with one mob member.
///
/// This is the target 0.5 API surface for message/turn submission. The mob
/// handle remains orchestration/control-plane oriented, while member-directed
/// delivery goes through this narrower capability.
#[derive(Clone)]
pub struct MemberHandle {
    mob: MobHandle,
    meerkat_id: MeerkatId,
}

#[derive(Clone)]
pub struct MobEventsView {
    inner: Arc<dyn MobEventStore>,
}

/// Spawn request for first-class batch member provisioning.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SpawnMemberSpec {
    pub profile_name: ProfileName,
    pub meerkat_id: MeerkatId,
    pub initial_message: Option<String>,
    pub runtime_mode: Option<crate::MobRuntimeMode>,
    pub backend: Option<MobBackendKind>,
    /// Opaque application context passed through to the agent build pipeline.
    pub context: Option<serde_json::Value>,
    /// Application-defined labels for this member.
    pub labels: Option<std::collections::BTreeMap<String, String>>,
    /// How this member should be launched (fresh, resume, or fork).
    pub launch_mode: crate::launch::MemberLaunchMode,
    /// Tool access policy for this member.
    pub tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
    /// How to split budget from the orchestrator to this member.
    pub budget_split_policy: Option<crate::launch::BudgetSplitPolicy>,
    /// When true, automatically wire this member to its spawner.
    pub auto_wire_parent: bool,
    /// Additional instruction sections appended to the system prompt for this member.
    pub additional_instructions: Option<Vec<String>>,
    /// Per-agent environment variables injected into shell tool subprocesses.
    pub shell_env: Option<std::collections::HashMap<String, String>>,
}

impl SpawnMemberSpec {
    pub fn new(profile: impl Into<ProfileName>, meerkat_id: impl Into<MeerkatId>) -> Self {
        Self {
            profile_name: profile.into(),
            meerkat_id: meerkat_id.into(),
            initial_message: None,
            runtime_mode: None,
            backend: None,
            context: None,
            labels: None,
            launch_mode: crate::launch::MemberLaunchMode::Fresh,
            tool_access_policy: None,
            budget_split_policy: None,
            auto_wire_parent: false,
            additional_instructions: None,
            shell_env: None,
        }
    }

    pub fn with_shell_env(mut self, env: std::collections::HashMap<String, String>) -> Self {
        self.shell_env = Some(env);
        self
    }

    pub fn with_initial_message(mut self, message: String) -> Self {
        self.initial_message = Some(message);
        self
    }

    pub fn with_runtime_mode(mut self, mode: crate::MobRuntimeMode) -> Self {
        self.runtime_mode = Some(mode);
        self
    }

    pub fn with_backend(mut self, backend: MobBackendKind) -> Self {
        self.backend = Some(backend);
        self
    }

    pub fn with_context(mut self, context: serde_json::Value) -> Self {
        self.context = Some(context);
        self
    }

    pub fn with_labels(mut self, labels: std::collections::BTreeMap<String, String>) -> Self {
        self.labels = Some(labels);
        self
    }

    /// Set launch mode to resume an existing session.
    ///
    /// This is a convenience method equivalent to setting
    /// `launch_mode = MemberLaunchMode::Resume { session_id }`.
    pub fn with_resume_session_id(mut self, id: meerkat_core::types::SessionId) -> Self {
        self.launch_mode = crate::launch::MemberLaunchMode::Resume { session_id: id };
        self
    }

    pub fn with_launch_mode(mut self, mode: crate::launch::MemberLaunchMode) -> Self {
        self.launch_mode = mode;
        self
    }

    pub fn with_tool_access_policy(mut self, policy: meerkat_core::ops::ToolAccessPolicy) -> Self {
        self.tool_access_policy = Some(policy);
        self
    }

    pub fn with_budget_split_policy(mut self, policy: crate::launch::BudgetSplitPolicy) -> Self {
        self.budget_split_policy = Some(policy);
        self
    }

    pub fn with_auto_wire_parent(mut self, auto_wire: bool) -> Self {
        self.auto_wire_parent = auto_wire;
        self
    }

    pub fn with_additional_instructions(mut self, instructions: Vec<String>) -> Self {
        self.additional_instructions = Some(instructions);
        self
    }

    pub fn from_wire(
        profile: String,
        meerkat_id: String,
        initial_message: Option<String>,
        runtime_mode: Option<crate::MobRuntimeMode>,
        backend: Option<MobBackendKind>,
    ) -> Self {
        let mut spec = Self::new(profile, meerkat_id);
        spec.initial_message = initial_message;
        spec.runtime_mode = runtime_mode;
        spec.backend = backend;
        spec
    }

    /// Extract the resume session ID if the launch mode is `Resume`.
    pub fn resume_session_id(&self) -> Option<&meerkat_core::types::SessionId> {
        match &self.launch_mode {
            crate::launch::MemberLaunchMode::Resume { session_id } => Some(session_id),
            _ => None,
        }
    }
}

impl MobEventsView {
    pub async fn poll(
        &self,
        after_cursor: u64,
        limit: usize,
    ) -> Result<Vec<crate::event::MobEvent>, MobError> {
        self.inner.poll(after_cursor, limit).await
    }

    pub async fn replay_all(&self) -> Result<Vec<crate::event::MobEvent>, MobError> {
        self.inner.replay_all().await
    }
}

impl MobHandle {
    /// Poll mob events from the underlying store.
    pub async fn poll_events(
        &self,
        after_cursor: u64,
        limit: usize,
    ) -> Result<Vec<crate::event::MobEvent>, MobError> {
        self.events.poll(after_cursor, limit).await
    }

    /// Current mob lifecycle state (lock-free read).
    pub fn status(&self) -> MobState {
        MobState::from_u8(self.state.load(Ordering::Acquire))
    }

    /// Access the mob definition.
    pub fn definition(&self) -> &MobDefinition {
        &self.definition
    }

    /// Mob ID.
    pub fn mob_id(&self) -> &MobId {
        &self.definition.id
    }

    /// Snapshot of the current roster.
    pub async fn roster(&self) -> Roster {
        self.roster.read().await.clone()
    }

    /// List active (operational) members in the roster.
    ///
    /// Excludes members in `Retiring` state. Used by flow target selection,
    /// supervisor escalation, and other paths that assume operational members.
    /// For full roster visibility including retiring members, use
    /// [`list_all_members`](Self::list_all_members).
    pub async fn list_members(&self) -> Vec<RosterEntry> {
        self.roster.read().await.list().cloned().collect()
    }

    /// List all members including those in `Retiring` state.
    ///
    /// The `state` field on each [`RosterEntry`] distinguishes `Active` from
    /// `Retiring`. Use this for observability and membership inspection where
    /// in-flight retires should be visible.
    pub async fn list_all_members(&self) -> Vec<RosterEntry> {
        self.roster.read().await.list_all().cloned().collect()
    }

    /// Get a specific member entry.
    pub async fn get_member(&self, meerkat_id: &MeerkatId) -> Option<RosterEntry> {
        self.roster.read().await.get(meerkat_id).cloned()
    }

    /// Acquire a capability-bearing handle for a specific active member.
    pub async fn member(&self, meerkat_id: &MeerkatId) -> Result<MemberHandle, MobError> {
        let entry = self
            .get_member(meerkat_id)
            .await
            .ok_or_else(|| MobError::MeerkatNotFound(meerkat_id.clone()))?;
        if entry.state != crate::roster::MemberState::Active {
            return Err(MobError::MeerkatNotFound(meerkat_id.clone()));
        }
        Ok(MemberHandle {
            mob: self.clone(),
            meerkat_id: meerkat_id.clone(),
        })
    }

    /// Access a read-only events view for polling/replay.
    pub fn events(&self) -> MobEventsView {
        MobEventsView {
            inner: self.events.clone(),
        }
    }

    /// Subscribe to agent-level events for a specific meerkat.
    ///
    /// Looks up the meerkat's session ID from the roster, then subscribes
    /// to the session-level event stream via [`MobSessionService`].
    ///
    /// Returns `MobError::MeerkatNotFound` if the meerkat is not in the
    /// roster or has no session ID.
    pub async fn subscribe_agent_events(
        &self,
        meerkat_id: &MeerkatId,
    ) -> Result<EventStream, MobError> {
        let session_id = {
            let roster = self.roster.read().await;
            roster
                .session_id(meerkat_id)
                .cloned()
                .ok_or_else(|| MobError::MeerkatNotFound(meerkat_id.clone()))?
        };
        SessionService::subscribe_session_events(self.session_service.as_ref(), &session_id)
            .await
            .map_err(|e| {
                MobError::Internal(format!(
                    "failed to subscribe to agent events for '{meerkat_id}': {e}"
                ))
            })
    }

    /// Subscribe to agent events for all active members (point-in-time snapshot).
    ///
    /// Returns one stream per active member that has a session ID. Members
    /// spawned after this call are not included — use [`subscribe_mob_events`]
    /// for a continuously updated view.
    pub async fn subscribe_all_agent_events(&self) -> Vec<(MeerkatId, EventStream)> {
        let entries: Vec<_> = {
            let roster = self.roster.read().await;
            roster
                .list()
                .filter_map(|e| {
                    e.member_ref
                        .session_id()
                        .map(|sid| (e.meerkat_id.clone(), sid.clone()))
                })
                .collect()
        };
        let mut streams = Vec::with_capacity(entries.len());
        for (meerkat_id, session_id) in entries {
            if let Ok(stream) =
                SessionService::subscribe_session_events(self.session_service.as_ref(), &session_id)
                    .await
            {
                streams.push((meerkat_id, stream));
            }
        }
        streams
    }

    /// Subscribe to a continuously-updated, mob-level event bus.
    ///
    /// Spawns an independent task that merges per-member session streams,
    /// tags each event with [`AttributedEvent`], and tracks roster changes
    /// (spawns/retires) automatically. Drop the returned handle to stop
    /// the router.
    pub fn subscribe_mob_events(&self) -> super::event_router::MobEventRouterHandle {
        self.subscribe_mob_events_with_config(super::event_router::MobEventRouterConfig::default())
    }

    /// Like [`subscribe_mob_events`](Self::subscribe_mob_events) with explicit config.
    pub fn subscribe_mob_events_with_config(
        &self,
        config: super::event_router::MobEventRouterConfig,
    ) -> super::event_router::MobEventRouterHandle {
        super::event_router::spawn_event_router(
            self.session_service.clone(),
            self.events.clone(),
            self.roster.clone(),
            config,
        )
    }

    /// Snapshot of MCP server lifecycle state tracked by this runtime.
    pub async fn mcp_server_states(&self) -> BTreeMap<String, bool> {
        self.mcp_servers
            .lock()
            .await
            .iter()
            .map(|(name, entry)| (name.clone(), entry.running))
            .collect()
    }

    /// Start a flow run and return its run ID.
    pub async fn run_flow(
        &self,
        flow_id: FlowId,
        params: serde_json::Value,
    ) -> Result<RunId, MobError> {
        self.run_flow_with_stream(flow_id, params, None).await
    }

    /// Start a flow run with an optional scoped stream sink.
    pub async fn run_flow_with_stream(
        &self,
        flow_id: FlowId,
        params: serde_json::Value,
        scoped_event_tx: Option<mpsc::Sender<meerkat_core::ScopedAgentEvent>>,
    ) -> Result<RunId, MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::RunFlow {
                flow_id,
                activation_params: params,
                scoped_event_tx,
                reply_tx,
            })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Request cancellation of an in-flight flow run.
    pub async fn cancel_flow(&self, run_id: RunId) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::CancelFlow { run_id, reply_tx })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Fetch a flow run snapshot from the run store.
    pub async fn flow_status(&self, run_id: RunId) -> Result<Option<MobRun>, MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::FlowStatus { run_id, reply_tx })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// List all configured flow IDs in this mob definition.
    pub fn list_flows(&self) -> Vec<FlowId> {
        self.definition.flows.keys().cloned().collect()
    }

    /// Spawn a new member from a profile and return its member reference.
    pub async fn spawn(
        &self,
        profile_name: ProfileName,
        meerkat_id: MeerkatId,
        initial_message: Option<String>,
    ) -> Result<MemberRef, MobError> {
        self.spawn_with_options(profile_name, meerkat_id, initial_message, None, None)
            .await
    }

    /// Spawn a new member from a profile with explicit backend override.
    pub async fn spawn_with_backend(
        &self,
        profile_name: ProfileName,
        meerkat_id: MeerkatId,
        initial_message: Option<String>,
        backend: Option<MobBackendKind>,
    ) -> Result<MemberRef, MobError> {
        self.spawn_with_options(profile_name, meerkat_id, initial_message, None, backend)
            .await
    }

    /// Spawn a new member from a profile with explicit runtime mode/backend overrides.
    pub async fn spawn_with_options(
        &self,
        profile_name: ProfileName,
        meerkat_id: MeerkatId,
        initial_message: Option<String>,
        runtime_mode: Option<crate::MobRuntimeMode>,
        backend: Option<MobBackendKind>,
    ) -> Result<MemberRef, MobError> {
        let mut spec = SpawnMemberSpec::new(profile_name, meerkat_id);
        spec.initial_message = initial_message;
        spec.runtime_mode = runtime_mode;
        spec.backend = backend;
        self.spawn_spec(spec).await
    }

    /// Attach an existing session by reusing the mob spawn control-plane path.
    pub async fn attach_existing_session(
        &self,
        profile_name: ProfileName,
        meerkat_id: MeerkatId,
        session_id: meerkat_core::types::SessionId,
        runtime_mode: Option<crate::MobRuntimeMode>,
        backend: Option<MobBackendKind>,
    ) -> Result<MemberRef, MobError> {
        let mut spec = SpawnMemberSpec::new(profile_name, meerkat_id);
        spec.launch_mode = crate::launch::MemberLaunchMode::Resume { session_id };
        spec.runtime_mode = runtime_mode;
        spec.backend = backend;
        self.spawn_spec(spec).await
    }

    /// Attach an existing session as the mob orchestrator.
    pub async fn attach_existing_session_as_orchestrator(
        &self,
        profile_name: ProfileName,
        meerkat_id: MeerkatId,
        session_id: meerkat_core::types::SessionId,
    ) -> Result<MemberRef, MobError> {
        self.attach_existing_session(profile_name, meerkat_id, session_id, None, None)
            .await
    }

    /// Attach an existing session as a regular mob member.
    pub async fn attach_existing_session_as_member(
        &self,
        profile_name: ProfileName,
        meerkat_id: MeerkatId,
        session_id: meerkat_core::types::SessionId,
    ) -> Result<MemberRef, MobError> {
        self.attach_existing_session(profile_name, meerkat_id, session_id, None, None)
            .await
    }

    /// Spawn a member from a fully-specified [`SpawnMemberSpec`].
    pub async fn spawn_spec(&self, spec: SpawnMemberSpec) -> Result<MemberRef, MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::Spawn { spec, reply_tx })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Spawn multiple members in parallel.
    ///
    /// Results preserve input order.
    pub async fn spawn_many(
        &self,
        specs: Vec<SpawnMemberSpec>,
    ) -> Vec<Result<MemberRef, MobError>> {
        futures::future::join_all(specs.into_iter().map(|spec| self.spawn_spec(spec))).await
    }

    /// Retire a member, archiving its session and removing trust.
    pub async fn retire(&self, meerkat_id: MeerkatId) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::Retire {
                meerkat_id,
                reply_tx,
            })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Retire a member and enqueue a respawn with the same profile.
    ///
    /// Returns `Ok(())` once retire completes and spawn is enqueued.
    /// The new member becomes available when `MeerkatSpawned` is emitted.
    pub async fn respawn(
        &self,
        meerkat_id: MeerkatId,
        initial_message: Option<String>,
    ) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::Respawn {
                meerkat_id,
                initial_message,
                reply_tx,
            })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Retire all roster members concurrently in a single actor command.
    pub async fn retire_all(&self) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::RetireAll { reply_tx })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Wire two members together (bidirectional trust).
    pub async fn wire(&self, a: MeerkatId, b: MeerkatId) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::Wire { a, b, reply_tx })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Unwire two members (remove bidirectional trust).
    pub async fn unwire(&self, a: MeerkatId, b: MeerkatId) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::Unwire { a, b, reply_tx })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Compatibility wrapper for internal-turn submission.
    ///
    /// Prefer [`MobHandle::member`] plus [`MemberHandle::internal_turn`] for
    /// the target 0.5 API shape.
    pub async fn internal_turn(
        &self,
        meerkat_id: MeerkatId,
        message: impl Into<meerkat_core::types::ContentInput>,
    ) -> Result<(), MobError> {
        self.member(&meerkat_id).await?.internal_turn(message).await
    }

    pub(super) async fn external_turn_for_member(
        &self,
        meerkat_id: MeerkatId,
        message: meerkat_core::types::ContentInput,
        handling_mode: HandlingMode,
        render_metadata: Option<RenderMetadata>,
    ) -> Result<meerkat_core::types::SessionId, MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::ExternalTurn {
                meerkat_id,
                message,
                handling_mode,
                render_metadata,
                reply_tx,
            })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    pub(super) async fn internal_turn_for_member(
        &self,
        meerkat_id: MeerkatId,
        message: meerkat_core::types::ContentInput,
    ) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::InternalTurn {
                meerkat_id,
                message,
                reply_tx,
            })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Transition Running -> Stopped. Mutation commands are rejected while stopped.
    pub async fn stop(&self) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::Stop { reply_tx })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Transition Stopped -> Running.
    pub async fn resume(&self) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::ResumeLifecycle { reply_tx })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Archive all members, emit MobCompleted, and transition to Completed.
    pub async fn complete(&self) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::Complete { reply_tx })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Wipe all runtime state and transition back to `Running`.
    ///
    /// Like `destroy()` but keeps the actor alive and transitions to `Running`
    /// instead of `Destroyed`. The handle remains usable after reset.
    pub async fn reset(&self) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::Reset { reply_tx })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Retire active members and clear persisted mob storage.
    pub async fn destroy(&self) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::Destroy { reply_tx })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Create a task in the shared mob task board.
    pub async fn task_create(
        &self,
        subject: String,
        description: String,
        blocked_by: Vec<TaskId>,
    ) -> Result<TaskId, MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::TaskCreate {
                subject,
                description,
                blocked_by,
                reply_tx,
            })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Update task status/owner in the shared mob task board.
    pub async fn task_update(
        &self,
        task_id: TaskId,
        status: TaskStatus,
        owner: Option<MeerkatId>,
    ) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::TaskUpdate {
                task_id,
                status,
                owner,
                reply_tx,
            })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// List tasks from the in-memory task board projection.
    pub async fn task_list(&self) -> Result<Vec<MobTask>, MobError> {
        Ok(self.task_board.read().await.list().cloned().collect())
    }

    /// Get a task by ID from the in-memory task board projection.
    pub async fn task_get(&self, task_id: &TaskId) -> Result<Option<MobTask>, MobError> {
        Ok(self.task_board.read().await.get(task_id).cloned())
    }

    #[cfg(test)]
    pub async fn debug_flow_tracker_counts(&self) -> Result<(usize, usize), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::FlowTrackerCounts { reply_tx })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))
    }

    #[cfg(test)]
    pub async fn debug_orchestrator_snapshot(
        &self,
    ) -> Result<super::MobOrchestratorSnapshot, MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::OrchestratorSnapshot { reply_tx })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))
    }

    /// Set or clear the spawn policy for automatic member provisioning.
    ///
    /// When set, external turns targeting an unknown meerkat ID will
    /// consult the policy before returning `MeerkatNotFound`.
    pub async fn set_spawn_policy(
        &self,
        policy: Option<Arc<dyn super::spawn_policy::SpawnPolicy>>,
    ) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::SetSpawnPolicy { policy, reply_tx })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?;
        Ok(())
    }

    /// Shut down the actor. After this, no more commands are accepted.
    pub async fn shutdown(&self) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::Shutdown { reply_tx })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))??;
        Ok(())
    }

    /// Force-cancel a member's in-flight turn via session interrupt.
    ///
    /// Unlike [`retire`](Self::retire), this does not archive the session or
    /// remove the member from the roster — it only cancels the current turn.
    pub async fn force_cancel_member(&self, meerkat_id: MeerkatId) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::ForceCancel {
                meerkat_id,
                reply_tx,
            })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Get a point-in-time execution snapshot for a member.
    pub async fn member_status(
        &self,
        meerkat_id: &MeerkatId,
    ) -> Result<MemberExecutionSnapshot, MobError> {
        let roster = self.roster.read().await;
        let entry = roster.get(meerkat_id);
        let Some(entry) = entry else {
            return Ok(MemberExecutionSnapshot {
                status: MemberExecutionStatus::Unknown,
                output_preview: None,
                error: None,
                tokens_used: 0,
                is_final: true,
            });
        };
        let status = match entry.state {
            crate::roster::MemberState::Active => MemberExecutionStatus::Active,
            crate::roster::MemberState::Retiring => MemberExecutionStatus::Retiring,
        };
        let session_id = entry.member_ref.session_id().cloned();
        drop(roster);

        let (output_preview, tokens_used, is_active) = if let Some(ref sid) = session_id {
            match self.session_service.read(sid).await {
                Ok(view) => {
                    let output = view.state.last_assistant_text.clone();
                    let tokens = view.billing.total_tokens;
                    let active = view.state.is_active;
                    (output, tokens, active)
                }
                Err(_) => (None, 0, false),
            }
        } else {
            (None, 0, false)
        };

        let is_final = !is_active && status != MemberExecutionStatus::Active;
        Ok(MemberExecutionSnapshot {
            status,
            output_preview,
            error: None,
            tokens_used,
            is_final,
        })
    }

    /// Wait for a specific member to reach a terminal state, then return its snapshot.
    ///
    /// Polls the session state until the member is no longer active.
    pub async fn wait_one(
        &self,
        meerkat_id: &MeerkatId,
    ) -> Result<MemberExecutionSnapshot, MobError> {
        loop {
            let snapshot = self.member_status(meerkat_id).await?;
            if snapshot.is_final || snapshot.status == MemberExecutionStatus::Unknown {
                return Ok(snapshot);
            }
            // Check if session is still active
            let roster = self.roster.read().await;
            let session_id = roster
                .get(meerkat_id)
                .and_then(|e| e.member_ref.session_id().cloned());
            drop(roster);

            if let Some(sid) = session_id {
                match self.session_service.read(&sid).await {
                    Ok(view) if !view.state.is_active => {
                        return self.member_status(meerkat_id).await;
                    }
                    Err(_) => {
                        return self.member_status(meerkat_id).await;
                    }
                    _ => {}
                }
            } else {
                return self.member_status(meerkat_id).await;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    /// Wait for all specified members to reach terminal states.
    pub async fn wait_all(
        &self,
        meerkat_ids: &[MeerkatId],
    ) -> Result<Vec<MemberExecutionSnapshot>, MobError> {
        let futs = meerkat_ids
            .iter()
            .map(|id| self.wait_one(id))
            .collect::<Vec<_>>();
        let results = futures::future::join_all(futs).await;
        results.into_iter().collect()
    }

    /// Collect snapshots for all members that have reached terminal states.
    pub async fn collect_completed(&self) -> Vec<(MeerkatId, MemberExecutionSnapshot)> {
        let entries = self.list_all_members().await;
        let mut completed = Vec::new();
        for entry in entries {
            if let Ok(snapshot) = self.member_status(&entry.meerkat_id).await {
                if snapshot.is_final {
                    completed.push((entry.meerkat_id, snapshot));
                }
            }
        }
        completed
    }

    /// Spawn a fresh helper, wait for it to complete, retire it, and return its result.
    ///
    /// This is a convenience wrapper around spawn → wait → collect → retire for
    /// short-lived sub-tasks.
    pub async fn spawn_helper(
        &self,
        meerkat_id: MeerkatId,
        task: impl Into<String>,
        options: HelperOptions,
    ) -> Result<HelperResult, MobError> {
        let profile_name = options
            .profile_name
            .or_else(|| self.definition.profiles.keys().next().cloned())
            .ok_or_else(|| {
                MobError::Internal("no profile specified and definition has no profiles".into())
            })?;
        let task_text = task.into();
        let mut spec = SpawnMemberSpec::new(profile_name, meerkat_id.clone());
        spec.initial_message = Some(task_text);
        spec.runtime_mode = options.runtime_mode;
        spec.backend = options.backend;
        spec.tool_access_policy = options.tool_access_policy;
        spec.auto_wire_parent = true;

        let member_ref = self.spawn_spec(spec).await?;
        let snapshot = self.wait_one(&meerkat_id).await?;
        let _ = self.retire(meerkat_id.clone()).await;

        Ok(HelperResult {
            output: snapshot.output_preview,
            tokens_used: snapshot.tokens_used,
            session_id: member_ref.session_id().cloned(),
        })
    }

    /// Fork from an existing member's context, wait for completion, retire, and return.
    ///
    /// Like `spawn_helper` but uses `MemberLaunchMode::Fork` to share conversation
    /// context with the source member.
    pub async fn fork_helper(
        &self,
        source_member_id: &MeerkatId,
        meerkat_id: MeerkatId,
        task: impl Into<String>,
        fork_context: crate::launch::ForkContext,
        options: HelperOptions,
    ) -> Result<HelperResult, MobError> {
        let profile_name = options
            .profile_name
            .or_else(|| self.definition.profiles.keys().next().cloned())
            .ok_or_else(|| {
                MobError::Internal("no profile specified and definition has no profiles".into())
            })?;
        let task_text = task.into();
        let mut spec = SpawnMemberSpec::new(profile_name, meerkat_id.clone());
        spec.initial_message = Some(task_text);
        spec.runtime_mode = options.runtime_mode;
        spec.backend = options.backend;
        spec.tool_access_policy = options.tool_access_policy;
        spec.auto_wire_parent = true;
        spec.launch_mode = crate::launch::MemberLaunchMode::Fork {
            source_member_id: source_member_id.clone(),
            fork_context,
        };

        let member_ref = self.spawn_spec(spec).await?;
        let snapshot = self.wait_one(&meerkat_id).await?;
        let _ = self.retire(meerkat_id.clone()).await;

        Ok(HelperResult {
            output: snapshot.output_preview,
            tokens_used: snapshot.tokens_used,
            session_id: member_ref.session_id().cloned(),
        })
    }
}

impl MemberHandle {
    /// Target member id for this capability.
    pub fn meerkat_id(&self) -> &MeerkatId {
        &self.meerkat_id
    }

    /// Submit external work to this member through the canonical runtime path.
    pub async fn send(
        &self,
        content: impl Into<meerkat_core::types::ContentInput>,
        handling_mode: HandlingMode,
    ) -> Result<meerkat_core::types::SessionId, MobError> {
        self.send_with_render_metadata(content, handling_mode, None)
            .await
    }

    /// Submit external work with explicit normalized render metadata.
    pub async fn send_with_render_metadata(
        &self,
        content: impl Into<meerkat_core::types::ContentInput>,
        handling_mode: HandlingMode,
        render_metadata: Option<RenderMetadata>,
    ) -> Result<meerkat_core::types::SessionId, MobError> {
        self.mob
            .external_turn_for_member(
                self.meerkat_id.clone(),
                content.into(),
                handling_mode,
                render_metadata,
            )
            .await
    }

    /// Submit internal work to this member without external addressability checks.
    pub async fn internal_turn(
        &self,
        content: impl Into<meerkat_core::types::ContentInput>,
    ) -> Result<(), MobError> {
        self.mob
            .internal_turn_for_member(self.meerkat_id.clone(), content.into())
            .await
    }

    /// Subscribe to this member's agent events.
    pub async fn subscribe_events(&self) -> Result<EventStream, MobError> {
        self.mob.subscribe_agent_events(&self.meerkat_id).await
    }
}

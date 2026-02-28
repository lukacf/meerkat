use super::*;
#[cfg(target_arch = "wasm32")]
use crate::tokio;

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

#[derive(Clone)]
pub struct MobEventsView {
    inner: Arc<dyn MobEventStore>,
}

/// Spawn request for first-class batch member provisioning.
#[derive(Clone, Debug)]
pub struct SpawnMemberSpec {
    pub profile_name: ProfileName,
    pub meerkat_id: MeerkatId,
    pub initial_message: Option<String>,
    pub runtime_mode: Option<crate::MobRuntimeMode>,
    pub backend: Option<MobBackendKind>,
}

impl SpawnMemberSpec {
    pub fn from_wire(
        profile: String,
        meerkat_id: String,
        initial_message: Option<String>,
        runtime_mode: Option<crate::MobRuntimeMode>,
        backend: Option<MobBackendKind>,
    ) -> Self {
        Self {
            profile_name: ProfileName::from(profile),
            meerkat_id: MeerkatId::from(meerkat_id),
            initial_message,
            runtime_mode,
            backend,
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
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::Spawn {
                profile_name,
                meerkat_id,
                initial_message,
                runtime_mode,
                backend,
                reply_tx,
            })
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
        futures::future::join_all(specs.into_iter().map(|spec| {
            self.spawn_with_options(
                spec.profile_name,
                spec.meerkat_id,
                spec.initial_message,
                spec.runtime_mode,
                spec.backend,
            )
        }))
        .await
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

    /// Send a message to a member (enforces external_addressable).
    pub async fn send_message(
        &self,
        meerkat_id: MeerkatId,
        message: String,
    ) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::ExternalTurn {
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

    /// Send an internal turn to a member (no external_addressable check).
    pub async fn internal_turn(
        &self,
        meerkat_id: MeerkatId,
        message: String,
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
}

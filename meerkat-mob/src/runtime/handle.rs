use super::*;

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
    pub(super) mcp_running: Arc<RwLock<BTreeMap<String, bool>>>,
}

#[derive(Clone)]
pub struct MobEventsView {
    inner: Arc<dyn MobEventStore>,
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

    /// List all meerkats in the roster.
    pub async fn list_meerkats(&self) -> Vec<RosterEntry> {
        self.roster.read().await.list().cloned().collect()
    }

    /// Get a specific meerkat entry.
    pub async fn get_meerkat(&self, meerkat_id: &MeerkatId) -> Option<RosterEntry> {
        self.roster.read().await.get(meerkat_id).cloned()
    }

    /// Access a read-only events view for polling/replay.
    pub fn events(&self) -> MobEventsView {
        MobEventsView {
            inner: self.events.clone(),
        }
    }

    /// Snapshot of MCP server lifecycle state tracked by this runtime.
    pub async fn mcp_server_states(&self) -> BTreeMap<String, bool> {
        self.mcp_running.read().await.clone()
    }

    /// Start a flow run and return its run ID.
    pub async fn run_flow(
        &self,
        flow_id: FlowId,
        params: serde_json::Value,
    ) -> Result<RunId, MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::RunFlow {
                flow_id,
                activation_params: params,
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

    /// Spawn a new meerkat from a profile and return its member reference.
    pub async fn spawn_member_ref(
        &self,
        profile_name: ProfileName,
        meerkat_id: MeerkatId,
        initial_message: Option<String>,
    ) -> Result<MemberRef, MobError> {
        self.spawn_member_ref_with_runtime_mode_and_backend(
            profile_name,
            meerkat_id,
            initial_message,
            None,
            None,
        )
            .await
    }

    /// Spawn a new meerkat from a profile with explicit backend override.
    pub async fn spawn_member_ref_with_backend(
        &self,
        profile_name: ProfileName,
        meerkat_id: MeerkatId,
        initial_message: Option<String>,
        backend: Option<MobBackendKind>,
    ) -> Result<MemberRef, MobError> {
        self.spawn_member_ref_with_runtime_mode_and_backend(
            profile_name,
            meerkat_id,
            initial_message,
            None,
            backend,
        )
        .await
    }

    /// Spawn a new meerkat from a profile with explicit runtime mode/backend overrides.
    pub async fn spawn_member_ref_with_runtime_mode_and_backend(
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

    /// Spawn a new meerkat from a profile.
    ///
    /// Compatibility API: returns the bridged session ID for session-backed members.
    pub async fn spawn(
        &self,
        profile_name: ProfileName,
        meerkat_id: MeerkatId,
        initial_message: Option<String>,
    ) -> Result<SessionId, MobError> {
        let member_ref = self
            .spawn_member_ref(profile_name, meerkat_id, initial_message)
            .await?;
        member_ref.session_id().cloned().ok_or_else(|| {
            MobError::Internal(format!(
                "spawned member has no session bridge; use spawn_member_ref() instead: {member_ref:?}"
            ))
        })
    }

    /// Retire a meerkat, archiving its session and removing trust.
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

    /// Wire two meerkats together (bidirectional trust).
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

    /// Unwire two meerkats (remove bidirectional trust).
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

    /// Send an external turn to a meerkat (enforces external_addressable).
    pub async fn external_turn(
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

    /// Send an internal turn to a meerkat (no external_addressable check).
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

    /// Archive all meerkats, emit MobCompleted, and transition to Completed.
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

    /// Retire active meerkats and clear persisted mob storage.
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

    /// Shut down the actor. After this, no more commands are accepted.
    pub async fn shutdown(&self) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self
            .command_tx
            .send(MobCommand::Shutdown { reply_tx })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))??;
        Ok(())
    }
}

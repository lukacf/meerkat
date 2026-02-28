use super::disposal::{
    BulkBestEffort, DisposalContext, DisposalReport, DisposalStep, ErrorPolicy, WarnAndContinue,
};
use super::provision_guard::PendingProvision;
use super::terminalization::{FlowTerminalizationAuthority, TerminalizationTarget};
use super::transaction::LifecycleRollback;
use super::*;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use futures::FutureExt;
use futures::stream::{FuturesUnordered, StreamExt};
use std::collections::VecDeque;

type AutonomousHostLoopHandle = tokio::task::JoinHandle<Result<(), MobError>>;
// Sized for real mob-scale startup/shutdown fan-out (50+ members).
const MAX_PARALLEL_HOST_LOOP_OPS: usize = 64;
const MAX_LIFECYCLE_NOTIFICATION_TASKS: usize = 16;

/// Unified MCP server entry: process handle + running status behind a single lock.
pub(super) struct McpServerEntry {
    #[cfg(not(target_arch = "wasm32"))]
    pub process: Option<Child>,
    pub running: bool,
}

pub(super) struct PendingSpawn {
    profile_name: ProfileName,
    meerkat_id: MeerkatId,
    prompt: String,
    runtime_mode: crate::MobRuntimeMode,
    labels: std::collections::BTreeMap<String, String>,
    reply_tx: oneshot::Sender<Result<MemberRef, MobError>>,
}

// ---------------------------------------------------------------------------
// MobActor
// ---------------------------------------------------------------------------

/// The actor that processes mob commands sequentially.
///
/// Owns all mutable state. Runs in a dedicated tokio task.
/// All mutations go through here; reads bypass via shared `Arc` state.
pub(super) struct MobActor {
    pub(super) definition: Arc<MobDefinition>,
    pub(super) roster: Arc<RwLock<Roster>>,
    pub(super) task_board: Arc<RwLock<TaskBoard>>,
    pub(super) state: Arc<AtomicU8>,
    pub(super) events: Arc<dyn MobEventStore>,
    pub(super) run_store: Arc<dyn MobRunStore>,
    pub(super) provisioner: Arc<dyn MobProvisioner>,
    pub(super) flow_engine: FlowEngine,
    pub(super) run_tasks: BTreeMap<RunId, tokio::task::JoinHandle<()>>,
    pub(super) run_cancel_tokens: BTreeMap<RunId, (tokio_util::sync::CancellationToken, FlowId)>,
    pub(super) flow_streams:
        Arc<tokio::sync::Mutex<BTreeMap<RunId, mpsc::Sender<meerkat_core::ScopedAgentEvent>>>>,
    pub(super) mcp_servers: Arc<tokio::sync::Mutex<BTreeMap<String, McpServerEntry>>>,
    pub(super) command_tx: mpsc::Sender<MobCommand>,
    pub(super) tool_bundles: BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
    pub(super) default_llm_client: Option<Arc<dyn LlmClient>>,
    pub(super) retired_event_index: Arc<RwLock<HashSet<String>>>,
    pub(super) autonomous_host_loops:
        Arc<tokio::sync::Mutex<BTreeMap<MeerkatId, AutonomousHostLoopHandle>>>,
    pub(super) next_spawn_ticket: u64,
    pub(super) pending_spawns: BTreeMap<u64, PendingSpawn>,
    pub(super) pending_spawn_ids: HashSet<MeerkatId>,
    pub(super) pending_spawn_tasks: BTreeMap<u64, tokio::task::JoinHandle<()>>,
    pub(super) edge_locks: Arc<super::edge_locks::EdgeLockRegistry>,
    pub(super) lifecycle_tasks: tokio::task::JoinSet<()>,
    pub(super) session_service: Arc<dyn MobSessionService>,
    pub(super) spawn_policy: Option<Arc<dyn super::spawn_policy::SpawnPolicy>>,
}

impl MobActor {
    fn state(&self) -> MobState {
        MobState::from_u8(self.state.load(Ordering::Acquire))
    }

    fn mob_handle_for_tools(&self) -> MobHandle {
        MobHandle {
            command_tx: self.command_tx.clone(),
            roster: self.roster.clone(),
            task_board: self.task_board.clone(),
            definition: self.definition.clone(),
            state: self.state.clone(),
            events: self.events.clone(),
            mcp_servers: self.mcp_servers.clone(),
            flow_streams: self.flow_streams.clone(),
            session_service: self.session_service.clone(),
        }
    }

    fn expect_state(&self, expected: &[MobState], to: MobState) -> Result<(), MobError> {
        let current = self.state();
        if !expected.contains(&current) {
            return Err(MobError::InvalidTransition { from: current, to });
        }
        Ok(())
    }

    /// Guard that the mob is in one of the `allowed` states.
    ///
    /// Unlike `expect_state`, this does not imply a state transition — it is
    /// used by command handlers that operate *within* the current state
    /// (retire, wire, external turn, etc.). The `to` parameter in the error
    /// is set to the first allowed state as a hint; no actual transition occurs.
    fn require_state(&self, allowed: &[MobState]) -> Result<(), MobError> {
        let current = self.state();
        if !allowed.contains(&current) {
            return Err(MobError::InvalidTransition {
                from: current,
                to: allowed[0],
            });
        }
        Ok(())
    }

    async fn notify_orchestrator_lifecycle(&mut self, message: String) {
        // Drain completed lifecycle tasks (non-blocking).
        while let Some(result) = self.lifecycle_tasks.try_join_next() {
            if let Err(error) = result {
                tracing::debug!(error = %error, "lifecycle notification task failed");
            }
        }

        let Some(orchestrator) = &self.definition.orchestrator else {
            return;
        };
        let Some(orchestrator_entry) = self
            .roster
            .read()
            .await
            .by_profile(&orchestrator.profile)
            .next()
            .cloned()
        else {
            return;
        };

        // Backpressure: drop notification if at capacity.
        if self.lifecycle_tasks.len() >= MAX_LIFECYCLE_NOTIFICATION_TASKS {
            tracing::warn!(
                mob_id = %self.definition.id,
                pending = self.lifecycle_tasks.len(),
                "lifecycle notification dropped: task limit reached"
            );
            return;
        }

        let provisioner = self.provisioner.clone();
        let member_ref = orchestrator_entry.member_ref;
        let runtime_mode = orchestrator_entry.runtime_mode;
        let meerkat_id = orchestrator_entry.meerkat_id;
        self.lifecycle_tasks.spawn(async move {
            let result = match runtime_mode {
                crate::MobRuntimeMode::AutonomousHost => {
                    let Some(session_id) = member_ref.session_id() else {
                        return;
                    };
                    let Some(injector) = provisioner.event_injector(session_id).await else {
                        return;
                    };
                    injector
                        .inject(message, meerkat_core::PlainEventSource::Rpc)
                        .map_err(|error| {
                            MobError::Internal(format!(
                                "orchestrator lifecycle inject failed for '{meerkat_id}': {error}"
                            ))
                        })
                }
                crate::MobRuntimeMode::TurnDriven => {
                    provisioner
                        .start_turn(
                            &member_ref,
                            meerkat_core::service::StartTurnRequest {
                                prompt: message,
                                event_tx: None,
                                host_mode: false,
                                skill_references: None,
                                flow_tool_overlay: None,
                                additional_instructions: None,
                            },
                        )
                        .await
                }
            };
            if let Err(error) = result {
                tracing::warn!(
                    orchestrator_member_ref = ?member_ref,
                    error = %error,
                    "failed to notify orchestrator lifecycle turn"
                );
            }
        });
    }

    fn retire_event_key(meerkat_id: &MeerkatId, member_ref: &MemberRef) -> String {
        let member =
            serde_json::to_string(member_ref).unwrap_or_else(|_| format!("{member_ref:?}"));
        format!("{meerkat_id}|{member}")
    }

    async fn stop_mcp_servers(&self) -> Result<(), MobError> {
        let mut servers = self.mcp_servers.lock().await;
        #[cfg(not(target_arch = "wasm32"))]
        let mut first_error: Option<MobError> = None;
        for (_name, entry) in servers.iter_mut() {
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(child) = entry.process.as_mut() {
                if let Err(error) = child.kill().await {
                    let mob_error =
                        MobError::Internal(format!("failed to stop mcp server '{_name}': {error}"));
                    tracing::warn!(error = %mob_error, "mcp server kill failed");
                    if first_error.is_none() {
                        first_error = Some(mob_error);
                    }
                }
                if let Err(error) = child.wait().await {
                    let mob_error = MobError::Internal(format!(
                        "failed waiting for mcp server '{_name}' to exit: {error}"
                    ));
                    tracing::warn!(error = %mob_error, "mcp server wait failed");
                    if first_error.is_none() {
                        first_error = Some(mob_error);
                    }
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                entry.process = None;
            }
            entry.running = false;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(())
    }

    async fn start_mcp_servers(&self) -> Result<(), MobError> {
        let mut servers = self.mcp_servers.lock().await;
        for (name, cfg) in &self.definition.mcp_servers {
            if cfg.command.is_empty() {
                continue;
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                if servers
                    .get(name)
                    .is_some_and(|entry| entry.process.is_some())
                {
                    continue;
                }
                let mut cmd = Command::new(&cfg.command[0]);
                for arg in cfg.command.iter().skip(1) {
                    cmd.arg(arg);
                }
                for (k, v) in &cfg.env {
                    cmd.env(k, v);
                }
                let child = cmd.spawn().map_err(|error| {
                    MobError::Internal(format!(
                        "failed to start mcp server '{name}' command '{}': {error}",
                        cfg.command.join(" ")
                    ))
                })?;
                servers.insert(
                    name.clone(),
                    McpServerEntry {
                        process: Some(child),
                        running: true,
                    },
                );
            }
            #[cfg(target_arch = "wasm32")]
            servers.insert(name.clone(), McpServerEntry { running: true });
        }
        // Mark any servers that were already in the map but had no command
        // (i.e. URL-only servers) as running.
        for (name, entry) in servers.iter_mut() {
            if self.definition.mcp_servers.contains_key(name) {
                entry.running = true;
            }
        }
        Ok(())
    }

    async fn cleanup_namespace(&self) -> Result<(), MobError> {
        self.mcp_servers.lock().await.clear();
        Ok(())
    }

    fn fallback_spawn_prompt(&self, profile_name: &ProfileName, meerkat_id: &MeerkatId) -> String {
        format!(
            "You have been spawned as '{}' (role: {}) in mob '{}'.",
            meerkat_id, profile_name, self.definition.id
        )
    }

    fn resume_host_loop_prompt(
        &self,
        profile_name: &ProfileName,
        meerkat_id: &MeerkatId,
    ) -> String {
        format!(
            "Mob '{}' resumed autonomous host loop for '{}' (role: {}). Continue coordinated execution.",
            self.definition.id, meerkat_id, profile_name
        )
    }

    async fn start_autonomous_host_loop(
        &self,
        meerkat_id: &MeerkatId,
        member_ref: &MemberRef,
        prompt: String,
    ) -> Result<(), MobError> {
        {
            let mut loops = self.autonomous_host_loops.lock().await;
            if let Some(existing) = loops.get(meerkat_id)
                && !existing.is_finished()
            {
                return Ok(());
            }
            loops.remove(meerkat_id);
        }

        let member_ref_cloned = member_ref.clone();
        let provisioner = self.provisioner.clone();
        let loop_id = meerkat_id.clone();
        let log_id = meerkat_id.clone();
        let handle = tokio::spawn(async move {
            let result = provisioner
                .start_turn(
                    &member_ref_cloned,
                    meerkat_core::service::StartTurnRequest {
                        prompt,
                        event_tx: None,
                        host_mode: true,
                        skill_references: None,
                        flow_tool_overlay: None,
                        additional_instructions: None,
                    },
                )
                .await;
            match &result {
                Ok(()) => tracing::info!(
                    meerkat_id = %log_id,
                    "autonomous host loop exited normally"
                ),
                Err(error) => tracing::error!(
                    meerkat_id = %log_id,
                    error = %error,
                    "autonomous host loop failed"
                ),
            }
            result
        });

        tokio::task::yield_now().await;
        if handle.is_finished() {
            match handle.await {
                Ok(Ok(())) => {
                    return Err(MobError::Internal(format!(
                        "autonomous host loop for '{loop_id}' exited immediately"
                    )));
                }
                Ok(Err(error)) => return Err(error),
                Err(join_error) => {
                    return Err(MobError::Internal(format!(
                        "autonomous host loop task join failed for '{loop_id}': {join_error}"
                    )));
                }
            }
        }

        self.autonomous_host_loops
            .lock()
            .await
            .insert(meerkat_id.clone(), handle);
        Ok(())
    }

    async fn start_autonomous_host_loops_from_roster(&self) -> Result<(), MobError> {
        let entries = {
            let roster = self.roster.read().await;
            roster.list().cloned().collect::<Vec<_>>()
        };
        let autonomous_entries = entries
            .into_iter()
            .filter(|entry| entry.runtime_mode == crate::MobRuntimeMode::AutonomousHost)
            .collect::<Vec<_>>();
        if autonomous_entries.is_empty() {
            return Ok(());
        }

        let actor: &MobActor = self;
        let mut remaining = autonomous_entries.into_iter();
        let mut in_flight = FuturesUnordered::new();
        let mut first_error: Option<MobError> = None;

        for _ in 0..MAX_PARALLEL_HOST_LOOP_OPS {
            let Some(entry) = remaining.next() else {
                break;
            };
            in_flight.push(actor.start_autonomous_host_loop_for_entry(entry));
        }

        while let Some(result) = in_flight.next().await {
            if let Err(error) = result
                && first_error.is_none()
            {
                first_error = Some(error);
            }
            if let Some(entry) = remaining.next() {
                in_flight.push(actor.start_autonomous_host_loop_for_entry(entry));
            }
        }

        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(())
    }

    async fn ensure_autonomous_dispatch_capability_for_provisioner(
        provisioner: &Arc<dyn MobProvisioner>,
        meerkat_id: &MeerkatId,
        member_ref: &MemberRef,
    ) -> Result<(), MobError> {
        let session_id = member_ref.session_id().ok_or_else(|| {
            MobError::Internal(format!(
                "autonomous member '{meerkat_id}' must be session-backed for injector dispatch"
            ))
        })?;
        if provisioner.event_injector(session_id).await.is_none() {
            return Err(MobError::Internal(format!(
                "autonomous member '{meerkat_id}' is missing event injector capability"
            )));
        }
        Ok(())
    }

    async fn ensure_autonomous_dispatch_capability(
        &self,
        meerkat_id: &MeerkatId,
        member_ref: &MemberRef,
    ) -> Result<(), MobError> {
        Self::ensure_autonomous_dispatch_capability_for_provisioner(
            &self.provisioner,
            meerkat_id,
            member_ref,
        )
        .await
    }

    async fn stop_autonomous_host_loop_for_member(
        &self,
        meerkat_id: &MeerkatId,
        member_ref: &MemberRef,
    ) -> Result<(), MobError> {
        if let Err(error) = self.provisioner.interrupt_member(member_ref).await
            && !matches!(
                error,
                MobError::SessionError(meerkat_core::service::SessionError::NotFound { .. })
            )
        {
            return Err(error);
        }
        if let Some(handle) = self.autonomous_host_loops.lock().await.remove(meerkat_id) {
            handle.abort();
        }
        // Ensure stop semantics are strong: do not report completion while the
        // session still appears active, otherwise immediate resume can race into
        // SessionError::Busy.
        let mut still_active = false;
        for _ in 0..40 {
            match self.provisioner.is_member_active(member_ref).await? {
                Some(true) => tokio::time::sleep(std::time::Duration::from_millis(25)).await,
                _ => {
                    still_active = false;
                    break;
                }
            }
            still_active = true;
        }
        if still_active {
            tracing::warn!(
                mob_id = %self.definition.id,
                meerkat_id = %meerkat_id,
                "autonomous host loop stop polling exhausted before member became idle"
            );
        }
        Ok(())
    }

    async fn stop_all_autonomous_host_loops(&self) -> Result<(), MobError> {
        let entries = {
            let roster = self.roster.read().await;
            roster
                .list()
                .filter(|entry| entry.runtime_mode == crate::MobRuntimeMode::AutonomousHost)
                .cloned()
                .collect::<Vec<_>>()
        };
        if entries.is_empty() {
            return Ok(());
        }
        let actor: &MobActor = self;
        let mut remaining = entries.into_iter();
        let mut in_flight = FuturesUnordered::new();
        let mut first_error: Option<MobError> = None;

        for _ in 0..MAX_PARALLEL_HOST_LOOP_OPS {
            let Some(entry) = remaining.next() else {
                break;
            };
            in_flight.push(actor.stop_autonomous_host_loop_for_entry(entry));
        }

        while let Some(result) = in_flight.next().await {
            if let Err((meerkat_id, error)) = result {
                tracing::warn!(
                    meerkat_id = %meerkat_id,
                    error = %error,
                    "failed stopping autonomous host loop member"
                );
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
            if let Some(entry) = remaining.next() {
                in_flight.push(actor.stop_autonomous_host_loop_for_entry(entry));
            }
        }

        let mut loops = self.autonomous_host_loops.lock().await;
        for (_, handle) in std::mem::take(&mut *loops) {
            handle.abort();
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(())
    }

    async fn start_autonomous_host_loop_for_entry(
        &self,
        entry: RosterEntry,
    ) -> Result<(), MobError> {
        self.ensure_autonomous_dispatch_capability(&entry.meerkat_id, &entry.member_ref)
            .await?;
        self.start_autonomous_host_loop(
            &entry.meerkat_id,
            &entry.member_ref,
            self.resume_host_loop_prompt(&entry.profile, &entry.meerkat_id),
        )
        .await
    }

    async fn stop_autonomous_host_loop_for_entry(
        &self,
        entry: RosterEntry,
    ) -> Result<(), (MeerkatId, MobError)> {
        self.stop_autonomous_host_loop_for_member(&entry.meerkat_id, &entry.member_ref)
            .await
            .map_err(|error| (entry.meerkat_id, error))
    }

    /// Main actor loop: process commands sequentially until Shutdown.
    pub(super) async fn run(mut self, mut command_rx: mpsc::Receiver<MobCommand>) {
        if matches!(self.state(), MobState::Running) {
            if let Err(error) = self.start_mcp_servers().await {
                tracing::error!(
                    mob_id = %self.definition.id,
                    error = %error,
                    "failed to start mcp servers during actor startup; entering Stopped"
                );
                if let Err(stop_error) = self.stop_all_autonomous_host_loops().await {
                    tracing::warn!(
                        mob_id = %self.definition.id,
                        error = %stop_error,
                        "failed cleaning up autonomous host loops after mcp startup error"
                    );
                }
                if let Err(stop_error) = self.stop_mcp_servers().await {
                    tracing::warn!(
                        mob_id = %self.definition.id,
                        error = %stop_error,
                        "failed cleaning up mcp servers after startup error"
                    );
                }
                self.state.store(MobState::Stopped as u8, Ordering::Release);
            } else if let Err(error) = self.start_autonomous_host_loops_from_roster().await {
                tracing::error!(
                    mob_id = %self.definition.id,
                    error = %error,
                    "failed to start autonomous host loops during actor startup; entering Stopped"
                );
                if let Err(stop_error) = self.stop_all_autonomous_host_loops().await {
                    tracing::warn!(
                        mob_id = %self.definition.id,
                        error = %stop_error,
                        "failed cleaning up autonomous host loops after startup error"
                    );
                }
                if let Err(stop_error) = self.stop_mcp_servers().await {
                    tracing::warn!(
                        mob_id = %self.definition.id,
                        error = %stop_error,
                        "failed cleaning up mcp servers after startup error"
                    );
                }
                self.state.store(MobState::Stopped as u8, Ordering::Release);
            }
        }
        let mut deferred_commands = VecDeque::new();
        loop {
            let cmd = if let Some(cmd) = deferred_commands.pop_front() {
                cmd
            } else if let Some(cmd) = command_rx.recv().await {
                cmd
            } else {
                break;
            };
            match cmd {
                MobCommand::Spawn { spec, reply_tx } => {
                    if let Err(error) = self.require_state(&[MobState::Running, MobState::Creating])
                    {
                        let _ = reply_tx.send(Err(error));
                        continue;
                    }
                    self.enqueue_spawn(spec, reply_tx).await;
                }
                MobCommand::SpawnProvisioned {
                    spawn_ticket,
                    result,
                } => {
                    let mut completions = vec![(spawn_ticket, result)];
                    loop {
                        match command_rx.try_recv() {
                            Ok(MobCommand::SpawnProvisioned {
                                spawn_ticket,
                                result,
                            }) => completions.push((spawn_ticket, result)),
                            Ok(other) => {
                                deferred_commands.push_back(other);
                                break;
                            }
                            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
                        }
                    }
                    self.handle_spawn_provisioned_batch(completions).await;
                }
                MobCommand::Retire {
                    meerkat_id,
                    reply_tx,
                } => {
                    let result = match self.require_state(&[
                        MobState::Running,
                        MobState::Creating,
                        MobState::Stopped,
                    ]) {
                        Ok(()) => self.handle_retire(meerkat_id).await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::Respawn {
                    meerkat_id,
                    initial_message,
                    reply_tx,
                } => {
                    let result = match self.require_state(&[MobState::Running, MobState::Creating])
                    {
                        Ok(()) => self.handle_respawn(meerkat_id, initial_message).await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::RetireAll { reply_tx } => {
                    let result = match self.require_state(&[
                        MobState::Running,
                        MobState::Creating,
                        MobState::Stopped,
                    ]) {
                        Ok(()) => self.retire_all_members("retire_all").await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::Wire { a, b, reply_tx } => {
                    let result = match self.require_state(&[MobState::Running, MobState::Creating])
                    {
                        Ok(()) => self.handle_wire(a, b).await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::Unwire { a, b, reply_tx } => {
                    let result = match self.require_state(&[MobState::Running, MobState::Creating])
                    {
                        Ok(()) => self.handle_unwire(a, b).await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::ExternalTurn {
                    meerkat_id,
                    message,
                    reply_tx,
                } => {
                    let result = match self.require_state(&[MobState::Running, MobState::Creating])
                    {
                        Ok(()) => self.handle_external_turn(meerkat_id, message).await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::InjectAndSubscribe {
                    meerkat_id,
                    message,
                    reply_tx,
                } => {
                    let result = match self.require_state(&[MobState::Running, MobState::Creating])
                    {
                        Ok(()) => self.handle_inject_and_subscribe(meerkat_id, message).await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::InternalTurn {
                    meerkat_id,
                    message,
                    reply_tx,
                } => {
                    let result = match self.require_state(&[MobState::Running, MobState::Creating])
                    {
                        Ok(()) => self.handle_internal_turn(meerkat_id, message).await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::RunFlow {
                    flow_id,
                    activation_params,
                    scoped_event_tx,
                    reply_tx,
                } => {
                    let result = match self.require_state(&[MobState::Running]) {
                        Ok(()) => {
                            self.handle_run_flow(flow_id, activation_params, scoped_event_tx)
                                .await
                        }
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::CancelFlow { run_id, reply_tx } => {
                    let result = match self.require_state(&[MobState::Running]) {
                        Ok(()) => self.handle_cancel_flow(run_id).await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::FlowStatus { run_id, reply_tx } => {
                    let result = self.run_store.get_run(&run_id).await;
                    let _ = reply_tx.send(result);
                }
                MobCommand::FlowFinished { run_id } => {
                    self.run_tasks.remove(&run_id);
                    self.run_cancel_tokens.remove(&run_id);
                    self.flow_streams.lock().await.remove(&run_id);
                }
                #[cfg(test)]
                MobCommand::FlowTrackerCounts { reply_tx } => {
                    let tasks = self.run_tasks.len();
                    let tokens = self.run_cancel_tokens.len();
                    let _ = reply_tx.send((tasks, tokens));
                }
                MobCommand::Stop { reply_tx } => {
                    let result = match self.expect_state(&[MobState::Running], MobState::Stopped) {
                        Ok(()) => {
                            self.fail_all_pending_spawns("mob is stopping").await;
                            self.notify_orchestrator_lifecycle(format!(
                                "Mob '{}' is stopping.",
                                self.definition.id
                            ))
                            .await;
                            // Cancel checkpointer gates before stopping host loops so
                            // in-flight saves that complete after the loop stops don't
                            // race with subsequent external cleanup (e.g. DML deletes).
                            self.provisioner.cancel_all_checkpointers().await;
                            let mut stop_result: Result<(), MobError> = Ok(());
                            let (loop_result, mcp_result) = tokio::join!(
                                self.stop_all_autonomous_host_loops(),
                                self.stop_mcp_servers()
                            );
                            if let Err(error) = loop_result {
                                tracing::warn!(
                                    mob_id = %self.definition.id,
                                    error = %error,
                                    "stop encountered autonomous loop cleanup error"
                                );
                                if stop_result.is_ok() {
                                    stop_result = Err(error);
                                }
                            }
                            if let Err(error) = mcp_result {
                                tracing::warn!(
                                    mob_id = %self.definition.id,
                                    error = %error,
                                    "stop encountered mcp cleanup error"
                                );
                                if stop_result.is_ok() {
                                    stop_result = Err(error);
                                }
                            }
                            if stop_result.is_ok() {
                                self.state.store(MobState::Stopped as u8, Ordering::Release);
                            } else {
                                // Restore checkpointer state — mob stays Running.
                                self.provisioner.rearm_all_checkpointers().await;
                            }
                            stop_result
                        }
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::ResumeLifecycle { reply_tx } => {
                    let result = match self.expect_state(&[MobState::Stopped], MobState::Running) {
                        Ok(()) => {
                            // Re-enable checkpointers cancelled during stop.
                            self.provisioner.rearm_all_checkpointers().await;
                            if let Err(error) = self.start_mcp_servers().await {
                                if let Err(stop_error) = self.stop_mcp_servers().await {
                                    tracing::warn!(
                                        mob_id = %self.definition.id,
                                        error = %stop_error,
                                        "resume cleanup failed while stopping mcp servers"
                                    );
                                }
                                Err(error)
                            } else if let Err(error) =
                                self.start_autonomous_host_loops_from_roster().await
                            {
                                if let Err(stop_error) = self.stop_all_autonomous_host_loops().await
                                {
                                    tracing::warn!(
                                        mob_id = %self.definition.id,
                                        error = %stop_error,
                                        "resume cleanup failed while stopping autonomous loops"
                                    );
                                }
                                if let Err(stop_error) = self.stop_mcp_servers().await {
                                    tracing::warn!(
                                        mob_id = %self.definition.id,
                                        error = %stop_error,
                                        "resume cleanup failed while stopping mcp servers"
                                    );
                                }
                                Err(error)
                            } else {
                                self.state.store(MobState::Running as u8, Ordering::Release);
                                Ok(())
                            }
                        }
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::Complete { reply_tx } => {
                    let result = match self.expect_state(&[MobState::Running], MobState::Completed)
                    {
                        Ok(()) => {
                            self.fail_all_pending_spawns("mob is completing").await;
                            self.handle_complete().await
                        }
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::Destroy { reply_tx } => {
                    let result = match self.state() {
                        MobState::Running | MobState::Stopped | MobState::Completed => {
                            self.fail_all_pending_spawns("mob is destroying").await;
                            self.handle_destroy().await
                        }
                        current => Err(MobError::InvalidTransition {
                            from: current,
                            to: MobState::Destroyed,
                        }),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::Reset { reply_tx } => {
                    let result = match self.state() {
                        MobState::Running | MobState::Stopped | MobState::Completed => {
                            self.fail_all_pending_spawns("mob is resetting").await;
                            self.handle_reset().await
                        }
                        current => Err(MobError::InvalidTransition {
                            from: current,
                            to: MobState::Running,
                        }),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::TaskCreate {
                    subject,
                    description,
                    blocked_by,
                    reply_tx,
                } => {
                    let result = match self.require_state(&[MobState::Running, MobState::Creating])
                    {
                        Ok(()) => {
                            self.handle_task_create(subject, description, blocked_by)
                                .await
                        }
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::TaskUpdate {
                    task_id,
                    status,
                    owner,
                    reply_tx,
                } => {
                    let result = match self.require_state(&[MobState::Running, MobState::Creating])
                    {
                        Ok(()) => self.handle_task_update(task_id, status, owner).await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::SetSpawnPolicy { policy, reply_tx } => {
                    self.spawn_policy = policy;
                    let _ = reply_tx.send(());
                }
                MobCommand::Shutdown { reply_tx } => {
                    self.fail_all_pending_spawns("mob runtime is shutting down")
                        .await;
                    self.cancel_all_flow_tasks().await;
                    let mut result: Result<(), MobError> = Ok(());
                    if let Err(error) = self.stop_all_autonomous_host_loops().await {
                        tracing::warn!(error = %error, "shutdown loop stop encountered errors");
                        if result.is_ok() {
                            result = Err(error);
                        }
                    }
                    if let Err(error) = self.stop_mcp_servers().await {
                        tracing::warn!(error = %error, "shutdown mcp stop encountered errors");
                        if result.is_ok() {
                            result = Err(error);
                        }
                    }
                    // Cancel remaining lifecycle notification tasks.
                    // abort_all is non-blocking; join_next drains the abort results.
                    self.lifecycle_tasks.abort_all();
                    while self.lifecycle_tasks.join_next().await.is_some() {}
                    self.state.store(MobState::Stopped as u8, Ordering::Release);
                    let _ = reply_tx.send(result);
                    break;
                }
            }
        }
    }

    async fn fail_all_pending_spawns(&mut self, reason: &str) {
        if self.pending_spawns.is_empty() {
            return;
        }

        for (spawn_ticket, pending) in std::mem::take(&mut self.pending_spawns) {
            self.pending_spawn_ids.remove(&pending.meerkat_id);
            let _ = pending.reply_tx.send(Err(MobError::Internal(format!(
                "spawn canceled for '{}': {}",
                pending.meerkat_id, reason
            ))));
            tracing::debug!(
                spawn_ticket,
                meerkat_id = %pending.meerkat_id,
                "failed pending spawn due to lifecycle transition"
            );
        }
    }

    /// P1-T04: spawn() creates a real session.
    ///
    /// Provisioning runs in parallel tasks; final actor commit stays serialized.
    async fn enqueue_spawn(
        &mut self,
        spec: super::handle::SpawnMemberSpec,
        reply_tx: oneshot::Sender<Result<MemberRef, MobError>>,
    ) {
        let super::handle::SpawnMemberSpec {
            profile_name,
            meerkat_id,
            initial_message,
            runtime_mode,
            backend,
            context,
            labels,
            resume_session_id,
        } = spec;
        let prepare_result = async {
            if meerkat_id
                .as_str()
                .starts_with(FLOW_SYSTEM_MEMBER_ID_PREFIX)
            {
                return Err(MobError::WiringError(format!(
                    "meerkat id '{meerkat_id}' uses reserved system prefix '{FLOW_SYSTEM_MEMBER_ID_PREFIX}'"
                )));
            }
            tracing::debug!(
                mob_id = %self.definition.id,
                meerkat_id = %meerkat_id,
                profile = %profile_name,
                "MobActor::enqueue_spawn start"
            );

            if self.pending_spawn_ids.contains(&meerkat_id) {
                return Err(MobError::MeerkatAlreadyExists(meerkat_id.clone()));
            }

            {
                let roster = self.roster.read().await;
                if roster.get(&meerkat_id).is_some() {
                    return Err(MobError::MeerkatAlreadyExists(meerkat_id.clone()));
                }
            }

            let profile = self
                .definition
                .profiles
                .get(&profile_name)
                .ok_or_else(|| MobError::ProfileNotFound(profile_name.clone()))?;

            let selected_runtime_mode = runtime_mode.unwrap_or(profile.runtime_mode);

            // ---------- Resume session fast-path ----------
            // When resume_session_id is set, skip provisioning and go straight
            // to finalization. The session must already exist and be usable.
            if let Some(resume_id) = resume_session_id {
                let member_ref = MemberRef::from_session_id(resume_id.clone());

                // Validate the session exists and is active.
                let is_active = self
                    .provisioner
                    .is_member_active(&member_ref)
                    .await
                    .map_err(|e| {
                        MobError::Internal(format!(
                            "resume session check failed for '{meerkat_id}': {e}"
                        ))
                    })?;
                if !is_active.unwrap_or(false) {
                    return Err(MobError::Internal(
                        format!("resumed session '{resume_id}' not found or inactive for '{meerkat_id}'"),
                    ));
                }

                // Validate event injector for autonomous mode.
                if selected_runtime_mode == crate::MobRuntimeMode::AutonomousHost
                    && self.provisioner.event_injector(&resume_id).await.is_none()
                {
                    return Err(MobError::Internal(format!(
                        "resumed session '{resume_id}' has no event injector for autonomous '{meerkat_id}'"
                    )));
                }

                // Validate comms if wiring rules exist.
                let has_wiring = self.definition.wiring.auto_wire_orchestrator
                    || !self.definition.wiring.role_wiring.is_empty();
                if has_wiring
                    && self
                        .provisioner
                        .comms_runtime(&member_ref)
                        .await
                        .is_none()
                {
                    return Err(MobError::Internal(format!(
                        "resumed session '{resume_id}' has no comms runtime for '{meerkat_id}'"
                    )));
                }

                let prompt = initial_message
                    .clone()
                    .unwrap_or_else(|| self.fallback_spawn_prompt(&profile_name, &meerkat_id));
                let resolved_labels = labels.unwrap_or_default();

                return Ok((
                    profile_name,
                    meerkat_id,
                    prompt,
                    selected_runtime_mode,
                    resolved_labels,
                    Some(member_ref),
                    None,
                ));
            }

            let external_tools = self.external_tools_for_profile(profile)?;
            let mut config = build::build_agent_config(build::BuildAgentConfigParams {
                mob_id: &self.definition.id,
                profile_name: &profile_name,
                meerkat_id: &meerkat_id,
                profile,
                definition: &self.definition,
                external_tools,
                context,
                labels: labels.clone(),
            })
            .await?;
            if let Some(ref client) = self.default_llm_client {
                config.llm_client_override = Some(client.clone());
            }

            let prompt = initial_message
                .clone()
                .unwrap_or_else(|| self.fallback_spawn_prompt(&profile_name, &meerkat_id));
            let req = build::to_create_session_request(&config, prompt.clone());
            let selected_backend = backend
                .or(profile.backend)
                .unwrap_or(self.definition.backend.default);
            let peer_name = format!("{}/{}/{}", self.definition.id, profile_name, meerkat_id);
            let provision_request = ProvisionMemberRequest {
                create_session: req,
                backend: selected_backend,
                peer_name,
            };
            let resolved_labels = labels.unwrap_or_default();
            Ok((
                profile_name,
                meerkat_id,
                prompt,
                selected_runtime_mode,
                resolved_labels,
                None::<MemberRef>,
                Some(provision_request),
            ))
        }
        .await;

        let (
            profile_name,
            meerkat_id,
            prompt,
            selected_runtime_mode,
            resolved_labels,
            resume_member_ref,
            maybe_provision_request,
        ) = match prepare_result {
            Ok(prepared) => prepared,
            Err(error) => {
                let _ = reply_tx.send(Err(error));
                return;
            }
        };

        // ---------- Resume fast-path: skip async provisioning ----------
        if let Some(member_ref) = resume_member_ref {
            let provision =
                PendingProvision::new(member_ref, meerkat_id.clone(), self.provisioner.clone());
            // Go straight to finalization — no async provisioning task needed.
            let result = self
                .finalize_spawn_from_pending(
                    &profile_name,
                    &meerkat_id,
                    selected_runtime_mode,
                    prompt,
                    resolved_labels,
                    provision,
                )
                .await;
            let _ = reply_tx.send(result);
            return;
        }

        // Normal provisioning path — resume path already returned above.
        let Some(provision_request) = maybe_provision_request else {
            let _ = reply_tx.send(Err(MobError::Internal(
                "provision_request missing for normal spawn path".into(),
            )));
            return;
        };

        let pending = PendingSpawn {
            profile_name,
            meerkat_id,
            prompt,
            runtime_mode: selected_runtime_mode,
            labels: resolved_labels,
            reply_tx,
        };

        let spawn_ticket = self.next_spawn_ticket;
        self.next_spawn_ticket = self.next_spawn_ticket.wrapping_add(1);
        let spawn_meerkat_id = pending.meerkat_id.clone();
        let spawn_runtime_mode = pending.runtime_mode;

        self.pending_spawn_ids.insert(spawn_meerkat_id.clone());
        self.pending_spawns.insert(spawn_ticket, pending);

        tracing::debug!(
            spawn_ticket,
            meerkat_id = %spawn_meerkat_id,
            runtime_mode = ?spawn_runtime_mode,
            "MobActor::enqueue_spawn queued provisioning task"
        );

        let provisioner = self.provisioner.clone();
        let command_tx = self.command_tx.clone();
        let task = tokio::spawn(async move {
            let panic_meerkat_id = spawn_meerkat_id.clone();
            let provision_result = std::panic::AssertUnwindSafe(async {
                let member_ref = provisioner.provision_member(provision_request).await?;
                if spawn_runtime_mode == crate::MobRuntimeMode::AutonomousHost
                    && let Err(capability_error) =
                        Self::ensure_autonomous_dispatch_capability_for_provisioner(
                            &provisioner,
                            &spawn_meerkat_id,
                            &member_ref,
                        )
                        .await
                {
                    if let Err(retire_error) = provisioner.retire_member(&member_ref).await {
                        return Err(MobError::Internal(format!(
                            "autonomous capability check failed for '{spawn_meerkat_id}': {capability_error}; cleanup retire failed for member '{member_ref:?}': {retire_error}"
                        )));
                    }
                    return Err(capability_error);
                }
                Ok(member_ref)
            })
            .catch_unwind()
            .await;
            let provision_result = match provision_result {
                Ok(result) => result,
                Err(_) => Err(MobError::Internal(format!(
                    "spawn provisioning task panicked for '{panic_meerkat_id}'"
                ))),
            };

            if let Err(send_error) = command_tx
                .send(MobCommand::SpawnProvisioned {
                    spawn_ticket,
                    result: provision_result,
                })
                .await
                && let MobCommand::SpawnProvisioned {
                    result: Ok(member_ref),
                    ..
                } = send_error.0
                && let Err(cleanup_error) = provisioner.retire_member(&member_ref).await
            {
                tracing::warn!(
                    spawn_ticket,
                    member_ref = ?member_ref,
                    error = %cleanup_error,
                    "spawn completion dropped; failed cleanup retire for provisioned member"
                );
            }
        });
        self.pending_spawn_tasks.insert(spawn_ticket, task);
    }

    async fn handle_spawn_provisioned_batch(
        &mut self,
        completions: Vec<(u64, Result<MemberRef, MobError>)>,
    ) {
        let mut pending_items = Vec::with_capacity(completions.len());
        for (spawn_ticket, result) in completions {
            self.pending_spawn_tasks.remove(&spawn_ticket);
            let Some(pending) = self.pending_spawns.remove(&spawn_ticket) else {
                tracing::warn!(spawn_ticket, "received spawn completion for unknown ticket");
                if let Ok(member_ref) = result {
                    let orphan = PendingProvision::new(
                        member_ref,
                        MeerkatId::from("__unknown_ticket__"),
                        self.provisioner.clone(),
                    );
                    if let Err(error) = orphan.rollback().await {
                        tracing::warn!(
                            spawn_ticket,
                            error = %error,
                            "unknown spawn completion cleanup failed"
                        );
                    }
                }
                continue;
            };
            self.pending_spawn_ids.remove(&pending.meerkat_id);
            pending_items.push((pending, result));
        }

        let mut in_flight = FuturesUnordered::new();
        let actor: &MobActor = self;
        for (pending, result) in pending_items {
            let PendingSpawn {
                profile_name,
                meerkat_id,
                prompt,
                runtime_mode,
                labels,
                reply_tx,
            } = pending;
            in_flight.push(async move {
                let reply = match result {
                    Ok(member_ref) => {
                        let provision = PendingProvision::new(
                            member_ref,
                            meerkat_id.clone(),
                            actor.provisioner.clone(),
                        );
                        if let Err(error) = actor
                            .require_state(&[MobState::Running, MobState::Creating])
                        {
                            if let Err(retire_error) = provision.rollback().await {
                                Err(MobError::Internal(format!(
                                    "spawn completed while mob state changed for '{meerkat_id}': {error}; cleanup retire failed: {retire_error}"
                                )))
                            } else {
                                Err(error)
                            }
                        } else {
                            actor.finalize_spawn_from_pending(
                                &profile_name,
                                &meerkat_id,
                                runtime_mode,
                                prompt,
                                labels,
                                provision,
                            )
                            .await
                        }
                    }
                    Err(error) => Err(error),
                };
                (reply_tx, reply)
            });
        }

        while let Some((reply_tx, reply)) = in_flight.next().await {
            let _ = reply_tx.send(reply);
        }
    }

    async fn finalize_spawn_from_pending(
        &self,
        profile_name: &ProfileName,
        meerkat_id: &MeerkatId,
        runtime_mode: crate::MobRuntimeMode,
        prompt: String,
        labels: std::collections::BTreeMap<String, String>,
        provision: PendingProvision,
    ) -> Result<MemberRef, MobError> {
        if let Err(append_error) = self
            .events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MeerkatSpawned {
                    meerkat_id: meerkat_id.clone(),
                    role: profile_name.clone(),
                    runtime_mode,
                    member_ref: provision.member_ref().clone(),
                    labels: labels.clone(),
                },
            })
            .await
        {
            if let Err(rollback_error) = provision.rollback().await {
                return Err(MobError::Internal(format!(
                    "spawn append failed for '{meerkat_id}': {append_error}; archive compensation failed: {rollback_error}"
                )));
            }
            return Err(append_error);
        }

        // Commit the provision: the member is now owned by the roster.
        // From this point, rollback_failed_spawn handles cleanup via the
        // disposal pipeline.
        let member_ref = provision.commit()?;

        {
            let mut roster = self.roster.write().await;
            let inserted = roster.add(crate::roster::RosterAddEntry {
                meerkat_id: meerkat_id.clone(),
                profile: profile_name.clone(),
                runtime_mode,
                member_ref: member_ref.clone(),
                labels,
            });
            debug_assert!(
                inserted,
                "duplicate meerkat insert should be prevented before add()"
            );
        }
        tracing::debug!(
            meerkat_id = %meerkat_id,
            "MobActor::finalize_spawn_from_pending roster updated"
        );

        let planned_wiring_targets = self.spawn_wiring_targets(profile_name, meerkat_id).await;

        if let Err(wiring_error) = self
            .apply_spawn_wiring(meerkat_id, &planned_wiring_targets)
            .await
        {
            if let Err(rollback_error) = self
                .rollback_failed_spawn(
                    meerkat_id,
                    profile_name,
                    &member_ref,
                    &planned_wiring_targets,
                )
                .await
            {
                return Err(MobError::Internal(format!(
                    "spawn wiring failed for '{meerkat_id}': {wiring_error}; rollback failed: {rollback_error}"
                )));
            }
            return Err(wiring_error);
        }

        if runtime_mode == crate::MobRuntimeMode::AutonomousHost
            && let Err(start_error) = self
                .start_autonomous_host_loop(meerkat_id, &member_ref, prompt)
                .await
        {
            if let Err(rollback_error) = self
                .rollback_failed_spawn(
                    meerkat_id,
                    profile_name,
                    &member_ref,
                    &planned_wiring_targets,
                )
                .await
            {
                return Err(MobError::Internal(format!(
                    "spawn host-loop start failed for '{meerkat_id}': {start_error}; rollback failed: {rollback_error}"
                )));
            }
            return Err(start_error);
        }

        tracing::debug!(
            meerkat_id = %meerkat_id,
            "MobActor::finalize_spawn_from_pending done"
        );
        Ok(member_ref.clone())
    }

    async fn spawn_wiring_targets(
        &self,
        profile_name: &ProfileName,
        meerkat_id: &MeerkatId,
    ) -> Vec<MeerkatId> {
        let mut targets = Vec::new();

        if self.definition.wiring.auto_wire_orchestrator
            && let Some(orchestrator) = &self.definition.orchestrator
            && profile_name != &orchestrator.profile
        {
            let orchestrator_ids = {
                let roster = self.roster.read().await;
                roster
                    .by_profile(&orchestrator.profile)
                    .map(|entry| entry.meerkat_id.clone())
                    .collect::<Vec<_>>()
            };
            for orchestrator_id in orchestrator_ids {
                if orchestrator_id != *meerkat_id && !targets.contains(&orchestrator_id) {
                    targets.push(orchestrator_id);
                }
            }
        }

        for rule in &self.definition.wiring.role_wiring {
            let target_profile = if &rule.a == profile_name {
                Some(&rule.b)
            } else if &rule.b == profile_name {
                Some(&rule.a)
            } else {
                None
            };
            if let Some(target_profile) = target_profile {
                let target_ids = {
                    let roster = self.roster.read().await;
                    roster
                        .by_profile(target_profile)
                        .filter(|entry| entry.meerkat_id != *meerkat_id)
                        .map(|entry| entry.meerkat_id.clone())
                        .collect::<Vec<_>>()
                };
                for target_id in target_ids {
                    if !targets.contains(&target_id) {
                        targets.push(target_id);
                    }
                }
            }
        }

        targets
    }

    /// P1-T05: retire() removes a meerkat.
    ///
    /// Mark-then-best-effort-cleanup: event first, mark Retiring, disposal
    /// pipeline (policy-driven), then unconditional roster removal.
    async fn handle_retire(&self, meerkat_id: MeerkatId) -> Result<(), MobError> {
        self.handle_retire_inner(&meerkat_id, false).await
    }

    async fn handle_retire_inner(
        &self,
        meerkat_id: &MeerkatId,
        bulk: bool,
    ) -> Result<(), MobError> {
        // Idempotent: already retired / never existed is success.
        let entry = {
            let roster = self.roster.read().await;
            let Some(entry) = roster.get(meerkat_id).cloned() else {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    meerkat_id = %meerkat_id,
                    "retire requested for unknown meerkat id"
                );
                return Ok(());
            };
            entry
        };

        // Append retire event (event-first for crash recovery).
        let retire_event_already_present = self
            .retire_event_exists(meerkat_id, &entry.member_ref)
            .await?;
        if !retire_event_already_present {
            self.append_retire_event(meerkat_id, &entry.profile, &entry.member_ref)
                .await?;
        }

        // Mark as Retiring (blocks re-spawn with same ID).
        {
            let mut roster = self.roster.write().await;
            roster.mark_retiring(meerkat_id);
        }

        // Snapshot context and run disposal pipeline.
        let ctx = self.disposal_context_from_entry(meerkat_id, &entry).await;
        let mut policy: Box<dyn ErrorPolicy> = if bulk {
            Box::new(BulkBestEffort)
        } else {
            Box::new(WarnAndContinue)
        };
        self.dispose_member(&ctx, policy.as_mut()).await;

        Ok(())
    }

    /// Retire a member and re-spawn it with the same profile (non-blocking).
    ///
    /// Retire completes synchronously within this call. Spawn is enqueued
    /// asynchronously — the new member becomes available when
    /// `MeerkatSpawned` is emitted. Returns `Ok(())` once retire completes
    /// and spawn is enqueued.
    async fn handle_respawn(
        &mut self,
        meerkat_id: MeerkatId,
        initial_message: Option<String>,
    ) -> Result<(), MobError> {
        // Capture profile before retire removes entry.
        let profile_name = {
            let roster = self.roster.read().await;
            let entry = roster
                .get(&meerkat_id)
                .ok_or_else(|| MobError::MeerkatNotFound(meerkat_id.clone()))?;
            entry.profile.clone()
        };

        // Full retire (disposal pipeline).
        self.handle_retire(meerkat_id.clone()).await?;

        // Enqueue async spawn with same profile and meerkat ID.
        // The spawn result is delivered via MeerkatSpawned event.
        let (spawn_reply_tx, spawn_reply_rx) = oneshot::channel();
        self.enqueue_spawn(
            super::handle::SpawnMemberSpec {
                profile_name,
                meerkat_id: meerkat_id.clone(),
                initial_message,
                runtime_mode: None,
                backend: None,
                context: None,
                labels: None,
                resume_session_id: None,
            },
            spawn_reply_tx,
        )
        .await;

        // Fire-and-forget: log spawn failures but don't block the caller.
        let mid = meerkat_id.clone();
        self.lifecycle_tasks.spawn(async move {
            match spawn_reply_rx.await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => {
                    tracing::error!(meerkat_id = %mid, error = %e, "respawn: spawn failed after retire");
                }
                Err(_) => {
                    tracing::error!(meerkat_id = %mid, "respawn: spawn reply dropped");
                }
            }
        });

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Disposal pipeline
    // -----------------------------------------------------------------------

    /// Snapshot member state for disposal from a roster entry.
    async fn disposal_context_from_entry(
        &self,
        meerkat_id: &MeerkatId,
        entry: &RosterEntry,
    ) -> DisposalContext {
        let retiring_comms = self.provisioner_comms(&entry.member_ref).await;
        let retiring_key = retiring_comms.as_ref().and_then(|comms| comms.public_key());
        DisposalContext {
            meerkat_id: meerkat_id.clone(),
            entry: entry.clone(),
            retiring_comms,
            retiring_key,
        }
    }

    /// Execute the disposal pipeline for a member.
    ///
    /// Runs policy-driven steps in order, then unconditionally removes the
    /// member from the roster and prunes wire edge locks. The finally block
    /// runs regardless of whether the policy aborted.
    async fn dispose_member(
        &self,
        ctx: &DisposalContext,
        policy: &mut dyn ErrorPolicy,
    ) -> DisposalReport {
        let mut report = DisposalReport::new();

        for &step in &DisposalStep::ORDERED {
            match self.execute_step(step, ctx).await {
                Ok(()) => report.completed.push(step),
                Err(error) => {
                    if policy.on_step_error(step, &error, ctx) {
                        report.skipped.push((step, error));
                    } else {
                        report.aborted_at = Some((step, error));
                        break;
                    }
                }
            }
        }

        // Finally: unconditional, outside policy control.
        self.dispose_prune_edge_locks(ctx).await;
        self.dispose_remove_from_roster(ctx).await;
        report.roster_removed = true;
        report
    }

    /// Dispatch a disposal step. Exhaustive match ensures compiler forces new
    /// arms when `DisposalStep` variants are added.
    async fn execute_step(
        &self,
        step: DisposalStep,
        ctx: &DisposalContext,
    ) -> Result<(), MobError> {
        match step {
            DisposalStep::StopHostLoop => self.dispose_stop_host_loop(ctx).await,
            DisposalStep::NotifyPeers => self.dispose_notify_peers(ctx).await,
            DisposalStep::RemoveTrustEdges => self.dispose_remove_trust_edges(ctx).await,
            DisposalStep::ArchiveSession => self.dispose_archive_session(ctx).await,
        }
    }

    /// Stop the autonomous host loop if the member is in AutonomousHost mode.
    async fn dispose_stop_host_loop(&self, ctx: &DisposalContext) -> Result<(), MobError> {
        if ctx.entry.runtime_mode == crate::MobRuntimeMode::AutonomousHost {
            self.stop_autonomous_host_loop_for_member(&ctx.meerkat_id, &ctx.entry.member_ref)
                .await?;
        }
        Ok(())
    }

    /// Notify all wired peers that this member is retiring.
    ///
    /// Iterates the full `wired_to` set internally; skips absent peers.
    /// Returns the first error encountered, if any.
    async fn dispose_notify_peers(&self, ctx: &DisposalContext) -> Result<(), MobError> {
        let Some(retiring_comms) = &ctx.retiring_comms else {
            return Ok(());
        };
        let mut first_error: Option<MobError> = None;
        for peer_id in &ctx.entry.wired_to {
            // Skip absent peers (already retired).
            let peer_present = {
                let roster = self.roster.read().await;
                roster.get(peer_id).is_some()
            };
            if !peer_present {
                tracing::debug!(
                    mob_id = %self.definition.id,
                    meerkat_id = %ctx.meerkat_id,
                    peer_id = %peer_id,
                    "dispose_notify_peers: skipping absent peer"
                );
                continue;
            }

            if let Err(error) = self
                .notify_peer_retired(peer_id, &ctx.meerkat_id, &ctx.entry, retiring_comms)
                .await
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Remove the retiring member's trust edges from all wired peers.
    ///
    /// Iterates the full `wired_to` set; skips absent peers and peers
    /// missing comms.
    async fn dispose_remove_trust_edges(&self, ctx: &DisposalContext) -> Result<(), MobError> {
        let Some(retiring_key) = &ctx.retiring_key else {
            return Ok(());
        };
        let mut first_error: Option<MobError> = None;
        for peer_id in &ctx.entry.wired_to {
            let peer_member_ref = {
                let roster = self.roster.read().await;
                roster.get(peer_id).map(|e| e.member_ref.clone())
            };
            let Some(peer_member_ref) = peer_member_ref else {
                tracing::debug!(
                    mob_id = %self.definition.id,
                    meerkat_id = %ctx.meerkat_id,
                    peer_id = %peer_id,
                    "dispose_remove_trust_edges: skipping absent peer"
                );
                continue;
            };
            let Some(peer_comms) = self.provisioner_comms(&peer_member_ref).await else {
                continue;
            };
            if let Err(error) = peer_comms.remove_trusted_peer(retiring_key).await
                && first_error.is_none()
            {
                first_error = Some(error.into());
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Archive the member's session. Treats NotFound as success.
    pub(super) async fn dispose_archive_session(
        &self,
        ctx: &DisposalContext,
    ) -> Result<(), MobError> {
        if let Err(error) = self.provisioner.retire_member(&ctx.entry.member_ref).await {
            if matches!(
                error,
                MobError::SessionError(meerkat_core::service::SessionError::NotFound { .. })
            ) {
                return Ok(());
            }
            return Err(error);
        }
        Ok(())
    }

    /// Prune edge locks for the member. Infallible.
    async fn dispose_prune_edge_locks(&self, ctx: &DisposalContext) {
        self.edge_locks.prune(ctx.meerkat_id.as_str()).await;
    }

    /// Remove the member from the roster. Infallible.
    pub(super) async fn dispose_remove_from_roster(&self, ctx: &DisposalContext) {
        let mut roster = self.roster.write().await;
        roster.remove(&ctx.meerkat_id);
    }

    /// P1-T06: wire() establishes bidirectional trust.
    async fn handle_wire(&self, a: MeerkatId, b: MeerkatId) -> Result<(), MobError> {
        // Verify both exist
        {
            let roster = self.roster.read().await;
            if roster.get(&a).is_none() {
                return Err(MobError::MeerkatNotFound(a.clone()));
            }
            if roster.get(&b).is_none() {
                return Err(MobError::MeerkatNotFound(b.clone()));
            }
        }

        self.do_wire(&a, &b).await
    }

    /// P1-T07: unwire() removes bidirectional trust.
    async fn handle_unwire(&self, a: MeerkatId, b: MeerkatId) -> Result<(), MobError> {
        // Look up both entries
        let (entry_a, entry_b) = {
            let roster = self.roster.read().await;
            let ea = roster
                .get(&a)
                .cloned()
                .ok_or_else(|| MobError::MeerkatNotFound(a.clone()))?;
            let eb = roster
                .get(&b)
                .cloned()
                .ok_or_else(|| MobError::MeerkatNotFound(b.clone()))?;
            (ea, eb)
        };

        // Get comms and keys for both sides (required for unwire).
        let comms_a = self
            .provisioner_comms(&entry_a.member_ref)
            .await
            .ok_or_else(|| {
                MobError::WiringError(format!("unwire requires comms runtime for '{a}'"))
            })?;
        let comms_b = self
            .provisioner_comms(&entry_b.member_ref)
            .await
            .ok_or_else(|| {
                MobError::WiringError(format!("unwire requires comms runtime for '{b}'"))
            })?;
        let key_a = comms_a.public_key().ok_or_else(|| {
            MobError::WiringError(format!("unwire requires public key for '{a}'"))
        })?;
        let key_b = comms_b.public_key().ok_or_else(|| {
            MobError::WiringError(format!("unwire requires public key for '{b}'"))
        })?;
        let comms_name_a = self.comms_name_for(&entry_a);
        let comms_name_b = self.comms_name_for(&entry_b);
        let spec_a = self
            .provisioner
            .trusted_peer_spec(&entry_a.member_ref, &comms_name_a, &key_a)
            .await?;
        let spec_b = self
            .provisioner
            .trusted_peer_spec(&entry_b.member_ref, &comms_name_b, &key_b)
            .await?;
        let mut rollback = LifecycleRollback::new("unwire");

        // Notify both peers BEFORE removing trust (need trust to send).
        // Send FROM a TO b: notify b that a is being unwired
        self.notify_peer_unwired(&b, &a, &entry_a, &comms_a).await?;
        rollback.defer(format!("compensating mob.peer_added '{a}' -> '{b}'"), {
            let comms_a = comms_a.clone();
            let comms_name_b = comms_name_b.clone();
            let a = a.clone();
            let entry_a = entry_a.clone();
            move || async move {
                self.notify_peer_added(&comms_a, &comms_name_b, &a, &entry_a)
                    .await
            }
        });
        // Send FROM b TO a: notify a that b is being unwired
        if let Err(second_notification_error) =
            self.notify_peer_unwired(&a, &b, &entry_b, &comms_b).await
        {
            return Err(rollback.fail(second_notification_error).await);
        }
        rollback.defer(format!("compensating mob.peer_added '{b}' -> '{a}'"), {
            let comms_b = comms_b.clone();
            let comms_name_a = comms_name_a.clone();
            let b = b.clone();
            let entry_b = entry_b.clone();
            move || async move {
                self.notify_peer_added(&comms_b, &comms_name_a, &b, &entry_b)
                    .await
            }
        });

        // Remove trust on both sides (required)
        if let Err(remove_a_error) = comms_a.remove_trusted_peer(&key_b).await {
            return Err(rollback.fail(remove_a_error.into()).await);
        }
        rollback.defer(format!("restore trust '{a}' -> '{b}'"), {
            let comms_a = comms_a.clone();
            let spec_b = spec_b.clone();
            move || async move {
                comms_a.add_trusted_peer(spec_b).await?;
                Ok(())
            }
        });

        if let Err(remove_b_error) = comms_b.remove_trusted_peer(&key_a).await {
            return Err(rollback.fail(remove_b_error.into()).await);
        }
        rollback.defer(format!("restore trust '{b}' -> '{a}'"), {
            let comms_b = comms_b.clone();
            let spec_a = spec_a.clone();
            move || async move {
                comms_b.add_trusted_peer(spec_a).await?;
                Ok(())
            }
        });

        // Emit PeersUnwired event
        if let Err(append_error) = self
            .events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::PeersUnwired {
                    a: a.clone(),
                    b: b.clone(),
                },
            })
            .await
        {
            return Err(rollback.fail(append_error).await);
        }

        // Update roster
        {
            let mut roster = self.roster.write().await;
            roster.unwire(&a, &b);
        }
        self.edge_locks.remove(a.as_str(), b.as_str()).await;

        Ok(())
    }

    async fn handle_complete(&mut self) -> Result<(), MobError> {
        self.cancel_all_flow_tasks().await;
        self.notify_orchestrator_lifecycle(format!("Mob '{}' is completing.", self.definition.id))
            .await;
        self.retire_all_members("complete").await?;
        self.stop_mcp_servers().await?;

        self.events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MobCompleted,
            })
            .await?;
        self.state
            .store(MobState::Completed as u8, Ordering::Release);
        Ok(())
    }

    async fn handle_destroy(&mut self) -> Result<(), MobError> {
        self.cancel_all_flow_tasks().await;
        self.notify_orchestrator_lifecycle(format!("Mob '{}' is destroying.", self.definition.id))
            .await;
        self.retire_all_members("destroy").await?;
        self.stop_mcp_servers().await?;
        self.events.clear().await?;
        self.cleanup_namespace().await?;
        self.edge_locks.clear().await;
        self.state
            .store(MobState::Destroyed as u8, Ordering::Release);
        Ok(())
    }

    /// Cancel checkpointers and transition to Stopped. Used by `handle_reset`
    /// error paths after destructive steps have already been taken.
    async fn fail_reset_to_stopped(&self) {
        self.provisioner.cancel_all_checkpointers().await;
        self.state.store(MobState::Stopped as u8, Ordering::Release);
    }

    async fn handle_reset(&mut self) -> Result<(), MobError> {
        let was_stopped = self.state() == MobState::Stopped;
        self.cancel_all_flow_tasks().await;

        // Rearm checkpointers temporarily so retire can checkpoint if needed.
        if was_stopped {
            self.provisioner.rearm_all_checkpointers().await;
        }

        // --- Destructive phase: retire members and stop MCP servers. ---
        // After this point the mob is effectively stopped regardless of what
        // the prior state field says.
        if let Err(error) = self.retire_all_members("reset").await {
            if was_stopped {
                self.provisioner.cancel_all_checkpointers().await;
            }
            return Err(error);
        }
        if let Err(error) = self.stop_mcp_servers().await {
            // Members already retired -- fail-closed to Stopped.
            self.fail_reset_to_stopped().await;
            return Err(error);
        }

        // --- Event rewrite phase: append new epoch markers. ---
        // Append-only epoch model: MobCreated (for resume) + MobReset (epoch
        // marker). Projections (roster, task board) clear on MobReset; resume
        // uses the last MobCreated. No clear() needed -- crash-safe.
        // Batch append ensures both events land atomically.
        let mob_id = self.definition.id.clone();
        if let Err(error) = self
            .events
            .append_batch(vec![
                NewMobEvent {
                    mob_id: mob_id.clone(),
                    timestamp: None,
                    kind: MobEventKind::MobCreated {
                        definition: Box::new(self.definition.as_ref().clone()),
                    },
                },
                NewMobEvent {
                    mob_id,
                    timestamp: None,
                    kind: MobEventKind::MobReset,
                },
            ])
            .await
        {
            self.fail_reset_to_stopped().await;
            return Err(error);
        }

        // Clear in-memory projections. Don't call cleanup_namespace() — it
        // wipes mcp_servers keys which start_mcp_servers needs to track state.
        // stop_mcp_servers already cleared processes and set running=false.
        self.edge_locks.clear().await;
        self.retired_event_index.write().await.clear();
        self.task_board.write().await.clear();

        // --- Restart phase: bring MCP servers back up. ---
        if let Err(error) = self.start_mcp_servers().await {
            if let Err(stop_error) = self.stop_mcp_servers().await {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    error = %stop_error,
                    "reset cleanup failed while stopping mcp servers"
                );
            }
            self.fail_reset_to_stopped().await;
            return Err(error);
        }

        self.state.store(MobState::Running as u8, Ordering::Release);
        Ok(())
    }

    /// Retire all roster members in parallel (sliding window of
    /// `MAX_PARALLEL_HOST_LOOP_OPS`). handle_retire only returns Err on
    /// event-append failures (pre-cleanup); cleanup errors are best-effort.
    /// If any member fails to retire the operation is aborted — the caller
    /// can retry since already-retired members are idempotent.
    async fn retire_all_members(&self, context: &str) -> Result<(), MobError> {
        let ids = {
            let roster = self.roster.read().await;
            roster
                .list_all()
                .map(|entry| entry.meerkat_id.clone())
                .collect::<Vec<_>>()
        };
        if ids.is_empty() {
            return Ok(());
        }

        let mut remaining = ids.into_iter();
        let mut in_flight = FuturesUnordered::new();
        let mut retire_failures: Vec<String> = Vec::new();

        for _ in 0..MAX_PARALLEL_HOST_LOOP_OPS {
            let Some(id) = remaining.next() else {
                break;
            };
            in_flight.push(self.retire_one(id));
        }

        while let Some(result) = in_flight.next().await {
            if let Err((id, error)) = result {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    meerkat_id = %id,
                    error = %error,
                    "{context}: retire failed for member"
                );
                retire_failures.push(format!("{id}: {error}"));
            }
            if let Some(id) = remaining.next() {
                in_flight.push(self.retire_one(id));
            }
        }

        if !retire_failures.is_empty() {
            return Err(MobError::Internal(format!(
                "{context} aborted: {} member(s) could not be retired: {}",
                retire_failures.len(),
                retire_failures.join("; ")
            )));
        }
        Ok(())
    }

    async fn retire_one(&self, id: MeerkatId) -> Result<(), (MeerkatId, MobError)> {
        self.handle_retire_inner(&id, true)
            .await
            .map_err(|error| (id, error))
    }

    async fn handle_task_create(
        &self,
        subject: String,
        description: String,
        blocked_by: Vec<TaskId>,
    ) -> Result<TaskId, MobError> {
        if subject.trim().is_empty() {
            return Err(MobError::Internal(
                "task subject cannot be empty".to_string(),
            ));
        }

        let task_id = TaskId::from(uuid::Uuid::new_v4().to_string());

        let appended = self
            .events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::TaskCreated {
                    task_id: task_id.clone(),
                    subject,
                    description,
                    blocked_by,
                },
            })
            .await?;
        self.task_board.write().await.apply(&appended);
        Ok(task_id)
    }

    async fn handle_task_update(
        &self,
        task_id: TaskId,
        status: TaskStatus,
        owner: Option<MeerkatId>,
    ) -> Result<(), MobError> {
        {
            let board = self.task_board.read().await;
            let task = board
                .get(&task_id)
                .ok_or_else(|| MobError::Internal(format!("task '{task_id}' not found")))?;

            if owner.is_some() {
                if !matches!(status, TaskStatus::InProgress) {
                    return Err(MobError::Internal(format!(
                        "task '{task_id}' owner can only be set with in_progress status"
                    )));
                }
                let blocked = task.blocked_by.iter().any(|dependency| {
                    board.get(dependency).map(|t| t.status) != Some(TaskStatus::Completed)
                });
                if blocked {
                    return Err(MobError::Internal(format!(
                        "task '{task_id}' is blocked by incomplete dependencies"
                    )));
                }
            }
        }

        let appended = self
            .events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::TaskUpdated {
                    task_id,
                    status,
                    owner,
                },
            })
            .await?;
        self.task_board.write().await.apply(&appended);
        Ok(())
    }

    /// P1-T10: external_turn enforces addressability.
    ///
    /// When the target meerkat is not in the roster and a [`SpawnPolicy`] is
    /// set, the policy is consulted. If it resolves a [`SpawnSpec`], the
    /// member is auto-spawned and the message is delivered after provisioning
    /// completes.
    async fn handle_external_turn(
        &mut self,
        meerkat_id: MeerkatId,
        message: String,
    ) -> Result<(), MobError> {
        // Look up the entry
        let entry = {
            let roster = self.roster.read().await;
            roster.get(&meerkat_id).cloned()
        };
        let entry = match entry {
            Some(e) => e,
            None => {
                // Consult spawn policy for auto-provisioning
                if let Some(ref policy) = self.spawn_policy
                    && let Some(spec) = policy.resolve(&meerkat_id).await
                {
                    let (spawn_reply_tx, spawn_reply_rx) = oneshot::channel();
                    self.enqueue_spawn(
                        super::handle::SpawnMemberSpec {
                            profile_name: spec.profile,
                            meerkat_id: meerkat_id.clone(),
                            initial_message: None,
                            runtime_mode: spec.runtime_mode,
                            backend: None,
                            context: None,
                            labels: None,
                            resume_session_id: None,
                        },
                        spawn_reply_tx,
                    )
                    .await;

                    // Schedule deferred message delivery after spawn completes.
                    let command_tx = self.command_tx.clone();
                    let target_id = meerkat_id.clone();
                    self.lifecycle_tasks.spawn(async move {
                        if let Ok(Ok(_)) = spawn_reply_rx.await {
                            let (reply_tx, reply_rx) = oneshot::channel();
                            let _ = command_tx
                                .send(MobCommand::ExternalTurn {
                                    meerkat_id: target_id.clone(),
                                    message,
                                    reply_tx,
                                })
                                .await;
                            match reply_rx.await {
                                Ok(Ok(())) => {}
                                Ok(Err(e)) => {
                                    tracing::error!(
                                        meerkat_id = %target_id,
                                        error = %e,
                                        "deferred delivery after auto-spawn failed"
                                    );
                                }
                                Err(_) => {
                                    tracing::error!(
                                        meerkat_id = %target_id,
                                        "deferred delivery channel dropped before response"
                                    );
                                }
                            }
                        }
                    });
                    return Ok(());
                }
                return Err(MobError::MeerkatNotFound(meerkat_id));
            }
        };

        // Check external_addressable
        let profile = self
            .definition
            .profiles
            .get(&entry.profile)
            .ok_or_else(|| MobError::ProfileNotFound(entry.profile.clone()))?;

        if !profile.external_addressable {
            return Err(MobError::NotExternallyAddressable(meerkat_id));
        }

        self.dispatch_member_turn(&entry, message).await?;

        Ok(())
    }

    /// Internal-turn path bypasses external_addressable checks.
    async fn handle_internal_turn(
        &self,
        meerkat_id: MeerkatId,
        message: String,
    ) -> Result<(), MobError> {
        let entry = {
            let roster = self.roster.read().await;
            roster
                .get(&meerkat_id)
                .cloned()
                .ok_or_else(|| MobError::MeerkatNotFound(meerkat_id.clone()))?
        };

        self.dispatch_member_turn(&entry, message).await?;
        Ok(())
    }

    async fn dispatch_member_turn(
        &self,
        entry: &RosterEntry,
        message: String,
    ) -> Result<(), MobError> {
        match entry.runtime_mode {
            crate::MobRuntimeMode::AutonomousHost => {
                let session_id = entry.member_ref.session_id().ok_or_else(|| {
                    MobError::Internal(format!(
                        "autonomous dispatch requires session-backed member ref for '{}'",
                        entry.meerkat_id
                    ))
                })?;
                let injector = self
                    .provisioner
                    .event_injector(session_id)
                    .await
                    .ok_or_else(|| {
                        MobError::Internal(format!(
                            "missing event injector for autonomous member '{}'",
                            entry.meerkat_id
                        ))
                    })?;
                injector
                    .inject(message, meerkat_core::PlainEventSource::Rpc)
                    .map_err(|error| {
                        MobError::Internal(format!(
                            "autonomous dispatch inject failed for '{}': {}",
                            entry.meerkat_id, error
                        ))
                    })?;
                Ok(())
            }
            crate::MobRuntimeMode::TurnDriven => {
                let req = meerkat_core::service::StartTurnRequest {
                    prompt: message,
                    event_tx: None,
                    host_mode: false,
                    skill_references: None,
                    flow_tool_overlay: None,
                    additional_instructions: None,
                };
                self.provisioner.start_turn(&entry.member_ref, req).await
            }
        }
    }

    /// Inject a message into an autonomous member and return a subscription
    /// for streaming interaction-scoped events.
    ///
    /// Like `handle_external_turn`, this enforces lifecycle state and external
    /// addressability. Unlike `handle_external_turn`, it does **not** support
    /// spawn-policy auto-provisioning for unknown targets (the subscription
    /// cannot be deferred through a lifecycle task). Returns
    /// `UnsupportedForMode` for `TurnDriven` members.
    async fn handle_inject_and_subscribe(
        &mut self,
        meerkat_id: MeerkatId,
        message: String,
    ) -> Result<meerkat_core::InteractionSubscription, MobError> {
        // Look up the entry (mirrors handle_external_turn)
        let entry = {
            let roster = self.roster.read().await;
            roster.get(&meerkat_id).cloned()
        };
        let entry = match entry {
            Some(e) => e,
            None => {
                // Consult spawn policy — but unlike ExternalTurn we can't defer
                // the subscription through a lifecycle task, so reject cleanly.
                return Err(MobError::MeerkatNotFound(meerkat_id));
            }
        };

        // Enforce external_addressable
        let profile = self
            .definition
            .profiles
            .get(&entry.profile)
            .ok_or_else(|| MobError::ProfileNotFound(entry.profile.clone()))?;

        if !profile.external_addressable {
            return Err(MobError::NotExternallyAddressable(meerkat_id));
        }

        // Only AutonomousHost supports inject_with_subscription
        if entry.runtime_mode != crate::MobRuntimeMode::AutonomousHost {
            return Err(MobError::UnsupportedForMode {
                mode: entry.runtime_mode,
                reason: "inject_and_subscribe requires autonomous_host mode".into(),
            });
        }

        let session_id = entry.member_ref.session_id().ok_or_else(|| {
            MobError::Internal(format!(
                "autonomous dispatch requires session-backed member ref for '{}'",
                entry.meerkat_id
            ))
        })?;
        let injector = self
            .provisioner
            .event_injector(session_id)
            .await
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "missing event injector for autonomous member '{}'",
                    entry.meerkat_id
                ))
            })?;
        injector
            .inject_with_subscription(message, meerkat_core::PlainEventSource::Rpc)
            .map_err(|error| {
                MobError::Internal(format!(
                    "inject_and_subscribe failed for '{}': {}",
                    entry.meerkat_id, error
                ))
            })
    }

    async fn handle_run_flow(
        &mut self,
        flow_id: FlowId,
        activation_params: serde_json::Value,
        scoped_event_tx: Option<mpsc::Sender<meerkat_core::ScopedAgentEvent>>,
    ) -> Result<RunId, MobError> {
        let run_id = RunId::new();
        let config = FlowRunConfig::from_definition(flow_id, &self.definition)?;

        let initial_run = MobRun {
            run_id: run_id.clone(),
            mob_id: self.definition.id.clone(),
            flow_id: config.flow_id.clone(),
            status: MobRunStatus::Pending,
            activation_params: activation_params.clone(),
            created_at: chrono::Utc::now(),
            completed_at: None,
            step_ledger: Vec::new(),
            failure_ledger: Vec::new(),
        };
        self.run_store.create_run(initial_run).await?;

        let cancel_token = tokio_util::sync::CancellationToken::new();
        self.run_cancel_tokens.insert(
            run_id.clone(),
            (cancel_token.clone(), config.flow_id.clone()),
        );
        if let Some(scoped_event_tx) = scoped_event_tx {
            self.flow_streams
                .lock()
                .await
                .insert(run_id.clone(), scoped_event_tx);
        }

        let engine = self.flow_engine.clone();
        let cleanup_tx = self.command_tx.clone();
        let run_store = self.run_store.clone();
        let events = self.events.clone();
        let mob_id = self.definition.id.clone();
        let flow_run_id = run_id.clone();
        let flow_id_for_task = config.flow_id.clone();
        let cleanup_run_id = run_id.clone();
        let handle = tokio::spawn(async move {
            let run_id_for_execute = flow_run_id.clone();
            if let Err(error) = engine
                .execute_flow(run_id_for_execute, config, activation_params, cancel_token)
                .await
            {
                tracing::error!(
                    run_id = %flow_run_id,
                    flow_id = %flow_id_for_task,
                    error = %error,
                    "flow task execution failed; applying actor fallback finalization"
                );
                if let Err(finalize_error) = Self::finalize_run_failed(
                    run_store,
                    events,
                    mob_id,
                    flow_run_id.clone(),
                    flow_id_for_task,
                    error.to_string(),
                )
                .await
                {
                    tracing::error!(
                        run_id = %flow_run_id,
                        error = %finalize_error,
                        "failed to finalize run after flow task error"
                    );
                }
            }
            if cleanup_tx
                .send(MobCommand::FlowFinished {
                    run_id: cleanup_run_id,
                })
                .await
                .is_err()
            {
                tracing::warn!(
                    run_id = %flow_run_id,
                    "failed to send FlowFinished cleanup command"
                );
            }
        });
        self.run_tasks.insert(run_id.clone(), handle);

        Ok(run_id)
    }

    async fn handle_cancel_flow(&mut self, run_id: RunId) -> Result<(), MobError> {
        let Some((cancel_token, flow_id)) = self.run_cancel_tokens.remove(&run_id) else {
            return Ok(());
        };
        self.flow_streams.lock().await.remove(&run_id);
        cancel_token.cancel();

        let Some(mut handle) = self.run_tasks.remove(&run_id) else {
            return Ok(());
        };

        let run_store = self.run_store.clone();
        let events = self.events.clone();
        let mob_id = self.definition.id.clone();
        let cancel_grace_timeout = self
            .definition
            .limits
            .as_ref()
            .and_then(|limits| limits.cancel_grace_timeout_ms)
            .map_or_else(
                || std::time::Duration::from_secs(5),
                std::time::Duration::from_millis,
            );
        tokio::spawn(async move {
            let completed = tokio::select! {
                _ = &mut handle => true,
                () = tokio::time::sleep(cancel_grace_timeout) => false,
            };
            if completed {
                return;
            }

            handle.abort();
            if let Err(error) =
                Self::finalize_run_canceled(run_store, events, mob_id, run_id, flow_id).await
            {
                tracing::error!(
                    error = %error,
                    "failed actor fallback cancellation finalization"
                );
            }
        });

        Ok(())
    }

    async fn finalize_run_failed(
        run_store: Arc<dyn MobRunStore>,
        events: Arc<dyn MobEventStore>,
        mob_id: MobId,
        run_id: RunId,
        flow_id: FlowId,
        reason: String,
    ) -> Result<(), MobError> {
        FlowTerminalizationAuthority::new(run_store, events, mob_id)
            .terminalize(run_id, flow_id, TerminalizationTarget::Failed { reason })
            .await?;
        Ok(())
    }

    async fn finalize_run_canceled(
        run_store: Arc<dyn MobRunStore>,
        events: Arc<dyn MobEventStore>,
        mob_id: MobId,
        run_id: RunId,
        flow_id: FlowId,
    ) -> Result<(), MobError> {
        FlowTerminalizationAuthority::new(run_store, events, mob_id)
            .terminalize(run_id, flow_id, TerminalizationTarget::Canceled)
            .await?;
        Ok(())
    }

    async fn cancel_all_flow_tasks(&mut self) {
        let cancel_tokens = std::mem::take(&mut self.run_cancel_tokens);
        let tasks = std::mem::take(&mut self.run_tasks);
        for (_, handle) in tasks {
            handle.abort();
        }
        for (run_id, (token, flow_id)) in cancel_tokens {
            token.cancel();
            if let Err(error) = Self::finalize_run_canceled(
                self.run_store.clone(),
                self.events.clone(),
                self.definition.id.clone(),
                run_id.clone(),
                flow_id.clone(),
            )
            .await
            {
                tracing::error!(
                    run_id = %run_id,
                    flow_id = %flow_id,
                    error = %error,
                    "failed to finalize run cancellation during lifecycle shutdown"
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Apply auto/role wiring for a newly spawned meerkat.
    ///
    /// `wiring_targets` is expected to be deduplicated by `spawn_wiring_targets()`.
    /// The actor keeps command ordering, but per-edge wire effects run with bounded
    /// parallelism to reduce spawn fan-out latency.
    async fn apply_spawn_wiring(
        &self,
        meerkat_id: &MeerkatId,
        wiring_targets: &[MeerkatId],
    ) -> Result<(), MobError> {
        if wiring_targets.is_empty() {
            return Ok(());
        }

        const MAX_PARALLEL_SPAWN_WIRES: usize = 8;
        let mut in_flight = FuturesUnordered::new();
        let mut remaining = wiring_targets.iter().cloned();
        let mut first_error: Option<MobError> = None;

        for _ in 0..MAX_PARALLEL_SPAWN_WIRES {
            let Some(target_id) = remaining.next() else {
                break;
            };
            in_flight.push(self.wire_spawn_target(meerkat_id, target_id));
        }

        while let Some(result) = in_flight.next().await {
            if let Err(error) = result
                && first_error.is_none()
            {
                first_error = Some(error);
            }
            if let Some(target_id) = remaining.next() {
                in_flight.push(self.wire_spawn_target(meerkat_id, target_id));
            }
        }

        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(())
    }

    async fn wire_spawn_target(
        &self,
        meerkat_id: &MeerkatId,
        target_id: MeerkatId,
    ) -> Result<(), MobError> {
        self.do_wire(meerkat_id, &target_id).await.map_err(|e| {
            MobError::WiringError(format!(
                "role_wiring fan-out failed for {meerkat_id} <-> {target_id}: {e}"
            ))
        })
    }

    /// Compensate a failed spawn wiring path to avoid partial state.
    async fn rollback_failed_spawn(
        &self,
        meerkat_id: &MeerkatId,
        profile_name: &ProfileName,
        member_ref: &MemberRef,
        planned_wiring_targets: &[MeerkatId],
    ) -> Result<(), MobError> {
        let retire_event_already_present = self.retire_event_exists(meerkat_id, member_ref).await?;
        if !retire_event_already_present {
            self.append_retire_event(meerkat_id, profile_name, member_ref)
                .await?;
        }

        let wired_peers = {
            let roster = self.roster.read().await;
            roster
                .get(meerkat_id)
                .map(|entry| entry.wired_to.iter().cloned().collect::<Vec<_>>())
                .unwrap_or_default()
        };
        let mut cleanup_peers = wired_peers.clone();
        for peer_id in planned_wiring_targets {
            if peer_id != meerkat_id && !cleanup_peers.contains(peer_id) {
                cleanup_peers.push(peer_id.clone());
            }
        }
        let spawned_entry = {
            let roster = self.roster.read().await;
            roster.get(meerkat_id).cloned()
        };
        let spawned_comms = self.provisioner_comms(member_ref).await;
        let mut rollback = LifecycleRollback::new("spawn rollback");

        if !wired_peers.is_empty() {
            let spawned_comms = spawned_comms.as_ref().ok_or_else(|| {
                MobError::WiringError(format!(
                    "spawn rollback requires comms runtime for '{meerkat_id}'"
                ))
            })?;
            let spawned_entry = spawned_entry.as_ref().ok_or_else(|| {
                MobError::WiringError(format!(
                    "spawn rollback requires roster entry for '{meerkat_id}'"
                ))
            })?;
            for peer_meerkat_id in &wired_peers {
                let peer_comms_name = {
                    let roster = self.roster.read().await;
                    roster
                        .get(peer_meerkat_id)
                        .map(|entry| self.comms_name_for(entry))
                        .ok_or_else(|| {
                            MobError::WiringError(format!(
                                "spawn rollback requires roster entry for wired peer '{peer_meerkat_id}'"
                            ))
                        })?
                };
                self.notify_peer_retired(peer_meerkat_id, meerkat_id, spawned_entry, spawned_comms)
                    .await?;
                rollback.defer(
                    format!("compensating mob.peer_added '{meerkat_id}' -> '{peer_meerkat_id}'"),
                    {
                        let spawned_comms = spawned_comms.clone();
                        let peer_comms_name = peer_comms_name.clone();
                        let spawned_entry = spawned_entry.clone();
                        let meerkat_id = meerkat_id.clone();
                        move || async move {
                            self.notify_peer_added(
                                &spawned_comms,
                                &peer_comms_name,
                                &meerkat_id,
                                &spawned_entry,
                            )
                            .await
                        }
                    },
                );
            }
        }

        let spawned_key = spawned_comms.as_ref().and_then(|comms| comms.public_key());
        let spawned_spec = if let (Some(spawned_key), Some(spawned_entry)) =
            (spawned_key.clone(), spawned_entry.as_ref())
        {
            let spawned_comms_name = self.comms_name_for(spawned_entry);
            Some(
                self.provisioner
                    .trusted_peer_spec(member_ref, &spawned_comms_name, &spawned_key)
                    .await?,
            )
        } else {
            None
        };

        if let Some(spawned_key) = spawned_key {
            for peer_meerkat_id in &cleanup_peers {
                let is_wired_peer = wired_peers.contains(peer_meerkat_id);
                let peer_entry = {
                    let roster = self.roster.read().await;
                    roster.get(peer_meerkat_id).cloned()
                };
                let Some(peer_entry) = peer_entry else {
                    if is_wired_peer {
                        return Err(rollback
                            .fail(MobError::Internal(format!(
                                "spawn rollback cannot remove trust for '{meerkat_id}': wired peer '{peer_meerkat_id}' missing from roster"
                            )))
                            .await);
                    }
                    continue;
                };
                let peer_comms = self.provisioner_comms(&peer_entry.member_ref).await;
                let Some(peer_comms) = peer_comms else {
                    if is_wired_peer {
                        return Err(rollback
                            .fail(MobError::Internal(format!(
                                "spawn rollback cannot remove trust for '{meerkat_id}': comms runtime missing for wired peer '{peer_meerkat_id}'"
                            )))
                            .await);
                    }
                    continue;
                };
                if let Err(error) = peer_comms.remove_trusted_peer(&spawned_key).await {
                    if is_wired_peer {
                        return Err(rollback
                            .fail(MobError::Internal(format!(
                                "spawn rollback cannot remove trust for '{meerkat_id}' from wired peer '{peer_meerkat_id}': {error}"
                            )))
                            .await);
                    }
                    continue;
                }
                if let Some(spawned_spec) = spawned_spec.clone() {
                    rollback.defer(
                        format!(
                            "restore trust '{peer_meerkat_id}' -> '{meerkat_id}' during spawn rollback"
                        ),
                        {
                            let peer_comms = peer_comms.clone();
                            move || async move {
                                peer_comms.add_trusted_peer(spawned_spec).await?;
                                Ok(())
                            }
                        },
                    );
                }
            }
        }

        // Reuse disposal pipeline methods for session archive + roster removal.
        let rollback_ctx = DisposalContext {
            meerkat_id: meerkat_id.clone(),
            entry: spawned_entry.clone().unwrap_or_else(|| RosterEntry {
                meerkat_id: meerkat_id.clone(),
                profile: profile_name.clone(),
                member_ref: member_ref.clone(),
                runtime_mode: crate::MobRuntimeMode::TurnDriven,
                state: crate::roster::MemberState::Active,
                wired_to: std::collections::BTreeSet::new(),
                labels: std::collections::BTreeMap::new(),
            }),
            retiring_comms: spawned_comms.clone(),
            retiring_key: spawned_comms.as_ref().and_then(|c| c.public_key()),
        };
        if let Err(error) = self.dispose_archive_session(&rollback_ctx).await {
            return Err(rollback.fail(error).await);
        }

        self.dispose_remove_from_roster(&rollback_ctx).await;

        Ok(())
    }

    /// Resolve profile-declared rust tool bundles to a dispatcher.
    fn external_tools_for_profile(
        &self,
        profile: &crate::profile::Profile,
    ) -> Result<Option<Arc<dyn AgentToolDispatcher>>, MobError> {
        compose_external_tools_for_profile(profile, &self.tool_bundles, self.mob_handle_for_tools())
    }

    async fn retire_event_exists(
        &self,
        meerkat_id: &MeerkatId,
        member_ref: &MemberRef,
    ) -> Result<bool, MobError> {
        let key = Self::retire_event_key(meerkat_id, member_ref);
        let index = self.retired_event_index.read().await;
        Ok(index.contains(&key))
    }

    async fn append_retire_event(
        &self,
        meerkat_id: &MeerkatId,
        profile_name: &ProfileName,
        member_ref: &MemberRef,
    ) -> Result<(), MobError> {
        self.events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MeerkatRetired {
                    meerkat_id: meerkat_id.clone(),
                    role: profile_name.clone(),
                    member_ref: member_ref.clone(),
                },
            })
            .await?;
        let key = Self::retire_event_key(meerkat_id, member_ref);
        self.retired_event_index.write().await.insert(key);
        Ok(())
    }

    /// Internal wire operation (used by handle_wire and auto_wire/role_wiring).
    async fn do_wire(&self, a: &MeerkatId, b: &MeerkatId) -> Result<(), MobError> {
        let _edge_guard = self.edge_locks.acquire(a.as_str(), b.as_str()).await;

        {
            let roster = self.roster.read().await;
            if let Some(entry_a) = roster.get(a)
                && entry_a.wired_to.contains(b)
            {
                return Ok(());
            }
        }

        let (entry_a, entry_b) = {
            let roster = self.roster.read().await;
            let ea = roster
                .get(a)
                .cloned()
                .ok_or_else(|| MobError::MeerkatNotFound(a.clone()))?;
            let eb = roster
                .get(b)
                .cloned()
                .ok_or_else(|| MobError::MeerkatNotFound(b.clone()))?;
            (ea, eb)
        };

        // Establish bidirectional trust via comms (required).
        let comms_a = self
            .provisioner_comms(&entry_a.member_ref)
            .await
            .ok_or_else(|| {
                MobError::WiringError(format!("wire requires comms runtime for '{a}'"))
            })?;
        let comms_b = self
            .provisioner_comms(&entry_b.member_ref)
            .await
            .ok_or_else(|| {
                MobError::WiringError(format!("wire requires comms runtime for '{b}'"))
            })?;

        let key_a = comms_a
            .public_key()
            .ok_or_else(|| MobError::WiringError(format!("wire requires public key for '{a}'")))?;
        let key_b = comms_b
            .public_key()
            .ok_or_else(|| MobError::WiringError(format!("wire requires public key for '{b}'")))?;

        // Get peer info for trust establishment
        let comms_name_a = self.comms_name_for(&entry_a);
        let comms_name_b = self.comms_name_for(&entry_b);

        let spec_b = self
            .provisioner
            .trusted_peer_spec(&entry_b.member_ref, &comms_name_b, &key_b)
            .await?;
        let spec_a = self
            .provisioner
            .trusted_peer_spec(&entry_a.member_ref, &comms_name_a, &key_a)
            .await?;

        let mut rollback = LifecycleRollback::new("wire");

        comms_a.add_trusted_peer(spec_b.clone()).await?;
        rollback.defer(format!("remove trust '{a}' -> '{b}'"), {
            let comms_a = comms_a.clone();
            let key_b = key_b.clone();
            move || async move {
                comms_a.remove_trusted_peer(&key_b).await?;
                Ok(())
            }
        });

        if let Err(error) = comms_b.add_trusted_peer(spec_a.clone()).await {
            return Err(rollback.fail(error.into()).await);
        }
        rollback.defer(format!("remove trust '{b}' -> '{a}'"), {
            let comms_b = comms_b.clone();
            let key_a = key_a.clone();
            move || async move {
                comms_b.remove_trusted_peer(&key_a).await?;
                Ok(())
            }
        });

        // Notify both peers (required for successful wire):
        // Send FROM b TO a about new peer b
        if let Err(error) = self
            .notify_peer_added(&comms_b, &comms_name_a, b, &entry_b)
            .await
        {
            return Err(rollback.fail(error).await);
        }
        rollback.defer(format!("compensating mob.peer_retired '{b}' -> '{a}'"), {
            let comms_b = comms_b.clone();
            let entry_b = entry_b.clone();
            let a = a.clone();
            let b = b.clone();
            move || async move { self.notify_peer_retired(&a, &b, &entry_b, &comms_b).await }
        });

        // Send FROM a TO b about new peer a
        if let Err(error) = self
            .notify_peer_added(&comms_a, &comms_name_b, a, &entry_a)
            .await
        {
            return Err(rollback.fail(error).await);
        }
        rollback.defer(format!("compensating mob.peer_retired '{a}' -> '{b}'"), {
            let comms_a = comms_a.clone();
            let entry_a = entry_a.clone();
            let a = a.clone();
            let b = b.clone();
            move || async move { self.notify_peer_retired(&b, &a, &entry_a, &comms_a).await }
        });

        // Emit PeersWired event
        if let Err(append_error) = self
            .events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::PeersWired {
                    a: a.clone(),
                    b: b.clone(),
                },
            })
            .await
        {
            return Err(rollback.fail(append_error).await);
        }

        // Update roster
        {
            let mut roster = self.roster.write().await;
            roster.wire(a, b);
        }

        Ok(())
    }

    /// Get the comms runtime for a session, if available.
    async fn provisioner_comms(&self, member_ref: &MemberRef) -> Option<Arc<dyn CoreCommsRuntime>> {
        self.provisioner.comms_runtime(member_ref).await
    }

    /// Generate the comms name for a roster entry.
    fn comms_name_for(&self, entry: &RosterEntry) -> String {
        format!(
            "{}/{}/{}",
            self.definition.id, entry.profile, entry.meerkat_id
        )
    }

    /// Notify a peer that a new peer was added.
    ///
    /// Sends a `PeerRequest` with intent `mob.peer_added` FROM `sender_comms`
    /// TO the peer identified by `recipient_comms_name`. The params contain
    /// the new peer's identity and role.
    ///
    /// REQ-MOB-010/011: Notification is required for successful wiring.
    async fn notify_peer_added(
        &self,
        sender_comms: &Arc<dyn CoreCommsRuntime>,
        recipient_comms_name: &str,
        new_peer_id: &MeerkatId,
        new_peer_entry: &RosterEntry,
    ) -> Result<(), MobError> {
        let peer_description = self
            .definition
            .profiles
            .get(&new_peer_entry.profile)
            .map_or("", |p| p.peer_description.as_str());

        let peer_name = PeerName::new(recipient_comms_name).map_err(|error| {
            MobError::WiringError(format!(
                "notify_peer_added: invalid recipient comms name '{recipient_comms_name}': {error}"
            ))
        })?;

        let cmd = CommsCommand::PeerRequest {
            to: peer_name,
            intent: "mob.peer_added".to_string(),
            params: serde_json::json!({
                "peer": new_peer_id.as_str(),
                "role": new_peer_entry.profile.as_str(),
                "description": peer_description,
            }),
            stream: InputStreamMode::None,
        };

        sender_comms.send(cmd).await?;
        Ok(())
    }

    async fn notify_peer_event(
        &self,
        intent: &'static str,
        peer_id: &MeerkatId,
        other_peer_id: &MeerkatId,
        other_peer_entry: &RosterEntry,
        sender_comms: &Arc<dyn CoreCommsRuntime>,
    ) -> Result<(), MobError> {
        let peer_entry = {
            let roster = self.roster.read().await;
            roster.get(peer_id).cloned()
        };

        let peer_entry = peer_entry.ok_or_else(|| {
            MobError::WiringError(format!(
                "notify_peer_retired: peer '{peer_id}' missing from roster"
            ))
        })?;

        let peer_comms_name = self.comms_name_for(&peer_entry);
        let peer_name = PeerName::new(&peer_comms_name).map_err(|error| {
            MobError::WiringError(format!(
                "notify_peer_retired: invalid peer comms name '{peer_comms_name}': {error}"
            ))
        })?;

        let cmd = CommsCommand::PeerRequest {
            to: peer_name,
            intent: intent.to_string(),
            params: serde_json::json!({
                "peer": other_peer_id.as_str(),
                "role": other_peer_entry.profile.as_str(),
            }),
            stream: InputStreamMode::None,
        };

        sender_comms.send(cmd).await?;
        Ok(())
    }

    /// Notify a peer that another peer was retired from the mob.
    async fn notify_peer_retired(
        &self,
        peer_id: &MeerkatId,
        retired_id: &MeerkatId,
        retired_entry: &RosterEntry,
        retiring_comms: &Arc<dyn CoreCommsRuntime>,
    ) -> Result<(), MobError> {
        self.notify_peer_event(
            "mob.peer_retired",
            peer_id,
            retired_id,
            retired_entry,
            retiring_comms,
        )
        .await
    }

    /// Notify a peer that another peer was unwired (trust link removed).
    async fn notify_peer_unwired(
        &self,
        peer_id: &MeerkatId,
        unwired_id: &MeerkatId,
        unwired_entry: &RosterEntry,
        sender_comms: &Arc<dyn CoreCommsRuntime>,
    ) -> Result<(), MobError> {
        self.notify_peer_event(
            "mob.peer_unwired",
            peer_id,
            unwired_id,
            unwired_entry,
            sender_comms,
        )
        .await
    }
}

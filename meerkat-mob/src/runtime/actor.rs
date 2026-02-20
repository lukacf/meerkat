use super::terminalization::{FlowTerminalizationAuthority, TerminalizationTarget};
use super::transaction::LifecycleRollback;
use super::*;

type AutonomousHostLoopHandle = tokio::task::JoinHandle<Result<(), MobError>>;

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
    pub(super) mcp_running: Arc<RwLock<BTreeMap<String, bool>>>,
    pub(super) mcp_processes: Arc<tokio::sync::Mutex<BTreeMap<String, Child>>>,
    pub(super) command_tx: mpsc::Sender<MobCommand>,
    pub(super) tool_bundles: BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
    pub(super) default_llm_client: Option<Arc<dyn LlmClient>>,
    pub(super) retired_event_index: Arc<RwLock<HashSet<String>>>,
    pub(super) autonomous_host_loops: Arc<tokio::sync::Mutex<BTreeMap<MeerkatId, AutonomousHostLoopHandle>>>,
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
            mcp_running: self.mcp_running.clone(),
        }
    }

    fn expect_state(&self, expected: &[MobState], to: MobState) -> Result<(), MobError> {
        let current = self.state();
        if !expected.contains(&current) {
            return Err(MobError::InvalidTransition { from: current, to });
        }
        Ok(())
    }

    async fn notify_orchestrator_lifecycle(&self, message: String) {
        if let Some(orchestrator) = &self.definition.orchestrator
            && let Some(orchestrator_entry) = self
                .roster
                .read()
                .await
                .by_profile(&orchestrator.profile)
                .next()
                .cloned()
        {
            let provisioner = self.provisioner.clone();
            let member_ref = orchestrator_entry.member_ref;
            let runtime_mode = orchestrator_entry.runtime_mode;
            let meerkat_id = orchestrator_entry.meerkat_id;
            tokio::spawn(async move {
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
                                    "orchestrator lifecycle inject failed for '{}': {}",
                                    meerkat_id, error
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
    }

    fn retire_event_key(meerkat_id: &MeerkatId, member_ref: &MemberRef) -> String {
        let member =
            serde_json::to_string(member_ref).unwrap_or_else(|_| format!("{member_ref:?}"));
        format!("{meerkat_id}|{member}")
    }

    async fn stop_mcp_servers(&self) -> Result<(), MobError> {
        let mut processes = self.mcp_processes.lock().await;
        let mut first_error: Option<MobError> = None;
        for (name, child) in processes.iter_mut() {
            if let Err(error) = child.kill().await {
                let mob_error =
                    MobError::Internal(format!("failed to stop mcp server '{name}': {error}"));
                tracing::warn!(error = %mob_error, "mcp server kill failed");
                if first_error.is_none() {
                    first_error = Some(mob_error);
                }
            }
            if let Err(error) = child.wait().await {
                let mob_error = MobError::Internal(format!(
                    "failed waiting for mcp server '{name}' to exit: {error}"
                ));
                tracing::warn!(error = %mob_error, "mcp server wait failed");
                if first_error.is_none() {
                    first_error = Some(mob_error);
                }
            }
        }
        processes.clear();

        let mut running = self.mcp_running.write().await;
        for state in running.values_mut() {
            *state = false;
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(())
    }

    async fn start_mcp_servers(&self) -> Result<(), MobError> {
        let mut processes = self.mcp_processes.lock().await;
        for (name, cfg) in &self.definition.mcp_servers {
            if cfg.command.is_empty() {
                continue;
            }
            if processes.contains_key(name) {
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
            processes.insert(name.clone(), child);
        }

        let mut running = self.mcp_running.write().await;
        for state in running.values_mut() {
            *state = true;
        }
        Ok(())
    }

    async fn cleanup_namespace(&self) -> Result<(), MobError> {
        self.mcp_processes.lock().await.clear();
        self.mcp_running.write().await.clear();
        Ok(())
    }

    fn fallback_spawn_prompt(&self, profile_name: &ProfileName, meerkat_id: &MeerkatId) -> String {
        format!(
            "You have been spawned as '{}' (role: {}) in mob '{}'.",
            meerkat_id, profile_name, self.definition.id
        )
    }

    fn resume_host_loop_prompt(&self, profile_name: &ProfileName, meerkat_id: &MeerkatId) -> String {
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
        let handle = tokio::spawn(async move {
            provisioner
                .start_turn(
                    &member_ref_cloned,
                    meerkat_core::service::StartTurnRequest {
                        prompt,
                        event_tx: None,
                        host_mode: true,
                        skill_references: None,
                    },
                )
                .await
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
        for entry in entries {
            if entry.runtime_mode == crate::MobRuntimeMode::AutonomousHost {
                self.ensure_autonomous_dispatch_capability(&entry.meerkat_id, &entry.member_ref)
                    .await?;
                self.start_autonomous_host_loop(
                    &entry.meerkat_id,
                    &entry.member_ref,
                    self.resume_host_loop_prompt(&entry.profile, &entry.meerkat_id),
                )
                .await?;
            }
        }
        Ok(())
    }

    async fn ensure_autonomous_dispatch_capability(
        &self,
        meerkat_id: &MeerkatId,
        member_ref: &MemberRef,
    ) -> Result<(), MobError> {
        let session_id = member_ref.session_id().ok_or_else(|| {
            MobError::Internal(format!(
                "autonomous member '{}' must be session-backed for injector dispatch",
                meerkat_id
            ))
        })?;
        if self.provisioner.event_injector(session_id).await.is_none() {
            return Err(MobError::Internal(format!(
                "autonomous member '{}' is missing event injector capability",
                meerkat_id
            )));
        }
        Ok(())
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
        let mut first_error: Option<MobError> = None;
        for entry in entries {
            if let Err(error) = self
                .stop_autonomous_host_loop_for_member(&entry.meerkat_id, &entry.member_ref)
                .await
            {
                tracing::warn!(
                    meerkat_id = %entry.meerkat_id,
                    error = %error,
                    "failed stopping autonomous host loop member"
                );
                if first_error.is_none() {
                    first_error = Some(error);
                }
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

    /// Main actor loop: process commands sequentially until Shutdown.
    pub(super) async fn run(mut self, mut command_rx: mpsc::Receiver<MobCommand>) {
        if matches!(self.state(), MobState::Running) {
            if let Err(error) = self.start_mcp_servers().await {
                tracing::error!(
                    mob_id = %self.definition.id,
                    error = %error,
                    "failed to start mcp servers during actor startup; entering Stopped"
                );
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
        while let Some(cmd) = command_rx.recv().await {
            match cmd {
                MobCommand::Spawn {
                    profile_name,
                    meerkat_id,
                    initial_message,
                    runtime_mode,
                    backend,
                    reply_tx,
                } => {
                    let result = match self
                        .expect_state(&[MobState::Running, MobState::Creating], MobState::Running)
                    {
                        Ok(()) => {
                            self.handle_spawn(
                                profile_name,
                                meerkat_id,
                                initial_message,
                                runtime_mode,
                                backend,
                            )
                            .await
                        }
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::Retire {
                    meerkat_id,
                    reply_tx,
                } => {
                    let result = match self
                        .expect_state(&[MobState::Running, MobState::Creating], MobState::Running)
                    {
                        Ok(()) => self.handle_retire(meerkat_id).await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::Wire { a, b, reply_tx } => {
                    let result = match self
                        .expect_state(&[MobState::Running, MobState::Creating], MobState::Running)
                    {
                        Ok(()) => self.handle_wire(a, b).await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::Unwire { a, b, reply_tx } => {
                    let result = match self
                        .expect_state(&[MobState::Running, MobState::Creating], MobState::Running)
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
                    let result = match self
                        .expect_state(&[MobState::Running, MobState::Creating], MobState::Running)
                    {
                        Ok(()) => self.handle_external_turn(meerkat_id, message).await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::InternalTurn {
                    meerkat_id,
                    message,
                    reply_tx,
                } => {
                    let result = match self
                        .expect_state(&[MobState::Running, MobState::Creating], MobState::Running)
                    {
                        Ok(()) => self.handle_internal_turn(meerkat_id, message).await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::RunFlow {
                    flow_id,
                    activation_params,
                    reply_tx,
                } => {
                    let result = match self.expect_state(&[MobState::Running], MobState::Running) {
                        Ok(()) => self.handle_run_flow(flow_id, activation_params).await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::CancelFlow { run_id, reply_tx } => {
                    let result = match self.expect_state(&[MobState::Running], MobState::Running) {
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
                            self.notify_orchestrator_lifecycle(format!(
                                "Mob '{}' is stopping.",
                                self.definition.id
                            ))
                            .await;
                            let mut stop_result: Result<(), MobError> = Ok(());
                            if let Err(error) = self.stop_all_autonomous_host_loops().await {
                                tracing::warn!(
                                    mob_id = %self.definition.id,
                                    error = %error,
                                    "stop encountered autonomous loop cleanup error"
                                );
                                if stop_result.is_ok() {
                                    stop_result = Err(error);
                                }
                            }
                            if let Err(error) = self.stop_mcp_servers().await {
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
                            if let Err(error) = self.start_mcp_servers().await {
                                if let Err(stop_error) = self.stop_mcp_servers().await {
                                    tracing::warn!(
                                        mob_id = %self.definition.id,
                                        error = %stop_error,
                                        "resume cleanup failed while stopping mcp servers"
                                    );
                                }
                                Err(error)
                            } else if let Err(error) = self.start_autonomous_host_loops_from_roster().await {
                                if let Err(stop_error) = self.stop_all_autonomous_host_loops().await {
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
                        Ok(()) => self.handle_complete().await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::Destroy { reply_tx } => {
                    let result = match self.state() {
                        MobState::Running | MobState::Stopped | MobState::Completed => {
                            self.handle_destroy().await
                        }
                        current => Err(MobError::InvalidTransition {
                            from: current,
                            to: MobState::Destroyed,
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
                    let result = match self
                        .expect_state(&[MobState::Running, MobState::Creating], MobState::Running)
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
                    let result = match self
                        .expect_state(&[MobState::Running, MobState::Creating], MobState::Running)
                    {
                        Ok(()) => self.handle_task_update(task_id, status, owner).await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::Shutdown { reply_tx } => {
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
                    self.state.store(MobState::Stopped as u8, Ordering::Release);
                    let _ = reply_tx.send(result);
                    break;
                }
            }
        }
    }

    /// P1-T04: spawn() creates a real session.
    ///
    /// Effect-first ordering: create session -> emit event -> apply wiring -> update roster.
    async fn handle_spawn(
        &self,
        profile_name: ProfileName,
        meerkat_id: MeerkatId,
        initial_message: Option<String>,
        runtime_mode: Option<crate::MobRuntimeMode>,
        backend: Option<MobBackendKind>,
    ) -> Result<MemberRef, MobError> {
        if meerkat_id
            .as_str()
            .starts_with(FLOW_SYSTEM_MEMBER_ID_PREFIX)
        {
            return Err(MobError::WiringError(format!(
                "meerkat id '{}' uses reserved system prefix '{}'",
                meerkat_id, FLOW_SYSTEM_MEMBER_ID_PREFIX
            )));
        }
        tracing::debug!(
            mob_id = %self.definition.id,
            meerkat_id = %meerkat_id,
            profile = %profile_name,
            "MobActor::handle_spawn start"
        );
        // Check meerkat doesn't already exist
        {
            let roster = self.roster.read().await;
            if roster.get(&meerkat_id).is_some() {
                return Err(MobError::MeerkatAlreadyExists(meerkat_id));
            }
        }

        // Resolve profile
        let profile = self
            .definition
            .profiles
            .get(&profile_name)
            .ok_or_else(|| MobError::ProfileNotFound(profile_name.clone()))?;

        // Build agent config
        let external_tools = self.external_tools_for_profile(profile)?;
        let mut config = build::build_agent_config(
            &self.definition.id,
            &profile_name,
            &meerkat_id,
            profile,
            &self.definition,
            external_tools,
        )
        .await?;

        // Inject default LLM client if set
        if let Some(ref client) = self.default_llm_client {
            config.llm_client_override = Some(client.clone());
        }

        // Create the session via SessionService (effect-first)
        let prompt = initial_message
            .clone()
            .unwrap_or_else(|| self.fallback_spawn_prompt(&profile_name, &meerkat_id));
        let req = build::to_create_session_request(&config, prompt.clone());
        let selected_backend = backend
            .or(profile.backend)
            .unwrap_or(self.definition.backend.default);
        let selected_runtime_mode = runtime_mode.unwrap_or(profile.runtime_mode);
        let peer_name = format!("{}/{}/{}", self.definition.id, profile_name, meerkat_id);
        tracing::debug!(
            selected_backend = ?selected_backend,
            peer_name = %peer_name,
            "MobActor::handle_spawn provisioning member"
        );
        let member_ref = self
            .provisioner
            .provision_member(ProvisionMemberRequest {
                create_session: req,
                backend: selected_backend,
                peer_name,
            })
            .await?;
        tracing::debug!(
            member_ref = ?member_ref,
            "MobActor::handle_spawn provisioned member"
        );

        if selected_runtime_mode == crate::MobRuntimeMode::AutonomousHost
            && let Err(capability_error) = self
                .ensure_autonomous_dispatch_capability(&meerkat_id, &member_ref)
                .await
        {
            if let Err(retire_error) = self.provisioner.retire_member(&member_ref).await {
                return Err(MobError::Internal(format!(
                    "autonomous capability check failed for '{meerkat_id}': {capability_error}; cleanup retire failed for member '{member_ref:?}': {retire_error}"
                )));
            }
            return Err(capability_error);
        }

        // Emit MeerkatSpawned event
        if let Err(append_error) = self
            .events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MeerkatSpawned {
                    meerkat_id: meerkat_id.clone(),
                    role: profile_name.clone(),
                    runtime_mode: selected_runtime_mode,
                    member_ref: member_ref.clone(),
                },
            })
            .await
        {
            if let Err(rollback_error) = self.provisioner.retire_member(&member_ref).await {
                return Err(MobError::Internal(format!(
                    "spawn append failed for '{meerkat_id}': {append_error}; archive compensation failed for member '{member_ref:?}': {rollback_error}"
                )));
            }
            return Err(append_error);
        }

        // Update roster
        {
            let mut roster = self.roster.write().await;
            let inserted = roster.add(
                meerkat_id.clone(),
                profile_name.clone(),
                selected_runtime_mode,
                member_ref.clone(),
            );
            debug_assert!(
                inserted,
                "duplicate meerkat insert should be prevented before add()"
            );
        }
        tracing::debug!(
            meerkat_id = %meerkat_id,
            "MobActor::handle_spawn roster updated"
        );

        let planned_wiring_targets = self.spawn_wiring_targets(&profile_name, &meerkat_id).await;

        if let Err(wiring_error) = self.apply_spawn_wiring(&profile_name, &meerkat_id).await {
            if let Err(rollback_error) = self
                .rollback_failed_spawn(
                    &meerkat_id,
                    &profile_name,
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

        if selected_runtime_mode == crate::MobRuntimeMode::AutonomousHost
            && let Err(start_error) = self
                .start_autonomous_host_loop(&meerkat_id, &member_ref, prompt)
                .await
        {
            if let Err(rollback_error) = self
                .rollback_failed_spawn(
                    &meerkat_id,
                    &profile_name,
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
            "MobActor::handle_spawn done"
        );
        Ok(member_ref)
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
    /// Append retire event -> notify wired peers -> remove trust -> archive session -> update roster.
    ///
    /// Event-first ordering ensures append failures remain retryable without
    /// committing side effects that cannot be replay-projected.
    async fn handle_retire(&self, meerkat_id: MeerkatId) -> Result<(), MobError> {
        // Idempotent: already retired / never existed is success.
        let entry = {
            let roster = self.roster.read().await;
            let Some(entry) = roster.get(&meerkat_id).cloned() else {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    meerkat_id = %meerkat_id,
                    "retire requested for unknown meerkat id"
                );
                return Ok(());
            };
            entry
        };

        let retire_event_already_present = self
            .retire_event_exists(&meerkat_id, &entry.member_ref)
            .await?;
        if !retire_event_already_present {
            self.append_retire_event(&meerkat_id, &entry.profile, &entry.member_ref)
                .await?;
        }
        if entry.runtime_mode == crate::MobRuntimeMode::AutonomousHost {
            self.stop_autonomous_host_loop_for_member(&meerkat_id, &entry.member_ref)
                .await?;
        }

        // Notify wired peers and remove trust.
        if !entry.wired_to.is_empty() {
            let retiring_comms = self.provisioner_comms(&entry.member_ref).await;
            let retiring_key = retiring_comms.as_ref().and_then(|comms| comms.public_key());

            // If a retire event already exists, this can be a retry path after a
            // prior partial cleanup. Missing comms in that case should not block
            // roster convergence.
            let Some(retiring_comms) = retiring_comms else {
                if !retire_event_already_present {
                    return Err(MobError::WiringError(format!(
                        "retire requires comms runtime for '{meerkat_id}'"
                    )));
                }
                if let Err(error) = self.provisioner.retire_member(&entry.member_ref).await
                    && !matches!(
                        error,
                        MobError::SessionError(
                            meerkat_core::service::SessionError::NotFound { .. }
                        )
                    )
                {
                    return Err(error);
                }
                let mut roster = self.roster.write().await;
                roster.remove(&meerkat_id);
                return Ok(());
            };
            let Some(retiring_key) = retiring_key else {
                if !retire_event_already_present {
                    return Err(MobError::WiringError(format!(
                        "retire requires public key for '{meerkat_id}'"
                    )));
                }
                if let Err(error) = self.provisioner.retire_member(&entry.member_ref).await
                    && !matches!(
                        error,
                        MobError::SessionError(
                            meerkat_core::service::SessionError::NotFound { .. }
                        )
                    )
                {
                    return Err(error);
                }
                let mut roster = self.roster.write().await;
                roster.remove(&meerkat_id);
                return Ok(());
            };
            let retiring_comms_name = self.comms_name_for(&entry);
            let retiring_spec = self
                .provisioner
                .trusted_peer_spec(&entry.member_ref, &retiring_comms_name, &retiring_key)
                .await?;
            let mut rollback = LifecycleRollback::new("retire");

            // Notify wired peers about retirement (required for successful retire).
            for peer_id in &entry.wired_to {
                let peer_comms_name = {
                    let roster = self.roster.read().await;
                    roster
                        .get(peer_id)
                        .map(|peer_entry| self.comms_name_for(peer_entry))
                        .ok_or_else(|| {
                            MobError::WiringError(format!(
                                "retire requires roster entry for wired peer '{peer_id}'"
                            ))
                        })?
                };
                self.notify_peer_retired(peer_id, &meerkat_id, &entry, &retiring_comms)
                    .await?;
                rollback.defer(
                    format!("compensating mob.peer_added '{meerkat_id}' -> '{peer_id}'"),
                    {
                        let retiring_comms = retiring_comms.clone();
                        let peer_comms_name = peer_comms_name.clone();
                        let entry = entry.clone();
                        let meerkat_id = meerkat_id.clone();
                        move || async move {
                            self.notify_peer_added(
                                &retiring_comms,
                                &peer_comms_name,
                                &meerkat_id,
                                &entry,
                            )
                            .await
                        }
                    },
                );
            }

            // Remove trust from wired peers
            for peer_meerkat_id in &entry.wired_to {
                let peer_entry = {
                    let roster = self.roster.read().await;
                    roster.get(peer_meerkat_id).cloned().ok_or_else(|| {
                        MobError::WiringError(format!(
                            "retire cannot remove trust for '{meerkat_id}': peer '{peer_meerkat_id}' missing from roster"
                        ))
                    })?
                };
                let peer_comms = self
                    .provisioner_comms(&peer_entry.member_ref)
                    .await
                    .ok_or_else(|| {
                        MobError::WiringError(format!(
                            "retire cannot remove trust for '{meerkat_id}': comms runtime missing for peer '{peer_meerkat_id}'"
                        ))
                    })?;
                if let Err(error) = peer_comms.remove_trusted_peer(&retiring_key).await {
                    return Err(rollback.fail(error.into()).await);
                }
                rollback.defer(
                    format!("restore trust '{peer_meerkat_id}' -> '{meerkat_id}'"),
                    {
                        let peer_comms = peer_comms.clone();
                        let retiring_spec = retiring_spec.clone();
                        move || async move {
                            peer_comms.add_trusted_peer(retiring_spec).await?;
                            Ok(())
                        }
                    },
                );
            }

            if let Err(error) = self.provisioner.retire_member(&entry.member_ref).await
                && !matches!(
                    error,
                    MobError::SessionError(meerkat_core::service::SessionError::NotFound { .. })
                )
            {
                return Err(rollback.fail(error).await);
            }
        } else if let Err(error) = self.provisioner.retire_member(&entry.member_ref).await
            && !matches!(
                error,
                MobError::SessionError(meerkat_core::service::SessionError::NotFound { .. })
            )
        {
            return Err(error);
        }

        // Update roster
        {
            let mut roster = self.roster.write().await;
            roster.remove(&meerkat_id);
        }

        Ok(())
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

        Ok(())
    }

    async fn handle_complete(&mut self) -> Result<(), MobError> {
        self.cancel_all_flow_tasks().await;
        self.notify_orchestrator_lifecycle(format!("Mob '{}' is completing.", self.definition.id))
            .await;
        let ids = {
            let roster = self.roster.read().await;
            roster
                .list()
                .map(|entry| entry.meerkat_id.clone())
                .collect::<Vec<_>>()
        };
        for id in ids {
            self.handle_retire(id).await?;
        }
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
        let ids = {
            let roster = self.roster.read().await;
            roster
                .list()
                .map(|entry| entry.meerkat_id.clone())
                .collect::<Vec<_>>()
        };
        for id in ids {
            self.handle_retire(id).await?;
        }
        self.stop_mcp_servers().await?;
        self.events.clear().await?;
        self.cleanup_namespace().await?;
        self.state
            .store(MobState::Destroyed as u8, Ordering::Release);
        Ok(())
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
    async fn handle_external_turn(
        &self,
        meerkat_id: MeerkatId,
        message: String,
    ) -> Result<(), MobError> {
        // Look up the entry
        let entry = {
            let roster = self.roster.read().await;
            roster
                .get(&meerkat_id)
                .cloned()
                .ok_or_else(|| MobError::MeerkatNotFound(meerkat_id.clone()))?
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

    async fn dispatch_member_turn(&self, entry: &RosterEntry, message: String) -> Result<(), MobError> {
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
                };
                self.provisioner.start_turn(&entry.member_ref, req).await
            }
        }
    }

    async fn handle_run_flow(
        &mut self,
        flow_id: FlowId,
        activation_params: serde_json::Value,
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
            .map(std::time::Duration::from_millis)
            .unwrap_or_else(|| std::time::Duration::from_secs(5));
        tokio::spawn(async move {
            let completed = tokio::select! {
                _ = &mut handle => true,
                _ = tokio::time::sleep(cancel_grace_timeout) => false,
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
    async fn apply_spawn_wiring(
        &self,
        profile_name: &ProfileName,
        meerkat_id: &MeerkatId,
    ) -> Result<(), MobError> {
        // P1-T08: Auto-wire to orchestrator if configured
        if self.definition.wiring.auto_wire_orchestrator
            && let Some(ref orch) = self.definition.orchestrator
        {
            let orch_id = {
                let roster = self.roster.read().await;
                roster
                    .by_profile(&orch.profile)
                    .next()
                    .map(|e| e.meerkat_id.clone())
            };
            if let Some(ref orch_meerkat_id) = orch_id
                && *orch_meerkat_id != *meerkat_id
            {
                self.do_wire(meerkat_id, orch_meerkat_id).await.map_err(|e| {
                    MobError::WiringError(format!(
                        "auto_wire_orchestrator failed for {meerkat_id} <-> {orch_meerkat_id}: {e}"
                    ))
                })?;
            }
        }

        // P1-T09: Role-wiring fan-out
        for rule in &self.definition.wiring.role_wiring {
            let target_role = if *profile_name == rule.a {
                Some(&rule.b)
            } else if *profile_name == rule.b {
                Some(&rule.a)
            } else {
                None
            };

            if let Some(target) = target_role {
                let target_ids: Vec<MeerkatId> = {
                    let roster = self.roster.read().await;
                    roster
                        .by_profile(target)
                        .filter(|entry| entry.meerkat_id != *meerkat_id)
                        .map(|entry| entry.meerkat_id.clone())
                        .collect()
                };

                for target_id in target_ids {
                    self.do_wire(meerkat_id, &target_id).await.map_err(|e| {
                        MobError::WiringError(format!(
                            "role_wiring fan-out failed for {meerkat_id} <-> {target_id}: {e}"
                        ))
                    })?;
                }
            }
        }

        Ok(())
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

        if let Err(error) = self.provisioner.retire_member(member_ref).await
            && !matches!(
                error,
                MobError::SessionError(meerkat_core::service::SessionError::NotFound { .. })
            )
        {
            return Err(rollback.fail(error).await);
        }

        {
            let mut roster = self.roster.write().await;
            roster.remove(meerkat_id);
        }

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
            .map(|p| p.peer_description.as_str())
            .unwrap_or("");

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

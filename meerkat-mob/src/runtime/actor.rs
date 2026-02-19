use super::*;

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
    pub(super) session_service: Arc<dyn MobSessionService>,
    pub(super) mcp_running: Arc<RwLock<BTreeMap<String, bool>>>,
    pub(super) mcp_processes: Arc<tokio::sync::Mutex<BTreeMap<String, Child>>>,
    pub(super) command_tx: mpsc::Sender<MobCommand>,
    pub(super) tool_bundles: BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
    pub(super) default_llm_client: Option<Arc<dyn LlmClient>>,
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
            let _ = self
                .session_service
                .start_turn(
                    &orchestrator_entry.session_id,
                    meerkat_core::service::StartTurnRequest {
                        prompt: message,
                        event_tx: None,
                        host_mode: true,
                        skill_references: None,
                    },
                )
                .await;
        }
    }

    async fn stop_mcp_servers(&self) -> Result<(), MobError> {
        let mut processes = self.mcp_processes.lock().await;
        for (name, child) in processes.iter_mut() {
            child.kill().await.map_err(|error| {
                MobError::Internal(format!("failed to stop mcp server '{name}': {error}"))
            })?;
            let _ = child.wait().await.map_err(|error| {
                MobError::Internal(format!(
                    "failed waiting for mcp server '{name}' to exit: {error}"
                ))
            })?;
        }
        processes.clear();

        let mut running = self.mcp_running.write().await;
        for state in running.values_mut() {
            *state = false;
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

    /// Main actor loop: process commands sequentially until Shutdown.
    pub(super) async fn run(self, mut command_rx: mpsc::Receiver<MobCommand>) {
        if matches!(self.state(), MobState::Running)
            && let Err(error) = self.start_mcp_servers().await
        {
            tracing::error!(
                mob_id = %self.definition.id,
                error = %error,
                "failed to start mcp servers during actor startup"
            );
        }
        while let Some(cmd) = command_rx.recv().await {
            match cmd {
                MobCommand::Spawn {
                    profile_name,
                    meerkat_id,
                    initial_message,
                    reply_tx,
                } => {
                    let result = match self
                        .expect_state(&[MobState::Running, MobState::Creating], MobState::Running)
                    {
                        Ok(()) => {
                            self.handle_spawn(profile_name, meerkat_id, initial_message)
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
                MobCommand::Stop { reply_tx } => {
                    let result = match self.expect_state(&[MobState::Running], MobState::Stopped) {
                        Ok(()) => {
                            self.notify_orchestrator_lifecycle(format!(
                                "Mob '{}' is stopping.",
                                self.definition.id
                            ))
                            .await;
                            if let Err(error) = self.stop_mcp_servers().await {
                                Err(error)
                            } else {
                                self.state.store(MobState::Stopped as u8, Ordering::Release);
                                Ok(())
                            }
                        }
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::ResumeLifecycle { reply_tx } => {
                    let result = match self.expect_state(&[MobState::Stopped], MobState::Running) {
                        Ok(()) => {
                            if let Err(error) = self.start_mcp_servers().await {
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
                    self.state.store(MobState::Stopped as u8, Ordering::Release);
                    let _ = reply_tx.send(());
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
    ) -> Result<SessionId, MobError> {
        // Check meerkat doesn't already exist
        {
            let roster = self.roster.read().await;
            if roster.get(&meerkat_id).is_some() {
                return Err(MobError::MeerkatAlreadyExists(meerkat_id.to_string()));
            }
        }

        // Resolve profile
        let profile = self
            .definition
            .profiles
            .get(&profile_name)
            .ok_or_else(|| MobError::ProfileNotFound(profile_name.to_string()))?;

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
        let prompt = initial_message.unwrap_or_else(|| {
            format!(
                "You have been spawned as '{}' (role: {}) in mob '{}'.",
                meerkat_id, profile_name, self.definition.id
            )
        });
        let req = build::to_create_session_request(&config, prompt);
        let result = self.session_service.create_session(req).await?;
        let session_id = result.session_id;

        // Emit MeerkatSpawned event
        if let Err(append_error) = self
            .events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MeerkatSpawned {
                    meerkat_id: meerkat_id.clone(),
                    role: profile_name.clone(),
                    session_id: session_id.clone(),
                },
            })
            .await
        {
            if let Err(rollback_error) = self.session_service.archive(&session_id).await {
                return Err(MobError::Internal(format!(
                    "spawn append failed for '{meerkat_id}': {append_error}; archive compensation failed for session '{session_id}': {rollback_error}"
                )));
            }
            return Err(append_error);
        }

        // Update roster
        {
            let mut roster = self.roster.write().await;
            let inserted = roster.add(meerkat_id.clone(), profile_name.clone(), session_id.clone());
            debug_assert!(
                inserted,
                "duplicate meerkat insert should be prevented before add()"
            );
        }

        let planned_wiring_targets = self.spawn_wiring_targets(&profile_name, &meerkat_id).await;

        if let Err(wiring_error) = self.apply_spawn_wiring(&profile_name, &meerkat_id).await {
            if let Err(rollback_error) = self
                .rollback_failed_spawn(
                    &meerkat_id,
                    &profile_name,
                    &session_id,
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

        Ok(session_id)
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
            .retire_event_exists(&meerkat_id, &entry.session_id)
            .await?;
        if !retire_event_already_present {
            self.append_retire_event(&meerkat_id, &entry.profile, &entry.session_id)
                .await?;
        }

        // Notify wired peers and remove trust.
        if !entry.wired_to.is_empty() {
            let retiring_comms = self.session_service_comms(&entry.session_id).await;
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
                if let Err(error) = self.session_service.archive(&entry.session_id).await
                    && !matches!(error, meerkat_core::service::SessionError::NotFound { .. })
                {
                    return Err(error.into());
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
                if let Err(error) = self.session_service.archive(&entry.session_id).await
                    && !matches!(error, meerkat_core::service::SessionError::NotFound { .. })
                {
                    return Err(error.into());
                }
                let mut roster = self.roster.write().await;
                roster.remove(&meerkat_id);
                return Ok(());
            };

            // Notify wired peers about retirement (required for successful retire).
            for peer_id in &entry.wired_to {
                self.notify_peer_retired(peer_id, &meerkat_id, &entry, &retiring_comms)
                    .await?;
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
                    .session_service_comms(&peer_entry.session_id)
                    .await
                    .ok_or_else(|| {
                        MobError::WiringError(format!(
                            "retire cannot remove trust for '{meerkat_id}': comms runtime missing for peer '{peer_meerkat_id}'"
                        ))
                    })?;
                peer_comms.remove_trusted_peer(&retiring_key).await?;
            }
        }

        // Archive session (required)
        if let Err(error) = self.session_service.archive(&entry.session_id).await
            && !matches!(error, meerkat_core::service::SessionError::NotFound { .. })
        {
            return Err(error.into());
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
                return Err(MobError::MeerkatNotFound(a.to_string()));
            }
            if roster.get(&b).is_none() {
                return Err(MobError::MeerkatNotFound(b.to_string()));
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
                .ok_or_else(|| MobError::MeerkatNotFound(a.to_string()))?;
            let eb = roster
                .get(&b)
                .cloned()
                .ok_or_else(|| MobError::MeerkatNotFound(b.to_string()))?;
            (ea, eb)
        };

        // Get comms and keys for both sides (required for unwire).
        let comms_a = self
            .session_service_comms(&entry_a.session_id)
            .await
            .ok_or_else(|| {
                MobError::WiringError(format!("unwire requires comms runtime for '{a}'"))
            })?;
        let comms_b = self
            .session_service_comms(&entry_b.session_id)
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
        let spec_a = TrustedPeerSpec::new(
            &comms_name_a,
            key_a.clone(),
            format!("inproc://{comms_name_a}"),
        )
        .map_err(|e| MobError::WiringError(format!("invalid peer spec: {e}")))?;
        let spec_b = TrustedPeerSpec::new(
            &comms_name_b,
            key_b.clone(),
            format!("inproc://{comms_name_b}"),
        )
        .map_err(|e| MobError::WiringError(format!("invalid peer spec: {e}")))?;

        // Notify both peers BEFORE removing trust (need trust to send).
        // Send FROM a TO b: notify b that a is being unwired
        self.notify_peer_unwired(&b, &a, &entry_a, &comms_a).await?;
        // Send FROM b TO a: notify a that b is being unwired
        if let Err(second_notification_error) =
            self.notify_peer_unwired(&a, &b, &entry_b, &comms_b).await
        {
            if let Err(compensation_error) = self
                .notify_peer_added(&comms_a, &comms_name_b, &a, &entry_a)
                .await
            {
                return Err(MobError::Internal(format!(
                    "unwire second notification failed for '{b}' -> '{a}': {second_notification_error}; compensating mob.peer_added '{a}' -> '{b}' failed: {compensation_error}"
                )));
            }
            return Err(second_notification_error);
        }

        // Remove trust on both sides (required)
        if let Err(remove_a_error) = comms_a.remove_trusted_peer(&key_b).await {
            let mut cleanup_errors = Vec::new();
            if let Err(compensation_error) = self
                .notify_peer_added(&comms_b, &comms_name_a, &b, &entry_b)
                .await
            {
                cleanup_errors.push(format!(
                    "compensating mob.peer_added '{b}' -> '{a}' failed: {compensation_error}"
                ));
            }
            if let Err(compensation_error) = self
                .notify_peer_added(&comms_a, &comms_name_b, &a, &entry_a)
                .await
            {
                cleanup_errors.push(format!(
                    "compensating mob.peer_added '{a}' -> '{b}' failed: {compensation_error}"
                ));
            }
            if !cleanup_errors.is_empty() {
                return Err(MobError::Internal(format!(
                    "unwire failed removing trust '{a}' -> '{b}': {remove_a_error}; rollback failures: {}",
                    cleanup_errors.join("; ")
                )));
            }
            return Err(remove_a_error.into());
        }
        if let Err(remove_b_error) = comms_b.remove_trusted_peer(&key_a).await {
            let mut cleanup_errors = Vec::new();
            if let Err(restore_error) = comms_a.add_trusted_peer(spec_b.clone()).await {
                cleanup_errors.push(format!(
                    "restore trust '{a}' -> '{b}' failed after second removal failure: {restore_error}"
                ));
            }
            if let Err(compensation_error) = self
                .notify_peer_added(&comms_b, &comms_name_a, &b, &entry_b)
                .await
            {
                cleanup_errors.push(format!(
                    "compensating mob.peer_added '{b}' -> '{a}' failed: {compensation_error}"
                ));
            }
            if let Err(compensation_error) = self
                .notify_peer_added(&comms_a, &comms_name_b, &a, &entry_a)
                .await
            {
                cleanup_errors.push(format!(
                    "compensating mob.peer_added '{a}' -> '{b}' failed: {compensation_error}"
                ));
            }
            if !cleanup_errors.is_empty() {
                return Err(MobError::Internal(format!(
                    "unwire failed removing trust '{b}' -> '{a}': {remove_b_error}; rollback failures: {}",
                    cleanup_errors.join("; ")
                )));
            }
            return Err(remove_b_error.into());
        }

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
            let mut cleanup_errors = Vec::new();
            if let Err(error) = comms_a.add_trusted_peer(spec_b).await {
                cleanup_errors.push(format!(
                    "restore trust '{a}' -> '{b}' failed during append rollback: {error}"
                ));
            }
            if let Err(error) = comms_b.add_trusted_peer(spec_a).await {
                cleanup_errors.push(format!(
                    "restore trust '{b}' -> '{a}' failed during append rollback: {error}"
                ));
            }
            if let Err(error) = self
                .notify_peer_added(&comms_b, &comms_name_a, &b, &entry_b)
                .await
            {
                cleanup_errors.push(format!(
                    "compensating mob.peer_added '{b}' -> '{a}' failed during append rollback: {error}"
                ));
            }
            if let Err(error) = self
                .notify_peer_added(&comms_a, &comms_name_b, &a, &entry_a)
                .await
            {
                cleanup_errors.push(format!(
                    "compensating mob.peer_added '{a}' -> '{b}' failed during append rollback: {error}"
                ));
            }
            if !cleanup_errors.is_empty() {
                return Err(MobError::Internal(format!(
                    "unwire append failed for '{a}' <-> '{b}': {append_error}; rollback failures: {}",
                    cleanup_errors.join("; ")
                )));
            }
            return Err(append_error);
        }

        // Update roster
        {
            let mut roster = self.roster.write().await;
            roster.unwire(&a, &b);
        }

        Ok(())
    }

    async fn handle_complete(&self) -> Result<(), MobError> {
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

    async fn handle_destroy(&self) -> Result<(), MobError> {
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
        blocked_by: Vec<String>,
    ) -> Result<String, MobError> {
        if subject.trim().is_empty() {
            return Err(MobError::Internal(
                "task subject cannot be empty".to_string(),
            ));
        }

        let task_id = uuid::Uuid::new_v4().to_string();

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
        task_id: String,
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
                .ok_or_else(|| MobError::MeerkatNotFound(meerkat_id.to_string()))?
        };

        // Check external_addressable
        let profile = self
            .definition
            .profiles
            .get(&entry.profile)
            .ok_or_else(|| MobError::ProfileNotFound(entry.profile.to_string()))?;

        if !profile.external_addressable {
            return Err(MobError::NotExternallyAddressable(meerkat_id.to_string()));
        }

        // Start a turn on the session
        let req = meerkat_core::service::StartTurnRequest {
            prompt: message,
            event_tx: None,
            host_mode: false,
            skill_references: None,
        };
        self.session_service
            .start_turn(&entry.session_id, req)
            .await?;

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
                .ok_or_else(|| MobError::MeerkatNotFound(meerkat_id.to_string()))?
        };

        let req = meerkat_core::service::StartTurnRequest {
            prompt: message,
            event_tx: None,
            host_mode: false,
            skill_references: None,
        };
        self.session_service
            .start_turn(&entry.session_id, req)
            .await?;
        Ok(())
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
        session_id: &SessionId,
        planned_wiring_targets: &[MeerkatId],
    ) -> Result<(), MobError> {
        let retire_event_already_present = self.retire_event_exists(meerkat_id, session_id).await?;
        if !retire_event_already_present {
            self.append_retire_event(meerkat_id, profile_name, session_id)
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
        let spawned_comms = self.session_service_comms(session_id).await;

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
                self.notify_peer_retired(peer_meerkat_id, meerkat_id, spawned_entry, spawned_comms)
                    .await?;
            }
        }

        let spawned_key = spawned_comms.as_ref().and_then(|comms| comms.public_key());
        if let Some(spawned_key) = spawned_key {
            let mut cleanup_errors = Vec::new();
            for peer_meerkat_id in &cleanup_peers {
                let is_wired_peer = wired_peers.contains(peer_meerkat_id);
                let peer_entry = {
                    let roster = self.roster.read().await;
                    roster.get(peer_meerkat_id).cloned()
                };
                let Some(peer_entry) = peer_entry else {
                    if is_wired_peer {
                        cleanup_errors.push(format!(
                            "spawn rollback cannot remove trust for '{meerkat_id}': wired peer '{peer_meerkat_id}' missing from roster"
                        ));
                    }
                    continue;
                };
                let peer_comms = self.session_service_comms(&peer_entry.session_id).await;
                let Some(peer_comms) = peer_comms else {
                    if is_wired_peer {
                        cleanup_errors.push(format!(
                            "spawn rollback cannot remove trust for '{meerkat_id}': comms runtime missing for wired peer '{peer_meerkat_id}'"
                        ));
                    }
                    continue;
                };
                if let Err(error) = peer_comms.remove_trusted_peer(&spawned_key).await
                    && is_wired_peer
                {
                    cleanup_errors.push(format!(
                        "spawn rollback cannot remove trust for '{meerkat_id}' from wired peer '{peer_meerkat_id}': {error}"
                    ));
                }
            }
            if !cleanup_errors.is_empty() {
                return Err(MobError::Internal(format!(
                    "spawn rollback cleanup failed for '{meerkat_id}': {}",
                    cleanup_errors.join("; ")
                )));
            }
        }

        if let Err(error) = self.session_service.archive(session_id).await
            && !matches!(error, meerkat_core::service::SessionError::NotFound { .. })
        {
            return Err(error.into());
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
        session_id: &SessionId,
    ) -> Result<bool, MobError> {
        let events = self.events.replay_all().await?;
        Ok(events.iter().any(|event| {
            matches!(
                &event.kind,
                MobEventKind::MeerkatRetired {
                    meerkat_id: existing_meerkat,
                    session_id: existing_session,
                    ..
                } if existing_meerkat == meerkat_id && existing_session == session_id
            )
        }))
    }

    async fn append_retire_event(
        &self,
        meerkat_id: &MeerkatId,
        profile_name: &ProfileName,
        session_id: &SessionId,
    ) -> Result<(), MobError> {
        self.events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MeerkatRetired {
                    meerkat_id: meerkat_id.clone(),
                    role: profile_name.clone(),
                    session_id: session_id.clone(),
                },
            })
            .await?;
        Ok(())
    }

    /// Internal wire operation (used by handle_wire and auto_wire/role_wiring).
    async fn do_wire(&self, a: &MeerkatId, b: &MeerkatId) -> Result<(), MobError> {
        let (entry_a, entry_b) = {
            let roster = self.roster.read().await;
            let ea = roster
                .get(a)
                .cloned()
                .ok_or_else(|| MobError::MeerkatNotFound(a.to_string()))?;
            let eb = roster
                .get(b)
                .cloned()
                .ok_or_else(|| MobError::MeerkatNotFound(b.to_string()))?;
            (ea, eb)
        };

        // Establish bidirectional trust via comms (required).
        let comms_a = self
            .session_service_comms(&entry_a.session_id)
            .await
            .ok_or_else(|| {
                MobError::WiringError(format!("wire requires comms runtime for '{a}'"))
            })?;
        let comms_b = self
            .session_service_comms(&entry_b.session_id)
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

        let spec_b = TrustedPeerSpec::new(
            &comms_name_b,
            key_b.clone(),
            format!("inproc://{comms_name_b}"),
        )
        .map_err(|e| MobError::WiringError(format!("invalid peer spec: {e}")))?;
        let spec_a = TrustedPeerSpec::new(
            &comms_name_a,
            key_a.clone(),
            format!("inproc://{comms_name_a}"),
        )
        .map_err(|e| MobError::WiringError(format!("invalid peer spec: {e}")))?;

        comms_a.add_trusted_peer(spec_b.clone()).await?;
        if let Err(error) = comms_b.add_trusted_peer(spec_a.clone()).await {
            if let Err(cleanup_error) = comms_a.remove_trusted_peer(&key_b).await {
                return Err(MobError::Internal(format!(
                    "wire failed adding trust '{b}' -> '{a}': {error}; rollback failed removing trust '{a}' -> '{b}': {cleanup_error}"
                )));
            }
            return Err(error.into());
        }

        // Notify both peers (required for successful wire):
        // Send FROM b TO a about new peer b
        if let Err(error) = self
            .notify_peer_added(&comms_b, &comms_name_a, b, &entry_b)
            .await
        {
            let mut cleanup_errors = Vec::new();
            if let Err(cleanup_error) = comms_a.remove_trusted_peer(&key_b).await {
                cleanup_errors.push(format!(
                    "remove trust '{a}' -> '{b}' failed during rollback: {cleanup_error}"
                ));
            }
            if let Err(cleanup_error) = comms_b.remove_trusted_peer(&key_a).await {
                cleanup_errors.push(format!(
                    "remove trust '{b}' -> '{a}' failed during rollback: {cleanup_error}"
                ));
            }
            if !cleanup_errors.is_empty() {
                return Err(MobError::Internal(format!(
                    "wire failed sending mob.peer_added '{b}' -> '{a}': {error}; rollback failures: {}",
                    cleanup_errors.join("; ")
                )));
            }
            return Err(error);
        }
        // Send FROM a TO b about new peer a
        if let Err(error) = self
            .notify_peer_added(&comms_a, &comms_name_b, a, &entry_a)
            .await
        {
            let mut cleanup_errors = Vec::new();
            if let Err(compensation_error) =
                self.notify_peer_retired(a, b, &entry_b, &comms_b).await
            {
                cleanup_errors.push(format!(
                    "compensating mob.peer_retired '{b}' -> '{a}' failed: {compensation_error}"
                ));
            }
            if let Err(cleanup_error) = comms_a.remove_trusted_peer(&key_b).await {
                cleanup_errors.push(format!(
                    "remove trust '{a}' -> '{b}' failed during rollback: {cleanup_error}"
                ));
            }
            if let Err(cleanup_error) = comms_b.remove_trusted_peer(&key_a).await {
                cleanup_errors.push(format!(
                    "remove trust '{b}' -> '{a}' failed during rollback: {cleanup_error}"
                ));
            }
            if !cleanup_errors.is_empty() {
                return Err(MobError::Internal(format!(
                    "wire failed sending mob.peer_added '{a}' -> '{b}': {error}; rollback failures: {}",
                    cleanup_errors.join("; ")
                )));
            }
            return Err(error);
        }

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
            let mut cleanup_errors = Vec::new();
            if let Err(compensation_error) =
                self.notify_peer_retired(a, b, &entry_b, &comms_b).await
            {
                cleanup_errors.push(format!(
                    "compensating mob.peer_retired '{b}' -> '{a}' failed: {compensation_error}"
                ));
            }
            if let Err(compensation_error) =
                self.notify_peer_retired(b, a, &entry_a, &comms_a).await
            {
                cleanup_errors.push(format!(
                    "compensating mob.peer_retired '{a}' -> '{b}' failed: {compensation_error}"
                ));
            }
            if let Err(cleanup_error) = comms_a.remove_trusted_peer(&key_b).await {
                cleanup_errors.push(format!(
                    "remove trust '{a}' -> '{b}' failed during append rollback: {cleanup_error}"
                ));
            }
            if let Err(cleanup_error) = comms_b.remove_trusted_peer(&key_a).await {
                cleanup_errors.push(format!(
                    "remove trust '{b}' -> '{a}' failed during append rollback: {cleanup_error}"
                ));
            }
            if !cleanup_errors.is_empty() {
                return Err(MobError::Internal(format!(
                    "wire append failed for '{a}' <-> '{b}': {append_error}; rollback failures: {}",
                    cleanup_errors.join("; ")
                )));
            }
            return Err(append_error);
        }

        // Update roster
        {
            let mut roster = self.roster.write().await;
            roster.wire(a, b);
        }

        Ok(())
    }

    /// Get the comms runtime for a session, if available.
    async fn session_service_comms(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn CoreCommsRuntime>> {
        self.session_service.comms_runtime(session_id).await
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

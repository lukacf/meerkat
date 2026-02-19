use super::*;

// ---------------------------------------------------------------------------
// MobBuilder
// ---------------------------------------------------------------------------

/// Builder for creating or resuming a mob.
pub struct MobBuilder {
    mode: BuilderMode,
    storage: MobStorage,
    session_service: Option<Arc<dyn MobSessionService>>,
    allow_ephemeral_sessions: bool,
    tool_bundles: BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
    default_llm_client: Option<Arc<dyn LlmClient>>,
}

enum BuilderMode {
    Create(Arc<MobDefinition>),
    Resume,
}

struct RuntimeWiring {
    roster: Arc<RwLock<Roster>>,
    task_board: Arc<RwLock<TaskBoard>>,
    state: Arc<AtomicU8>,
    mcp_running: Arc<RwLock<BTreeMap<String, bool>>>,
    mcp_processes: Arc<tokio::sync::Mutex<BTreeMap<String, Child>>>,
    command_tx: mpsc::Sender<MobCommand>,
    command_rx: mpsc::Receiver<MobCommand>,
}

impl MobBuilder {
    /// Create a builder for a new mob.
    pub fn new(definition: MobDefinition, storage: MobStorage) -> Self {
        Self {
            mode: BuilderMode::Create(Arc::new(definition)),
            storage,
            session_service: None,
            allow_ephemeral_sessions: false,
            tool_bundles: BTreeMap::new(),
            default_llm_client: None,
        }
    }

    /// Create a builder that resumes a mob from persisted events.
    pub fn for_resume(storage: MobStorage) -> Self {
        Self {
            mode: BuilderMode::Resume,
            storage,
            session_service: None,
            allow_ephemeral_sessions: false,
            tool_bundles: BTreeMap::new(),
            default_llm_client: None,
        }
    }

    /// Set the session service for creating meerkat sessions.
    ///
    /// The service must implement both `SessionService` and `MobSessionService`
    /// to provide comms runtime access for wiring operations.
    pub fn with_session_service(mut self, service: Arc<dyn MobSessionService>) -> Self {
        self.session_service = Some(service);
        self
    }

    /// Allow non-persistent session services (for ephemeral/dev/test workflows).
    ///
    /// Default is `false`, which enforces the persistent-session contract.
    pub fn allow_ephemeral_sessions(mut self, allow: bool) -> Self {
        self.allow_ephemeral_sessions = allow;
        self
    }

    /// Set a default LLM client override (primarily for testing).
    pub fn with_default_llm_client(mut self, client: Arc<dyn LlmClient>) -> Self {
        self.default_llm_client = Some(client);
        self
    }

    /// Register a named Rust tool bundle for profile `tools.rust_bundles` wiring.
    pub fn register_tool_bundle(
        mut self,
        name: impl Into<String>,
        dispatcher: Arc<dyn AgentToolDispatcher>,
    ) -> Self {
        self.tool_bundles.insert(name.into(), dispatcher);
        self
    }

    /// Create the mob: emit MobCreated event, start the actor, return handle.
    pub async fn create(self) -> Result<MobHandle, MobError> {
        let MobBuilder {
            mode,
            storage,
            session_service,
            allow_ephemeral_sessions,
            tool_bundles,
            default_llm_client,
        } = self;

        let definition = match mode {
            BuilderMode::Create(definition) => definition,
            BuilderMode::Resume => {
                return Err(MobError::Internal(
                    "MobBuilder::create() cannot be used with for_resume(); call resume()"
                        .to_string(),
                ));
            }
        };
        let diagnostics = crate::validate::validate_definition(&definition);
        if !diagnostics.is_empty() {
            return Err(MobError::DefinitionError(diagnostics));
        }
        let session_service = session_service
            .ok_or_else(|| MobError::Internal("session_service is required".into()))?;
        if !allow_ephemeral_sessions && !session_service.supports_persistent_sessions() {
            return Err(MobError::Internal(
                "session_service must satisfy persistent-session contract (REQ-MOB-030)"
                    .to_string(),
            ));
        }

        // Emit MobCreated event first
        let definition_for_event = (*definition).clone();
        storage
            .events
            .append(NewMobEvent {
                mob_id: definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MobCreated {
                    definition: definition_for_event,
                },
            })
            .await?;

        Ok(Self::start_runtime(
            definition,
            Roster::new(),
            TaskBoard::default(),
            MobState::Running,
            storage.events.clone(),
            session_service,
            tool_bundles,
            default_llm_client,
        ))
    }

    /// Resume a mob from persisted events.
    ///
    /// Resume behavior:
    /// - Recover definition from `MobCreated`.
    /// - Rebuild roster by replaying structural events.
    /// - Start actor/runtime in Running state.
    pub async fn resume(self) -> Result<MobHandle, MobError> {
        let MobBuilder {
            mode,
            storage,
            session_service,
            allow_ephemeral_sessions,
            tool_bundles,
            default_llm_client,
        } = self;

        if !matches!(mode, BuilderMode::Resume) {
            return Err(MobError::Internal(
                "MobBuilder::resume() requires MobBuilder::for_resume(storage)".to_string(),
            ));
        }

        let session_service = session_service
            .ok_or_else(|| MobError::Internal("session_service is required".into()))?;
        if !allow_ephemeral_sessions && !session_service.supports_persistent_sessions() {
            return Err(MobError::Internal(
                "session_service must satisfy persistent-session contract (REQ-MOB-030)"
                    .to_string(),
            ));
        }
        let all_events = storage.events.replay_all().await?;

        let definition = all_events
            .iter()
            .find_map(|event| match &event.kind {
                MobEventKind::MobCreated { definition } => Some(definition.clone()),
                _ => None,
            })
            .ok_or_else(|| {
                MobError::Internal(
                    "cannot resume mob: no MobCreated event found in storage".to_string(),
                )
            })?;

        let definition = Arc::new(definition);
        let diagnostics = crate::validate::validate_definition(&definition);
        if !diagnostics.is_empty() {
            return Err(MobError::DefinitionError(diagnostics));
        }
        let mob_events: Vec<_> = all_events
            .into_iter()
            .filter(|event| event.mob_id == definition.id)
            .collect();
        let mut roster = Roster::project(&mob_events);
        let task_board = TaskBoard::project(&mob_events);
        let resumed_state = if mob_events
            .iter()
            .any(|event| matches!(event.kind, MobEventKind::MobCompleted))
        {
            MobState::Completed
        } else {
            MobState::Running
        };

        // Prepare shared runtime components early so resume reconciliation can
        // wire tool dispatchers for recreated sessions to the final actor channel.
        let roster_state = Arc::new(RwLock::new(Roster::new()));
        let task_board_state = Arc::new(RwLock::new(TaskBoard::default()));
        let state = Arc::new(AtomicU8::new(resumed_state as u8));
        let mcp_servers_running = false;
        let mcp_running = Arc::new(RwLock::new(
            definition
                .mcp_servers
                .keys()
                .map(|name| (name.clone(), mcp_servers_running))
                .collect::<BTreeMap<_, _>>(),
        ));
        let mcp_processes = Arc::new(tokio::sync::Mutex::new(BTreeMap::new()));
        let (command_tx, command_rx) = mpsc::channel(64);
        let wiring = RuntimeWiring {
            roster: roster_state.clone(),
            task_board: task_board_state.clone(),
            state: state.clone(),
            mcp_running: mcp_running.clone(),
            mcp_processes: mcp_processes.clone(),
            command_tx: command_tx.clone(),
            command_rx,
        };
        let preview_handle = MobHandle {
            command_tx: command_tx.clone(),
            roster: roster_state.clone(),
            task_board: task_board_state.clone(),
            definition: definition.clone(),
            state: state.clone(),
            events: storage.events.clone(),
            mcp_running: mcp_running.clone(),
        };

        if resumed_state == MobState::Running {
            Self::reconcile_resume(
                &definition,
                &mut roster,
                &session_service,
                default_llm_client.clone(),
                &tool_bundles,
                &preview_handle,
            )
            .await?;
        }

        *wiring.roster.write().await = roster;
        *wiring.task_board.write().await = task_board;

        Ok(Self::start_runtime_with_components(
            definition,
            wiring,
            storage.events.clone(),
            session_service,
            tool_bundles,
            default_llm_client,
        ))
    }

    async fn reconcile_resume(
        definition: &Arc<MobDefinition>,
        roster: &mut Roster,
        session_service: &Arc<dyn MobSessionService>,
        default_llm_client: Option<Arc<dyn LlmClient>>,
        tool_bundles: &BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
        tool_handle: &MobHandle,
    ) -> Result<(), MobError> {
        let active_sessions = session_service
            .list(meerkat_core::service::SessionQuery::default())
            .await?;
        let active_ids = active_sessions
            .iter()
            .map(|s| s.session_id.clone())
            .collect::<std::collections::HashSet<_>>();

        let roster_entries = roster.list().cloned().collect::<Vec<_>>();
        let roster_session_ids = roster_entries
            .iter()
            .map(|entry| entry.session_id.clone())
            .collect::<std::collections::HashSet<_>>();

        // Archive orphan sessions that are active but not present in the event-projected roster.
        for session_id in active_ids.difference(&roster_session_ids) {
            if session_service
                .session_belongs_to_mob(session_id, &definition.id)
                .await
                && let Err(error) = session_service.archive(session_id).await
                && !matches!(error, meerkat_core::service::SessionError::NotFound { .. })
            {
                return Err(error.into());
            }
        }

        // Recreate missing sessions referenced by MeerkatSpawned events.
        for entry in &roster_entries {
            if !active_ids.contains(&entry.session_id) {
                let profile = definition
                    .profiles
                    .get(&entry.profile)
                    .ok_or_else(|| MobError::ProfileNotFound(entry.profile.to_string()))?;
                let mut config = build::build_agent_config(
                    &definition.id,
                    &entry.profile,
                    &entry.meerkat_id,
                    profile,
                    definition,
                    compose_external_tools_for_profile(profile, tool_bundles, tool_handle.clone())?,
                )
                .await?;
                if let Some(ref client) = default_llm_client {
                    config.llm_client_override = Some(client.clone());
                }
                let prompt = format!(
                    "You have been spawned as '{}' (role: {}) in mob '{}'.",
                    entry.meerkat_id, entry.profile, definition.id
                );
                let req = build::to_create_session_request(&config, prompt);
                let created = session_service.create_session(req).await?;
                let _ = roster.set_session_id(&entry.meerkat_id, created.session_id);
            }
        }

        // Re-establish trust from projected wiring (idempotent upsert).
        let entries = roster.list().cloned().collect::<Vec<_>>();
        for entry in &entries {
            let comms_a = match session_service.comms_runtime(&entry.session_id).await {
                Some(comms) => comms,
                None => continue,
            };
            let key_a = match comms_a.public_key() {
                Some(key) => key,
                None => continue,
            };
            let name_a = format!("{}/{}/{}", definition.id, entry.profile, entry.meerkat_id);

            for peer_id in &entry.wired_to {
                if entry.meerkat_id.as_str() >= peer_id.as_str() {
                    continue;
                }
                let Some(peer_entry) = roster.get(peer_id).cloned() else {
                    continue;
                };
                let Some(comms_b) = session_service.comms_runtime(&peer_entry.session_id).await
                else {
                    continue;
                };
                let Some(key_b) = comms_b.public_key() else {
                    continue;
                };
                let name_b = format!(
                    "{}/{}/{}",
                    definition.id, peer_entry.profile, peer_entry.meerkat_id
                );

                let spec_b =
                    TrustedPeerSpec::new(&name_b, key_b.clone(), format!("inproc://{name_b}"))
                        .map_err(|e| MobError::WiringError(format!("invalid peer spec: {e}")))?;
                let spec_a =
                    TrustedPeerSpec::new(&name_a, key_a.clone(), format!("inproc://{name_a}"))
                        .map_err(|e| MobError::WiringError(format!("invalid peer spec: {e}")))?;
                comms_a.add_trusted_peer(spec_b).await?;
                comms_b.add_trusted_peer(spec_a).await?;
            }
        }

        // Notify orchestrator that the mob resumed.
        if let Some(orchestrator) = &definition.orchestrator
            && let Some(orchestrator_entry) = roster.by_profile(&orchestrator.profile).next()
        {
            let active_count = roster.len();
            let wired_edges = roster
                .list()
                .map(|entry| entry.wired_to.len())
                .sum::<usize>()
                / 2;
            let _ = session_service
                .start_turn(
                    &orchestrator_entry.session_id,
                    meerkat_core::service::StartTurnRequest {
                        prompt: format!(
                            "Mob '{}' resumed with {} active meerkats and {} wiring links. Reconcile worker state and continue orchestration.",
                            definition.id, active_count, wired_edges
                        ),
                        event_tx: None,
                        host_mode: true,
                        skill_references: None,
                    },
                )
                .await;
        }

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn start_runtime(
        definition: Arc<MobDefinition>,
        initial_roster: Roster,
        initial_task_board: TaskBoard,
        initial_state: MobState,
        events: Arc<dyn MobEventStore>,
        session_service: Arc<dyn MobSessionService>,
        tool_bundles: BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
        default_llm_client: Option<Arc<dyn LlmClient>>,
    ) -> MobHandle {
        let roster = Arc::new(RwLock::new(initial_roster));
        let task_board = Arc::new(RwLock::new(initial_task_board));
        let state = Arc::new(AtomicU8::new(initial_state as u8));
        let mcp_servers_running = false;
        let mcp_running = Arc::new(RwLock::new(
            definition
                .mcp_servers
                .keys()
                .map(|name| (name.clone(), mcp_servers_running))
                .collect::<BTreeMap<_, _>>(),
        ));
        let mcp_processes = Arc::new(tokio::sync::Mutex::new(BTreeMap::new()));
        let (command_tx, command_rx) = mpsc::channel(64);
        let wiring = RuntimeWiring {
            roster,
            task_board,
            state,
            mcp_running,
            mcp_processes,
            command_tx,
            command_rx,
        };

        Self::start_runtime_with_components(
            definition,
            wiring,
            events,
            session_service,
            tool_bundles,
            default_llm_client,
        )
    }

    fn start_runtime_with_components(
        definition: Arc<MobDefinition>,
        wiring: RuntimeWiring,
        events: Arc<dyn MobEventStore>,
        session_service: Arc<dyn MobSessionService>,
        tool_bundles: BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
        default_llm_client: Option<Arc<dyn LlmClient>>,
    ) -> MobHandle {
        let RuntimeWiring {
            roster,
            task_board,
            state,
            mcp_running,
            mcp_processes,
            command_tx,
            command_rx,
        } = wiring;
        let handle = MobHandle {
            command_tx: command_tx.clone(),
            roster: roster.clone(),
            task_board: task_board.clone(),
            definition: definition.clone(),
            state: state.clone(),
            events: events.clone(),
            mcp_running: mcp_running.clone(),
        };

        let actor = MobActor {
            definition,
            roster,
            task_board,
            state,
            events,
            session_service,
            mcp_running,
            mcp_processes,
            command_tx,
            tool_bundles,
            default_llm_client,
        };
        tokio::spawn(actor.run(command_rx));

        handle
    }
}

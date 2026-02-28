use super::*;
#[cfg(target_arch = "wasm32")]
use crate::tokio;

// ---------------------------------------------------------------------------
// MobBuilder
// ---------------------------------------------------------------------------

/// Builder for creating or resuming a mob.
pub struct MobBuilder {
    mode: BuilderMode,
    storage: MobStorage,
    session_service: Option<Arc<dyn MobSessionService>>,
    allow_ephemeral_sessions: bool,
    notify_orchestrator_on_resume: bool,
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
    mcp_servers: Arc<tokio::sync::Mutex<BTreeMap<String, actor::McpServerEntry>>>,
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
            notify_orchestrator_on_resume: true,
            tool_bundles: BTreeMap::new(),
            default_llm_client: None,
        }
    }

    /// Create a builder from a mobpack definition + packed skill files.
    ///
    /// Any `SkillSource::Path` entries are resolved against `packed_skills` and
    /// converted to inline sources for runtime execution.
    pub fn from_mobpack(
        mut definition: MobDefinition,
        packed_skills: BTreeMap<String, Vec<u8>>,
        storage: MobStorage,
    ) -> Result<Self, MobError> {
        for (skill_name, source) in &mut definition.skills {
            if let crate::definition::SkillSource::Path { path } = source {
                let bytes = packed_skills.get(path).ok_or_else(|| {
                    MobError::Internal(format!(
                        "mobpack skill path '{path}' for '{skill_name}' missing from archive"
                    ))
                })?;
                let content = String::from_utf8(bytes.clone()).map_err(|_| {
                    MobError::Internal(format!(
                        "mobpack skill path '{path}' for '{skill_name}' is not valid UTF-8"
                    ))
                })?;
                *source = crate::definition::SkillSource::Inline { content };
            }
        }
        Ok(Self::new(definition, storage))
    }

    /// Create a builder that resumes a mob from persisted events.
    pub fn for_resume(storage: MobStorage) -> Self {
        Self {
            mode: BuilderMode::Resume,
            storage,
            session_service: None,
            allow_ephemeral_sessions: false,
            notify_orchestrator_on_resume: true,
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

    /// Control whether `resume()` sends an informational turn to the orchestrator.
    ///
    /// Default is `true`. CLI command rehydration can set this to `false` to avoid
    /// triggering extra orchestrator turns on every command invocation.
    pub fn notify_orchestrator_on_resume(mut self, notify: bool) -> Self {
        self.notify_orchestrator_on_resume = notify;
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
            notify_orchestrator_on_resume: _,
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
        let mut diagnostics = crate::validate::validate_definition(&definition);
        diagnostics.extend(crate::spec::SpecValidator::validate(&definition));
        let (errors, warnings) = crate::validate::partition_diagnostics(diagnostics);
        if !errors.is_empty() {
            return Err(MobError::DefinitionError(errors));
        }
        for warning in warnings {
            tracing::warn!(
                code = %warning.code,
                location = ?warning.location,
                "{}",
                warning.message
            );
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
                    definition: Box::new(definition_for_event),
                },
            })
            .await?;
        Self::sync_definition_with_spec_store(
            storage.specs.clone(),
            definition.id.clone(),
            definition.as_ref(),
        )
        .await?;

        Ok(Self::start_runtime(
            definition,
            Roster::new(),
            TaskBoard::default(),
            MobState::Running,
            storage.events.clone(),
            storage.runs.clone(),
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
            notify_orchestrator_on_resume,
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

        // Use the last MobCreated event — reset appends a fresh MobCreated
        // to start a new epoch, so the latest one reflects the current definition.
        let definition = all_events
            .iter()
            .rev()
            .find_map(|event| match &event.kind {
                MobEventKind::MobCreated { definition } => Some(*definition.clone()),
                _ => None,
            })
            .ok_or_else(|| {
                MobError::Internal(
                    "cannot resume mob: no MobCreated event found in storage".to_string(),
                )
            })?;
        Self::sync_definition_with_spec_store(
            storage.specs.clone(),
            definition.id.clone(),
            &definition,
        )
        .await?;

        let definition = Arc::new(definition);
        let mut diagnostics = crate::validate::validate_definition(&definition);
        diagnostics.extend(crate::spec::SpecValidator::validate(definition.as_ref()));
        let (errors, warnings) = crate::validate::partition_diagnostics(diagnostics);
        if !errors.is_empty() {
            return Err(MobError::DefinitionError(errors));
        }
        for warning in warnings {
            tracing::warn!(
                code = %warning.code,
                location = ?warning.location,
                "{}",
                warning.message
            );
        }
        let mob_events: Vec<_> = all_events
            .into_iter()
            .filter(|event| event.mob_id == definition.id)
            .collect();
        let mut roster = Roster::project(&mob_events);
        let task_board = TaskBoard::project(&mob_events);
        // Determine resumed state from events in the current epoch (after the
        // last MobReset, or all events if no reset has occurred).
        let epoch_start = mob_events
            .iter()
            .rposition(|event| matches!(event.kind, MobEventKind::MobReset))
            .map_or(0, |pos| pos + 1);
        let resumed_state = if mob_events[epoch_start..]
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
        let mcp_servers = Arc::new(tokio::sync::Mutex::new(
            definition
                .mcp_servers
                .keys()
                .map(|name| {
                    (
                        name.clone(),
                        actor::McpServerEntry {
                            #[cfg(not(target_arch = "wasm32"))]
                            process: None,
                            running: false,
                        },
                    )
                })
                .collect::<BTreeMap<_, _>>(),
        ));
        let (command_tx, command_rx) = mpsc::channel(64);
        let wiring = RuntimeWiring {
            roster: roster_state.clone(),
            task_board: task_board_state.clone(),
            state: state.clone(),
            mcp_servers: mcp_servers.clone(),
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
            mcp_servers: mcp_servers.clone(),
            flow_streams: Arc::new(tokio::sync::Mutex::new(BTreeMap::new())),
            session_service: session_service.clone(),
        };
        // session_service is still live here (not consumed until start_runtime_with_components)

        if resumed_state == MobState::Running {
            Self::reconcile_resume(
                &definition,
                &mut roster,
                &session_service,
                notify_orchestrator_on_resume,
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
            storage.runs.clone(),
            session_service,
            tool_bundles,
            default_llm_client,
        ))
    }

    async fn sync_definition_with_spec_store(
        specs: Arc<dyn crate::store::MobSpecStore>,
        mob_id: MobId,
        definition: &MobDefinition,
    ) -> Result<(), MobError> {
        match specs.get_spec(&mob_id).await? {
            Some((stored, _revision)) if stored != *definition => Err(MobError::Internal(
                "persisted spec store definition does not match MobCreated runtime definition"
                    .to_string(),
            )),
            Some(_) => Ok(()),
            None => {
                let _ = specs.put_spec(&mob_id, definition, None).await?;
                Ok(())
            }
        }
    }

    async fn reconcile_resume(
        definition: &Arc<MobDefinition>,
        roster: &mut Roster,
        session_service: &Arc<dyn MobSessionService>,
        notify_orchestrator_on_resume: bool,
        default_llm_client: Option<Arc<dyn LlmClient>>,
        tool_bundles: &BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
        tool_handle: &MobHandle,
    ) -> Result<(), MobError> {
        let provisioner = MultiBackendProvisioner::new(
            session_service.clone(),
            definition.backend.external.clone(),
        );
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
            .filter_map(|entry| entry.session_id().cloned())
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
            if entry
                .session_id()
                .is_some_and(|session_id| active_ids.contains(session_id))
            {
                continue;
            }
            let profile = definition
                .profiles
                .get(&entry.profile)
                .ok_or_else(|| MobError::ProfileNotFound(entry.profile.clone()))?;
            let mut config = build::build_agent_config(
                &definition.id,
                &entry.profile,
                &entry.meerkat_id,
                profile,
                definition,
                compose_external_tools_for_profile(profile, tool_bundles, tool_handle.clone())?,
                None,
                None,
            )
            .await?;
            // Resume reconciliation needs live comms runtimes, but this path is
            // infrastructure restoration and should not consume provider quota.
            // If no explicit override is configured, use the local test client
            // for deterministic, no-network bootstrap turns.
            let reconcile_client: Arc<dyn LlmClient> = default_llm_client
                .clone()
                .unwrap_or_else(|| Arc::new(meerkat_client::TestClient::default()));
            config.llm_client_override = Some(reconcile_client);
            let prompt = format!(
                "You have been spawned as '{}' (role: {}) in mob '{}'.",
                entry.meerkat_id, entry.profile, definition.id
            );
            let req = build::to_create_session_request(&config, prompt);
            let created = session_service.create_session(req).await?;
            let _ = roster.set_session_id(&entry.meerkat_id, created.session_id);
        }

        // Re-establish trust from projected wiring (idempotent upsert).
        let entries = roster.list().cloned().collect::<Vec<_>>();
        for entry in &entries {
            let comms_a = match provisioner.comms_runtime(&entry.member_ref).await {
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
                let Some(comms_b) = provisioner.comms_runtime(&peer_entry.member_ref).await else {
                    continue;
                };
                let Some(key_b) = comms_b.public_key() else {
                    continue;
                };
                let name_b = format!(
                    "{}/{}/{}",
                    definition.id, peer_entry.profile, peer_entry.meerkat_id
                );

                let spec_b = provisioner
                    .trusted_peer_spec(&peer_entry.member_ref, &name_b, &key_b)
                    .await?;
                let spec_a = provisioner
                    .trusted_peer_spec(&entry.member_ref, &name_a, &key_a)
                    .await?;
                comms_a.add_trusted_peer(spec_b).await?;
                comms_b.add_trusted_peer(spec_a).await?;
            }
        }
        // Notify orchestrator that the mob resumed.
        if notify_orchestrator_on_resume
            && let Some(orchestrator) = &definition.orchestrator
            && let Some(orchestrator_entry) = roster.by_profile(&orchestrator.profile).next()
        {
            let active_count = roster.len();
            let wired_edges = roster
                .list()
                .map(|entry| entry.wired_to.len())
                .sum::<usize>()
                / 2;
            let session_id = orchestrator_entry.session_id().ok_or_else(|| {
                MobError::Internal(
                    "orchestrator entry missing session-backed member ref".to_string(),
                )
            })?;
            let resume_message = format!(
                "Mob '{}' resumed with {} active meerkats and {} wiring links. Reconcile worker state and continue orchestration.",
                definition.id, active_count, wired_edges
            );
            match orchestrator_entry.runtime_mode {
                crate::MobRuntimeMode::AutonomousHost => {
                    let injector = session_service.event_injector(session_id).await.ok_or_else(|| {
                        MobError::Internal(format!(
                            "orchestrator '{}' missing event injector during resume notification",
                            orchestrator_entry.meerkat_id
                        ))
                    })?;
                    injector
                        .inject(resume_message, meerkat_core::PlainEventSource::Rpc)
                        .map_err(|error| {
                            MobError::Internal(format!(
                                "orchestrator resume inject failed for '{}': {}",
                                orchestrator_entry.meerkat_id, error
                            ))
                        })?;
                }
                crate::MobRuntimeMode::TurnDriven => {
                    session_service
                        .start_turn(
                            session_id,
                            meerkat_core::service::StartTurnRequest {
                                prompt: resume_message,
                                event_tx: None,
                                host_mode: false,
                                skill_references: None,
                                flow_tool_overlay: None,
                                additional_instructions: None,
                            },
                        )
                        .await?;
                }
            }
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
        run_store: Arc<dyn MobRunStore>,
        session_service: Arc<dyn MobSessionService>,
        tool_bundles: BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
        default_llm_client: Option<Arc<dyn LlmClient>>,
    ) -> MobHandle {
        let roster = Arc::new(RwLock::new(initial_roster));
        let task_board = Arc::new(RwLock::new(initial_task_board));
        let state = Arc::new(AtomicU8::new(initial_state as u8));
        let mcp_servers = Arc::new(tokio::sync::Mutex::new(
            definition
                .mcp_servers
                .keys()
                .map(|name| {
                    (
                        name.clone(),
                        actor::McpServerEntry {
                            #[cfg(not(target_arch = "wasm32"))]
                            process: None,
                            running: false,
                        },
                    )
                })
                .collect::<BTreeMap<_, _>>(),
        ));
        let (command_tx, command_rx) = mpsc::channel(64);
        let wiring = RuntimeWiring {
            roster,
            task_board,
            state,
            mcp_servers,
            command_tx,
            command_rx,
        };

        Self::start_runtime_with_components(
            definition,
            wiring,
            events,
            run_store,
            session_service,
            tool_bundles,
            default_llm_client,
        )
    }

    fn start_runtime_with_components(
        definition: Arc<MobDefinition>,
        wiring: RuntimeWiring,
        events: Arc<dyn MobEventStore>,
        run_store: Arc<dyn MobRunStore>,
        session_service: Arc<dyn MobSessionService>,
        tool_bundles: BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
        default_llm_client: Option<Arc<dyn LlmClient>>,
    ) -> MobHandle {
        let RuntimeWiring {
            roster,
            task_board,
            state,
            mcp_servers,
            command_tx,
            command_rx,
        } = wiring;
        let external_backend = definition.backend.external.clone();
        let handle_session_service = session_service.clone();
        let handle = MobHandle {
            command_tx: command_tx.clone(),
            roster: roster.clone(),
            task_board: task_board.clone(),
            definition: definition.clone(),
            state: state.clone(),
            events: events.clone(),
            mcp_servers: mcp_servers.clone(),
            flow_streams: Arc::new(tokio::sync::Mutex::new(BTreeMap::new())),
            session_service: handle_session_service.clone(),
        };
        let provisioner: Arc<dyn MobProvisioner> = Arc::new(MultiBackendProvisioner::new(
            session_service,
            external_backend,
        ));
        let max_orphaned_turns = definition
            .limits
            .as_ref()
            .and_then(|limits| limits.max_orphaned_turns)
            .unwrap_or(8) as usize;
        let flow_executor: Arc<dyn FlowTurnExecutor> = Arc::new(ActorFlowTurnExecutor::new(
            handle.clone(),
            provisioner.clone(),
            max_orphaned_turns,
        ));
        let flow_engine = FlowEngine::new(
            flow_executor,
            handle.clone(),
            run_store.clone(),
            events.clone(),
        );

        let actor = MobActor {
            definition,
            roster,
            task_board,
            state,
            events,
            run_store,
            provisioner,
            flow_engine,
            run_tasks: BTreeMap::new(),
            run_cancel_tokens: BTreeMap::new(),
            flow_streams: handle.flow_streams.clone(),
            mcp_servers,
            command_tx,
            tool_bundles,
            default_llm_client,
            retired_event_index: Arc::new(RwLock::new(HashSet::new())),
            autonomous_host_loops: Arc::new(tokio::sync::Mutex::new(BTreeMap::new())),
            next_spawn_ticket: 0,
            pending_spawns: BTreeMap::new(),
            pending_spawn_ids: HashSet::new(),
            pending_spawn_tasks: BTreeMap::new(),
            edge_locks: Arc::new(super::edge_locks::EdgeLockRegistry::new()),
            lifecycle_tasks: tokio::task::JoinSet::new(),
            session_service: handle_session_service,
            spawn_policy: None,
        };
        tokio::spawn(actor.run(command_rx));

        handle
    }
}

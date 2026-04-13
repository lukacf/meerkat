use super::mob_orchestrator_authority::MobOrchestratorMutator;
use super::*;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use std::collections::{HashMap, HashSet};

// ---------------------------------------------------------------------------
// MobBuilder
// ---------------------------------------------------------------------------

/// Builder for creating or resuming a mob.
pub struct MobBuilder {
    mode: BuilderMode,
    storage: MobStorage,
    session_service: Option<Arc<dyn MobSessionService>>,
    runtime_adapter: Option<Arc<meerkat_runtime::MeerkatMachine>>,
    allow_ephemeral_sessions: bool,
    notify_orchestrator_on_resume: bool,
    tool_bundles: BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
    default_llm_client: Option<Arc<dyn LlmClient>>,
    default_external_tools_provider: Option<crate::ExternalToolsProvider>,
}

enum BuilderMode {
    Create(Arc<MobDefinition>),
    Resume,
}

struct RuntimeWiring {
    roster: Arc<RwLock<RosterAuthority>>,
    task_board: Arc<RwLock<TaskBoard>>,
    state: Arc<AtomicU8>,
    restore_diagnostics: Arc<RwLock<HashMap<MeerkatId, super::handle::RestoreFailureDiagnostic>>>,
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
            runtime_adapter: None,
            allow_ephemeral_sessions: false,
            notify_orchestrator_on_resume: true,
            tool_bundles: BTreeMap::new(),
            default_llm_client: None,
            default_external_tools_provider: None,
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
            runtime_adapter: None,
            allow_ephemeral_sessions: false,
            notify_orchestrator_on_resume: true,
            tool_bundles: BTreeMap::new(),
            default_llm_client: None,
            default_external_tools_provider: None,
        }
    }

    /// Set the session service for creating meerkat sessions.
    ///
    /// The service must implement both `SessionService` and `MobSessionService`
    /// to provide comms runtime access for wiring operations. If no explicit
    /// runtime adapter override has been set yet, the builder seeds its
    /// canonical runtime adapter from `service.runtime_adapter()`.
    pub fn with_session_service(mut self, service: Arc<dyn MobSessionService>) -> Self {
        if self.runtime_adapter.is_none() {
            self.runtime_adapter = service.runtime_adapter();
        }
        self.session_service = Some(service);
        self
    }

    /// Attach the canonical runtime adapter for the mob runtime.
    ///
    /// When set, this override is used consistently by both provisioning and
    /// autonomous-host comms-drain ingress instead of re-deriving an adapter
    /// from the session service at runtime.
    pub fn with_runtime_adapter(mut self, adapter: Arc<meerkat_runtime::MeerkatMachine>) -> Self {
        self.runtime_adapter = Some(adapter);
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

    /// Set a provider closure that is called at each member spawn to get a fresh
    /// snapshot of default external tools (e.g. callback tools from the SDK).
    pub fn with_default_external_tools_provider(
        mut self,
        provider: Option<crate::ExternalToolsProvider>,
    ) -> Self {
        self.default_external_tools_provider = provider;
        self
    }

    /// Create the mob: emit MobCreated event, start the actor, return handle.
    pub async fn create(self) -> Result<MobHandle, MobError> {
        let MobBuilder {
            mode,
            storage,
            session_service,
            runtime_adapter,
            allow_ephemeral_sessions,
            notify_orchestrator_on_resume: _,
            tool_bundles,
            default_llm_client,
            default_external_tools_provider,
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

        // §8: AutonomousHost profiles require a runtime adapter. Validate at
        // build time so Option<adapter> on the trait doesn't hide an ownership
        // requirement that only surfaces at spawn time.
        let has_autonomous = definition
            .profiles
            .values()
            .filter_map(|b| b.as_inline())
            .any(|p| p.runtime_mode == crate::MobRuntimeMode::AutonomousHost);
        if has_autonomous && runtime_adapter.is_none() {
            return Err(MobError::Internal(
                "definition contains AutonomousHost profiles but no runtime adapter is available; \
                 provide one via with_runtime_adapter() or use a session service that implements \
                 runtime_adapter()"
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
            runtime_adapter,
            tool_bundles,
            default_llm_client,
            default_external_tools_provider,
            storage.realm_profiles.clone(),
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
            runtime_adapter,
            allow_ephemeral_sessions,
            notify_orchestrator_on_resume,
            tool_bundles,
            default_llm_client,
            default_external_tools_provider,
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
        // §8 check deferred until after definition recovery — the definition
        // comes from the event log, so we can't check profiles before replay.
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
        // §8: AutonomousHost profiles require a runtime adapter. Same check
        // as create(), but deferred until after definition recovery from events.
        let has_autonomous = definition
            .profiles
            .values()
            .filter_map(|b| b.as_inline())
            .any(|p| p.runtime_mode == crate::MobRuntimeMode::AutonomousHost);
        if has_autonomous && runtime_adapter.is_none() {
            return Err(MobError::Internal(
                "definition contains AutonomousHost profiles but no runtime adapter is available; \
                 provide one via with_runtime_adapter() or use a session service that implements \
                 runtime_adapter()"
                    .to_string(),
            ));
        }

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
        let roster_state = Arc::new(RwLock::new(RosterAuthority::new()));
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
        let restore_diagnostics = Arc::new(RwLock::new(HashMap::new()));
        let wiring = RuntimeWiring {
            roster: roster_state.clone(),
            task_board: task_board_state.clone(),
            state: state.clone(),
            restore_diagnostics: restore_diagnostics.clone(),
            mcp_servers: mcp_servers.clone(),
            command_tx: command_tx.clone(),
            command_rx,
        };
        let preview_handle = MobHandle {
            command_tx: command_tx.clone(),
            roster: roster_state.clone(),
            definition: definition.clone(),
            state: state.clone(),
            events: storage.events.clone(),
            flow_streams: Arc::new(tokio::sync::Mutex::new(BTreeMap::new())),
            session_service: session_service.clone(),
            restore_diagnostics,
        };
        // session_service is still live here (not consumed until start_runtime_with_components)

        if resumed_state == MobState::Running {
            Self::reconcile_resume(
                &definition,
                &mut roster,
                &session_service,
                runtime_adapter.clone(),
                notify_orchestrator_on_resume,
                default_llm_client.clone(),
                &tool_bundles,
                &preview_handle,
                &default_external_tools_provider,
                storage.realm_profiles.clone(),
            )
            .await?;
        }

        *wiring.roster.write().await = RosterAuthority::from_roster(roster);
        *wiring.task_board.write().await = task_board;

        Ok(Self::start_runtime_with_components(
            definition,
            wiring,
            storage.events.clone(),
            storage.runs.clone(),
            session_service,
            runtime_adapter,
            tool_bundles,
            default_llm_client,
            default_external_tools_provider,
            storage.realm_profiles.clone(),
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

    #[allow(clippy::too_many_arguments)]
    async fn reconcile_resume(
        definition: &Arc<MobDefinition>,
        roster: &mut Roster,
        session_service: &Arc<dyn MobSessionService>,
        runtime_adapter: Option<Arc<meerkat_runtime::MeerkatMachine>>,
        notify_orchestrator_on_resume: bool,
        default_llm_client: Option<Arc<dyn LlmClient>>,
        tool_bundles: &BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
        tool_handle: &MobHandle,
        default_external_tools_provider: &Option<crate::ExternalToolsProvider>,
        realm_profile_store: Option<Arc<dyn crate::store::RealmProfileStore>>,
    ) -> Result<(), MobError> {
        tool_handle.restore_diagnostics.write().await.clear();
        let provisioner = MultiBackendProvisioner::new(
            session_service.clone(),
            runtime_adapter,
            definition.backend.external.clone(),
        );
        let listed_sessions = session_service
            .list(meerkat_core::service::SessionQuery::default())
            .await?;
        // Live bridge existence is session-service-owned truth. Do not infer it
        // from SessionSummary presence or `is_active`, because persisted-only
        // summaries are not live and idle live sessions are not "active".
        let mut active_ids = std::collections::HashSet::new();
        for summary in &listed_sessions {
            if session_service
                .has_live_session(&summary.session_id)
                .await?
            {
                active_ids.insert(summary.session_id.clone());
            }
        }

        let roster_entries = roster.list().cloned().collect::<Vec<_>>();
        let roster_session_ids = roster_entries
            .iter()
            .filter_map(|entry| entry.bridge_session_id().cloned())
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
        // Recreate missing sessions referenced by MemberSpawned events.
        for entry in &roster_entries {
            let Some(bridge_session_id) = entry.bridge_session_id().cloned() else {
                continue;
            };
            if active_ids.contains(&bridge_session_id) {
                continue;
            }
            let record_restore_failure = |reason: String| async {
                tool_handle.restore_diagnostics.write().await.insert(
                    entry.meerkat_id.clone(),
                    super::handle::RestoreFailureDiagnostic {
                        bridge_session_id: bridge_session_id.clone(),
                        reason,
                    },
                );
            };

            if matches!(entry.member_ref, MemberRef::Session { .. })
                && session_service.supports_persistent_sessions()
            {
                let Some(stored_session) = session_service
                    .load_persisted_session(&bridge_session_id)
                    .await?
                else {
                    record_restore_failure(format!(
                        "missing durable session snapshot for '{bridge_session_id}'"
                    ))
                    .await;
                    continue;
                };
                // Prefer roster's effective_profile_override on restore for lifecycle safety.
                let profile = if let Some(ref p) = entry.effective_profile_override {
                    p.clone()
                } else {
                    definition
                        .resolve_profile(&entry.role, realm_profile_store.as_ref())
                        .await?
                };
                let profile = &profile;
                let default_ext = default_external_tools_provider.as_ref().and_then(|p| p());
                let resumed_config =
                    build::build_resumed_agent_config(build::BuildResumedAgentConfigParams {
                        base: build::BuildAgentConfigParams {
                            mob_id: &definition.id,
                            profile_name: &entry.role,
                            meerkat_id: &entry.meerkat_id,
                            profile,
                            definition,
                            external_tools: compose_external_tools_for_profile(
                                profile,
                                tool_bundles,
                                tool_handle.clone(),
                                default_ext,
                                crate::build::MobToolAccessContext::None,
                            )?,
                            context: None,
                            labels: Some(entry.labels.clone()),
                            additional_instructions: None,
                            shell_env: None,
                            mob_tool_access_context: crate::build::MobToolAccessContext::None,
                            inherited_tool_filter: None,
                        },
                        expected_session_id: &bridge_session_id,
                        resumed_session: stored_session,
                    })
                    .await;
                let mut resumed_config = match resumed_config {
                    Ok(config) => config,
                    Err(error) => {
                        record_restore_failure(error.to_string()).await;
                        continue;
                    }
                };
                resumed_config.keep_alive =
                    entry.runtime_mode == crate::MobRuntimeMode::AutonomousHost;
                let reconcile_client: Arc<dyn LlmClient> = default_llm_client
                    .clone()
                    .unwrap_or_else(|| Arc::new(meerkat_client::TestClient::default()));
                resumed_config.llm_client_override = Some(reconcile_client);
                let prompt = format!(
                    "You have been spawned as '{}' (role: {}) in mob '{}'.",
                    entry.meerkat_id, entry.role, definition.id
                );
                let req = build::to_create_session_request(&resumed_config, prompt.into());
                match session_service.create_session(req).await {
                    Ok(created) => {
                        let created_bridge_session_id = created.session_id;
                        let _ = roster
                            .set_bridge_session_id(&entry.meerkat_id, created_bridge_session_id);
                        tool_handle
                            .restore_diagnostics
                            .write()
                            .await
                            .remove(&entry.meerkat_id);
                    }
                    Err(error) => {
                        record_restore_failure(format!(
                            "failed to restore durable session '{bridge_session_id}': {error}"
                        ))
                        .await;
                    }
                }
                continue;
                // Ephemeral services can still fall back to fresh-create.
            }
            let profile = definition
                .resolve_profile(&entry.role, realm_profile_store.as_ref())
                .await?;
            let default_ext_fresh = default_external_tools_provider.as_ref().and_then(|p| p());
            let mut config = build::build_agent_config(build::BuildAgentConfigParams {
                mob_id: &definition.id,
                profile_name: &entry.role,
                meerkat_id: &entry.meerkat_id,
                profile: &profile,
                definition,
                external_tools: compose_external_tools_for_profile(
                    &profile,
                    tool_bundles,
                    tool_handle.clone(),
                    default_ext_fresh,
                    crate::build::MobToolAccessContext::None,
                )?,
                context: None,
                labels: None,
                additional_instructions: None,
                shell_env: None,
                mob_tool_access_context: crate::build::MobToolAccessContext::None,
                inherited_tool_filter: None,
            })
            .await?;
            config.keep_alive = entry.runtime_mode == crate::MobRuntimeMode::AutonomousHost;
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
                entry.meerkat_id, entry.role, definition.id
            );
            let req = build::to_create_session_request(&config, prompt.into());
            let created = session_service.create_session(req).await?;
            let created_bridge_session_id = created.session_id;
            let _ = roster.set_bridge_session_id(&entry.meerkat_id, created_bridge_session_id);
            tool_handle
                .restore_diagnostics
                .write()
                .await
                .remove(&entry.meerkat_id);
        }

        // Re-establish trust from projected wiring and prune stale trust so
        // live comms truth matches the canonical roster projection.
        let entries = roster.list().cloned().collect::<Vec<_>>();
        let broken_members = tool_handle
            .restore_diagnostics
            .read()
            .await
            .keys()
            .cloned()
            .collect::<HashSet<_>>();
        for entry in &entries {
            if broken_members.contains(&entry.meerkat_id) {
                let _ = roster.set_peer_id(&entry.meerkat_id, None);
                continue;
            }
            let comms_a = match provisioner.comms_runtime(&entry.member_ref).await {
                Some(comms) => comms,
                None if entry.wired_to.is_empty() => continue,
                None => {
                    return Err(MobError::WiringError(format!(
                        "resume requires comms runtime for wired member '{}'",
                        entry.meerkat_id
                    )));
                }
            };
            let key_a = comms_a.public_key().ok_or_else(|| {
                MobError::WiringError(format!(
                    "resume requires public key for wired member '{}'",
                    entry.meerkat_id
                ))
            })?;
            let _ = roster.set_peer_id(&entry.meerkat_id, Some(key_a.clone()));
            let mut desired_specs = Vec::new();

            for peer_identity in &entry.wired_to {
                let peer_meerkat_id = MeerkatId::from(peer_identity.as_str());
                if let Some(spec) = entry.external_peer_specs.get(&peer_meerkat_id) {
                    desired_specs.push(spec.clone());
                    continue;
                }
                let peer_entry = roster.get(&peer_meerkat_id).cloned().ok_or_else(|| {
                    MobError::WiringError(format!(
                        "resume wiring target '{}' missing for '{}'",
                        peer_identity, entry.meerkat_id
                    ))
                })?;
                if broken_members.contains(&peer_meerkat_id) {
                    continue;
                }
                let comms_b = provisioner
                    .comms_runtime(&peer_entry.member_ref)
                    .await
                    .ok_or_else(|| {
                        MobError::WiringError(format!(
                            "resume requires comms runtime for '{}' -> '{}'",
                            entry.meerkat_id, peer_identity
                        ))
                    })?;
                let key_b = comms_b.public_key().ok_or_else(|| {
                    MobError::WiringError(format!(
                        "resume requires public key for '{}' -> '{}'",
                        entry.meerkat_id, peer_identity
                    ))
                })?;
                let name_b = format!(
                    "{}/{}/{}",
                    definition.id, peer_entry.role, peer_entry.meerkat_id
                );
                desired_specs.push(
                    provisioner
                        .trusted_peer_spec(&peer_entry.member_ref, &name_b, &key_b)
                        .await?,
                );
            }

            let desired_peer_ids = desired_specs
                .iter()
                .map(|spec| spec.peer_id.clone())
                .collect::<std::collections::BTreeSet<_>>();
            for peer in comms_a.peers().await {
                if !desired_peer_ids.contains(&peer.peer_id) {
                    let _ = comms_a.remove_trusted_peer(&peer.peer_id).await?;
                }
            }
            for spec in desired_specs {
                comms_a.add_trusted_peer(spec).await?;
            }
        }
        // Notify orchestrator that the mob resumed.
        if notify_orchestrator_on_resume
            && let Some(orchestrator) = &definition.orchestrator
            && let Some(orchestrator_entry) = roster.by_profile(&orchestrator.profile).next()
        {
            if broken_members.contains(&orchestrator_entry.meerkat_id) {
                tracing::warn!(
                    member_id = %orchestrator_entry.meerkat_id,
                    "Skipping orchestrator resume notification because the orchestrator is Broken"
                );
                return Ok(());
            }
            let active_count = roster.len();
            let wired_edges = roster
                .list()
                .map(|entry| entry.wired_to.len())
                .sum::<usize>()
                / 2;
            let bridge_session_id = orchestrator_entry.bridge_session_id().ok_or_else(|| {
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
                    let injector = session_service
                        .event_injector(bridge_session_id)
                        .await
                        .ok_or_else(|| {
                            MobError::Internal(format!(
                                "orchestrator '{}' missing event injector during resume notification",
                                orchestrator_entry.meerkat_id
                            ))
                        })?;
                    injector
                        .inject(
                            resume_message.into(),
                            meerkat_core::PlainEventSource::Rpc,
                            meerkat_core::types::HandlingMode::Queue,
                            None,
                        )
                        .map_err(|error| {
                            MobError::Internal(format!(
                                "orchestrator resume inject failed for '{}': {}",
                                orchestrator_entry.meerkat_id, error
                            ))
                        })?;
                }
                crate::MobRuntimeMode::TurnDriven => {
                    provisioner
                        .start_turn(
                            &orchestrator_entry.member_ref,
                            meerkat_core::service::StartTurnRequest {
                                prompt: resume_message.into(),
                                system_prompt: None,
                                render_metadata: None,
                                handling_mode: meerkat_core::types::HandlingMode::Queue,
                                event_tx: None,
                                skill_references: None,
                                flow_tool_overlay: None,
                                additional_instructions: None,
                                execution_kind: None,
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
        runtime_adapter: Option<Arc<meerkat_runtime::MeerkatMachine>>,
        tool_bundles: BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
        default_llm_client: Option<Arc<dyn LlmClient>>,
        default_external_tools_provider: Option<crate::ExternalToolsProvider>,
        realm_profile_store: Option<Arc<dyn crate::store::RealmProfileStore>>,
    ) -> MobHandle {
        let roster = Arc::new(RwLock::new(RosterAuthority::from_roster(initial_roster)));
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
        let restore_diagnostics = Arc::new(RwLock::new(HashMap::new()));
        let wiring = RuntimeWiring {
            roster,
            task_board,
            state,
            restore_diagnostics,
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
            runtime_adapter,
            tool_bundles,
            default_llm_client,
            default_external_tools_provider,
            realm_profile_store,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn start_runtime_with_components(
        definition: Arc<MobDefinition>,
        wiring: RuntimeWiring,
        events: Arc<dyn MobEventStore>,
        run_store: Arc<dyn MobRunStore>,
        session_service: Arc<dyn MobSessionService>,
        runtime_adapter: Option<Arc<meerkat_runtime::MeerkatMachine>>,
        tool_bundles: BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
        default_llm_client: Option<Arc<dyn LlmClient>>,
        default_external_tools_provider: Option<crate::ExternalToolsProvider>,
        realm_profile_store: Option<Arc<dyn crate::store::RealmProfileStore>>,
    ) -> MobHandle {
        let RuntimeWiring {
            roster,
            task_board,
            state,
            restore_diagnostics,
            mcp_servers,
            command_tx,
            command_rx,
        } = wiring;
        let external_backend = definition.backend.external.clone();
        let handle_session_service = session_service.clone();
        let handle = MobHandle {
            command_tx: command_tx.clone(),
            roster: roster.clone(),
            definition: definition.clone(),
            state: state.clone(),
            events: events.clone(),
            flow_streams: Arc::new(tokio::sync::Mutex::new(BTreeMap::new())),
            session_service: handle_session_service.clone(),
            restore_diagnostics: restore_diagnostics.clone(),
        };
        let provisioner: Arc<dyn MobProvisioner> = Arc::new(MultiBackendProvisioner::new(
            session_service,
            runtime_adapter.clone(),
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
        let topology_service = Arc::new(super::topology::MobTopologyService::new(
            definition.topology.clone(),
        ));
        // Only create the orchestrator authority when the definition has an orchestrator.
        // Plain mobs (orchestrator: None) skip orchestrator guards entirely.
        let initial_phase = MobState::from_u8(state.load(Ordering::Acquire));
        let orchestrator = if definition.orchestrator.is_some() {
            let mut orch = match initial_phase {
                MobState::Creating | MobState::Running => {
                    // Fresh creation: start in Creating and initialize.
                    let mut authority =
                        super::mob_orchestrator_authority::MobOrchestratorAuthority::new(
                            state.clone(),
                        );
                    let _ = authority.apply(
                        super::mob_orchestrator_authority::MobOrchestratorInput::InitializeOrchestrator,
                    );
                    authority
                }
                _ => {
                    // Resume: restore to the persisted phase.
                    super::mob_orchestrator_authority::MobOrchestratorAuthority::with_phase(
                        state.clone(),
                        initial_phase,
                    )
                }
            };
            if orch
                .apply(super::mob_orchestrator_authority::MobOrchestratorInput::BindCoordinator)
                .is_ok()
            {
                topology_service.bind_coordinator();
            }
            Some(orch)
        } else {
            None
        };
        let flow_kernel = super::flow_run_kernel::FlowRunKernel::new(
            handle.mob_id().clone(),
            run_store.clone(),
            events.clone(),
        );
        let flow_engine = FlowEngine::new(
            flow_executor,
            handle.clone(),
            run_store.clone(),
            events.clone(),
            topology_service,
            flow_kernel.clone(),
        );
        // Use the initial_phase captured before orchestrator construction to avoid
        // clobbering persisted state (e.g. Completed/Stopped) with Running.
        let lifecycle_authority = super::mob_lifecycle_authority::MobLifecycleAuthority::with_phase(
            state.clone(),
            initial_phase,
        );
        let task_board_service = crate::tasks::MobTaskBoardService::new(
            definition.id.clone(),
            task_board.clone(),
            events.clone(),
        );
        let spawn_policy = Arc::new(super::spawn_policy::SpawnPolicyService::new());

        let actor = MobActor {
            definition,
            roster,
            task_board,
            state,
            events,
            run_store,
            provisioner,
            flow_engine,
            flow_kernel,
            orchestrator,
            run_tasks: BTreeMap::new(),
            run_cancel_tokens: BTreeMap::new(),
            flow_streams: handle.flow_streams.clone(),
            mcp_servers,
            command_tx,
            tool_bundles,
            default_llm_client,
            retired_event_index: Arc::new(RwLock::new(HashSet::new())),
            autonomous_initial_turns: Arc::new(tokio::sync::Mutex::new(BTreeMap::new())),
            next_spawn_ticket: 0,
            next_fence_token: std::sync::atomic::AtomicU64::new(1),
            pending_spawns: PendingSpawnLineage::new(),
            edge_locks: Arc::new(super::edge_locks::EdgeLockRegistry::new()),
            lifecycle_tasks: tokio::task::JoinSet::new(),
            session_service: handle_session_service,
            runtime_adapter,
            restore_diagnostics,
            task_board_service,
            spawn_policy,
            lifecycle_authority,
            default_external_tools_provider,
            realm_profile_store,
        };
        tokio::spawn(actor.run(command_rx));

        handle
    }
}

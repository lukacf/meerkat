use super::*;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use std::collections::HashMap;
#[cfg(feature = "runtime-adapter")]
use std::collections::HashSet;

// ---------------------------------------------------------------------------
// MobBuilder
// ---------------------------------------------------------------------------

/// Builder for creating or resuming a mob.
pub struct MobBuilder {
    mode: BuilderMode,
    storage: MobStorage,
    session_service: Option<Arc<dyn MobSessionService>>,
    #[cfg(feature = "runtime-adapter")]
    runtime_adapter: RuntimeAdapterOption,
    allow_ephemeral_sessions: bool,
    notify_orchestrator_on_resume: bool,
    tool_bundles: BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
    default_llm_client: Option<Arc<dyn LlmClient>>,
    default_external_tools_provider: Option<crate::ExternalToolsProvider>,
    /// Optional realtime session factory injected for mob-provisioned
    /// members that open a realtime channel (W2-E / issue #264).
    ///
    /// Today this field is a carrier: tests can set it via
    /// [`MobBuilder::with_realtime_session_factory`] and retrieve it from
    /// [`MobHandle::realtime_session_factory`] to wire into a
    /// `RealtimeWsHost` bound to the same runtime. Follow-up work lands a
    /// direct mob→host plumbing path so `RealtimeAttached` cells in the
    /// W1-C coverage matrix can be exercised end-to-end without per-test
    /// TCP orchestration.
    realtime_session_factory: Option<Arc<dyn meerkat_client::RealtimeSessionFactory>>,
}

enum BuilderMode {
    Create(Arc<MobDefinition>),
    Resume,
}

/// Construct a `MobMachineAuthority` whose `lifecycle_phase` matches the
/// `initial_phase` threaded from the builder's reconstruction logic.
///
/// The DSL `init(Running)` clause always starts the authority in
/// `MobPhase::Running`; for resumes that landed in `Stopped`, `Completed`,
/// or `Destroyed` we overwrite the phase field directly before handing the
/// authority to the actor. This is the seam that used to live in the
/// separate `Arc<AtomicU8>` projection: with the shadow deleted (dogma
/// #1/#13/#17), the DSL authority becomes the single source of truth at
/// construction time too.
fn seed_mob_authority(
    initial_phase: MobState,
) -> crate::machines::mob_machine::MobMachineAuthority {
    use crate::machines::mob_machine::{MobMachineAuthority, MobPhase};
    let mut authority = MobMachineAuthority::new();
    let dsl_phase = match initial_phase {
        MobState::Creating | MobState::Running => MobPhase::Running,
        MobState::Stopped => MobPhase::Stopped,
        MobState::Completed => MobPhase::Completed,
        MobState::Destroyed => MobPhase::Destroyed,
    };
    authority.state.lifecycle_phase = dsl_phase;
    authority
}

/// Seed the DSL authority's membership-tracking fields from a reconstructed
/// shell roster on resume paths.
///
/// The shell roster is projected from the event log (`Roster::project`), but
/// the DSL authority has no corresponding event-projection because only the
/// live spawn pipeline calls `MobMachineInput::Spawn`. On resume the DSL
/// would otherwise start with empty `live_runtime_ids`, which breaks any
/// MobMachine-owned guard that checks membership — including the work-lane
/// `SubmitWork` transitions that own External/Internal legality.
///
/// This helper replays every live roster entry into the DSL exactly as a
/// fresh spawn would have done, populating `live_runtime_ids`,
/// `runtime_fence_tokens`, `externally_addressable_runtime_ids`, and
/// `identity_to_runtime`. `external_addressable` is resolved from the
/// definition's inline profile (realm-profile overrides are resolved
/// asynchronously elsewhere; callers that need realm overrides can re-feed
/// spawn events through the live pipeline).
fn seed_mob_authority_sync_from_roster(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    roster: &Roster,
    definition: &MobDefinition,
) {
    use crate::machines::mob_machine as mob_dsl;
    for entry in roster.list_all() {
        // Use the inline profile where available. Realm-profile overrides
        // are not observed here (the realm store is async-only); resumes
        // that depend on a realm override for addressability would need to
        // also emit a spawn event through the normal async pipeline, which
        // `handle_submit_work` would then see once the entry re-enters the
        // DSL via that path.
        let external_addressable = definition
            .profiles
            .get(&entry.role)
            .and_then(|binding| binding.as_inline())
            .map(|profile| profile.external_addressable)
            .unwrap_or(false);
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(&entry.agent_identity);
        let dsl_runtime_id = mob_dsl::AgentRuntimeId::from_domain(&entry.agent_runtime_id);
        let dsl_fence = mob_dsl::FenceToken::from_domain(entry.fence_token);
        authority
            .state
            .live_runtime_ids
            .insert(dsl_runtime_id.clone());
        if external_addressable {
            authority
                .state
                .externally_addressable_runtime_ids
                .insert(dsl_runtime_id.clone());
        }
        authority
            .state
            .runtime_fence_tokens
            .insert(dsl_runtime_id.clone(), dsl_fence);
        authority
            .state
            .identity_to_runtime
            .insert(dsl_identity, dsl_runtime_id);
    }
}

struct RuntimeWiring {
    roster: Arc<RwLock<RosterAuthority>>,
    task_board: Arc<RwLock<TaskBoard>>,
    /// Resumed-or-initial phase threaded from the builder's reconstruction
    /// logic into `start_runtime_with_components` where it seeds the DSL
    /// authority. The DSL authority is the single source of truth for the
    /// lifecycle phase — no atomic shadow exists (dogma #1, #13, #17).
    initial_phase: MobState,
    /// DSL authority pre-seeded from the reconstructed roster on resume paths
    /// (and from scratch on create paths). Carried through the wiring so the
    /// actor receives membership-populated authority state before the first
    /// command is processed — MobMachine guards that check
    /// `live_runtime_ids` / `externally_addressable_runtime_ids` see the
    /// resumed members immediately. Boxed so `RuntimeWiring` stays slim
    /// inside the async `resume()` future (the DSL state struct itself is
    /// several hundred bytes worth of maps + sets).
    dsl_authority: Box<crate::machines::mob_machine::MobMachineAuthority>,
    restore_diagnostics: Arc<RwLock<HashMap<MeerkatId, super::handle::RestoreFailureDiagnostic>>>,
    runtime_metadata: Arc<dyn crate::store::MobRuntimeMetadataStore>,
    supervisor_bridge: Arc<MobSupervisorBridge>,
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
            #[cfg(feature = "runtime-adapter")]
            runtime_adapter: None,
            allow_ephemeral_sessions: false,
            notify_orchestrator_on_resume: true,
            tool_bundles: BTreeMap::new(),
            default_llm_client: None,
            default_external_tools_provider: None,
            realtime_session_factory: None,
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
            #[cfg(feature = "runtime-adapter")]
            runtime_adapter: None,
            allow_ephemeral_sessions: false,
            notify_orchestrator_on_resume: true,
            tool_bundles: BTreeMap::new(),
            default_llm_client: None,
            default_external_tools_provider: None,
            realtime_session_factory: None,
        }
    }

    /// Set the session service for creating meerkat sessions.
    ///
    /// The service must implement both `SessionService` and `MobSessionService`
    /// to provide comms runtime access for wiring operations. If no explicit
    /// runtime adapter override has been set yet, the builder seeds its
    /// canonical runtime adapter from `service.runtime_adapter()`.
    pub fn with_session_service(mut self, service: Arc<dyn MobSessionService>) -> Self {
        #[cfg(feature = "runtime-adapter")]
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
    #[cfg(feature = "runtime-adapter")]
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

    /// Inject a [`meerkat_client::RealtimeSessionFactory`] for mob-provisioned
    /// members that open a realtime channel (W2-E / issue #264).
    ///
    /// Tests use this to point mob members at a deterministic in-process
    /// mock (`mock_realtime_ws::RealtimeMockServer`) instead of a live
    /// OpenAI endpoint. The factory is carried through to [`MobHandle`]
    /// so test harnesses can wire it into a `RealtimeWsHost` bound to the
    /// same runtime.
    pub fn with_realtime_session_factory(
        mut self,
        factory: Arc<dyn meerkat_client::RealtimeSessionFactory>,
    ) -> Self {
        self.realtime_session_factory = Some(factory);
        self
    }

    /// Create the mob: emit MobCreated event, start the actor, return handle.
    #[cfg(feature = "runtime-adapter")]
    pub async fn create(self) -> Result<MobHandle, MobError> {
        let MobBuilder {
            mode,
            storage,
            session_service,
            #[cfg(feature = "runtime-adapter")]
            runtime_adapter,
            allow_ephemeral_sessions,
            notify_orchestrator_on_resume: _,
            tool_bundles,
            default_llm_client,
            default_external_tools_provider,
            realtime_session_factory,
        } = self;
        #[cfg(not(feature = "runtime-adapter"))]
        let runtime_adapter: RuntimeAdapterOption = None;

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
        #[cfg(feature = "runtime-adapter")]
        {
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
        Self::ensure_supervisor_authority(storage.runtime_metadata.clone(), definition.id.clone())
            .await?;
        let supervisor_authority = storage
            .runtime_metadata
            .load_supervisor_authority(&definition.id)
            .await?
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "missing supervisor runtime metadata for newly created mob '{}'",
                    definition.id
                ))
            })?;
        let supervisor_bridge = Arc::new(MobSupervisorBridge::new(
            &definition.id,
            supervisor_authority,
        )?);

        Ok(Self::start_runtime(
            definition,
            Roster::new(),
            TaskBoard::default(),
            MobState::Running,
            storage.events.clone(),
            storage.runs.clone(),
            storage.runtime_metadata.clone(),
            supervisor_bridge,
            session_service,
            runtime_adapter,
            tool_bundles,
            default_llm_client,
            default_external_tools_provider,
            storage.realm_profiles.clone(),
            realtime_session_factory,
        ))
    }

    /// Resume a mob from persisted events.
    ///
    /// Resume behavior:
    /// - Recover definition from `MobCreated`.
    /// - Rebuild roster by replaying structural events.
    /// - Start actor/runtime in Running state.
    #[cfg(feature = "runtime-adapter")]
    pub async fn resume(self) -> Result<MobHandle, MobError> {
        let MobBuilder {
            mode,
            storage,
            session_service,
            #[cfg(feature = "runtime-adapter")]
            runtime_adapter,
            allow_ephemeral_sessions,
            notify_orchestrator_on_resume,
            tool_bundles,
            default_llm_client,
            default_external_tools_provider,
            realtime_session_factory,
        } = self;
        #[cfg(not(feature = "runtime-adapter"))]
        let runtime_adapter: RuntimeAdapterOption = None;

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
        #[allow(unused_mut)]
        let mut mob_events: Vec<_> = all_events
            .into_iter()
            .filter(|event| event.mob_id == definition.id)
            .collect();
        // Runtime metadata is authoritative for supervisor and external-binding
        // normalization state. Older mobs created before bridge metadata
        // existed will not have a supervisor record yet, so resume must seed
        // one before treating absence as corruption.
        Self::ensure_supervisor_authority(storage.runtime_metadata.clone(), definition.id.clone())
            .await?;
        let supervisor_authority = storage
            .runtime_metadata
            .load_supervisor_authority(&definition.id)
            .await?
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "cannot resume mob '{}': missing supervisor runtime metadata",
                    definition.id
                ))
            })?;
        let supervisor_bridge = Arc::new(MobSupervisorBridge::new(
            &definition.id,
            supervisor_authority,
        )?);
        #[cfg(not(target_arch = "wasm32"))]
        let seeded_restore_diagnostics = {
            let binding_overlays = storage
                .runtime_metadata
                .list_external_binding_overlays(&definition.id)
                .await?;
            Self::apply_external_binding_overlays(&mut mob_events, &binding_overlays)
        };
        #[cfg(target_arch = "wasm32")]
        let seeded_restore_diagnostics = HashMap::new();
        #[cfg(not(target_arch = "wasm32"))]
        Self::normalize_sessionless_backend_runtime_modes(&mut mob_events);
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
        let restore_diagnostics = Arc::new(RwLock::new(seeded_restore_diagnostics));
        // Preview phase watch so the preview handle can answer status()
        // before the actor spawns. The real actor-side sender replaces
        // this once start_runtime_with_components owns the final pair.
        let (_preview_phase_tx, preview_phase_rx) = tokio::sync::watch::channel(resumed_state);
        let mut wiring = RuntimeWiring {
            roster: roster_state.clone(),
            task_board: task_board_state.clone(),
            initial_phase: resumed_state,
            // Placeholder; the final authority is seeded below after
            // `reconcile_resume` finalizes the shell roster. The DSL
            // membership state is populated from the finalized roster so
            // MobMachine guards see the resumed members immediately.
            dsl_authority: Box::new(seed_mob_authority(resumed_state)),
            restore_diagnostics: restore_diagnostics.clone(),
            runtime_metadata: storage.runtime_metadata.clone(),
            supervisor_bridge: supervisor_bridge.clone(),
            mcp_servers: mcp_servers.clone(),
            command_tx: command_tx.clone(),
            command_rx,
        };
        // W3-H: preview handle gets a placeholder broadcast sender. The
        // handle is only used for reconcile_resume which doesn't invoke
        // realtime binding paths; the real binding channel is created in
        // start_runtime_with_components alongside the actor that will emit
        // onto it.
        let (preview_realtime_binding_tx, _) = tokio::sync::broadcast::channel(1);
        let preview_handle = MobHandle {
            command_tx: command_tx.clone(),
            roster: roster_state.clone(),
            definition: definition.clone(),
            events: storage.events.clone(),
            flow_streams: Arc::new(tokio::sync::Mutex::new(BTreeMap::new())),
            session_service: session_service.clone(),
            runtime_adapter: runtime_adapter.clone(),
            restore_diagnostics,
            phase_watch_rx: preview_phase_rx,
            realtime_session_factory: realtime_session_factory.clone(),
            realtime_binding_tx: preview_realtime_binding_tx,
        };
        // session_service is still live here (not consumed until start_runtime_with_components)

        if resumed_state == MobState::Running {
            Self::reconcile_resume(
                &definition,
                &mut roster,
                &session_service,
                runtime_adapter.clone(),
                supervisor_bridge.clone(),
                notify_orchestrator_on_resume,
                default_llm_client.clone(),
                &tool_bundles,
                &preview_handle,
                &default_external_tools_provider,
                storage.realm_profiles.clone(),
            )
            .await?;
        }

        // Seed the DSL authority from the finalized roster. After
        // `reconcile_resume` the roster reflects every member that was
        // alive at resume time; replaying those as DSL spawns is what
        // lets MobMachine guards (SubmitWork legality, Retire membership,
        // etc.) see resumed members on the first command.
        seed_mob_authority_sync_from_roster(&mut wiring.dsl_authority, &roster, &definition);
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
            realtime_session_factory,
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

    async fn ensure_supervisor_authority(
        runtime_metadata: Arc<dyn crate::store::MobRuntimeMetadataStore>,
        mob_id: MobId,
    ) -> Result<(), MobError> {
        const MOB_RUNTIME_BRIDGE_PROTOCOL_VERSION: u32 = 1;

        if runtime_metadata
            .load_supervisor_authority(&mob_id)
            .await?
            .is_none()
        {
            let record = crate::store::SupervisorAuthorityRecord::generate(
                MOB_RUNTIME_BRIDGE_PROTOCOL_VERSION,
            );
            runtime_metadata
                .put_supervisor_authority_if_absent(&mob_id, &record)
                .await?;
        }

        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn apply_external_binding_overlays(
        mob_events: &mut [crate::event::MobEvent],
        overlays: &[crate::store::ExternalBindingOverlayRecord],
    ) -> HashMap<MeerkatId, super::handle::RestoreFailureDiagnostic> {
        let overlay_index = overlays
            .iter()
            .map(|overlay| {
                (
                    (overlay.agent_identity.clone(), overlay.generation),
                    overlay.clone(),
                )
            })
            .collect::<HashMap<_, _>>();
        let mut diagnostics = HashMap::new();

        for event in mob_events {
            let Some(member_spawned) = event.kind.member_spawned_mut() else {
                continue;
            };
            let Some(overlay) = overlay_index.get(&(
                member_spawned.agent_identity.clone(),
                member_spawned.generation,
            )) else {
                continue;
            };

            let original_member_ref = member_spawned.bridge_member_ref.clone();
            match &overlay.status {
                crate::store::ExternalBindingOverlayStatus::Normalized => {
                    if let Some(normalized_member_ref) = overlay.normalized_member_ref.clone() {
                        member_spawned.bridge_member_ref =
                            Some(Self::member_ref_with_bootstrap_token(
                                normalized_member_ref,
                                overlay.bootstrap_token.clone(),
                            ));
                    }
                }
                crate::store::ExternalBindingOverlayStatus::Failed { reason } => {
                    let normalized_member_ref =
                        overlay.normalized_member_ref.clone().or_else(|| {
                            original_member_ref
                                .as_ref()
                                .and_then(Self::sessionless_member_ref)
                        });
                    member_spawned.bridge_member_ref = normalized_member_ref.map(|member_ref| {
                        Self::member_ref_with_bootstrap_token(
                            member_ref,
                            overlay.bootstrap_token.clone(),
                        )
                    });
                    diagnostics.insert(
                        MeerkatId::from(member_spawned.agent_identity.as_str()),
                        super::handle::RestoreFailureDiagnostic {
                            bridge_session_id: original_member_ref
                                .as_ref()
                                .and_then(crate::event::MemberRef::bridge_session_id)
                                .cloned(),
                            reason: reason.clone(),
                        },
                    );
                }
            }

            member_spawned.runtime_mode = Self::normalize_runtime_mode_for_member_ref(
                member_spawned.runtime_mode,
                member_spawned.bridge_member_ref.as_ref(),
            );
        }

        diagnostics
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn normalize_sessionless_backend_runtime_modes(mob_events: &mut [crate::event::MobEvent]) {
        for event in mob_events {
            let Some(member_spawned) = event.kind.member_spawned_mut() else {
                continue;
            };
            member_spawned.runtime_mode = Self::normalize_runtime_mode_for_member_ref(
                member_spawned.runtime_mode,
                member_spawned.bridge_member_ref.as_ref(),
            );
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn normalize_runtime_mode_for_member_ref(
        runtime_mode: crate::MobRuntimeMode,
        member_ref: Option<&crate::event::MemberRef>,
    ) -> crate::MobRuntimeMode {
        match member_ref {
            Some(crate::event::MemberRef::BackendPeer {
                session_id: None, ..
            }) => crate::MobRuntimeMode::TurnDriven,
            _ => runtime_mode,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn sessionless_member_ref(
        member_ref: &crate::event::MemberRef,
    ) -> Option<crate::event::MemberRef> {
        match member_ref {
            crate::event::MemberRef::BackendPeer {
                peer_id,
                address,
                bootstrap_token,
                session_id: _,
            } => Some(crate::event::MemberRef::BackendPeer {
                peer_id: peer_id.clone(),
                address: super::bridge_protocol::canonicalize_bridge_address(address),
                bootstrap_token: bootstrap_token.clone(),
                session_id: None,
            }),
            crate::event::MemberRef::Session { .. } => None,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn member_ref_with_bootstrap_token(
        member_ref: crate::event::MemberRef,
        bootstrap_token: Option<super::bridge_protocol::BridgeBootstrapToken>,
    ) -> crate::event::MemberRef {
        match member_ref {
            crate::event::MemberRef::BackendPeer {
                peer_id,
                address,
                session_id,
                ..
            } => crate::event::MemberRef::BackendPeer {
                peer_id,
                address: super::bridge_protocol::canonicalize_bridge_address(&address),
                bootstrap_token,
                session_id,
            },
            crate::event::MemberRef::Session { session_id } => {
                crate::event::MemberRef::Session { session_id }
            }
        }
    }

    #[cfg(feature = "runtime-adapter")]
    #[allow(clippy::too_many_arguments)]
    async fn reconcile_resume(
        definition: &Arc<MobDefinition>,
        roster: &mut Roster,
        session_service: &Arc<dyn MobSessionService>,
        runtime_adapter: RuntimeAdapterOption,
        supervisor_bridge: Arc<MobSupervisorBridge>,
        notify_orchestrator_on_resume: bool,
        default_llm_client: Option<Arc<dyn LlmClient>>,
        tool_bundles: &BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
        tool_handle: &MobHandle,
        default_external_tools_provider: &Option<crate::ExternalToolsProvider>,
        realm_profile_store: Option<Arc<dyn crate::store::RealmProfileStore>>,
    ) -> Result<(), MobError> {
        let provisioner = MultiBackendProvisioner::new(
            session_service.clone(),
            runtime_adapter.clone(),
            definition.backend.external.clone(),
            supervisor_bridge,
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
                    entry.agent_identity.clone(),
                    super::handle::RestoreFailureDiagnostic {
                        bridge_session_id: Some(bridge_session_id.clone()),
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
                            agent_identity: &entry.agent_identity,
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
                    entry.agent_identity, entry.role, definition.id
                );
                let req = build::to_create_session_request(&resumed_config, prompt.into());
                match session_service.create_session(req).await {
                    Ok(created) => {
                        let created_bridge_session_id = created.session_id;
                        let _ = roster.set_bridge_session_id(
                            &entry.agent_identity,
                            created_bridge_session_id.clone(),
                        );
                        tool_handle
                            .restore_diagnostics
                            .write()
                            .await
                            .remove(&entry.agent_identity);
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
                agent_identity: &entry.agent_identity,
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
                entry.agent_identity, entry.role, definition.id
            );
            let req = build::to_create_session_request(&config, prompt.into());
            let created = session_service.create_session(req).await?;
            let created_bridge_session_id = created.session_id;
            let _ = roster
                .set_bridge_session_id(&entry.agent_identity, created_bridge_session_id.clone());
            tool_handle
                .restore_diagnostics
                .write()
                .await
                .remove(&entry.agent_identity);
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
            if broken_members.contains(&entry.agent_identity) {
                let _ = roster.set_peer_id(&entry.agent_identity, None);
                continue;
            }
            let local_comms = provisioner.comms_runtime(&entry.member_ref).await;
            if let Some(comms_a) = &local_comms {
                let key_a = comms_a.public_key().ok_or_else(|| {
                    MobError::WiringError(format!(
                        "resume requires public key for wired member '{}'",
                        entry.agent_identity
                    ))
                })?;
                let _ = roster.set_peer_id(&entry.agent_identity, Some(key_a.clone()));
            } else if entry.wired_to.is_empty() {
                continue;
            }
            let mut desired_specs = Vec::new();
            let mut candidate_specs = Vec::new();

            for peer_entry in &entries {
                if peer_entry.agent_identity == entry.agent_identity
                    || broken_members.contains(&peer_entry.agent_identity)
                {
                    continue;
                }
                let name_b = format!(
                    "{}/{}/{}",
                    definition.id, peer_entry.role, peer_entry.agent_identity
                );
                let fallback_peer_id = match provisioner.comms_runtime(&peer_entry.member_ref).await
                {
                    Some(comms_b) => comms_b.public_key().ok_or_else(|| {
                        MobError::WiringError(format!(
                            "resume requires public key for '{}' -> '{}'",
                            entry.agent_identity, peer_entry.agent_identity
                        ))
                    })?,
                    None => match &peer_entry.member_ref {
                        crate::event::MemberRef::BackendPeer {
                            peer_id,
                            session_id: None,
                            ..
                        } => peer_id.clone(),
                        _ => {
                            return Err(MobError::WiringError(format!(
                                "resume requires comms runtime for '{}' -> '{}'",
                                entry.agent_identity, peer_entry.agent_identity
                            )));
                        }
                    },
                };
                candidate_specs.push(
                    provisioner
                        .trusted_peer_spec(&peer_entry.member_ref, &name_b, &fallback_peer_id)
                        .await?,
                );
            }

            for peer_identity in &entry.wired_to {
                let peer_meerkat_id = MeerkatId::from(peer_identity.as_str());
                if let Some(spec) = entry.external_peer_specs.get(&peer_meerkat_id) {
                    desired_specs.push(spec.clone());
                    continue;
                }
                let peer_entry = roster.get(&peer_meerkat_id).cloned().ok_or_else(|| {
                    MobError::WiringError(format!(
                        "resume wiring target '{}' missing for '{}'",
                        peer_identity, entry.agent_identity
                    ))
                })?;
                if broken_members.contains(&peer_meerkat_id) {
                    continue;
                }
                let name_b = format!(
                    "{}/{}/{}",
                    definition.id, peer_entry.role, peer_entry.agent_identity
                );
                let fallback_peer_id = match provisioner.comms_runtime(&peer_entry.member_ref).await
                {
                    Some(comms_b) => comms_b.public_key().ok_or_else(|| {
                        MobError::WiringError(format!(
                            "resume requires public key for '{}' -> '{}'",
                            entry.agent_identity, peer_identity
                        ))
                    })?,
                    None => match &peer_entry.member_ref {
                        crate::event::MemberRef::BackendPeer {
                            peer_id,
                            session_id: None,
                            ..
                        } => peer_id.clone(),
                        _ => {
                            return Err(MobError::WiringError(format!(
                                "resume requires comms runtime for '{}' -> '{}'",
                                entry.agent_identity, peer_identity
                            )));
                        }
                    },
                };
                desired_specs.push(
                    provisioner
                        .trusted_peer_spec(&peer_entry.member_ref, &name_b, &fallback_peer_id)
                        .await?,
                );
            }

            provisioner
                .reconcile_member_trust(&entry.member_ref, &desired_specs, &candidate_specs)
                .await?;
        }
        // Notify orchestrator that the mob resumed.
        if notify_orchestrator_on_resume
            && let Some(orchestrator) = &definition.orchestrator
            && let Some(orchestrator_entry) = roster.by_profile(&orchestrator.profile).next()
        {
            if broken_members.contains(&orchestrator_entry.agent_identity) {
                tracing::warn!(
                    member_id = %orchestrator_entry.agent_identity,
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
                                orchestrator_entry.agent_identity
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
                                orchestrator_entry.agent_identity, error
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

    #[cfg(feature = "runtime-adapter")]
    #[allow(clippy::too_many_arguments)]
    fn start_runtime(
        definition: Arc<MobDefinition>,
        initial_roster: Roster,
        initial_task_board: TaskBoard,
        initial_state: MobState,
        events: Arc<dyn MobEventStore>,
        run_store: Arc<dyn MobRunStore>,
        runtime_metadata: Arc<dyn crate::store::MobRuntimeMetadataStore>,
        supervisor_bridge: Arc<MobSupervisorBridge>,
        session_service: Arc<dyn MobSessionService>,
        runtime_adapter: RuntimeAdapterOption,
        tool_bundles: BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
        default_llm_client: Option<Arc<dyn LlmClient>>,
        default_external_tools_provider: Option<crate::ExternalToolsProvider>,
        realm_profile_store: Option<Arc<dyn crate::store::RealmProfileStore>>,
        realtime_session_factory: Option<Arc<dyn meerkat_client::RealtimeSessionFactory>>,
    ) -> MobHandle {
        // Seed the DSL authority from the reconstructed shell roster so the
        // MobMachine sees already-spawned members immediately on resume.
        // Profile lookup is best-effort-sync here — resume paths thread a
        // concrete profile via `start_runtime_with_components`; fresh create
        // paths pass an empty roster so the replay is a no-op either way.
        let mut dsl_authority = Box::new(seed_mob_authority(initial_state));
        seed_mob_authority_sync_from_roster(&mut dsl_authority, &initial_roster, &definition);
        let roster = Arc::new(RwLock::new(RosterAuthority::from_roster(initial_roster)));
        let task_board = Arc::new(RwLock::new(initial_task_board));
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
            initial_phase: initial_state,
            dsl_authority,
            restore_diagnostics,
            runtime_metadata,
            supervisor_bridge,
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
            realtime_session_factory,
        )
    }

    #[cfg(feature = "runtime-adapter")]
    #[allow(clippy::too_many_arguments)]
    fn start_runtime_with_components(
        definition: Arc<MobDefinition>,
        wiring: RuntimeWiring,
        events: Arc<dyn MobEventStore>,
        run_store: Arc<dyn MobRunStore>,
        session_service: Arc<dyn MobSessionService>,
        runtime_adapter: RuntimeAdapterOption,
        tool_bundles: BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
        default_llm_client: Option<Arc<dyn LlmClient>>,
        default_external_tools_provider: Option<crate::ExternalToolsProvider>,
        realm_profile_store: Option<Arc<dyn crate::store::RealmProfileStore>>,
        realtime_session_factory: Option<Arc<dyn meerkat_client::RealtimeSessionFactory>>,
    ) -> MobHandle {
        let RuntimeWiring {
            roster,
            task_board,
            initial_phase: wiring_initial_phase,
            dsl_authority,
            restore_diagnostics,
            runtime_metadata,
            supervisor_bridge,
            mcp_servers,
            command_tx,
            command_rx,
        } = wiring;
        let external_backend = definition.backend.external.clone();
        let handle_session_service = session_service.clone();
        // Terminal-phase watch: seed with the initial phase so a status()
        // call before any DSL transition returns the right answer.
        let (phase_watch_tx_actor, phase_watch_rx) =
            tokio::sync::watch::channel(wiring_initial_phase);
        // W3-H: broadcast channel for `MemberRealtimeBindingEvent`s. 128 is a
        // comfortable capacity for the typical event rate (one event per
        // spawn/respawn/retire). Slow subscribers are lag-tolerant — they
        // miss intermediate rotations but can re-read the canonical binding
        // map from the DSL authority on recovery.
        let (realtime_binding_tx, _) = tokio::sync::broadcast::channel(128);
        let handle = MobHandle {
            command_tx: command_tx.clone(),
            roster: roster.clone(),
            definition: definition.clone(),
            events: events.clone(),
            flow_streams: Arc::new(tokio::sync::Mutex::new(BTreeMap::new())),
            session_service: handle_session_service.clone(),
            runtime_adapter: runtime_adapter.clone(),
            restore_diagnostics: restore_diagnostics.clone(),
            phase_watch_rx,
            realtime_session_factory,
            realtime_binding_tx: realtime_binding_tx.clone(),
        };
        let provisioner: Arc<dyn MobProvisioner> = Arc::new(
            MultiBackendProvisioner::new(
                session_service,
                runtime_adapter.clone(),
                external_backend,
                supervisor_bridge.clone(),
            )
            .with_binding_persistence(
                definition.id.clone(),
                runtime_metadata.clone(),
                roster.clone(),
            ),
        );
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
        // Normalize initial phase: fresh creation + stale Creating restores both
        // become Running. Persisted Stopped/Completed/Destroyed are preserved.
        let initial_phase = match wiring_initial_phase {
            MobState::Creating => MobState::Running,
            phase => phase,
        };
        // Plain mobs (orchestrator: None in the definition) skip orchestrator
        // guards entirely; mobs with an orchestrator bind the topology
        // coordinator when entering Running.
        let has_orchestrator = definition.orchestrator.is_some();
        if has_orchestrator && initial_phase == MobState::Running {
            topology_service.bind_coordinator();
        }
        let flow_engine = FlowEngine::new(
            flow_executor,
            handle.clone(),
            run_store.clone(),
            events.clone(),
            topology_service,
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
            events,
            run_store,
            provisioner,
            flow_engine,
            has_orchestrator,
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
            #[cfg(feature = "runtime-adapter")]
            runtime_adapter,
            restore_diagnostics,
            runtime_metadata,
            supervisor_bridge,
            task_board_service,
            spawn_policy,
            dsl_authority: *dsl_authority,
            phase_watch_tx: phase_watch_tx_actor,
            default_external_tools_provider,
            realm_profile_store,
            realtime_binding_tx,
        };
        tokio::spawn(actor.run(command_rx));

        handle
    }
}

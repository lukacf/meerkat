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

#[cfg(feature = "runtime-adapter")]
fn canonical_runtime_adapter_for_session_service(
    session_service: &Arc<dyn MobSessionService>,
    runtime_adapter: RuntimeAdapterOption,
) -> Result<RuntimeAdapterOption, MobError> {
    let service_adapter = session_service.runtime_adapter();
    match (runtime_adapter, service_adapter) {
        (Some(adapter), Some(service_adapter))
            if !adapter.shares_runtime_persistence_with(&service_adapter) =>
        {
            Err(MobError::Internal(
                "explicit mob runtime adapter does not share the session service runtime persistence authority".to_string(),
            ))
        }
        (Some(adapter), _) => Ok(Some(adapter)),
        (None, service_adapter) => Ok(service_adapter),
    }
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
/// `identity_to_runtime`. It also rehydrates machine-owned local and external
/// wiring edges from the event-projected roster so respawn/reconcile restore
/// logic can read topology from MobMachine instead of using roster fields as
/// authority. `external_addressable` is resolved from the definition's inline
/// profile (realm-profile overrides are resolved asynchronously elsewhere;
/// callers that need realm overrides can re-feed spawn events through the live
/// pipeline).
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

        for peer_identity in &entry.wired_to {
            if let Some(peer_entry) = roster.get_by_identity(peer_identity) {
                authority
                    .state
                    .wiring_edges
                    .insert(mob_dsl::WiringEdge::new(
                        mob_dsl::AgentIdentity::from_domain(&entry.agent_identity),
                        mob_dsl::AgentIdentity::from_domain(&peer_entry.agent_identity),
                    ));
            }
        }
        for spec in entry.external_peer_specs.values() {
            authority
                .state
                .external_peer_edges
                .insert(mob_dsl::ExternalPeerEdge::new(
                    mob_dsl::AgentIdentity::from_domain(&entry.agent_identity),
                    mob_dsl::ExternalPeerEndpoint::from(spec),
                ));
        }
    }
}

async fn seed_mob_authority_sync_from_flow_runs(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    run_store: Arc<dyn crate::store::MobRunStore>,
    event_store: Arc<dyn crate::store::MobEventStore>,
    mob_id: &MobId,
) -> Result<(), MobError> {
    use crate::machines::mob_machine as mob_dsl;
    use crate::run::MobRun;

    let terminalization = super::terminalization::FlowTerminalizationAuthority::new(
        run_store.clone(),
        event_store,
        mob_id.clone(),
    );
    let mut runs = run_store.list_runs(mob_id, None).await?;
    runs.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then_with(|| left.run_id.to_string().cmp(&right.run_id.to_string()))
    });

    let resumed_phase = authority.state.lifecycle_phase;
    for mut run in runs {
        super::recovery::reconcile_run_state(&mut run).map_err(|error| {
            MobError::Internal(format!("cannot resume flow run '{}': {error}", run.run_id))
        })?;
        if run.status.is_terminal() || run.flow_authority_inputs.is_empty() {
            continue;
        }

        authority.state.lifecycle_phase = mob_dsl::MobPhase::Running;
        MobRun::replay_flow_authority_inputs_into(
            authority,
            &run.flow_authority_inputs,
            &format!("resume_flow_run_{}", run.run_id),
        )?;
        authority.state.lifecycle_phase = resumed_phase;
        if flow_run_replayed_active_admission(&run) {
            converge_recovered_active_flow_run(
                authority,
                run_store.clone(),
                &terminalization,
                run.run_id.clone(),
            )
            .await?;
            authority.state.lifecycle_phase = resumed_phase;
        }
    }
    authority.state.lifecycle_phase = resumed_phase;
    Ok(())
}

fn flow_run_replayed_active_admission(run: &crate::run::MobRun) -> bool {
    run.flow_authority_inputs
        .iter()
        .any(|record| matches!(record, crate::run::FlowAuthorityInputRecord::RunFlow(_)))
}

async fn converge_recovered_active_flow_run(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    run_store: Arc<dyn crate::store::MobRunStore>,
    terminalization: &super::terminalization::FlowTerminalizationAuthority,
    run_id: crate::ids::RunId,
) -> Result<(), MobError> {
    use crate::machines::mob_machine as mob_dsl;
    use crate::run::{MobMachineFlowRunCommand, MobRunStatus, flow_run};

    start_recovered_flow_run_if_pending(authority, run_store.clone(), &run_id).await?;
    cancel_recovered_unfinished_steps(authority, run_store.clone(), &run_id).await?;

    let before_terminal = run_store
        .get_run(&run_id)
        .await?
        .ok_or_else(|| MobError::RunNotFound(run_id.clone()))?;
    let transitioned = if before_terminal.status.is_terminal() {
        false
    } else {
        commit_recovered_flow_run_command(
            authority,
            run_store.clone(),
            &run_id,
            MobMachineFlowRunCommand::TerminalizeCanceled(flow_run::inputs::TerminalizeCanceled {}),
            Some(MobRunStatus::Canceled),
            "resume_recovered_flow_terminalize_canceled",
        )
        .await?
    };
    if transitioned {
        terminalization
            .record_persisted_terminalization(
                run_id.clone(),
                before_terminal.flow_id,
                super::terminalization::TerminalizationTarget::Canceled,
            )
            .await?;
    }

    apply_recovered_flow_signal(
        authority,
        mob_dsl::MobMachineSignal::CompleteFlow,
        "resume_recovered_flow_complete",
    )?;
    apply_recovered_flow_signal(
        authority,
        mob_dsl::MobMachineSignal::FinishRun,
        "resume_recovered_flow_finish",
    )?;
    Ok(())
}

async fn start_recovered_flow_run_if_pending(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    run_store: Arc<dyn crate::store::MobRunStore>,
    run_id: &crate::ids::RunId,
) -> Result<(), MobError> {
    use crate::run::{MobMachineFlowRunCommand, MobRunStatus, flow_run};

    let run = run_store
        .get_run(run_id)
        .await?
        .ok_or_else(|| MobError::RunNotFound(run_id.clone()))?;
    if run.status != MobRunStatus::Pending {
        return Ok(());
    }
    let _ = commit_recovered_flow_run_command(
        authority,
        run_store,
        run_id,
        MobMachineFlowRunCommand::StartRun(flow_run::inputs::StartRun {}),
        Some(MobRunStatus::Running),
        "resume_recovered_flow_start",
    )
    .await?;
    Ok(())
}

async fn cancel_recovered_unfinished_steps(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    run_store: Arc<dyn crate::store::MobRunStore>,
    run_id: &crate::ids::RunId,
) -> Result<(), MobError> {
    use crate::run::{MobMachineFlowRunCommand, flow_run};

    let run = run_store
        .get_run(run_id)
        .await?
        .ok_or_else(|| MobError::RunNotFound(run_id.clone()))?;
    for step_id in run.ordered_steps()? {
        let is_terminal = run
            .flow_state
            .step_status
            .get(&step_id)
            .and_then(|status| *status)
            .is_some_and(|status| {
                matches!(
                    status,
                    flow_run::StepRunStatus::Completed
                        | flow_run::StepRunStatus::Failed
                        | flow_run::StepRunStatus::Skipped
                        | flow_run::StepRunStatus::Canceled
                )
            });
        if is_terminal {
            continue;
        }
        let _ = commit_recovered_flow_run_command(
            authority,
            run_store.clone(),
            run_id,
            MobMachineFlowRunCommand::CancelStep(flow_run::inputs::CancelStep { step_id }),
            None,
            "resume_recovered_flow_cancel_step",
        )
        .await?;
    }
    Ok(())
}

async fn commit_recovered_flow_run_command(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    run_store: Arc<dyn crate::store::MobRunStore>,
    run_id: &crate::ids::RunId,
    command: crate::run::MobMachineFlowRunCommand,
    next_status: Option<crate::run::MobRunStatus>,
    context: &'static str,
) -> Result<bool, MobError> {
    use crate::machines::mob_machine as mob_dsl;
    use crate::run::{MobMachineFlowAuthorityToken, apply_mob_machine_flow_run_command};

    let authority_input = command.authority_input(run_id);
    for attempt in 0..5u32 {
        let run = run_store
            .get_run(run_id)
            .await?
            .ok_or_else(|| MobError::RunNotFound(run_id.clone()))?;
        if run.status.is_terminal() {
            return Ok(false);
        }

        let mut prepared = mob_dsl::MobMachineAuthority::from_state(authority.state.clone());
        let input_debug = format!("{authority_input:?}");
        let transition =
            mob_dsl::MobMachineMutator::apply(&mut prepared, authority_input.clone()).map_err(
                |error| {
                    MobError::Internal(format!(
                        "MobMachine recovered flow authority ({context}) rejected {input_debug}: {error}"
                    ))
                },
            )?;
        if transition.from_phase != transition.to_phase {
            prepared.state.lifecycle_phase = transition.to_phase;
        }
        let token =
            MobMachineFlowAuthorityToken::from_accepted_mob_machine_input(&authority_input)?;
        let outcome = apply_mob_machine_flow_run_command(
            &run.flow_state,
            &prepared.state,
            run_id,
            command.clone(),
            token,
        )?;

        let won = if let Some(next_status) = &next_status {
            run_store
                .cas_run_snapshot_with_authority(
                    run_id,
                    run.status.clone(),
                    &run.flow_state,
                    next_status.clone(),
                    &outcome.next_state,
                    vec![authority_input.clone()],
                )
                .await?
        } else {
            run_store
                .cas_flow_state_with_authority(
                    run_id,
                    &run.flow_state,
                    &outcome.next_state,
                    vec![authority_input.clone()],
                )
                .await?
        };
        if won {
            *authority = prepared;
            return Ok(true);
        }
        if attempt < 4 {
            tracing::debug!(attempt, context, "recovered flow CAS contention, retrying");
        }
    }
    Err(MobError::Internal(format!(
        "{context}: CAS contention after 5 attempts for recovered run {run_id}"
    )))
}

fn apply_recovered_flow_signal(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    signal: crate::machines::mob_machine::MobMachineSignal,
    context: &'static str,
) -> Result<(), MobError> {
    let signal_debug = format!("{signal:?}");
    let transition = authority.apply_signal(signal).map_err(|error| {
        MobError::Internal(format!(
            "MobMachine recovered flow signal ({context}) rejected {signal_debug}: {error}"
        ))
    })?;
    if transition.from_phase != transition.to_phase {
        authority.state.lifecycle_phase = transition.to_phase;
    }
    Ok(())
}

fn seed_mob_authority_restore_failures(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    restore_diagnostics: &HashMap<MeerkatId, super::handle::RestoreFailureDiagnostic>,
) {
    use crate::machines::mob_machine as mob_dsl;
    for (identity, diagnostic) in restore_diagnostics {
        authority.state.member_restore_failures.insert(
            mob_dsl::AgentIdentity::from_domain(&crate::ids::AgentIdentity::from(
                identity.as_str(),
            )),
            diagnostic.reason.clone(),
        );
    }
}

struct RuntimeWiring {
    roster: Arc<RwLock<RosterAuthority>>,
    task_board: Arc<RwLock<TaskBoard>>,
    /// Observable phase threaded from the builder's reconstruction logic into
    /// `start_runtime_with_components`. This can differ from the DSL authority
    /// phase after a retained `MobDestroying` marker: public authority is
    /// terminal, while the DSL authority must still be able to replay missing
    /// retire/destroy cleanup transitions on retry.
    public_phase: MobState,
    destroy_admitted: bool,
    /// DSL authority pre-seeded from the reconstructed roster on resume paths
    /// (and from scratch on create paths). Carried through the wiring so the
    /// actor receives membership-populated authority state before the first
    /// command is processed — MobMachine guards that check
    /// `live_runtime_ids` / `externally_addressable_runtime_ids` see the
    /// resumed members immediately. Boxed so `RuntimeWiring` stays slim
    /// inside the async `resume()` future (the DSL state struct itself is
    /// several hundred bytes worth of maps + sets).
    dsl_authority: Box<crate::machines::mob_machine::MobMachineAuthority>,
    machine_state_watch_tx:
        tokio::sync::watch::Sender<crate::machines::mob_machine::MobMachineState>,
    restore_diagnostics: Arc<RwLock<HashMap<MeerkatId, super::handle::RestoreFailureDiagnostic>>>,
    runtime_metadata: Arc<dyn crate::store::MobRuntimeMetadataStore>,
    supervisor_bridge: Arc<MobSupervisorBridge>,
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
        let runtime_adapter =
            canonical_runtime_adapter_for_session_service(&session_service, runtime_adapter)?;

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
        let runtime_adapter =
            canonical_runtime_adapter_for_session_service(&session_service, runtime_adapter)?;
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
        // Determine resumed state from events in the current epoch (after the
        // last MobReset, or all events if no reset has occurred). Do this
        // before supervisor-authority recovery so destroy-finalizing storage
        // can fail closed instead of minting replacement live authority.
        let epoch_start = mob_events
            .iter()
            .rposition(|event| matches!(event.kind, MobEventKind::MobReset))
            .map_or(0, |pos| pos + 1);
        let epoch_events = &mob_events[epoch_start..];
        let destroy_storage_finalizing = epoch_events
            .iter()
            .any(|event| matches!(event.kind, MobEventKind::MobDestroyStorageFinalizing));
        let destroy_admitted = epoch_events.iter().any(|event| {
            matches!(
                event.kind,
                MobEventKind::MobDestroying | MobEventKind::MobDestroyStorageFinalizing
            )
        });
        let resumed_state = if destroy_admitted {
            MobState::Destroyed
        } else if epoch_events
            .iter()
            .any(|event| matches!(event.kind, MobEventKind::MobCompleted))
        {
            MobState::Completed
        } else {
            MobState::Running
        };
        let dsl_seed_state = if destroy_admitted && !destroy_storage_finalizing {
            MobState::Running
        } else {
            resumed_state
        };
        // Runtime metadata owns supervisor authority. External-binding
        // overlays are compatibility projections only: restart authority for
        // member material, bridge bindings, and lifecycle status comes from
        // the event-projected roster seeded into MobMachine below.
        if !destroy_storage_finalizing {
            Self::ensure_supervisor_authority(
                storage.runtime_metadata.clone(),
                definition.id.clone(),
            )
            .await?;
        }
        let supervisor_authority = match storage
            .runtime_metadata
            .load_supervisor_authority(&definition.id)
            .await?
        {
            Some(record) if record.protocol_version.is_supported() => record,
            Some(record) => {
                return Err(MobError::WiringError(format!(
                    "unsupported supervisor bridge protocol version {} (supported {:?}; default {})",
                    record.protocol_version,
                    super::bridge_protocol::supervisor_bridge_supported_protocol_versions(),
                    super::bridge_protocol::supervisor_bridge_default_protocol_version()
                )));
            }
            None if destroy_storage_finalizing => {
                crate::store::SupervisorAuthorityRecord::generate(
                    super::bridge_protocol::supervisor_bridge_default_protocol_version(),
                )
            }
            None => {
                return Err(MobError::Internal(format!(
                    "cannot resume mob '{}': missing supervisor runtime metadata",
                    definition.id
                )));
            }
        };
        let supervisor_bridge = Arc::new(MobSupervisorBridge::new(
            &definition.id,
            supervisor_authority,
        )?);
        let mut roster = Roster::project(&mob_events);
        #[cfg(not(target_arch = "wasm32"))]
        Self::normalize_sessionless_backend_runtime_modes(&mut roster);
        let seeded_restore_diagnostics = HashMap::new();
        let task_board = TaskBoard::project(&mob_events);
        // Prepare shared runtime components early so resume reconciliation can
        // wire tool dispatchers for recreated sessions to the final actor channel.
        let roster_state = Arc::new(RwLock::new(RosterAuthority::new()));
        let task_board_state = Arc::new(RwLock::new(TaskBoard::default()));
        let (command_tx, command_rx) = mpsc::channel(64);
        let restore_diagnostics = Arc::new(RwLock::new(seeded_restore_diagnostics));
        let initial_dsl_authority = Box::new(seed_mob_authority(dsl_seed_state));
        let (machine_state_watch_tx, machine_state_watch_rx) =
            tokio::sync::watch::channel(initial_dsl_authority.state.clone());
        // Preview phase watch so the preview handle can answer status()
        // before the actor spawns. The real actor-side sender replaces
        // this once start_runtime_with_components owns the final pair.
        let (_preview_phase_tx, preview_phase_rx) = tokio::sync::watch::channel(resumed_state);
        let mut wiring = RuntimeWiring {
            roster: roster_state.clone(),
            task_board: task_board_state.clone(),
            public_phase: resumed_state,
            destroy_admitted,
            // Placeholder; the final authority is seeded below after
            // `reconcile_resume` finalizes the shell roster. The DSL
            // membership state is populated from the finalized roster so
            // MobMachine guards see the resumed members immediately.
            dsl_authority: initial_dsl_authority,
            machine_state_watch_tx,
            restore_diagnostics: restore_diagnostics.clone(),
            runtime_metadata: storage.runtime_metadata.clone(),
            supervisor_bridge: supervisor_bridge.clone(),
            command_tx: command_tx.clone(),
            command_rx,
        };
        let preview_handle = MobHandle {
            command_tx: command_tx.clone(),
            roster: roster_state.clone(),
            definition: definition.clone(),
            events: storage.events.clone(),
            run_store: storage.runs.clone(),
            flow_streams: Arc::new(tokio::sync::Mutex::new(BTreeMap::new())),
            session_service: session_service.clone(),
            runtime_adapter: runtime_adapter.clone(),
            restore_diagnostics,
            machine_state_watch_rx,
            phase_watch_rx: preview_phase_rx,
            realtime_session_factory: realtime_session_factory.clone(),
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
        seed_mob_authority_sync_from_flow_runs(
            &mut wiring.dsl_authority,
            storage.runs.clone(),
            storage.events.clone(),
            &definition.id,
        )
        .await?;
        let restore_diagnostics_snapshot = preview_handle.restore_diagnostics.read().await.clone();
        seed_mob_authority_restore_failures(
            &mut wiring.dsl_authority,
            &restore_diagnostics_snapshot,
        );
        let _ = wiring
            .machine_state_watch_tx
            .send(wiring.dsl_authority.state.clone());
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
        let default_protocol_version =
            super::bridge_protocol::supervisor_bridge_default_protocol_version();
        match runtime_metadata.load_supervisor_authority(&mob_id).await? {
            None => {
                let record =
                    crate::store::SupervisorAuthorityRecord::generate(default_protocol_version);
                runtime_metadata
                    .put_supervisor_authority_if_absent(&mob_id, &record)
                    .await?;
            }
            Some(record) if record.protocol_version.is_supported() => {}
            Some(record) => {
                return Err(MobError::WiringError(format!(
                    "unsupported supervisor bridge protocol version {} (supported {:?}; default {})",
                    record.protocol_version,
                    super::bridge_protocol::supervisor_bridge_supported_protocol_versions(),
                    default_protocol_version
                )));
            }
        }

        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn normalize_sessionless_backend_runtime_modes(roster: &mut Roster) {
        let identities = roster
            .list_all()
            .map(|entry| entry.agent_identity.clone())
            .collect::<Vec<_>>();
        for identity in identities {
            if let Some(entry) = roster.get_by_identity_mut(&identity) {
                entry.runtime_mode = Self::normalize_runtime_mode_for_member_ref(
                    entry.runtime_mode,
                    Some(&entry.member_ref),
                );
            }
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
                && let Err(error) = session_service
                    .archive_with_mob_lifecycle_authority(session_id)
                    .await
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
                                None,
                            )?,
                            context: None,
                            labels: Some(entry.labels.clone()),
                            additional_instructions: None,
                            shell_env: None,
                            mob_tool_authority_context: None,
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
                    None,
                )?,
                context: None,
                labels: None,
                additional_instructions: None,
                shell_env: None,
                mob_tool_authority_context: None,
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
                let _ = roster.set_comms_identity(&entry.agent_identity, None, None);
                continue;
            }
            let local_comms = provisioner.comms_runtime(&entry.member_ref).await;
            if let Some(comms_a) = &local_comms {
                let peer_id_a = comms_a.peer_id().ok_or_else(|| {
                    MobError::WiringError(format!(
                        "resume requires peer id for wired member '{}'",
                        entry.agent_identity
                    ))
                })?;
                let key_a = comms_a.public_key().ok_or_else(|| {
                    MobError::WiringError(format!(
                        "resume requires public key for wired member '{}'",
                        entry.agent_identity
                    ))
                })?;
                let _ = roster.set_comms_identity(
                    &entry.agent_identity,
                    Some(peer_id_a),
                    Some(key_a.clone()),
                );
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

            // D31 resume wire-restoration: install trust for every desired
            // edge and prune stale trust that matches one of "our own"
            // candidate peers but is no longer in the wiring projection. The
            // two-sided diff ensures (a) resumed members re-enter comms trust
            // sets after restart / manual `force_remove_trust` test scaffolds,
            // and (b) leftover trust entries from retired members or
            // orphaned external peers are revoked so `peers()` matches the
            // canonical roster projection.
            let Some(comms_a) = local_comms else {
                // Peer-only external members have no local comms runtime on
                // the supervisor side; their trust lives on the remote
                // process, so resume reconciles it through the supervisor
                // bridge instead of mutating local state.
                provisioner
                    .reconcile_peer_only_trust(&entry.member_ref, &desired_specs)
                    .await?;
                continue;
            };
            let desired_peer_ids: std::collections::HashSet<String> = desired_specs
                .iter()
                .map(|spec| spec.peer_id.to_string())
                .collect();
            // `candidate_peer_ids` is retained as an explicit signal of the
            // "our members" set — it currently guides diagnostics, and the
            // d-external-peer-wire agent's work will extend the diff below
            // to restrict pruning to the mob's own namespace once external
            // peer wiring rejoins the canonical seam.
            let _ = &candidate_specs;
            let current_peers = comms_a.peers().await;
            for current in &current_peers {
                let current_peer_id = current.peer_id.to_string();
                if desired_peer_ids.contains(&current_peer_id) {
                    continue;
                }
                // Prune any trust entry that no longer corresponds to a
                // desired edge. Resume aligns live comms trust with the
                // canonical roster projection, so trust left behind from
                // retired members or test scaffolding (see
                // `test_resume_prunes_stale_trust_not_present_in_roster`)
                // must be revoked here.
                if let Err(error) = comms_a.remove_trusted_peer(&current_peer_id).await {
                    tracing::warn!(
                        agent_identity = %entry.agent_identity,
                        peer = %current.name,
                        %error,
                        "resume: failed to prune stale trust"
                    );
                }
            }
            for spec in &desired_specs {
                let spec_peer_id = spec.peer_id.to_string();
                if current_peers
                    .iter()
                    .any(|entry| entry.peer_id.to_string() == spec_peer_id)
                {
                    continue;
                }
                if let Err(error) = comms_a.add_trusted_peer(spec.clone()).await {
                    tracing::warn!(
                        agent_identity = %entry.agent_identity,
                        peer = %spec.name,
                        %error,
                        "resume: failed to re-establish trust"
                    );
                }
            }
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
                                event_tx: None,
                                runtime: meerkat_core::service::StartTurnRuntimeSemantics::default(
                                ),
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
        let (machine_state_watch_tx, _machine_state_watch_rx) =
            tokio::sync::watch::channel(dsl_authority.state.clone());
        let roster = Arc::new(RwLock::new(RosterAuthority::from_roster(initial_roster)));
        let task_board = Arc::new(RwLock::new(initial_task_board));
        let (command_tx, command_rx) = mpsc::channel(64);
        let restore_diagnostics = Arc::new(RwLock::new(HashMap::new()));
        let wiring = RuntimeWiring {
            roster,
            task_board,
            public_phase: initial_state,
            destroy_admitted: initial_state == MobState::Destroyed,
            dsl_authority,
            machine_state_watch_tx,
            restore_diagnostics,
            runtime_metadata,
            supervisor_bridge,
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
            public_phase: wiring_public_phase,
            destroy_admitted,
            dsl_authority,
            machine_state_watch_tx,
            restore_diagnostics,
            runtime_metadata,
            supervisor_bridge,
            command_tx,
            command_rx,
        } = wiring;
        let external_backend = definition.backend.external.clone();
        let handle_session_service = session_service.clone();
        // Terminal-phase watch: seed with the initial phase so a status()
        // call before any DSL transition returns the right answer.
        let (phase_watch_tx_actor, phase_watch_rx) =
            tokio::sync::watch::channel(wiring_public_phase);
        let pending_supervisor_rotation_fallback = Arc::new(tokio::sync::RwLock::new(None));
        let handle = MobHandle {
            command_tx: command_tx.clone(),
            roster: roster.clone(),
            definition: definition.clone(),
            events: events.clone(),
            run_store: run_store.clone(),
            flow_streams: Arc::new(tokio::sync::Mutex::new(BTreeMap::new())),
            session_service: handle_session_service.clone(),
            runtime_adapter: runtime_adapter.clone(),
            restore_diagnostics: restore_diagnostics.clone(),
            machine_state_watch_rx: machine_state_watch_tx.subscribe(),
            phase_watch_rx,
            realtime_session_factory,
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
                pending_supervisor_rotation_fallback.clone(),
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
        // Normalize public phase: fresh creation + stale Creating restores both
        // become Running. Persisted Stopped/Completed/Destroyed are preserved.
        let public_phase = match wiring_public_phase {
            MobState::Creating => MobState::Running,
            phase => phase,
        };
        // Plain mobs (orchestrator: None in the definition) skip orchestrator
        // guards entirely. The coordinator-bound fact is MobMachine state
        // (see `coordinator_bound` in the mob DSL), initialized to `true`
        // by the machine's init block; no shell-side bind is needed.
        let has_orchestrator = definition.orchestrator.is_some();
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

        // Wave-c C-6c — flip the composition binding from `Standalone`
        // to `Wired(_)` whenever a runtime adapter is present, wiring
        // the mob producer into the `MeerkatConsumerSurface` on
        // `MeerkatMachine`. Builds without `runtime-adapter` keep the
        // standalone path (no consumer exists by construction).
        #[cfg(feature = "runtime-adapter")]
        let composition_binding = match &runtime_adapter {
            Some(adapter) => {
                let binding = super::composition::wired_binding_from_runtime_adapter(adapter);
                super::composition::attach_signal_dispatcher_to_runtime_adapter(
                    adapter,
                    command_tx.clone(),
                );
                binding
            }
            None => meerkat_runtime::composition::CompositionBinding::Standalone,
        };
        #[cfg(not(feature = "runtime-adapter"))]
        let composition_binding = meerkat_runtime::composition::CompositionBinding::Standalone;

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
            pending_supervisor_rotation_fallback,
            task_board_service,
            spawn_policy,
            dsl_authority: *dsl_authority,
            machine_state_watch_tx,
            phase_watch_tx: phase_watch_tx_actor,
            default_external_tools_provider,
            realm_profile_store,
            composition_binding,
            pending_routed_effects: Vec::new(),
            destroy_admitted: destroy_admitted || public_phase == MobState::Destroyed,
            destroy_cleanup_active: false,
        };
        tokio::spawn(actor.run(command_rx));

        handle
    }
}

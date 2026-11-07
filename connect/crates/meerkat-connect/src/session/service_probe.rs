//! Same-session, post-close private service continuation. No staged token is
//! manufactured and no old run context is reused for the fresh probe input.
use super::*;
use crate::{
    service_probe::{
        self as contract, CommittedProbeResult, ProbeAccess, ProbeResourceTarget, ProbeStatus,
        RequiredServiceProbe, ResourceAssessment, ServiceHost, ServiceProbeReport,
        ServiceProbeSelection,
    },
    terminal_connector::{FinalizedAction, ProbeReviewedSelection},
};
use async_trait::async_trait;
use meerkat::governed_realm::{
    open_stock_realm_persistence_for_host,
    stock::{
        StockPrincipals,
        issuance::{McpOperations, StockGrantTarget, stock_agent_principal},
    },
};
use meerkat_core::{
    LiveBridgeEffectKind, LiveBridgeEffectOutcome, Message, ToolDispatchAdmission,
    ToolDispatchContext, ToolError, ToolResult, types::ToolCallView,
};
use meerkat_runtime::{
    completion::{CompletionClass, CompletionOutcome},
    input_state::InputTerminalOutcome,
    terminal_status::{
        InputTerminalReceiptObservation, InteractionSelector, TerminalWitnessSource,
    },
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Mutex;
mod collection;
pub(crate) mod commission;
mod read_only;
pub(crate) use collection::CollectionExecution;
pub(crate) use commission::CommissionObservation;

/// Created only by the actual successful private setup opening. It retains no
/// old service, router, grant, actor, registration, runtime or dispatch context.
pub(crate) struct Captured {
    storage: ResolvedStorage,
    factory: AgentFactory,
    model: CreateSessionModelResolutionRequest,
    session: SessionId,
}
pub(crate) struct Closed(Captured);
pub(crate) struct Ready {
    // One actual native materialization at a time, shared by v6 and collections.
    execution: tokio::sync::Mutex<std::collections::BTreeSet<String>>,
    // Exact original trusted host, retained at the actual finalization handoff.
    host: Arc<ServiceHost>,
    capture: Captured,
    selected: ProbeReviewedSelection,
    // Preserve the actual administrative observation independently of probes.
    finalized: Arc<crate::terminal_connector::FinalizedInstallation>,
}
impl SessionStartup {
    pub(crate) fn capture_probe_source(
        &self,
        model: CreateSessionModelResolutionRequest,
        session: SessionId,
    ) -> Captured {
        Captured {
            storage: self.storage.clone(),
            factory: self.factory.clone(),
            model,
            session,
        }
    }
    pub(crate) async fn open_private_services_with_probe_capture(
        self,
        options: SessionOptions,
        lazy: &crate::service_preparation::LazySetup,
        session: Session,
    ) -> Result<(ConnectSession, Captured), ConnectError> {
        let capture = self.capture_probe_source(options.model.clone(), session.id().clone());
        let opened = self
            .open_private_services_setup(options, lazy, session)
            .await?;
        Ok((opened, capture))
    }
}
impl Captured {
    pub(crate) async fn close(self, closed: ClosedSetup) -> Result<Closed, ConnectError> {
        if closed.session.id() != &self.session || closed.session.realm != self.storage.locator {
            return Err(closed.retain_error(contract::unavailable()));
        }
        closed
            .finish_host_cleanup()
            .await
            .map_err(|(closed, error)| closed.retain_error(error))?;
        Ok(Closed(self))
    }
}
impl Closed {
    pub(crate) fn finalized(
        self,
        selected: ProbeReviewedSelection,
        action: &FinalizedAction,
        host: Arc<ServiceHost>,
    ) -> Result<Ready, ConnectError> {
        let finalized = action.installation().ok_or_else(contract::unavailable)?;
        if host.binding.realm != self.0.storage.locator
            || selected.installed.realm != self.0.storage.locator
            || selected.installed.session != self.0.session
            || !finalized.probe_selected(&selected)
        {
            return Err(contract::unavailable());
        }
        Ok(Ready {
            capture: self.0,
            selected,
            finalized,
            host,
            execution: tokio::sync::Mutex::new(std::collections::BTreeSet::new()),
        })
    }
}

/// Keep a failed native close owner reachable through the returned error. No
/// serialized status replaces the actual retained construction/cleanup owner.
pub(crate) struct Execution {
    pub(crate) report: ServiceProbeReport,
    pub(crate) error: Option<ConnectError>,
}
// Only this owner and its collection child can inspect the raw result. The
// ordinary v6 projection discards it and keeps its existing wire shape.
struct DetailedExecution {
    execution: Execution,
    committed: Option<ToolResult>,
    partial: Option<PartialCall>,
}
// Body-free facts already observed before joined close. They do not prove
// durable terminality, effect success or cleanup and never expose ToolResult.
struct PartialCall {
    input: meerkat_core::InputId,
    run: Option<meerkat_core::RunId>,
    call: Option<String>,
    outcome: Option<LiveBridgeEffectOutcome>,
}
impl PartialCall {
    fn observe_dispatch(&mut self, attempt: &Mutex<Option<Attempt>>) {
        // Read only the actual per-input gate. Failure leaves known progress
        // intact and supplies no new correlation or effect observation.
        let Ok(held) = attempt.lock() else {
            return;
        };
        if let Some(actual) = held.as_ref() {
            self.run = Some(actual.run.clone());
            self.call = Some(actual.call.clone());
            self.outcome = actual.outcome;
        }
    }
    fn retain(self, report: &mut ServiceProbeReport) {
        report.input_id = Some(self.input);
        if let Some(run) = self.run {
            report.run_id = Some(run);
        }
        if let Some(call) = self.call {
            report.tool_call_id = Some(call);
        }
        if let Some(outcome) = self.outcome {
            report.dispatch_outcome = Some(outcome);
        }
    }
}
impl Ready {
    pub(crate) fn job_id(&self) -> &str {
        &self.selected.installed.job_id
    }
    pub(crate) fn session_id(&self) -> &SessionId {
        &self.capture.session
    }
    pub(crate) async fn run(
        &self,
        host: Arc<ServiceProbeSelection>,
        profile: Arc<RequiredServiceProbe>,
    ) -> Execution {
        let _owner = self.execution.lock().await;
        self.run_unlocked(&host.host, &profile).await.execution
    }
    async fn run_unlocked(
        &self,
        host: &Arc<ServiceHost>,
        profile: &Arc<RequiredServiceProbe>,
    ) -> DetailedExecution {
        self.run_unlocked_with_read_only(host, profile, None).await
    }
    async fn run_unlocked_with_read_only(
        &self,
        host: &Arc<ServiceHost>,
        profile: &Arc<RequiredServiceProbe>,
        read_intent: Option<(u64, &dyn crate::service_collection::CollectionProgram)>,
    ) -> DetailedExecution {
        let mut report = ServiceProbeReport {
            profile_id: profile.id.clone(),
            application_id: host.binding.application_id.clone(),
            instance_id: host.binding.instance_id.clone(),
            login_scope_id: host.binding.login_scope_id.clone(),
            viewer_id: host.binding.viewer_id.clone(),
            origin: host.binding.origin.clone(),
            realm: self.capture.storage.locator.realm.as_str().to_owned(),
            session_id: self.capture.session.clone(),
            job_id: self.selected.installed.job_id.clone(),
            server: self.selected.config.name.clone(),
            agent: profile.agent.clone(),
            operation: profile.operation.clone(),
            resource: profile.resource.clone(),
            status: ProbeStatus::SelectionChanged,
            input_id: None,
            run_id: None,
            tool_call_id: None,
            result_sha256: None,
            installed_connection_sha256: None,
            dispatch_outcome: None,
            native_terminal_committed: false,
            native_close_joined: false,
        };
        if let Err(error) = require_original_host(&self.host, host) {
            return DetailedExecution {
                execution: Execution {
                    report,
                    error: Some(error),
                },
                committed: None,
                partial: None,
            };
        }
        let mut partial = None;
        let (committed, error) = match self
            .run_selected(host, profile, &mut report, &mut partial, read_intent)
            .await
        {
            Ok(committed) => (committed, None),
            Err(error) => (None, Some(error)),
        };
        DetailedExecution {
            execution: Execution { report, error },
            committed,
            partial,
        }
    }

    async fn run_selected(
        &self,
        host: &Arc<ServiceHost>,
        profile: &Arc<RequiredServiceProbe>,
        report: &mut ServiceProbeReport,
        partial: &mut Option<PartialCall>,
        read_intent: Option<(u64, &dyn crate::service_collection::CollectionProgram)>,
    ) -> Result<Option<ToolResult>, ConnectError> {
        if host.binding.realm != self.capture.storage.locator
            || profile.family != self.selected.installed.family
            || profile.profile != self.selected.installed.profile
            || profile.agent != self.selected.request.agent
            || !self.finalized.probe_selected(&self.selected)
            || !host.lifetime.current(host.binding.expires_at_ms)
        {
            return Err(contract::unavailable());
        }
        let StockGrantTarget::Mcp {
            server,
            operations: McpOperations::Exact(operations),
        } = &self.selected.request.target
        else {
            return Err(contract::unavailable());
        };
        if server != &self.selected.config.name
            || !operations.contains(&profile.operation)
            || self
                .selected
                .config
                .access
                .as_ref()
                .is_none_or(|ceiling| !ceiling.allowed_operations.contains(&profile.operation))
        {
            return Err(contract::unavailable());
        }
        let target = || ProbeResourceTarget {
            profile_id: &profile.id,
            resource: &profile.resource,
            raw_operation: &profile.operation,
            arguments: &profile.arguments,
            installed_connection: &self.selected.config,
        };
        if profile
            .contract
            .as_ref()
            .is_some_and(|check| !check.accepts_target(&target()))
        {
            return Err(contract::unavailable());
        }
        report.installed_connection_sha256 = Some(format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&self.selected.config).map_err(|_| contract::unavailable())?
            )
        ));
        self.selected.recheck().await?;
        let runtime = crate::realm::own_config_runtime(&self.capture.storage)?;
        let current = meerkat_core::McpRealmPersistTarget::new(&runtime)
            .servers()
            .await
            .map_err(|_| contract::unavailable())?;
        require_selected(&current.0, &self.selected.config)?;
        let principals = StockPrincipals::for_local_operator(&self.capture.storage.locator.realm)
            .map_err(|_| contract::unavailable())?;
        let access = Arc::new(ProbeAccess {
            lifetime: host.lifetime.clone(),
            expires_at_ms: host.binding.expires_at_ms,
            caller: host.binding.caller.clone(),
            owner: principals.operator,
            executor: stock_agent_principal(&self.capture.storage.locator.realm, &profile.agent)
                .map_err(|_| contract::unavailable())?,
            session: self.capture.session.clone(),
        });
        report.status = ProbeStatus::GovernedPrerequisiteUnavailable;
        let (_, persistence) = open_stock_realm_persistence_for_host(
            &meerkat::DiskStorageProvider,
            &self.capture.storage.locator.state_root,
            self.capture.storage.locator.realm.as_str(),
            None,
            Some(meerkat_store::RealmOrigin::Explicit),
            Some(self.capture.storage.layout.clone()),
            &profile.agent,
            host.binding.ingress.clone(),
            access.clone(),
        )
        .await
        .map_err(|error| {
            ConnectError::native("Open governed private service continuation", error)
        })?;
        let stock = persistence
            .stock_governance()
            .cloned()
            .ok_or_else(contract::unavailable)?;
        host.lifetime.join(stock.authorization_publication())?;
        let committed = persistence
            .session_store()
            .load(&self.capture.session)
            .await
            .map_err(|_| contract::unavailable())?
            .ok_or_else(contract::unavailable)?;
        if committed.id() != &self.capture.session {
            return Err(contract::unavailable());
        }
        let mut config = crate::realm::read_config(&self.capture.storage).await?;
        require_selected(&config.tools.mcp_servers, &self.selected.config)?;
        require_selected(
            &meerkat_core::McpRealmPersistTarget::new(&runtime)
                .servers()
                .await
                .map_err(|_| contract::unavailable())?
                .0,
            &self.selected.config,
        )?;
        self.selected.recheck().await?;
        if !host.lifetime.current(host.binding.expires_at_ms) {
            report.status = ProbeStatus::Cancelled;
            return Err(contract::unavailable());
        }
        // This fresh composition has only the exact reviewed MCP connection.
        // Native installation equality and grants still decide its attachment.
        // No setup helper, shell, peer, external dispatcher or callback is added.
        config.tools.mcp_servers = vec![self.selected.config.clone()];
        let model = meerkat::resolve_create_session_model(&config, self.capture.model.clone())
            .map_err(|_| contract::unavailable())?;
        let exposed = self
            .selected
            .config
            .tool_names
            .get(&profile.operation)
            .unwrap_or(&profile.operation)
            .clone();
        let gate = ProbeGate::new(access.clone(), profile.clone(), exposed);
        let mut build = meerkat_core::SessionBuildOptions {
            // The native request owns attachment declarations; the factory config
            // alone does not select a connection for this resumed actor.
            mcp_servers: vec![self.selected.config.clone()],
            realm_id: Some(self.capture.storage.locator.realm.clone()),
            provider: Some(model.provider),
            auth_binding: model.auth_binding,
            ..Default::default()
        };
        build.resume_existing_session(committed);
        build.tool_dispatch_admission = Some(gate.clone());
        build.tool_access_policy = Some(read_only::policy(&gate.exposed, read_intent.is_some()));
        use meerkat_core::ToolCategoryOverride::Disable;
        build.override_builtins = Disable;
        build.override_shell = Disable;
        build.override_comms = Disable;
        build.override_memory = Disable;
        build.override_schedule = Disable;
        build.override_workgraph = Disable;
        build.override_mob = Disable;
        build.override_image_generation = Disable;
        build.override_web_search = Disable;
        let mut startup = SessionStartup {
            storage: self.capture.storage.clone(),
            provider_store_root: None,
            config,
            factory: self.capture.factory.clone(),
            persistence: Some(persistence),
            connector_auth: None,
            standalone: None,
        };
        let source = Arc::new(meerkat::RealmConnectorClients::from_realm_source(
            crate::realm::config_source(&self.capture.storage)?,
            self.capture.storage.locator.realm.clone(),
        ));
        startup.configure_realm_connectors(source).await?;
        let mut factory = startup.factory.clone();
        if let Some((generation, program)) = read_intent {
            let auth = startup
                .connector_auth
                .as_ref()
                .ok_or_else(contract::unavailable)?;
            let declaration = crate::service_install::read_only::selected(
                &self.selected.installed,
                &self.selected.config,
                &profile.operation,
                self.selected.selected_inputs(),
                program,
            )
            .await?;
            self.selected.recheck().await?;
            let provider = read_only::Provider::new(
                auth.context_provider.clone(),
                declaration,
                self.selected.config.clone(),
                self.capture.storage.clone(),
                host.clone(),
                gate.clone(),
                generation,
            );
            provider
                .recheck()
                .await
                .map_err(|_| contract::unavailable())?;
            factory = factory.mcp_call_context_provider(Arc::new(provider));
        } else if let Some(auth) = &startup.connector_auth {
            factory = factory.mcp_call_context_provider(auth.context_provider.clone());
        }
        let mut builder = FactoryAgentBuilder::new(factory, startup.config.clone());
        builder.wait_for_mcp = true;
        let session = build
            .resume_session
            .as_ref()
            .cloned()
            .ok_or_else(contract::unavailable)?;
        let request = CreateSessionRequest {
            model: model.model,
            prompt: String::new().into(),
            injected_context: Vec::new(),
            system_prompt: SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(build),
            labels: None,
        };
        let opened = ConnectSession::materialize_governed(
            &self.capture.storage,
            builder,
            startup
                .persistence
                .take()
                .ok_or_else(contract::unavailable)?,
            session,
            request,
            stock,
            None,
            startup.connector_auth.take(),
            None,
            std::slice::from_ref(&self.selected.config.name),
        )
        .await?;
        // From here every fallible observation is inside the retained close
        // owner region. Cancellation never drops an entered materialization.
        let machine = opened.machine.clone();
        report.status = ProbeStatus::NotObserved;
        let observed = probe_turn_retaining(&opened, &gate, host, partial).await;
        let close = opened.close().await;
        // Even a failed turn/close may have reached real dispatch admission.
        // Preserve every available gate fact, never a result or terminal claim.
        if let Some(progress) = partial.as_mut() {
            progress.observe_dispatch(&gate.attempt);
        }
        if let Err(cleanup) = close {
            report.status = ProbeStatus::CleanupUnconfirmed;
            return Err(match observed {
                Err((_, primary)) => ConnectError::Cleanup {
                    primary: Box::new(primary),
                    cleanup: Box::new(cleanup),
                },
                Ok(_) => cleanup,
            });
        }
        report.native_close_joined = true;
        let observation = match observed {
            Ok(observation) => observation,
            Err((status, error)) => {
                report.status = status;
                return Err(error);
            }
        };
        report.input_id = Some(observation.input.clone());
        report.run_id = Some(observation.run.clone());
        report.tool_call_id = Some(observation.call.clone());
        report.dispatch_outcome = Some(observation.dispatch_outcome);
        let terminal = machine
            .input_terminal_receipt_observation(
                &self.capture.session,
                InteractionSelector::InputId(observation.input.clone()),
            )
            .await
            .map_err(|_| contract::unavailable())?
            .ok_or_else(contract::unavailable)?;
        if terminal.source != TerminalWitnessSource::DurableStore {
            report.status = ProbeStatus::OutcomeUnknown;
            return Err(contract::unavailable());
        }
        let InputTerminalReceiptObservation::Finalized(receipt) = terminal.report else {
            report.status = ProbeStatus::OutcomeUnknown;
            return Err(contract::unavailable());
        };
        if receipt.input_id() != &observation.input
            || receipt.owner_input_id() != &observation.input
            || receipt.recipient_input_ids() != [observation.input.clone()]
            || receipt.run_id() != Some(&observation.run)
            || receipt.terminal() != &InputTerminalOutcome::Consumed
            || !matches!(
                receipt.class(),
                CompletionClass::Completed | CompletionClass::CompletedWithoutResult
            )
        {
            report.status = ProbeStatus::OutcomeUnknown;
            return Err(contract::unavailable());
        }
        report.native_terminal_committed = true;
        let bytes = serde_json::to_vec(&observation.result).map_err(|_| contract::unavailable())?;
        report.result_sha256 = Some(format!("{:x}", Sha256::digest(bytes)));
        if let Some(status) = dispatch_refusal(observation.dispatch_outcome, &observation.result) {
            report.status = status;
            return Ok(None);
        }
        if !host.lifetime.current(host.binding.expires_at_ms) {
            report.status = ProbeStatus::Cancelled;
            return Ok(None);
        }
        report.status = match &profile.contract {
            None => ProbeStatus::ToolCompleted,
            Some(check) if !check.accepts_target(&target()) => ProbeStatus::SelectionChanged,
            Some(check) => match check.assess(&CommittedProbeResult {
                target: target(),
                result: &observation.result,
            }) {
                ResourceAssessment::Verified => ProbeStatus::CapabilityVerified,
                ResourceAssessment::NotVerified => ProbeStatus::CapabilityNotVerified,
                ResourceAssessment::Unknown => ProbeStatus::ToolCompleted,
            },
        };
        Ok(match &report.status {
            ProbeStatus::ToolCompleted
            | ProbeStatus::CapabilityVerified
            | ProbeStatus::CapabilityNotVerified => Some(observation.result),
            _ => None,
        })
    }
}
fn require_original_host(
    original: &Arc<ServiceHost>,
    selected: &Arc<ServiceHost>,
) -> Result<(), ConnectError> {
    if !Arc::ptr_eq(original, selected) {
        return Err(contract::unavailable());
    }
    Ok(())
}
fn require_selected(
    servers: &[meerkat_core::McpServerConfig],
    selected: &meerkat_core::McpServerConfig,
) -> Result<(), ConnectError> {
    let mut named = servers.iter().filter(|server| server.name == selected.name);
    if named.next() != Some(selected) || named.next().is_some() {
        return Err(contract::unavailable());
    }
    Ok(())
}

struct Attempt {
    call: String,
    run: meerkat_core::RunId,
    outcome: Option<LiveBridgeEffectOutcome>,
}
pub(super) struct ProbeGate {
    access: Arc<ProbeAccess>,
    profile: Arc<RequiredServiceProbe>,
    exposed: String,
    attempt: Mutex<Option<Attempt>>,
}
impl ProbeGate {
    pub(super) fn new(
        access: Arc<ProbeAccess>,
        profile: Arc<RequiredServiceProbe>,
        exposed: String,
    ) -> Arc<Self> {
        Arc::new(Self {
            access,
            profile,
            exposed,
            attempt: Mutex::new(None),
        })
    }
}
#[async_trait]
impl ToolDispatchAdmission for ProbeGate {
    async fn await_dispatch_admission(
        &self,
        call: ToolCallView<'_>,
        context: Option<&ToolDispatchContext>,
        _: LiveBridgeEffectKind,
    ) -> Result<(), ToolError> {
        let refused = || {
            ToolError::execution_failed(
                "This private probe admits only its exact selected operation once",
            )
        };
        let context = context.ok_or_else(refused)?;
        let run = context.run_id().cloned().ok_or_else(refused)?;
        if context.origin_session_id() != Some(&self.access.session)
            || context.round().is_none()
            || context.work_authorization().is_none()
            || call.name != self.exposed
            || call.id.is_empty()
            || call.args.get().len() > 32 * 1024
            || !self.access.lifetime.current(self.access.expires_at_ms)
            || serde_json::from_str::<crate::forge::UniqueJson>(call.args.get()).is_err()
            || serde_json::from_str::<Value>(call.args.get()).ok().as_ref()
                != Some(&self.profile.arguments)
        {
            return Err(refused());
        }
        let mut attempt = self.attempt.lock().map_err(|_| refused())?;
        if attempt.is_some() {
            return Err(refused());
        }
        *attempt = Some(Attempt {
            call: call.id.to_owned(),
            run,
            outcome: None,
        });
        Ok(())
    }
    async fn record_dispatch_outcome(
        &self,
        call: ToolCallView<'_>,
        context: Option<&ToolDispatchContext>,
        _: LiveBridgeEffectKind,
        outcome: LiveBridgeEffectOutcome,
    ) -> Result<(), ToolError> {
        let mut attempt = self
            .attempt
            .lock()
            .map_err(|_| ToolError::execution_failed("Probe observation unavailable"))?;
        let actual = attempt
            .as_mut()
            .ok_or_else(|| ToolError::execution_failed("Probe observation absent"))?;
        if actual.call != call.id
            || context.and_then(ToolDispatchContext::run_id) != Some(&actual.run)
            || actual.outcome.is_some()
        {
            return Err(ToolError::execution_failed("Probe observation changed"));
        }
        actual.outcome = Some(outcome);
        Ok(())
    }
}
pub(super) struct Observation {
    pub(super) input: meerkat_core::InputId,
    pub(super) run: meerkat_core::RunId,
    pub(super) call: String,
    pub(super) result: ToolResult,
    pub(super) dispatch_outcome: LiveBridgeEffectOutcome,
}
#[cfg(test)]
pub(super) async fn probe_turn(
    session: &ConnectSession,
    gate: &ProbeGate,
    host: &ServiceHost,
) -> Result<Observation, (ProbeStatus, ConnectError)> {
    probe_turn_retaining(session, gate, host, &mut None).await
}
fn probe_prompt(
    exposed: &str,
    arguments: &Value,
    catalog: &meerkat_core::ToolCatalogView,
) -> Result<PromptInput, ConnectError> {
    use meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata;
    use meerkat_core::service::{TURN_DEFERRED_TOOL_AUTHORITIES_CONTEXT_KEY, TurnToolOverlay};
    use meerkat_core::{
        DeferredToolLoadAuthority, ToolCatalogDeferredEligibility, ToolCatalogVisibility,
        ToolPlaneClass, ToolVisibilityWitness,
    };
    if !catalog.exact_catalog || !catalog.pending_sources.is_empty() {
        return Err(contract::unavailable());
    }
    let mut selected = catalog
        .entries
        .iter()
        .filter(|entry| entry.name.as_str() == exposed);
    let entry = selected.next().ok_or_else(contract::unavailable)?;
    if selected.next().is_some()
        || entry.plane != ToolPlaneClass::Session
        || !entry.callability.is_callable()
        || !matches!(
            entry.visibility,
            ToolCatalogVisibility::Visible | ToolCatalogVisibility::Deferred
        )
    {
        return Err(contract::unavailable());
    }
    let ToolCatalogDeferredEligibility::DeferredEligible { provenance } =
        &entry.deferred_eligibility
    else {
        return Err(contract::unavailable());
    };
    // Load only this current catalog identity for this turn. Core revalidates
    // the witness against its base catalog before the model can call the tool.
    // Visibility grants no dispatch authority; ProbeGate and policy stay exact.
    let authority = DeferredToolLoadAuthority::new(
        entry.name.clone(),
        ToolVisibilityWitness {
            last_seen_provenance: Some(provenance.clone()),
        },
    );
    let overlay = TurnToolOverlay {
        allowed_tools: Some(vec![entry.name.clone()]),
        dispatch_context: std::collections::BTreeMap::from([(
            TURN_DEFERRED_TOOL_AUTHORITIES_CONTEXT_KEY.to_owned(),
            serde_json::to_value(vec![authority]).map_err(|_| contract::unavailable())?,
        )]),
        ..Default::default()
    };
    let prompt=json!({"instruction":"Perform only this exact required-resource check once, then stop. Do not substitute a resource, call another tool, retry, or infer capability from prose.",
        "tool":exposed,"arguments":arguments}).to_string();
    Ok(PromptInput::new(
        prompt,
        Some(RuntimeTurnMetadata {
            turn_tool_overlay: Some(overlay),
            ..Default::default()
        }),
    ))
}
async fn probe_turn_retaining(
    session: &ConnectSession,
    gate: &ProbeGate,
    host: &ServiceHost,
    partial: &mut Option<PartialCall>,
) -> Result<Observation, (ProbeStatus, ConnectError)> {
    let fail = |status| (status, contract::unavailable());
    if !host.lifetime.current(host.binding.expires_at_ms) {
        return Err(fail(ProbeStatus::Cancelled));
    }
    let catalog = session
        .service
        .tool_catalog_view(session.id())
        .await
        .map_err(|error| {
            (
                ProbeStatus::NativeRefused,
                ConnectError::native("Read private probe catalog", error),
            )
        })?
        .ok_or_else(|| fail(ProbeStatus::NativeRefused))?;
    let input = probe_prompt(&gate.exposed, &gate.profile.arguments, &catalog)
        .map_err(|error| (ProbeStatus::NativeRefused, error))?;
    let (admission, completion) = session
        .submit_input_for_caller(Input::Prompt(input), Some(&host.binding.caller))
        .await
        .map_err(|error| (ProbeStatus::NativeRefused, error))?;
    let AcceptOutcome::Accepted { input_id, .. } = &admission else {
        // Even an unexpected admission shape does not discard its actual owner.
        let cancel = session.cancel(&admission).await;
        let joined = completion.try_wait_with_terminal_outcome().await;
        let cleanup = joined_probe_stop(
            cancel.map(|_| ()),
            joined
                .map(|_| ())
                .map_err(|error| ConnectError::native("Join unexpected probe admission", error)),
        );
        return Err((
            ProbeStatus::OutcomeUnknown,
            cleanup.err().unwrap_or_else(contract::unavailable),
        ));
    };
    // This identity is the actual accepted native input, even if cancellation
    // precedes a tool dispatch. No run, call, result or terminality is inferred.
    *partial = Some(PartialCall {
        input: input_id.clone(),
        run: None,
        call: None,
        outcome: None,
    });
    let wait = completion.try_wait_with_terminal_outcome();
    tokio::pin!(wait);
    let outcome = tokio::select! {biased;
        _=host.lifetime.closed(host.binding.expires_at_ms)=>{
            let cancel=session.cancel(&admission).await;
            let joined=wait.await;
            return Err(match joined_probe_stop(cancel.map(|_|()),joined.map(|_|())
                .map_err(|error|ConnectError::native("Join cancelled probe input",error))) {
                Ok(())=>fail(ProbeStatus::Cancelled),
                Err(error)=>(ProbeStatus::OutcomeUnknown,error),
            });
        },
        result=wait.as_mut()=>result.map_err(|error|(ProbeStatus::OutcomeUnknown,ConnectError::native("Join probe input",error)))?,
    };
    if outcome.input_terminal_outcome() != Some(&InputTerminalOutcome::Consumed)
        || !matches!(
            outcome.outcome(),
            CompletionOutcome::Completed(_) | CompletionOutcome::CompletedWithoutResult
        )
    {
        return Err(fail(ProbeStatus::OutcomeUnknown));
    }
    let (call, run, dispatch_outcome) = {
        let held = gate
            .attempt
            .lock()
            .map_err(|_| fail(ProbeStatus::OutcomeUnknown))?;
        let attempt = held
            .as_ref()
            .ok_or_else(|| fail(ProbeStatus::NotObserved))?;
        let outcome = attempt
            .outcome
            .ok_or_else(|| fail(ProbeStatus::NotObserved))?;
        (attempt.call.clone(), attempt.run.clone(), outcome)
    };
    use meerkat_core::service::{SessionHistoryQuery, SessionServiceHistoryExt};
    let history = session
        .service
        .read_history(
            session.id(),
            SessionHistoryQuery {
                offset: 0,
                limit: Some(4097),
            },
        )
        .await
        .map_err(|_| fail(ProbeStatus::OutcomeUnknown))?;
    if history.session_id != *session.id()
        || history.has_more
        || history.messages.len() > 4096
        || serde_json::to_vec(&history.messages)
            .map_err(|_| fail(ProbeStatus::OutcomeUnknown))?
            .len()
            > 8 * 1024 * 1024
    {
        return Err(fail(ProbeStatus::OutcomeUnknown));
    }
    let result = correlated_result(
        &history.messages,
        &run,
        &call,
        &gate.exposed,
        &gate.profile.arguments,
    )
    .ok_or_else(|| fail(ProbeStatus::NotObserved))?;
    Ok(Observation {
        input: input_id.clone(),
        run,
        call,
        result,
        dispatch_outcome,
    })
}
// Both retained operations have been awaited. A successful cancellation does
// not erase a failing terminal join, nor does either error replace the other.
fn joined_probe_stop(
    cancel: Result<(), ConnectError>,
    joined: Result<(), ConnectError>,
) -> Result<(), ConnectError> {
    match (cancel, joined) {
        (Err(primary), Err(cleanup)) => Err(ConnectError::Cleanup {
            primary: Box::new(primary),
            cleanup: Box::new(cleanup),
        }),
        (Err(error), _) | (_, Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}
// A committed input is not a known physical tool outcome. In particular,
// Unknown remains unknown even if a durable ToolResult happens to look successful.
fn dispatch_refusal(outcome: LiveBridgeEffectOutcome, result: &ToolResult) -> Option<ProbeStatus> {
    match outcome {
        LiveBridgeEffectOutcome::Unknown => Some(ProbeStatus::OutcomeUnknown),
        LiveBridgeEffectOutcome::Failed => Some(ProbeStatus::ToolFailed),
        LiveBridgeEffectOutcome::Committed
            if result.is_error || !result.settlement_failures.is_empty() =>
        {
            Some(ProbeStatus::ToolFailed)
        }
        LiveBridgeEffectOutcome::Committed => None,
    }
}
fn correlated_result(
    messages: &[Message],
    run: &meerkat_core::RunId,
    call: &str,
    name: &str,
    args: &Value,
) -> Option<ToolResult> {
    let mut calls = 0usize;
    let mut result = None;
    for message in messages {
        match message {
            Message::BlockAssistant(assistant) => {
                for actual in assistant.tool_calls().filter(|c| c.id == call) {
                    if assistant.identity.run_id.as_ref() != Some(run)
                        || actual.name != name
                        || serde_json::from_str::<crate::forge::UniqueJson>(actual.args.get())
                            .is_err()
                        || serde_json::from_str::<Value>(actual.args.get())
                            .ok()
                            .as_ref()
                            != Some(args)
                    {
                        return None;
                    }
                    calls += 1;
                }
            }
            Message::ToolResults { results, .. } => {
                for actual in results.iter().filter(|r| r.tool_use_id == call) {
                    if calls != 1 || result.is_some() {
                        return None;
                    }
                    result = Some(actual.clone());
                }
            }
            _ => (),
        }
    }
    (calls == 1).then_some(result).flatten()
}

#[cfg(test)]
#[path = "service_probe_tests.rs"]
mod tests;

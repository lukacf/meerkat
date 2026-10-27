//! First real governed input using the installed policy, source, destination
//! hook and native grant owner. Only the physical model transport is scripted.
use super::*;
use meerkat_toolkit::host_lifetime::NativeHostLifetime;

pub(crate) struct FirstFixture<S = EphemeralSessionService<FactoryAgentBuilder>> {
    pub(crate) store: Arc<toolkit_store::SqliteStore>,
    pub(crate) source_path: std::path::PathBuf,
    pub(crate) scope: toolkit_store::StoreScope,
    pub(crate) service: Arc<S>,
    pub(crate) probe: Option<Arc<crate::installed_host::acquisition::ContextProbe>>,
    _directory: Arc<tempfile::TempDir>,
    _lifetime: Arc<NativeHostLifetime>,
    pub(crate) source: Arc<PrivateOriginals>,
    pub(crate) machine: Arc<MeerkatMachine>,
    native: UnifiedRuntime,
    runtime: PublicIdentityRuntime,
    identity: AgentIdentity,
    pub(crate) session: SessionId,
    provider: Arc<ScriptedProvider>,
    submission: PrivateOriginalSubmit,
}

impl FirstFixture {
    async fn new(select_controller: bool) -> Self {
        Self::new_with_probe(select_controller, false).await
    }
    pub(crate) async fn for_continuation() -> Self {
        // Keep the large fixture-construction future out of the continuation
        // test's outer frame. Native ownership and awaited cleanup stay here.
        Box::pin(Self::new_with_probe(true, true)).await
    }
    pub(crate) async fn for_cold_continuation(explicit: bool, targeted: bool) -> Self {
        let fixture = if explicit || targeted { Self::for_service_input().await } else {
            Box::pin(Self::new_selected_configuration(true, true, true, None, None, true)).await
        };
        let prepared = std::panic::AssertUnwindSafe(async {
        if targeted {
            use crate::installed_host::{acquisition::ConfiguredHelperReportCapability, operator_setup::AgentSetupCapabilityRequest};
            use meerkat_authorization_contracts::constraints::{DelegationDepth, ProcessorRef};
            use meerkat_toolkit::host_lifetime::policy::CompiledAgentSetupTarget;
            let native = fixture.lifetime().selected_policy().native_host.as_ref().unwrap();
            let owner = fixture.source.local_operator_setup(fixture.lifetime().clone()).unwrap();
            let mut selected = owner.configure_agent_setup(&AgentSetupCapabilityRequest {
                helper: None,
                member: native.members[0].member.clone(), route: native.routes[0].id.clone(),
                issuance_id: EvidenceId::new("cold-a-find-parent").unwrap(), helper_targets: vec![],
            }).unwrap();
            // Match the existing canonical-selection fixture: the test's
            // compiler selects an actual native report parent, not a fake grant.
            let restrictions = ExecutionRestrictions {
                actions: ExactRestriction::exact([ActionRef { feature: "toolkit".into(), action: "setup-report".into() }]),
                processors: ExactRestriction::exact([ProcessorRef::Principal { principal: native.routes[0].logical_executor.clone() }]),
                audiences: ExactRestriction::exact([native.members[0].audience.clone()]),
                delegation_depth: DelegationDepth::remaining(1), ..ExecutionRestrictions::unrestricted()
            };
            let parent = fixture.source.owner.grants().issue_root(&selected.parent.root_authority,
                EvidenceId::new("cold-a-report-parent").unwrap(), native.grant_administrator.clone(),
                native.routes[0].represented_subject.clone(), restrictions.clone()).unwrap();
            selected.helper_targets.push(CompiledAgentSetupTarget {
                target: Some(ConfiguredHelperReportCapability::target()), parent, restrictions,
            });
            fixture.source.retain_configured_setup(&fixture.submission.original, &fixture.session, selected).await.unwrap();
        }
        if explicit {
            use crate::installed_host::operator_setup::OperatorParentOutcome;
            use meerkat_core::{ApprovalDecision, ApprovalPrincipalId, ApprovalService};
            let prepared = fixture.source.prepare_local_controller_setup(fixture.submission.clone()).await.unwrap();
            let request = crate::private_originals::setup_request::tests::request(&prepared);
            let owner = fixture.source.local_operator_setup(fixture.lifetime().clone()).unwrap();
            let result = owner.review_original_setup_request(prepared, request, |request| async {
                let approvals = ApprovalService::new();
                let pending = approvals.request(request).unwrap();
                Ok(approvals.decide(&pending.approval_id, ApprovalDecision::Approve,
                    ApprovalPrincipalId::new("cold-continuation-fixture").unwrap(), None, None).unwrap())
            }).await.unwrap();
            assert!(matches!(result, OperatorParentOutcome::SetupRequestSelected { .. }));
        }
        }).catch_unwind().await;
        if let Err(panic) = prepared {
            let cleanup = fixture.shutdown().await;
            assert!(cleanup.is_ok(), "{cleanup:?}");
            std::panic::resume_unwind(panic);
        }
        fixture
    }
    pub(crate) async fn for_service_input() -> Self {
        Box::pin(Self::new_selected(true, true, true)).await
    }
    pub(crate) async fn for_service_input_with_source(
        prepare_source: crate::installed_host::controller_first_input_fixture::PrepareOriginalSource,
    ) -> Self {
        Box::pin(Self::new_selected_source(true, true, true, None, None, false,
            Some(prepare_source), None, false)).await
    }
    pub(crate) async fn for_console_input(human: PrincipalRef) -> Self {
        Box::pin(Self::new_selected_source(true, true, true, None, None, false, None, Some(human), false)).await
    }
    pub(crate) async fn for_console_operator_input(human: PrincipalRef) -> Self {
        Box::pin(Self::new_selected_source(true, true, true, None, None, true, None, Some(human), false)).await
    }
    pub(crate) async fn for_task_parent_service_input() -> Self {
        Box::pin(Self::new_selected_source(true, true, true, None, None, false, None, None, true)).await
    }
    pub(crate) async fn for_source_status(mode: DeliveryMode) -> Self {
        Box::pin(Self::new_selected_fault(true, true, true, Some(mode))).await
    }
    async fn new_with_probe(select_controller: bool, probe: bool) -> Self {
        Self::new_selected(select_controller, probe, false).await
    }
    async fn new_selected(select_controller: bool, probe: bool, local_operator: bool) -> Self {
        Self::new_selected_fault(select_controller, probe, local_operator, None).await
    }
    async fn new_selected_fault(select_controller: bool, probe: bool, local_operator: bool, status_fault: Option<DeliveryMode>) -> Self {
        Self::new_selected_acquisition(select_controller, probe, local_operator, status_fault, None).await
    }
    async fn new_selected_acquisition(select_controller: bool, probe: bool, local_operator: bool,
        status_fault: Option<DeliveryMode>, acquisition: Option<Arc<acquisition_denial::Script>>) -> Self
    {
        Self::new_selected_configuration(select_controller, probe, local_operator, status_fault, acquisition, false).await
    }
    async fn new_selected_configuration(select_controller: bool, probe: bool, local_operator: bool,
        status_fault: Option<DeliveryMode>, acquisition: Option<Arc<acquisition_denial::Script>>, configured_setup: bool) -> Self
    {
        Self::new_selected_configuration_with_source(select_controller, probe, local_operator,
            status_fault, acquisition, configured_setup, None).await
    }
    async fn new_selected_configuration_with_source(select_controller: bool, probe: bool, local_operator: bool,
        status_fault: Option<DeliveryMode>, acquisition: Option<Arc<acquisition_denial::Script>>, configured_setup: bool,
        prepare_source: Option<crate::installed_host::controller_first_input_fixture::PrepareOriginalSource>) -> Self
    {
        Self::new_selected_source(select_controller, probe, local_operator, status_fault, acquisition,
            configured_setup, prepare_source, None, false).await
    }
    async fn new_selected_source(select_controller: bool, probe: bool, local_operator: bool,
        status_fault: Option<DeliveryMode>, acquisition: Option<Arc<acquisition_denial::Script>>, configured_setup: bool,
        prepare_source: Option<crate::installed_host::controller_first_input_fixture::PrepareOriginalSource>,
        console_human: Option<PrincipalRef>, task_parent_administration: bool) -> Self
    {
        assert!(prepare_source.is_none() || local_operator);
        assert!(!(console_human.is_some() && task_parent_administration));
        let mut opened = None;
        let mut owners = None;
        let built = std::panic::AssertUnwindSafe(tokio::time::timeout(
            WAIT, Box::pin(Self::build(select_controller, probe, local_operator, status_fault, acquisition, configured_setup, prepare_source,
                console_human, task_parent_administration, &mut opened, &mut owners)),
        )).catch_unwind().await;
        match built {
            Ok(Ok(fixture)) => fixture,
            failed => {
                let cleanup = match opened {
                    Some(runtime) => shutdown_fixture(&runtime).await,
                    None => Ok(()),
                };
                // Preserve the actual directory and lifetime through teardown.
                drop(owners);
                assert!(cleanup.is_ok(), "{cleanup:?}");
                match failed {
                    Err(payload) => std::panic::resume_unwind(payload),
                    _ => panic!("first-input fixture construction timed out"),
                }
            }
        }
    }

    async fn build(
        select_controller: bool,
        probe: bool,
        local_operator: bool,
        status_fault: Option<DeliveryMode>,
        acquisition: Option<Arc<acquisition_denial::Script>>,
        configured_setup: bool,
        prepare_source: Option<crate::installed_host::controller_first_input_fixture::PrepareOriginalSource>,
        console_human: Option<PrincipalRef>,
        task_parent_administration: bool,
        opened: &mut Option<UnifiedRuntime>,
        owners: &mut Option<(Arc<tempfile::TempDir>, Arc<NativeHostLifetime>)>,
    ) -> Self {
        let facts = meerkat_core::ControllerModelFacts::new(selection(), ENDPOINT.into(), MODEL.into());
        let console_owned_identity = console_human.is_some();
        let host = if let Some(human) = console_human {
            assert!(select_controller && probe && local_operator && status_fault.is_none()
                && acquisition.is_none() && prepare_source.is_none());
            if configured_setup {
                crate::installed_host::controller_first_input_fixture::open_for_console_operator_input(&facts, human).await
            } else { crate::installed_host::controller_first_input_fixture::open_for_console_input(&facts, human).await }
        } else if task_parent_administration {
            assert!(select_controller && probe && local_operator && status_fault.is_none()
                && acquisition.is_none() && !configured_setup && prepare_source.is_none());
            crate::installed_host::controller_first_input_fixture::open_for_task_parent_service_input(&facts).await
        } else if configured_setup {
            assert!(select_controller && local_operator && status_fault.is_none() && acquisition.is_none());
            match prepare_source {
                Some(prepare_source) => crate::installed_host::controller_first_input_fixture::open_with_configured_setup_source(&facts, prepare_source).await,
                None if probe => crate::installed_host::controller_first_input_fixture::open_with_configured_setup_probe(&facts).await,
                None => crate::installed_host::controller_first_input_fixture::open_with_configured_setup(&facts).await,
            }
        } else if let Some(script) = &acquisition {
            assert!(select_controller && !probe && !local_operator && status_fault.is_none());
            crate::installed_host::controller_first_input_fixture::open_with_acquisition(&facts, script.dispatcher()).await
        } else if status_fault.is_some() {
            crate::installed_host::controller_first_input_fixture::open_for_source_status(&facts).await
        } else if local_operator {
            match prepare_source {
                Some(prepare_source) => crate::installed_host::controller_first_input_fixture::open_for_service_input_with_source(
                    &facts, prepare_source).await,
                None => crate::installed_host::controller_first_input_fixture::open_for_service_input(&facts).await,
            }
        } else {
            crate::installed_host::controller_first_input_fixture::open_with_probe(&facts, select_controller, probe).await
        };
        *owners = Some((host.directory.clone(), host.lifetime.clone()));
        if let Some(script) = &acquisition {
            script.retain_owner(crate::installed_host::controller_first_input_fixture::bind_acquisition(&host, script.tools()));
        }
        let machine = Arc::new(MeerkatMachine::ephemeral()
            .with_local_grant_authorization(host.configuration).unwrap());
        let provider = Arc::new(ScriptedProvider::default());
        let mut builder = FactoryAgentBuilder::new(AgentFactory::minimal().comms(true), Config::default());
        let probe = probe.then(|| Arc::new(crate::installed_host::acquisition::ContextProbe::default()));
        let client: Arc<dyn LlmClient> = if let Some(script) = &acquisition {
            script.provider(provider.clone())
        } else { match &probe {
            Some(probe) => Arc::new(crate::installed_host::acquisition::ProbeProvider::with_probe(provider.clone(), probe)),
            None => provider.clone(),
        }};
        builder.default_llm_client = Some(client);
        *builder.default_agent_llm_client_decorator.write().unwrap() =
            Some(fixture_controller_decorator());
        builder.default_session_store = Some(Arc::new(SessionStore::default()));
        let service = Arc::new(EphemeralSessionService::new(builder, 4));
        let mut definition = definition();
        definition.id = MobId::from("factory");
        let ProfileBinding::Inline(profile) = definition.profiles.get_mut(&ProfileName::from("worker")).unwrap()
            else { panic!("inline fixture") };
        profile.tools = ToolConfig { comms: true, ..Default::default() };
        if acquisition.is_some() { profile.tools.rust_bundles.push(crate::FIND_AND_INSTALL_BUNDLE.into()); }
        let namespace = definition.id.to_string();
        let mut spec = MobBootstrapSpec::new(definition, MobStorage::in_memory(), service.clone())
            .with_session_runtime_adapter(machine.clone()).unwrap()
            .with_member_runtime_registration_hook(host.registration)
            .with_tool_consequence_policy_registry(host.tool_policy_registry)
            .with_declared_ephemeral_mob_storage();
        if let Some(script) = &acquisition {
            spec = spec.register_tool_bundle(crate::FIND_AND_INSTALL_BUNDLE, script.dispatcher());
        }
        if let Some(probe) = &probe {
            let probe = probe.clone();
            spec = spec.with_default_external_tools_provider(Arc::new(move || Some(probe.clone() as Arc<dyn meerkat_core::AgentToolDispatcher>)));
        }
        *opened = Some(bootstrap_fixture(spec, namespace).await);
        let native = opened.as_mut().unwrap();
        let handle = native.mob_handle();
        let (session, runtime, member_spec) = if console_owned_identity {
            // The real Console aggregator validates durable identity against
            // the live roster. Let its native owner realize both; a raw spawn
            // followed by an invented ContinuityRecord is not that authority.
            let bridge = MobSessionBridge::with_session_service(
                handle.clone(), native.mob_runtime().session_service().cloned().unwrap(),
            ).with_controller_pin_source(service.clone());
            let runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
                continuity_store: Arc::new(LocalContinuityStore::in_memory().unwrap()),
                lease_provider: Arc::new(LocalLeaseProvider::new()),
                runtime_instance_id: "toolkit-first-input".into(), has_runtime_store: true,
                durability_policy: DurabilityPolicy::SyncWriteThrough,
                bridge: Some(Arc::new(bridge)), default_timeout: None,
            }).with_runtime_services(
                meerkat_mobkit::identity_first::AgentRuntimeServices::new(handle.clone()),
            ));
            let identity = AgentIdentity::parse(MEMBER).unwrap();
            let member_spec = DurableAgentSpec {
                identity: identity.clone(), profile: ProfileName::from("worker"),
                addressability: AgentAddressability::Addressable, display_name: None,
                labels: BTreeMap::new(), context: None, additional_instructions: vec![],
                initial_message: None, runtime_mode_override: Some(MobRuntimeMode::TurnDriven),
                backend: None, binding: None, placement: None,
            };
            meerkat_mobkit::identity_first::orchestrator::restore_flow(
                &runtime, std::slice::from_ref(&member_spec), None, None,
            ).await.unwrap();
            let status = runtime.status(&identity).await.unwrap();
            let session = status.session_id.expect("native Console session realized");
            assert!(status.agent_runtime_id.is_some(), "native Console runtime identity realized");
            let member = handle.get_member(&MemberIdentity::from(MEMBER)).await.unwrap().unwrap();
            assert_eq!(member.bridge_session_id(), Some(&session));
            let actor = service.live_session_actor_witness(&session).await.unwrap();
            let pin = service.pin_controller_client_for_actor(&actor).await.unwrap_or_else(|error| {
                if let meerkat_core::service::SessionError::Agent(
                    AgentError::OperationRefused { refusal }
                ) = &error {
                    panic!("actual actor controller pin refused: {:?}", refusal.kind());
                }
                panic!("actual actor controller pin failed: {error:?}");
            });
            assert_eq!(pin.selection(), &selection());
            // Native startup may briefly hold controller custody. The existing
            // bounded owner helper retries only typed Busy and rechecks references;
            // no input is submitted until this actual publication succeeds.
            meerkat_core::publish_token_lifecycle_acquired_for_identity_within(
                &machine.generated_auth_lease_handle(), pin.selection().credential(),
                &meerkat_core::auth::PersistedTokens::api_key("synthetic-controller-only-fixture"),
                tokio::time::Instant::now() + WAIT,
            ).await.expect("selected fixture credential publishes within its setup deadline");
            (session, runtime, member_spec)
        } else {
            let member = MemberIdentity::from(MEMBER);
            handle.spawn_spec(SpawnMemberSpec::new("worker", member.clone())).await.unwrap();
            let session = handle.get_member(&member).await.unwrap().unwrap()
                .bridge_session_id().cloned().unwrap();
            let actor = service.live_session_actor_witness(&session).await.unwrap();
            let pin = service.pin_controller_client_for_actor(&actor).await.unwrap_or_else(|error| {
                if let meerkat_core::service::SessionError::Agent(
                    AgentError::OperationRefused { refusal }
                ) = &error {
                    panic!("actual actor controller pin refused: {:?}", refusal.kind());
                }
                panic!("actual actor controller pin failed: {error:?}");
            });
            assert_eq!(pin.selection(), &selection());
            // Native startup may briefly hold controller custody. The existing
            // bounded owner helper retries only typed Busy and rechecks references;
            // no input is submitted until this actual publication succeeds.
            meerkat_core::publish_token_lifecycle_acquired_for_identity_within(
                &machine.generated_auth_lease_handle(), pin.selection().credential(),
                &meerkat_core::auth::PersistedTokens::api_key("synthetic-controller-only-fixture"),
                tokio::time::Instant::now() + WAIT,
            ).await.expect("selected fixture credential publishes within its setup deadline");
            let lease_provider = Arc::new(LocalLeaseProvider::new());
            let runtime_instance = "toolkit-first-input";
            let bridge = MobSessionBridge::new(handle.observer()).with_controller_pin_source(service.clone());
            let bridge: Arc<dyn meerkat_mobkit::identity_first::SessionBridge> = match status_fault {
                Some(mode) => Arc::new(observed::ObservedBridge {
                    inner: bridge, fault: Arc::new(DeliveryFault::new(mode)),
                    publication: host.originals.owner.publication(),
                }),
                None => Arc::new(bridge),
            };
            let runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
                continuity_store: Arc::new(LocalContinuityStore::in_memory().unwrap()),
                lease_provider: lease_provider.clone(),
                runtime_instance_id: runtime_instance.into(), has_runtime_store: true,
                durability_policy: DurabilityPolicy::SyncWriteThrough,
                bridge: Some(bridge),
                default_timeout: None,
            }).with_runtime_services(
                meerkat_mobkit::identity_first::AgentRuntimeServices::new(handle.clone()),
            ));
            let identity = AgentIdentity::parse(MEMBER).unwrap();
            let member_spec = DurableAgentSpec {
                identity: identity.clone(), profile: ProfileName::from("worker"),
                addressability: AgentAddressability::Addressable, display_name: None,
                labels: BTreeMap::new(), context: None, additional_instructions: vec![],
                initial_message: None, runtime_mode_override: Some(MobRuntimeMode::TurnDriven),
                backend: None, binding: None, placement: None,
            };
            // Core already owns this spawned member and session; registration adds its roster spec.
            runtime.register(member_spec.clone()).await;
            let status = runtime.status(&identity).await.unwrap();
            assert_eq!(status.state, IdentityLifecycleState::Active);
            assert_eq!(status.session_id, Some(session.clone()));
            assert!(status.agent_runtime_id.is_some(), "spawned Core member has a runtime identity");
            assert_eq!(runtime.core_session_owner(&session).await.unwrap(), Some(identity.clone()));
            (session, runtime, member_spec)
        };
        let identity = member_spec.identity.clone();
        native.attach_identity_first_context(Arc::new(IdentityFirstRuntimeContext::new(
            runtime,
            Arc::new(MutableRosterProvider::new(vec![member_spec])),
            None, None, Some(handle.definition().clone()),
        )));
        let runtime = native.identity_runtime().unwrap();
        let source = host.originals;
        source.bind_machine(machine.clone()).unwrap();
        source.bind_identity_runtime(&runtime).unwrap();
        let mut event = private_event("first-conversation");
        event.route = Id::new("console").unwrap();
        event.content_sha256 = digest(PROMPT.as_bytes());
        let authentication = event.original_authentication.clone();
        let original = source.retain_event(event).await.unwrap();
        Self {
            store: host.store, source_path: host.source_path, scope: host.source_scope, service, probe,
            _directory: host.directory, _lifetime: host.lifetime,
            source, machine, native: opened.take().unwrap(), runtime, identity, session, provider,
            submission: PrivateOriginalSubmit { original, content: PROMPT.into(), transport_authentication: authentication },
        }
    }
}

// These observations do not depend on which actual native service owns the session.
impl<S> FirstFixture<S> {
    pub(crate) async fn binding(&self) -> Result<Option<InputAuthorityAssociation>, String> {
        tokio::time::timeout(WAIT, self.source.binding_evidence(&self.submission.original)).await
            .map_err(|_| "binding read timed out".to_owned())?
            .map_err(|error| error.to_string())
    }
    pub(crate) async fn submit(&self) -> Result<PrivateOriginalSubmission, String> {
        tokio::time::timeout(WAIT, self.source.submit_authenticated_original(self.submission.clone())).await
            .map_err(|_| "first input timed out".to_owned())?
            .map_err(|error| error.to_string())
    }
    pub(crate) async fn inputs(&self) -> Result<usize, String> {
        tokio::time::timeout(WAIT, self.machine.meerkat_machine_spine_snapshot(&self.session)).await
            .map_err(|_| "input observation timed out".to_owned())?
            .map(|snapshot| snapshot.ledger.input_count)
            .ok_or_else(|| "native input snapshot missing".into())
    }
    pub(crate) async fn finish(&self, result: &Result<PrivateOriginalSubmission, String>) -> Result<(), String> {
        let Ok(PrivateOriginalSubmission::Admitted { turn_ticket: Some(ticket), .. }) = result
            else { return Err(format!("first input has no native ticket: {}", submission_context(result))); };
        let ticket = TurnTicket::parse(ticket).map_err(|error| format!("{error:?}"))?;
        let outcome = tokio::time::timeout(WAIT, self.runtime.wait_for_turn(&self.identity, ticket, WAIT)).await
            .map_err(|_| "native first turn timed out".to_owned())?;
        if matches!(outcome, TurnOutcome::Completed { output: TurnOutput::Text { text, .. } } if text == ANSWER) {
            Ok(())
        } else { Err("native ticket did not complete the original answer".into()) }
    }
    pub(crate) fn hold_next_response(&self) -> super::PreparedResponseHold {
        self.provider.hold_next_response()
    }
    pub(crate) fn model_requests(&self) -> usize {
        self.provider.0.load(Ordering::SeqCst)
    }
    pub(crate) fn lifetime(&self) -> &Arc<NativeHostLifetime> { &self._lifetime }
    // Compose the test's real Console router on this same native owner.
    pub(crate) fn native_for_console_mut(&mut self) -> &mut UnifiedRuntime { &mut self.native }
    pub(crate) fn original_reference(&self) -> &PrivateOriginalRef { &self.submission.original }
    pub(crate) async fn cold_source_record(&self) -> toolkit_store::StoredRecord {
        self.store.get(&self.source.key(&self.submission.original).unwrap()).await.unwrap().unwrap()
    }
    pub(crate) fn cold_source_custody(&self, association: &InputAuthorityAssociation) -> Option<(bool, bool)> {
        self.source.owner.historical_task_custody_for_test(association)
    }
    pub(crate) fn forget_source_custody(&self, association: &InputAuthorityAssociation) {
        self.source.retained.lock().unwrap().retain(|held| held.reference != self.submission.original);
        self.source.owner.forget_task_custody_for_test(association);
        assert!(self.cold_source_custody(association).is_none());
    }
    pub(crate) fn release_source_custody(&self) -> Result<(), String> {
        let original = self.source.retained.lock().unwrap().iter()
            .find(|held| held.reference == self.submission.original)
            .map(|held| held.original.clone()).ok_or("source is not retained")?;
        self.source.owner.release_original(&original).map_err(|error| error.to_string())
    }

    pub(crate) async fn shutdown(&self) -> Result<(), String> {
        shutdown_fixture(&self.native).await
    }

    fn preissue_controller(&self, wrong_executor: bool) -> Result<GrantLineageRef, String> {
        // Model a native append that completed before the source final row.
        // This is real durable issuance, not a synthesized lookup result.
        let route = self.source.route(&self.submission.original.route).map_err(|error| error.to_string())?;
        let parent = route.controller_parent.as_ref().ok_or("fixture controller parent missing")?;
        let grants = self.source.owner.grants();
        let resolved = grants.resolve_controller_lineage(
            parent, &route.grant_caller, route.represented_subject.as_ref(),
        ).map_err(|error| format!("{error:?}"))?;
        let bounds = resolved.restrictions().for_child(&ExecutionRestrictions {
            delegation_depth: meerkat_authorization_contracts::constraints::DelegationDepth::remaining(0),
            ..ExecutionRestrictions::unrestricted()
        }).map_err(|error| format!("{error:?}"))?;
        grants.issue_child(
            &route.grant_caller, parent.last().ok_or("fixture controller parent empty")?,
            EvidenceId::new(format!("original-controller-{}", self.source.key(&self.submission.original)
                .map_err(|error| error.to_string())?.id)).map_err(|error| format!("{error:?}"))?,
            if wrong_executor { principal("unrelated-agent") } else { route.logical_executor.clone() },
            bounds,
        ).map_err(|error| format!("{error:?}"))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unbound_first_conversation_uses_only_controller_authority_and_replays_exactly() {
    let fixture = FirstFixture::new(true).await;
    let before = fixture.binding().await;
    let before_inputs = fixture.inputs().await;
    let first = fixture.submit().await;
    let completion = fixture.finish(&first).await;
    let original = fixture.binding().await;
    let replay = fixture.submit().await;
    let replay_completion = fixture.finish(&replay).await;
    let after = fixture.binding().await;
    let inputs = fixture.inputs().await;
    let requests = fixture.provider.0.load(Ordering::SeqCst);
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    assert!(matches!(before, Ok(None)));
    assert_eq!(before_inputs, Ok(0));
    assert!(completion.is_ok(), "{completion:?}");
    assert!(replay_completion.is_ok(), "{replay_completion:?}");
    let Ok(PrivateOriginalSubmission::Admitted { input_id: first_id, deduplicated: false,
        source_settlement: PrivateSourceSettlement::FreshAcceptancePromoted, .. }) = first
        else { panic!("first real admission and source promotion required") };
    let Ok(PrivateOriginalSubmission::Admitted { input_id: replay_id, deduplicated: true,
        source_settlement: PrivateSourceSettlement::RetryWithdrawn, .. }) = replay
        else { panic!("native canonical replay required") };
    assert_eq!(first_id, replay_id);
    let Ok(Some(original)) = original else { panic!("durable first binding required") };
    assert!(matches!(&after, Ok(Some(saved)) if saved == &original));
    assert!(crate::original_ingress::controller_only_basis(&original.candidate().authority_basis));
    assert_eq!(original.candidate().authority_basis.grant_lineages().count(), 0);
    assert_eq!(original.candidate().controller_grant_lineage.len(), 2);
    assert_eq!(inputs, Ok(1));
    assert_eq!(requests, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unbound_first_conversation_without_selected_controller_stays_unbound() {
    let fixture = FirstFixture::new(false).await;
    let result = fixture.submit().await;
    let binding = fixture.binding().await;
    let inputs = fixture.inputs().await;
    let requests = fixture.provider.0.load(Ordering::SeqCst);
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    assert!(matches!(result, Ok(PrivateOriginalSubmission::NativeOutcomeUnavailable)), "{}", submission_context(&result));
    assert!(matches!(binding, Ok(None)));
    assert_eq!(inputs, Ok(0));
    assert_eq!(requests, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_binding_recovers_only_the_exact_usable_native_controller_child() {
    let fixture = FirstFixture::new(true).await;
    let issued = fixture.preissue_controller(false);
    let before = fixture.binding().await;
    let result = if issued.is_ok() { fixture.submit().await } else { Err("fixture issuance failed".into()) };
    let completion = fixture.finish(&result).await;
    let binding = fixture.binding().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    let Ok(issued) = issued else { panic!("fixture native issuance failed") };
    assert!(matches!(before, Ok(None)));
    assert!(completion.is_ok(), "{completion:?}");
    let Ok(Some(binding)) = binding else { panic!("canonical binding required") };
    assert!(binding.candidate().controller_grant_lineage.last() == Some(&issued));
    assert!(crate::original_ingress::controller_only_basis(&binding.candidate().authority_basis));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retained_native_id_for_another_executor_is_not_recovered_or_reissued() {
    let fixture = FirstFixture::new(true).await;
    let issued = fixture.preissue_controller(true);
    let first = if issued.is_ok() { fixture.submit().await } else { Err("fixture issuance failed".into()) };
    let second = if issued.is_ok() { fixture.submit().await } else { Err("fixture issuance failed".into()) };
    let binding = fixture.binding().await;
    let inputs = fixture.inputs().await;
    let still_issued = match &issued {
        Ok(issued) => fixture.source.owner.grants().issued_reference(&issued.grant_id).ok().flatten(),
        Err(_) => None,
    };
    let requests = fixture.provider.0.load(Ordering::SeqCst);
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    let Ok(issued) = issued else { panic!("fixture native issuance failed") };
    assert!(matches!(first, Ok(PrivateOriginalSubmission::NativeOutcomeUnavailable)), "{}", submission_context(&first));
    assert!(matches!(second, Ok(PrivateOriginalSubmission::NativeOutcomeUnavailable)), "{}", submission_context(&second));
    assert!(matches!(binding, Ok(None)));
    assert_eq!(inputs, Ok(0));
    assert!(still_issued.as_ref() == Some(&issued));
    assert_eq!(requests, 0);
}

#[path = "helper_entry_native_tests.rs"]
mod helper_entry_native;

#[path = "persistent_first_input_fixture.rs"]
mod persistent;

#[path = "controller_only_acquisition_tests.rs"]
mod acquisition_denial;

#[path = "approved_acquisition_fixture.rs"]
mod approved_fixture;
#[path = "approved_acquisition_tests.rs"]
mod acquisition_approved;

#[path = "helper_report_native_tests.rs"]
mod helper_report;

#[path = "configured_setup_native_tests.rs"]
mod configured_setup_native;

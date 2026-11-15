//! The installed canonical policy and actual native held-add share one owner.
//! Only model transport is scripted; callback dispatch and ordinary input run
//! through the real factory, registration, native actor and grant machine.
use super::*;
use crate::host_gateway::{MemberClassAddError, MemberClassAddStart, ToolkitGatewayHost};
use crate::installed_host::{
    controller_first_input_fixture as installed,
    dynamic_members::{AdmittedMemberClass, ApplicationMemberPolicyRequest},
    member_mcp_consumer::{
        MemberMcpAdmissionSelection, MemberMcpApplication, RetainedMemberMcpBindings,
    },
    preparation::{PreparedControllerIntent, PreparedControllerTarget},
    tool_policy::{ApplicationToolPolicyRecoveryDecision, PreparedInstalledToolPolicy},
};
use meerkat_core::{
    CompiledMemberToolAction, CompiledMemberToolGrant, CompiledToolConsequence, MobMemberBinding,
};
use meerkat_mobkit::identity_first::{CallbackBridge, GatewayAgentCustomizer, RestoreOutcome};
use meerkat_mobkit::rpc_gateway::{GatewayHostCloseReason, GatewayHostComposition};
use meerkat_toolkit::host_lifetime::{
    control::HostLifetimeError,
    policy::{
        CompiledClassCallback, CompiledMemberClass, CompiledMemberClassInstance,
        CompiledNativeSourceOrigin, CompiledNativeSourceRoute, PreparedNativeMember,
    },
};
use std::sync::atomic::AtomicBool;

const CALLBACK: &str = "project_read";
const OTHER_CALLBACK: &str = "project_status";
const ADDED: &str = "project-one";
// These cases reuse the installed fixture's pre-issued factory realm. Its
// process-global native supervisor route must be released before the next
// class fixture opens. Other modules using that realm are a separate concern.
static CLASS_FIXTURE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn class() -> CompiledMemberClass {
    CompiledMemberClass {
        purpose: Default::default(),
        id: Id::new("project-owner").unwrap(),
        profile: "project-owner".into(),
        factory: Id::new("host:project-owner").unwrap(),
        callback_account: Some(meerkat_toolkit::host_lifetime::policy::CompiledClassCallbackAccount::DeclaredNone),
        callbacks: [CALLBACK, OTHER_CALLBACK].into_iter().map(|name| CompiledClassCallback {
            name: name.into(),
            description: "Read this exact selected project".into(),
            input_schema: serde_json::json!({"type":"object","properties":{},"additionalProperties":false}),
        }).collect(),
        grants: [CALLBACK, OTHER_CALLBACK].into_iter().map(|name| CompiledMemberToolGrant {
            tool_name: name.into(),
            action: CompiledMemberToolAction::Invoke,
            consequence: CompiledToolConsequence::R0,
        }).collect(),
    }
}

fn instance(name: &str) -> CompiledMemberClassInstance {
    let owner = PrincipalRef::in_domain(
        meerkat_core::PrincipalKind::SharedAgent,
        name,
        meerkat_core::TrustDomainId::new("factory-test").unwrap(),
    )
    .unwrap();
    let operator = meerkat::governed_realm::stock::StockPrincipals::for_local_operator(
        &meerkat_core::mob_realm_id("factory").unwrap(),
    )
    .unwrap()
    .operator;
    CompiledMemberClassInstance {
        class: class().id,
        member: PreparedNativeMember {
            member: MobMemberBinding {
                mob_id: "factory".into(),
                member: meerkat_mobkit::member_comms_id::roster_member_id_for_identity(name)
                    .to_string(),
                role: class().profile,
            },
            logical_owner: owner.clone(),
            context_generation: 1,
            audience: AudienceRef::Principal {
                principal: operator,
            },
            setup_capability: None,
        },
    }
}

struct Rule {
    selected: CompiledMemberClass,
    current: Arc<AtomicBool>,
}
struct Reservation {
    owner: PrincipalRef,
    current: Arc<AtomicBool>,
    provenance: serde_json::Map<String, serde_json::Value>,
}
impl ApplicationServicePolicyReservation for Reservation {
    fn owner(&self) -> &PrincipalRef {
        &self.owner
    }
    fn provenance(&self) -> &serde_json::Map<String, serde_json::Value> {
        &self.provenance
    }
    fn check_current(&self) -> Result<(), HostLifetimeError> {
        if self.current.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(HostLifetimeError::Denied)
        }
    }
}
impl ApplicationServicePolicy for Rule {
    fn reserve(
        &self,
        _: &ApplicationServicePolicyRequest<'_>,
    ) -> Result<ApplicationServicePolicyDecision, HostLifetimeError> {
        Ok(ApplicationServicePolicyDecision::RequiresHuman)
    }
    fn reserve_member_policy(
        &self,
        request: &ApplicationMemberPolicyRequest<'_>,
    ) -> Result<ApplicationToolPolicyRecoveryDecision, HostLifetimeError> {
        if request.owner
            != &meerkat_core::PrincipalRef::in_domain(
                meerkat_core::PrincipalKind::ServiceAccount,
                "application",
                meerkat_core::TrustDomainId::new("factory-test").unwrap(),
            )
            .unwrap()
            || request.deployment != "deployment"
            || request.realm != &meerkat_core::mob_realm_id("factory").unwrap()
            || request.admission.class != self.selected
            || request.admission.instance
                != instance(&request.admission.instance.member.member.member)
            || ![ADDED, "project-aborted", "project-refused"]
                .contains(&request.admission.instance.member.member.member.as_str())
            || !self.current.load(Ordering::SeqCst)
        {
            return Err(HostLifetimeError::Denied);
        }
        Ok(ApplicationToolPolicyRecoveryDecision::Permit(Box::new(
            Reservation {
                owner: request.owner.clone(),
                current: self.current.clone(),
                provenance: [("fixture".into(), "exact-native-class".into())]
                    .into_iter()
                    .collect(),
            },
        )))
    }
}

// The real gateway customizer creates its dispatcher only after this trusted
// callback returns. No in-process dispatcher exists at class reservation time.
struct Factory {
    calls: Mutex<Vec<String>>,
    next_call: Mutex<Option<&'static str>>,
    next_args: Mutex<serde_json::Value>,
    scopes: Mutex<Vec<String>>,
    preparations: AtomicUsize,
    request_tools: Mutex<Vec<Vec<String>>>,
    callback_results: Mutex<Vec<(bool, String)>>,
    model_errors: Mutex<Vec<String>>,
    capture_model_check: AtomicBool,
    captured_model_check: Mutex<Option<meerkat_core::authorization::PreparedOperationCheck>>,
    recorder_ticket: Arc<peer_ingress_tests::RecorderTicketCapture>,
}
struct GatewayBridge(Arc<Factory>);
#[async_trait]
impl CallbackBridge for GatewayBridge {
    async fn call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        match method {
            "callback/agent_customizer/customize_build" => {
                let mut draft = params["draft"].clone();
                if params["spec"]["profile"] == class().profile {
                    self.0.preparations.fetch_add(1, Ordering::SeqCst);
                    self.0
                        .scopes
                        .lock()
                        .unwrap()
                        .push(params["scope_id"].as_str().unwrap().into());
                    draft["external_tools"] = serde_json::to_value(class().callbacks).unwrap();
                }
                Ok(draft)
            }
            "callback/agent_customizer/after_create" => Ok(serde_json::Value::Null),
            "callback/call_tool" => {
                let scope = params["scope_id"]
                    .as_str()
                    .ok_or("missing callback scope")?;
                if !self
                    .0
                    .scopes
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|selected| selected == scope)
                    || !class()
                        .callbacks
                        .iter()
                        .any(|callback| params["tool"] == callback.name)
                {
                    return Err("unregistered callback scope or tool".into());
                }
                self.0
                    .calls
                    .lock()
                    .unwrap()
                    .push(params["tool"].as_str().unwrap().into());
                Ok(serde_json::json!({"content":"selected-project-only"}))
            }
            _ => Err("unexpected gateway callback".into()),
        }
    }
}

struct CallbackProvider {
    inner: Arc<ScriptedProvider>,
    factory: Arc<Factory>,
}
#[async_trait]
impl LlmClient for CallbackProvider {
    async fn prepare_controller_credential(&self) -> Result<(), meerkat_core::auth::AuthError> {
        self.inner.prepare_controller_credential().await
    }
    fn plain_model_route(
        &self,
        model: &str,
    ) -> Result<PlainModelRoute, meerkat_core::ControllerFactsUnavailable> {
        self.inner.plain_model_route(model)
    }
    fn controller_model_selection(&self) -> Option<ControllerModelSelection> {
        self.inner.controller_model_selection()
    }
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        self.inner.project_replay_messages(messages)
    }
    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        self.inner.stream(request)
    }
    fn stream_prepared<'a>(&'a self, request: &'a PreparedLlmRequest) -> LlmStream<'a> {
        use futures::StreamExt;
        let pending = *self.factory.next_call.lock().unwrap();
        let args = self.factory.next_args.lock().unwrap().clone();
        let has_callback = pending
            .is_some_and(|name| request.request().tools.iter().any(|tool| tool.name == name));
        let mut tools = self.factory.request_tools.lock().unwrap();
        if tools.len() < 8 {
            tools.push(
                request
                    .request()
                    .tools
                    .iter()
                    .take(64)
                    .map(|tool| tool.name.to_string())
                    .collect(),
            );
        }
        drop(tools);
        let mut feedback = self.factory.callback_results.lock().unwrap();
        for message in &request.request().messages {
            if let Message::ToolResults { results, .. } = message {
                for result in results
                    .iter()
                    .filter(|result| result.tool_use_id == "project-read-call")
                {
                    if feedback.len() < 8 {
                        feedback.push((
                            result.is_error,
                            result.text_content().chars().take(512).collect(),
                        ));
                    }
                }
            }
        }
        drop(feedback);
        // Opt-in retention of the actual request's native model decision.
        // The normal transport still enters and settles this same request.
        if self
            .factory
            .capture_model_check
            .swap(false, Ordering::SeqCst)
        {
            let prepared = request.authorization().unwrap().prepare(model_facts());
            let prepared = match prepared.and_then(|check| check.current()) {
                Ok(check) => check,
                Err(error) => {
                    return Box::pin(futures::stream::iter([Err(
                        LlmError::from_operation_authorization(error),
                    )]));
                }
            };
            assert!(
                self.factory
                    .captured_model_check
                    .lock()
                    .unwrap()
                    .replace(prepared)
                    .is_none()
            );
        }
        let stream = self.inner.stream_prepared(request);
        let mut emitted_tool = false;
        Box::pin(stream.map(move |event| {
            if let Err(error) = &event {
                let mut errors = self.factory.model_errors.lock().unwrap();
                if errors.len() < 8 {
                    errors.push(format!("{error:?}").chars().take(512).collect());
                }
            }
            event.map(|event| match event {
                // A refused preparation can be retried by Core without any
                // successful model output. It must not consume this step.
                LlmEvent::TextDelta { .. }
                    if has_callback && self.factory.next_call.lock().unwrap().take().is_some() =>
                {
                    emitted_tool = true;
                    LlmEvent::ToolCallComplete {
                        id: "project-read-call".into(),
                        name: pending.unwrap().into(),
                        args: args.clone(),
                        meta: None,
                    }
                }
                LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success { .. },
                } if emitted_tool => LlmEvent::Done {
                    outcome: LlmDoneOutcome::success(StopReason::ToolUse),
                },
                event => event,
            })
        }))
    }
    fn provider(&self) -> Provider {
        self.inner.provider()
    }
    async fn health_check(&self) -> Result<(), LlmError> {
        self.inner.health_check().await
    }
}

fn spec(name: &str, profile: &str) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: AgentIdentity::parse(name).unwrap(),
        profile: profile.into(),
        addressability: AgentAddressability::Addressable,
        display_name: None,
        labels: BTreeMap::new(),
        context: None,
        additional_instructions: vec![],
        initial_message: None,
        runtime_mode_override: Some(MobRuntimeMode::TurnDriven),
        backend: None,
        binding: None,
        placement: None,
    }
}

#[derive(Default)]
struct ApplicationInputs {
    rows: Mutex<Vec<RetainedMemberMcpBindings>>,
    pause_lookup: Mutex<Option<Arc<ApplicationLookupPause>>>,
}
#[derive(Default)]
struct ApplicationLookupPause {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
#[async_trait::async_trait]
impl MemberMcpApplication for ApplicationInputs {
    async fn retain(&self, value: RetainedMemberMcpBindings) -> Result<(), HostLifetimeError> {
        value.check(&value.selection)?;
        let mut rows = self
            .rows
            .lock()
            .map_err(|_| HostLifetimeError::Unavailable)?;
        match rows
            .iter()
            .find(|row| row.selection.admission_id == value.selection.admission_id)
        {
            Some(existing) if existing != &value => Err(HostLifetimeError::Denied),
            Some(_) => Ok(()),
            None => {
                rows.push(value);
                Ok(())
            }
        }
    }
    async fn lookup(
        &self,
        selection: MemberMcpAdmissionSelection,
    ) -> Result<RetainedMemberMcpBindings, HostLifetimeError> {
        let value = self
            .rows
            .lock()
            .map_err(|_| HostLifetimeError::Unavailable)?
            .iter()
            .find(|row| row.selection == selection)
            .cloned()
            .ok_or(HostLifetimeError::Denied)?;
        let pause = self
            .pause_lookup
            .lock()
            .map_err(|_| HostLifetimeError::Unavailable)?
            .take();
        if let Some(pause) = pause {
            pause.entered.notify_one();
            pause.release.notified().await;
        }
        Ok(value)
    }
}

struct Fixture {
    _directory: Arc<tempfile::TempDir>,
    selection: meerkat_toolkit::host_lifetime::control::HostBootstrapSelection,
    roster: Arc<MutableRosterProvider>,
    runtime_store: Arc<dyn meerkat_runtime::RuntimeStore>,
    lifetime: Arc<meerkat_toolkit::host_lifetime::NativeHostLifetime>,
    native: UnifiedRuntime,
    gateway: Arc<ToolkitGatewayHost>,
    source: Arc<PrivateOriginals>,
    policy: PreparedInstalledToolPolicy,
    factory: Arc<Factory>,
    service: Arc<meerkat::PersistentSessionService<FactoryAgentBuilder>>,
    machine: Arc<MeerkatMachine>,
    provider: Arc<ScriptedProvider>,
    peer_observer: Arc<peer_ingress_tests::ObservedReceiver>,
    application_inputs: Arc<ApplicationInputs>,
    _current: Arc<AtomicBool>,
    _private_open: tokio::sync::watch::Sender<bool>,
    _exclusive: Option<tokio::sync::MutexGuard<'static, ()>>,
}

// Only immutable application inputs and store locators cross the cold close.
// Native, source, registration, grant and callback owners are constructed anew.
struct RetainedClassFixture {
    directory: Arc<tempfile::TempDir>,
    selection: meerkat_toolkit::host_lifetime::control::HostBootstrapSelection,
    definition: MobDefinition,
    roster: Vec<DurableAgentSpec>,
    application_inputs: Vec<RetainedMemberMcpBindings>,
}

impl Fixture {
    async fn start() -> Self {
        Self::start_with_policy_store(None).await
    }
    async fn start_with_policy_store(decorate: Option<installed::PrepareOriginalSource>) -> Self {
        Self::start_selected(class(), decorate).await
    }
    async fn start_selected(
        selected: CompiledMemberClass,
        decorate: Option<installed::PrepareOriginalSource>,
    ) -> Self {
        Self::start_selected_source(selected, decorate, None).await
    }
    async fn start_selected_source(
        selected: CompiledMemberClass,
        decorate: Option<installed::PrepareOriginalSource>,
        source: Option<installed::PrepareOriginalSource>,
    ) -> Self {
        Self::start_selected_source_with_recorder(selected, decorate, source, false).await
    }
    async fn start_selected_source_with_recorder(
        selected: CompiledMemberClass,
        decorate: Option<installed::PrepareOriginalSource>,
        source: Option<installed::PrepareOriginalSource>,
        recorder: bool,
    ) -> Self {
        let exclusive = CLASS_FIXTURE.lock().await;
        Self::construct(
            selected, decorate, source, recorder, false, None, None, exclusive,
        )
        .await
    }
    async fn start_durable_with_recorder(selected: CompiledMemberClass) -> Self {
        let exclusive = CLASS_FIXTURE.lock().await;
        Self::construct(selected, None, None, true, true, None, None, exclusive).await
    }
    async fn reopen_with_recorder(mut self) -> Self {
        let retained = RetainedClassFixture {
            directory: self._directory.clone(),
            selection: self.selection.clone(),
            definition: self.native.mob_handle().definition().clone(),
            roster: self.roster.snapshot(),
            application_inputs: self.application_inputs.rows.lock().unwrap().clone(),
        };
        let selected = self
            .lifetime
            .selected_policy()
            .native_host
            .as_ref()
            .unwrap()
            .member_classes[0]
            .clone();
        self.close().await;
        // Every join of this host returned. The gateway's join does not end
        // the native custody: the close's final release does, as the
        // process's close makes it after its joins. Without it the grant
        // store's owner lease ends only when the last reference to this
        // host's owner drops, and the fresh host opened below can find it
        // still held (its restore open answers unavailable). Only the reopen
        // makes it: a row that closes and then reads this host's store keeps
        // the custody its reads need.
        let released = self.gateway.release_native_custody().await;
        assert!(released.is_ok(), "{released:?}");
        let exclusive = self._exclusive.take().unwrap();
        drop(self);
        Self::construct(
            selected,
            None,
            None,
            true,
            true,
            Some(retained),
            None,
            exclusive,
        )
        .await
    }
    async fn construct(
        selected: CompiledMemberClass,
        decorate: Option<installed::PrepareOriginalSource>,
        source: Option<installed::PrepareOriginalSource>,
        recorder: bool,
        durable: bool,
        retained: Option<RetainedClassFixture>,
        topology: Option<Arc<dyn meerkat_mobkit::identity_first::TopologyProvider>>,
        exclusive: tokio::sync::MutexGuard<'static, ()>,
    ) -> Self {
        let mut opened = None;
        let mut owners = None;
        let built = std::panic::AssertUnwindSafe(tokio::time::timeout(
            WAIT * 4,
            Box::pin(Self::build(
                selected,
                decorate,
                source,
                &mut opened,
                &mut owners,
                recorder,
                durable,
                retained,
                topology,
            )),
        ))
        .catch_unwind()
        .await;
        match built {
            Ok(Ok(mut fixture)) => {
                fixture._exclusive = Some(exclusive);
                fixture
            }
            failed => {
                use meerkat_toolkit::host_lifetime::control::{
                    HostCloseReason, HostCloseState, PrivateHostLifetimeControl,
                };
                let close = owners.as_ref().map(|(_, lifetime)| {
                    lifetime.begin_close_activation(HostCloseReason::Shutdown)
                });
                let cleanup = match opened {
                    Some(native) => shutdown_fixture(&native).await,
                    None => Ok(()),
                };
                if let (Some((_, lifetime)), Some(Ok(close))) = (&owners, &close) {
                    assert_eq!(
                        lifetime.join_close(&close.handle).await.unwrap().state,
                        HostCloseState::Settled
                    );
                }
                assert!(
                    close.as_ref().is_none_or(|close| close.is_ok()),
                    "host close entry refused: {close:?}"
                );
                drop(owners);
                assert!(cleanup.is_ok(), "{cleanup:?}");
                match failed {
                    Err(panic) => std::panic::resume_unwind(panic),
                    _ => panic!("class fixture construction timed out"),
                }
            }
        }
    }
    async fn build(
        selected: CompiledMemberClass,
        decorate: Option<installed::PrepareOriginalSource>,
        source: Option<installed::PrepareOriginalSource>,
        opened: &mut Option<UnifiedRuntime>,
        owners: &mut Option<(
            Arc<tempfile::TempDir>,
            Arc<meerkat_toolkit::host_lifetime::NativeHostLifetime>,
        )>,
        recorder: bool,
        durable: bool,
        retained: Option<RetainedClassFixture>,
        topology: Option<Arc<dyn meerkat_mobkit::identity_first::TopologyProvider>>,
    ) -> Self {
        let current = Arc::new(AtomicBool::new(true));
        let participant = Arc::new(Rule {
            selected: selected.clone(),
            current: current.clone(),
        });
        let host = if let Some(retained) = &retained {
            assert!(durable && decorate.is_none() && source.is_none());
            installed::reopen_for_member_classes(
                retained.directory.clone(),
                retained.selection.clone(),
                participant,
            )
            .await
        } else {
            Box::pin(installed::open_for_member_classes(
                &meerkat_core::ControllerModelFacts::new(
                    selection(),
                    ENDPOINT.into(),
                    MODEL.into(),
                ),
                selected.clone(),
                participant,
                decorate,
                source,
            ))
            .await
        };
        *owners = Some((host.directory.clone(), host.lifetime.clone()));
        let application_inputs = Arc::new(ApplicationInputs::default());
        if let Some(retained) = &retained {
            for row in &retained.application_inputs {
                application_inputs.retain(row.clone()).await.unwrap();
            }
        }
        host.originals
            .bind_member_mcp_application(application_inputs.clone())
            .unwrap();
        let factory = Arc::new(Factory {
            calls: Mutex::new(Vec::new()),
            next_call: Mutex::new(Some(CALLBACK)),
            next_args: Mutex::new(serde_json::json!({})),
            scopes: Mutex::new(Vec::new()),
            preparations: AtomicUsize::new(0),
            request_tools: Mutex::new(Vec::new()),
            callback_results: Mutex::new(Vec::new()),
            model_errors: Mutex::new(Vec::new()),
            capture_model_check: AtomicBool::new(false),
            captured_model_check: Mutex::new(None),
            recorder_ticket: Arc::new(peer_ingress_tests::RecorderTicketCapture::new()),
        });
        let provider = Arc::new(ScriptedProvider::default());
        let mut agent_factory = AgentFactory::minimal().comms(true);
        // A profile may enable Core session memory as well as Meta Recorder.
        // Both stores stay under this fixture's retained temporary root.
        agent_factory.store_path = host.directory.path().join("factory-state");
        let mut builder = FactoryAgentBuilder::new(agent_factory, Config::default());
        builder.default_llm_client = Some(Arc::new(CallbackProvider {
            inner: provider.clone(),
            factory: factory.clone(),
        }));
        let mob_tools = builder.default_mob_tools.clone();
        *builder.default_agent_llm_client_decorator.write().unwrap() =
            Some(fixture_controller_decorator());
        let persistence = meerkat::PersistenceBundle::new_with_local_grant_authorization(
            Arc::new(
                meerkat::SqliteSessionStore::open(
                    host.directory.path().join("native-sessions.sqlite3"),
                )
                .unwrap(),
            ),
            if durable {
                Arc::new(
                    meerkat_runtime::store::SqliteRuntimeStore::new(
                        host.directory.path().join("native-sessions.sqlite3"),
                    )
                    .unwrap(),
                ) as Arc<dyn meerkat_runtime::RuntimeStore>
            } else {
                Arc::new(meerkat_runtime::InMemoryRuntimeStore::new())
            },
            Arc::new(meerkat::MemoryBlobStore::new()),
            host.configuration,
        )
        .unwrap();
        let runtime_store = persistence.runtime_store();
        let jobs = persistence.job_store();
        let (service, machine) =
            meerkat::surface::build_runtime_backed_service(builder, 4, persistence).unwrap();
        let service = Arc::new(service);
        let peer_observer = Arc::new(peer_ingress_tests::ObservedReceiver::new(
            host.originals.clone(),
        ));
        machine
            .install_peer_ingress_binder(peer_observer.clone())
            .unwrap();
        let mut definition = definition();
        definition.id = "factory".into();
        definition.profiles.insert(
            class().profile.into(),
            definition.profiles[&ProfileName::from("worker")].clone(),
        );
        if recorder {
            let ProfileBinding::Inline(profile) = definition
                .profiles
                .get_mut(&ProfileName::from(class().profile))
                .unwrap()
            else {
                unreachable!()
            };
            profile.tools.memory = true;
        }
        if let Some(retained) = &retained {
            definition = retained.definition.clone();
        }
        let (mob_store, provenance) = if durable {
            let (store, provenance) =
                meerkat_mobkit::mob_composition_manifest::persistent_mob_storage(
                    host.directory.path().join("native-mob.sqlite3"),
                )
                .unwrap();
            (store, Some(provenance))
        } else {
            (MobStorage::in_memory(), None)
        };
        // Match the native gateway's paired registry: a per-spawn overlay
        // alone cannot reconstruct class callbacks or Recorder after close.
        let customizer_tools =
            durable.then(meerkat_mobkit::identity_first::CustomizerToolRegistry::new);
        let bootstrap = MobBootstrapSpec::new(definition, mob_store, service.clone())
            .with_session_runtime_adapter(machine.clone())
            .unwrap()
            .with_member_runtime_registration_hook(host.registration.clone())
            .with_tool_consequence_policy_registry(host.tool_policy_registry);
        let bootstrap = match &customizer_tools {
            Some(registry) => bootstrap.with_spawn_member_customizer(Arc::new(
                meerkat_mobkit::identity_first::CustomizerToolsSpawnCustomizer::new(
                    registry.clone(),
                ),
            )),
            None => bootstrap,
        };
        let bootstrap = match provenance {
            Some(provenance) => bootstrap
                .with_mob_storage_provenance(provenance)
                .with_child_mob_storage(meerkat_mobkit::ChildMobStorage::ProcessBound),
            None => bootstrap.with_declared_ephemeral_mob_storage(),
        };
        // The native mob-tool composition owns the identity-runtime slot.
        // Keep a cold lift pending until actual customizer tools are published.
        let bootstrap = if durable {
            bootstrap
                .with_runtime_delivery(meerkat_mobkit::mob_handle_runtime::MobRuntimeDelivery::new(
                    runtime_store.clone(),
                    jobs,
                ))
                .with_agent_mob_tools(mob_tools)
                .unwrap()
        } else {
            bootstrap
        };
        *opened = Some(Box::pin(bootstrap_fixture(bootstrap, "factory".into())).await);
        let native = opened.as_mut().unwrap();
        let handle = native.mob_handle();
        if retained.is_some() {
            assert_eq!(
                handle.status().await.unwrap(),
                meerkat_mob::MobState::Stopped,
                "cold activation must wait for the actual identity context and customizer tools"
            );
        }
        let bridge = MobSessionBridge::with_session_service(
            handle.clone(),
            native.mob_runtime().session_service().cloned().unwrap(),
        )
        .with_controller_pin_source(service.clone());
        let runtime = Arc::new(
            IdentityRuntime::new(IdentityRuntimeConfig {
                continuity_store: Arc::new(if durable {
                    LocalContinuityStore::open(
                        host.directory.path().join("native-continuity.sqlite3"),
                    )
                    .unwrap()
                } else {
                    LocalContinuityStore::in_memory().unwrap()
                }),
                lease_provider: Arc::new(LocalLeaseProvider::new()),
                runtime_instance_id: "class-fixture".into(),
                has_runtime_store: true,
                durability_policy: DurabilityPolicy::SyncWriteThrough,
                bridge: Some(Arc::new(bridge)),
                default_timeout: None,
            })
            .with_runtime_services(
                meerkat_mobkit::identity_first::AgentRuntimeServices::new(handle.clone()),
            ),
        );
        runtime.set_customizer_tool_registry(customizer_tools).await;
        let coordinator = spec(MEMBER, "worker");
        let desired = retained
            .as_ref()
            .map_or_else(|| vec![coordinator], |saved| saved.roster.clone());
        let roster = Arc::new(MutableRosterProvider::new(desired.clone()));
        let callbacks: Arc<dyn meerkat_mobkit::identity_first::AgentCustomizer> =
            Arc::new(GatewayAgentCustomizer::new(GatewayBridge(factory.clone())));
        let callbacks = if recorder {
            use meerkat_mobkit::identity_first::agent_memory::{
                AgentMemoryConfig, AgentMemoryCustomizer,
            };
            let memory = Arc::new(
                meerkat_mobkit::memory::SqliteAgentMemoryStore::open(
                    host.directory.path().join("selected-recorder"),
                )
                .unwrap(),
            );
            Arc::new(AgentMemoryCustomizer::wrap(Some(callbacks), memory, AgentMemoryConfig {
                realm: host.lifetime.selected_policy().native_host.as_ref().unwrap().realm.to_string(),
                per_turn_injection: meerkat_mobkit::identity_first::agent_memory::AgentMemoryPerTurnInjection::Off,
                ..AgentMemoryConfig::default()
            })) as Arc<dyn meerkat_mobkit::identity_first::AgentCustomizer>
        } else {
            callbacks
        };
        let callbacks =
            peer_ingress_tests::observe_recorder_ticket(callbacks, factory.recorder_ticket.clone());
        let customizer = host.tool_policy.member_class_gateway_customizer(callbacks);
        let context = Arc::new(IdentityFirstRuntimeContext::new(
            runtime,
            roster.clone(),
            topology,
            Some(customizer),
            Some(handle.definition().clone()),
        ));
        let restored = native
            .install_and_bootstrap_identity_first_context(context, &desired)
            .await
            .unwrap();
        if retained.is_some() {
            assert_eq!(restored.outcomes.len(), desired.len());
            for member in &desired {
                assert!(
                    matches!(
                        restored.outcomes.get(&member.identity),
                        Some(RestoreOutcome::Resumed { .. })
                    ),
                    "every retained class fixture member must resume: {:?}",
                    restored.outcomes
                );
            }
        }
        let source = host.originals;
        source.bind_machine(machine.clone()).unwrap();
        source
            .bind_identity_runtime(&native.identity_runtime().unwrap())
            .unwrap();
        source.bind_controller_pin_source(service.clone()).unwrap();
        let (private_open, private_closed) = tokio::sync::watch::channel(false);
        let gateway = Arc::new(ToolkitGatewayHost::with_prepared_private_originals(
            host.lifetime.clone(),
            crate::private_host::PrivateHostClosed(private_closed),
            source.clone(),
            host.registration,
        ));
        Self {
            _directory: host.directory,
            selection: host.selection,
            roster,
            runtime_store,
            lifetime: host.lifetime,
            native: opened.take().unwrap(),
            gateway,
            source,
            policy: host.tool_policy,
            factory,
            service,
            machine,
            provider,
            peer_observer,
            application_inputs,
            _current: current,
            _private_open: private_open,
            _exclusive: None,
        }
    }
    async fn admit(&self, name: &str) -> Arc<AdmittedMemberClass> {
        let admitted = Arc::new(
            self.policy
                .reserve_member_class(Id::new(format!("admit-{name}")).unwrap(), instance(name))
                .await
                .unwrap(),
        );
        let record = self
            .policy
            .member_class_admission(&instance(name).member.member)
            .unwrap()
            .unwrap();
        // The application explicitly retains the complete empty set for this
        // exact admission before native registration. Absence is not no-MCP.
        self.application_inputs
            .retain(RetainedMemberMcpBindings {
                selection: MemberMcpAdmissionSelection::from_admission(&record).unwrap(),
                bindings: vec![],
                accepted_requirement: None,
            })
            .await
            .unwrap();
        admitted
    }
    fn route(&self, name: &str) -> CompiledNativeSourceRoute {
        let mut route = self
            .lifetime
            .selected_policy()
            .native_host
            .as_ref()
            .unwrap()
            .routes[0]
            .clone();
        route.id = Id::new(format!("console-{name}")).unwrap();
        let CompiledNativeSourceOrigin::LocalConsole { member, .. } = &mut route.origin else {
            panic!("console fixture")
        };
        *member = instance(name).member.member;
        route.logical_executor = instance(name).member.logical_owner;
        let native = self
            .lifetime
            .selected_policy()
            .native_host
            .as_ref()
            .unwrap();
        if native.member_classes[0].purpose.is_ordinary() {
            let CompiledNativeSourceOrigin::LocalConsole {
                requester,
                ingress_actor,
                ..
            } = &mut route.origin
            else {
                unreachable!()
            };
            *requester = native.remote_operator.clone().unwrap();
            *ingress_actor = native.ingress_domain.authority.clone();
            route.represented_subject = native.remote_operator.clone();
            route.source_authority = native.ingress_domain.authority.clone();
            route.authentication_domain = native.authentication_domain.clone().unwrap();
            route.event_domain = native.event_domain.clone().unwrap();
        }
        route.application_parent = None;
        route.controller_parent = None;
        route
    }
    fn intent(&self, name: &str) -> PreparedControllerIntent {
        PreparedControllerIntent {
            target: PreparedControllerTarget::OriginalRoute {
                route: self.route(name).id,
            },
            issuance_id: EvidenceId::new(format!("controller-{name}")).unwrap(),
            controller_route: None,
        }
    }
    async fn close(&self) {
        self.gateway
            .begin_close(GatewayHostCloseReason::Shutdown)
            .unwrap();
        let (native, local) =
            tokio::join!(shutdown_fixture(&self.native), self.gateway.join_close(),);
        assert!(native.is_ok(), "{native:?}");
        assert!(local.is_ok(), "{local:?}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn accepted_dynamic_source_settles_after_activation_close_without_live_route() {
    use crate::original_ingress::custody_attempt::CustodyDecision;
    let fixture = Fixture::start_selected_source_with_recorder(class(), None, None, true).await;
    let mut first_round = Some(fixture.source.hold_first_custody_round_for_test());
    fixture
        .source
        .start_custody_recovery(fixture.native.mob_runtime().continuation_source_announcer())
        .unwrap();
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT * 8,
        Box::pin(async {
            let receiver = native_operations_tests::add(&fixture, ADDED).await;
            let sender = native_operations_tests::add(&fixture, "project-aborted").await;
            native_operations_tests::ready(&fixture, &receiver).await;
            native_operations_tests::ready(&fixture, &sender).await;
            let handle = fixture.native.mob_handle();
            let recipient = handle
                .get_member(&meerkat_mobkit::member_comms_id::mob_member_id(ADDED))
                .await
                .unwrap()
                .unwrap();
            let sending = handle
                .get_member(&meerkat_mobkit::member_comms_id::mob_member_id(
                    "project-aborted",
                ))
                .await
                .unwrap()
                .unwrap();
            handle
                .wire(recipient.agent_identity.clone(), sending.agent_identity)
                .await
                .unwrap();
            *fixture.factory.next_call.lock().unwrap() = None;
            let sent = fixture
                .service
                .send_comms(
                    &sender,
                    meerkat_core::CommsCommand::PeerMessage {
                        to: meerkat_core::comms::PeerRoute::new(recipient.peer_id().unwrap()),
                        body: "Complete this accepted source before host close".into(),
                        blocks: None,
                        content_taint: None,
                        handling_mode: meerkat_core::types::HandlingMode::Queue,
                        objective_id: None,
                        in_reply_to: None,
                    },
                )
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(
                sent,
                meerkat_core::SendReceipt::PeerMessageSent {
                    delivery: meerkat_core::comms::PeerDeliveryOutcome::DurablyResolved {
                        outcome:
                            meerkat_core::interaction::PeerIngressTerminalOutcomeKind::Accepted
                    },
                    ..
                }
            ));
            let rows = native_operations_tests::native_input_states(&fixture, &receiver).await;
            assert_eq!(rows.len(), 1);
            let input_id = rows[0].state.input_id.clone();
            // Native acceptance is the prerequisite. Model execution may still
            // await source promotion, which this actual worker hold prevents.
            let held = fixture
                .source
                .retained
                .lock()
                .unwrap()
                .iter()
                .find(|held| {
                    held.first_custody
                        .lock()
                        .unwrap()
                        .as_ref()
                        .is_some_and(|pending| pending.input.id() == &input_id)
                })
                .cloned()
                .unwrap();
            let pending = held.first_custody.lock().unwrap().clone().unwrap();
            assert!(pending.handoff.phase() == custody_recovery::HandoffPhase::Released);
            assert!(
                pending.handoff.native_admitted(),
                "the real Peer callback retains Core's Admitted fact"
            );
            assert!(
                pending.native_admission.lock().unwrap().is_none(),
                "Peer transfer is not a synthetic governed callback"
            );
            let key = fixture.source.key(&held.reference).unwrap();
            let before = fixture.source.store.get(&key).await.unwrap().unwrap();
            let attempt = fixture
                .source
                .decode_record(&before, &held.reference)
                .unwrap()
                .custody_attempt
                .unwrap();
            assert!(
                attempt.decision() == CustodyDecision::Pending,
                "the held real recovery round has not promoted the accepted source"
            );
            let grants = fixture.source.callback_grant_context().unwrap();
            let admission = fixture
                .policy
                .member_class_admission(&instance(ADDED).member.member)
                .unwrap()
                .unwrap();
            let WorkAuthorityBasis::TargetedGrantLineages { targets, .. } =
                &held.original.association().candidate().authority_basis
            else {
                panic!("actual Peer targets");
            };
            fixture
                .gateway
                .begin_close(GatewayHostCloseReason::Shutdown)
                .unwrap();
            assert!(matches!(
                fixture.source.key(&held.reference),
                Err(PrivateOriginalError::Denied)
            ));
            assert!(
                fixture.source.callback_grant_context().is_err(),
                "close must still fence new authority"
            );
            fixture
                .source
                .retained_custody_record(&before, &held)
                .unwrap();
            for field in ["key", "event", "source", "association"] {
                let mut changed = before.clone();
                match field {
                    "key" => changed.key.id.push_str("-foreign"),
                    "event" => {
                        changed.data.body["event"]["content_sha256"] =
                            serde_json::json!("0".repeat(64))
                    }
                    "source" => {
                        changed.data.body["source"]["realm"] = serde_json::json!("foreign-realm")
                    }
                    "association" => {
                        changed.data.body["final_association"] = serde_json::Value::Null
                    }
                    _ => unreachable!(),
                }
                changed.content_digest = changed.data.digest().unwrap();
                assert!(
                    fixture
                        .source
                        .retained_custody_record(&changed, &held)
                        .is_err(),
                    "{field}"
                );
            }
            first_round.take().unwrap().send(()).unwrap();
            (
                key,
                before,
                attempt,
                pending,
                targets.clone(),
                admission,
                grants,
                receiver,
            )
        }),
    ))
    .catch_unwind()
    .await;
    // Release the actual worker on every assertion/timeout path, then join
    // the native and local owners exactly once outside the input timeout.
    drop(first_round.take());
    fixture.close().await;
    let (key, before, attempt, pending, targets, admission, grants, receiver) = match checked {
        Ok(result) => result.expect("accepted source close preparation did not settle"),
        Err(panic) => std::panic::resume_unwind(panic),
    };
    let after = fixture.source.store.get(&key).await.unwrap().unwrap();
    let settled = fixture.source.decode_stored(&after).unwrap();
    let completed = settled.custody_attempt.unwrap();
    assert!(completed.decision() == CustodyDecision::Promoted && completed.same_attempt(&attempt));
    assert!(
        matches!(
            &*pending.pending.lock().unwrap(),
            governed_submit::SourcePublication::Settled {
                source_acknowledged: false,
                ..
            }
        ),
        "closed promotion is not live source publication"
    );
    let mut old = before.data.body.clone();
    let mut new = after.data.body.clone();
    old.as_object_mut().unwrap().remove("custody_attempt");
    new.as_object_mut().unwrap().remove("custody_attempt");
    assert_eq!(
        new, old,
        "settlement changes only the existing custody attempt"
    );
    for target in targets {
        grants
            .grants
            .resolve_lineage(
                &target.lineage,
                &admission.route.as_ref().unwrap().logical_executor,
                admission
                    .route
                    .as_ref()
                    .unwrap()
                    .represented_subject
                    .as_ref(),
            )
            .unwrap()
            .check_current()
            .unwrap();
        for reference in &target.lineage {
            assert_eq!(
                grants
                    .grants
                    .issued_reference(&reference.grant_id)
                    .unwrap()
                    .as_ref(),
                Some(reference)
            );
        }
    }
    assert_eq!(
        native_operations_tests::native_input_states(&fixture, &receiver)
            .await
            .len(),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ordinary_class_add_refuses_foreign_installed_authority_before_native_entry() {
    let fixture = Box::pin(Fixture::start()).await;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT * 4,
        Box::pin(async {
            let identity = AgentIdentity::parse(ADDED).unwrap();
            let selected = instance(ADDED).member;
            let route = fixture.route(ADDED);
            fixture
                .source
                .check_operator_member_route(&selected, &route, &identity)
                .unwrap();
            let mut variants = Vec::new();
            for field in [
                "requester",
                "represented_subject",
                "ingress_actor",
                "source_authority",
                "authentication_domain",
                "event_domain",
                "logical_executor",
                "qualification",
            ] {
                let mut changed = route.clone();
                match field {
                    "requester" => {
                        let CompiledNativeSourceOrigin::LocalConsole { requester, .. } =
                            &mut changed.origin
                        else {
                            unreachable!()
                        };
                        *requester = principal("foreign");
                    }
                    "represented_subject" => {
                        changed.represented_subject = Some(principal("foreign"))
                    }
                    "ingress_actor" => {
                        let CompiledNativeSourceOrigin::LocalConsole { ingress_actor, .. } =
                            &mut changed.origin
                        else {
                            unreachable!()
                        };
                        *ingress_actor = principal("foreign");
                    }
                    "source_authority" => changed.source_authority = principal("foreign"),
                    "authentication_domain" => {
                        changed.authentication_domain.namespace.push_str("-foreign")
                    }
                    "event_domain" => changed.event_domain.namespace.push_str("-foreign"),
                    "logical_executor" => changed.logical_executor.id = principal("foreign").id,
                    "qualification" => {
                        changed.logical_executor.qualification = principal(ADDED).qualification
                    }
                    _ => unreachable!(),
                }
                variants.push((field, changed));
            }
            // Canonical application reservations are real. Each failed Add must
            // retire before the application explicitly reserves its exact successor.
            for (index, (field, changed)) in variants.into_iter().enumerate() {
                let id = Id::new(format!("authority-refusal-{index}")).unwrap();
                let admitted = if index == 0 {
                    fixture
                        .policy
                        .reserve_member_class(id, instance(ADDED))
                        .await
                        .unwrap()
                } else {
                    let predecessor = fixture
                        .policy
                        .member_class_checkpoint(&selected.member)
                        .await
                        .unwrap();
                    fixture
                        .policy
                        .readmit_member_class_declaration(predecessor, id, instance(ADDED))
                        .await
                        .unwrap()
                };
                let expected_admission = admitted.admission_id().clone();
                let context = fixture.native.identity_first_context().unwrap();
                let refused = fixture
                    .gateway
                    .add_member_class(
                        &context,
                        Arc::new(admitted),
                        spec(ADDED, &class().profile),
                        changed,
                        fixture.intent(ADDED),
                        tokio::time::Instant::now() + WAIT,
                        || async { panic!("refused Add cannot publish") },
                    )
                    .await;
                assert!(
                    matches!(
                        refused,
                        Err(MemberClassAddError::Preflight {
                            error: HostLifetimeError::Denied,
                            cleanup: Ok(())
                        })
                    ),
                    "{field}: {refused:?}"
                );
                assert!(matches!(
                    fixture.policy.member_class_admission(&selected.member),
                    Err(HostLifetimeError::Denied)
                ));
                let observation = fixture
                    .policy
                    .observe_member_class(&selected.member)
                    .await
                    .unwrap();
                assert_eq!(observation.authorization, Err(HostLifetimeError::Denied));
                let retired = observation.admission;
                assert_eq!(retired.id, expected_admission);
                assert_eq!(retired.class, class());
                assert_eq!(retired.instance, instance(ADDED));
                assert!(matches!(
                    retired.phase,
                    crate::installed_host::dynamic_members::MemberClassAdmissionPhase::Retired
                ));
                assert!(retired.route.is_none() && retired.destination.is_none());
                assert_eq!(fixture.factory.preparations.load(Ordering::SeqCst), 0);
                assert_eq!(fixture.native.mob_handle().list_members().await.len(), 1);
            }
            // These member facts precede the application's own reservation check;
            // check the same native guard directly, without a permissive policy fake.
            let mut wrong_owner = selected.clone();
            wrong_owner.logical_owner = principal(ADDED);
            assert_eq!(
                fixture
                    .source
                    .check_operator_member_route(&wrong_owner, &route, &identity),
                Err(HostLifetimeError::Denied)
            );
            let mut wrong_audience = selected.clone();
            wrong_audience.audience = AudienceRef::Destination {
                authority: selected.logical_owner.clone(),
                destination_id: ADDED.into(),
            };
            assert_eq!(
                fixture
                    .source
                    .check_operator_member_route(&wrong_audience, &route, &identity),
                Err(HostLifetimeError::Denied)
            );
            wrong_audience.audience = AudienceRef::Principal {
                principal: principal("foreign"),
            };
            assert_eq!(
                fixture
                    .source
                    .check_operator_member_route(&wrong_audience, &route, &identity),
                Err(HostLifetimeError::Denied)
            );
            // Durable identities are not their encoded roster aliases.
            let encoded = AgentIdentity::parse("project:one").unwrap();
            let encoded_member = instance(encoded.as_str()).member;
            let encoded_route = fixture.route(encoded.as_str());
            fixture
                .source
                .check_operator_member_route(&encoded_member, &encoded_route, &encoded)
                .unwrap();
            assert_ne!(encoded_member.member.member, encoded.as_str());
            assert_eq!(
                fixture.source.check_operator_member_route(
                    &encoded_member,
                    &encoded_route,
                    &identity
                ),
                Err(HostLifetimeError::Denied)
            );
        }),
    ))
    .catch_unwind()
    .await;
    fixture.close().await;
    match checked {
        Ok(Ok(())) => (),
        Err(panic) => std::panic::resume_unwind(panic),
        _ => panic!("installed member authority control timed out"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn actual_held_class_factory_and_ordinary_route_share_one_native_owner() {
    let fixture = Box::pin(Fixture::start()).await;
    let result = std::panic::AssertUnwindSafe(tokio::time::timeout(WAIT * 5, Box::pin(async {
        assert_eq!(fixture.native.mob_handle().list_members().await.len(), 1);
        assert_eq!(fixture.factory.preparations.load(Ordering::SeqCst), 0, "zero initial class instances");
        assert!(fixture.source.member_route(&fixture.route(ADDED).id).unwrap().is_none());
        let admitted = fixture.admit(ADDED).await;
        assert!(admitted.dispatcher().is_none());
        assert!(fixture.policy.member_class_admission(&instance(ADDED).member.member).unwrap().unwrap().installed_surface.is_none());
        assert!(fixture.source.member_route(&fixture.route(ADDED).id).unwrap().is_none(), "reservation is not publication");
        let context = fixture.native.identity_first_context().unwrap();
        let finalized = Arc::new(AtomicUsize::new(0));
        let finalizer_calls = finalized.clone();
        let source = fixture.source.clone();
        let policy = fixture.policy.clone();
        let route_id = fixture.route(ADDED).id;
        let result = fixture.gateway.add_member_class(&context, admitted.clone(), spec(ADDED, &class().profile),
            fixture.route(ADDED), fixture.intent(ADDED), tokio::time::Instant::now() + WAIT,
            move || async move {
                let selected = policy.member_class_admission(&instance(ADDED).member.member)?
                    .ok_or(HostLifetimeError::Denied)?;
                if selected.route.is_none()
                    || !matches!(selected.phase, crate::installed_host::dynamic_members::MemberClassAdmissionPhase::Prepared { .. })
                    || source.member_route(&route_id).map_err(|_| HostLifetimeError::Unavailable)?.is_some()
                { return Err(HostLifetimeError::Denied); }
                finalizer_calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }).await.unwrap();
        let outcome = match result {
            MemberClassAddStart::Settled(outcome) => outcome,
            MemberClassAddStart::Unsettled(settlement) => {
                let actual = settlement.settled().await;
                panic!("native owner refused the expected fresh add: {actual:?}");
            }
        };
        assert!(outcome.authorization.is_ok() && outcome.cleanup.is_ok(), "{outcome:?}");
        assert_eq!(finalized.load(Ordering::SeqCst), 1, "finalizer runs after durable route commitment, before projection");
        let native = outcome.native.unwrap();
        assert!(matches!(native.outcomes.get(&AgentIdentity::parse(ADDED).unwrap()), Some(RestoreOutcome::Created { .. })), "pass Activated is insufficient without exact Created: {native:?}");
        assert_eq!(fixture.factory.preparations.load(Ordering::SeqCst), 1, "actual gateway construction only");
        let runtime = fixture.native.identity_runtime().unwrap();
        let identity = AgentIdentity::parse(ADDED).unwrap();
        let status = runtime.status(&identity).await.unwrap();
        assert_eq!(status.state, IdentityLifecycleState::Active);
        let session = status.session_id.unwrap();
        // Neither the accepted handle nor a separately observed same-id replay
        // owns a second Add. In particular, refusal must not retire this live
        // original before its ordinary callback/input path is exercised below.
        let before_duplicate = fixture.policy.member_class_admission(&instance(ADDED).member.member).unwrap().unwrap();
        for duplicate in [admitted.clone(), fixture.admit(ADDED).await] {
            let calls = finalized.clone();
            let refused = fixture.gateway.add_member_class(&context, duplicate,
                spec(ADDED, &class().profile), fixture.route(ADDED), fixture.intent(ADDED),
                tokio::time::Instant::now() + WAIT, move || async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }).await;
            assert!(matches!(refused, Err(MemberClassAddError::ReservationNotAccepted { error: HostLifetimeError::Denied })), "duplicate must not enter native or cleanup: {refused:?}");
            assert_eq!(fixture.policy.member_class_admission(&instance(ADDED).member.member).unwrap().unwrap(), before_duplicate);
        }
        assert_eq!(finalized.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.factory.preparations.load(Ordering::SeqCst), 1);
        let (issued_route, destination, context_id) = fixture.source.selected_source_route(&fixture.route(ADDED).id).unwrap();
        assert!(issued_route.controller_parent.as_ref().is_some_and(|parents| !parents.is_empty()));
        fixture.source.check_operator_member_route(&destination, &issued_route, &identity).unwrap();
        assert_eq!(fixture.source.selected_source_route(&issued_route.id).unwrap().0, issued_route,
            "retained Active projection preserves the actual issued parent");
        assert_eq!(fixture.policy.member_class_admission(&instance(ADDED).member.member).unwrap().unwrap(), before_duplicate);
        assert_eq!(destination, instance(ADDED).member);
        assert_eq!(context_id.unwrap().as_str(), session.to_string());
        assert!(destination.setup_capability.is_none());
        let lease = fixture.service.acquire_live_session_actor_turn_boundary_lease(&session).await.unwrap();
        let pin = fixture.service.pin_controller_client_for_actor(&lease).await.unwrap();
        drop(lease);
        meerkat_core::publish_token_lifecycle_acquired_for_identity_within(
            &fixture.machine.generated_auth_lease_handle(), pin.selection().credential(),
            &meerkat_core::auth::PersistedTokens::api_key("synthetic-controller-only-fixture"),
            tokio::time::Instant::now() + WAIT,
        ).await.unwrap();
        // A slow application lookup must not hold source_writer. Cancel an
        // unrelated not-yet-bound original while that actual lookup is entered,
        // then join it and confirm that no class authority was committed.
        let mut cancelled_event = private_event("dynamic-cancel-during-application-lookup");
        cancelled_event.route = fixture.route(ADDED).id;
        let cancelled = fixture.source.retain_event(cancelled_event).await.unwrap();
        let before_cancel = fixture.source.store.get(&fixture.source.key(&cancelled).unwrap()).await.unwrap().unwrap();
        let cancel_record = fixture.source.decode_record(&before_cancel, &cancelled).unwrap();
        let pause = Arc::new(ApplicationLookupPause::default());
        *fixture.application_inputs.pause_lookup.lock().unwrap() = Some(pause.clone());
        let (selection, cancellation) = tokio::join!(
            fixture.source.class_input_basis(&cancel_record, &cancelled, &session),
            async {
                pause.entered.notified().await;
                let result = fixture.source.record_operation_cancelled(&cancelled).await;
                pause.release.notify_one();
                result
            }
        );
        cancellation.unwrap();
        assert!(matches!(selection, Err(PrivateOriginalError::Denied)), "cancelled lookup cannot publish a basis");
        let after_cancel = fixture.source.store.get(&before_cancel.key).await.unwrap().unwrap();
        let cancelled_record = fixture.source.decode_record(&after_cancel, &cancelled).unwrap();
        assert!(cancelled_record.operation_cancelled);
        assert!(cancelled_record.class_input.is_none() && cancelled_record.final_association.is_none());
        let mut event = private_event("dynamic-ordinary-input");
        event.route = fixture.route(ADDED).id;
        event.content_sha256 = digest(PROMPT.as_bytes());
        let authentication = event.original_authentication.clone();
        let original = fixture.source.retain_event(event).await.unwrap();
        let before = fixture.source.store.get(&fixture.source.key(&original).unwrap())
            .await.unwrap().unwrap();
        let record = fixture.source.decode_record(&before, &original).unwrap();
        assert!(fixture.source.configured_setup_basis(&record, &original, &session, None)
            .await.unwrap().is_none(), "the dynamic route selects no setup capability");
        assert_eq!(fixture.source.store.get(&before.key).await.unwrap(), Some(before),
            "checking an absent capability does not change the original");
        let submitted = fixture.source.submit_authenticated_original(PrivateOriginalSubmit {
            original: original.clone(), content: PROMPT.into(), transport_authentication: authentication.clone(),
        }).await.unwrap();
        let PrivateOriginalSubmission::Admitted { input_id, turn_ticket: Some(ticket), .. } = submitted else {
            panic!("dynamic ordinary source must receive actual native admission");
        };
        let outcome = runtime.wait_for_turn(&identity, TurnTicket::parse(&ticket).unwrap(), WAIT).await;
        assert!(matches!(outcome, TurnOutcome::Completed { output: TurnOutput::Text { text, .. } } if text == ANSWER));
        eprintln!("dynamic callback evidence: successful_model_requests={}, request_tools={:?}, callback_results={:?}, model_errors={:?}",
            fixture.provider.0.load(Ordering::SeqCst), fixture.factory.request_tools.lock().unwrap(),
            fixture.factory.callback_results.lock().unwrap(), fixture.factory.model_errors.lock().unwrap());
        // Read the real per-input audit before the callback assertion, so a
        // denied dispatch cannot be mistaken for a successful model answer.
        use meerkat_authorization_contracts::audit::{AuditObservation, AuditTarget, StoredAuthorizationAuditObservation};
        use meerkat_core::authorization::OperationObservedOutcome;
        let native_input = meerkat_core::InputId::from_uuid(uuid::Uuid::parse_str(&input_id).unwrap());
        let actual = fixture.machine.input_state(&session, &native_input).await.unwrap().unwrap();
        let encoded = serde_json::to_value(&actual).unwrap();
        eprintln!("dynamic native authorization_audit={}", encoded["authorization_audit"]);
        let audit: Vec<StoredAuthorizationAuditObservation> = serde_json::from_value(encoded["authorization_audit"].clone()).unwrap();
        let callback_operations = audit.iter().filter_map(|row| match &row.observation.observation {
            AuditObservation::Prepared { target, policy }
                if matches!(target.as_ref(), AuditTarget::Tool { tool_name, owners, .. }
                    if tool_name == CALLBACK && owners.iter().any(|owner| owner.owner_key == CALLBACK
                        && owner.authority_key.starts_with("tool-bundle:")))
                && policy.operation_authorities.iter().any(|basis| matches!(basis,
                    WorkAuthorityBasis::TargetedGrantLineages { base: None, targets }
                        if targets.iter().any(|target| matches!(&target.target,
                            meerkat_authorization_contracts::work_association::GrantTarget::RegisteredToolBundleOperation { operation, .. }
                                if operation == CALLBACK)))) => Some(row.observation.operation_id.clone()),
            _ => None,
        }).collect::<std::collections::HashSet<_>>();
        assert!(!callback_operations.is_empty(), "actual callback authorization was never prepared");
        assert!(audit.iter().any(|row| callback_operations.contains(&row.observation.operation_id)
            && matches!(&row.observation.observation, AuditObservation::Outcome {
                outcome: OperationObservedOutcome::ToolDispatchReturned { result_is_error: false, terminal_error: None, .. }
            })), "native audit has no successful callback outcome");
        assert!(audit.iter().all(|row| !callback_operations.contains(&row.observation.operation_id)
            || !matches!(row.observation.observation, AuditObservation::Refused { .. } | AuditObservation::AuthorizationUnavailable { .. })));
        assert_eq!(fixture.factory.calls.lock().unwrap().as_slice(), [CALLBACK]);
        assert_eq!(fixture.provider.0.load(Ordering::SeqCst), 2);
        let binding = fixture.source.binding_evidence(&original).await.unwrap().unwrap();
        assert!(matches!(&binding.candidate().authority_basis,
            WorkAuthorityBasis::TargetedGrantLineages { base: None, targets } if targets.len() == 2
                && matches!(&targets[0].target, meerkat_authorization_contracts::work_association::GrantTarget::RegisteredToolBundleOperation { operation, .. } if operation == CALLBACK)
                && targets[0].lineage.len() == 2),
            "ordinary callback authority is exact and has no setup or base authority");
        let saved = fixture.source.store.get(&fixture.source.key(&original).unwrap()).await.unwrap().unwrap();
        let replay = fixture.source.submit_authenticated_original(PrivateOriginalSubmit {
            original: original.clone(), content: PROMPT.into(), transport_authentication: authentication,
        }).await.unwrap();
        assert!(matches!(replay, PrivateOriginalSubmission::Admitted { input_id: same, deduplicated: true, .. } if same == input_id));
        let replayed = fixture.source.store.get(&fixture.source.key(&original).unwrap()).await.unwrap().unwrap();
        let saved = fixture.source.decode_record(&saved, &original).unwrap();
        let replayed = fixture.source.decode_record(&replayed, &original).unwrap();
        assert!(saved.class_input == replayed.class_input && saved.final_association == replayed.final_association,
            "replay must not rewrite the callback selection or immutable input");
        let selected_json = serde_json::to_value(saved.class_input.as_ref().unwrap()).unwrap();
        assert_eq!(selected_json["mcp"]["entries"], serde_json::json!([]),
            "no MCP is an explicit retained application selection for this admission");
        assert_eq!(selected_json["mcp"]["admission"]["admission_id"], serde_json::json!(admitted.admission_id()));
        let replay_basis = fixture.source.class_input_basis(&replayed, &original, &session).await.unwrap().unwrap();
        assert_eq!(replay_basis, binding.candidate().authority_basis,
            "fresh application readback authenticates the same immutable replay basis");
        let unchanged = fixture.source.store.get(&fixture.source.key(&original).unwrap()).await.unwrap().unwrap();
        assert!(fixture.source.decode_record(&unchanged, &original).unwrap() == replayed,
            "selection revalidation must not rewrite the original");
        let mut old_snapshot = replayed.clone();
        let mut old_selection = selected_json;
        old_selection.as_object_mut().unwrap().remove("mcp");
        old_snapshot.class_input = Some(serde_json::from_value(old_selection).unwrap());
        assert!(matches!(fixture.source.class_input_basis(&old_snapshot, &original, &session).await,
            Err(PrivateOriginalError::Denied)), "an old None snapshot cannot acquire a new Some selection");
        assert_eq!(fixture.source.store.get(&unchanged.key).await.unwrap(), Some(unchanged));
        assert_eq!(fixture.factory.calls.lock().unwrap().as_slice(), [CALLBACK]);
        let WorkAuthorityBasis::TargetedGrantLineages { targets, .. } = &binding.candidate().authority_basis else { unreachable!() };
        let lineage = targets[0].lineage.clone();
        let mut mutation = fixture.machine.try_controller_grant_mutation().unwrap();
        let native_owner = fixture.lifetime.selected_policy().native_host.as_ref().unwrap().grant_administrator.clone();
        fixture.source.owner.grants().revoke(&meerkat_core::AdmissionGuard::none(), &native_owner, lineage.last().unwrap(), &mut mutation).unwrap();
        drop(mutation);
        for (event_id, operation, expected_calls) in [
            ("dynamic-unrelated-after-revoke", OTHER_CALLBACK, vec![CALLBACK, OTHER_CALLBACK]),
            ("dynamic-refused-after-revoke", CALLBACK, vec![CALLBACK, OTHER_CALLBACK]),
        ] {
            *fixture.factory.next_call.lock().unwrap() = Some(operation);
            let mut event = private_event(event_id);
            event.route = fixture.route(ADDED).id;
            event.content_sha256 = digest(PROMPT.as_bytes());
            let authentication = event.original_authentication.clone();
            let next = fixture.source.retain_event(event).await.unwrap();
            let submitted = fixture.source.submit_authenticated_original(PrivateOriginalSubmit {
                original: next.clone(), content: PROMPT.into(), transport_authentication: authentication,
            }).await.unwrap();
            let PrivateOriginalSubmission::Admitted { input_id, turn_ticket: Some(ticket), .. } = submitted
                else { panic!("a revoked callback cannot gate new controller input"); };
            let outcome = runtime.wait_for_turn(&identity, TurnTicket::parse(&ticket).unwrap(), WAIT).await;
            assert!(matches!(outcome, TurnOutcome::Completed { output: TurnOutput::Text { text, .. } } if text == ANSWER));
            let actual = fixture.machine.input_state(&session, &meerkat_core::InputId::from_uuid(uuid::Uuid::parse_str(&input_id).unwrap())).await.unwrap().unwrap();
            let encoded = serde_json::to_value(&actual).unwrap();
            eprintln!("callback operation {operation} after leaf revoke authorization_audit={}", encoded["authorization_audit"]);
            let audit: Vec<StoredAuthorizationAuditObservation> = serde_json::from_value(encoded["authorization_audit"].clone()).unwrap();
            let prepared = audit.iter().any(|row| matches!(&row.observation.observation,
                AuditObservation::Prepared { target, .. } if matches!(target.as_ref(),
                    AuditTarget::Tool { tool_name, .. } if tool_name == operation)));
            let denied = audit.iter().any(|row| matches!(&row.observation.observation,
                AuditObservation::Refused { target, reason: meerkat_core::authorization::OperationRefusalKind::Denied, .. }
                    if matches!(target.as_ref(), AuditTarget::Tool { tool_name, .. } if tool_name == operation)));
            assert_eq!(prepared, operation == OTHER_CALLBACK);
            assert_eq!(denied, operation == CALLBACK);
            assert_eq!(fixture.factory.calls.lock().unwrap().as_slice(), expected_calls);
            assert!(fixture.source.binding_evidence(&next).await.unwrap().unwrap().candidate().authority_basis
                == binding.candidate().authority_basis, "new input retains the same exact independent target refs");
        }
        assert_eq!(fixture.provider.0.load(Ordering::SeqCst), 6);
        assert!(fixture.source.owner.grants().resolve_lineage(&lineage,
            &binding.candidate().logical_executor, binding.candidate().represented_subject.as_ref()).is_err(),
            "fresh composition cannot revive the revoked callback leaf");
        assert!(fixture.source.owner.grants().resolve_lineage(&lineage[..1],
            &native_owner, binding.candidate().represented_subject.as_ref()).is_ok(),
            "revoking a leaf must leave the class root current");
        fixture.gateway.begin_close(GatewayHostCloseReason::Shutdown).unwrap();
        assert!(matches!(fixture.source.callback_grant_context(),
            Err(HostLifetimeError::Denied)), "close still fences new callback issuance");
        admitted.retire().await.unwrap();
        assert_eq!(fixture.policy.observe_member_class(&instance(ADDED).member.member)
            .await.unwrap().admission.phase,
            crate::installed_host::dynamic_members::MemberClassAdmissionPhase::Retired,
            "exact canonical retirement settles after host close");
        assert!(fixture.source.owner.grants().resolve_lineage(&lineage[..1],
            &native_owner, binding.candidate().represented_subject.as_ref()).is_err(),
            "retirement must revoke the actual native class root, independently of the already revoked leaf");
        assert!(fixture.source.member_route(&fixture.route(ADDED).id).is_err());
        assert!(matches!(fixture.policy.member_class_admission(&instance(ADDED).member.member), Err(HostLifetimeError::Denied)));
    }))).catch_unwind().await;
    fixture.close().await;
    match result {
        Ok(Ok(())) => (),
        Err(panic) => std::panic::resume_unwind(panic),
        _ => panic!("class native input timed out"),
    }
}

#[path = "dynamic_member_publication_store.rs"]
mod publication_store;
use publication_store::HeldPublicationWrite;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn new_reservation_preflight_refusal_retires_without_native_entry() {
    let fixture = Box::pin(Fixture::start()).await;
    let result = std::panic::AssertUnwindSafe(tokio::time::timeout(WAIT * 2, Box::pin(async {
        let admitted = fixture.admit("project-refused").await;
        let context = fixture.native.identity_first_context().unwrap();
        let mut route = fixture.route("project-refused");
        route.id = fixture.lifetime.selected_policy().native_host.as_ref().unwrap().routes[0].id.clone();
        let mut intent = fixture.intent("project-refused");
        intent.target = PreparedControllerTarget::OriginalRoute { route: route.id.clone() };
        let finalized = Arc::new(AtomicUsize::new(0));
        let calls = finalized.clone();
        let refused = fixture.gateway.add_member_class(&context, admitted,
            spec("project-refused", &class().profile), route, intent,
            tokio::time::Instant::now() + WAIT, move || async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }).await;
        assert!(matches!(refused, Err(MemberClassAddError::Preflight {
            error: HostLifetimeError::Denied, cleanup: Ok(())
        })), "static route collision must return actual joined retirement, not a native result: {refused:?}");
        let observed = fixture.policy.observe_member_class(&instance("project-refused").member.member).await.unwrap();
        assert_eq!(observed.admission.phase, crate::installed_host::dynamic_members::MemberClassAdmissionPhase::Retired);
        assert_eq!(fixture.native.mob_handle().list_members().await.len(), 1);
        assert_eq!(fixture.factory.preparations.load(Ordering::SeqCst), 0);
        assert_eq!(finalized.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.provider.0.load(Ordering::SeqCst), 0);
    }))).catch_unwind().await;
    fixture.close().await;
    match result {
        Ok(Ok(())) => (),
        Err(panic) => std::panic::resume_unwind(panic),
        _ => panic!("preflight refusal cleanup timed out"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_deadline_joins_entered_publication_then_retires_its_authorization() {
    let captured = Arc::new(std::sync::OnceLock::new());
    let capture = captured.clone();
    let fixture = Box::pin(Fixture::start_with_policy_store(Some(Box::new(
        move |inner| {
            let store = Arc::new(HeldPublicationWrite::new(inner));
            assert!(capture.set(store.clone()).is_ok());
            store
        },
    ))))
    .await;
    let store = captured.get().unwrap().clone();
    // Release stays outside all panicking or timed futures; the native owner
    // retains the callback and its entered CAS until actual settlement.
    let mut caller = None;
    let result = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT * 4,
        Box::pin(async {
            let admitted = fixture.admit("project-aborted").await;
            let context = fixture.native.identity_first_context().unwrap();
            let gateway = fixture.gateway.clone();
            let route = fixture.route("project-aborted");
            let intent = fixture.intent("project-aborted");
            let retained = admitted.clone();
            caller = Some(tokio::spawn(async move {
                gateway
                    .add_member_class(
                        &context,
                        retained,
                        spec("project-aborted", &class().profile),
                        route,
                        intent,
                        tokio::time::Instant::now() + WAIT,
                        || async { Ok(()) },
                    )
                    .await
            }));
            store.entered.notified().await;
            assert_eq!(store.entered_count.load(Ordering::SeqCst), 1);
            assert_eq!(store.returned_count.load(Ordering::SeqCst), 0);
            let started = caller.as_mut().unwrap().await.unwrap().unwrap();
            let _joined = caller.take().unwrap();
            let MemberClassAddStart::Unsettled(settlement) = started else {
                panic!("native deadline cannot activate before its paused publication returns");
            };
            assert_eq!(store.returned_count.load(Ordering::SeqCst), 0);
            assert!(
                fixture
                    .source
                    .member_route(&fixture.route("project-aborted").id)
                    .unwrap()
                    .is_none()
            );
            assert!(fixture.factory.calls.lock().unwrap().is_empty());
            assert_eq!(fixture.provider.0.load(Ordering::SeqCst), 0);
            store.release.add_permits(1);
            let outcome = settlement.settled().await.unwrap();
            assert!(outcome.native.is_err());
            assert!(outcome.authorization.is_err());
            assert!(outcome.cleanup.is_ok());
            assert_eq!(
                store.returned_count.load(Ordering::SeqCst),
                1,
                "entered SQLite CAS was joined"
            );
            assert!(
                fixture
                    .source
                    .member_route(&fixture.route("project-aborted").id)
                    .is_err()
            );
            assert!(matches!(
                fixture
                    .policy
                    .member_class_admission(&instance("project-aborted").member.member),
                Err(HostLifetimeError::Denied)
            ));
            assert_eq!(
                fixture.native.mob_handle().list_members().await.len(),
                1,
                "coordinator remains; refused addition is cleaned"
            );
            let runtime = fixture.native.identity_runtime().unwrap();
            let status = runtime
                .status(&AgentIdentity::parse("project-aborted").unwrap())
                .await;
            assert!(!status.is_ok_and(|status| status.state == IdentityLifecycleState::Active));
            admitted.retire().await.unwrap();
        }),
    ))
    .catch_unwind()
    .await;
    store.release.add_permits(1);
    if let Some(caller) = caller {
        let _joined = caller.await;
    }
    fixture.close().await;
    match result {
        Ok(Ok(())) => (),
        Err(panic) => std::panic::resume_unwind(panic),
        _ => panic!("held abort fixture timed out"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deadline_before_held_callback_retires_prepared_policy_without_listing() {
    let captured = Arc::new(std::sync::OnceLock::new());
    let capture = captured.clone();
    let fixture = Box::pin(Fixture::start_with_policy_store(Some(Box::new(
        move |inner| {
            let store = Arc::new(HeldPublicationWrite::for_event(
                inner,
                "member-policy-current",
            ));
            assert!(capture.set(store.clone()).is_ok());
            store
        },
    ))))
    .await;
    let store = captured.get().unwrap().clone();
    let finalizers = Arc::new(AtomicUsize::new(0));
    let mut caller = None;
    let result = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT * 4,
        Box::pin(async {
            let admitted = fixture.admit("project-aborted").await;
            let context = fixture.native.identity_first_context().unwrap();
            let gateway = fixture.gateway.clone();
            let route = fixture.route("project-aborted");
            let intent = fixture.intent("project-aborted");
            let entered = finalizers.clone();
            caller = Some(tokio::spawn(async move {
                gateway
                    .add_member_class(
                        &context,
                        admitted,
                        spec("project-aborted", &class().profile),
                        route,
                        intent,
                        tokio::time::Instant::now() + WAIT,
                        move || async move {
                            entered.fetch_add(1, Ordering::SeqCst);
                            Ok(())
                        },
                    )
                    .await
            }));
            store.entered.notified().await;
            let started = caller.as_mut().unwrap().await.unwrap().unwrap();
            let _joined = caller.take();
            let MemberClassAddStart::Unsettled(settlement) = started else {
                panic!("construction still held at the deadline cannot activate");
            };
            assert_eq!(finalizers.load(Ordering::SeqCst), 0);
            assert_eq!(store.returned_count.load(Ordering::SeqCst), 0);
            // The entered policy promotion has deliberately withheld its
            // current observation until this CAS returns. That is unavailable,
            // not a successful read of an absent route.
            assert!(matches!(
                fixture
                    .source
                    .member_route(&fixture.route("project-aborted").id),
                Err(PrivateOriginalError::Unavailable)
            ));
            store.release.add_permits(1);
            let outcome = settlement.settled().await.unwrap();
            assert!(outcome.native.is_err());
            assert!(outcome.authorization.is_err());
            assert!(outcome.cleanup.is_ok());
            assert_eq!(
                finalizers.load(Ordering::SeqCst),
                0,
                "NotAsked must not list a desired member"
            );
            assert_eq!(
                store.returned_count.load(Ordering::SeqCst),
                1,
                "entered preparation CAS joined"
            );
            assert!(
                fixture
                    .source
                    .member_route(&fixture.route("project-aborted").id)
                    .unwrap()
                    .is_none(),
                "joined retirement leaves no projected route"
            );
            assert!(matches!(
                fixture
                    .policy
                    .member_class_admission(&instance("project-aborted").member.member),
                Err(HostLifetimeError::Denied)
            ));
            assert_eq!(fixture.native.mob_handle().list_members().await.len(), 1);
            assert!(fixture.factory.calls.lock().unwrap().is_empty());
            assert_eq!(fixture.provider.0.load(Ordering::SeqCst), 0);
        }),
    ))
    .catch_unwind()
    .await;
    store.release.add_permits(1);
    if let Some(caller) = caller {
        let _ = caller.await;
    }
    fixture.close().await;
    match result {
        Ok(Ok(())) => (),
        Err(panic) => std::panic::resume_unwind(panic),
        _ => panic!("pre-callback expiry settlement timed out"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lost_observer_and_host_close_join_entered_finalizer_before_cleanup() {
    let fixture = Box::pin(Fixture::start()).await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let returned = Arc::new(AtomicUsize::new(0));
    let mut caller = None;
    let result = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT * 4,
        Box::pin(async {
            let admitted = fixture.admit("project-aborted").await;
            let duplicate_admitted = admitted.clone();
            let context = fixture.native.identity_first_context().unwrap();
            let gateway = fixture.gateway.clone();
            let route = fixture.route("project-aborted");
            let intent = fixture.intent("project-aborted");
            let notice = entered.clone();
            let released = release.clone();
            let count = returned.clone();
            caller = Some(tokio::spawn(async move {
                gateway
                    .add_member_class(
                        &context,
                        admitted,
                        spec("project-aborted", &class().profile),
                        route,
                        intent,
                        tokio::time::Instant::now() + WAIT * 2,
                        move || async move {
                            notice.notify_one();
                            released
                                .acquire()
                                .await
                                .map_err(|_| HostLifetimeError::Unavailable)?
                                .forget();
                            count.fetch_add(1, Ordering::SeqCst);
                            Ok(())
                        },
                    )
                    .await
            }));
            entered.notified().await;
            let before_duplicate = fixture
                .policy
                .member_class_admission(&instance("project-aborted").member.member)
                .unwrap()
                .unwrap();
            let standing = before_duplicate
                .standing_operations
                .as_ref()
                .unwrap()
                .clone();
            let parents = standing.issued.as_ref().unwrap().clone();
            let grant_context = fixture.source.callback_grant_context().unwrap();
            let duplicate_context = fixture.native.identity_first_context().unwrap();
            let duplicate_calls = returned.clone();
            let refused = fixture
                .gateway
                .add_member_class(
                    &duplicate_context,
                    duplicate_admitted,
                    spec("project-aborted", &class().profile),
                    fixture.route("project-aborted"),
                    fixture.intent("project-aborted"),
                    tokio::time::Instant::now() + WAIT,
                    move || async move {
                        duplicate_calls.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    },
                )
                .await;
            assert!(
                matches!(
                    refused,
                    Err(MemberClassAddError::ReservationNotAccepted {
                        error: HostLifetimeError::Denied
                    })
                ),
                "in-progress duplicate cannot retire the accepted callback: {refused:?}"
            );
            assert_eq!(
                fixture
                    .policy
                    .member_class_admission(&instance("project-aborted").member.member)
                    .unwrap()
                    .unwrap(),
                before_duplicate
            );
            assert_eq!(returned.load(Ordering::SeqCst), 0);
            assert!(
                fixture
                    .source
                    .member_route(&fixture.route("project-aborted").id)
                    .unwrap()
                    .is_none()
            );
            assert_eq!(fixture.native.mob_handle().list_members().await.len(), 1);
            // Only the observation is cancelled. The host's accepted task and the
            // native callback retain the real finalizer until it actually returns.
            let observation = caller.take().unwrap();
            observation.abort();
            assert!(observation.await.unwrap_err().is_cancelled());
            fixture
                .gateway
                .begin_close(GatewayHostCloseReason::Shutdown)
                .unwrap();
            let close = fixture.gateway.join_close();
            tokio::pin!(close);
            assert!(
                tokio::time::timeout(Duration::from_millis(40), &mut close)
                    .await
                    .is_err()
            );
            assert_eq!(returned.load(Ordering::SeqCst), 0);
            for root in [&parents.peer_message, &parents.recorder_write] {
                grant_context
                    .grants
                    .resolve_lineage(
                        std::slice::from_ref(root),
                        &grant_context.application,
                        before_duplicate
                            .route
                            .as_ref()
                            .unwrap()
                            .represented_subject
                            .as_ref(),
                    )
                    .unwrap();
            }
            release.add_permits(1);
            close.await.unwrap();
            let retired = fixture
                .policy
                .observe_member_class(&instance("project-aborted").member.member)
                .await
                .unwrap();
            assert_eq!(
                retired.admission.standing_operations.as_ref(),
                Some(&standing)
            );
            assert_eq!(retired.authorization, Err(HostLifetimeError::Denied));
            for root in [&parents.peer_message, &parents.recorder_write] {
                assert_eq!(
                    grant_context
                        .grants
                        .issued_reference(&root.grant_id)
                        .unwrap()
                        .as_ref(),
                    Some(root)
                );
                assert!(matches!(
                    grant_context.grants.resolve_lineage(
                        std::slice::from_ref(root),
                        &grant_context.application,
                        before_duplicate
                            .route
                            .as_ref()
                            .unwrap()
                            .represented_subject
                            .as_ref()
                    ),
                    Err(meerkat_authorization::grants::GrantRefusal::Denied)
                ));
            }
            assert_eq!(
                returned.load(Ordering::SeqCst),
                1,
                "entered finalizer was joined, not dropped"
            );
            assert_eq!(fixture.native.mob_handle().list_members().await.len(), 1);
            assert!(
                fixture
                    .source
                    .member_route(&fixture.route("project-aborted").id)
                    .is_err(),
                "closed owner grants no source access"
            );
            assert!(fixture.factory.calls.lock().unwrap().is_empty());
            assert_eq!(fixture.provider.0.load(Ordering::SeqCst), 0);
        }),
    ))
    .catch_unwind()
    .await;
    release.add_permits(1);
    if let Some(caller) = caller {
        let _ = caller.await;
    }
    fixture.close().await;
    match result {
        Ok(Ok(())) => (),
        Err(panic) => std::panic::resume_unwind(panic),
        _ => panic!("lost-observer finalizer cleanup timed out"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn admission_fence_preserves_entered_member_finalizer_and_refuses_fresh_add() {
    let fixture = Box::pin(Fixture::start()).await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let returned = Arc::new(AtomicUsize::new(0));
    let mut caller = None;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT * 4,
        Box::pin(async {
            let admitted = fixture.admit(ADDED).await;
            // Reserve a policy-permitted identity before fencing, so the later
            // refusal proves stopped admission rather than the fixture allowlist.
            let refused_admission = fixture.admit("project-refused").await;
            let before = fixture
                .policy
                .current_member_class(&refused_admission)
                .unwrap();
            let context = fixture.native.identity_first_context().unwrap();
            fixture
                .gateway
                .bind_member_class_context(context.clone())
                .unwrap();
            let gateway = fixture.gateway.clone();
            let route = fixture.route(ADDED);
            let intent = fixture.intent(ADDED);
            let notice = entered.clone();
            let released = release.clone();
            let count = returned.clone();
            caller = Some(tokio::spawn(async move {
                gateway
                    .add_member_class(
                        &context,
                        admitted,
                        spec(ADDED, &class().profile),
                        route,
                        intent,
                        tokio::time::Instant::now() + WAIT * 2,
                        move || async move {
                            notice.notify_one();
                            released
                                .acquire()
                                .await
                                .map_err(|_| HostLifetimeError::Unavailable)?
                                .forget();
                            count.fetch_add(1, Ordering::SeqCst);
                            Ok(())
                        },
                    )
                    .await
            }));
            entered.notified().await;
            GatewayHostComposition::stop_admitting(
                fixture.gateway.as_ref(),
                GatewayHostCloseReason::Shutdown,
            )
            .unwrap();
            GatewayHostComposition::stop_admitting(
                fixture.gateway.as_ref(),
                GatewayHostCloseReason::Shutdown,
            )
            .unwrap();
            assert!(
                matches!(
                    fixture.gateway.admit_member_class_context(),
                    Err(HostLifetimeError::Unavailable)
                ),
                "new private Add must refuse before reserve/readmit"
            );
            let context = fixture.native.identity_first_context().unwrap();
            let refused = fixture
                .gateway
                .add_member_class(
                    &context,
                    refused_admission.clone(),
                    spec("project-refused", &class().profile),
                    fixture.route("project-refused"),
                    fixture.intent("project-refused"),
                    tokio::time::Instant::now() + WAIT,
                    || async { panic!("fenced Add must not enter its finalizer") },
                )
                .await;
            assert!(
                matches!(
                    refused,
                    Err(MemberClassAddError::ReservationNotAccepted {
                        error: HostLifetimeError::Unavailable,
                    })
                ),
                "{refused:?}"
            );
            assert_eq!(
                fixture
                    .policy
                    .current_member_class(&refused_admission)
                    .unwrap(),
                before
            );
            fixture
                .gateway
                .retire_unaccepted_member_class(refused_admission)
                .await
                .unwrap();
            assert_eq!(
                returned.load(Ordering::SeqCst),
                0,
                "admission fence does not cancel entered work"
            );
            release.add_permits(1);
            let started = caller.take().unwrap().await.unwrap().unwrap();
            let outcome = match started {
                MemberClassAddStart::Settled(outcome) => outcome,
                MemberClassAddStart::Unsettled(settlement) => settlement.settled().await.unwrap(),
            };
            assert!(
                outcome.authorization.is_ok() && outcome.cleanup.is_ok(),
                "{outcome:?}"
            );
            let native = outcome.native.unwrap();
            assert!(
                matches!(
                    native.outcomes.get(&AgentIdentity::parse(ADDED).unwrap()),
                    Some(RestoreOutcome::Created { .. })
                ),
                "{native:?}"
            );
            assert_eq!(returned.load(Ordering::SeqCst), 1);
            assert!(
                fixture
                    .source
                    .member_route(&fixture.route(ADDED).id)
                    .unwrap()
                    .is_some()
            );
            assert_eq!(fixture.factory.preparations.load(Ordering::SeqCst), 1);
            assert!(fixture.factory.calls.lock().unwrap().is_empty());
        }),
    ))
    .catch_unwind()
    .await;
    release.add_permits(1);
    if let Some(caller) = caller {
        let _ = caller.await;
    }
    fixture.close().await;
    match checked {
        Ok(Ok(())) => (),
        Err(panic) => std::panic::resume_unwind(panic),
        _ => panic!("admission-only member drain control timed out"),
    }
}

#[path = "dynamic_member_native_tests/setup_review.rs"]
mod setup_review;
#[path = "dynamic_member_native_tests/unaccepted_retirement.rs"]
mod unaccepted_retirement;
#[path = "dynamic_member_native_tests/unsettled_resource_cleanup.rs"]
mod unsettled_resource_cleanup;
#[path = "dynamic_member_native_tests/workflow.rs"]
mod workflow;

#[path = "dynamic_member_native_tests/native_operations_tests.rs"]
mod native_operations_tests;

#[path = "dynamic_member_native_tests/peer_ingress_tests.rs"]
mod peer_ingress_tests;

#[path = "dynamic_member_native_tests/cold_source_coordinates.rs"]
mod cold_source_coordinates;

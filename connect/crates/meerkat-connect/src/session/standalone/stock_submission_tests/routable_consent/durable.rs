//! Canonical 205 uses installed durable source custody, not stock's
//! process-local originals. Only the transport peer is scripted.
use super::*;
use meerkat_core::{AgentToolDispatcher, ControllerModelClient, MobMemberBinding};
use meerkat_mobkit::rpc_gateway::GatewayHostComposition;
use meerkat_mobkit::{
    DiscoverySpec, MetaConfig, MobBootstrapSpec, MobTerminalShutdownOutcome, UnifiedRuntime,
    identity_first::{
        AgentIdentity, DurabilityPolicy, DurableAgentSpec, IdentityFirstRuntimeContext,
        IdentityRuntime, IdentityRuntimeConfig, LocalContinuityStore, LocalLeaseProvider,
        MobSessionBridge, MutableRosterProvider, RestoreOutcome, TurnOutcome, TurnOutput,
        TurnTicket,
    },
};
use meerkat_toolkit::host_lifetime::control::HostBootstrapSelection;
use std::{os::fd::IntoRawFd, os::unix::net::UnixStream};
use toolkit_mobkit::{
    installed_host::retained_consent_test_support as support,
    private_host::{InheritedHostPipes, PrivateHostConnection, PrivateHostStartup},
    private_originals::{
        PrivateAuthenticatedEvent, PrivateOriginalSubmission, PrivateOriginalSubmit,
        PrivateSourceEvidence, PrivateSourceSettlement,
    },
};

const MOB: &str = "connect-consent-census";
const MEMBER: &str = "consent-agent";
const DURABLE_CHILD: &str = "session::standalone::stock_submission_tests::routable_consent::durable::durable_consent_predecessor_child";

fn member() -> MobMemberBinding {
    MobMemberBinding {
        mob_id: MOB.into(),
        role: "worker".into(),
        member: MEMBER.into(),
    }
}
fn selected_storage(root: &Path) -> meerkat_core::ResolvedStorage {
    crate::RealmOptions {
        context_root: root.to_owned(),
        state_root: Some(root.join("state")),
        user_config_root: Some(root.join("home")),
    }
    .resolve(meerkat_core::mob_realm_id(MOB).unwrap().as_str())
    .unwrap()
}

// Create and transfer these exact new private descriptors before constructing
// this control's Tokio runtime. No descriptor number comes from wire data.
struct Pipes {
    child: InheritedHostPipes,
    parent: UnixStream,
}
impl Pipes {
    fn new() -> Self {
        let (parent, child) = UnixStream::pair().unwrap();
        let output = child.try_clone().unwrap();
        let input = child.into_raw_fd();
        let output = output.into_raw_fd();
        Self {
            child: InheritedHostPipes::adopt(input, output).unwrap(),
            parent,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct DurableReady {
    common: Ready,
    installed: HostBootstrapSelection,
    originals: Vec<PrivateOriginalSubmit>,
    source_rows: Vec<toolkit_store::StoredRecord>,
    model_address: std::net::SocketAddr,
}

struct DurableNative {
    service: Arc<Service>,
    machine: Arc<MeerkatMachine>,
    store: Arc<dyn RuntimeStore>,
    session: Option<meerkat_core::SessionId>,
    control: support::RetainedConsentControl,
    connection: Option<PrivateHostConnection>,
    parent_pipe: Option<tokio::net::UnixStream>,
    router: Arc<meerkat::McpRouterAdapter>,
    spec: Option<MobBootstrapSpec>,
    native: Option<UnifiedRuntime>,
    owner: meerkat::RuntimeDeliveryOwner,
    host: Arc<meerkat::surface::SessionServiceDeliveryHost<Service>>,
    delivery: Option<meerkat::RuntimeDeliveryOwnerHandle>,
    retired: Option<meerkat::RuntimeDeliveryOwnerRetired>,
    selected: HostBootstrapSelection,
}
// Construction retains actual owners even if an assertion fails before the
// complete runtime is returned. Cleanup is awaited before resuming that panic.
#[derive(Default)]
struct Opening {
    raw_router: Option<meerkat::McpRouter>,
    router: Option<Arc<meerkat::McpRouterAdapter>>,
    control: Option<support::RetainedConsentControl>,
    connection: Option<PrivateHostConnection>,
    parent: Option<tokio::net::UnixStream>,
    service: Option<Arc<Service>>,
}
impl Opening {
    async fn close(&mut self) -> Vec<&'static str> {
        let mut errors = Vec::new();
        if let Some(control) = &self.control {
            if control.gateway.stop_admitting_work().is_err() {
                errors.push("opening stop admission");
            }
            if control
                .gateway
                .begin_close(meerkat_mobkit::rpc_gateway::GatewayHostCloseReason::Shutdown)
                .is_err()
            {
                errors.push("opening begin close");
            }
            if control.gateway.join_close().await.is_err() {
                errors.push("opening installed join");
            }
        }
        self.parent.take();
        if let Some(connection) = self.connection.take()
            && connection.finish().await.is_err()
        {
            errors.push("opening private join");
        }
        if let Some(router) = &self.router {
            match router.shutdown_with_report().await {
                meerkat_mcp::McpRouterShutdownObservation::Reported(r)
                    if r.is_settled() && r.guards_completed() => {}
                _ => errors.push("opening adapter join"),
            }
        }
        if let Some(router) = self.raw_router.take() {
            let report = router.shutdown_with_report().await;
            if !report.is_settled() || !report.guards_completed() {
                errors.push("opening router join");
            }
        }
        if let Some(service) = &self.service
            && service.try_shutdown().await.is_err()
        {
            errors.push("opening service join");
        }
        errors
    }
}
impl DurableNative {
    async fn open(
        root: &Path,
        pipes: Pipes,
        consent: &Arc<ApplicationConsentOwner>,
        server: &meerkat_core::McpServerConfig,
        selected: Option<HostBootstrapSelection>,
        session: Option<meerkat_core::SessionId>,
    ) -> Self {
        let mut opening = Opening::default();
        let opened = AssertUnwindSafe(async {
        let storage = selected_storage(root);
        let startup = SessionStartup::open(storage.clone()).await.unwrap();
        let config = startup.config.clone();
        assert!(config.tools.mcp_servers.is_empty(), "the fixture selects exactly one trusted host router");
        let factory = startup.factory.with_operation_review(issuer::review(consent));
        opening.raw_router = Some(meerkat::McpRouter::new_with_surface_handle_and_stdio_profile(
            Arc::new(meerkat_runtime::RuntimeExternalToolSurfaceHandle::ephemeral()),
            meerkat::McpStdioLaunchProfile::trusted_host()));
        opening.raw_router.as_mut().unwrap().add_server(server.clone()).await.unwrap();
        let router = Arc::new(meerkat::McpRouterAdapter::new(opening.raw_router.take().unwrap()));
        opening.router = Some(router.clone());
        assert!(router.tools().iter().any(|tool| tool.name == "setup_help"));
        let selected = match selected {
            Some(selected) => selected,
            None => {
                // Prepare the real factory composition with the same typed
                // member and external router. Abort joins that construction;
                // no prompt, agent run or original input is created.
                let captured: Arc<std::sync::Mutex<Option<ControllerModelClient>>> = Default::default();
                let sink = captured.clone();
                let mut build = meerkat::AgentBuildConfig::new(MODEL.to_owned());
                build.mob_member_binding = Some(member());
                build.comms_name = Some(member().comms_name().unwrap().to_string());
                build.realm_id = Some(storage.locator.realm.clone());
                build.provider = Some(meerkat_core::Provider::Anthropic);
                build.auth_binding = native_model_peer::model_request(storage.locator.realm.as_str(), MODEL).unwrap().auth_binding;
                build.external_tools = Some(router.clone());
                build.override_builtins = meerkat_core::ToolCategoryOverride::Disable;
                build.override_shell = meerkat_core::ToolCategoryOverride::Disable;
                build.override_memory = meerkat_core::ToolCategoryOverride::Disable;
                build.override_schedule = meerkat_core::ToolCategoryOverride::Disable;
                build.override_workgraph = meerkat_core::ToolCategoryOverride::Disable;
                build.override_mob = meerkat_core::ToolCategoryOverride::Disable;
                build.override_image_generation = meerkat_core::ToolCategoryOverride::Disable;
                build.override_web_search = meerkat_core::ToolCategoryOverride::Disable;
                build.override_comms = meerkat_core::ToolCategoryOverride::Enable;
                build.agent_llm_client_decorator = Some(Arc::new(move |client| {
                    let pin = client.clone().pin_controller().expect("actual factory-selected client supports controller facts");
                    *sink.lock().unwrap() = Some(pin);
                    client
                }));
                // An empty registry is only preparation plumbing. No policy
                // decision/finish or native tool call occurs in this assembly.
                build.tool_consequence_policy_registry = Some(Arc::new(meerkat_core::ToolConsequencePolicyRegistry::new(
                    vec![], Default::default(), None).unwrap()));
                let prepared = factory.prepare_agent_with_controller(build, &config).await.unwrap();
                let surface = prepared.tool_surface().clone();
                prepared.abort().await;
                let controller = captured.lock().unwrap().take().unwrap();
                support::provision(root, member(), &surface, &controller).unwrap()
            },
        };
        let Pipes { child, parent } = pipes;
        parent.set_nonblocking(true).unwrap();
        let mut parent = tokio::net::UnixStream::from_std(parent).unwrap();
        let request = serde_json::json!({"version":1,"id":1,"command":{"kind":"bootstrap","selection":selected}});
        let handshake = async {
            parent.write_all(format!("{request}\n").as_bytes()).await.map_err(|e| e.to_string())?;
            let mut reply = String::new();
            tokio::io::BufReader::new(&mut parent).read_line(&mut reply).await.map_err(|e| e.to_string())?;
            let reply: serde_json::Value = serde_json::from_str(&reply).map_err(|e| e.to_string())?;
            if reply["version"] != 1 || reply["id"] != 1 || reply["result"]["kind"] != "bootstrap_accepted" {
                return Err("actual private bootstrap response required".to_owned());
            }
            Ok::<_, String>(parent)
        };
        let install = async {
            let startup = PrivateHostStartup::read(child).await.map_err(|e| e.to_string())?;
            let (control, connection) = support::bootstrap(startup, Arc::new(support::RetainedConsentReview)).await?;
            // Retain real owners before returning to join: a sibling handshake
            // failure must not drop an unjoined successful connection.
            opening.control = Some(control); opening.connection = Some(connection);
            Ok::<_, String>(())
        };
        let (installed, parent) = tokio::join!(install, handshake);
        match parent { Ok(parent) => opening.parent = Some(parent), Err(error) => panic!("private handshake: {error}") }
        installed.unwrap();
        let control = opening.control.as_ref().unwrap();
        let configuration = control.gateway.native_grant_work_configuration().unwrap().unwrap();
        // Open the shared session schema before activating its native
        // head-canonical profile. Reopen uses the same case-owned hosting paths.
        let native_root = root.canonicalize().unwrap();
        let native_path = native_root.join("native.sqlite3");
        let session_store = Arc::new(meerkat::SqliteSessionStore::open(&native_path).unwrap());
        let hosting_lock_dir = native_root.join("native-hosting");
        std::fs::create_dir_all(&hosting_lock_dir).unwrap();
        let runtime_store = meerkat_runtime::store::SqliteRuntimeStore::new_head_canonical(native_path).unwrap()
            .with_hosting_paths(meerkat_core::session_hosting::HostingPaths {
                hosting_lock_dir, cold_delivery_lock: native_root.join("native-cold-delivery.lock"), database: None,
            });
        assert!(runtime_store.hosting_capability().domain_ref().is_some(),
            "durable consent requires an actual cross-process hosting domain");
        let persistence = meerkat::PersistenceBundle::new_with_local_grant_authorization(
            session_store, Arc::new(runtime_store),
            Arc::new(meerkat::MemoryBlobStore::new()), configuration).unwrap();
        let store = persistence.runtime_store();
        assert_eq!(store.session_persistence_profile(), RuntimeSessionPersistenceProfile::HeadCanonicalV1);
        let owner = persistence.runtime_delivery_owner();
        let inbox = persistence.runtime_delivery_inbox();
        let bindings = persistence.continuation_bindings();
        let domain = inbox.hosting_capability().domain_ref().unwrap();
        let mut builder = FactoryAgentBuilder::new(factory, config.clone());
        builder.wait_for_mcp = true;
        let (service, machine) = meerkat::surface::build_runtime_backed_service_with_capacities(builder, 2, 2, persistence).unwrap();
        let service = Arc::new(service);
        opening.service = Some(service.clone());
        let host = Arc::new(meerkat::surface::SessionServiceDeliveryHost::new(&service, &machine,
            meerkat::DetachedJobService::new(owner.job_store()), Some(storage.locator.realm.to_string()))
            .with_continuation_bindings(bindings.clone())
            .with_consent_notices(Arc::new(meerkat::ConsentNoticeProjector::new(Arc::new(consent.approvals()), domain, inbox, &machine))));
        bindings.announce_runless_input_terminals_of(&machine);
        let owner = owner.with_attachment_commits(machine.subscribe_attachment_commits())
            .with_run_settlements(machine.subscribe_run_settlements()).with_source_changes(bindings.subscribe_source_changes());
        let definition: meerkat_mob::MobDefinition = serde_json::from_value(serde_json::json!({"id":MOB,"profiles":{
            "worker":{"model":MODEL,"provider":"anthropic","tools":{"comms":true},
                "external_addressable":true,"runtime_mode":"turn_driven"}
        }})).unwrap();
        let (mob_store, provenance) = meerkat_mobkit::mob_composition_manifest::persistent_mob_storage(root.join("mob.sqlite3")).unwrap();
        let selected_router = router.clone();
        let mut spec = MobBootstrapSpec::new(definition, mob_store, service.clone())
            .with_member_model_config(Arc::new(config))
            .with_session_runtime_adapter(machine.clone()).unwrap()
            .with_mob_storage_provenance(provenance).with_child_mob_storage(meerkat_mobkit::ChildMobStorage::ProcessBound)
            .with_default_external_tools_provider(Arc::new(move || Some(selected_router.clone() as Arc<dyn AgentToolDispatcher>)));
        // Declare the same user-global identity root selected by realm_factory.
        if let Some(root) = storage.layout.user_home_root() {
            spec = spec.with_session_identity_config_root(root);
        }
        // Capture the actual concrete service before the bootstrap erases it.
        // The gateway uses this same public native pin source for cold custody.
        spec.controller_pin_source = Some(service.clone());
        let spec = control.gateway.prepare_bootstrap(spec).await.unwrap();
        (service, machine, store, router, spec, owner, host, selected)
        }).catch_unwind().await;
        match opened {
            Ok((service, machine, store, router, spec, owner, host, selected)) => Self {
                service,
                machine,
                store,
                session,
                control: opening.control.take().unwrap(),
                connection: opening.connection.take(),
                parent_pipe: opening.parent.take(),
                router,
                spec: Some(spec),
                native: None,
                owner,
                host,
                delivery: None,
                retired: None,
                selected,
            },
            Err(panic) => {
                let errors = opening.close().await;
                if !errors.is_empty() {
                    let _ = writeln!(
                        std::io::stderr(),
                        "durable construction cleanup: {errors:?}"
                    );
                }
                resume_unwind(panic)
            }
        }
    }
    fn id(&self) -> &meerkat_core::SessionId {
        self.session.as_ref().unwrap()
    }
    async fn attach(&mut self, root: &Path, cold: bool) {
        if let Some(session) = &self.session {
            self.machine
                .stage_registration_run_start_hold_only(session, RunStartHoldReason::MobStop);
        }
        let spec = self.spec.take().unwrap();
        self.native = Some(
            UnifiedRuntime::bootstrap(
                spec,
                MetaConfig {
                    modules: vec![],
                    discovery: DiscoverySpec {
                        namespace: MOB.into(),
                        modules: vec![],
                    },
                    pre_spawn: vec![],
                },
                BOUND,
            )
            .await
            .unwrap(),
        );
        let native = self.native.as_mut().unwrap();
        self.control.gateway.bind_runtime(native).await.unwrap();
        let handle = native.mob_handle();
        let runtime = Arc::new(
            IdentityRuntime::new(IdentityRuntimeConfig {
                continuity_store: Arc::new(
                    LocalContinuityStore::open(root.join("continuity.sqlite3")).unwrap(),
                ),
                lease_provider: Arc::new(LocalLeaseProvider::new()),
                runtime_instance_id: format!("durable-consent-{}", std::process::id()),
                has_runtime_store: true,
                durability_policy: DurabilityPolicy::SyncWriteThrough,
                bridge: Some(Arc::new(
                    MobSessionBridge::with_session_service(
                        handle.clone(),
                        native.mob_runtime().session_service().cloned().unwrap(),
                    )
                    .with_controller_pin_source(self.service.clone()),
                )),
                default_timeout: None,
            })
            .with_runtime_services(
                meerkat_mobkit::identity_first::AgentRuntimeServices::new(handle.clone()),
            ),
        );
        let member: DurableAgentSpec = serde_json::from_value(serde_json::json!({"identity":MEMBER,"profile":"worker", "initial_message":null,"runtime_mode_override":"turn_driven"})).unwrap();
        assert!(
            member.initial_message.is_none(),
            "restore must not create a fourth prompt"
        );
        let identity = AgentIdentity::parse(MEMBER).unwrap();
        let context = Arc::new(IdentityFirstRuntimeContext::new(
            runtime,
            Arc::new(MutableRosterProvider::new(vec![member.clone()])),
            None,
            None,
            Some(handle.definition().clone()),
        ));
        let restored = native
            .install_and_bootstrap_identity_first_context(context, &[member])
            .await
            .unwrap();
        match (&restored.outcomes[&identity], cold) {
            (RestoreOutcome::Created { .. }, false) | (RestoreOutcome::Resumed { .. }, true) => (),
            other => panic!("actual durable member create/restore required: {other:?}"),
        }
        let session = native
            .identity_runtime()
            .unwrap()
            .status(&identity)
            .await
            .unwrap()
            .session_id
            .unwrap();
        if let Some(old) = &self.session {
            assert_eq!(old, &session);
        }
        self.session = Some(session.clone());
        self.control
            .gateway
            .bind_identity_runtime(native)
            .await
            .unwrap();
        let identity_runtime = native.identity_runtime().unwrap();
        let inputs_before_wake = self.input_ids().await;
        // Created/Resumed identifies the durable seat, which may be dormant.
        // Materialize the selected fixture member through its actual owner;
        // this must preserve its session and create no prompt or native input.
        let controller = identity_runtime
            .materialize_tracked(&identity)
            .await
            .unwrap()
            .expect("the fixture member has a native session");
        assert_eq!(
            controller.session_id(),
            &session,
            "wake preserves the durable session"
        );
        assert_eq!(
            self.input_ids().await,
            inputs_before_wake,
            "wake creates no native input"
        );
        assert!(
            self.machine
                .current_executor_attachment_witness(&session)
                .await
                .is_some()
        );
        // The service publishes the actual newly attached actor's controller;
        // no DTO, synthetic lifecycle token, or old client is reinstated.
        let lease = self
            .service
            .acquire_live_session_actor_turn_boundary_lease(&session)
            .await
            .unwrap();
        let _controller = self
            .service
            .pin_controller_client_for_actor(&lease)
            .await
            .unwrap();
        drop(lease);
        if cold {
            assert!(
                self.machine
                    .hold_run_starts(&session, RunStartHoldReason::MobStop)
                    .await
                    .unwrap()
                    .current_run
                    .is_none()
            );
        }
    }
    fn arm(&mut self) {
        assert!(self.delivery.is_none());
        self.delivery = Some(self.owner.clone().arm(self.host.clone()).unwrap());
    }
    async fn retire_delivery(&mut self) -> Result<(), String> {
        if let Some(owner) = &self.delivery {
            let mut expired = false;
            loop {
                match owner.retire().join(BOUND).await {
                    Ok(report) => {
                        self.retired = Some(report);
                        break;
                    }
                    Err(_) => expired = true,
                }
            }
            self.delivery.take();
            if expired {
                return Err("same actual notice owner joined after exceeding deadline".into());
            }
        }
        Ok(())
    }
    async fn terminal(&self, ids: &HashSet<meerkat_core::lifecycle::InputId>) {
        for id in ids {
            let actual = self
                .machine
                .input_terminal_receipt_observation(
                    self.id(),
                    InteractionSelector::InputId(id.clone()),
                )
                .await
                .unwrap()
                .unwrap();
            assert_eq!(actual.source, TerminalWitnessSource::DurableStore);
            let InputTerminalReceiptObservation::Finalized(receipt) = actual.report else {
                panic!("actual final receipt required")
            };
            assert_eq!(receipt.input_id(), id);
            let recipients = receipt.recipient_input_ids();
            assert!(
                recipients.contains(id),
                "the observed input belongs to its actual terminal batch"
            );
            assert_eq!(
                recipients.first(),
                Some(receipt.owner_input_id()),
                "the first recipient owns the batch"
            );
            assert!(
                recipients.iter().all(|recipient| ids.contains(recipient)),
                "the terminal batch contains only inputs in the exact observed census"
            );
        }
    }
    async fn close(&mut self) -> Result<(), String> {
        let mut errors = Vec::new();
        if self.retire_delivery().await.is_err() {
            errors.push("delivery retirement deadline");
        }
        if self.control.gateway.stop_admitting_work().is_err() {
            errors.push("stop installed admission");
        }
        if let Some(native) = &self.native {
            let report = native.shutdown().await;
            let terminal = match &report.mob_terminal_shutdown {
                MobTerminalShutdownOutcome::Completed(r) => r.is_clean(),
                MobTerminalShutdownOutcome::AlreadyShutDown => true,
                _ => false,
            };
            if !report.cleanup_completed() || !terminal {
                errors.push("native shutdown");
            }
        }
        if self
            .control
            .gateway
            .begin_close(meerkat_mobkit::rpc_gateway::GatewayHostCloseReason::Shutdown)
            .is_err()
        {
            errors.push("begin installed close");
        }
        if self.control.gateway.join_close().await.is_err() {
            errors.push("installed close");
        }
        self.parent_pipe.take();
        if let Some(connection) = self.connection.take()
            && connection.finish().await.is_err()
        {
            errors.push("private connection join");
        }
        match self.router.shutdown_with_report().await {
            meerkat_mcp::McpRouterShutdownObservation::Reported(report)
                if report.is_settled() && report.guards_completed() => {}
            _ => errors.push("actual MCP router shutdown"),
        }
        if self.service.try_shutdown().await.is_err() {
            errors.push("actual service join");
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(format!("durable cleanup: {errors:?}"))
        }
    }
}

impl DurableNative {
    async fn retain(&mut self, number: usize, content: &str) -> PrivateOriginalSubmit {
        use meerkat_authorization_contracts::evidence::EvidenceId;
        use sha2::Digest as _;
        let digest = |bytes: &[u8]| format!("{:x}", sha2::Sha256::digest(bytes));
        let evidence = |kind: &str| PrivateSourceEvidence {
            resource_id: EvidenceId::new(format!("{kind}-{number}")).unwrap(),
            revision: EvidenceId::new("one").unwrap(),
            sha256: digest(format!("{kind}-{number}").as_bytes()),
        };
        // These are this real trusted local application's source observations.
        // The installed route supplies every principal/domain/grant. Each
        // occurrence is retained through the actual private channel first.
        let event = PrivateAuthenticatedEvent {
            route: toolkit_package::Id::new("console").unwrap(),
            original: EvidenceId::new(format!("original-{number}")).unwrap(),
            occurrence: EvidenceId::new(format!("occurrence-{number}")).unwrap(),
            original_authentication: evidence("authenticated"),
            root_event: evidence("event"),
            contributors: vec![],
            content_sha256: digest(content.as_bytes()),
        };
        let authentication = event.original_authentication.clone();
        let request = serde_json::json!({"version":1,"id":number+2,
            "command":{"kind":"retain_authenticated_event","event":event}});
        let pipe = self.parent_pipe.as_mut().unwrap();
        pipe.write_all(format!("{request}\n").as_bytes())
            .await
            .unwrap();
        let mut reply = String::new();
        tokio::io::BufReader::new(pipe)
            .read_line(&mut reply)
            .await
            .unwrap();
        let reply: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(reply["id"], number + 2);
        assert_eq!(reply["result"]["kind"], "authenticated_event_retained");
        assert_eq!(
            reply["result"]["state"],
            "retained_pending_native_admission"
        );
        PrivateOriginalSubmit {
            original: serde_json::from_value(reply["result"]["original"].clone()).unwrap(),
            content: content.into(),
            transport_authentication: authentication,
        }
    }
    async fn input_ids(&self) -> HashSet<meerkat_core::lifecycle::InputId> {
        self.store
            .load_input_states_strict(&LogicalRuntimeId::for_session(self.id()))
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.state.input_id)
            .collect()
    }
}

#[test]
#[ignore = "private child of canonical durable consent census"]
fn durable_consent_predecessor_child() {
    let pipes = Pipes::new();
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
        .block_on(predecessor(pipes));
}
async fn predecessor(pipes: Pipes) {
    let parent = PathBuf::from(std::env::var_os(ROOT_ENV).expect("parent-owned root"));
    // The consent issuer refuses a parent with group or other access. Set
    // this owned root's mode instead of inheriting the process umask.
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(&parent)
        .unwrap();
    let root = directory.path().canonicalize().unwrap();
    let client = Arc::new(ScriptedProvider::default());
    let observed = client.clone();
    let peer = ModelPeer::open(
        MODEL,
        Arc::new(move |body| Box::pin(std::future::ready(observed.reply(body)))),
    )
    .await
    .unwrap();
    let mut runtime: Option<DurableNative> = None;
    let mut issuer_owner = None;
    let caught: Result<(), _> = AssertUnwindSafe(async {
        let storage = selected_storage(&root);
        peer.configure(&storage).await.unwrap();
        // The exact selected physical peer is shared by preparation and the
        // member. It declares no account; operation review still gates calls.
        let peer_source = format!("printf '%s\\n' \"$$\" >> \"$1.pids\"\n{MCP_PEER}");
        let mut server = meerkat_core::McpServerConfig::stdio("toolkit-setup", "/bin/sh",
            vec!["-c".into(), peer_source, "durable-consent-peer".into(), root.join("unexpected-mcp-calls").display().to_string()], Default::default());
        server.account = Some(meerkat_core::mcp_config::McpAccountDeclaration::NoAccount);
        let identity = issuer::identity_for_realm(storage.locator.realm.clone());
        let owner = ApplicationConsentOwner::create(&root, identity.clone()).unwrap();
        issuer_owner = Some(owner.clone());
        runtime = Some(DurableNative::open(&root, pipes, &owner, &server, None, None).await);
        let native = runtime.as_mut().unwrap();
        native.attach(&root, false).await;
        let id = native.id().clone();
        assert!(native.input_ids().await.is_empty(), "initial_message=None creates no original");
        let claim = native.machine.grant_session_hosting(&id).unwrap();
        assert!(claim.is_cross_process());
        let association = owner.bind_association(issuer::artifact(&root.join("association.json"),
            b"{\"fixture\":\"three actual durable native R3 runs\"}"), &claim).unwrap();
        let mut originals = Vec::new();
        let mut original_inputs = Vec::new();
        for number in 0..3 {
            let submission = native.retain(number, &format!("Request number {number}")).await;
            client.1.lock().unwrap().push_back((format!("routable-{number}"), "setup_help".into(), serde_json::json!({})));
            let actual = native.control.gateway.submit_authenticated_original(submission.clone()).await.unwrap();
            let PrivateOriginalSubmission::Admitted { session_id, input_id, deduplicated, turn_ticket: Some(ticket),
                source_settlement: PrivateSourceSettlement::FreshAcceptancePromoted } = actual
                else { panic!("fresh actual promoted original admission required") };
            assert_eq!(session_id, id.to_string()); assert!(!deduplicated);
            let input_id = serde_json::from_value(serde_json::json!(input_id)).unwrap();
            let outcome = native.native.as_ref().unwrap().identity_runtime().unwrap().wait_for_turn(
                &AgentIdentity::parse(MEMBER).unwrap(), TurnTicket::parse(&ticket).unwrap(), BOUND).await;
            assert!(matches!(outcome, TurnOutcome::Completed { output: TurnOutput::Text { text, truncated: false } } if text == ANSWER));
            original_inputs.push(input_id); originals.push(submission);
            native.terminal(&original_inputs.iter().cloned().collect()).await;
        }
        let rows = scoped(&owner, association.scope());
        assert_eq!(rows.len(), 3);
        let mut origins = HashSet::new();
        for row in &rows {
            let consent = row.consent.as_ref().unwrap();
            assert_eq!(row.status, ApprovalStatus::Pending);
            assert!(consent.routable() && consent.withdrawal.is_none() && consent.consumed_at.is_none());
            assert!(matches!(consent.custody, Some(ApprovalConsentCustody::Parked)));
            assert!(consent.deliveries.is_empty());
            let work = &consent.origin.as_ref().unwrap().retained_work;
            assert_eq!(work.runtime_id(), LogicalRuntimeId::for_session(&id).0.as_str());
            assert_eq!(work.contributors().len(), 1);
            let input = work.contributors()[0].input_id.clone();
            assert!(original_inputs.contains(&input));
            assert!(origins.insert(input));
            let actual = native.machine.input_terminal_receipt_observation(&id,
                InteractionSelector::InputId(work.contributors()[0].input_id.clone())).await.unwrap().unwrap();
            assert_eq!(actual.source, TerminalWitnessSource::DurableStore);
            let InputTerminalReceiptObservation::Finalized(receipt) = actual.report else { panic!("real R3 run terminal") };
            assert_eq!(receipt.run_id(), Some(work.run_id()));
            assert_eq!(receipt.terminal(), &meerkat_runtime::InputTerminalOutcome::Consumed);
        }
        assert_eq!(origins.len(), 3, "three real native origins, no copied run identity");
        let held = native.machine.hold_run_starts(&id, RunStartHoldReason::MobStop).await.unwrap();
        assert!(held.current_run.is_none());
        for row in &rows[..2] { issuer::approve(&owner, &row.approval_id); }
        native.arm();
        let rows = wait_rows(&owner, association.scope(), native.delivery.as_ref().unwrap(), |rows|
            rows.len() == 3 && delivery_ids(rows, ApprovalConsentTransition::Approved).len() == 2).await;
        let approved_inputs = delivery_ids(&rows, ApprovalConsentTransition::Approved);
        assert_eq!(approved_inputs.iter().collect::<HashSet<_>>().len(), 2);
        let stored = native.store.load_input_states_strict(&LogicalRuntimeId::for_session(&id)).await.unwrap();
        for input in &approved_inputs {
            let row = stored.iter().find(|row| &row.state.input_id == input).unwrap();
            assert_eq!(row.seed.phase, meerkat_runtime::input_state::InputLifecycleState::Queued);
            assert!(row.seed.terminal_outcome.is_none());
        }
        // A genuine issuer row outside the target session/domain is a negative
        // census sentinel. It is intentionally not claimed as routable work.
        let foreign_session = meerkat_core::SessionId::new();
        let foreign_hosting = meerkat_core::session_hosting::HostingCapability::OsLock(Arc::new(
            meerkat_core::session_hosting::HostingPaths { hosting_lock_dir: root.join("foreign-hosting"),
                cold_delivery_lock: root.join("foreign-delivery.lock"), database: None }));
        let foreign_claim = meerkat_core::session_hosting::grant_session_hosting(&foreign_hosting,
            &meerkat_core::session_hosting::HostingOwner::mint(), &foreign_session).unwrap();
        let probe = Arc::new(issuer::Probe::default());
        let foreign_id = issuer::request(&probe, &issuer::context(&owner, &foreign_claim), "foreign-row").await;
        let foreign = owner.approvals().get(&foreign_id).unwrap();
        assert_ne!(foreign.consent.as_ref().unwrap().scope.as_ref(), Some(association.scope()));
        assert_eq!(probe.0.load(Ordering::SeqCst), 0);
        assert!(!root.join("unexpected-mcp-calls").exists(), "R3 issued consent before any MCP effect");
        assert!(std::fs::read_to_string(root.join("unexpected-mcp-calls.pids")).unwrap().lines().count() == 1,
            "one selected host MCP peer serves preparation and all three original runs");
        let source_rows = native.control.readback.snapshot(&originals.iter().map(|s|s.original.clone()).collect::<Vec<_>>()).await.unwrap()
            .into_iter().map(|row|row.record).collect();
        let exact_inputs: HashSet<_> = original_inputs.iter().chain(&approved_inputs).cloned().collect();
        assert_eq!(native.input_ids().await, exact_inputs);
        assert_eq!(client.0.load(Ordering::SeqCst), 6, "three tool requests and their actual R3 refusals only");
        assert_eq!(peer.request_count(), 6, "every received HTTP request belongs to the three original runs");
        let ready = DurableReady { installed: native.selected.clone(), originals, source_rows, model_address: peer.address().unwrap(),
            common: Ready { root, session: id, identity, issuer: owner.selection().clone(), association,
                rows, foreign, original_inputs, approved_inputs, server } };
        println!("{READY_PREFIX}{}", serde_json::to_string(&ready).unwrap());
        std::io::stdout().flush().unwrap();
        let mut release = [0]; std::io::stdin().read_exact(&mut release).unwrap(); assert_eq!(release, [1]);
        std::process::exit(77);
    }).catch_unwind().await;
    let mut failures = Vec::new();
    if let Some(native) = runtime.as_mut()
        && let Err(error) = native.close().await
    {
        failures.push(error);
    }
    if let Err(error) = peer.finish().await {
        failures.push(format!("model peer join: {error}"));
    }
    drop((runtime, issuer_owner));
    let _retained = directory.keep();
    if !failures.is_empty() {
        let _ = writeln!(
            std::io::stderr(),
            "durable predecessor cleanup: {failures:?}"
        );
    }
    if let Err(panic) = caught {
        resume_unwind(panic);
    }
    panic!("durable predecessor must exit through the parent's release");
}

pub(super) fn run() {
    let pipes = Pipes::new();
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
        .block_on(census(pipes));
}
async fn census(pipes: Pipes) {
    let parent = tempfile::tempdir().unwrap();
    let parent_root = parent.path().canonicalize().unwrap();
    let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            DURABLE_CHILD,
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(ROOT_ENV, &parent_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut runtime: Option<DurableNative> = None;
    let mut selected_root = None;
    let mut retained_owner = None;
    let mut reopened_peer = None;
    let model_requests = Arc::new(AtomicUsize::new(0));
    let caught = AssertUnwindSafe(async {
        let mut lines = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
        let retained: DurableReady = tokio::time::timeout(BOUND, async {
            loop {
                let line = lines.next_line().await.unwrap().expect("predecessor exited before readiness");
                if let Some(start) = line.find(READY_PREFIX) {
                    break serde_json::from_str(&line[start+READY_PREFIX.len()..]).unwrap();
                }
            }
        }).await.expect("bounded real predecessor readiness");
        let ready = &retained.common;
        assert!(ready.root.is_absolute() && ready.root.parent() == Some(parent_root.as_path()));
        selected_root = Some(ready.root.clone());
        assert!(matches!(ApplicationConsentOwner::reopen(ready.issuer.clone(), &ready.identity),
            Err(crate::application_consent::ConsentOwnerError::OwnerHeld)));
        child.stdin.take().unwrap().write_all(&[1]).await.unwrap();
        assert_eq!(tokio::time::timeout(BOUND, child.wait()).await.unwrap().unwrap().code(), Some(77));
        let requests = model_requests.clone();
        reopened_peer = Some(ModelPeer::reopen_exact(MODEL, Arc::new(move |_body| {
            requests.fetch_add(1, Ordering::SeqCst);
            Box::pin(std::future::ready(Err(io::Error::other("Cold held actor must not call the model"))))
        }), retained.model_address).await.unwrap());
        let owner = ApplicationConsentOwner::reopen(ready.issuer.clone(), &ready.identity).unwrap();
        retained_owner = Some(owner.clone());
        assert_eq!(scoped(&owner, ready.association.scope()), ready.rows);
        assert_eq!(owner.approvals().get(&ready.foreign.approval_id).unwrap(), ready.foreign);
        runtime = Some(DurableNative::open(&ready.root, pipes, &owner, &ready.server,
            Some(retained.installed.clone()), Some(ready.session.clone())).await);
        let native = runtime.as_mut().unwrap();
        assert!(native.machine.current_session_registration_witness(&ready.session).await.is_none());
        let claim = native.machine.grant_session_hosting(&ready.session).unwrap();
        assert!(claim.is_cross_process() && claim.completed_owner_loss().is_none());
        assert_eq!(owner.bind_association(ready.association.artifact().clone(), &claim).unwrap(), ready.association);
        let first = native.service.settle_cold_predecessor_with_mob_lifecycle_authority_before(
            &ready.session, &claim, meerkat_core::time_compat::Instant::now()+BOUND).await.unwrap();
        let Some(PredecessorOwnerLoss::Completed(proof)) = &first.owner_loss else { panic!("actual R5 owner loss required") };
        assert_eq!(claim.completed_owner_loss().as_ref(), Some(proof));
        assert!(first.terminals_owed.is_empty() && first.migrated_without_receipt.is_empty());
        let newly_closed: HashSet<_> = first.inputs.iter().map(|input| input.receipt.input_id().clone()).collect();
        assert_eq!(newly_closed, ready.approved_inputs.iter().cloned().collect(),
            "R5 must close the exact two previously admitted approval notices");
        let closed_before: HashSet<_> = ready.original_inputs.iter().chain(&ready.approved_inputs).cloned().collect();
        assert_eq!(closed_before.len(), 5);
        native.terminal(&closed_before).await;
        let withdrawn = scoped(&owner, ready.association.scope());
        assert_eq!(withdrawn.len(), 3);
        for row in &withdrawn {
            let old = ready.rows.iter().find(|old| old.approval_id == row.approval_id).unwrap();
            let consent = row.consent.as_ref().unwrap();
            assert_eq!(row.status, if old.status == ApprovalStatus::Pending { ApprovalStatus::Cancelled } else { ApprovalStatus::Approved });
            assert_eq!(consent.origin, old.consent.as_ref().unwrap().origin);
            assert!(consent.routable() && consent.consumed_at.is_none());
            assert!(consent.withdrawal.as_ref().is_some_and(|w| w.reason == ApprovalConsentWithdrawalReason::Abandoned));
            assert!(matches!(consent.delivery(ApprovalConsentTransition::Withdrawn).unwrap().state,
                ApprovalConsentDeliveryState::Owed));
        }
        assert!(owner.observe_abandoned_after_native(&ready.association, &claim, proof, &closed_before).is_err(),
            "native input settlement cannot stand in for owed withdrawal delivery");
        let second = native.service.settle_cold_predecessor_with_mob_lifecycle_authority_before(
            &ready.session, &claim, meerkat_core::time_compat::Instant::now()+BOUND).await.unwrap();
        assert!(matches!(&second.owner_loss, Some(PredecessorOwnerLoss::NotOwed(actual)) if actual == proof));
        assert!(second.inputs.is_empty());
        assert_eq!(scoped(&owner, ready.association.scope()), withdrawn,
            "repeated settlement must not add another withdrawal obligation");
        // Reattach the actual persisted member without an initial message,
        // keep its actual actor under MobStop, and use native preparation to
        // restore each original's admitted source custody. No new input is
        // submitted by that preparation or by the cold bootstrap.
        native.attach(&ready.root, true).await;
        assert_eq!(native.input_ids().await, closed_before);
        for submission in &retained.originals {
            native.control.restore_promoted(submission.clone()).await.unwrap();
        }
        let rechecked = native.control.readback.snapshot(&retained.originals.iter().map(|s|s.original.clone()).collect::<Vec<_>>()).await.unwrap();
        assert_eq!(rechecked.into_iter().map(|row|row.record).collect::<Vec<_>>(), retained.source_rows);
        assert_eq!(native.input_ids().await, closed_before, "restoration cannot create a fourth original");
        assert!(native.machine.hold_run_starts(&ready.session, RunStartHoldReason::MobStop).await.unwrap().current_run.is_none());
        assert!(native.machine.current_executor_attachment_witness(&ready.session).await.is_some());
        assert_eq!(model_requests.load(Ordering::SeqCst), 0, "cold source preparation causes no model call");
        assert_eq!(reopened_peer.as_ref().unwrap().request_count(), 0, "no valid or refused HTTP request during cold preparation");
        native.arm();
        let delivered = wait_rows(&owner, ready.association.scope(), native.delivery.as_ref().unwrap(), |rows| rows.len() == 3
            && delivery_ids(rows, ApprovalConsentTransition::Withdrawn).len() == 3).await;
        let withdrawn_inputs = delivery_ids(&delivered, ApprovalConsentTransition::Withdrawn);
        assert_eq!(withdrawn_inputs.iter().collect::<HashSet<_>>().len(), 3);
        assert!(withdrawn_inputs.iter().all(|id| !closed_before.contains(id)));
        let stored = native.store.load_input_states_strict(&LogicalRuntimeId::for_session(&ready.session)).await.unwrap();
        for input in &withdrawn_inputs {
            let row = stored.iter().find(|row| &row.state.input_id == input).unwrap();
            assert_eq!(row.seed.phase, meerkat_runtime::input_state::InputLifecycleState::Queued);
            assert!(row.seed.terminal_outcome.is_none());
        }
        assert!(native.machine.hold_run_starts(&ready.session, RunStartHoldReason::MobStop).await.unwrap().current_run.is_none());
        assert!(owner.observe_abandoned_after_native(&ready.association, &claim, proof, &closed_before).is_err(),
            "admitted but not terminal notices must still refuse the whole census");
        native.retire_delivery().await.unwrap();
        assert!(native.delivery.is_none() && native.retired.is_some());
        // The native retirement report may retain a cancelled OS notification
        // registration thread; it owns no store reads/writes. Do not equate
        // joined delivery work with joining that platform call.
        let registration = native.machine.current_session_registration_witness(&ready.session).await.unwrap();
        assert!(native.machine.unregister_session_registration_until_terminal_if_current(&registration).await.unwrap());
        let closed: HashSet<_> = closed_before.iter().chain(&withdrawn_inputs).cloned().collect();
        assert_eq!(closed.len(), 8);
        native.terminal(&closed).await;
        assert_eq!(native.input_ids().await, closed, "exactly three originals and five real notices");
        let census = owner.observe_abandoned_after_native(&ready.association, &claim, proof, &closed).unwrap();
        assert_eq!(census.association, ready.association);
        assert_eq!(census.records, delivered);
        assert_eq!(census.records.len(), 3);
        for input in ready.approved_inputs.iter().chain(&withdrawn_inputs) {
            let mut incomplete = closed.clone(); incomplete.remove(input);
            assert!(owner.observe_abandoned_after_native(&ready.association, &claim, proof, &incomplete).is_err(),
                "each real prior/withdrawn delivery input is required across every row");
        }
        let again = owner.observe_abandoned_after_native(&ready.association, &claim, proof, &closed).unwrap();
        assert_eq!(again.records, census.records);
        assert_eq!(owner.approvals().get(&ready.foreign.approval_id).unwrap(), ready.foreign);
        assert_eq!(owner.approvals().list(ApprovalListFilter::default()).unwrap().len(), 4);
        assert!(!ready.root.join("unexpected-mcp-calls").exists());
        assert!(native.machine.current_session_registration_witness(&ready.session).await.is_none());
        assert_eq!(model_requests.load(Ordering::SeqCst), 0);
        assert_eq!(reopened_peer.as_ref().unwrap().request_count(), 0);
    }).catch_unwind().await;
    let mut failures = Vec::new();
    if child
        .try_wait()
        .map(|value| value.is_none())
        .unwrap_or(true)
    {
        let _ = child.start_kill();
        failures.push("predecessor needed emergency termination");
    }
    if child.wait().await.is_err() {
        failures.push("predecessor reap");
    }
    if let Some(native) = runtime.as_mut()
        && let Err(error) = native.close().await
    {
        let _ = writeln!(std::io::stderr(), "{error}");
        failures.push("actual native/delivery close");
    }
    drop((runtime, retained_owner));
    if let Some(peer) = reopened_peer {
        if peer.request_count() != 0 {
            failures.push("cold HTTP requests occurred");
        }
        if let Err(error) = peer.finish().await {
            let _ = writeln!(std::io::stderr(), "cold model peer: {error}");
            failures.push("cold model peer join/requests");
        }
    }
    if model_requests.load(Ordering::SeqCst) != 0 {
        failures.push("cold model requests occurred");
    }
    // Check actual helper liveness on every exit, including failure before the
    // ready frame. No direct-child reap is presented as a transitive join.
    let roots = match selected_root {
        Some(root) => vec![root],
        None => match std::fs::read_dir(&parent_root) {
            Ok(entries) => entries
                .filter_map(|entry| match entry {
                    Ok(entry) => Some(entry.path()),
                    Err(_) => {
                        failures.push("helper root observation failed");
                        None
                    }
                })
                .filter(|path| path.is_dir())
                .collect(),
            Err(_) => {
                failures.push("helper root census failed");
                Vec::new()
            }
        },
    };
    for root in &roots {
        if let Err(error) = helpers_gone(root).await {
            let _ = writeln!(std::io::stderr(), "{error}");
            failures.push("native helper liveness not confirmed");
        }
    }
    if caught.is_err() || !failures.is_empty() {
        let _retained = parent.keep();
    }
    if !failures.is_empty() {
        let _ = writeln!(
            std::io::stderr(),
            "multi-row cleanup failures: {failures:?}"
        );
    }
    if let Err(panic) = caught {
        resume_unwind(panic);
    }
    assert!(failures.is_empty(), "{failures:?}");
}

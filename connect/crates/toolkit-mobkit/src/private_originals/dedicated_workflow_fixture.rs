//! A fresh held roster, with a persistent mode for actual idle ordinary resume.
//! Selected names are test inputs, not an online append API.
use super::*;
#[path = "setup_workflow/launch/context_transfer/entered_dispatch_fixture.rs"]
pub(in crate::private_originals) mod entered_fixture;
use meerkat_authorization_contracts::constraints::{DelegationDepth, LifetimeRestriction};
use meerkat_mobkit::rpc_gateway::GatewayHostComposition;
use meerkat_toolkit::setup::{SetupWorkBackend, SetupWorkContext, SetupWorkflowTerms};
use meerkat_workgraph::{
    ClaimWorkItemRequest, CreateWorkItemRequest, LinkWorkItemsRequest, SqliteWorkGraphStore,
    WorkEdgeKind, WorkGraphService, WorkItemRef, WorkNamespace, WorkOwner, WorkOwnerKey,
};
use toolkit_package::{ContentDigest, Revision};

pub(in crate::private_originals) struct DedicatedFixture<
    S = EphemeralSessionService<FactoryAgentBuilder>,
> {
    pub f: FirstFixture<S>,
    pub history: Option<Arc<dyn meerkat_runtime::RuntimeStore>>,
    pub catalog_probe: Option<Arc<super::helper_entry_native::CatalogProbe>>,
    pub operation_owner: Arc<dyn meerkat_authorization::grant_policy::OperationPolicyOwner>,
    durable: Option<DurableDedicated>,
    pub member: Arc<crate::private_originals::InstalledWorkflowMember>,
    pub parent: PrivateOriginalSubmit,
    pub lead: PrivateOriginalSubmit,
    pub terms: SetupWorkflowTerms,
    pub lineage: Vec<GrantLineageRef>,
    pub backend: SetupWorkBackend,
    pub work: SetupWorkContext,
    pub parent_session: SessionId,
    pub lead_session: SessionId,
    _gateway: crate::host_gateway::ToolkitGatewayHost,
    _private_open: tokio::sync::watch::Sender<bool>,
}
// No native/source/controller owner survives the close/reopen boundary.
#[derive(Clone)]
pub(in crate::private_originals) struct DurableDedicated {
    directory: Arc<tempfile::TempDir>,
    selection: meerkat_toolkit::host_lifetime::control::HostBootstrapSelection,
    parent: PrivateOriginalSubmit,
    lead: PrivateOriginalSubmit,
    submission: PrivateOriginalSubmit,
    terms: SetupWorkflowTerms,
    lineage: Vec<GrantLineageRef>,
    work: SetupWorkContext,
    sessions: Vec<SessionId>,
}

async fn event(
    source: &Arc<PrivateOriginals>,
    route: &str,
    label: &str,
    contributors: Vec<EvidenceId>,
) -> PrivateOriginalSubmit {
    let mut event = private_event(label);
    event.route = Id::new(route).unwrap();
    event.content_sha256 = digest(label.as_bytes());
    event.contributors = contributors;
    let authentication = event.original_authentication.clone();
    let original = source.retain_event(event).await.unwrap();
    PrivateOriginalSubmit {
        original,
        content: label.into(),
        transport_authentication: authentication,
    }
}
fn work_ref(item: &meerkat_workgraph::WorkItem) -> WorkItemRef {
    WorkItemRef {
        realm_id: item.realm_id.clone(),
        namespace: item.namespace.clone(),
        item_id: item.id.clone(),
    }
}
async fn claimed(service: &WorkGraphService, label: &str) -> meerkat_workgraph::WorkItem {
    let item = service
        .create(CreateWorkItemRequest {
            title: label.into(),
            labels: ["dedicated-cycle".into()].into(),
            ..Default::default()
        })
        .await
        .unwrap();
    service
        .claim(ClaimWorkItemRequest {
            id: item.id,
            realm_id: None,
            namespace: None,
            expected_revision: item.revision,
            owner: WorkOwner {
                key: WorkOwnerKey::agent(label).unwrap(),
                display_name: None,
            },
            lease_seconds: Some(300),
            lease_expires_at: None,
        })
        .await
        .unwrap()
}
#[derive(Clone, Copy)]
enum WaiterProfile {
    None,
    Find,
    Effect,
    Public,
    OperationalPublic,
    Catalog,
}

impl DedicatedFixture {
    pub async fn new() -> Self {
        Self::with_participant(true).await
    }
    pub async fn without_participant() -> Self {
        Self::with_participant(false).await
    }
    async fn with_participant(capture_workflow: bool) -> Self {
        Self::construct(
            capture_workflow,
            false,
            false,
            WaiterProfile::None,
            None,
            |mut builder, configuration, _| {
                let machine = Arc::new(
                    MeerkatMachine::ephemeral()
                        .with_local_grant_authorization(configuration)
                        .unwrap(),
                );
                builder.default_session_store = Some(Arc::new(SessionStore::default()));
                (
                    Arc::new(EphemeralSessionService::new(builder, 4)),
                    machine,
                    None,
                )
            },
        )
        .await
    }
}

impl DedicatedFixture<meerkat::PersistentSessionService<FactoryAgentBuilder>> {
    pub async fn persistent_operational_parent() -> Self {
        Self::persistent_from(WaiterProfile::Catalog, None).await
    }
    pub async fn reopen_operational_parent(retained: DurableDedicated) -> Self {
        Self::persistent_from(WaiterProfile::Catalog, Some(retained)).await
    }
    pub async fn persistent() -> Self {
        Self::persistent_from(WaiterProfile::None, None).await
    }
    pub async fn persistent_configured_waiter() -> Self {
        Self::persistent_from(WaiterProfile::Find, None).await
    }
    pub async fn persistent_reviewed_workflow(
        reviewed: Arc<entered_fixture::ReviewedWorkflow>,
    ) -> Self {
        let delivery = reviewed.clone();
        Self::construct_selected(
            true,
            true,
            false,
            WaiterProfile::Find,
            None,
            Some(reviewed),
            move |builder, configuration, directory| {
                let persistence = meerkat::PersistenceBundle::new_with_local_grant_authorization(
                    Arc::new(
                        meerkat::SqliteSessionStore::open(
                            directory.join("dedicated-native.sqlite3"),
                        )
                        .unwrap(),
                    ),
                    Arc::new(
                        meerkat_runtime::store::SqliteRuntimeStore::new(
                            directory.join("dedicated-native.sqlite3"),
                        )
                        .unwrap(),
                    ),
                    Arc::new(meerkat::MemoryBlobStore::new()),
                    configuration,
                )
                .unwrap();
                let history = persistence.runtime_store();
                assert!(
                    delivery
                        .delivery
                        .set(meerkat_mobkit::MobRuntimeDelivery::new(
                            history.clone(),
                            persistence.job_store()
                        ))
                        .is_ok()
                );
                let (service, machine) =
                    meerkat::surface::build_runtime_backed_service(builder, 4, persistence)
                        .unwrap();
                (Arc::new(service), machine, Some(history))
            },
        )
        .await
    }
    pub async fn persistent_effect_waiter() -> Self {
        Self::persistent_from(WaiterProfile::Effect, None).await
    }
    pub async fn persistent_public_waiter() -> Self {
        Self::persistent_from(WaiterProfile::Public, None).await
    }
    pub async fn persistent_operational_public_waiter() -> Self {
        Self::persistent_from(WaiterProfile::OperationalPublic, None).await
    }
    pub async fn reopen(retained: DurableDedicated) -> Self {
        Self::persistent_from(WaiterProfile::None, Some(retained)).await
    }
    pub async fn persistent_configured_waiter_for_gateway() -> Self {
        Self::persistent_for_gateway(WaiterProfile::Find, false).await
    }
    pub async fn persistent_effect_waiter_for_gateway() -> Self {
        Self::persistent_for_gateway(WaiterProfile::Effect, true).await
    }
    async fn persistent_for_gateway(profile: WaiterProfile, gateway_members: bool) -> Self {
        Self::construct(
            true,
            true,
            gateway_members,
            profile,
            None,
            |builder, configuration, directory| {
                let layout = meerkat_mobkit::storage_layout::MetaStorageLayout::standalone(
                    directory.to_path_buf(),
                    directory.join("gateway-home"),
                );
                let persistence = meerkat::PersistenceBundle::new_with_local_grant_authorization(
                    Arc::new(
                        meerkat::SqliteSessionStore::open(layout.session_db().unwrap().path)
                            .unwrap(),
                    ),
                    Arc::new(
                        meerkat_runtime::store::SqliteRuntimeStore::new(layout.runtime_db())
                            .unwrap(),
                    ),
                    Arc::new(meerkat::MemoryBlobStore::new()),
                    configuration,
                )
                .unwrap();
                let history = persistence.runtime_store();
                let (service, machine) =
                    meerkat::surface::build_runtime_backed_service(builder, 4, persistence)
                        .unwrap();
                (Arc::new(service), machine, Some(history))
            },
        )
        .await
    }
    async fn persistent_from(
        configured_waiter: WaiterProfile,
        retained: Option<DurableDedicated>,
    ) -> Self {
        Self::construct(
            true,
            true,
            false,
            configured_waiter,
            retained,
            |builder, configuration, directory| {
                let persistence = meerkat::PersistenceBundle::new_with_local_grant_authorization(
                    Arc::new(
                        meerkat::SqliteSessionStore::open(
                            directory.join("dedicated-native.sqlite3"),
                        )
                        .unwrap(),
                    ),
                    Arc::new(
                        meerkat_runtime::store::SqliteRuntimeStore::new(
                            directory.join("dedicated-native.sqlite3"),
                        )
                        .unwrap(),
                    ),
                    Arc::new(meerkat::MemoryBlobStore::new()),
                    configuration,
                )
                .unwrap();
                let history = persistence.runtime_store();
                let (service, machine) =
                    meerkat::surface::build_runtime_backed_service(builder, 4, persistence)
                        .unwrap();
                (Arc::new(service), machine, Some(history))
            },
        )
        .await
    }
    // Only the reviewed workflow controls publish the real in-process
    // bootstrap completion. Their caller already retains this whole fixture
    // inside its caught body, so failure still reaches the owned close below.
    pub fn publish_reviewed_workflow_bootstrap(&self) -> Result<(), String> {
        self._gateway
            .publish_runtime_bootstrap_completed(&self.f.native)
            .map_err(|error| error.to_string())
    }
    pub async fn close_reviewed_workflow(&self) -> Result<(), String> {
        let closing = self
            ._gateway
            .begin_close(meerkat_mobkit::rpc_gateway::GatewayHostCloseReason::Shutdown);
        // Both actual owners enter their joins even if close entry or either
        // join refuses. Dropping the gateway only stops recovery, not joins it.
        let (native, gateway) = tokio::join!(self.f.shutdown(), self._gateway.join_close());
        closing.map_err(|error| error.to_string())?;
        native?;
        gateway.map_err(|error| error.to_string())
    }
    pub fn durable_state(&self) -> DurableDedicated {
        self.durable.as_ref().unwrap().clone()
    }
    pub async fn close(&self) -> Result<(), String> {
        self.f.close_durable_owners().await
    }
}

impl<
    S: meerkat_mob::MobSessionService + meerkat_mobkit::identity_first::ControllerPinSource + 'static,
> DedicatedFixture<S>
{
    async fn construct(
        capture_workflow: bool,
        durable: bool,
        gateway_members: bool,
        configured_waiter: WaiterProfile,
        saved: Option<DurableDedicated>,
        build_service: impl FnOnce(
            FactoryAgentBuilder,
            meerkat_runtime::meerkat_machine::NativeGrantWorkConfiguration,
            &std::path::Path,
        ) -> (
            Arc<S>,
            Arc<MeerkatMachine>,
            Option<Arc<dyn meerkat_runtime::RuntimeStore>>,
        ) + Send,
    ) -> Self {
        Self::construct_selected(
            capture_workflow,
            durable,
            gateway_members,
            configured_waiter,
            saved,
            None,
            build_service,
        )
        .await
    }
    async fn construct_selected(
        capture_workflow: bool,
        durable: bool,
        gateway_members: bool,
        configured_waiter: WaiterProfile,
        saved: Option<DurableDedicated>,
        reviewed: Option<Arc<entered_fixture::ReviewedWorkflow>>,
        build_service: impl FnOnce(
            FactoryAgentBuilder,
            meerkat_runtime::meerkat_machine::NativeGrantWorkConfiguration,
            &std::path::Path,
        ) -> (
            Arc<S>,
            Arc<MeerkatMachine>,
            Option<Arc<dyn meerkat_runtime::RuntimeStore>>,
        ) + Send,
    ) -> Self {
        let mut opened = None;
        let mut retained = None;
        let result = std::panic::AssertUnwindSafe(Self::build(
            &mut opened,
            &mut retained,
            capture_workflow,
            durable,
            gateway_members,
            configured_waiter,
            saved,
            reviewed,
            build_service,
        ))
        .catch_unwind()
        .await;
        match result {
            Ok(result) => result,
            Err(panic) => {
                // Observe every cleanup assertion without replacing the
                // construction failure. Retain the same lifetime until joined.
                let _cleanup = std::panic::AssertUnwindSafe(async {
                    use meerkat_toolkit::host_lifetime::control::{
                        HostCloseReason, HostCloseState, PrivateHostLifetimeControl,
                    };
                    let close = retained.as_ref().filter(|_| durable).map(|(_, lifetime)| {
                        lifetime.begin_close_activation(HostCloseReason::Shutdown)
                    });
                    let native = match &opened {
                        Some(native) => shutdown_fixture(native).await,
                        None => Ok(()),
                    };
                    if let (Some((_, lifetime)), Some(Ok(close))) = (&retained, &close) {
                        assert_eq!(
                            lifetime.join_close(&close.handle).await.unwrap().state,
                            HostCloseState::Settled
                        );
                    }
                    assert!(
                        close.as_ref().is_none_or(|close| close.is_ok()),
                        "host close entry refused: {close:?}"
                    );
                    assert!(native.is_ok(), "{native:?}");
                })
                .catch_unwind()
                .await;
                drop(retained);
                std::panic::resume_unwind(panic)
            }
        }
    }
    async fn build(
        opened: &mut Option<UnifiedRuntime>,
        retained: &mut Option<(Arc<tempfile::TempDir>, Arc<NativeHostLifetime>)>,
        capture_workflow: bool,
        durable: bool,
        gateway_members: bool,
        configured_waiter: WaiterProfile,
        saved: Option<DurableDedicated>,
        reviewed: Option<Arc<entered_fixture::ReviewedWorkflow>>,
        build_service: impl FnOnce(
            FactoryAgentBuilder,
            meerkat_runtime::meerkat_machine::NativeGrantWorkConfiguration,
            &std::path::Path,
        ) -> (
            Arc<S>,
            Arc<MeerkatMachine>,
            Option<Arc<dyn meerkat_runtime::RuntimeStore>>,
        ) + Send,
    ) -> Self {
        let facts =
            meerkat_core::ControllerModelFacts::new(selection(), ENDPOINT.into(), MODEL.into());
        assert!(
            reviewed.is_none()
                || (saved.is_none() && durable && capture_workflow && !gateway_members)
        );
        let reviewed_host = if let Some(reviewed) = &reviewed {
            Some(crate::installed_host::controller_first_input_fixture::open_for_reviewed_dedicated_workflow(
                &facts, reviewed.script.dispatcher()).await)
        } else {
            None
        };
        let (mut host, reviewed_helper) = match reviewed_host {
            Some((host, helper)) => (host, Some(helper)),
            None => (match &saved {
            Some(saved) => crate::installed_host::controller_first_input_fixture::reopen_for_dedicated_workflow(
                saved.directory.clone(), saved.selection.clone()).await,
            None if matches!(configured_waiter, WaiterProfile::Catalog) => crate::installed_host::controller_first_input_fixture::open_for_operational_dedicated_workflow(&facts).await,
            None if matches!(configured_waiter, WaiterProfile::Effect) => crate::installed_host::controller_first_input_fixture::open_for_effect_dedicated_workflow(&facts).await,
            None if matches!(configured_waiter, WaiterProfile::OperationalPublic) => crate::installed_host::controller_first_input_fixture::open_for_operational_public_dedicated_workflow(&facts).await,
            None if matches!(configured_waiter, WaiterProfile::Public) => crate::installed_host::controller_first_input_fixture::open_for_public_dedicated_workflow(&facts).await,
            None if matches!(configured_waiter, WaiterProfile::Find) => crate::installed_host::controller_first_input_fixture::open_for_configured_dedicated_workflow(&facts).await,
            None => crate::installed_host::controller_first_input_fixture::open_for_dedicated_workflow(&facts).await,
        }, None),
        };
        *retained = Some((host.directory.clone(), host.lifetime.clone()));
        let source = host.originals.clone();
        assert!(
            source.host_lifetime.get().is_none(),
            "the dedicated fixture must not prebind a private lifetime"
        );
        let (private_open, private_closed) = tokio::sync::watch::channel(false);
        let other_lifetime =
            Arc::new(NativeHostLifetime::bootstrap(host.selection.clone()).unwrap());
        let competing = host.competing_gateway(
            other_lifetime.clone(),
            crate::private_host::PrivateHostClosed(private_closed.clone()),
        );
        let same_host_without_participant = (!capture_workflow).then(|| {
            host.competing_gateway(
                host.lifetime.clone(),
                crate::private_host::PrivateHostClosed(private_closed.clone()),
            )
        });
        let installed = host.installed.take().unwrap();
        let installed = match &reviewed {
            Some(reviewed) => reviewed.enroll(installed, reviewed_helper.unwrap()),
            None => installed,
        };
        let mut gateway = installed.compose(
            host.lifetime.clone(),
            crate::private_host::PrivateHostClosed(private_closed),
        );
        let (parent, lead, submission, terms, lineage, backend, work) = if let Some(saved) = &saved
        {
            let graph = WorkGraphService::with_scope(
                Arc::new(
                    SqliteWorkGraphStore::open(
                        host.source_path.with_extension("dedicated-work.sqlite"),
                    )
                    .unwrap(),
                ),
                host.source_scope.realm.clone(),
                WorkNamespace::new("dedicated-workflow").unwrap(),
            );
            let backend = SetupWorkBackend::new(
                graph,
                "dedicated-sqlite".into(),
                Id::new("one").unwrap(),
                &[meerkat_core::DurabilityDeclaration::durable(
                    "workgraph",
                    meerkat_core::DurabilityResolution::Persistent,
                )],
            )
            .unwrap();
            assert_eq!(backend.pin(), &saved.terms.backend);
            (
                saved.parent.clone(),
                saved.lead.clone(),
                saved.submission.clone(),
                saved.terms.clone(),
                saved.lineage.clone(),
                backend,
                saved.work.clone(),
            )
        } else {
            let parent = event(
                &source,
                "planning_parent",
                "dedicated parent request",
                vec![],
            )
            .await;
            let lead = event(
                &source,
                "planning_lead",
                "dedicated planning request",
                vec![parent.original.original.clone()],
            )
            .await;
            let submission = event(&source, "console", PROMPT, vec![]).await;
            let selected = host
                .lifetime
                .selected_policy()
                .native_host
                .as_ref()
                .unwrap();
            let route = source.route(&lead.original.route).unwrap();
            let member_selection = selected
                .members
                .iter()
                .find(|m| m.member.member == "planning_lead")
                .unwrap();
            let now = source.owner.clock().now().unwrap().unix_ms;
            let deadline = now + 180_000;
            let bounds = ExecutionRestrictions {
                delegation_depth: DelegationDepth::remaining(1),
                lifetime: LifetimeRestriction::window(now, deadline + 60_000),
                ..ExecutionRestrictions::unrestricted()
            };
            let root = source
                .owner
                .grants()
                .issue_root(
                    selected.remote_operator.as_ref().unwrap_or(
                        &meerkat::governed_realm::stock::StockPrincipals::for_local_operator(
                            &selected.realm,
                        )
                        .unwrap()
                        .operator,
                    ),
                    EvidenceId::new("dedicated-operation-root").unwrap(),
                    selected.grant_administrator.clone(),
                    route.represented_subject.clone(),
                    bounds.clone(),
                )
                .unwrap();
            let child = bounds
                .for_child(&ExecutionRestrictions {
                    delegation_depth: DelegationDepth::remaining(0),
                    ..ExecutionRestrictions::unrestricted()
                })
                .unwrap();
            let leaf = source
                .owner
                .grants()
                .issue_child(
                    &selected.grant_administrator,
                    &root,
                    EvidenceId::new("dedicated-operation-leaf").unwrap(),
                    route.logical_executor.clone(),
                    child.clone(),
                )
                .unwrap();
            let lineage = vec![root, leaf];
            let graph = WorkGraphService::with_scope(
                Arc::new(
                    SqliteWorkGraphStore::open(
                        host.source_path.with_extension("dedicated-work.sqlite"),
                    )
                    .unwrap(),
                ),
                host.source_scope.realm.clone(),
                WorkNamespace::new("dedicated-workflow").unwrap(),
            );
            let parent_work = claimed(&graph, "parent").await;
            let waiter_work = claimed(&graph, "waiter-a").await;
            graph
                .link(LinkWorkItemsRequest {
                    realm_id: None,
                    namespace: None,
                    kind: WorkEdgeKind::Parent,
                    from_id: waiter_work.id.clone(),
                    to_id: parent_work.id.clone(),
                })
                .await
                .unwrap();
            let backend = SetupWorkBackend::new(
                graph,
                "dedicated-sqlite".into(),
                Id::new("one").unwrap(),
                &[meerkat_core::DurabilityDeclaration::durable(
                    "workgraph",
                    meerkat_core::DurabilityResolution::Persistent,
                )],
            )
            .unwrap();
            let work = SetupWorkContext::from_owner(
                backend.pin().clone(),
                work_ref(&parent_work),
                work_ref(&waiter_work),
                parent_work.owner.as_ref().unwrap().key.clone(),
                waiter_work.owner.as_ref().unwrap().key.clone(),
                "dedicated-cycle".into(),
            )
            .unwrap();
            let terms = SetupWorkflowTerms {
                key: Id::new("dedicated-workflow").unwrap(),
                backend: backend.pin().clone(),
                parent: work_ref(&parent_work),
                parent_owner: parent_work.owner.as_ref().unwrap().key.clone(),
                cycle_label: "dedicated-cycle".into(),
                requester: route.requester.clone(),
                realm: route.realm.clone(),
                authority_revision: if reviewed.is_some() {
                    Revision::new(
                        ContentDigest::of_bytes(
                            &canonical_json(&host.lifetime.current_setup_policy().unwrap())
                                .unwrap(),
                        )
                        .to_string(),
                    )
                    .unwrap()
                } else {
                    Revision::new("dedicated-policy").unwrap()
                },
                planning_source: source.source_record_key(&lead.original).await.unwrap(),
                lead: member_selection.member.clone(),
                envelope_digest: ContentDigest::of_bytes(&canonical_json(&child).unwrap()),
                deadline_ms: deadline,
                max_waiters: 8,
            };
            (parent, lead, submission, terms, lineage, backend, work)
        };
        if let Some(reviewed) = &reviewed {
            reviewed.select_work(&submission.original, backend.clone(), work.clone());
        }
        let roster = gateway_roster();
        let mut wrong_spec = roster[2].clone();
        wrong_spec.addressability = AgentAddressability::Addressable;
        assert!(
            gateway
                .reserve_workflow_member(&terms, &lead.original, &wrong_spec)
                .await
                .is_err()
        );
        assert!(
            source.host_lifetime.get().is_none(),
            "a rejected reservation cannot freeze a host lifetime"
        );
        let member = gateway
            .reserve_workflow_member(&terms, &lead.original, &roster[2])
            .await
            .unwrap();
        assert!(
            Arc::ptr_eq(
                &source
                    .host_lifetime
                    .get()
                    .and_then(std::sync::Weak::upgrade)
                    .unwrap(),
                &host.lifetime
            ),
            "successful reservation selects the exact host before any capture"
        );
        assert!(
            member.session().is_err(),
            "a selection is not a native session"
        );
        let same_member = gateway
            .reserve_workflow_member(&terms, &lead.original, &roster[2])
            .await
            .unwrap();
        assert!(
            same_member.session().is_err(),
            "public exact replay does not create or capture a member"
        );
        assert!(
            Arc::ptr_eq(
                &source
                    .host_lifetime
                    .get()
                    .and_then(std::sync::Weak::upgrade)
                    .unwrap(),
                &host.lifetime
            ),
            "public replay confirms the existing exact host lifetime"
        );
        let configuration = gateway.native_grant_work_configuration().unwrap().unwrap();
        let operation_owner = configuration.operation_owner.clone();
        let provider = Arc::new(ScriptedProvider::default());
        let probe = Arc::new(if matches!(configured_waiter, WaiterProfile::Catalog) {
            crate::installed_host::acquisition::ContextProbe::with_exact_catalog()
        } else {
            crate::installed_host::acquisition::ContextProbe::default()
        });
        let custody = meerkat_mobkit::MemberMcpCustody::new();
        let factory = custody.install(AgentFactory::minimal().comms(true));
        let factory = match &reviewed {
            Some(reviewed) => factory.with_operation_review(reviewed.bound_review()),
            None => factory,
        };
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        let mob_tools = builder.default_mob_tools.clone();
        let catalog_probe = matches!(configured_waiter, WaiterProfile::Catalog)
            .then(super::helper_entry_native::CatalogProbe::for_workflow);
        let model: Arc<dyn LlmClient> = match &reviewed {
            Some(reviewed) => reviewed.model(provider.clone()),
            None => Arc::new(
                crate::installed_host::acquisition::ProbeProvider::with_probe(
                    provider.clone(),
                    &probe,
                ),
            ),
        };
        builder.default_llm_client = Some(match &catalog_probe {
            Some(catalog) => catalog.provider(model),
            None => model,
        });
        *builder.default_agent_llm_client_decorator.write().unwrap() =
            Some(fixture_controller_decorator());
        let (service, machine, history) =
            build_service(builder, configuration, host.directory.path());
        let mut definition = definition();
        definition.id = MobId::from("factory");
        let ProfileBinding::Inline(profile) = definition
            .profiles
            .get_mut(&ProfileName::from("worker"))
            .unwrap()
        else {
            panic!("inline")
        };
        profile.tools = ToolConfig {
            comms: true,
            ..Default::default()
        };
        if reviewed.is_some() {
            profile
                .tools
                .rust_bundles
                .push(crate::FIND_AND_INSTALL_BUNDLE.into());
        }
        let competing_spec =
            MobBootstrapSpec::new(definition.clone(), MobStorage::in_memory(), service.clone())
                .with_session_runtime_adapter(machine.clone())
                .unwrap()
                .with_declared_ephemeral_mob_storage();
        assert!(
            matches!(
                competing.prepare_bootstrap(competing_spec).await,
                Err(meerkat_mobkit::rpc_gateway::GatewayHostError::Denied)
            ),
            "a different host cannot bind the reserved source before its registration captures G"
        );
        assert!(Arc::ptr_eq(
            &source
                .host_lifetime
                .get()
                .and_then(std::sync::Weak::upgrade)
                .unwrap(),
            &host.lifetime
        ));
        assert!(
            source.machine().is_err(),
            "the losing bootstrap cannot bind the source machine"
        );
        assert!(
            member.session().is_err(),
            "the losing bootstrap cannot materialize or capture a member"
        );
        drop(competing);
        drop(other_lifetime);
        // The negative mode uses a second public composition over the SAME
        // lifetime/source, with the real registration but no workflow observer.
        // No source selection or actual native destination is fabricated.
        if let Some(without_participant) = same_host_without_participant {
            gateway = without_participant;
        }
        let tool_probe = probe.clone();
        let (storage, provenance) = if durable {
            let (storage, provenance) =
                meerkat_mobkit::mob_composition_manifest::persistent_mob_storage(
                    if gateway_members {
                        gateway_layout(host.directory.path()).event_log_db()
                    } else {
                        host.directory.path().join("dedicated-mob.sqlite3")
                    },
                )
                .unwrap();
            (storage, Some(provenance))
        } else {
            (MobStorage::in_memory(), None)
        };
        let spec = MobBootstrapSpec::new(definition, storage, service.clone())
            .with_session_runtime_adapter(machine.clone())
            .unwrap()
            .with_member_mcp_custody(custody);
        let spec = match &reviewed {
            Some(reviewed) => {
                let mut spec = spec
                    .with_workgraph_service(Some(backend.service().clone()))
                    .with_runtime_delivery(reviewed.delivery.get().unwrap().clone())
                    .with_agent_mob_tools(mob_tools)
                    .unwrap()
                    .with_host_job_source(
                        meerkat_mob_mcp::HostJobSource::new(
                            [reviewed.continuation().namespace().to_owned()],
                            reviewed.continuation(),
                        )
                        .unwrap(),
                    )
                    .unwrap();
                let mut census = meerkat_mobkit::storage_health::ResolvedStorageSummary::new(
                    meerkat_mobkit::storage_health::BlobDurability::DeclaredEphemeral,
                    None,
                );
                census.slots.push(
                    meerkat_mobkit::storage_health::StorageSlotSummary::persistent(
                        "workgraph",
                        "dedicated-sqlite",
                    ),
                );
                spec.resolved_storage = Some(census);
                spec
            }
            None => spec.with_default_external_tools_provider(Arc::new(move || {
                Some(tool_probe.clone() as Arc<dyn meerkat_core::AgentToolDispatcher>)
            })),
        };
        let spec = match provenance {
            Some(provenance) => spec
                .with_mob_storage_provenance(provenance)
                .with_child_mob_storage(meerkat_mobkit::ChildMobStorage::ProcessBound),
            None => spec.with_declared_ephemeral_mob_storage(),
        };
        let controller_pins: Arc<dyn meerkat_mobkit::identity_first::ControllerPinSource> =
            service.clone();
        let mut spec = spec;
        spec.controller_pin_source = Some(controller_pins.clone());
        let spec = gateway.prepare_bootstrap(spec).await.unwrap();
        // Register an observing wrapper around the exact enrolled leaf. The
        // native registry still owns its bundle identity and entry protocol.
        let spec = match &reviewed {
            Some(reviewed) => spec.register_tool_bundle(
                crate::FIND_AND_INSTALL_BUNDLE,
                Arc::new(
                    meerkat_core::RegisteredToolBundleDispatcher::new(
                        crate::FIND_AND_INSTALL_BUNDLE,
                        reviewed.script.dispatcher(),
                    )
                    .with_account(meerkat_core::OperationAccount::DeclaredNone),
                ),
            ),
            None => spec,
        };
        assert!(Arc::ptr_eq(
            &source
                .host_lifetime
                .get()
                .and_then(std::sync::Weak::upgrade)
                .unwrap(),
            &host.lifetime
        ));
        assert!(
            gateway
                .reserve_workflow_member(&terms, &lead.original, &roster[2])
                .await
                .is_err(),
            "materialization cannot accept a new late registration participant"
        );
        *opened = Some(bootstrap_fixture(spec, "factory".into()).await);
        let native = opened.as_mut().unwrap();
        if let Some(reviewed) = &reviewed {
            reviewed
                .continuation()
                .bind_native(native.mob_runtime())
                .unwrap();
        }
        let handle = native.mob_handle();
        let bridge = MobSessionBridge::with_session_service(
            handle.clone(),
            native.mob_runtime().session_service().cloned().unwrap(),
        )
        .with_controller_pin_source(service.clone());
        let runtime = Arc::new(
            IdentityRuntime::new(IdentityRuntimeConfig {
                continuity_store: Arc::new(if durable {
                    LocalContinuityStore::open(if gateway_members {
                        gateway_layout(host.directory.path())
                            .continuity_db()
                            .unwrap()
                            .path
                    } else {
                        host.directory.path().join("dedicated-continuity.sqlite3")
                    })
                    .unwrap()
                } else {
                    LocalContinuityStore::in_memory().unwrap()
                }),
                lease_provider: Arc::new(LocalLeaseProvider::new()),
                runtime_instance_id: "dedicated-workflow-test".into(),
                has_runtime_store: true,
                durability_policy: DurabilityPolicy::SyncWriteThrough,
                bridge: Some(Arc::new(bridge)),
                default_timeout: None,
            })
            .with_runtime_services(
                meerkat_mobkit::identity_first::AgentRuntimeServices::new(handle.clone()),
            ),
        );
        let context = Arc::new(IdentityFirstRuntimeContext::new(
            runtime,
            Arc::new(MutableRosterProvider::new(roster.clone())),
            None,
            None,
            Some(handle.definition().clone()),
        ));
        if reviewed.is_some() {
            // Runtime-owned bootstrap below mints the actual one-use completion.
            native.install_identity_first_context(context.clone());
        } else {
            native.attach_identity_first_context(context.clone());
        }
        let runtime = native.identity_runtime().unwrap();
        gateway.bind_runtime(native).await.unwrap();
        gateway.bind_identity_runtime(native).await.unwrap();
        let runtime_pins = native.mob_runtime().controller_pin_source().unwrap();
        let source_pins = source
            .controller_pin_source
            .get()
            .and_then(std::sync::Weak::upgrade)
            .unwrap();
        assert!(
            Arc::ptr_eq(&runtime_pins, &controller_pins),
            "runtime must retain the actual service pin owner"
        );
        assert!(
            Arc::ptr_eq(&source_pins, &controller_pins),
            "normal gateway bind must retain the actual service pin owner"
        );
        if saved.is_some() {
            let restored = context.bootstrap_roster(&roster).await.unwrap();
            for row in &roster {
                assert!(
                    matches!(
                        restored.outcomes.get(&row.identity),
                        Some(meerkat_mobkit::identity_first::RestoreOutcome::Resumed { .. })
                    ),
                    "ordinary native resume required: {:?}",
                    restored.outcomes.get(&row.identity)
                );
            }
            assert!(
                member.session().is_err(),
                "actual destination capture is not source custody completion"
            );
        } else if reviewed.is_some() {
            let complete_member = member.clone();
            let result = native
                .bootstrap_installed_identity_first_context_held_until(
                    &roster,
                    tokio::time::Instant::now() + Duration::from_secs(30),
                    Box::new(move |members, _completion| {
                        Box::pin(async move {
                            let held = members
                                .iter()
                                .find(|row| row.identity.as_str() == "planning_lead")
                                .ok_or("missing actual G")?;
                            if capture_workflow {
                                complete_member
                                    .complete_held_member(held)
                                    .await
                                    .map_err(|error| error.to_string())
                            } else {
                                Ok(())
                            }
                        })
                    }),
                )
                .await
                .unwrap();
            match result {
                meerkat_mobkit::identity_first::HeldBootstrapStart::Activated(_) => (),
                meerkat_mobkit::identity_first::HeldBootstrapStart::Unsettled(settlement) => {
                    let closing = gateway.begin_close(
                        meerkat_mobkit::rpc_gateway::GatewayHostCloseReason::InitializationFailed,
                    );
                    // The actual abort settlement and gateway owners both join
                    // before construction failure reaches the outer cleanup.
                    let (settled, closed) = tokio::join!(
                        native.finish_held_identity_bootstrap(settlement),
                        gateway.join_close()
                    );
                    assert!(
                        closing.is_ok() && closed.is_ok(),
                        "held bootstrap owner joins: {closing:?}/{closed:?}"
                    );
                    panic!("held full-roster construction did not activate: {settled:?}")
                }
            }
        } else {
            let complete_member = member.clone();
            let result = context
                .bootstrap_roster_held(
                    &roster,
                    tokio::time::Instant::now() + Duration::from_secs(30),
                    move |members, _completion| async move {
                        let held = members
                            .iter()
                            .find(|row| row.identity.as_str() == "planning_lead")
                            .ok_or("missing actual G")?;
                        if capture_workflow {
                            complete_member
                                .complete_held_member(held)
                                .await
                                .map_err(|error| error.to_string())
                        } else {
                            Ok(())
                        }
                    },
                )
                .await
                .unwrap();
            match result {
                meerkat_mobkit::identity_first::HeldBootstrapStart::Activated(_) => (),
                meerkat_mobkit::identity_first::HeldBootstrapStart::Unsettled(settlement) => {
                    let _actual = settlement.settled().await;
                    panic!("held full-roster construction did not activate")
                }
            }
        }
        let mut sessions = Vec::new();
        for row in &roster {
            let status = runtime.status(&row.identity).await.unwrap();
            assert_eq!(status.state, IdentityLifecycleState::Active);
            sessions.push(status.session_id.unwrap());
        }
        assert!(
            sessions[0] != sessions[1] && sessions[0] != sessions[2] && sessions[1] != sessions[2]
        );
        if let Some(saved) = &saved {
            assert_eq!(
                sessions, saved.sessions,
                "fresh fallback is not restoration"
            );
        } else if capture_workflow {
            assert_eq!(member.session().unwrap(), sessions[2]);
        } else {
            assert!(
                member.session().is_err(),
                "omitting the participant cannot manufacture capture"
            );
        }
        let selected_sessions = if reviewed.is_some() {
            sessions.as_slice()
        } else {
            &sessions[2..3]
        };
        for session in selected_sessions {
            let actor = service.live_actor(session).await.unwrap();
            let pin = service.pin_actor(&actor).await.unwrap();
            meerkat_core::publish_token_lifecycle_acquired_for_identity_within(
                &machine.generated_auth_lease_handle(),
                pin.selection().credential(),
                &meerkat_core::auth::PersistedTokens::api_key("synthetic-controller-only-fixture"),
                tokio::time::Instant::now() + WAIT,
            )
            .await
            .unwrap();
        }
        let durable = durable.then(|| DurableDedicated {
            directory: host.directory.clone(),
            selection: host.selection.clone(),
            parent: parent.clone(),
            lead: lead.clone(),
            submission: submission.clone(),
            terms: terms.clone(),
            lineage: lineage.clone(),
            work: work.clone(),
            sessions: sessions.clone(),
        });
        let f = FirstFixture {
            store: host.store,
            source_path: host.source_path,
            scope: host.source_scope,
            service,
            probe: reviewed.is_none().then_some(probe),
            _directory: host.directory,
            _lifetime: host.lifetime,
            source,
            machine,
            native: opened.take().unwrap(),
            runtime,
            identity: roster[0].identity.clone(),
            session: sessions[0].clone(),
            provider,
            submission,
        };
        Self {
            f,
            history,
            catalog_probe,
            operation_owner,
            durable,
            member,
            parent,
            lead,
            terms,
            lineage,
            backend,
            work,
            parent_session: sessions[1].clone(),
            lead_session: sessions[2].clone(),
            _gateway: gateway,
            _private_open: private_open,
        }
    }
}

/// Reopen the same durable stores and normal installed host preparation without
/// materializing any roster. This tests the metadata page before actor revival;
/// it does not fabricate native Ready or start the recovery worker.
pub(in crate::private_originals) struct MetadataDedicated {
    pub source: Arc<PrivateOriginals>,
    pub machine: Arc<MeerkatMachine>,
    pub service: Arc<meerkat::PersistentSessionService<FactoryAgentBuilder>>,
    pub history: Arc<dyn meerkat_runtime::RuntimeStore>,
    pub backend: SetupWorkBackend,
    pub sessions: Vec<SessionId>,
    provider: Arc<ScriptedProvider>,
    host: crate::installed_host::controller_first_input_fixture::DedicatedHost,
    _gateway: crate::host_gateway::ToolkitGatewayHost,
    _private_open: tokio::sync::watch::Sender<bool>,
}
impl DurableDedicated {
    pub async fn reopen_stdio_host(
        &self,
    ) -> crate::installed_host::controller_first_input_fixture::DedicatedHost {
        crate::installed_host::controller_first_input_fixture::reopen_for_dedicated_workflow(
            self.directory.clone(),
            self.selection.clone(),
        )
        .await
    }
    pub fn lead_source(&self) -> &PrivateOriginalRef {
        &self.lead.original
    }
    pub fn sessions(&self) -> &[SessionId] {
        &self.sessions
    }
    pub fn terms(&self) -> &SetupWorkflowTerms {
        &self.terms
    }
    pub async fn reopen_metadata(self) -> MetadataDedicated {
        let mut host =
            crate::installed_host::controller_first_input_fixture::reopen_for_dedicated_workflow(
                self.directory,
                self.selection,
            )
            .await;
        let lifetime = host.lifetime.clone();
        let directory = host.directory.clone();
        let result = std::panic::AssertUnwindSafe(async move {
            let source = host.originals.clone();
            let (private_open, private_closed) = tokio::sync::watch::channel(false);
            let gateway = host.installed.take().unwrap().compose(
                host.lifetime.clone(),
                crate::private_host::PrivateHostClosed(private_closed),
            );
            let configuration = gateway.native_grant_work_configuration().unwrap().unwrap();
            let provider = Arc::new(ScriptedProvider::default());
            let mut builder =
                FactoryAgentBuilder::new(AgentFactory::minimal().comms(true), Config::default());
            builder.default_llm_client = Some(provider.clone());
            let persistence = meerkat::PersistenceBundle::new_with_local_grant_authorization(
                Arc::new(
                    meerkat::SqliteSessionStore::open(
                        host.directory.path().join("dedicated-native.sqlite3"),
                    )
                    .unwrap(),
                ),
                Arc::new(
                    meerkat_runtime::store::SqliteRuntimeStore::new(
                        host.directory.path().join("dedicated-native.sqlite3"),
                    )
                    .unwrap(),
                ),
                Arc::new(meerkat::MemoryBlobStore::new()),
                configuration,
            )
            .unwrap();
            let history = persistence.runtime_store();
            let (service, machine) =
                meerkat::surface::build_runtime_backed_service(builder, 4, persistence).unwrap();
            let service = Arc::new(service);
            let mut definition = definition();
            definition.id = MobId::from("factory");
            let spec = MobBootstrapSpec::new(definition, MobStorage::in_memory(), service.clone())
                .with_session_runtime_adapter(machine.clone())
                .unwrap()
                .with_declared_ephemeral_mob_storage();
            let prepared = gateway.prepare_bootstrap(spec).await.unwrap();
            assert!(Arc::ptr_eq(&source.machine().unwrap(), &machine));
            assert!(source.bound_identity_runtime().is_err());
            // The prepared spec is not a bootstrapped runtime or Ready token.
            drop(prepared);
            let graph = WorkGraphService::with_scope(
                Arc::new(
                    SqliteWorkGraphStore::open(
                        host.source_path.with_extension("dedicated-work.sqlite"),
                    )
                    .unwrap(),
                ),
                host.source_scope.realm.clone(),
                WorkNamespace::new("dedicated-workflow").unwrap(),
            );
            let backend = SetupWorkBackend::new(
                graph,
                "dedicated-sqlite".into(),
                Id::new("one").unwrap(),
                &[meerkat_core::DurabilityDeclaration::durable(
                    "workgraph",
                    meerkat_core::DurabilityResolution::Persistent,
                )],
            )
            .unwrap();
            assert_eq!(backend.pin(), &self.terms.backend);
            MetadataDedicated {
                source,
                machine,
                service,
                history,
                backend,
                sessions: self.sessions,
                provider,
                host,
                _gateway: gateway,
                _private_open: private_open,
            }
        })
        .catch_unwind()
        .await;
        match result {
            Ok(fixture) => fixture,
            Err(panic) => {
                // No roster/actor was entered. Still observe the original host
                // close owner on construction failure, preserving that failure.
                let _cleanup = std::panic::AssertUnwindSafe(async {
                    use meerkat_toolkit::host_lifetime::control::{
                        HostCloseReason, HostCloseState, PrivateHostLifetimeControl,
                    };
                    let close = lifetime
                        .begin_close_activation(HostCloseReason::Shutdown)
                        .unwrap();
                    assert_eq!(
                        lifetime.join_close(&close.handle).await.unwrap().state,
                        HostCloseState::Settled
                    );
                })
                .catch_unwind()
                .await;
                drop(directory);
                std::panic::resume_unwind(panic)
            }
        }
    }
}
impl MetadataDedicated {
    pub fn model_requests(&self) -> usize {
        self.provider.0.load(Ordering::SeqCst)
    }
    pub async fn close(&self) {
        use meerkat_toolkit::host_lifetime::control::{
            HostCloseReason, HostCloseState, PrivateHostLifetimeControl,
        };
        let close = self
            .host
            .lifetime
            .begin_close_activation(HostCloseReason::Shutdown)
            .unwrap();
        assert_eq!(
            self.host
                .lifetime
                .join_close(&close.handle)
                .await
                .unwrap()
                .state,
            HostCloseState::Settled
        );
    }
}

fn gateway_layout(
    directory: &std::path::Path,
) -> meerkat_mobkit::storage_layout::MetaStorageLayout {
    meerkat_mobkit::storage_layout::MetaStorageLayout::standalone(
        directory.to_path_buf(),
        directory.join("gateway-home"),
    )
}
fn gateway_roster() -> Vec<DurableAgentSpec> {
    [MEMBER, "planning_parent", "planning_lead"]
        .into_iter()
        .map(|name| DurableAgentSpec {
            identity: AgentIdentity::parse(name).unwrap(),
            profile: ProfileName::from("worker"),
            addressability: if name == "planning_lead" {
                AgentAddressability::InternalOnly
            } else {
                AgentAddressability::Addressable
            },
            display_name: None,
            labels: BTreeMap::new(),
            context: None,
            additional_instructions: vec![],
            initial_message: None,
            runtime_mode_override: Some(MobRuntimeMode::TurnDriven),
            backend: None,
            binding: None,
            placement: None,
        })
        .collect()
}
impl DurableDedicated {
    pub fn gateway_configuration(&self) -> (&'static str, Vec<DurableAgentSpec>) {
        const CONFIG: &str = r#"[mob]
id = "factory"
[profiles.worker]
model = "claude-sonnet-4-5"
peer_description = "controller preparation fixture"
external_addressable = true
runtime_mode = "turn_driven"
[profiles.worker.tools]
comms = true
"#;
        let mut expected = definition();
        expected.id = MobId::from("factory");
        assert_eq!(
            MobDefinition::from_toml(CONFIG).unwrap(),
            expected,
            "stdio must select the seed's complete actual definition"
        );
        (CONFIG, gateway_roster())
    }
    pub fn gateway_model() -> (
        Arc<dyn meerkat_llm_core::LlmClient>,
        impl Fn() -> usize + Send + Sync,
    ) {
        let provider = Arc::new(ScriptedProvider::default());
        let observed = provider.clone();
        (provider, move || observed.0.load(Ordering::SeqCst))
    }
}

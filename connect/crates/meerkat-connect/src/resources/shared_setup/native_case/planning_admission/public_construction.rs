//! The full packaged adapter is constructed before finalization. The actual
//! held roster supplies surface/session evidence; the same pending installation
//! is adopted before the held callback releases native member publication.
use super::*;
use crate::application_acquisition::selected_policy::{
    AccessSelection, InstallationSelection, MemberToolPolicySelection, SelectedPolicyBinding,
    StandingPolicySelection,
};
use toolkit_mobkit::installed_host::operator_setup::SharedWorkflowSetupRequest;

fn inputs() -> PreparedLocalPolicy {
    let mut selected = construction::inputs();
    selected.native_host.service_parent_administration =
        Some(ServiceParentAdministration::ApplicationPolicyTasks {
            routes: [id("original-b")].into(),
        });
    selected
}
fn policy() -> PreparedInstalledToolPolicy {
    let original = super::super::super::policy();
    let template = original.compiled();
    let mut rows = template.members.clone();
    rows.retain(|row| !row.member_identity.contains("helper-b"));
    for row in &mut rows {
        if row.member_identity == member("helper-a").member {
            row.member_identity = member("planning-lead").member;
        }
    }
    let lead = rows
        .iter_mut()
        .find(|row| row.member_identity == member("planning-lead").member)
        .unwrap();
    lead.grants
        .retain(|row| row.tool_name != meerkat_toolkit::setup::SETUP_REPORT);
    lead.grants.push(CompiledMemberToolGrant {
        tool_name: meerkat_toolkit::setup::SETUP_PUBLIC_SERVICE_REQUEST.into(),
        action: CompiledMemberToolAction::Invoke,
        consequence: CompiledToolConsequence::R2,
    });
    lead.grants.sort_by(|a, b| a.tool_name.cmp(&b.tool_name));
    rows.push(CompiledMemberToolGrants {
        member_identity: member("planning-parent").member,
        grants: vec![],
    });
    rows.sort_by(|a, b| a.member_identity.cmp(&b.member_identity));
    let compiled = CompiledApplicationToolPolicy::new(
        PolicyProviderId::new("sdk-shared-public").unwrap(),
        PolicyId::new("sdk-shared-public").unwrap(),
        PolicyRevision(1),
        CompiledPolicySourceProvenance {
            source_id: "actual-shared-public-application".into(),
            source_digest: PolicyDigest::from_canonical_bytes(
                b"two independent Find-only Originals; one checked public setup G",
            ),
        },
        rows,
    )
    .unwrap();
    PreparedInstalledToolPolicy::from_canonical(&compiled.canonical_json().unwrap()).unwrap()
}
fn requests() -> PreparationAuthorities {
    let mut controllers = NAMES
        .iter()
        .map(|name| PreparedControllerRequest {
            target: PreparedControllerTarget::OriginalRoute { route: id(name) },
            issuance_id: evidence(&format!("{name}-controller")),
            controller_route: controller_facts().route_key().clone(),
        })
        .collect::<Vec<_>>();
    controllers.push(PreparedControllerRequest {
        target: PreparedControllerTarget::Helper,
        issuance_id: evidence("public-g-controller"),
        controller_route: controller_facts().route_key().clone(),
    });
    PreparationAuthorities {
        controllers,
        helper_mcp: Some(PreparedHelperMcpRequest {
            issuance_id: evidence("public-g-mcp"),
        }),
        helper_catalog: Some(PreparedHelperCatalogRequest {
            issuance_id: evidence("public-g-catalog"),
            scope: PreparedHelperCatalogScope::CurrentHelperSuccessors,
        }),
        helper_report: Some(HelperReportCapabilityRequest {
            member: member("planning-lead"),
            issuance_id: evidence("public-g-report"),
            account_issuance_id: None,
            public_issuance_id: Some(evidence("public-g-public-service")),
        }),
        shared_workflow_setup: NAMES[..2]
            .iter()
            .map(|name| SharedWorkflowSetupRequest {
                member: member(name),
                route: id(name),
                issuance_id: evidence(&format!("{name}-fresh-shared-effects")),
                helper: member("planning-lead"),
                targets: vec![
                    SetupTargetRequest {
                        target: Some(GrantTarget::McpServer {
                            server: "toolkit-setup".into(),
                        }),
                        issuance_id: evidence(&format!("{name}-shared-mcp")),
                    },
                    SetupTargetRequest {
                        target: Some(GrantTarget::RegisteredToolBundleOperation {
                            bundle: meerkat_toolkit::setup::SETUP_ACCOUNTS_BUNDLE.into(),
                            operation: meerkat_toolkit::setup::SETUP_PUBLIC_SERVICE_REQUEST.into(),
                        }),
                        issuance_id: evidence(&format!("{name}-shared-public")),
                    },
                ],
            })
            .collect(),
        ..Default::default()
    }
}

pub(super) async fn construct(
    case: &mut Case,
    pipes: toolkit_mobkit::private_host::InheritedHostPipes,
    slots: Arc<Mutex<Option<Constructed>>>,
    prepared_runtime: Arc<Mutex<Option<Arc<toolkit_mobkit::host_gateway::PreparedGatewayRuntime>>>>,
    script: Arc<PublicScript>,
) {
    let executable = PathBuf::from(
        std::env::var_os("TOOLKIT_SHARED_PUBLIC_EXECUTABLE")
            .expect("current staged product executable"),
    );
    let pin = std::env::var("TOOLKIT_SHARED_PUBLIC_RESOURCES_SHA256")
        .expect("current exact resource digest");
    let resources = InstalledResources::from_executable(&executable, &pin).unwrap();
    let state = case.directory.path().join("resource-state");
    fs::create_dir(&state).unwrap();
    let storage = meerkat_core::StorageLayout::resolve_with_global_candidate(
        meerkat_core::StorageLayoutInputs {
            invocation_context: case.directory.path().into(),
            explicit_state_root: Some(state),
            ..Default::default()
        },
        &meerkat_core::RealmConfig {
            selection: meerkat_core::RealmSelection::Explicit {
                realm_id: meerkat_core::mob_realm_id(MOB).unwrap().to_string(),
            },
            ..Default::default()
        },
        &case.directory.path().join("unused-global"),
    )
    .unwrap();
    let physical = resources
        .prepare_with_shared_setup(&storage, 1, std::future::pending())
        .await
        .unwrap();
    case.resource = Some(physical.owner);
    assert_eq!(physical.connections.len(), 1);
    let peer = physical.connections.into_iter().next().unwrap();
    let helper = peer
        .connection
        .for_mobkit_helper_in_place(
            member("planning-lead"),
            crate::setup::SetupHelperTools::CheckedInstallation,
        )
        .unwrap();
    let (approvals, presenter) =
        crate::approval::LocalApprovals::create(case.directory.path()).unwrap();
    case.reviews.push(presenter);
    let owner = PreparedInstallationOwner::open(
        case.directory.path().into(),
        inputs(),
        OperatorOwnerStartup::fresh("sdk-shared-public", 1).unwrap(),
    )
    .unwrap();
    let before = owner.selected().clone();
    let policy = policy();
    let standing = StandingPolicySelection {
        installation: InstallationSelection::HumanReview,
        access: AccessSelection::ProvisionTaskParent,
        tool_policy: Some(MemberToolPolicySelection::UntilSuperseded),
    };
    let selected_policy = SelectedPolicyBinding::from_preparation(&owner, standing);
    let setup_bytes = canonical_json(&standing).unwrap();
    let setup_reference = CompiledArtifactReference {
        name: "application-setup.json".into(),
        sha256: digest(&setup_bytes),
    };
    let capture = Arc::new(SurfaceOwner {
        members: before.native_host.members.clone(),
        policy: policy.clone(),
        surfaces: Mutex::new(BTreeMap::new()),
    });
    let shared = crate::installed_service_review::prepare_shared_setup_installation(
        &owner,
        RealmLocator {
            state_root: case.directory.path().into(),
            realm: before.native_host.realm.clone(),
        },
        vec![crate::installed_service_review::SharedSetupInstallation {
            helper: helper.clone(),
            peer: peer.peer,
            approvals,
        }],
        policy.clone(),
        Arc::new(NoBusinessContext),
    )
    .unwrap();
    let limits = InstalledSetupLimits {
        setup_timeout_ms: PUBLIC_BOUND.as_millis() as u64,
        ..Default::default()
    };
    let tools =
        InstalledAcquisitionTools::new(Duration::from_millis(limits.admission_timeout_ms)).unwrap();
    let roster = NAMES
        .iter()
        .map(|name| DurableAgentSpec {
            identity: AgentIdentity::parse(name).unwrap(),
            profile: ProfileName::from(*name),
            addressability: if *name == "planning-lead" {
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
        .collect::<Vec<_>>();
    let mut prepared = owner
        .prepare_with_application_service_policy(policy.clone(), selected_policy.participant())
        .await
        .unwrap();
    let pending_member = prepared
        .prepare_workflow_member(
            &id("planning-lead"),
            &evidence("sdk-planning-lead"),
            &roster[3],
        )
        .await
        .unwrap();
    let shared = shared
        .bind_native_with_workflow_members(
            prepared.take_configuration().unwrap(),
            capture.clone(),
            policy.registry(),
            vec![pending_member.clone()],
        )
        .with_selected_public_service_policy(selected_policy.clone());
    let registration = shared.registration();
    let participant = Arc::new(
        prepared
            .gateway_runtime(registration.clone(), tools.clone(), [id("human")].into())
            .unwrap(),
    );
    // Retain before the first native bootstrap await. The same participant
    // must stop/join custody recovery on early failure as on normal shutdown.
    *prepared_runtime.lock().unwrap() = Some(participant.clone());
    {
        // Exercise the real shared-reserve boundary against this not-yet-adopted
        // acquisition owner. No fake task source or synthetic error is supplied.
        let (approvals, mut presenter) =
            crate::approval::LocalApprovals::create(case.directory.path()).unwrap();
        let approvals = approvals.with_service_tasks(Some(participant.service_tasks()));
        let arguments = serde_json::value::RawValue::from_string("{}".into()).unwrap();
        let call = meerkat_core::ToolCallView {
            id: "unadopted-shared-reserve",
            name: "setup_reconcile",
            args: &arguments,
        };
        let refused = approvals
            .reserve_shared_entry(
                call,
                &meerkat_core::ToolDispatchContext::default(),
                None,
                None,
            )
            .await;
        assert!(
            matches!(
                refused,
                Err(meerkat_core::ToolError::OperationAuthorizationUnavailable)
            ),
            "unavailable acquisition must not be labeled a policy denial"
        );
        assert!(
            futures_util::FutureExt::now_or_never(presenter.next()).is_none(),
            "unavailable reservation must not request approval"
        );
        assert!(!approvals.has_uncertain_dispatch());
        approvals.retire_checked().unwrap();
    }
    let originals = prepared.originals();
    let custody = meerkat_mobkit::MemberMcpCustody::new();
    let (factory, configuration) = shared
        .configure_factory_with_attempt_guards(
            custody.install(AgentFactory::minimal().comms(true)),
            participant.service_tasks(),
            case.resource.as_ref().unwrap(),
        )
        .unwrap();
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    builder.default_llm_client = Some(script.clone());
    *builder.default_agent_llm_client_decorator.write().unwrap() = Some(Arc::new(|client| {
        assert_eq!(
            client.controller_model_selection().as_ref(),
            Some(&model_selection())
        );
        client
    }));
    let mob_tools = builder.default_mob_tools.clone();
    // Retain the same authority that the one-use prepared configuration gives
    // this machine. Final prepared adoption cannot issue that configuration again.
    let native_grants = configuration.grants.clone();
    let persistence = meerkat::PersistenceBundle::new_with_local_grant_authorization(
        Arc::new(
            meerkat::SqliteSessionStore::open(
                case.directory.path().join("native-sessions.sqlite3"),
            )
            .unwrap(),
        ),
        Arc::new(
            meerkat_runtime::store::SqliteRuntimeStore::new(
                case.directory.path().join("native-runtime.sqlite3"),
            )
            .unwrap(),
        ),
        Arc::new(meerkat::MemoryBlobStore::new()),
        configuration,
    )
    .unwrap();
    let history = persistence.runtime_store();
    case.audit_store = Some(history.clone());
    let jobs = persistence.job_store();
    let (service, machine) =
        meerkat::surface::build_runtime_backed_service(builder, 4, persistence).unwrap();
    let service = Arc::new(service);
    let graph = meerkat_workgraph::WorkGraphService::with_namespace_grant(
        Arc::new(
            meerkat_workgraph::SqliteWorkGraphStore::open(
                case.directory.path().join("workgraph.sqlite3"),
            )
            .unwrap(),
        ),
        meerkat_core::service::WorkGraphNamespaceGrant::new(
            before.native_host.realm.as_str(),
            "setup",
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
            "sdk-sqlite-workgraph",
        ),
    );
    let spec = MobBootstrapSpec::new(
        construction::mob_definition(),
        MobStorage::in_memory(),
        service.clone(),
    )
    .with_member_mcp_custody(custody)
    .with_session_runtime_adapter(machine.clone())
    .unwrap()
    .with_declared_ephemeral_mob_storage()
    .with_workgraph_service(Some(graph.clone()))
    .with_runtime_delivery(meerkat_mobkit::MobRuntimeDelivery::new(history, jobs))
    .with_agent_mob_tools(mob_tools)
    .unwrap();
    let mut spec = helper.compose(spec).unwrap();
    spec.resolved_storage = Some(census.clone());
    let definition = spec.definition.clone();
    let spec = participant.prepare_bootstrap(spec).unwrap();
    case.native = Some(
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
    let native = case.native.as_mut().unwrap();
    participant.bind_runtime(native).unwrap();
    let handle = native.mob_runtime().handle();
    let bridge = MobSessionBridge::with_session_service(
        handle.clone(),
        native.mob_runtime().session_service().cloned().unwrap(),
    )
    .with_controller_pin_source(service.clone());
    let runtime = Arc::new(
        IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory().unwrap()),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "sdk-shared-public".into(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(Arc::new(bridge)),
            default_timeout: None,
        })
        .with_runtime_services(meerkat_mobkit::identity_first::AgentRuntimeServices::new(
            handle.clone(),
        )),
    );
    let context = Arc::new(IdentityFirstRuntimeContext::new(
        runtime,
        Arc::new(MutableRosterProvider::new(roster.clone())),
        None,
        None,
        Some(handle.definition().clone()),
    ));
    native.install_identity_first_context(context);
    participant.bind_identity_runtime(native).unwrap();
    let root = case.directory.path().to_path_buf();
    let client = case.client.clone();
    let retained_lifetime = case.lifetime.clone();
    let final_policy = prepared.policy();
    let callback_slots = slots.clone();
    let held=native.bootstrap_installed_identity_first_context_held_until(
        &roster,tokio::time::Instant::now()+BOUND,Box::new(move |members,_completion| Box::pin(async move {
        if members.len()!=NAMES.len() {return Err("incomplete actual held roster".into());}
        let lead=members.iter().find(|row|row.identity.as_str()=="planning-lead").ok_or("missing actual held G")?;
        let captured=capture.surfaces.lock().unwrap().clone();
        let lead_session=captured.get(&member("planning-lead").member).ok_or("missing actual captured G session")?.0.clone();
        let mut selected_helper=selection("planning-lead");
        let selected=prepared.finalize_authorities_with_helper_resources(&mut selected_helper,&requests(),Some(helper.server()),
            Some((helper.member(),&lead_session))).map_err(|e|e.to_string())?;
        let (selection,application_reference,application_bytes)=public_artifacts::publish(&root,&selected,&policy,&captured,&definition,&setup_bytes);
        let application=PreparedSetupApplication::from_verified_artifact(application_reference,application_bytes,definition.clone(),Arc::new(CurrentRoster)).map_err(|e|e.to_string())?;
        let expected=definition.clone();
        let projection:toolkit_mobkit::host_gateway::SetupBootstrapProjection=Arc::new(move |spec| {
            if spec.definition!=expected {return Err(meerkat_toolkit::host_lifetime::control::HostLifetimeError::InvalidSelection);} Ok(spec)
        });
        let saved=Arc::new(Mutex::new(None));let save=saved.clone();let launch_client=client.clone();
        let observed_lifetime=retained_lifetime.clone();
        let server=async move {
            let startup=PrivateHostStartup::read(pipes).await?;
            PrivateHostConnection::bootstrap_prepared_with_service_parent(startup,move |selection|async move {
                let (owner,installed)=prepared.adopt(selection)?; let source=installed.originals();*save.lock().unwrap()=Some(installed);Ok((owner,source))
            },move |owner,closed,logins,endpoint|async move {
                *retained_lifetime.lock().unwrap()=Some(owner.clone());
                selected_policy.bind_with_service_parent(owner.clone(),standing,&setup_reference,Some(endpoint))?;
                let launch=SdkLaunchOwner::new_shared(participant.acquisition().unwrap().1,owner.clone(),id("sdk-workgraph-generation"),launch_client,
                    meerkat_workgraph::WorkOwnerKey::agent("sdk-parent").unwrap(),"sdk-independent-children".into());
                launch.capture_prepared_backend(graph,&census)?;
                let actions=NAMES[..2].iter().flat_map(|name|[Action::FindAndInstall,Action::Delegate].into_iter()
                    .map(move|action|ActionPermission {action,resource:id(&member(name).member)})).collect();
                let installed=saved.lock().unwrap().take().unwrap().with_enrolled_bootstrap_acquisition(InstalledSetupBootstrap {
                    helper:selected_helper,proposal_ceiling:Ceiling {actions,services:BTreeSet::new(),frontends:BTreeSet::new(),output_audiences:[id("human")].into()},
                    continuation:launch.clone(),backend_generation:id("sdk-workgraph-generation"),references:None,
                    limits,
                    work_step:WorkAdmissionStepOptions {worker:id("sdk-public-worker"),lease_ms:60_000,owner_timeout_ms:10_000,settlement_budget_ms:10_000},
                },tools)?;
                let host=installed.compose_with_prepared_registration(owner,closed,registration,final_policy)?.with_console_logins(logins)?
                    .with_native_setup_bootstrap(participant.acquisition().unwrap().1,projection,application)?;
                let host=participant.adopt(host).await?; Ok((host,launch))
            }).await
        };
        let request=async {client.lock().await.call(json!({"kind":"bootstrap","selection":selection})).await};
        let (reply,server)=tokio::join!(request,server);
        let ((host,launch),private,local,remote)=server.map_err(|e|e.to_string())?;
        let host = Arc::new(host);
        drop(local);
        // Retain connection custody before any later fallible selection. The
        // outer test drains these exact owners even when held bootstrap aborts.
        *callback_slots.lock().unwrap()=Some(Constructed {host:Some(host.clone()),private:Some(private),launch:launch.clone(),remote:Some(remote),planning:None});
        if reply.map_err(|e|e.to_string())?["kind"]!="bootstrap_accepted" {return Err("bootstrap not accepted".into());}
        launch.create_parent().await;
        let (backend,parent_work,parent_owner,cycle_label)=launch.workflow_parent().map_err(|e|format!("{e:?}"))?;
        let retain=|event:PrivateAuthenticatedEvent,content:&'static str| {let source=originals.clone();async move {
            let authentication=event.original_authentication.clone();let original=source.retain_event(event).await.map_err(|e|e.to_string())?;
            Ok::<_,String>(PrivateOriginalSubmit {original,content:content.into(),transport_authentication:authentication})}};
        let parent=retain(source_event("planning-parent","sdk-planning-parent",P_TEXT,vec![]),P_TEXT).await?;
        let lead_source=retain(source_event("planning-lead","sdk-planning-lead",G_TEXT,vec![parent.original.original.clone()]),G_TEXT).await?;
        let now=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
        let deadline=now+PUBLIC_BOUND.as_millis() as u64;
        let bounds=ExecutionRestrictions {delegation_depth:DelegationDepth::remaining(1),lifetime:LifetimeRestriction::window(now,deadline+60_000),..ExecutionRestrictions::unrestricted()};
        let native=&selected.native_host; let route=native.routes.iter().find(|row|row.id==lead_source.original.route).unwrap();
        let root_grant=native_grants.issue_root(&meerkat_core::AdmissionGuard::none(),native.remote_operator.as_ref().unwrap(),evidence("public-independent-g-root"),
            native.grant_administrator.clone(),route.represented_subject.clone(),bounds.clone()).map_err(|e|e.to_string())?;
        let leaf_bounds=bounds.for_child(&ExecutionRestrictions {delegation_depth:DelegationDepth::remaining(0),..ExecutionRestrictions::unrestricted()}).map_err(|e|e.to_string())?;
        let leaf=native_grants.issue_child(&meerkat_core::AdmissionGuard::none(),&native.grant_administrator,&root_grant,evidence("public-independent-g-leaf"),route.logical_executor.clone(),leaf_bounds.clone()).map_err(|e|e.to_string())?;
        let lifetime=observed_lifetime.lock().unwrap().as_ref().unwrap().clone();
        let current=lifetime.current_setup_policy().map_err(|e|e.to_string())?;
        let terms=SetupWorkflowTerms {key:id("sdk-public-cohort"),backend,parent:parent_work,parent_owner,cycle_label,
            requester:principal("human"),realm:native.realm.clone(),authority_revision:Revision::new(ContentDigest::of_bytes(&canonical_json(&current).unwrap()).to_string()).unwrap(),
            planning_source:originals.source_record_key(&lead_source.original).await.map_err(|e|e.to_string())?,lead:member("planning-lead"),
            envelope_digest:ContentDigest::of_bytes(&canonical_json(&leaf_bounds).unwrap()),deadline_ms:deadline,max_waiters:8};
        let workflow_member=pending_member.bind(&lifetime,&terms,&lead_source.original).await.map_err(|e|e.to_string())?;
        let planning=PublicPlanning {source:originals,parent,lead:lead_source,terms,lineage:vec![root_grant,leaf],member:workflow_member.clone()};
        {let mut retained=callback_slots.lock().unwrap();let slot=retained.as_mut().unwrap();slot.planning=Some(planning);}
        workflow_member.complete_held_member(lead).await.map_err(|e|e.to_string())
    }))).await.unwrap();
    match held {
        meerkat_mobkit::identity_first::HeldBootstrapStart::Activated(_) => (),
        meerkat_mobkit::identity_first::HeldBootstrapStart::Unsettled(settlement) => {
            let host = slots
                .lock()
                .unwrap()
                .as_ref()
                .and_then(|slot| slot.host.clone());
            let closing = host.as_ref().map(|host| {
                host.begin_close(
                    meerkat_mobkit::rpc_gateway::GatewayHostCloseReason::InitializationFailed,
                )
            });
            let settled = native.finish_held_identity_bootstrap(settlement).await;
            // Outer caught cleanup still retains and joins this exact host,
            // including when either bootstrap or close entry refused.
            assert!(
                closing.as_ref().is_none_or(Result::is_ok),
                "held bootstrap host close refused: {closing:?}"
            );
            panic!("actual held bootstrap unsettled: {settled:?}")
        }
    }
    for row in roster {
        let status = native
            .identity_runtime()
            .unwrap()
            .status(&row.identity)
            .await
            .unwrap();
        assert_eq!(status.state, IdentityLifecycleState::Active);
        let session = status.session_id.unwrap();
        case.audit_sessions
            .insert(row.identity.to_string(), session.clone());
        let actor = service.live_session_actor_witness(&session).await.unwrap();
        let boundary = service
            .acquire_live_session_actor_turn_boundary_lease_exact(&actor)
            .await
            .unwrap()
            .unwrap();
        let pin = service
            .pin_controller_client_for_actor(&boundary)
            .await
            .unwrap();
        drop(boundary);
        meerkat_core::publish_token_lifecycle_acquired_for_identity_within(
            &machine.generated_auth_lease_handle(),
            pin.selection().credential(),
            &meerkat_core::auth::PersistedTokens::api_key("synthetic-shared-public"),
            tokio::time::Instant::now() + BOUND,
        )
        .await
        .unwrap();
    }
    let host = slots
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .host
        .as_ref()
        .unwrap()
        .clone();
    host.publish_runtime_bootstrap_completed(native).unwrap();
}

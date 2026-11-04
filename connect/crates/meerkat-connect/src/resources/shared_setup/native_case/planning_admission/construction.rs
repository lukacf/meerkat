//! Public, all-fresh host composition. Expected definitions are compiler
//! inputs; only registration and the held report capture native destinations.
use super::*;

pub(super) fn inputs() -> PreparedLocalPolicy {
    let mut inputs = prepared_inputs();
    let audience = inputs.native_host.members[0].audience.clone();
    inputs.native_host.members = NAMES
        .iter()
        .map(|name| PreparedNativeMember {
            member: member(name),
            logical_owner: principal(name),
            context_generation: 1,
            audience: audience.clone(),
            setup_capability: None,
        })
        .collect();
    let prototype = inputs.native_host.routes[0].clone();
    inputs.native_host.routes = NAMES
        .iter()
        .map(|name| {
            let mut route = prototype.clone();
            route.id = id(name);
            route.logical_executor = principal(name);
            route.origin = CompiledNativeSourceOrigin::LocalConsole {
                member: member(name),
                requester: principal("human"),
                ingress_actor: principal("sdk"),
            };
            route
        })
        .collect();
    inputs
}
fn tool_policy(_consent: bool) -> PreparedInstalledToolPolicy {
    let mut rows = NAMES
        .iter()
        .enumerate()
        .map(|(index, name)| CompiledMemberToolGrants {
            member_identity: member(name).member,
            grants: if index < 2 {
                vec![CompiledMemberToolGrant {
                    tool_name: toolkit_mobkit::FIND_AND_INSTALL.into(),
                    action: CompiledMemberToolAction::Invoke,
                    consequence: CompiledToolConsequence::R2,
                }]
            } else {
                vec![]
            },
        })
        .collect::<Vec<_>>();
    rows.sort_by(|a, b| a.member_identity.cmp(&b.member_identity));
    let compiled = CompiledApplicationToolPolicy::new(
        PolicyProviderId::new("sdk-planning").unwrap(),
        PolicyId::new("sdk-planning").unwrap(),
        PolicyRevision(1),
        CompiledPolicySourceProvenance {
            source_id: "fresh-sdk-host-selection".into(),
            source_digest: PolicyDigest::from_canonical_bytes(
                b"A and B Find only; P and G controller only",
            ),
        },
        rows,
    )
    .unwrap();
    PreparedInstalledToolPolicy::from_canonical(&compiled.canonical_json().unwrap()).unwrap()
}
pub(super) fn mob_definition() -> MobDefinition {
    let mut definition = definition();
    let ProfileBinding::Inline(template) = definition
        .profiles
        .get(&ProfileName::from("original-a"))
        .unwrap()
    else {
        panic!("inline")
    };
    let template = template.clone();
    definition.profiles.clear();
    for (index, name) in NAMES.iter().enumerate() {
        let mut profile = template.clone();
        profile.tools = ToolConfig {
            comms: true,
            rust_bundles: if index < 2 {
                vec![toolkit_mobkit::FIND_AND_INSTALL_BUNDLE.into()]
            } else {
                vec![]
            },
            ..Default::default()
        };
        profile.external_addressable = *name != "planning-lead";
        profile.peer_description = format!("Fresh SDK admission {name}");
        definition
            .profiles
            .insert(ProfileName::from(*name), ProfileBinding::Inline(profile));
    }
    definition
}
fn expected_surfaces(tools: &InstalledAcquisitionTools) -> BTreeMap<String, Vec<ToolIdentity>> {
    let registered: Arc<dyn AgentToolDispatcher> =
        Arc::new(meerkat_core::RegisteredToolBundleDispatcher::new(
            toolkit_mobkit::FIND_AND_INSTALL_BUNDLE,
            tools.acquisition_tool(),
        ));
    let catalog = meerkat_tools::CatalogControlDispatcher::new(
        registered.clone(),
        Arc::new(meerkat_tools::CatalogControlVisibilityProvider::new()),
    );
    // Governed session registration supplies the configured-MCP owner even
    // when it refuses attachments. Its live router requires Search/Load.
    // Mob members also require comms; these are canonical definitions, not
    // captured runtime identities or grants to invoke the comms operations.
    let comms = meerkat_comms::comms_tool_defs();
    NAMES
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let mut identities = catalog
                .tools()
                .iter()
                .map(|tool| tool.identity())
                .collect::<Vec<_>>();
            identities.extend(comms.iter().map(|tool| tool.identity()));
            if index < 2 {
                identities.extend(registered.tools().iter().map(|tool| {
                    let mut identity = tool.identity();
                    identity.provenance.get_or_insert_with(|| {
                        meerkat_core::ToolProvenance::new(
                            meerkat_core::ToolSourceKind::RustBundle,
                            toolkit_mobkit::FIND_AND_INSTALL_BUNDLE,
                        )
                    });
                    identity
                }));
            }
            (member(name).member, identities)
        })
        .collect()
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
        issuance_id: evidence("selected-g-helper-controller"),
        controller_route: controller_facts().route_key().clone(),
    });
    PreparationAuthorities {
        controllers,
        agent_setup: NAMES[..2]
            .iter()
            .map(|name| AgentSetupCapabilityRequest {
                member: member(name),
                route: id(name),
                issuance_id: evidence(&format!("{name}-find")),
                helper: Some(member("planning-lead")),
                helper_targets: vec![],
            })
            .collect(),
        ..Default::default()
    }
}

pub(super) async fn construct(
    case: &mut Case,
    pipes: toolkit_mobkit::private_host::InheritedHostPipes,
) -> PlanningCase {
    // This fixture's request lifetime stays inside its existing cohort and
    // native case bounds. Custom expiry controls retain their supplied limit.
    let limits = InstalledSetupLimits {
        setup_timeout_ms: BOUND.as_millis().try_into().unwrap(),
        ..InstalledSetupLimits::default()
    };
    construct_with_limits(case, pipes, limits).await
}

pub(super) async fn construct_with_limits(
    case: &mut Case,
    pipes: toolkit_mobkit::private_host::InheritedHostPipes,
    limits: InstalledSetupLimits,
) -> PlanningCase {
    construct_selected(case, pipes, limits, None).await
}

pub(super) async fn construct_with_consent(
    case: &mut Case,
    pipes: toolkit_mobkit::private_host::InheritedHostPipes,
    consent: Arc<super::console_consent_case::ConsentControl>,
) -> PlanningCase {
    construct_selected(
        case,
        pipes,
        InstalledSetupLimits {
            setup_timeout_ms: BOUND.as_millis().try_into().unwrap(),
            ..InstalledSetupLimits::default()
        },
        Some(consent),
    )
    .await
}

async fn construct_selected(
    case: &mut Case,
    pipes: toolkit_mobkit::private_host::InheritedHostPipes,
    limits: InstalledSetupLimits,
    consent: Option<Arc<super::console_consent_case::ConsentControl>>,
) -> PlanningCase {
    let mut owner = PreparedInstallationOwner::open(
        case.directory.path().to_path_buf(),
        inputs(),
        OperatorOwnerStartup::fresh("sdk-fresh-planning", 1).unwrap(),
    )
    .unwrap();
    let mut helper = selection("planning-lead");
    owner
        .configure_authorities(&mut helper, &requests())
        .unwrap();
    let selected = owner.selected().clone();
    let policy = tool_policy(consent.is_some());
    let tools =
        InstalledAcquisitionTools::new(Duration::from_millis(limits.admission_timeout_ms)).unwrap();
    let expected_surfaces = expected_surfaces(&tools);
    let definition = mob_definition();
    let prepared = owner.complete(policy.clone()).await.unwrap();
    let originals = prepared.originals();
    let (selection, application_reference, application_bytes) =
        super::super::super::native_case_artifacts::publish_expected(
            case.directory.path(),
            &selected,
            &policy,
            &expected_surfaces,
            &definition,
        );
    let carrier = PreparedApplicationRequest {
        bundle: application_bytes.clone(),
        declared_digest: application_reference.sha256.clone(),
    };
    let application = PreparedSetupApplication::from_verified_artifact(
        application_reference,
        application_bytes,
        definition.clone(),
        Arc::new(CurrentRoster),
    )
    .unwrap();
    let graph = meerkat_workgraph::WorkGraphService::with_namespace_grant(
        Arc::new(
            meerkat_workgraph::SqliteWorkGraphStore::open(
                case.directory.path().join("workgraph.sqlite3"),
            )
            .unwrap(),
        ),
        meerkat_core::service::WorkGraphNamespaceGrant::new(
            selected.native_host.realm.as_str(),
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
    let launch_graph = graph.clone();
    let launch_census = census.clone();
    let launch_client = case.client.clone();
    let retained_lifetime = case.lifetime.clone();
    let finished = Arc::new(Mutex::new(None));
    let saved = finished.clone();
    let expected_definition = definition.clone();
    let projection: toolkit_mobkit::host_gateway::SetupBootstrapProjection =
        Arc::new(move |spec| {
            if spec.definition != expected_definition {
                return Err(
                    meerkat_toolkit::host_lifetime::control::HostLifetimeError::InvalidSelection,
                );
            }
            Ok(spec)
        });
    let consent_host = consent.clone();
    let server = async move {
        let startup = PrivateHostStartup::read(pipes).await?;
        PrivateHostConnection::bootstrap_prepared_with_controller_reviews(
            startup,
            move |selection| async move {
                let (owner, installed) = prepared.adopt(selection)?;
                let originals = installed.originals();
                *finished.lock().unwrap() = Some(installed);
                Ok((owner, originals))
            },
            move |owner, closed, logins| async move {
                *retained_lifetime.lock().unwrap() = Some(owner.clone());
                let installed = saved.lock().unwrap().take().unwrap();
                let continuation = installed.native_continuation([id("human")].into())?;
                let launch = SdkLaunchOwner::new_shared(
                    continuation.clone(),
                    owner.clone(),
                    id("sdk-workgraph-generation"),
                    launch_client,
                    meerkat_workgraph::WorkOwnerKey::agent("sdk-parent").unwrap(),
                    "sdk-independent-children".into(),
                );
                launch.capture_prepared_backend(launch_graph, &launch_census)?;
                let actions = NAMES[..2]
                    .iter()
                    .flat_map(|name| {
                        [Action::FindAndInstall, Action::Delegate]
                            .into_iter()
                            .map(move |action| ActionPermission {
                                action,
                                resource: id(&member(name).member),
                            })
                    })
                    .collect();
                let installed = installed.with_enrolled_bootstrap_acquisition(
                    InstalledSetupBootstrap {
                        helper,
                        proposal_ceiling: Ceiling {
                            actions,
                            services: BTreeSet::new(),
                            frontends: BTreeSet::new(),
                            output_audiences: [id("human")].into(),
                        },
                        continuation: launch.clone(),
                        backend_generation: id("sdk-workgraph-generation"),
                        references: None,
                        limits,
                        work_step: WorkAdmissionStepOptions {
                            worker: id("sdk-planning-worker"),
                            lease_ms: 60_000,
                            owner_timeout_ms: 10_000,
                            settlement_budget_ms: 10_000,
                        },
                    },
                    tools,
                )?;
                let review_policy = consent_host.as_ref().map(|selected| {
                    selected.clone()
                        as Arc<dyn toolkit_mobkit::installed_host::InstalledOperationReviewPolicy>
                });
                let host = installed
                    .with_operation_review_policy(review_policy)
                    .compose(owner, closed)
                    .with_console_logins(logins)?
                    .with_native_setup_bootstrap(continuation, projection, application)?;
                let host = match &consent_host {
                    Some(consent) => consent.compose(host)?,
                    None => host,
                };
                Ok((host, launch))
            },
        )
        .await
    };
    let client = case.client.clone();
    let request = async move {
        let mut client = client.lock().await;
        let reply = client
            .call(json!({"kind":"bootstrap", "selection":selection}))
            .await;
        if reply.is_err() {
            let _ = client.close().await;
        }
        reply
    };
    let (reply, server) =
        tokio::join!(request, std::panic::AssertUnwindSafe(server).catch_unwind());
    let ((host, launch), private, local, remote) = match server {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => panic!("private bootstrap refused: {error}"),
        Err(panic) => std::panic::resume_unwind(panic),
    };
    case.host = Some(host);
    case.launch = Some(launch.clone());
    case.private = Some(private);
    drop(local);
    drop(remote);
    assert_eq!(reply.unwrap()["kind"], "bootstrap_accepted");
    let host = case.host.as_ref().unwrap();
    assert!(
        host.prepared_application(Some(&carrier))
            .await
            .unwrap()
            .is_some()
    );
    launch.create_parent().await;
    let (backend, parent_work, parent_owner, cycle_label) = launch.workflow_parent().unwrap();
    let retain = |event: PrivateAuthenticatedEvent, content: &'static str| {
        let source = originals.clone();
        async move {
            let authentication = event.original_authentication.clone();
            let original = source.retain_event(event).await.unwrap();
            PrivateOriginalSubmit {
                original,
                content: content.into(),
                transport_authentication: authentication,
            }
        }
    };
    let parent = retain(
        source_event("planning-parent", "sdk-planning-parent", P_TEXT, vec![]),
        P_TEXT,
    )
    .await;
    let lead = retain(
        source_event(
            "planning-lead",
            "sdk-planning-lead",
            G_TEXT,
            vec![parent.original.original.clone()],
        ),
        G_TEXT,
    )
    .await;
    let configuration = host.native_grant_work_configuration().unwrap().unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let deadline = now + 180_000;
    let bounds = ExecutionRestrictions {
        delegation_depth: DelegationDepth::remaining(1),
        lifetime: LifetimeRestriction::window(now, deadline + 60_000),
        ..ExecutionRestrictions::unrestricted()
    };
    let native = &selected.native_host;
    let route = native
        .routes
        .iter()
        .find(|route| route.id == lead.original.route)
        .unwrap();
    let operation_root = configuration
        .grants
        .issue_root(
            native
                .remote_operator
                .as_ref()
                .expect("selected application root issuer"),
            evidence("sdk-independent-g-root"),
            native.grant_administrator.clone(),
            route.represented_subject.clone(),
            bounds.clone(),
        )
        .unwrap();
    let child_bounds = bounds
        .for_child(&ExecutionRestrictions {
            delegation_depth: DelegationDepth::remaining(0),
            ..ExecutionRestrictions::unrestricted()
        })
        .unwrap();
    let operation_leaf = configuration
        .grants
        .issue_child(
            &native.grant_administrator,
            &operation_root,
            evidence("sdk-independent-g-leaf"),
            route.logical_executor.clone(),
            child_bounds.clone(),
        )
        .unwrap();
    let lineage = vec![operation_root, operation_leaf];
    // Match the same current compiled policy identity that the actual
    // installed admission owner binds into each request's evidence.
    let lifetime = case.lifetime.lock().unwrap().as_ref().unwrap().clone();
    let current_policy = lifetime.current_setup_policy().unwrap();
    let authority_revision = Revision::new(
        ContentDigest::of_bytes(&canonical_json(&current_policy).unwrap()).to_string(),
    )
    .unwrap();
    let terms = SetupWorkflowTerms {
        key: id("sdk-preselected-cohort"),
        backend,
        parent: parent_work,
        parent_owner,
        cycle_label,
        requester: principal("human"),
        realm: native.realm.clone(),
        authority_revision,
        planning_source: originals.source_record_key(&lead.original).await.unwrap(),
        lead: member("planning-lead"),
        envelope_digest: ContentDigest::of_bytes(&canonical_json(&child_bounds).unwrap()),
        deadline_ms: deadline,
        max_waiters: 8,
    };
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
    let workflow_member = case
        .host
        .as_mut()
        .unwrap()
        .reserve_workflow_member(&terms, &lead.original, &roster[3])
        .await
        .unwrap();
    assert!(workflow_member.session().is_err());
    let member_mcp_custody = meerkat_mobkit::MemberMcpCustody::new();
    let script = Arc::new(PlanningScript {
        consent: consent.clone(),
        ..PlanningScript::default()
    });
    let review = case
        .host
        .as_ref()
        .unwrap()
        .selected_operation_review()
        .unwrap();
    let mut factory = member_mcp_custody.install(AgentFactory::minimal().comms(false));
    if let Some(review) = &review {
        factory = factory.with_operation_review(review.clone());
    }
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    builder.default_llm_client = Some(script.clone());
    *builder.default_agent_llm_client_decorator.write().unwrap() = Some(Arc::new(|client| {
        assert!(client.controller_model_selection().as_ref() == Some(&model_selection()));
        client
    }));
    let mob_tools = builder.default_mob_tools.clone();
    let mut runtime_store = meerkat_runtime::store::SqliteRuntimeStore::new(
        case.directory.path().join("native-runtime.sqlite3"),
    )
    .unwrap();
    if consent.is_some() {
        // This consent fixture owns the real hosting paths that supply the
        // native work's scope. A database alone does not confer that capability.
        let root = case.directory.path().canonicalize().unwrap();
        let hosting_lock_dir = root.join("native-hosting");
        std::fs::create_dir_all(&hosting_lock_dir).unwrap();
        runtime_store =
            runtime_store.with_hosting_paths(meerkat_core::session_hosting::HostingPaths {
                hosting_lock_dir,
                cold_delivery_lock: root.join("native-cold-delivery.lock"),
                database: None,
            });
        assert!(
            meerkat_runtime::store::RuntimeStore::hosting_capability(&runtime_store)
                .domain_ref()
                .is_some(),
            "the consent fixture must obtain an actual hosting domain"
        );
    }
    let persistence = meerkat::PersistenceBundle::new_with_local_grant_authorization(
        Arc::new(
            meerkat::SqliteSessionStore::open(
                case.directory.path().join("native-sessions.sqlite3"),
            )
            .unwrap(),
        ),
        Arc::new(runtime_store),
        Arc::new(meerkat::MemoryBlobStore::new()),
        configuration,
    )
    .unwrap();
    let history = persistence.runtime_store();
    case.audit_store = Some(history.clone());
    let jobs = persistence.job_store();
    // The native runtime-backed builder installs this factory's selected
    // review owner-loss settlement before session registration. Installing it
    // again here would refuse AlreadyInstalled, not add another cleanup owner.
    let (service, machine) =
        meerkat::surface::build_runtime_backed_service(builder, 4, persistence).unwrap();
    let service = Arc::new(service);
    let mut spec = MobBootstrapSpec::new(definition, MobStorage::in_memory(), service.clone())
        .with_member_mcp_custody(member_mcp_custody)
        .with_session_runtime_adapter(machine.clone())
        .unwrap()
        .with_declared_ephemeral_mob_storage()
        .with_workgraph_service(Some(graph.clone()))
        .with_runtime_delivery(meerkat_mobkit::MobRuntimeDelivery::new(history, jobs))
        .with_agent_mob_tools(mob_tools)
        .unwrap();
    spec.resolved_storage = Some(census);
    let host = case.host.as_ref().unwrap();
    let spec = host.prepare_bootstrap(spec).await.unwrap();
    assert_eq!(
        spec.workgraph_service().unwrap().namespace_grant(),
        graph.namespace_grant()
    );
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
    let actual = case.native.as_mut().unwrap();
    host.bind_runtime(actual).await.unwrap();
    let handle = actual.mob_runtime().handle();
    let bridge = MobSessionBridge::with_session_service(
        handle.clone(),
        actual.mob_runtime().session_service().cloned().unwrap(),
    )
    .with_controller_pin_source(service.clone());
    let runtime = Arc::new(
        IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory().unwrap()),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "sdk-fresh-planning".into(),
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
    actual.install_identity_first_context(context);
    if let Some(consent) = &consent {
        consent.configure_runtime(actual);
    }
    host.bind_identity_runtime(actual).await.unwrap();
    let complete = workflow_member.clone();
    let completed = Arc::new(Mutex::new(Vec::new()));
    let observed = completed.clone();
    let held = actual
        .bootstrap_installed_identity_first_context_held_until(
            &roster,
            tokio::time::Instant::now() + BOUND,
            Box::new(move |members, _completion| {
                Box::pin(async move {
                    if members.len() != NAMES.len() {
                        return Err("incomplete actual held roster".into());
                    }
                    for row in members.iter() {
                        let report = row
                            .controller_route
                            .report()
                            .ok_or("actual controller route unavailable")?;
                        if report.key != controller_facts().route_key() {
                            return Err("actual controller route changed".into());
                        }
                        let mut actual = row.finished_surface.compiled_against.tools.clone();
                        if let PreparedToolCatalog::Complete(entries) =
                            &row.finished_surface.compiled_against.catalog
                        {
                            for entry in entries {
                                if !actual.contains(&entry.identity) {
                                    actual.push(entry.identity.clone());
                                }
                            }
                        }
                        let expected = expected_surfaces
                            .get(&member(row.identity.as_str()).member)
                            .ok_or("unexpected held member")?;
                        if actual.len() != expected.len()
                            || actual.iter().any(|tool| !expected.contains(tool))
                        {
                            return Err(
                                "actual native surface differs from selected definitions".into()
                            );
                        }
                        observed.lock().unwrap().push(row.identity.clone());
                    }
                    let lead = members
                        .iter()
                        .find(|row| row.identity.as_str() == "planning-lead")
                        .ok_or("missing G")?;
                    complete
                        .complete_held_member(lead)
                        .await
                        .map_err(|error| error.to_string())
                })
            }),
        )
        .await
        .unwrap();
    match held {
        meerkat_mobkit::identity_first::HeldBootstrapStart::Activated(_) => (),
        meerkat_mobkit::identity_first::HeldBootstrapStart::Unsettled(settlement) => {
            let closing = host.begin_close(
                meerkat_mobkit::rpc_gateway::GatewayHostCloseReason::InitializationFailed,
            );
            let settled = actual.finish_held_identity_bootstrap(settlement).await;
            closing.unwrap();
            panic!("actual held bootstrap did not activate: {settled:?}");
        }
    }
    if let Some(consent) = &consent {
        consent.bind_routes(actual, host);
    }
    assert_eq!(completed.lock().unwrap().len(), 4);
    let runtime = actual.identity_runtime().unwrap();
    for row in &roster {
        let status = runtime.status(&row.identity).await.unwrap();
        assert_eq!(status.state, IdentityLifecycleState::Active);
        let session = status.session_id.unwrap();
        case.audit_sessions
            .insert(row.identity.to_string(), session.clone());
        let actor = service.live_session_actor_witness(&session).await.unwrap();
        let boundary = service
            .acquire_live_session_actor_turn_boundary_lease_exact(&actor)
            .await
            .unwrap()
            .expect("actual held actor remains current");
        let pin = service
            .pin_controller_client_for_actor(&boundary)
            .await
            .unwrap();
        drop(boundary);
        assert_eq!(pin.selection(), &model_selection());
        meerkat_core::publish_token_lifecycle_acquired_for_identity_within(
            &machine.generated_auth_lease_handle(),
            pin.selection().credential(),
            &meerkat_core::auth::PersistedTokens::api_key("synthetic-sdk-planning"),
            tokio::time::Instant::now() + BOUND,
        )
        .await
        .unwrap();
    }
    assert_eq!(
        case.audit_sessions.values().collect::<BTreeSet<_>>().len(),
        4
    );
    assert_eq!(
        workflow_member.session().unwrap(),
        case.audit_sessions["planning-lead"]
    );
    for name in NAMES {
        assert_eq!(input_count(case, name).await, 0);
    }
    assert!(script.calls.lock().unwrap().is_empty());
    PlanningCase {
        source: originals,
        terms,
        parent,
        lead,
        lineage,
        member: workflow_member,
        script,
    }
}

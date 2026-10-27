//! Inert compiler controls. Direct capture uses synthetic native surface values;
//! it does not qualify registration, native finish or withheld publication.
use super::*;
use meerkat_core::{PreparedCatalogEntry, ToolCallability, ToolDef, ToolPlaneClass};
use serde_json::json;

fn selected(root: &Path) -> ApplicationSetupCompilationInput {
    let legacy = super::super::tests::input(root);
    let mut realm = legacy.realm;
    realm.realm_id = meerkat_core::mob_realm_id(&legacy.helper.member.mob_id)
        .unwrap()
        .to_string();
    let members = legacy
        .native_members
        .into_iter()
        .map(|row| ApplicationMemberSelection {
            member: row.member,
            logical_owner: row.logical_owner,
            context_generation: row.context_generation,
            audience: row.audience,
            setup_capability: row.setup_capability,
        })
        .collect();
    ApplicationSetupCompilationInput {
        schema: 4,
        realm,
        definition: legacy.definition,
        roster: legacy.roster,
        members,
        helper: legacy.helper,
        helper_tools: legacy.helper_tools,
        helper_profile: legacy.helper_profile,
        tool_policy: legacy.tool_policy,
    }
}
fn identity(name: &str) -> ToolIdentity {
    ToolDef::new(name, "fixture", json!({})).identity()
}
fn surface(helper: bool) -> PreparedToolSurface {
    let mut tools = vec![identity("research"), identity("hidden")];
    if helper {
        tools.push(
            toolkit_mobkit::installed_host::acquisition::setup_report_definition().identity(),
        );
    }
    PreparedToolSurface {
        projected: vec![tools[0].clone()],
        tools,
        catalog: PreparedToolCatalog::Complete(vec![PreparedCatalogEntry {
            identity: identity("lazy"),
            plane: ToolPlaneClass::Session,
            callability: ToolCallability::Callable,
            deferred_eligible: true,
        }]),
    }
}
fn phase(root: &Path) -> ApplicationSetupCompilation {
    phase_with_operator(root, None)
}
fn phase_with_operator(root: &Path, operator: Option<meerkat_core::PrincipalRef>) -> ApplicationSetupCompilation {
    let phase = selected(root)
        .validate()
        .unwrap()
        .prepare(super::super::tests::prepared(root))
        .unwrap();
    phase.retain_owner(owner_selection(&phase, operator)).unwrap();
    phase
}

fn owner_selection(phase: &ApplicationSetupCompilation, operator: Option<meerkat_core::PrincipalRef>)
    -> meerkat_toolkit::host_lifetime::policy::PreparedLocalPolicy
{
    // Mechanical artifact selection only. This is not issuance, currentness,
    // native held completion or a controller observation.
    let authority = phase.input.members[0].logical_owner.clone();
    meerkat_toolkit::host_lifetime::policy::PreparedLocalPolicy {
        deployment: toolkit_package::Id::new("compiler-fixture").unwrap(), components: vec![], channels: vec![],
        native_host: meerkat_toolkit::host_lifetime::policy::PreparedNativeHost {
            realm: meerkat_core::mob_realm_id(&phase.input.helper.member.mob_id).unwrap(),
            grant_administrator: authority.clone(), remote_operator: operator,
            service_parent_administration: None,
            ingress_domain: meerkat_authorization_contracts::constraints::ResourceDomain {
                authority, namespace: "compiler-fixture".into(),
            },
            members: phase.input.members.clone(), routes: vec![],
        },
    }
}

#[test]
fn held_controller_target_uses_selected_member_and_exact_roster_identity() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let phase = phase(&root);
    for row in &phase.input.members {
        let index = matching_roster(&phase.input.roster, &row.member).unwrap();
        assert_eq!(phase.held_member_identity(&row.member).unwrap(), &phase.input.roster[index].identity);
        let mut wrong_role = row.member.clone();
        wrong_role.role.push_str("-foreign");
        assert!(phase.held_member_identity(&wrong_role).is_err());
        let mut wrong_mob = row.member.clone();
        wrong_mob.mob_id.push_str("-foreign");
        assert!(phase.held_member_identity(&wrong_mob).is_err());
    }
    assert_eq!(phase.held_member_identity(&phase.helper().member).unwrap(),
        &phase.source_helper_roster.identity);
    if phase.source_helper_member != phase.helper().member {
        assert!(phase.held_member_identity(&phase.source_helper_member).is_err(),
            "the original role cannot replace the compiler's projected helper binding");
    }
}

#[test]
fn held_finished_inventory_refuses_missing_repeated_and_cross_member_surfaces() {
    // These are inert compiler values, not evidence of a native held finish.
    // The actual Driver must receive its inventory from the native callback.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let phase = phase(&root);
    let ordinary = phase.capture(0, &surface(false)).unwrap();
    let helper = phase.capture(1, &surface(true)).unwrap();
    let ordinary_identity = phase.held_member_identity(&phase.input.members[0].member).unwrap();
    let helper_identity = phase.held_member_identity(&phase.helper().member).unwrap();
    assert_ne!(ordinary_identity, helper_identity);
    assert!(phase.validate_finished([(ordinary_identity, &ordinary), (helper_identity, &helper)]).is_ok());
    assert!(phase.validate_finished([(ordinary_identity, &ordinary)]).is_err());
    assert!(phase.validate_finished([(ordinary_identity, &ordinary), (ordinary_identity, &ordinary)]).is_err());
    assert!(phase.validate_finished([(ordinary_identity, &helper), (helper_identity, &ordinary)]).is_err());
}

#[test]
fn captured_surfaces_require_final_owner_and_project_late_helper_and_member_authority() {
    use meerkat_toolkit::host_lifetime::policy::{CompiledAgentSetupCapability,
        CompiledNativeSourceOrigin, CompiledNativeSourceRoute};
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let mut input = selected(&root);
    let parent = input.helper.controller_parent.clone();
    input.helper.controller_parent.clear();
    let phase = input.validate().unwrap().prepare(super::super::tests::prepared(&root)).unwrap();
    phase.capture(0, &surface(false)).unwrap();
    phase.capture(1, &surface(true)).unwrap();
    let output = root.join("late-authority");
    assert!(phase.complete_bundle(&output).is_err());
    assert!(!output.exists());

    let mut selected = owner_selection(&phase, None);
    let route = toolkit_package::Id::new("late-setup-route").unwrap();
    let ordinary = &selected.native_host.members[0];
    let principal = ordinary.logical_owner.clone();
    let domain = selected.native_host.ingress_domain.clone();
    selected.native_host.routes.push(CompiledNativeSourceRoute {
        id: route.clone(), origin: CompiledNativeSourceOrigin::LocalConsole {
            member: ordinary.member.clone(), requester: principal.clone(), ingress_actor: principal.clone(),
        }, application_parent: None, controller_parent: None,
        logical_executor: principal.clone(), represented_subject: None, source_authority: principal,
        authentication_domain: domain.clone(), event_domain: domain,
    });
    selected.native_host.members[0].setup_capability = Some(CompiledAgentSetupCapability {
        helper: None,
        route, parent: parent[0].clone(), helper_targets: vec![],
    });
    let mut helper = phase.helper().clone();
    helper.controller_parent = parent;
    phase.retain_authorities(selected.clone(), helper.clone()).unwrap();
    let (setup, _) = phase.complete_bundle(&output).unwrap();
    assert!(setup.helper == helper);
    assert_eq!(setup.native_owner.as_ref().unwrap(), &selected);
    assert_eq!(setup.native_members[0].setup_capability, selected.native_host.members[0].setup_capability);
    assert!(phase.captured.lock().unwrap()[&0].member.setup_capability.is_none(),
        "early surface capture is not rewritten into authority evidence");
    assert!(phase.retain_authorities(selected, helper).is_err());
}


#[test]
fn task_parent_admin_rule_survives_exact_owner_projection_without_implicit_operator() {
    use meerkat_toolkit::host_lifetime::policy::{CompiledNativeSourceOrigin,
        CompiledNativeSourceRoute, ServiceParentAdministration};
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let phase = selected(&root).validate().unwrap().prepare(super::super::tests::prepared(&root)).unwrap();
    let realm = meerkat_core::mob_realm_id(&phase.input.helper.member.mob_id).unwrap();
    let operator = meerkat::governed_realm::stock::StockPrincipals::for_local_operator(&realm).unwrap().operator;
    let mut early = owner_selection(&phase, Some(operator.clone()));
    let helper = early.native_host.members.iter_mut().find(|row| row.member == phase.input.helper.member).unwrap();
    helper.member = phase.source_helper_member.clone();
    let principal = helper.logical_owner.clone();
    let route = toolkit_package::Id::new("administrative-service-route").unwrap();
    let domain = early.native_host.ingress_domain.clone();
    early.native_host.routes.push(CompiledNativeSourceRoute {
        id: route.clone(), origin: CompiledNativeSourceOrigin::LocalConsole {
            member: phase.source_helper_member.clone(), requester: principal.clone(), ingress_actor: principal.clone(),
        }, application_parent: None, controller_parent: None,
        logical_executor: principal.clone(), represented_subject: None, source_authority: principal,
        authentication_domain: domain.clone(), event_domain: domain,
    });
    early.native_host.service_parent_administration = Some(ServiceParentAdministration::ApplicationPolicyTasks {
        routes: [route.clone()].into_iter().collect(),
    });
    let mut missing_operator = early.clone();
    missing_operator.native_host.remote_operator = None;
    assert!(phase.project_policy(missing_operator).is_err());
    let mut wrong_route = early.clone();
    wrong_route.native_host.service_parent_administration = Some(ServiceParentAdministration::ApplicationPolicyTasks {
        routes: [toolkit_package::Id::new("absent-route").unwrap()].into_iter().collect(),
    });
    assert!(phase.project_policy(wrong_route).is_err());
    let projected = phase.project_policy(early.clone()).unwrap();
    assert_eq!(projected.native_host.remote_operator, Some(operator));
    assert_eq!(projected.native_host.service_parent_administration, early.native_host.service_parent_administration);
    assert_eq!(projected.native_host.routes[0].id, route);
    let CompiledNativeSourceOrigin::LocalConsole { member, .. } = &projected.native_host.routes[0].origin else { panic!("exact source kind"); };
    assert_eq!(member, &phase.input.helper.member);
    assert!(projected.native_host.routes[0].application_parent.is_none());
    phase.retain_owner(projected.clone()).unwrap();
    phase.capture(0, &surface(false)).unwrap();
    phase.capture(1, &surface(true)).unwrap();
    let (completed, _) = phase.complete_bundle(&root.join("admin-rule-output")).unwrap();
    assert_eq!(completed.native_owner.as_ref(), Some(&projected));
    // This fixture proves compiler propagation/validation, not authenticated
    // administration, native held completion or issuance of a service parent.
}

#[test]
fn final_authority_cannot_replace_destinations_or_explicit_helper_selection() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let phase = selected(&root).validate().unwrap().prepare(super::super::tests::prepared(&root)).unwrap();
    let selected = owner_selection(&phase, None);
    let mut changed = selected.clone();
    changed.native_host.members[0].context_generation += 1;
    assert!(phase.retain_authorities(changed, phase.helper().clone()).is_err());
    let mut helper = phase.helper().clone();
    helper.operation_basis = meerkat_authorization_contracts::work_association::WorkAuthorityBasis::TargetedGrantLineages {
        base: None, targets: vec![],
    };
    assert!(phase.retain_authorities(selected.clone(), helper).is_err());
    let mut helper = phase.helper().clone();
    helper.controller_parent.clear();
    assert!(phase.retain_authorities(selected.clone(), helper).is_err());
    assert!(phase.final_authorities.get().is_none());
    phase.retain_authorities(selected, phase.helper().clone()).unwrap();
}

#[test]
fn early_phase_needs_no_original_surface_and_preserves_explicit_policy() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let input = selected(&root);
    let original = input.clone();
    std::fs::remove_file(root.join("helper-surface.json")).unwrap();
    let validated = input.validate().unwrap();
    let phase = validated
        .prepare(super::super::tests::prepared(&root))
        .unwrap();
    assert_eq!(phase.policy_bytes, original.tool_policy.compile().unwrap());
    assert!(Arc::ptr_eq(&phase.registry(), &phase.registry()));
    assert_eq!(phase.helper().member.member, original.helper.member.member);
    assert_ne!(phase.helper().member.role, original.helper.member.role);
    let projected = phase.project_roster(&original.roster).unwrap();
    assert_eq!(projected[0], original.roster[0]);
    assert_eq!(projected[1].profile.as_str(), phase.helper().member.role);
    assert!(phase.captured.lock().unwrap().is_empty());
}
#[test]
fn dedicated_public_research_compilation_matches_driver_and_retained_resource() {
    use meerkat_core::{CompiledMemberToolAction, CompiledMemberToolGrant, CompiledToolConsequence};
    use meerkat_mobkit::mob_handle_runtime::MobBootstrapSpec;
    for enabled in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let mut input = selected(&root);
        input.helper_tools = ApplicationSetupTools::CheckedInstallation;
        assert_eq!(input.helper_profile, ApplicationSetupProfile::Dedicated);
        let names = ["setup_read_github", "setup_search_github"];
        if enabled {
            let member = input.tool_policy.members.iter_mut()
                .find(|member| member.member_identity == input.helper.member.member).unwrap();
            member.grants.extend(names.map(|name| CompiledMemberToolGrant {
                tool_name: name.into(), action: CompiledMemberToolAction::Invoke,
                consequence: CompiledToolConsequence::R1,
            }));
            member.grants.sort_by(|a, b| a.tool_name.cmp(&b.tool_name));
        }
        let expected_policy = input.tool_policy.clone().compile().unwrap();
        // This is the production order: capture the reserved definition first,
        // then invoke the same bootstrap composition called by Driver.
        let phase = input.validate().unwrap().prepare(super::super::tests::prepared(&root)).unwrap();
        let service = Arc::new(meerkat_session::EphemeralSessionService::new(
            ReportBundleSurfaceProbe(Arc::new(Mutex::new(None))), 1,
        ));
        let spec = MobBootstrapSpec::new(phase.definition().clone(),
            meerkat_mob::MobStorage::in_memory(), service);
        let spec = phase.prepare_helper_bootstrap(spec).unwrap();
        assert_eq!(&spec.definition, phase.definition());
        assert_eq!(phase.policy_bytes, expected_policy);
        assert_eq!(phase.resource.helper_projection().unwrap().server(), phase.server());
        for name in names {
            assert_eq!(phase.server().access.as_ref().unwrap().permits(name), enabled);
        }
        // Complete only an inert synthetic surface fixture here; this is not
        // evidence of native held completion or a live MCP operation.
        phase.retain_owner(owner_selection(&phase, None)).unwrap();
        phase.capture(0, &surface(false)).unwrap();
        let mut helper_surface = surface(true);
        if enabled { helper_surface.tools.extend(names.map(identity)); }
        phase.capture(1, &helper_surface).unwrap();
        let (setup, _) = phase.complete_bundle(&root.join("complete")).unwrap();
        let retained = setup.helper_projection().unwrap();
        assert_eq!(retained.server(), phase.server());
        assert_eq!(setup.helper_projection_for_policy(phase.policy.compiled()).unwrap().server(),
            &setup.server);
        let mut opposite_policy = phase.policy.compiled().clone();
        let selected_member = opposite_policy.members.iter_mut()
            .find(|member| member.member_identity == setup.helper.member.member).unwrap();
        if enabled {
            selected_member.grants.retain(|grant| !names.contains(&grant.tool_name.as_str()));
        } else {
            selected_member.grants.extend(names.map(|name| CompiledMemberToolGrant {
                tool_name: name.into(), action: CompiledMemberToolAction::Invoke,
                consequence: CompiledToolConsequence::R1,
            }));
            selected_member.grants.sort_by(|a, b| a.tool_name.cmp(&b.tool_name));
        }
        assert!(setup.helper_projection_for_policy(&opposite_policy).is_err(),
            "both missing and extra policy-selected research entries refuse");
        let completed = super::super::resource::SetupResourceSelection::from_completed(&setup).unwrap();
        assert_eq!(completed.helper_projection().unwrap().server(), phase.server());
        let mut unexpected = setup.clone();
        unexpected.server.access.as_mut().unwrap().allowed_operations.insert("unselected_operation".into());
        assert!(unexpected.helper_projection().is_err(), "unknown ceiling entries still refuse");
    }
}

#[test]
fn completion_requires_every_selected_output_and_copies_exact_full_bound() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let phase = phase(&root);
    let output = root.join("complete");
    assert!(phase.complete_bundle(&output).is_err());
    assert!(!output.exists());
    let ordinary = surface(false);
    phase.capture(0, &ordinary).unwrap();
    assert!(phase.complete_bundle(&output).is_err());
    assert!(!output.exists());
    let helper = surface(true);
    let selection = phase.capture(1, &helper).unwrap();
    assert_eq!(selection.compiled_against, helper);
    assert!(phase.capture(1, &helper).is_err());
    // Inert fixture stands in for the producer only at compiler completion.
    // It does not assert that the native preparations finished.
    let (setup, manifest) = phase.complete_bundle(&output).unwrap();
    assert_eq!(setup.schema, 4);
    assert!(setup.helper_source_surface.is_none());
    assert!(setup.helper_projection().is_ok());
    let artifacts = validate_final_artifacts(&setup).unwrap();
    assert_eq!(artifacts.len(), 2);
    for (index, (name, bytes)) in artifacts.iter().enumerate() {
        assert_eq!(std::fs::read(output.join(name)).unwrap(), *bytes);
        let identities: Vec<ToolIdentity> = serde_json::from_slice(bytes).unwrap();
        assert!(identities.contains(&identity("hidden")));
        assert!(identities.contains(&identity("lazy")));
        assert_eq!(
            setup.native_members[index].initial_surface.sha256,
            hash(bytes)
        );
    }
    assert_eq!(
        manifest.sha256,
        hash(&std::fs::read(manifest.path).unwrap())
    );
    assert!(phase.complete_bundle(&output).is_err());
}
#[test]
fn missing_report_and_changed_artifacts_refuse_without_fabricating_identity() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let phase = phase(&root);
    assert!(phase.capture(1, &surface(false)).is_err());
    assert!(
        !phase
            .resources
            .artifact_directory
            .join(member_surface_name(&phase.helper().member).unwrap())
            .exists()
    );
    phase.capture(0, &surface(false)).unwrap();
    phase.capture(1, &surface(true)).unwrap();
    let (mut setup, _) = phase.complete_bundle(&root.join("complete")).unwrap();
    setup.native_members[0].initial_surface.name =
        setup.native_members[1].initial_surface.name.clone();
    assert!(setup.helper_projection().is_err());
    let captured = phase.captured.lock().unwrap();
    let path = phase
        .resources
        .artifact_directory
        .join(&captured[&0].member.initial_surface.name);
    std::fs::write(path, b"[]").unwrap();
    drop(captured);
    assert!(phase.complete_bundle(&root.join("changed")).is_err());
    assert!(!root.join("changed").exists());
}
#[test]
fn full_surface_never_invents_a_visible_only_catalog_or_drops_conflicts() {
    let mut actual = surface(false);
    let identities = full_identities(&actual).unwrap();
    assert_eq!(identities.len(), 3);
    actual.catalog = PreparedToolCatalog::VisibleOnly;
    assert_eq!(full_identities(&actual).unwrap(), actual.tools);
    actual.projected.push(identity("foreign"));
    assert!(full_identities(&actual).is_err());
    actual = surface(false);
    actual.tools.push(actual.tools[0].clone());
    assert!(full_identities(&actual).is_err());
    actual = surface(false);
    if let PreparedToolCatalog::Complete(entries) = &mut actual.catalog {
        entries.push(entries[0].clone());
    }
    assert!(full_identities(&actual).is_err());
}
#[test]
fn input_cannot_drop_current_members_or_use_a_different_native_realm() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let mut input = selected(&root);
    input.members.remove(0);
    assert!(input.validate().is_err());
    let mut input = selected(&root);
    input.realm.realm_id = "default".into();
    assert!(input.validate().is_err());
    let mut input = selected(&root);
    input.members[0].member.role = "wrong-profile".into();
    assert!(input.validate().is_err());
    let mut input = selected(&root);
    input.tool_policy.members[1].grants.clear();
    assert!(input.validate().is_err());
}
#[test]
fn legacy_completed_bytes_round_trip_with_required_original_source() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let (legacy, _) = super::super::finish(
        super::super::tests::input(&root).validate().unwrap(),
        super::super::tests::prepared(&root),
    )
    .unwrap();
    let bytes = toolkit_package::canonical_json(&legacy).unwrap();
    #[derive(Serialize, Deserialize)]
    struct LegacySourceField {
        helper_source_surface: SelectedHostArtifact,
        #[serde(flatten)]
        remaining: BTreeMap<String, serde_json::Value>,
    }
    let old: LegacySourceField = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(toolkit_package::canonical_json(&old).unwrap(), bytes);
    let decoded: PreparedApplicationSetup = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(toolkit_package::canonical_json(&decoded).unwrap(), bytes);
    assert!(decoded.helper_projection().is_ok());
    let mut missing = decoded.clone();
    missing.helper_source_surface = None;
    assert!(missing.helper_projection().is_err());
    let mut relabeled = decoded;
    relabeled.schema = 4;
    assert!(relabeled.helper_projection().is_err());
}

#[test]
fn standalone_command_refuses_schema4_before_resource_effects() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let path = root.join("input.json");
    let output = root.join("output");
    std::fs::write(
        &path,
        toolkit_package::canonical_json(&selected(&root)).unwrap(),
    )
    .unwrap();
    let mut progress = InstallerProgress::default();
    let result = super::super::command(
        &[
            "--prepare-application-setup".into(),
            "--input".into(),
            path.to_str().unwrap().into(),
            "--output".into(),
            output.to_str().unwrap().into(),
        ],
        &mut progress,
    );
    assert!(
        result
            .unwrap_err()
            .contains("native producer/activation bridge")
    );
    assert!(!progress.effects_may_have_occurred);
    assert!(!output.exists());
}


#[test]
fn schema4_owner_output_is_required_and_matches_finished_member_facts() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let unowned = selected(&root).validate().unwrap().prepare(super::super::tests::prepared(&root)).unwrap();
    unowned.capture(0, &surface(false)).unwrap();
    unowned.capture(1, &surface(true)).unwrap();
    assert!(unowned.complete_bundle(&root.join("unowned")).err().unwrap().contains("owner"));
    assert!(!root.join("unowned").exists());
    // Use a separate preparation's retained artifact namespace for the owned
    // compiler output; never reuse the previous append-only capture files.
    let owned_dir = tempfile::tempdir().unwrap();
    let owned_root = owned_dir.path().canonicalize().unwrap();
    let owned = phase(&owned_root);
    owned.capture(0, &surface(false)).unwrap();
    owned.capture(1, &surface(true)).unwrap();
    let (mut setup, _) = owned.complete_bundle(&owned_root.join("complete")).unwrap();
    assert!(setup.native_owner.is_some());
    let mut missing = setup.clone();
    missing.native_owner = None;
    assert!(missing.helper_projection().is_err());
    setup.native_owner.as_mut().unwrap().native_host.members[0].context_generation += 1;
    assert!(setup.helper_projection().is_err());
}

// The following controls use actual native registration/bindings/retirement and
// the production resource provider. Selecting the helper SessionId directly is
// a mechanical compiler-hook port, not proof of the held gateway/HTTP path.
async fn registration_fixture(root: &Path) -> (
    Arc<ApplicationSetupCompilation>, Arc<meerkat_runtime::MeerkatMachine>,
    toolkit_mobkit::installed_host::PreparedInstalledHost,
) {
    registration_fixture_from_phase(root, phase(root)).await
}

async fn registration_fixture_from_phase(root: &Path, phase: ApplicationSetupCompilation) -> (
    Arc<ApplicationSetupCompilation>, Arc<meerkat_runtime::MeerkatMachine>,
    toolkit_mobkit::installed_host::PreparedInstalledHost,
) {
    use toolkit_mobkit::installed_host::{PreparedInstallationOwner, operator_setup::OperatorOwnerStartup};
    let phase = Arc::new(phase);
    let mut selected = phase.final_authorities.get().unwrap().owner.clone();
    selected.native_host.remote_operator = Some(meerkat_core::PrincipalRef::in_domain(
        meerkat_core::PrincipalKind::ServiceAccount, "registration-admin",
        meerkat_core::TrustDomainId::new("application-setup-fixture").unwrap(),
    ).unwrap());
    std::fs::create_dir_all(root.join("shared/data")).unwrap();
    let prepared = PreparedInstallationOwner::open(root.to_owned(), selected,
        OperatorOwnerStartup::Initialize {
            namespace: meerkat_authorization_contracts::evidence::EvidenceId::new("registration-fixture").unwrap(), generation: 1,
        }).unwrap().complete(phase.prepared_tool_policy()).await.unwrap();
    let mut configuration = prepared.take_configuration().unwrap();
    configuration.configured_mcp_attachment = Some(Arc::new(crate::application_setup::resource::SetupResourceProvider::new(
        phase.resource_selection(), configuration.configured_mcp_attachment.take(),
    )));
    let machine = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral().with_local_grant_authorization(configuration).unwrap());
    (phase, machine, prepared)
}

#[tokio::test]
async fn selected_live_unbound_slot_authorizes_and_native_retirement_refuses() {
    use meerkat_core::mcp_attachment::McpAttachmentRefusal;
    use meerkat_runtime::meerkat_machine::AgentBindingUnavailable;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let (phase, machine, _owner) = registration_fixture(&root).await;
    let session = meerkat_core::SessionId::new();
    machine.register_session(session.clone()).await.unwrap();
    let bindings = machine.prepare_bindings(session.clone()).await.unwrap();
    let authority = bindings.configured_mcp_attachment().cloned().unwrap();
    // Provider construction happens before native destination selection. It
    // must retain the exact slot while refusing physical use at this stage.
    assert_eq!(authority.authorize_configured(phase.server()), Err(McpAttachmentRefusal::NotCurrent));
    let participant = phase.resource.registration_slots.lock().unwrap().get(&session.to_string()).unwrap().upgrade().unwrap();
    phase.resource.select_registration(&meerkat_authorization_contracts::evidence::EvidenceId::new(session.to_string()).unwrap(), true).unwrap();
    assert!(phase.resource.registration_slots.lock().unwrap().is_empty(), "temporary correlation is removed on helper selection");
    assert!(matches!(participant.agent.current(), Err(AgentBindingUnavailable::Unbound)));
    authority.authorize_configured(phase.server()).unwrap();
    machine.unregister_session(&session).await.unwrap();
    assert!(matches!(participant.agent.current(), Err(AgentBindingUnavailable::Retired)));
    assert_eq!(authority.authorize_configured(phase.server()), Err(McpAttachmentRefusal::NotCurrent));
}

#[tokio::test]
async fn same_session_replacement_before_first_physical_use_cannot_inherit_preparation() {
    use meerkat_core::mcp_attachment::McpAttachmentRefusal;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let (phase, machine, _owner) = registration_fixture(&root).await;
    let session = meerkat_core::SessionId::new();
    machine.register_session(session.clone()).await.unwrap();
    let original = machine.prepare_bindings(session.clone()).await.unwrap();
    let original_authority = original.configured_mcp_attachment().cloned().unwrap();
    phase.resource.select_registration(&meerkat_authorization_contracts::evidence::EvidenceId::new(session.to_string()).unwrap(), true).unwrap();
    // No successful authorize has occurred. Capture at first authorize would
    // let the replacement steal the old preparation in this schedule.
    machine.unregister_session(&session).await.unwrap();
    machine.register_session(session.clone()).await.unwrap();
    let replacement = machine.prepare_bindings(session.clone()).await.unwrap();
    let replacement_authority = replacement.configured_mcp_attachment().cloned().unwrap();
    assert_eq!(phase.resource.registration_slots.lock().unwrap().len(), 1, "replacement waits for its own actual destination hook");
    assert_eq!(original_authority.authorize_configured(phase.server()), Err(McpAttachmentRefusal::NotCurrent));
    assert_eq!(replacement_authority.authorize_configured(phase.server()), Err(McpAttachmentRefusal::NotCurrent));
    // A later actual reconstruction may select its own participant. This does
    // not change the old authority's original, now-retired registration.
    phase.resource.select_registration(&meerkat_authorization_contracts::evidence::EvidenceId::new(session.to_string()).unwrap(), true).unwrap();
    replacement_authority.authorize_configured(phase.server()).unwrap();
    assert_eq!(original_authority.authorize_configured(phase.server()), Err(McpAttachmentRefusal::NotCurrent));
    assert!(phase.resource.registration_slots.lock().unwrap().is_empty());
    machine.unregister_session(&session).await.unwrap();
}

#[tokio::test]
async fn resource_authority_rechecks_full_descriptor_origin_and_exact_registration() {
    use meerkat_core::mcp_attachment::McpAttachmentRefusal;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let (phase, machine, _owner) = registration_fixture(&root).await;
    let selected = meerkat_core::SessionId::new();
    let other = meerkat_core::SessionId::new();
    machine.register_session(selected.clone()).await.unwrap();
    machine.register_session(other.clone()).await.unwrap();
    let selected_bindings = machine.prepare_bindings(selected.clone()).await.unwrap();
    let other_bindings = machine.prepare_bindings(other.clone()).await.unwrap();
    let authority = selected_bindings.configured_mcp_attachment().cloned().unwrap();
    let other_authority = other_bindings.configured_mcp_attachment().cloned().unwrap();
    phase.resource.select_registration(&meerkat_authorization_contracts::evidence::EvidenceId::new(selected.to_string()).unwrap(), true).unwrap();
    authority.authorize_configured(phase.server()).unwrap();
    assert_eq!(other_authority.authorize_configured(phase.server()), Err(McpAttachmentRefusal::NotCurrent));
    let mut changed = phase.server().clone();
    match &mut changed.transport {
        meerkat_core::mcp_config::McpTransportConfig::Stdio(config) => config.args.push("unreviewed-argument".into()),
        _ => panic!("fixture uses its exact stdio preparer descriptor"),
    }
    assert_eq!(authority.authorize_configured(&changed), Err(McpAttachmentRefusal::Mismatch));
    changed = phase.server().clone();
    changed.name = "another-server".into();
    assert!(authority.authorize_configured(&changed).is_err(), "another server requires its actual installation owner");
    std::fs::write(&phase.resources.handoff.path, b"changed retained output").unwrap();
    assert_eq!(authority.authorize_configured(phase.server()), Err(McpAttachmentRefusal::Unavailable));
    machine.unregister_session(&selected).await.unwrap();
    machine.unregister_session(&other).await.unwrap();
}

#[tokio::test]
async fn completed_nonhelper_capture_is_released_and_live_helper_allows_repeated_entry() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let (phase, machine, _owner) = registration_fixture(&root).await;
    // Model the real destination hook's non-helper cleanup with more sequential
    // constructions than the roster bound. No history is retained after each.
    for _ in 0..phase.input.members.len() + 1 {
        let session = meerkat_core::SessionId::new();
        machine.register_session(session.clone()).await.unwrap();
        let _bindings = machine.prepare_bindings(session.clone()).await.unwrap();
        phase.resource.select_registration(&meerkat_authorization_contracts::evidence::EvidenceId::new(session.to_string()).unwrap(), false).unwrap();
        assert!(phase.resource.registration_slots.lock().unwrap().is_empty());
        machine.unregister_session(&session).await.unwrap();
    }
    let helper = meerkat_core::SessionId::new();
    machine.register_session(helper.clone()).await.unwrap();
    let bindings = machine.prepare_bindings(helper.clone()).await.unwrap();
    let authority = bindings.configured_mcp_attachment().cloned().unwrap();
    phase.resource.select_registration(&meerkat_authorization_contracts::evidence::EvidenceId::new(helper.to_string()).unwrap(), true).unwrap();
    authority.authorize_configured(phase.server()).unwrap();
    authority.authorize_configured(phase.server()).unwrap();
    assert!(phase.resource.registration_slots.lock().unwrap().is_empty());
    machine.unregister_session(&helper).await.unwrap();
}

#[tokio::test]
async fn replacement_before_destination_cannot_steal_the_pending_original_slot() {
    use meerkat_core::mcp_attachment::McpAttachmentRefusal;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let (phase, machine, _owner) = registration_fixture(&root).await;
    let session = meerkat_core::SessionId::new();
    machine.register_session(session.clone()).await.unwrap();
    let original = machine.prepare_bindings(session.clone()).await.unwrap();
    let original_authority = original.configured_mcp_attachment().cloned().unwrap();
    machine.unregister_session(&session).await.unwrap();
    machine.register_session(session.clone()).await.unwrap();
    let replacement = machine.prepare_bindings(session.clone()).await.unwrap();
    let replacement_authority = replacement.configured_mcp_attachment().cloned().unwrap();
    assert!(phase.resource.select_registration(&meerkat_authorization_contracts::evidence::EvidenceId::new(session.to_string()).unwrap(), true).is_err());
    assert_eq!(original_authority.authorize_configured(phase.server()), Err(McpAttachmentRefusal::NotCurrent));
    assert_eq!(replacement_authority.authorize_configured(phase.server()), Err(McpAttachmentRefusal::NotCurrent));
    machine.unregister_session(&session).await.unwrap();
}

#[tokio::test]
async fn settled_abandoned_construction_does_not_pin_the_next_registration() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let (phase, machine, _owner) = registration_fixture(&root).await;
    let session = meerkat_core::SessionId::new();
    machine.register_session(session.clone()).await.unwrap();
    let abandoned = machine.prepare_bindings(session.clone()).await.unwrap();
    machine.unregister_session(&session).await.unwrap();
    // Exact old construction cleanup releases its canonical capability. The
    // compiler holds only Weak, so this is not pinned by a history registry.
    drop(abandoned);
    machine.register_session(session.clone()).await.unwrap();
    let next = machine.prepare_bindings(session.clone()).await.unwrap();
    let authority = next.configured_mcp_attachment().cloned().unwrap();
    phase.resource.select_registration(&meerkat_authorization_contracts::evidence::EvidenceId::new(session.to_string()).unwrap(), true).unwrap();
    authority.authorize_configured(phase.server()).unwrap();
    assert!(phase.resource.registration_slots.lock().unwrap().is_empty());
    machine.unregister_session(&session).await.unwrap();
}


// These completed-bundle controls reopen the actual canonical owner pair and
// obtain capabilities from real native registration. Compiler surfaces and the
// destination callback remain mechanical ports. They do not run a gateway,
// restore transcript state, finish a native held member or authenticate HTTP.
async fn completed_registration_fixture(root: &Path) -> (
    Arc<super::super::resource::SetupResourceSelection>,
    Arc<meerkat_runtime::MeerkatMachine>,
    toolkit_mobkit::installed_host::PreparedInstalledHost,
    PreparedApplicationSetup,
) {
    use toolkit_mobkit::installed_host::{PreparedInstallationOwner,
        tool_policy::PreparedInstalledToolPolicy, operator_setup::OperatorOwnerStartup};
    let operator = meerkat_core::PrincipalRef::in_domain(
        meerkat_core::PrincipalKind::ServiceAccount, "completed-registration-admin",
        meerkat_core::TrustDomainId::new("application-setup-fixture").unwrap(),
    ).unwrap();
    let phase = phase_with_operator(root, Some(operator));
    phase.capture(0, &surface(false)).unwrap();
    phase.capture(1, &surface(true)).unwrap();
    let (completed, selected) = phase.complete_bundle(&root.join("complete")).unwrap();
    let selected_bytes = std::fs::read(&selected.path).unwrap();
    assert_eq!(hash(&selected_bytes), selected.sha256);
    let manifest: serde_json::Value = serde_json::from_slice(&selected_bytes).unwrap();
    let setup_bytes = std::fs::read(root.join("complete/application-setup.json")).unwrap();
    let recorded = manifest["artifacts"].as_array().unwrap().iter()
        .find(|row| row["path"] == "application-setup.json").unwrap();
    assert_eq!(recorded["sha256"].as_str().unwrap(), hash(&setup_bytes));
    let setup: PreparedApplicationSetup = serde_json::from_slice(&setup_bytes).unwrap();
    assert_eq!(toolkit_package::canonical_json(&completed).unwrap(), setup_bytes);
    let native_owner = setup.native_owner.as_ref().unwrap().clone();
    std::fs::create_dir_all(root.join("shared/data")).unwrap();
    // Initialize once, release that owner, then use explicit Restore. This
    // cannot silently initialize a missing or partial pair during restart.
    let initial = PreparedInstallationOwner::open(root.to_owned(), native_owner.clone(),
        OperatorOwnerStartup::Initialize {
            namespace: meerkat_authorization_contracts::evidence::EvidenceId::new("completed-registration-fixture").unwrap(),
            generation: 1,
        }).unwrap();
    drop(initial);
    drop(phase);
    let policy_bytes = std::fs::read(root.join("complete").join(&setup.application_tool_policy.name)).unwrap();
    assert_eq!(hash(&policy_bytes), setup.application_tool_policy.sha256);
    let policy = PreparedInstalledToolPolicy::from_canonical(&policy_bytes).unwrap();
    let owner = PreparedInstallationOwner::open(root.to_owned(), native_owner,
        OperatorOwnerStartup::Restore).unwrap().complete(policy).await.unwrap();
    // A fresh resource participant is derived from this exact completed output;
    // no compiler-phase Arc or historical registration is carried over.
    let resource = Arc::new(super::super::resource::SetupResourceSelection::from_completed(&setup).unwrap());
    let mut configuration = owner.take_configuration().unwrap();
    configuration.configured_mcp_attachment = Some(Arc::new(
        super::super::resource::SetupResourceProvider::new(resource.clone(), configuration.configured_mcp_attachment.take()),
    ));
    let machine = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral()
        .with_local_grant_authorization(configuration).unwrap());
    (resource, machine, owner, setup)
}

#[tokio::test]
async fn completed_bundle_restore_uses_fresh_native_registration_and_rechecks_artifacts() {
    use meerkat_core::mcp_attachment::McpAttachmentRefusal;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let (resource, machine, _owner, setup) = completed_registration_fixture(&root).await;
    let session = meerkat_core::SessionId::new();
    machine.register_session(session.clone()).await.unwrap();
    let bindings = machine.prepare_bindings(session.clone()).await.unwrap();
    let authority = bindings.configured_mcp_attachment().cloned().unwrap();
    assert_eq!(authority.authorize_configured(resource.server()), Err(McpAttachmentRefusal::NotCurrent));
    resource.select_registration(&meerkat_authorization_contracts::evidence::EvidenceId::new(session.to_string()).unwrap(), true).unwrap();
    authority.authorize_configured(resource.server()).unwrap();
    // Native actor-only recovery reuses canonical bindings and the same selected
    // authority. No second destination observation or new participant is needed.
    let repeated = machine.prepare_bindings(session.clone()).await.unwrap();
    assert!(Arc::ptr_eq(&authority, repeated.configured_mcp_attachment().unwrap()));
    repeated.configured_mcp_attachment().unwrap().authorize_configured(resource.server()).unwrap();
    assert!(resource.registration_slots.lock().unwrap().is_empty());
    let mut changed = resource.server().clone();
    match &mut changed.transport {
        meerkat_core::mcp_config::McpTransportConfig::Stdio(config) => config.args.push("changed-resource".into()),
        _ => panic!("fixture uses the retained stdio descriptor"),
    }
    assert_eq!(authority.authorize_configured(&changed), Err(McpAttachmentRefusal::Mismatch));
    std::fs::write(&setup.resources.handoff.path, b"changed completed handoff").unwrap();
    assert_eq!(authority.authorize_configured(resource.server()), Err(McpAttachmentRefusal::Unavailable));
    machine.unregister_session(&session).await.unwrap();
    assert_eq!(authority.authorize_configured(resource.server()), Err(McpAttachmentRefusal::NotCurrent));
}

#[tokio::test]
async fn completed_bundle_retired_helper_reconstruction_does_not_retarget_old_authority() {
    use meerkat_core::mcp_attachment::McpAttachmentRefusal;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let (resource, machine, _owner, _setup) = completed_registration_fixture(&root).await;
    let session = meerkat_core::SessionId::new();
    let document = meerkat_authorization_contracts::evidence::EvidenceId::new(session.to_string()).unwrap();
    machine.register_session(session.clone()).await.unwrap();
    let initial = machine.prepare_bindings(session.clone()).await.unwrap();
    let old = initial.configured_mcp_attachment().cloned().unwrap();
    resource.select_registration(&document, true).unwrap();
    old.authorize_configured(resource.server()).unwrap();
    machine.unregister_session(&session).await.unwrap();
    // Keep the actual old authority alive while the same durable SessionId gets
    // its new native registration. It must never follow the new selection.
    machine.register_session(session.clone()).await.unwrap();
    let rebuilt = machine.prepare_bindings(session.clone()).await.unwrap();
    let new = rebuilt.configured_mcp_attachment().cloned().unwrap();
    assert!(!Arc::ptr_eq(&old, &new));
    assert_eq!(old.authorize_configured(resource.server()), Err(McpAttachmentRefusal::NotCurrent));
    assert_eq!(new.authorize_configured(resource.server()), Err(McpAttachmentRefusal::NotCurrent));
    resource.select_registration(&document, true).unwrap();
    new.authorize_configured(resource.server()).unwrap();
    assert_eq!(old.authorize_configured(resource.server()), Err(McpAttachmentRefusal::NotCurrent));
    assert!(resource.registration_slots.lock().unwrap().is_empty());
    machine.unregister_session(&session).await.unwrap();
    assert_eq!(new.authorize_configured(resource.server()), Err(McpAttachmentRefusal::NotCurrent));
}

#[test]
fn final_helper_mcp_projection_allows_only_the_requested_bundled_target_insertion() {
    use meerkat_authorization_contracts::{evidence::EvidenceId,
        work_association::{GrantTarget, TargetGrantLineage, WorkAuthorityBasis}};
    use toolkit_mobkit::installed_host::preparation::PreparedHelperMcpRequest;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let mut input = selected(&root);
    let retained_base = input.helper.controller_parent.clone();
    input.helper.operation_basis = WorkAuthorityBasis::TargetedGrantLineages {
        base: Some(retained_base.clone()), targets: vec![],
    };
    let phase = input.validate().unwrap().prepare(super::super::tests::prepared(&root)).unwrap();
    let realm = meerkat_core::mob_realm_id(&phase.input.helper.member.mob_id).unwrap();
    let operator = meerkat::governed_realm::stock::StockPrincipals::for_local_operator(&realm).unwrap().operator;
    let selected = owner_selection(&phase, Some(operator.clone()));
    let request = PreparedHelperMcpRequest { issuance_id: EvidenceId::new("compiled-helper-mcp-parent").unwrap() };
    // Inert projection value only. Canonical issuance/currentness belongs to
    // the separate preparation-owner controls, not this compiler fixture.
    let mut reference = retained_base[0].clone();
    reference.root_authority = operator;
    reference.grant_id = request.issuance_id.clone();
    let added = TargetGrantLineage { target: GrantTarget::McpServer { server: phase.server().name.clone() },
        lineage: vec![reference] };
    let mut helper = phase.helper().clone();
    helper.operation_basis = WorkAuthorityBasis::TargetedGrantLineages {
        base: Some(retained_base.clone()), targets: vec![added.clone()],
    };
    assert!(phase.retain_authorities(selected.clone(), helper.clone()).is_err(), "no implicit grant insertion");
    for change in 0..5 {
        let mut changed = helper.clone();
        let WorkAuthorityBasis::TargetedGrantLineages { base, targets } = &mut changed.operation_basis else { unreachable!() };
        match change {
            0 => *base = None,
            1 => targets[0].target = GrantTarget::McpServer { server: "other-server".into() },
            2 => targets[0].lineage[0].grant_id = EvidenceId::new("other-parent").unwrap(),
            3 => targets[0].lineage[0].root_authority = phase.input.members[0].logical_owner.clone(),
            _ => targets.push(TargetGrantLineage {
                target: GrantTarget::McpServer { server: "unrequested-server".into() }, lineage: retained_base.clone(),
            }),
        }
        assert!(phase.retain_authorities_with_helper_mcp(selected.clone(), changed, Some(&request)).is_err());
        assert!(phase.final_authorities.get().is_none());
    }
    phase.retain_authorities_with_helper_mcp(selected, helper.clone(), Some(&request)).unwrap();
    assert!(phase.final_authorities.get().unwrap().helper == helper);
    assert!(phase.input.helper.operation_basis == WorkAuthorityBasis::TargetedGrantLineages {
        base: Some(retained_base), targets: vec![],
    }, "early input is not rewritten into authority evidence");
}

#[test]
fn final_helper_mcp_projection_cannot_replace_an_existing_target() {
    use meerkat_authorization_contracts::{evidence::EvidenceId,
        work_association::{GrantTarget, TargetGrantLineage, WorkAuthorityBasis}};
    use toolkit_mobkit::installed_host::preparation::PreparedHelperMcpRequest;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let mut input = selected(&root);
    let realm = meerkat_core::mob_realm_id(&input.helper.member.mob_id).unwrap();
    let operator = meerkat::governed_realm::stock::StockPrincipals::for_local_operator(&realm).unwrap().operator;
    // Inert projection values with a valid selected operator. Only the occupied
    // target's issuance identity changes below; missing ownership cannot mask it.
    let mut prior = input.helper.controller_parent.clone();
    prior[0].root_authority = operator.clone();
    input.helper.operation_basis = WorkAuthorityBasis::TargetedGrantLineages { base: None,
        targets: vec![TargetGrantLineage {
            target: GrantTarget::McpServer { server: "toolkit-setup".into() }, lineage: prior,
        }],
    };
    let phase = input.validate().unwrap().prepare(super::super::tests::prepared(&root)).unwrap();
    let selected = owner_selection(&phase, Some(operator));
    let request = PreparedHelperMcpRequest { issuance_id: EvidenceId::new("replacement-parent").unwrap() };
    let mut helper = phase.helper().clone();
    let WorkAuthorityBasis::TargetedGrantLineages { targets, .. } = &mut helper.operation_basis else { unreachable!() };
    targets[0].lineage[0].grant_id = request.issuance_id.clone();
    assert!(phase.retain_authorities_with_helper_mcp(selected.clone(), helper, Some(&request)).is_err());
    phase.retain_authorities(selected, phase.helper().clone()).unwrap();
}


#[test]
fn held_helper_session_requires_capture_and_the_complete_matching_inventory() {
    // Mechanical capture ports only. Production supplies Some(session) solely
    // from prepare_member_surface's validated native build identity.
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let inert = phase(&root);
    let ordinary = inert.capture(0, &surface(false)).unwrap();
    let helper = inert.capture(1, &surface(true)).unwrap();
    let ordinary_identity = inert.held_member_identity(&inert.input.members[0].member).unwrap();
    let helper_identity = inert.held_member_identity(&inert.helper().member).unwrap();
    assert!(inert.held_helper_session([(ordinary_identity, &ordinary), (helper_identity, &helper)])
        .unwrap_err().contains("native helper session was not captured"));
    // A separate append-only compilation has an explicitly supplied mechanical
    // observation. Missing, duplicate or cross-member output cannot expose it.
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let captured = phase(&root);
    let session = meerkat_core::SessionId::new();
    let ordinary = captured.capture_selected(0, &surface(false), Some(meerkat_core::SessionId::new())).unwrap();
    let helper = captured.capture_selected(1, &surface(true), Some(session.clone())).unwrap();
    let ordinary_identity = captured.held_member_identity(&captured.input.members[0].member).unwrap();
    let helper_identity = captured.held_member_identity(&captured.helper().member).unwrap();
    assert!(captured.held_helper_session([(ordinary_identity, &ordinary)]).is_err());
    assert!(captured.held_helper_session([(ordinary_identity, &ordinary), (ordinary_identity, &ordinary)]).is_err());
    assert!(captured.held_helper_session([(ordinary_identity, &helper), (helper_identity, &ordinary)]).is_err());
    assert_eq!(captured.held_helper_session([(helper_identity, &helper), (ordinary_identity, &ordinary)]).unwrap(), session);
    assert!(captured.capture_selected(1, &surface(true), Some(meerkat_core::SessionId::new())).is_err(),
        "an existing member capture cannot be replaced with a later session");
}

#[test]
fn final_helper_catalog_projection_adds_only_requested_base_and_keeps_targets() {
    use meerkat_authorization_contracts::{evidence::EvidenceId,
        work_association::{GrantTarget, TargetGrantLineage, WorkAuthorityBasis}};
    use toolkit_mobkit::installed_host::preparation::PreparedHelperCatalogRequest;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let mut input = selected(&root);
    let realm = meerkat_core::mob_realm_id(&input.helper.member.mob_id).unwrap();
    let operator = meerkat::governed_realm::stock::StockPrincipals::for_local_operator(&realm).unwrap().operator;
    let mut reference = input.helper.controller_parent[0].clone();
    reference.root_authority = operator.clone();
    let target = TargetGrantLineage { target: GrantTarget::McpServer { server: "retained-service".into() },
        lineage: vec![reference.clone()] };
    input.helper.operation_basis = WorkAuthorityBasis::TargetedGrantLineages { base: None, targets: vec![target.clone()] };
    let phase = input.validate().unwrap().prepare(super::super::tests::prepared(&root)).unwrap();
    let selected = owner_selection(&phase, Some(operator));
    let request = PreparedHelperCatalogRequest { scope: Default::default(), issuance_id: EvidenceId::new("compiled-helper-catalog-parent").unwrap() };
    // Inert output reference, not native issuance/currentness evidence.
    reference.grant_id = request.issuance_id.clone();
    let mut helper = phase.helper().clone();
    helper.operation_basis = WorkAuthorityBasis::TargetedGrantLineages {
        base: Some(vec![reference.clone()]), targets: vec![target],
    };
    assert!(phase.retain_authorities(selected.clone(), helper.clone()).is_err());
    for change in 0..5 {
        let mut changed = helper.clone();
        let WorkAuthorityBasis::TargetedGrantLineages { base, targets } = &mut changed.operation_basis else { unreachable!() };
        match change {
            0 => *base = Some(vec![]),
            1 => base.as_mut().unwrap().push(reference.clone()),
            2 => base.as_mut().unwrap()[0].grant_id = EvidenceId::new("unrequested-base").unwrap(),
            3 => base.as_mut().unwrap()[0].root_authority = phase.input.members[0].logical_owner.clone(),
            _ => targets[0].target = GrantTarget::McpServer { server: "changed-service".into() },
        }
        assert!(phase.retain_authorities_with_helper_access(selected.clone(), changed, None, Some(&request)).is_err());
        assert!(phase.final_authorities.get().is_none());
    }
    phase.retain_authorities_with_helper_access(selected, helper.clone(), None, Some(&request)).unwrap();
    assert!(phase.final_authorities.get().unwrap().helper == helper);
    assert!(phase.input.helper.operation_basis.lineage_for(None).is_none(), "early capture is not rewritten");
}

#[test]
fn final_helper_catalog_projection_cannot_replace_an_occupied_base() {
    use meerkat_authorization_contracts::{evidence::EvidenceId, work_association::WorkAuthorityBasis};
    use toolkit_mobkit::installed_host::preparation::PreparedHelperCatalogRequest;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let mut input = selected(&root);
    let realm = meerkat_core::mob_realm_id(&input.helper.member.mob_id).unwrap();
    let operator = meerkat::governed_realm::stock::StockPrincipals::for_local_operator(&realm).unwrap().operator;
    let mut prior = input.helper.controller_parent.clone(); prior[0].root_authority = operator.clone();
    input.helper.operation_basis = WorkAuthorityBasis::TargetedGrantLineages { base: Some(prior), targets: vec![] };
    let phase = input.validate().unwrap().prepare(super::super::tests::prepared(&root)).unwrap();
    let selected = owner_selection(&phase, Some(operator));
    let request = PreparedHelperCatalogRequest { scope: Default::default(), issuance_id: EvidenceId::new("replacement-catalog-parent").unwrap() };
    let mut helper = phase.helper().clone();
    let WorkAuthorityBasis::TargetedGrantLineages { base, .. } = &mut helper.operation_basis else { unreachable!() };
    base.as_mut().unwrap()[0].grant_id = request.issuance_id.clone();
    assert!(phase.retain_authorities_with_helper_access(selected.clone(), helper, None, Some(&request)).is_err());
    assert!(phase.final_authorities.get().is_none());
    phase.retain_authorities(selected, phase.helper().clone()).unwrap();
}

#[test]
fn final_helper_catalog_successor_parent_requires_explicit_scope_and_preserves_operation_basis() {
    use meerkat_authorization_contracts::{evidence::EvidenceId, work_association::WorkAuthorityBasis};
    use toolkit_mobkit::installed_host::{acquisition::ConfiguredHelperCatalogCapability,
        preparation::{PreparedHelperCatalogRequest, PreparedHelperCatalogScope}};
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let mut input = selected(&root);
    let realm = meerkat_core::mob_realm_id(&input.helper.member.mob_id).unwrap();
    let operator = meerkat::governed_realm::stock::StockPrincipals::for_local_operator(&realm).unwrap().operator;
    let mut reference = input.helper.controller_parent[0].clone();
    reference.root_authority = operator.clone();
    input.helper.operation_basis = WorkAuthorityBasis::TargetedGrantLineages { base: None, targets: vec![] };
    let phase = input.validate().unwrap().prepare(super::super::tests::prepared(&root)).unwrap();
    let selected = owner_selection(&phase, Some(operator));
    let mut request = PreparedHelperCatalogRequest { scope: PreparedHelperCatalogScope::ExactSession,
        issuance_id: EvidenceId::new("explicit-successor-catalog-parent").unwrap() };
    reference.grant_id = request.issuance_id.clone();
    let mut helper = phase.helper().clone();
    helper.catalog_capability = Some(ConfiguredHelperCatalogCapability { parent: vec![reference.clone()] });
    assert!(phase.retain_authorities_with_helper_access(selected.clone(), helper.clone(), None, None).is_err());
    assert!(phase.retain_authorities_with_helper_access(selected.clone(), helper.clone(), None, Some(&request)).is_err());
    request.scope = PreparedHelperCatalogScope::CurrentHelperSuccessors;
    for change in 0..3 {
        let mut changed = helper.clone();
        match change {
            0 => changed.catalog_capability.as_mut().unwrap().parent[0].grant_id = EvidenceId::new("unrequested-parent").unwrap(),
            1 => changed.catalog_capability.as_mut().unwrap().parent[0].root_authority = phase.input.members[0].logical_owner.clone(),
            _ => changed.operation_basis = WorkAuthorityBasis::TargetedGrantLineages { base: Some(vec![reference.clone()]), targets: vec![] },
        }
        assert!(phase.retain_authorities_with_helper_access(selected.clone(), changed, None, Some(&request)).is_err());
        assert!(phase.final_authorities.get().is_none());
    }
    phase.retain_authorities_with_helper_access(selected, helper.clone(), None, Some(&request)).unwrap();
    assert!(phase.final_authorities.get().unwrap().helper == helper);
    assert_eq!(helper.operation_basis, phase.input.helper.operation_basis);
}

// Read the real Core bundle composition at its public session-builder boundary.
// No agent is constructed, no model is called and no setup resource is executed.
struct ReportBundleSurfaceProbe(Arc<Mutex<Option<PreparedToolSurface>>>);

#[async_trait]
impl meerkat_session::SessionAgentBuilder for ReportBundleSurfaceProbe {
    type Agent = meerkat::FactoryAgent;

    async fn build_agent(
        &self,
        request: &meerkat_core::service::CreateSessionRequest,
        _events: tokio::sync::mpsc::Sender<meerkat_core::AgentEvent>,
    ) -> Result<Self::Agent, meerkat_core::service::SessionError> {
        let dispatcher = request
            .build
            .as_ref()
            .and_then(|build| build.external_tools.as_ref())
            .expect("the selected native bundle must reach the session builder");
        let mut captured = self.0.lock().unwrap();
        assert!(
            captured.is_none(),
            "the probe must see exactly one construction"
        );
        *captured = Some(PreparedToolSurface::of(dispatcher.as_ref(), &[]));
        Err(meerkat_core::service::SessionError::Unsupported(
            "report bundle surface captured before agent construction".into(),
        ))
    }
}

#[tokio::test]
async fn prepared_helper_report_identity_matches_native_bundle_mount() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let phase = phase(&root);
    let helper = phase.helper().member.clone();
    let observed = Arc::new(Mutex::new(None));
    let service = Arc::new(meerkat_session::EphemeralSessionService::new(
        ReportBundleSurfaceProbe(observed.clone()),
        1,
    ));
    let tools = toolkit_mobkit::installed_host::acquisition::InstalledAcquisitionTools::new(
        std::time::Duration::from_secs(1),
    )
    .unwrap();
    // The compiler's historical data fixture contains non-executable skill and
    // MCP placeholders. Exercise only the selected bundle here, using the same
    // helper mob/profile/member binding and the real production result leaf.
    let mut profiles = serde_json::Map::new();
    profiles.insert(
        helper.role.clone(),
        json!({
            "model": "claude-opus-4-8", "provider": "anthropic",
            "tools": {"comms": true, "rust_bundles": [meerkat_toolkit::setup::SETUP_RESULTS_BUNDLE]}
        }),
    );
    let definition = serde_json::from_value(json!({
        "id": helper.mob_id, "profiles": profiles
    }))
    .unwrap();
    let runtime = meerkat_mob::MobBuilder::new(definition, meerkat_mob::MobStorage::in_memory())
        .with_session_service(service)
        .allow_ephemeral_sessions(true)
        .register_tool_bundle(
            meerkat_toolkit::setup::SETUP_RESULTS_BUNDLE,
            tools.result_tool(),
        )
        .create()
        .await
        .unwrap();
    let spawned = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        runtime.spawn_spec(meerkat_mob::SpawnMemberSpec::new(
            helper.role.clone(),
            meerkat_mob::AgentIdentity::from(helper.member.as_str()),
        )),
    )
    .await;
    let cleanup =
        tokio::time::timeout(std::time::Duration::from_secs(10), runtime.shutdown()).await;
    assert!(cleanup.expect("probe runtime shutdown timed out").is_ok());
    let failure = spawned
        .expect("native bundle composition timed out")
        .unwrap_err();
    assert!(
        failure
            .to_string()
            .contains("report bundle surface captured before agent construction"),
        "the native composition must reach the inert observation boundary: {failure}"
    );
    let surface = observed
        .lock()
        .unwrap()
        .take()
        .expect("actual native bundle surface");
    let helper_index = phase
        .input
        .members
        .iter()
        .position(|row| row.member == helper)
        .unwrap();
    // This is the unchanged production guard and policy selection, supplied the
    // actual mounted surface, rather than a fixture's copied tool identity.
    phase
        .capture(helper_index, &surface)
        .expect("the real result bundle must match its compiler definition");
}

mod resource_aggregate;

/// Actual native registration fixture for the attempt consumer controls.
/// Selection is the same mechanical hook port as the resource tests above;
/// only the SDK case exercises the real selected-destination callback.
pub(crate) struct AttemptRegistrationFixture {
    pub source_member: meerkat_core::MobMemberBinding,
    pub member: meerkat_core::MobMemberBinding,
    pub session: meerkat_core::SessionId,
    pub participant: Arc<crate::application_setup::resource::SetupResourceRegistration>,
    pub machine: Arc<meerkat_runtime::MeerkatMachine>,
    _owner: toolkit_mobkit::installed_host::PreparedInstalledHost,
    _authority: Arc<dyn meerkat_core::mcp_attachment::ConfiguredMcpAttachmentAuthority>,
}
pub(crate) async fn attempt_registration_fixture(
    root: &Path,
    helper_identity: Option<&str>,
) -> AttemptRegistrationFixture {
    let (phase, machine, owner) = if let Some(identity) = helper_identity {
        // Select the member in compiler input before validation, helper-role
        // projection, authority preparation or native session registration.
        // A new SessionId alone reconstructs the same native member.
        let mut input = selected(root);
        let member_id = roster_member_id_for_identity(identity).to_string();
        input.roster[1].identity =
            meerkat_mobkit::identity_first::AgentIdentity::parse(identity).unwrap();
        input.members[1].member.member = member_id.clone();
        input.helper.member.member = member_id.clone();
        input.tool_policy.members[1].member_identity = member_id;
        input.tool_policy.members.sort_by(|a, b| a.member_identity.cmp(&b.member_identity));
        let phase = input.validate().unwrap()
            .prepare(super::super::tests::prepared(root)).unwrap();
        phase.retain_owner(owner_selection(&phase, None)).unwrap();
        registration_fixture_from_phase(root, phase).await
    } else {
        // Keep the already-qualified storage/failure fixture selection exact.
        registration_fixture(root).await
    };
    let session = meerkat_core::SessionId::new();
    machine.register_session(session.clone()).await.unwrap();
    let bindings = machine.prepare_bindings(session.clone()).await.unwrap();
    let authority = bindings.configured_mcp_attachment().cloned().unwrap();
    let participant = phase
        .resource
        .registration_slots
        .lock()
        .unwrap()
        .get(&session.to_string())
        .unwrap()
        .upgrade()
        .unwrap();
    phase
        .resource
        .select_registration(
            &meerkat_authorization_contracts::evidence::EvidenceId::new(session.to_string())
                .unwrap(),
            true,
        )
        .unwrap();
    authority.authorize_configured(phase.server()).unwrap();
    AttemptRegistrationFixture {
        source_member: phase.source_helper_member.clone(),
        member: phase.helper().member.clone(),
        session,
        participant,
        machine,
        _owner: owner,
        _authority: authority,
    }
}

/// Direct-router LocalRound controls retain the ordinary compiler resource and
/// use this existing mechanical destination hook after actual native binding.
/// No gateway construction or reset transport handoff is implied by this port.
#[cfg(all(unix, feature = "console"))]
pub(crate) struct LocalRoundRegistrationFixture {
    phase: Arc<ApplicationSetupCompilation>,
    machine: Arc<meerkat_runtime::MeerkatMachine>,
    _owner: toolkit_mobkit::installed_host::PreparedInstalledHost,
    authorities: Vec<Arc<dyn meerkat_core::mcp_attachment::ConfiguredMcpAttachmentAuthority>>,
    sessions: Vec<meerkat_core::SessionId>,
}
#[cfg(all(unix, feature = "console"))]
impl LocalRoundRegistrationFixture {
    pub(crate) fn server(&self) -> &meerkat_core::McpServerConfig { self.phase.server() }
    pub(crate) fn member(&self) -> &meerkat_core::MobMemberBinding { &self.phase.helper().member }
    pub(crate) async fn register(&mut self) -> meerkat_core::SessionId {
        let session = meerkat_core::SessionId::new();
        self.machine.register_session(session.clone()).await.unwrap();
        self.sessions.push(session.clone());
        let bindings = self.machine.prepare_bindings(session.clone()).await.unwrap();
        let authority = bindings.configured_mcp_attachment().cloned().unwrap();
        self.authorities.push(authority.clone());
        self.phase.resource.select_registration(
            &meerkat_authorization_contracts::evidence::EvidenceId::new(session.to_string()).unwrap(),
            true,
        ).unwrap();
        authority.authorize_configured(self.server()).unwrap();
        session
    }
    pub(crate) async fn retire(&mut self, session: &meerkat_core::SessionId) {
        self.machine.unregister_session(session).await.unwrap();
        self.sessions.retain(|s| s != session);
    }
    pub(crate) async fn cleanup(&mut self) -> Vec<String> {
        let mut failures = Vec::new();
        for session in &self.sessions {
            if let Err(error) = self.machine.unregister_session(session).await {
                failures.push(error.to_string());
            }
        }
        self.sessions.clear();
        failures
    }
}
#[cfg(all(unix, feature = "console"))]
pub(crate) async fn local_round_registration_fixture(
    root: &Path, prepared: crate::resources::PreparedResources,
    local_owner: crate::resources::LocalRoundOwner,
) -> (LocalRoundRegistrationFixture, Arc<crate::resources::LocalRoundAttempts>) {
    let mut phase = selected(root).validate().unwrap().prepare(prepared).unwrap();
    let attempts = match phase.retain_local_round_owner(local_owner) {
        Ok(attempts) => attempts,
        Err((_owner, error)) => panic!("ordinary local preparation must match: {error}"),
    };
    phase.retain_owner(owner_selection(&phase, None)).unwrap();
    let (phase, machine, owner) = registration_fixture_from_phase(root, phase).await;
    (LocalRoundRegistrationFixture {
        phase, machine, _owner: owner, authorities: Vec::new(), sessions: Vec::new(),
    }, attempts)
}

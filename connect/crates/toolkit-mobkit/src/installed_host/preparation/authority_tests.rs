//! Actual empty canonical-pair and issuer controls. The route key is explicit
//! mechanical admin configuration, not a claimed prepared controller client.
use super::*;
use crate::installed_host::operator_setup::{AgentSetupCapabilityRequest, HelperReportCapabilityRequest};
use meerkat_authorization_contracts::evidence::EvidenceId;
use meerkat_core::ControllerRouteKey;

fn fixture() -> (tempfile::TempDir, PathBuf, PreparedLocalPolicy, acquisition::InstalledSetupSelection, PreparationAuthorities) {
    let (directory, selection) = super::super::tests::fixture();
    let (mut selected, _) = super::tests::inputs(&selection);
    selected.native_host.remote_operator = Some(super::super::tests::principal("first-install-admin"));
    let member = selected.native_host.members[0].member.clone();
    let route = selected.native_host.routes[0].id.clone();
    let helper = acquisition::InstalledSetupSelection {
        catalog_capability: None,
        report_capability: None, member: member.clone(),
        ingress_actor: super::super::tests::principal("helper-ingress"),
        logical_executor: selected.native_host.members[0].logical_owner.clone(), represented_subject: None,
        controller_parent: vec![],
        operation_basis: WorkAuthorityBasis::TargetedGrantLineages { base: None, targets: vec![] },
    };
    let key = ControllerRouteKey::parse(&format!("model-route/v1/{}", "a".repeat(64))).unwrap();
    let requests = PreparationAuthorities {
        helper_mcp: None, helper_catalog: None,
        controllers: vec![
            PreparedControllerRequest { target: PreparedControllerTarget::OriginalRoute { route: route.clone() },
                issuance_id: EvidenceId::new("original-controller-parent").unwrap(), controller_route: key.clone() },
            PreparedControllerRequest { target: PreparedControllerTarget::Helper,
                issuance_id: EvidenceId::new("helper-controller-parent").unwrap(), controller_route: key },
        ],
        agent_setup: vec![AgentSetupCapabilityRequest {
            helper: None, member: member.clone(), route, helper_targets: vec![],
            issuance_id: EvidenceId::new("original-setup-parent").unwrap() }],
        helper_report: Some(HelperReportCapabilityRequest { member, account_issuance_id: None, public_issuance_id: None,
            issuance_id: EvidenceId::new("helper-report-parent").unwrap() }),
    };
    (directory, selection.deployment_root, selected, helper, requests)
}
fn initialize(root: PathBuf, selected: PreparedLocalPolicy) -> PreparedInstallationOwner {
    PreparedInstallationOwner::open(root, selected, operator_setup::OperatorOwnerStartup::Initialize {
        namespace: EvidenceId::new("first-install-namespace").unwrap(), generation: 1,
    }).unwrap()
}
fn bytes(owner: &PreparedInstallationOwner) -> Vec<u8> {
    let directory = state_directory_in(&owner.deployment_root, &owner.selected.native_host.realm, NativeOwnerStartup::Restore).unwrap();
    fs::read(directory.join("grants.jsonl")).unwrap()
}

#[test]
fn empty_owner_issues_separate_real_parents_then_recovers_same_ids() {
    let (_directory, root, selected, mut helper, requests) = fixture();
    assert!(PreparedInstallationOwner::open(root.clone(), selected.clone(), operator_setup::OperatorOwnerStartup::Restore).is_err());
    let mut owner = initialize(root.clone(), selected.clone());
    assert!(helper.controller_parent.is_empty());
    assert!(owner.selected.native_host.routes[0].controller_parent.is_none());
    owner.configure_authorities(&mut helper, &requests).unwrap();
    let resolved = owner.selected().clone();
    let grants = bytes(&owner);
    let application = &resolved.native_host.grant_administrator;
    let setup = resolved.native_host.members[0].setup_capability.as_ref().unwrap();
    let report = helper.report_capability.as_ref().unwrap();
    let controller = resolved.native_host.routes[0].controller_parent.as_ref().unwrap();
    assert_eq!(setup.parent.grant_id, requests.agent_setup[0].issuance_id);
    assert_eq!(report.parent[0].grant_id, requests.helper_report.as_ref().unwrap().issuance_id);
    assert_ne!(helper.controller_parent, *controller);
    assert_ne!(report.parent, helper.controller_parent);
    let setup_resolved = owner.source.grants().resolve_lineage(std::slice::from_ref(&setup.parent), application, None).unwrap();
    assert_eq!(setup_resolved.restrictions(), &application_operations::configured_setup_parent(
        application, &resolved.native_host.routes[0].logical_executor, &resolved.native_host.members[0].audience));
    setup_resolved.check_current().unwrap();
    let report_resolved = owner.source.grants().resolve_controller_lineage(&report.parent, application, None).unwrap();
    assert_eq!(report_resolved.restrictions(), &operator_setup::configured_report::report_parent(
        application, &helper.logical_executor, &resolved.native_host.members[0].audience));
    report_resolved.check_current().unwrap();
    drop(report_resolved); drop(setup_resolved);
    owner.configure_authorities(&mut helper, &requests).unwrap();
    assert_eq!(owner.selected(), &resolved);
    assert_eq!(bytes(&owner), grants);
    // Actual stopped-owner retry after lost output uses Restore and retained
    // IDs. Initialize is never an automatic fallback or replacement owner.
    drop(owner);
    let mut restored = PreparedInstallationOwner::open(root, selected, operator_setup::OperatorOwnerStartup::Restore).unwrap();
    helper.controller_parent.clear(); helper.report_capability = None;
    restored.configure_authorities(&mut helper, &requests).unwrap();
    assert_eq!(restored.selected(), &resolved);
    assert_eq!(bytes(&restored), grants);
}

#[test]
fn changed_request_or_selected_parent_refuses_without_replacement_issuance() {
    let (_directory, root, selected, mut helper, requests) = fixture();
    let mut owner = initialize(root, selected);
    owner.configure_authorities(&mut helper, &requests).unwrap();
    let before = bytes(&owner);
    let mut changed = requests.clone();
    changed.controllers[1].issuance_id = EvidenceId::new("replacement-controller").unwrap();
    assert!(matches!(owner.configure_authorities(&mut helper, &changed), Err(HostLifetimeError::Denied)));
    assert_eq!(bytes(&owner), before);
    changed = requests.clone();
    changed.controllers[0].controller_route = ControllerRouteKey::parse(&format!("model-route/v1/{}", "b".repeat(64))).unwrap();
    assert!(matches!(owner.configure_authorities(&mut helper, &changed), Err(HostLifetimeError::Denied)));
    assert_eq!(bytes(&owner), before);
    changed = requests.clone();
    changed.helper_report.as_mut().unwrap().member.member = "another-helper".into();
    assert!(matches!(owner.configure_authorities(&mut helper, &changed), Err(HostLifetimeError::Denied)));
    assert_eq!(bytes(&owner), before);
}

#[test]
fn absent_requests_never_infer_controller_setup_or_report_authority() {
    let (_directory, root, selected, mut helper, _) = fixture();
    let mut owner = initialize(root, selected.clone());
    let before = bytes(&owner);
    owner.configure_authorities(&mut helper, &PreparationAuthorities::default()).unwrap();
    assert_eq!(owner.selected(), &selected);
    assert!(helper.controller_parent.is_empty());
    assert!(helper.report_capability.is_none());
    assert_eq!(bytes(&owner), before);
}

#[test]
fn partial_native_pair_never_becomes_initialize_or_restore_success() {
    let (_directory, root, selected, _helper, _requests) = fixture();
    let state = state_directory_in(&root, &selected.native_host.realm, NativeOwnerStartup::Initialize).unwrap();
    let partial = state.join("grants.jsonl");
    fs::write(&partial, b"retained incomplete owner evidence").unwrap();
    let before = fs::read(&partial).unwrap();
    assert!(PreparedInstallationOwner::open(root.clone(), selected.clone(), operator_setup::OperatorOwnerStartup::Restore).is_err());
    assert!(PreparedInstallationOwner::open(root, selected, operator_setup::OperatorOwnerStartup::Initialize {
        namespace: EvidenceId::new("replacement-is-forbidden").unwrap(), generation: 2,
    }).is_err());
    assert_eq!(fs::read(partial).unwrap(), before);
    assert!(!state.join("originals.sqlite3").exists());
}

fn observed_routes(requests: &PreparationAuthorities) -> Vec<(PreparedControllerTarget, ControllerRouteKey)> {
    requests.controllers.iter().map(|request|
        (request.target.clone(), request.controller_route.clone())).collect()
}

#[test]
fn controller_intent_roundtrip_preserves_legacy_keys_and_accepts_no_key() {
    let (_directory, _root, _selected, _helper, requests) = fixture();
    let legacy = serde_json::to_value(&requests).unwrap();
    let intent: PreparationAuthorityIntent = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(serde_json::to_value(&intent).unwrap(), legacy);
    assert_eq!(serde_json::to_value(intent.resolve_known_routes().unwrap()).unwrap(), legacy);

    let mut no_key = legacy.clone();
    for request in no_key["controllers"].as_array_mut().unwrap() {
        request.as_object_mut().unwrap().remove("controller_route");
    }
    let intent: PreparationAuthorityIntent = serde_json::from_value(no_key.clone()).unwrap();
    assert!(intent.controllers.iter().all(|request| request.controller_route.is_none()));
    assert_eq!(serde_json::to_value(&intent).unwrap(), no_key);
    assert!(matches!(intent.resolve_known_routes(), Err(HostLifetimeError::Unavailable)));
    no_key["controllers"][0]["controller_route"] = serde_json::Value::Null;
    let explicit_null: PreparationAuthorityIntent = serde_json::from_value(no_key).unwrap();
    assert!(explicit_null.controllers[0].controller_route.is_none());

    let mut invalid = legacy.clone();
    invalid["controllers"][0]["controller_route"] = serde_json::json!("not-a-native-key");
    assert!(serde_json::from_value::<PreparationAuthorityIntent>(invalid).is_err());
    let mut unknown = legacy;
    unknown["controllers"][0]["inferred_route"] = serde_json::json!(true);
    assert!(serde_json::from_value::<PreparationAuthorityIntent>(unknown).is_err());
}

#[test]
fn controller_resolution_matches_targets_not_order_and_preserves_helper_selections() {
    let (_directory, _root, _selected, helper, mut requests) = fixture();
    requests.controllers[1].controller_route = ControllerRouteKey::parse(
        &format!("model-route/v1/{}", "b".repeat(64))).unwrap();
    requests.helper_report.as_mut().unwrap().account_issuance_id =
        Some(EvidenceId::new("account-parent").unwrap());
    requests.helper_report.as_mut().unwrap().public_issuance_id =
        Some(EvidenceId::new("public-parent").unwrap());
    requests.agent_setup[0].helper_targets = vec![
        operator_setup::SetupTargetRequest {
            target: Some(acquisition::ConfiguredHelperReportCapability::public_target()),
            issuance_id: EvidenceId::new("original-public-scope").unwrap(),
        },
        operator_setup::SetupTargetRequest {
            target: Some(acquisition::ConfiguredHelperReportCapability::target()),
            issuance_id: EvidenceId::new("original-report-scope").unwrap(),
        },
        operator_setup::SetupTargetRequest {
            target: Some(acquisition::ConfiguredHelperReportCapability::account_target()),
            issuance_id: EvidenceId::new("original-account-scope").unwrap(),
        },
    ];
    requests.agent_setup[0].helper_targets.sort_by(|left, right| left.target.cmp(&right.target));
    let mut intent = PreparationAuthorityIntent::from(requests.clone());
    intent.controllers[1].controller_route = None;
    let mut observations = observed_routes(&requests);
    observations.reverse();
    let resolved = intent.resolve_controller_routes(observations.clone()).unwrap();
    assert_eq!(serde_json::to_value(&resolved).unwrap(), serde_json::to_value(&requests).unwrap());

    let mut compiled = helper.member.clone();
    compiled.role = "compiled-helper".into();
    let projected = intent.project_helper_member(&helper.member, &compiled)
        .resolve_controller_routes(observations).unwrap();
    let expected = requests.project_helper_member(&helper.member, &compiled);
    assert_eq!(serde_json::to_value(&projected).unwrap(), serde_json::to_value(&expected).unwrap());
    assert_eq!(projected.helper_report.as_ref().unwrap().member, compiled);
    assert_eq!(projected.agent_setup[0].member, compiled);
    assert_eq!(serde_json::to_value(&projected.controllers).unwrap(), serde_json::to_value(&requests.controllers).unwrap());
    assert_eq!(serde_json::to_value(&projected.agent_setup[0].helper_targets).unwrap(),
        serde_json::to_value(&requests.agent_setup[0].helper_targets).unwrap());
}

#[test]
fn invalid_controller_inventory_never_reaches_any_native_issuance() {
    let (_directory, root, selected, mut helper, requests) = fixture();
    let mut owner = initialize(root, selected.clone());
    let before = bytes(&owner);
    let intent = PreparationAuthorityIntent::from(requests.clone());
    let routes = observed_routes(&requests);
    let mut cases = Vec::new();

    let mut missing = routes.clone(); missing.pop();
    cases.push((intent.clone(), missing));
    let mut duplicate = routes.clone(); duplicate.push(routes[0].clone());
    cases.push((intent.clone(), duplicate));
    let mut extra = routes.clone();
    extra.push((PreparedControllerTarget::OriginalRoute { route: toolkit_package::Id::new("not-requested").unwrap() },
        routes[0].1.clone()));
    cases.push((intent.clone(), extra));
    let mut mismatch = routes.clone();
    mismatch[1].1 = ControllerRouteKey::parse(&format!("model-route/v1/{}", "b".repeat(64))).unwrap();
    cases.push((intent.clone(), mismatch));
    let mut duplicate_target = intent.clone();
    duplicate_target.controllers[1].target = duplicate_target.controllers[0].target.clone();
    cases.push((duplicate_target, routes.clone()));
    let mut duplicate_id = intent.clone();
    duplicate_id.controllers[1].issuance_id = duplicate_id.controllers[0].issuance_id.clone();
    cases.push((duplicate_id, routes.clone()));
    let mut setup_id = intent.clone();
    setup_id.agent_setup[0].issuance_id = setup_id.controllers[0].issuance_id.clone();
    cases.push((setup_id, routes.clone()));
    let mut target_id = intent.clone();
    target_id.agent_setup[0].helper_targets.push(operator_setup::SetupTargetRequest {
        target: None, issuance_id: target_id.controllers[0].issuance_id.clone(),
    });
    cases.push((target_id, routes.clone()));
    let mut report_id = intent.clone();
    report_id.helper_report.as_mut().unwrap().issuance_id = report_id.controllers[0].issuance_id.clone();
    cases.push((report_id, routes.clone()));
    let mut account_id = intent.clone();
    account_id.helper_report.as_mut().unwrap().account_issuance_id = Some(account_id.controllers[0].issuance_id.clone());
    cases.push((account_id, routes.clone()));
    let mut public_id = intent;
    public_id.helper_report.as_mut().unwrap().public_issuance_id = Some(public_id.controllers[0].issuance_id.clone());
    cases.push((public_id, routes));

    for (invalid, observations) in cases {
        let result = invalid.resolve_controller_routes(observations)
            .and_then(|resolved| owner.configure_authorities(&mut helper, &resolved));
        assert!(result.is_err());
        assert_eq!(bytes(&owner), before, "all requests must resolve before any issuance");
        assert_eq!(owner.selected(), &selected);
        assert!(helper.controller_parent.is_empty());
        assert!(helper.report_capability.is_none());
    }
}

#[test]
fn missing_later_known_key_refuses_before_opening_the_canonical_pair() {
    let (_directory, root, selected, mut helper, requests) = fixture();
    let state = state_directory_in(&root, &selected.native_host.realm, NativeOwnerStartup::Initialize).unwrap();
    let startup = operator_setup::OperatorOwnerStartup::Initialize {
        namespace: EvidenceId::new("first-install-namespace").unwrap(), generation: 1,
    };
    let mut intent = PreparationAuthorityIntent::from(requests.clone());
    intent.controllers[1].controller_route = None;
    assert!(matches!(PreparedInstallationOwner::open_with_known_authorities(
        root.clone(), selected.clone(), startup.clone(), &intent), Err(HostLifetimeError::Unavailable)));
    assert!(!state.join("grants.jsonl").exists());
    assert!(!state.join("originals.sqlite3").exists());
    let (mut owner, resolved) = PreparedInstallationOwner::open_with_known_authorities(
        root, selected, startup, &PreparationAuthorityIntent::from(requests)).unwrap();
    owner.configure_authorities(&mut helper, &resolved).unwrap();
    assert!(!helper.controller_parent.is_empty());
    assert!(helper.report_capability.is_some());
}

#[test]
fn changed_observed_route_with_same_id_refuses_after_owner_restore() {
    let (_directory, root, selected, mut helper, requests) = fixture();
    let mut intent = PreparationAuthorityIntent::from(requests.clone());
    for request in &mut intent.controllers { request.controller_route = None; }
    let mut observations = observed_routes(&requests);
    let resolved = intent.resolve_controller_routes(observations.clone()).unwrap();
    let mut owner = initialize(root.clone(), selected.clone());
    owner.configure_authorities(&mut helper, &resolved).unwrap();
    let before = bytes(&owner);
    drop(owner);
    let mut restored = PreparedInstallationOwner::open(root, selected,
        operator_setup::OperatorOwnerStartup::Restore).unwrap();
    helper.controller_parent.clear(); helper.report_capability = None;
    observations[0].1 = ControllerRouteKey::parse(&format!("model-route/v1/{}", "b".repeat(64))).unwrap();
    let changed = intent.resolve_controller_routes(observations).unwrap();
    assert!(matches!(restored.configure_authorities(&mut helper, &changed), Err(HostLifetimeError::Denied)));
    assert_eq!(bytes(&restored), before);
    assert!(helper.controller_parent.is_empty());
    assert!(helper.report_capability.is_none());
}

#[test]
fn configured_public_parent_cannot_be_removed_or_replaced_on_recovery() {
    let (_directory, root, selected, mut helper, mut requests) = fixture();
    requests.helper_report.as_mut().unwrap().public_issuance_id = Some(EvidenceId::new("public-parent").unwrap());
    let mut owner = initialize(root, selected);
    owner.configure_authorities(&mut helper, &requests).unwrap();
    assert!(helper.report_capability.as_ref().unwrap().account_parent.is_none());
    assert!(helper.report_capability.as_ref().unwrap().public_parent.is_some());
    let before = bytes(&owner);
    for id in [None, Some(EvidenceId::new("replacement-public-parent").unwrap())] {
        let mut changed = requests.clone(); changed.helper_report.as_mut().unwrap().public_issuance_id = id;
        assert!(matches!(owner.configure_authorities(&mut helper, &changed), Err(HostLifetimeError::Denied)));
        assert_eq!(bytes(&owner), before);
    }
    owner.configure_authorities(&mut helper, &requests).unwrap();
    assert_eq!(bytes(&owner), before);
}

fn bundled_helper_server() -> meerkat_core::McpServerConfig {
    let mut server = meerkat_core::McpServerConfig::stdio("toolkit-setup", "fixture-packaged-runtime",
        vec![], Default::default());
    server.access = Some(meerkat_core::McpServerAccess::allowing(["setup_check", "setup_install"]));
    server
}
fn request_helper_mcp(requests: &mut PreparationAuthorities) {
    requests.helper_mcp = Some(PreparedHelperMcpRequest {
        issuance_id: EvidenceId::new("bundled-helper-mcp-parent").unwrap(),
    });
}

#[test]
fn bundled_helper_mcp_issues_helper_held_scope_before_original_target_and_recovers_exact_ids() {
    use meerkat_authorization_contracts::{constraints::DelegationDepth, work_association::GrantTarget};
    let (_directory, root, selected, mut helper, mut requests) = fixture();
    request_helper_mcp(&mut requests);
    let server = bundled_helper_server();
    let target = GrantTarget::McpServer { server: server.name.clone() };
    requests.agent_setup[0].helper_targets = vec![operator_setup::SetupTargetRequest {
        target: Some(target.clone()), issuance_id: EvidenceId::new("original-helper-mcp-parent").unwrap(),
    }];
    let mut owner = initialize(root.clone(), selected.clone());
    owner.configure_authorities_with_helper_server(&mut helper, &requests, Some(&server)).unwrap();
    let helper_lineage = helper.operation_basis.lineage_for(Some(&target)).unwrap();
    let original = &owner.selected().native_host.members[0].setup_capability.as_ref().unwrap().helper_targets[0];
    assert_eq!(helper_lineage.len(), 1);
    assert_eq!(helper_lineage[0].grant_id, requests.helper_mcp.as_ref().unwrap().issuance_id);
    assert_ne!(helper_lineage[0], original.parent);
    let expected = application_operations::mcp_access_restrictions(
        &selected.native_host.grant_administrator, &helper.logical_executor,
        &selected.native_host.members[0].audience, &server.name,
        ["setup_check", "setup_install"], &meerkat_core::mcp_config::McpConnectorApproval::NoConnector,
    );
    assert_eq!(expected.delegation_depth, DelegationDepth::remaining(0));
    let current = owner.source.grants().resolve_lineage(helper_lineage,
        &helper.logical_executor, helper.represented_subject.as_ref()).unwrap();
    assert_eq!(current.restrictions(), &expected);
    current.check_current().unwrap();
    let mut original_expected = expected;
    original_expected.delegation_depth = DelegationDepth::remaining(1);
    assert_eq!(original.restrictions, original_expected);
    let original_current = owner.source.grants().resolve_lineage(std::slice::from_ref(&original.parent),
        &selected.native_host.grant_administrator, selected.native_host.routes[0].represented_subject.as_ref()).unwrap();
    assert_eq!(original_current.restrictions(), &original_expected);
    original_current.check_current().unwrap();
    drop(current); drop(original_current);
    let complete = helper.clone();
    let selected_complete = owner.selected().clone();
    let before = bytes(&owner);
    owner.configure_authorities_with_helper_server(&mut helper, &requests, Some(&server)).unwrap();
    assert!(helper == complete);
    assert_eq!(bytes(&owner), before);
    // Simulate lost output by dropping the stopped owner and reopening its
    // actual canonical pair with the same request IDs, never Initialize.
    drop(owner);
    let mut owner = PreparedInstallationOwner::open(root, selected,
        operator_setup::OperatorOwnerStartup::Restore).unwrap();
    helper.controller_parent.clear(); helper.report_capability = None;
    helper.operation_basis = WorkAuthorityBasis::TargetedGrantLineages { base: None, targets: vec![] };
    owner.configure_authorities_with_helper_server(&mut helper, &requests, Some(&server)).unwrap();
    assert!(helper == complete);
    assert_eq!(owner.selected(), &selected_complete);
    assert_eq!(bytes(&owner), before);
}

#[test]
fn bundled_helper_mcp_requires_exact_finite_compiler_selection_before_any_issuance() {
    let (_directory, root, selected, mut helper, mut requests) = fixture();
    request_helper_mcp(&mut requests);
    let mut owner = initialize(root, selected);
    let before = bytes(&owner);
    assert!(matches!(owner.configure_authorities(&mut helper, &requests), Err(HostLifetimeError::Unavailable)));
    let mut missing = bundled_helper_server(); missing.access = None;
    let mut empty = bundled_helper_server(); empty.access = Some(meerkat_core::McpServerAccess::allowing(Vec::<String>::new()));
    let mut unnamed = bundled_helper_server(); unnamed.name.clear();
    let mut empty_operation = bundled_helper_server();
    empty_operation.access = Some(meerkat_core::McpServerAccess::allowing([""]));
    for server in [missing, empty, unnamed, empty_operation] {
        assert!(owner.configure_authorities_with_helper_server(&mut helper, &requests, Some(&server)).is_err());
        assert_eq!(bytes(&owner), before);
    }
    let mut foreign = helper.clone(); foreign.member.member = "unselected-helper".into();
    assert!(owner.configure_authorities_with_helper_server(&mut foreign, &requests, Some(&bundled_helper_server())).is_err());
    let mut duplicate = requests.clone();
    duplicate.helper_mcp.as_mut().unwrap().issuance_id = duplicate.controllers[0].issuance_id.clone();
    assert!(owner.configure_authorities_with_helper_server(&mut helper, &duplicate, Some(&bundled_helper_server())).is_err());
    assert_eq!(bytes(&owner), before);
    assert!(helper.operation_basis.grant_lineages().next().is_none());
}

#[test]
fn bundled_helper_mcp_rejects_changed_scope_replacement_and_revocation_without_reissue() {
    let (_directory, root, selected, mut helper, mut requests) = fixture();
    request_helper_mcp(&mut requests);
    let mut owner = initialize(root, selected);
    let server = bundled_helper_server();
    owner.configure_authorities_with_helper_server(&mut helper, &requests, Some(&server)).unwrap();
    let before = bytes(&owner);
    let original = helper.clone();
    let mut fewer = server.clone(); fewer.access = Some(meerkat_core::McpServerAccess::allowing(["setup_check"]));
    let mut extra = server.clone(); extra.access.as_mut().unwrap().allowed_operations.insert("other_write".into());
    let mut other = server.clone(); other.name = "other-server".into();
    for changed in [fewer, extra, other] {
        assert!(owner.configure_authorities_with_helper_server(&mut helper, &requests, Some(&changed)).is_err());
        assert!(helper == original);
        assert_eq!(bytes(&owner), before);
    }
    let mut foreign = helper.clone(); foreign.logical_executor = super::super::tests::principal("other-executor");
    assert!(owner.configure_authorities_with_helper_server(&mut foreign, &requests, Some(&server)).is_err());
    let mut other_subject = helper.clone(); other_subject.represented_subject = Some(super::super::tests::principal("other-subject"));
    assert!(owner.configure_authorities_with_helper_server(&mut other_subject, &requests, Some(&server)).is_err());
    let mut replacement = requests.clone();
    replacement.helper_mcp.as_mut().unwrap().issuance_id = EvidenceId::new("replacement-mcp-parent").unwrap();
    assert!(owner.configure_authorities_with_helper_server(&mut helper, &replacement, Some(&server)).is_err());
    assert_eq!(bytes(&owner), before);
    // Absence of a request preserves the entire selected scope and issues none.
    owner.configure_authorities_with_helper_server(&mut helper, &PreparationAuthorities::default(), Some(&server)).unwrap();
    assert!(helper == original);
    assert_eq!(bytes(&owner), before);
    let parent = helper.operation_basis.grant_lineages().next().unwrap()[0].clone();
    let machine = crate::original_ingress::tests::machine(&owner.source);
    let mut custody = machine.try_controller_grant_mutation().unwrap();
    owner.source.grants().revoke(owner.selected.native_host.remote_operator.as_ref().unwrap(), &parent, &mut custody).unwrap();
    drop(custody); drop(machine);
    let revoked = bytes(&owner);
    assert!(owner.configure_authorities_with_helper_server(&mut helper, &requests, Some(&server)).is_err());
    assert!(helper == original);
    assert_eq!(bytes(&owner), revoked);
}

#[test]
fn bundled_helper_mcp_preserves_existing_base_and_unrelated_target_constraints() {
    use meerkat_authorization_contracts::{evidence::EvidenceId, work_association::{GrantTarget, TargetGrantLineage}};
    let (_directory, root, selected, mut helper, _) = fixture();
    let mut owner = initialize(root, selected.clone());
    let operator = selected.native_host.remote_operator.as_ref().unwrap();
    let restrictions = application_operations::mcp_access_restrictions(
        &selected.native_host.grant_administrator, &helper.logical_executor,
        &selected.native_host.members[0].audience, "unrelated-server", ["read"],
        &meerkat_core::mcp_config::McpConnectorApproval::NoConnector,
    );
    let unrelated = owner.source.grants().issue_root(operator, EvidenceId::new("unrelated-parent").unwrap(),
        helper.logical_executor.clone(), helper.represented_subject.clone(), restrictions).unwrap();
    let base = Some(vec![unrelated.clone()]);
    let target = TargetGrantLineage {
        target: GrantTarget::McpServer { server: "unrelated-server".into() }, lineage: vec![unrelated],
    };
    helper.operation_basis = WorkAuthorityBasis::TargetedGrantLineages { base: base.clone(), targets: vec![target.clone()] };
    let mut request = PreparationAuthorities::default(); request_helper_mcp(&mut request);
    owner.configure_authorities_with_helper_server(&mut helper, &request, Some(&bundled_helper_server())).unwrap();
    let WorkAuthorityBasis::TargetedGrantLineages { base: actual, targets } = &helper.operation_basis else { panic!("same selection kind"); };
    assert_eq!(actual, &base);
    assert_eq!(targets.len(), 2);
    assert!(targets.contains(&target));
    let before = bytes(&owner);
    let kept = helper.clone();
    owner.configure_authorities_with_helper_server(&mut helper, &request, Some(&bundled_helper_server())).unwrap();
    assert!(helper == kept);
    assert_eq!(bytes(&owner), before);
}

#[test]
fn bundled_helper_mcp_intent_selects_only_issuance_identity_and_survives_held_resolution() {
    let request = serde_json::json!({"helper_mcp":{"issuance_id":"bundled-helper-mcp-parent"}});
    let intent: PreparationAuthorityIntent = serde_json::from_value(request.clone()).unwrap();
    let resolved = intent.resolve_controller_routes([]).unwrap();
    assert_eq!(resolved.helper_mcp.as_ref().unwrap().issuance_id.as_str(), "bundled-helper-mcp-parent");
    let round_trip = PreparationAuthorityIntent::from(resolved);
    assert_eq!(round_trip.helper_mcp.unwrap().issuance_id.as_str(), "bundled-helper-mcp-parent");
    for field in ["server", "operations", "actor", "restrictions"] {
        let mut changed = request.clone(); changed["helper_mcp"][field] = serde_json::json!("caller-selected");
        assert!(serde_json::from_value::<PreparationAuthorityIntent>(changed).is_err());
    }
}


fn request_helper_catalog(requests: &mut PreparationAuthorities) {
    requests.helper_catalog = Some(PreparedHelperCatalogRequest {
        issuance_id: EvidenceId::new("bundled-helper-catalog-parent").unwrap(),
        scope: PreparedHelperCatalogScope::ExactSession,
    });
}

#[test]
fn helper_catalog_issues_exact_session_base_before_original_target_and_recovers_same_id() {
    use meerkat_authorization_contracts::{constraints::DelegationDepth, work_association::GrantTarget};
    let (_directory, root, selected, mut helper, mut requests) = fixture();
    request_helper_catalog(&mut requests);
    request_helper_mcp(&mut requests);
    let server = bundled_helper_server();
    let target = GrantTarget::McpServer { server: server.name.clone() };
    requests.agent_setup[0].helper_targets = vec![
        operator_setup::SetupTargetRequest { target: None,
            issuance_id: EvidenceId::new("original-helper-catalog-parent").unwrap() },
        operator_setup::SetupTargetRequest { target: Some(target.clone()),
            issuance_id: EvidenceId::new("original-helper-mcp-parent").unwrap() },
    ];
    // This is a mechanical observation port into the actual canonical issuer,
    // not a claim that this fixture constructed a held native helper session.
    let session = meerkat_core::SessionId::new();
    let member = helper.member.clone();
    let mut owner = initialize(root.clone(), selected.clone());
    owner.configure_authorities_with_helper_resources(&mut helper, &requests, Some(&server), Some((&member, &session))).unwrap();
    let base = helper.operation_basis.lineage_for(None).unwrap();
    let mcp = helper.operation_basis.lineage_for(Some(&target)).unwrap();
    assert_eq!(base.len(), 1);
    assert_eq!(base[0].grant_id, requests.helper_catalog.as_ref().unwrap().issuance_id);
    assert_ne!(base, mcp, "catalog visibility is not MCP execution authority");
    let mut expected = application_operations::parent_restrictions(application_operations::catalog_read(
        &selected.native_host.grant_administrator, &helper.logical_executor,
        &selected.native_host.members[0].audience, &session));
    expected.delegation_depth = DelegationDepth::remaining(0);
    let current = owner.source.grants().resolve_lineage(base, &helper.logical_executor,
        helper.represented_subject.as_ref()).unwrap();
    assert_eq!(current.restrictions(), &expected);
    current.check_current().unwrap();
    let original = &owner.selected().native_host.members[0].setup_capability.as_ref().unwrap().helper_targets[0];
    assert!(original.target.is_none());
    assert_ne!(original.parent, base[0]);
    expected.delegation_depth = DelegationDepth::remaining(1);
    assert_eq!(original.restrictions, expected);
    let original_current = owner.source.grants().resolve_lineage(std::slice::from_ref(&original.parent),
        &selected.native_host.grant_administrator, selected.native_host.routes[0].represented_subject.as_ref()).unwrap();
    assert_eq!(original_current.restrictions(), &expected);
    original_current.check_current().unwrap();
    drop(current); drop(original_current);
    let complete = helper.clone();
    let selected_complete = owner.selected().clone();
    let before = bytes(&owner);
    owner.configure_authorities_with_helper_resources(&mut helper, &requests, Some(&server), Some((&member, &session))).unwrap();
    assert!(helper == complete);
    assert_eq!(bytes(&owner), before);
    drop(owner);
    let mut owner = PreparedInstallationOwner::open(root, selected, operator_setup::OperatorOwnerStartup::Restore).unwrap();
    helper.controller_parent.clear(); helper.report_capability = None;
    helper.operation_basis = WorkAuthorityBasis::TargetedGrantLineages { base: None, targets: vec![] };
    owner.configure_authorities_with_helper_resources(&mut helper, &requests, Some(&server), Some((&member, &session))).unwrap();
    assert!(helper == complete);
    assert_eq!(owner.selected(), &selected_complete);
    assert_eq!(bytes(&owner), before);
}

#[test]
fn helper_catalog_missing_or_cross_member_observation_preflights_all_sibling_issuers() {
    let (_directory, root, selected, mut helper, mut requests) = fixture();
    request_helper_catalog(&mut requests);
    request_helper_mcp(&mut requests);
    let mut owner = initialize(root, selected.clone());
    let before = bytes(&owner);
    let initial = helper.clone();
    let server = bundled_helper_server();
    assert!(matches!(owner.configure_authorities_with_helper_server(&mut helper, &requests, Some(&server)),
        Err(HostLifetimeError::Unavailable)));
    let session = meerkat_core::SessionId::new();
    let mut other = helper.member.clone(); other.member = "unselected-helper".into();
    assert!(matches!(owner.configure_authorities_with_helper_resources(&mut helper, &requests,
        Some(&server), Some((&other, &session))), Err(HostLifetimeError::Denied)));
    let mut duplicate = requests.clone();
    duplicate.helper_catalog.as_mut().unwrap().issuance_id = duplicate.helper_mcp.as_ref().unwrap().issuance_id.clone();
    let member = helper.member.clone();
    assert!(owner.configure_authorities_with_helper_resources(&mut helper, &duplicate,
        Some(&server), Some((&member, &session))).is_err());
    assert_eq!(bytes(&owner), before, "no controller/report/MCP grant may precede failed catalog preflight");
    assert!(helper == initial);
    assert_eq!(owner.selected(), &selected);
}

#[test]
fn helper_catalog_preserves_targets_and_refuses_changed_session_occupied_base_and_revocation() {
    let (_directory, root, selected, mut helper, mut requests) = fixture();
    request_helper_mcp(&mut requests);
    let mut owner = initialize(root, selected);
    let server = bundled_helper_server();
    owner.configure_authorities_with_helper_server(&mut helper, &requests, Some(&server)).unwrap();
    let targets = helper.operation_basis.clone();
    let mcp_parent = helper.operation_basis.grant_lineages().next().unwrap()[0].clone();
    request_helper_catalog(&mut requests);
    let member = helper.member.clone();
    let session = meerkat_core::SessionId::new();
    let before = bytes(&owner);
    let mut occupied = helper.clone();
    let WorkAuthorityBasis::TargetedGrantLineages { base, .. } = &mut occupied.operation_basis else { unreachable!() };
    *base = Some(vec![mcp_parent]);
    assert!(matches!(owner.configure_authorities_with_helper_resources(&mut occupied, &requests,
        Some(&server), Some((&member, &session))), Err(HostLifetimeError::Denied)));
    assert_eq!(bytes(&owner), before);
    owner.configure_authorities_with_helper_resources(&mut helper, &requests, Some(&server), Some((&member, &session))).unwrap();
    let WorkAuthorityBasis::TargetedGrantLineages { targets: old, .. } = &targets else { unreachable!() };
    let WorkAuthorityBasis::TargetedGrantLineages { targets: kept, .. } = &helper.operation_basis else { unreachable!() };
    assert_eq!(old, kept);
    let complete = helper.clone();
    let before = bytes(&owner);
    let other_session = meerkat_core::SessionId::new();
    assert!(matches!(owner.configure_authorities_with_helper_resources(&mut helper, &requests,
        Some(&server), Some((&member, &other_session))), Err(HostLifetimeError::Denied)));
    let mut replacement = requests.clone();
    replacement.helper_catalog.as_mut().unwrap().issuance_id = EvidenceId::new("replacement-catalog-parent").unwrap();
    assert!(owner.configure_authorities_with_helper_resources(&mut helper, &replacement,
        Some(&server), Some((&member, &session))).is_err());
    let mut foreign = helper.clone(); foreign.logical_executor = super::super::tests::principal("foreign-helper");
    assert!(owner.configure_authorities_with_helper_resources(&mut foreign, &requests,
        Some(&server), Some((&member, &session))).is_err());
    let mut subject = helper.clone(); subject.represented_subject = Some(super::super::tests::principal("foreign-subject"));
    assert!(owner.configure_authorities_with_helper_resources(&mut subject, &requests,
        Some(&server), Some((&member, &session))).is_err());
    owner.configure_authorities_with_helper_resources(&mut helper, &PreparationAuthorities::default(),
        Some(&server), None).unwrap();
    assert!(helper == complete);
    assert_eq!(bytes(&owner), before);
    let parent = helper.operation_basis.lineage_for(None).unwrap()[0].clone();
    let machine = crate::original_ingress::tests::machine(&owner.source);
    let mut custody = machine.try_controller_grant_mutation().unwrap();
    owner.source.grants().revoke(owner.selected.native_host.remote_operator.as_ref().unwrap(), &parent, &mut custody).unwrap();
    drop(custody); drop(machine);
    let revoked = bytes(&owner);
    assert!(owner.configure_authorities_with_helper_resources(&mut helper, &requests,
        Some(&server), Some((&member, &session))).is_err());
    assert!(helper == complete);
    assert_eq!(bytes(&owner), revoked);
}

#[test]
fn helper_catalog_intent_has_no_caller_session_and_preserves_exact_id() {
    let request = serde_json::json!({"helper_catalog":{"issuance_id":"bundled-helper-catalog-parent"}});
    let intent: PreparationAuthorityIntent = serde_json::from_value(request.clone()).unwrap();
    let resolved = intent.resolve_controller_routes([]).unwrap();
    assert_eq!(resolved.helper_catalog.as_ref().unwrap().issuance_id.as_str(), "bundled-helper-catalog-parent");
    assert!(resolved.helper_catalog.as_ref().unwrap().scope == PreparedHelperCatalogScope::ExactSession);
    assert_eq!(serde_json::to_value(resolved.helper_catalog.as_ref().unwrap()).unwrap(), request["helper_catalog"],
        "default scope preserves the legacy serialized request");
    assert_eq!(PreparationAuthorityIntent::from(resolved).helper_catalog.unwrap().issuance_id.as_str(), "bundled-helper-catalog-parent");
    for field in ["session", "member", "action", "resource", "actor", "restrictions"] {
        let mut changed = request.clone(); changed["helper_catalog"][field] = serde_json::json!("caller-selected");
        assert!(serde_json::from_value::<PreparationAuthorityIntent>(changed).is_err());
    }
    let mut successors = request.clone();
    successors["helper_catalog"]["scope"] = serde_json::json!("current_helper_successors");
    let resolved = serde_json::from_value::<PreparationAuthorityIntent>(successors.clone()).unwrap()
        .resolve_controller_routes([]).unwrap();
    assert!(resolved.helper_catalog.unwrap().scope == PreparedHelperCatalogScope::CurrentHelperSuccessors);
    for invalid in [serde_json::json!(null), serde_json::json!(true), serde_json::json!("all_sessions")] {
        successors["helper_catalog"]["scope"] = invalid;
        assert!(serde_json::from_value::<PreparationAuthorityIntent>(successors.clone()).is_err());
    }
}

#[test]
fn helper_catalog_successor_parent_is_explicit_catalog_only_and_not_an_executable_base() {
    let (_directory, root, selected, mut helper, mut requests) = fixture();
    request_helper_catalog(&mut requests);
    requests.helper_catalog.as_mut().unwrap().scope = PreparedHelperCatalogScope::CurrentHelperSuccessors;
    requests.agent_setup[0].helper_targets = vec![operator_setup::SetupTargetRequest {
        target: None, issuance_id: EvidenceId::new("explicit-original-successor-catalog").unwrap(),
    }];
    let session = meerkat_core::SessionId::new();
    let member = helper.member.clone();
    let mut owner = initialize(root.clone(), selected.clone());
    owner.configure_authorities_with_helper_resources(&mut helper, &requests, None, Some((&member, &session))).unwrap();
    assert!(helper.operation_basis.lineage_for(None).is_none());
    let capability = helper.catalog_capability.as_ref().unwrap();
    let expected = application_operations::configured_catalog_parent(&selected.native_host.grant_administrator, &helper.logical_executor,
        &selected.native_host.members[0].audience);
    let current = owner.source.grants().resolve_lineage(&capability.parent,
        &selected.native_host.grant_administrator, helper.represented_subject.as_ref()).unwrap();
    assert_eq!(current.restrictions(), &expected);
    current.check_current().unwrap();
    assert!(owner.source.grants().resolve_lineage(&capability.parent,
        &helper.logical_executor, helper.represented_subject.as_ref()).is_err());
    let original = &owner.selected().native_host.members[0].setup_capability.as_ref().unwrap().helper_targets[0];
    assert!(original.target.is_none());
    assert_ne!(original.parent, capability.parent[0]);
    assert_eq!(original.restrictions, expected);
    drop(current);
    let complete = helper.clone();
    let before = bytes(&owner);
    let successor = meerkat_core::SessionId::new();
    owner.configure_authorities_with_helper_resources(&mut helper, &requests, None, Some((&member, &successor))).unwrap();
    assert!(helper == complete);
    assert_eq!(bytes(&owner), before);
    let retained = serde_json::to_value(&helper).unwrap();
    assert!(serde_json::from_value::<acquisition::InstalledSetupSelection>(retained).unwrap() == helper);
}

#[test]
fn helper_catalog_successor_selection_refuses_old_exact_id_foreign_owner_and_revocation() {
    let (_directory, root, selected, mut helper, mut requests) = fixture();
    request_helper_catalog(&mut requests);
    let session = meerkat_core::SessionId::new();
    let member = helper.member.clone();
    let mut owner = initialize(root, selected);
    owner.configure_authorities_with_helper_resources(&mut helper, &requests, None, Some((&member, &session))).unwrap();
    let exact = helper.operation_basis.clone();
    let before = bytes(&owner);
    requests.helper_catalog.as_mut().unwrap().scope = PreparedHelperCatalogScope::CurrentHelperSuccessors;
    assert!(owner.configure_authorities_with_helper_resources(&mut helper, &requests, None, Some((&member, &session))).is_err());
    helper.operation_basis = WorkAuthorityBasis::TargetedGrantLineages { base: None, targets: vec![] };
    assert!(owner.configure_authorities_with_helper_resources(&mut helper, &requests, None, Some((&member, &session))).is_err(),
        "clearing local selection cannot reinterpret the old native depth-zero ID");
    assert_eq!(bytes(&owner), before);
    requests.helper_catalog.as_mut().unwrap().issuance_id = EvidenceId::new("new-explicit-successor-parent").unwrap();
    owner.configure_authorities_with_helper_resources(&mut helper, &requests, None, Some((&member, &session))).unwrap();
    let complete = helper.clone();
    let before = bytes(&owner);
    for (application, audience) in [
        (super::super::tests::principal("foreign-application"), owner.selected.native_host.members[0].audience.clone()),
        (owner.selected.native_host.grant_administrator.clone(), meerkat_authorization_contracts::constraints::AudienceRef::Principal {
            principal: super::super::tests::principal("foreign-audience"),
        }),
    ] {
        assert!(super::helper_catalog::PreparedHelperCatalog::select(&owner.source, &application,
            owner.selected.native_host.remote_operator.as_ref().unwrap(), &helper, &audience,
            requests.helper_catalog.as_ref(), Some((&member, &session))).is_err());
    }
    for change in 0..4 {
        let mut foreign = helper.clone();
        match change {
            0 => foreign.logical_executor = super::super::tests::principal("foreign-helper"),
            1 => foreign.represented_subject = Some(super::super::tests::principal("foreign-subject")),
            2 => foreign.catalog_capability.as_mut().unwrap().parent[0].grant_id = EvidenceId::new("foreign-parent").unwrap(),
            _ => foreign.operation_basis = exact.clone(),
        }
        assert!(owner.configure_authorities_with_helper_resources(&mut foreign, &requests, None, Some((&member, &session))).is_err());
    }
    assert_eq!(bytes(&owner), before);
    let parent = helper.catalog_capability.as_ref().unwrap().parent[0].clone();
    let machine = crate::original_ingress::tests::machine(&owner.source);
    let mut custody = machine.try_controller_grant_mutation().unwrap();
    owner.source.grants().revoke(owner.selected.native_host.remote_operator.as_ref().unwrap(), &parent, &mut custody).unwrap();
    drop(custody); drop(machine);
    let revoked = bytes(&owner);
    assert!(owner.configure_authorities_with_helper_resources(&mut helper, &requests, None, Some((&member, &meerkat_core::SessionId::new()))).is_err());
    assert!(helper == complete);
    assert_eq!(bytes(&owner), revoked);
}

#[test]
fn helper_catalog_successor_does_not_reinterpret_existing_original_catalog_target() {
    let (_directory, root, selected, mut helper, mut requests) = fixture();
    request_helper_catalog(&mut requests);
    requests.agent_setup[0].helper_targets = vec![operator_setup::SetupTargetRequest {
        target: None, issuance_id: EvidenceId::new("original-exact-catalog-parent").unwrap(),
    }];
    let session = meerkat_core::SessionId::new();
    let member = helper.member.clone();
    let mut owner = initialize(root, selected);
    owner.configure_authorities_with_helper_resources(&mut helper, &requests, None, Some((&member, &session))).unwrap();
    let original = owner.selected.native_host.members[0].setup_capability.as_ref().unwrap().clone();
    let old_target = original.helper_targets[0].clone();
    helper.operation_basis = WorkAuthorityBasis::TargetedGrantLineages { base: None, targets: vec![] };
    let catalog = requests.helper_catalog.as_mut().unwrap();
    catalog.issuance_id = EvidenceId::new("fresh-successor-catalog-parent").unwrap();
    catalog.scope = PreparedHelperCatalogScope::CurrentHelperSuccessors;
    assert!(owner.configure_authorities_with_helper_resources(&mut helper, &requests, None,
        Some((&member, &meerkat_core::SessionId::new()))).is_err());
    assert_eq!(owner.selected.native_host.members[0].setup_capability.as_ref(), Some(&original));
    let current = owner.source.grants().resolve_lineage(std::slice::from_ref(&old_target.parent),
        &owner.selected.native_host.grant_administrator, owner.selected.native_host.routes[0].represented_subject.as_ref()).unwrap();
    assert_eq!(current.restrictions(), &old_target.restrictions);
    current.check_current().unwrap();
    assert_ne!(current.restrictions(), &application_operations::configured_catalog_parent(&owner.selected.native_host.grant_administrator, &helper.logical_executor,
        &owner.selected.native_host.members[0].audience));
}

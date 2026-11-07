use super::*;
use meerkat_authorization_contracts::{
    constraints::{ActionRef, ResourceDomain, SetBound},
    evidence::EvidenceId,
};
use meerkat_core::{
    CompiledMemberToolGrant, CompiledToolConsequence, MobMemberBinding, PrincipalKind,
    TrustDomainId,
};
use meerkat_toolkit::host_lifetime::policy::{
    CompiledArtifactReference, CompiledMemberClass, CompiledMemberClassInstance,
    PreparedNativeMember,
};
use meerkat_workgraph::{WorkItemRef, WorkOwnerKey};
use toolkit_mobkit::private_originals::{
    PrivateAuthenticatedEvent, PrivateOriginalRef, PrivateSourceEvidence,
};
fn id(value: &str) -> Id {
    Id::new(value).unwrap()
}
fn evidence(value: &str) -> EvidenceId {
    EvidenceId::new(value).unwrap()
}
fn principal(value: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::ServiceAccount,
        value,
        TrustDomainId::new("commission-test").unwrap(),
    )
    .unwrap()
}
fn selected() -> (CompiledNativeHost, WorkflowCommissionRequest) {
    let requester = principal("requester");
    let domain = ResourceDomain {
        authority: principal("application"),
        namespace: "sources".into(),
    };
    let member = MobMemberBinding {
        mob_id: "commission-test".into(),
        role: "worker".into(),
        member: "parent".into(),
    };
    let route = CompiledNativeSourceRoute {
        id: id("parent"),
        origin: CompiledNativeSourceOrigin::LocalConsole {
            member: member.clone(),
            requester: requester.clone(),
            ingress_actor: principal("console"),
        },
        application_parent: None,
        controller_parent: None,
        logical_executor: principal("parent"),
        represented_subject: None,
        source_authority: principal("application"),
        authentication_domain: domain.clone(),
        event_domain: domain.clone(),
    };
    let class = CompiledMemberClass {
        id: id("shared"),
        purpose: CompiledMemberClassPurpose::SharedWorkflow,
        profile: "shared".into(),
        factory: id("host:shared"),
        callback_account: None,
        callbacks: vec![],
        grants: [
            "setup_check",
            "tool_catalog_load",
            "tool_catalog_search",
            "toolkit_request_public_service",
        ]
        .into_iter()
        .map(|name| CompiledMemberToolGrant {
            tool_name: name.into(),
            action: CompiledMemberToolAction::Invoke,
            consequence: CompiledToolConsequence::R0,
        })
        .collect(),
    };
    let native = CompiledNativeHost {
        authentication_domain: None,
        event_domain: None,
        realm: meerkat_core::mob_realm_id("commission-test").unwrap(),
        grant_administrator: principal("application"),
        remote_operator: Some(principal("operator")),
        service_parent_administration: None,
        ingress_domain: domain,
        application_tool_policy: CompiledArtifactReference {
            name: "policy.json".into(),
            sha256: "a".repeat(64),
        },
        setup: None,
        member_classes: vec![class],
        members: vec![],
        routes: vec![route],
    };
    let auth = PrivateSourceEvidence {
        resource_id: evidence("auth"),
        revision: evidence("1"),
        sha256: "b".repeat(64),
    };
    let parent = PrivateOriginalRef {
        route: id("parent"),
        original: evidence("parent-original"),
        event_sha256: "c".repeat(64),
    };
    let request = WorkflowCommissionRequest {
        admission_id: id("admission"), identity: meerkat_mobkit::identity_first::AgentIdentity::parse("lead").unwrap(),
        parent: PrivateOriginalSubmit { original: parent.clone(), content: "parent text".into(), transport_authentication: auth.clone() },
        instance: CompiledMemberClassInstance { class: id("shared"), member: PreparedNativeMember {
            member: MobMemberBinding { role: "shared".into(), member: "lead".into(), ..member },
            logical_owner: principal("lead"), context_generation: 1,
            audience: AudienceRef::Principal { principal: requester }, setup_capability: None } },
        event: PrivateAuthenticatedEvent { route: id("lead"), original: evidence("lead-original"), occurrence: evidence("occurrence"),
            original_authentication: auth.clone(), root_event: auth, contributors: vec![parent.original],
            content_sha256: ContentDigest::of_bytes(b"lead text").to_string() },
        content: "lead text".into(), workflow: id("workflow"),
        parent_work: serde_json::from_value::<WorkItemRef>(serde_json::json!({"realm_id": "commission-test", "namespace":"work", "item_id":"parent"})).unwrap(),
        parent_owner: WorkOwnerKey::agent("parent-owner").unwrap(), cycle_label: "cycle".into(),
        not_before_ms: 10, expires_at_ms: 500, deadline_ms: 400, max_waiters: 2, hold_timeout_ms: 100,
        controller_issuance_id: evidence("controller"), planning_root_issuance_id: evidence("root"), planning_leaf_issuance_id: evidence("leaf"),
    };
    (native, request)
}
#[test]
fn commissioning_projection_has_no_wire_authority_and_refuses_widened_or_foreign_selection() {
    let (native, request) = selected();
    let chosen = select(&native, &request, 100).unwrap();
    assert!(chosen.route.application_parent.is_none() && chosen.route.controller_parent.is_none());
    assert_eq!(
        chosen.route.authentication_domain,
        native.routes[0].authentication_domain
    );
    assert_eq!(
        chosen.route.source_authority,
        native.routes[0].source_authority
    );
    assert_eq!(chosen.operations, BTreeSet::from(["setup_check".into()]));
    let SetBound::Exact(actions) = chosen.envelope.actions.bound() else {
        panic!("finite actions required");
    };
    assert_eq!(
        actions,
        &BTreeSet::from([
            ActionRef {
                feature: "mcp".into(),
                action: "setup_check".into()
            },
            ActionRef {
                feature: "toolkit".into(),
                action: "catalog-read".into()
            },
            ActionRef {
                feature: "toolkit".into(),
                action: "setup-public-service-request".into()
            },
            meerkat_authorization::mcp_installation::mcp_connector_action(
                "toolkit-setup",
                &meerkat_core::mcp_config::McpConnectorApproval::NoConnector
            ),
        ])
    );
    let mut ordinary = native.clone();
    ordinary.member_classes[0].purpose = CompiledMemberClassPurpose::Ordinary;
    assert!(select(&ordinary, &request, 100).is_err());
    let mut foreign = request.clone();
    foreign.instance.member.audience = AudienceRef::Principal {
        principal: principal("other"),
    };
    assert!(select(&native, &foreign, 100).is_err());
    foreign = request.clone();
    foreign.identity = meerkat_mobkit::identity_first::AgentIdentity::parse("other").unwrap();
    assert!(select(&native, &foreign, 100).is_err());
    foreign = request.clone();
    foreign.event.contributors.clear();
    assert!(select(&native, &foreign, 100).is_err());
    foreign = request.clone();
    foreign.content.push_str(" changed");
    assert!(select(&native, &foreign, 100).is_err());
    assert!(select(&native, &request, 400).is_err());
    for field in [
        "grant",
        "backend",
        "storage",
        "controller_route",
        "planning",
        "input_authority_association",
    ] {
        let mut wire = serde_json::to_value(&request).unwrap();
        wire[field] = serde_json::json!({});
        assert!(
            serde_json::from_value::<WorkflowCommissionRequest>(wire).is_err(),
            "unexpected authority field {field}"
        );
    }
}

#[tokio::test]
async fn commissioning_parent_selector_reads_the_real_selected_graph_claim() {
    use meerkat_workgraph::{
        ClaimWorkItemRequest, CreateWorkItemRequest, SqliteWorkGraphStore, WorkGraphService,
        WorkNamespace, WorkOwner,
    };
    let directory = tempfile::tempdir().unwrap();
    let (native, mut request) = selected();
    let graph = WorkGraphService::with_scope(
        Arc::new(SqliteWorkGraphStore::open(directory.path().join("work.sqlite")).unwrap()),
        native.realm,
        WorkNamespace::new("work").unwrap(),
    );
    let row = graph
        .create(CreateWorkItemRequest {
            title: "actual parent".into(),
            labels: ["cycle".into()].into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let row = graph
        .claim(ClaimWorkItemRequest {
            id: row.id,
            realm_id: None,
            namespace: None,
            expected_revision: row.revision,
            owner: WorkOwner {
                key: request.parent_owner.clone(),
                display_name: None,
            },
            lease_seconds: Some(300),
            lease_expires_at: None,
        })
        .await
        .unwrap();
    // More than a page of same-cycle siblings must not hide this exact parent.
    // The snapshot lists the most recently updated items first, so the
    // siblings are created after the claim to keep the parent off page one.
    for number in 0..65 {
        graph
            .create(CreateWorkItemRequest {
                title: format!("same-cycle sibling {number}"),
                labels: ["cycle".into()].into(),
                ..Default::default()
            })
            .await
            .unwrap();
    }
    let first_page = graph
        .snapshot(meerkat_workgraph::WorkGraphSnapshotFilter {
            labels: vec!["cycle".into()],
            include_terminal: true,
            limit: Some(64),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(first_page.items.len(), 64);
    assert!(!first_page.items.iter().any(|item| item.id == row.id));
    request.parent_work = WorkItemRef {
        realm_id: row.realm_id,
        namespace: row.namespace,
        item_id: row.id,
    };
    let backend = SetupWorkBackend::new(
        graph,
        "commission-sqlite".into(),
        id("generation-one"),
        &[meerkat_core::DurabilityDeclaration::durable(
            "workgraph",
            meerkat_core::DurabilityResolution::Persistent,
        )],
    )
    .unwrap();
    check_parent_work(&backend, &request).await.unwrap();
    let mut foreign = request.clone();
    foreign.parent_owner = WorkOwnerKey::agent("other-owner").unwrap();
    assert!(matches!(
        check_parent_work(&backend, &foreign).await,
        Err(HostLifetimeError::Denied)
    ));
    foreign = request.clone();
    foreign.cycle_label = "other-cycle".into();
    assert!(matches!(
        check_parent_work(&backend, &foreign).await,
        Err(HostLifetimeError::Denied)
    ));
    foreign = request;
    foreign.parent_work.namespace = WorkNamespace::new("other").unwrap();
    assert!(matches!(
        check_parent_work(&backend, &foreign).await,
        Err(HostLifetimeError::Denied)
    ));
}

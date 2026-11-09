//! Boundary controls use real selected principals and native grant storage.
//! Tool identity below is validator input, not a claimed constructed G.
use super::*;
use meerkat_authorization_contracts::constraints::LifetimeRestriction;
use meerkat_core::{ToolIdentity, ToolProvenance};

fn owner() -> (tempfile::TempDir, OperatorSetup) {
    let (directory, selection) = crate::installed_host::tests::fixture();
    let owner = OperatorSetup::open(
        selection,
        OperatorOwnerStartup::fresh("late-helper-test", 1).unwrap(),
    )
    .unwrap();
    (directory, owner)
}

#[tokio::test]
async fn late_helper_envelope_refuses_widening_and_missing_actual_server() {
    let (_directory, owner) = owner();
    let prepared = owner.native.preparation();
    let member = &prepared.members[0];
    let route = &owner.native.routes[0];
    let mut server = McpServerConfig::stdio("toolkit-setup", "unused", vec![], Default::default());
    server.access = Some(meerkat_core::mcp_config::McpServerAccess::allowing([
        "setup_state",
    ]));
    let surface = [ToolIdentity {
        name: "setup_state".into(),
        provenance: Some(ToolProvenance::new(ToolSourceKind::Mcp, "toolkit-setup")),
    }];
    let mut envelope = application_operations::mcp_access_restrictions(
        &owner.native.grant_administrator,
        &route.logical_executor,
        &member.audience,
        &server.name,
        ["setup_state"],
        &meerkat_core::mcp_config::McpConnectorApproval::NoConnector,
    );
    let exact_mcp_domains = envelope.resource_domains.clone();
    envelope.resource_domains =
        DomainRestriction::owned_by([owner.native.grant_administrator.clone()]);
    let now = owner.source.clock().now().unwrap().unix_ms;
    envelope.delegation_depth = DelegationDepth::remaining(0);
    envelope.lifetime = LifetimeRestriction::window(now, now + 60_000);
    let request = WorkflowHelperAuthorityRequest {
        parent_issuance_id: EvidenceId::new("g-root").unwrap(),
        leaf_issuance_id: EvidenceId::new("g-leaf").unwrap(),
        envelope,
    };
    let validate = |request: &WorkflowHelperAuthorityRequest,
                    surface: Option<&[ToolIdentity]>,
                    server: &McpServerConfig| {
        validate_envelope(
            &owner.native.grant_administrator,
            member,
            route,
            surface,
            server,
            request,
        )
    };
    assert!(validate(&request, Some(&surface), &server).is_ok());
    assert!(matches!(
        validate(&request, None, &server),
        Err(HostLifetimeError::Denied)
    ));
    let foreign = [ToolIdentity {
        provenance: Some(ToolProvenance::new(ToolSourceKind::Mcp, "other")),
        ..surface[0].clone()
    }];
    assert!(matches!(
        validate(&request, Some(&foreign), &server),
        Err(HostLifetimeError::Denied)
    ));
    let mut no_access = server.clone();
    no_access.access = None;
    assert!(matches!(
        validate(&request, Some(&surface), &no_access),
        Err(HostLifetimeError::Denied)
    ));
    for change in 0..8 {
        let mut changed = request.clone();
        match change {
            0 => changed.leaf_issuance_id = changed.parent_issuance_id.clone(),
            1 => {
                changed.envelope.actions = ExactRestriction::exact([ActionRef {
                    feature: "toolkit".into(),
                    action: "setup-account-request".into(),
                }])
            }
            2 => {
                changed.envelope.resource_domains =
                    DomainRestriction::owned_by([route.logical_executor.clone()])
            }
            3 => {
                changed.envelope.processors = ExactRestriction::exact([ProcessorRef::Principal {
                    principal: owner.native.grant_administrator.clone(),
                }])
            }
            4 => changed.envelope.audiences = ExactRestriction::unrestricted(),
            5 => changed.envelope.delegation_depth = DelegationDepth::remaining(1),
            6 => changed.envelope.lifetime = LifetimeRestriction::unrestricted(),
            _ => changed.envelope.resource_domains = exact_mcp_domains.clone(),
        }
        assert!(
            matches!(
                validate(&changed, Some(&surface), &server),
                Err(HostLifetimeError::Denied)
            ),
            "case {change}"
        );
    }
    owner.finish().await.unwrap();
}

#[tokio::test]
async fn late_helper_native_root_replay_preserves_exact_bounds_and_identity() {
    let (_directory, owner) = owner();
    let grants = owner.source.grants();
    let id = EvidenceId::new("exact-late-root").unwrap();
    let now = owner.source.clock().now().unwrap().unix_ms;
    let bounds = ExecutionRestrictions {
        actions: ExactRestriction::exact([ActionRef {
            feature: "toolkit".into(),
            action: "catalog-read".into(),
        }]),
        lifetime: LifetimeRestriction::window(now, now + 60_000),
        delegation_depth: DelegationDepth::remaining(1),
        ..ExecutionRestrictions::unrestricted()
    };
    let root = issue_root(
        &grants,
        &owner.operator,
        &owner.native.grant_administrator,
        None,
        &id,
        &bounds,
    )
    .unwrap();
    assert_eq!(
        issue_root(
            &grants,
            &owner.operator,
            &owner.native.grant_administrator,
            None,
            &id,
            &bounds
        )
        .unwrap(),
        root
    );
    let mut changed = bounds.clone();
    changed.delegation_depth = DelegationDepth::remaining(2);
    assert!(matches!(
        issue_root(
            &grants,
            &owner.operator,
            &owner.native.grant_administrator,
            None,
            &id,
            &changed
        ),
        Err(HostLifetimeError::Denied)
    ));
    assert!(matches!(
        issue_root(
            &grants,
            &owner.operator,
            &owner.native.grant_administrator,
            Some(&owner.operator),
            &id,
            &bounds
        ),
        Err(HostLifetimeError::Denied)
    ));
    assert_eq!(grants.issued_reference(&id).unwrap(), Some(root.clone()));
    let resolved = grants
        .resolve_lineage(&[root], &owner.native.grant_administrator, None)
        .unwrap();
    assert_eq!(resolved.restrictions(), &bounds);
    resolved.check_current().unwrap();
    drop(resolved);
    drop(grants);
    owner.finish().await.unwrap();
}

#[tokio::test]
async fn late_helper_exact_mcp_target_records_connector_without_widening_base() {
    use meerkat_authorization::mcp_installation::{
        McpConnectorBound, lineage_connector_bound, mcp_connector_action,
    };
    use meerkat_authorization_contracts::constraints::ResourceDomain;
    let (_directory, owner) = owner();
    let grants = owner.source.grants();
    let application = &owner.native.grant_administrator;
    let route = &owner.native.routes[0];
    let preparation = owner.native.preparation();
    let audience = &preparation.members[0].audience;
    let mut server = McpServerConfig::stdio("toolkit-setup", "unused", vec![], Default::default());
    // Actual access includes another operation. Selection deliberately does not.
    server.access = Some(meerkat_core::mcp_config::McpServerAccess::allowing([
        "setup_state",
        "setup_check",
    ]));
    let mut envelope = application_operations::mcp_access_restrictions(
        application,
        &route.logical_executor,
        audience,
        &server.name,
        ["setup_state"],
        &meerkat_core::McpConnectorApproval::NoConnector,
    );
    let SetBound::Exact(actions) = envelope.actions.bound() else {
        panic!("exact actions")
    };
    envelope.actions = ExactRestriction::exact(actions.iter().cloned().chain([
        ActionRef {
            feature: "toolkit".into(),
            action: "catalog-read".into(),
        },
        ActionRef {
            feature: "toolkit".into(),
            action: "setup-public-service-request".into(),
        },
    ]));
    envelope.resource_domains = DomainRestriction::owned_by([application.clone()]);
    envelope.delegation_depth = DelegationDepth::remaining(0);
    let now = owner.source.clock().now().unwrap().unix_ms;
    envelope.lifetime = LifetimeRestriction::window(now, now + 60_000);
    let mut root_bounds = envelope.clone();
    root_bounds.delegation_depth = DelegationDepth::remaining(1);
    let root = issue_root(
        &grants,
        &owner.operator,
        application,
        route.represented_subject.as_ref(),
        &EvidenceId::new("targeted-g-root").unwrap(),
        &root_bounds,
    )
    .unwrap();
    let leaf = grants
        .issue_child(
            &meerkat_core::AdmissionGuard::none(),
            application,
            &root,
            EvidenceId::new("targeted-g-leaf").unwrap(),
            route.logical_executor.clone(),
            envelope.clone(),
        )
        .unwrap();
    let base = vec![root, leaf];
    let original = grants
        .resolve_lineage(
            &base,
            &route.logical_executor,
            route.represented_subject.as_ref(),
        )
        .unwrap();
    let approval = mcp_connector_action(
        &server.name,
        &meerkat_core::McpConnectorApproval::NoConnector,
    );
    let installation = ResourceDomain {
        authority: application.clone(),
        namespace: "mcp-server/toolkit-setup".into(),
    };
    // Reproduce the actual failed selection: its base action is present, but
    // OwnedBy does not record the exact native connector approval.
    assert_eq!(
        lineage_connector_bound(
            original.lineage_restrictions(),
            original.restrictions(),
            &approval,
            &installation
        ),
        McpConnectorBound::Unrecorded
    );
    let target = issue_mcp_target(
        &grants,
        &owner.operator,
        application,
        &route.logical_executor,
        route.represented_subject.as_ref(),
        &base,
        &envelope,
        &server,
    )
    .unwrap();
    let publication = owner.source.publication();
    {
        let _change = publication.begin_owner_change().unwrap();
        assert!(matches!(
            workflow_mcp::resolve_target_observed(
                &grants,
                application,
                &route.logical_executor,
                route.represented_subject.as_ref(),
                &base,
                &target,
                &envelope,
                &server
            ),
            Err(meerkat_authorization::grants::LineageResolutionRefusal::Changed)
        ));
    }
    let selected = workflow_mcp::resolve_target(
        &grants,
        application,
        &route.logical_executor,
        route.represented_subject.as_ref(),
        &base,
        &target,
        &envelope,
        &server,
    )
    .unwrap();
    assert_eq!(
        lineage_connector_bound(
            selected.lineage_restrictions(),
            selected.restrictions(),
            &approval,
            &installation
        ),
        McpConnectorBound::Approved
    );
    assert_eq!(
        selected.restrictions().actions,
        ExactRestriction::exact([
            ActionRef {
                feature: "mcp".into(),
                action: "setup_state".into()
            },
            approval,
        ])
    );
    assert_eq!(selected.restrictions().processors, envelope.processors);
    assert_eq!(selected.restrictions().audiences, envelope.audiences);
    assert_eq!(selected.restrictions().lifetime, envelope.lifetime);
    // A named P with this independently selected envelope remains a
    // narrowing ceiling. It need not claim G's connector approval itself.
    let narrowed = envelope.conjoin(selected.restrictions());
    let processor = ProcessorRef::Principal {
        principal: route.logical_executor.clone(),
    };
    for action in [
        ActionRef {
            feature: "mcp".into(),
            action: "setup_state".into(),
        },
        mcp_connector_action(
            &server.name,
            &meerkat_core::McpConnectorApproval::NoConnector,
        ),
    ] {
        narrowed
            .check_bounds(
                meerkat_authorization_contracts::constraints::OperationRestrictionValues {
                    action: &action,
                    resource_domain: &installation,
                    processor: &processor,
                    audience,
                    now_ms: now,
                },
            )
            .unwrap();
    }
    assert_eq!(
        grants
            .resolve_lineage(
                &base,
                &route.logical_executor,
                route.represented_subject.as_ref()
            )
            .unwrap()
            .restrictions(),
        &envelope
    );
    assert_eq!(
        issue_mcp_target(
            &grants,
            &owner.operator,
            application,
            &route.logical_executor,
            route.represented_subject.as_ref(),
            &base,
            &envelope,
            &server
        )
        .unwrap(),
        target
    );
    let mut changed = envelope.clone();
    changed.lifetime = LifetimeRestriction::window(now, now + 30_000);
    assert!(matches!(
        issue_mcp_target(
            &grants,
            &owner.operator,
            application,
            &route.logical_executor,
            route.represented_subject.as_ref(),
            &base,
            &changed,
            &server
        ),
        Err(HostLifetimeError::Denied)
    ));
    let mut foreign = server.clone();
    foreign.name = "other".into();
    assert!(matches!(
        workflow_mcp::target_bounds(application, &envelope, &foreign),
        Err(HostLifetimeError::Denied)
    ));
    let mut missing = server.clone();
    missing.access = None;
    assert!(matches!(
        workflow_mcp::target_bounds(application, &envelope, &missing),
        Err(HostLifetimeError::Denied)
    ));
    let machine = crate::original_ingress::tests::machine(&owner.source);
    {
        let mut custody = machine.try_controller_grant_mutation().unwrap();
        grants
            .revoke(
                &meerkat_core::AdmissionGuard::none(),
                &owner.operator,
                &target[0],
                &mut custody,
            )
            .unwrap();
    }
    assert!(selected.check_current().is_err());
    assert!(matches!(
        workflow_mcp::resolve_target(
            &grants,
            application,
            &route.logical_executor,
            route.represented_subject.as_ref(),
            &base,
            &target,
            &envelope,
            &server
        ),
        Err(HostLifetimeError::Denied)
    ));
    assert_eq!(
        grants
            .resolve_lineage(
                &base,
                &route.logical_executor,
                route.represented_subject.as_ref()
            )
            .unwrap()
            .restrictions(),
        &envelope
    );
    for reference in base.iter().chain(&target) {
        assert_eq!(
            grants
                .issued_reference(&reference.grant_id)
                .unwrap()
                .as_ref(),
            Some(reference)
        );
    }
    owner.finish().await.unwrap();
}

#[tokio::test]
async fn workflow_public_child_preserves_legacy_replay_and_target_local_revocation() {
    use crate::installed_host::operator_setup::workflow_public;
    use meerkat_authorization_contracts::work_association::WorkAuthorityBasis;
    let (_directory, selection) = crate::installed_host::tests::fixture();
    let owner = OperatorSetup::open(
        selection.clone(),
        OperatorOwnerStartup::fresh("public-target-test", 1).unwrap(),
    )
    .unwrap();
    let path = state_directory(&selection, &owner.native, NativeOwnerStartup::Restore)
        .unwrap()
        .join("grants.jsonl");
    let grants = owner.source.grants();
    let application = &owner.native.grant_administrator;
    let route = &owner.native.routes[0];
    let preparation = owner.native.preparation();
    let mut server = McpServerConfig::stdio("toolkit-setup", "unused", vec![], Default::default());
    server.access = Some(meerkat_core::McpServerAccess::allowing(["setup_state"]));
    let mut envelope = application_operations::mcp_access_restrictions(
        application,
        &route.logical_executor,
        &preparation.members[0].audience,
        &server.name,
        ["setup_state"],
        &meerkat_core::McpConnectorApproval::NoConnector,
    );
    let SetBound::Exact(actions) = envelope.actions.bound() else {
        panic!("exact actions")
    };
    envelope.actions = ExactRestriction::exact(actions.iter().cloned().chain([
        ActionRef {
            feature: "toolkit".into(),
            action: "catalog-read".into(),
        },
        ActionRef {
            feature: "toolkit".into(),
            action: "setup-public-service-request".into(),
        },
    ]));
    envelope.resource_domains = DomainRestriction::owned_by([application.clone()]);
    envelope.delegation_depth = DelegationDepth::remaining(0);
    let now = owner.source.clock().now().unwrap().unix_ms;
    envelope.lifetime = LifetimeRestriction::window(now, now + 60_000);
    let mut root_bounds = envelope.clone();
    root_bounds.delegation_depth = DelegationDepth::remaining(1);
    let root = issue_root(
        &grants,
        &owner.operator,
        application,
        route.represented_subject.as_ref(),
        &EvidenceId::new("public-g-root").unwrap(),
        &root_bounds,
    )
    .unwrap();
    let leaf = grants
        .issue_child(
            &meerkat_core::AdmissionGuard::none(),
            application,
            &root,
            EvidenceId::new("public-g-base-leaf").unwrap(),
            route.logical_executor.clone(),
            envelope.clone(),
        )
        .unwrap();
    let base = vec![root, leaf];
    let mcp = issue_mcp_target(
        &grants,
        &owner.operator,
        application,
        &route.logical_executor,
        route.represented_subject.as_ref(),
        &base,
        &envelope,
        &server,
    )
    .unwrap();
    let legacy = workflow_mcp::basis(base.clone(), mcp.clone());
    let single = WorkAuthorityBasis::GrantLineage {
        lineage: base.clone(),
    };
    let before = std::fs::read(&path).unwrap();
    let select = |saved, bounds: &ExecutionRestrictions, needs_mcp| {
        workflow_public::select(
            &grants,
            application,
            &route.logical_executor,
            route.represented_subject.as_ref(),
            &base,
            bounds,
            saved,
            needs_mcp,
        )
    };
    // Exact already-bound MCP-only selection remains old even though public
    // intent is present. The single-lineage decoder keeps its legacy shape.
    assert!(select(Some(&legacy), &envelope, true).unwrap().is_none());
    assert!(workflow_mcp::public_lineage(&single).unwrap().is_none());
    assert_eq!(
        workflow_mcp::split(&single).unwrap(),
        (base.as_slice(), None)
    );
    let mut absent = envelope.clone();
    absent.actions = ExactRestriction::exact([ActionRef {
        feature: "toolkit".into(),
        action: "catalog-read".into(),
    }]);
    assert!(select(None, &absent, true).unwrap().is_none());
    assert!(matches!(
        workflow_public::issue(
            &grants,
            application,
            &route.logical_executor,
            route.represented_subject.as_ref(),
            &base,
            &absent
        ),
        Err(HostLifetimeError::Denied)
    ));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let public = select(None, &envelope, true).unwrap().unwrap();
    assert_eq!(public[0], base[0]);
    assert_ne!(public[1], base[1]);
    let after = std::fs::read(&path).unwrap();
    assert_eq!(
        String::from_utf8(after.clone()).unwrap().lines().count(),
        String::from_utf8(before).unwrap().lines().count() + 1,
        "exactly one new child, no new root"
    );
    assert_eq!(
        select(None, &envelope, true).unwrap().unwrap(),
        public,
        "lost reply reconciles the same child"
    );
    let basis = workflow_mcp::basis_with_public(base.clone(), mcp.clone(), Some(public.clone()));
    let encoded = serde_json::to_vec(&basis).unwrap();
    let saved: WorkAuthorityBasis = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(
        select(Some(&saved), &envelope, true).unwrap().unwrap(),
        public
    );
    for case in 0..4 {
        let mut changed = envelope.clone();
        match case {
            0 => changed.lifetime = LifetimeRestriction::window(now, now + 30_000),
            1 => {
                changed.resource_domains =
                    DomainRestriction::owned_by([route.logical_executor.clone()])
            }
            2 => changed.processors = ExactRestriction::unrestricted(),
            _ => changed.audiences = ExactRestriction::unrestricted(),
        }
        assert!(
            matches!(select(None, &changed, true), Err(HostLifetimeError::Denied)),
            "changed bounds {case}"
        );
    }
    let mut foreign = public.clone();
    foreign[1] = base[1].clone();
    assert!(
        workflow_mcp::split(&workflow_mcp::basis_with_public(
            base.clone(),
            mcp.clone(),
            Some(foreign)
        ))
        .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), after);
    let machine = crate::original_ingress::tests::machine(&owner.source);
    {
        let mut custody = machine.try_controller_grant_mutation().unwrap();
        grants
            .revoke(
                &meerkat_core::AdmissionGuard::none(),
                &owner.operator,
                &public[1],
                &mut custody,
            )
            .unwrap();
    }
    let revoked = std::fs::read(&path).unwrap();
    assert!(matches!(
        workflow_public::resolve(
            &grants,
            application,
            &route.logical_executor,
            route.represented_subject.as_ref(),
            &base,
            &public,
            &envelope
        ),
        Err(HostLifetimeError::Denied)
    ));
    assert!(matches!(
        select(None, &envelope, true),
        Err(HostLifetimeError::Denied)
    ));
    assert_eq!(
        select(Some(&saved), &envelope, true).unwrap().unwrap(),
        public
    );
    assert!(
        select(Some(&legacy), &envelope, true).unwrap().is_none(),
        "no legacy retrofit after another selection issued"
    );
    assert_eq!(serde_json::to_vec(&saved).unwrap(), encoded);
    for lineage in workflow_mcp::planning_lineages(&saved).unwrap() {
        grants
            .resolve_lineage(
                lineage,
                &route.logical_executor,
                route.represented_subject.as_ref(),
            )
            .unwrap()
            .check_current()
            .unwrap();
    }
    workflow_mcp::resolve_target(
        &grants,
        application,
        &route.logical_executor,
        route.represented_subject.as_ref(),
        &base,
        &mcp,
        &envelope,
        &server,
    )
    .unwrap()
    .check_current()
    .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), revoked);
    owner.finish().await.unwrap();
}

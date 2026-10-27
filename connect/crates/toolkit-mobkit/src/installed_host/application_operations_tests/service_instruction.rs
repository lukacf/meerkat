//! Native-policy conjunction controls using the existing installed component
//! fixture. Native input admission, helper dispatch, account authentication and
//! physical MCP callability are not exercised by this fixture.
use super::*;

async fn service_fixture() -> Fixture {
    Fixture::with_operations(|native, _| {
        let mut bounds = application_operations::mcp_access_restrictions(
            &native.grant_administrator, &native.routes[0].logical_executor,
            &native.members[0].audience, "reviewed-connector", ["read", "list"],
            &meerkat_core::mcp_config::McpConnectorApproval::NoConnector,
        );
        bounds.delegation_depth = DelegationDepth::remaining(1);
        bounds
    }).await
}

async fn controller_only_a(f: &Fixture) -> InputAuthorityAssociation {
    let originals = &f.application.originals;
    let mut event = crate::original_ingress::tests::private_event("original-a");
    event.route = f.application.native.routes[0].id.clone();
    let reference = originals.retain_event(event.clone()).await.unwrap();
    let mut candidate = f.association.candidate().clone();
    candidate.original_work.work = event.original;
    candidate.ingress_namespace.occurrence_scope = event.occurrence;
    for evidence in [&mut candidate.original_authentication, &mut candidate.root_event] {
        evidence.resource.resource_id = "original-a".into();
        evidence.digest = EvidenceDigest::of_bytes(b"original-a");
    }
    candidate.authority_basis = WorkAuthorityBasis::TargetedGrantLineages { base: None, targets: vec![] };
    candidate.admitted_ceiling = ExecutionRestrictions::unrestricted();
    let association = InputAuthorityAssociation::new(candidate).unwrap();
    let persist_reference = &reference;
    let retained = f.source.bind_controller_only_async(
        association.clone(), crate::original_ingress::tests::source(&association),
        &association.candidate().requester, &association.candidate().ingress_actor,
        |association| async move { originals.persist_binding(persist_reference, &association).await },
    ).await.unwrap();
    originals.hold_binding(reference, retained).await.unwrap();
    association
}

#[tokio::test]
async fn service_instruction_does_not_elevate_unchanged_a() {
    let f = service_fixture().await;
    let a = controller_only_a(&f).await;
    let before = a.clone();
    let context = f.policy.work_context(vec![a.clone()].into(), scope(&f.session)).unwrap();
    for operation in ["read", "list"] {
        let binding = mcp_policy_binding(&f, "reviewed-connector", operation, 1);
        denied(context.authorization().prepare(&binding));
        // B's actual native operation lineage is independently valid. Neither
        // that fact nor retaining the same session changes A's association.
        f.context.authorization().prepare(&binding).unwrap().check_current(&binding).unwrap();
        denied(context.authorization().prepare(&binding));
    }
    assert_eq!(a, before);
}

#[tokio::test]
async fn service_instruction_a_plus_b_conjoins_controller_only_a() {
    let f = service_fixture().await;
    let a = controller_only_a(&f).await;
    let context = f.policy.work_context(vec![a, f.association.clone()].into(), scope(&f.session)).unwrap();
    for operation in ["read", "list"] {
        denied(context.authorization().prepare(&mcp_policy_binding(
            &f, "reviewed-connector", operation, 1,
        )));
    }
}

#[tokio::test]
async fn service_instruction_b_only_keeps_session_without_inheriting_a() {
    let f = service_fixture().await;
    let a = controller_only_a(&f).await;
    assert_eq!(a.candidate().target.context, f.association.candidate().target.context);
    assert_ne!(a.candidate().original_work, f.association.candidate().original_work);
    let context = f.policy.work_context(vec![f.association.clone()].into(), scope(&f.session)).unwrap();
    for operation in ["read", "list"] {
        let binding = mcp_policy_binding(&f, "reviewed-connector", operation, 1);
        context.authorization().prepare(&binding).unwrap().check_current(&binding).unwrap();
    }
    denied(context.authorization().prepare(&mcp_policy_binding(
        &f, "reviewed-connector", "unapproved-operation", 1,
    )));
}

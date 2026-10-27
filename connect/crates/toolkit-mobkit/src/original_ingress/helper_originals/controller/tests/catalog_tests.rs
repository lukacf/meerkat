//! Canonical intent/grant controls; the separate native fixture covers reset.
use meerkat_authorization_contracts::constraints::DomainRestriction;
use super::*;

fn issue(f: &Fixture, intent: &ChildGrantIntent) -> Vec<GrantLineageRef> {
    let leaf = f.owner.grants.issued_reference(&intent.id).unwrap().unwrap_or_else(||
        f.owner.grants.issue_child(&intent.issuer, intent.parent_lineage.last().unwrap(),
            intent.id.clone(), intent.grantee.clone(), intent.requested.clone()).unwrap());
    let mut lineage = intent.parent_lineage.clone(); lineage.push(leaf); lineage
}

async fn issued(f: &Fixture) -> (HelperNativeBinding, ChildGrantIntent) {
    let (_, report, account, public, catalog) = f.owner.retain_helper_operation_intents(
        &f.store, &f.admitted, f.candidate()).await.unwrap();
    let catalog = catalog.unwrap();
    let helper = &f.admitted.selection().helper;
    let basis = helper.catalog_capability.as_ref().unwrap()
        .project(&helper.operation_basis, issue(f, &catalog)).unwrap();
    let basis = helper.report_capability.as_ref().unwrap().project_with_operations(&basis,
        issue(f, &report.unwrap()), Some(issue(f, &account.unwrap())), Some(issue(f, &public.unwrap()))).unwrap();
    let mut binding = stage_fixture_controller(&f.owner, &f.store, &f.admitted, f.binding.clone()).await;
    binding.authority_basis = basis;
    (binding, catalog)
}

#[tokio::test]
async fn helper_catalog_child_freezes_current_session_and_preserves_other_operations() {
    use meerkat_authorization_contracts::work_association::GrantTarget;
    let f = Fixture::with_catalog().await;
    let candidate = f.candidate();
    let (_, _, _, _, catalog) = f.owner.retain_helper_operation_intents(
        &f.store, &f.admitted, candidate.clone()).await.unwrap();
    let catalog = catalog.unwrap();
    let pending = f.row().await.unwrap();
    assert!(f.owner.grants.issued_reference(&catalog.id).unwrap().is_none());
    f.assert_no_source();
    let session = meerkat_core::SessionId::parse(f.binding.target.context.as_str()).unwrap();
    let mut expected = crate::installed_host::application_operations::parent_restrictions(
        crate::installed_host::application_operations::catalog_read(&f.admitted.selection().grant_caller,
            &f.admitted.selection().helper.logical_executor, &f.binding.target.audience, &session));
    expected.delegation_depth = DelegationDepth::remaining(0);
    assert_eq!(catalog.requested, expected);
    for change in 0..3 {
        let mut changed = candidate.clone();
        match change {
            0 => changed.target.context = EvidenceId::new(meerkat_core::SessionId::new().to_string()).unwrap(),
            1 => changed.catalog = None,
            _ => {
                let mut value = serde_json::to_value(&changed).unwrap();
                value["catalog"]["requested"] = serde_json::to_value(ExecutionRestrictions::unrestricted()).unwrap();
                changed = serde_json::from_value(value).unwrap();
            }
        }
        assert!(f.owner.retain_helper_operation_intents(&f.store, &f.admitted, changed).await.is_err());
    }
    assert_eq!(f.row().await.unwrap(), pending);
    let (binding, issued_catalog) = issued(&f).await;
    assert_eq!(issued_catalog, catalog);
    let helper = &f.admitted.selection().helper;
    let mcp = GrantTarget::McpServer { server: "existing-server".into() };
    assert_eq!(binding.authority_basis.lineage_for(Some(&mcp)), helper.operation_basis.lineage_for(Some(&mcp)));
    let current = f.owner.grants.resolve_lineage(binding.authority_basis.lineage_for(None).unwrap(),
        &helper.logical_executor, helper.represented_subject.as_ref()).unwrap();
    assert_eq!(current.restrictions(), &expected);
    assert_eq!(current.restrictions().resource_domains,
        DomainRestriction::exact([
            meerkat_authorization_contracts::constraints::ResourceDomain {
                authority: f.admitted.selection().grant_caller.clone(),
                namespace: format!("toolkit-session-catalog/{session}"),
            },
        ]));
    current.check_current().unwrap();
    drop(current);
    let source = f.owner.bind_admitted_helper(&f.store, &f.admitted, binding.clone()).await.unwrap();
    assert!(source.policy_source().is_ok());
    let replay = f.owner.bind_admitted_helper(&f.store, &f.admitted, binding.clone()).await.unwrap();
    assert_eq!(source.source_record, replay.source_record);
    for change in 0..3 {
        let mut changed = binding.clone();
        match change {
            0 => changed.target.context = EvidenceId::new(meerkat_core::SessionId::new().to_string()).unwrap(),
            1 => changed.logical_executor = crate::original_ingress::tests::principal("foreign-helper"),
            _ => {
                let WorkAuthorityBasis::TargetedGrantLineages { base, .. } = &mut changed.authority_basis else { unreachable!() };
                *base = Some(helper.catalog_capability.as_ref().unwrap().parent.clone());
            }
        }
        assert!(f.owner.bind_admitted_helper(&f.store, &f.admitted, changed).await.is_err());
    }
}

#[tokio::test]
async fn helper_catalog_revocation_preserves_historical_binding_but_refuses_current_use() {
    for revoke_parent in [false, true] {
        let f = Fixture::with_catalog().await;
        let (binding, _) = issued(&f).await;
        let source = f.owner.bind_admitted_helper(&f.store, &f.admitted, binding.clone()).await.unwrap();
        let saved = f.row().await.unwrap();
        let helper = &f.admitted.selection().helper;
        let lineage = binding.authority_basis.lineage_for(None).unwrap();
        let observed = f.owner.grants.resolve_lineage(lineage, &helper.logical_executor,
            helper.represented_subject.as_ref()).unwrap();
        let reference = if revoke_parent { &lineage[0] } else { lineage.last().unwrap() };
        let machine = crate::original_ingress::tests::machine(&f.owner);
        let mut custody = machine.try_controller_grant_mutation().unwrap();
        f.owner.grants.revoke(&crate::original_ingress::tests::principal("admin"), reference, &mut custody).unwrap();
        drop(custody); drop(machine);
        assert!(observed.check_current().is_err());
        assert!(f.owner.grants.resolve_lineage(lineage, &helper.logical_executor,
            helper.represented_subject.as_ref()).is_err());
        assert!(f.candidate().catalog.as_ref().unwrap().check_current(
            &f.owner, &f.admitted.facts, &binding.authority_basis).is_err());
        // The immutable source is historical evidence, not current catalog permission.
        let replay = f.owner.bind_admitted_helper(&f.store, &f.admitted, binding.clone()).await.unwrap();
        assert_eq!(replay.source_record, source.source_record);
        assert_eq!(replay.original.association(), source.original.association());
        assert_eq!(f.row().await.unwrap(), saved);
        assert!(replay.policy_source().is_ok());
        assert_independent_target_current(&f, &binding);
    }
}

#[tokio::test]
async fn helper_catalog_revocation_before_first_binding_preserves_pending_source() {
    for revoke_parent in [false, true] {
        let f = Fixture::with_catalog().await;
        let (binding, _) = issued(&f).await;
        let pending = f.row().await.unwrap();
        assert!(HelperSourceRecord::decode(&pending).unwrap().bound().is_err());
        let lineage = binding.authority_basis.lineage_for(None).unwrap();
        let reference = if revoke_parent { &lineage[0] } else { lineage.last().unwrap() };
        let machine = crate::original_ingress::tests::machine(&f.owner);
        let mut custody = machine.try_controller_grant_mutation().unwrap();
        f.owner.grants.revoke(&crate::original_ingress::tests::principal("admin"), reference, &mut custody).unwrap();
        drop(custody); drop(machine);
        assert!(f.owner.bind_admitted_helper(&f.store, &f.admitted, binding.clone()).await.is_err());
        assert_eq!(f.row().await.unwrap(), pending);
        f.assert_no_source();
        assert_independent_target_current(&f, &binding);
    }
}

fn assert_independent_target_current(f: &Fixture, binding: &HelperNativeBinding) {
    let target = meerkat_authorization_contracts::work_association::GrantTarget::McpServer {
        server: "existing-server".into(),
    };
    let helper = &f.admitted.selection().helper;
    let current = f.owner.grants.resolve_lineage(binding.authority_basis.lineage_for(Some(&target)).unwrap(),
        &helper.logical_executor, helper.represented_subject.as_ref()).unwrap();
    current.check_current().unwrap();
}

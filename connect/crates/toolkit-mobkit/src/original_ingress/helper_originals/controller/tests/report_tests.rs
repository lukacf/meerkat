//! Same canonical SQLite/grant-owner controls, not live native dispatch proof.
use super::*;

pub(super) async fn issued(fixture: &Fixture) -> (HelperNativeBinding, ChildGrantIntent) {
    let candidate = fixture.candidate();
    let (_, report) = fixture.owner.retain_helper_grant_intents(
        &fixture.store, &fixture.admitted, candidate,
    ).await.unwrap();
    let report = report.unwrap();
    let mut binding = stage_fixture_controller(&fixture.owner, &fixture.store,
        &fixture.admitted, fixture.binding.clone()).await;
    let leaf = match fixture.owner.grants.issued_reference(&report.id).unwrap() {
        Some(leaf) => leaf,
        None => fixture.owner.grants.issue_child(&report.issuer, report.parent_lineage.last().unwrap(),
            report.id.clone(), report.grantee.clone(), report.requested.clone()).unwrap(),
    };
    let mut lineage = report.parent_lineage.clone(); lineage.push(leaf);
    binding.authority_basis = fixture.admitted.selection().helper.report_capability.as_ref().unwrap()
        .project(&fixture.admitted.selection().helper.operation_basis, lineage).unwrap();
    (binding, report)
}

#[tokio::test]
async fn report_intent_stages_before_issue_and_refuses_changed_session_parent_or_intent() {
    let fixture = Fixture::build(true).await;
    let candidate = fixture.candidate();
    let (controller, report) = fixture.owner.retain_helper_grant_intents(
        &fixture.store, &fixture.admitted, candidate.clone(),
    ).await.unwrap();
    let report = report.unwrap();
    assert_ne!(controller.id, report.id);
    assert_ne!(controller.parent_lineage, report.parent_lineage);
    assert_eq!(report.requested.delegation_depth, DelegationDepth::remaining(0));
    let saved = fixture.row().await.unwrap();
    fixture.assert_no_source();
    assert!(fixture.owner.observe_helper_binding(&fixture.store, &fixture.admitted).await.unwrap().is_none());
    let mut changed = candidate.clone();
    changed.target.context = EvidenceId::new(meerkat_core::SessionId::new().to_string()).unwrap();
    let mut changed_value = serde_json::to_value(&candidate).unwrap();
    changed_value["report"]["requested"] = serde_json::to_value(ExecutionRestrictions::unrestricted()).unwrap();
    let changed_intent: HelperControllerIntent = serde_json::from_value(changed_value).unwrap();
    for changed in [changed, changed_intent] {
        assert!(matches!(fixture.owner.retain_helper_grant_intents(&fixture.store, &fixture.admitted, changed).await,
            Err(OriginalWriterError::Conflict)));
    }
    let mut different = fixture.admitted.facts.clone();
    different.selection.helper.report_capability.as_mut().unwrap().parent = controller.parent_lineage.clone();
    let changed = AdmittedHelperWork { owner: fixture.owner.clone(), source_scope: fixture.admitted.source_scope.clone(),
        facts: different, contributor: fixture.admitted.contributor.clone(), contributors: fixture.admitted.contributors.clone() };
    assert!(fixture.owner.retain_helper_grant_intents(&fixture.store, &changed, candidate).await.is_err());
    assert_eq!(fixture.row().await.unwrap(), saved);
    for id in [&controller.id, &report.id] { assert!(fixture.owner.grants.issued_reference(id).unwrap().is_none()); }
}

#[tokio::test]
async fn report_stage_unknown_never_issues_and_final_cas_loss_recovers_exact_base() {
    for fault in [1, 2, 3, 4] {
        let fixture = Fixture::build(true).await;
        let candidate = fixture.candidate();
        if fault <= 2 {
            fixture.store.fault.store(fault, Ordering::SeqCst);
            assert!(matches!(fixture.owner.retain_helper_grant_intents(&fixture.store, &fixture.admitted, candidate.clone()).await,
                Err(OriginalWriterError::Unavailable)));
            let report = candidate.report.as_ref().unwrap().native(&fixture.admitted.facts).unwrap();
            assert!(fixture.owner.grants.issued_reference(&report.id).unwrap().is_none());
            assert!(fixture.owner.grants.issued_reference(&candidate.id).unwrap().is_none());
            fixture.assert_no_source();
        }
        let (binding, report) = issued(&fixture).await;
        let leaf = fixture.owner.grants.issued_reference(&report.id).unwrap().unwrap();
        // Discarded native reply recovers the same actual child, no new ID.
        let (again, same) = issued(&fixture).await;
        assert_eq!(same, report);
        assert!(again == binding);
        if fault >= 3 {
            fixture.store.fault.store(fault, Ordering::SeqCst);
            assert!(matches!(fixture.owner.bind_admitted_helper(&fixture.store, &fixture.admitted, binding.clone()).await,
                Err(OriginalWriterError::Unavailable)));
            fixture.assert_no_source();
        }
        let source = fixture.owner.bind_admitted_helper(&fixture.store, &fixture.admitted, binding.clone()).await.unwrap();
        let replay = fixture.owner.bind_admitted_helper(&fixture.store, &fixture.admitted, binding.clone()).await.unwrap();
        assert_eq!(source.source_record, replay.source_record);
        assert!(Arc::ptr_eq(&source.original, &replay.original));
        assert_eq!(source.original.association().candidate().authority_basis, binding.authority_basis);
        assert!(source.policy_source().is_ok());
        assert_eq!(fixture.owner.grants.issued_reference(&report.id).unwrap(), Some(leaf));
        let mut no_report = binding;
        no_report.authority_basis = fixture.admitted.selection().helper.operation_basis.clone();
        assert!(matches!(fixture.owner.bind_admitted_helper(&fixture.store, &fixture.admitted, no_report).await,
            Err(OriginalWriterError::Conflict)));
    }
}

#[tokio::test]
async fn report_pending_source_refuses_cancellation_even_with_both_real_children() {
    let fixture = Fixture::build(true).await;
    let (binding, _) = issued(&fixture).await;
    let pending = fixture.row().await.unwrap();
    assert_eq!(pending.revision, 1);
    fixture.owner.originals.lock().unwrap().iter_mut().filter_map(OriginalRecord::bound_mut)
        .find(|held| Arc::ptr_eq(&held.retained, &fixture.admitted.contributor)).unwrap()
        .operation = OriginalOperationDisposition::CancellationPending;
    assert!(matches!(fixture.owner.bind_admitted_helper(&fixture.store, &fixture.admitted, binding).await,
        Err(OriginalWriterError::Unavailable)));
    assert_eq!(fixture.row().await.unwrap(), pending);
    fixture.assert_no_source();
}

#[tokio::test]
async fn configured_report_refuses_an_unstructured_operation_basis() {
    let fixture = Fixture::build(true).await;
    let capability = fixture.admitted.selection().helper.report_capability.as_ref().unwrap();
    let existing = WorkAuthorityBasis::GrantLineage { lineage: capability.parent.clone() };
    assert!(capability.validate(&existing).is_err());
    assert!(fixture.row().await.is_none());
    fixture.assert_no_source();
}

fn report_target() -> meerkat_authorization_contracts::work_association::GrantTarget {
    meerkat_authorization_contracts::work_association::GrantTarget::RegisteredToolBundleOperation {
        bundle: meerkat_toolkit::setup::SETUP_RESULTS_BUNDLE.into(),
        operation: meerkat_toolkit::setup::SETUP_REPORT.into(),
    }
}

#[tokio::test]
async fn report_target_preserves_independent_operations_through_canonical_binding() {
    use meerkat_authorization_contracts::work_association::GrantTarget;
    let fixture = Fixture::with_operations().await;
    let before = fixture.admitted.selection().helper.operation_basis.clone();
    let mcp = GrantTarget::McpServer { server: "existing-server".into() };
    let (binding, report) = issued(&fixture).await;
    let mut report_lineage = report.parent_lineage.clone();
    report_lineage.push(fixture.owner.grants.issued_reference(&report.id).unwrap().unwrap());
    assert_eq!(binding.authority_basis.lineage_for(None), before.lineage_for(None));
    assert_eq!(binding.authority_basis.lineage_for(Some(&mcp)), before.lineage_for(Some(&mcp)));
    assert_eq!(binding.authority_basis.lineage_for(Some(&report_target())), Some(report_lineage.as_slice()));
    let sibling = GrantTarget::RegisteredToolBundleOperation {
        bundle: meerkat_toolkit::setup::SETUP_RESULTS_BUNDLE.into(), operation: "unselected-operation".into(),
    };
    assert!(binding.authority_basis.lineage_for(Some(&sibling)).is_none());
    let source = fixture.owner.bind_admitted_helper(&fixture.store, &fixture.admitted, binding.clone()).await.unwrap();
    assert!(source.policy_source().is_ok());
    assert!(source.original.association().candidate().authority_basis == binding.authority_basis);
    let again = fixture.owner.bind_admitted_helper(&fixture.store, &fixture.admitted, binding.clone()).await.unwrap();
    assert_eq!(again.source_record, source.source_record);
    assert!(Arc::ptr_eq(&again.original, &source.original));
    // Moving the report child back into the catalog base cannot replay this
    // source, even when its exact grant and every unrelated target are kept.
    let WorkAuthorityBasis::TargetedGrantLineages { targets, .. } = &binding.authority_basis else { unreachable!() };
    let targets = targets.iter().filter(|selected| selected.target != report_target()).cloned().collect();
    let mut misplaced = binding;
    misplaced.authority_basis = WorkAuthorityBasis::TargetedGrantLineages {
        base: Some(report_lineage),
        targets,
    };
    assert!(matches!(fixture.owner.bind_admitted_helper(&fixture.store, &fixture.admitted, misplaced).await,
        Err(OriginalWriterError::Conflict)));
}

#[tokio::test]
async fn report_target_refuses_occupied_or_exhausted_selection_before_staging() {
    use meerkat_authorization_contracts::work_association::{GrantTarget, TargetGrantLineage, MAX_ASSOCIATION_REFERENCES};
    let fixture = Fixture::build(true).await;
    let capability = fixture.admitted.selection().helper.report_capability.as_ref().unwrap();
    let occupied = WorkAuthorityBasis::TargetedGrantLineages { base: None, targets: vec![
        TargetGrantLineage { target: report_target(), lineage: capability.parent.clone() },
    ] };
    assert!(capability.validate(&occupied).is_err());
    let full = WorkAuthorityBasis::TargetedGrantLineages { base: None, targets: (0..MAX_ASSOCIATION_REFERENCES)
        .map(|index| TargetGrantLineage { target: GrantTarget::McpServer { server: format!("server-{index:02}") },
            lineage: capability.parent.clone() }).collect() };
    assert!(capability.validate(&full).is_err());
    let malformed = WorkAuthorityBasis::TargetedGrantLineages { base: None, targets: vec![
        TargetGrantLineage { target: GrantTarget::McpServer { server: "".into() }, lineage: capability.parent.clone() },
    ] };
    assert!(capability.validate(&malformed).is_err());
    assert!(fixture.row().await.is_none());
    fixture.assert_no_source();
}

#[tokio::test]
async fn partial_helper_issuance_never_publishes_and_recovers_each_exact_child() {
    for report_first in [false, true] {
        let fixture = Fixture::with_operations().await;
        let (controller, report) = fixture.owner.retain_helper_grant_intents(
            &fixture.store, &fixture.admitted, fixture.candidate()).await.unwrap();
        let report = report.unwrap();
        let saved = fixture.row().await.unwrap();
        let (first, remaining) = if report_first { (&report, &controller) } else { (&controller, &report) };
        // The real owner commits one child; discard that response before the
        // remaining issuance. This does not simulate a torn native append.
        fixture.owner.grants.issue_child(&first.issuer, first.parent_lineage.last().unwrap(),
            first.id.clone(), first.grantee.clone(), first.requested.clone()).unwrap();
        let first_leaf = fixture.owner.grants.issued_reference(&first.id).unwrap().unwrap();
        assert!(fixture.owner.grants.issued_reference(&remaining.id).unwrap().is_none());
        let mut partial = fixture.binding.clone();
        let mut lineage = first.parent_lineage.clone();
        lineage.push(first_leaf.clone());
        if report_first {
            partial.authority_basis = fixture.admitted.selection().helper.report_capability.as_ref().unwrap()
                .project(&fixture.admitted.selection().helper.operation_basis, lineage).unwrap();
        } else {
            partial.controller_grant_lineage = lineage;
            partial.controller_ceiling = fixture.owner.grants.resolve_controller_lineage(
                &partial.controller_grant_lineage, &first.grantee, first.represented_subject.as_ref())
                .unwrap().restrictions().clone();
        }
        assert!(fixture.owner.bind_admitted_helper(&fixture.store, &fixture.admitted, partial).await.is_err());
        assert_eq!(fixture.row().await.unwrap(), saved);
        fixture.assert_no_source();
        let (binding, recovered_report) = issued(&fixture).await;
        assert_eq!(recovered_report, report);
        assert_eq!(fixture.owner.grants.issued_reference(&first.id).unwrap(), Some(first_leaf));
        let remaining_leaf = fixture.owner.grants.issued_reference(&remaining.id).unwrap().unwrap();
        let source = fixture.owner.bind_admitted_helper(&fixture.store, &fixture.admitted, binding.clone()).await.unwrap();
        let replay = fixture.owner.bind_admitted_helper(&fixture.store, &fixture.admitted, binding).await.unwrap();
        assert_eq!(source.source_record, replay.source_record);
        assert_eq!(fixture.owner.grants.issued_reference(&remaining.id).unwrap(), Some(remaining_leaf));
    }
}

#[tokio::test]
async fn report_revocation_is_target_local_and_does_not_erase_retained_binding() {
    let fixture = Fixture::with_operations().await;
    let candidate = fixture.candidate();
    let (binding, report) = issued(&fixture).await;
    let source = fixture.owner.bind_admitted_helper(&fixture.store, &fixture.admitted, binding.clone()).await.unwrap();
    let saved = source.source_record.clone();
    let report_leaf = fixture.owner.grants.issued_reference(&report.id).unwrap().unwrap();
    let native = crate::original_ingress::tests::machine(&fixture.owner);
    let mut custody = native.try_controller_grant_mutation().unwrap();
    fixture.owner.grants.revoke(&report.issuer, &report_leaf, &mut custody).unwrap();
    drop(custody);
    assert!(candidate.report.as_ref().unwrap().check_current(
        &fixture.owner, &fixture.admitted.facts, &binding.authority_basis).is_err());
    // These are separate native lineages. The closed-policy native fixture
    // proves no dispatch; it establishes exact target-local currentness only.
    let helper = &fixture.admitted.selection().helper;
    for lineage in helper.operation_basis.grant_lineages()
        .chain(std::iter::once(binding.controller_grant_lineage.as_slice()))
    {
        fixture.owner.grants.resolve_controller_lineage(lineage, &helper.logical_executor,
            helper.represented_subject.as_ref()).unwrap().check_current().unwrap();
    }
    candidate.check_record_binding(&fixture.admitted.facts, &binding).unwrap();
    assert!(source.policy_source().is_ok());
    assert_eq!(fixture.row().await.unwrap(), *saved);
    assert_eq!(fixture.owner.grants.issued_reference(&report.id).unwrap(), Some(report_leaf));
}

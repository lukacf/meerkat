//! Canonical helper staging with independent real native operation children.
//! These controls do not claim a native dispatch or protected human interaction.
use super::*;

fn issue(f: &Fixture, intent: &ChildGrantIntent) -> Vec<meerkat_authorization_contracts::grant::GrantLineageRef> {
    let leaf = f.owner.grants.issued_reference(&intent.id).unwrap().unwrap_or_else(||
        f.owner.grants.issue_child(&intent.issuer, intent.parent_lineage.last().unwrap(),
            intent.id.clone(), intent.grantee.clone(), intent.requested.clone()).unwrap());
    let mut lineage = intent.parent_lineage.clone(); lineage.push(leaf); lineage
}

#[tokio::test]
async fn account_and_report_partial_issuance_keep_one_pending_row_until_all_children_reconcile() {
    for first_account in [false, true] {
        let f = Fixture::with_account().await;
        let candidate = f.candidate();
        let (controller, report, account, public, _catalog) = f.owner.retain_helper_operation_intents(
            &f.store, &f.admitted, candidate.clone()).await.unwrap();
        let report = report.unwrap(); let account = account.unwrap(); assert!(public.is_none());
        assert_ne!(report.id, account.id);
        assert_ne!(report.parent_lineage, account.parent_lineage);
        assert_ne!(report.requested.actions, account.requested.actions);
        assert_ne!(report.requested.resource_domains, account.requested.resource_domains);
        let pending = f.row().await.unwrap();
        let first = if first_account { &account } else { &report };
        let first_lineage = issue(&f, first);
        assert!(f.owner.grants.issued_reference(&controller.id).unwrap().is_none());
        f.assert_no_source();
        assert!(f.owner.observe_helper_binding(&f.store, &f.admitted).await.unwrap().is_none());
        let mut binding = stage_fixture_controller(&f.owner, &f.store, &f.admitted, f.binding.clone()).await;
        // Missing either operation is not a complete configured binding.
        assert!(f.owner.bind_admitted_helper(&f.store, &f.admitted, binding.clone()).await.is_err());
        assert_eq!(f.row().await.unwrap(), pending);
        let again = f.owner.retain_helper_operation_intents(&f.store, &f.admitted, candidate).await.unwrap();
        assert_eq!(again, (controller, Some(report.clone()), Some(account.clone()), None, None));
        let report_lineage = issue(&f, &report); let account_lineage = issue(&f, &account);
        assert_eq!(issue(&f, first), first_lineage);
        let cap = f.admitted.selection().helper.report_capability.as_ref().unwrap();
        binding.authority_basis = cap.project_with_account(&f.admitted.selection().helper.operation_basis,
            report_lineage, Some(account_lineage)).unwrap();
        f.store.fault.store(4, Ordering::SeqCst);
        assert!(matches!(f.owner.bind_admitted_helper(&f.store, &f.admitted, binding.clone()).await,
            Err(OriginalWriterError::Unavailable)));
        let recovered = f.owner.bind_admitted_helper(&f.store, &f.admitted, binding.clone()).await.unwrap();
        assert!(recovered.policy_source().is_ok());
        assert_eq!(recovered.original.association().candidate().authority_basis, binding.authority_basis);
    }
}

#[tokio::test]
async fn account_selection_refuses_report_parent_and_retains_narrow_target_revocation() {
    let f = Fixture::with_account().await;
    let (controller, report, account, public, _catalog) = f.owner.retain_helper_operation_intents(
        &f.store, &f.admitted, f.candidate()).await.unwrap();
    let report = report.unwrap(); let account = account.unwrap(); assert!(public.is_none());
    let report_lineage = issue(&f, &report); let account_lineage = issue(&f, &account);
    let cap = f.admitted.selection().helper.report_capability.as_ref().unwrap();
    assert!(cap.project_with_account(&f.admitted.selection().helper.operation_basis,
        report_lineage.clone(), Some(report_lineage.clone())).is_err());
    let mut binding = stage_fixture_controller(&f.owner, &f.store, &f.admitted, f.binding.clone()).await;
    binding.authority_basis = cap.project_with_account(&f.admitted.selection().helper.operation_basis,
        report_lineage.clone(), Some(account_lineage.clone())).unwrap();
    let source = f.owner.bind_admitted_helper(&f.store, &f.admitted, binding.clone()).await.unwrap();
    // Revocation is evaluated for the selected operation, not by decoding the
    // historical helper or by sweeping unrelated targets on reconstruction.
    let native = crate::original_ingress::tests::machine(&f.owner);
    let mut custody = native.try_controller_grant_mutation().unwrap();
    f.owner.grants.revoke(&account.issuer, account_lineage.last().unwrap(), &mut custody).unwrap();
    drop(custody);
    assert!(source.policy_source().is_ok());
    let saved = f.row().await.unwrap();
    let rebound = f.owner.bind_admitted_helper(&f.store, &f.admitted, binding.clone()).await.unwrap();
    assert_eq!(rebound.original.association(), source.original.association());
    assert_eq!(f.row().await.unwrap(), saved);
    let mut changed = binding.clone();
    changed.controller_ceiling = ExecutionRestrictions::unrestricted();
    assert_ne!(changed.controller_ceiling, binding.controller_ceiling);
    assert!(matches!(f.owner.bind_admitted_helper(&f.store, &f.admitted, changed).await,
        Err(OriginalWriterError::Conflict)));
    assert_eq!(f.row().await.unwrap(), saved);
    assert!(f.candidate().account.as_ref().unwrap().check_account_current(
        &f.owner, &f.admitted.facts, &binding.authority_basis).is_err());
    let report_resolved = f.owner.grants.resolve_lineage(&report_lineage, &report.grantee,
        report.represented_subject.as_ref()).unwrap();
    assert!(report_resolved.check_current().is_ok());
    assert!(f.owner.grants.issued_reference(&controller.id).unwrap().is_some());
}

#[tokio::test]
async fn public_request_child_is_independent_and_partial_settlement_recovers_the_same_row() {
    for first_public in [false, true] {
        let f = Fixture::with_public(true).await;
        let candidate = f.candidate();
        let (controller, report, account, public, _catalog) = f.owner.retain_helper_operation_intents(
            &f.store, &f.admitted, candidate.clone()).await.unwrap();
        let report = report.unwrap(); let account = account.unwrap(); let public = public.unwrap();
        for other in [&report, &account] {
            assert_ne!(public.id, other.id);
            assert_ne!(public.parent_lineage, other.parent_lineage);
            assert_ne!(public.requested.actions, other.requested.actions);
            assert_ne!(public.requested.resource_domains, other.requested.resource_domains);
        }
        let pending = f.row().await.unwrap();
        let first = if first_public { &public } else { &account };
        let first_lineage = issue(&f, first);
        f.assert_no_source();
        let mut binding = stage_fixture_controller(&f.owner, &f.store, &f.admitted, f.binding.clone()).await;
        assert!(f.owner.bind_admitted_helper(&f.store, &f.admitted, binding.clone()).await.is_err());
        assert_eq!(f.row().await.unwrap(), pending);
        let mut changed = candidate.clone(); changed.public = None;
        assert!(f.owner.retain_helper_operation_intents(&f.store, &f.admitted, changed).await.is_err());
        assert_eq!(f.row().await.unwrap(), pending);
        assert_eq!(f.owner.retain_helper_operation_intents(&f.store, &f.admitted, candidate).await.unwrap(),
            (controller, Some(report.clone()), Some(account.clone()), Some(public.clone()), None));
        let report_lineage = issue(&f, &report); let account_lineage = issue(&f, &account);
        let public_lineage = issue(&f, &public);
        assert_eq!(issue(&f, first), first_lineage);
        let helper = &f.admitted.selection().helper;
        let cap = helper.report_capability.as_ref().unwrap();
        // Each exact target requires its independently issued child, including
        // when all of the siblings have already settled in the native owner.
        assert!(cap.project_with_operations(&helper.operation_basis, report_lineage.clone(),
            Some(account_lineage.clone()), None).is_err());
        assert!(cap.project_with_operations(&helper.operation_basis, report_lineage.clone(),
            Some(account_lineage.clone()), Some(account_lineage.clone())).is_err());
        assert!(cap.project_with_operations(&helper.operation_basis, report_lineage.clone(),
            Some(public_lineage.clone()), Some(public_lineage.clone())).is_err());
        binding.authority_basis = cap.project_with_operations(&helper.operation_basis,
            report_lineage.clone(), Some(account_lineage.clone()), Some(public_lineage.clone())).unwrap();
        f.store.fault.store(4, Ordering::SeqCst);
        assert!(matches!(f.owner.bind_admitted_helper(&f.store, &f.admitted, binding.clone()).await,
            Err(OriginalWriterError::Unavailable)));
        let source = f.owner.bind_admitted_helper(&f.store, &f.admitted, binding.clone()).await.unwrap();
        let bound = f.row().await.unwrap();
        assert_eq!(source.original.association().candidate().authority_basis, binding.authority_basis);
        // A revoked public request leaves its sibling account/report grants
        // current. Historical source reconstruction never fabricates a reissue.
        let native = crate::original_ingress::tests::machine(&f.owner);
        let mut custody = native.try_controller_grant_mutation().unwrap();
        f.owner.grants.revoke(&public.issuer, public_lineage.last().unwrap(), &mut custody).unwrap();
        drop(custody);
        assert!(source.policy_source().is_ok());
        assert!(f.candidate().public.as_ref().unwrap().check_public_current(
            &f.owner, &f.admitted.facts, &binding.authority_basis).is_err());
        for (intent, lineage) in [(&report, &report_lineage), (&account, &account_lineage)] {
            f.owner.grants.resolve_lineage(lineage, &intent.grantee,
                intent.represented_subject.as_ref()).unwrap().check_current().unwrap();
        }
        let rebound = f.owner.bind_admitted_helper(&f.store, &f.admitted, binding).await.unwrap();
        assert_eq!(rebound.original.association(), source.original.association());
        assert_eq!(f.row().await.unwrap(), bound);
    }
}

#[tokio::test]
async fn public_only_selection_never_infers_account_and_legacy_rows_omit_public() {
    let legacy = Fixture::with_account().await;
    let candidate = legacy.candidate();
    assert!(candidate.public.is_none());
    let bytes = canonical_json(&candidate).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(value.get("public").is_none());
    let restored: HelperControllerIntent = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(canonical_json(&restored).unwrap(), bytes);
    let capability = legacy.admitted.selection().helper.report_capability.as_ref().unwrap();
    let value = serde_json::to_value(capability).unwrap();
    assert!(value.get("public_parent").is_none());
    let restored: crate::installed_host::acquisition::ConfiguredHelperReportCapability =
        serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(&restored).unwrap(), value);

    let f = Fixture::with_public(false).await;
    let (_, report, account, public, _catalog) = f.owner.retain_helper_operation_intents(
        &f.store, &f.admitted, f.candidate()).await.unwrap();
    assert!(account.is_none());
    let helper = &f.admitted.selection().helper;
    let cap = helper.report_capability.as_ref().unwrap();
    assert!(cap.account_parent.is_none());
    let report = issue(&f, &report.unwrap()); let public = issue(&f, &public.unwrap());
    let basis = cap.project_with_operations(&helper.operation_basis, report, None, Some(public)).unwrap();
    assert!(crate::installed_host::acquisition::ConfiguredHelperReportCapability::selected_account_lineage(&basis).is_err());
    assert!(crate::installed_host::acquisition::ConfiguredHelperReportCapability::selected_public_lineage(&basis).is_ok());
    let mut wrong = cap.clone(); wrong.account_parent = wrong.public_parent.clone();
    assert!(wrong.validate(&helper.operation_basis).is_err());
}

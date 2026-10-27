//! Actual native owners and canonical SQLite decisions. Model transport and
//! approval click are controlled; this does not execute helper account OAuth.
use super::*;
use crate::private_originals::{ContinuationFixture, LocalControllerSetup, PrivateOriginalSubmit, PrivateOriginalSubmission};
use crate::installed_host::acquisition::{ConfiguredHelperReportCapability, InstalledSetupSelection};
use meerkat_authorization_contracts::work_association::WorkAuthorityBasis;
use futures::FutureExt;
use std::{panic::AssertUnwindSafe, sync::atomic::Ordering};

fn helper(owner: &OperatorSetup) -> InstalledSetupSelection {
    let route = &owner.native.routes[0];
    let (binding, _, actor) = route.source_binding(owner.lifetime.selected_policy()).unwrap();
    let member = owner.native.members.iter().find(|member| &member.member == binding).unwrap();
    let mut helper = InstalledSetupSelection { member: member.member.clone(), ingress_actor: actor.clone(),
        logical_executor: member.logical_owner.clone(), represented_subject: route.represented_subject.clone(),
        controller_parent: route.controller_parent.clone().unwrap(),
        catalog_capability: None,
        operation_basis: WorkAuthorityBasis::TargetedGrantLineages { base: None, targets: vec![] }, report_capability: None };
    helper.report_capability = Some(configured_report::issue_or_recover(&owner.source, &owner.native.grant_administrator,
        &owner.operator, &helper, &HelperReportCapabilityRequest { member: helper.member.clone(),
            issuance_id: EvidenceId::new("paired-helper-report").unwrap(),
            account_issuance_id: Some(EvidenceId::new("paired-helper-account").unwrap()), public_issuance_id: Some(EvidenceId::new("paired-helper-public").unwrap()) },
        configured_report::report_parent(&owner.native.grant_administrator, &helper.logical_executor, &member.audience), &member.audience).unwrap());
    helper
}

async fn prepared(fixture: &ContinuationFixture, label: &str) -> (LocalControllerSetup, PrivateOriginalSubmit) {
    let content = "Find the connector needed for this original task".to_owned();
    let mut event = crate::original_ingress::tests::private_event(label);
    event.route = Id::new("console").unwrap(); event.content_sha256 = digest(content.as_bytes());
    let authentication = event.original_authentication.clone();
    let original = fixture.source.retain_event(event).await.unwrap();
    let submission = PrivateOriginalSubmit { original, content, transport_authentication: authentication };
    let prepared = fixture.source.prepare_local_controller_setup(submission.clone()).await.unwrap();
    (prepared, submission)
}
fn setup_request(prepared: &LocalControllerSetup) -> ApprovalRequest {
    let mut request = request();
    request.owner = ApprovalOwnerRef::Session { session_id: prepared.session_id().clone() }; request
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_correlated_targets_use_one_review_source_and_real_first_input() {
    let fixture = Box::pin(ContinuationFixture::for_service_input()).await;
    let result = AssertUnwindSafe(Box::pin(async {
        let owner = fixture.source.local_operator_setup(fixture.lifetime().clone()).unwrap();
        let selected_helper = helper(&owner);
        let owner = owner.with_helper_selection(Some(selected_helper)).unwrap();
        let (prepared, submission) = prepared(&fixture, "paired-explicit-source").await;
        let original = prepared.original().clone();
        let request = setup_request(&prepared);
        let outcome = owner.review_original_setup_request(prepared, request, |request| async move {
            let approvals = ApprovalService::new(); let pending = approvals.request(request).unwrap();
            Ok(approvals.decide(&pending.approval_id, ApprovalDecision::Approve,
                ApprovalPrincipalId::new("paired-review").unwrap(), None, None).unwrap())
        }).await.unwrap();
        let OperatorParentOutcome::SetupRequestSelected { selection } = outcome else { panic!("paired selection missing") };
        assert_eq!(selection.helper_targets.len(), 3);
        assert_eq!(selection.helper_targets[0].target, Some(ConfiguredHelperReportCapability::public_target()));
        assert_eq!(selection.helper_targets[1].target, Some(ConfiguredHelperReportCapability::account_target()));
        assert_eq!(selection.helper_targets[2].target, Some(ConfiguredHelperReportCapability::target()));
        for (index, target) in selection.helper_targets.iter().enumerate() {
            assert!(selection.helper_targets[..index].iter().all(|other| other.parent != target.parent));
        }
        assert!(selection.helper_targets.iter().all(|target| !selection.parent.contains(&target.parent)));
        let before = owner.retained_setup_decision(&original).await.unwrap().unwrap();
        let reviewed_origin = decode(&before, &before.key).unwrap().setup_request.unwrap().origin.unwrap();
        assert!(reviewed_origin.original_authority == owner.route(&original.route).unwrap().source_authority);
        let first = fixture.source.with_synthetic_admission_diagnostics(async {
            let first = fixture.source.submit_authenticated_original(submission.clone()).await.unwrap();
            let state = match &first {
                PrivateOriginalSubmission::Admitted { turn_ticket: Some(_), .. } => "AdmittedWithTicket",
                PrivateOriginalSubmission::Admitted { turn_ticket: None, .. } => "AdmittedWithoutTicket",
                PrivateOriginalSubmission::AdmissionUnresolved => "AdmissionUnresolved",
                PrivateOriginalSubmission::NativeOutcomeUnavailable => "NativeOutcomeUnavailable",
            };
            use std::io::Write;
            let _ = writeln!(std::io::stderr(), "synthetic-admission target.first_submission.outcome: {state}");
            first
        }).await;
        assert!(matches!(first, PrivateOriginalSubmission::Admitted { turn_ticket: Some(_), .. }));
        fixture.finish(&Ok(first)).await.unwrap();
        let binding = fixture.source.binding_evidence(&original).await.unwrap().unwrap();
        assert!(reviewed_origin.original_authority == binding.candidate().original_work.authority);
        assert!(reviewed_origin.source_owner == binding.candidate().ingress_namespace.ingress);
        assert!(reviewed_origin.original == original);
        let WorkAuthorityBasis::TargetedGrantLineages { base: None, targets } = &binding.candidate().authority_basis else { panic!("target basis missing") };
        assert_eq!(targets.len(), 4);
        for (index, target) in targets.iter().enumerate() {
            assert!(targets[..index].iter().all(|other| other.lineage.last() != target.lineage.last()));
        }
        let replay = fixture.source.submit_authenticated_original(submission).await.unwrap();
        assert!(matches!(replay, PrivateOriginalSubmission::Admitted { deduplicated: true, .. }));
        let after = owner.retained_setup_decision(&original).await.unwrap().unwrap();
        assert_eq!(before.content_digest, after.content_digest);
        assert_eq!(fixture.source.binding_evidence(&original).await.unwrap(), Some(binding.clone()));
        let source_key = StoreKey { realm: fixture.scope.realm.clone(), owner: fixture.scope.owner.clone(),
            id: crate::private_originals::source_key_id(&owner.native.realm, owner.source.ingress_domain(),
                &binding.candidate().original_work.authority, &original.original).unwrap() };
        let source_before = fixture.store.get(&source_key).await.unwrap().unwrap();
        {
            let mut custody = fixture.machine.try_controller_grant_mutation().unwrap();
            for parent in [&selection.parent[0], &selection.helper_targets[0].parent] {
                owner.source.grants().revoke(&parent.root_authority, parent, &mut custody).unwrap();
            }
        }
        // The exact same canonical decision is still required. Revoking FI
        // and public does not revoke the independent account/report selections.
        assert!(validate_setup_selection(&owner.lifetime, &owner.source, owner.originals.as_ref(), &selection).await.is_err());
        validate_retained_setup_selection(&owner.lifetime, &owner.source, owner.originals.as_ref(), &selection).await.unwrap();
        let mut changed = selection.clone(); changed.helper_targets.clear();
        assert!(validate_retained_setup_selection(&owner.lifetime, &owner.source, owner.originals.as_ref(), &changed).await.is_err());
        let claims = binding.candidate();
        let custody = meerkat_runtime::meerkat_machine::AuthenticatedOriginal {
            original_work: claims.original_work.clone(), occurrence_scope: claims.ingress_namespace.occurrence_scope.clone(),
            original_authentication: claims.original_authentication.clone(), root_event: claims.root_event.clone(),
            contributing_work: claims.contributing_work.clone(), target: claims.target.clone(), authority_basis: claims.authority_basis.clone(),
        };
        // Exercise the actual rebind callback, under the actual mutation owner.
        // This reconstructs source evidence only, never cold native admission.
        owner.source.bind_issued_access_async(binding.clone(), custody, &claims.requester, &claims.ingress_actor,
            Some(&owner.native.grant_administrator), |association| {
                let source = &fixture.source; let original = &original;
                async move { source.persist_or_observe_binding(original, &association, true).await }
            }).await.unwrap();
        assert_eq!(fixture.store.get(&source_key).await.unwrap().unwrap(), source_before);
        let report_target = ConfiguredHelperReportCapability::target();
        let report_path = claims.authority_basis.lineage_for(Some(&report_target)).unwrap();
        owner.source.grants().resolve_lineage(report_path, &claims.logical_executor,
            claims.represented_subject.as_ref()).unwrap().check_current().unwrap();
        let account_target = ConfiguredHelperReportCapability::account_target();
        let account_path = claims.authority_basis.lineage_for(Some(&account_target)).unwrap();
        owner.source.grants().resolve_lineage(account_path, &claims.logical_executor,
            claims.represented_subject.as_ref()).unwrap().check_current().unwrap();
        assert_eq!(fixture.model_requests(), 1, "reconstruction must not submit another input");
    })).catch_unwind().await;
    let closed = fixture.shutdown().await; assert!(closed.is_ok(), "{closed:?}");
    if let Err(payload) = result { std::panic::resume_unwind(payload); }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn paired_target_unknown_entry_never_reissues_and_lost_issued_cas_recovers_exactly() {
    for fault in [1, 2] {
        let fixture = Box::pin(ContinuationFixture::for_service_input()).await;
        let result = AssertUnwindSafe(Box::pin(async {
            let mut owner = fixture.source.local_operator_setup(fixture.lifetime().clone()).unwrap();
            let selected_helper = helper(&owner);
            owner = owner.with_helper_selection(Some(selected_helper)).unwrap();
            let store = owner_tests::StoreProbe::new(owner.originals.clone(), false);
            store.target_fault.store(fault, Ordering::SeqCst); owner.originals = store;
            let (prepared, _) = prepared(&fixture, "paired-unknown-source").await;
            let proposal = owner.prepare_original_setup_parent_review(&prepared, setup_request(&prepared)).unwrap();
            let approval = reviewed(&proposal, ApprovalDecision::Approve);
            assert!(matches!(owner.apply_parent_decision(proposal.clone(), approval).await.unwrap(),
                OperatorParentOutcome::PendingReconciliation));
            let saved = owner.retained_setup_decision(prepared.original()).await.unwrap().unwrap();
            let record = decode(&saved, &saved.key).unwrap();
            let first = setup_targets::target_id(&saved.key, &record.setup_request.as_ref().unwrap().targets[0].target).unwrap();
            let second = setup_targets::target_id(&saved.key, &record.setup_request.as_ref().unwrap().targets[1].target).unwrap();
            let third = setup_targets::target_id(&saved.key, &record.setup_request.as_ref().unwrap().targets[2].target).unwrap();
            let retained = owner.source.grants().issued_reference(&first).unwrap();
            assert_eq!(retained.is_some(), fault == 2);
            assert!(owner.source.grants().issued_reference(&second).unwrap().is_none());
            assert!(owner.source.grants().issued_reference(&third).unwrap().is_none());
            // A changed source authority cannot reinterpret this already
            // reviewed original or replace its partially entered target intents.
            let mut changed = proposal.clone();
            changed.setup_request.as_mut().unwrap().origin.as_mut().unwrap().original_authority = crate::installed_host::tests::principal("other-original-source");
            let changed_approval = reviewed(&changed, ApprovalDecision::Approve);
            assert!(owner.apply_parent_decision(changed, changed_approval).await.is_err());
            assert_eq!(owner.retained_setup_decision(prepared.original()).await.unwrap().unwrap(), saved);
            assert_eq!(owner.source.grants().issued_reference(&first).unwrap(), retained);
            // A new target intent under the same approved original is refused,
            // including when no child was known to complete.
            let mut changed = proposal.clone(); changed.setup_request.as_mut().unwrap().targets.pop();
            let changed_approval = reviewed(&changed, ApprovalDecision::Approve);
            assert!(owner.apply_parent_decision(changed, changed_approval).await.is_err());
            let outcome = owner.observe_setup_decision(saved, record).await.unwrap();
            if fault == 1 {
                assert!(matches!(outcome, OperatorParentOutcome::PendingReconciliation));
                assert!(owner.source.grants().issued_reference(&first).unwrap().is_none());
                assert!(owner.source.grants().issued_reference(&second).unwrap().is_none());
                assert!(owner.source.grants().issued_reference(&third).unwrap().is_none());
            } else {
                assert!(matches!(outcome, OperatorParentOutcome::Selected { .. }));
                assert_eq!(owner.source.grants().issued_reference(&first).unwrap(), retained);
                assert!(owner.source.grants().issued_reference(&second).unwrap().is_some());
                assert!(owner.source.grants().issued_reference(&third).unwrap().is_some());
            }
            assert_eq!(fixture.model_requests(), 0);
            assert!(fixture.source.binding_evidence(prepared.original()).await.unwrap().is_none());
        })).catch_unwind().await;
        let closed = fixture.shutdown().await; assert!(closed.is_ok(), "{closed:?}");
        if let Err(payload) = result { std::panic::resume_unwind(payload); }
    }
}

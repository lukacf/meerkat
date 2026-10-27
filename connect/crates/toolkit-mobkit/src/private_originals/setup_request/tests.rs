//! Source-only authored controls. Real installed/native preparation, operator
//! decisions and first binding; the model transport and protected human click
//! are fixture-owned. This is not the ordinary protected HTTP Console journey.
use super::*;
use crate::installed_host::operator_setup::{
    OperatorParentOutcome, OperatorParentProposal, OperatorSetup,
};
use crate::private_originals::ContinuationFixture;
use futures::FutureExt;
use meerkat_core::{
    ApprovalActionKind, ApprovalDecision, ApprovalOwnerRef, ApprovalPrincipalId,
    ApprovalProposedAction, ApprovalRequest, ApprovalResourceId, ApprovalResourceKind,
    ApprovalResourceRef, ApprovalRisk, ApprovalService,
};
use std::panic::AssertUnwindSafe;

pub(super) async fn prepared(
    fixture: &ContinuationFixture,
    label: &str,
) -> (LocalControllerSetup, PrivateOriginalSubmit) {
    let content = "Find the connector needed for this original task".to_owned();
    let mut event = crate::original_ingress::tests::private_event(label);
    event.route = toolkit_package::Id::new("console").unwrap();
    event.content_sha256 = digest(content.as_bytes());
    let authentication = event.original_authentication.clone();
    let original = fixture.source.retain_event(event).await.unwrap();
    let submission = PrivateOriginalSubmit {
        original,
        content,
        transport_authentication: authentication,
    };
    let prepared = fixture
        .source
        .prepare_local_controller_setup(submission.clone())
        .await
        .unwrap();
    assert_eq!(prepared.session_id(), &fixture.session);
    (prepared, submission)
}
pub(in crate::private_originals) fn request(prepared: &LocalControllerSetup) -> ApprovalRequest {
    ApprovalRequest {
        requester: ApprovalPrincipalId::new("setup-source-control").unwrap(),
        owner: ApprovalOwnerRef::Session {
            session_id: prepared.session_id().clone(),
        },
        resource: ApprovalResourceRef {
            kind: ApprovalResourceKind::Runtime,
            id: ApprovalResourceId::new("original-setup-request"),
        },
        proposed_action: ApprovalProposedAction {
            kind: ApprovalActionKind::Other,
            summary: "The owner supplies the exact setup request scope".into(),
            body: None,
        },
        risk: ApprovalRisk::High,
        request_body: None,
        allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny]
            .into_iter()
            .collect(),
        expires_at: None,
        metadata: Default::default(),
        request_provenance: None,
    }
}
async fn issue(owner: &OperatorSetup, proposal: OperatorParentProposal) -> Vec<GrantLineageRef> {
    let approvals = ApprovalService::new();
    let pending = approvals.request(proposal.request().clone()).unwrap();
    let decided = approvals
        .decide(
            &pending.approval_id,
            ApprovalDecision::Approve,
            ApprovalPrincipalId::new("protected-presenter-fixture").unwrap(),
            None,
            None,
        )
        .unwrap();
    match owner
        .apply_parent_decision(proposal, decided)
        .await
        .unwrap()
    {
        OperatorParentOutcome::Selected { lineage, .. } => lineage,
        _ => panic!("the actual operator owner did not retain an issued decision"),
    }
}
async fn reviewed(
    fixture: &ContinuationFixture,
    prepared: &LocalControllerSetup,
) -> Vec<GrantLineageRef> {
    let owner = fixture
        .source
        .local_operator_setup(fixture.lifetime().clone())
        .unwrap();
    let proposal = owner
        .prepare_original_setup_parent_review(prepared, request(prepared))
        .unwrap();
    assert_eq!(
        proposal.request().proposed_action.body.as_ref().unwrap()["purpose"],
        "setup_request"
    );
    issue(&owner, proposal).await
}
async fn submit(
    fixture: &ContinuationFixture,
    submission: PrivateOriginalSubmit,
) -> Result<PrivateOriginalSubmission, String> {
    fixture
        .source
        .submit_authenticated_original(submission)
        .await
        .map_err(|e| e.to_string())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reviewed_setup_selects_same_unbound_session_then_real_first_input_and_replay() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let (prepared, submission) = prepared(&fixture, "setup-selected").await;
        assert_eq!(fixture.inputs().await.unwrap(), 0);
        assert_eq!(fixture.model_requests(), 0);
        let parent = reviewed(&fixture, &prepared).await;
        let selected = fixture
            .source
            .select_setup_request(&prepared, parent.clone())
            .await
            .unwrap();
        assert_eq!(selected.session_id, fixture.session);
        let key = fixture.source.key(&submission.original).unwrap();
        let selected_row = fixture.store.get(&key).await.unwrap().unwrap();
        assert!(
            fixture
                .source
                .binding_evidence(&submission.original)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            fixture
                .source
                .select_setup_request(&prepared, parent)
                .await
                .is_ok()
        );
        assert_eq!(
            fixture.store.get(&key).await.unwrap().unwrap(),
            selected_row,
            "an exact unbound selection retry does not rewrite the row"
        );
        let admitted = submit(&fixture, submission.clone()).await;
        fixture.finish(&admitted).await.unwrap();
        let bound = fixture
            .source
            .binding_evidence(&submission.original)
            .await
            .unwrap()
            .unwrap();
        let WorkAuthorityBasis::GrantLineage { lineage } = &bound.candidate().authority_basis
        else {
            panic!("actual first binder did not consume reviewed setup selection")
        };
        assert_eq!(&lineage[..1], selected.parent.as_slice());
        assert_eq!(lineage.len(), 2);
        assert_eq!(
            bound.candidate().target.context.as_str(),
            fixture.session.to_string()
        );
        assert_ne!(
            lineage.last(),
            bound.candidate().controller_grant_lineage.last()
        );
        let current = fixture
            .source
            .owner
            .grants()
            .resolve_lineage(
                lineage,
                &bound.candidate().logical_executor,
                bound.candidate().represented_subject.as_ref(),
            )
            .unwrap();
        assert_eq!(
            current.restrictions().delegation_depth,
            DelegationDepth::remaining(0)
        );
        current.check_current().unwrap();
        let requests = fixture.model_requests();
        let replay = submit(&fixture, submission).await.unwrap();
        assert!(matches!(
            replay,
            PrivateOriginalSubmission::Admitted {
                deduplicated: true,
                ..
            }
        ));
        assert_eq!(fixture.inputs().await.unwrap(), 1);
        assert_eq!(fixture.model_requests(), requests);
        assert!(
            fixture
                .source
                .select_setup_request(&prepared, selected.parent)
                .await
                .is_err(),
            "a bound original cannot be assigned a different or fresh operation mandate"
        );
    }))
    .catch_unwind()
    .await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tuple_only_generic_and_other_original_reviews_cannot_select_setup_source() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let (prepared, submission) = prepared(&fixture, "setup-purpose").await;
        let owner = fixture
            .source
            .local_operator_setup(fixture.lifetime().clone())
            .unwrap();
        let tuple = owner
            .prepare_setup_parent_review(
                submission.original.route.clone(),
                prepared.session_id(),
                request(&prepared),
            )
            .unwrap();
        let tuple_parent = issue(&owner, tuple).await;
        assert!(
            fixture
                .source
                .select_setup_request(&prepared, tuple_parent.clone())
                .await
                .is_err()
        );
        let route = fixture.source.route(&submission.original.route).unwrap();
        let bounds = fixture
            .source
            .owner
            .grants()
            .resolve_lineage(
                &tuple_parent,
                &route.grant_caller,
                route.represented_subject.as_ref(),
            )
            .unwrap()
            .restrictions()
            .clone();
        let generic = owner
            .prepare_parent_review(
                submission.original.route.clone(),
                bounds,
                request(&prepared),
            )
            .unwrap();
        assert_eq!(
            generic.request().proposed_action.body.as_ref().unwrap()["purpose"],
            "operations"
        );
        let generic_parent = issue(&owner, generic).await;
        assert!(
            fixture
                .source
                .select_setup_request(&prepared, generic_parent)
                .await
                .is_err()
        );
        let exact_parent = reviewed(&fixture, &prepared).await;
        let (other, _) = self::prepared(&fixture, "setup-other-original").await;
        assert!(
            fixture
                .source
                .select_setup_request(&other, exact_parent.clone())
                .await
                .is_err()
        );
        fixture
            .source
            .select_setup_request(&prepared, exact_parent)
            .await
            .unwrap();
        let stored = fixture
            .store
            .get(&fixture.source.key(&submission.original).unwrap())
            .await
            .unwrap()
            .unwrap();
        let record = fixture
            .source
            .decode_record(&stored, &submission.original)
            .unwrap();
        assert!(
            fixture
                .source
                .setup_request_lineage(
                    &record,
                    &submission.original,
                    &meerkat_core::SessionId::new(),
                    None
                )
                .await
                .is_err()
        );
        assert_eq!(fixture.inputs().await.unwrap(), 0);
        assert_eq!(fixture.model_requests(), 0);
    }))
    .catch_unwind()
    .await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revoked_parent_or_cancelled_source_refuses_selected_first_input() {
    for revoke in [false, true] {
        let fixture = ContinuationFixture::for_service_input().await;
        let result = AssertUnwindSafe(Box::pin(async {
            let (prepared, submission) = prepared(&fixture, "setup-no-longer-current").await;
            let parent = reviewed(&fixture, &prepared).await;
            fixture
                .source
                .select_setup_request(&prepared, parent.clone())
                .await
                .unwrap();
            if revoke {
                let mut custody = fixture.machine.try_controller_grant_mutation().unwrap();
                fixture
                    .source
                    .owner
                    .grants()
                    .revoke(&parent[0].root_authority, &parent[0], &mut custody)
                    .unwrap();
            } else {
                fixture
                    .source
                    .cancel_original_operation(&submission.original)
                    .await
                    .unwrap();
            }
            // Native bind refusal is an unavailable outcome on this API,
            // not an outer error. The owner observations below establish
            // that neither cancellation nor revocation admitted work.
            assert!(matches!(
                submit(&fixture, submission.clone()).await,
                Ok(PrivateOriginalSubmission::NativeOutcomeUnavailable)
            ));
            assert!(
                fixture
                    .source
                    .binding_evidence(&submission.original)
                    .await
                    .unwrap()
                    .is_none()
            );
            assert_eq!(fixture.inputs().await.unwrap(), 0);
            assert_eq!(fixture.model_requests(), 0);
            let selected = PrivateSetupRequestSelection {
                helper_targets: vec![],
                original: submission.original,
                session_id: prepared.session_id().clone(),
                parent,
            };
            assert!(
                fixture
                    .source
                    .owner
                    .grants()
                    .issued_reference(&fixture.source.setup_child_id(&selected).unwrap())
                    .unwrap()
                    .is_none()
            );
        }))
        .catch_unwind()
        .await;
        let cleanup = fixture.shutdown().await;
        assert!(cleanup.is_ok(), "{cleanup:?}");
        if let Err(error) = result {
            std::panic::resume_unwind(error);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn controller_only_binding_wins_before_setup_selection_without_rebinding() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let (prepared, submission) = prepared(&fixture, "controller-first").await;
        let parent = reviewed(&fixture, &prepared).await;
        let admitted = submit(&fixture, submission.clone()).await;
        fixture.finish(&admitted).await.unwrap();
        let before = fixture.store.get(&fixture.source.key(&submission.original).unwrap()).await.unwrap().unwrap();
        let bound = fixture.source.binding_evidence(&submission.original).await.unwrap().unwrap();
        assert!(matches!(&bound.candidate().authority_basis,
            WorkAuthorityBasis::TargetedGrantLineages { base: None, targets } if targets.is_empty()));
        assert!(fixture.source.select_setup_request(&prepared, parent).await.is_err());
        assert_eq!(fixture.store.get(&before.key).await.unwrap().unwrap(), before);
        assert_eq!(fixture.inputs().await.unwrap(), 1);
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_source_selection_retries_same_reviewed_parent_without_reissue() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let (prepared, submission) = prepared(&fixture, "setup-cas-failure").await;
        let parent = reviewed(&fixture, &prepared).await;
        let key = fixture.source.key(&submission.original).unwrap();
        let before = fixture.store.get(&key).await.unwrap().unwrap();
        // Actual SQLite conditional write failure. It does not simulate a
        // lost response after commit or parent-decision issuance recovery.
        let sql = rusqlite::Connection::open(&fixture.source_path).unwrap();
        sql.execute_batch(
            "CREATE TRIGGER refuse_setup_choice BEFORE UPDATE OF data ON toolkit_records \
            WHEN json_extract(NEW.data, '$.body.setup_request') IS NOT NULL \
            BEGIN SELECT RAISE(FAIL, 'fixture setup choice write failure'); END;",
        )
        .unwrap();
        assert!(
            fixture
                .source
                .select_setup_request(&prepared, parent.clone())
                .await
                .is_err()
        );
        assert_eq!(fixture.store.get(&key).await.unwrap().unwrap(), before);
        sql.execute_batch("DROP TRIGGER refuse_setup_choice;")
            .unwrap();
        let selected = fixture
            .source
            .select_setup_request(&prepared, parent.clone())
            .await
            .unwrap();
        assert!(selected.parent == parent);
        assert_eq!(fixture.inputs().await.unwrap(), 0);
        assert_eq!(fixture.model_requests(), 0);
        assert!(
            fixture
                .source
                .owner
                .grants()
                .issued_reference(&fixture.source.setup_child_id(&selected).unwrap())
                .unwrap()
                .is_none()
        );
    }))
    .catch_unwind()
    .await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn setup_selection_wins_against_stale_controller_source_writer_and_revision() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        // Obtain a real controller-only association in the same native actor.
        // The changed candidate below is used only for a negative stale-writer
        // attempt. It is never supplied to native admission as authority.
        let original = fixture.submit().await;
        fixture.finish(&original).await.unwrap();
        let controller = fixture.binding().await.unwrap().unwrap();
        let (prepared, submission) = prepared(&fixture, "selected-before-stale-writer").await;
        let key = fixture.source.key(&submission.original).unwrap();
        let old = fixture.store.get(&key).await.unwrap().unwrap();
        let mut stale_record = fixture
            .source
            .decode_record(&old, &submission.original)
            .unwrap();
        let mut candidate = controller.candidate().clone();
        candidate.original_work.work = submission.original.original.clone();
        candidate.ingress_namespace.occurrence_scope = stale_record.event.occurrence.clone();
        candidate.original_authentication = super::super::governed_submit::evidence(
            &stale_record.source.authentication_domain,
            &stale_record.event.original_authentication,
        )
        .unwrap();
        candidate.root_event = super::super::governed_submit::evidence(
            &stale_record.source.event_domain,
            &stale_record.event.root_event,
        )
        .unwrap();
        candidate.contributing_work.clear();
        let stale = InputAuthorityAssociation::new(candidate).unwrap();
        let parent = reviewed(&fixture, &prepared).await;
        fixture
            .source
            .select_setup_request(&prepared, parent)
            .await
            .unwrap();
        let selected = fixture.store.get(&key).await.unwrap().unwrap();
        assert!(matches!(
            fixture
                .source
                .persist_or_observe_binding(&submission.original, &stale, true)
                .await,
            Err(OriginalWriterError::Conflict)
        ));
        stale_record.final_association = Some(stale);
        assert!(matches!(
            fixture
                .store
                .compare_and_swap(Transition {
                    key: key.clone(),
                    expected_revision: old.revision,
                    expected_digest: Some(old.content_digest),
                    claim: None,
                    data: encode(stale_record).unwrap(),
                    events: vec![],
                    effects: vec![],
                })
                .await,
            Err(toolkit_store::StoreError::Conflict)
        ));
        assert_eq!(fixture.store.get(&key).await.unwrap().unwrap(), selected);
        assert!(
            fixture
                .source
                .binding_evidence(&submission.original)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            fixture.inputs().await.unwrap(),
            1,
            "only the original control turn was admitted"
        );
    }))
    .catch_unwind()
    .await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pending_unbound_cancellation_blocks_selection_and_reconciles_same_source() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let (prepared, submission) = prepared(&fixture, "cancel-before-setup-choice").await;
        let parent = reviewed(&fixture, &prepared).await;
        let key = fixture.source.key(&submission.original).unwrap();
        let before = fixture.store.get(&key).await.unwrap().unwrap();
        let sql = rusqlite::Connection::open(&fixture.source_path).unwrap();
        sql.execute_batch(&format!(
            "CREATE TRIGGER refuse_setup_cancellation BEFORE UPDATE OF data ON toolkit_records \
             WHEN NEW.id = '{}' AND json_extract(NEW.data, '$.body.operation_cancelled') = 1 \
             BEGIN SELECT RAISE(ABORT, 'fixture cancellation persistence failure'); END;",
            key.id.replace('\'', "''")
        ))
        .unwrap();
        // The actual cancellation owner has published Pending while the
        // canonical row remains unchanged. Selection cannot change its origin.
        assert!(
            fixture
                .source
                .cancel_original_operation(&submission.original)
                .await
                .is_err()
        );
        assert_eq!(fixture.store.get(&key).await.unwrap().unwrap(), before);
        assert!(
            fixture
                .source
                .select_setup_request(&prepared, parent.clone())
                .await
                .is_err()
        );
        assert_eq!(fixture.store.get(&key).await.unwrap().unwrap(), before);
        sql.execute_batch("DROP TRIGGER refuse_setup_cancellation;")
            .unwrap();
        fixture
            .source
            .cancel_original_operation(&submission.original)
            .await
            .unwrap();
        let cancelled = fixture.store.get(&key).await.unwrap().unwrap();
        let record = fixture
            .source
            .decode_record(&cancelled, &submission.original)
            .unwrap();
        assert!(record.operation_cancelled);
        assert!(record.setup_request.is_none());
        assert!(
            fixture
                .source
                .select_setup_request(&prepared, parent)
                .await
                .is_err()
        );
        assert_eq!(fixture.store.get(&key).await.unwrap().unwrap(), cancelled);
        assert_eq!(fixture.inputs().await.unwrap(), 0);
        assert_eq!(fixture.model_requests(), 0);
    }))
    .catch_unwind()
    .await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}

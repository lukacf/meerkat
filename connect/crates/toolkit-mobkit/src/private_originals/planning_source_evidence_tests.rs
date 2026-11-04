//! Explicit source retention and native historical custody, without redispatch.
use super::*;
use crate::private_originals::planning_source_evidence::PlanningSourceEvidence;

fn forget<S>(fixture: &FirstFixture<S>, association: &InputAuthorityAssociation) {
    fixture
        .source
        .retained
        .lock()
        .unwrap()
        .retain(|held| &held.reference != fixture.original_reference());
    fixture
        .source
        .owner
        .forget_controller_custody_for_test(association);
    assert!(
        !fixture
            .source
            .owner
            .has_admitted_source(association)
            .unwrap()
    );
}

/// Wait for the canonical row of the admitted source to record its custody
/// attempt Promoted. Connect settles a fresh admission's custody after the
/// native admission returns: its custody handoff writes the row's decision in
/// the background under the source writer. The historical restoration below
/// starts from that settled row, once the writer that made it is released.
async fn custody_promoted<S>(fixture: &FirstFixture<S>) {
    let reference = fixture.original_reference();
    let key = fixture.source.key(reference).unwrap();
    tokio::time::timeout(WAIT, async {
        loop {
            let row = fixture.source.store.get(&key).await.unwrap().unwrap();
            let record = fixture.source.decode_record(&row, reference).unwrap();
            if record.custody_attempt.as_ref().is_some_and(|attempt| {
                attempt.decision()
                    == crate::original_ingress::custody_attempt::CustodyDecision::Promoted
            }) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the admitted source's custody settles Promoted");
    drop(fixture.source.owner.source_writer().await);
}

async fn close<S>(fixture: &FirstFixture<S>, body: Result<(), Box<dyn std::any::Any + Send>>) {
    let cleanup = std::panic::AssertUnwindSafe(async {
        let result = fixture.shutdown().await;
        assert!(result.is_ok(), "{result:?}");
    })
    .catch_unwind()
    .await;
    if let Err(panic) = body {
        std::panic::resume_unwind(panic);
    }
    if let Err(panic) = cleanup {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn selected_parent_restores_only_actual_controller_custody_without_replay() {
    let (fixture, history) = FirstFixture::for_live_attachment().await;
    let body = std::panic::AssertUnwindSafe(Box::pin(async {
        fixture
            .source
            .bind_controller_pin_source(fixture.service.clone())
            .unwrap();
        // The production custody worker settles each admitted source's
        // custody attempt; the historical restoration below reads its
        // Promoted decision. Start it before the first input, as the host does.
        fixture.source.start_custody_recovery(None).unwrap();
        let unselected = fixture.cold_source_record().await;
        fixture
            .source
            .prepare_workflow_parent_source(fixture.original_reference())
            .await
            .unwrap();
        let selected = fixture.cold_source_record().await;
        assert!(
            !same_source_origin(&unselected, &selected),
            "evidence purpose is part of exact source selection"
        );
        fixture
            .source
            .prepare_workflow_parent_source(fixture.original_reference())
            .await
            .unwrap();
        assert_eq!(
            fixture.cold_source_record().await,
            selected,
            "selection replay writes nothing"
        );
        assert!(fixture.binding().await.unwrap().is_none());
        assert_eq!(fixture.inputs().await, Ok(0));
        let first = fixture.submit().await;
        fixture.finish(&first).await.unwrap();
        custody_promoted(&fixture).await;
        let association = fixture.binding().await.unwrap().unwrap();
        let row = fixture.cold_source_record().await;
        let record = fixture
            .source
            .decode_record(&row, fixture.original_reference())
            .unwrap();
        assert!(record.planning_source_evidence == Some(PlanningSourceEvidence::Parent));
        let input = record
            .bound_service_input
            .as_ref()
            .unwrap()
            .decode()
            .unwrap();
        assert!(input.header().authority_association.as_ref() == Some(&association));
        let actual_id = match &first {
            Ok(PrivateOriginalSubmission::Admitted { input_id, .. }) => {
                InputId::from_uuid(input_id.parse().unwrap())
            }
            _ => panic!("actual native admission required"),
        };
        let runtime = meerkat_runtime::LogicalRuntimeId::for_session(&fixture.session);
        let before = tokio::time::timeout(WAIT, async {
            loop {
                let rows = history.load_input_states_strict(&runtime).await.unwrap();
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].state.input_id, actual_id);
                if rows[0].state.persisted_input.is_none() {
                    break rows;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("actual terminal input payload must retire before historical restoration");
        let requests = fixture.model_requests();
        forget(&fixture, &association);
        tokio::time::timeout(
            WAIT,
            fixture
                .source
                .restore_workflow_parent_source(fixture.original_reference()),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            fixture
                .source
                .owner
                .has_admitted_source(&association)
                .unwrap()
        );
        assert_eq!(
            fixture
                .source
                .owner
                .historical_task_custody_for_test(&association),
            Some((true, false)),
            "restored P must not become a Task with a revoke issuer"
        );
        assert!(crate::original_ingress::controller_only_basis(
            &association.candidate().authority_basis
        ));
        assert_eq!(
            association
                .candidate()
                .authority_basis
                .grant_lineages()
                .count(),
            0
        );
        assert_eq!(
            fixture.cold_source_record().await,
            row,
            "native custody promotion does not rewrite evidence"
        );
        let after = history.load_input_states_strict(&runtime).await.unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].state.input_id, before[0].state.input_id);
        assert!(after[0].state.persisted_input.is_none());
        meerkat_runtime::input_state::verify_exact_replay(&after[0].state, &input).unwrap();
        assert_eq!(fixture.model_requests(), requests);
        fixture
            .source
            .prepare_workflow_parent_source(fixture.original_reference())
            .await
            .unwrap();
        assert_eq!(fixture.cold_source_record().await, row);
    }))
    .catch_unwind()
    .await;
    close(&fixture, body).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_parent_cannot_retrofit_planning_input_evidence() {
    let fixture = FirstFixture::new(true).await;
    let body = std::panic::AssertUnwindSafe(Box::pin(async {
        let first = fixture.submit().await;
        fixture.finish(&first).await.unwrap();
        let row = fixture.cold_source_record().await;
        let record = fixture
            .source
            .decode_record(&row, fixture.original_reference())
            .unwrap();
        assert!(record.planning_source_evidence.is_none() && record.bound_service_input.is_none());
        assert_eq!(
            fixture
                .source
                .prepare_workflow_parent_source(fixture.original_reference())
                .await,
            Err(PrivateOriginalError::Unavailable)
        );
        let association = fixture.binding().await.unwrap().unwrap();
        forget(&fixture, &association);
        assert_eq!(
            fixture
                .source
                .restore_workflow_parent_source(fixture.original_reference())
                .await,
            Err(PrivateOriginalError::Unavailable)
        );
        assert!(
            !fixture
                .source
                .owner
                .has_admitted_source(&association)
                .unwrap()
        );
        assert_eq!(fixture.cold_source_record().await, row);
        assert_eq!(fixture.model_requests(), 1);
    }))
    .catch_unwind()
    .await;
    close(&fixture, body).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn selected_parent_missing_changed_or_cancelled_evidence_never_promotes() {
    let fixture = FirstFixture::new(true).await;
    let body = std::panic::AssertUnwindSafe(Box::pin(async {
        fixture
            .source
            .bind_controller_pin_source(fixture.service.clone())
            .unwrap();
        let mut wrong = fixture.original_reference().clone();
        wrong.event_sha256 = "f".repeat(64);
        assert_eq!(
            fixture.source.prepare_workflow_parent_source(&wrong).await,
            Err(PrivateOriginalError::Conflict)
        );
        assert!(
            fixture
                .cold_source_record()
                .await
                .data
                .body
                .get("planning_source_evidence")
                .is_none()
        );
        fixture
            .source
            .prepare_workflow_parent_source(fixture.original_reference())
            .await
            .unwrap();
        let first = fixture.submit().await;
        fixture.finish(&first).await.unwrap();
        let association = fixture.binding().await.unwrap().unwrap();
        let saved = fixture.cold_source_record().await;
        let requests = fixture.model_requests();
        let inputs = fixture.inputs().await;
        forget(&fixture, &association);
        for changed in [false, true] {
            let current = fixture.cold_source_record().await;
            let mut data = saved.data.clone();
            if changed {
                data.body["bound_service_input"]["content"] =
                    serde_json::json!("different original bytes");
            } else {
                data.body
                    .as_object_mut()
                    .unwrap()
                    .remove("bound_service_input");
            }
            fixture
                .store
                .compare_and_swap(Transition {
                    key: current.key,
                    expected_revision: current.revision,
                    expected_digest: Some(current.content_digest),
                    claim: None,
                    data,
                    events: vec![],
                    effects: vec![],
                })
                .await
                .unwrap();
            assert_eq!(
                fixture
                    .source
                    .restore_workflow_parent_source(fixture.original_reference())
                    .await,
                Err(if changed {
                    PrivateOriginalError::Conflict
                } else {
                    PrivateOriginalError::Unavailable
                })
            );
            assert!(
                !fixture
                    .source
                    .owner
                    .has_admitted_source(&association)
                    .unwrap()
            );
        }
        let current = fixture.cold_source_record().await;
        fixture
            .store
            .compare_and_swap(Transition {
                key: current.key,
                expected_revision: current.revision,
                expected_digest: Some(current.content_digest),
                claim: None,
                data: saved.data,
                events: vec![],
                effects: vec![],
            })
            .await
            .unwrap();
        fixture
            .source
            .record_operation_cancelled(fixture.original_reference())
            .await
            .unwrap();
        assert_eq!(
            fixture
                .source
                .restore_workflow_parent_source(fixture.original_reference())
                .await,
            Err(PrivateOriginalError::Denied)
        );
        assert!(
            !fixture
                .source
                .owner
                .has_admitted_source(&association)
                .unwrap()
        );
        assert!(
            fixture
                .source
                .decode_record(
                    &fixture.cold_source_record().await,
                    fixture.original_reference()
                )
                .unwrap()
                .operation_cancelled
        );
        assert_eq!(fixture.model_requests(), requests);
        assert_eq!(fixture.inputs().await, inputs);
    }))
    .catch_unwind()
    .await;
    close(&fixture, body).await;
}

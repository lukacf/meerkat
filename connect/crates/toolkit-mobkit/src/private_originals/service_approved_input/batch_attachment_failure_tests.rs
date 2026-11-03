//! One actual member becomes usable before another fails initialization. The
//! ordinary batch caller must reject unbound B and settle exactly this set.
use super::*;
use meerkat_toolkit::setup::{ApprovedServiceAttachmentFailure, ApprovedServiceAttachmentPhase};

fn grant_journal(fixture: &PersistentFixture) -> Vec<u8> {
    std::fs::read(fixture.source_path.with_file_name("grants.jsonl")).unwrap()
}

fn journal_rows(bytes: &[u8]) -> Vec<serde_json::Value> {
    std::str::from_utf8(bytes)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

async fn input_rows(fixture: &PersistentFixture, history: &dyn RuntimeStore) -> serde_json::Value {
    serde_json::to_value(
        history
            .load_input_states_strict(&LogicalRuntimeId::for_session(&fixture.session))
            .await
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_second_attachment_failure_retires_and_compensates_only_unbound_batch_once() {
    let (mut fixture, history) =
        PersistentFixture::for_console_service_batch(selected_human(), |inner| inner).await;
    let endpoints = [Endpoint::new(), Endpoint::failing_initialize()];
    let outcome = AssertUnwindSafe(tokio::time::timeout(WAIT * 6, async {
        let (b, stored, task, _pipe) = prepare_batch(&mut fixture, &endpoints).await;
        let members = task.attachments();
        assert_eq!(members.len(), 2);
        let before_inputs = input_rows(&fixture, history.as_ref()).await;
        assert_eq!(before_inputs.as_array().unwrap().len(), 1);
        let model_requests = fixture.model_requests();
        let parent_key = fixture.source.key(&task.parent).unwrap();
        let parent_before = fixture.store.get(&parent_key).await.unwrap().unwrap();

        let route = fixture.source.route(&b.route).unwrap();
        let original = meerkat_authorization_contracts::work_association::OriginalWorkRef {
            authority: route.source_authority.clone(),
            work: b.original.clone(),
        };
        let mut expected_revokes = Vec::new();
        for grant in task.grants() {
            let lineage = fixture
                .source
                .service_grant_lineage(grant, &original)
                .await
                .unwrap();
            assert_eq!(
                lineage.len(),
                2,
                "each exact member has its own parent and child"
            );
            expected_revokes.extend(
                lineage
                    .iter()
                    .map(|reference| reference.grant_id.as_str().to_owned()),
            );
        }
        assert_eq!(expected_revokes.len(), 6);
        assert_eq!(
            expected_revokes
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            6,
            "catalog and both targets have independent exact grant pairs"
        );

        fixture
            .source
            .step_service_input_with_attachment(&fixture.store, &stored)
            .await
            .unwrap();
        endpoints[0].wait_for(&endpoints[0].listed).await;
        let alpha = fixture
            .source
            .held_service_attachment(&b, &members[0])
            .unwrap()
            .unwrap();
        let alpha_receipt = alpha
            .receipt(|| panic!("one accepted alpha attachment only"))
            .await
            .unwrap()
            .unwrap();
        assert!(alpha_receipt.outcome.staged && !alpha_receipt.outcome.persisted);
        assert_eq!(alpha_receipt.attempt.server(), ALPHA);
        assert_eq!(
            alpha_receipt.attempt.observe("read").await,
            McpAttachmentReadiness::Pending
        );
        endpoints[0].release();
        assert!(matches!(
            alpha_receipt.attempt.wait("read", WAIT).await,
            McpAttachmentReadiness::Usable { .. }
        ));

        // The ordinary caller now reaches the second actual attachment while
        // the first is usable. It still cannot bind or submit B.
        fixture
            .source
            .step_service_input_with_attachment(&fixture.store, &stored)
            .await
            .unwrap();
        endpoints[1].wait_for(&endpoints[1].started).await;
        let beta = fixture
            .source
            .held_service_attachment(&b, &members[1])
            .unwrap()
            .unwrap();
        let beta_receipt = beta
            .receipt(|| panic!("one accepted beta attachment only"))
            .await
            .unwrap()
            .unwrap();
        assert!(beta_receipt.outcome.staged && !beta_receipt.outcome.persisted);
        assert_eq!(beta_receipt.attempt.server(), BETA);
        assert_eq!(
            beta_receipt.attempt.observe("read").await,
            McpAttachmentReadiness::Pending
        );
        assert!(matches!(
            alpha_receipt.attempt.observe("read").await,
            McpAttachmentReadiness::Usable { .. }
        ));
        assert_eq!(input_rows(&fixture, history.as_ref()).await, before_inputs);
        assert_eq!(fixture.model_requests(), model_requests);
        let pending = fixture
            .source
            .decode_record(&fixture.store.get(&stored.key).await.unwrap().unwrap(), &b)
            .unwrap();
        assert!(!pending.operation_cancelled);
        assert!(
            pending.final_association.is_none()
                && pending.bound_service_input.is_none()
                && pending.service_admission.is_none()
        );
        let before_journal = grant_journal(&fixture);
        let issued_ids: std::collections::BTreeSet<_> = journal_rows(&before_journal)
            .iter()
            .filter_map(|row| {
                row["change"]["kind"]["issued"]["record"]["id"]
                    .as_str()
                    .map(str::to_owned)
            })
            .collect();
        assert!(expected_revokes.iter().all(|id| issued_ids.contains(id)));

        endpoints[1].release();
        assert!(matches!(
            beta_receipt.attempt.wait("read", WAIT).await,
            McpAttachmentReadiness::Failed { .. }
        ));
        fixture
            .source
            .step_service_input_with_attachment(&fixture.store, &stored)
            .await
            .unwrap();
        let saved = fixture.store.get(&stored.key).await.unwrap().unwrap();
        let rejected = fixture.source.decode_record(&saved, &b).unwrap();
        assert!(rejected.operation_cancelled);
        assert!(
            rejected.final_association.is_none()
                && rejected.bound_service_input.is_none()
                && rejected.service_admission.is_none()
        );
        assert!(
            rejected.service_attachment_failure == Some(ApprovedServiceAttachmentFailure::Failed)
        );
        assert!(rejected.service_attachment_member.as_ref() == Some(&members[1].grant));
        assert_eq!(
            rejected
                .service_attachment_observation
                .as_ref()
                .unwrap()
                .phase,
            ApprovedServiceAttachmentPhase::Initialize
        );
        assert!(rejected.approved_task.as_ref() == Some(&task));
        assert_eq!(
            fixture.store.get(&parent_key).await.unwrap().unwrap(),
            parent_before
        );
        for member in &members {
            assert!(
                fixture
                    .source
                    .held_service_attachment(&b, member)
                    .unwrap()
                    .is_none()
            );
        }
        assert_eq!(input_rows(&fixture, history.as_ref()).await, before_inputs);
        assert_eq!(fixture.model_requests(), model_requests);
        assert!(
            !endpoints[1].listed.exists(),
            "failed initialize never discovered tools"
        );
        assert!(
            endpoints
                .iter()
                .all(|endpoint| endpoint.starts() == 1 && endpoint.calls().is_empty())
        );

        let after_journal = grant_journal(&fixture);
        assert!(
            after_journal.starts_with(&before_journal),
            "full prior journal bytes remain exact"
        );
        let suffix = journal_rows(&after_journal[before_journal.len()..]);
        assert_eq!(
            suffix.len(),
            6,
            "only the six exact revocations are appended"
        );
        let actual_revokes: Vec<_> = suffix
            .iter()
            .map(|row| {
                row["change"]["kind"]["revoked"]["grant_id"]
                    .as_str()
                    .expect("cleanup cannot issue or mutate another kind")
                    .to_owned()
            })
            .collect();
        assert_eq!(
            actual_revokes, expected_revokes,
            "each selected parent precedes its child exactly once"
        );

        // Replay uses retained cancellation and the actual revoked native
        // lineages. It may neither restart an attachment nor reissue a grant.
        fixture
            .source
            .step_service_input_with_attachment(&fixture.store, &saved)
            .await
            .unwrap();
        assert_eq!(
            fixture.store.get(&stored.key).await.unwrap().unwrap(),
            saved
        );
        assert_eq!(
            fixture.store.get(&parent_key).await.unwrap().unwrap(),
            parent_before
        );
        assert_eq!(grant_journal(&fixture), after_journal);
        assert_eq!(input_rows(&fixture, history.as_ref()).await, before_inputs);
        assert_eq!(fixture.model_requests(), model_requests);
        assert!(
            endpoints
                .iter()
                .all(|endpoint| endpoint.starts() == 1 && endpoint.calls().is_empty())
        );
        for member in &members {
            assert!(
                fixture
                    .source
                    .held_service_attachment(&b, member)
                    .unwrap()
                    .is_none()
            );
        }
    }))
    .catch_unwind()
    .await;
    for endpoint in &endpoints {
        endpoint.release();
    }
    fixture.source.join_service_attachments().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match outcome {
        Ok(Ok(())) => (),
        Ok(Err(_)) => panic!("actual batch attachment failure control timed out"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

//! Actual canonical paging and ordinary native resume. The production worker
//! consumes the same step; these controls do not construct native Ready or
//! start another worker, submit input, or recover private context transfers.
use super::idle_restore_tests::close;
use super::waiter_restore_tests::{
    PersistentDedicated, all_ids, histories, join_recovery_for_idle_restore, selected_then_close,
};
use super::*;
use std::sync::atomic::AtomicUsize;

async fn sweep(c: &PersistentDedicated) -> (usize, BTreeSet<String>) {
    let mut after = None;
    let mut keys = BTreeSet::new();
    let mut restored = 0;
    for _ in 0..128 {
        let page =
            c.f.source
                .workflow_close_recovery_page(after.as_deref(), 1)
                .await
                .unwrap();
        assert!(page.sources.len() <= 1);
        for source in page.sources {
            assert!(
                keys.insert(source.key.id.clone()),
                "canonical cursor cannot repeat a row"
            );
            match c
                .f
                .source
                .recover_workflow_interest_source(&source, || true)
                .await
                .unwrap()
            {
                WorkflowInterestRecovery::Restored { memberships } => restored += memberships,
                WorkflowInterestRecovery::NotSelected => (),
                WorkflowInterestRecovery::Stopped => panic!("this sweep remains entered"),
            }
        }
        match page.next_after_id {
            Some(next) => {
                assert_ne!(after.as_ref(), Some(&next));
                after = Some(next);
            }
            None => return (restored, keys),
        }
    }
    panic!("bounded canonical source page must terminate");
}

fn assert_no_restored_sources(
    c: &PersistentDedicated,
    a: &InputAuthorityAssociation,
    p: &InputAuthorityAssociation,
    g: &InputAuthorityAssociation,
) {
    for association in [a, p, g] {
        assert!(!c.f.source.owner.has_admitted_source(association).unwrap());
    }
    assert!(c.member.session().is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn canonical_interest_pages_restore_once_and_replays_keep_publication_current() {
    let saved = selected_then_close().await;
    let c = PersistentDedicated::reopen(saved.durable.clone()).await;
    let body = AssertUnwindSafe(Box::pin(async {
        join_recovery_for_idle_restore(&c).await;
        let source = &c.f.source;
        let publication = source.owner.publication();
        let (_, before) = publication.observe(|| ()).unwrap();
        let grants_path = c.f.source_path.parent().unwrap().join("grants.jsonl");
        let grants = std::fs::read(&grants_path).unwrap();
        let effect =
            c.f.store
                .get_effect(&saved.membership.delivery_effect().unwrap())
                .await
                .unwrap()
                .unwrap();
        let (count, keys) = sweep(&c).await;
        assert_eq!(
            count, 1,
            "only the selected A row restores its canonical membership"
        );
        assert!(
            keys.len() >= 3,
            "paging includes distinct P, G and A source rows"
        );
        assert!(keys.contains(&saved.row.key.id));
        assert!(
            before.check_current().is_err(),
            "first source custody and attachment publish"
        );
        assert_eq!(c.member.session().unwrap(), c.lead_session);
        let (a, g) = {
            let held = source.retained.lock().unwrap();
            (
                held.iter()
                    .find(|row| &row.reference == c.f.original_reference())
                    .unwrap()
                    .clone(),
                held.iter()
                    .find(|row| row.reference == c.lead.original)
                    .unwrap()
                    .clone(),
            )
        };
        let plan = source
            .retained_workflow_selection(&a.original, &id("selected-a-restore-call"))
            .unwrap()
            .unwrap();
        let attachment = g.setup_workflow.get().unwrap();
        assert!(attachment.accepted.load(Ordering::Acquire));
        assert!(
            attachment.waiters.lock().unwrap().is_empty(),
            "interest restoration grants no G effect custody"
        );
        assert_eq!(plan.interests.lock().unwrap().len(), 1);
        let (_, replay) = publication.observe(|| ()).unwrap();
        assert_eq!(sweep(&c).await, (count, keys.clone()));
        replay
            .check_current()
            .expect("actual P/A/G observations and exact attachment replay are publication-free");
        let same = source
            .retained_workflow_selection(&a.original, &id("selected-a-restore-call"))
            .unwrap()
            .unwrap();
        assert!(Arc::ptr_eq(&plan, &same));
        assert_eq!(
            c.f.store.get_effect(&effect.key).await.unwrap().unwrap(),
            effect
        );
        assert_eq!(std::fs::read(grants_path).unwrap(), grants);
        assert_eq!(all_ids(&c).await, saved.ids);
        assert_eq!(histories(&c).await, saved.histories);
        assert_eq!(c.f.model_requests(), 0);
        plan.depart_waiter(&saved.membership).await.unwrap();
        let (_, departed) = publication.observe(|| ()).unwrap();
        assert_eq!(sweep(&c).await, (0, keys));
        departed
            .check_current()
            .expect("departed rows do not promote or reopen their tombstone");
        let interests = plan.interests.lock().unwrap();
        assert_eq!(interests.len(), 1);
        assert!(interests[0].closed);
    }))
    .catch_unwind()
    .await;
    close(&c, body).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn canonical_interest_step_rereads_selection_and_cancelled_evidence_before_promotion() {
    let saved = selected_then_close().await;
    let c = PersistentDedicated::reopen(saved.durable.clone()).await;
    let body = AssertUnwindSafe(Box::pin(async {
        let source = &c.f.source;
        let p = source
            .binding_evidence(&c.parent.original)
            .await
            .unwrap()
            .unwrap();
        let g = source
            .binding_evidence(&c.lead.original)
            .await
            .unwrap()
            .unwrap();
        let observed = saved.row.clone();
        for case in 0..3 {
            let row = c.f.store.get(&observed.key).await.unwrap().unwrap();
            let mut data = saved.row.data.clone();
            match case {
                0 => {
                    data.body
                        .as_object_mut()
                        .unwrap()
                        .remove("planning_source_evidence");
                }
                1 => {
                    data.body
                        .as_object_mut()
                        .unwrap()
                        .remove("bound_service_input");
                }
                _ => {
                    data.body["planning_source_evidence"] =
                        serde_json::json!({"not_a_selection": true});
                }
            }
            c.f.store
                .compare_and_swap(toolkit_store::Transition {
                    key: row.key,
                    expected_revision: row.revision,
                    expected_digest: Some(row.content_digest),
                    claim: None,
                    data,
                    events: vec![],
                    effects: vec![],
                })
                .await
                .unwrap();
            let publication = source.owner.publication();
            let (_, stamp) = publication.observe(|| ()).unwrap();
            let result = source
                .recover_workflow_interest_source(&observed, || true)
                .await;
            match case {
                0 => assert_eq!(result, Ok(WorkflowInterestRecovery::NotSelected)),
                1 => assert_eq!(result, Err(PrivateOriginalError::Unavailable)),
                _ => assert_eq!(result, Err(PrivateOriginalError::Conflict)),
            }
            stamp
                .check_current()
                .expect("unselected or incomplete evidence cannot publish source custody");
            assert_no_restored_sources(&c, &saved.association, &p, &g);
        }
        let row = c.f.store.get(&observed.key).await.unwrap().unwrap();
        c.f.store
            .compare_and_swap(toolkit_store::Transition {
                key: row.key,
                expected_revision: row.revision,
                expected_digest: Some(row.content_digest),
                claim: None,
                data: saved.row.data.clone(),
                events: vec![],
                effects: vec![],
            })
            .await
            .unwrap();
        source
            .record_operation_cancelled(c.f.original_reference())
            .await
            .unwrap();
        assert_eq!(
            source
                .recover_workflow_interest_source(&observed, || true)
                .await,
            Err(PrivateOriginalError::Denied)
        );
        assert_no_restored_sources(&c, &saved.association, &p, &g);
        assert_eq!(all_ids(&c).await, saved.ids);
        assert_eq!(histories(&c).await, saved.histories);
        assert_eq!(c.f.model_requests(), 0);
    }))
    .catch_unwind()
    .await;
    close(&c, body).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn canonical_interest_step_observes_stop_and_departure_after_page_read() {
    let saved = selected_then_close().await;
    let c = PersistentDedicated::reopen(saved.durable.clone()).await;
    let body = AssertUnwindSafe(Box::pin(async {
        let source = &c.f.source;
        let p = source
            .binding_evidence(&c.parent.original)
            .await
            .unwrap()
            .unwrap();
        let g = source
            .binding_evidence(&c.lead.original)
            .await
            .unwrap()
            .unwrap();
        let page = source
            .workflow_close_recovery_page(None, 128)
            .await
            .unwrap();
        assert!(page.next_after_id.is_none());
        let observed = page
            .sources
            .iter()
            .find(|row| row.key == saved.row.key)
            .unwrap();
        let publication = source.owner.publication();
        let (_, stopped) = publication.observe(|| ()).unwrap();
        assert_eq!(
            source
                .recover_workflow_interest_source(observed, || false)
                .await,
            Ok(WorkflowInterestRecovery::Stopped)
        );
        let entries = AtomicUsize::new(0);
        assert_eq!(
            source
                .recover_workflow_interest_source(observed, || entries
                    .fetch_add(1, Ordering::AcqRel)
                    == 0)
                .await,
            Ok(WorkflowInterestRecovery::Stopped)
        );
        assert_eq!(
            entries.load(Ordering::Acquire),
            2,
            "stop is checked after the entered canonical reread"
        );
        stopped
            .check_current()
            .expect("stop admits no source restoration");
        assert_no_restored_sources(&c, &saved.association, &p, &g);
        // The page remains unchanged while its actual canonical membership
        // ceases to be live. A page snapshot cannot recreate that interest.
        source
            .workflow_store()
            .unwrap()
            .depart_setup_workflow(&saved.membership)
            .await
            .unwrap();
        let (_, departed) = publication.observe(|| ()).unwrap();
        assert_eq!(
            source
                .recover_workflow_interest_source(observed, || true)
                .await,
            Ok(WorkflowInterestRecovery::NotSelected)
        );
        departed
            .check_current()
            .expect("a departed canonical membership stays absent locally");
        assert_no_restored_sources(&c, &saved.association, &p, &g);
        assert_eq!(all_ids(&c).await, saved.ids);
        assert_eq!(histories(&c).await, saved.histories);
        assert_eq!(c.f.model_requests(), 0);
    }))
    .catch_unwind()
    .await;
    close(&c, body).await;
}

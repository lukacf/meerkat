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

/// Neither A's, P's nor G's member was woken: a stopped, refused or
/// unselected recovery step reaches no wake. Every read here is a registry
/// observation. The controls that use it reopen with G left Dormant, so an
/// unwanted wake of the lead shows here too.
async fn assert_unwoken(c: &PersistentDedicated) {
    for session in [&c.f.session, &c.parent_session, &c.lead_session] {
        assert!(
            super::idle_restore_tests::dormant(c, session).await,
            "no recovery step woke {session}"
        );
    }
}

/// The controls' reopen: every member stays Dormant, G included (its pin
/// step is skipped), so a control can prove its step wakes no one.
async fn reopen_all_dormant(saved: &super::waiter_restore_tests::Saved) -> PersistentDedicated {
    use crate::private_originals::governed_submit::tests::first_input_tests::dedicated::LEAVE_LEAD_DORMANT;
    LEAVE_LEAD_DORMANT
        .scope((), PersistentDedicated::reopen(saved.durable.clone()))
        .await
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
    let c = reopen_all_dormant(&saved).await;
    let body = AssertUnwindSafe(Box::pin(async {
        // The custody worker's rounds stop first, as for every idle
        // component step, so the step under test is the only actor on the
        // source writer and on the members.
        join_recovery_for_idle_restore(&c).await;
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
            assert_unwoken(&c).await;
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
        assert_unwoken(&c).await;
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
    let c = reopen_all_dormant(&saved).await;
    let body = AssertUnwindSafe(Box::pin(async {
        // The custody worker's rounds stop first, as for every idle
        // component step, so the step under test is the only actor on the
        // source writer and on the members.
        join_recovery_for_idle_restore(&c).await;
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
        assert_unwoken(&c).await;
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
        assert_unwoken(&c).await;
        assert_eq!(all_ids(&c).await, saved.ids);
        assert_eq!(histories(&c).await, saved.histories);
        assert_eq!(c.f.model_requests(), 0);
    }))
    .catch_unwind()
    .await;
    close(&c, body).await;
}

/// The waiter restoration the acquisition sweep runs for a selected interest
/// holds the source writer across its canonical reads. The workflow lead's
/// first wake arrives in that window: its registration capture is ordered by
/// the publication, not the source writer, so G wakes while the restoration
/// holds the writer. The released restoration then refuses its observation,
/// which the capture made stale, and the same restoration run again
/// completes. G registers exactly once, with no input and no model request.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lead_capture_races_a_writer_held_waiter_restoration() {
    use crate::private_originals::governed_submit::tests::first_input_tests::dedicated::LEAVE_LEAD_DORMANT;
    use crate::private_originals::planning_source_evidence::WAITER_SOURCE_READ;
    use meerkat_mobkit::identity_first::ControllerPinSource;
    let saved = selected_then_close().await;
    let c = LEAVE_LEAD_DORMANT
        .scope((), PersistentDedicated::reopen(saved.durable.clone()))
        .await;
    let body = AssertUnwindSafe(Box::pin(async {
        join_recovery_for_idle_restore(&c).await;
        assert!(
            c.f.service
                .live_session_actor_witness(&c.lead_session)
                .await
                .is_none(),
            "the reopen left G Dormant"
        );
        let hold = Arc::new((tokio::sync::Notify::new(), tokio::sync::Notify::new()));
        let member = c.member.clone();
        let waiter = c.f.original_reference().clone();
        let restoration = tokio::spawn(WAITER_SOURCE_READ.scope(hold.clone(), async move {
            member.restore_waiter_source(&waiter).await
        }));
        tokio::time::timeout(GUARD, hold.0.notified())
            .await
            .expect("the restoration holds the source writer at its canonical read");
        assert!(
            c.f.source.owner.try_source_writer().is_err(),
            "the restoration holds the actual source writer"
        );
        let woke = meerkat_core::session_owner_wake::SessionOwnerWakeRegistry::global()
            .wake(&c.lead_session)
            .await;
        assert!(
            matches!(
                woke,
                Some(Ok(
                    meerkat_core::session_owner_wake::SessionOwnerWakeOutcome::Ready
                ))
            ),
            "G wakes while the restoration holds the writer: {woke:?}"
        );
        let registration =
            c.f.machine
                .current_session_registration_witness(&c.lead_session)
                .await
                .expect("G registered on its own session");
        // The fixture's pin step for G, at this exact point.
        let actor = c.f.service.live_actor(&c.lead_session).await.unwrap();
        let pin = c.f.service.pin_actor(&actor).await.unwrap();
        meerkat_core::publish_token_lifecycle_acquired_for_identity_within(
            &c.f.machine.generated_auth_lease_handle(),
            pin.selection().credential(),
            &meerkat_core::auth::PersistedTokens::api_key("synthetic-controller-only-fixture"),
            tokio::time::Instant::now() + GUARD,
        )
        .await
        .unwrap();
        hold.1.notify_one();
        let first = tokio::time::timeout(GUARD, restoration)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            first,
            Err(PrivateOriginalError::Unavailable),
            "the capture made the restoration's observation stale"
        );
        c.member
            .restore_waiter_source(c.f.original_reference())
            .await
            .unwrap();
        assert_eq!(
            c.f.machine
                .current_session_registration_witness(&c.lead_session)
                .await,
            Some(registration),
            "G registered exactly once"
        );
        assert_eq!(all_ids(&c).await, saved.ids, "no input");
        assert_eq!(c.f.model_requests(), 0, "no model request");
    }))
    .catch_unwind()
    .await;
    close(&c, body).await;
}

/// After a restart the acquisition sweep's step for a selected interest
/// wakes the Dormant workflow lead on its own session before it reads G's
/// capture. The lead's capture changes the publication, so the first step
/// answers Unavailable (its retryable refusal); the next step restores the
/// selected interest. G registers exactly once, with no input and no model
/// request.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sweep_step_wakes_its_dormant_lead_before_reading_its_capture() {
    use crate::private_originals::governed_submit::tests::first_input_tests::dedicated::LEAVE_LEAD_DORMANT;
    use meerkat_mobkit::identity_first::ControllerPinSource;
    let saved = selected_then_close().await;
    let c = LEAVE_LEAD_DORMANT
        .scope((), PersistentDedicated::reopen(saved.durable.clone()))
        .await;
    let body = AssertUnwindSafe(Box::pin(async {
        join_recovery_for_idle_restore(&c).await;
        assert!(
            c.f.machine
                .current_session_registration_witness(&c.lead_session)
                .await
                .is_none(),
            "the reopen left G Dormant"
        );
        let first =
            c.f.source
                .recover_workflow_interest_source(&saved.row, || true)
                .await;
        assert_eq!(first, Err(PrivateOriginalError::Unavailable));
        let registration =
            c.f.machine
                .current_session_registration_witness(&c.lead_session)
                .await
                .expect("the step woke G on its own session");
        // The fixture's pin step for G, once the step woke it.
        let actor = c.f.service.live_actor(&c.lead_session).await.unwrap();
        let pin = c.f.service.pin_actor(&actor).await.unwrap();
        meerkat_core::publish_token_lifecycle_acquired_for_identity_within(
            &c.f.machine.generated_auth_lease_handle(),
            pin.selection().credential(),
            &meerkat_core::auth::PersistedTokens::api_key("synthetic-controller-only-fixture"),
            tokio::time::Instant::now() + GUARD,
        )
        .await
        .unwrap();
        let restored =
            c.f.source
                .recover_workflow_interest_source(&saved.row, || true)
                .await
                .unwrap();
        assert!(matches!(
            restored,
            WorkflowInterestRecovery::Restored { .. }
        ));
        assert_eq!(
            c.f.machine
                .current_session_registration_witness(&c.lead_session)
                .await,
            Some(registration),
            "G registered exactly once"
        );
        assert_eq!(all_ids(&c).await, saved.ids, "no input");
        assert_eq!(c.f.model_requests(), 0, "no model request");
    }))
    .catch_unwind()
    .await;
    close(&c, body).await;
}

/// A selected waiter whose workflow lead was cancelled (the cancellation
/// recorded in G's own source row, the workflow not yet departed) never
/// wakes that lead. The production step reads G's cancellation with its
/// other capture-independent canonical facts before any wake and refuses
/// Denied. G, A and P stay Dormant: no registration, attachment or actor,
/// no source custody, no input and no model request.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sweep_step_leaves_a_cancelled_dormant_lead_dormant() {
    let saved = selected_then_close().await;
    let c = reopen_all_dormant(&saved).await;
    let body = AssertUnwindSafe(Box::pin(async {
        join_recovery_for_idle_restore(&c).await;
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
        assert_unwoken(&c).await;
        source
            .record_operation_cancelled(&c.lead.original)
            .await
            .unwrap();
        assert_unwoken(&c).await;
        assert_eq!(
            source
                .recover_workflow_interest_source(&saved.row, || true)
                .await,
            Err(PrivateOriginalError::Denied),
            "the cancelled lead is refused before any wake"
        );
        assert_no_restored_sources(&c, &saved.association, &p, &g);
        assert_unwoken(&c).await;
        assert_eq!(all_ids(&c).await, saved.ids, "no input");
        assert_eq!(histories(&c).await, saved.histories);
        assert_eq!(c.f.model_requests(), 0, "no model request");
    }))
    .catch_unwind()
    .await;
    close(&c, body).await;
}

/// The step's publication observation, taken before its canonical reads, is
/// the one its lead's wake carries. Held at the lead's wake selection, after
/// those reads, the step meets a change completed there: first another owner
/// change, then a cancellation of G itself. Each makes the observation stale
/// and the step refuses before it asks Core to wake G. A later step reads
/// G's cancellation and refuses Denied, again with no wake. G, A and P stay
/// Dormant throughout, with no custody, input or model request.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lead_wake_selection_refuses_changes_completed_after_its_canonical_reads() {
    use crate::private_originals::setup_workflow::launch::recovery::LEAD_WAKE_SELECTION;
    let saved = selected_then_close().await;
    let c = reopen_all_dormant(&saved).await;
    let body = AssertUnwindSafe(Box::pin(async {
        join_recovery_for_idle_restore(&c).await;
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
        assert_unwoken(&c).await;
        for cancel_lead in [false, true] {
            let hold = Arc::new((tokio::sync::Notify::new(), tokio::sync::Notify::new()));
            let step_source = source.clone();
            let row = saved.row.clone();
            let step = tokio::spawn(LEAD_WAKE_SELECTION.scope(hold.clone(), async move {
                step_source
                    .recover_workflow_interest_source(&row, || true)
                    .await
            }));
            tokio::time::timeout(GUARD, hold.0.notified())
                .await
                .expect("the step reached its lead's wake selection");
            if cancel_lead {
                source
                    .record_operation_cancelled(&c.lead.original)
                    .await
                    .unwrap();
            } else {
                drop(source.owner.publication().begin_owner_change().unwrap());
            }
            hold.1.notify_one();
            let refused = tokio::time::timeout(GUARD, step)
                .await
                .expect("the held step settles")
                .unwrap();
            assert_eq!(
                refused,
                Err(PrivateOriginalError::Unavailable),
                "the stale observation refuses before the wake"
            );
            assert_no_restored_sources(&c, &saved.association, &p, &g);
            assert_unwoken(&c).await;
        }
        assert_eq!(
            source
                .recover_workflow_interest_source(&saved.row, || true)
                .await,
            Err(PrivateOriginalError::Denied),
            "the cancelled lead is refused before any wake"
        );
        assert_no_restored_sources(&c, &saved.association, &p, &g);
        assert_unwoken(&c).await;
        assert_eq!(all_ids(&c).await, saved.ids, "no input");
        assert_eq!(histories(&c).await, saved.histories);
        assert_eq!(c.f.model_requests(), 0, "no model request");
    }))
    .catch_unwind()
    .await;
    close(&c, body).await;
}

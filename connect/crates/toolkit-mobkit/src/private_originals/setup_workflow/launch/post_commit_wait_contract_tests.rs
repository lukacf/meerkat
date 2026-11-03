//! Consumer controls for the owner-authored stale-read wait contract.
//! Uses existing public APIs only. These pure-read controls do not reproduce
//! B's unkeyed inner failure or qualify a native launch.
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::Poll;

// Each callback completes a real owner change after lineage resolution but
// before the binding observation checks its stamp. Two callbacks exhaust the
// existing observation plus its one immediate rebuild. There is no holder
// left when the async waiting boundary asks for its hint.
fn invalidate_binding_windows(
    plan: &Arc<InstalledWorkflowPlanning>,
    publication: meerkat_authorization::publication::LocalAuthorizationPublication,
    remaining: usize,
    changes: Arc<AtomicUsize>,
) {
    assert!(remaining > 0);
    let weak = Arc::downgrade(plan);
    *plan.binding_observed_for_test.lock().unwrap() = Some(Box::new(move || {
        drop(publication.begin_owner_change().unwrap());
        changes.fetch_add(1, Ordering::SeqCst);
        if remaining > 1 {
            invalidate_binding_windows(
                &weak.upgrade().expect("binding selection remains alive"),
                publication,
                remaining - 1,
                changes,
            );
        }
    }));
}

// The retained 0.9.7 helper returns typed Changed here after two windows.
// The owner-authored API and consumer wiring must earn the bounded fresh read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workflow_stale_read_after_two_completed_changes_rebuilds_without_launch_effects() {
    let f = FirstFixture::for_continuation().await;
    let result = AssertUnwindSafe(tokio::time::timeout(GUARD, async {
        let c = prepare(&f).await;
        let input_count = f.inputs().await.unwrap();
        let model_requests = f.model_requests();
        let effect_key = c.snapshot.reference().launch_effect();
        let before = serde_json::to_value(f.store.get_effect(&effect_key).await.unwrap()).unwrap();
        let authorization = c.a_dispatch.work_authorization().unwrap().authorization();
        let publication = f.source.owner.publication();
        let changes = Arc::new(AtomicUsize::new(0));
        invalidate_binding_windows(&c.planning, publication.clone(), 2, changes.clone());
        let churn = tokio::sync::Mutex::new(c.a_dispatch.owner_change_churn());
        let mut reads = 0;
        let observed = f
            .source
            .observe_workflow_for_admission(
                authorization.as_ref(),
                &churn,
                tokio::time::Instant::now() + GUARD,
                || {
                    reads += 1;
                    c.planning
                        .check_child_preparation(&f.source, &c.planning.submission.original)
                },
            )
            .await;
        assert_eq!(changes.load(Ordering::SeqCst), 2);
        assert!(publication.change_in_progress().is_none());
        assert!(authorization.owner_change_in_progress().is_none());
        assert!(
            c.planning
                .binding_observed_for_test
                .lock()
                .unwrap()
                .is_none()
        );
        assert_eq!(
            serde_json::to_value(f.store.get_effect(&effect_key).await.unwrap()).unwrap(),
            before
        );
        assert_eq!(f.inputs().await.unwrap(), input_count);
        assert_eq!(f.model_requests(), model_requests);
        assert_eq!(
            observed,
            Ok(()),
            "completed changes still require a bounded fresh observation"
        );
        assert_eq!(reads, 2, "one exhausted two-window read and one fresh read");
    }))
    .catch_unwind()
    .await;
    assert!(f.shutdown().await.is_ok());
    match result {
        Ok(result) => result.expect("completed-change control is bounded"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workflow_new_decision_uses_fresh_churn_after_prior_decision_exhaustion() {
    let f = FirstFixture::for_continuation().await;
    let result = AssertUnwindSafe(tokio::time::timeout(GUARD, async {
        let c = prepare(&f).await;
        let input_count = f.inputs().await.unwrap();
        let model_requests = f.model_requests();
        let effect_key = c.snapshot.reference().launch_effect();
        let before = serde_json::to_value(f.store.get_effect(&effect_key).await.unwrap()).unwrap();
        let authorization = c.a_dispatch.work_authorization().unwrap().authorization();
        let publication = f.source.owner.publication();
        let prior = tokio::sync::Mutex::new(c.a_dispatch.owner_change_churn());
        let deadline = tokio::time::Instant::now() + GUARD;
        for _ in 0..meerkat_core::authorization::OWNER_CHANGE_FREE_REBUILDS {
            let change = publication.begin_owner_change().unwrap();
            let mut waiting = Box::pin(f.source.observe_workflow_for_admission(
                authorization.as_ref(),
                &prior,
                deadline,
                || {
                    c.planning
                        .check_child_preparation(&f.source, &c.planning.submission.original)
                },
            ));
            assert!(matches!(futures::poll!(&mut waiting), Poll::Pending));
            drop(change);
            waiting.await.unwrap();
        }
        let change = publication.begin_owner_change().unwrap();
        let mut exhausted = Box::pin(f.source.observe_workflow_for_admission(
            authorization.as_ref(),
            &prior,
            deadline,
            || {
                c.planning
                    .check_child_preparation(&f.source, &c.planning.submission.original)
            },
        ));
        assert!(matches!(
            futures::poll!(&mut exhausted),
            Poll::Ready(Err(WorkflowChildPreparationError::ObservationChanged))
        ));
        drop(exhausted);
        // The production launch check, not the test, allocates fresh churn.
        let mut fresh = Box::pin(
            c.planning
                .check_for_launch(&f.source, Some((&c.a_dispatch, deadline))),
        );
        assert!(matches!(futures::poll!(&mut fresh), Poll::Pending));
        drop(change);
        fresh.await.unwrap();
        assert_eq!(
            serde_json::to_value(f.store.get_effect(&effect_key).await.unwrap()).unwrap(),
            before
        );
        assert_eq!(f.inputs().await.unwrap(), input_count);
        assert_eq!(f.model_requests(), model_requests);
    }))
    .catch_unwind()
    .await;
    assert!(f.shutdown().await.is_ok());
    match result {
        Ok(result) => result.expect("per-decision churn control is bounded"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workflow_dispatch_wait_cancellation_exhaustion_and_expiry_leave_effects_untouched() {
    for case in 0..3 {
        let f = FirstFixture::for_continuation().await;
        let result = AssertUnwindSafe(tokio::time::timeout(GUARD, async {
            let c = prepare(&f).await;
            let inputs = f.inputs().await.unwrap();
            let requests = f.model_requests();
            let key = c.snapshot.reference().launch_effect();
            let before = serde_json::to_value(f.store.get_effect(&key).await.unwrap()).unwrap();
            let changes = Arc::new(AtomicUsize::new(0));
            let windows = if case == 1 { 8 } else { 2 };
            invalidate_binding_windows(
                &c.planning,
                f.source.owner.publication(),
                windows,
                changes.clone(),
            );
            let deadline = if case == 2 {
                tokio::time::Instant::now() - Duration::from_secs(1)
            } else {
                tokio::time::Instant::now() + GUARD
            };
            let mut launch = Box::pin(c.planning.launch_from_dispatch_with_review(
                &c.snapshot,
                ReviewCustody::default(),
                &c.a_dispatch,
                deadline,
            ));
            if case == 0 {
                // The two invalidating changes have completed. The production
                // wrapper must yield in the no-holder pause before any effect.
                assert!(matches!(futures::poll!(&mut launch), Poll::Pending));
                assert_eq!(changes.load(Ordering::SeqCst), 2);
                assert!(f.source.owner.publication().change_in_progress().is_none());
                drop(launch);
                tokio::task::yield_now().await;
            } else {
                assert!(
                    matches!(
                        launch.await,
                        Err(WorkflowChildPreparationError::ObservationChanged)
                    ),
                    "exhaustion and deadline expiry must preserve the stale fact"
                );
                assert_eq!(changes.load(Ordering::SeqCst), windows);
            }
            assert!(
                c.planning
                    .binding_observed_for_test
                    .lock()
                    .unwrap()
                    .is_none()
            );
            assert_eq!(
                serde_json::to_value(f.store.get_effect(&key).await.unwrap()).unwrap(),
                before
            );
            assert_eq!(f.inputs().await.unwrap(), inputs);
            assert_eq!(f.model_requests(), requests);
        }))
        .catch_unwind()
        .await;
        assert!(f.shutdown().await.is_ok());
        match result {
            Ok(result) => result.expect("dispatch cancellation and refusal control is bounded"),
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workflow_dispatch_wait_settles_before_one_launch_and_replay_only_observes() {
    let f = FirstFixture::for_continuation().await;
    let result = AssertUnwindSafe(tokio::time::timeout(GUARD, async {
        let c = prepare(&f).await;
        c.planning
            .retain_launch_interest(&c.snapshot, &c.membership, &c.a)
            .await
            .unwrap();
        let inputs = f.inputs().await.unwrap();
        let key = c.snapshot.reference().launch_effect();
        let before = serde_json::to_value(f.store.get_effect(&key).await.unwrap()).unwrap();
        let changes = Arc::new(AtomicUsize::new(0));
        invalidate_binding_windows(
            &c.planning,
            f.source.owner.publication(),
            2,
            changes.clone(),
        );
        let deadline = tokio::time::Instant::now() + GUARD;
        let mut launch = Box::pin(c.planning.launch_from_dispatch_with_review(
            &c.snapshot,
            ReviewCustody::default(),
            &c.a_dispatch,
            deadline,
        ));
        assert!(matches!(futures::poll!(&mut launch), Poll::Pending));
        assert_eq!(changes.load(Ordering::SeqCst), 2);
        assert_eq!(
            serde_json::to_value(f.store.get_effect(&key).await.unwrap()).unwrap(),
            before
        );
        assert_eq!(f.inputs().await.unwrap(), inputs);
        let admitted = native(launch.await.unwrap());
        assert!(matches!(
            &admitted,
            PrivateOriginalSubmission::Admitted {
                source_settlement: PrivateSourceSettlement::FreshAcceptancePromoted,
                ..
            }
        ));
        f.finish(&Ok::<_, String>(admitted)).await.unwrap();
        assert_eq!(f.inputs().await.unwrap(), inputs + 1);
        let replay = native(
            c.planning
                .launch_from_dispatch_with_review(
                    &c.snapshot,
                    ReviewCustody::default(),
                    &c.a_dispatch,
                    deadline,
                )
                .await
                .unwrap(),
        );
        assert!(matches!(replay, PrivateOriginalSubmission::Admitted { .. }));
        assert_eq!(f.inputs().await.unwrap(), inputs + 1);
        let effect = f.store.get_effect(&key).await.unwrap().unwrap();
        assert_eq!(effect.status, EffectStatus::Succeeded);
        assert_eq!(effect.attempt, 1);
    }))
    .catch_unwind()
    .await;
    assert!(f.shutdown().await.is_ok());
    match result {
        Ok(result) => result.expect("dispatch launch control is bounded"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

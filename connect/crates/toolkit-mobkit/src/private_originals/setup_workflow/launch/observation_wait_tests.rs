//! Actual P/A/G owners and grants; the wait never submits another input.
//! These controls do not replace the concurrent registered Find SDK control.
use super::*;
use meerkat_core::authorization::OwnerChangeChurn;
use std::task::Poll;

fn selections(f: &FirstFixture, c: &Case) -> usize {
    let held = f
        .source
        .retained
        .lock()
        .unwrap()
        .iter()
        .find(|held| Arc::ptr_eq(&held.original, &c.a.original))
        .unwrap()
        .clone();
    held.workflow_selections.lock().unwrap().len()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workflow_waits_for_actual_publication_before_retaining_one_selection() {
    let f = FirstFixture::for_continuation().await;
    let result = AssertUnwindSafe(tokio::time::timeout(GUARD, async {
        let c = prepare(&f).await;
        assert_eq!(
            c.planning.late_helper_digest(&f.source, None).unwrap(),
            None
        );
        let input_count = f.inputs().await.unwrap();
        let model_requests = f.model_requests();
        let key = f.source.key(&c.planning.submission.original).unwrap();
        let before = f.store.get(&key).await.unwrap().unwrap();
        let count = selections(&f, &c);
        let correlation = id("publication-wait-success");
        let authorization = c.a_dispatch.work_authorization().unwrap().authorization();
        let publication = f.source.owner.publication();
        let change = publication.begin_owner_change().unwrap();
        assert!(publication.change_in_progress().is_some());
        assert_eq!(
            authorization.owner_change_in_progress(),
            publication.change_in_progress(),
            "the wait must use this actual admitted work's publication"
        );
        assert!(
            matches!(
                c.planning
                    .check_for_binding(&f.source, &c.planning.submission.original),
                Err(PrivateOriginalError::Unavailable)
            ),
            "the synchronous contract stays fail-closed"
        );
        let churn = tokio::sync::Mutex::new(OwnerChangeChurn::new());
        let mut pending = Box::pin(f.source.retain_workflow_binding_waiting(
            &c.a,
            &correlation,
            Some(&c.planning),
            None,
            Some(&c.work),
            authorization.as_ref(),
            &churn,
            tokio::time::Instant::now() + GUARD,
            || async { Ok(()) },
        ));
        assert!(matches!(futures::poll!(&mut pending), Poll::Pending));
        assert_eq!(
            selections(&f, &c),
            count,
            "no retention while the native publication is odd"
        );
        drop(change);
        pending.await.unwrap();
        assert_eq!(selections(&f, &c), count + 1);
        assert!(Arc::ptr_eq(
            &f.source
                .retained_workflow_selection(&c.a.original, &correlation)
                .unwrap()
                .unwrap(),
            &c.planning
        ));
        // Exact replay keeps the same canonical object, without a new row.
        f.source
            .retain_workflow_binding_waiting(
                &c.a,
                &correlation,
                Some(&c.planning),
                None,
                Some(&c.work),
                authorization.as_ref(),
                &churn,
                tokio::time::Instant::now() + GUARD,
                || async { Ok(()) },
            )
            .await
            .unwrap();
        assert_eq!(selections(&f, &c), count + 1);
        assert_eq!(f.store.get(&key).await.unwrap().unwrap(), before);
        assert_eq!(f.inputs().await.unwrap(), input_count);
        assert_eq!(f.model_requests(), model_requests);
    }))
    .catch_unwind()
    .await;
    assert!(f.shutdown().await.is_ok());
    match result {
        Ok(result) => result.expect("publication control is bounded"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workflow_wait_reobserves_real_revocation_without_retaining_selection() {
    let f = FirstFixture::for_continuation().await;
    let result = AssertUnwindSafe(tokio::time::timeout(GUARD, async {
        let c = prepare(&f).await;
        let input_count = f.inputs().await.unwrap();
        let model_requests = f.model_requests();
        let count = selections(&f, &c);
        let correlation = id("publication-wait-revoked");
        let authorization = c.a_dispatch.work_authorization().unwrap().authorization();
        let publication = f.source.owner.publication();
        let change = publication.begin_owner_change().unwrap();
        let churn = tokio::sync::Mutex::new(OwnerChangeChurn::new());
        let mut pending = Box::pin(f.source.retain_workflow_binding_waiting(
            &c.a,
            &correlation,
            Some(&c.planning),
            None,
            Some(&c.work),
            authorization.as_ref(),
            &churn,
            tokio::time::Instant::now() + GUARD,
            || async { Ok(()) },
        ));
        assert!(matches!(futures::poll!(&mut pending), Poll::Pending));
        assert_eq!(selections(&f, &c), count);
        // The existing pure-read seam revokes the actual selected leaf after
        // resolution. The outer observation must discard its old grant.
        let grants = f.source.owner.grants();
        let administrator = f
            .lifetime()
            .selected_policy()
            .native_host
            .as_ref()
            .unwrap()
            .grant_administrator
            .clone();
        let leaf = c.planning.lineage[1].clone();
        let machine = f.machine.clone();
        *c.planning.binding_observed_for_test.lock().unwrap() = Some(Box::new(move || {
            let mut custody = machine.try_controller_grant_mutation().unwrap();
            grants
                .revoke(
                    &meerkat_core::AdmissionGuard::none(),
                    &administrator,
                    &leaf,
                    &mut custody,
                )
                .unwrap();
        }));
        drop(change);
        assert!(matches!(
            pending.await,
            Err(WorkflowChildPreparationError::Refused(
                PrivateOriginalError::Denied
            ))
        ));
        assert!(
            c.planning
                .binding_observed_for_test
                .lock()
                .unwrap()
                .is_none()
        );
        assert_eq!(selections(&f, &c), count);
        assert!(
            f.source
                .retained_workflow_selection(&c.a.original, &correlation)
                .unwrap()
                .is_none()
        );
        assert_eq!(f.inputs().await.unwrap(), input_count);
        assert_eq!(f.model_requests(), model_requests);
    }))
    .catch_unwind()
    .await;
    assert!(f.shutdown().await.is_ok());
    match result {
        Ok(result) => result.expect("revocation control is bounded"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workflow_wait_budget_cancellation_and_unavailability_never_retain() {
    let f = FirstFixture::for_continuation().await;
    let result = AssertUnwindSafe(tokio::time::timeout(GUARD, async {
        let c = prepare(&f).await;
        let input_count = f.inputs().await.unwrap();
        let model_requests = f.model_requests();
        let count = selections(&f, &c);
        let authorization = c.a_dispatch.work_authorization().unwrap().authorization();
        let publication = f.source.owner.publication();
        let change = publication.begin_owner_change().unwrap();
        let churn = tokio::sync::Mutex::new(OwnerChangeChurn::new());
        let deadline = tokio::time::Instant::now() + Duration::from_millis(15);
        let refusal = f
            .source
            .retain_workflow_binding_waiting(
                &c.a,
                &id("publication-timeout"),
                Some(&c.planning),
                None,
                Some(&c.work),
                authorization.as_ref(),
                &churn,
                deadline,
                || async { Ok(()) },
            )
            .await;
        assert_eq!(
            refusal,
            Err(WorkflowChildPreparationError::ObservationChanged)
        );
        assert_eq!(selections(&f, &c), count);
        // A different read site in this invocation shares the exhausted Core
        // budget; a later nominal deadline cannot re-arm it.
        let mut reads = 0;
        let mut exhausted = Box::pin(f.source.observe_workflow_for_admission(
            authorization.as_ref(),
            &churn,
            tokio::time::Instant::now() + GUARD,
            || {
                reads += 1;
                c.planning
                    .check_child_preparation(&f.source, &c.planning.submission.original)
            },
        ));
        assert!(matches!(
            futures::poll!(&mut exhausted),
            Poll::Ready(Err(WorkflowChildPreparationError::ObservationChanged))
        ));
        drop(exhausted);
        assert_eq!(reads, 1);
        // Cancellation drops the actual waiting future. There is no detached
        // continuation that can retain a choice after the publication settles.
        let cancelled_churn = tokio::sync::Mutex::new(OwnerChangeChurn::new());
        let cancelled_id = id("publication-cancelled");
        let mut cancelled = Box::pin(f.source.retain_workflow_binding_waiting(
            &c.a,
            &cancelled_id,
            Some(&c.planning),
            None,
            Some(&c.work),
            authorization.as_ref(),
            &cancelled_churn,
            tokio::time::Instant::now() + GUARD,
            || async { Ok(()) },
        ));
        assert!(matches!(futures::poll!(&mut cancelled), Poll::Pending));
        drop(cancelled);
        assert_eq!(selections(&f, &c), count);
        // Typed unavailability wins even while an owner change is visible.
        let mut unavailable_reads = 0;
        let unavailable = f
            .source
            .observe_workflow_for_admission::<()>(
                authorization.as_ref(),
                &cancelled_churn,
                tokio::time::Instant::now() + GUARD,
                || {
                    unavailable_reads += 1;
                    Err(WorkflowChildPreparationError::Refused(
                        PrivateOriginalError::Unavailable,
                    ))
                },
            )
            .await;
        assert_eq!(
            unavailable,
            Err(WorkflowChildPreparationError::Refused(
                PrivateOriginalError::Unavailable
            ))
        );
        assert_eq!(unavailable_reads, 1);
        // Final prerequisite refusal after settlement must precede the first
        // retention. This controls only the read outcome, never native grants.
        let final_churn = tokio::sync::Mutex::new(OwnerChangeChurn::new());
        let final_id = id("publication-final-read-denied");
        let final_reads = std::sync::atomic::AtomicUsize::new(0);
        let mut final_denied = Box::pin(f.source.retain_workflow_binding_waiting(
            &c.a,
            &final_id,
            Some(&c.planning),
            None,
            Some(&c.work),
            authorization.as_ref(),
            &final_churn,
            tokio::time::Instant::now() + GUARD,
            || async {
                if final_reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    Ok(())
                } else {
                    Err(WorkflowChildPreparationError::Refused(
                        PrivateOriginalError::Denied,
                    ))
                }
            },
        ));
        assert!(matches!(futures::poll!(&mut final_denied), Poll::Pending));
        assert_eq!(final_reads.load(std::sync::atomic::Ordering::SeqCst), 1);
        drop(change);
        assert_eq!(
            final_denied.await,
            Err(WorkflowChildPreparationError::Refused(
                PrivateOriginalError::Denied
            ))
        );
        assert_eq!(final_reads.load(std::sync::atomic::Ordering::SeqCst), 2);
        tokio::task::yield_now().await;
        assert_eq!(selections(&f, &c), count);
        assert!(
            f.source
                .retained_workflow_selection(&c.a.original, &cancelled_id)
                .unwrap()
                .is_none()
        );
        assert_eq!(f.inputs().await.unwrap(), input_count);
        assert_eq!(f.model_requests(), model_requests);
    }))
    .catch_unwind()
    .await;
    assert!(f.shutdown().await.is_ok());
    match result {
        Ok(result) => result.expect("timeout and cancellation control is bounded"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workflow_wait_uses_one_three_rebuild_budget_across_observation_sites() {
    let f = FirstFixture::for_continuation().await;
    let result = AssertUnwindSafe(tokio::time::timeout(GUARD, async {
        let c = prepare(&f).await;
        let count = selections(&f, &c);
        let input_count = f.inputs().await.unwrap();
        let authorization = c.a_dispatch.work_authorization().unwrap().authorization();
        let publication = f.source.owner.publication();
        let churn = tokio::sync::Mutex::new(OwnerChangeChurn::new());
        let deadline = tokio::time::Instant::now() + GUARD;
        for _ in 0..meerkat_core::authorization::OWNER_CHANGE_FREE_REBUILDS {
            let change = publication.begin_owner_change().unwrap();
            let mut pending = Box::pin(f.source.observe_workflow_for_admission(
                authorization.as_ref(),
                &churn,
                deadline,
                || {
                    c.planning
                        .check_child_preparation(&f.source, &c.planning.submission.original)
                },
            ));
            assert!(matches!(futures::poll!(&mut pending), Poll::Pending));
            drop(change);
            pending.await.unwrap();
        }
        let change = publication.begin_owner_change().unwrap();
        let mut exhausted = Box::pin(f.source.observe_workflow_for_admission(
            authorization.as_ref(),
            &churn,
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
        drop(change);
        assert_eq!(selections(&f, &c), count);
        assert_eq!(f.inputs().await.unwrap(), input_count);
    }))
    .catch_unwind()
    .await;
    assert!(f.shutdown().await.is_ok());
    match result {
        Ok(result) => result.expect("shared rebuild budget is bounded"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

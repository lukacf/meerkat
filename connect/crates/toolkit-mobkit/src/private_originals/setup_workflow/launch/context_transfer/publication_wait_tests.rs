//! Actual restored fixture P/A/G identities. These checks exercise selection
//! and canonical retention only, not a new Find or native graph admission.
use super::*;
use meerkat_core::authorization::OwnerChangeChurn;
use std::task::Poll;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn transfer_observation_waits_before_retaining_the_exact_recipient() {
    let c = Fixture::persistent_configured_waiter().await;
    let lifecycle = AcquisitionRecovery::default();
    let body = AssertUnwindSafe(tokio::time::timeout(WAIT, async {
        let data = prepare(&c).await;
        let before = native_rows(&c).await;
        let model_requests = c.f.model_requests();
        // The existing restore fixture's exact selected recipient is data,
        // not a substitute graph authorizer or an entered-operation witness.
        let mut selected = data.envelope.recipient.clone();
        selected.correlation = id("publication-wait-transfer");
        let candidate = Arc::new(InstalledWorkflowContextTransfer {
            source: Arc::downgrade(&c.f.source),
            original: Arc::downgrade(&data.original.original),
            planning: Arc::downgrade(&data.planning),
            service: data.recipient.clone(),
            selected: selected.clone(),
            attempt: Mutex::new(None),
            checkpoint: std::sync::OnceLock::new(),
        });
        let dispatch = c.f.probe.as_ref().unwrap().captured();
        let authorization = dispatch.work_authorization().unwrap().authorization();
        let publication = c.f.source.owner.publication();
        let change = publication.begin_owner_change().unwrap();
        assert_eq!(
            authorization.owner_change_in_progress(),
            publication.change_in_progress()
        );
        assert!(publication.change_in_progress().is_some());
        assert_eq!(
            candidate.check_selection(
                &c.f.source,
                &data.original,
                &selected.correlation,
                &data.planning
            ),
            Err(PrivateOriginalError::Unavailable)
        );
        assert_eq!(
            candidate.check_selection_observed(
                &c.f.source,
                &data.original,
                &selected.correlation,
                &data.planning
            ),
            Err(WorkflowChildPreparationError::ObservationChanged)
        );
        let churn = tokio::sync::Mutex::new(OwnerChangeChurn::new());
        let mut waiting = Box::pin(c.f.source.retain_workflow_binding_waiting(
            &data.original,
            &selected.correlation,
            Some(&data.planning),
            Some(&candidate),
            Some(&c.work),
            authorization.as_ref(),
            &churn,
            tokio::time::Instant::now() + WAIT,
            || async { Ok(()) },
        ));
        assert!(matches!(futures::poll!(&mut waiting), Poll::Pending));
        assert!(data.planning.context_transfers.lock().unwrap().is_empty());
        assert!(
            c.f.source
                .retained_workflow_context_transfer(&data.original.original, &selected.correlation)
                .unwrap()
                .is_none()
        );
        drop(change);
        waiting.await.unwrap();
        assert_eq!(data.planning.context_transfers.lock().unwrap().len(), 1);
        assert!(Arc::ptr_eq(
            &c.f.source
                .retained_workflow_context_transfer(&data.original.original, &selected.correlation)
                .unwrap()
                .unwrap(),
            &candidate
        ));
        assert!(
            candidate.attempt.lock().unwrap().is_none(),
            "waiting never starts a transfer"
        );
        // A coherent wrong correlation stays denied; no unrelated selection
        // gets retried merely because it is passed to the async read boundary.
        let wrong = id("wrong-transfer-correlation");
        let denied =
            c.f.source
                .observe_workflow_for_admission(
                    authorization.as_ref(),
                    &churn,
                    tokio::time::Instant::now() + WAIT,
                    || {
                        candidate.check_selection_observed(
                            &c.f.source,
                            &data.original,
                            &wrong,
                            &data.planning,
                        )
                    },
                )
                .await;
        assert_eq!(
            denied,
            Err(WorkflowChildPreparationError::Refused(
                PrivateOriginalError::Denied
            ))
        );
        assert_eq!(data.planning.context_transfers.lock().unwrap().len(), 1);
        assert_eq!(native_rows(&c).await, before);
        assert_eq!(c.f.model_requests(), model_requests);
    }))
    .catch_unwind()
    .await;
    lifecycle.stop();
    let joined = c.f.source.join_workflow_context_transfers(false).await;
    let cleanup = c.close().await;
    assert!(joined.is_ok() && cleanup.is_ok());
    match body {
        Ok(result) => result.expect("transfer observation control is bounded"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

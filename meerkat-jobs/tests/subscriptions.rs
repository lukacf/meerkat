#![allow(clippy::expect_used)]

use std::sync::Arc;

use meerkat_core::{HandlingMode, SessionId};
use meerkat_jobs::{
    AttemptClaim, CanonicalArgumentsHash, DetachedJobService, ExecutionIntentId,
    InteractionLineageId, JobDeliveryKind, JobNotification, JobPhase, JobSpec, JobSubmissionKey,
    JobSubscription, JobSubscriptionId, MemoryDetachedJobStore, RestartClass, RunnerHandleRef,
    RunnerIdentity, ToolIdentity, WorkerId,
};

fn spec(key: &str, origin: SessionId) -> JobSpec {
    JobSpec::new(
        "realm-a",
        origin,
        ExecutionIntentId::new(),
        InteractionLineageId::new(),
        ToolIdentity::new("monitor", "v1").expect("tool"),
        RunnerIdentity::new("monitor.script", "v1").expect("runner"),
        RestartClass::CheckpointResumable,
        CanonicalArgumentsHash::new("sha256:monitor").expect("hash"),
        JobSubmissionKey::new(key).expect("key"),
    )
}

#[tokio::test]
async fn subscriptions_are_durable_and_unsubscribe_only_changes_future_delivery() {
    let store = Arc::new(MemoryDetachedJobStore::new());
    let service = DetachedJobService::new(store.clone());
    let origin = SessionId::new();
    let event_session = SessionId::new();
    let job_id = service
        .submit(spec("subscription-contract", origin.clone()))
        .await
        .expect("submit")
        .job_id;

    let notification = JobSubscription::new(
        JobSubscriptionId::new("subscription-notification").expect("id"),
        origin,
        JobDeliveryKind::Notification,
    );
    let event = JobSubscription::new(
        JobSubscriptionId::new("subscription-event").expect("id"),
        event_session,
        JobDeliveryKind::Event {
            handling_mode: HandlingMode::Queue,
        },
    );
    let subscribed = service
        .subscribe(&job_id, notification.clone())
        .await
        .expect("subscribe notification");
    assert_eq!(subscribed.subscriptions, vec![notification.clone()]);
    let duplicate = service
        .subscribe(&job_id, notification.clone())
        .await
        .expect("idempotent subscribe");
    assert_eq!(duplicate.revision, subscribed.revision);
    let subscribed = service
        .subscribe(&job_id, event.clone())
        .await
        .expect("subscribe event");
    assert_eq!(
        subscribed.subscriptions,
        vec![notification.clone(), event.clone()]
    );

    let claim = service
        .claim_attempt(
            &job_id,
            AttemptClaim::new(
                WorkerId::new("worker-a").expect("worker"),
                1,
                1_000,
                RunnerHandleRef::new("monitor:live").expect("handle"),
            ),
        )
        .await
        .expect("claim");
    service
        .emit_notification(
            &job_id,
            (&claim).into(),
            2,
            JobNotification::new("n1", "observation:1", "First", "first observation")
                .expect("notification"),
        )
        .await
        .expect("first notification");

    let unsubscribed = service
        .unsubscribe(&job_id, event.subscription_id())
        .await
        .expect("unsubscribe");
    assert_eq!(unsubscribed.phase, JobPhase::Running);
    assert!(unsubscribed.terminal_result.is_none());
    assert_eq!(unsubscribed.subscriptions, vec![notification.clone()]);
    service
        .emit_notification(
            &job_id,
            (&claim).into(),
            3,
            JobNotification::new("n2", "observation:2", "Second", "second observation")
                .expect("notification"),
        )
        .await
        .expect("second notification");

    let reopened = DetachedJobService::new(Arc::new(
        MemoryDetachedJobStore::from_snapshot(store.snapshot().await).expect("reopen"),
    ));
    let recovered = reopened.get(&job_id).await.expect("get").expect("job");
    assert_eq!(recovered.subscriptions, vec![notification.clone()]);
    assert_eq!(
        recovered.outbox[0].targets,
        vec![notification.clone(), event]
    );
    assert_eq!(recovered.outbox[1].targets, recovered.subscriptions);
    assert_eq!(recovered.attempt_count, 1);
    assert_eq!(recovered.current_fence.get(), 1);
}

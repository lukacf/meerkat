#![allow(clippy::expect_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use meerkat_core::{HandlingMode, SessionId};
use meerkat_jobs::{
    AttemptClaim, AttemptWriteAuthority, CanonicalArgumentsHash, DetachedJobService,
    DetachedJobStore, ExecutionIntentId, InsertJobOutcome, InteractionLineageId, JobDeliveryKind,
    JobId, JobNotification, JobOutboxEntry, JobOutboxPayload, JobPhase, JobSpec, JobSubmissionKey,
    JobSubscription, JobSubscriptionId, MemoryDetachedJobStore, RestartClass, RunnerHandleRef,
    RunnerIdentity, StoredJob, ToolIdentity, WorkerId,
};
use tokio::sync::Notify;

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

#[tokio::test]
async fn subscribing_after_terminality_returns_the_committed_terminal_snapshot() {
    let store = Arc::new(MemoryDetachedJobStore::new());
    let service = DetachedJobService::new(store);
    let origin = SessionId::new();
    let job_id = service
        .submit(spec("late-terminal-subscription", origin.clone()))
        .await
        .expect("submit")
        .job_id;
    let claim = service
        .claim_attempt(
            &job_id,
            AttemptClaim::new(
                WorkerId::new("worker-late").expect("worker"),
                1,
                1_000,
                RunnerHandleRef::new("monitor:late").expect("handle"),
            ),
        )
        .await
        .expect("claim");
    let terminal = service
        .complete_attempt(&job_id, AttemptWriteAuthority::from(&claim), 2, None)
        .await
        .expect("complete");
    assert!(terminal.outbox[0].targets.is_empty());

    let subscription = JobSubscription::new(
        JobSubscriptionId::new("late-await").expect("subscription id"),
        origin,
        JobDeliveryKind::Record,
    );
    let subscribed = service
        .subscribe(&job_id, subscription.clone())
        .await
        .expect("late subscription must join terminal delivery");

    assert_eq!(subscribed.subscriptions, vec![subscription.clone()]);
    assert_eq!(subscribed.outbox.len(), 1);
    assert!(matches!(
        subscribed.outbox[0].payload,
        JobOutboxPayload::Terminal(_)
    ));
    assert!(
        subscribed.terminal_result.is_some(),
        "the await coordinator must be able to resolve directly from this durable snapshot"
    );
    assert!(
        subscribed.outbox[0].targets.is_empty(),
        "late subscription must not rewrite a terminal delivery that may already be projected"
    );
}

#[derive(Debug)]
struct PausingSubscriptionStore {
    inner: Arc<MemoryDetachedJobStore>,
    subscription_cas_entered: Notify,
    release_subscription_cas: Notify,
    pause_once: AtomicBool,
}

impl PausingSubscriptionStore {
    fn new(inner: Arc<MemoryDetachedJobStore>) -> Self {
        Self {
            inner,
            subscription_cas_entered: Notify::new(),
            release_subscription_cas: Notify::new(),
            pause_once: AtomicBool::new(true),
        }
    }
}

#[async_trait]
impl DetachedJobStore for PausingSubscriptionStore {
    async fn insert_deduplicated(
        &self,
        job: StoredJob,
    ) -> Result<InsertJobOutcome, meerkat_jobs::DetachedJobError> {
        self.inner.insert_deduplicated(job).await
    }

    async fn get(
        &self,
        job_id: &JobId,
    ) -> Result<Option<StoredJob>, meerkat_jobs::DetachedJobError> {
        self.inner.get(job_id).await
    }

    async fn compare_and_swap(
        &self,
        expected_revision: u64,
        replacement: StoredJob,
    ) -> Result<StoredJob, meerkat_jobs::DetachedJobError> {
        if replacement.terminal_result.is_none()
            && !replacement.subscriptions.is_empty()
            && self.pause_once.swap(false, Ordering::AcqRel)
        {
            self.subscription_cas_entered.notify_one();
            self.release_subscription_cas.notified().await;
        }
        self.inner
            .compare_and_swap(expected_revision, replacement)
            .await
    }

    async fn list_pending_outbox(
        &self,
        limit: usize,
    ) -> Result<Vec<JobOutboxEntry>, meerkat_jobs::DetachedJobError> {
        self.inner.list_pending_outbox(limit).await
    }

    async fn list_for_origin(
        &self,
        realm_id: &str,
        origin_session_id: &SessionId,
        limit: usize,
    ) -> Result<Vec<StoredJob>, meerkat_jobs::DetachedJobError> {
        self.inner
            .list_for_origin(realm_id, origin_session_id, limit)
            .await
    }

    async fn list_all(
        &self,
        limit: usize,
    ) -> Result<Vec<StoredJob>, meerkat_jobs::DetachedJobError> {
        self.inner.list_all(limit).await
    }

    fn is_persistent(&self) -> bool {
        self.inner.is_persistent()
    }
}

#[tokio::test]
async fn terminal_completion_racing_subscription_returns_authoritative_terminal_state() {
    let memory = Arc::new(MemoryDetachedJobStore::new());
    let store = Arc::new(PausingSubscriptionStore::new(memory));
    let service = DetachedJobService::new(store.clone());
    let origin = SessionId::new();
    let job_id = service
        .submit(spec("subscription-terminal-race", origin.clone()))
        .await
        .expect("submit")
        .job_id;
    let claim = service
        .claim_attempt(
            &job_id,
            AttemptClaim::new(
                WorkerId::new("worker-race").expect("worker"),
                1,
                1_000,
                RunnerHandleRef::new("monitor:race").expect("handle"),
            ),
        )
        .await
        .expect("claim");
    let subscription = JobSubscription::new(
        JobSubscriptionId::new("racing-await").expect("subscription id"),
        origin,
        JobDeliveryKind::Record,
    );

    let subscribing_service = service.clone();
    let subscribing_job_id = job_id.clone();
    let subscribing = tokio::spawn(async move {
        subscribing_service
            .subscribe(&subscribing_job_id, subscription)
            .await
    });
    store.subscription_cas_entered.notified().await;
    service
        .complete_attempt(&job_id, AttemptWriteAuthority::from(&claim), 2, None)
        .await
        .expect("completion wins the first CAS");
    store.release_subscription_cas.notify_one();

    let subscribed = subscribing
        .await
        .expect("subscription task")
        .expect("subscription retries after stale revision");
    assert!(subscribed.terminal_result.is_some());
    assert_eq!(subscribed.subscriptions.len(), 1);
}

//! Recipient settlement through the real job outbox and runtime inbox.
//!
//! The sink injects typed outcomes and records every call. It deliberately has
//! no deduplication: replay prevention must come from the delivery owner.
//! These are completed-pass restart controls, not effect/commit crash atomicity.
//! Every injected error precedes its synthetic effect; an unknown effect still
//! needs the real sink's idempotency or reconciliation before retry.

use super::*;
use meerkat::JobDeliveryContent;
use meerkat_core::approval::review::{
    OperationReviewRefusal, ReviewUnavailableKind, ReviewUnsatisfiedKind,
};
use meerkat_core::authorization::OperationObservationError;
use meerkat_core::{OperationAuthorizationError, OperationRefusalKind, OperationRefused};
use meerkat_runtime::{
    RuntimeDeliveryRecipient, RuntimeDeliveryRecipientGroupOutcome,
    RuntimeDeliveryRecipientOutcome, RuntimeDeliveryRecipientState, RuntimeDeliveryRecord,
    RuntimeDeliveryStatus, RuntimeDeliveryStoreRecord, RuntimeStore,
};

#[derive(Clone, Copy, Debug)]
enum RecipientFailure {
    Denied,
    AuthorizationUnavailable,
    ReviewR2,
    ReviewR3,
    Infrastructure,
    ObservationUnavailable,
}

impl RecipientFailure {
    fn error(self) -> JobDeliveryApplyError {
        match self {
            Self::Denied => OperationAuthorizationError::Refused(OperationRefused::new(
                OperationRefusalKind::Denied,
            ))
            .into(),
            Self::AuthorizationUnavailable => OperationAuthorizationError::Unavailable.into(),
            Self::ReviewR2 => JobDeliveryApplyError::Review(OperationReviewRefusal::Unavailable {
                kind: ReviewUnavailableKind::UnsupportedEntry,
            }),
            Self::ReviewR3 => JobDeliveryApplyError::Review(OperationReviewRefusal::Unsatisfied {
                kind: ReviewUnsatisfiedKind::HumanConsentRequired,
            }),
            Self::Infrastructure => {
                JobDeliveryApplyError::Infrastructure("recipient offline".into())
            }
            Self::ObservationUnavailable => {
                OperationAuthorizationError::ObservationUnavailable(OperationObservationError)
                    .into()
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct RecipientTrace {
    attempts: Vec<JobDeliveryApplication>,
    delivered: Vec<JobDeliveryApplication>,
}

struct RecipientSink {
    fault_job: JobId,
    failure: Option<RecipientFailure>,
    trace: Arc<std::sync::Mutex<RecipientTrace>>,
}

#[async_trait::async_trait]
impl JobDeliverySink for RecipientSink {
    async fn apply(
        &self,
        application: JobDeliveryApplication,
    ) -> Result<(), JobDeliveryApplyError> {
        let (job_id, subscription) = match &application {
            JobDeliveryApplication::Record {
                job_id,
                subscription,
                ..
            }
            | JobDeliveryApplication::Notification {
                job_id,
                subscription,
                ..
            }
            | JobDeliveryApplication::Event {
                job_id,
                subscription,
                ..
            } => (job_id, subscription),
        };
        let rejected_recipient =
            job_id == &self.fault_job && subscription.subscription_id().as_str() == "b";
        let mut trace = self.trace.lock().expect("recipient trace");
        trace.attempts.push(application.clone());
        if rejected_recipient && let Some(failure) = self.failure {
            return Err(failure.error());
        }
        trace.delivered.push(application);
        Ok(())
    }
}

enum BackingStore {
    Memory(Arc<InMemoryRuntimeStore>),
    #[cfg(feature = "sqlite-store")]
    Sqlite(tempfile::TempDir),
}

impl BackingStore {
    fn open(&self) -> Arc<dyn RuntimeStore> {
        match self {
            Self::Memory(store) => store.clone(),
            #[cfg(feature = "sqlite-store")]
            Self::Sqlite(directory) => Arc::new(
                meerkat_runtime::SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"))
                    .expect("open runtime SQLite store"),
            ),
        }
    }
}

struct RecipientsFixture {
    jobs: Arc<MemoryDetachedJobStore>,
    runtime_id: LogicalRuntimeId,
    rows: Vec<RuntimeDeliveryRecord>,
    stored_rows: Vec<RuntimeDeliveryStoreRecord>,
    producers: Vec<meerkat_jobs::StoredJob>,
    expected: Vec<JobDeliveryApplication>,
}

impl RecipientsFixture {
    fn recipient_manifest(&self) -> Vec<RuntimeDeliveryRecipient> {
        let payload: JobTerminalDeliveryPayload =
            serde_json::from_slice(self.rows[0].submission.payload()).expect("terminal payload");
        payload
            .targets
            .iter()
            .map(|subscription| {
                RuntimeDeliveryRecipient::new(
                    subscription.subscription_id().as_str(),
                    serde_json::to_string(subscription).expect("exact subscription binding"),
                )
                .expect("recipient binding")
            })
            .collect()
    }

    fn expected_states(
        &self,
        outcomes: [Option<RuntimeDeliveryRecipientOutcome>; 3],
    ) -> Vec<RuntimeDeliveryRecipientState> {
        self.recipient_manifest()
            .into_iter()
            .zip(outcomes)
            .map(|(recipient, outcome)| RuntimeDeliveryRecipientState { recipient, outcome })
            .collect()
    }

    async fn new(backing: &BackingStore) -> Self {
        let job_store = Arc::new(MemoryDetachedJobStore::new());
        let jobs = DetachedJobService::new(job_store.clone());
        let origin = SessionId::new();
        let runtime_id = LogicalRuntimeId::for_session(&origin);
        let store = backing.open();
        let inbox = RuntimeDeliveryInbox::new(store.clone());
        let projector = JobOutboxProjector::new(job_store.clone(), inbox.clone());
        let mut producers = Vec::new();

        // Project each completed job before submitting the next, so runtime
        // row order does not depend on the job store's enumeration order.
        for (key, recipients) in [
            ("mixed-recipients", vec!["a", "b", "c"]),
            ("following-job", vec!["next"]),
        ] {
            let receipt = jobs
                .submit(spec(key, origin.clone()))
                .await
                .expect("submit job");
            for recipient in recipients {
                jobs.subscribe(
                    &receipt.job_id,
                    JobSubscription::new(
                        JobSubscriptionId::new(recipient).expect("subscription id"),
                        SessionId::new(),
                        JobDeliveryKind::Notification,
                    ),
                )
                .await
                .expect("commit subscription");
            }
            let claim = jobs
                .claim_attempt(
                    &receipt.job_id,
                    AttemptClaim::new(
                        WorkerId::new("recipient-worker").expect("worker"),
                        1,
                        100,
                        RunnerHandleRef::new("recipient-handle").expect("handle"),
                    ),
                )
                .await
                .expect("claim job");
            jobs.complete_attempt(
                &receipt.job_id,
                (&claim).into(),
                2,
                Some(JobResultRef::new(format!("result-{key}")).expect("result")),
            )
            .await
            .expect("commit terminal producer result");
            let projected = projector
                .project_pending(10)
                .await
                .expect("project real outbox");
            assert!(projected.is_fully_projected());
            assert_eq!(projected.projected.len(), 1);
            producers.push(
                job_store
                    .get(&receipt.job_id)
                    .await
                    .expect("read committed job")
                    .expect("job"),
            );
        }

        let rows = inbox
            .list_pending(&runtime_id, 10)
            .await
            .expect("two pending rows");
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].sequence, rows[1].sequence), (1, 2));
        let mut expected = Vec::new();
        let mut stored_rows = Vec::new();
        for (index, row) in rows.iter().enumerate() {
            let payload: JobTerminalDeliveryPayload =
                serde_json::from_slice(row.submission.payload()).expect("terminal payload");
            assert_eq!(payload.job_id, producers[index].job_id);
            assert_eq!(
                Some(&payload.terminal_result),
                producers[index].terminal_result.as_ref()
            );
            assert_eq!(payload.targets, producers[index].subscriptions);
            let ids: Vec<_> = payload
                .targets
                .iter()
                .map(|target| target.subscription_id().as_str())
                .collect();
            assert_eq!(
                ids,
                if index == 0 {
                    vec!["a", "b", "c"]
                } else {
                    vec!["next"]
                }
            );
            for subscription in payload.targets {
                expected.push(JobDeliveryApplication::Notification {
                    job_id: payload.job_id.clone(),
                    delivery_sequence: payload.delivery_sequence,
                    subscription,
                    content: JobDeliveryContent::Terminal(payload.terminal_result.clone()),
                });
            }
            stored_rows.push(
                store
                    .load_runtime_delivery_record(
                        &runtime_id,
                        row.submission.delivery_id().as_str(),
                    )
                    .await
                    .expect("read durable delivery")
                    .expect("durable row"),
            );
        }
        // Projector, inbox and runtime-store handles all drop before return.
        // Only the in-memory job producer store survives this runtime reopen.
        Self {
            jobs: job_store,
            runtime_id,
            rows,
            stored_rows,
            producers,
            expected,
        }
    }

    async fn assert_sources_unchanged(&self, store: &dyn RuntimeStore) {
        for (row, before) in self.rows.iter().zip(&self.stored_rows) {
            assert_eq!(
                store
                    .load_runtime_delivery_record(
                        &self.runtime_id,
                        row.submission.delivery_id().as_str(),
                    )
                    .await
                    .expect("read preserved delivery")
                    .as_ref(),
                Some(before),
                "recipient settlement must preserve exact committed payload bytes"
            );
        }
        for before in &self.producers {
            assert_eq!(
                self.jobs
                    .get(&before.job_id)
                    .await
                    .expect("read producer")
                    .as_ref(),
                Some(before),
                "recipient refusal must not rewrite successful producer truth"
            );
        }
    }
}

struct ObservedPass {
    drain: meerkat::RuntimeJobDeliveryDrain,
    pending: Vec<RuntimeDeliveryRecord>,
    cursor: u64,
    recipient_states: Vec<RuntimeDeliveryRecipientState>,
    statuses: Vec<RuntimeDeliveryStatus>,
}

async fn apply_once(
    backing: &BackingStore,
    fixture: &RecipientsFixture,
    trace: &Arc<std::sync::Mutex<RecipientTrace>>,
    failure: Option<RecipientFailure>,
) -> ObservedPass {
    let store = backing.open();
    let inbox = RuntimeDeliveryInbox::new(store.clone());
    let sink = Arc::new(RecipientSink {
        fault_job: fixture.producers[0].job_id.clone(),
        failure,
        trace: trace.clone(),
    });
    let applier = JobRuntimeDeliveryApplier::new(inbox.clone(), sink);
    let drain = applier
        .apply_pending(&fixture.runtime_id, 10)
        .await
        .expect("drain result");
    let pending = inbox
        .list_pending(&fixture.runtime_id, 10)
        .await
        .expect("pending rows");
    let cursor = inbox
        .applied_cursor(&fixture.runtime_id)
        .await
        .expect("durable cursor");
    // Repeating the exact binding reads its retained dispositions through a
    // new inbox, without invoking the sink or changing any settled outcome.
    let observer = RuntimeDeliveryInbox::new(store.clone());
    let recipient_states = observer
        .bind_recipients(
            &fixture.runtime_id,
            &fixture.rows[0],
            &fixture.recipient_manifest(),
        )
        .await
        .expect("read exact retained recipient state");
    let mut statuses = Vec::new();
    for row in &fixture.rows {
        statuses.push(
            observer
                .delivery_status(&fixture.runtime_id, row.submission.delivery_id())
                .await
                .expect("read exact parent status"),
        );
    }
    fixture.assert_sources_unchanged(&*store).await;
    // No runtime-store, inbox, applier or sink handle escapes this pass.
    ObservedPass {
        drain,
        pending,
        cursor,
        recipient_states,
        statuses,
    }
}

fn assert_exact_applications(
    actual: &[JobDeliveryApplication],
    expected: &[JobDeliveryApplication],
) {
    assert_eq!(
        actual.len(),
        expected.len(),
        "no lost or duplicate recipient effects"
    );
    for application in expected {
        assert_eq!(
            actual.iter().filter(|item| *item == application).count(),
            1,
            "each exact recipient effect must occur once: {application:?}"
        );
    }
}

async fn local_refusal_case(backing: BackingStore, failure: RecipientFailure) {
    let fixture = RecipientsFixture::new(&backing).await;
    let trace = Arc::new(std::sync::Mutex::new(RecipientTrace::default()));
    let first = apply_once(&backing, &fixture, &trace, Some(failure)).await;
    let rejected = match failure {
        RecipientFailure::Denied | RecipientFailure::ReviewR2 | RecipientFailure::ReviewR3 => {
            Some(RuntimeDeliveryRecipientOutcome::Refused)
        }
        RecipientFailure::AuthorizationUnavailable => {
            Some(RuntimeDeliveryRecipientOutcome::OperationAuthorizationUnavailable)
        }
        _ => None,
    }
    .expect("this control requires a local recipient disposition");
    let expected_states = fixture.expected_states([
        Some(RuntimeDeliveryRecipientOutcome::Applied),
        Some(rejected),
        Some(RuntimeDeliveryRecipientOutcome::Applied),
    ]);
    assert_eq!(first.recipient_states, expected_states);
    assert_eq!(
        first.statuses,
        vec![
            RuntimeDeliveryStatus::Mixed {
                delivery_sequence: fixture.rows[0].sequence,
                recipients: expected_states.clone(),
            },
            RuntimeDeliveryStatus::Applied {
                delivery_sequence: fixture.rows[1].sequence,
            },
        ]
    );
    assert_eq!(
        first.drain.locally_settled,
        vec![meerkat::LocallySettledRuntimeJobDelivery {
            runtime_id: fixture.runtime_id.clone(),
            delivery_id: fixture.rows[0].submission.delivery_id().clone(),
            runtime_sequence: fixture.rows[0].sequence,
            outcome: RuntimeDeliveryRecipientGroupOutcome::Mixed,
            recipients: expected_states,
        }]
    );
    let observed = trace.lock().expect("trace").clone();
    assert_exact_applications(
        &observed.delivered,
        &[
            fixture.expected[0].clone(),
            fixture.expected[2].clone(),
            fixture.expected[3].clone(),
        ],
    );
    assert_exact_applications(&observed.attempts, &fixture.expected);
    assert!(
        first.drain.blocked.is_none(),
        "local refusal must not block later work"
    );
    assert_eq!(
        first.drain.applied.len(),
        1,
        "mixed row is not wholly Applied"
    );
    assert_eq!(
        first.drain.applied[0].delivery_id,
        *fixture.rows[1].submission.delivery_id()
    );
    assert!(
        first.pending.is_empty(),
        "every recipient has a terminal local disposition"
    );
    assert_eq!(first.cursor, fixture.rows[1].sequence);

    // Memory reuses the backing store with a new inbox/applier. SQLite has no
    // remaining connection handles here and physically reopens the database.
    // Healing B must not reinterpret its previously settled refusal as success.
    let reopened = apply_once(&backing, &fixture, &trace, None).await;
    assert!(reopened.drain.is_fully_drained());
    assert!(reopened.drain.applied.is_empty());
    assert!(reopened.drain.locally_settled.is_empty());
    assert!(reopened.pending.is_empty());
    assert_eq!(reopened.cursor, first.cursor);
    assert_eq!(reopened.recipient_states, first.recipient_states);
    assert_eq!(reopened.statuses, first.statuses);
    assert_eq!(*trace.lock().expect("trace after reopen"), observed);
}

async fn retryable_failure_case(backing: BackingStore, failure: RecipientFailure) {
    let fixture = RecipientsFixture::new(&backing).await;
    let trace = Arc::new(std::sync::Mutex::new(RecipientTrace::default()));
    let first = apply_once(&backing, &fixture, &trace, Some(failure)).await;
    assert!(first.drain.applied.is_empty());
    assert!(first.drain.locally_settled.is_empty());
    assert_eq!(
        first.recipient_states,
        fixture.expected_states([Some(RuntimeDeliveryRecipientOutcome::Applied), None, None,])
    );
    assert_eq!(
        first.statuses,
        vec![
            RuntimeDeliveryStatus::Pending {
                delivery_sequence: fixture.rows[0].sequence
            },
            RuntimeDeliveryStatus::Pending {
                delivery_sequence: fixture.rows[1].sequence
            },
        ]
    );
    assert_eq!(
        first
            .drain
            .blocked
            .as_ref()
            .expect("retryable failure")
            .delivery_id,
        *fixture.rows[0].submission.delivery_id()
    );
    assert_eq!(
        first.pending, fixture.rows,
        "uncertain failure must remain pending"
    );
    assert_eq!(
        first.cursor, 0,
        "unsettled failure must not become a refusal or Applied"
    );
    let observed = trace.lock().expect("trace").clone();
    assert_eq!(
        observed
            .delivered
            .iter()
            .filter(|item| **item == fixture.expected[0])
            .count(),
        1
    );
    assert!(!observed.delivered.contains(&fixture.expected[1]));
    assert!(!observed.delivered.contains(&fixture.expected[3]));
    assert_eq!(
        observed
            .attempts
            .iter()
            .filter(|item| **item == fixture.expected[1])
            .count(),
        1
    );

    let healed = apply_once(&backing, &fixture, &trace, None).await;
    assert!(healed.drain.is_fully_drained());
    assert!(healed.drain.locally_settled.is_empty());
    assert_eq!(
        healed.recipient_states,
        fixture.expected_states([Some(RuntimeDeliveryRecipientOutcome::Applied); 3])
    );
    assert_eq!(
        healed.statuses,
        vec![
            RuntimeDeliveryStatus::Applied {
                delivery_sequence: fixture.rows[0].sequence,
            },
            RuntimeDeliveryStatus::Applied {
                delivery_sequence: fixture.rows[1].sequence,
            },
        ]
    );
    assert_eq!(healed.drain.applied.len(), 2);
    assert_eq!(
        healed.drain.applied[0].delivery_id,
        *fixture.rows[0].submission.delivery_id()
    );
    assert_eq!(
        healed.drain.applied[1].delivery_id,
        *fixture.rows[1].submission.delivery_id()
    );
    assert!(healed.pending.is_empty());
    assert_eq!(healed.cursor, fixture.rows[1].sequence);
    let settled = trace.lock().expect("healed trace").clone();
    assert_exact_applications(&settled.delivered, &fixture.expected);
    assert_eq!(
        settled
            .attempts
            .iter()
            .filter(|item| **item == fixture.expected[0])
            .count(),
        1,
        "successful A is durably settled before B's retry"
    );
    assert_eq!(
        settled
            .attempts
            .iter()
            .filter(|item| **item == fixture.expected[1])
            .count(),
        2,
        "B's unresolved error is retried, not falsely settled as refusal"
    );

    let reopened = apply_once(&backing, &fixture, &trace, None).await;
    assert!(reopened.drain.applied.is_empty());
    assert!(reopened.drain.locally_settled.is_empty());
    assert!(reopened.drain.is_fully_drained());
    assert!(reopened.pending.is_empty());
    assert_eq!(reopened.cursor, healed.cursor);
    assert_eq!(reopened.recipient_states, healed.recipient_states);
    assert_eq!(reopened.statuses, healed.statuses);
    assert_eq!(*trace.lock().expect("settled trace"), settled);
}

#[tokio::test]
async fn memory_recipient_refusals_settle_locally_and_survive_new_inbox() {
    for failure in [
        RecipientFailure::Denied,
        RecipientFailure::AuthorizationUnavailable,
    ] {
        local_refusal_case(
            BackingStore::Memory(Arc::new(InMemoryRuntimeStore::new())),
            failure,
        )
        .await;
    }
}

#[tokio::test]
async fn memory_recipient_infrastructure_failures_retry_without_reapplying_successes() {
    for failure in [
        RecipientFailure::Infrastructure,
        RecipientFailure::ObservationUnavailable,
    ] {
        retryable_failure_case(
            BackingStore::Memory(Arc::new(InMemoryRuntimeStore::new())),
            failure,
        )
        .await;
    }
}

#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn sqlite_recipient_refusals_survive_close_reopen_and_permission_healing() {
    for failure in [
        RecipientFailure::Denied,
        RecipientFailure::AuthorizationUnavailable,
    ] {
        local_refusal_case(
            BackingStore::Sqlite(tempfile::tempdir().expect("tempdir")),
            failure,
        )
        .await;
    }
}

#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn sqlite_recipient_infrastructure_failures_preserve_successes_across_reopen() {
    for failure in [
        RecipientFailure::Infrastructure,
        RecipientFailure::ObservationUnavailable,
    ] {
        retryable_failure_case(
            BackingStore::Sqlite(tempfile::tempdir().expect("tempdir")),
            failure,
        )
        .await;
    }
}

#[tokio::test]
async fn memory_review_refusals_settle_only_recipient_and_preserve_following_work() {
    for failure in [RecipientFailure::ReviewR2, RecipientFailure::ReviewR3] {
        local_refusal_case(
            BackingStore::Memory(Arc::new(InMemoryRuntimeStore::new())),
            failure,
        )
        .await;
    }
}

#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn sqlite_review_refusals_stay_settled_after_reopen_and_review_healing() {
    for failure in [RecipientFailure::ReviewR2, RecipientFailure::ReviewR3] {
        local_refusal_case(
            BackingStore::Sqlite(tempfile::tempdir().expect("tempdir")),
            failure,
        )
        .await;
    }
}

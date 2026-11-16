#![allow(clippy::expect_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use chrono::Utc;
use meerkat::{
    AttemptClaim, AttemptWriteAuthority, CanonicalArgumentsHash, CreateWorkItemRequest,
    DetachedJobError, DetachedJobService, DetachedJobStore, ExecutionIntentId, HostRunnable,
    HostRunnableInvocation, HostRunnableName, InteractionLineageId, JobAwaitCoordinator,
    JobAwaitDeliverySink, JobDeliveryApplication, JobDeliveryApplyError, JobDeliveryContent,
    JobDeliveryKind, JobDeliverySink, JobFailureCode, JobId, JobOutboxEntry, JobReference,
    JobSubmissionKey, JobSubscription, JobSubscriptionId, JobTerminalEvidenceKind,
    JobTerminalEvidenceProjection, JobTerminalEvidenceProjector, JobTerminalResult,
    JobWorkGraphLink, MemoryDetachedJobStore, MemoryWorkGraphStore, RestartClass, RunnerHandleRef,
    RunnerIdentity, ScheduledDurableJobRunnable, ScheduledJobTemplate, SessionId, ToolIdentity,
    WorkGraphService, WorkItemRef, WorkNamespace, WorkerId,
};
use meerkat_core::ops_lifecycle::{
    OperationKind, OperationSource, OperationSpec, OperationStatus, OpsLifecycleRegistry,
};
use meerkat_jobs::{InsertJobOutcome, StoredJob};
use meerkat_runtime::RuntimeOpsLifecycleRegistry;
use meerkat_schedule::{OccurrenceId, ScheduleId};
use tokio::sync::{Mutex, Notify};

fn job_spec(key: &str, session_id: SessionId) -> meerkat::JobSpec {
    meerkat::JobSpec::new(
        "realm-a",
        session_id,
        ExecutionIntentId::new(),
        InteractionLineageId::new(),
        ToolIdentity::new("scan", "v1").expect("tool"),
        RunnerIdentity::new("runner.scan", "v1").expect("runner"),
        RestartClass::Adoptable,
        CanonicalArgumentsHash::new("sha256:scan").expect("hash"),
        JobSubmissionKey::new(key).expect("submission key"),
    )
}

async fn terminal_job(
    service: &DetachedJobService,
    key: &str,
    session_id: SessionId,
    result: JobTerminalResult,
) -> JobReference {
    let mut spec = job_spec(key, session_id);
    if matches!(&result, JobTerminalResult::WorkerLost) {
        spec.restart_class = RestartClass::NonResumable;
    }
    let receipt = service.submit(spec).await.expect("submit");
    if let JobTerminalResult::NeedsAttention { reason } = &result {
        service
            .mark_needs_attention(&receipt.job_id, 2, reason.clone())
            .await
            .expect("mark attention");
        return JobReference::new("realm-a", receipt.job_id).expect("reference");
    }
    let claim = service
        .claim_attempt(
            &receipt.job_id,
            AttemptClaim::new(
                WorkerId::new(format!("worker-{key}")).expect("worker"),
                1,
                1_000,
                RunnerHandleRef::new(format!("runner:{key}")).expect("handle"),
            ),
        )
        .await
        .expect("claim");
    let write = AttemptWriteAuthority::from(&claim);
    match result {
        JobTerminalResult::Succeeded { result_ref } => {
            service
                .complete_attempt(&receipt.job_id, write, 2, result_ref)
                .await
                .expect("complete");
        }
        JobTerminalResult::Failed { code, detail_ref } => {
            service
                .fail_attempt(&receipt.job_id, write, 2, code, detail_ref)
                .await
                .expect("fail");
        }
        JobTerminalResult::Cancelled => {
            service
                .request_cancel(&receipt.job_id)
                .await
                .expect("request cancel");
            service
                .acknowledge_cancel(&receipt.job_id, write, 2)
                .await
                .expect("acknowledge cancel");
        }
        JobTerminalResult::WorkerLost => {
            service
                .observe_lease_expired(&receipt.job_id, write, 2_000)
                .await
                .expect("observe lease loss");
            service
                .classify_worker_loss(&receipt.job_id, 2_001)
                .await
                .expect("classify loss");
        }
        JobTerminalResult::NeedsAttention { .. } => unreachable!("handled before claim"),
    }
    JobReference::new("realm-a", receipt.job_id).expect("reference")
}

#[tokio::test]
async fn schedule_redelivery_ensures_one_job_per_occurrence_and_returns_after_acceptance() {
    let store = Arc::new(MemoryDetachedJobStore::new());
    let jobs = DetachedJobService::new(store.clone());
    let session_id = SessionId::new();
    let template = ScheduledJobTemplate::new(
        "realm-a",
        session_id,
        ToolIdentity::new("scan", "v1").expect("tool"),
        RunnerIdentity::new("runner.scan", "v1").expect("runner"),
        RestartClass::Adoptable,
        CanonicalArgumentsHash::new("sha256:scheduled-scan").expect("hash"),
    )
    .expect("template");
    let runnable = ScheduledDurableJobRunnable::new(jobs, template.clone());
    let invocation = HostRunnableInvocation {
        occurrence_id: OccurrenceId::new(),
        schedule_id: ScheduleId::new(),
        delivery_idempotency_key: "scheduled-job-test:one-occurrence".to_string(),
        runnable: HostRunnableName::parse("long-scan").expect("runnable name"),
        trigger_time: Utc::now(),
        params: None,
    };

    runnable
        .run(invocation.clone())
        .await
        .expect("first delivery");
    let reopened_store = Arc::new(
        MemoryDetachedJobStore::from_snapshot(store.snapshot().await).expect("reopen after crash"),
    );
    let reopened =
        ScheduledDurableJobRunnable::new(DetachedJobService::new(reopened_store.clone()), template);
    reopened
        .run(invocation)
        .await
        .expect("redelivery after commit-before-ack crash");

    let stored = reopened_store.list_all(10).await.expect("list jobs");
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].machine_state.attempt_count, 0);
    assert_eq!(stored[0].machine_state.current_fence, 0);
}

#[tokio::test]
async fn terminal_evidence_is_exactly_once_and_maps_every_terminal_class() {
    let jobs = DetachedJobService::new(Arc::new(MemoryDetachedJobStore::new()));
    let graph = WorkGraphService::with_scope(
        Arc::new(MemoryWorkGraphStore::new()),
        "realm-a",
        WorkNamespace::default(),
    );
    let item = graph
        .create(CreateWorkItemRequest {
            title: "Durable scan".into(),
            ..CreateWorkItemRequest::default()
        })
        .await
        .expect("create work item");
    let work_ref = WorkItemRef {
        realm_id: item.realm_id.clone(),
        namespace: item.namespace.clone(),
        item_id: item.id.clone(),
    };
    let projector = JobTerminalEvidenceProjector::new(jobs.clone(), graph.clone());
    let cases = [
        (
            JobTerminalResult::Succeeded { result_ref: None },
            JobTerminalEvidenceKind::Succeeded,
        ),
        (
            JobTerminalResult::Failed {
                code: JobFailureCode::new("scan_failed").expect("code"),
                detail_ref: None,
            },
            JobTerminalEvidenceKind::Failed,
        ),
        (
            JobTerminalResult::Cancelled,
            JobTerminalEvidenceKind::Cancelled,
        ),
        (
            JobTerminalResult::WorkerLost,
            JobTerminalEvidenceKind::WorkerLost,
        ),
        (
            JobTerminalResult::NeedsAttention {
                reason: JobFailureCode::new("credentials_blocked").expect("reason"),
            },
            JobTerminalEvidenceKind::NeedsAttention,
        ),
    ];

    for (index, (terminal, expected_kind)) in cases.into_iter().enumerate() {
        let job = terminal_job(
            &jobs,
            &format!("evidence-{index}"),
            SessionId::new(),
            terminal.clone(),
        )
        .await;
        let link = JobWorkGraphLink::new(job, work_ref.clone()).expect("link");
        let (left, right) = tokio::join!(
            projector.project_terminal(&link, &terminal),
            projector.project_terminal(&link, &terminal)
        );
        let left = left.expect("left projection");
        let right = right.expect("right projection");
        assert!(
            matches!(
                (left, right),
                (
                    JobTerminalEvidenceProjection::Added(left_kind),
                    JobTerminalEvidenceProjection::AlreadyPresent(right_kind)
                ) | (
                    JobTerminalEvidenceProjection::AlreadyPresent(left_kind),
                    JobTerminalEvidenceProjection::Added(right_kind)
                ) if left_kind == expected_kind && right_kind == expected_kind
            ),
            "one racing projection must add and the other must observe the exact evidence"
        );
    }

    let refreshed = graph
        .get(None, None, item.id)
        .await
        .expect("read work item");
    assert_eq!(refreshed.evidence_refs.len(), 5);
}

#[tokio::test]
async fn workgraph_is_optional_for_detached_execution() {
    let jobs = DetachedJobService::new(Arc::new(MemoryDetachedJobStore::new()));
    let terminal = JobTerminalResult::Succeeded { result_ref: None };
    let job = terminal_job(
        &jobs,
        "without-workgraph",
        SessionId::new(),
        terminal.clone(),
    )
    .await;
    let projector = JobTerminalEvidenceProjector::disabled(jobs);
    let link = JobWorkGraphLink::new(
        job,
        WorkItemRef {
            realm_id: "realm-a".into(),
            namespace: WorkNamespace::default(),
            item_id: meerkat::WorkItemId::new("work-disabled").expect("work id"),
        },
    )
    .expect("link");
    assert_eq!(
        projector
            .project_terminal(&link, &terminal)
            .await
            .expect("disabled projection"),
        JobTerminalEvidenceProjection::WorkGraphDisabled
    );
}

#[tokio::test]
async fn await_job_is_durable_authorized_and_reopen_does_not_advance_job_fencing() {
    let job_store = Arc::new(MemoryDetachedJobStore::new());
    let jobs = DetachedJobService::new(job_store.clone());
    let session_id = SessionId::new();
    let receipt = jobs
        .submit(job_spec("await-recovery", session_id.clone()))
        .await
        .expect("submit");
    let reference = JobReference::new("realm-a", receipt.job_id.clone()).expect("reference");
    let ops = Arc::new(RuntimeOpsLifecycleRegistry::new());
    let coordinator = JobAwaitCoordinator::new("realm-a", jobs.clone(), ops.clone());

    let waiting = coordinator
        .await_job(&session_id, &reference)
        .await
        .expect("bind wait");
    assert!(!waiting.already_terminal);
    assert_eq!(
        ops.snapshot(&waiting.operation_id)
            .expect("snapshot")
            .expect("wait")
            .status,
        OperationStatus::Running
    );

    let before = jobs.get(&receipt.job_id).await.expect("read").expect("job");
    let job_snapshot = job_store.snapshot().await;
    let ops_snapshot = ops
        .capture_persistence_snapshot(
            meerkat_core::RuntimeEpochId::new(),
            &meerkat_core::EpochCursorState::new(),
        )
        .expect("ops persistence snapshot");
    let reopened_job_store =
        Arc::new(MemoryDetachedJobStore::from_snapshot(job_snapshot).expect("reopen jobs"));
    let reopened_jobs = DetachedJobService::new(reopened_job_store);
    let reopened_ops =
        Arc::new(RuntimeOpsLifecycleRegistry::from_recovered(ops_snapshot).expect("reopen ops"));
    let reopened = JobAwaitCoordinator::new("realm-a", reopened_jobs.clone(), reopened_ops.clone());
    let rebound = reopened
        .await_job(&session_id, &reference)
        .await
        .expect("rebind wait");

    assert_eq!(rebound.operation_id, waiting.operation_id);
    assert_eq!(
        reopened_ops
            .snapshot(&rebound.operation_id)
            .expect("snapshot")
            .expect("wait")
            .status,
        OperationStatus::Running
    );
    let after = reopened_jobs
        .get(&receipt.job_id)
        .await
        .expect("read")
        .expect("job");
    assert_eq!(after.attempt_count, before.attempt_count);
    assert_eq!(after.current_fence, before.current_fence);

    let foreign = JobReference::new("realm-b", receipt.job_id.clone()).expect("foreign ref");
    assert!(
        reopened.await_job(&session_id, &foreign).await.is_err(),
        "cross-realm references fail closed"
    );
    assert!(
        reopened
            .await_job(&SessionId::new(), &reference)
            .await
            .is_err(),
        "cross-session references fail closed"
    );
}

#[tokio::test]
async fn already_terminal_and_all_terminal_mappings_close_the_wait_binding() {
    let cases = [
        JobTerminalResult::Succeeded { result_ref: None },
        JobTerminalResult::Failed {
            code: JobFailureCode::new("failed").expect("code"),
            detail_ref: None,
        },
        JobTerminalResult::Cancelled,
        JobTerminalResult::WorkerLost,
        JobTerminalResult::NeedsAttention {
            reason: JobFailureCode::new("attention").expect("reason"),
        },
    ];

    for (index, terminal) in cases.into_iter().enumerate() {
        let store = Arc::new(MemoryDetachedJobStore::new());
        let jobs = DetachedJobService::new(store);
        let session_id = SessionId::new();
        let reference = terminal_job(
            &jobs,
            &format!("await-terminal-{index}"),
            session_id.clone(),
            terminal,
        )
        .await;
        let ops = Arc::new(RuntimeOpsLifecycleRegistry::new());
        let coordinator = JobAwaitCoordinator::new("realm-a", jobs, ops.clone());
        let receipt = coordinator
            .await_job(&session_id, &reference)
            .await
            .expect("await terminal job");
        let operation = ops
            .snapshot(&receipt.operation_id)
            .expect("snapshot")
            .expect("operation");

        assert!(receipt.already_terminal);
        assert!(operation.terminal);
        assert_ne!(operation.status, OperationStatus::Running);
    }
}

#[derive(Debug)]
struct PausingFirstReadStore {
    inner: Arc<MemoryDetachedJobStore>,
    first_read_captured: Notify,
    release_first_read: Notify,
    pause_once: AtomicBool,
}

impl PausingFirstReadStore {
    fn new(inner: Arc<MemoryDetachedJobStore>) -> Self {
        Self {
            inner,
            first_read_captured: Notify::new(),
            release_first_read: Notify::new(),
            pause_once: AtomicBool::new(true),
        }
    }
}

#[async_trait]
impl DetachedJobStore for PausingFirstReadStore {
    async fn insert_deduplicated(
        &self,
        job: StoredJob,
    ) -> Result<InsertJobOutcome, DetachedJobError> {
        self.inner.insert_deduplicated(job).await
    }

    async fn get(&self, job_id: &JobId) -> Result<Option<StoredJob>, DetachedJobError> {
        let snapshot = self.inner.get(job_id).await?;
        if self.pause_once.swap(false, Ordering::AcqRel) {
            self.first_read_captured.notify_one();
            self.release_first_read.notified().await;
        }
        Ok(snapshot)
    }

    async fn compare_and_swap(
        &self,
        expected_revision: u64,
        replacement: StoredJob,
    ) -> Result<StoredJob, DetachedJobError> {
        self.inner
            .compare_and_swap(expected_revision, replacement)
            .await
    }

    async fn list_pending_outbox(
        &self,
        limit: usize,
    ) -> Result<Vec<JobOutboxEntry>, DetachedJobError> {
        self.inner.list_pending_outbox(limit).await
    }

    async fn list_for_origin(
        &self,
        realm_id: &str,
        origin_session_id: &SessionId,
        limit: usize,
    ) -> Result<Vec<StoredJob>, DetachedJobError> {
        self.inner
            .list_for_origin(realm_id, origin_session_id, limit)
            .await
    }

    async fn list_all(&self, limit: usize) -> Result<Vec<StoredJob>, DetachedJobError> {
        self.inner.list_all(limit).await
    }

    async fn count_pending_outbox_jobs(
        &self,
        realm_id: Option<&str>,
    ) -> Result<u64, DetachedJobError> {
        self.inner.count_pending_outbox_jobs(realm_id).await
    }

    async fn list_census_candidates(
        &self,
        realm_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<StoredJob>, DetachedJobError> {
        self.inner.list_census_candidates(realm_id, limit).await
    }

    fn is_persistent(&self) -> bool {
        self.inner.is_persistent()
    }

    fn outbox_commit_signal(&self) -> meerkat::JobOutboxCommitSignal {
        self.inner.outbox_commit_signal()
    }
}

#[tokio::test]
async fn completion_racing_await_registration_resolves_from_latest_committed_truth() {
    let memory = Arc::new(MemoryDetachedJobStore::new());
    let base_jobs = DetachedJobService::new(memory.clone());
    let session_id = SessionId::new();
    let receipt = base_jobs
        .submit(job_spec("await-registration-race", session_id.clone()))
        .await
        .expect("submit");
    let claim = base_jobs
        .claim_attempt(
            &receipt.job_id,
            AttemptClaim::new(
                WorkerId::new("worker-await-race").expect("worker"),
                1,
                1_000,
                RunnerHandleRef::new("runner:await-race").expect("handle"),
            ),
        )
        .await
        .expect("claim");
    let reference = JobReference::new("realm-a", receipt.job_id.clone()).expect("reference");
    let racing_store = Arc::new(PausingFirstReadStore::new(memory));
    let operations = Arc::new(RuntimeOpsLifecycleRegistry::new());
    let coordinator = JobAwaitCoordinator::new(
        "realm-a",
        DetachedJobService::new(racing_store.clone()),
        operations.clone(),
    );

    let waiting_coordinator = coordinator.clone();
    let waiting_session = session_id.clone();
    let waiting_reference = reference.clone();
    let waiting = tokio::spawn(async move {
        waiting_coordinator
            .await_job(&waiting_session, &waiting_reference)
            .await
    });
    racing_store.first_read_captured.notified().await;
    base_jobs
        .complete_attempt(
            &receipt.job_id,
            AttemptWriteAuthority::from(&claim),
            2,
            None,
        )
        .await
        .expect("terminal commits while await holds a stale read");
    racing_store.release_first_read.notify_one();

    let await_receipt = waiting
        .await
        .expect("join")
        .expect("await registration closes the race");
    assert!(await_receipt.already_terminal);
    assert!(
        operations
            .snapshot(&await_receipt.operation_id)
            .expect("snapshot")
            .expect("wait operation")
            .terminal
    );
}

#[derive(Default)]
struct CountingDeliverySink {
    applications: Mutex<usize>,
}

struct RefusingDeliverySink(JobDeliveryApplyError);

#[async_trait]
impl JobDeliverySink for RefusingDeliverySink {
    async fn apply(
        &self,
        _application: JobDeliveryApplication,
    ) -> Result<(), JobDeliveryApplyError> {
        Err(self.0.clone())
    }
}

#[tokio::test]
async fn terminal_delivery_preserves_local_authorization_failures_and_completed_job_truth() {
    use meerkat_core::authorization::{
        OperationAuthorizationError, OperationObservationError, OperationRefusalKind,
        OperationRefused,
    };

    for expected in [
        JobDeliveryApplyError::Authorization(OperationAuthorizationError::Refused(
            OperationRefused::new(OperationRefusalKind::Denied),
        )),
        JobDeliveryApplyError::Authorization(OperationAuthorizationError::Unavailable),
        JobDeliveryApplyError::Authorization(OperationAuthorizationError::ObservationUnavailable(
            OperationObservationError,
        )),
        JobDeliveryApplyError::Infrastructure("session store unavailable".into()),
    ] {
        let jobs = DetachedJobService::new(Arc::new(MemoryDetachedJobStore::new()));
        let session_id = SessionId::new();
        let receipt = jobs
            .submit(job_spec("typed-delivery-failure", session_id.clone()))
            .await
            .expect("submit");
        let claim = jobs
            .claim_attempt(
                &receipt.job_id,
                AttemptClaim::new(
                    WorkerId::new("typed-delivery-worker").expect("worker"),
                    1,
                    1_000,
                    RunnerHandleRef::new("runner:typed-delivery").expect("handle"),
                ),
            )
            .await
            .expect("claim");
        let reference = JobReference::new("realm-a", receipt.job_id.clone()).expect("reference");
        let operations = Arc::new(RuntimeOpsLifecycleRegistry::new());
        let coordinator = JobAwaitCoordinator::new("realm-a", jobs.clone(), operations.clone());
        let waiting = coordinator
            .await_job(&session_id, &reference)
            .await
            .expect("await live job");
        let terminal = jobs
            .complete_attempt(
                &receipt.job_id,
                AttemptWriteAuthority::from(&claim),
                2,
                None,
            )
            .await
            .expect("complete job")
            .terminal_result
            .expect("terminal result");
        let application = JobDeliveryApplication::Notification {
            job_id: receipt.job_id,
            delivery_sequence: 1,
            subscription: JobSubscription::new(
                JobSubscriptionId::new("typed-delivery-subscriber").expect("subscription"),
                session_id,
                JobDeliveryKind::Notification,
            ),
            content: JobDeliveryContent::Terminal(terminal),
        };
        let sink = JobAwaitDeliverySink::new(
            coordinator,
            Arc::new(RefusingDeliverySink(expected.clone())),
        );

        assert_eq!(
            sink.apply(application.clone()).await,
            Err(expected.clone()),
            "the recipient disposition must retain its native error, not become text or success"
        );
        assert!(
            operations
                .snapshot(&waiting.operation_id)
                .expect("snapshot")
                .expect("wait operation")
                .terminal,
            "a refused recipient append cannot undo the already completed job"
        );
        assert_eq!(
            sink.apply(application).await,
            Err(expected),
            "replaying the completed wait must not hide a refused recipient append"
        );
    }
}

#[async_trait]
impl JobDeliverySink for CountingDeliverySink {
    async fn apply(
        &self,
        _application: JobDeliveryApplication,
    ) -> Result<(), JobDeliveryApplyError> {
        *self.applications.lock().await += 1;
        Ok(())
    }
}

#[tokio::test]
async fn terminal_delivery_closes_wait_once_and_still_reaches_the_normal_sink() {
    let store = Arc::new(MemoryDetachedJobStore::new());
    let jobs = DetachedJobService::new(store);
    let session_id = SessionId::new();
    let receipt = jobs
        .submit(job_spec("await-delivery", session_id.clone()))
        .await
        .expect("submit");
    let claim = jobs
        .claim_attempt(
            &receipt.job_id,
            AttemptClaim::new(
                WorkerId::new("worker-await-delivery").expect("worker"),
                1,
                1_000,
                RunnerHandleRef::new("runner:await-delivery").expect("handle"),
            ),
        )
        .await
        .expect("claim");
    let reference = JobReference::new("realm-a", receipt.job_id.clone()).expect("reference");
    let operations = Arc::new(RuntimeOpsLifecycleRegistry::new());
    let coordinator = JobAwaitCoordinator::new("realm-a", jobs.clone(), operations.clone());
    let waiting = coordinator
        .await_job(&session_id, &reference)
        .await
        .expect("await");
    let terminal = jobs
        .complete_attempt(
            &receipt.job_id,
            AttemptWriteAuthority::from(&claim),
            2,
            None,
        )
        .await
        .expect("complete")
        .terminal_result
        .expect("terminal result");
    let subscription = JobSubscription::new(
        JobSubscriptionId::new("origin-await-delivery").expect("subscription"),
        session_id,
        JobDeliveryKind::Notification,
    );
    let application = JobDeliveryApplication::Notification {
        job_id: receipt.job_id,
        delivery_sequence: 1,
        subscription,
        content: JobDeliveryContent::Terminal(terminal),
    };
    let downstream = Arc::new(CountingDeliverySink::default());
    let sink = JobAwaitDeliverySink::new(coordinator, downstream.clone());

    sink.apply(application.clone())
        .await
        .expect("first delivery");
    sink.apply(application).await.expect("duplicate delivery");

    assert!(
        operations
            .snapshot(&waiting.operation_id)
            .expect("snapshot")
            .expect("wait")
            .terminal
    );
    assert_eq!(*downstream.applications.lock().await, 2);
}

#[tokio::test]
async fn terminal_delivery_without_an_await_binding_never_blocks_the_normal_sink() {
    let store = Arc::new(MemoryDetachedJobStore::new());
    let jobs = DetachedJobService::new(store);
    let origin_session_id = SessionId::new();
    let subscriber_session_id = SessionId::new();
    let terminal = JobTerminalResult::Succeeded { result_ref: None };
    let reference = terminal_job(
        &jobs,
        "delivery-without-await",
        origin_session_id,
        terminal.clone(),
    )
    .await;
    let operations = Arc::new(RuntimeOpsLifecycleRegistry::new());
    let coordinator = JobAwaitCoordinator::new("realm-a", jobs, operations.clone());
    let downstream = Arc::new(CountingDeliverySink::default());
    let sink = JobAwaitDeliverySink::new(coordinator, downstream.clone());
    let application = JobDeliveryApplication::Notification {
        job_id: reference.job_id().clone(),
        delivery_sequence: 1,
        subscription: JobSubscription::new(
            JobSubscriptionId::new("non-origin-subscriber").expect("subscription"),
            subscriber_session_id,
            JobDeliveryKind::Notification,
        ),
        content: JobDeliveryContent::Terminal(terminal),
    };

    sink.apply(application)
        .await
        .expect("ordinary delivery must not require await authorization");

    assert_eq!(*downstream.applications.lock().await, 1);
    assert!(
        operations
            .list_operations()
            .expect("list operations")
            .is_empty(),
        "delivery must not synthesize an await binding"
    );
}

#[tokio::test]
async fn terminal_delivery_rejects_a_conflicting_operation_at_the_wait_identity() {
    let store = Arc::new(MemoryDetachedJobStore::new());
    let jobs = DetachedJobService::new(store);
    let session_id = SessionId::new();
    let terminal = JobTerminalResult::Succeeded { result_ref: None };
    let reference = terminal_job(
        &jobs,
        "conflicting-wait-identity",
        session_id.clone(),
        terminal.clone(),
    )
    .await;
    let operations = Arc::new(RuntimeOpsLifecycleRegistry::new());
    let operation_id = meerkat_core::OperationId::for_detached_job_wait(
        &session_id,
        reference.realm_id(),
        reference.job_id().as_str(),
    );
    operations
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "conflicting operation".into(),
            source_label: "test".into(),
            operation_source: Some(OperationSource::detached_job(
                reference.realm_id(),
                reference.job_id().as_str(),
            )),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("register conflict");
    operations
        .provisioning_succeeded(&operation_id)
        .expect("start conflict");
    let coordinator = JobAwaitCoordinator::new("realm-a", jobs, operations.clone());

    assert!(
        coordinator
            .apply_terminal(&session_id, &reference, &terminal)
            .await
            .is_err(),
        "a delivery must not terminalize a different operation kind"
    );
    assert_eq!(
        operations
            .snapshot(&operation_id)
            .expect("snapshot")
            .expect("conflict")
            .status,
        OperationStatus::Running
    );
}

#[tokio::test]
async fn await_activity_is_projected_only_while_the_binding_is_running() {
    let store = Arc::new(MemoryDetachedJobStore::new());
    let jobs = DetachedJobService::new(store);
    let session_id = SessionId::new();
    let receipt = jobs
        .submit(job_spec("await-activity", session_id.clone()))
        .await
        .expect("submit");
    let reference = JobReference::new("realm-a", receipt.job_id.clone()).expect("reference");
    let operations = Arc::new(RuntimeOpsLifecycleRegistry::new());
    let coordinator = JobAwaitCoordinator::new("realm-a", jobs.clone(), operations);

    coordinator
        .await_job(&session_id, &reference)
        .await
        .expect("await");
    let activity = coordinator
        .activity_for_session(&session_id)
        .expect("activity")
        .expect("running wait");
    assert_eq!(activity.job_ids, vec![receipt.job_id.clone()]);
    assert!(activity.since_ms > 0);

    let claim = jobs
        .claim_attempt(
            &receipt.job_id,
            AttemptClaim::new(
                WorkerId::new("worker-await-activity").expect("worker"),
                1,
                1_000,
                RunnerHandleRef::new("runner:await-activity").expect("handle"),
            ),
        )
        .await
        .expect("claim");
    let terminal = jobs
        .complete_attempt(
            &receipt.job_id,
            AttemptWriteAuthority::from(&claim),
            2,
            None,
        )
        .await
        .expect("complete")
        .terminal_result
        .expect("terminal");
    coordinator
        .apply_terminal(&session_id, &reference, &terminal)
        .await
        .expect("apply terminal");

    assert!(
        coordinator
            .activity_for_session(&session_id)
            .expect("activity")
            .is_none(),
        "terminal waits are not current execution activity"
    );
}

/// Regression: an owner session that unregisters (runtime shutdown/retire)
/// terminalizes its wait bindings as `Terminated` — a legitimate lifecycle
/// closure, not corruption. A later terminal delivery for the awaited job
/// must apply cleanly and reach the downstream sink; treating it as corrupt
/// re-errors on every ~1s delivery retry and head-of-line blocks the entire
/// durable delivery queue.
#[tokio::test]
async fn terminal_delivery_after_wait_owner_termination_applies_cleanly() {
    let store = Arc::new(MemoryDetachedJobStore::new());
    let jobs = DetachedJobService::new(store);
    let session_id = SessionId::new();
    let receipt = jobs
        .submit(job_spec("await-unregistered", session_id.clone()))
        .await
        .expect("submit");
    let claim = jobs
        .claim_attempt(
            &receipt.job_id,
            AttemptClaim::new(
                WorkerId::new("worker-await-unregistered").expect("worker"),
                1,
                1_000,
                RunnerHandleRef::new("runner:await-unregistered").expect("handle"),
            ),
        )
        .await
        .expect("claim");
    let reference = JobReference::new("realm-a", receipt.job_id.clone()).expect("reference");
    let operations = Arc::new(RuntimeOpsLifecycleRegistry::new());
    let coordinator = JobAwaitCoordinator::new("realm-a", jobs.clone(), operations.clone());
    let waiting = coordinator
        .await_job(&session_id, &reference)
        .await
        .expect("await");

    // The owner session goes away before the job finishes: unregister
    // persists the running wait as Terminated (the same transition
    // `retire_owner_for_unregister` applies).
    operations
        .terminate_owner("runtime session unregistered".into())
        .expect("terminate owner");
    let terminated = operations
        .snapshot(&waiting.operation_id)
        .expect("snapshot")
        .expect("wait");
    assert!(terminated.terminal);
    assert_eq!(terminated.status, OperationStatus::Terminated);

    let terminal = jobs
        .complete_attempt(
            &receipt.job_id,
            AttemptWriteAuthority::from(&claim),
            2,
            None,
        )
        .await
        .expect("complete")
        .terminal_result
        .expect("terminal result");
    let downstream = Arc::new(CountingDeliverySink::default());
    let sink = JobAwaitDeliverySink::new(coordinator, downstream.clone());
    let application = JobDeliveryApplication::Notification {
        job_id: receipt.job_id,
        delivery_sequence: 1,
        subscription: JobSubscription::new(
            JobSubscriptionId::new("origin-await-unregistered").expect("subscription"),
            session_id,
            JobDeliveryKind::Notification,
        ),
        content: JobDeliveryContent::Terminal(terminal),
    };

    sink.apply(application).await.expect(
        "a wait closed by runtime lifecycle must not corrupt the terminal delivery; \
         Terminated-from-unregister is a legitimate terminal state",
    );
    assert_eq!(*downstream.applications.lock().await, 1);
}

/// End-to-end regression for the mob-realm terminal-delivery strand: a
/// runtime-backed service whose persistence manifest realm is `default`
/// builds a session under `mob.<mob_id>` (exactly what meerkat-mob's member
/// build produces). A background shell job submitted from that session must
/// terminalize with its outbox applied, deliver the JobTerminal submission to
/// the origin session's runtime inbox, and stay readable through shell job
/// recovery - not wedge on a service-lifetime projector bound to the manifest
/// realm ("belongs to realm mob.<id>, outside projector realm default").
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
mod mob_realm_shell_delivery {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use meerkat::surface::{
        build_runtime_backed_service, default_persistent_executor, materialize_session,
    };
    use meerkat::{
        AgentFactory, Config, CreateSessionRequest, FactoryAgentBuilder, PersistentSessionService,
        Session,
    };
    use meerkat_client::types::LlmStream;
    use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
    use meerkat_core::{Message, SessionBuildOptions};
    use meerkat_runtime::completion::CompletionOutcome;
    use meerkat_runtime::{Input, LogicalRuntimeId, MeerkatMachine, PromptInput};
    use meerkat_tools::builtin::shell::{
        DurableShellJobRuntime, JobId as ShellJobId, JobManager, JobStatus, ShellConfig,
        ShellJobDeliveryProjector,
    };
    use tokio::time::Duration;

    /// Scripted LLM: the first call spawns a background shell job; every
    /// later call ends its turn with plain text.
    struct BackgroundShellScript {
        llm_calls: AtomicUsize,
    }

    impl BackgroundShellScript {
        fn new() -> Self {
            Self {
                llm_calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl LlmClient for BackgroundShellScript {
        fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
            let usage = LlmEvent::UsageUpdate {
                usage: meerkat_core::TurnUsage::host_declared(
                    meerkat_core::Provider::OpenAI,
                    &request.model,
                    meerkat_core::Usage::default(),
                ),
            };
            let events = if self.llm_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                vec![
                    LlmEvent::ToolCallComplete {
                        id: "toolu_bg_shell".to_string(),
                        name: "shell".to_string(),
                        args: serde_json::json!({
                            "command": "printf mob-realm-delivery",
                            "background": true,
                        }),
                        meta: None,
                    },
                    usage,
                    LlmEvent::Done {
                        outcome: LlmDoneOutcome::success(meerkat_core::StopReason::ToolUse),
                    },
                ]
            } else {
                vec![
                    LlmEvent::TextDelta {
                        delta: "ok".to_string(),
                        meta: None,
                    },
                    usage,
                    LlmEvent::Done {
                        outcome: LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
                    },
                ]
            };
            Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::Other
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    async fn run_prompt(
        adapter: &Arc<MeerkatMachine>,
        session_id: &meerkat::SessionId,
        prompt: &str,
    ) {
        let (_outcome, handle) = adapter
            .accept_input_with_completion(session_id, Input::Prompt(PromptInput::new(prompt, None)))
            .await
            .expect("accept prompt input");
        let handle = handle.expect("completion handle");
        let outcome = tokio::time::timeout(Duration::from_secs(30), handle.wait())
            .await
            .expect("prompt should complete in time")
            .expect("completion waiter should resolve");
        assert!(
            matches!(outcome, CompletionOutcome::Completed(_)),
            "unexpected completion outcome: {outcome:?}"
        );
    }

    #[tokio::test]
    async fn mob_realm_session_background_shell_terminal_delivery_is_not_stranded() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (_manifest, persistence) = meerkat::open_realm_persistence_in(
            temp.path(),
            "default",
            Some(meerkat_store::RealmBackend::Sqlite),
            Some(meerkat_store::RealmOrigin::Explicit),
        )
        .await
        .expect("open realm persistence");
        let job_store = persistence.job_store();
        let blob_store = persistence.blob_store();
        let runtime_inbox = meerkat_runtime::RuntimeDeliveryInbox::new(persistence.runtime_store());

        let factory = AgentFactory::new(temp.path().join("sessions")).shell(true);
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        builder.default_llm_client = Some(Arc::new(BackgroundShellScript::new()));
        let (service, adapter) = build_runtime_backed_service(builder, 4, persistence)
            .expect("compose the runtime-backed service");
        let service: Arc<PersistentSessionService<FactoryAgentBuilder>> = Arc::new(service);

        let mob_realm = meerkat_core::mob_realm_id("test-mob").expect("mob realm id");
        let session = Session::new();
        let session_id = session.id().clone();
        let build = SessionBuildOptions {
            realm_id: Some(mob_realm.clone()),
            ..SessionBuildOptions::default()
        };
        let request = CreateSessionRequest {
            injected_context: Vec::new(),
            model: "gpt-5.4".to_string(),
            prompt: meerkat_core::ContentInput::Text(String::new()),
            system_prompt: meerkat::SystemPromptOverride::Set(
                "mob realm shell delivery".to_string(),
            ),
            max_tokens: None,
            event_tx: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(build),
            labels: None,
        };
        let service_for_executor = Arc::clone(&service);
        let adapter_for_executor = Arc::clone(&adapter);
        materialize_session(&service, &adapter, session, request, move |session_id| {
            default_persistent_executor(service_for_executor, adapter_for_executor, session_id)
        })
        .await
        .expect("materialize session");

        run_prompt(&adapter, &session_id, "run the background job").await;

        // (a) The job terminalizes under the mob realm and its terminal outbox
        // entry is applied. Pre-fix this never converges: the projector is
        // bound to the manifest realm and refuses the mob-realm job, so the
        // entry stays pending forever.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let stored = loop {
            let jobs = job_store.list_all(16).await.expect("list jobs");
            let candidate = jobs
                .iter()
                .find(|job| job.spec.realm_id == mob_realm.as_str())
                .cloned();
            if let Some(job) = candidate
                && job.terminal_result.is_some()
                && !job.outbox.is_empty()
                && job.outbox.iter().all(|entry| entry.applied)
            {
                break job;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "background shell job never terminalized with applied outbox: {jobs:?}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        assert_eq!(stored.spec.origin_session_id, session_id);

        // (b) The JobTerminal delivery reached the origin session's runtime
        // inbox: still pending, or already acknowledged past the cursor.
        let runtime_id = LogicalRuntimeId::for_session(&session_id);
        let pending = runtime_inbox
            .list_pending(&runtime_id, usize::MAX)
            .await
            .expect("list pending deliveries");
        let delivered_pending = pending
            .iter()
            .any(|record| record.submission.delivery_id().as_str() == stored.job_id.as_str());
        let applied_cursor = runtime_inbox
            .applied_cursor(&runtime_id)
            .await
            .expect("applied cursor");
        assert!(
            delivered_pending || applied_cursor >= 1,
            "JobTerminal delivery never reached the origin session runtime inbox"
        );

        // (c) A restarted shell JobManager over the same durable stores - the
        // recovery path that previously hard-errored with "job delivery
        // recovery failed ... outside projector realm" - returns the terminal
        // status.
        let recovery_projector: Arc<dyn ShellJobDeliveryProjector> = Arc::new(
            meerkat::JobOutboxProjector::new(job_store.clone(), runtime_inbox.clone())
                .bound_to_realm(mob_realm.to_string()),
        );
        let recovery_runtime = DurableShellJobRuntime::new(
            mob_realm.as_str(),
            session_id.clone(),
            job_store.clone(),
            blob_store,
            recovery_projector,
        )
        .expect("recovery durable shell runtime");
        let recovery_manager =
            JobManager::new(ShellConfig::with_project_root(temp.path().to_path_buf()))
                .with_durable_job_runtime(recovery_runtime);
        let restored = recovery_manager
            .get_status(&ShellJobId::from_string(stored.job_id.as_str()))
            .await
            .expect("recovered shell job status must not realm-mismatch")
            .expect("recovered shell job present");
        assert!(
            matches!(
                restored.status,
                JobStatus::Completed { ref stdout, .. } if stdout == "mob-realm-delivery"
            ),
            "unexpected recovered status: {:?}",
            restored.status
        );
    }
}

/// The shell family as one session meets it: `shell` turns on the whole
/// family, `monitor_start` included. Each row is a runtime-backed service on
/// a persistent realm with a scripted model that reads the tool list it is
/// sent and makes one tool call.
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
mod shell_family_monitor {
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::Mutex;

    use meerkat::surface::{
        build_runtime_backed_service, default_persistent_executor, materialize_session,
    };
    use meerkat::{
        AgentFactory, Config, CreateSessionRequest, DetachedJobService, DetachedJobStore,
        FactoryAgentBuilder, JobTerminalResult, PersistentSessionService, Session, SessionId,
    };
    use meerkat_client::types::LlmStream;
    use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
    use meerkat_core::{BlobStore, Message, SessionBuildOptions};
    use meerkat_runtime::completion::CompletionOutcome;
    use meerkat_runtime::{Input, MeerkatMachine, PromptInput, RuntimeDeliveryInbox};
    use meerkat_tools::builtin::shell::{
        DurableShellJobRuntime, JobId as ShellJobId, JobManager, MonitorOutputProtocol,
        MonitorProtocolLimits, MonitorStartOptions, ShellConfig, ShellJobDeliveryProjector,
    };
    use tokio::time::Duration;

    const SHELL_FAMILY: [&str; 5] = [
        "shell",
        "shell_job_status",
        "shell_jobs",
        "shell_job_cancel",
        "monitor_start",
    ];
    const MONITOR: &str = "monitor_start";
    const CALL_ID: &str = "toolu_scripted_call";
    const NOT_PERMITTED: &str = "monitor_start_not_permitted";
    /// A monitor script that completes at once.
    const COMPLETES_AT_ONCE: &str = "printf '%s\\n' '{\"type\":\"complete\"}'";
    /// How long a row waits for a monitor to run again or for its job to end.
    const SETTLE: std::time::Duration = std::time::Duration::from_secs(20);
    /// How long a row whose session has no shell job call watches for a
    /// run that must not come.
    const QUIET: std::time::Duration = std::time::Duration::from_secs(3);

    /// What the scripted model was sent, request by request.
    #[derive(Default)]
    struct Seen {
        /// The tool names of each request.
        tools: Vec<Vec<String>>,
        /// The messages of each request, as JSON.
        messages: Vec<serde_json::Value>,
    }

    /// Scripted model: the first request makes one tool call; every later
    /// request ends the turn with text.
    struct OneCallScript {
        seen: Arc<Mutex<Seen>>,
        name: &'static str,
        args: serde_json::Value,
    }

    impl OneCallScript {
        fn new(name: &'static str, args: serde_json::Value) -> (Arc<Self>, Arc<Mutex<Seen>>) {
            let seen = Arc::new(Mutex::new(Seen::default()));
            let script = Arc::new(Self {
                seen: Arc::clone(&seen),
                name,
                args,
            });
            (script, seen)
        }
    }

    #[async_trait::async_trait]
    impl LlmClient for OneCallScript {
        fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
            let first = {
                let mut seen = self.seen.lock().expect("seen");
                seen.tools.push(
                    request
                        .tools
                        .iter()
                        .map(|tool| tool.name.as_str().to_string())
                        .collect(),
                );
                seen.messages
                    .push(serde_json::to_value(&request.messages).expect("messages as json"));
                seen.tools.len() == 1
            };
            let usage = LlmEvent::UsageUpdate {
                usage: meerkat_core::TurnUsage::host_declared(
                    meerkat_core::Provider::OpenAI,
                    &request.model,
                    meerkat_core::Usage::default(),
                ),
            };
            let events = if first {
                vec![
                    LlmEvent::ToolCallComplete {
                        id: CALL_ID.to_string(),
                        name: self.name.to_string(),
                        args: self.args.clone(),
                        meta: None,
                    },
                    usage,
                    LlmEvent::Done {
                        outcome: LlmDoneOutcome::success(meerkat_core::StopReason::ToolUse),
                    },
                ]
            } else {
                vec![
                    LlmEvent::TextDelta {
                        delta: "ok".to_string(),
                        meta: None,
                    },
                    usage,
                    LlmEvent::Done {
                        outcome: LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
                    },
                ]
            };
            Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::Other
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    /// What the session's profile declares for one build.
    #[derive(Default)]
    struct Profile {
        restriction: Option<meerkat_core::ops::DeclaredToolRestriction>,
        grant_gated: Option<meerkat_core::tool_grant::GrantGatedTools>,
        /// The launch's own tool access policy, as a spawn or a fork gives one.
        launch: Option<meerkat_core::ops::ToolAccessPolicy>,
        /// The block list of the session's turn.
        blocked: Vec<&'static str>,
        /// The session's own tool filter, as `session/create` takes one.
        session_filter: Option<meerkat_core::ToolFilter>,
    }

    impl Profile {
        /// A profile that narrows nothing.
        fn open() -> Self {
            Self::default()
        }

        /// A profile with `read_only` and the `deny` names.
        fn restricted(read_only: bool, deny: &[&str]) -> Self {
            Self {
                restriction: Some(restriction(read_only, deny)),
                ..Self::default()
            }
        }

        /// A profile whose grant gate closes the `names` until a grant.
        fn gating(names: &[&str]) -> Self {
            let mut tools = meerkat_core::ToolNameSet::default();
            for name in names {
                tools.0.insert(meerkat_core::ToolName::new(*name));
            }
            Self {
                grant_gated: Some(meerkat_core::tool_grant::GrantGatedTools {
                    tools,
                    ..Default::default()
                }),
                ..Self::default()
            }
        }

        /// A launch whose tool access policy is a deny list of the `names`,
        /// as a spawn's or a fork's `tool_access_policy` gives one.
        fn launch_denying(names: &[&str]) -> Self {
            let mut deny = meerkat_core::ToolNameSet::default();
            for name in names {
                deny.0.insert(meerkat_core::ToolName::new(*name));
            }
            Self {
                launch: Some(meerkat_core::ops::ToolAccessPolicy::DenyList(deny)),
                ..Self::default()
            }
        }

        /// A launch whose tool access policy is an allow list of the `names`.
        fn launch_allowing(names: &[&str]) -> Self {
            let mut allow = meerkat_core::ToolNameSet::default();
            for name in names {
                allow.0.insert(meerkat_core::ToolName::new(*name));
            }
            Self {
                launch: Some(meerkat_core::ops::ToolAccessPolicy::AllowList(allow)),
                ..Self::default()
            }
        }

        /// A session created with a tool filter that denies the `names`.
        fn filtering_out(names: &[&str]) -> Self {
            let mut deny = meerkat_core::ToolNameSet::default();
            for name in names {
                deny.0.insert(meerkat_core::ToolName::new(*name));
            }
            Self {
                session_filter: Some(meerkat_core::ToolFilter::Deny(deny)),
                ..Self::default()
            }
        }

        /// A session whose turn carries a block list of the `names`.
        fn blocking(names: &[&'static str]) -> Self {
            Self {
                blocked: names.to_vec(),
                ..Self::default()
            }
        }
    }

    /// The restriction a mob profile's `read_only` and `deny` lower to.
    fn restriction(read_only: bool, deny: &[&str]) -> meerkat_core::ops::DeclaredToolRestriction {
        let mut names = meerkat_core::ToolNameSet::default();
        for name in deny {
            names.0.insert(meerkat_core::ToolName::new(*name));
        }
        meerkat_core::ops::DeclaredToolRestriction {
            declared_by: "profile 'worker'".to_string(),
            enabled_families: vec!["shell".to_string()],
            read_only,
            deny: names,
            vocabulary: std::collections::BTreeMap::new(),
            deferred_mcp_servers: std::collections::BTreeSet::new(),
        }
    }

    /// Whether the answer to the scripted call in `messages` is an error;
    /// `None` when the messages carry no answer to it.
    fn call_answer_is_error(messages: &serde_json::Value) -> Option<bool> {
        match messages {
            serde_json::Value::Array(items) => items.iter().find_map(call_answer_is_error),
            serde_json::Value::Object(map) => {
                if map.get("tool_use_id").and_then(serde_json::Value::as_str) == Some(CALL_ID) {
                    return Some(
                        map.get("is_error")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false),
                    );
                }
                map.values().find_map(call_answer_is_error)
            }
            _ => None,
        }
    }

    fn mob_realm() -> meerkat_core::RealmId {
        meerkat_core::mob_realm_id("shell-family-mob").expect("mob realm id")
    }

    /// One host lifetime over the realm under `root`.
    struct Lifetime {
        service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
        adapter: Arc<MeerkatMachine>,
        job_store: Arc<dyn DetachedJobStore>,
        blob_store: Arc<dyn BlobStore>,
        runtime_inbox: RuntimeDeliveryInbox,
    }

    impl Lifetime {
        /// Open the realm under `root` with `shell` switched as given and
        /// `script` as every session's model.
        async fn open(root: &Path, shell: bool, script: Arc<OneCallScript>) -> Self {
            let (_manifest, persistence) = meerkat::open_realm_persistence_in(
                root,
                "default",
                Some(meerkat_store::RealmBackend::Sqlite),
                Some(meerkat_store::RealmOrigin::Explicit),
            )
            .await
            .expect("open realm persistence");
            let job_store = persistence.job_store();
            let blob_store = persistence.blob_store();
            let runtime_inbox = RuntimeDeliveryInbox::new(persistence.runtime_store());
            let factory = AgentFactory::new(root.join("sessions")).shell(shell);
            let mut builder = FactoryAgentBuilder::new(factory, Config::default());
            builder.default_llm_client = Some(script);
            let (service, adapter) = build_runtime_backed_service(builder, 4, persistence)
                .expect("compose the runtime-backed service");
            Self {
                service: Arc::new(service),
                adapter,
                job_store,
                blob_store,
                runtime_inbox,
            }
        }

        /// Build `session` (a new one, or one a former lifetime stored) under
        /// `profile` and run one prompt on it.
        async fn run_session(&self, session: Session, profile: Profile) {
            let session_id = session.id().clone();
            let build = SessionBuildOptions {
                realm_id: Some(mob_realm()),
                declared_tool_restriction: profile.restriction,
                grant_gated_tools: profile.grant_gated,
                tool_access_policy: profile.launch,
                initial_tool_filter: profile.session_filter,
                ..SessionBuildOptions::default()
            };
            let request = CreateSessionRequest {
                injected_context: Vec::new(),
                model: "gpt-5.4".to_string(),
                prompt: meerkat_core::ContentInput::Text(String::new()),
                system_prompt: meerkat::SystemPromptOverride::Set("shell family".to_string()),
                max_tokens: None,
                event_tx: None,
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                build: Some(build),
                labels: None,
            };
            let service_for_executor = Arc::clone(&self.service);
            let adapter_for_executor = Arc::clone(&self.adapter);
            Box::pin(materialize_session(
                &self.service,
                &self.adapter,
                session,
                request,
                move |session_id| {
                    default_persistent_executor(
                        service_for_executor,
                        adapter_for_executor,
                        session_id,
                    )
                },
            ))
            .await
            .expect("materialize session");

            let turn_metadata = (!profile.blocked.is_empty()).then(|| {
                meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                    turn_tool_overlay: Some(meerkat_core::TurnToolOverlay {
                        blocked_tools: Some(
                            profile
                                .blocked
                                .iter()
                                .map(|name| meerkat_core::ToolName::new(*name))
                                .collect(),
                        ),
                        ..Default::default()
                    }),
                    ..Default::default()
                }
            });
            let (_outcome, handle) = self
                .adapter
                .accept_input_with_completion(
                    &session_id,
                    Input::Prompt(PromptInput::new("go on", turn_metadata)),
                )
                .await
                .expect("accept prompt input");
            let outcome = tokio::time::timeout(
                Duration::from_secs(60),
                handle.expect("completion handle").wait(),
            )
            .await
            .expect("the prompt completes in time")
            .expect("the completion waiter resolves");
            assert!(
                matches!(outcome, CompletionOutcome::Completed(_)),
                "unexpected completion outcome: {outcome:?}"
            );
        }

        /// The monitor jobs of `session_id`.
        async fn monitor_jobs(&self, session_id: &SessionId) -> Vec<meerkat_jobs::StoredJob> {
            self.job_store
                .list_all(16)
                .await
                .expect("list jobs")
                .into_iter()
                .filter(|job| {
                    &job.spec.origin_session_id == session_id && job.spec.tool.name() == MONITOR
                })
                .collect()
        }

        /// A job manager for `session_id` built exactly as the host's
        /// monitor route builds its own: the realm's stores, the session as
        /// origin, and no say of the session on a monitor's re-run.
        fn operator_route_manager(&self, root: &Path, session_id: &SessionId) -> JobManager {
            let projector: Arc<dyn ShellJobDeliveryProjector> = Arc::new(
                meerkat::JobOutboxProjector::new(
                    self.job_store.clone(),
                    self.runtime_inbox.clone(),
                )
                .bound_to_realm(mob_realm().to_string()),
            );
            let durable = DurableShellJobRuntime::new(
                mob_realm().as_str(),
                session_id.clone(),
                self.job_store.clone(),
                self.blob_store.clone(),
                projector,
            )
            .expect("the operator route's durable runtime");
            JobManager::new(ShellConfig::with_project_root(root.to_path_buf()))
                .bind_canonical_async_ops(
                    session_id.clone(),
                    Arc::new(meerkat_runtime::RuntimeOpsLifecycleRegistry::new()),
                )
                .with_durable_job_runtime(durable)
        }

        /// End this lifetime as a host stop does, so a successor can serve
        /// its sessions.
        async fn stand_down(self) {
            for registration in self.adapter.current_session_registration_witnesses().await {
                assert!(
                    self.adapter
                        .unregister_session_registration_until_terminal_if_current(&registration)
                        .await
                        .expect("unregister the lifetime's runtime registration"),
                    "the lifetime's current registration is torn down"
                );
            }
            self.service
                .try_shutdown()
                .await
                .expect("shut the lifetime's session service down");
        }
    }

    /// What one session met.
    struct Met {
        /// The tool names of the first request.
        offered: Vec<String>,
        /// Whether the answer to the `monitor_start` call was an error.
        call_refused: bool,
        /// The monitor jobs the session's realm recorded.
        monitor_jobs: usize,
    }

    /// Run one session with `shell` switched as given under `profile`, let
    /// its model call `monitor_start` once and answer what the session met.
    async fn session_meets(shell: bool, profile: Profile) -> Met {
        let temp = tempfile::tempdir().expect("tempdir");
        let (script, seen) = OneCallScript::new(
            MONITOR,
            serde_json::json!({
                "command": COMPLETES_AT_ONCE,
                "timeout_secs": 30,
            }),
        );
        let lifetime = Lifetime::open(temp.path(), shell, script).await;
        let session = Session::new();
        let session_id = session.id().clone();
        lifetime.run_session(session, profile).await;

        let monitor_jobs = lifetime.monitor_jobs(&session_id).await.len();
        let seen = seen.lock().expect("seen");
        let offered = seen.tools.first().cloned().expect("a first request");
        let call_refused = seen
            .messages
            .iter()
            .find_map(call_answer_is_error)
            .expect("the model is sent an answer to its monitor_start call");
        Met {
            offered,
            call_refused,
            monitor_jobs,
        }
    }

    fn offers(met: &Met, name: &str) -> bool {
        met.offered.iter().any(|offered| offered == name)
    }

    /// `shell` alone, with no other configuration, offers the whole family
    /// and a `monitor_start` call starts a durable monitor job.
    #[tokio::test]
    async fn a_shell_session_is_offered_the_whole_family_and_can_start_a_monitor() {
        let met = session_meets(true, Profile::open()).await;
        for name in SHELL_FAMILY {
            assert!(
                offers(&met, name),
                "a shell session is offered `{name}`: {:?}",
                met.offered
            );
        }
        assert!(
            !met.call_refused,
            "the session's `{MONITOR}` call is answered without an error"
        );
        assert_eq!(
            met.monitor_jobs, 1,
            "the call recorded one durable monitor job"
        );
    }

    /// A profile that denies the name keeps the four other shell tools,
    /// hides `monitor_start` and refuses a call by name.
    #[tokio::test]
    async fn a_denied_monitor_start_is_hidden_and_refused_by_name() {
        let met = session_meets(true, Profile::restricted(false, &[MONITOR])).await;
        for name in &SHELL_FAMILY[..4] {
            assert!(
                offers(&met, name),
                "the deny leaves `{name}` offered: {:?}",
                met.offered
            );
        }
        assert!(
            !offers(&met, MONITOR),
            "a denied `{MONITOR}` is not offered: {:?}",
            met.offered
        );
        assert!(met.call_refused, "a denied `{MONITOR}` call is refused");
        assert_eq!(met.monitor_jobs, 0, "a refused call records no job");
    }

    /// A read-only profile meets `monitor_start` as it meets `shell`: listed
    /// with its family, and every call refused.
    #[tokio::test]
    async fn a_read_only_session_is_refused_monitor_start_like_shell() {
        let met = session_meets(true, Profile::restricted(true, &[])).await;
        assert!(
            offers(&met, "shell") && offers(&met, MONITOR),
            "read-only hides no shell tool, `{MONITOR}` included: {:?}",
            met.offered
        );
        assert!(
            met.call_refused,
            "a read-only session's `{MONITOR}` call is refused"
        );
        assert_eq!(met.monitor_jobs, 0, "a refused call records no job");
    }

    /// Without `shell` the session has no tool of the family.
    #[tokio::test]
    async fn a_session_without_shell_has_no_monitor_start() {
        let met = session_meets(false, Profile::open()).await;
        for name in SHELL_FAMILY {
            assert!(
                !offers(&met, name),
                "a session without shell is not offered `{name}`: {:?}",
                met.offered
            );
        }
        assert!(
            met.call_refused,
            "a `{MONITOR}` call without shell is refused"
        );
        assert_eq!(met.monitor_jobs, 0, "a refused call records no job");
    }

    /// What a session meets whose `shell` is closed by name: neither `shell`
    /// nor `monitor_start` is offered, the family's three job tools still
    /// are, and a `monitor_start` call is refused and records nothing.
    fn assert_monitor_start_closed_with_shell(met: &Met, why: &str) {
        for name in ["shell", MONITOR] {
            assert!(
                !offers(met, name),
                "{why}: `{name}` is not offered: {:?}",
                met.offered
            );
        }
        for name in &SHELL_FAMILY[1..4] {
            assert!(
                offers(met, name),
                "{why}: `{name}` is still offered: {:?}",
                met.offered
            );
        }
        assert!(met.call_refused, "{why}: a `{MONITOR}` call is refused");
        assert_eq!(met.monitor_jobs, 0, "{why}: a refused call records no job");
    }

    /// `monitor_start` is never wider than `shell`: a profile whose deny
    /// list names only `shell` closes the monitor with it.
    #[tokio::test]
    async fn a_profile_deny_that_names_shell_closes_monitor_start_with_it() {
        let met = session_meets(true, Profile::restricted(false, &["shell"])).await;
        assert_monitor_start_closed_with_shell(&met, "the profile denies `shell`");
    }

    /// The same for a launch's own deny list, the road of a spawn's and of
    /// a fork's `tool_access_policy`.
    #[tokio::test]
    async fn a_launch_deny_list_that_names_shell_closes_monitor_start_with_it() {
        let met = session_meets(true, Profile::launch_denying(&["shell"])).await;
        assert_monitor_start_closed_with_shell(&met, "the launch denies `shell`");
    }

    /// The same for the session's own tool filter, the form `session/create`
    /// takes: a filter that denies `shell`.
    #[tokio::test]
    async fn a_session_tool_filter_that_denies_shell_closes_monitor_start_with_it() {
        let met = session_meets(true, Profile::filtering_out(&["shell"])).await;
        assert_monitor_start_closed_with_shell(&met, "the session filter denies `shell`");
    }

    /// And for an allow list that names `monitor_start` and the job tools
    /// but not `shell`: the monitor is not wider than the `shell` the list
    /// leaves out.
    #[tokio::test]
    async fn an_allow_list_that_names_monitor_start_and_not_shell_closes_it() {
        let met = session_meets(
            true,
            Profile::launch_allowing(&[
                MONITOR,
                "shell_job_status",
                "shell_jobs",
                "shell_job_cancel",
            ]),
        )
        .await;
        assert_monitor_start_closed_with_shell(&met, "the allow list leaves `shell` out");
    }

    /// The same for a turn's block list.
    #[tokio::test]
    async fn a_turn_block_list_that_names_shell_closes_monitor_start_with_it() {
        let met = session_meets(true, Profile::blocking(&["shell"])).await;
        assert_monitor_start_closed_with_shell(&met, "the turn blocks `shell`");
    }

    /// The same for a grant gate on the name `shell` (not on the family)
    /// before any grant.
    #[tokio::test]
    async fn a_grant_gate_on_the_name_shell_closes_monitor_start_with_it() {
        let met = session_meets(true, Profile::gating(&["shell"])).await;
        assert_monitor_start_closed_with_shell(&met, "a grant gate closes `shell`");
    }

    /// Run a session under `first`, stop the host, build the same session
    /// again under `again` and answer what it meets then, its model
    /// calling `monitor_start` once.
    async fn session_meets_after_a_restart(first: Profile, again: Profile) -> Met {
        let temp = tempfile::tempdir().expect("tempdir");
        let call = || {
            OneCallScript::new(
                MONITOR,
                serde_json::json!({ "command": COMPLETES_AT_ONCE, "timeout_secs": 30 }),
            )
        };
        let (script, _seen) = call();
        let lifetime = Lifetime::open(temp.path(), true, script).await;
        let session = Session::new();
        let session_id = session.id().clone();
        lifetime.run_session(session, first).await;
        lifetime.stand_down().await;

        let (script, seen) = call();
        let lifetime = Lifetime::open(temp.path(), true, script).await;
        let stored = lifetime
            .service
            .load_authoritative_session(&session_id)
            .await
            .expect("load the stored session")
            .expect("the session survives the restart");
        lifetime.run_session(stored, again).await;

        let monitor_jobs = lifetime.monitor_jobs(&session_id).await.len();
        let seen = seen.lock().expect("seen");
        let offered = seen.tools.first().cloned().expect("a first request");
        let call_refused = seen
            .messages
            .iter()
            .find_map(call_answer_is_error)
            .expect("the model is sent an answer to its monitor_start call");
        Met {
            offered,
            call_refused,
            monitor_jobs,
        }
    }

    /// A restart does not bring the monitor back: the profile's deny of
    /// `shell` is read again at the session's next build.
    #[tokio::test]
    async fn a_profile_deny_that_names_shell_still_closes_monitor_start_after_a_restart() {
        let met = session_meets_after_a_restart(
            Profile::restricted(false, &["shell"]),
            Profile::restricted(false, &["shell"]),
        )
        .await;
        assert_monitor_start_closed_with_shell(&met, "the profile denies `shell`, resumed");
    }

    /// Nor does a resume: the launch's deny list is the session's own
    /// record, restored when the session is built again with none given.
    #[tokio::test]
    async fn a_launch_deny_list_that_names_shell_still_closes_monitor_start_after_a_restart() {
        let met =
            session_meets_after_a_restart(Profile::launch_denying(&["shell"]), Profile::open())
                .await;
        assert_monitor_start_closed_with_shell(&met, "the launch denied `shell`, resumed");
    }

    /// How often the monitor's script started: it appends one line to
    /// `marker` each time it runs.
    fn runs(marker: &Path) -> usize {
        std::fs::read_to_string(marker)
            .map(|text| text.lines().count())
            .unwrap_or(0)
    }

    /// A script that records each start in `marker` and completes at once.
    fn recording_script(marker: &Path) -> String {
        format!(
            "printf 'run\\n' >> '{}'; printf '%s\\n' '{{\"type\":\"complete\"}}'",
            marker.display()
        )
    }

    /// A script that records each start in `marker`, completes at once the
    /// first time and stays alive every later time, so a cancel finds the
    /// monitor running when it was started again.
    fn recording_script_that_stays_alive_when_run_again(marker: &Path) -> String {
        format!(
            "printf 'run\\n' >> '{0}'; if [ \"$(wc -l < '{0}')\" -gt 1 ]; then sleep 60; fi; \
             printf '%s\\n' '{{\"type\":\"complete\"}}'",
            marker.display()
        )
    }

    /// A session of `lifetime` whose model starts a replayable monitor
    /// running `command`. Answers the session and the recorded job.
    async fn a_session_starts_a_replayable_monitor(
        lifetime: &Lifetime,
        seen: &Arc<Mutex<Seen>>,
    ) -> (SessionId, meerkat_jobs::StoredJob) {
        let session = Session::new();
        let session_id = session.id().clone();
        lifetime.run_session(session, Profile::open()).await;
        let refused = seen
            .lock()
            .expect("seen")
            .messages
            .iter()
            .find_map(call_answer_is_error);
        assert_eq!(
            refused,
            Some(false),
            "the session's `{MONITOR}` call is answered without an error"
        );
        let recorded = lifetime.monitor_jobs(&session_id).await;
        assert_eq!(
            recorded.len(),
            1,
            "the call recorded one durable monitor job"
        );
        let recorded = recorded.into_iter().next().expect("the recorded monitor");
        (session_id, recorded)
    }

    /// The scripted call that starts a replayable monitor running `command`.
    fn a_replayable_monitor_call(command: String) -> (Arc<OneCallScript>, Arc<Mutex<Seen>>) {
        OneCallScript::new(
            MONITOR,
            serde_json::json!({
                "command": command,
                "timeout_secs": 120,
                "restart_class": "replayable",
            }),
        )
    }

    /// Poll until `job_id` has a terminal result or `wait` passed.
    async fn ended(
        job_store: &Arc<dyn DetachedJobStore>,
        job_id: &meerkat::JobId,
        wait: std::time::Duration,
    ) -> meerkat_jobs::JobSnapshot {
        let jobs = DetachedJobService::new(job_store.clone());
        let deadline = std::time::Instant::now() + wait;
        loop {
            let snapshot = jobs.get(job_id).await.expect("read job").expect("job");
            if snapshot.terminal_result.is_some() || std::time::Instant::now() >= deadline {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// One host lifetime in which a shell session's model starts a
    /// replayable monitor, and that leaves a monitor of the session whose
    /// worker was lost: a second job with the recorded monitor's own
    /// specification, whose first attempt holds a lease that expired long
    /// ago, as an abrupt host stop leaves a running monitor once its lease
    /// ran out. (A monitor's lease is its timeout plus a minute, so the row
    /// plants the lost attempt instead of waiting for a real one.) Answers
    /// the session and the lost monitor's job.
    async fn a_lifetime_that_loses_a_resumable_monitor(
        root: &Path,
        marker: &Path,
    ) -> (SessionId, meerkat::JobId) {
        a_lifetime_that_loses_a_resumable_monitor_running(root, marker, recording_script(marker))
            .await
    }

    /// As [`a_lifetime_that_loses_a_resumable_monitor`], with the monitor's
    /// script given.
    async fn a_lifetime_that_loses_a_resumable_monitor_running(
        root: &Path,
        marker: &Path,
        command: String,
    ) -> (SessionId, meerkat::JobId) {
        let (script, seen) = a_replayable_monitor_call(command);
        let lifetime = Lifetime::open(root, true, script).await;
        let (session_id, recorded) = a_session_starts_a_replayable_monitor(&lifetime, &seen).await;

        // The recorded monitor runs once and completes.
        let jobs = DetachedJobService::new(lifetime.job_store.clone());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let terminal = jobs
                .get(&recorded.job_id)
                .await
                .expect("read job")
                .expect("job")
                .terminal_result;
            if terminal.is_some() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the session's monitor never completed"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(runs(marker), 1, "the monitor's script ran once");

        // The lost monitor: the same recorded specification under its own
        // identity, claimed by a worker that is gone.
        let mut spec = recorded.spec.clone();
        spec.execution_intent_id =
            meerkat::ExecutionIntentId::from_string("monitor-call:lost-with-the-host")
                .expect("intent");
        spec.submission_key =
            meerkat::JobSubmissionKey::new(format!("monitor:{session_id}:lost-with-the-host"))
                .expect("submission key");
        let lost = jobs.submit(spec).await.expect("submit the lost monitor");
        jobs.claim_attempt(
            &lost.job_id,
            meerkat::AttemptClaim::new(
                meerkat::WorkerId::new("monitor-worker:gone").expect("worker"),
                1,
                10,
                meerkat::RunnerHandleRef::new("inproc-monitor:gone").expect("handle"),
            ),
        )
        .await
        .expect("the lost worker's claim");
        lifetime.stand_down().await;
        (session_id, lost.job_id)
    }

    /// What became of a lost monitor in the next host lifetime.
    struct After {
        /// How often the monitor's script started, the first run included.
        runs: usize,
        /// The job's terminal result, when it has one.
        terminal: Option<JobTerminalResult>,
    }

    /// Poll until the monitor ran again, its job ended, or `settle` passed.
    async fn settled(
        job_store: &Arc<dyn DetachedJobStore>,
        job_id: &meerkat::JobId,
        marker: &Path,
        settle: std::time::Duration,
    ) -> After {
        let jobs = DetachedJobService::new(job_store.clone());
        let deadline = std::time::Instant::now() + settle;
        loop {
            let terminal = jobs
                .get(job_id)
                .await
                .expect("read job")
                .expect("job")
                .terminal_result;
            let runs = runs(marker);
            if runs > 1 || terminal.is_some() || std::time::Instant::now() >= deadline {
                return After { runs, terminal };
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// The next host lifetime: the session is built again with `shell`
    /// switched as given under `profile`, and its model lists its shell
    /// jobs, the read-only call that recovers them.
    async fn the_session_lists_its_jobs_after_a_restart(
        shell: bool,
        profile: Profile,
        settle: std::time::Duration,
    ) -> After {
        let temp = tempfile::tempdir().expect("tempdir");
        let marker = temp.path().join("monitor-runs");
        let (session_id, job_id) =
            a_lifetime_that_loses_a_resumable_monitor(temp.path(), &marker).await;

        let (script, _seen) = OneCallScript::new("shell_jobs", serde_json::json!({}));
        let lifetime = Lifetime::open(temp.path(), shell, script).await;
        let stored = lifetime
            .service
            .load_authoritative_session(&session_id)
            .await
            .expect("load the stored session")
            .expect("the session survives the restart");
        lifetime.run_session(stored, profile).await;
        settled(&lifetime.job_store, &job_id, &marker, settle).await
    }

    fn assert_ended_not_permitted(after: &After, why: &str) {
        assert_eq!(
            after.runs, 1,
            "{why}: the recorded monitor is not run again"
        );
        assert!(
            matches!(
                &after.terminal,
                Some(JobTerminalResult::NeedsAttention { reason }) if reason.as_str() == NOT_PERMITTED
            ),
            "{why}: the job ends as needing attention, `{NOT_PERMITTED}`: {:?}",
            after.terminal
        );
    }

    /// Nothing changed: after a restart the session's next shell job call
    /// runs its recorded replayable monitor again.
    #[tokio::test]
    async fn a_recorded_monitor_is_run_again_after_a_restart_when_nothing_changed() {
        let after = the_session_lists_its_jobs_after_a_restart(true, Profile::open(), SETTLE).await;
        assert_eq!(
            after.runs, 2,
            "the recorded monitor is run again once: terminal {:?}",
            after.terminal
        );
    }

    /// A deny the profile gained since the monitor started holds for its
    /// re-run: the monitor is not run again and its job ends.
    #[tokio::test]
    async fn a_recorded_monitor_is_not_run_again_when_its_name_is_denied_since() {
        let after = the_session_lists_its_jobs_after_a_restart(
            true,
            Profile::restricted(false, &[MONITOR]),
            SETTLE,
        )
        .await;
        assert_ended_not_permitted(&after, "the profile denies `monitor_start`");
    }

    /// A profile that became read-only since holds for the re-run too.
    #[tokio::test]
    async fn a_recorded_monitor_is_not_run_again_when_the_profile_is_read_only_since() {
        let after = the_session_lists_its_jobs_after_a_restart(
            true,
            Profile::restricted(true, &[]),
            SETTLE,
        )
        .await;
        assert_ended_not_permitted(&after, "the profile is read-only");
    }

    /// A grant gate that closes the name since, with no grant, holds for the
    /// re-run as well: the session can still list its jobs, and the
    /// recorded monitor is not run again.
    #[tokio::test]
    async fn a_recorded_monitor_is_not_run_again_when_a_grant_gate_closes_its_name_since() {
        let after =
            the_session_lists_its_jobs_after_a_restart(true, Profile::gating(&[MONITOR]), SETTLE)
                .await;
        assert_ended_not_permitted(&after, "a grant gate closes `monitor_start`");
    }

    /// The re-run follows `shell` too: a profile that denies `shell` since
    /// (and so closes `monitor_start` with it) still lists its jobs, and
    /// the recorded monitor is not run again.
    #[tokio::test]
    async fn a_recorded_monitor_is_not_run_again_when_shell_is_denied_since() {
        let after = the_session_lists_its_jobs_after_a_restart(
            true,
            Profile::restricted(false, &["shell"]),
            SETTLE,
        )
        .await;
        assert_ended_not_permitted(&after, "the profile denies `shell`");
    }

    /// And a grant gate that closes the name `shell` since, with no grant.
    #[tokio::test]
    async fn a_recorded_monitor_is_not_run_again_when_a_grant_gate_closes_shell_since() {
        let after =
            the_session_lists_its_jobs_after_a_restart(true, Profile::gating(&["shell"]), SETTLE)
                .await;
        assert_ended_not_permitted(&after, "a grant gate closes `shell`");
    }

    /// A session whose shell family was turned off has no shell job call, so
    /// nothing of its own runs the recorded monitor again.
    #[tokio::test]
    async fn a_recorded_monitor_is_not_run_again_when_the_shell_family_is_off_since() {
        let after = the_session_lists_its_jobs_after_a_restart(false, Profile::open(), QUIET).await;
        assert_eq!(
            after.runs, 1,
            "the shell family is off: the recorded monitor is not run again"
        );
    }

    /// The operator starts a monitor of its own for the manager's session,
    /// which first recovers that session's recorded jobs.
    async fn operator_starts_a_monitor(operator: &JobManager, command: &str) -> ShellJobId {
        operator
            .spawn_monitor_for_call(
                command,
                None,
                30,
                "operator-submission",
                MonitorStartOptions {
                    protocol: MonitorOutputProtocol::FramedJsonl,
                    restart_class: meerkat::RestartClass::NonResumable,
                    limits: MonitorProtocolLimits::default(),
                    delivery: meerkat::JobDeliveryKind::Record,
                },
            )
            .await
            .expect("the operator's monitor start")
    }

    /// After a restart, an operator's cancel of a lost resumable monitor
    /// starts the monitor once more before it cancels it. The host's route
    /// first recovers the session's jobs under the host's authority, which
    /// claims a second attempt of the lost monitor and starts its script
    /// again; the cancel then stops that attempt. The row pins the second
    /// attempt and the cancelled end, not how far the script got before the
    /// cancel reached it. Its level is the pin row's: the route's own
    /// manager construction on the realm's real stores, not the RPC handler.
    #[tokio::test]
    async fn an_operators_cancel_of_a_lost_monitor_after_a_restart_starts_it_once_more_before_cancelling()
     {
        let temp = tempfile::tempdir().expect("tempdir");
        let marker = temp.path().join("monitor-runs");
        let (session_id, job_id) = a_lifetime_that_loses_a_resumable_monitor_running(
            temp.path(),
            &marker,
            recording_script_that_stays_alive_when_run_again(&marker),
        )
        .await;

        // The next lifetime: nothing of the session's own runs; the operator
        // cancels the lost monitor's job.
        let (script, _seen) = OneCallScript::new("shell_jobs", serde_json::json!({}));
        let lifetime = Lifetime::open(temp.path(), true, script).await;
        let lost = ended(&lifetime.job_store, &job_id, std::time::Duration::ZERO).await;
        assert_eq!(
            (lost.attempt_count, lost.terminal_result.clone()),
            (1, None),
            "the lost monitor has its one lost attempt and no end"
        );

        let operator = lifetime.operator_route_manager(temp.path(), &session_id);
        operator
            .cancel_job(&ShellJobId::from_string(job_id.as_str()))
            .await
            .expect("the operator's cancel of the lost monitor");

        let after = ended(&lifetime.job_store, &job_id, SETTLE).await;
        assert_eq!(
            after.attempt_count, 2,
            "the cancel's recovery started the lost monitor once more"
        );
        assert_eq!(
            after.terminal_result,
            Some(JobTerminalResult::Cancelled),
            "and the cancel then ended it"
        );
    }

    /// The safe order: a cancel accepted while the monitor still runs stops
    /// it, and nothing is run again after the restart. The operator's route
    /// cancels the session's running monitor in the first lifetime; the
    /// next lifetime's session lists its shell jobs, which recovers them,
    /// and the cancelled monitor keeps its one attempt and its cancelled
    /// end.
    #[tokio::test]
    async fn a_monitor_cancelled_while_it_runs_is_not_run_again_after_a_restart() {
        let temp = tempfile::tempdir().expect("tempdir");
        let marker = temp.path().join("monitor-runs");
        let (script, seen) = a_replayable_monitor_call(format!(
            "printf 'run\\n' >> '{}'; sleep 60",
            marker.display()
        ));
        let lifetime = Lifetime::open(temp.path(), true, script).await;
        let (session_id, recorded) = a_session_starts_a_replayable_monitor(&lifetime, &seen).await;
        let job_id = recorded.job_id;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while runs(&marker) == 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "the monitor's script never started"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // The operator cancels the running monitor through the host's route;
        // the session's worker sees the request at its next lease renewal.
        let operator = lifetime.operator_route_manager(temp.path(), &session_id);
        operator
            .cancel_job(&ShellJobId::from_string(job_id.as_str()))
            .await
            .expect("the operator's cancel of the running monitor");
        let cancelled = ended(
            &lifetime.job_store,
            &job_id,
            std::time::Duration::from_secs(60),
        )
        .await;
        assert_eq!(
            cancelled.terminal_result,
            Some(JobTerminalResult::Cancelled),
            "the cancel stops the running monitor"
        );
        lifetime.stand_down().await;

        // The next lifetime: the session lists its shell jobs.
        let (script, _seen) = OneCallScript::new("shell_jobs", serde_json::json!({}));
        let lifetime = Lifetime::open(temp.path(), true, script).await;
        let stored = lifetime
            .service
            .load_authoritative_session(&session_id)
            .await
            .expect("load the stored session")
            .expect("the session survives the restart");
        lifetime.run_session(stored, Profile::open()).await;
        tokio::time::sleep(QUIET).await;

        let after = ended(&lifetime.job_store, &job_id, std::time::Duration::ZERO).await;
        assert_eq!(
            runs(&marker),
            1,
            "the cancelled monitor is not run again after the restart"
        );
        assert_eq!(
            (after.attempt_count, after.terminal_result),
            (1, Some(JobTerminalResult::Cancelled)),
            "it keeps its one attempt and its cancelled end"
        );
    }

    /// A monitor the session's recovery ended stays ended. When the host's
    /// monitor route next recovers that session's jobs (an operator's
    /// monitor start, then a cancel of that operator's job), the ended
    /// monitor is not run again and its job is not touched: the same
    /// terminal result, the same attempt count, the same fence. The row's
    /// level is the pin row's below: the route's own manager construction
    /// on the realm's real stores, not the RPC handler.
    #[tokio::test]
    async fn a_monitor_ended_as_not_permitted_stays_ended_under_the_operators_route() {
        let temp = tempfile::tempdir().expect("tempdir");
        let marker = temp.path().join("monitor-runs");
        let (session_id, job_id) =
            a_lifetime_that_loses_a_resumable_monitor(temp.path(), &marker).await;

        // The next lifetime builds the session with the deny and its model
        // lists its shell jobs: the session's recovery ends the monitor.
        let (script, _seen) = OneCallScript::new("shell_jobs", serde_json::json!({}));
        let lifetime = Lifetime::open(temp.path(), true, script).await;
        let stored = lifetime
            .service
            .load_authoritative_session(&session_id)
            .await
            .expect("load the stored session")
            .expect("the session survives the restart");
        lifetime
            .run_session(stored, Profile::restricted(false, &[MONITOR]))
            .await;
        let ended = settled(&lifetime.job_store, &job_id, &marker, SETTLE).await;
        assert_ended_not_permitted(&ended, "the profile denies `monitor_start`");
        let jobs = DetachedJobService::new(lifetime.job_store.clone());
        let before = jobs.get(&job_id).await.expect("read job").expect("job");

        // The operator's route recovers the session's jobs twice.
        let operator = lifetime.operator_route_manager(temp.path(), &session_id);
        let started = operator_starts_a_monitor(&operator, "sleep 30").await;
        operator
            .cancel_job(&started)
            .await
            .expect("the operator's job cancel");
        tokio::time::sleep(QUIET).await;

        let after = jobs.get(&job_id).await.expect("read job").expect("job");
        assert_eq!(
            runs(&marker),
            1,
            "the operator's route does not run the ended monitor again"
        );
        assert_eq!(
            after.terminal_result, before.terminal_result,
            "the ended job keeps its terminal result"
        );
        assert_eq!(
            (after.attempt_count, after.current_fence),
            (before.attempt_count, before.current_fence),
            "the ended job gets no new attempt"
        );
    }

    /// The host's own monitor route is not bound by a member's deny: an
    /// operator's monitor start for a session runs that session's recorded
    /// resumable monitors again under the host's authority, whoever started
    /// them and whatever the session's profile denies. The row's level: it
    /// builds a job manager exactly as the host's monitor route builds its
    /// own, on the realm's real stores; it does not go through the RPC
    /// handler. The recorded monitor is one the session's own recovery has
    /// not ended yet: its turn here makes no shell job call.
    #[tokio::test]
    async fn an_operators_monitor_start_runs_a_recorded_monitor_again_whatever_the_profile_denies()
    {
        let temp = tempfile::tempdir().expect("tempdir");
        let marker = temp.path().join("monitor-runs");
        let (session_id, job_id) =
            a_lifetime_that_loses_a_resumable_monitor(temp.path(), &marker).await;

        // The next lifetime builds the session with the deny; its turn makes
        // a foreground shell call, which recovers no job. Then the operator
        // starts a monitor of its own for the session.
        let (script, _seen) = OneCallScript::new("shell", serde_json::json!({ "command": "true" }));
        let lifetime = Lifetime::open(temp.path(), true, script).await;
        let stored = lifetime
            .service
            .load_authoritative_session(&session_id)
            .await
            .expect("load the stored session")
            .expect("the session survives the restart");
        lifetime
            .run_session(stored, Profile::restricted(false, &[MONITOR]))
            .await;
        assert_eq!(
            runs(&marker),
            1,
            "the denied session's own turn runs nothing again"
        );

        let operator = lifetime.operator_route_manager(temp.path(), &session_id);
        operator_starts_a_monitor(&operator, COMPLETES_AT_ONCE).await;

        let after = settled(&lifetime.job_store, &job_id, &marker, SETTLE).await;
        assert_eq!(
            after.runs, 2,
            "the operator's start runs the session's recorded monitor again: terminal {:?}",
            after.terminal
        );
    }
}

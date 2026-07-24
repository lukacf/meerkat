#![allow(clippy::expect_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use chrono::Utc;
use meerkat::{
    AttemptClaim, AttemptWriteAuthority, CanonicalArgumentsHash, CreateWorkItemRequest,
    DetachedJobError, DetachedJobService, DetachedJobStore, ExecutionIntentId, HostRunnable,
    HostRunnableInvocation, HostRunnableName, InteractionLineageId, JobAwaitCoordinator,
    JobAwaitDeliverySink, JobDeliveryApplication, JobDeliveryContent, JobDeliveryKind,
    JobDeliverySink, JobFailureCode, JobId, JobOutboxEntry, JobReference, JobSubmissionKey,
    JobSubscription, JobSubscriptionId, JobTerminalEvidenceKind, JobTerminalEvidenceProjection,
    JobTerminalEvidenceProjector, JobTerminalResult, JobWorkGraphLink, MemoryDetachedJobStore,
    MemoryWorkGraphStore, RestartClass, RunnerHandleRef, RunnerIdentity,
    ScheduledDurableJobRunnable, ScheduledJobTemplate, SessionId, ToolIdentity, WorkGraphService,
    WorkItemRef, WorkNamespace, WorkerId,
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

    fn is_persistent(&self) -> bool {
        self.inner.is_persistent()
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

#[async_trait]
impl JobDeliverySink for CountingDeliverySink {
    async fn apply(&self, _application: JobDeliveryApplication) -> Result<(), String> {
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

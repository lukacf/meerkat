//! Force a second real projector to finish after the first takes its snapshot.
//! No sleep, scheduler ordering, or competing-owner stub decides the outcome.

use super::*;
use meerkat::{DetachedJobError, JobOutboxEntry};
use meerkat_jobs::{InsertJobOutcome, JobOutboxPayload, StoredJob};
use meerkat_runtime::{RuntimeDeliveryAuthorityRecord, RuntimeStore};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, PartialEq, Eq)]
enum SnapshotRead {
    PendingList,
    Job,
}

#[derive(Clone, Copy)]
enum RivalOutcome {
    Project,
    ProjectAndDrain,
    MissingAcceptance,
    MismatchedAcceptance,
}

#[derive(Clone, Copy)]
enum StaleMutation {
    None,
    Payload,
    Targets,
}

struct RacingJobStore {
    inner: Arc<MemoryDetachedJobStore>,
    runtime: Arc<InMemoryRuntimeStore>,
    inbox: RuntimeDeliveryInbox,
    read: SnapshotRead,
    outcome: RivalOutcome,
    mutation: StaleMutation,
    fired: AtomicBool,
    after_rival: Mutex<Option<(Option<RuntimeDeliveryAuthorityRecord>, u64)>>,
}

impl RacingJobStore {
    async fn finish_rival(&self, entry: &JobOutboxEntry) {
        let projector = JobOutboxProjector::new(self.inner.clone(), self.inbox.clone());
        let prepared = projector
            .prepare(entry)
            .await
            .expect("prepare rival delivery");
        match self.outcome {
            RivalOutcome::Project | RivalOutcome::ProjectAndDrain => {
                let pass = projector
                    .project_pending(10)
                    .await
                    .expect("rival projection");
                assert!(pass.is_fully_projected());
                assert_eq!(pass.projected.len(), 1);
                if matches!(self.outcome, RivalOutcome::ProjectAndDrain) {
                    let sink = Arc::new(RecordingDeliverySink::default());
                    let drain = JobRuntimeDeliveryApplier::new(self.inbox.clone(), sink.clone())
                        .apply_pending(&prepared.runtime_id, 10)
                        .await
                        .expect("rival applies the accepted delivery");
                    assert!(drain.is_fully_drained());
                    assert_eq!(drain.applied.len(), 1);
                    assert_eq!(sink.applications.lock().await.len(), 1);
                }
            }
            RivalOutcome::MissingAcceptance | RivalOutcome::MismatchedAcceptance => {
                if matches!(self.outcome, RivalOutcome::MismatchedAcceptance) {
                    let original = &prepared.submission;
                    let mismatch = RuntimeDeliverySubmission::new(
                        original.delivery_id().clone(),
                        original.kind(),
                        original.source_id(),
                        original.source_sequence(),
                        original.interaction_lineage_id(),
                        b"different committed payload".to_vec(),
                    )
                    .expect("mismatched submission");
                    self.inbox
                        .submit(&prepared.runtime_id, mismatch)
                        .await
                        .expect("commit conflicting runtime acceptance");
                }
                DetachedJobService::new(self.inner.clone())
                    .mark_delivery_applied(&entry.job_id, entry.delivery_sequence)
                    .await
                    .expect("inject acknowledged producer without exact runtime acceptance");
            }
        }
        let authority = self
            .runtime
            .load_runtime_delivery_authority(&prepared.runtime_id)
            .await
            .expect("authority after rival");
        let revision = self
            .inner
            .get(&entry.job_id)
            .await
            .expect("job after rival")
            .expect("job retained")
            .revision;
        *self.after_rival.lock().await = Some((authority, revision));
    }

    fn corrupt_stale_entry(&self, entry: &mut JobOutboxEntry) {
        match self.mutation {
            StaleMutation::None => {}
            StaleMutation::Payload => {
                entry.payload = JobOutboxPayload::Terminal(JobTerminalResult::Succeeded {
                    result_ref: Some(JobResultRef::new("changed-result").expect("result")),
                });
            }
            StaleMutation::Targets => {
                entry.targets.push(JobSubscription::new(
                    JobSubscriptionId::new("changed-target").expect("subscription"),
                    SessionId::new(),
                    JobDeliveryKind::Notification,
                ));
            }
        }
    }
}

#[async_trait::async_trait]
impl DetachedJobStore for RacingJobStore {
    async fn insert_deduplicated(
        &self,
        job: StoredJob,
    ) -> Result<InsertJobOutcome, DetachedJobError> {
        self.inner.insert_deduplicated(job).await
    }

    async fn get(&self, job_id: &JobId) -> Result<Option<StoredJob>, DetachedJobError> {
        let mut snapshot = self.inner.get(job_id).await?;
        if self.read == SnapshotRead::Job && !self.fired.swap(true, Ordering::SeqCst) {
            let entry = snapshot
                .as_mut()
                .expect("fixture job exists")
                .outbox
                .first_mut()
                .expect("fixture delivery exists");
            assert!(!entry.applied);
            self.finish_rival(entry).await;
            self.corrupt_stale_entry(entry);
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

    async fn predicate_delivery_receipt(
        &self,
        job_id: &JobId,
        identity: &meerkat_jobs::PredicateDeliveryIdentity,
    ) -> Result<Option<meerkat_jobs::PredicateDeliveryReceipt>, DetachedJobError> {
        self.inner
            .predicate_delivery_receipt(job_id, identity)
            .await
    }

    async fn commit_predicate_delivery(
        &self,
        expected_revision: u64,
        replacement: StoredJob,
        commit: meerkat_jobs::PredicateDeliveryCommit,
    ) -> Result<meerkat_jobs::PredicateDeliveryCommitOutcome, DetachedJobError> {
        self.inner
            .commit_predicate_delivery(expected_revision, replacement, commit)
            .await
    }

    async fn list_pending_outbox(
        &self,
        limit: usize,
    ) -> Result<Vec<JobOutboxEntry>, DetachedJobError> {
        let mut snapshot = self.inner.list_pending_outbox(limit).await?;
        if self.read == SnapshotRead::PendingList && !self.fired.swap(true, Ordering::SeqCst) {
            assert_eq!(snapshot.len(), 1);
            let entry = &mut snapshot[0];
            assert!(!entry.applied);
            self.finish_rival(entry).await;
            self.corrupt_stale_entry(entry);
        }
        Ok(snapshot)
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

struct RaceFixture {
    store: Arc<RacingJobStore>,
    projector: JobOutboxProjector,
    job_id: JobId,
    runtime_id: LogicalRuntimeId,
    original: JobOutboxEntry,
}

#[derive(serde::Deserialize)]
struct RetainedSubmission {
    submission: RuntimeDeliverySubmission,
}

impl RaceFixture {
    async fn new(read: SnapshotRead, outcome: RivalOutcome, mutation: StaleMutation) -> Self {
        let inner = Arc::new(MemoryDetachedJobStore::new());
        let session_id = SessionId::new();
        let job_id = completed_job(
            &DetachedJobService::new(inner.clone()),
            session_id.clone(),
            "concurrent-projection",
        )
        .await;
        let original = inner
            .list_pending_outbox(10)
            .await
            .expect("initial outbox")
            .remove(0);
        let runtime = Arc::new(InMemoryRuntimeStore::new());
        let inbox = RuntimeDeliveryInbox::new(runtime.clone());
        let store = Arc::new(RacingJobStore {
            inner,
            runtime,
            inbox: inbox.clone(),
            read,
            outcome,
            mutation,
            fired: AtomicBool::new(false),
            after_rival: Mutex::new(None),
        });
        Self {
            projector: JobOutboxProjector::new(store.clone(), inbox),
            store,
            job_id,
            runtime_id: LogicalRuntimeId::for_session(&session_id),
            original,
        }
    }

    async fn assert_loser_did_not_write(&self) {
        assert!(self.store.fired.load(Ordering::SeqCst));
        let observed = self.store.after_rival.lock().await;
        let (authority, job_revision) = observed.as_ref().expect("rival completed");
        assert_eq!(
            &self
                .store
                .runtime
                .load_runtime_delivery_authority(&self.runtime_id)
                .await
                .expect("authority"),
            authority,
            "the stale projector must only observe acceptance, not submit or acknowledge it"
        );
        let job = self
            .store
            .inner
            .get(&self.job_id)
            .await
            .expect("job")
            .expect("retained job");
        assert_eq!(
            job.revision, *job_revision,
            "no duplicate producer acknowledgement"
        );
        assert!(job.outbox[0].applied);
    }

    async fn assert_one_exact_runtime_row(&self) {
        let row = self
            .store
            .runtime
            .load_runtime_delivery_record(&self.runtime_id, &self.original.runtime_delivery_id())
            .await
            .expect("runtime row")
            .expect("retained runtime row");
        assert_eq!(row.sequence(), 1);
        let retained: RetainedSubmission =
            serde_json::from_slice(row.submission_json()).expect("submission envelope");
        let payload: JobTerminalDeliveryPayload =
            serde_json::from_slice(retained.submission.payload()).expect("terminal payload");
        assert_eq!(payload.job_id, self.job_id);
        assert_eq!(payload.targets, self.original.targets);
        assert_eq!(
            JobOutboxPayload::Terminal(payload.terminal_result),
            self.original.payload
        );
    }
}

#[tokio::test]
async fn stale_pending_scan_observes_a_competing_projection_without_writing() {
    let fixture = RaceFixture::new(
        SnapshotRead::PendingList,
        RivalOutcome::Project,
        StaleMutation::None,
    )
    .await;
    let pass = fixture
        .projector
        .project_pending(10)
        .await
        .expect("stale projection");
    assert!(pass.is_fully_projected(), "{pass:?}");
    assert!(
        pass.projected.is_empty(),
        "the rival already projected this entry"
    );
    fixture.assert_loser_did_not_write().await;
    fixture.assert_one_exact_runtime_row().await;
    assert_eq!(
        fixture
            .store
            .inbox
            .list_pending(&fixture.runtime_id, 10)
            .await
            .expect("pending")
            .len(),
        1
    );
    assert!(
        fixture.projector.prepare(&fixture.original).await.is_err(),
        "public prepare stays strict after acknowledgement"
    );
    let acknowledged = fixture
        .store
        .inner
        .get(&fixture.job_id)
        .await
        .expect("job")
        .expect("retained job");
    assert!(
        fixture
            .projector
            .prepare(&acknowledged.outbox[0])
            .await
            .is_err(),
        "public prepare also rejects an acknowledged input"
    );
}

#[tokio::test]
async fn stale_pending_scan_accepts_exact_retained_delivery_after_runtime_application() {
    let fixture = RaceFixture::new(
        SnapshotRead::PendingList,
        RivalOutcome::ProjectAndDrain,
        StaleMutation::None,
    )
    .await;
    let pass = fixture
        .projector
        .project_pending(10)
        .await
        .expect("stale projection");
    assert!(pass.is_fully_projected(), "{pass:?}");
    assert!(pass.projected.is_empty());
    fixture.assert_loser_did_not_write().await;
    fixture.assert_one_exact_runtime_row().await;
    assert!(
        fixture
            .store
            .inbox
            .list_pending(&fixture.runtime_id, 10)
            .await
            .expect("pending")
            .is_empty()
    );
    assert_eq!(
        fixture
            .store
            .inbox
            .applied_cursor(&fixture.runtime_id)
            .await
            .expect("cursor"),
        1
    );
}

#[tokio::test]
async fn shell_stale_job_read_observes_a_competing_projection_without_writing() {
    let fixture = RaceFixture::new(
        SnapshotRead::Job,
        RivalOutcome::Project,
        StaleMutation::None,
    )
    .await;
    fixture
        .projector
        .project_job(fixture.job_id.as_str())
        .await
        .expect("shell stale projection");
    fixture.assert_loser_did_not_write().await;
    fixture.assert_one_exact_runtime_row().await;
    assert_eq!(
        fixture
            .store
            .inbox
            .list_pending(&fixture.runtime_id, 10)
            .await
            .expect("pending")
            .len(),
        1
    );
}

#[tokio::test]
async fn acknowledgement_does_not_hide_changed_outbox_payload_or_targets() {
    for mutation in [StaleMutation::Payload, StaleMutation::Targets] {
        let fixture =
            RaceFixture::new(SnapshotRead::PendingList, RivalOutcome::Project, mutation).await;
        let pass = fixture
            .projector
            .project_pending(10)
            .await
            .expect("projection pass");
        assert!(pass.projected.is_empty());
        assert_eq!(pass.skipped.len(), 1);
        assert!(
            pass.skipped[0]
                .error
                .contains("disagrees with the pending outbox projection")
        );
        fixture.assert_loser_did_not_write().await;
        fixture.assert_one_exact_runtime_row().await;
    }
}

#[tokio::test]
async fn acknowledgement_without_exact_runtime_acceptance_is_refused_without_repairing_it() {
    for outcome in [
        RivalOutcome::MissingAcceptance,
        RivalOutcome::MismatchedAcceptance,
    ] {
        let fixture =
            RaceFixture::new(SnapshotRead::PendingList, outcome, StaleMutation::None).await;
        let pass = fixture
            .projector
            .project_pending(10)
            .await
            .expect("projection pass");
        assert!(pass.projected.is_empty());
        assert_eq!(
            pass.skipped.len(),
            1,
            "acknowledgement alone cannot prove runtime acceptance"
        );
        fixture.assert_loser_did_not_write().await;
        let row = fixture
            .store
            .runtime
            .load_runtime_delivery_record(
                &fixture.runtime_id,
                &fixture.original.runtime_delivery_id(),
            )
            .await
            .expect("runtime row");
        assert_eq!(
            row.is_some(),
            matches!(outcome, RivalOutcome::MismatchedAcceptance)
        );
    }
}

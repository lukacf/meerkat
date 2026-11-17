//! A session's shell job tools answer for that session's own jobs.
//!
//! A host keeps one job store for every session it serves, and for mob
//! members under their own realm tags. `shell_job_status` and
//! `shell_job_cancel` take a job id from the model. A job that another
//! session submitted, or one under another realm tag, answers exactly as an
//! id nothing was ever submitted under: no status, no output, and no
//! cancellation request.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use meerkat_core::{BlobStore, SessionId};
use meerkat_jobs::{
    AttemptClaim, CanonicalArgumentsHash, DetachedJobError, DetachedJobService, DetachedJobStore,
    ExecutionIntentId, InsertJobOutcome, InteractionLineageId, JobOutboxCommitSignal,
    JobOutboxEntry, JobSpec, JobSubmissionKey, OriginMemberId, PredicateDeliveryCommit,
    PredicateDeliveryCommitOutcome, PredicateDeliveryIdentity, PredicateDeliveryReceipt,
    RestartClass, RunnerHandleRef, RunnerIdentity, SqliteDetachedJobStore, StoredJob, ToolIdentity,
    WorkerId,
};
use meerkat_runtime::RuntimeOpsLifecycleRegistry;
use meerkat_store::FsBlobStore;
use serde_json::json;

use super::{
    DurableShellJobRuntime, JobId, JobManager, MonitorStartOptions, ShellConfig,
    ShellJobCancelTool, ShellJobDeliveryProjector, ShellJobStatusTool, ShellJobsListTool,
};
use crate::builtin::{BuiltinTool, ToolOutput};

/// The realm tag of a plain session.
const PLAIN_REALM: &str = "plain-realm";
/// The realm tag of a mob's members.
const MEMBER_REALM: &str = "mob.team";

struct DeliveryProjection;

#[async_trait::async_trait]
impl ShellJobDeliveryProjector for DeliveryProjection {
    async fn project_job(&self, _job_id: &str) -> Result<(), String> {
        Ok(())
    }
}

/// One host's job and blob stores. Every session's tools are built over them.
struct Host {
    root: tempfile::TempDir,
    work: PathBuf,
    store: Arc<dyn DetachedJobStore>,
    blobs: Arc<dyn BlobStore>,
}

impl Host {
    fn new() -> Self {
        Self::open(tempfile::tempdir().unwrap())
    }

    /// A host that runs mobs: its job store records, with each submission,
    /// the member the submitting session serves.
    fn with_members() -> (Self, Arc<MemberJobStore>) {
        let mut host = Self::new();
        let members = Arc::new(MemberJobStore {
            inner: host.store.clone(),
            serving: std::sync::Mutex::default(),
        });
        host.store = members.clone();
        (host, members)
    }

    fn open(root: tempfile::TempDir) -> Self {
        let work = root.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let work = work.canonicalize().unwrap();
        let store: Arc<dyn DetachedJobStore> =
            Arc::new(SqliteDetachedJobStore::open(root.path().join("jobs.db")).unwrap());
        let blobs: Arc<dyn BlobStore> = Arc::new(FsBlobStore::new(root.path().join("blobs")));
        Self {
            root,
            work,
            store,
            blobs,
        }
    }

    /// The same directory opened again, as a restarted host opens it.
    fn reopened(self) -> Self {
        let Self { root, .. } = self;
        Self::open(root)
    }

    /// The shell job tools of one session under one realm tag.
    fn seat(&self, realm_tag: &str, session: &SessionId) -> Seat {
        let runtime = DurableShellJobRuntime::new(
            realm_tag,
            session.clone(),
            self.store.clone(),
            self.blobs.clone(),
            Arc::new(DeliveryProjection),
        )
        .unwrap();
        let config = ShellConfig {
            enabled: true,
            shell: "sh".into(),
            shell_path: Some(PathBuf::from("/bin/sh")),
            project_root: self.work.clone(),
            ..ShellConfig::default()
        };
        Seat {
            manager: Arc::new(
                JobManager::new(config)
                    .bind_canonical_async_ops(
                        session.clone(),
                        Arc::new(RuntimeOpsLifecycleRegistry::new()),
                    )
                    .with_durable_job_runtime(runtime),
            ),
        }
    }

    /// A job the shell tools did not start: a host's own durable job with
    /// its own runner, recorded for `session` under `realm_tag`. It waits in
    /// the queue, or a host worker runs it under a long lease.
    async fn host_job(
        &self,
        realm_tag: &str,
        session: &SessionId,
        name: &str,
        running: bool,
    ) -> String {
        let service = DetachedJobService::new(self.store.clone());
        let receipt = service
            .submit(JobSpec::new(
                realm_tag,
                session.clone(),
                ExecutionIntentId::from_string(format!("host-intent-{name}")).unwrap(),
                InteractionLineageId::from_string(format!("host-lineage-{name}")).unwrap(),
                ToolIdentity::new("host_tool", "v1").unwrap(),
                RunnerIdentity::new("host.runner", "v1").unwrap(),
                RestartClass::NonResumable,
                CanonicalArgumentsHash::new(format!("host-arguments-{name}")).unwrap(),
                JobSubmissionKey::new(format!("host:{name}")).unwrap(),
            ))
            .await
            .unwrap();
        if running {
            let now_ms = u64::try_from(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis(),
            )
            .unwrap();
            service
                .claim_attempt(
                    &receipt.job_id,
                    AttemptClaim::new(
                        WorkerId::new("host-worker").unwrap(),
                        now_ms,
                        now_ms.saturating_add(3_600_000),
                        RunnerHandleRef::new(format!("host-runner:{name}")).unwrap(),
                    ),
                )
                .await
                .unwrap();
        }
        receipt.job_id.to_string()
    }

    /// The member address the job's record carries, if any.
    async fn origin_member(&self, job_id: &str) -> Option<OriginMemberId> {
        self.store
            .get(&meerkat_jobs::JobId::new(job_id).unwrap())
            .await
            .unwrap()
            .expect("the job is in the host's store")
            .spec
            .origin_member_id
    }

    async fn snapshot(&self, job_id: &str) -> meerkat_jobs::JobSnapshot {
        DetachedJobService::new(self.store.clone())
            .get(&meerkat_jobs::JobId::new(job_id).unwrap())
            .await
            .unwrap()
            .expect("the job is in the host's store")
    }

    /// Nothing asked for the job's cancellation and it has not ended.
    async fn assert_untouched(&self, job_id: &str) {
        let snapshot = self.snapshot(job_id).await;
        assert!(
            !snapshot.cancel_requested,
            "no cancellation was requested for {job_id}"
        );
        assert!(
            snapshot.terminal_result.is_none(),
            "{job_id} is still running: {:?}",
            snapshot.terminal_result
        );
    }

    async fn ended(&self, job_id: &str) -> meerkat_jobs::JobSnapshot {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let snapshot = self.snapshot(job_id).await;
                if snapshot.terminal_result.is_some() {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the job ends")
    }
}

/// The job store of a host that runs mobs. It knows which member each
/// session serves now and records that member's delivery address with the
/// session's submissions, the address the job's notices are then sent to.
struct MemberJobStore {
    inner: Arc<dyn DetachedJobStore>,
    serving: std::sync::Mutex<HashMap<SessionId, OriginMemberId>>,
}

impl MemberJobStore {
    /// `session` serves `member` from now on, and no other session does.
    fn serve(&self, member: &str, session: &SessionId) {
        let member = OriginMemberId::new(member).unwrap();
        let mut serving = self.serving.lock().unwrap();
        serving.retain(|_, served| served != &member);
        serving.insert(session.clone(), member);
    }

    fn member_of(&self, session: &SessionId) -> Option<OriginMemberId> {
        self.serving.lock().unwrap().get(session).cloned()
    }
}

#[async_trait::async_trait]
impl DetachedJobStore for MemberJobStore {
    async fn insert_deduplicated(
        &self,
        mut job: StoredJob,
    ) -> Result<InsertJobOutcome, DetachedJobError> {
        if job.spec.origin_member_id.is_none() {
            job.spec.origin_member_id = self.member_of(&job.spec.origin_session_id);
        }
        self.inner.insert_deduplicated(job).await
    }

    async fn get(
        &self,
        job_id: &meerkat_jobs::JobId,
    ) -> Result<Option<StoredJob>, DetachedJobError> {
        self.inner.get(job_id).await
    }

    async fn get_by_submission_key(
        &self,
        realm_id: &str,
        submission_key: &JobSubmissionKey,
    ) -> Result<Option<StoredJob>, DetachedJobError> {
        self.inner
            .get_by_submission_key(realm_id, submission_key)
            .await
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
        job_id: &meerkat_jobs::JobId,
        identity: &PredicateDeliveryIdentity,
    ) -> Result<Option<PredicateDeliveryReceipt>, DetachedJobError> {
        self.inner
            .predicate_delivery_receipt(job_id, identity)
            .await
    }

    async fn commit_predicate_delivery(
        &self,
        expected_revision: u64,
        replacement: StoredJob,
        commit: PredicateDeliveryCommit,
    ) -> Result<PredicateDeliveryCommitOutcome, DetachedJobError> {
        self.inner
            .commit_predicate_delivery(expected_revision, replacement, commit)
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

    fn outbox_commit_signal(&self) -> JobOutboxCommitSignal {
        self.inner.outbox_commit_signal()
    }
}

/// The two tools that take a job id from the model.
#[derive(Debug, Clone, Copy)]
enum Door {
    Status,
    Cancel,
}

const DOORS: [Door; 2] = [Door::Status, Door::Cancel];

/// One session's shell job tools.
struct Seat {
    manager: Arc<JobManager>,
}

impl Seat {
    /// The tool's whole answer: what the model reads and the typed value, or
    /// the whole error.
    async fn ask(&self, door: Door, job_id: &str) -> Result<String, String> {
        let args = json!({ "job_id": job_id });
        let answer = match door {
            Door::Status => {
                ShellJobStatusTool::new(self.manager.clone())
                    .call(args)
                    .await
            }
            Door::Cancel => {
                ShellJobCancelTool::new(self.manager.clone())
                    .call(args)
                    .await
            }
        };
        match answer {
            Ok(ToolOutput::JsonRenderedAsText { value, text }) => Ok(format!("{text}\n{value}")),
            Ok(other) => Ok(other.into_json().expect("a JSON answer").to_string()),
            Err(error) => Err(format!("{error:?}")),
        }
    }

    /// The ids `shell_jobs` lists.
    async fn listed(&self) -> BTreeSet<String> {
        ShellJobsListTool::new(self.manager.clone())
            .call(json!({}))
            .await
            .expect("list")
            .into_json()
            .expect("a JSON list")
            .as_array()
            .expect("an array")
            .iter()
            .map(|job| job["id"].as_str().expect("an id").to_string())
            .collect()
    }

    /// A job that stays running until it is cancelled.
    async fn start_running(&self, call: &str) -> String {
        self.manager
            .spawn_job_for_call("sleep 30", None, 60, call)
            .await
            .expect("spawn")
            .to_string()
    }

    /// A job that has printed `output` and completed.
    async fn start_completed(&self, output: &str, call: &str) -> String {
        let job_id = self
            .manager
            .spawn_job_for_call(&format!("printf {output}"), None, 30, call)
            .await
            .expect("spawn")
            .to_string();
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let own = self
                    .ask(Door::Status, &job_id)
                    .await
                    .expect("a session reads its own job");
                if own.contains(output) && own.contains("\"completed\"") {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the job completes with its output");
        job_id
    }

    /// The answer this session gets for an id nothing was ever submitted
    /// under, with `job_id` written where that id stands.
    async fn unknown_answer_about(&self, door: Door, job_id: &str) -> String {
        let made_up = meerkat_jobs::JobId::generated().to_string();
        let answer = self
            .ask(door, &made_up)
            .await
            .expect_err("an id nothing was submitted under has no job");
        answer.replace(&made_up, job_id)
    }

    /// This session's answer about `job_id` is the answer of an unknown id,
    /// byte for byte, and carries none of `withheld`.
    async fn assert_reads_as_unknown(&self, door: Door, job_id: &str, withheld: &[&str]) {
        let unknown = self.unknown_answer_about(door, job_id).await;
        let answer = match self.ask(door, job_id).await {
            Ok(answer) => panic!("{door:?} of {job_id} answered a job: {answer}"),
            Err(answer) => answer,
        };
        assert_eq!(answer, unknown, "{door:?} of {job_id}");
        for text in withheld {
            assert!(!answer.contains(text), "{door:?} of {job_id}: {answer}");
        }
        assert!(
            self.manager
                .canonical_operation_for_job(&JobId::from_string(job_id))
                .is_none(),
            "{door:?} of {job_id} registered no operation in this session"
        );
    }
}

/// The owner still reads its completed job with its output and its running
/// job as running, then cancels the running one itself.
async fn assert_owner_keeps_its_jobs(
    host: &Host,
    owner: &Seat,
    completed: &str,
    output: &str,
    running: &str,
) {
    let read = owner
        .ask(Door::Status, completed)
        .await
        .expect("the owner reads its completed job");
    assert!(read.contains(output), "{read}");
    let read = owner
        .ask(Door::Status, running)
        .await
        .expect("the owner reads its running job");
    assert!(read.contains("\"running\""), "{read}");
    host.assert_untouched(running).await;
    let cancelled = owner
        .ask(Door::Cancel, running)
        .await
        .expect("the owner cancels its running job");
    assert!(cancelled.contains(running), "{cancelled}");
    let ended = host.ended(running).await;
    assert_eq!(
        ended.terminal_result,
        Some(meerkat_jobs::JobTerminalResult::Cancelled)
    );
}

#[tokio::test]
async fn a_job_of_another_session_reads_as_an_unknown_job() {
    let host = Host::new();
    let owner = host.seat(PLAIN_REALM, &SessionId::new());
    let other = host.seat(PLAIN_REALM, &SessionId::new());
    let completed = owner.start_completed("owner-output", "call-1").await;
    let running = owner.start_running("call-2").await;

    other
        .assert_reads_as_unknown(Door::Status, &completed, &["owner-output", "printf"])
        .await;
    other
        .assert_reads_as_unknown(Door::Status, &running, &["sleep", "running"])
        .await;

    assert_owner_keeps_its_jobs(&host, &owner, &completed, "owner-output", &running).await;
}

#[tokio::test]
async fn a_cancel_of_another_sessions_job_answers_as_an_unknown_job_and_requests_nothing() {
    let host = Host::new();
    let owner = host.seat(PLAIN_REALM, &SessionId::new());
    let other = host.seat(PLAIN_REALM, &SessionId::new());
    let completed = owner.start_completed("owner-output", "call-1").await;
    let running = owner.start_running("call-2").await;

    other
        .assert_reads_as_unknown(Door::Cancel, &running, &["cancel"])
        .await;
    host.assert_untouched(&running).await;
    // A completed job of this session would answer "not running"; another
    // session's answers as unknown.
    other
        .assert_reads_as_unknown(Door::Cancel, &completed, &["running"])
        .await;
    host.assert_untouched(&running).await;

    assert_owner_keeps_its_jobs(&host, &owner, &completed, "owner-output", &running).await;
}

/// A mob member's jobs carry the mob's realm tag in the host's one store. The
/// tag alone separates them: the same holds when both sides are served under
/// one session id.
#[tokio::test]
async fn a_job_under_another_realm_tag_reads_as_an_unknown_job() {
    for same_session_id in [false, true] {
        let host = Host::new();
        let member_session = SessionId::new();
        let plain_session = if same_session_id {
            member_session.clone()
        } else {
            SessionId::new()
        };
        let member = host.seat(MEMBER_REALM, &member_session);
        let plain = host.seat(PLAIN_REALM, &plain_session);
        let member_completed = member.start_completed("member-output", "call-1").await;
        let member_running = member.start_running("call-2").await;
        let plain_completed = plain.start_completed("plain-output", "call-1").await;
        let plain_running = plain.start_running("call-2").await;

        plain
            .assert_reads_as_unknown(Door::Status, &member_completed, &["member-output"])
            .await;
        plain
            .assert_reads_as_unknown(Door::Status, &member_running, &["sleep", "running"])
            .await;
        member
            .assert_reads_as_unknown(Door::Status, &plain_completed, &["plain-output"])
            .await;
        member
            .assert_reads_as_unknown(Door::Status, &plain_running, &["sleep", "running"])
            .await;

        assert_owner_keeps_its_jobs(
            &host,
            &member,
            &member_completed,
            "member-output",
            &member_running,
        )
        .await;
        assert_owner_keeps_its_jobs(
            &host,
            &plain,
            &plain_completed,
            "plain-output",
            &plain_running,
        )
        .await;
    }
}

#[tokio::test]
async fn a_cancel_of_a_job_under_another_realm_tag_answers_as_an_unknown_job_and_requests_nothing()
{
    for same_session_id in [false, true] {
        let host = Host::new();
        let member_session = SessionId::new();
        let plain_session = if same_session_id {
            member_session.clone()
        } else {
            SessionId::new()
        };
        let member = host.seat(MEMBER_REALM, &member_session);
        let plain = host.seat(PLAIN_REALM, &plain_session);
        let member_completed = member.start_completed("member-output", "call-1").await;
        let member_running = member.start_running("call-2").await;
        let plain_completed = plain.start_completed("plain-output", "call-1").await;
        let plain_running = plain.start_running("call-2").await;

        plain
            .assert_reads_as_unknown(Door::Cancel, &member_running, &["cancel"])
            .await;
        member
            .assert_reads_as_unknown(Door::Cancel, &plain_running, &["cancel"])
            .await;
        plain
            .assert_reads_as_unknown(Door::Cancel, &member_completed, &["running"])
            .await;
        member
            .assert_reads_as_unknown(Door::Cancel, &plain_completed, &["running"])
            .await;
        host.assert_untouched(&member_running).await;
        host.assert_untouched(&plain_running).await;

        assert_owner_keeps_its_jobs(
            &host,
            &member,
            &member_completed,
            "member-output",
            &member_running,
        )
        .await;
        assert_owner_keeps_its_jobs(
            &host,
            &plain,
            &plain_completed,
            "plain-output",
            &plain_running,
        )
        .await;
    }
}

/// A manager without a durable store keeps its jobs in its own memory, and a
/// host builds one manager per session: a job held in one session's memory
/// is no job of another session's tools.
#[tokio::test]
async fn a_job_held_in_another_sessions_memory_answers_as_an_unknown_job() {
    let memory_seat = || Seat {
        manager: Arc::new(
            JobManager::new(ShellConfig::default()).bind_canonical_async_ops(
                SessionId::new(),
                Arc::new(RuntimeOpsLifecycleRegistry::new()),
            ),
        ),
    };
    let owner = memory_seat();
    let other = memory_seat();
    let job_id = owner
        .manager
        .register_synthetic_running_job("sleep 30", None, 60)
        .await
        .expect("a job in the owner's memory")
        .to_string();

    for door in DOORS {
        other
            .assert_reads_as_unknown(door, &job_id, &["sleep", "running"])
            .await;
    }

    let read = owner
        .ask(Door::Status, &job_id)
        .await
        .expect("the owner reads its job");
    assert!(read.contains("\"running\""), "{read}");
    let cancelled = owner
        .ask(Door::Cancel, &job_id)
        .await
        .expect("the owner cancels its job");
    assert!(cancelled.contains("\"cancelled\""), "{cancelled}");
}

/// The store also holds jobs the shell tools did not start. Such a job is no
/// shell job of any session, the one it is recorded for included: its status
/// answers as an unknown id.
#[tokio::test]
async fn a_job_of_another_kind_reads_as_an_unknown_job() {
    let host = Host::new();
    let session = SessionId::new();
    let seat = host.seat(PLAIN_REALM, &session);
    let other = host.seat(PLAIN_REALM, &SessionId::new());
    let queued = host.host_job(PLAIN_REALM, &session, "queued", false).await;
    let running = host.host_job(PLAIN_REALM, &session, "running", true).await;

    for asker in [&seat, &other] {
        for job_id in [&queued, &running] {
            asker
                .assert_reads_as_unknown(Door::Status, job_id, &["runner", "specification"])
                .await;
        }
    }
    host.assert_untouched(&queued).await;
    host.assert_untouched(&running).await;
}

/// A cancel by the shell tools reaches shell jobs only: a job of another kind
/// answers as an unknown id and nothing is requested for it.
#[tokio::test]
async fn a_cancel_of_a_job_of_another_kind_answers_as_an_unknown_job_and_requests_nothing() {
    let host = Host::new();
    let session = SessionId::new();
    let seat = host.seat(PLAIN_REALM, &session);
    let other = host.seat(PLAIN_REALM, &SessionId::new());
    let queued = host.host_job(PLAIN_REALM, &session, "queued", false).await;
    let running = host.host_job(PLAIN_REALM, &session, "running", true).await;

    for asker in [&seat, &other] {
        for job_id in [&running, &queued] {
            asker
                .assert_reads_as_unknown(Door::Cancel, job_id, &["cancel"])
                .await;
            host.assert_untouched(job_id).await;
        }
    }
}

/// A fork child starts from its parent's transcript, which holds the ids of
/// the parent's jobs. The child is another member in another session: it
/// never owns them.
#[tokio::test]
async fn a_fork_child_does_not_own_its_parents_jobs() {
    let (host, members) = Host::with_members();
    let parent_session = SessionId::new();
    let child_session = SessionId::new();
    members.serve("rt:mob-member:team:1:lead:1", &parent_session);
    members.serve("rt:mob-member:team:1:lead-fork:1", &child_session);
    let parent = host.seat(MEMBER_REALM, &parent_session);
    let child = host.seat(MEMBER_REALM, &child_session);
    let completed = parent.start_completed("parent-output", "call-1").await;
    let running = parent.start_running("call-2").await;
    assert_eq!(
        host.origin_member(&running).await,
        members.member_of(&parent_session),
        "the job's record carries its member's address"
    );

    for door in DOORS {
        child
            .assert_reads_as_unknown(door, &running, &["sleep"])
            .await;
        child
            .assert_reads_as_unknown(door, &completed, &["parent-output", "printf"])
            .await;
    }
    assert!(child.listed().await.is_empty());

    assert_owner_keeps_its_jobs(&host, &parent, &completed, "parent-output", &running).await;
}

/// The stated limit of "the caller's own": a job belongs to the session that
/// submitted it. When a member is moved to a successor session, the job's
/// record still carries the member's address and the host reports the
/// successor as serving that member (so the job's notices go to the
/// successor), yet the successor's shell job tools answer the predecessor's
/// jobs as unknown ids and list none of them. The predecessor session, and
/// the host's own job calls, still reach them.
#[tokio::test]
async fn a_members_successor_session_reads_its_predecessors_jobs_as_unknown_jobs() {
    let (host, members) = Host::with_members();
    let member = "rt:mob-member:team:1:worker:1";
    let predecessor_session = SessionId::new();
    let successor_session = SessionId::new();
    members.serve(member, &predecessor_session);
    let predecessor = host.seat(MEMBER_REALM, &predecessor_session);
    let completed = predecessor
        .start_completed("predecessor-output", "call-1")
        .await;
    let running = predecessor.start_running("call-2").await;

    members.serve(member, &successor_session);
    let successor = host.seat(MEMBER_REALM, &successor_session);
    assert_eq!(members.member_of(&predecessor_session), None);
    assert_eq!(
        host.origin_member(&running).await,
        members.member_of(&successor_session),
        "the successor serves the member the job's notices are addressed to"
    );

    for door in DOORS {
        successor
            .assert_reads_as_unknown(door, &running, &["sleep"])
            .await;
        successor
            .assert_reads_as_unknown(door, &completed, &["predecessor-output", "printf"])
            .await;
    }
    host.assert_untouched(&running).await;
    assert!(successor.listed().await.is_empty());

    assert_owner_keeps_its_jobs(
        &host,
        &predecessor,
        &completed,
        "predecessor-output",
        &running,
    )
    .await;
}

/// A monitor a session started itself is a job of its shell manager: the
/// session reads, lists and cancels it, from the tools that started it and
/// from tools built again for the session, and another session does not.
#[tokio::test]
async fn a_session_reads_and_cancels_its_own_monitor() {
    let host = Host::new();
    let owner_session = SessionId::new();
    let owner = host.seat(PLAIN_REALM, &owner_session);
    let other = host.seat(PLAIN_REALM, &SessionId::new());
    let monitor = owner
        .manager
        .spawn_monitor_for_call(
            "sleep 30",
            None,
            60,
            "monitor-1",
            MonitorStartOptions::default(),
        )
        .await
        .expect("a monitor")
        .to_string();

    for door in DOORS {
        other
            .assert_reads_as_unknown(door, &monitor, &["sleep"])
            .await;
    }
    host.assert_untouched(&monitor).await;

    let rebuilt = host.seat(PLAIN_REALM, &owner_session);
    for seat in [&owner, &rebuilt] {
        assert_eq!(seat.listed().await, BTreeSet::from([monitor.clone()]));
        let read = seat
            .ask(Door::Status, &monitor)
            .await
            .expect("the session reads its own monitor");
        assert!(read.contains("\"running\""), "{read}");
    }
    let cancelled = rebuilt
        .ask(Door::Cancel, &monitor)
        .await
        .expect("the session cancels its own monitor");
    assert!(cancelled.contains(&monitor), "{cancelled}");
    assert_eq!(
        host.ended(&monitor).await.terminal_result,
        Some(meerkat_jobs::JobTerminalResult::Cancelled)
    );
}

/// Controls: each session reads, lists and cancels exactly its own jobs.
#[tokio::test]
async fn each_session_reads_lists_and_cancels_its_own_jobs() {
    let host = Host::new();
    let first = host.seat(PLAIN_REALM, &SessionId::new());
    let second = host.seat(PLAIN_REALM, &SessionId::new());
    let member = host.seat(MEMBER_REALM, &SessionId::new());
    let seats = [
        (&first, "first-output"),
        (&second, "second-output"),
        (&member, "member-output"),
    ];

    let mut jobs = Vec::new();
    for (seat, output) in seats {
        let completed = seat.start_completed(output, "call-1").await;
        let running = seat.start_running("call-2").await;
        jobs.push((seat, output, completed, running));
    }

    for (seat, _, completed, running) in &jobs {
        assert_eq!(
            seat.listed().await,
            BTreeSet::from([completed.clone(), running.clone()]),
            "a session lists its own jobs and no others"
        );
    }
    for (seat, output, completed, running) in &jobs {
        assert_owner_keeps_its_jobs(&host, seat, completed, output, running).await;
    }
    for (seat, _, completed, running) in &jobs {
        assert_eq!(
            seat.listed().await,
            BTreeSet::from([completed.clone(), running.clone()]),
            "the list is the same after the cancellations"
        );
        // An ended job of the session itself still answers as that job.
        let again = seat
            .ask(Door::Cancel, running)
            .await
            .expect_err("an ended job is not running");
        assert!(again.contains("not running"), "{again}");
    }
}

/// The same answers from tools built again over the reopened stores, where
/// every read comes from the durable records.
#[tokio::test]
async fn after_a_reopen_jobs_of_others_still_read_as_unknown_jobs() {
    let host = Host::new();
    let owner_session = SessionId::new();
    let other_session = SessionId::new();
    let (completed, running) = {
        let owner = host.seat(PLAIN_REALM, &owner_session);
        let completed = owner.start_completed("owner-output", "call-1").await;
        let running = owner.start_running("call-2").await;
        (completed, running)
    };

    let host = host.reopened();
    let owner = host.seat(PLAIN_REALM, &owner_session);
    let other = host.seat(PLAIN_REALM, &other_session);
    let member = host.seat(MEMBER_REALM, &other_session);
    let member_as_owner = host.seat(MEMBER_REALM, &owner_session);

    for asker in [&other, &member, &member_as_owner] {
        for door in DOORS {
            asker
                .assert_reads_as_unknown(door, &completed, &["owner-output", "printf"])
                .await;
            asker
                .assert_reads_as_unknown(door, &running, &["sleep"])
                .await;
        }
        assert!(asker.listed().await.is_empty());
    }
    host.assert_untouched(&running).await;

    assert_eq!(
        owner.listed().await,
        BTreeSet::from([completed.clone(), running.clone()])
    );
    assert_owner_keeps_its_jobs(&host, &owner, &completed, "owner-output", &running).await;
}

//! A session's shell job tools answer for that session's own jobs.
//!
//! A host keeps one job store for every session it serves, and for mob
//! members under their own realm tags. `shell_job_status` and
//! `shell_job_cancel` take a job id from the model. A job that another
//! session submitted, or one under another realm tag, answers exactly as an
//! id nothing was ever submitted under: no status, no output, and no
//! cancellation request.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use meerkat_core::{BlobStore, SessionId};
use meerkat_jobs::{DetachedJobService, DetachedJobStore, SqliteDetachedJobStore};
use meerkat_runtime::RuntimeOpsLifecycleRegistry;
use meerkat_store::FsBlobStore;
use serde_json::json;

use super::{
    DurableShellJobRuntime, JobId, JobManager, ShellConfig, ShellJobCancelTool,
    ShellJobDeliveryProjector, ShellJobStatusTool, ShellJobsListTool,
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

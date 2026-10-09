//! Two RPC runtimes in two real processes on one SQLite realm (#1813).
//!
//! Every RPC runtime arms its own library delivery owner over the realm's
//! runtime delivery inbox, so two processes on one realm race to apply every
//! pending job delivery row. These cases pin, from durable state only:
//!
//! 1. an Event row for an idle session no process hosts takes effect once
//!    (one admitted input, one turn), and only one process attempts it;
//! 2. a Notification row for an idle persisted session appends exactly one
//!    System-context message, and only one process attempts it;
//! 3. a Notification row for a session LIVE in process B is not applied by
//!    process A: A writes nothing under B's live session.
//!
//! The parent seeds the session (in a setup child that exits, so nothing
//! hosts it), commits the completed job straight into the realm's job store,
//! then releases both owner children at once. Each child reports its first
//! delivery pass. Synchronization is by fifos and process exit; timeouts are
//! hang guards only.

#![cfg(any(target_os = "linux", target_os = "macos"))]
#![allow(
    clippy::expect_used,
    clippy::large_futures,
    clippy::panic,
    clippy::unwrap_used
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use meerkat::AgentFactory;
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::{Config, ConfigRuntime, MemoryConfigStore, Message};
use meerkat_rpc::server::RpcServer;
use meerkat_rpc::session_runtime::SessionRuntime;
use meerkat_runtime::SessionServiceRuntimeExt as _;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

const ROOT_ENV: &str = "MEERKAT_MULTI_PROCESS_DELIVERY_ROOT";
const ROLE_ENV: &str = "MEERKAT_MULTI_PROCESS_DELIVERY_ROLE";
const CHILD_TEST: &str = "multi_process_delivery_child";
const REALM: &str = "multi-process-delivery-realm";
const SEED_PROMPT: &str = "seed prompt";
/// Hang guard for every awaited cross-process signal; never pacing.
const GUARD: Duration = Duration::from_secs(120);

/// Scripted model: answers every request with a fixed text and counts calls.
struct TextClient {
    requests: AtomicUsize,
}

#[async_trait::async_trait]
impl LlmClient for TextClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> meerkat_llm_core::LlmStream<'a> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        let events = vec![
            LlmEvent::TextDelta {
                delta: "answered".to_string(),
                meta: None,
            },
            LlmEvent::UsageUpdate {
                usage: meerkat_core::TurnUsage::host_declared(
                    meerkat_core::Provider::OpenAI,
                    &request.model,
                    meerkat_core::Usage::default(),
                ),
            },
            LlmEvent::Done {
                outcome: LlmDoneOutcome::Success {
                    stop_reason: meerkat_core::StopReason::EndTurn,
                },
            },
        ];
        Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

fn new_client() -> Arc<TextClient> {
    Arc::new(TextClient {
        requests: AtomicUsize::new(0),
    })
}

async fn open_persistence(root: &Path) -> meerkat::PersistenceBundle {
    let (_manifest, persistence) = meerkat::open_realm_persistence_in(
        root,
        REALM,
        Some(meerkat_store::RealmBackend::Sqlite),
        Some(meerkat_store::RealmOrigin::Explicit),
    )
    .await
    .expect("open realm persistence");
    persistence
}

/// An RPC runtime over the shared realm. Its delivery owner is armed when an
/// [`RpcServer`] is built over it, or by an explicit
/// [`SessionRuntime::arm_runtime_delivery_owner`].
async fn build_runtime(root: &Path, client: Arc<TextClient>) -> Arc<SessionRuntime> {
    let persistence = open_persistence(root).await;
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("project dir");
    let factory = AgentFactory::new(root.join("sessions")).project_root(&project);
    let config = Config::default();
    let runtime = SessionRuntime::new(
        factory,
        config.clone(),
        10,
        persistence,
        meerkat_rpc::router::NotificationSink::noop(),
    );
    runtime.set_default_llm_client(Some(client));
    runtime.set_realm_context(
        Some(meerkat_core::connection::RealmId::parse(REALM).expect("realm id")),
        None,
        None,
    );
    let config_store: Arc<dyn meerkat_core::ConfigStore> =
        Arc::new(MemoryConfigStore::new(config, meerkat_models::canonical()));
    runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
        config_store,
        root.join("config_state.json"),
    )));
    Arc::new(runtime)
}

struct RpcClient {
    writer: DuplexStream,
    reader: BufReader<DuplexStream>,
    next_id: u64,
}

impl RpcClient {
    /// Serve `runtime` over an in-process duplex. Building the server arms
    /// the runtime's delivery owner.
    fn serve(runtime: Arc<SessionRuntime>) -> Self {
        let config_store: Arc<dyn meerkat_core::ConfigStore> = Arc::new(MemoryConfigStore::new(
            Config::default(),
            meerkat_models::canonical(),
        ));
        let (server_reader, client_writer) = tokio::io::duplex(1 << 16);
        let (client_reader, server_writer) = tokio::io::duplex(1 << 16);
        let mut server = RpcServer::new(
            BufReader::new(server_reader),
            server_writer,
            runtime,
            config_store,
        )
        .expect("build the rpc server");
        tokio::spawn(async move {
            let _ = server.run().await;
        });
        Self {
            writer: client_writer,
            reader: BufReader::new(client_reader),
            next_id: 1,
        }
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let line = format!(
            "{}\n",
            json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
        );
        self.writer
            .write_all(line.as_bytes())
            .await
            .expect("write request");
        self.writer.flush().await.expect("flush request");
        loop {
            let mut line = String::new();
            self.reader.read_line(&mut line).await.expect("read line");
            assert!(!line.is_empty(), "rpc server closed");
            let value: Value = serde_json::from_str(&line).expect("parse json");
            if value.get("id").and_then(Value::as_u64) == Some(id) {
                assert!(value["error"].is_null(), "{method} failed: {value}");
                return value["result"].clone();
            }
        }
    }
}

fn mkfifo(path: &Path) {
    let status = std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .expect("run mkfifo");
    assert!(status.success(), "mkfifo {}", path.display());
}

/// Block (no polling) until a line arrives in `fifo`.
async fn read_fifo(fifo: PathBuf) -> String {
    tokio::time::timeout(
        GUARD,
        tokio::task::spawn_blocking(move || std::fs::read_to_string(fifo)),
    )
    .await
    .expect("cross-process signal within the hang guard")
    .unwrap()
    .unwrap()
    .trim()
    .to_string()
}

async fn write_fifo(fifo: PathBuf, line: &'static str) {
    tokio::time::timeout(
        GUARD,
        tokio::task::spawn_blocking(move || std::fs::write(fifo, format!("{line}\n"))),
    )
    .await
    .expect("cross-process signal within the hang guard")
    .unwrap()
    .unwrap();
}

fn report_fifo(root: &Path, role: &str) -> PathBuf {
    root.join(format!("{role}.report"))
}

fn go_fifo(root: &Path, role: &str) -> PathBuf {
    root.join(format!("{role}.go"))
}

/// A child process in `role`, owned by a watcher thread. If the child exits
/// while the parent still awaits one of its reports, the watcher writes
/// `exited` into its report fifo so the parent's read fails with the child's
/// log instead of hanging.
struct Child {
    role: String,
    pid: u32,
    exited: std::sync::mpsc::Receiver<()>,
    reports_done: Arc<std::sync::atomic::AtomicBool>,
}

fn spawn_child(root: &Path, role: &str) -> Child {
    mkfifo(&report_fifo(root, role));
    mkfifo(&go_fifo(root, role));
    let log = std::fs::File::create(root.join(format!("{role}.log"))).expect("child log");
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", CHILD_TEST, "--nocapture", "--test-threads=1"])
        .env(ROOT_ENV, root)
        .env(ROLE_ENV, role)
        .stdout(log.try_clone().expect("child log"))
        .stderr(log)
        .spawn()
        .expect("spawn child");
    let pid = child.id();
    let (exited_tx, exited) = std::sync::mpsc::channel::<()>();
    let reports_done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let fifo = report_fifo(root, role);
    let watcher_done = Arc::clone(&reports_done);
    std::thread::spawn(move || {
        let mut child = child;
        let _ = child.wait();
        let _ = exited_tx.send(());
        if !watcher_done.load(Ordering::SeqCst) {
            let _ = std::fs::write(fifo, "exited\n");
        }
    });
    Child {
        role: role.to_string(),
        pid,
        exited,
        reports_done,
    }
}

impl Child {
    async fn expect_report(&self, root: &Path, expected: &str) {
        let line = read_fifo(report_fifo(root, &self.role)).await;
        if line != expected {
            let log = std::fs::read_to_string(root.join(format!("{}.log", self.role)))
                .unwrap_or_default();
            panic!(
                "child {} reported {line:?}, expected {expected:?}; output:\n{log}",
                self.role
            );
        }
    }

    /// Every report this child owes has been read; its exit is expected.
    fn reports_complete(&self) {
        self.reports_done.store(true, Ordering::SeqCst);
    }

    /// SIGKILL by pid and wait for the exit.
    async fn kill(self) {
        self.reports_complete();
        let status = std::process::Command::new("kill")
            .args(["-9", &self.pid.to_string()])
            .status()
            .expect("kill child");
        assert!(status.success(), "kill -9 {}", self.pid);
        let exited = self.exited;
        tokio::time::timeout(GUARD, tokio::task::spawn_blocking(move || exited.recv()))
            .await
            .expect("child exit within the hang guard")
            .unwrap()
            .expect("child exit observed");
    }

    /// Wait for a child that exits on its own.
    async fn exit(self) {
        self.reports_complete();
        let exited = self.exited;
        tokio::time::timeout(GUARD, tokio::task::spawn_blocking(move || exited.recv()))
            .await
            .expect("child exit within the hang guard")
            .unwrap()
            .expect("child exit observed");
    }
}

/// What one owner child observed in its first delivery pass.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct OwnerReport {
    projected: usize,
    applied: usize,
    failures: Vec<String>,
    model_requests: usize,
}

fn read_owner_report(root: &Path, role: &str) -> OwnerReport {
    let text = std::fs::read_to_string(root.join(format!("{role}.json"))).expect("owner report");
    serde_json::from_str(&text).expect("owner report json")
}

/// Child roles, selected by environment:
/// - `setup`: create a session with one turn, record its id, exit;
/// - `owner-*`: build a runtime, report `ready`, wait for `go`, arm the
///   delivery owner, record the first pass, report `done`, wait to be killed;
/// - `host`: serve a runtime (owner armed with an empty inbox), create a
///   session that stays live here, record its id, report `ready`, wait to be
///   killed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multi_process_delivery_child() {
    let Some(root) = std::env::var_os(ROOT_ENV).map(PathBuf::from) else {
        return;
    };
    let role = std::env::var(ROLE_ENV).expect("child role");
    let client = new_client();
    let runtime = build_runtime(&root, Arc::clone(&client)).await;
    match role.as_str() {
        "setup" | "host" => {
            let mut rpc = RpcClient::serve(Arc::clone(&runtime));
            let created = rpc
                .call("session/create", json!({ "prompt": SEED_PROMPT }))
                .await;
            let session_id = created["session_id"].as_str().expect("session id");
            std::fs::write(root.join("session-id"), format!("{session_id}\n")).expect("session id");
            write_fifo(report_fifo(&root, &role), "ready").await;
            if role == "host" {
                // The host's own delivery owner applies the recipient of the
                // session it hosts, woken by the cross-process commit watch
                // once another process projects the row.
                let mut passes = runtime
                    .subscribe_job_delivery_passes()
                    .expect("a runtime with a realm arms its delivery owner");
                let applied =
                    tokio::time::timeout(GUARD, passes.wait_for(|pass| pass.applied >= 1))
                        .await
                        .map(|wait| wait.map(|_| ()));
                let last_pass = passes.borrow().clone();
                assert!(
                    matches!(applied, Ok(Ok(()))),
                    "the host applies its recipient within the hang guard: {applied:?}; last \
                     pass: {last_pass:?}"
                );
                write_fifo(report_fifo(&root, &role), "applied").await;
                std::future::pending::<()>().await;
            }
        }
        owner => {
            write_fifo(report_fifo(&root, owner), "ready").await;
            let go = read_fifo(go_fifo(&root, owner)).await;
            assert_eq!(go, "go");
            runtime.arm_runtime_delivery_owner();
            let mut passes = runtime
                .subscribe_job_delivery_passes()
                .expect("a runtime with a realm arms its delivery owner");
            let pass = tokio::time::timeout(GUARD, passes.wait_for(|pass| pass.generation >= 1))
                .await
                .expect("the reconcile pass completes")
                .expect("owner pass channel open")
                .clone();
            // A turn this process admitted runs here; let it finish so the
            // durable transcript is complete before the parent reads it.
            if pass.applied > 0 {
                let session_id = meerkat_core::SessionId::parse(
                    std::fs::read_to_string(root.join("session-id"))
                        .expect("session id")
                        .trim(),
                )
                .expect("session id");
                if let Ok(key) = std::fs::read_to_string(root.join("event-key"))
                    && let Some(admitted) = runtime
                        .runtime_adapter()
                        .input_state_by_idempotency_key(&session_id, key.trim())
                        .await
                        .expect("idempotency lookup")
                {
                    let _ = tokio::time::timeout(
                        GUARD,
                        runtime
                            .runtime_adapter()
                            .wait_input_terminal_receipt(&session_id, &admitted.state.input_id),
                    )
                    .await;
                }
            }
            let report = OwnerReport {
                projected: pass.projected,
                applied: pass.applied,
                failures: pass.failures.clone(),
                model_requests: client.requests.load(Ordering::SeqCst),
            };
            std::fs::write(
                root.join(format!("{owner}.json")),
                serde_json::to_string(&report).expect("report json"),
            )
            .expect("owner report");
            write_fifo(report_fifo(&root, owner), "done").await;
            std::future::pending::<()>().await;
        }
    }
}

fn session_id_of(root: &Path) -> meerkat_core::SessionId {
    meerkat_core::SessionId::parse(
        std::fs::read_to_string(root.join("session-id"))
            .expect("session id")
            .trim(),
    )
    .expect("session id")
}

/// Seed: a session with one completed turn, hosted by nothing afterwards.
async fn seed_unhosted_session(root: &Path) -> meerkat_core::SessionId {
    let setup = spawn_child(root, "setup");
    setup.expect_report(root, "ready").await;
    setup.exit().await;
    session_id_of(root)
}

/// Commit a completed job straight into the realm's job store, subscribed
/// to `session_id` with `delivery`. No delivery owner runs in this process.
async fn commit_completed_job(
    root: &Path,
    session_id: &meerkat_core::SessionId,
    key: &str,
    delivery: meerkat::JobDeliveryKind,
) -> meerkat::JobId {
    let persistence = open_persistence(root).await;
    let jobs = meerkat::DetachedJobService::new(persistence.job_store());
    let spec = meerkat::JobSpec::new(
        REALM,
        session_id.clone(),
        meerkat::ExecutionIntentId::new(),
        meerkat::InteractionLineageId::new(),
        meerkat::ToolIdentity::new("scan", "v1").expect("tool"),
        meerkat::RunnerIdentity::new("runner.scan", "v1").expect("runner"),
        meerkat::RestartClass::Adoptable,
        meerkat::CanonicalArgumentsHash::new(format!("sha256:{key}")).expect("hash"),
        meerkat::JobSubmissionKey::new(key).expect("submission key"),
    );
    let receipt = jobs.submit(spec).await.expect("submit");
    jobs.subscribe(
        &receipt.job_id,
        meerkat::JobSubscription::new(
            meerkat::JobSubscriptionId::new("watcher").expect("subscription id"),
            session_id.clone(),
            delivery,
        ),
    )
    .await
    .expect("subscribe");
    let claim = jobs
        .claim_attempt(
            &receipt.job_id,
            meerkat::AttemptClaim::new(
                meerkat::WorkerId::new(format!("worker-{key}")).expect("worker"),
                1,
                1_000,
                meerkat::RunnerHandleRef::new(format!("runner:{key}")).expect("handle"),
            ),
        )
        .await
        .expect("claim");
    jobs.complete_attempt(
        &receipt.job_id,
        meerkat::AttemptWriteAuthority::from(&claim),
        2,
        None,
    )
    .await
    .expect("complete");
    receipt.job_id
}

/// Release both owner children together and collect their first passes.
async fn race_owners(root: &Path) -> (OwnerReport, OwnerReport) {
    let a = spawn_child(root, "owner-a");
    let b = spawn_child(root, "owner-b");
    a.expect_report(root, "ready").await;
    b.expect_report(root, "ready").await;
    tokio::join!(
        write_fifo(go_fifo(root, "owner-a"), "go"),
        write_fifo(go_fifo(root, "owner-b"), "go"),
    );
    a.expect_report(root, "done").await;
    b.expect_report(root, "done").await;
    a.kill().await;
    b.kill().await;
    (
        read_owner_report(root, "owner-a"),
        read_owner_report(root, "owner-b"),
    )
}

/// The durable transcript, read through a fresh runtime in this process once
/// every child is gone.
async fn durable_history(root: &Path, session_id: &meerkat_core::SessionId) -> Vec<Value> {
    let runtime = build_runtime(root, new_client()).await;
    let mut rpc = RpcClient::serve(runtime);
    let history = rpc
        .call(
            "session/history",
            json!({ "session_id": session_id.to_string() }),
        )
        .await;
    history["messages"].as_array().expect("history").clone()
}

fn count_role(messages: &[Value], role: &str) -> usize {
    messages
        .iter()
        .filter(|message| message["role"] == role)
        .count()
}

fn count_mentions(messages: &[Value], role: &str, needle: &str) -> usize {
    messages
        .iter()
        .filter(|message| message["role"] == role && message.to_string().contains(needle))
        .count()
}

/// Case 1: an Event row for an idle session no process hosts.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_event_for_an_unhosted_session_takes_effect_once_across_two_processes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    let session_id = seed_unhosted_session(root).await;
    let job_id = commit_completed_job(
        root,
        &session_id,
        "event-unhosted",
        meerkat::JobDeliveryKind::Event {
            handling_mode: meerkat_core::types::HandlingMode::Queue,
        },
    )
    .await;
    let event_key = format!("job:{job_id}:1:watcher");
    std::fs::write(root.join("event-key"), &event_key).expect("event key");

    let (a, b) = race_owners(root).await;
    let messages = durable_history(root, &session_id).await;
    // Durable history projects assistant output as `block_assistant`.
    let assistant_turns = count_role(&messages, "block_assistant");
    eprintln!(
        "case 1 evidence: owner-a={a:?} owner-b={b:?} assistant_messages={assistant_turns} \
         model_requests={}",
        a.model_requests + b.model_requests
    );

    // Single effect (a failure here is a release-blocking double effect).
    assert_eq!(
        a.model_requests + b.model_requests,
        1,
        "the event ran exactly one turn across both processes: a={a:?} b={b:?}"
    );
    assert_eq!(
        assistant_turns, 2,
        "the transcript holds the seed turn and exactly one event turn: {messages:?}"
    );
    assert_eq!(
        count_role(&messages, "system_notice"),
        1,
        "exactly one event notice in the transcript: {messages:?}"
    );
    // Single attempt (#1813): only one process applies a cold delivery.
    assert_eq!(
        a.applied + a.failures.len() + b.applied + b.failures.len(),
        1,
        "exactly one process attempted the cold delivery: a={a:?} b={b:?}"
    );
}

/// Case 2: a Notification row for an idle persisted session.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_notification_for_an_unhosted_session_appends_once_across_two_processes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    let session_id = seed_unhosted_session(root).await;
    let job_id = commit_completed_job(
        root,
        &session_id,
        "notification-unhosted",
        meerkat::JobDeliveryKind::Notification,
    )
    .await;

    let (a, b) = race_owners(root).await;
    let messages = durable_history(root, &session_id).await;
    let appended = count_mentions(&messages, "system", job_id.as_str());
    eprintln!("case 2 evidence: owner-a={a:?} owner-b={b:?} system_appends={appended}");

    // Single effect (a failure here is a release-blocking double effect).
    assert_eq!(
        appended, 1,
        "exactly one System-context message for the job: {messages:?}"
    );
    assert_eq!(
        a.model_requests + b.model_requests,
        0,
        "a notification runs no turn: a={a:?} b={b:?}"
    );
    // Single attempt (#1813): only one process applies a cold delivery.
    assert_eq!(
        a.applied + a.failures.len() + b.applied + b.failures.len(),
        1,
        "exactly one process attempted the cold delivery: a={a:?} b={b:?}"
    );
}

/// Case 3: a Notification row for a session LIVE in process B (the host).
/// Process A arms after the commit; it must not write under B's session.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_notification_for_a_session_live_elsewhere_is_not_applied_by_another_process() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    let host = spawn_child(root, "host");
    host.expect_report(root, "ready").await;
    let session_id = session_id_of(root);
    let job_id = commit_completed_job(
        root,
        &session_id,
        "notification-live-elsewhere",
        meerkat::JobDeliveryKind::Notification,
    )
    .await;

    let a = spawn_child(root, "owner-a");
    a.expect_report(root, "ready").await;
    write_fifo(go_fifo(root, "owner-a"), "go").await;
    a.expect_report(root, "done").await;
    a.kill().await;
    let report = read_owner_report(root, "owner-a");
    eprintln!("case 3 evidence: owner-a={report:?}");
    assert_eq!(
        report.applied + report.failures.len(),
        0,
        "a process that does not host the session neither applies nor attempts its \
         delivery: {report:?}"
    );
    host.expect_report(root, "applied").await;
    host.kill().await;
    let messages = durable_history(root, &session_id).await;
    let appended = count_mentions(&messages, "system", job_id.as_str());
    assert_eq!(
        appended, 1,
        "the hosting process applied the recipient exactly once: {messages:?}"
    );
}

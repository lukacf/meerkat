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
//!    process A: A writes nothing under B's live session;
//! 4. the process hosting a session is killed while it applies a row to that
//!    session (projected by another owner process, as in case 3), after the
//!    row's effect and before its recipient is settled. A fresh owner process
//!    then applies the row exactly once: a Notification row, an Event row
//!    whose turn committed before the kill, and an Event row whose turn was
//!    still in flight. A test-support hold before the settlement pins the
//!    kill. Continuation rows have the same window (between their admission
//!    and `mark_applied`) and are not covered here.
//!
//! The parent seeds the session (in a setup child that exits, so nothing
//! hosts it), commits the completed job straight into the realm's job store,
//! then releases both owner children at once. Each child reports its first
//! delivery pass. Synchronization is by fifos and process exit; timeouts are
//! hang guards only. The parent reads durable state without arming a
//! delivery owner, which could itself apply a pending row.

#![cfg(any(target_os = "linux", target_os = "macos"))]
#![allow(
    clippy::expect_used,
    clippy::large_futures,
    clippy::panic,
    clippy::unwrap_used,
    clippy::disallowed_methods
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
/// The one subscription every committed job delivers to.
const SUBSCRIPTION: &str = "watcher";
/// Hang guard for every awaited cross-process signal; never pacing.
const GUARD: Duration = Duration::from_mins(2);

/// Scripted model: answers every request with a fixed text and counts calls.
/// A gated client leaves its `gate`-th request unanswered after announcing
/// it on `gated_request_started`, so that turn is in flight until its
/// process dies.
struct TextClient {
    requests: AtomicUsize,
    gate: Option<usize>,
    gated_request_started: tokio::sync::Notify,
}

#[async_trait::async_trait]
impl LlmClient for TextClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> meerkat_llm_core::LlmStream<'a> {
        let request_number = self.requests.fetch_add(1, Ordering::SeqCst) + 1;
        if self.gate == Some(request_number) {
            self.gated_request_started.notify_one();
            return Box::pin(futures::stream::pending());
        }
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
                outcome: LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
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
        gate: None,
        gated_request_started: tokio::sync::Notify::new(),
    })
}

/// A client whose `gate`-th request never answers.
fn gated_client(gate: usize) -> Arc<TextClient> {
    Arc::new(TextClient {
        requests: AtomicUsize::new(0),
        gate: Some(gate),
        gated_request_started: tokio::sync::Notify::new(),
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
    )
    .expect("compose the RPC session runtime");
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

/// Block (no polling) until a line arrives in `fifo`; `None` once the hang
/// guard elapses. The guard then releases the blocked read, so its thread
/// cannot keep the runtime from shutting down.
async fn read_fifo_within_guard(fifo: PathBuf) -> Option<String> {
    let mut read = tokio::task::spawn_blocking({
        let fifo = fifo.clone();
        move || std::fs::read_to_string(fifo)
    });
    if let Ok(line) = tokio::time::timeout(GUARD, &mut read).await {
        return Some(line.unwrap().unwrap().trim().to_string());
    }
    release_fifo(&fifo);
    let _ = read.await;
    None
}

/// Open `fifo` for reading and writing and close it again. This never
/// blocks, and it releases a peer blocked opening either end.
fn release_fifo(fifo: &Path) {
    let _ = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(fifo);
}

/// Block (no polling) until a line arrives in `fifo`.
async fn read_fifo(fifo: PathBuf) -> String {
    read_fifo_within_guard(fifo)
        .await
        .expect("cross-process signal within the hang guard")
}

async fn write_fifo(fifo: PathBuf, line: &'static str) {
    let mut write = tokio::task::spawn_blocking({
        let fifo = fifo.clone();
        move || std::fs::write(fifo, format!("{line}\n"))
    });
    if let Ok(written) = tokio::time::timeout(GUARD, &mut write).await {
        written.unwrap().unwrap();
        return;
    }
    release_fifo(&fifo);
    let _ = write.await;
    panic!(
        "cross-process signal within the hang guard: nobody read {}",
        fifo.display()
    );
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
/// log instead of hanging. A child still running when its handle drops (a
/// failed parent) is killed.
struct Child {
    role: String,
    pid: u32,
    exited: Option<std::sync::mpsc::Receiver<()>>,
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
        exited: Some(exited),
        reports_done,
    }
}

impl Child {
    async fn expect_report(&self, root: &Path, expected: &str) {
        let line = read_fifo_within_guard(report_fifo(root, &self.role)).await;
        if line.as_deref() != Some(expected) {
            let log = std::fs::read_to_string(root.join(format!("{}.log", self.role)))
                .unwrap_or_default();
            panic!(
                "child {} reported {line:?} (None: nothing within the hang guard), expected \
                 {expected:?}; output:\n{log}",
                self.role
            );
        }
    }

    /// Every report this child owes has been read; its exit is expected.
    fn reports_complete(&self) {
        self.reports_done.store(true, Ordering::SeqCst);
    }

    /// SIGKILL by pid and wait for the exit.
    async fn kill(mut self) {
        self.reports_complete();
        let status = std::process::Command::new("kill")
            .args(["-9", &self.pid.to_string()])
            .status()
            .expect("kill child");
        assert!(status.success(), "kill -9 {}", self.pid);
        let exited = self.exited.take().expect("child not yet reaped");
        tokio::time::timeout(GUARD, tokio::task::spawn_blocking(move || exited.recv()))
            .await
            .expect("child exit within the hang guard")
            .unwrap()
            .expect("child exit observed");
    }

    /// Wait for a child that exits on its own.
    async fn exit(mut self) {
        self.reports_complete();
        let exited = self.exited.take().expect("child not yet reaped");
        tokio::time::timeout(GUARD, tokio::task::spawn_blocking(move || exited.recv()))
            .await
            .expect("child exit within the hang guard")
            .unwrap()
            .expect("child exit observed");
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        if let Some(exited) = &self.exited
            && exited.try_recv().is_err()
        {
            self.reports_complete();
            let _ = std::process::Command::new("kill")
                .args(["-9", &self.pid.to_string()])
                .status();
        }
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
///   killed;
/// - `crash-host-*`: as `host`, then hold the session's recipient after its
///   effect and before its settlement (see [`crash_host`]).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multi_process_delivery_child() {
    let Some(root) = std::env::var_os(ROOT_ENV).map(PathBuf::from) else {
        return;
    };
    let role = std::env::var(ROLE_ENV).expect("child role");
    let mid_apply = MidApply::from_role(&role);
    // The in-flight host never answers its second model request: the
    // event's turn, after the seed turn.
    let client = match mid_apply {
        Some(MidApply::EventInFlight) => gated_client(2),
        _ => new_client(),
    };
    let runtime = build_runtime(&root, Arc::clone(&client)).await;
    if let Some(case) = mid_apply {
        crash_host(&root, &role, case, &runtime, &client).await;
        return;
    }
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
            // A turn this process admitted, or recovered from a dead host,
            // runs here; let every active input of the session finish so the
            // durable transcript is complete before the parent reads it.
            if pass.applied > 0 {
                let session_id = session_id_of(&root);
                wait_for_active_inputs(&runtime, &session_id).await;
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

/// Which effect case 4 kills the hosting process in the middle of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MidApply {
    /// A Notification row: its System message is appended.
    Notification,
    /// An Event row whose turn committed before the kill.
    EventCommitted,
    /// An Event row whose turn is still in flight at the kill.
    EventInFlight,
}

impl MidApply {
    const ALL: [Self; 3] = [
        Self::Notification,
        Self::EventCommitted,
        Self::EventInFlight,
    ];

    fn role(self) -> &'static str {
        match self {
            Self::Notification => "crash-host-notification",
            Self::EventCommitted => "crash-host-event-committed",
            Self::EventInFlight => "crash-host-event-in-flight",
        }
    }

    fn from_role(role: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|case| case.role() == role)
    }

    fn delivery(self) -> meerkat::JobDeliveryKind {
        match self {
            Self::Notification => meerkat::JobDeliveryKind::Notification,
            Self::EventCommitted | Self::EventInFlight => meerkat::JobDeliveryKind::Event {
                handling_mode: meerkat_core::types::HandlingMode::Queue,
            },
        }
    }
}

/// The `crash-host-*` roles: serve a runtime (owner armed), create a
/// session that stays live here, record its id, hold its recipient's next
/// `Applied` settlement, and report `ready`. After `go` (the parent has
/// committed the job), wait for the host's own owner to reach the hold with
/// the case's effect in place, report `mid-apply`, and wait to be killed.
async fn crash_host(
    root: &Path,
    role: &str,
    case: MidApply,
    runtime: &Arc<SessionRuntime>,
    client: &TextClient,
) {
    let mut rpc = RpcClient::serve(Arc::clone(runtime));
    let created = rpc
        .call("session/create", json!({ "prompt": SEED_PROMPT }))
        .await;
    let session_id =
        meerkat_core::SessionId::parse(created["session_id"].as_str().expect("session id"))
            .expect("session id");
    std::fs::write(root.join("session-id"), format!("{session_id}\n")).expect("session id");
    // Kept until this process is killed, so the hold is never released.
    let (held, _release) = meerkat_runtime::delivery_inbox::test_hold_next_applied_settlement(
        meerkat_runtime::LogicalRuntimeId::for_session(&session_id),
        SUBSCRIPTION,
    );
    write_fifo(report_fifo(root, role), "ready").await;
    let go = read_fifo(go_fifo(root, role)).await;
    assert_eq!(go, "go");
    tokio::time::timeout(GUARD, held)
        .await
        .expect("the host's owner reached the settlement hold")
        .expect("hold signal");
    match case {
        MidApply::Notification => {}
        // The event was admitted before the hold; its turn runs to its end.
        MidApply::EventCommitted => wait_for_active_inputs(runtime, &session_id).await,
        MidApply::EventInFlight => {
            tokio::time::timeout(GUARD, client.gated_request_started.notified())
                .await
                .expect("the event's turn reached its model request");
        }
    }
    write_fifo(report_fifo(root, role), "mid-apply").await;
    std::future::pending::<()>().await;
}

/// Wait for every active input of `session_id` in this process to reach a
/// terminal state. A session this process never registered has none.
async fn wait_for_active_inputs(runtime: &SessionRuntime, session_id: &meerkat_core::SessionId) {
    let adapter = runtime.runtime_adapter();
    let active = match adapter.list_active_inputs(session_id).await {
        Ok(active) => active,
        Err(error) => {
            eprintln!("no active inputs to wait for: {error}");
            Vec::new()
        }
    };
    for input_id in active {
        let finished = tokio::time::timeout(
            GUARD,
            adapter.wait_input_terminal_receipt(session_id, &input_id),
        )
        .await;
        eprintln!("active input {input_id} wait: {finished:?}");
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
            meerkat::JobSubscriptionId::new(SUBSCRIPTION).expect("subscription id"),
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
/// every child is gone. No delivery owner is armed here: one could apply a
/// pending row itself and hide a lost apply.
async fn durable_history(root: &Path, session_id: &meerkat_core::SessionId) -> Vec<Value> {
    let runtime = build_runtime(root, new_client()).await;
    let history = runtime
        .read_session_history_rich(
            session_id,
            meerkat_core::service::SessionHistoryQuery::default(),
        )
        .await
        .expect("read the durable history")
        .expect("the session is persisted");
    match serde_json::to_value(history.messages).expect("history json") {
        Value::Array(messages) => messages,
        other => panic!("history messages are not an array: {other}"),
    }
}

/// The durable input admitted under `key`, if any: its id and its terminal
/// outcome. Read from the realm's runtime store only, without arming an
/// owner.
async fn durable_input(
    root: &Path,
    session_id: &meerkat_core::SessionId,
    key: &str,
) -> Option<(
    meerkat_core::lifecycle::InputId,
    Option<meerkat_runtime::InputTerminalOutcome>,
)> {
    let runtime = build_runtime(root, new_client()).await;
    runtime
        .runtime_adapter()
        .durable_input_state_by_idempotency_key(session_id, key)
        .await
        .expect("durable idempotency lookup")
        .map(|stored| (stored.state.input_id, stored.seed.terminal_outcome))
}

/// The session's runtime delivery inbox in the realm's store.
async fn delivery_inbox(root: &Path) -> meerkat_runtime::RuntimeDeliveryInbox {
    meerkat_runtime::RuntimeDeliveryInbox::new(open_persistence(root).await.runtime_store())
}

/// The rows still pending after the session inbox's applied cursor, and the
/// cursor.
async fn delivery_rows(
    root: &Path,
    session_id: &meerkat_core::SessionId,
) -> (Vec<meerkat_runtime::RuntimeDeliveryRecord>, u64) {
    let inbox = delivery_inbox(root).await;
    let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(session_id);
    let pending = inbox
        .list_pending(&runtime_id, 16)
        .await
        .expect("pending delivery rows");
    let cursor = inbox
        .applied_cursor(&runtime_id)
        .await
        .expect("applied cursor");
    (pending, cursor)
}

/// The generated delivery authority's verdict for one row.
async fn delivery_status(
    root: &Path,
    session_id: &meerkat_core::SessionId,
    delivery_id: &meerkat_runtime::RuntimeDeliveryId,
) -> meerkat_runtime::RuntimeDeliveryStatus {
    delivery_inbox(root)
        .await
        .delivery_status(
            &meerkat_runtime::LogicalRuntimeId::for_session(session_id),
            delivery_id,
        )
        .await
        .expect("delivery status")
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
    commit_completed_job(
        root,
        &session_id,
        "event-unhosted",
        meerkat::JobDeliveryKind::Event {
            handling_mode: meerkat_core::types::HandlingMode::Queue,
        },
    )
    .await;

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

/// Case 4: the process hosting the session is killed while it applies the
/// row, after the effect and before the settlement; a fresh owner process
/// then recovers the row. The transcript and the delivery authority are
/// asserted before the input ledger, so a doubled effect is reported as one.
async fn kill_the_host_mid_apply(case: MidApply) {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    let role = case.role();
    let host = spawn_child(root, role);
    host.expect_report(root, "ready").await;
    let session_id = session_id_of(root);
    let job_id = commit_completed_job(root, &session_id, role, case.delivery()).await;
    let event_key = format!("job:{job_id}:1:{SUBSCRIPTION}");
    write_fifo(go_fifo(root, role), "go").await;
    // As in case 3, another owner process projects the committed job into
    // the realm's inbox; the host's own owner, woken by that commit, applies
    // the row up to its settlement hold.
    let projector = spawn_child(root, "owner-a");
    projector.expect_report(root, "ready").await;
    write_fifo(go_fifo(root, "owner-a"), "go").await;
    projector.expect_report(root, "done").await;
    projector.kill().await;
    let projected = read_owner_report(root, "owner-a");
    assert_eq!(
        projected.applied + projected.failures.len(),
        0,
        "the projector neither applies nor attempts the live host's row: {projected:?}"
    );
    host.expect_report(root, "mid-apply").await;
    host.kill().await;

    // Where the kill landed: the effect is durable, the recipient unsettled.
    let killed = durable_history(root, &session_id).await;
    let killed_input = durable_input(root, &session_id, &event_key).await;
    let (pending, cursor) = delivery_rows(root, &session_id).await;
    eprintln!(
        "case 4 {role} after the kill: system={} notices={} assistant={} input={killed_input:?} \
         pending={} cursor={cursor}",
        count_mentions(&killed, "system", job_id.as_str()),
        count_role(&killed, "system_notice"),
        count_role(&killed, "block_assistant"),
        pending.len(),
    );
    let [row] = pending.as_slice() else {
        panic!("the row is pending after the kill: {pending:?}");
    };
    let delivery_id = row.submission.delivery_id().clone();
    let sequence = row.sequence;
    assert!(
        cursor < sequence,
        "the cursor stopped before the row: {cursor}"
    );
    assert_eq!(
        delivery_status(root, &session_id, &delivery_id).await,
        meerkat_runtime::RuntimeDeliveryStatus::Pending {
            delivery_sequence: sequence
        },
        "the killed host never settled its recipient"
    );
    match case {
        MidApply::Notification => assert_eq!(
            count_mentions(&killed, "system", job_id.as_str()),
            1,
            "the host's System message was durable before its settlement: {killed:?}"
        ),
        MidApply::EventCommitted => {
            assert_eq!(
                count_role(&killed, "system_notice"),
                1,
                "the event's notice committed before the kill: {killed:?}"
            );
            assert_eq!(
                count_role(&killed, "block_assistant"),
                2,
                "the seed turn and the event's turn committed before the kill: {killed:?}"
            );
        }
        MidApply::EventInFlight => assert_eq!(
            count_role(&killed, "block_assistant"),
            1,
            "only the seed turn committed; the event's turn was in flight: {killed:?}"
        ),
    }

    // Recovery: a fresh owner process applies what the dead host left.
    let owner = spawn_child(root, "owner-b");
    owner.expect_report(root, "ready").await;
    write_fifo(go_fifo(root, "owner-b"), "go").await;
    owner.expect_report(root, "done").await;
    owner.kill().await;
    let report = read_owner_report(root, "owner-b");

    let recovered = durable_history(root, &session_id).await;
    let recovered_input = durable_input(root, &session_id, &event_key).await;
    let (pending, cursor) = delivery_rows(root, &session_id).await;
    eprintln!(
        "case 4 {role} after recovery: owner={report:?} system={} notices={} assistant={} \
         input={recovered_input:?} pending={} cursor={cursor}",
        count_mentions(&recovered, "system", job_id.as_str()),
        count_role(&recovered, "system_notice"),
        count_role(&recovered, "block_assistant"),
        pending.len(),
    );

    // Exactly one effect (a failure here is a double or a lost apply).
    match case {
        MidApply::Notification => assert_eq!(
            count_mentions(&recovered, "system", job_id.as_str()),
            1,
            "exactly one System message for the job: {recovered:?}"
        ),
        MidApply::EventCommitted | MidApply::EventInFlight => {
            assert_eq!(
                count_role(&recovered, "system_notice"),
                1,
                "exactly one event notice in the transcript: {recovered:?}"
            );
            assert_eq!(
                count_role(&recovered, "block_assistant"),
                2,
                "the seed turn and exactly one event turn: {recovered:?}"
            );
        }
    }
    let expected_model_requests = usize::from(case == MidApply::EventInFlight);
    assert_eq!(
        report.model_requests, expected_model_requests,
        "only an event turn left in flight runs again: {report:?}"
    );

    // The delivery authority settled the row once, as applied.
    assert_eq!(
        (report.applied, report.failures.len()),
        (1, 0),
        "the recovering owner applied the row: {report:?}"
    );
    assert!(
        pending.is_empty(),
        "nothing is pending after recovery: {pending:?}"
    );
    assert!(cursor >= sequence, "the cursor passed the row: {cursor}");
    assert_eq!(
        delivery_status(root, &session_id, &delivery_id).await,
        meerkat_runtime::RuntimeDeliveryStatus::Applied {
            delivery_sequence: sequence
        }
    );

    // The input ledger: one admission, which took effect.
    match case {
        MidApply::Notification => {
            assert_eq!(recovered_input, None, "a notification admits no input");
        }
        MidApply::EventCommitted | MidApply::EventInFlight => {
            let (killed_id, killed_outcome) =
                killed_input.expect("the event's input was durable before the kill");
            let expected_killed_outcome = (case == MidApply::EventCommitted)
                .then_some(meerkat_runtime::InputTerminalOutcome::Consumed);
            assert_eq!(
                killed_outcome, expected_killed_outcome,
                "the event's input at the kill"
            );
            assert_eq!(
                recovered_input,
                Some((
                    killed_id,
                    Some(meerkat_runtime::InputTerminalOutcome::Consumed)
                )),
                "the same input, admitted once, took effect"
            );
        }
    }
}

/// Case 4, Notification: killed after the System message is appended.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_notification_is_applied_once_after_its_host_dies_mid_apply() {
    kill_the_host_mid_apply(MidApply::Notification).await;
}

/// Case 4, Event: killed after the event's turn committed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_event_with_a_committed_turn_takes_effect_once_after_its_host_dies_mid_apply() {
    kill_the_host_mid_apply(MidApply::EventCommitted).await;
}

/// Case 4, Event: killed while the event's turn is in flight.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_event_with_a_turn_in_flight_takes_effect_once_after_its_host_dies_mid_apply() {
    kill_the_host_mid_apply(MidApply::EventInFlight).await;
}

//! Abrupt gateway death mid-tool on an RPC session, recovered through the
//! RPC on-demand attach path.
//!
//! A child RPC "gateway" process creates a deferred session and starts a turn
//! whose scripted model calls the shell tool with a delayed file effect, and
//! is SIGKILLed while the tool runs. The run's round commit precedes the
//! tool, so the run is open at a committed round. The next RPC incarnation
//! (this test process) serves `turn/start` for the session, which attaches
//! the runtime executor on demand before any agent is built. The runtime
//! must settle the session's process custody at that attach (kill the
//! orphaned tool, so its delayed effect never happens), and the open run
//! resumes: its round answers the interrupted call with an outcome-uncertain
//! result instead of running it again, and the run ends once, without an
//! interrupted-run notice (the call's own result tells the model). The new
//! turn then runs after it.
//!
//! The scripted tool appends a line to `runs` on every execution and signals
//! `started.fifo` on its first execution only, so a call that runs again
//! shows as a second line (and as its effect) instead of blocking on the
//! FIFO.
//!
//! The on-demand attach row runs on a HeadCanonical (SQLite) realm and on a
//! WholeBlob (JSONL) realm.

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
use meerkat_core::types::SystemNoticeKind;
use meerkat_core::{Config, ConfigRuntime, MemoryConfigStore, Message, SessionHistoryQuery};
use meerkat_rpc::server::RpcServer;
use meerkat_rpc::session_runtime::SessionRuntime;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

const PHASE_ENV: &str = "MEERKAT_CUSTODY_RPC_PHASE";
const ROOT_ENV: &str = "MEERKAT_CUSTODY_RPC_ROOT";
const CHILD_TEST: &str = "rpc_custody_gateway_child";
const TOOL_CALL_ID: &str = "call-rpc-crash";
const INTERRUPTED_PROMPT: &str = "create the effect file";
const NEXT_PROMPT: &str = "what happened?";
const ANSWER: &str = "noted";
/// Bound on the gateway reaching its tool; a gateway that exits first fails
/// the row at once.
const CHILD_BOUND: Duration = Duration::from_secs(120);
/// Present in a row's root when every incarnation opens a WholeBlob (JSONL)
/// realm instead of a HeadCanonical (SQLite) one.
const WHOLE_BLOB_MARKER: &str = "whole-blob";
/// Bound on turn/start answering after the restart.
const TURN_BOUND: Duration = Duration::from_secs(90);
const PHASE_GATEWAY: &str = "gateway";
/// Run a quick tool, then pause the run's terminal commit (the turn is
/// terminal in its agent, its provisional tail written, its boundary not
/// committed) and wait to be killed.
const PHASE_BEFORE_COMMIT: &str = "before-commit";

/// Scripted model: on a fresh request (no tool results, no interrupted-run
/// notice) it calls the shell tool; otherwise it answers.
struct ScriptedShellClient {
    command: String,
    tool_calls: AtomicUsize,
    requests: AtomicUsize,
    requests_seeing_notice: AtomicUsize,
}

impl ScriptedShellClient {
    fn new(command: String) -> Self {
        Self {
            command,
            tool_calls: AtomicUsize::new(0),
            requests: AtomicUsize::new(0),
            requests_seeing_notice: AtomicUsize::new(0),
        }
    }
}

#[async_trait::async_trait]
impl LlmClient for ScriptedShellClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> meerkat_llm_core::LlmStream<'a> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        let sees_notice = request.messages.iter().any(|message| {
            matches!(
                message,
                Message::SystemNotice(notice) if notice.kind == SystemNoticeKind::ToolProcessRecovery
            )
        });
        if sees_notice {
            self.requests_seeing_notice.fetch_add(1, Ordering::SeqCst);
        }
        let answered = sees_notice
            || request
                .messages
                .iter()
                .any(|message| matches!(message, Message::ToolResults { .. }));
        let mut events = Vec::new();
        let stop_reason = if answered {
            events.push(LlmEvent::TextDelta {
                delta: ANSWER.to_string(),
                meta: None,
            });
            meerkat_core::StopReason::EndTurn
        } else {
            self.tool_calls.fetch_add(1, Ordering::SeqCst);
            events.push(LlmEvent::ToolCallComplete {
                id: TOOL_CALL_ID.to_string(),
                name: "shell".to_string(),
                args: serde_json::json!({ "command": self.command, "timeout_secs": 60 }),
                meta: None,
            });
            meerkat_core::StopReason::ToolUse
        };
        events.push(LlmEvent::UsageUpdate {
            usage: meerkat_core::TurnUsage::host_declared(
                meerkat_core::Provider::OpenAI,
                &request.model,
                meerkat_core::Usage::default(),
            ),
        });
        events.push(LlmEvent::Done {
            outcome: LlmDoneOutcome::success(stop_reason),
        });
        Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

/// The scripted tool: one `runs` line per execution, the FIFO signal on the
/// first execution only, then the delayed effect.
fn tool_command(root: &Path) -> String {
    format!(
        "echo $$ >> '{pids}'; echo ran >> '{runs}'; if [ ! -e '{once}' ]; then : > '{once}'; echo started > '{fifo}'; fi; sleep 2; echo effect >> '{effect}'",
        pids = root.join("pids").display(),
        runs = root.join("runs").display(),
        once = root.join("started-once").display(),
        fifo = root.join("started.fifo").display(),
        effect = root.join("effect").display()
    )
}

/// Bound on the scripted tool's shells exiting after recovery killed them.
const PROCESS_EXIT_BOUND: Duration = Duration::from_secs(10);

/// Whether a process with this pid exists (`kill -0`; a zombie counts until
/// it is reaped).
fn process_exists(pid: u32) -> bool {
    std::process::Command::new("sh")
        .args(["-c", &format!("kill -0 {pid}")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// The scripted tool's delayed effect can no longer happen: every shell the
/// tool started has exited (recovery killed the interrupted one, which
/// writes the effect only after its sleep). Waits a bounded time for exits
/// still being reaped; a shell still running fails the row.
async fn assert_tool_processes_gone(root: &Path) {
    let pids = std::fs::read_to_string(root.join("pids")).unwrap_or_default();
    assert!(
        !pids.trim().is_empty(),
        "the scripted tool recorded no process"
    );
    let started = std::time::Instant::now();
    for pid in pids.lines() {
        let pid: u32 = pid.trim().parse().expect("a recorded pid");
        while process_exists(pid) {
            assert!(
                started.elapsed() < PROCESS_EXIT_BOUND,
                "the interrupted tool's shell {pid} is still running"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

/// How many times the scripted tool has run.
fn tool_runs(root: &Path) -> usize {
    std::fs::read_to_string(root.join("runs"))
        .map(|runs| runs.lines().count())
        .unwrap_or(0)
}

/// A tool that finishes at once, appending one line per execution.
fn quick_tool_command(root: &Path) -> String {
    format!("echo ran >> '{}'", root.join("ran").display())
}

fn child_log_path(root: &Path, phase: &str) -> PathBuf {
    root.join(format!("child-{phase}.log"))
}

fn spawn_child(root: &Path, phase: &str) -> std::process::Child {
    let log = std::fs::File::create(child_log_path(root, phase)).expect("child log");
    std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", CHILD_TEST, "--nocapture", "--test-threads=1"])
        .env(PHASE_ENV, phase)
        .env(ROOT_ENV, root)
        .env("MEERKAT_DISABLE_GRAPH_DECODE_MEMO", "1")
        .stdout(log.try_clone().expect("child log"))
        .stderr(log)
        .spawn()
        .expect("spawn child")
}

/// Wait for the tool's line in `started.fifo`. A gateway that exits first,
/// or never starts the tool within the bound, fails the row at once with its
/// log.
async fn tool_started(root: &Path, gateway: &mut std::process::Child, phase: &str) -> String {
    let fifo = root.join("started.fifo");
    // A plain thread, not a runtime blocking task, so a read that never ends
    // cannot hold the test runtime's shutdown.
    let reader = std::thread::spawn({
        let fifo = fifo.clone();
        move || std::fs::read_to_string(fifo)
    });
    let started = std::time::Instant::now();
    loop {
        if reader.is_finished() {
            return reader
                .join()
                .expect("join the FIFO reader")
                .expect("read the FIFO");
        }
        let failure = match gateway.try_wait().expect("poll the gateway") {
            Some(status) => Some(format!("exited before its tool started ({status})")),
            None if started.elapsed() > CHILD_BOUND => Some("never started its tool".to_string()),
            None => None,
        };
        if let Some(failure) = failure {
            let _ = gateway.kill();
            let _ = gateway.wait();
            // End the blocked read: a writer that opens and closes the FIFO
            // gives it end of file. Detached, since its open blocks when the
            // reader has already finished.
            drop(std::thread::spawn(move || {
                std::fs::OpenOptions::new().write(true).open(fifo)
            }));
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while !reader.is_finished() && std::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            if reader.is_finished() {
                let line = reader
                    .join()
                    .expect("join the FIFO reader")
                    .unwrap_or_default();
                if !line.is_empty() {
                    return line;
                }
            }
            panic!(
                "the gateway {failure}: {}",
                std::fs::read_to_string(child_log_path(root, phase)).unwrap_or_default()
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Block (no polling) until a child writes a line into `fifo`.
async fn read_fifo(fifo: PathBuf) -> Option<String> {
    tokio::time::timeout(
        Duration::from_secs(120),
        tokio::task::spawn_blocking(move || std::fs::read_to_string(fifo)),
    )
    .await
    .ok()
    .map(|joined| joined.unwrap().unwrap())
}

fn mkfifo(path: &Path) {
    let status = std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .expect("run mkfifo");
    assert!(status.success(), "mkfifo {}", path.display());
}

async fn build_runtime(root: &Path, client: Arc<ScriptedShellClient>) -> Arc<SessionRuntime> {
    let whole_blob = root.join(WHOLE_BLOB_MARKER).exists();
    let backend = if whole_blob {
        meerkat_store::RealmBackend::Jsonl
    } else {
        meerkat_store::RealmBackend::Sqlite
    };
    let (_manifest, persistence) = meerkat::open_realm_persistence_in(
        root,
        "custody-rpc-realm",
        Some(backend),
        Some(meerkat_store::RealmOrigin::Explicit),
    )
    .await
    .expect("open realm persistence");
    let expected = if whole_blob {
        meerkat_runtime::RuntimeSessionPersistenceProfile::WholeBlobV1
    } else {
        meerkat_runtime::RuntimeSessionPersistenceProfile::HeadCanonicalV1
    };
    assert_eq!(persistence.session_persistence_profile(), expected);
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("project dir");
    let factory = AgentFactory::new(root.join("sessions"))
        .runtime_root(root.join("realm"))
        .project_root(&project)
        .shell(true);
    let mut config = Config::default();
    config.shell.program = "sh".to_string();
    config.shell.security_mode = meerkat_core::types::SecurityMode::Unrestricted;
    let runtime = SessionRuntime::new(
        factory,
        config.clone(),
        10,
        persistence,
        meerkat_rpc::router::NotificationSink::noop(),
    )
    .expect("compose the RPC session runtime");
    runtime.set_default_llm_client(Some(client));
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
    fn serve(runtime: Arc<SessionRuntime>) -> Self {
        let config_store: Arc<dyn meerkat_core::ConfigStore> = Arc::new(MemoryConfigStore::new(
            Config::default(),
            meerkat_models::canonical(),
        ));
        let (server_reader, client_writer) = tokio::io::duplex(1 << 16);
        let (client_reader, server_writer) = tokio::io::duplex(1 << 16);
        tokio::spawn(async move {
            let mut server = RpcServer::new(
                BufReader::new(server_reader),
                server_writer,
                runtime,
                config_store,
            )
            .expect("construct runtime authority");
            let _ = server.run().await;
        });
        Self {
            writer: client_writer,
            reader: BufReader::new(client_reader),
            next_id: 1,
        }
    }

    async fn send(&mut self, method: &str, params: serde_json::Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let line = format!(
            "{}\n",
            serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
        );
        self.writer
            .write_all(line.as_bytes())
            .await
            .expect("write request");
        self.writer.flush().await.expect("flush request");
        id
    }

    async fn response(&mut self, id: u64) -> serde_json::Value {
        loop {
            let mut line = String::new();
            self.reader.read_line(&mut line).await.expect("read line");
            assert!(!line.is_empty(), "rpc server closed");
            let value: serde_json::Value = serde_json::from_str(&line).expect("parse json");
            if value.get("id").and_then(serde_json::Value::as_u64) == Some(id) {
                return value;
            }
        }
    }

    async fn call(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        let id = self.send(method, params).await;
        let response = self.response(id).await;
        assert!(response["error"].is_null(), "{method} failed: {response}");
        response["result"].clone()
    }
}

/// The RPC gateway role: create a deferred session, record its id, start a
/// turn that runs the delayed-effect shell tool, and wait to be killed.
#[tokio::test]
async fn rpc_custody_gateway_child() {
    let Some(root) = std::env::var_os(ROOT_ENV).map(PathBuf::from) else {
        return;
    };
    let Some(phase) = std::env::var(PHASE_ENV).ok() else {
        return;
    };
    let command = match phase.as_str() {
        PHASE_BEFORE_COMMIT => quick_tool_command(&root),
        _ => tool_command(&root),
    };
    let client = Arc::new(ScriptedShellClient::new(command));
    let runtime = build_runtime(&root, client).await;
    let mut rpc = RpcClient::serve(Arc::clone(&runtime));
    let created = rpc
        .call(
            "session/create",
            serde_json::json!({ "prompt": "", "initial_turn": "deferred" }),
        )
        .await;
    let session_id = created["session_id"].as_str().expect("session id");
    std::fs::write(root.join("session-id"), format!("{session_id}\n")).expect("session id");
    let paused = (phase == PHASE_BEFORE_COMMIT).then(|| {
        runtime
            .runtime_adapter()
            .arm_runtime_loop_before_terminal_commit_test_hook(
                meerkat_core::SessionId::parse(session_id).unwrap(),
            )
    });
    // The turn blocks in the tool (or at its terminal commit) until the
    // parent kills this process.
    rpc.send(
        "turn/start",
        serde_json::json!({ "session_id": session_id, "prompt": INTERRUPTED_PROMPT }),
    )
    .await;
    if let Some((entered, _release)) = paused {
        entered.await.expect("terminal commit reached");
        let fifo = root.join("phase.fifo");
        tokio::task::spawn_blocking(move || std::fs::write(fifo, "held\n"))
            .await
            .unwrap()
            .unwrap();
        std::future::pending::<()>().await;
    }
    std::future::pending::<()>().await;
}

async fn on_demand_attach_resumes_the_interrupted_run(whole_blob: bool) {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    if whole_blob {
        std::fs::write(root.join(WHOLE_BLOB_MARKER), "").expect("profile marker");
    }
    mkfifo(&root.join("started.fifo"));

    let mut gateway = spawn_child(root, PHASE_GATEWAY);
    // The tool itself reports it is running.
    let started = tool_started(root, &mut gateway, PHASE_GATEWAY).await;
    // Abrupt gateway death mid-tool.
    gateway.kill().unwrap();
    gateway.wait().unwrap();
    assert_eq!(started.trim(), "started");
    assert_eq!(tool_runs(root), 1, "the gateway ran the tool once");
    let session_id = std::fs::read_to_string(root.join("session-id"))
        .expect("session id")
        .trim()
        .to_string();

    // Next incarnation: `turn/start` attaches the runtime executor on demand
    // before any agent exists for the session. The open run resumes there,
    // and the new turn runs after it.
    let client = Arc::new(ScriptedShellClient::new(tool_command(root)));
    let runtime = build_runtime(root, Arc::clone(&client)).await;
    let mut rpc = RpcClient::serve(Arc::clone(&runtime));
    let id = rpc
        .send(
            "turn/start",
            serde_json::json!({ "session_id": session_id, "prompt": NEXT_PROMPT }),
        )
        .await;
    let response = tokio::time::timeout(TURN_BOUND, rpc.response(id))
        .await
        .unwrap_or_else(|_| {
            panic!(
                "turn/start did not answer within {TURN_BOUND:?} (the interrupted call ran {} times)",
                tool_runs(root)
            )
        });
    // The open run resumes at the attach, before the new turn is admitted,
    // so a call it runs again has its `runs` line by now.
    assert_eq!(
        tool_runs(root),
        1,
        "the interrupted call ran again: {response}"
    );
    assert!(response["error"].is_null(), "turn/start failed: {response}");
    assert_eq!(
        client.tool_calls.load(Ordering::SeqCst),
        0,
        "the model never asked for the call again"
    );
    assert_eq!(
        client.requests.load(Ordering::SeqCst),
        2,
        "two model calls: the resumed run's next one and the new turn's"
    );
    assert_eq!(
        client.requests_seeing_notice.load(Ordering::SeqCst),
        0,
        "the call's own result tells the model: no interrupted-run notice"
    );
    let history = runtime
        .session_service()
        .read_history(
            &meerkat_core::SessionId::parse(&session_id).unwrap(),
            SessionHistoryQuery::default(),
        )
        .await
        .expect("read history");
    let prompts = |prompt: &str| {
        history
            .messages
            .iter()
            .filter(|message| {
                matches!(message, Message::User(user) if user.text_content().contains(prompt))
            })
            .count()
    };
    assert_eq!(
        prompts(INTERRUPTED_PROMPT),
        1,
        "the interrupted request appears exactly once"
    );
    assert_eq!(prompts(NEXT_PROMPT), 1, "the new request appears once");
    let calls = history
        .messages
        .iter()
        .filter(|message| {
            matches!(message, Message::BlockAssistant(answer)
                if answer.tool_calls().any(|call| call.id == TOOL_CALL_ID))
        })
        .count();
    assert_eq!(calls, 1, "the model asked for the call exactly once");
    let results: Vec<String> = history
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::ToolResults { results, .. } => Some(results),
            _ => None,
        })
        .flatten()
        .filter(|result| result.tool_use_id == TOOL_CALL_ID)
        .map(meerkat_core::ToolResult::text_content)
        .collect();
    assert_eq!(
        results.len(),
        1,
        "the call has exactly one result: {results:?}"
    );
    assert!(
        results[0].contains("outcome_uncertain") && results[0].contains("It was not run again"),
        "the interrupted call is answered outcome-uncertain: {}",
        results[0]
    );
    assert!(
        !history.messages.iter().any(|message| matches!(
            message,
            Message::SystemNotice(notice) if notice.kind == SystemNoticeKind::ToolProcessRecovery
        )),
        "no interrupted-run notice"
    );
    let answers = history
        .messages
        .iter()
        .filter(|message| {
            matches!(message, Message::BlockAssistant(answer)
                if answer.text_blocks().collect::<String>() == ANSWER)
        })
        .count();
    assert_eq!(
        answers, 2,
        "the resumed run and the new turn answered once each"
    );
    // The killed tool's delayed effect never happens: its shell is gone.
    assert_tool_processes_gone(root).await;
    assert!(
        !root.join("effect").exists(),
        "the interrupted tool's effect happened"
    );
    assert_eq!(tool_runs(root), 1, "the interrupted call ran again");
}

#[tokio::test]
async fn rpc_on_demand_attach_settles_an_interrupted_run_instead_of_replaying_it() {
    on_demand_attach_resumes_the_interrupted_run(false).await;
}

#[tokio::test]
async fn rpc_on_demand_attach_settles_an_interrupted_run_instead_of_replaying_it_whole_blob() {
    on_demand_attach_resumes_the_interrupted_run(true).await;
}

/// Crash window: the run finished (its tool ran once) and its provisional
/// tail was written, but the host died before the run's boundary committed.
/// Durable-tail recovery owns that run. The cold attach resolves it first
/// (#1285): the run is committed exactly once, so custody has nothing left to
/// settle (no record remains, no notice is shown), the finished tool is not
/// re-run, and the next turn is served with a single model call.
#[tokio::test]
async fn a_run_with_a_provisional_tail_is_left_to_durable_tail_recovery() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    mkfifo(&root.join("phase.fifo"));
    let mut gateway = spawn_child(root, PHASE_BEFORE_COMMIT);
    let held = read_fifo(root.join("phase.fifo")).await;
    gateway.kill().unwrap();
    gateway.wait().unwrap();
    assert_eq!(held.as_deref().map(str::trim), Some("held"));
    let session_id = std::fs::read_to_string(root.join("session-id"))
        .expect("session id")
        .trim()
        .to_string();
    let scope = root
        .join("realm")
        .join("tool_process_custody")
        .join(&session_id);

    let client = Arc::new(ScriptedShellClient::new(quick_tool_command(root)));
    let runtime = build_runtime(root, Arc::clone(&client)).await;
    let mut rpc = RpcClient::serve(Arc::clone(&runtime));
    let id = rpc
        .send(
            "turn/start",
            serde_json::json!({ "session_id": session_id, "prompt": NEXT_PROMPT }),
        )
        .await;
    let response = rpc.response(id).await;
    assert!(
        response["error"].is_null(),
        "the turn after the crash window is served: {response}"
    );
    assert_eq!(
        client.requests.load(Ordering::SeqCst),
        1,
        "one model call: the interrupted run was committed, not rolled back and re-run"
    );

    let records: Vec<serde_json::Value> = match std::fs::read_dir(&scope) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().and_then(|e| e.to_str()) == Some("json"))
            .map(|entry| serde_json::from_slice(&std::fs::read(entry.path()).unwrap()).unwrap())
            .collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => panic!("custody scope: {error}"),
    };
    assert!(
        records.is_empty(),
        "a run durable-tail recovery committed leaves no custody record to settle: {records:?}"
    );
    assert_eq!(
        client.requests_seeing_notice.load(Ordering::SeqCst),
        0,
        "no interrupted-run notice for a run durable-tail recovery owns"
    );
    let ran = std::fs::read_to_string(root.join("ran")).unwrap_or_default();
    assert_eq!(
        ran.lines().count(),
        1,
        "the finished tool was re-run: {ran:?}"
    );

    // The committed transcript holds the interrupted run once, as committed
    // by durable-tail recovery (its tool results included), and no
    // interrupted-run notice.
    let history = runtime
        .session_service()
        .read_history(
            &meerkat_core::SessionId::parse(&session_id).unwrap(),
            SessionHistoryQuery::default(),
        )
        .await
        .expect("read history");
    let prompts = history
        .messages
        .iter()
        .filter(|message| {
            matches!(message, Message::User(user) if user.text_content().contains(INTERRUPTED_PROMPT))
        })
        .count();
    assert_eq!(prompts, 1, "the interrupted request appears exactly once");
    assert!(
        history
            .messages
            .iter()
            .any(|message| matches!(message, Message::ToolResults { .. })),
        "the recovered run's committed turn is in the transcript"
    );
    assert!(
        !history.messages.iter().any(|message| matches!(
            message,
            Message::SystemNotice(notice) if notice.kind == SystemNoticeKind::ToolProcessRecovery
        )),
        "no interrupted-run notice for a run durable-tail recovery committed"
    );
}

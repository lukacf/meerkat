//! RPC lazy resume across the run-end crash window.
//!
//! A child RPC "gateway" process creates a deferred session and runs one turn,
//! paused at the runtime loop's terminal commit: the agent finished the run
//! and its run-end checkpoint wrote the provisional tail, but the run's
//! boundary never committed. The parent SIGKILLs the child in that window,
//! opens the same realm in a fresh RPC runtime, and serves `turn/start` for
//! the session, which resumes it lazily. That turn must succeed, and the
//! transcript must hold the interrupted run's exchange exactly once followed
//! by the new one.

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
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

const ROOT_ENV: &str = "MEERKAT_LAZY_RESUME_TAIL_ROOT";
const CHILD_TEST: &str = "lazy_resume_tail_gateway_child";
const INTERRUPTED_PROMPT: &str = "first prompt";
const NEXT_PROMPT: &str = "second prompt";

/// Scripted model: answers every request with a fixed text.
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

fn spawn_child(root: &Path) -> std::process::Child {
    let log = std::fs::File::create(root.join("child.log")).expect("child log");
    std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", CHILD_TEST, "--nocapture", "--test-threads=1"])
        .env(ROOT_ENV, root)
        .stdout(log.try_clone().expect("child log"))
        .stderr(log)
        .spawn()
        .expect("spawn child")
}

/// Run the gateway child until it reports the terminal-commit pause, then
/// SIGKILL it (by pid) and confirm it exited.
///
/// A watcher thread owns the child and waits for its exit. If the child dies
/// before it reports the pause, the watcher writes `exited` into the fifo,
/// which unblocks the read instead of leaving the test hung, and the failure
/// then shows the child's output.
async fn run_gateway_until_paused_then_kill(root: &Path) {
    let fifo = root.join("phase.fifo");
    let child = spawn_child(root);
    let pid = child.id();
    let paused_seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (exited_tx, exited_rx) = std::sync::mpsc::channel::<()>();
    {
        let fifo = fifo.clone();
        let paused_seen = Arc::clone(&paused_seen);
        std::thread::spawn(move || {
            let mut child = child;
            let _ = child.wait();
            let _ = exited_tx.send(());
            if !paused_seen.load(Ordering::SeqCst) {
                let _ = std::fs::write(fifo, "exited\n");
            }
        });
    }
    let reported = read_fifo(fifo).await;
    if reported.as_deref().map(str::trim) != Some("held") {
        let log = std::fs::read_to_string(root.join("child.log")).unwrap_or_default();
        panic!("gateway did not reach its terminal-commit pause ({reported:?}); output:\n{log}");
    }
    paused_seen.store(true, Ordering::SeqCst);
    let status = std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status()
        .expect("kill gateway");
    assert!(status.success(), "kill -9 {pid}");
    tokio::task::spawn_blocking(move || exited_rx.recv())
        .await
        .unwrap()
        .expect("gateway exit observed");
}

fn mkfifo(path: &Path) {
    let status = std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .expect("run mkfifo");
    assert!(status.success(), "mkfifo {}", path.display());
}

/// Block (no polling) until the child writes a line into `fifo`.
async fn read_fifo(fifo: PathBuf) -> Option<String> {
    tokio::time::timeout(
        Duration::from_secs(120),
        tokio::task::spawn_blocking(move || std::fs::read_to_string(fifo)),
    )
    .await
    .ok()
    .map(|joined| joined.unwrap().unwrap())
}

async fn build_runtime(root: &Path, client: Arc<TextClient>) -> Arc<SessionRuntime> {
    let (_manifest, persistence) = meerkat::open_realm_persistence_in(
        root,
        "lazy-resume-tail-realm",
        Some(meerkat_store::RealmBackend::Sqlite),
        Some(meerkat_store::RealmOrigin::Explicit),
    )
    .await
    .expect("open realm persistence");
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("project dir");
    // No runtime root: tool-process custody is disabled, so nothing but the
    // durable-tail recovery can act on the interrupted run.
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

/// The gateway role: create a deferred session, record its id, run one turn
/// paused at its terminal commit, report the pause, and wait to be killed.
#[tokio::test]
async fn lazy_resume_tail_gateway_child() {
    let Some(root) = std::env::var_os(ROOT_ENV).map(PathBuf::from) else {
        return;
    };
    let client = Arc::new(TextClient {
        requests: AtomicUsize::new(0),
    });
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
    let (entered, _release) = runtime
        .runtime_adapter()
        .arm_runtime_loop_before_terminal_commit_test_hook(
            meerkat_core::SessionId::parse(session_id).unwrap(),
        );
    rpc.send(
        "turn/start",
        serde_json::json!({ "session_id": session_id, "prompt": INTERRUPTED_PROMPT }),
    )
    .await;
    entered.await.expect("terminal commit reached");
    let fifo = root.join("phase.fifo");
    tokio::task::spawn_blocking(move || std::fs::write(fifo, "held\n"))
        .await
        .unwrap()
        .unwrap();
    std::future::pending::<()>().await;
}

#[tokio::test]
async fn lazy_resume_after_an_uncommitted_run_terminal_serves_the_next_turn() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    mkfifo(&root.join("phase.fifo"));
    run_gateway_until_paused_then_kill(root).await;
    let session_id = std::fs::read_to_string(root.join("session-id"))
        .expect("session id")
        .trim()
        .to_string();

    let client = Arc::new(TextClient {
        requests: AtomicUsize::new(0),
    });
    let runtime = build_runtime(root, Arc::clone(&client)).await;
    let mut rpc = RpcClient::serve(Arc::clone(&runtime));
    let result = rpc
        .call(
            "turn/start",
            serde_json::json!({ "session_id": session_id, "prompt": NEXT_PROMPT }),
        )
        .await;
    assert_eq!(result["text"], "answered", "{result}");
    assert_eq!(
        client.requests.load(Ordering::SeqCst),
        1,
        "the resumed session makes exactly one model call: the interrupted run was \
         committed by durable-tail recovery, not rolled back and re-run"
    );

    let history = rpc
        .call(
            "session/history",
            serde_json::json!({ "session_id": session_id }),
        )
        .await;
    let users: Vec<String> = history["messages"]
        .as_array()
        .expect("history messages")
        .iter()
        .filter(|message| message["role"] == "user")
        .map(|message| message.to_string())
        .collect();
    assert_eq!(
        users
            .iter()
            .filter(|m| m.contains(INTERRUPTED_PROMPT))
            .count(),
        1,
        "the interrupted run's prompt is committed exactly once: {users:?}"
    );
    assert_eq!(
        users.iter().filter(|m| m.contains(NEXT_PROMPT)).count(),
        1,
        "the next turn's prompt is committed: {users:?}"
    );
}

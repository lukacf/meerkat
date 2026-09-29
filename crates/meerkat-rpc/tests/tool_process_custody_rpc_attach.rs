//! Abrupt gateway death mid-tool on an RPC session, recovered through the
//! RPC on-demand attach path.
//!
//! A child RPC "gateway" process creates a deferred session and starts a turn
//! whose scripted model calls the shell tool with a delayed file effect, and
//! is SIGKILLed while the tool runs. The next RPC incarnation (this test
//! process) serves `turn/start` for the session, which attaches the runtime
//! executor on demand before any agent is built. The runtime must settle the
//! session's process custody at that attach: kill the orphaned tool, settle
//! the interrupted input instead of replaying it (a replay would reach the
//! model without the notice and call the tool again), and record the typed
//! notice so the next real turn's one model call sees it.

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
use meerkat_core::types::{SystemNoticeBlock, SystemNoticeKind};
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
                delta: "noted".to_string(),
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
            outcome: LlmDoneOutcome::Success { stop_reason },
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

fn tool_command(root: &Path) -> String {
    format!(
        "echo started > '{}'; sleep 2; echo effect >> '{}'",
        root.join("started.fifo").display(),
        root.join("effect").display()
    )
}

fn mkfifo(path: &Path) {
    let status = std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .expect("run mkfifo");
    assert!(status.success(), "mkfifo {}", path.display());
}

async fn build_runtime(root: &Path, client: Arc<ScriptedShellClient>) -> Arc<SessionRuntime> {
    let (_manifest, persistence) = meerkat::open_realm_persistence_in(
        root,
        "custody-rpc-realm",
        Some(meerkat_store::RealmBackend::Sqlite),
        Some(meerkat_store::RealmOrigin::Explicit),
    )
    .await
    .expect("open realm persistence");
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
            );
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
    if std::env::var_os(PHASE_ENV).is_none() {
        return;
    }
    let client = Arc::new(ScriptedShellClient::new(tool_command(&root)));
    let runtime = build_runtime(&root, client).await;
    let mut rpc = RpcClient::serve(runtime);
    let created = rpc
        .call(
            "session/create",
            serde_json::json!({ "prompt": "", "initial_turn": "deferred" }),
        )
        .await;
    let session_id = created["session_id"].as_str().expect("session id");
    std::fs::write(root.join("session-id"), format!("{session_id}\n")).expect("session id");
    // The turn blocks in the tool until the parent kills this process.
    rpc.send(
        "turn/start",
        serde_json::json!({ "session_id": session_id, "prompt": INTERRUPTED_PROMPT }),
    )
    .await;
    std::future::pending::<()>().await;
}

#[tokio::test]
async fn rpc_on_demand_attach_settles_an_interrupted_run_instead_of_replaying_it() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    mkfifo(&root.join("started.fifo"));

    let mut gateway = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", CHILD_TEST, "--nocapture", "--test-threads=1"])
        .env(PHASE_ENV, "gateway")
        .env(ROOT_ENV, root)
        .env("MEERKAT_DISABLE_GRAPH_DECODE_MEMO", "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn gateway");
    // Block (no polling) until the tool itself reports it is running.
    let fifo = root.join("started.fifo");
    let started = tokio::time::timeout(
        Duration::from_secs(120),
        tokio::task::spawn_blocking(move || std::fs::read_to_string(fifo)),
    )
    .await;
    // Abrupt gateway death mid-tool.
    gateway.kill().unwrap();
    gateway.wait().unwrap();
    let started = started
        .expect("the gateway's shell tool never started")
        .unwrap()
        .unwrap();
    assert_eq!(started.trim(), "started");
    let session_id = std::fs::read_to_string(root.join("session-id"))
        .expect("session id")
        .trim()
        .to_string();

    // Next incarnation: `turn/start` attaches the runtime executor on demand
    // before any agent exists for the session.
    let client = Arc::new(ScriptedShellClient::new(tool_command(root)));
    let runtime = build_runtime(root, Arc::clone(&client)).await;
    let mut rpc = RpcClient::serve(Arc::clone(&runtime));
    rpc.call(
        "turn/start",
        serde_json::json!({ "session_id": session_id, "prompt": NEXT_PROMPT }),
    )
    .await;

    assert_eq!(
        client.requests.load(Ordering::SeqCst),
        1,
        "only the new turn reached the model: the interrupted input was not replayed"
    );
    assert_eq!(
        client.requests_seeing_notice.load(Ordering::SeqCst),
        1,
        "the new turn sees the interrupted-run notice"
    );
    assert_eq!(
        client.tool_calls.load(Ordering::SeqCst),
        0,
        "the interrupted input was replayed"
    );
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
    let blocks: Vec<&SystemNoticeBlock> = history
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::SystemNotice(notice)
                if notice.kind == SystemNoticeKind::ToolProcessRecovery =>
            {
                Some(notice.blocks.iter())
            }
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(blocks.len(), 1, "exactly one interrupted-run notice");
    assert!(
        matches!(
            blocks[0],
            SystemNoticeBlock::ToolProcessInterrupted { tool_call_id: Some(id), .. }
                if id == TOOL_CALL_ID
        ),
        "{:?}",
        blocks[0]
    );
    // The killed tool's delayed effect never happens.
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(
        !root.join("effect").exists(),
        "the interrupted tool's effect happened"
    );
}

//! Real-binary scenarios: a sign-in-protected MCP connection reaches only the
//! original live session that asked for it, under its realm's credential.
//!
//! Two `rkat-rpc` processes serve realms `alpha` and `beta` from one state
//! root. A loopback OAuth-protected MCP server (the canary fixture) is
//! installed in `alpha`'s own config and attached live to one existing
//! `alpha` session. A scripted chat server records what
//! each turn is offered and calls the server's tool when it is offered.
//!
//! Without a credential nothing is offered. A sign-in for another account
//! and a denied sign-in attach nothing. A real sign-in in `alpha` gives the
//! original session (same session id, not a new one) the tool; a sibling
//! session in the same realm and a session in `beta` stay without it, and
//! `beta` cannot sign in for `alpha`'s connection. Signing out (revocation)
//! unpublishes the tool from the original session, and a later sign-in gives
//! the same session the tool back.

#![cfg(any(target_os = "linux", target_os = "macos"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::large_futures,
    clippy::disallowed_methods
)]

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use meerkat::test_fixtures::mcp_oauth::{CODE_CANARY, ECHO_REPLY, McpOAuthFixture, SUBJECT};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc;
use tokio::time::{Duration, sleep, timeout};

const MODEL: &str = "realm-mcp-model";
const SERVER: &str = "guarded";
const TOOL: &str = "echo";
const REDIRECT_URI: &str = "http://127.0.0.1:1/mcp/oauth/callback";
const CALL_DEADLINE: Duration = Duration::from_mins(3);

/// Cargo and nextest advertise the crate's own binary to its integration
/// tests through the runtime environment.
fn rkat_rpc_binary() -> PathBuf {
    let advertised = std::env::var_os("CARGO_BIN_EXE_rkat-rpc")
        .map(PathBuf::from)
        .expect("CARGO_BIN_EXE_rkat-rpc is set for meerkat-rpc integration tests");
    assert!(
        advertised.exists(),
        "rkat-rpc binary at {}",
        advertised.display()
    );
    advertised
}

/// One chat-completions request the scripted server answered.
#[derive(Debug, Clone)]
struct ChatRequest {
    offered_tools: Vec<String>,
    tool_result: Option<String>,
}

/// Scripted OpenAI-compatible chat-completions server: a request offered
/// [`TOOL`] and carrying no tool result is answered with a call to it, any
/// other with a plain answer. Every request is recorded.
struct ScriptedChat {
    requests: Arc<Mutex<Vec<ChatRequest>>>,
    port: u16,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for ScriptedChat {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl ScriptedChat {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind scripted chat server");
        let port = listener.local_addr().expect("scripted chat address").port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        let task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                tokio::spawn(serve_chat(socket, Arc::clone(&recorded)));
            }
        });
        Self {
            requests,
            port,
            task,
        }
    }

    fn take(&self) -> Vec<ChatRequest> {
        std::mem::take(&mut *self.requests.lock().expect("chat request log"))
    }

    /// A realm document binding `realm` to this chat server, plus `extra`.
    fn realm_doc(&self, realm: &str, extra: &str) -> String {
        let port = self.port;
        format!(
            r#"[self_hosted.servers.scripted]
base_url = "http://127.0.0.1:{port}/v1"

[self_hosted.models."{MODEL}"]
server = "scripted"
remote_model = "{MODEL}"

[realm."{realm}"]
default_binding = "scripted"

[realm."{realm}".backend.scripted]
provider = "self_hosted"
backend_kind = "self_hosted"
server = "scripted"

[realm."{realm}".auth.scripted]
provider = "self_hosted"
auth_method = "none"
source = {{ kind = "platform_default" }}

[realm."{realm}".binding.scripted]
backend_profile = "scripted"
auth_profile = "scripted"
{extra}"#
        )
    }
}

fn sse(value: Value) -> String {
    format!("data: {value}\n\n")
}

fn text_of(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

fn chat_reply(request: &Value) -> (ChatRequest, String) {
    let offered_tools: Vec<String> = request["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let last = request["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .cloned()
        .unwrap_or_default();
    let tool_result = (last["role"].as_str() == Some("tool")).then(|| text_of(&last["content"]));
    let mut body = String::new();
    if tool_result.is_none() && offered_tools.iter().any(|tool| tool == TOOL) {
        body.push_str(&sse(json!({
            "choices": [{
                "delta": { "tool_calls": [{
                    "index": 0,
                    "id": "call_echo",
                    "type": "function",
                    "function": { "name": TOOL, "arguments": "{}" }
                }] },
                "finish_reason": null
            }]
        })));
        body.push_str(&sse(
            json!({ "choices": [{ "delta": {}, "finish_reason": "tool_calls" }] }),
        ));
    } else {
        body.push_str(&sse(json!({
            "choices": [{ "delta": { "content": "Done." }, "finish_reason": null }]
        })));
        body.push_str(&sse(
            json!({ "choices": [{ "delta": {}, "finish_reason": "stop" }] }),
        ));
    }
    body.push_str(&sse(json!({
        "choices": [],
        "usage": { "prompt_tokens": 10, "completion_tokens": 5 }
    })));
    body.push_str("data: [DONE]\n\n");
    (
        ChatRequest {
            offered_tools,
            tool_result,
        },
        body,
    )
}

async fn serve_chat(mut socket: tokio::net::TcpStream, requests: Arc<Mutex<Vec<ChatRequest>>>) {
    use tokio::io::AsyncReadExt;
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    let header_end = loop {
        if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
        match socket.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
        }
    };
    let headers = String::from_utf8_lossy(&buffer[..header_end]).to_ascii_lowercase();
    let content_length = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while buffer.len() < header_end + content_length {
        match socket.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
        }
    }
    let request: Value = serde_json::from_slice(&buffer[header_end..header_end + content_length])
        .unwrap_or_default();
    let (recorded, body) = chat_reply(&request);
    requests.lock().expect("chat request log").push(recorded);
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.shutdown().await;
}

/// Roots shared by both realms.
struct Roots {
    _temp: TempDir,
    home: PathBuf,
    project: PathBuf,
    state_root: PathBuf,
}

impl Roots {
    async fn new() -> Self {
        let temp = TempDir::new().expect("temp dir");
        let home = temp.path().join("home");
        let project = temp.path().join("project");
        let state_root = temp.path().join("realms");
        for dir in [&home, &project, &state_root] {
            tokio::fs::create_dir_all(dir).await.expect("create root");
        }
        Self {
            _temp: temp,
            home,
            project,
            state_root,
        }
    }

    fn realm_doc_path(&self, realm: &str) -> PathBuf {
        self.state_root.join(realm).join("config.toml")
    }

    async fn write_realm_doc(&self, realm: &str, content: &str) {
        let path = self.realm_doc_path(realm);
        tokio::fs::create_dir_all(path.parent().expect("realm dir"))
            .await
            .expect("create realm dir");
        tokio::fs::write(&path, content)
            .await
            .expect("write realm doc");
    }

    async fn read_realm_doc(&self, realm: &str) -> String {
        tokio::fs::read_to_string(self.realm_doc_path(realm))
            .await
            .expect("read realm doc")
    }
}

/// A JSON-RPC client over one `rkat-rpc` process's stdio.
struct RpcHost {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::UnboundedReceiver<String>,
    next_id: u64,
    /// Every line that was not the awaited response, in order.
    notifications: Vec<String>,
}

impl RpcHost {
    fn spawn(binary: &Path, roots: &Roots, realm: &str) -> Self {
        let mut command = Command::new(binary);
        for name in [
            "RKAT_ANTHROPIC_API_KEY",
            "ANTHROPIC_API_KEY",
            "RKAT_OPENAI_API_KEY",
            "OPENAI_API_KEY",
            "RKAT_GEMINI_API_KEY",
            "GEMINI_API_KEY",
            "RKAT_TEST_CLIENT",
        ] {
            command.env_remove(name);
        }
        let mut child = command
            .current_dir(&roots.project)
            .env("HOME", &roots.home)
            .env("XDG_CONFIG_HOME", roots.home.join("config"))
            .arg("--realm")
            .arg(realm)
            .arg("--state-root")
            .arg(&roots.state_root)
            .arg("--context-root")
            .arg(&roots.project)
            .arg("--user-config-root")
            .arg(&roots.home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("rkat-rpc spawns");
        let stdin = child.stdin.take().expect("stdin pipe");
        let stdout = child.stdout.take().expect("stdout pipe");
        let (sender, lines) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if sender.send(line).is_err() {
                    return;
                }
            }
        });
        Self {
            child,
            stdin,
            lines,
            next_id: 1,
            notifications: Vec::new(),
        }
    }

    /// Send one request and wait for its response; notifications are skipped.
    async fn call(&mut self, method: &str, params: Value) -> Result<Value, Value> {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.stdin
            .write_all(format!("{request}\n").as_bytes())
            .await
            .expect("write request");
        self.stdin.flush().await.expect("flush request");
        let wait = async {
            loop {
                let line = self
                    .lines
                    .recv()
                    .await
                    .unwrap_or_else(|| panic!("rkat-rpc closed stdout during {method}"));
                let Ok(message) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if message["id"].as_u64() != Some(id) {
                    self.notifications.push(line);
                    continue;
                }
                return match message.get("error") {
                    Some(error) if !error.is_null() => Err(error.clone()),
                    _ => Ok(message["result"].clone()),
                };
            }
        };
        timeout(CALL_DEADLINE, wait)
            .await
            .unwrap_or_else(|_| panic!("{method} did not answer in time"))
    }

    async fn ok(&mut self, method: &str, params: Value) -> Value {
        self.call(method, params)
            .await
            .unwrap_or_else(|error| panic!("{method} failed: {error}"))
    }

    async fn create_session(&mut self) -> String {
        let created = self
            .ok(
                "session/create",
                json!({ "prompt": "Hello.", "model": MODEL, "provider": "self_hosted" }),
            )
            .await;
        created["session_id"]
            .as_str()
            .unwrap_or_else(|| panic!("session/create carries a session_id: {created}"))
            .to_owned()
    }

    /// One turn on `session`; the answering session must be that session.
    async fn turn(&mut self, session: &str) {
        let result = self
            .ok(
                "turn/start",
                json!({ "session_id": session, "prompt": "Use what you have, then answer." }),
            )
            .await;
        assert_eq!(
            result["session_id"].as_str(),
            Some(session),
            "the turn ran in the original session: {result}"
        );
    }

    async fn shutdown(mut self) {
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
    }
}

fn offered(requests: &[ChatRequest]) -> bool {
    requests
        .iter()
        .any(|request| request.offered_tools.iter().any(|tool| tool == TOOL))
}

/// Whether the fixture's echo reply came back as a tool result: an
/// observed answer, not proof of what reached the server (see
/// [`protected_requests`]).
fn answered(requests: &[ChatRequest]) -> bool {
    requests
        .iter()
        .any(|request| request.tool_result.as_deref() == Some(ECHO_REPLY))
}

/// The server's config as installed: the fixture URL, the account the
/// realm's connection is for.
fn server_config(fixture: &McpOAuthFixture) -> Value {
    json!({
        "name": SERVER,
        "url": fixture.mcp_url(),
        "oauth_account": SUBJECT,
    })
}

fn installed_server_toml(fixture: &McpOAuthFixture) -> String {
    format!(
        "\n[[tools.mcp_servers]]\nname = \"{SERVER}\"\nurl = \"{}\"\noauth_account = \"{SUBJECT}\"\n",
        fixture.mcp_url()
    )
}

fn login_target(fixture: &McpOAuthFixture, account: &str) -> Value {
    json!({
        "server_name": SERVER,
        "server_url": fixture.mcp_url(),
        "oauth_account": account,
    })
}

/// Whether `line` is `session`'s notification that the server's live
/// change reached `phase` (`failed` when it awaits sign-in or was refused).
fn is_phase(line: &str, session: &str, phase: &str) -> bool {
    let Ok(message) = serde_json::from_str::<Value>(line) else {
        return false;
    };
    let params = &message["params"];
    let change = &params["event"]["payload"];
    message["method"] == "session/event"
        && params["session_id"] == session
        && change["type"] == "tool_config_changed"
        && change["payload"]["target"] == SERVER
        && change["payload"]["status_info"]["phase"] == phase
}

/// Turns on `session` until the server's change reports `phase` in a
/// notification after index `since`, so a negative observation follows a
/// settled connection attempt.
async fn turn_until_phase(
    host: &mut RpcHost,
    chat: &ScriptedChat,
    session: &str,
    since: usize,
    phase: &str,
    step: &str,
) {
    for _ in 0..20 {
        host.turn(session).await;
        chat.take();
        if host.notifications[since..]
            .iter()
            .any(|line| is_phase(line, session, phase))
        {
            return;
        }
        sleep(Duration::from_millis(250)).await;
    }
    panic!("{step}: the server never reported {phase} within 20 turns");
}

/// Turns on `session` until `predicate` holds for one turn's requests.
async fn turn_until(
    host: &mut RpcHost,
    chat: &ScriptedChat,
    session: &str,
    step: &str,
    predicate: impl Fn(&[ChatRequest]) -> bool,
) -> Vec<ChatRequest> {
    for _ in 0..20 {
        host.turn(session).await;
        let requests = chat.take();
        if predicate(&requests) {
            return requests;
        }
        sleep(Duration::from_millis(250)).await;
    }
    let changes: Vec<&String> = host
        .notifications
        .iter()
        .filter(|line| line.contains("tool_config_changed") && line.contains(SERVER))
        .collect();
    panic!("{step}: never observed within 20 turns; server changes: {changes:#?}");
}

/// Requests the fixture's OAuth-protected MCP endpoint received, of any
/// method and with or without a credential.
fn protected_requests(fixture: &McpOAuthFixture) -> usize {
    fixture
        .request_paths()
        .iter()
        .filter(|path| path.as_str() == "/mcp")
        .count()
}

fn token_requests(fixture: &McpOAuthFixture) -> usize {
    fixture
        .request_paths()
        .iter()
        .filter(|path| path.as_str() == "/token")
        .count()
}

async fn sign_in(
    host: &mut RpcHost,
    fixture: &McpOAuthFixture,
    account: &str,
) -> Result<Value, Value> {
    let start = host
        .call(
            "auth/login/start",
            json!({ "mcp": login_target(fixture, account), "redirect_uri": REDIRECT_URI }),
        )
        .await?;
    host.call(
        "auth/login/complete",
        json!({
            "mcp": login_target(fixture, account),
            "code": CODE_CANARY,
            "state": start["state"],
            "redirect_uri": start["redirect_uri"],
        }),
    )
    .await
}

/// Shared set-up: `alpha` (with the installed server) and `beta` documents,
/// an `alpha` host with the original session and a sibling, and the server
/// attached live to the original session only, awaiting sign-in.
struct Scenario {
    roots: Roots,
    chat: ScriptedChat,
    fixture: McpOAuthFixture,
    binary: PathBuf,
    alpha: RpcHost,
    original: String,
    sibling: String,
}

impl Scenario {
    async fn start() -> Self {
        let binary = rkat_rpc_binary();
        let roots = Roots::new().await;
        let chat = ScriptedChat::start().await;
        let fixture = McpOAuthFixture::spawn().await.expect("OAuth MCP fixture");
        roots
            .write_realm_doc(
                "alpha",
                &chat.realm_doc("alpha", &installed_server_toml(&fixture)),
            )
            .await;
        roots
            .write_realm_doc("beta", &chat.realm_doc("beta", ""))
            .await;

        let mut alpha = RpcHost::spawn(&binary, &roots, "alpha");
        let original = alpha.create_session().await;
        let sibling = alpha.create_session().await;
        chat.take();

        // Attach to the original session only. Without a credential the
        // server is contacted, refused, and nothing is offered.
        let since = alpha.notifications.len();
        alpha
            .ok(
                "mcp/add",
                json!({ "session_id": original, "server_config": server_config(&fixture) }),
            )
            .await;
        turn_until_phase(
            &mut alpha,
            &chat,
            &original,
            since,
            "failed",
            "attach without a credential",
        )
        .await;
        alpha.turn(&original).await;
        let requests = chat.take();
        assert!(
            !offered(&requests),
            "without a credential nothing is offered: {requests:?}"
        );
        Self {
            roots,
            chat,
            fixture,
            binary,
            alpha,
            original,
            sibling,
        }
    }

    /// The original session gains the tool after a sign-in, and calls it.
    async fn expect_original_attached(&mut self, step: &str) {
        let requests = turn_until(&mut self.alpha, &self.chat, &self.original, step, offered).await;
        assert!(
            answered(&requests),
            "{step}: the tool answered in the original session: {requests:?}"
        );
    }

    /// A normal restart: the `alpha` host exits, then a new one starts on
    /// the same realm.
    async fn restart_alpha(&mut self) {
        let _ = self.alpha.child.start_kill();
        let _ = self.alpha.child.wait().await;
        self.alpha = RpcHost::spawn(&self.binary, &self.roots, "alpha");
    }

    async fn expect_offered_nothing(&mut self, session: &str, step: &str) {
        self.alpha.turn(session).await;
        let requests = self.chat.take();
        assert!(!offered(&requests), "{step}: nothing offered: {requests:?}");
    }
}

/// Wrong-account and denied sign-ins attach nothing; a real sign-in joins
/// the tool to the original session that awaits it (no re-add, no reload);
/// a sibling session stays without it.
#[tokio::test]
#[ignore = "lane:e2e-system"]
async fn integration_real_rpc_mcp_original_session_attachment() {
    let mut scenario = Scenario::start().await;
    let original = scenario.original.clone();
    let sibling = scenario.sibling.clone();

    // Wrong account: a sign-in for an account the realm's connection is not
    // for is refused before any authorization traffic.
    let tokens_before = token_requests(&scenario.fixture);
    let wrong = sign_in(&mut scenario.alpha, &scenario.fixture, "another-subject").await;
    assert!(
        wrong.is_err(),
        "a wrong-account sign-in is refused: {wrong:?}"
    );
    assert_eq!(
        token_requests(&scenario.fixture),
        tokens_before,
        "a wrong-account sign-in never reaches the token endpoint"
    );
    scenario
        .expect_offered_nothing(&original, "after the wrong-account sign-in")
        .await;

    // Denied: the authorization server refuses the code.
    scenario.fixture.fail_token_exchange(true);
    let denied = sign_in(&mut scenario.alpha, &scenario.fixture, SUBJECT).await;
    scenario.fixture.fail_token_exchange(false);
    assert!(denied.is_err(), "a denied sign-in fails: {denied:?}");
    scenario
        .expect_offered_nothing(&original, "after the denied sign-in")
        .await;

    // A real sign-in in alpha: the original session gains the tool.
    sign_in(&mut scenario.alpha, &scenario.fixture, SUBJECT)
        .await
        .unwrap_or_else(|error| panic!("the sign-in succeeds: {error}"));
    // The credential is stored in this test's own config root (the host's
    // file store under XDG_CONFIG_HOME), which the temp dir removes; nothing
    // reaches a shared or OS credential store.
    let credentials = scenario
        .roots
        .home
        .join("config")
        .join("meerkat")
        .join("credentials");
    assert!(
        std::fs::read_dir(&credentials).is_ok_and(|mut entries| entries.next().is_some()),
        "the sign-in is stored under the test's config root at {}",
        credentials.display()
    );
    scenario.expect_original_attached("after the sign-in").await;

    // The sibling session in the same realm never asked for it.
    scenario
        .expect_offered_nothing(&sibling, "the sibling session")
        .await;

    scenario.alpha.shutdown().await;
}

/// Another realm on the same state root stays without the realm's
/// connection: it cannot sign in for it, the same server attached live
/// there gets no tool (the credential is the first realm's), its document
/// is unchanged, and the original session keeps the tool.
#[tokio::test]
#[ignore = "lane:e2e-system"]
async fn integration_real_rpc_mcp_other_realm_gets_no_credential() {
    let mut scenario = Scenario::start().await;
    sign_in(&mut scenario.alpha, &scenario.fixture, SUBJECT)
        .await
        .unwrap_or_else(|error| panic!("the sign-in succeeds: {error}"));
    scenario.expect_original_attached("after the sign-in").await;

    // Beta: same state root, no installation. It cannot sign in for alpha's
    // connection, and the same server attached live there gets no tool (the
    // credential is alpha's).
    let beta_doc = scenario.roots.read_realm_doc("beta").await;
    let mut beta = RpcHost::spawn(&scenario.binary, &scenario.roots, "beta");
    let beta_session = beta.create_session().await;
    scenario.chat.take();
    let foreign = sign_in(&mut beta, &scenario.fixture, SUBJECT).await;
    assert!(
        foreign.is_err(),
        "beta cannot sign in for a connection it does not have: {foreign:?}"
    );
    let since = beta.notifications.len();
    beta.ok(
        "mcp/add",
        json!({ "session_id": beta_session, "server_config": server_config(&scenario.fixture) }),
    )
    .await;
    turn_until_phase(
        &mut beta,
        &scenario.chat,
        &beta_session,
        since,
        "failed",
        "beta attach",
    )
    .await;
    beta.turn(&beta_session).await;
    let requests = scenario.chat.take();
    assert!(
        !offered(&requests),
        "beta never uses alpha's credential: {requests:?}"
    );
    assert_eq!(
        scenario.roots.read_realm_doc("beta").await,
        beta_doc,
        "beta's document is unchanged"
    );
    beta.shutdown().await;

    // The original session still has the tool after beta's attempts.
    scenario
        .expect_original_attached("after beta's attempts")
        .await;
    scenario.alpha.shutdown().await;
}

/// Signing out of the realm's connection (revocation) unpublishes the tool
/// from the original session that was using it, with no reload: the next
/// turn is offered nothing and no old-credential echo is observed, and no
/// request reaches the protected endpoint. A later sign-in gives the same
/// original session the tool back, also when it lands before the session's
/// next turn; the sibling never gets it.
#[tokio::test]
#[ignore = "lane:e2e-system"]
async fn integration_real_rpc_mcp_sign_out_revokes_original_session() {
    let mut scenario = Scenario::start().await;
    let original = scenario.original.clone();
    let sibling = scenario.sibling.clone();
    sign_in(&mut scenario.alpha, &scenario.fixture, SUBJECT)
        .await
        .unwrap_or_else(|error| panic!("the sign-in succeeds: {error}"));
    scenario.expect_original_attached("after the sign-in").await;

    let sent_before_sign_out = protected_requests(&scenario.fixture);
    scenario
        .alpha
        .ok(
            "auth/logout",
            json!({ "mcp": login_target(&scenario.fixture, SUBJECT) }),
        )
        .await;
    // The unpublication applies at the next turn's boundary.
    scenario.alpha.turn(&original).await;
    let requests = scenario.chat.take();
    assert!(
        !answered(&requests),
        "after sign-out no old-credential echo is observed: {requests:?}"
    );
    assert!(
        !offered(&requests),
        "after sign-out the original session is offered nothing: {requests:?}"
    );
    scenario
        .expect_offered_nothing(&original, "after sign-out, again")
        .await;
    // Nothing was sent to the protected endpoint from the sign-out on: no
    // call, and no request of the closing connection.
    assert_eq!(
        protected_requests(&scenario.fixture),
        sent_before_sign_out,
        "after sign-out no request reaches the protected MCP endpoint"
    );

    // Signing in again joins the tool back to the same original session.
    sign_in(&mut scenario.alpha, &scenario.fixture, SUBJECT)
        .await
        .unwrap_or_else(|error| panic!("the second sign-in succeeds: {error}"));
    scenario
        .expect_original_attached("after signing in again")
        .await;
    scenario
        .expect_offered_nothing(&sibling, "the sibling session")
        .await;

    // Reverse order: sign out and sign in again before any turn of the
    // session. The sign-in landing during the release is kept, so the same
    // session gets the tool back at its next turns with no further sign-in.
    scenario
        .alpha
        .ok(
            "auth/logout",
            json!({ "mcp": login_target(&scenario.fixture, SUBJECT) }),
        )
        .await;
    sign_in(&mut scenario.alpha, &scenario.fixture, SUBJECT)
        .await
        .unwrap_or_else(|error| panic!("the third sign-in succeeds: {error}"));
    scenario
        .expect_original_attached("after sign-out and sign-in before a turn")
        .await;
    scenario
        .expect_offered_nothing(&sibling, "the sibling session, again")
        .await;
    scenario.alpha.shutdown().await;
}

/// After a normal restart of the host, the original session, reopened by the
/// new process, has the connection again under the realm's stored credential
/// (the same session id, its original attachment preserved), and still does
/// not after a sign-out.
#[tokio::test]
#[ignore = "needs S3/S4: original-session MCP attachment preserved across a host restart"]
async fn integration_real_rpc_mcp_original_session_attachment_survives_restart() {
    let mut scenario = Scenario::start().await;
    let original = scenario.original.clone();
    let sibling = scenario.sibling.clone();
    sign_in(&mut scenario.alpha, &scenario.fixture, SUBJECT)
        .await
        .unwrap_or_else(|error| panic!("the sign-in succeeds: {error}"));
    scenario
        .expect_original_attached("before the restart")
        .await;

    scenario.restart_alpha().await;
    scenario.expect_original_attached("after the restart").await;
    scenario
        .expect_offered_nothing(&sibling, "the sibling session after the restart")
        .await;

    scenario
        .alpha
        .ok(
            "auth/logout",
            json!({ "mcp": login_target(&scenario.fixture, SUBJECT) }),
        )
        .await;
    scenario.restart_alpha().await;
    scenario
        .expect_offered_nothing(&original, "after sign-out and restart")
        .await;
    scenario.alpha.shutdown().await;
}

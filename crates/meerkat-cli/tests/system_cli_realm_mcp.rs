#![cfg(feature = "integration-real-tests")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Real-binary regression: an MCP server installed in one realm's own config
//! is part of an ordinary `rkat` session in that realm, and only there.
//!
//! Realms `alpha` and `beta` share one state root, one context root and one
//! user config root, with no `mcp.toml` anywhere. A read-only, no-auth stdio
//! sentinel is installed into `alpha`'s own config under the generation
//! check. A fresh `rkat run` in `alpha`, driven by a scripted chat server, is
//! offered the sentinel's tool, calls it, and gets the sentinel's marker
//! back. The same run in `beta` is offered no such tool and never starts the
//! sentinel. A child realm of `alpha` inherits the server.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::process::Command;
use tokio::time::{Duration, timeout};

use tempfile::TempDir;

const MODEL: &str = "realm-mcp-model";
const SENTINEL_SERVER: &str = "realm-sentinel";
const SENTINEL_TOOL: &str = "sentinel_probe";
const SENTINEL_MARKER: &str = "REALM-SENTINEL-OK";

fn rkat_binary_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_rkat") {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path.canonicalize().unwrap_or(path));
        }
    }
    if let Some(target_dir) = std::env::var_os("CARGO_TARGET_DIR") {
        let target_dir = PathBuf::from(target_dir);
        for profile in ["debug", "release"] {
            let path = target_dir.join(profile).join("rkat");
            if path.exists() {
                return Some(path);
            }
        }
    }
    let manifest_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR")?);
    let workspace_root = manifest_dir.parent()?.parent()?;
    for dir in [
        "target-codex/debug",
        "target-codex/release",
        "target/debug",
        "target/release",
    ] {
        let path = workspace_root.join(dir).join("rkat");
        if path.exists() {
            return Some(path);
        }
    }
    None
}

/// A stdio MCP server in POSIX sh: one read-only tool that returns a fixed
/// marker, no auth. It touches `spawned` in its marker directory when it
/// starts and `called` when its tool runs.
const SENTINEL_SCRIPT: &str = r#"#!/bin/sh
exec 2>/dev/null
markers="$1"
: > "$markers/spawned"
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      version=$(printf '%s\n' "$line" | sed -n 's/.*"protocolVersion":"\([^"]*\)".*/\1/p')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"%s","capabilities":{"tools":{}},"serverInfo":{"name":"realm-sentinel","version":"1.0.0"}}}\n' "$id" "$version"
      ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"sentinel_probe","description":"Return the realm sentinel marker.","inputSchema":{"type":"object","properties":{},"required":[]}}]}}\n' "$id"
      ;;
    *'"method":"tools/call"'*)
      : > "$markers/called"
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"REALM-SENTINEL-OK"}],"isError":false}}\n' "$id"
      ;;
    *)
      if [ -n "$id" ]; then
        printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32601,"message":"method not found"}}\n' "$id"
      fi
      ;;
  esac
done
"#;

/// One chat-completions request the scripted server answered.
#[derive(Debug, Clone)]
struct ChatRequest {
    offered_tools: Vec<String>,
    tool_result: Option<String>,
}

/// Scripted OpenAI-compatible chat-completions server. A request offered the
/// sentinel's tool is answered with a call to it, the request carrying the
/// tool result with a final answer, and any other request with a plain
/// answer. Every request is recorded.
struct ScriptedChatServer {
    requests: Arc<Mutex<Vec<ChatRequest>>>,
    port: u16,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for ScriptedChatServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl ScriptedChatServer {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind scripted chat server");
        let port = listener.local_addr().expect("scripted chat address").port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        let task = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(serve_chat_connection(socket, Arc::clone(&recorded)));
            }
        });
        Self {
            requests,
            port,
            task,
        }
    }

    fn take_requests(&self) -> Vec<ChatRequest> {
        std::mem::take(&mut *self.requests.lock().expect("chat request log"))
    }

    /// A realm document binding `realm` to this server as its self-hosted
    /// default provider.
    fn realm_doc(&self, realm: &str, parent: Option<&str>) -> String {
        let port = self.port;
        let parent = parent.map_or_else(String::new, |parent| format!("parent = \"{parent}\"\n"));
        format!(
            r#"[self_hosted.servers.scripted]
base_url = "http://127.0.0.1:{port}/v1"

[self_hosted.models."{MODEL}"]
server = "scripted"
remote_model = "{MODEL}"

[realm."{realm}"]
{parent}default_binding = "scripted"

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
"#
        )
    }
}

fn sse_chunk(value: serde_json::Value) -> String {
    format!("data: {value}\n\n")
}

fn text_of(content: &serde_json::Value) -> String {
    match content {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

fn scripted_chat_reply(request: &serde_json::Value) -> (ChatRequest, String) {
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
    if tool_result.is_none() && offered_tools.iter().any(|tool| tool == SENTINEL_TOOL) {
        body.push_str(&sse_chunk(serde_json::json!({
            "choices": [{
                "delta": { "tool_calls": [{
                    "index": 0,
                    "id": "call_sentinel",
                    "type": "function",
                    "function": { "name": SENTINEL_TOOL, "arguments": "{}" }
                }] },
                "finish_reason": null
            }]
        })));
        body.push_str(&sse_chunk(
            serde_json::json!({ "choices": [{ "delta": {}, "finish_reason": "tool_calls" }] }),
        ));
    } else {
        let answer = if tool_result.is_some() {
            "The sentinel answered."
        } else {
            "No sentinel here."
        };
        body.push_str(&sse_chunk(serde_json::json!({
            "choices": [{ "delta": { "content": answer }, "finish_reason": null }]
        })));
        body.push_str(&sse_chunk(
            serde_json::json!({ "choices": [{ "delta": {}, "finish_reason": "stop" }] }),
        ));
    }
    body.push_str(&sse_chunk(serde_json::json!({
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

async fn serve_chat_connection(
    mut socket: tokio::net::TcpStream,
    requests: Arc<Mutex<Vec<ChatRequest>>>,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
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
    let request: serde_json::Value =
        serde_json::from_slice(&buffer[header_end..header_end + content_length])
            .unwrap_or_default();
    let (recorded, body) = scripted_chat_reply(&request);
    requests.lock().expect("chat request log").push(recorded);
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.shutdown().await;
}

/// Roots shared by every realm of the regression.
struct Roots {
    _temp: TempDir,
    home: PathBuf,
    project: PathBuf,
    state_root: PathBuf,
    markers: PathBuf,
    sentinel: PathBuf,
}

impl Roots {
    async fn new() -> Self {
        let temp = TempDir::new().expect("temp dir");
        let home = temp.path().join("home");
        let project = temp.path().join("project");
        let state_root = temp.path().join("realms");
        let markers = temp.path().join("markers");
        for dir in [&home, &project, &state_root, &markers] {
            tokio::fs::create_dir_all(dir).await.expect("create root");
        }
        let sentinel = temp.path().join("sentinel.sh");
        tokio::fs::write(&sentinel, SENTINEL_SCRIPT)
            .await
            .expect("write sentinel");
        Self {
            _temp: temp,
            home,
            project,
            state_root,
            markers,
            sentinel,
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

    fn marker(&self, name: &str) -> bool {
        self.markers.join(name).exists()
    }

    /// `rkat` with this regression's roots and no ambient provider keys.
    fn rkat(&self, rkat: &Path, realm: &str) -> Command {
        let mut command = Command::new(rkat);
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
        command
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .arg("--state-root")
            .arg(&self.state_root)
            .arg("--context-root")
            .arg(&self.project)
            .arg("--user-config-root")
            .arg(&self.home)
            .args(["--realm", realm])
            .stdin(std::process::Stdio::null());
        command
    }
}

async fn output_of(command: &mut Command, step: &str) -> std::process::Output {
    timeout(Duration::from_secs(180), command.output())
        .await
        .unwrap_or_else(|_| panic!("{step} did not finish in time"))
        .unwrap_or_else(|error| panic!("{step} did not start: {error}"))
}

fn expect_success(step: &str, output: &std::process::Output) {
    assert!(
        output.status.success(),
        "{step} failed (exit {:?})\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

async fn run_turn(roots: &Roots, rkat: &Path, realm: &str) -> std::process::Output {
    let mut command = roots.rkat(rkat, realm);
    command.args([
        "run",
        "--wait-for-mcp",
        "--output",
        "json",
        "-m",
        MODEL,
        "--provider",
        "self-hosted",
        "Ask the sentinel, then answer.",
    ]);
    output_of(&mut command, &format!("rkat run in {realm}")).await
}

async fn realm_generation(roots: &Roots, rkat: &Path, realm: &str) -> u64 {
    let mut command = roots.rkat(rkat, realm);
    command.args(["config", "get", "--format", "json", "--with-generation"]);
    let output = output_of(&mut command, "rkat config get").await;
    expect_success("rkat config get", &output);
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("config envelope is JSON");
    envelope["generation"]
        .as_u64()
        .expect("config envelope carries a generation")
}

/// Install the sentinel into `realm`'s own config through the realm writer,
/// under the generation check.
async fn install_sentinel(
    roots: &Roots,
    rkat: &Path,
    realm: &str,
    generation: u64,
) -> std::process::Output {
    let mut command = roots.rkat(rkat, realm);
    command
        .args(["mcp", "add", SENTINEL_SERVER, "--scope", "realm"])
        .args(["--expected-generation", &generation.to_string()])
        .arg("--")
        .arg("/bin/sh")
        .arg(&roots.sentinel)
        .arg(&roots.markers);
    output_of(&mut command, "install the sentinel").await
}

async fn realm_scope_listing(roots: &Roots, rkat: &Path, realm: &str) -> Vec<serde_json::Value> {
    let mut command = roots.rkat(rkat, realm);
    command.args(["mcp", "list", "--scope", "realm", "--json"]);
    let output = output_of(&mut command, "rkat mcp list --scope realm").await;
    expect_success("rkat mcp list --scope realm", &output);
    serde_json::from_slice(&output.stdout).expect("mcp list --json is a JSON array")
}

#[tokio::test]
#[ignore = "lane:e2e-system"]
async fn integration_real_cli_realm_mcp_servers() {
    if cfg!(windows) {
        eprintln!("Skipping: the sentinel MCP server is a POSIX sh script");
        return;
    }
    let rkat = rkat_binary_path().expect("rkat binary (build with `cargo build -p rkat`)");
    let roots = Roots::new().await;
    let chat = ScriptedChatServer::start().await;
    roots
        .write_realm_doc("alpha", &chat.realm_doc("alpha", None))
        .await;
    roots
        .write_realm_doc("beta", &chat.realm_doc("beta", None))
        .await;

    // Install through the realm writer at the current generation. The write
    // adds `[[tools.mcp_servers]]` and leaves the rest of the document as
    // written (bytes and presence).
    let alpha_doc = tokio::fs::read_to_string(roots.realm_doc_path("alpha"))
        .await
        .expect("read alpha doc");
    let generation = realm_generation(&roots, &rkat, "alpha").await;
    let installed = install_sentinel(&roots, &rkat, "alpha", generation).await;
    expect_success("install the sentinel", &installed);
    let installed_doc = tokio::fs::read_to_string(roots.realm_doc_path("alpha"))
        .await
        .expect("read alpha doc");
    assert!(
        installed_doc.starts_with(&alpha_doc),
        "the realm writer keeps the document as written:\n{installed_doc}"
    );
    let raw: toml::Table = toml::from_str(&installed_doc).expect("alpha doc is TOML");
    let mut keys: Vec<&str> = raw.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec!["realm", "self_hosted", "tools"],
        "{installed_doc}"
    );
    assert_eq!(
        raw["tools"]
            .as_table()
            .expect("tools table")
            .keys()
            .collect::<Vec<_>>(),
        vec!["mcp_servers"],
        "{installed_doc}"
    );

    // A write at a stale generation is refused and leaves the bytes alone.
    let stale = install_sentinel(&roots, &rkat, "alpha", generation).await;
    assert!(
        !stale.status.success(),
        "a stale generation must be refused"
    );
    assert!(
        String::from_utf8_lossy(&stale.stderr).contains("generation conflict"),
        "stderr:\n{}",
        String::from_utf8_lossy(&stale.stderr)
    );
    assert_eq!(
        tokio::fs::read_to_string(roots.realm_doc_path("alpha"))
            .await
            .expect("read alpha doc"),
        installed_doc
    );

    // Realm servers are literal: a definition holding an environment
    // reference is refused and nothing is written.
    let mut command = roots.rkat(&rkat, "alpha");
    command
        .args(["mcp", "add", "referencing", "--scope", "realm"])
        .args(["-e", "TOKEN=${HOST_SECRET}"])
        .args(["--", "/bin/true"]);
    let referencing = output_of(&mut command, "rkat mcp add with a reference").await;
    assert!(
        !referencing.status.success(),
        "a realm server with an environment reference must be refused"
    );
    let stderr = String::from_utf8_lossy(&referencing.stderr);
    assert!(
        stderr.contains("never expanded from the environment") && !stderr.contains("HOST_SECRET"),
        "stderr:\n{stderr}"
    );
    assert_eq!(
        tokio::fs::read_to_string(roots.realm_doc_path("alpha"))
            .await
            .expect("read alpha doc"),
        installed_doc
    );

    let alpha_listing = realm_scope_listing(&roots, &rkat, "alpha").await;
    assert_eq!(alpha_listing.len(), 1, "{alpha_listing:?}");
    assert_eq!(alpha_listing[0]["name"], SENTINEL_SERVER);
    assert_eq!(alpha_listing[0]["scope"], "realm");
    assert!(realm_scope_listing(&roots, &rkat, "beta").await.is_empty());

    // Realm beta first: same roots, no sentinel in its config. The sentinel
    // is never offered and its process never starts.
    let beta = run_turn(&roots, &rkat, "beta").await;
    expect_success("rkat run in beta", &beta);
    let requests = chat.take_requests();
    assert!(!requests.is_empty(), "beta's run reached the chat server");
    assert!(
        requests.iter().all(|request| !request
            .offered_tools
            .iter()
            .any(|tool| tool == SENTINEL_TOOL)),
        "beta must not be offered the sentinel: {requests:?}"
    );
    assert!(
        !roots.marker("spawned"),
        "the sentinel must not start in beta"
    );

    // Realm alpha: a fresh ordinary run lists the sentinel's tool and calls it.
    let alpha = run_turn(&roots, &rkat, "alpha").await;
    expect_success("rkat run in alpha", &alpha);
    let requests = chat.take_requests();
    assert!(
        requests.first().is_some_and(|request| request
            .offered_tools
            .iter()
            .any(|tool| tool == SENTINEL_TOOL)),
        "alpha's run must offer the sentinel's tool: {requests:?}\nstderr:\n{}",
        String::from_utf8_lossy(&alpha.stderr)
    );
    assert!(
        requests
            .iter()
            .any(|request| request.tool_result.as_deref() == Some(SENTINEL_MARKER)),
        "the sentinel's answer must reach the model: {requests:?}"
    );
    assert!(roots.marker("spawned") && roots.marker("called"));

    // A child realm of alpha inherits the server through composition.
    roots
        .write_realm_doc("alpha-child", &chat.realm_doc("alpha-child", Some("alpha")))
        .await;
    let child = run_turn(&roots, &rkat, "alpha-child").await;
    expect_success("rkat run in alpha-child", &child);
    let requests = chat.take_requests();
    assert!(
        requests.first().is_some_and(|request| request
            .offered_tools
            .iter()
            .any(|tool| tool == SENTINEL_TOOL)),
        "a child realm of alpha inherits the sentinel: {requests:?}"
    );

    // The child cannot remove the server it only inherits, with or without
    // `--scope realm`; the refusal names where it is configured and the
    // child's document is untouched.
    let child_doc = tokio::fs::read_to_string(roots.realm_doc_path("alpha-child"))
        .await
        .expect("read alpha-child doc");
    for scope in [&["--scope", "realm"][..], &[][..]] {
        let mut command = roots.rkat(&rkat, "alpha-child");
        command.args(["mcp", "remove", SENTINEL_SERVER]).args(scope);
        let removed = output_of(&mut command, "rkat mcp remove in alpha-child").await;
        assert!(
            !removed.status.success(),
            "an inherited server must not be removable from the child ({scope:?})"
        );
        assert!(
            String::from_utf8_lossy(&removed.stderr).contains("is inherited from a parent realm"),
            "{scope:?} stderr:\n{}",
            String::from_utf8_lossy(&removed.stderr)
        );
    }
    assert_eq!(
        tokio::fs::read_to_string(roots.realm_doc_path("alpha-child"))
            .await
            .expect("read alpha-child doc"),
        child_doc
    );

    // A project mcp.toml that defines the same name differently is a typed
    // conflict: neither definition silently shadows the other.
    tokio::fs::create_dir_all(roots.project.join(".rkat"))
        .await
        .expect("create project .rkat");
    tokio::fs::write(
        roots.project.join(".rkat/mcp.toml"),
        format!("[[servers]]\nname = \"{SENTINEL_SERVER}\"\ncommand = \"/bin/false\"\n"),
    )
    .await
    .expect("write conflicting project mcp.toml");
    let conflicted = run_turn(&roots, &rkat, "alpha").await;
    assert!(
        !conflicted.status.success(),
        "a conflicting definition must fail the run"
    );
    assert!(
        String::from_utf8_lossy(&conflicted.stderr)
            .contains("is defined differently in the realm config and in the project mcp.toml"),
        "stderr:\n{}",
        String::from_utf8_lossy(&conflicted.stderr)
    );
}

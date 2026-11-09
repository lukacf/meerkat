#![cfg(feature = "integration-real-tests")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::disallowed_methods
)]
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
# With `with-write`, the server also lists a mutating operation.
write_tool=''
if [ "$2" = with-write ]; then
  write_tool=',{"name":"sentinel_write","description":"Change the realm sentinel.","inputSchema":{"type":"object","properties":{},"required":[]}}'
fi
: > "$markers/spawned"
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      version=$(printf '%s\n' "$line" | sed -n 's/.*"protocolVersion":"\([^"]*\)".*/\1/p')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"%s","capabilities":{"tools":{}},"serverInfo":{"name":"realm-sentinel","version":"1.0.0"}}}\n' "$id" "$version"
      ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"sentinel_probe","description":"Return the realm sentinel marker.","inputSchema":{"type":"object","properties":{},"required":[]}}%s]}}\n' "$id" "$write_tool"
      ;;
    *'"method":"tools/call"'*)
      : > "$markers/called"
      case "$line" in
        *'"name":"sentinel_write"'*) : > "$markers/called-write" ;;
      esac
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
    /// Every tool result the request carries after its last assistant turn,
    /// in call order.
    tool_results: Vec<String>,
}

/// Scripted OpenAI-compatible chat-completions server. A request offered the
/// sentinel's tool is answered with a call to it, the request carrying the
/// tool result with a final answer, and any other request with a plain
/// answer. Every request is recorded. With forced calls set, a request
/// carrying no tool result is answered with one call to each of those tools
/// in a single assistant turn instead, whether they were offered or not, as
/// a model naming an operation it was never given.
struct ScriptedChatServer {
    requests: Arc<Mutex<Vec<ChatRequest>>>,
    forced_call: Arc<Mutex<Vec<String>>>,
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
        let forced_call = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        let forced = Arc::clone(&forced_call);
        let task = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(serve_chat_connection(
                    socket,
                    Arc::clone(&recorded),
                    Arc::clone(&forced),
                ));
            }
        });
        Self {
            requests,
            forced_call,
            port,
            task,
        }
    }

    /// Answer every later request that carries no tool result with a call to
    /// `tool` (`None` restores the default script).
    fn force_call(&self, tool: Option<&str>) {
        self.force_calls(tool.as_slice());
    }

    /// Answer every later request that carries no tool result with one call
    /// to each of `tools`, in order (empty restores the default script).
    fn force_calls(&self, tools: &[&str]) {
        *self.forced_call.lock().expect("forced call") =
            tools.iter().map(|tool| (*tool).to_owned()).collect();
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

fn scripted_chat_reply(
    request: &serde_json::Value,
    forced_calls: &[String],
) -> (ChatRequest, String) {
    let offered_tools: Vec<String> = request["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let messages = request["messages"].as_array().cloned().unwrap_or_default();
    let last = messages.last().cloned().unwrap_or_default();
    let tool_result = (last["role"].as_str() == Some("tool")).then(|| text_of(&last["content"]));
    let mut tool_results: Vec<String> = messages
        .iter()
        .rev()
        .take_while(|message| message["role"].as_str() == Some("tool"))
        .map(|message| text_of(&message["content"]))
        .collect();
    tool_results.reverse();
    let mut body = String::new();
    let calls: Vec<String> = if !forced_calls.is_empty() {
        forced_calls.to_vec()
    } else if offered_tools.iter().any(|tool| tool == SENTINEL_TOOL) {
        vec![SENTINEL_TOOL.to_owned()]
    } else {
        Vec::new()
    };
    if tool_result.is_none() && !calls.is_empty() {
        let tool_calls: Vec<serde_json::Value> = calls
            .iter()
            .enumerate()
            .map(|(index, name)| {
                serde_json::json!({
                    "index": index,
                    "id": format!("call_{name}"),
                    "type": "function",
                    "function": { "name": name, "arguments": "{}" }
                })
            })
            .collect();
        body.push_str(&sse_chunk(serde_json::json!({
            "choices": [{
                "delta": { "tool_calls": tool_calls },
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
            tool_results,
        },
        body,
    )
}

async fn serve_chat_connection(
    mut socket: tokio::net::TcpStream,
    requests: Arc<Mutex<Vec<ChatRequest>>>,
    forced_call: Arc<Mutex<Vec<String>>>,
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
    let forced = forced_call.lock().expect("forced call").clone();
    let (recorded, body) = scripted_chat_reply(&request, &forced);
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
    timeout(Duration::from_mins(3), command.output())
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
    install_sentinel_with(roots, rkat, realm, generation, &[], &[]).await
}

/// [`install_sentinel`] with extra `rkat mcp add` flags and extra sentinel
/// arguments.
async fn install_sentinel_with(
    roots: &Roots,
    rkat: &Path,
    realm: &str,
    generation: u64,
    add_flags: &[&str],
    sentinel_args: &[&str],
) -> std::process::Output {
    let mut command = roots.rkat(rkat, realm);
    command
        .args(["mcp", "add", SENTINEL_SERVER, "--scope", "realm"])
        .args(["--expected-generation", &generation.to_string()])
        .args(add_flags)
        .arg("--")
        .arg("/bin/sh")
        .arg(&roots.sentinel)
        .arg(&roots.markers)
        .args(sentinel_args);
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

/// A connection installed with an access ceiling in a realm's own config is
/// enforced for every fresh ordinary `rkat` in that realm, through the
/// persisted document alone: each run is a new process (as after a restart),
/// with no helper left running. The server lists a read and a write; the
/// ceiling allows only the read, so each run is offered only the read, the
/// write never reaches the server, and another realm stays without the
/// connection.
#[tokio::test]
#[ignore = "lane:e2e-system"]
async fn integration_real_cli_realm_mcp_access_ceiling() {
    const WRITE_TOOL: &str = "sentinel_write";
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
    let generation = realm_generation(&roots, &rkat, "alpha").await;
    let installed = install_sentinel_with(
        &roots,
        &rkat,
        "alpha",
        generation,
        &["--allow-operation", SENTINEL_TOOL],
        &["with-write"],
    )
    .await;
    expect_success("install the sentinel with an access ceiling", &installed);
    let doc = tokio::fs::read_to_string(roots.realm_doc_path("alpha"))
        .await
        .expect("read alpha doc");
    assert!(doc.contains("allowed_operations"), "{doc}");

    for attempt in ["first run", "run after restart"] {
        let _ = tokio::fs::remove_file(roots.markers.join("called")).await;
        let alpha = run_turn(&roots, &rkat, "alpha").await;
        expect_success(&format!("rkat run in alpha ({attempt})"), &alpha);
        let requests = chat.take_requests();
        let first = requests
            .first()
            .expect("alpha's run reached the chat server");
        assert!(
            first.offered_tools.iter().any(|tool| tool == SENTINEL_TOOL),
            "{attempt}: the allowed operation is offered: {requests:?}\nstderr:\n{}",
            String::from_utf8_lossy(&alpha.stderr)
        );
        assert!(
            requests
                .iter()
                .all(|request| !request.offered_tools.iter().any(|tool| tool == WRITE_TOOL)),
            "{attempt}: the operation outside the ceiling is never offered: {requests:?}"
        );
        assert!(
            requests
                .iter()
                .any(|request| request.tool_result.as_deref() == Some(SENTINEL_MARKER)),
            "{attempt}: the allowed operation answered: {requests:?}"
        );
        assert!(
            roots.marker("called"),
            "{attempt}: the read reached the server"
        );
        assert!(
            !roots.marker("called-write"),
            "{attempt}: the operation outside the ceiling never reaches the server"
        );
    }

    let beta = run_turn(&roots, &rkat, "beta").await;
    expect_success("rkat run in beta", &beta);
    let requests = chat.take_requests();
    assert!(
        requests.iter().all(|request| request
            .offered_tools
            .iter()
            .all(|tool| tool != SENTINEL_TOOL && tool != WRITE_TOOL)),
        "beta must not be offered the connection: {requests:?}"
    );
}

const WRITE_TOOL: &str = "sentinel_write";

/// `rkat run --resume <session>` in `realm`: the original session, reopened
/// by a new process.
async fn resume_turn(
    roots: &Roots,
    rkat: &Path,
    realm: &str,
    session: &str,
) -> std::process::Output {
    let mut command = roots.rkat(rkat, realm);
    command.args([
        "run",
        "--resume",
        session,
        "--wait-for-mcp",
        "--output",
        "json",
        "Ask the sentinel again, then answer.",
    ]);
    output_of(&mut command, &format!("rkat run --resume in {realm}")).await
}

fn session_id_of(step: &str, output: &std::process::Output) -> String {
    let result: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("{step}: run output is not JSON: {error}"));
    result["session_id"]
        .as_str()
        .unwrap_or_else(|| panic!("{step}: run output carries no session_id: {result}"))
        .to_owned()
}

async fn clear_markers(roots: &Roots) {
    for name in ["spawned", "called", "called-write"] {
        let _ = tokio::fs::remove_file(roots.markers.join(name)).await;
    }
}

fn offers(request: &ChatRequest, tool: &str) -> bool {
    request.offered_tools.iter().any(|offered| offered == tool)
}

/// The requests of one run offered the read and never the write.
fn assert_offered_read_only(step: &str, requests: &[ChatRequest], output: &std::process::Output) {
    let first = requests
        .first()
        .unwrap_or_else(|| panic!("{step}: the run reached the chat server"));
    assert!(
        offers(first, SENTINEL_TOOL),
        "{step}: the allowed operation is offered: {requests:?}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        requests.iter().all(|request| !offers(request, WRITE_TOOL)),
        "{step}: the operation outside the ceiling is never offered: {requests:?}"
    );
}

/// The requests of one run offered nothing of the sentinel.
fn assert_offered_nothing(step: &str, requests: &[ChatRequest]) {
    assert!(
        requests
            .iter()
            .all(|request| !offers(request, SENTINEL_TOOL) && !offers(request, WRITE_TOOL)),
        "{step}: no sentinel operation may be offered: {requests:?}"
    );
}

/// Install the sentinel (listing a read and a write) into `alpha` with a
/// ceiling that allows only the read, and check that `beta`'s document, its
/// generation and its realm listing are exactly as before.
async fn install_read_only_in_alpha(roots: &Roots, rkat: &Path) {
    let beta_doc = tokio::fs::read_to_string(roots.realm_doc_path("beta"))
        .await
        .expect("read beta doc");
    let beta_generation = realm_generation(roots, rkat, "beta").await;
    let generation = realm_generation(roots, rkat, "alpha").await;
    let installed = install_sentinel_with(
        roots,
        rkat,
        "alpha",
        generation,
        &["--allow-operation", SENTINEL_TOOL],
        &["with-write"],
    )
    .await;
    expect_success("install the sentinel with a read-only ceiling", &installed);
    assert_eq!(
        tokio::fs::read_to_string(roots.realm_doc_path("beta"))
            .await
            .expect("read beta doc"),
        beta_doc,
        "installing in alpha leaves beta's document byte for byte"
    );
    assert_eq!(
        realm_generation(roots, rkat, "beta").await,
        beta_generation,
        "installing in alpha leaves beta's generation"
    );
    assert!(realm_scope_listing(roots, rkat, "beta").await.is_empty());
}

/// A forced call to `WRITE_TOOL` never reaches the server. The model gets
/// the ordinary typed tool-not-found as feedback for that call: the run
/// either succeeds or fails naming exactly that refusal, and never shows the
/// sentinel's answer.
fn assert_write_refused(step: &str, roots: &Roots, output: &std::process::Output) {
    assert!(
        !roots.marker("called") && !roots.marker("called-write"),
        "{step}: the refused write never reaches the server"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() || stderr.contains("Tool not found: sentinel_write"),
        "{step}: the only failure allowed is the typed refusal of the write\nstderr:\n{stderr}"
    );
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains(SENTINEL_MARKER),
        "{step}: no sentinel answer reaches the output"
    );
}

/// The ceiling is enforced at the actual invocation, not only by what is
/// offered: a model that names the write anyway (in a fresh process, again
/// after a restart, and in the original session reopened by a new process)
/// never reaches the server. The original session still uses the read
/// afterwards. Another realm is unchanged by the installation and never
/// starts the server.
#[tokio::test]
#[ignore = "lane:e2e-system"]
async fn integration_real_cli_realm_mcp_ceiling_refuses_invocation() {
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
    install_read_only_in_alpha(&roots, &rkat).await;

    // The original session: one ordinary read first.
    clear_markers(&roots).await;
    let first = run_turn(&roots, &rkat, "alpha").await;
    expect_success("read in alpha", &first);
    assert_offered_read_only("read in alpha", &chat.take_requests(), &first);
    assert!(roots.marker("called") && !roots.marker("called-write"));
    let session = session_id_of("read in alpha", &first);

    chat.force_call(Some(WRITE_TOOL));
    for attempt in ["first process", "process after restart"] {
        clear_markers(&roots).await;
        let alpha = run_turn(&roots, &rkat, "alpha").await;
        let requests = chat.take_requests();
        assert_offered_read_only(attempt, &requests, &alpha);
        assert!(
            roots.marker("spawned"),
            "{attempt}: the connection itself was made"
        );
        assert_write_refused(attempt, &roots, &alpha);
    }
    clear_markers(&roots).await;
    let resumed = resume_turn(&roots, &rkat, "alpha", &session).await;
    assert_offered_read_only("resumed session", &chat.take_requests(), &resumed);
    assert_write_refused("resumed session", &roots, &resumed);

    // The original session still reads, in a new process.
    chat.force_call(None);
    clear_markers(&roots).await;
    let read = resume_turn(&roots, &rkat, "alpha", &session).await;
    expect_success("read in the resumed session", &read);
    assert_eq!(session_id_of("read after a refusal", &read), session);
    let requests = chat.take_requests();
    assert_offered_read_only("read after a refusal", &requests, &read);
    assert!(
        requests
            .iter()
            .any(|request| request.tool_result.as_deref() == Some(SENTINEL_MARKER)),
        "the read answered after the refusal: {requests:?}"
    );
    assert!(roots.marker("called") && !roots.marker("called-write"));

    // Beta: even a model naming the write gets nothing, and nothing starts.
    chat.force_call(Some(WRITE_TOOL));
    clear_markers(&roots).await;
    let beta = run_turn(&roots, &rkat, "beta").await;
    assert_offered_nothing("beta", &chat.take_requests());
    assert!(
        !roots.marker("spawned") && !roots.marker("called-write"),
        "the sentinel never starts in beta"
    );
    let stderr = String::from_utf8_lossy(&beta.stderr);
    assert!(
        beta.status.success() || stderr.contains("Tool not found: sentinel_write"),
        "beta: the only failure allowed is the typed refusal\nstderr:\n{stderr}"
    );
}

/// One run in which the model named the write outside the ceiling and the
/// offered read in the same turn: the write is refused as feedback for that
/// call only, the read sibling still reaches the server, and the next model
/// turn receives both results in call order and gives the final answer.
fn assert_call_local_refusal(step: &str, roots: &Roots, output: &std::process::Output) {
    expect_success(step, output);
    assert!(
        roots.marker("called"),
        "{step}: the permitted sibling call reached the server"
    );
    assert!(
        !roots.marker("called-write"),
        "{step}: the refused write never reaches the server"
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("The sentinel answered."),
        "{step}: the run continued to the final model answer\nstdout:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

fn assert_follow_up_results(step: &str, requests: &[ChatRequest]) {
    assert_eq!(
        requests.len(),
        2,
        "{step}: one tool-calling model turn, then one more model turn: {requests:?}"
    );
    assert!(
        requests.iter().all(|request| !offers(request, WRITE_TOOL)),
        "{step}: the operation outside the ceiling is never offered: {requests:?}"
    );
    let results = &requests[1].tool_results;
    assert_eq!(
        results.len(),
        2,
        "{step}: the next model turn carries both results: {requests:?}"
    );
    assert!(
        results[0].contains("tool_not_found") && results[0].contains(WRITE_TOOL),
        "{step}: the refused call returns typed not-found feedback: {results:?}"
    );
    assert!(
        !results[0].contains("access_denied"),
        "{step}: a missing tool is not an authorization denial: {results:?}"
    );
    assert_eq!(
        results[1], SENTINEL_MARKER,
        "{step}: the permitted sibling answered: {results:?}"
    );
}

/// A refused operation is feedback for that call only: the model receives a
/// typed refusal as the call's result, the permitted sibling in the same
/// turn still runs, and the turn continues to its answer, in a fresh process
/// and in the original session reopened by a new one.
#[tokio::test]
#[ignore = "lane:e2e-system"]
async fn integration_real_cli_realm_mcp_refused_write_is_call_local_feedback() {
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
    install_read_only_in_alpha(&roots, &rkat).await;

    chat.force_calls(&[WRITE_TOOL, SENTINEL_TOOL]);
    clear_markers(&roots).await;
    let fresh = run_turn(&roots, &rkat, "alpha").await;
    assert_call_local_refusal("fresh process", &roots, &fresh);
    assert_follow_up_results("fresh process", &chat.take_requests());
    let session = session_id_of("fresh process", &fresh);

    clear_markers(&roots).await;
    let resumed = resume_turn(&roots, &rkat, "alpha", &session).await;
    assert_call_local_refusal("resumed session", &roots, &resumed);
    assert_eq!(session_id_of("resumed session", &resumed), session);
    assert_follow_up_results("resumed session", &chat.take_requests());
}

/// A session created before the installation is the one that gains the
/// connection: reopened by a new process (and again after another restart)
/// through the realm's persisted configuration, the same session id is
/// offered the read and not the write and calls it. The session cannot be
/// reopened from another realm, and after the connection is removed from the
/// realm (revocation) the original session is offered nothing and the
/// server does not start.
#[tokio::test]
#[ignore = "lane:e2e-system"]
async fn integration_real_cli_realm_mcp_original_session_across_restart() {
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

    let before = run_turn(&roots, &rkat, "alpha").await;
    expect_success("rkat run in alpha before the installation", &before);
    assert_offered_nothing("before the installation", &chat.take_requests());
    assert!(!roots.marker("spawned"));
    let session = session_id_of("before the installation", &before);

    install_read_only_in_alpha(&roots, &rkat).await;

    for attempt in ["first reopen", "reopen after restart"] {
        clear_markers(&roots).await;
        let resumed = resume_turn(&roots, &rkat, "alpha", &session).await;
        expect_success(
            &format!("resume the original session ({attempt})"),
            &resumed,
        );
        assert_eq!(
            session_id_of(attempt, &resumed),
            session,
            "{attempt}: the original session, not a new one"
        );
        let requests = chat.take_requests();
        assert_offered_read_only(attempt, &requests, &resumed);
        assert!(
            requests
                .iter()
                .any(|request| request.tool_result.as_deref() == Some(SENTINEL_MARKER)),
            "{attempt}: the read answered in the original session: {requests:?}"
        );
        assert!(roots.marker("called") && !roots.marker("called-write"));
    }

    // Another realm cannot reopen alpha's session or reach its connection.
    clear_markers(&roots).await;
    let foreign = resume_turn(&roots, &rkat, "beta", &session).await;
    assert!(
        !foreign.status.success(),
        "beta must not reopen alpha's session\nstdout:\n{}",
        String::from_utf8_lossy(&foreign.stdout)
    );
    assert_offered_nothing("resume from beta", &chat.take_requests());
    assert!(
        !roots.marker("spawned"),
        "the sentinel never starts for beta"
    );

    // Revocation: remove the connection from alpha under the generation
    // check; the original session is offered nothing and nothing starts.
    let generation = realm_generation(&roots, &rkat, "alpha").await;
    let mut command = roots.rkat(&rkat, "alpha");
    command
        .args(["mcp", "remove", SENTINEL_SERVER, "--scope", "realm"])
        .args(["--expected-generation", &generation.to_string()]);
    let removed = output_of(&mut command, "rkat mcp remove in alpha").await;
    expect_success("rkat mcp remove in alpha", &removed);
    assert!(realm_scope_listing(&roots, &rkat, "alpha").await.is_empty());
    clear_markers(&roots).await;
    let revoked = resume_turn(&roots, &rkat, "alpha", &session).await;
    expect_success("resume the original session after revocation", &revoked);
    assert_eq!(session_id_of("after revocation", &revoked), session);
    assert_offered_nothing("after revocation", &chat.take_requests());
    assert!(
        !roots.marker("spawned"),
        "a revoked connection never starts for the original session"
    );
}

/// The agent the operator does not grant the installation to.
const UNGRANTED_AGENT: &str = "ungranted-agent";

/// Arguments that make an ordinary `rkat run` in the governed realm act as
/// an agent the operator did not grant the installation.
const UNGRANTED_AGENT_ARGS: &[&str] = &["--as-agent", UNGRANTED_AGENT];

/// `rkat realm grant <realm> --agent <agent> <target...>` as the local
/// operator.
async fn realm_grant(roots: &Roots, rkat: &Path, realm: &str, agent: &str, target: &[&str]) {
    let mut command = roots.rkat(rkat, realm);
    command
        .args(["realm", "grant", realm, "--agent", agent])
        .args(target);
    let step = format!(
        "rkat realm grant {realm} --agent {agent} {}",
        target.join(" ")
    );
    let output = output_of(&mut command, &step).await;
    expect_success(&step, &output);
}

/// The key of the realm's actual controller route, as an operator learns it:
/// a prompt under a controller grant naming another route is refused, and
/// the refusal names the command that grants the actual one.
async fn discover_controller_route(roots: &Roots, rkat: &Path, realm: &str) -> String {
    let placeholder = format!("model-route/v1/{}", "0".repeat(64));
    realm_grant(
        roots,
        rkat,
        realm,
        "rkat-agent",
        &["--controller", "--route", &placeholder],
    )
    .await;
    let refused = run_turn(roots, rkat, realm).await;
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        !refused.status.success(),
        "a prompt under a controller grant for another route is refused\nstderr:\n{stderr}"
    );
    let marker = "--controller --route ";
    let start = stderr
        .find(marker)
        .unwrap_or_else(|| panic!("the refusal names the controller route\nstderr:\n{stderr}"))
        + marker.len();
    let key: String = stderr[start..]
        .chars()
        .take_while(|c| !c.is_whitespace())
        .collect();
    assert!(
        key.starts_with("model-route/v1/") && key != placeholder,
        "the refusal names the actual route: {key}"
    );
    key
}

/// Installing a connection into a realm does not grant every agent in it:
/// in the governed realm a session of the agent granted the installation is
/// offered the read, a fresh ordinary session of an agent granted its
/// controller and catalog but not the installation is offered nothing and
/// never starts the server, before and after a restart.
#[tokio::test]
#[ignore = "lane:e2e-system"]
async fn integration_real_cli_realm_mcp_installation_does_not_grant_every_agent() {
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
    install_read_only_in_alpha(&roots, &rkat).await;

    // Per-agent decisions belong to a governed realm: materialize and govern
    // alpha, and grant each agent as its local operator.
    let mut command = roots.rkat(&rkat, "alpha");
    command.args(["realm", "create", "alpha"]);
    let created = output_of(&mut command, "rkat realm create alpha").await;
    expect_success("rkat realm create alpha", &created);
    let mut command = roots.rkat(&rkat, "alpha");
    command.args(["realm", "govern", "alpha"]);
    let governed = output_of(&mut command, "rkat realm govern alpha").await;
    expect_success("rkat realm govern alpha", &governed);
    let sentinel_grant = format!("{SENTINEL_SERVER}={SENTINEL_TOOL}");
    realm_grant(
        &roots,
        &rkat,
        "alpha",
        "rkat-agent",
        &["--mcp", &sentinel_grant],
    )
    .await;
    realm_grant(&roots, &rkat, "alpha", "rkat-agent", &["--catalog"]).await;
    let route = discover_controller_route(&roots, &rkat, "alpha").await;
    chat.take_requests();
    for agent in ["rkat-agent", UNGRANTED_AGENT] {
        realm_grant(
            &roots,
            &rkat,
            "alpha",
            agent,
            &["--controller", "--route", &route],
        )
        .await;
    }
    realm_grant(&roots, &rkat, "alpha", UNGRANTED_AGENT, &["--catalog"]).await;

    for attempt in ["first process", "process after restart"] {
        clear_markers(&roots).await;
        let granted = run_turn(&roots, &rkat, "alpha").await;
        expect_success(&format!("granted run ({attempt})"), &granted);
        assert_offered_read_only(attempt, &chat.take_requests(), &granted);

        clear_markers(&roots).await;
        let mut command = roots.rkat(&rkat, "alpha");
        command
            .args(["run", "--wait-for-mcp", "--output", "json"])
            .args(["-m", MODEL, "--provider", "self-hosted"])
            .args(UNGRANTED_AGENT_ARGS)
            .arg("Ask the sentinel, then answer.");
        let ungranted = output_of(&mut command, "ungranted run").await;
        expect_success(&format!("ungranted run ({attempt})"), &ungranted);
        let requests = chat.take_requests();
        assert!(
            !requests.is_empty(),
            "{attempt}: the ungranted run reached the chat server"
        );
        assert_offered_nothing(attempt, &requests);
        assert!(
            !roots.marker("spawned") && !roots.marker("called"),
            "{attempt}: the connection never starts for an ungranted agent"
        );
    }
}

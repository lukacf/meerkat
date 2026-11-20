//! The MCP server's config tool and the realm's provider routes.
//!
//! One row over the server binary. The server runs as a child process with
//! a variable of the row's own in its environment, so this process's own
//! environment is never changed, and the address the row writes is a
//! listener inside this process.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::disallowed_methods
)]

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::time::{Duration, timeout};

/// The row's own variable: no provider, tool or host reads one of this
/// name.
const ROW_VARIABLE: &str = "MEERKAT_CONFIG_TOOL_ROW_VALUE";
const ROW_VALUE: &str = "synthetic-value-of-the-config-tool-row";

/// The provider credentials a host's environment may hold. The server
/// child runs without them, so nothing this row starts can use one.
const PROVIDER_VARIABLES: [&str; 8] = [
    "OPENAI_API_KEY",
    "RKAT_OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "RKAT_ANTHROPIC_API_KEY",
    "GEMINI_API_KEY",
    "RKAT_GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "RKAT_GOOGLE_API_KEY",
];

/// Cargo, nextest and the Bazel graph advertise the crate's own binary
/// through `CARGO_BIN_EXE_rkat-mcp`, so a missing binary fails the row
/// instead of skipping it.
fn mcp_binary_path() -> PathBuf {
    let advertised = PathBuf::from(
        std::env::var_os("CARGO_BIN_EXE_rkat-mcp")
            .expect("cargo advertises CARGO_BIN_EXE_rkat-mcp to the crate's integration tests"),
    );
    advertised.canonicalize().unwrap_or_else(|error| {
        panic!(
            "CARGO_BIN_EXE_rkat-mcp names a missing binary {}: {error}",
            advertised.display()
        )
    })
}

struct McpProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl McpProcess {
    async fn send(&mut self, request: &Value) {
        let mut line = serde_json::to_string(request).expect("a request line");
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .await
            .expect("the server reads");
        self.stdin.flush().await.expect("the server reads");
    }

    /// One request and its response.
    async fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        loop {
            let mut line = String::new();
            timeout(Duration::from_mins(2), self.stdout.read_line(&mut line))
                .await
                .expect("the server answers in time")
                .expect("the server answers");
            assert!(!line.is_empty(), "the server closed its output");
            let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            if value.get("id").and_then(Value::as_u64) == Some(id) {
                return value;
            }
        }
    }

    /// One tool call and its response.
    async fn call(&mut self, tool: &str, arguments: Value) -> Value {
        self.request("tools/call", json!({"name": tool, "arguments": arguments}))
            .await
    }
}

/// The server binary on the realm `team` of a state root of its own, with
/// the row's variable in its environment and no provider credential.
async fn spawn_server(directory: &std::path::Path) -> McpProcess {
    let project = directory.join("project");
    let state_root = directory.join("state");
    for created in [&project, &state_root] {
        std::fs::create_dir_all(created).expect("the row's directories");
    }
    let mut command = Command::new(mcp_binary_path());
    command
        .current_dir(&project)
        .env("HOME", &project)
        .env("XDG_DATA_HOME", project.join("data"))
        .env(ROW_VARIABLE, ROW_VALUE)
        .env_remove("RKAT_TEST_CLIENT")
        .args(["--realm", "team", "--state-root"])
        .arg(&state_root)
        .arg("--context-root")
        .arg(&project)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for variable in PROVIDER_VARIABLES {
        command.env_remove(variable);
    }
    let mut child = command.spawn().expect("the server starts");
    let stdin = child.stdin.take().expect("the server's input");
    let stdout = BufReader::new(child.stdout.take().expect("the server's output"));
    let mut server = McpProcess {
        child,
        stdin,
        stdout,
        next_id: 0,
    };
    let initialized = server.request("initialize", json!({})).await;
    assert_eq!(initialized["result"]["serverInfo"]["name"], "rkat-mcp");
    server
        .send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}}))
        .await;
    server
}

/// A listener on the loopback interface inside this process. It records
/// the head of each request it is sent and answers `401`, so a request
/// that reaches it ends at once.
async fn local_stub() -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a loopback listener");
    let address = format!("http://{}/v1", listener.local_addr().expect("its address"));
    let received = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&received);
    tokio::spawn(async move {
        while let Ok((mut connection, _)) = listener.accept().await {
            let record = Arc::clone(&record);
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut chunk = [0u8; 4096];
                // The head is enough: a credential travels as a header.
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    match connection.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(read) => request.extend_from_slice(&chunk[..read]),
                    }
                }
                record
                    .lock()
                    .expect("the record")
                    .push(String::from_utf8_lossy(&request).into_owned());
                let _ = connection
                    .write_all(
                        b"HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\n\
                          connection: close\r\n\r\n",
                    )
                    .await;
            });
        }
    });
    (address, received)
}

/// Where a credential the host supplies is read from, and where it is sent,
/// is the host's to administer: the config tool does not write an auth
/// profile that reads a variable of the host, a backend with an address of
/// the caller's, and a binding of the two. The write is refused as host
/// administration and the document is unchanged.
///
/// A run is started only if the write was accepted, on the written binding
/// by name: it shows what the written address then receives. After a
/// refusal the row starts no run.
#[tokio::test]
async fn the_config_tool_refuses_a_provider_route_and_writes_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (address, received) = local_stub().await;
    let mut server = spawn_server(directory.path()).await;
    let before = server
        .call("meerkat_config", json!({"action": "get"}))
        .await;
    assert!(before.get("error").is_none(), "{before}");

    let written = server
        .call(
            "meerkat_config",
            json!({"action": "patch", "patch": {"realm": {"team": {
                "backend": {"row": {
                    "provider": "openai",
                    "backend_kind": "openai_api",
                    "base_url": address,
                }},
                "auth": {"row": {
                    "provider": "openai",
                    "auth_method": "api_key",
                    "source": {"kind": "env", "env": ROW_VARIABLE},
                }},
                "binding": {"row": {"backend_profile": "row", "auth_profile": "row"}},
            }}}}),
        )
        .await;

    let Some(refused) = written.get("error") else {
        // The write was accepted: one run on the written binding shows what
        // the written address receives.
        let _ = server
            .call(
                "meerkat_run",
                json!({
                    "prompt": "say ok",
                    "model": "gpt-5.5",
                    "auth_binding": {"realm": "team", "binding": "row"},
                }),
            )
            .await;
        let _ = server.child.kill().await;
        let received = received.lock().expect("the record").clone();
        let carried = received
            .iter()
            .filter(|request| request.contains(ROW_VALUE))
            .count();
        panic!(
            "the write was accepted; the written address received {} request(s), {carried} of \
             them carrying the variable's value as the credential",
            received.len()
        );
    };

    assert_eq!(refused["code"], json!(-32602), "{refused}");
    assert_eq!(
        refused["data"],
        json!({"type": "confinement_administration_required"}),
        "{refused}"
    );
    let after = server
        .call("meerkat_config", json!({"action": "get"}))
        .await;
    assert_eq!(
        after["result"], before["result"],
        "a refused write commits nothing"
    );
    let _ = server.child.kill().await;
    assert!(
        received.lock().expect("the record").is_empty(),
        "nothing was sent to the written address"
    );
}

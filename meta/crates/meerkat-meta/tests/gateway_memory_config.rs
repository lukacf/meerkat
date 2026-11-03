#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::uninlined_format_args,
    clippy::manual_assert
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use base64::Engine;
use hmac::{Hmac, Mac};
use meerkat_meta::storage_layout::MEMORY_LEDGER_FILE_NAME;
use meerkat_meta::storage_predecessor::PREDECESSOR_MEMORY_LEDGER_FILE_NAME;
use serde_json::{Value, json};
use sha2::Sha256;
use tempfile::TempDir;

const MOB_CONFIG: &str = r#"
[mob]
id = "gateway-memory-config-test"

[profiles.default]
model = "gpt-5.5"
external_addressable = true
"#;

struct HealthEndpointServer {
    endpoint: String,
    paths: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl HealthEndpointServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind health endpoint");
        listener
            .set_nonblocking(true)
            .expect("set listener nonblocking");
        let endpoint = format!(
            "http://{}",
            listener.local_addr().expect("listener address")
        );
        let paths = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_paths = Arc::clone(&paths);
        let thread_stop = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _addr)) => {
                        let mut reader =
                            BufReader::new(stream.try_clone().expect("clone health stream"));
                        let mut request_line = String::new();
                        if reader.read_line(&mut request_line).is_ok()
                            && let Some(path) = request_line.split_whitespace().nth(1)
                        {
                            thread_paths
                                .lock()
                                .expect("paths lock")
                                .push(path.to_string());
                        }
                        let response =
                            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK";
                        let _ = stream.write_all(response);
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            endpoint,
            paths,
            stop,
            thread: Some(thread),
        }
    }

    fn endpoint(&self) -> &str {
        &self.endpoint
    }

    fn paths(&self) -> Vec<String> {
        self.paths.lock().expect("paths lock").clone()
    }
}

impl Drop for HealthEndpointServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = std::net::TcpStream::connect(self.endpoint.trim_start_matches("http://"));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct GatewayProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
}

impl GatewayProcess {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_rpc_gateway"))
            .arg("--persistent")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn rpc_gateway");
        let stdin = child.stdin.take().expect("gateway stdin");
        let stdout = BufReader::new(child.stdout.take().expect("gateway stdout"));
        Self {
            child,
            stdin,
            stdout,
        }
    }

    fn rpc(&mut self, request: Value) -> Value {
        writeln!(
            self.stdin,
            "{}",
            serde_json::to_string(&request).expect("request json")
        )
        .expect("write request");
        self.stdin.flush().expect("flush request");

        let mut line = String::new();
        self.stdout.read_line(&mut line).expect("read response");
        assert!(!line.is_empty(), "gateway closed stdout before response");
        serde_json::from_str(line.trim()).expect("response json")
    }

    fn init_with_memory(&mut self, state_dir: &TempDir, endpoint: &str) -> Value {
        self.rpc(json!({
            "jsonrpc": "2.0",
            "id": "init",
            "method": "meta/init",
            "params": {
                "persistent_state": state_dir.path(),
                "mob_config": MOB_CONFIG,
                "runtime_options": {
                    "memory_config": {
                        "backend": "local_json",
                        "health_check_endpoint": endpoint
                    }
                }
            }
        }))
    }

    fn init_with_runtime_options(&mut self, state_dir: &TempDir, runtime_options: Value) -> Value {
        self.rpc(json!({
            "jsonrpc": "2.0",
            "id": "init",
            "method": "meta/init",
            "params": {
                "persistent_state": state_dir.path(),
                "mob_config": MOB_CONFIG,
                "runtime_options": runtime_options
            }
        }))
    }
}

impl Drop for GatewayProcess {
    fn drop(&mut self) {
        let _ = self.stdin.flush();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn gateway_runtime_options_memory_config_persists_memory_across_restart() {
    let state_dir = tempfile::tempdir().expect("state dir");
    let endpoint = HealthEndpointServer::start();

    {
        let mut gateway = GatewayProcess::start();
        let init = gateway.init_with_memory(&state_dir, endpoint.endpoint());
        assert!(init["result"]["contract_version"].is_string());

        let indexed = gateway.rpc(json!({
            "jsonrpc": "2.0",
            "id": "index",
            "method": "meta/memory/index",
            "params": {
                "entity": "delivery",
                "topic": "email_send",
                "store": "todo",
                "fact": "double-check recipient consent"
            }
        }));
        assert_eq!(indexed["result"]["store"], json!("todo"));
    }

    assert!(
        state_dir.path().join(MEMORY_LEDGER_FILE_NAME).exists(),
        "gateway should store the memory ledger under persistent_state"
    );
    assert!(
        !state_dir
            .path()
            .join(PREDECESSOR_MEMORY_LEDGER_FILE_NAME)
            .exists(),
        "the gateway never writes the predecessor ledger name"
    );

    {
        let mut gateway = GatewayProcess::start();
        let init = gateway.init_with_memory(&state_dir, endpoint.endpoint());
        assert!(init["result"]["contract_version"].is_string());

        let queried = gateway.rpc(json!({
            "jsonrpc": "2.0",
            "id": "query",
            "method": "meta/memory/query",
            "params": {
                "entity": "delivery",
                "topic": "email_send",
                "store": "todo"
            }
        }));
        assert_eq!(
            queried["result"]["assertions"][0]["fact"],
            json!("double-check recipient consent")
        );
    }

    assert!(
        endpoint.paths().iter().any(|path| path == "/v1/health"),
        "local_json health_check_endpoint should be health-checked"
    );
}

#[test]
fn gateway_local_json_memory_config_persists_without_health_endpoint() {
    let state_dir = tempfile::tempdir().expect("state dir");

    {
        let mut gateway = GatewayProcess::start();
        let init = gateway.init_with_runtime_options(
            &state_dir,
            json!({
                "memory_config": {
                    "backend": "local_json"
                }
            }),
        );
        assert!(init["result"]["contract_version"].is_string());

        let indexed = gateway.rpc(json!({
            "jsonrpc": "2.0",
            "id": "index",
            "method": "meta/memory/index",
            "params": {
                "entity": "delivery",
                "topic": "email_send",
                "store": "todo",
                "fact": "double-check recipient consent"
            }
        }));
        assert_eq!(indexed["result"]["store"], json!("todo"));
    }

    assert!(
        state_dir.path().join(MEMORY_LEDGER_FILE_NAME).exists(),
        "local_json backend should store the ledger under persistent_state"
    );

    {
        let mut gateway = GatewayProcess::start();
        let init = gateway.init_with_runtime_options(
            &state_dir,
            json!({
                "memory_config": {
                    "backend": "local_json"
                }
            }),
        );
        assert!(init["result"]["contract_version"].is_string());

        let queried = gateway.rpc(json!({
            "jsonrpc": "2.0",
            "id": "query",
            "method": "meta/memory/query",
            "params": { "query": "recipient consent" }
        }));
        assert_eq!(
            queried["result"]["assertions"][0]["fact"],
            json!("double-check recipient consent")
        );
    }
}

/// A predecessor ledger under persistent_state refuses init, with memory
/// configured or not, instead of a ledger starting empty beside it. The
/// refusal names the exact adoption command; that command carries the file
/// over byte for byte, and the gateway then serves the adopted ledger.
#[test]
fn gateway_refuses_an_unadopted_predecessor_ledger_until_it_is_adopted() {
    let state_dir = tempfile::tempdir().expect("state dir");
    let predecessor = state_dir.path().join(PREDECESSOR_MEMORY_LEDGER_FILE_NAME);
    let canonical = state_dir.path().join(MEMORY_LEDGER_FILE_NAME);
    let ledger = serde_json::to_vec_pretty(&json!({
        "assertions": [{
            "assertion_id": "memory-assert-000001",
            "entity": "delivery",
            "topic": "email_send",
            "store": "todo",
            "fact": "written before the rename",
            "metadata": null,
            "indexed_at_ms": 1
        }],
        "conflicts": []
    }))
    .expect("ledger json");
    std::fs::write(&predecessor, &ledger).expect("write the predecessor ledger");

    for runtime_options in [
        json!({ "memory_config": { "backend": "local_json" } }),
        json!({}),
    ] {
        let mut gateway = GatewayProcess::start();
        let init = gateway.init_with_runtime_options(&state_dir, runtime_options);
        assert_eq!(
            init["error"]["code"],
            json!(meerkat_meta::STORAGE_RESOLUTION_CODE),
            "{init}"
        );
        let message = init["error"]["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(&predecessor.display().to_string())
                && message.contains(&format!(
                    "storage-adopt-predecessor --state-dir {} --apply",
                    state_dir.path().display()
                )),
            "init must refuse with the exact adoption command: {init}"
        );
        assert_eq!(
            std::fs::read(&predecessor).expect("predecessor ledger"),
            ledger
        );
        assert!(
            !canonical.exists(),
            "a refused init never starts an empty ledger"
        );
    }

    let adopted = Command::new(env!("CARGO_BIN_EXE_meta_gateway"))
        .args(["storage-adopt-predecessor", "--state-dir"])
        .arg(state_dir.path())
        .arg("--gateway-home")
        .arg(state_dir.path().join("gateway-home"))
        .args(["--apply", "--json"])
        .output()
        .expect("run the adoption verb");
    assert!(
        adopted.status.success(),
        "adoption failed: {}\n{}",
        String::from_utf8_lossy(&adopted.stdout),
        String::from_utf8_lossy(&adopted.stderr)
    );
    assert_eq!(std::fs::read(&canonical).expect("adopted ledger"), ledger);
    assert!(!predecessor.exists());

    let mut gateway = GatewayProcess::start();
    let init = gateway.init_with_runtime_options(
        &state_dir,
        json!({ "memory_config": { "backend": "local_json" } }),
    );
    assert!(init["result"]["contract_version"].is_string(), "{init}");
    let queried = gateway.rpc(json!({
        "jsonrpc": "2.0",
        "id": "query",
        "method": "meta/memory/query",
        "params": {
            "entity": "delivery",
            "topic": "email_send",
            "store": "todo"
        }
    }));
    assert_eq!(
        queried["result"]["assertions"][0]["fact"],
        json!("written before the rename"),
        "{queried}"
    );
}

#[test]
fn gateway_rejects_unsupported_memory_config_fields() {
    let state_dir = tempfile::tempdir().expect("state dir");
    let mut gateway = GatewayProcess::start();

    let init = gateway.rpc(json!({
        "jsonrpc": "2.0",
        "id": "init",
        "method": "meta/init",
        "params": {
            "persistent_state": state_dir.path(),
            "mob_config": MOB_CONFIG,
            "runtime_options": {
                "memory_config": {
                    "backend": "local_json",
                    "stores": ["todo"]
                }
            }
        }
    }));

    assert_eq!(init["error"]["code"], json!(-32602));
    assert!(
        init["error"]["message"]
            .as_str()
            .expect("error message")
            .contains("unsupported runtime_options.memory_config fields: stores")
    );
}

/// The predecessor's backend value is refused like any other unsupported
/// value, naming the one supported backend; it is never reinterpreted, and
/// no ledger file appears under either name.
#[test]
fn gateway_refuses_the_retired_memory_backend() {
    let state_dir = tempfile::tempdir().expect("state dir");
    let mut gateway = GatewayProcess::start();
    let init = gateway.init_with_runtime_options(
        &state_dir,
        json!({
            "memory_config": {
                "backend": "elephant",
                "endpoint": "http://127.0.0.1:9"
            }
        }),
    );
    assert_eq!(init["error"]["code"], json!(-32602), "{init}");
    let message = init["error"]["message"].as_str().expect("error message");
    assert!(
        message.contains("unsupported runtime_options.memory_config.backend 'elephant'")
            && message.contains("'local_json'"),
        "{message}"
    );
    assert!(!state_dir.path().join(MEMORY_LEDGER_FILE_NAME).exists());
    assert!(
        !state_dir
            .path()
            .join(PREDECESSOR_MEMORY_LEDGER_FILE_NAME)
            .exists()
    );
}

fn write_config(dir: &Path, name: &str, contents: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, contents).expect("write config");
    path.to_string_lossy().to_string()
}

#[test]
fn gateway_runtime_options_routing_config_path_loads_routes() {
    let state_dir = tempfile::tempdir().expect("state dir");
    let routing_path = write_config(
        state_dir.path(),
        "routing.toml",
        r#"
[[routes]]
route_key = "ops.email"
recipient = "ops@example.com"
channel = "notification"
sink = "email"
target_module = "delivery"
retry_max = 2
backoff_ms = 500
rate_limit_per_minute = 10
"#,
    );
    let mut gateway = GatewayProcess::start();

    let init = gateway.init_with_runtime_options(
        &state_dir,
        json!({
            "routing_config_path": routing_path
        }),
    );
    assert!(init["result"]["contract_version"].is_string());

    let routes = gateway.rpc(json!({
        "jsonrpc": "2.0",
        "id": "routes",
        "method": "meta/routing/routes/list",
        "params": {}
    }));
    assert_eq!(
        routes["result"]["routes"][0]["route_key"],
        json!("ops.email")
    );
    assert_eq!(routes["result"]["routes"][0]["sink"], json!("email"));
}

/// Phase B rejects the removed static scheduling config at the init boundary.
#[test]
fn gateway_runtime_options_scheduling_files_are_rejected() {
    let state_dir = tempfile::tempdir().expect("state dir");
    let mut gateway = GatewayProcess::start();

    let init = gateway.init_with_runtime_options(
        &state_dir,
        json!({
            "scheduling_files": ["config/defaults/schedules.toml"]
        }),
    );
    assert!(
        init["result"].is_null(),
        "removed config must refuse init: {init}"
    );
    assert_eq!(init["error"]["code"], json!(-32602), "{init}");
    assert!(
        init["error"]["message"].as_str().is_some_and(
            |message| message.contains("unsupported runtime_options fields: scheduling_files")
        ),
        "{init}"
    );
}

/// Removed module-plane scheduling methods use the ordinary JSON-RPC
/// method-not-found contract. Durable schedule tools remain separate.
#[test]
fn gateway_static_scheduling_methods_are_not_found() {
    let state_dir = tempfile::tempdir().expect("state dir");
    let mut gateway = GatewayProcess::start();
    let init = gateway.init_with_runtime_options(&state_dir, json!({}));
    assert_eq!(init["result"]["contract_version"], json!("0.5.0"), "{init}");

    for method in ["meta/scheduling/evaluate", "meta/scheduling/dispatch"] {
        let response = gateway.rpc(json!({
            "jsonrpc": "2.0",
            "id": method,
            "method": method,
            "params": {}
        }));
        assert!(response["result"].is_null(), "{response}");
        assert_eq!(response["error"]["code"], json!(-32601), "{response}");
    }
}

/// `meta/schedule/tools` answers Meerkat's own schedule tool definitions,
/// unchanged, through the real gateway, and `meta/capabilities` lists it.
#[test]
fn gateway_schedule_tools_lists_the_durable_schedule_tool_definitions() {
    let state_dir = tempfile::tempdir().expect("state dir");
    let mut gateway = GatewayProcess::start();
    let init = gateway.init_with_runtime_options(&state_dir, json!({}));
    assert_eq!(init["result"]["contract_version"], json!("0.5.0"), "{init}");

    let listed = gateway.rpc(json!({
        "jsonrpc": "2.0",
        "id": "schedule-tools",
        "method": "meta/schedule/tools",
        "params": {}
    }));
    assert!(listed["error"].is_null(), "{listed}");
    assert_eq!(
        listed["result"],
        json!({ "tools": meerkat::schedule_tools_list() }),
        "{listed}"
    );

    let capabilities = gateway.rpc(json!({
        "jsonrpc": "2.0",
        "id": "capabilities",
        "method": "meta/capabilities",
        "params": {}
    }));
    let advertised = capabilities["result"]["methods"]
        .as_array()
        .expect("methods array")
        .iter()
        .filter(|method| method.as_str() == Some("meta/schedule/tools"))
        .count();
    assert_eq!(advertised, 1, "{capabilities}");
}

#[test]
fn gateway_runtime_options_gating_config_path_supplies_risk_tiers() {
    let state_dir = tempfile::tempdir().expect("state dir");
    let gating_path = write_config(
        state_dir.path(),
        "gating.toml",
        r#"
[actions."delivery.send"]
risk_tier = "r3"
"#,
    );
    let mut gateway = GatewayProcess::start();

    let init = gateway.init_with_runtime_options(
        &state_dir,
        json!({
            "gating_config_path": gating_path
        }),
    );
    assert!(init["result"]["contract_version"].is_string());

    let evaluated = gateway.rpc(json!({
        "jsonrpc": "2.0",
        "id": "gate",
        "method": "meta/gating/evaluate",
        "params": {
            "action": "delivery.send",
            "actor_id": "agent-lead"
        }
    }));
    assert_eq!(evaluated["result"]["risk_tier"], json!("r3"));
    assert_eq!(evaluated["result"]["outcome"], json!("pending_approval"));
}

#[test]
fn gateway_runtime_options_event_log_configures_queryable_store() {
    let state_dir = tempfile::tempdir().expect("state dir");
    let mut gateway = GatewayProcess::start();

    let init = gateway.init_with_runtime_options(
        &state_dir,
        json!({
            "event_log": {
                "storage": "memory",
                "batch_size": 1,
                "flush_interval_ms": 10
            }
        }),
    );
    assert!(init["result"]["contract_version"].is_string());

    let queried = gateway.rpc(json!({
        "jsonrpc": "2.0",
        "id": "events",
        "method": "meta/query_events",
        "params": {}
    }));
    assert_eq!(queried["result"], json!([]));
}

#[test]
fn gateway_runtime_options_auth_config_protects_reference_http_routes() {
    let state_dir = tempfile::tempdir().expect("state dir");
    let mut gateway = GatewayProcess::start();

    let init = gateway.init_with_runtime_options(
        &state_dir,
        json!({
            "auth_config": {
                "provider": "jwt",
                "shared_secret": "top-secret",
                "issuer": "http://127.0.0.1/meta-test",
                "audience": "meta-test",
                "email_allowlist": ["svc:gateway-test"]
            }
        }),
    );
    let base_url = init["result"]["http_base_url"]
        .as_str()
        .expect("http_base_url");

    let unauthenticated = http_get_status(base_url, "/console/experience", None);
    assert_eq!(unauthenticated, 401);
    let unauthenticated_editor_rpc = http_post_status(
        base_url,
        "/flow-editor/rpc",
        r#"{"jsonrpc":"2.0","id":"caps","method":"meta/capabilities","params":{}}"#,
        None,
    );
    assert_eq!(unauthenticated_editor_rpc, 401);

    let token = hs256_jwt(
        "top-secret",
        json!({
            "iss": "http://127.0.0.1/meta-test",
            "aud": "meta-test",
            "sub": "svc:gateway-test",
            "actor_type": "service",
            "exp": 4_102_444_800u64
        }),
    );
    let authenticated = http_get_status(base_url, "/console/experience", Some(&token));
    assert_eq!(authenticated, 200);
    let authenticated_editor_rpc = http_post_status(
        base_url,
        "/flow-editor/rpc",
        r#"{"jsonrpc":"2.0","id":"caps","method":"meta/capabilities","params":{}}"#,
        Some(&token),
    );
    assert_eq!(authenticated_editor_rpc, 200);
}

fn http_get_status(base_url: &str, path: &str, bearer: Option<&str>) -> u16 {
    let addr = base_url.trim_start_matches("http://");
    let mut stream = std::net::TcpStream::connect(addr).expect("connect http");
    let auth = bearer
        .map(|token| format!("Authorization: Bearer {token}\r\n"))
        .unwrap_or_default();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\n{auth}Connection: close\r\n\r\n"
    )
    .expect("write http request");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("read http response");
    response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .expect("status code")
}

fn http_post_status(base_url: &str, path: &str, body: &str, bearer: Option<&str>) -> u16 {
    let addr = base_url.trim_start_matches("http://");
    let mut stream = std::net::TcpStream::connect(addr).expect("connect http");
    let auth = bearer
        .map(|token| format!("Authorization: Bearer {token}\r\n"))
        .unwrap_or_default();
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .expect("write http request");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("read http response");
    response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .expect("status code")
}

fn hs256_jwt(secret: &str, claims: Value) -> String {
    type HmacSha256 = Hmac<Sha256>;
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let header = engine.encode(r#"{"alg":"HS256","typ":"JWT"}"#);
    let claims = engine.encode(serde_json::to_vec(&claims).expect("claims json"));
    let signing_input = format!("{header}.{claims}");
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(signing_input.as_bytes());
    let signature = engine.encode(mac.finalize().into_bytes());
    format!("{signing_input}.{signature}")
}

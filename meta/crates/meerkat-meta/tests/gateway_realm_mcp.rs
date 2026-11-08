#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::uninlined_format_args
)]

//! Host administration of the installation realm's own MCP servers on the
//! stdio gateway (`meta/realm_mcp/{servers, install, set_secret,
//! remove_secret}`): each method is one call of the gateway's realm MCP
//! installation, admitted while the stdio connection that sent it is open,
//! with typed outcomes (the `realm_mcp_refused` umbrella and the other
//! kinds). A secret value appears only in the `set_secret` request: never in
//! a response, an error, a listing or a log line. Each case is a
//! `rpc_gateway` process on a pipe, with an identity roster the test answers
//! as the SDK host.

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

const IDENTITY: &str = "agent-a";
const MOB_CONFIG: &str = r#"
[mob]
id = "gateway-realm-mcp-test"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.worker.tools]
comms = true
"#;
const LINE_WAIT: Duration = Duration::from_secs(180);
/// The secret slot the fixture server declares.
const SLOT: &str = "SEARCH_API_KEY";
/// The value the rows set: it must appear in no response, no error, no
/// listing and no log line of the gateway.
const SECRET: &str = "realm-mcp-secret-7f3a9c41";

/// One `rpc_gateway` process on a pipe. Requests go to its stdin; a reader
/// thread hands every stdout line to the test, which keeps them all, and the
/// test answers the gateway's roster callback as the SDK host. The
/// gateway's stderr (its log) goes to a file.
struct Gateway {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
    next_id: u64,
    stderr: PathBuf,
    stdout: Vec<String>,
}

impl Gateway {
    /// Start a gateway logging at `log_filter`.
    fn start(scratch: &Path, log_filter: &str) -> Self {
        let stderr = scratch.join(format!(
            "gateway-{}.stderr",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let mut child = Command::new(env!("CARGO_BIN_EXE_rpc_gateway"))
            .arg("--persistent")
            .env("OPENAI_API_KEY", "sk-regression-test")
            .env("XDG_STATE_HOME", scratch.join("xdg-state"))
            .env("RUST_LOG", log_filter)
            .current_dir(scratch)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(
                File::create(&stderr).expect("gateway stderr file"),
            ))
            .spawn()
            .expect("spawn rpc_gateway");
        let stdin = child.stdin.take().expect("gateway stdin");
        let stdout = child.stdout.take().expect("gateway stdout");
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin,
            lines,
            next_id: 0,
            stderr,
            stdout: Vec::new(),
        }
    }

    fn send(&mut self, message: &Value) {
        writeln!(self.stdin, "{message}").expect("write");
        self.stdin.flush().expect("flush");
    }

    /// Send `method` and read stdout until its answer, answering the
    /// gateway's roster callback on the way.
    fn rpc(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = format!("realm-mcp-{}", self.next_id);
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let line = self
                .lines
                .recv_timeout(LINE_WAIT)
                .unwrap_or_else(|_| panic!("no answer to {method}"));
            self.stdout.push(line.clone());
            let Some(message) = line
                .find('{')
                .and_then(|start| serde_json::from_str::<Value>(line[start..].trim()).ok())
            else {
                continue;
            };
            if message.get("id") == Some(&json!(id)) && message.get("method").is_none() {
                return message;
            }
            if let (Some(callback), Some(callback_id)) = (
                message.get("method").and_then(Value::as_str),
                message.get("id").cloned(),
            ) {
                let reply = match callback {
                    "callback/roster_provider/roster" => json!({
                        "jsonrpc": "2.0",
                        "id": callback_id,
                        "result": [{"identity": IDENTITY, "profile": "worker"}],
                    }),
                    other => json!({
                        "jsonrpc": "2.0",
                        "id": callback_id,
                        "error": {"code": -32601, "message": format!("no handler for {other}")},
                    }),
                };
                self.send(&reply);
            }
        }
    }

    /// `meta/init` on `state` with the identity roster and `runtime_options`.
    fn init(&mut self, state: &Path, runtime_options: Value) -> Value {
        let init = self.rpc(
            "meta/init",
            json!({
                "mob_config": MOB_CONFIG,
                "persistent_state": state,
                "has_roster_provider": true,
                "runtime_options": runtime_options,
            }),
        );
        assert!(init["error"].is_null(), "the gateway starts: {init:#}");
        init
    }

    /// Every stdout line read so far and the gateway's log.
    fn everything_it_wrote(&self) -> String {
        let log = std::fs::read_to_string(&self.stderr).expect("gateway log");
        format!("{}\n{log}", self.stdout.join("\n"))
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A stdio server declaring one secret slot. It is never launched here: no
/// member is built.
fn server(name: &str, args: &[&str]) -> Value {
    json!({
        "name": name,
        "command": "sh",
        "args": args,
        "secret_env": [SLOT],
        "connect_timeout_secs": 1,
    })
}

/// The persistent launch's options with a provider credential store of its
/// own (where the secret values live).
fn with_credentials(scratch: &Path) -> Value {
    json!({
        "demo_llm": true,
        "provider_credentials_root": scratch.join("credentials"),
    })
}

/// The typed refusal of `response`: invalid params with the
/// `realm_mcp_refused` umbrella; answers the refusal.
fn realm_mcp_refusal(response: &Value) -> Value {
    assert_eq!(
        response["error"]["code"],
        json!(-32602),
        "a realm MCP refusal is invalid params: {response:#}"
    );
    assert_eq!(
        response["error"]["data"]["kind"],
        json!("realm_mcp_refused"),
        "the refusal is the typed umbrella: {response:#}"
    );
    response["error"]["data"]["realm_mcp_refusal"].clone()
}

fn generation(response: &Value) -> u64 {
    response["result"]["generation"]
        .as_u64()
        .unwrap_or_else(|| panic!("a generation: {response:#}"))
}

/// `servers` lists the realm's own document's servers and its generation;
/// `install` of the reviewed definition at that generation answers the next
/// one. An install checked against the old generation is
/// `generation_conflict` with `expected` and `current` and writes nothing,
/// and a second server of a listed name is `server_exists`.
#[test]
fn install_writes_the_reviewed_server_at_the_generation_servers_read() {
    let scratch = tempfile::tempdir().unwrap();
    let mut gateway = Gateway::start(scratch.path(), "info");
    gateway.init(
        &scratch.path().join("state"),
        with_credentials(scratch.path()),
    );

    let listed = gateway.rpc("meta/realm_mcp/servers", json!({}));
    assert!(listed["error"].is_null(), "{listed:#}");
    assert_eq!(listed["result"]["servers"], json!([]), "{listed:#}");
    let first = generation(&listed);

    let search = server("search", &["-c", "sleep 30"]);
    let installed = gateway.rpc(
        "meta/realm_mcp/install",
        json!({"server": search, "expected_generation": first}),
    );
    assert!(installed["error"].is_null(), "{installed:#}");
    let second = generation(&installed);
    assert!(second > first, "the install answers the next generation");

    let stale = gateway.rpc(
        "meta/realm_mcp/install",
        json!({"server": server("other", &["-c", "sleep 30"]), "expected_generation": first}),
    );
    let refusal = realm_mcp_refusal(&stale);
    assert_eq!(refusal["kind"], json!("generation_conflict"), "{stale:#}");
    assert_eq!(refusal["expected"], json!(first), "{stale:#}");
    assert_eq!(refusal["current"], json!(second), "{stale:#}");

    let again = gateway.rpc(
        "meta/realm_mcp/install",
        json!({"server": search, "expected_generation": second}),
    );
    assert_eq!(
        realm_mcp_refusal(&again)["kind"],
        json!("server_exists"),
        "{again:#}"
    );

    let listed = gateway.rpc("meta/realm_mcp/servers", json!({}));
    assert_eq!(
        listed["result"]["servers"],
        json!([search]),
        "the refused installs wrote nothing: {listed:#}"
    );
    assert_eq!(generation(&listed), second, "{listed:#}");
}

/// `set_secret` binds the value to the definition `servers` listed and
/// answers `{}`; a definition the document does not list is
/// `definition_changed`. `remove_secret` of the current definition clears
/// its slot, and an obsolete-value cleanup of a listed definition is
/// `still_current`. The value appears in no response, no error, no listing
/// and no line of the gateway's log at trace level, and a malformed
/// `set_secret` is refused without quoting its request.
#[test]
fn a_secret_binds_to_the_reviewed_definition_and_is_never_echoed_or_logged() {
    let scratch = tempfile::tempdir().unwrap();
    let mut gateway = Gateway::start(scratch.path(), "trace");
    gateway.init(
        &scratch.path().join("state"),
        with_credentials(scratch.path()),
    );
    let listed = gateway.rpc("meta/realm_mcp/servers", json!({}));
    let search = server("search", &["-c", "sleep 30"]);
    let installed = gateway.rpc(
        "meta/realm_mcp/install",
        json!({"server": search, "expected_generation": generation(&listed)}),
    );
    assert!(installed["error"].is_null(), "{installed:#}");

    let set = gateway.rpc(
        "meta/realm_mcp/set_secret",
        json!({"server": search, "env": SLOT, "value": SECRET}),
    );
    assert!(set["error"].is_null(), "{set:#}");
    assert_eq!(set["result"], json!({}), "{set:#}");

    let changed = gateway.rpc(
        "meta/realm_mcp/set_secret",
        json!({"server": server("search", &["-c", "sleep 60"]), "env": SLOT, "value": SECRET}),
    );
    assert_eq!(
        realm_mcp_refusal(&changed)["kind"],
        json!("definition_changed"),
        "{changed:#}"
    );

    let malformed = gateway.rpc(
        "meta/realm_mcp/set_secret",
        json!({"server": search, "env": SLOT, "value": SECRET, "note": SECRET}),
    );
    assert_eq!(malformed["error"]["code"], json!(-32602), "{malformed:#}");

    let still = gateway.rpc(
        "meta/realm_mcp/remove_secret",
        json!({"server": search, "env": SLOT, "removal": "obsolete"}),
    );
    assert_eq!(
        realm_mcp_refusal(&still)["kind"],
        json!("still_current"),
        "{still:#}"
    );
    let removed = gateway.rpc(
        "meta/realm_mcp/remove_secret",
        json!({"server": search, "env": SLOT, "removal": "current"}),
    );
    assert!(removed["error"].is_null(), "{removed:#}");
    assert_eq!(removed["result"], json!({}), "{removed:#}");

    let listed = gateway.rpc("meta/realm_mcp/servers", json!({}));
    assert_eq!(listed["result"]["servers"], json!([search]), "{listed:#}");

    let everything = gateway.everything_it_wrote();
    assert!(
        !everything.contains(SECRET),
        "the value appears in no response, error, listing or log line"
    );
}

/// A launch without a provider credential store of its own keeps no secret
/// value: `set_secret` and `remove_secret` answer `realm_mcp_unavailable`
/// (`-32004`), while the listing and the install still serve.
#[test]
fn without_a_credential_store_the_secret_methods_are_unavailable() {
    let scratch = tempfile::tempdir().unwrap();
    let mut gateway = Gateway::start(scratch.path(), "info");
    gateway.init(&scratch.path().join("state"), json!({"demo_llm": true}));
    let listed = gateway.rpc("meta/realm_mcp/servers", json!({}));
    assert!(listed["error"].is_null(), "{listed:#}");
    let search = server("search", &["-c", "sleep 30"]);
    let installed = gateway.rpc(
        "meta/realm_mcp/install",
        json!({"server": search, "expected_generation": generation(&listed)}),
    );
    assert!(installed["error"].is_null(), "{installed:#}");

    for (method, params) in [
        (
            "meta/realm_mcp/set_secret",
            json!({"server": search, "env": SLOT, "value": SECRET}),
        ),
        (
            "meta/realm_mcp/remove_secret",
            json!({"server": search, "env": SLOT, "removal": "current"}),
        ),
    ] {
        let answer = gateway.rpc(method, params);
        assert_eq!(
            answer["error"]["code"],
            json!(-32004),
            "{method}: {answer:#}"
        );
        assert_eq!(
            answer["error"]["data"]["kind"],
            json!("realm_mcp_unavailable"),
            "{method}: {answer:#}"
        );
    }
}

/// The stdio gateway advertises the realm MCP methods it serves, and not
/// `meta/realm_mcp/remove`, which a later part adds.
#[test]
fn the_stdio_capabilities_list_the_realm_mcp_methods_served() {
    let scratch = tempfile::tempdir().unwrap();
    let mut gateway = Gateway::start(scratch.path(), "info");
    gateway.init(
        &scratch.path().join("state"),
        with_credentials(scratch.path()),
    );
    let capabilities = gateway.rpc("meta/capabilities", json!({}));
    let methods = capabilities["result"]["methods"]
        .as_array()
        .unwrap_or_else(|| panic!("methods: {capabilities:#}"))
        .clone();
    for method in [
        "meta/realm_mcp/servers",
        "meta/realm_mcp/install",
        "meta/realm_mcp/set_secret",
        "meta/realm_mcp/remove_secret",
    ] {
        assert!(
            methods.contains(&json!(method)),
            "{method} is advertised: {methods:?}"
        );
    }
    assert!(
        !methods.contains(&json!("meta/realm_mcp/remove")),
        "remove is not served yet: {methods:?}"
    );
    let remove = gateway.rpc(
        "meta/realm_mcp/remove",
        json!({"name": "search", "expected_generation": 1}),
    );
    assert_eq!(remove["error"]["code"], json!(-32601), "{remove:#}");
}

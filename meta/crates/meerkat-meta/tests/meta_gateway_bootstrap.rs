#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args
)]

//! Regression: the DEFAULT (ephemeral) `meta_gateway` bootstrap path failed
//! closed on the very first `meta/init` in every shipped 0.7.x binary. It
//! attached an explicit runtime adapter that did NOT share the session service's
//! runtime persistence authority, which meerkat 0.7 rejects with
//! `failed to bootstrap local runtime` (the `with_session_runtime_adapter` call
//! that `rpc_gateway` already had was missing). `persistent_sessions` defaults
//! off, so this is what every gateway launch hit — yet no test ever spawned
//! `meta_gateway`, so it shipped broken (0.7.5, 0.7.6).
//!
//! These tests drive a real `meta/init` for BOTH session modes and assert the
//! local runtime bootstraps. No API keys are needed — bootstrap runs before any
//! provider call. The mob is the gateway's built-in fallback definition (the bug
//! is independent of the mob), so an empty workspace is sufficient.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;

/// Spawn the real `meta_gateway` binary, send one `meta/init` (with the
/// gateway's actual `InitParams` field names), and assert the local runtime
/// bootstraps (an init `result`, not an `error`). The read happens on a worker
/// thread bounded by `recv_timeout` so a bootstrap hang can't wedge the suite.
fn assert_bootstraps(persistent_sessions: bool) {
    let bin = env!("CARGO_BIN_EXE_meta_gateway");
    let workspace = TempDir::new().expect("workspace tempdir");
    let store = TempDir::new().expect("store tempdir");

    let mut params = serde_json::Map::new();
    params.insert(
        "workspace_root".into(),
        json!(workspace.path().to_string_lossy()),
    );
    params.insert(
        "store_path".into(),
        json!(store.path().join("store").to_string_lossy()),
    );
    if persistent_sessions {
        params.insert("persistent_sessions".into(), json!(true));
    }
    let init = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "meta/init",
        "params": Value::Object(params),
    });

    let mut child = Command::new(bin)
        .current_dir(workspace.path())
        // Dummy provider secrets: an empty workspace boots the fallback mob whose
        // agent's LLM client is CREATED at bootstrap (it needs a secret present)
        // but never CALLED, so placeholders suffice and the test stays
        // key-independent / CI-safe.
        .env("ANTHROPIC_API_KEY", "sk-ant-regression-test")
        .env("OPENAI_API_KEY", "sk-regression-test")
        // Isolate the gateway's state dir (peer keys) into this test's temp
        // workspace: the default ~/.local/state/meerkat-meta is shared
        // across concurrent test processes on CI runners and intermittently
        // fails the peer-key load/mint (a recurring CI flake).
        .env("XDG_STATE_HOME", workspace.path().join("xdg-state"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn meta_gateway");

    let mut stdin = child.stdin.take().expect("stdin");
    writeln!(stdin, "{}", serde_json::to_string(&init).unwrap()).expect("write init");
    stdin.flush().expect("flush");

    let stdout = child.stdout.take().expect("stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let _ = reader.read_line(&mut line);
        let _ = tx.send(line);
    });

    let line = rx
        .recv_timeout(Duration::from_secs(45))
        .expect("meta_gateway did not answer meta/init within 45s (bootstrap hung?)");

    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();

    let resp: Value = serde_json::from_str(line.trim())
        .unwrap_or_else(|e| panic!("non-JSON init response {:?}: {}", line, e));
    // Boot evidence requires a usable gateway, including any failure surfaced
    // before or after the canonical runtime-owner check.
    assert!(
        resp.get("error").is_none() && resp["result"]["http_base_url"].is_string(),
        "meta_gateway failed to bootstrap a local runtime (persistent_sessions={}): {}",
        persistent_sessions,
        resp
    );
}

/// The default mode — this is the path that broke in 0.7.5/0.7.6.
#[test]
fn meta_gateway_bootstraps_ephemeral_runtime() {
    assert_bootstraps(false);
}

/// The persistent-sessions mode also goes through `with_session_runtime_adapter`.
#[test]
fn meta_gateway_bootstraps_persistent_runtime() {
    assert_bootstraps(true);
}

/// One Meta runtime per state directory: a second `meta_gateway` on a store
/// a running gateway holds refuses `meta/init` typed (STORAGE_RESOLUTION,
/// -32014) before it opens any store, and the store boots again once the
/// first gateway has exited. The refused gateway gets its own workspace and
/// gateway home, so the store directory is the only thing it shares with the
/// first. The third reuses the first's workspace: the fallback mob's id
/// derives from it, and the store pins the first gateway's definition.
#[test]
fn meta_gateway_refuses_a_store_another_gateway_holds() {
    let bin = env!("CARGO_BIN_EXE_meta_gateway");
    let store = TempDir::new().expect("store tempdir");
    let first_workspace = TempDir::new().expect("first workspace tempdir");
    let init = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "meta/init",
        "params": {
            "workspace_root": first_workspace.path().to_string_lossy(),
            "store_path": store.path().join("store").to_string_lossy(),
            "persistent_sessions": true,
        },
    });
    let mut first = Command::new(bin)
        .current_dir(first_workspace.path())
        .env("ANTHROPIC_API_KEY", "sk-ant-regression-test")
        .env("OPENAI_API_KEY", "sk-regression-test")
        .env("XDG_STATE_HOME", first_workspace.path().join("xdg-state"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn the first meta_gateway");
    let mut stdin = first.stdin.take().expect("stdin");
    writeln!(stdin, "{}", serde_json::to_string(&init).unwrap()).expect("write init");
    stdin.flush().expect("flush");
    let stdout = first.stdout.take().expect("stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let _ = reader.read_line(&mut line);
        let _ = tx.send(line);
    });
    let line = rx
        .recv_timeout(Duration::from_secs(45))
        .expect("the first meta_gateway did not answer meta/init within 45s");
    let ready: Value = serde_json::from_str(line.trim())
        .unwrap_or_else(|e| panic!("non-JSON init response {:?}: {}", line, e));
    assert!(
        ready.get("error").is_none() && ready["result"]["http_base_url"].is_string(),
        "the first meta_gateway must boot: {ready}"
    );

    let second_workspace = TempDir::new().expect("second workspace tempdir");
    let refused = init_once_in(
        second_workspace.path(),
        store.path(),
        json!({ "persistent_sessions": true }),
        Duration::from_secs(45),
    );
    assert_eq!(refused["error"]["code"], -32014, "{refused}");
    let message = refused["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("is in use by another Meta runtime"),
        "{message}"
    );

    drop(stdin);
    let _ = first.kill();
    let _ = first.wait();

    let again = init_once_in(
        first_workspace.path(),
        store.path(),
        json!({ "persistent_sessions": true }),
        Duration::from_secs(45),
    );
    assert!(
        again.get("error").is_none() && again["result"]["http_base_url"].is_string(),
        "the store must boot again once the first gateway has exited: {again}"
    );
}

/// Keep the actual HTTP host alive through a clean stop. Hard-kill is only an
/// unwind guard; a successful test must observe graceful cleanup and MobStopped.
#[cfg(unix)]
struct RestartGateway {
    child: std::process::Child,
    _stdin: std::process::ChildStdin,
    stderr: Option<std::thread::JoinHandle<Vec<u8>>>,
    http_base_url: String,
    bearer: Option<String>,
}

#[cfg(unix)]
impl RestartGateway {
    fn start(workspace: &Path, store: &Path, identity_first: bool, voice: bool) -> Self {
        let mut params = json!({
            "workspace_root": workspace,
            "store_path": store,
            "persistent_sessions": true,
            "identity_first": identity_first,
        });
        if identity_first {
            params["identity_roster"] = json!([{
                "identity": "personal:alice", "profile": "alpha"
            }]);
        }
        let bearer = voice.then(|| {
            params["auth_config"] = json!({
                "provider": "jwt", "shared_secret": "test-console-signing-key",
                "email_allowlist": ["voice@example.com"]
            });
            params["console_voice"] = json!({
                "principal": "voice@example.com", "realm": "voice",
                "auth_binding": {"realm": "voice", "binding": "openai"},
                "voice": "marin"
            });
            jsonwebtoken::encode(
                &jsonwebtoken::Header::default(),
                &json!({
                    "iss": "http://127.0.0.1/meta-gateway",
                    "aud": "persistent-gateway", "sub": "voice@example.com",
                    "email": "voice@example.com",
                    "exp": chrono::Utc::now().timestamp() + 600,
                }),
                &jsonwebtoken::EncodingKey::from_secret(b"test-console-signing-key"),
            )
            .expect("sign console token")
        });
        let mut child = Command::new(env!("CARGO_BIN_EXE_meta_gateway"))
            .current_dir(workspace)
            .env("ANTHROPIC_API_KEY", "sk-ant-regression-test")
            .env("OPENAI_API_KEY", "sk-regression-test")
            .env("XDG_STATE_HOME", workspace.join("xdg-state"))
            .env("RUST_LOG", "info")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn persistent HTTP gateway");
        let stdin = child.stdin.take().expect("gateway stdin");
        let stdout = child.stdout.take().expect("gateway stdout");
        let mut stderr = child.stderr.take().expect("gateway stderr");
        let stderr = std::thread::spawn(move || {
            use std::io::Read;
            let mut bytes = Vec::new();
            let _ = stderr.read_to_end(&mut bytes);
            bytes
        });
        let mut gateway = Self {
            child,
            _stdin: stdin,
            stderr: Some(stderr),
            http_base_url: String::new(),
            bearer,
        };
        writeln!(
            gateway._stdin,
            "{}",
            json!({
                "jsonrpc": "2.0", "id": 1, "method": "meta/init", "params": params
            })
        )
        .expect("write persistent init");
        gateway._stdin.flush().expect("flush persistent init");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = BufReader::new(stdout).read_line(&mut line);
            let _ = tx.send(line);
        });
        let line = rx
            .recv_timeout(Duration::from_secs(45))
            .expect("persistent init timeout");
        let response: Value = serde_json::from_str(&line).expect("persistent init JSON");
        assert!(
            response.get("error").is_none(),
            "persistent init failed: {response}"
        );
        assert_eq!(response["result"]["launch_state"], "created", "{response}");
        gateway.http_base_url = response["result"]["http_base_url"]
            .as_str()
            .expect("HTTP base URL")
            .to_owned();
        gateway
    }

    fn rpc(&self, method: &str, params: Value) -> Value {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("HTTP client");
        let mut request = client
            .post(format!("{}/console/rpc", self.http_base_url))
            .json(&json!({"jsonrpc": "2.0", "id": 2, "method": method, "params": params}));
        if let Some(bearer) = &self.bearer {
            request = request.bearer_auth(bearer);
        }
        let response: Value = request
            .send()
            .expect("HTTP RPC")
            .error_for_status()
            .expect("HTTP RPC status")
            .json()
            .expect("HTTP RPC JSON");
        assert!(
            response.get("error").is_none(),
            "{method} failed: {response}"
        );
        response["result"].clone()
    }

    fn stop(mut self) {
        assert!(
            Command::new("kill")
                .args(["-TERM", &self.child.id().to_string()])
                .status()
                .expect("signal gateway")
                .success()
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(45);
        let status = loop {
            if let Some(status) = self.child.try_wait().expect("gateway exit status") {
                break status;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "graceful shutdown timed out"
            );
            std::thread::sleep(Duration::from_millis(20));
        };
        let bytes = self
            .stderr
            .take()
            .expect("stderr task")
            .join()
            .expect("stderr drain");
        let stderr = String::from_utf8_lossy(&bytes);
        assert!(status.success(), "gateway exit {status}: {stderr}");
        assert!(
            stderr.contains("meta_gateway shutdown complete; exiting"),
            "{stderr}"
        );
    }
}

#[cfg(unix)]
impl Drop for RestartGateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if std::thread::panicking()
            && let Some(stderr) = self.stderr.take()
            && let Ok(bytes) = stderr.join()
        {
            eprintln!(
                "persistent HTTP gateway stderr: {}",
                String::from_utf8_lossy(&bytes)
            );
        }
    }
}

#[cfg(unix)]
fn assert_persistent_http_restart(identity_first: bool, voice: bool) {
    let workspace = TempDir::new().expect("restart workspace");
    let store = workspace.path().join("store");
    let layout = meerkat_meta::storage_layout::MetaStorageLayout::standalone_from_store_path(
        &store,
        workspace.path().join("xdg-state"),
    );
    let mut first_binding = None;
    for boot in 1..=2 {
        let gateway = RestartGateway::start(workspace.path(), &store, identity_first, voice);
        // HTTP status reads the actual MobMachine state, not module activity.
        assert_eq!(gateway.rpc("meta/status", json!({}))["running"], true);
        let fallback = gateway.rpc("meta/member_status", json!({"member_id": "alpha"}));
        assert!(fallback["current_session_id"].is_string(), "{fallback}");
        let binding = if identity_first {
            let identity = gateway.rpc(
                "meta/status_identity",
                json!({"identity": "personal:alice"}),
            );
            assert_eq!(identity["state"], "active", "{identity}");
            assert!(identity["session_id"].is_string(), "{identity}");
            assert!(identity["agent_runtime_id"].is_string(), "{identity}");
            let member = gateway.rpc(
                "meta/member_status",
                json!({
                    "member_id": identity["agent_runtime_id"]
                }),
            );
            assert_eq!(member["current_session_id"], identity["session_id"]);
            json!([identity["agent_runtime_id"], identity["session_id"]])
        } else {
            fallback["current_session_id"].clone()
        };
        if let Some(first) = &first_binding {
            assert_eq!(
                &binding, first,
                "second boot must preserve the real session binding"
            );
        } else {
            first_binding = Some(binding);
        }
        // Exercise the activated owner without any provider call. A successful
        // status projection alone would not establish that membership works.
        let worker = format!("restart-worker-{boot}");
        gateway.rpc(
            "meta/ensure_member",
            json!({
                "role": "worker", "agent_identity": worker, "plane": "worker"
            }),
        );
        let member = gateway.rpc("meta/member_status", json!({"member_id": worker}));
        assert!(member["current_session_id"].is_string(), "{member}");
        gateway.stop();

        // Each boot must start from a genuinely persisted Stopped mob. Inspect
        // the stored event envelope, not a shutdown log or a mocked state.
        let db = rusqlite::Connection::open_with_flags(
            layout.event_log_db(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .expect("durable mob event log");
        let stopped: u32 = db.query_row(
            "SELECT COUNT(*) FROM mob_events WHERE json_extract(CAST(event_json AS TEXT), '$.event.kind.type') = 'mob_stopped'",
            [], |row| row.get(0),
        ).expect("count durable stop events");
        assert_eq!(
            stopped, boot,
            "each clean shutdown must persist exactly one MobStopped"
        );
    }
}

#[cfg(unix)]
#[test]
fn persistent_http_restarts_classic_without_voice() {
    assert_persistent_http_restart(false, false);
}

#[cfg(unix)]
#[test]
fn persistent_http_restarts_identity_without_voice() {
    assert_persistent_http_restart(true, false);
}

/// A console-gateway state directory `storage-migrate-native` converted:
/// the identity's session is in Core's session store, Core's mob store is
/// gone (0.9.3 kept the mob journal in memory), and the first-boot
/// manifest's `Resume` row names that session. The gateway's first roster
/// compile carries the row out: the identity resumes the session and the
/// manifest is removed. A gateway that never reads the manifest wakes the
/// identity fresh on a new session, and the migrated one stays unbound.
#[cfg(unix)]
#[test]
fn persistent_http_first_boot_carries_out_a_migrated_resume_row() {
    let workspace = TempDir::new().expect("first-boot workspace");
    let store = workspace.path().join("store");
    let alice = json!({"identity": "personal:alice"});

    let gateway = RestartGateway::start(workspace.path(), &store, true, false);
    let written = gateway.rpc("meta/status_identity", alice.clone());
    let session = written["session_id"]
        .as_str()
        .unwrap_or_else(|| panic!("alice has a session: {written}"))
        .to_owned();
    gateway.stop();

    let layout = meerkat_meta::storage_layout::MetaStorageLayout::standalone_from_store_path(
        &store,
        workspace.path().join("xdg-state"),
    );
    let mob_store = layout
        .event_log_db()
        .file_name()
        .and_then(|name| name.to_str())
        .expect("the mob store has a file name")
        .to_owned();
    let mut removed = 0;
    for entry in std::fs::read_dir(layout.state_dir()).expect("read the state directory") {
        let entry = entry.expect("a state entry");
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(&mob_store))
        {
            std::fs::remove_file(entry.path()).expect("remove the mob store");
            removed += 1;
        }
    }
    assert!(removed > 0, "the persistent launch kept no mob store");
    meerkat_meta::identity_first::FirstBootManifest::write_new(
        layout.state_dir(),
        vec![meerkat_meta::identity_first::FirstBootRow::Resume {
            identity: meerkat_meta::identity_first::AgentIdentity::parse("personal:alice")
                .expect("the identity parses"),
            session: meerkat_core::types::SessionId::parse(&session)
                .expect("the session id parses"),
        }],
    )
    .expect("storage-migrate-native writes the first-boot manifest");

    let gateway = RestartGateway::start(workspace.path(), &store, true, false);
    let carried = gateway.rpc("meta/status_identity", alice);
    assert_eq!(carried["state"], "active", "{carried}");
    assert_eq!(
        carried["session_id"],
        json!(session),
        "the first boot resumes the session the migration names: {carried}"
    );
    assert!(
        !meerkat_meta::identity_first::FirstBootManifest::path(layout.state_dir()).exists(),
        "the manifest is removed once its last row is carried out"
    );
    gateway.stop();
}

#[cfg(all(unix, feature = "openai-live"))]
#[test]
fn persistent_http_restarts_classic_with_voice() {
    assert_persistent_http_restart(false, true);
}

#[cfg(all(unix, feature = "openai-live"))]
#[test]
fn persistent_http_restarts_identity_with_voice() {
    assert_persistent_http_restart(true, true);
}

/// Both gateway binaries must self-identify via `--version` (name + version),
/// so operators can tell which of the two they have without hashing the file —
/// the gap that turned a mislabeled binary into a multi-day investigation.
#[test]
fn gateways_report_name_and_version() {
    for (bin, needle) in [
        (env!("CARGO_BIN_EXE_meta_gateway"), "meta_gateway"),
        (env!("CARGO_BIN_EXE_rpc_gateway"), "rpc_gateway"),
    ] {
        let out = Command::new(bin)
            .arg("--version")
            .output()
            .expect("run --version");
        assert!(out.status.success(), "{needle} --version exited non-zero");
        let stdout = String::from_utf8_lossy(&out.stdout);
        // Assert the ACTUAL crate version, not just "some digit", so this test
        // catches a stale/wrong baked-in version (the BUILD.bazel drift class).
        assert!(
            stdout.contains(needle) && stdout.contains(env!("CARGO_PKG_VERSION")),
            "{needle} --version did not print name + version {}: {stdout:?}",
            env!("CARGO_PKG_VERSION")
        );
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    format!("{:x}", sha2::Sha256::digest(bytes))
}

/// Both gateway binaries embed the MCP Apps sandbox proxy that the stock
/// Console needs on an origin of its own, so a released binary is enough to
/// run it. `--write-mcp-apps-sandbox <dir>` writes the proxy in the layout
/// `node mcp-apps-sandbox.cjs` loads, byte for byte the console sources the
/// binary was built from, lists each file with its SHA-256, and exits before
/// it reads an init request.
#[test]
fn gateways_write_the_mcp_apps_sandbox_they_embed_byte_for_byte() {
    let console = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../console");
    let files = ["mcp-apps-sandbox.cjs", "src/mcp-apps/sandbox.js"];
    let sources = files
        .iter()
        .map(|file| std::fs::read(console.join(file)).expect("console sandbox source"))
        .collect::<Vec<_>>();
    let mut actual = Vec::new();
    let mut expected = Vec::new();
    let mut stderr = Vec::new();
    for (bin, name) in [
        (env!("CARGO_BIN_EXE_meta_gateway"), "meta_gateway"),
        (env!("CARGO_BIN_EXE_rpc_gateway"), "rpc_gateway"),
    ] {
        let dir = TempDir::new().expect("sandbox tempdir");
        let target = dir.path().join("mcp-apps-sandbox");
        let out = Command::new(bin)
            .arg("--write-mcp-apps-sandbox")
            .arg(&target)
            .stdin(Stdio::null())
            .output()
            .expect("run --write-mcp-apps-sandbox");
        actual.push((
            name,
            out.status.code(),
            files
                .iter()
                .map(|file| {
                    std::fs::read(target.join(file))
                        .ok()
                        .map(|bytes| sha256_hex(&bytes))
                })
                .collect::<Vec<_>>(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
        ));
        expected.push((
            name,
            Some(0),
            sources
                .iter()
                .map(|bytes| Some(sha256_hex(bytes)))
                .collect::<Vec<_>>(),
            files
                .iter()
                .zip(&sources)
                .map(|(file, bytes)| {
                    format!("{}  {}\n", sha256_hex(bytes), target.join(file).display())
                })
                .collect::<Vec<_>>()
                .concat(),
        ));
        stderr.push(format!("{name}: {}", String::from_utf8_lossy(&out.stderr)));
    }
    assert_eq!(actual, expected, "gateway stderr:\n{}", stderr.join("\n"));
}

/// `meta_gateway` is the console/HTTP gateway, not the SDK's stdin-RPC gateway
/// (that is `rpc_gateway`). If the SDK — which drives a gateway by sending
/// JSON-RPC over stdin — is misconfigured to spawn `meta_gateway`, init still
/// succeeds (one stdin line), but the *next* RPC must get a clear error instead
/// of an infinite hang. This is the regression test for a downstream app's reconcile
/// "deadlock": before the fail-loud guard, `meta_gateway` read only the init
/// line and then served HTTP forever, silently ignoring `reconcile_identity`.
#[test]
fn meta_gateway_rejects_post_init_stdin_rpc_loudly() {
    let bin = env!("CARGO_BIN_EXE_meta_gateway");
    let workspace = TempDir::new().expect("workspace tempdir");
    let store = TempDir::new().expect("store tempdir");

    let init = json!({
        "jsonrpc": "2.0", "id": 1, "method": "meta/init",
        "params": {
            "workspace_root": workspace.path().to_string_lossy(),
            "store_path": store.path().join("store").to_string_lossy(),
        }
    });
    let reconcile =
        json!({ "jsonrpc": "2.0", "id": 2, "method": "meta/reconcile_identity", "params": {} });

    let mut child = Command::new(bin)
        .current_dir(workspace.path())
        .env("ANTHROPIC_API_KEY", "sk-ant-regression-test")
        .env("OPENAI_API_KEY", "sk-regression-test")
        // Isolated state dir (peer keys) — see the sibling spawn above; the
        // shared ~/.local/state default is a recurring CI peer-key flake.
        .env("XDG_STATE_HOME", workspace.path().join("xdg-state"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn meta_gateway");

    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            match line {
                Ok(l) => {
                    if tx.send(l).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    writeln!(stdin, "{}", serde_json::to_string(&init).unwrap()).expect("write init");
    stdin.flush().expect("flush init");
    let init_resp = rx
        .recv_timeout(Duration::from_secs(45))
        .expect("no init response within 45s");
    let init_v: Value = serde_json::from_str(init_resp.trim())
        .unwrap_or_else(|e| panic!("non-JSON init response {init_resp:?}: {e}"));
    assert!(
        init_v.get("result").is_some(),
        "init did not return a result (cannot exercise the post-init guard): {init_resp}"
    );

    writeln!(stdin, "{}", serde_json::to_string(&reconcile).unwrap()).expect("write reconcile");
    stdin.flush().expect("flush reconcile");
    let reconcile_resp = rx.recv_timeout(Duration::from_secs(20)).expect(
        "meta_gateway did not answer a post-init stdin RPC within 20s — the silent-hang regressed",
    );

    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();

    let v: Value = serde_json::from_str(reconcile_resp.trim())
        .unwrap_or_else(|e| panic!("non-JSON reconcile response {reconcile_resp:?}: {e}"));
    let msg = v
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
        .unwrap_or("");
    assert!(
        msg.contains("rpc_gateway"),
        "post-init stdin RPC must fail loudly and point at rpc_gateway, got: {reconcile_resp}"
    );
}

/// meerkat-studio K2 root cause on this binary: `meta_gateway` had no
/// tracing subscriber, so every WARN/ERROR in the process — runtime
/// failures, console internal-error logs, the schedule claim watchdog's
/// stall diagnosis — was silently dropped. Pin that the binary emits
/// tracing on stderr: with RUST_LOG=info the bootstrap path logs at least
/// one INFO line.
#[test]
fn meta_gateway_emits_tracing_on_stderr() {
    let bin = env!("CARGO_BIN_EXE_meta_gateway");
    let workspace = TempDir::new().expect("workspace tempdir");
    let store = TempDir::new().expect("store tempdir");

    let init = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "meta/init",
        "params": {
            "workspace_root": workspace.path().to_string_lossy(),
            "store_path": store.path().join("store").to_string_lossy(),
        },
    });

    let mut child = Command::new(bin)
        .current_dir(workspace.path())
        .env("ANTHROPIC_API_KEY", "sk-ant-regression-test")
        .env("OPENAI_API_KEY", "sk-regression-test")
        .env("XDG_STATE_HOME", workspace.path().join("xdg-state"))
        .env("RUST_LOG", "info")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn meta_gateway");

    let mut stdin = child.stdin.take().expect("stdin");
    writeln!(stdin, "{}", serde_json::to_string(&init).unwrap()).expect("write init");
    stdin.flush().expect("flush");

    // Drain stderr CONCURRENTLY from spawn. Leaving the pipe unread until
    // after init deadlocks under kernel pipe-buffer pressure: when the
    // machine-wide pipe pool is exhausted (macOS degrades fresh pipes to
    // 512-byte buffers; observed with ~1.5k pipes leaked by a co-located
    // agent), the gateway's ~576 bytes of bootstrap tracing overflow the
    // buffer and its 4th stderr write blocks BEFORE the init response is
    // printed — the test then times out on stdout with an almost-empty pipe.
    let stderr_pipe = child.stderr.take().expect("stderr");
    let stderr_drain = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        use std::io::Read;
        let mut reader = stderr_pipe;
        let _ = reader.read_to_end(&mut buffer);
        buffer
    });

    // Wait for the init response so bootstrap tracing has happened.
    let stdout = child.stdout.take().expect("stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let _ = reader.read_line(&mut line);
        let _ = tx.send(line);
    });
    let _init_response = rx
        .recv_timeout(Duration::from_mins(1))
        .expect("gateway responded to init");

    let _ = stdin;
    let _ = child.kill();
    let _ = child.wait().expect("gateway exit");
    let stderr_bytes = stderr_drain.join().expect("stderr drain thread");
    let stderr = String::from_utf8_lossy(&stderr_bytes);
    assert!(
        stderr.contains("INFO") || stderr.contains("WARN"),
        "meta_gateway must emit tracing on stderr (RUST_LOG=info); \
         got stderr: {stderr:?}"
    );
}

/// Doctrine default-on: a plain init (no identity_first param) boots the
/// gateway with the durable-identity substrate (continuity store + leases +
/// identity console surface) constructed from the existing store paths.
/// `identity_first: false` remains as a one-release opt-out (tested below).
#[test]
fn meta_gateway_bootstraps_identity_first_runtime() {
    let bin = env!("CARGO_BIN_EXE_meta_gateway");
    let workspace = TempDir::new().expect("workspace tempdir");
    let store = TempDir::new().expect("store tempdir");

    let init = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "meta/init",
        "params": {
            "workspace_root": workspace.path().to_string_lossy(),
            "store_path": store.path().join("store").to_string_lossy(),
            "persistent_sessions": true,
            "identity_roster": [{
                "identity": "personal:alice",
                "profile": "alpha",
            }],
        },
    });

    let mut child = Command::new(bin)
        .current_dir(workspace.path())
        .env("ANTHROPIC_API_KEY", "sk-ant-regression-test")
        .env("OPENAI_API_KEY", "sk-regression-test")
        .env("XDG_STATE_HOME", workspace.path().join("xdg-state"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn meta_gateway");

    let mut stdin = child.stdin.take().expect("stdin");
    writeln!(stdin, "{}", serde_json::to_string(&init).unwrap()).expect("write init");
    stdin.flush().expect("flush");

    let stdout = child.stdout.take().expect("stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let _ = reader.read_line(&mut line);
        let _ = tx.send(line);
    });
    let line = rx
        .recv_timeout(Duration::from_secs(90))
        .expect("gateway responded to identity-first init");
    let _ = child.kill();
    let _ = child.wait();
    let response: Value = serde_json::from_str(line.trim()).expect("init response json");
    assert!(
        response.get("error").is_none(),
        "identity-first init must succeed: {response}"
    );
    assert!(
        response["result"]["http_base_url"].is_string(),
        "init response carries the http base url: {response}"
    );
    // The identity substrate is durable: the canonical continuity store
    // exists in the store dir (M2 spelling on a fresh dir; a legacy
    // `continuity.db` would keep being used where it lies instead).
    let continuity = store.path().join("store").join("continuity.sqlite3");
    assert!(
        continuity.exists(),
        "identity-first boot must create {}",
        continuity.display()
    );
}

/// The one-release opt-out: `identity_first: false` boots the pure mob-plane
/// gateway — no continuity store is created.
#[test]
fn meta_gateway_identity_first_opt_out_skips_the_substrate() {
    let bin = env!("CARGO_BIN_EXE_meta_gateway");
    let workspace = TempDir::new().expect("workspace tempdir");
    let store = TempDir::new().expect("store tempdir");

    let init = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "meta/init",
        "params": {
            "workspace_root": workspace.path().to_string_lossy(),
            "store_path": store.path().join("store").to_string_lossy(),
            "identity_first": false,
        },
    });

    let mut child = Command::new(bin)
        .current_dir(workspace.path())
        .env("ANTHROPIC_API_KEY", "sk-ant-regression-test")
        .env("OPENAI_API_KEY", "sk-regression-test")
        .env("XDG_STATE_HOME", workspace.path().join("xdg-state"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn meta_gateway");

    let mut stdin = child.stdin.take().expect("stdin");
    writeln!(stdin, "{}", serde_json::to_string(&init).unwrap()).expect("write init");
    stdin.flush().expect("flush");

    let stdout = child.stdout.take().expect("stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let _ = reader.read_line(&mut line);
        let _ = tx.send(line);
    });
    let line = rx
        .recv_timeout(Duration::from_mins(1))
        .expect("gateway responded to opt-out init");
    let _ = child.kill();
    let _ = child.wait();
    let response: Value = serde_json::from_str(line.trim()).expect("init response json");
    assert!(
        response.get("error").is_none(),
        "opt-out init must succeed: {response}"
    );
    for name in ["continuity.sqlite3", "continuity.db"] {
        let continuity = store.path().join("store").join(name);
        assert!(
            !continuity.exists(),
            "identity_first: false must not create {}",
            continuity.display()
        );
    }
}

/// Spawn the real binary, send one `meta/init` whose params are `extra`
/// plus a fresh workspace and store, and return the first stdout line as
/// JSON (a `result` or an `error`).
fn init_once(extra: Value, timeout: Duration) -> Value {
    let workspace = TempDir::new().expect("workspace tempdir");
    let store = TempDir::new().expect("store tempdir");
    init_once_in(workspace.path(), store.path(), extra, timeout)
}

/// [`init_once`] against caller-owned directories, so a test can inspect what
/// the boot left (or did not leave) on disk. `store_path` is `<store>/store`.
fn init_once_in(workspace: &Path, store: &Path, extra: Value, timeout: Duration) -> Value {
    init_at(workspace, &store.join("store"), extra, timeout)
}

/// [`init_once_in`] with the `store_path` given exactly.
fn init_at(workspace: &Path, store_path: &Path, extra: Value, timeout: Duration) -> Value {
    let bin = env!("CARGO_BIN_EXE_meta_gateway");

    let mut params = match extra {
        Value::Object(map) => map,
        other => panic!("init params must be a JSON object, got {other}"),
    };
    params.insert("workspace_root".into(), json!(workspace.to_string_lossy()));
    params.insert("store_path".into(), json!(store_path.to_string_lossy()));
    let init = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "meta/init",
        "params": Value::Object(params),
    });

    let mut child = Command::new(bin)
        .current_dir(workspace)
        .env("ANTHROPIC_API_KEY", "sk-ant-regression-test")
        .env("OPENAI_API_KEY", "sk-regression-test")
        .env("XDG_STATE_HOME", workspace.join("xdg-state"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn meta_gateway");

    let mut stdin = child.stdin.take().expect("stdin");
    writeln!(stdin, "{}", serde_json::to_string(&init).unwrap()).expect("write init");
    stdin.flush().expect("flush");

    let stdout = child.stdout.take().expect("stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let _ = reader.read_line(&mut line);
        let _ = tx.send(line);
    });
    let line = rx
        .recv_timeout(timeout)
        .expect("meta_gateway did not answer meta/init in time");
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    serde_json::from_str(line.trim())
        .unwrap_or_else(|e| panic!("non-JSON init response {:?}: {}", line, e))
}

/// A `[self_hosted.models]` alias whose server is not declared: parses, fails
/// meerkat's `Config::validate`, and must be refused at init by path on both
/// binaries instead of killing the first member build.
const DANGLING_HOST_CONFIG: &str =
    "[self_hosted.models.gemma-4-31b]\nserver = \"nowhere\"\nremote_model = \"gemma4:31b\"\n";

/// A non-loopback `http_listen` without `allow_remote` is refused at init,
/// before bootstrap: the reply is a `-32602` error ON THE REQUEST ID that
/// names the address and the acknowledgement, not a bootstrapped runtime
/// bound to every interface and not an "internal error" with a null id.
#[test]
fn meta_gateway_refuses_a_non_loopback_http_listen_without_allow_remote() {
    let response = init_once(
        json!({ "http_listen": "0.0.0.0:0" }),
        Duration::from_secs(45),
    );
    assert!(
        response.get("result").is_none(),
        "a wildcard bind must not succeed without allow_remote: {response}"
    );
    assert_eq!(response["id"], json!(1), "{response}");
    assert_eq!(response["error"]["code"], json!(-32602), "{response}");
    let message = response["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("0.0.0.0:0"), "{response}");
    assert!(message.contains("allow_remote"), "{response}");
}

/// `http_listen` makes a fixed port possible, so a port another process holds
/// is a realistic init failure. It must be answered before any bootstrap:
/// `-32603` on the request id naming the address, and nothing under the store
/// path (the identity substrate, session store and schedule executor lease
/// all live there and none may have been touched).
#[test]
fn meta_gateway_refuses_a_taken_fixed_port_at_init_without_bootstrapping() {
    let holder = std::net::TcpListener::bind("127.0.0.1:0").expect("hold a loopback port");
    let taken = holder.local_addr().expect("held address");
    let workspace = TempDir::new().expect("workspace tempdir");
    let store = TempDir::new().expect("store tempdir");

    let response = init_once_in(
        workspace.path(),
        store.path(),
        json!({ "http_listen": taken.to_string() }),
        Duration::from_secs(45),
    );

    assert!(
        response.get("result").is_none(),
        "a taken port must not produce a runtime: {response}"
    );
    assert_eq!(response["id"], json!(1), "{response}");
    assert_eq!(response["error"]["code"], json!(-32603), "{response}");
    let message = response["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains(&taken.to_string()), "{response}");
    assert!(message.contains("failed to bind"), "{response}");
    let store_dir = store.path().join("store");
    assert!(
        !store_dir.exists(),
        "a refused bind must not have bootstrapped anything under {}",
        store_dir.display()
    );
    drop(holder);
}

/// The host config is validated with meerkat's own rules at init: a dangling
/// server reference is `-32602` on the request id naming the file, on this
/// binary too.
#[test]
fn meta_gateway_refuses_a_host_config_that_does_not_validate_by_path() {
    let workspace = TempDir::new().expect("workspace tempdir");
    let store = TempDir::new().expect("store tempdir");
    let config = workspace.path().join("dangling.toml");
    std::fs::write(&config, DANGLING_HOST_CONFIG).expect("write host config");

    let response = init_once_in(
        workspace.path(),
        store.path(),
        json!({ "meerkat_config_path": config.to_string_lossy() }),
        Duration::from_secs(45),
    );

    assert!(response.get("result").is_none(), "{response}");
    assert_eq!(response["id"], json!(1), "{response}");
    assert_eq!(response["error"]["code"], json!(-32602), "{response}");
    let message = response["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("meerkat_config_path"), "{response}");
    assert!(message.contains("dangling.toml"), "{response}");
    assert!(message.contains("references unknown server"), "{response}");
}

/// With the acknowledgement the wildcard bind succeeds. `http_base_url` is
/// the same-host loopback form with the kernel-assigned port (never `:0`),
/// and the advertised base rides beside it with its trailing slash dropped.
#[test]
fn meta_gateway_binds_a_non_loopback_http_listen_with_allow_remote() {
    let response = init_once(
        json!({
            "http_listen": "0.0.0.0:0",
            "allow_remote": true,
            "http_public_base_url": "https://mob.example.com/"
        }),
        Duration::from_secs(90),
    );
    assert!(
        response.get("error").is_none(),
        "an acknowledged wildcard bind must succeed: {response}"
    );
    let base = response["result"]["http_base_url"]
        .as_str()
        .unwrap_or_default();
    assert!(base.starts_with("http://127.0.0.1:"), "{response}");
    assert!(!base.ends_with(":0"), "{response}");
    assert_eq!(
        response["result"]["http_public_base_url"],
        json!("https://mob.example.com"),
        "{response}"
    );
}

const RPC_MOB_CONFIG: &str =
    "[mob]\nid = \"http-exposure-test\"\n\n[profiles.default]\nmodel = \"gpt-5.5\"\n";

/// The same two facts on the SDK gateway: spawn `rpc_gateway --persistent`,
/// send one `meta/init` carrying `runtime_options`, return the reply.
fn rpc_init_once(runtime_options: Value, timeout: Duration) -> Value {
    let workspace = TempDir::new().expect("workspace tempdir");
    rpc_init_in(
        workspace.path(),
        RPC_MOB_CONFIG,
        json!({}),
        runtime_options,
        timeout,
    )
}

/// [`rpc_init_once`] with the mob definition text, extra TOP-LEVEL init
/// params (`top_level`, an object) and the workspace under the test's control.
fn rpc_init_in(
    workspace: &Path,
    mob_config: &str,
    top_level: Value,
    runtime_options: Value,
    timeout: Duration,
) -> Value {
    let bin = env!("CARGO_BIN_EXE_rpc_gateway");
    let mut params = match top_level {
        Value::Object(map) => map,
        other => panic!("top-level init params must be a JSON object, got {other}"),
    };
    params.insert("mob_config".into(), json!(mob_config));
    params.insert("runtime_options".into(), runtime_options);
    let init = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "meta/init",
        "params": Value::Object(params),
    });

    let mut child = Command::new(bin)
        .arg("--persistent")
        .current_dir(workspace)
        .env("ANTHROPIC_API_KEY", "sk-ant-regression-test")
        .env("OPENAI_API_KEY", "sk-regression-test")
        .env("XDG_STATE_HOME", workspace.join("xdg-state"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn rpc_gateway");

    let mut stdin = child.stdin.take().expect("stdin");
    writeln!(stdin, "{}", serde_json::to_string(&init).unwrap()).expect("write init");
    stdin.flush().expect("flush");

    let stdout = child.stdout.take().expect("stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let _ = reader.read_line(&mut line);
        let _ = tx.send(line);
    });
    let line = rx
        .recv_timeout(timeout)
        .expect("rpc_gateway did not answer meta/init in time");
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    serde_json::from_str(line.trim())
        .unwrap_or_else(|e| panic!("non-JSON init response {:?}: {}", line, e))
}

/// `rpc_gateway`: a port another process holds is refused at init, before
/// any bootstrap: `-32603` on the request id naming the address, and the
/// declared `persistent_state` directory was never created (the session
/// store, identity substrate and schedule executor lease all live there).
#[test]
fn rpc_gateway_refuses_a_taken_fixed_port_at_init_without_bootstrapping() {
    let holder = std::net::TcpListener::bind("127.0.0.1:0").expect("hold a loopback port");
    let taken = holder.local_addr().expect("held address");
    let workspace = TempDir::new().expect("workspace tempdir");
    let state = workspace.path().join("state");

    let response = rpc_init_in(
        workspace.path(),
        RPC_MOB_CONFIG,
        json!({ "persistent_state": state.to_string_lossy() }),
        json!({ "http_listen": taken.to_string() }),
        Duration::from_secs(45),
    );

    assert!(
        response.get("result").is_none(),
        "a taken port must not produce a runtime: {response}"
    );
    assert_eq!(response["id"], json!(1), "{response}");
    assert_eq!(response["error"]["code"], json!(-32603), "{response}");
    let message = response["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains(&taken.to_string()), "{response}");
    assert!(message.contains("failed to bind"), "{response}");
    assert!(
        !state.exists(),
        "a refused bind must not have bootstrapped anything under {}",
        state.display()
    );
    drop(holder);
}

/// `RPC_MOB_CONFIG` with the default profile's model pin moved: a composition
/// change an operator could make between restarts.
const RPC_MOB_CONFIG_REPINNED: &str =
    "[mob]\nid = \"http-exposure-test\"\n\n[profiles.default]\nmodel = \"gpt-5.4\"\n";

/// The data of a composition provenance refusal, after asserting the reply is
/// that refusal on the request id: code `-32019`, not the generic `-32603`.
fn composition_refusal(response: &Value) -> &Value {
    assert!(
        response.get("result").is_none(),
        "a refused composition must not produce a runtime: {response}"
    );
    assert_eq!(response["id"], json!(1), "{response}");
    assert_eq!(
        response["error"]["code"],
        json!(meerkat_meta::COMPOSITION_PROVENANCE_CODE),
        "{response}"
    );
    assert_eq!(meerkat_meta::COMPOSITION_PROVENANCE_CODE, -32019);
    &response["error"]["data"]
}

/// `rpc_gateway`: a composition provenance refusal at init is its own typed
/// code with the refusal as data, so an SDK can show the diverged fields
/// without parsing the message. A persistent state is created, then booted
/// with a moved model pin: authoritatively (refused as `divergent`) and as a
/// candidate (refused as `candidate_divergent`).
#[test]
fn rpc_gateway_refuses_a_diverged_composition_with_a_typed_code_and_its_fields() {
    let workspace = TempDir::new().expect("workspace tempdir");
    let state = workspace.path().join("state");
    let top_level = json!({ "persistent_state": state.to_string_lossy() });

    let created = rpc_init_in(
        workspace.path(),
        RPC_MOB_CONFIG,
        top_level.clone(),
        json!({}),
        Duration::from_secs(90),
    );
    assert!(
        created.get("result").is_some(),
        "the first boot creates: {created}"
    );

    let authoritative = rpc_init_in(
        workspace.path(),
        RPC_MOB_CONFIG_REPINNED,
        top_level.clone(),
        json!({}),
        Duration::from_secs(45),
    );
    let data = composition_refusal(&authoritative);
    assert_eq!(data["kind"], json!("divergent"), "{authoritative}");
    let fields = data["fields"].as_array().expect("fields is a list");
    assert!(
        fields.iter().any(|field| field
            .as_str()
            .is_some_and(|f| f.starts_with("profiles.default"))),
        "the diverged field paths: {authoritative}"
    );
    assert!(data["manifest"].is_string(), "{authoritative}");
    assert!(
        authoritative["error"]["message"]
            .as_str()
            .is_some_and(|message| message.starts_with("Runtime bootstrap failed: ")),
        "the message is unchanged: {authoritative}"
    );

    let candidate = rpc_init_in(
        workspace.path(),
        RPC_MOB_CONFIG_REPINNED,
        top_level,
        json!({ "mob_composition": { "authority": "candidate" } }),
        Duration::from_secs(45),
    );
    let data = composition_refusal(&candidate);
    assert_eq!(data["kind"], json!("candidate_divergent"), "{candidate}");
    let fields = data["fields"].as_array().expect("fields is a list");
    assert!(
        fields.iter().any(|field| field
            .as_str()
            .is_some_and(|f| f.starts_with("profiles.default"))),
        "the diverged field paths: {candidate}"
    );
}

/// `rpc_gateway`: the four exposure/config options live inside
/// `runtime_options` here; the top-level shape (meta_gateway's) is refused
/// with `-32602` naming their home, never bound to loopback in silence.
#[test]
fn rpc_gateway_refuses_top_level_exposure_options_by_naming_runtime_options() {
    let workspace = TempDir::new().expect("workspace tempdir");
    let response = rpc_init_in(
        workspace.path(),
        RPC_MOB_CONFIG,
        json!({ "http_listen": "0.0.0.0:8080", "allow_remote": true }),
        json!({}),
        Duration::from_secs(45),
    );
    assert!(response.get("result").is_none(), "{response}");
    assert_eq!(response["error"]["code"], json!(-32602), "{response}");
    let message = response["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("runtime_options.http_listen"),
        "{response}"
    );
    assert!(
        message.contains("runtime_options.allow_remote"),
        "{response}"
    );
}

/// `rpc_gateway`: a `[self_hosted]` table in `mob_config` is refused at init
/// with `-32602` pointing at `meerkat_config_path`, not dropped by the
/// definition parser.
#[test]
fn rpc_gateway_refuses_host_config_tables_in_mob_config() {
    let workspace = TempDir::new().expect("workspace tempdir");
    let mob_config = format!(
        "{RPC_MOB_CONFIG}\n[self_hosted.servers.local]\nbase_url = \"http://127.0.0.1:11434\"\n"
    );
    let response = rpc_init_in(
        workspace.path(),
        &mob_config,
        json!({}),
        json!({}),
        Duration::from_secs(45),
    );
    assert!(response.get("result").is_none(), "{response}");
    assert_eq!(response["error"]["code"], json!(-32602), "{response}");
    let message = response["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("[self_hosted]"), "{response}");
    assert!(message.contains("meerkat_config_path"), "{response}");
}

/// `rpc_gateway`: a host config that parses but fails meerkat's own
/// `Config::validate` (dangling server reference) is `-32602` at init naming
/// the option and the file, the fail-late class the ingress exists to remove.
#[test]
fn rpc_gateway_refuses_a_host_config_that_does_not_validate_by_path() {
    let workspace = TempDir::new().expect("workspace tempdir");
    let config = workspace.path().join("dangling.toml");
    std::fs::write(&config, DANGLING_HOST_CONFIG).expect("write host config");
    let response = rpc_init_in(
        workspace.path(),
        RPC_MOB_CONFIG,
        json!({}),
        json!({ "meerkat_config_path": config.to_string_lossy() }),
        Duration::from_secs(45),
    );
    assert!(response.get("result").is_none(), "{response}");
    assert_eq!(response["error"]["code"], json!(-32602), "{response}");
    let message = response["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("runtime_options.meerkat_config_path"),
        "{response}"
    );
    assert!(message.contains("dangling.toml"), "{response}");
    assert!(message.contains("references unknown server"), "{response}");
}

/// `rpc_gateway`: a non-loopback `runtime_options.http_listen` with neither
/// enforced console auth nor `allow_remote` is a `-32602` init refusal that
/// names the address and the acknowledgement. `console_require_app_auth =
/// false` (the common local opt-out) does not open the gate either.
#[test]
fn rpc_gateway_refuses_a_non_loopback_http_listen_without_allow_remote() {
    let response = rpc_init_once(
        json!({ "http_listen": "0.0.0.0:0", "console_require_app_auth": false }),
        Duration::from_secs(45),
    );
    assert!(
        response.get("result").is_none(),
        "a wildcard bind must not succeed without allow_remote: {response}"
    );
    assert_eq!(response["error"]["code"], json!(-32602), "{response}");
    let message = response["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("0.0.0.0:0"), "{response}");
    assert!(message.contains("allow_remote"), "{response}");
}

/// `rpc_gateway`: with the acknowledgement the wildcard bind succeeds, the
/// reported `http_base_url` is the same-host loopback form with the real
/// port, and `http_public_base_url` echoes the declared base normalised.
#[test]
fn rpc_gateway_binds_a_non_loopback_http_listen_with_allow_remote() {
    let response = rpc_init_once(
        json!({
            "http_listen": "0.0.0.0:0",
            "allow_remote": true,
            "http_public_base_url": "https://mob.example.com/",
            "console_require_app_auth": false
        }),
        Duration::from_secs(90),
    );
    assert!(
        response.get("error").is_none(),
        "an acknowledged wildcard bind must succeed: {response}"
    );
    let base = response["result"]["http_base_url"]
        .as_str()
        .unwrap_or_default();
    assert!(base.starts_with("http://127.0.0.1:"), "{response}");
    assert!(!base.ends_with(":0"), "{response}");
    assert_eq!(
        response["result"]["http_public_base_url"],
        json!("https://mob.example.com"),
        "{response}"
    );
}

/// The default launch on both binaries is byte-for-byte what every earlier
/// release reported: a loopback `http_base_url` and, new but inert, a null
/// `http_public_base_url`.
#[test]
fn both_gateways_default_to_a_loopback_bind_with_no_advertised_base() {
    let rpc = rpc_init_once(json!({}), Duration::from_secs(90));
    assert!(rpc.get("error").is_none(), "{rpc}");
    let base = rpc["result"]["http_base_url"].as_str().unwrap_or_default();
    assert!(base.starts_with("http://127.0.0.1:"), "{rpc}");
    assert!(rpc["result"]["http_public_base_url"].is_null(), "{rpc}");

    let console = init_once(json!({}), Duration::from_secs(90));
    assert!(console.get("error").is_none(), "{console}");
    let base = console["result"]["http_base_url"]
        .as_str()
        .unwrap_or_default();
    assert!(base.starts_with("http://127.0.0.1:"), "{console}");
    assert!(
        console["result"]["http_public_base_url"].is_null(),
        "{console}"
    );
}

/// Names directly under `dir`.
fn dir_entries(dir: &Path) -> std::collections::BTreeSet<String> {
    std::fs::read_dir(dir)
        .expect("read the directory")
        .map(|entry| {
            entry
                .expect("directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

/// `meta_gateway`: a `store_path` naming a separate session database is
/// refused typed (-32014) before any store opens, whether or not the file
/// exists. Sessions live in `runtime.sqlite` under the state directory, so
/// the refusal names the file, the state directory and the remedy steps in
/// order, and leaves the directory as it found it. Control: the same
/// directory passed as `store_path` starts.
#[test]
fn meta_gateway_refuses_a_session_database_store_path_before_any_store_opens() {
    for exists in [true, false] {
        let workspace = TempDir::new().expect("workspace tempdir");
        let state = TempDir::new().expect("state tempdir");
        let state_dir = std::fs::canonicalize(state.path()).expect("canonical state dir");
        let file = state_dir.join("my-sessions.db");
        if exists {
            std::fs::write(&file, b"not opened").expect("write the named file");
        }
        let before = dir_entries(&state_dir);
        let refused = init_at(
            workspace.path(),
            &file,
            json!({ "persistent_sessions": true }),
            Duration::from_secs(45),
        );
        assert!(refused.get("result").is_none(), "{refused}");
        assert_eq!(
            refused["error"]["code"],
            json!(meerkat_meta::STORAGE_RESOLUTION_CODE),
            "{refused}"
        );
        let message = refused["error"]["message"].as_str().unwrap_or_default();
        assert!(message.contains(&file.display().to_string()), "{message}");
        assert!(
            message.contains(&state_dir.display().to_string()),
            "{message}"
        );
        assert!(message.contains("runtime.sqlite"), "{message}");
        let import = message
            .find("meta_gateway storage-migrate --state-dir")
            .unwrap_or_else(|| panic!("the import step is missing: {message}"));
        let native = message
            .find("meta_gateway storage-migrate-native --state-dir")
            .unwrap_or_else(|| panic!("the native migration step is missing: {message}"));
        assert!(import < native, "{message}");
        assert_eq!(
            dir_entries(&state_dir),
            before,
            "the refusal created or removed files (file exists: {exists})"
        );
        if exists {
            assert_eq!(
                std::fs::read(&file).expect("read the named file"),
                b"not opened",
                "the refusal opened the named file"
            );
        } else {
            let started = init_at(
                workspace.path(),
                &state_dir,
                json!({ "persistent_sessions": true }),
                Duration::from_secs(45),
            );
            assert!(
                started.get("error").is_none() && started["result"]["http_base_url"].is_string(),
                "the state directory itself must start: {started}"
            );
        }
    }
}

/// A meta_gateway on its own workspace whose `config/mob.toml` names the mob
/// `mob_id` and the roster's member `personal:alice`, launched with the
/// persistent identity-first init plus `overrides` and the flags `args`.
#[cfg(unix)]
struct ComposedGateway {
    child: std::process::Child,
    _stdin: std::process::ChildStdin,
    http_base_url: String,
    init: Value,
}

#[cfg(unix)]
impl ComposedGateway {
    /// `Err` carries the init answer of a gateway that refused to start.
    fn launch(
        workspace: &Path,
        mob_id: &str,
        overrides: Value,
        args: &[&str],
    ) -> Result<Self, Value> {
        let config = workspace.join("config");
        std::fs::create_dir_all(&config).expect("config dir");
        std::fs::write(
            config.join("mob.toml"),
            format!(
                "[mob]\nid = \"{mob_id}\"\norchestrator = \"alpha\"\n\n[profiles.alpha]\nmodel = \
                 \"gpt-5.5\"\nexternal_addressable = true\n\n[profiles.alpha.tools]\ncomms = true\n"
            ),
        )
        .expect("mob.toml");
        let mut params = json!({
            "workspace_root": workspace,
            "store_path": workspace.join("store"),
            "persistent_sessions": true,
            "identity_roster": [{ "identity": "personal:alice", "profile": "alpha" }],
        });
        for (key, value) in overrides.as_object().expect("init overrides") {
            params[key.as_str()] = value.clone();
        }
        let mut child = Command::new(env!("CARGO_BIN_EXE_meta_gateway"))
            .args(args)
            .current_dir(workspace)
            .env("ANTHROPIC_API_KEY", "sk-ant-regression-test")
            .env("OPENAI_API_KEY", "sk-regression-test")
            .env("XDG_STATE_HOME", workspace.join("xdg-state"))
            .env("RUST_LOG", "error")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn meta_gateway");
        let mut stdin = child.stdin.take().expect("gateway stdin");
        let stdout = child.stdout.take().expect("gateway stdout");
        writeln!(
            stdin,
            "{}",
            json!({ "jsonrpc": "2.0", "id": 1, "method": "meta/init", "params": params })
        )
        .expect("write init");
        stdin.flush().expect("flush init");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = BufReader::new(stdout).read_line(&mut line);
            let _ = tx.send(line);
        });
        let line = rx
            .recv_timeout(Duration::from_mins(1))
            .expect("init answer");
        let mut gateway = Self {
            child,
            _stdin: stdin,
            http_base_url: String::new(),
            init: Value::Null,
        };
        let response: Value = serde_json::from_str(&line).expect("init JSON");
        if response.get("error").is_some() {
            return Err(response);
        }
        gateway.http_base_url = response["result"]["http_base_url"]
            .as_str()
            .expect("HTTP base URL")
            .to_owned();
        gateway.init = response["result"].clone();
        Ok(gateway)
    }

    /// The console's whole answer to one JSON-RPC call: result or error.
    fn call(&self, method: &str, params: Value) -> Value {
        reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("HTTP client")
            .post(format!("{}/console/rpc", self.http_base_url))
            .json(&json!({ "jsonrpc": "2.0", "id": 2, "method": method, "params": params }))
            .send()
            .expect("HTTP RPC")
            .json()
            .expect("HTTP RPC JSON")
    }
}

#[cfg(unix)]
impl Drop for ComposedGateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// meta_gateway composes mob announcements from its init's
/// `mob_announcements` and serves them on `/console/rpc`: a subscription put
/// there is listed there.
#[cfg(unix)]
#[test]
fn meta_gateway_serves_mob_announcements_on_its_console() {
    let workspace = TempDir::new().expect("workspace");
    let gateway = ComposedGateway::launch(
        workspace.path(),
        "meta-announcing",
        json!({ "mob_announcements": { "enabled": true } }),
        &[],
    )
    .expect("init with mob announcements");
    let member = json!({ "mob_id": "meta-announcing", "identity": "personal:alice" });
    let put = gateway.call(
        "meta/mob_announcements/subscriptions/put",
        json!({
            "operation_id": "op-1",
            "subscription_id": "sub-1",
            "member": member,
            "topics": ["t"]
        }),
    );
    assert_eq!(put["result"]["subscription"]["state"], "active", "{put}");
    assert_eq!(
        put["result"]["subscription"]["selector"]["mobs"],
        json!(["meta-announcing"]),
        "a subscription that names no mob takes the gateway's own: {put}"
    );
    let listed = gateway.call(
        "meta/mob_announcements/subscriptions/list",
        json!({ "member": member }),
    );
    assert_eq!(
        listed["result"]["subscriptions"].as_array().map(Vec::len),
        Some(1),
        "{listed}"
    );
}

/// A read-only console refuses the announcement methods that change state
/// and still answers the reads.
#[cfg(unix)]
#[test]
fn a_read_only_meta_gateway_console_refuses_announcement_changes() {
    let workspace = TempDir::new().expect("workspace");
    let gateway = ComposedGateway::launch(
        workspace.path(),
        "meta-read-only",
        json!({ "mob_announcements": { "enabled": true }, "console_read_only": true }),
        &[],
    )
    .expect("init with mob announcements");
    let put = gateway.call(
        "meta/mob_announcements/subscriptions/put",
        json!({
            "operation_id": "op-1",
            "subscription_id": "sub-1",
            "member": { "mob_id": "meta-read-only", "identity": "personal:alice" },
            "topics": ["t"]
        }),
    );
    assert_eq!(put["error"]["data"]["kind"], "read_only", "{put}");
    let listed = gateway.call("meta/mob_announcements/subscriptions/list", json!({}));
    assert_eq!(listed["result"]["subscriptions"], json!([]), "{listed}");
}

/// Without `mob_announcements`, meta_gateway answers the family exactly as
/// rpc_gateway does when they are off: the one handler both dispatch to.
#[cfg(unix)]
#[test]
fn meta_gateway_without_mob_announcements_answers_as_rpc_gateway_does() {
    let workspace = TempDir::new().expect("workspace");
    let gateway = ComposedGateway::launch(workspace.path(), "meta-plain", json!({}), &[])
        .expect("init without mob announcements");
    let unavailable = serde_json::to_value(
        meerkat_meta::mob_announcements::rpc::announcements_unavailable_error(),
    )
    .expect("error JSON");
    for method in [
        "meta/mob_announcements/subscriptions/list",
        "meta/mob_announcements/subscriptions/put",
    ] {
        let answer = gateway.call(method, json!({}));
        assert_eq!(answer["error"], unavailable, "{method}: {answer}");
    }
}

/// meta_gateway refuses at init the mob announcements it cannot compose,
/// through the parser and refusals rpc_gateway uses.
#[cfg(unix)]
#[test]
fn meta_gateway_refuses_mob_announcements_it_cannot_compose() {
    for (overrides, refusal) in [
        (
            json!({ "mob_announcements": { "enabled": true }, "identity_first": false }),
            "identity_first: false",
        ),
        (
            json!({ "mob_announcements": { "enabled": true }, "persistent_sessions": false }),
            "persistent_sessions: true",
        ),
        (
            json!({ "mob_announcements": { "enabled": true, "retention_ms": 0 } }),
            "mob_announcements.retention_ms must be at least",
        ),
    ] {
        let workspace = TempDir::new().expect("workspace");
        let Err(answer) =
            ComposedGateway::launch(workspace.path(), "meta-refused", overrides.clone(), &[])
        else {
            panic!("{overrides} must be refused at init");
        };
        assert!(
            answer.to_string().contains(refusal),
            "{overrides}: {answer}"
        );
    }
}

/// meta_gateway's control listener serves keyed inject status reads with the
/// default window: there is no switch that turns it off. A peer granted
/// `inject_status` on a roster member gets a signed answer.
#[cfg(unix)]
#[test]
fn meta_gateway_serves_keyed_inject_status_on_its_control_listener() {
    use meerkat_meta::contact_directory::{ContactEntry, MobTransport};
    use meerkat_meta::runtime::cross_mob_remote::RemoteMobProxy;
    use meerkat_meta::runtime::keyed_inject::KeyedInjectCallOutcome;

    let workspace = TempDir::new().expect("workspace");
    let gateway_home = workspace.path().join("xdg-state").join("meerkat-meta");
    std::fs::create_dir_all(&gateway_home).expect("gateway home");
    let server_keys =
        meerkat_meta::GatewayPeerKeys::load_or_create(&gateway_home).expect("gateway keys");
    let caller = std::sync::Arc::new(meerkat_meta::GatewayPeerKeys::ephemeral());
    std::fs::create_dir_all(workspace.path().join("config")).expect("config dir");
    std::fs::write(
        workspace.path().join("config").join("contacts.toml"),
        format!(
            "[control_grants.peer-gateway]\npubkey = \"{}\"\nverbs = [\"inject_status\"]\nmembers \
             = [\"personal:alice\"]\n",
            caller.pubkey_b64()
        ),
    )
    .expect("contacts.toml");
    let gateway = ComposedGateway::launch(
        workspace.path(),
        "meta-keyed",
        json!({}),
        &["--control-listen", "tcp://127.0.0.1:0"],
    )
    .expect("init with a control listener");
    let address = gateway.init["control_listen_address"]
        .as_str()
        .unwrap_or_else(|| panic!("a bound control listener: {}", gateway.init))
        .to_owned();
    let entry = ContactEntry {
        mob_id: "meta-keyed".to_string(),
        transport: MobTransport::Tcp(
            address
                .strip_prefix("tcp://")
                .expect("tcp listener address")
                .to_string(),
        ),
        pubkey: Some(server_keys.pubkey_bytes()),
        require_signed_control: Some(true),
    };
    let proxy = RemoteMobProxy::from_entry_with_caller(&entry, Some(caller))
        .expect("valid contact")
        .expect("tcp proxy");
    let now_ms = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after the epoch")
            .as_millis(),
    )
    .expect("milliseconds fit");
    let outcome = tokio::runtime::Runtime::new()
        .expect("tokio runtime")
        .block_on(proxy.inject_status("personal:alice", "batch-1", false, now_ms))
        .expect("a signed answer");
    assert!(
        matches!(outcome, KeyedInjectCallOutcome::Answered(_)),
        "{outcome:?}"
    );
}

/// meta_gateway takes the ceiling of the member tools in child mobs from
/// its init, with announcements on or off.
#[cfg(unix)]
#[test]
fn meta_gateway_takes_the_member_tools_ceiling_for_child_mobs() {
    for enabled in [true, false] {
        let workspace = TempDir::new().expect("workspace");
        let gateway = ComposedGateway::launch(
            workspace.path(),
            "meta-ceiling",
            json!({ "mob_announcements": { "enabled": enabled, "member_tools_in_child_mobs": true } }),
            &[],
        )
        .unwrap_or_else(|answer| panic!("the ceiling is an option, not a refusal: {answer}"));
        let listed = gateway.call("meta/mob_announcements/subscriptions/list", json!({}));
        if enabled {
            assert_eq!(listed["result"]["subscriptions"], json!([]), "{listed}");
        } else {
            assert_eq!(
                listed["error"]["data"]["kind"], "mob_announcements_unavailable",
                "{listed}"
            );
        }
    }
}

/// `meta_gateway`: an existing directory is the state directory even with a
/// dot in its name (`/srv/meta.d`). It is never read as a session-database
/// override: it starts and holds the runtime store.
#[test]
fn meta_gateway_starts_an_existing_dotted_state_directory() {
    let workspace = TempDir::new().expect("workspace tempdir");
    let root = TempDir::new().expect("root tempdir");
    let dotted = root.path().join("meta.d");
    std::fs::create_dir(&dotted).expect("create the dotted directory");
    let started = init_at(
        workspace.path(),
        &dotted,
        json!({ "persistent_sessions": true }),
        Duration::from_secs(45),
    );
    assert!(
        started.get("error").is_none() && started["result"]["http_base_url"].is_string(),
        "an existing dotted directory must start: {started}"
    );
    assert!(
        dotted.join("runtime.sqlite").is_file(),
        "the dotted directory is the state directory"
    );
}

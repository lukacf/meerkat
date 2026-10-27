//! A deployment's lineage after the supported route: a store the released MobKit
//! 0.8.26 rpc_gateway minted, adopted by Meta 0.9.2 and run on it, then
//! started once by the released Meta 0.9.3 rpc_gateway (the route for any
//! directory older than 0.9.3), migrates with
//! `meta_gateway storage-migrate-native --apply` and boots on the build
//! under test with its MobKit-minted composition manifest, whose creator field
//! still reads `created_by_mobkit`.
//!
//! The capture keeps no comms identity keys (they live outside the state
//! directory, see tests/fixtures/README.md), so the pins hold without them:
//! Core's binding names the migrated session, memory recall returns the
//! released memories, the event log and the memory ledger read back, the
//! roster lists the identity once, and Core's session store holds the
//! released transcript. A candidate and an authoritative launch over the
//! migrated store report the same identity facts.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args
)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[path = "support/gateway_shutdown.rs"]
mod gateway_shutdown;

// ---------------------------------------------------------------------------
// The capture
// ---------------------------------------------------------------------------

/// The released definition, byte for byte (0.9 ignores `tools.mob_tasks`).
const MOB_CONFIG: &str = r#"[mob]
id = "adoption-fixture-planners"

[profiles.planner]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.planner.tools]
comms = true
mob_tasks = true
"#;
/// The durable identity the roster declares.
const IDENTITY: &str = "learner:fixture-0001";
/// Its member id in the event log.
const MEMBER_ID: &str = "mk--learner_cfixture-0001";
/// The session the identity has been bound to since the 0.8.26 mint.
const SESSION_ID: &str = "01a15646-b33f-7ee2-b9d9-b4a5b2d1ad36";
/// The two agent memories the 0.8.26 mint stored for the identity.
const MEMORIES: [(&str, &str); 2] = [
    (
        "mem-1792448836736361251-1382c4-0-18395066da7f0be3",
        "FIXTURE-0826-MEMORY-ONE: the synthetic learner prefers short lessons.",
    ),
    (
        "mem-1792448836738719869-1382c4-1-6e92bf82a622ec38",
        "FIXTURE-0826-MEMORY-TWO: the synthetic learner finished unit one.",
    ),
];
/// The marker of the 0.8.26 mint's one turn.
const TURN_TOKEN: &str = "FIXTURE-0826-TOKEN-ALPHA";
/// The memory ledger assertion the 0.8.26 mint indexed.
const LEDGER_ENTITY: &str = "fixture-curriculum";
const LEDGER_ASSERTION_ID: &str = "memory-assert-000000";

const MANIFEST_FILE: &str = "event_log.sqlite3.composition.json";
const PREDECESSOR_CREATOR_KEY: &str = "\"created_by_mobkit\"";

const INIT_TIMEOUT: Duration = Duration::from_mins(3);
const RPC_TIMEOUT: Duration = Duration::from_mins(1);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_mins(3);
const EXIT_TIMEOUT: Duration = Duration::from_mins(1);

fn roster_spec() -> Value {
    json!({
        "identity": IDENTITY,
        "profile": "planner",
        "addressability": "addressable",
        "labels": { "cohort": "fixture" },
        "additional_instructions": ["Answer in one sentence."]
    })
}

// ---------------------------------------------------------------------------
// Fixture staging
// ---------------------------------------------------------------------------

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/released_0_9_3_started_0_9_2_adopted_store")
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn files_under(root: &Path, dir: &Path, out: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).expect("read fixture dir") {
        let path = entry.expect("fixture dir entry").path();
        if path.is_dir() {
            files_under(root, &path, out);
        } else {
            out.push(
                path.strip_prefix(root)
                    .expect("fixture file under the fixture root")
                    .to_string_lossy()
                    .replace(std::path::MAIN_SEPARATOR, "/"),
            );
        }
    }
}

/// The committed store is exactly the capture: every file listed in
/// `checksums.sha256` with its digest, and nothing else. Hashing reads the
/// files; it never opens them as databases.
fn assert_fixture_is_the_capture() {
    let root = fixture_dir();
    let listing =
        std::fs::read_to_string(root.join("checksums.sha256")).expect("read fixture checksums");
    let mut pinned = Vec::new();
    for line in listing.lines() {
        let (digest, file) = line
            .split_once("  ")
            .unwrap_or_else(|| panic!("malformed checksum line {line:?}"));
        let bytes = std::fs::read(root.join(file))
            .unwrap_or_else(|error| panic!("read fixture file {file}: {error}"));
        assert_eq!(
            sha256_hex(&bytes),
            digest,
            "fixture file {file} no longer holds the captured bytes; restore it from git and \
             never open fixture files in place"
        );
        pinned.push(file.to_string());
    }
    let mut present = Vec::new();
    files_under(&root, &root.join("state"), &mut present);
    pinned.sort();
    present.sort();
    assert_eq!(
        present, pinned,
        "the fixture state holds files outside its checksums"
    );
}

fn copy_dir_recursive(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create staged dir");
    for entry in std::fs::read_dir(from).expect("read fixture dir") {
        let entry = entry.expect("fixture dir entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("fixture entry type").is_dir() {
            copy_dir_recursive(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copy fixture file");
        }
    }
}

/// Stage one pristine copy of the capture into `<temp>/state`.
fn stage_store(temp: &Path) -> PathBuf {
    assert_fixture_is_the_capture();
    let state = temp.join("state");
    copy_dir_recursive(&fixture_dir().join("state"), &state);
    // The capture holds an EMPTY `blobs/` directory, which git cannot track.
    std::fs::create_dir_all(state.join("blobs")).expect("restore empty blobs dir");
    state
}

fn manifest_text(state: &Path) -> String {
    std::fs::read_to_string(state.join(MANIFEST_FILE)).expect("read the staged manifest")
}

// ---------------------------------------------------------------------------
// Maintenance verbs (each a process of the build under test)
// ---------------------------------------------------------------------------

struct VerbRun {
    code: i32,
    stdout: String,
    stderr: String,
}

fn run_binary(binary: &str, args: &[&str], home: &Path) -> VerbRun {
    let output = Command::new(binary)
        .args(args)
        .env("HOME", home)
        .env("XDG_STATE_HOME", home.join(".local").join("state"))
        .output()
        .unwrap_or_else(|error| panic!("spawn {binary}: {error}"));
    VerbRun {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// One `meta_gateway storage-migrate-native --state-dir <state> --json` run,
/// with `--apply` when `apply`; it must exit 0. Returns its stdout.
fn migrate_native(state: &Path, home: &Path, apply: bool) -> String {
    let state_arg = state.to_str().expect("utf-8 state path");
    let mut args = vec!["storage-migrate-native", "--state-dir", state_arg, "--json"];
    if apply {
        args.push("--apply");
    }
    let run = run_binary(env!("CARGO_BIN_EXE_meta_gateway"), &args, home);
    assert_eq!(
        run.code, 0,
        "storage-migrate-native (apply={apply}) failed\nstdout: {}\nstderr: {}",
        run.stdout, run.stderr
    );
    run.stdout
}

// ---------------------------------------------------------------------------
// The gateway under test
// ---------------------------------------------------------------------------

struct Gateway {
    label: String,
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
    stderr: Arc<Mutex<Vec<String>>>,
}

impl Gateway {
    /// A cold `rpc_gateway --persistent` with a private HOME, so nothing it
    /// derives ambiently (session comms identities, gateway home) touches the
    /// invoking user's state.
    fn start(label: &str, home: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_rpc_gateway"))
            .arg("--persistent")
            .env("HOME", home)
            .env("XDG_STATE_HOME", home.join(".local").join("state"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn rpc_gateway --persistent");
        let stdin = child.stdin.take().expect("gateway stdin");
        let stdout = child.stdout.take().expect("gateway stdout");
        let child_stderr = child.stderr.take().expect("gateway stderr");
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&stderr);
        thread::spawn(move || {
            for line in BufReader::new(child_stderr).lines() {
                let Ok(line) = line else { break };
                let Ok(mut lines) = sink.lock() else { break };
                if lines.len() < 2000 {
                    lines.push(line);
                }
            }
        });
        Self {
            label: label.to_string(),
            child,
            stdin: Some(stdin),
            lines: rx,
            stderr,
        }
    }

    fn stderr_tail(&self) -> String {
        let Ok(lines) = self.stderr.lock() else {
            return "[gateway stderr unavailable]".to_string();
        };
        let start = lines.len().saturating_sub(40);
        format!(
            "--- [{}] gateway stderr (last {} lines) ---\n{}",
            self.label,
            lines.len() - start,
            lines[start..].join("\n")
        )
    }

    fn send(&mut self, value: &Value) {
        let stdin = self.stdin.as_mut().expect("gateway stdin open");
        writeln!(stdin, "{value}").expect("write to gateway stdin");
        stdin.flush().expect("flush gateway stdin");
    }

    /// One request; the response, error or not. Host callbacks are answered
    /// while waiting: this host provides the roster and nothing else.
    fn request(&mut self, id: &str, method: &str, params: Value, deadline: Duration) -> Value {
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        let start = Instant::now();
        loop {
            let remaining = deadline.checked_sub(start.elapsed()).unwrap_or_else(|| {
                panic!(
                    "[{}] no response to {method} within {deadline:?}\n{}",
                    self.label,
                    self.stderr_tail()
                )
            });
            let Ok(line) = self.lines.recv_timeout(remaining) else {
                panic!(
                    "[{}] the gateway closed or stalled before answering {method}\n{}",
                    self.label,
                    self.stderr_tail()
                );
            };
            let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            if let Some(callback) = message.get("method").and_then(Value::as_str) {
                let callback = callback.to_string();
                self.answer_callback(&callback, &message);
                continue;
            }
            if message.get("id").and_then(Value::as_str) == Some(id) {
                return message;
            }
        }
    }

    /// One request that must succeed; its result.
    fn call(&mut self, id: &str, method: &str, params: Value) -> Value {
        let response = self.request(id, method, params, RPC_TIMEOUT);
        assert!(
            response.get("error").is_none(),
            "[{}] {method} failed: {response}\n{}",
            self.label,
            self.stderr_tail()
        );
        response["result"].clone()
    }

    fn answer_callback(&mut self, method: &str, message: &Value) {
        let Some(id) = message.get("id").cloned() else {
            return;
        };
        if method == "callback/roster_provider/roster" {
            self.send(&json!({ "jsonrpc": "2.0", "id": id, "result": [roster_spec()] }));
            return;
        }
        assert!(
            !method.starts_with("callback/"),
            "[{}] unexpected host callback {method}: this host provides only the roster",
            self.label
        );
    }

    fn shutdown_and_reap(mut self) {
        let response = self.request("shutdown", "meta/shutdown", json!({}), SHUTDOWN_TIMEOUT);
        gateway_shutdown::assert_persistent_shutdown_completed(
            &response,
            &format!("[{}] shutdown\n{}", self.label, self.stderr_tail()),
        );
        drop(self.stdin.take());
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().expect("poll gateway exit") {
                assert!(
                    status.success(),
                    "[{}] rpc_gateway exited with {status}\n{}",
                    self.label,
                    self.stderr_tail()
                );
                return;
            }
            assert!(
                start.elapsed() < EXIT_TIMEOUT,
                "[{}] rpc_gateway did not exit\n{}",
                self.label,
                self.stderr_tail()
            );
            thread::sleep(Duration::from_millis(25));
        }
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Which composition authority a boot declares.
#[derive(Clone, Copy)]
enum Launch {
    /// The default: the launch speaks for the durable composition, so a
    /// resume over the persistent, non-empty event log verifies the supplied
    /// definition against the store through its composition manifest.
    Authoritative,
    /// `runtime_options.mob_composition.authority = "candidate"`: the resume
    /// boots the stored definition and refuses a supplied one that differs,
    /// after the released-representation allowance the manifest enables.
    Candidate,
}

/// Boot the build under test over the migrated `state` the way the captured
/// deployment ran: persistent mob storage over its non-empty event log, the
/// same roster, agent memory and memory ledger configuration, and the
/// released definition.
fn boot(label: &str, state: &Path, home: &Path, launch: Launch) -> Gateway {
    let mut gateway = Gateway::start(label, home);
    let response = gateway.request(
        "init",
        "meta/init",
        init_params(state, launch),
        INIT_TIMEOUT,
    );
    assert!(
        response.get("error").is_none(),
        "[{label}] meta/init over the migrated capture was refused: {response}\n{}",
        gateway.stderr_tail()
    );
    gateway
}

/// The `meta/init` params of a boot of the captured deployment.
fn init_params(state: &Path, launch: Launch) -> Value {
    let mut runtime_options = json!({
        "demo_llm": true,
        "identity_bootstrap_mode": { "mode": "eager_materialize" },
        "agent_memory": { "selection": "contextual", "max_entries": 4 },
        "memory_config": { "backend": "local_json" }
    });
    if matches!(launch, Launch::Candidate) {
        runtime_options["mob_composition"] = json!({ "authority": "candidate" });
    }
    json!({
        "persistent_state": state,
        "mob_config": MOB_CONFIG,
        "has_roster_provider": true,
        "runtime_options": runtime_options
    })
}

/// A boot of the unmigrated capture is refused before any store opens:
/// STORAGE_RESOLUTION, naming the native migration.
fn assert_unmigrated_boot_refused(label: &str, state: &Path, home: &Path, launch: Launch) {
    let mut gateway = Gateway::start(label, home);
    let response = gateway.request(
        "init",
        "meta/init",
        init_params(state, launch),
        INIT_TIMEOUT,
    );
    let Some(error) = response.get("error").cloned() else {
        panic!(
            "[{label}] meta/init over the unmigrated capture must be refused: {response}\n{}",
            gateway.stderr_tail()
        );
    };
    drop(gateway.stdin.take());
    let start = Instant::now();
    while gateway
        .child
        .try_wait()
        .expect("poll gateway exit")
        .is_none()
    {
        assert!(
            start.elapsed() < EXIT_TIMEOUT,
            "[{label}] rpc_gateway did not exit after a refused init and stdin EOF\n{}",
            gateway.stderr_tail()
        );
        thread::sleep(Duration::from_millis(25));
    }
    assert_eq!(
        error["code"],
        json!(meerkat_meta::STORAGE_RESOLUTION_CODE),
        "[{label}] {error}"
    );
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|message| message.contains("storage-migrate-native")),
        "[{label}] the refusal must name the migration: {error}"
    );
}

// ---------------------------------------------------------------------------
// What must hold without the comms keys
// ---------------------------------------------------------------------------

/// Core's binding names the migrated session, recall returns the released
/// memories, the event log and the ledger read back, and the roster lists the
/// identity once. Without its comms keys the member Meta 0.9.3 spawned cannot
/// publish its endpoint when Core restores it, so Core marks it broken; it
/// carries no durable-identity marker, so an authoritative launch's roster
/// pass respawns it onto the same session, and the roster lists it active
/// and without an error. A candidate launch respawns no member: its roster
/// lists the member as Core restored it, broken, for the authoritative
/// launch to respawn.
fn assert_migrated_identity_reads_back(gateway: &mut Gateway, launch: Launch) {
    let label = gateway.label.clone();
    let status = gateway.call(
        "status",
        "meta/status_identity",
        json!({ "identity": IDENTITY }),
    );
    assert_eq!(
        status["session_id"], SESSION_ID,
        "[{label}] the identity was rebound away from its migrated session: {status}"
    );

    let recalled = gateway.call(
        "recall",
        "meta/agent_memory/recall",
        json!({ "identity": IDENTITY, "selection": "always", "max_entries": 8 }),
    );
    let mut records: Vec<(String, String)> = recalled["records"]
        .as_array()
        .unwrap_or_else(|| panic!("[{label}] recall returned no records array: {recalled}"))
        .iter()
        .map(|record| {
            (
                record["memory_id"].as_str().unwrap_or_default().to_string(),
                record["body"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    records.sort();
    let mut expected: Vec<(String, String)> = MEMORIES
        .iter()
        .map(|(id, body)| ((*id).to_string(), (*body).to_string()))
        .collect();
    expected.sort();
    assert_eq!(
        records, expected,
        "[{label}] recall must return exactly the released memories"
    );

    let events = gateway.call("events", "meta/mob_events/query", json!({}));
    assert!(
        events.to_string().contains(MEMBER_ID),
        "[{label}] the member's events are missing from the event log: {events}"
    );

    let ledger = gateway.call(
        "ledger",
        "meta/memory/query",
        json!({ "entity": LEDGER_ENTITY }),
    );
    let assertion_ids: Vec<&str> = ledger["assertions"]
        .as_array()
        .unwrap_or_else(|| panic!("[{label}] no assertions array: {ledger}"))
        .iter()
        .filter_map(|assertion| assertion["assertion_id"].as_str())
        .collect();
    assert_eq!(
        assertion_ids,
        vec![LEDGER_ASSERTION_ID],
        "[{label}] the memory ledger assertion must read back: {ledger}"
    );

    // The wire names a member by its public alias: list_members decodes the
    // roster id (MEMBER_ID) back to the identity.
    let members = gateway.call("members", "meta/list_members", json!({}));
    let listed = members
        .as_array()
        .unwrap_or_else(|| panic!("[{label}] meta/list_members is not an array: {members}"))
        .iter()
        .filter(|member| member["agent_identity"] == IDENTITY)
        .collect::<Vec<_>>();
    assert_eq!(
        listed.len(),
        1,
        "[{label}] the migrated identity must be listed exactly once: {members}"
    );
    match launch {
        Launch::Authoritative => assert!(
            listed[0]["state"] == "active" && listed[0].get("error").is_none(),
            "[{label}] the migrated identity's member must be active, without an error: \
             {members}"
        ),
        Launch::Candidate => assert_eq!(
            listed[0]["state"], "broken",
            "[{label}] a candidate leaves the migrated identity's member as Core restored it: \
             {members}"
        ),
    }
}

/// The identity facts a launch reports that hold without the comms keys.
fn identity_facts(gateway: &mut Gateway) -> Value {
    let status = gateway.call(
        "status",
        "meta/status_identity",
        json!({ "identity": IDENTITY }),
    );
    json!({
        "session_id": status["session_id"],
        "profile": status["profile"],
        "runtime_mode": status["runtime_mode"],
        "addressability": status["addressability"],
        "labels": status["labels"],
        "intent_managed": status["intent_managed"],
    })
}

/// Durable truth after the gateway exited: Core's session store, on
/// `runtime.sqlite`, holds the migrated session and its released turn.
fn assert_transcript_in_core(state: &Path) {
    use meerkat::SessionStore as _;
    let store = meerkat_store::SqliteSessionStore::open(state.join("runtime.sqlite"))
        .expect("open Core's session store");
    let session_id =
        meerkat_core::types::SessionId::parse(SESSION_ID).expect("the migrated session id");
    let session = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime for the store read")
        .block_on(store.load(&session_id))
        .expect("read the migrated session")
        .expect("the migrated session is in Core's session store");
    let transcript = serde_json::to_string(session.messages()).expect("serialize the transcript");
    assert!(
        transcript.contains(TURN_TOKEN),
        "the released turn is missing from the migrated session's durable transcript"
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// The supported route ends in an authoritative boot of the MobKit-minted
/// manifest: refused before the migration, migrated by the census and its
/// apply, then booted with every key-free fact read back; neither step
/// rewrites the predecessor-spelled manifest.
#[test]
fn released_0_9_3_started_store_migrates_and_boots_its_mobkit_manifest_authoritatively() {
    let temp = tempfile::tempdir().expect("tempdir");
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).expect("home");
    let state = stage_store(temp.path());
    let manifest = manifest_text(&state);
    assert_eq!(
        manifest.matches(PREDECESSOR_CREATOR_KEY).count(),
        1,
        "the capture keeps the MobKit-minted creator field: {manifest}"
    );

    assert_unmigrated_boot_refused(
        "0.9.3-started/unmigrated",
        &state,
        &home,
        Launch::Authoritative,
    );
    let census = migrate_native(&state, &home, false);
    assert!(
        census.contains(SESSION_ID),
        "the census must list the identity's session: {census}"
    );
    migrate_native(&state, &home, true);
    assert_eq!(
        manifest_text(&state),
        manifest,
        "the migration never rewrites the manifest"
    );

    let mut gateway = boot(
        "0.9.3-started/authoritative",
        &state,
        &home,
        Launch::Authoritative,
    );
    assert_migrated_identity_reads_back(&mut gateway, Launch::Authoritative);
    gateway.shutdown_and_reap();
    assert_transcript_in_core(&state);
    assert_eq!(
        manifest_text(&state),
        manifest,
        "an authoritative boot never rewrites the manifest"
    );
}

/// Launch parity: a candidate launch and then an authoritative launch over
/// the same migrated store report the same identity facts, and both read the
/// capture back. The member Meta 0.9.3 spawned lacks the durable-identity
/// marker: the candidate does not respawn it and reports its identity
/// deferred as `resume_member`, with a complete bootstrap and no broken
/// identity, while its member detail shows it as Core restored it; the
/// authoritative launch respawns it.
#[test]
fn released_0_9_3_started_store_reports_one_identity_to_candidate_and_authoritative_launches() {
    let temp = tempfile::tempdir().expect("tempdir");
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).expect("home");
    let state = stage_store(temp.path());
    assert_unmigrated_boot_refused(
        "0.9.3-started/candidate-unmigrated",
        &state,
        &home,
        Launch::Candidate,
    );
    migrate_native(&state, &home, true);

    let mut gateway = boot("0.9.3-started/candidate", &state, &home, Launch::Candidate);
    let bootstrap = gateway.call(
        "bootstrap",
        "meta/wait_identity_bootstrap",
        json!({ "timeout_ms": 60_000 }),
    );
    assert_eq!(
        (
            bootstrap["complete"].clone(),
            bootstrap["counts"]["broken"].clone(),
            bootstrap["deferred"].clone(),
        ),
        (
            json!(true),
            json!(0),
            json!([{ "identity": IDENTITY, "reason": "resume_member" }]),
        ),
        "the candidate defers the unmarked member's respawn: {bootstrap}"
    );
    assert_migrated_identity_reads_back(&mut gateway, Launch::Candidate);
    let candidate = identity_facts(&mut gateway);
    gateway.shutdown_and_reap();

    let mut gateway = boot(
        "0.9.3-started/authoritative-after-candidate",
        &state,
        &home,
        Launch::Authoritative,
    );
    let bootstrap = gateway.call(
        "bootstrap",
        "meta/wait_identity_bootstrap",
        json!({ "timeout_ms": 60_000 }),
    );
    assert_eq!(
        (
            bootstrap["complete"].clone(),
            bootstrap["counts"]["broken"].clone(),
            bootstrap.get("deferred").cloned(),
            bootstrap["identities"][IDENTITY]["state"].clone(),
        ),
        (json!(true), json!(0), None, json!("active")),
        "the authoritative launch respawns the member: {bootstrap}"
    );
    assert_migrated_identity_reads_back(&mut gateway, Launch::Authoritative);
    let authoritative = identity_facts(&mut gateway);
    gateway.shutdown_and_reap();
    assert_eq!(
        candidate, authoritative,
        "a candidate and an authoritative launch report the same identity"
    );
    assert_transcript_in_core(&state);
}

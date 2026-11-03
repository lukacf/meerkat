//! Acceptance lane for a persistent store MINTED BY THE RELEASED MobKit
//! 0.8.26 `rpc_gateway`, adopted and resumed by the build under test.
//!
//! The store under `tests/fixtures/released_0_8_26_store/state/` was written
//! by the public-release 0.8.26 gateway, never by current code (see
//! `mint_released_0_8_26_store.py` beside it, and tests/fixtures/README.md):
//! one mob created on persistent SQLite mob storage, so its event log
//! `event_log.sqlite3` carries the mob's events and MobKit's composition
//! manifest `event_log.sqlite3.composition.json` sits beside it with the
//! creator field spelled `created_by_mobkit`; one durable identity with its
//! member session and one driven turn; two agent memories; one memory
//! ledger assertion. The fixture is byte-lossless (`checksums.sha256` pins
//! every file) and is never opened in place: every test works on a staged
//! copy.
//!
//! The upgrade a 0.8.x installation performs, end to end:
//!
//! 1. `meta_gateway storage-adopt-predecessor --state-dir <state>` (dry run)
//!    lists every step, the composition manifest rewrite included, and
//!    changes nothing;
//! 2. `--apply` completes, and the manifest differs from the released bytes
//!    in the creator field's name only;
//! 3. a boot of the build under test over the adopted store is refused
//!    before any store opens (STORAGE_RESOLUTION, -32014, naming
//!    `storage-migrate-native`), for an authoritative and a candidate launch,
//!    with the definition the store was created with byte for byte and
//!    without the `tools.mob_tasks` key 0.9 ignores. Sessions now live in
//!    Core's own stores, and the native migration reads only a directory a
//!    0.9.3 start last wrote: the supported route for a predecessor store is
//!    one start with published 0.9.3, then `storage-migrate-native`. That
//!    route, and the resume after it, are pinned on a 0.9.3-started capture.
//!
//! A store adopted before the manifest step existed (as Meta 0.9.0 left it:
//! adoption complete, manifest still predecessor-spelled) is refused alike,
//! and the refusal leaves its manifest as it was; `meta_repair
//! --composition-manifests` detects and repairs its manifest.
//!
//! Keys 0.8.26 ignored in the definition, and Meta 0.9 applies, never change
//! the store's behaviour silently: adoption given the definition
//! (`--mob-config`) names those the store's current definition does not
//! record and refuses before any change until they are removed or
//! acknowledged (a resume that keeps them refuses as divergent once the store
//! runs on a release that can open it). A key a
//! later definition update recorded is in force and is not named.

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

use meerkat_meta::storage_layout::MetaStorageLayout;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[path = "support/gateway_shutdown.rs"]
mod gateway_shutdown;

// ---------------------------------------------------------------------------
// What the released capture holds (pinned from the capture's own summary)
// ---------------------------------------------------------------------------

/// The exact mob definition the released gateway created the store for.
/// 0.8.26 accepted `tools.mob_tasks` (its profile tool config ignored the
/// unknown key, so the knob never reached the stored definition); 0.9 warns
/// that it ignores it.
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

/// The durable identity the released roster declared.
const IDENTITY: &str = "learner:fixture-0001";

const MANIFEST_FILE: &str = "event_log.sqlite3.composition.json";
const PREDECESSOR_CREATOR_KEY: &str = "\"created_by_mobkit\"";
const CREATOR_KEY: &str = "\"created_by_meta\"";

const INIT_TIMEOUT: Duration = Duration::from_mins(3);
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

/// The released definition without the key 0.9 ignores.
fn mob_config_without_ignored_key() -> String {
    let stripped = MOB_CONFIG.replace("mob_tasks = true\n", "");
    assert_ne!(
        stripped, MOB_CONFIG,
        "the released definition names mob_tasks"
    );
    stripped
}

/// The released definition with three keys 0.8.26 ignored and Meta 0.9
/// applies: a tool deny list, a profile model fallback and a mob-wide model
/// fallback. A deployment that kept them in its config ran without any of
/// them on 0.8.26.
fn mob_config_with_predecessor_ignored_keys() -> String {
    let config = format!(
        "{}deny = [\"shell\"]\n\n[profiles.planner.model_fallback]\nenabled = false\n\n\
         [runtime.model_fallback]\nenabled = false\n",
        MOB_CONFIG
    );
    assert!(config.contains("[profiles.planner.tools]\ncomms = true\nmob_tasks = true\ndeny"));
    config
}

/// The dotted paths of those keys, as a divergence refusal names them.
const PREDECESSOR_IGNORED_KEYS: [&str; 3] = [
    "profiles.planner.model_fallback",
    "profiles.planner.tools.deny",
    "runtime.model_fallback",
];

/// The released definition with only the tool deny list added.
fn mob_config_with_deny() -> String {
    let config = format!("{MOB_CONFIG}deny = [\"shell\"]\n");
    assert!(config.contains("[profiles.planner.tools]\ncomms = true\nmob_tasks = true\ndeny"));
    config
}

/// Advance the stored definition of the staged store to `definition`, as a
/// later release's one-shot definition update does, then wait until its
/// store has closed.
fn advance_stored_definition(state: &Path, definition: &str) {
    let path = state.join("event_log.sqlite3");
    let definition = meerkat_mob::MobDefinition::from_toml(definition).expect("definition");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let storage = meerkat_mob::MobStorage::persistent(&path).expect("open the staged store");
        let receipt = storage
            .update_definition(1, definition)
            .await
            .expect("advance the stored definition");
        assert_eq!(receipt.epoch, 2, "{receipt:?}");
    });
    drop(runtime);
    // The store's connection worker closes on its own thread after the
    // store is dropped; the exclusive fence waits for it.
    drop(
        meerkat_sqlite::ExclusiveFence::acquire(&path, Duration::from_secs(30))
            .expect("the advanced store closed"),
    );
}

// ---------------------------------------------------------------------------
// Fixture staging
// ---------------------------------------------------------------------------

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/released_0_8_26_store")
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

/// The committed store is exactly the released capture: every file listed in
/// `checksums.sha256` with its digest, and nothing else. Hashing reads the
/// files; it never opens them as databases.
fn assert_fixture_is_the_released_capture() {
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
            "fixture file {file} no longer holds the released bytes; restore it from git and \
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

/// Stage one pristine copy of the released store into `<temp>/state`.
fn stage_store(temp: &Path) -> PathBuf {
    assert_fixture_is_the_released_capture();
    let state = temp.join("state");
    copy_dir_recursive(&fixture_dir().join("state"), &state);
    // The released state holds an EMPTY `blobs/` directory, which git cannot
    // track; restore it rather than committing a placeholder.
    std::fs::create_dir_all(state.join("blobs")).expect("restore empty blobs dir");
    state
}

fn released_manifest() -> String {
    std::fs::read_to_string(fixture_dir().join("state").join(MANIFEST_FILE))
        .expect("read the released composition manifest")
}

/// The released manifest with the creator field's name changed and every
/// other byte kept.
fn adopted_manifest() -> String {
    let released = released_manifest();
    assert_eq!(
        released.matches(PREDECESSOR_CREATOR_KEY).count(),
        1,
        "the released manifest spells its creator field once: {released}"
    );
    assert!(!released.contains(CREATOR_KEY), "{released}");
    released.replacen(PREDECESSOR_CREATOR_KEY, CREATOR_KEY, 1)
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

impl VerbRun {
    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|error| {
            panic!(
                "stdout is not a JSON report ({error})\nstdout: {}\nstderr: {}",
                self.stdout, self.stderr
            )
        })
    }
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

fn adopt(state: &Path, home: &Path, apply: bool) -> Value {
    let run = adopt_with(state, home, apply, &[]);
    assert_eq!(
        run.code, 0,
        "storage-adopt-predecessor (apply={apply}) failed\nstdout: {}\nstderr: {}",
        run.stdout, run.stderr
    );
    run.json()
}

/// One `storage-adopt-predecessor --json` run with `extra` arguments, its
/// exit code unchecked.
fn adopt_with(state: &Path, home: &Path, apply: bool, extra: &[&str]) -> VerbRun {
    let state_arg = state.to_str().expect("utf-8 state path");
    let mut args = vec![
        "storage-adopt-predecessor",
        "--state-dir",
        state_arg,
        "--json",
    ];
    if apply {
        args.push("--apply");
    }
    args.extend_from_slice(extra);
    run_binary(env!("CARGO_BIN_EXE_meta_gateway"), &args, home)
}

fn actions_of<'a>(report: &'a Value, item: &str) -> Vec<&'a Value> {
    report["actions"]
        .as_array()
        .expect("report actions")
        .iter()
        .filter(|action| action["item"] == item)
        .collect()
}

/// The report names exactly one composition manifest action, for the
/// store's manifest, renaming the creator field, with `outcome`.
fn assert_manifest_action(report: &Value, state: &Path, outcome: &str) {
    let actions = actions_of(report, "composition_manifest");
    assert_eq!(actions.len(), 1, "one manifest action expected: {report:#}");
    let action = actions[0];
    assert_eq!(action["outcome"], outcome, "{report:#}");
    assert_eq!(
        action["subject"],
        json!(state.join(MANIFEST_FILE).display().to_string()),
        "{report:#}"
    );
    assert_eq!(action["from"], "created_by_mobkit", "{report:#}");
    assert_eq!(action["to"], "created_by_meta", "{report:#}");
}

// ---------------------------------------------------------------------------
// Child gateway process (stdin-JSONL, as the SDKs drive it)
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

/// The `meta/init` params of a boot of the released deployment with
/// `mob_config`.
fn init_params(state: &Path, mob_config: &str, launch: Launch) -> Value {
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
        "mob_config": mob_config,
        "has_roster_provider": true,
        "runtime_options": runtime_options
    })
}

/// Boot the build under test over `state` with `mob_config` as a `launch`
/// that must be refused; the `meta/init` error. The gateway is then closed
/// and reaped.
fn boot_refused_as(
    label: &str,
    state: &Path,
    home: &Path,
    mob_config: &str,
    launch: Launch,
) -> Value {
    let mut gateway = Gateway::start(label, home);
    let response = gateway.request(
        "init",
        "meta/init",
        init_params(state, mob_config, launch),
        INIT_TIMEOUT,
    );
    let Some(error) = response.get("error").cloned() else {
        panic!(
            "[{label}] meta/init must be refused: {response}\n{}",
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
    error
}

/// A boot of the adopted store is refused before any store opens:
/// STORAGE_RESOLUTION (-32014), naming the native migration. Both launch
/// authorities refuse alike, whatever definition they carry.
fn assert_unmigrated_boot_refused(
    label: &str,
    state: &Path,
    home: &Path,
    mob_config: &str,
    launch: Launch,
) {
    let refusal = boot_refused_as(label, state, home, mob_config, launch);
    assert_eq!(
        refusal["code"],
        json!(meerkat_meta::STORAGE_RESOLUTION_CODE),
        "[{label}] {refusal}"
    );
    assert!(
        refusal["message"]
            .as_str()
            .is_some_and(|message| message.contains("storage-migrate-native")),
        "[{label}] the refusal must name the migration: {refusal}"
    );
}

fn journal(state: &Path) -> Value {
    serde_json::from_slice(
        &std::fs::read(state.join("meta-adoption.json")).expect("read the adoption journal"),
    )
    .expect("decode the adoption journal")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// The adoption flow: dry run and apply; then a boot of the build under test
/// is refused before any store opens, for an authoritative and a candidate
/// launch, with the released definition byte for byte and without the ignored
/// `tools.mob_tasks` key.
#[test]
fn released_0_8_26_store_adopts_and_is_refused_until_its_supported_migration() {
    let temp = tempfile::tempdir().expect("tempdir");
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).expect("home");
    let state = stage_store(temp.path());
    let released = released_manifest();

    assert!(
        MetaStorageLayout::with_injected_roots(state.clone(), None)
            .continuity_db()
            .is_err(),
        "the unadopted released store must refuse to open"
    );

    // 1. Dry run: every step listed, the manifest rewrite among them, and
    // only the given state directory in scope.
    let dry = adopt(&state, &home, false);
    assert_eq!(dry["errors"], json!([]), "{dry:#}");
    assert_eq!(dry["changed_state"], false, "{dry:#}");
    assert_eq!(dry["complete"], false, "{dry:#}");
    assert_manifest_action(&dry, &state, "planned");
    for item in [
        "database_file",
        "ledger_domain",
        "metadata_table",
        "realm_id",
    ] {
        assert!(
            !actions_of(&dry, item).is_empty(),
            "{item} missing: {dry:#}"
        );
    }
    assert!(
        actions_of(&dry, "gateway_home").is_empty(),
        "a state-directory adoption must not plan the user's gateway home: {dry:#}"
    );
    assert_eq!(
        manifest_text(&state),
        released,
        "the dry run changed the manifest"
    );

    // 2. Apply: complete, journaled, and the manifest differs in the creator
    // field's name only.
    let applied = adopt(&state, &home, true);
    assert_eq!(applied["errors"], json!([]), "{applied:#}");
    assert_eq!(applied["complete"], true, "{applied:#}");
    assert_manifest_action(&applied, &state, "applied");
    assert_eq!(manifest_text(&state), adopted_manifest());
    let journal = journal(&state);
    assert_eq!(journal["status"], "complete", "{journal:#}");
    // Every step finished, recorded only under step names every 0.9 release
    // reads (the manifest rename rides the `ledger` step), so a binary
    // rollback to 0.9.0 can still read this journal.
    assert_eq!(
        journal["completed"],
        json!([
            "database_files",
            "memory_ledger_file",
            "ledger",
            "steward_schedule",
            "gateway_home",
            "draft_store"
        ]),
        "{journal:#}"
    );
    assert_eq!(journal["realm_id"], "mobkit", "{journal:#}");

    // Idempotent: a rerun finds nothing left to adopt.
    let again = adopt(&state, &home, false);
    assert_eq!(again["actions"], json!([]), "{again:#}");
    assert_eq!(again["complete"], true, "{again:#}");

    // 3. Refused before any store opens, whatever the launch and the
    // definition, and the refusal rewrites nothing.
    for (label, mob_config, launch) in [
        (
            "0.8.26/same-definition",
            MOB_CONFIG.to_string(),
            Launch::Authoritative,
        ),
        (
            "0.8.26/ignored-key-removed",
            mob_config_without_ignored_key(),
            Launch::Authoritative,
        ),
        (
            "0.8.26/candidate",
            MOB_CONFIG.to_string(),
            Launch::Candidate,
        ),
    ] {
        assert_unmigrated_boot_refused(label, &state, &home, &mob_config, launch);
    }
    assert_eq!(
        manifest_text(&state),
        adopted_manifest(),
        "a refused boot never rewrites the manifest"
    );
}

/// Keys the released 0.8.26 dropped without a diagnostic, and Meta 0.9
/// applies, cannot change the store's behaviour silently on upgrade.
/// Adoption given the definition (`--mob-config`) names each of them and
/// refuses before any change until the operator removes them or
/// acknowledges them (`--acknowledge-ignored-keys`). Acknowledged keys still
/// take no effect by resuming: the store recorded its definition without
/// them, so a boot that keeps them refuses as divergent until the stored
/// definition is advanced, while the released definition resumes as before.
#[test]
fn released_0_8_26_store_adoption_names_and_holds_keys_the_release_ignored() {
    let temp = tempfile::tempdir().expect("tempdir");
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).expect("home");
    let state = stage_store(temp.path());
    let released = released_manifest();
    let unchanged_path = temp.path().join("released-mob.toml");
    std::fs::write(&unchanged_path, MOB_CONFIG).expect("write the released definition");
    let unchanged = unchanged_path.to_str().expect("utf-8 config path");
    let keyed_path = temp.path().join("mob.toml");
    std::fs::write(&keyed_path, mob_config_with_predecessor_ignored_keys())
        .expect("write the definition with the ignored keys");
    let keyed = keyed_path.to_str().expect("utf-8 config path");
    let manifest = state.join(MANIFEST_FILE).display().to_string();
    let reported = |acknowledged: bool| {
        json!([{
            "manifest": manifest,
            "mob_id": "adoption-fixture-planners",
            "source": keyed,
            "created_by": "0.8.26",
            "keys": PREDECESSOR_IGNORED_KEYS,
            "acknowledged": acknowledged
        }])
    };

    // The released definition itself: nothing to report or refuse.
    let run = adopt_with(&state, &home, false, &["--mob-config", unchanged]);
    assert_eq!(
        run.code, 0,
        "stdout: {}\nstderr: {}",
        run.stdout, run.stderr
    );
    let clean = run.json();
    assert_eq!(clean["ignored_keys"], json!([]), "{clean:#}");
    assert_eq!(clean["errors"], json!([]), "{clean:#}");

    // The keys the release ignored: each named, and refused before any
    // change, by the dry run and by apply.
    for apply in [false, true] {
        let run = adopt_with(&state, &home, apply, &["--mob-config", keyed]);
        assert_eq!(
            run.code, 1,
            "apply={apply}\nstdout: {}\nstderr: {}",
            run.stdout, run.stderr
        );
        let refused = run.json();
        assert_eq!(refused["ignored_keys"], reported(false), "{refused:#}");
        assert_eq!(refused["changed_state"], false, "{refused:#}");
        let errors = refused["errors"].as_array().expect("report errors");
        assert_eq!(errors.len(), 1, "{refused:#}");
        let error = errors[0].as_str().expect("error text");
        for needle in PREDECESSOR_IGNORED_KEYS.into_iter().chain([
            "refused, nothing changed",
            "0.8.26",
            "an authoritative resume",
            "--acknowledge-ignored-keys",
        ]) {
            assert!(error.contains(needle), "{needle:?} missing from {error}");
        }
    }
    assert_eq!(
        manifest_text(&state),
        released,
        "a refused adoption changed the manifest"
    );
    assert!(
        !state.join("meta-adoption.json").exists(),
        "a refused adoption wrote its journal"
    );
    assert!(
        MetaStorageLayout::with_injected_roots(state.clone(), None)
            .continuity_db()
            .is_err(),
        "the refused store stays unadopted"
    );

    // Acknowledged: adoption completes, and its report keeps the keys.
    let run = adopt_with(
        &state,
        &home,
        true,
        &["--mob-config", keyed, "--acknowledge-ignored-keys"],
    );
    assert_eq!(
        run.code, 0,
        "stdout: {}\nstderr: {}",
        run.stdout, run.stderr
    );
    let applied = run.json();
    assert_eq!(applied["ignored_keys"], reported(true), "{applied:#}");
    assert_eq!(applied["errors"], json!([]), "{applied:#}");
    assert_eq!(applied["complete"], true, "{applied:#}");
    assert_eq!(manifest_text(&state), adopted_manifest());

    // A boot is refused before the composition is judged, with the keys and
    // without them.
    assert_unmigrated_boot_refused(
        "0.8.26/predecessor-ignored-keys",
        &state,
        &home,
        &mob_config_with_predecessor_ignored_keys(),
        Launch::Authoritative,
    );
    assert_unmigrated_boot_refused(
        "0.8.26/released-definition",
        &state,
        &home,
        MOB_CONFIG,
        Launch::Authoritative,
    );
}

/// A store 0.8.26 created whose definition a later release advanced with a
/// tool deny list runs with that list, although its creation manifest does
/// not record it. Adoption compares the definition the store holds, so it
/// names only the keys that definition lacks (here the model fallbacks) and
/// adopts with the deny list kept. A boot of the build under test is then
/// refused before the composition is judged, whether it keeps the list or not.
#[test]
fn released_0_8_26_store_with_an_advanced_definition_keeps_the_deny_list_it_holds() {
    let temp = tempfile::tempdir().expect("tempdir");
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).expect("home");
    let state = stage_store(temp.path());
    advance_stored_definition(&state, &mob_config_with_deny());
    assert_eq!(
        manifest_text(&state),
        released_manifest(),
        "a definition update leaves the creation manifest as it was"
    );

    let deny_path = temp.path().join("mob.toml");
    std::fs::write(&deny_path, mob_config_with_deny()).expect("write the definition");
    let deny = deny_path.to_str().expect("utf-8 config path");
    // The same keys as a serialized definition, the form of a definition.json.
    let keyed_path = temp.path().join("definition.json");
    std::fs::write(
        &keyed_path,
        serde_json::to_vec(
            &meerkat_mob::MobDefinition::from_toml(&mob_config_with_predecessor_ignored_keys())
                .expect("definition"),
        )
        .expect("serialize the definition"),
    )
    .expect("write the definition");
    let keyed = keyed_path.to_str().expect("utf-8 config path");

    // The definition that keeps the deny list the store holds: nothing to
    // name or refuse.
    let run = adopt_with(&state, &home, false, &["--mob-config", deny]);
    assert_eq!(
        run.code, 0,
        "stdout: {}\nstderr: {}",
        run.stdout, run.stderr
    );
    let report = run.json();
    assert_eq!(report["ignored_keys"], json!([]), "{report:#}");
    assert_eq!(report["errors"], json!([]), "{report:#}");

    let run = adopt_with(&state, &home, false, &["--mob-config", keyed]);
    assert_eq!(
        run.code, 1,
        "stdout: {}\nstderr: {}",
        run.stdout, run.stderr
    );
    let named = run.json();
    assert_eq!(
        named["ignored_keys"][0]["keys"],
        json!(["profiles.planner.model_fallback", "runtime.model_fallback"]),
        "{named:#}"
    );
    assert_eq!(named["ignored_keys"][0]["source"], keyed, "{named:#}");

    let run = adopt_with(&state, &home, true, &["--mob-config", deny]);
    assert_eq!(
        run.code, 0,
        "stdout: {}\nstderr: {}",
        run.stdout, run.stderr
    );
    let report = run.json();
    assert_eq!(report["ignored_keys"], json!([]), "{report:#}");
    assert_eq!(report["errors"], json!([]), "{report:#}");
    assert_eq!(report["complete"], true, "{report:#}");

    // A boot is refused before the composition is judged, with the deny list
    // kept and removed alike.
    assert_unmigrated_boot_refused(
        "0.8.26/advanced-deny-kept",
        &state,
        &home,
        &mob_config_with_deny(),
        Launch::Authoritative,
    );
    assert_unmigrated_boot_refused(
        "0.8.26/advanced-deny-removed",
        &state,
        &home,
        MOB_CONFIG,
        Launch::Authoritative,
    );
}

/// The released store adopted the way an adoption without the manifest step
/// (Meta 0.9.0) leaves it: journal complete, manifest still
/// predecessor-spelled.
fn stage_store_adopted_without_the_manifest_step(temp: &Path, home: &Path) -> PathBuf {
    let state = stage_store(temp);
    let applied = adopt(&state, home, true);
    assert_eq!(applied["complete"], true, "{applied:#}");
    std::fs::write(state.join(MANIFEST_FILE), released_manifest()).expect("restore manifest");
    state
}

/// A store an earlier 0.9 adoption completed without the manifest step is
/// refused before any store opens on the exact path 0.9.0 refused (-32019
/// malformed: persistent mob storage, non-empty event log, authoritative
/// launch), now with STORAGE_RESOLUTION, and the refusal leaves the
/// predecessor-spelled manifest as it was.
#[test]
fn released_0_8_26_store_adopted_before_the_manifest_step_is_refused_alike() {
    let temp = tempfile::tempdir().expect("tempdir");
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).expect("home");
    let state = stage_store_adopted_without_the_manifest_step(temp.path(), &home);
    assert_unmigrated_boot_refused(
        "0.8.26/predecessor-manifest",
        &state,
        &home,
        MOB_CONFIG,
        Launch::Authoritative,
    );
    assert_eq!(
        manifest_text(&state),
        released_manifest(),
        "a refused boot never rewrites the manifest; adoption and repair do"
    );
}

/// A candidate launch over the same store with the unchanged definition is
/// refused alike, before the released-representation allowance (which reads
/// the predecessor manifest's creator version) could judge it.
#[test]
fn released_0_8_26_store_candidate_launch_is_refused_alike() {
    let temp = tempfile::tempdir().expect("tempdir");
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).expect("home");
    let state = stage_store_adopted_without_the_manifest_step(temp.path(), &home);
    assert_unmigrated_boot_refused(
        "0.8.26/candidate",
        &state,
        &home,
        MOB_CONFIG,
        Launch::Candidate,
    );
    assert_eq!(
        manifest_text(&state),
        released_manifest(),
        "a refused boot never rewrites the manifest"
    );
}

/// `meta_repair --composition-manifests` detects a predecessor manifest,
/// repairs it to exactly the adopted bytes, and finds nothing left to do.
#[test]
fn meta_repair_detects_and_repairs_a_predecessor_composition_manifest() {
    let temp = tempfile::tempdir().expect("tempdir");
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).expect("home");
    let state = stage_store(temp.path());
    let state_arg = state.to_str().expect("utf-8 state path");
    let manifest = state.join(MANIFEST_FILE).display().to_string();
    let repair = |args: &[&str]| {
        let run = run_binary(env!("CARGO_BIN_EXE_meta_repair"), args, &home);
        assert_eq!(
            run.code, 0,
            "meta_repair {args:?} failed\nstdout: {}\nstderr: {}",
            run.stdout, run.stderr
        );
        run.json()
    };

    let detected = repair(&["--composition-manifests", state_arg]);
    assert_eq!(
        detected,
        json!({
            "mode": "composition_manifests",
            "applied": false,
            "manifests": [{ "path": manifest, "outcome": "predecessor" }],
            "refused": 0,
            "failed": 0
        })
    );
    assert_eq!(
        manifest_text(&state),
        released_manifest(),
        "detection is read-only"
    );

    let repaired = repair(&["--composition-manifests", state_arg, "--apply"]);
    assert_eq!(
        repaired,
        json!({
            "mode": "composition_manifests",
            "applied": true,
            "manifests": [{ "path": manifest, "outcome": "rewritten" }],
            "refused": 0,
            "failed": 0
        })
    );
    assert_eq!(manifest_text(&state), adopted_manifest());

    let current = repair(&["--composition-manifests", state_arg, "--apply"]);
    assert_eq!(
        current["manifests"],
        json!([{ "path": manifest, "outcome": "current" }])
    );
    assert_eq!(manifest_text(&state), adopted_manifest());
}

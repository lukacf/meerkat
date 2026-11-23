//! End-to-end coverage for `rkat storage doctor` (Phase 1 of the storage
//! unification arc): read-only diagnosis over explicit state roots.
//!
//! Hermeticity: every invocation passes explicit roots (`--state-root` /
//! `--root`) plus overridden `HOME`/`XDG_DATA_HOME`, so the developer's real
//! data dir is never read.

#![cfg(feature = "session-store")]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::disallowed_methods
)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use meerkat_core::{BlobId, ContentBlock, ImageData, Message, Session, UserMessage};
use rusqlite::Connection;
use tempfile::TempDir;

/// Session-store migration 4 (`head-canonical-v2-authority`) completes the
/// supported HeadCanonical v1-to-v2 authority crossing: the floor a healthy
/// realm's session-store ledger stamp must reach.
const SESSION_STORE_HEAD_CANONICAL_CROSSING: i64 = 4;

const SESSIONS_DDL: &str = "CREATE TABLE sessions (
    session_id TEXT PRIMARY KEY,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    message_count INTEGER NOT NULL,
    total_tokens INTEGER NOT NULL,
    metadata_json TEXT NOT NULL,
    session_json BLOB NOT NULL
)";

fn write_manifest(state_root: &Path, realm_id: &str) -> meerkat_store::RealmPaths {
    let paths = meerkat_store::realm_paths_in(state_root, realm_id);
    std::fs::create_dir_all(&paths.root).unwrap();
    std::fs::write(
        &paths.manifest_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "realm_id": realm_id,
            "backend": "sqlite",
            "origin": "explicit",
            "created_at": "0",
        }))
        .unwrap(),
    )
    .unwrap();
    paths
}

/// A ledger-stamped realm: opening the real `SqliteSessionStore` applies the
/// session-store schema domain.
fn create_healthy_realm(state_root: &Path, realm_id: &str) {
    let paths = write_manifest(state_root, realm_id);
    meerkat_store::SqliteSessionStore::open(&paths.sessions_sqlite_path).unwrap();
}

/// An unledgered realm: raw `sessions` table, one structurally valid
/// current-envelope session row, and no `meerkat_schema` ledger.
fn create_unledgered_realm(state_root: &Path, realm_id: &str) -> PathBuf {
    let paths = write_manifest(state_root, realm_id);
    let conn = Connection::open(&paths.sessions_sqlite_path).unwrap();
    conn.execute_batch(SESSIONS_DDL).unwrap();
    let mut session = Session::new();
    session.push(Message::User(UserMessage::text(
        "hello from an unledgered fixture",
    )));
    insert_session(&conn, &session);
    paths.sessions_sqlite_path
}

fn insert_session(conn: &Connection, session: &Session) {
    conn.execute(
        "INSERT INTO sessions (session_id, created_at_ms, updated_at_ms, message_count, \
         total_tokens, metadata_json, session_json) VALUES (?1, 0, 0, ?2, 0, ?3, ?4)",
        rusqlite::params![
            session.id().to_string(),
            session.messages().len() as i64,
            serde_json::to_string(session.metadata()).unwrap(),
            serde_json::to_vec(session).unwrap(),
        ],
    )
    .unwrap();
}

fn run_doctor(temp: &TempDir, args: &[&str]) -> Output {
    let binary = std::env::var_os("CARGO_BIN_EXE_rkat")
        .map(PathBuf::from)
        .expect("Cargo or Nextest must provide the runtime rkat binary path");
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let mut command = Command::new(binary);
    command
        .current_dir(&project)
        .env("HOME", temp.path())
        .env("XDG_DATA_HOME", temp.path().join("data"))
        .env("RKAT_TEST_CLIENT", "1")
        .arg("--context-root")
        .arg(&project)
        .args(args);
    command.output().unwrap()
}

fn parse_json(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "stdout is not JSON ({err})\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn findings_with_code<'a>(report: &'a serde_json::Value, code: &str) -> Vec<&'a serde_json::Value> {
    report["findings"]
        .as_array()
        .expect("findings array")
        .iter()
        .filter(|finding| finding["code"] == code)
        .collect()
}

#[test]
fn healthy_sqlite_realm_is_clean_and_exits_zero() {
    let temp = TempDir::new().unwrap();
    let state_root = temp.path().join("realms");
    create_healthy_realm(&state_root, "healthy");

    let output = run_doctor(
        &temp,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "storage",
            "doctor",
            "--json",
        ],
    );
    assert!(
        output.status.success(),
        "healthy realm must exit 0\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = parse_json(&output);
    let inventory = report["inventory"].as_array().expect("inventory array");
    assert_eq!(inventory.len(), 1);
    assert_eq!(inventory[0]["realm"], "healthy");
    assert_eq!(inventory[0]["backend"], "sqlite");
    let domains = inventory[0]["databases"][0]["domains"]
        .as_array()
        .expect("domains array");
    // A floor, not the current version: a healthy realm has completed the
    // HeadCanonical v1-to-v2 authority crossing (session-store migration 4,
    // "head-canonical-v2-authority"); later migrations (5,
    // "transcript-retirements", #1541) keep it healthy.
    assert!(
        domains.iter().any(|pair| pair[0] == "session-store"
            && pair[1]
                .as_i64()
                .is_some_and(|version| version >= SESSION_STORE_HEAD_CANONICAL_CROSSING)),
        "session-store domain must be ledger-stamped at v{SESSION_STORE_HEAD_CANONICAL_CROSSING} \
         or later (v{SESSION_STORE_HEAD_CANONICAL_CROSSING} completes the supported HeadCanonical \
         v1-to-v2 authority crossing): {domains:?}"
    );
    let errors: Vec<_> = report["findings"]
        .as_array()
        .expect("findings array")
        .iter()
        .filter(|finding| finding["severity"] == "error")
        .collect();
    assert!(errors.is_empty(), "no error findings expected: {errors:?}");

    // Human-readable mode: same diagnosis, summary line, exit 0.
    let text = run_doctor(
        &temp,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "storage",
            "doctor",
        ],
    );
    assert!(text.status.success());
    let stdout = String::from_utf8_lossy(&text.stdout);
    assert!(
        stdout.contains("storage doctor: 0 error(s)"),
        "summary line missing:\n{stdout}"
    );
}

/// `rkat` switches SQLite's allocator statistics off as the first statement
/// of `main`, and the doctor reports the lock profile the binary runs with.
/// Holds for any build: with the declared build flags nothing is on and the
/// switch finds statistics already off; without them the switch applies and
/// the warning names the two locks only the flags remove.
#[test]
fn doctor_reports_the_binarys_sqlite_lock_profile() {
    let temp = TempDir::new().unwrap();
    let state_root = temp.path().join("realms");
    let output = run_doctor(
        &temp,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "storage",
            "doctor",
            "--json",
        ],
    );
    assert!(
        output.status.success(),
        "doctor over an absent root must exit 0\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = parse_json(&output);
    let profile = &report["sqlite_lock_profile"];
    assert_eq!(profile["memory_statistics"], false, "{profile}");
    assert!(
        matches!(
            profile["memory_statistics_switch"].as_str(),
            Some("already_off" | "applied")
        ),
        "the switch ran before anything used SQLite: {profile}"
    );
    let lock_in_use =
        profile["shared_page_cache"] == true || profile["page_cache_overflow_statistics"] == true;
    assert_eq!(
        findings_with_code(&report, "sqlite-lock-profile").len(),
        usize::from(lock_in_use),
        "{report}"
    );

    let text = run_doctor(
        &temp,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "storage",
            "doctor",
        ],
    );
    assert!(text.status.success());
    let stdout = String::from_utf8_lossy(&text.stdout);
    assert!(
        stdout.contains("SQLite lock profile of this binary: memory statistics off"),
        "profile line missing:\n{stdout}"
    );
}

#[test]
fn unledgered_realm_reports_structural_inventory_without_mutating() {
    let temp = TempDir::new().unwrap();
    let state_root = temp.path().join("realms");
    let database = create_unledgered_realm(&state_root, "unledgered-shape");
    let before = std::fs::read(&database).unwrap();

    let output = run_doctor(
        &temp,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "storage",
            "doctor",
            "--json",
        ],
    );
    assert!(
        output.status.success(),
        "a structurally valid unledgered realm has no error-severity finding\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = parse_json(&output);

    let no_ledger = findings_with_code(&report, "no-schema-ledger");
    assert_eq!(no_ledger.len(), 1, "{report:#}");
    assert_eq!(no_ledger[0]["severity"], "warning");

    assert_eq!(no_ledger[0]["realm"], "unledgered-shape");
    let errors: Vec<_> = report["findings"]
        .as_array()
        .expect("findings array")
        .iter()
        .filter(|finding| finding["severity"] == "error")
        .collect();
    assert!(
        errors.is_empty(),
        "structurally valid current session rows must not produce errors: {errors:#?}"
    );

    // Read-only proof: the database bytes are untouched (no ledger was
    // created, no pragmas rewritten).
    let after = std::fs::read(&database).unwrap();
    assert_eq!(before, after, "doctor must not mutate the database");
}

#[test]
fn attached_global_option_forms_reach_the_storage_verbs() {
    // `--state-root=…` / `--realm=…` / `-r…` must not trip the implicit
    // `run` insertion in the argv scanner (which would misparse `storage`
    // as a prompt).
    let temp = TempDir::new().unwrap();
    let state_root = temp.path().join("realms");
    create_healthy_realm(&state_root, "alpha");
    create_healthy_realm(&state_root, "beta");
    let state_root_arg = format!("--state-root={}", state_root.display());

    let output = run_doctor(
        &temp,
        &[
            &state_root_arg,
            "--realm=alpha",
            "storage",
            "doctor",
            "--json",
        ],
    );
    assert!(
        output.status.success(),
        "attached long forms must reach storage doctor\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = parse_json(&output);
    let inventory = report["inventory"].as_array().expect("inventory array");
    assert_eq!(inventory.len(), 1, "{report:#}");
    assert_eq!(inventory[0]["realm"], "alpha");

    let output = run_doctor(
        &temp,
        &[&state_root_arg, "-rbeta", "storage", "doctor", "--json"],
    );
    assert!(
        output.status.success(),
        "combined short form must reach storage doctor\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = parse_json(&output);
    let inventory = report["inventory"].as_array().expect("inventory array");
    assert_eq!(inventory.len(), 1, "{report:#}");
    assert_eq!(inventory[0]["realm"], "beta");
}

#[test]
fn isolated_and_default_model_are_usage_errors_for_storage_verbs() {
    // Storage verbs dispatch before runtime-scope resolution: `--isolated`
    // and `--default-model` can have no effect there and must be rejected
    // as usage errors (exit 2), never silently ignored.
    let temp = TempDir::new().unwrap();
    let state_root = temp.path().join("realms");
    create_healthy_realm(&state_root, "healthy");

    for (rejected_flag, args) in [
        (
            "--isolated",
            vec!["--isolated", "storage", "doctor", "--json"],
        ),
        ("--isolated", vec!["--isolated", "storage", "migrate"]),
        (
            "--default-model",
            vec!["--default-model", "gpt-5.4", "storage", "doctor"],
        ),
        (
            "--default-model",
            vec!["--default-model", "gpt-5.4", "storage", "prune", "--apply"],
        ),
    ] {
        let mut full = vec!["--state-root", state_root.to_str().unwrap()];
        full.extend(args);
        let output = run_doctor(&temp, &full);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{rejected_flag} with storage verbs must exit 2\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(rejected_flag) && stderr.contains("rkat storage"),
            "usage error must name the flag and the storage verbs: {stderr}"
        );
    }
}

#[test]
fn split_brain_twin_across_two_roots_is_an_error() {
    let temp = TempDir::new().unwrap();
    let root_a = temp.path().join("root-a");
    let root_b = temp.path().join("root-b");
    create_healthy_realm(&root_a, "team");
    create_healthy_realm(&root_b, "team");

    let output = run_doctor(
        &temp,
        &[
            "storage",
            "doctor",
            "--json",
            "--root",
            root_a.to_str().unwrap(),
            "--root",
            root_b.to_str().unwrap(),
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(1),
        "split-brain must exit 1\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = parse_json(&output);
    let twins = findings_with_code(&report, "split-brain-realm");
    assert_eq!(twins.len(), 1, "{report:#}");
    assert_eq!(twins[0]["severity"], "error");
    let message = twins[0]["message"].as_str().unwrap();
    assert!(message.contains(&root_a.join("team").display().to_string()));
    assert!(message.contains(&root_b.join("team").display().to_string()));
    // Both materializations appear in the inventory.
    let inventory = report["inventory"].as_array().expect("inventory array");
    assert_eq!(
        inventory
            .iter()
            .filter(|entry| entry["realm"] == "team")
            .count(),
        2
    );
}

#[test]
fn dangling_blob_reference_and_future_schema_are_errors() {
    let temp = TempDir::new().unwrap();
    let state_root = temp.path().join("realms");

    // Realm with a session referencing a blob object that does not exist.
    let dangling_paths = write_manifest(&state_root, "dangling");
    let blob_id = BlobId::new(format!("sha256:{}", "c".repeat(64)));
    let session = {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::with_blocks(vec![
            ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: ImageData::Blob {
                    blob_id: blob_id.clone(),
                },
            },
        ])));
        session
    };
    {
        let conn = Connection::open(&dangling_paths.sessions_sqlite_path).unwrap();
        conn.execute_batch(SESSIONS_DDL).unwrap();
        insert_session(&conn, &session);
    }

    // Realm whose ledger claims a session-store version from the future.
    let future_paths = write_manifest(&state_root, "future");
    {
        let conn = Connection::open(&future_paths.sessions_sqlite_path).unwrap();
        conn.execute_batch(SESSIONS_DDL).unwrap();
        conn.execute_batch(
            "CREATE TABLE meerkat_schema (domain TEXT PRIMARY KEY, version INTEGER NOT NULL)",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO meerkat_schema (domain, version) VALUES ('session-store', 9999)",
            [],
        )
        .unwrap();
    }

    let output = run_doctor(
        &temp,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "storage",
            "doctor",
            "--json",
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(1),
        "error findings must exit 1\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = parse_json(&output);

    let dangling = findings_with_code(&report, "dangling-blob-reference");
    assert_eq!(dangling.len(), 1, "{report:#}");
    assert_eq!(dangling[0]["severity"], "error");
    let message = dangling[0]["message"].as_str().unwrap();
    assert!(message.contains(&session.id().to_string()), "{message}");
    assert!(message.contains(blob_id.as_str()), "{message}");

    let future = findings_with_code(&report, "schema-from-the-future");
    assert_eq!(future.len(), 1, "{report:#}");
    assert_eq!(future[0]["severity"], "error");
    assert!(
        future[0]["message"]
            .as_str()
            .unwrap()
            .contains("session-store")
    );

    // Realm filter: scoping to the future realm hides the dangling one.
    let filtered = run_doctor(
        &temp,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "--realm",
            "future",
            "storage",
            "doctor",
            "--json",
        ],
    );
    assert_eq!(filtered.status.code(), Some(1));
    let filtered_report = parse_json(&filtered);
    assert!(findings_with_code(&filtered_report, "dangling-blob-reference").is_empty());
    assert_eq!(
        findings_with_code(&filtered_report, "schema-from-the-future").len(),
        1
    );
}

#[test]
fn global_realm_config_writes_route_to_the_user_global_doc() {
    // The reserved `global` realm's config head is the user-global document
    // child realms inherit from (`<user-config-root>/.rkat/config.toml`);
    // writing `<state_root>/global/config.toml` instead would shadow it.
    let temp = TempDir::new().unwrap();
    let state_root = temp.path().join("realms");
    std::fs::create_dir_all(&state_root).unwrap();
    let user_root = temp.path().join("home");

    let set = run_doctor(
        &temp,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "--user-config-root",
            user_root.to_str().unwrap(),
            "--realm",
            "global",
            "config",
            "set",
            "--toml",
            "agent.model = \"gpt-5.5\"",
        ],
    );
    assert!(
        set.status.success(),
        "global-realm config set must succeed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&set.stdout),
        String::from_utf8_lossy(&set.stderr)
    );

    let global_doc = user_root.join(".rkat").join("config.toml");
    assert!(
        global_doc.is_file(),
        "the write must land in the user-global doc: {global_doc:?}"
    );
    let written = std::fs::read_to_string(&global_doc).unwrap();
    assert!(written.contains("gpt-5.5"), "{written}");
    assert!(
        !state_root.join("global").join("config.toml").exists(),
        "the reserved global realm must not materialize a shadowing per-realm doc"
    );

    // Reads route through the same projection.
    let get = run_doctor(
        &temp,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "--user-config-root",
            user_root.to_str().unwrap(),
            "--realm",
            "global",
            "config",
            "get",
        ],
    );
    assert!(
        get.status.success(),
        "global-realm config get must succeed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&get.stdout),
        String::from_utf8_lossy(&get.stderr)
    );
    assert!(
        String::from_utf8_lossy(&get.stdout).contains("gpt-5.5"),
        "config get must read the user-global doc back:\n{}",
        String::from_utf8_lossy(&get.stdout)
    );
}

/// Every regular file under `dir` with its length and modification time,
/// SQLite's `-wal`/`-shm` sidecars left out: a read-only WAL open may touch
/// those, never a database or any other file.
#[cfg(feature = "mob")]
fn file_facts(dir: &Path) -> std::collections::BTreeMap<PathBuf, (u64, std::time::SystemTime)> {
    let mut facts = std::collections::BTreeMap::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in std::fs::read_dir(&next).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if name.ends_with("-wal") || name.ends_with("-shm") {
                continue;
            }
            let metadata = std::fs::metadata(&path).unwrap();
            facts.insert(path, (metadata.len(), metadata.modified().unwrap()));
        }
    }
    facts
}

/// Row (RED): `rkat storage doctor` reports, for each mob database of a
/// realm that holds identity-store rows of schema version 2 or later, one
/// Info `identity-store-rows` finding with the counts only (the identity
/// census sample: 10 rows, 1 archived, 2 tombstones, 4 unreadable), and
/// none for a mob database without such rows. Info alone, so the doctor
/// exits 0, and it changes no file.
///
/// RED on the old tree: the doctor reports no identity-store rows.
#[cfg(feature = "mob")]
#[test]
fn identity_store_rows_are_counted_per_mob_database() {
    let temp = TempDir::new().unwrap();
    let state_root = temp.path().join("realms");
    let paths = write_manifest(&state_root, "census");
    let mobs = paths.root.join("mobs");
    std::fs::create_dir_all(&mobs).unwrap();
    let seeded = mobs.join("census.db");
    meerkat_mob::seed_identity_census_rows_for_test(&seeded).unwrap();
    let empty = mobs.join("empty.db");
    drop(meerkat_mob::MobStorage::persistent(&empty).unwrap());
    let before = file_facts(&state_root);

    let output = run_doctor(
        &temp,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "storage",
            "doctor",
            "--json",
        ],
    );
    assert!(
        output.status.success(),
        "an Info finding never fails the doctor\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = parse_json(&output);
    let rows = findings_with_code(&report, "identity-store-rows");
    assert_eq!(
        rows.len(),
        1,
        "only the database with such rows: {report:#}"
    );
    assert_eq!(rows[0]["severity"], "info");
    assert_eq!(rows[0]["realm"], "census");
    assert_eq!(rows[0]["path"], seeded.to_str().unwrap());
    let message = rows[0]["message"].as_str().unwrap();
    assert!(
        message.starts_with(
            "10 identity-store row(s) of schema version 2 or later (1 archived, 2 \
             tombstone(s), 4 unreadable)"
        ),
        "{message}"
    );
    assert!(
        message.contains(
            "a downgrade of this mob to such a release is supported only through the downgrade \
             cycle (export, purge, and import after the roll-forward), and not while any remain"
        ),
        "{message}"
    );
    assert_eq!(
        file_facts(&state_root),
        before,
        "the doctor changes no file of the realm"
    );
}

/// [`run_doctor`] from `context` (below `<temp>/project`), with no root
/// given unless `args` names one.
fn run_rkat_from(temp: &TempDir, context: &Path, args: &[&str]) -> Output {
    let binary = std::env::var_os("CARGO_BIN_EXE_rkat")
        .map(PathBuf::from)
        .expect("Cargo or Nextest must provide the runtime rkat binary path");
    std::fs::create_dir_all(context).unwrap();
    let mut command = Command::new(binary);
    command
        .current_dir(context)
        .env("HOME", temp.path())
        .env("XDG_DATA_HOME", temp.path().join("data"))
        .env("RKAT_TEST_CLIENT", "1")
        .arg("--context-root")
        .arg(context)
        .args(args);
    command.output().unwrap()
}

/// With no root given, the doctor reads the state root of every ancestor of
/// the context root that holds a `.rkat` entry, the roots ordinary
/// resolution probes: a realm present under two of them is reported.
#[test]
fn doctor_reads_every_ancestor_state_root_by_default() {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("project");
    let nested = project.join("sub");
    let project_realms = project.join(".rkat").join("realms");
    let nested_realms = nested.join(".rkat").join("realms");
    create_healthy_realm(&project_realms, "team");
    create_healthy_realm(&nested_realms, "team");

    let output = run_rkat_from(&temp, &nested, &["storage", "doctor", "--json"]);
    let report = parse_json(&output);
    let twins = findings_with_code(&report, "split-brain-realm");
    assert_eq!(twins.len(), 1, "{report:#}");
    let message = twins[0]["message"].as_str().unwrap();
    assert!(message.contains(&project_realms.join("team").display().to_string()));
    assert!(message.contains(&nested_realms.join("team").display().to_string()));
    assert_eq!(output.status.code(), Some(1), "{report:#}");
}

/// A session of `model` on Anthropic that took an instruction change at a
/// real fresh run start, lowered as for a model that takes System rows only
/// ahead of the conversation: behind a conversation row when `conversed`,
/// else while its transcript was still the leading System rows.
fn session_after_a_run_start_change(model: &str, conversed: bool) -> Session {
    use meerkat_core::types::{InstructionKey, InstructionNamespace};

    let mut session = Session::new();
    session
        .set_session_metadata(meerkat_core::SessionMetadata {
            model_fallback: None,
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: model.to_string(),
            max_tokens: 8_192,
            structured_output_retries: 2,
            provider: meerkat_core::Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            tooling: meerkat_core::SessionTooling::default(),
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            mob_member_binding: None,
        })
        .unwrap();
    session.append_system_message("You are a careful assistant.");
    if conversed {
        session.push(Message::User(UserMessage::text("hello")));
    }
    session
        .set_session_instruction(meerkat_core::SessionInstructionWrite {
            slot: meerkat_core::InstructionSlot {
                namespace: InstructionNamespace::new("app.example").unwrap(),
                key: InstructionKey::new("tone").unwrap(),
            },
            revision: meerkat_core::InstructionRevision::new(1).unwrap(),
            change: meerkat_core::SessionInstructionChange::Activate {
                body: "Answer formally.".to_string(),
                content_sha256: None,
            },
        })
        .unwrap();
    let outcome = session
        .apply_run_start_instructions(
            meerkat_core::InstructionRunStart::FreshRun,
            meerkat_core::InstructionLowering::LeadingPrefixOnly,
            None,
        )
        .unwrap();
    assert!(
        matches!(
            outcome,
            meerkat_core::RunStartInstructionOutcome::Applied { .. }
        ),
        "the run start applies the change: {outcome:?}"
    );
    session.push(Message::User(UserMessage::text("next")));
    session
}

/// The count of sessions that hold their own operator-instruction entry
/// after a conversation row, on a store the session store itself wrote and
/// through the binary. Two sessions are saved through `SqliteSessionStore`
/// after a real run start applied an instruction change: one behind a
/// conversation row, one while its transcript was still the leading System
/// rows. The doctor counts the first, with the model its metadata names, and
/// not the second. Info alone, so the doctor exits 0. The plain save of a
/// session the store does not hold yet writes a whole document; the form a
/// runtime-backed surface writes is the row below.
#[tokio::test]
async fn sessions_the_store_saved_after_a_late_run_start_entry_are_counted() {
    use meerkat_store::SessionStore;

    let temp = TempDir::new().unwrap();
    let state_root = temp.path().join("realms");
    let paths = write_manifest(&state_root, "saved-by-the-store");
    let late = session_after_a_run_start_change("claude-haiku-4-5", true);
    let leading = session_after_a_run_start_change("claude-haiku-4-5", false);
    {
        let store = meerkat_store::SqliteSessionStore::open(&paths.sessions_sqlite_path).unwrap();
        store.save(&late).await.unwrap();
        store.save(&leading).await.unwrap();
    }

    let output = run_doctor(
        &temp,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "storage",
            "doctor",
            "--json",
        ],
    );
    assert!(
        output.status.success(),
        "an Info finding never fails the doctor\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = parse_json(&output);
    let rows = findings_with_code(&report, "operator-instruction-entry-after-conversation");
    assert_eq!(rows.len(), 1, "one finding for the database: {report:#}");
    assert_eq!(rows[0]["severity"], "info");
    assert_eq!(rows[0]["realm"], "saved-by-the-store");
    assert_eq!(
        rows[0]["path"],
        paths.sessions_sqlite_path.to_str().unwrap()
    );
    let message = rows[0]["message"].as_str().unwrap();
    let expected = format!(
        "1 session(s) hold their own operator-instruction entry after a conversation row \
         (anthropic/claude-haiku-4-5: 1): {} (model anthropic/claude-haiku-4-5).",
        late.id()
    );
    assert!(
        message.starts_with(&expected),
        "the count, the count per model from the metadata the store wrote, and the session: \
         {message}"
    );
    assert!(
        !message.contains(&leading.id().to_string()),
        "an entry applied ahead of the conversation is not counted: {message}"
    );

    // Human-readable mode: the same line, exit 0.
    let text = run_doctor(
        &temp,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "storage",
            "doctor",
        ],
    );
    assert!(text.status.success());
    let stdout = String::from_utf8_lossy(&text.stdout);
    assert!(stdout.contains(&expected), "the line is printed:\n{stdout}");
}

/// The same count on the stored form every runtime-backed surface writes: a
/// session written through the store's head path. Its head row is compact:
/// the row's own metadata map names no model, and the session's metadata
/// (its model among it) stands out of line in the head's metadata cells. The
/// doctor counts the session and names its model from there.
#[tokio::test]
async fn a_session_written_through_the_head_path_is_counted_with_its_model() {
    use meerkat_core::session_store::{IncrementalSessionStore, PreparedHeadCanonicalMutation};

    let temp = TempDir::new().unwrap();
    let state_root = temp.path().join("realms");
    let paths = write_manifest(&state_root, "written-through-the-head-path");
    let late = session_after_a_run_start_change("claude-haiku-4-5", true);
    {
        let store = meerkat_store::SqliteSessionStore::open(&paths.sessions_sqlite_path).unwrap();
        let root = PreparedHeadCanonicalMutation::prepare_root(&late).unwrap();
        store
            .apply_prepared_head_canonical_mutation(&root)
            .await
            .unwrap();
    }
    {
        let conn = Connection::open(&paths.sessions_sqlite_path).unwrap();
        let inline: String = conn
            .query_row(
                "SELECT metadata_json FROM session_heads WHERE session_id = ?1",
                [late.id().to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            !inline.contains("session_metadata"),
            "the head row's own map names no model, so the label is read out of line: {inline}"
        );
        let cells: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session_head_metadata_current \
                 WHERE session_id = ?1 AND metadata_key = 'session_metadata'",
                [late.id().to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            cells, 1,
            "the head's current metadata holds the model's key"
        );
    }

    let output = run_doctor(
        &temp,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "storage",
            "doctor",
            "--json",
        ],
    );
    assert!(
        output.status.success(),
        "an Info finding never fails the doctor\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = parse_json(&output);
    let rows = findings_with_code(&report, "operator-instruction-entry-after-conversation");
    assert_eq!(rows.len(), 1, "one finding for the database: {report:#}");
    assert_eq!(
        rows[0]["path"],
        paths.sessions_sqlite_path.to_str().unwrap()
    );
    let message = rows[0]["message"].as_str().unwrap();
    let expected = format!(
        "1 session(s) hold their own operator-instruction entry after a conversation row \
         (anthropic/claude-haiku-4-5: 1): {} (model anthropic/claude-haiku-4-5).",
        late.id()
    );
    assert!(
        message.starts_with(&expected),
        "the count, and the model read from the head's out-of-line metadata: {message}"
    );
}

/// The model the finding names is the one the head holds now. A session is
/// written through the store's head path on one model; a later write through
/// the same path moves it to another model, as a change of model on a live
/// session does, and appends a row. The head then commits to a metadata map
/// that holds the later value, and the doctor names that model: the head's
/// current metadata follows every write, and an earlier value of the key
/// names nothing.
#[tokio::test]
async fn the_model_named_is_the_one_the_head_holds_after_a_later_write() {
    use meerkat_core::session_store::{IncrementalSessionStore, PreparedHeadCanonicalMutation};

    let temp = TempDir::new().unwrap();
    let state_root = temp.path().join("realms");
    let paths = write_manifest(&state_root, "written-twice");
    let mut late = session_after_a_run_start_change("claude-haiku-4-5", true);
    {
        let store = meerkat_store::SqliteSessionStore::open(&paths.sessions_sqlite_path).unwrap();
        let root = PreparedHeadCanonicalMutation::prepare_root(&late).unwrap();
        let token = store
            .apply_prepared_head_canonical_mutation(&root)
            .await
            .unwrap();
        root.acknowledge_session(&mut late, &token).unwrap();

        let mut metadata = late
            .session_metadata()
            .expect("the session's metadata names its model");
        metadata.model = "claude-haiku-5-5".to_string();
        late.set_session_metadata(metadata).unwrap();
        late.push(Message::User(UserMessage::text(
            "after the change of model",
        )));
        let later =
            PreparedHeadCanonicalMutation::prepare(&late, Some(root.successor_head().clone()))
                .unwrap();
        store
            .apply_prepared_head_canonical_mutation(&later)
            .await
            .unwrap();
    }

    let output = run_doctor(
        &temp,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "storage",
            "doctor",
            "--json",
        ],
    );
    assert!(
        output.status.success(),
        "an Info finding never fails the doctor\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = parse_json(&output);
    let rows = findings_with_code(&report, "operator-instruction-entry-after-conversation");
    assert_eq!(rows.len(), 1, "one finding for the database: {report:#}");
    let message = rows[0]["message"].as_str().unwrap();
    let expected = format!(
        "1 session(s) hold their own operator-instruction entry after a conversation row \
         (anthropic/claude-haiku-5-5: 1): {} (model anthropic/claude-haiku-5-5).",
        late.id()
    );
    assert!(
        message.starts_with(&expected),
        "the model the head holds after the later write: {message}"
    );
}

/// The count reads a head's live rows as the store serves them. A
/// same-session rewrite makes the head's strand a linked strand: it holds the
/// rewrite's replacement rows and what was appended after it, and the rows
/// before the splice are served by the strand it links to. A session is
/// written through the store's head path with its own operator-instruction
/// entry after a conversation row, and a rewrite then replaces its last row
/// only, so the entry row stands outside the rewrite's splice. The transcript
/// still holds the late entry: the doctor counts the session, with the model
/// its head names. The row asserts first that the head's own strand holds
/// fewer rows than the head counts, so it cannot pass on a strand that holds
/// every row itself.
#[tokio::test]
async fn a_late_entry_outside_the_last_rewrites_splice_is_counted() {
    use meerkat_core::session_store::{
        IncrementalSessionStore, PreparedHeadCanonicalMutation,
        PreparedHeadCanonicalRewriteMutation,
    };
    use meerkat_core::{TranscriptRewriteReason, TranscriptRewriteSelection};

    let temp = TempDir::new().unwrap();
    let state_root = temp.path().join("realms");
    let paths = write_manifest(&state_root, "rewritten-after-a-late-entry");
    let mut late = session_after_a_run_start_change("claude-haiku-4-5", true);
    let rows = late.messages().len();
    let head = {
        let store = meerkat_store::SqliteSessionStore::open(&paths.sessions_sqlite_path).unwrap();
        let root = PreparedHeadCanonicalMutation::prepare_root(&late).unwrap();
        let token = store
            .apply_prepared_head_canonical_mutation(&root)
            .await
            .unwrap();
        root.acknowledge_session(&mut late, &token).unwrap();
        // A generic rewrite of the last row only: the entry row keeps its
        // place, outside the splice.
        late.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange {
                start: rows - 1,
                end: rows,
            },
            vec![Message::User(UserMessage::text("next, said again"))],
            TranscriptRewriteReason::new("doctor-census-fixture"),
            None,
            None,
        )
        .expect("the rewrite commits");
        let rewrite =
            PreparedHeadCanonicalRewriteMutation::prepare(&late, root.successor_head().clone())
                .unwrap();
        store
            .apply_prepared_head_canonical_rewrite_mutation(&rewrite)
            .await
            .unwrap();
        rewrite.successor_head().clone()
    };
    {
        let conn = Connection::open(&paths.sessions_sqlite_path).unwrap();
        let held: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session_strand_messages \
                 WHERE session_id = ?1 AND strand = ?2",
                rusqlite::params![late.id().to_string(), head.strand.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            u64::try_from(held).unwrap() < head.message_count,
            "the head's own strand holds {held} of the head's {} rows, the others are served \
             through its link",
            head.message_count
        );
    }

    let output = run_doctor(
        &temp,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "storage",
            "doctor",
            "--json",
        ],
    );
    assert!(
        output.status.success(),
        "an Info finding never fails the doctor\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = parse_json(&output);
    let rows = findings_with_code(&report, "operator-instruction-entry-after-conversation");
    assert_eq!(
        rows.len(),
        1,
        "the session is counted from the rows its head serves: {report:#}"
    );
    let message = rows[0]["message"].as_str().unwrap();
    let expected = format!(
        "1 session(s) hold their own operator-instruction entry after a conversation row \
         (anthropic/claude-haiku-4-5: 1): {} (model anthropic/claude-haiku-4-5).",
        late.id()
    );
    assert!(
        message.starts_with(&expected),
        "the count, the model the head names, and the session: {message}"
    );
}

/// The same after two rewrites: the head's strand links to a strand that is
/// itself linked, so the rows before both splices are served two strands
/// away. Each rewrite replaces the last row only; the session's own entry
/// row stands outside both splices, on the strand the session was first
/// written to. The doctor follows both links and counts the session once,
/// with the model its head names. The row asserts first that the head's
/// strand holds fewer rows than the head counts and that the strand it links
/// to is itself a linked strand.
#[tokio::test]
async fn a_late_entry_two_rewrites_back_is_counted() {
    use meerkat_core::session_store::{
        IncrementalSessionStore, PreparedHeadCanonicalMutation,
        PreparedHeadCanonicalRewriteMutation,
    };
    use meerkat_core::{TranscriptRewriteReason, TranscriptRewriteSelection};

    let temp = TempDir::new().unwrap();
    let state_root = temp.path().join("realms");
    let paths = write_manifest(&state_root, "rewritten-twice-after-a-late-entry");
    let mut late = session_after_a_run_start_change("claude-haiku-4-5", true);
    let rows = late.messages().len();
    let head = {
        let store = meerkat_store::SqliteSessionStore::open(&paths.sessions_sqlite_path).unwrap();
        let root = PreparedHeadCanonicalMutation::prepare_root(&late).unwrap();
        let token = store
            .apply_prepared_head_canonical_mutation(&root)
            .await
            .unwrap();
        root.acknowledge_session(&mut late, &token).unwrap();
        let mut head = root.successor_head().clone();
        for said in ["next, said again", "next, said a third time"] {
            late.commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange {
                    start: rows - 1,
                    end: rows,
                },
                vec![Message::User(UserMessage::text(said))],
                TranscriptRewriteReason::new("doctor-census-fixture"),
                None,
                None,
            )
            .expect("the rewrite commits");
            let rewrite = PreparedHeadCanonicalRewriteMutation::prepare(&late, head).unwrap();
            store
                .apply_prepared_head_canonical_rewrite_mutation(&rewrite)
                .await
                .unwrap();
            rewrite
                .acknowledge_physical_projection(&mut late, rewrite.successor_head_token())
                .unwrap();
            head = rewrite.successor_head().clone();
        }
        head
    };
    {
        let conn = Connection::open(&paths.sessions_sqlite_path).unwrap();
        let held: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session_strand_messages \
                 WHERE session_id = ?1 AND strand = ?2",
                rusqlite::params![late.id().to_string(), head.strand.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            u64::try_from(held).unwrap() < head.message_count,
            "the head's own strand holds {held} of the head's {} rows, the others are served \
             through its link",
            head.message_count
        );
        let linked_twice: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session_strand_links AS first \
                 JOIN session_strand_links AS second \
                 ON second.session_id = first.session_id AND second.strand = first.successor \
                 WHERE first.session_id = ?1 AND first.strand = ?2",
                rusqlite::params![late.id().to_string(), head.strand.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            linked_twice, 1,
            "the strand the head's strand links to is itself a linked strand"
        );
    }

    let output = run_doctor(
        &temp,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "storage",
            "doctor",
            "--json",
        ],
    );
    assert!(
        output.status.success(),
        "an Info finding never fails the doctor\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = parse_json(&output);
    let rows = findings_with_code(&report, "operator-instruction-entry-after-conversation");
    assert_eq!(
        rows.len(),
        1,
        "the session is counted from the rows its head serves through both links: {report:#}"
    );
    let message = rows[0]["message"].as_str().unwrap();
    let expected = format!(
        "1 session(s) hold their own operator-instruction entry after a conversation row \
         (anthropic/claude-haiku-4-5: 1): {} (model anthropic/claude-haiku-4-5).",
        late.id()
    );
    assert!(
        message.starts_with(&expected),
        "the count, the model the head names, and the session: {message}"
    );
}

//! End-to-end coverage for `rkat storage migrate` / `rkat storage prune`
//! (Phase 6 of the storage unification arc): the offline migration
//! framework over explicit state roots.
//!
//! Hermeticity: every invocation passes explicit roots (`--state-root` /
//! `--root`) plus overridden `HOME`/`XDG_DATA_HOME`, so the developer's
//! real data dir is never read or written.

#![cfg(feature = "session-store")]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::fs::FileTimes;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

use meerkat_core::{Message, Session, UserMessage};
use rusqlite::Connection;
use tempfile::TempDir;

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

/// A legacy-shaped realm: raw `sessions` table, one unstamped
/// current-envelope session row, and no `meerkat_schema` ledger.
fn create_legacy_realm(state_root: &Path, realm_id: &str) -> PathBuf {
    let paths = write_manifest(state_root, realm_id);
    let conn = Connection::open(&paths.sessions_sqlite_path).unwrap();
    conn.execute_batch(SESSIONS_DDL).unwrap();
    let mut session = Session::new();
    session.push(Message::User(UserMessage::text("hello from 0.7.x")));
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

fn run_rkat(temp: &TempDir, args: &[&str]) -> Output {
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rkat"));
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

fn assert_success(output: &Output, operation: &str) {
    assert!(
        output.status.success(),
        "{operation} must exit 0\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn ledger_entries<'a>(
    realm: &'a serde_json::Value,
    domain: &str,
    database_suffix: &str,
) -> Vec<&'a serde_json::Value> {
    realm["ledger"]
        .as_array()
        .expect("ledger array")
        .iter()
        .filter(|entry| {
            entry["domain"] == domain
                && entry["database"]
                    .as_str()
                    .is_some_and(|db| db.ends_with(database_suffix))
        })
        .collect()
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
fn legacy_realm_dry_run_reports_then_apply_stamps_and_adopts_idempotently() {
    let temp = TempDir::new().unwrap();
    let state_root = temp.path().join("realms");
    let database = create_legacy_realm(&state_root, "legacy-shape");
    let before = std::fs::read(&database).unwrap();

    // Dry-run: pending baseline + legacy census, byte-identical database.
    let dry = run_rkat(
        &temp,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "storage",
            "migrate",
            "--json",
        ],
    );
    assert_success(&dry, "dry-run migrate");
    let report = parse_json(&dry);
    assert_eq!(report["mode"], "dry_run");
    let realms = report["realms"].as_array().expect("realms array");
    assert_eq!(realms.len(), 1, "{report:#}");
    assert_eq!(realms[0]["realm"], "legacy-shape");
    assert_eq!(realms[0]["backend"], "sqlite");
    let pending = ledger_entries(&realms[0], "session-store", "sessions.sqlite3");
    assert_eq!(pending.len(), 1, "{report:#}");
    assert_eq!(pending[0]["action"], "would-stamp");
    assert!(pending[0].get("before").is_none(), "no ledger row yet");
    let adoption = &realms[0]["adoption"];
    assert_eq!(adoption["legacy_pending"], 1, "{report:#}");
    assert!(
        adoption["skipped"]
            .as_str()
            .expect("dry-run adoption is a census")
            .contains("census"),
    );
    let after_dry = std::fs::read(&database).unwrap();
    assert_eq!(before, after_dry, "dry-run must not mutate the database");

    // Apply: ledgers stamped AND the legacy session adopted.
    let apply = run_rkat(
        &temp,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "storage",
            "migrate",
            "--apply",
            "--json",
        ],
    );
    assert_success(&apply, "apply migrate");
    let report = parse_json(&apply);
    assert_eq!(report["mode"], "apply");
    let realms = report["realms"].as_array().expect("realms array");
    assert_eq!(realms.len(), 1, "{report:#}");
    let stamped = ledger_entries(&realms[0], "session-store", "sessions.sqlite3");
    assert_eq!(stamped.len(), 1, "{report:#}");
    assert_eq!(stamped[0]["action"], "stamped");
    assert_eq!(stamped[0]["after"], 1);
    let adoption = &realms[0]["adoption"];
    assert_eq!(adoption["scanned"], 1, "{report:#}");
    assert_eq!(adoption["adopted"], 1, "{report:#}");
    assert_eq!(adoption["refused"].as_array().unwrap().len(), 0);

    // Doctor afterwards: no missing-ledger findings, census legacy = 0.
    let doctor = run_rkat(
        &temp,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "storage",
            "doctor",
            "--json",
        ],
    );
    assert_success(&doctor, "doctor after apply");
    let diagnosis = parse_json(&doctor);
    assert!(
        findings_with_code(&diagnosis, "no-schema-ledger").is_empty(),
        "{diagnosis:#}"
    );
    assert!(
        findings_with_code(&diagnosis, "legacy-unverified-sessions").is_empty(),
        "{diagnosis:#}"
    );

    // Idempotence: a second --apply is a no-op.
    let again = run_rkat(
        &temp,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "storage",
            "migrate",
            "--apply",
            "--json",
        ],
    );
    assert_success(&again, "second apply migrate");
    let report = parse_json(&again);
    let realms = report["realms"].as_array().expect("realms array");
    let entries = ledger_entries(&realms[0], "session-store", "sessions.sqlite3");
    assert_eq!(entries[0]["action"], "already-current");
    let adoption = &realms[0]["adoption"];
    assert_eq!(adoption["adopted"], 0, "{report:#}");
    assert_eq!(adoption["already_verified"], 1, "{report:#}");
}

#[test]
fn split_brain_refuses_without_adopt_root_then_archives_the_other_copy() {
    let temp = TempDir::new().unwrap();
    let root_a = temp.path().join("root-a");
    let root_b = temp.path().join("root-b");
    create_healthy_realm(&root_a, "team");
    create_healthy_realm(&root_b, "team");

    // Fail-closed refusal: exit 1 + divergence report as the whole output.
    let refused = run_rkat(
        &temp,
        &[
            "storage",
            "migrate",
            "--json",
            "--root",
            root_a.to_str().unwrap(),
            "--root",
            root_b.to_str().unwrap(),
        ],
    );
    assert_eq!(
        refused.status.code(),
        Some(1),
        "split-brain without --adopt-root must exit 1\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&refused.stdout),
        String::from_utf8_lossy(&refused.stderr)
    );
    let report = parse_json(&refused);
    let split = report["split_brain"].as_array().expect("split_brain array");
    assert_eq!(split.len(), 1, "{report:#}");
    assert_eq!(split[0]["realm"], "team");
    assert_eq!(split[0]["resolution"]["kind"], "refused");
    assert_eq!(split[0]["locations"].as_array().unwrap().len(), 2);
    assert!(
        report["realms"].as_array().unwrap().is_empty(),
        "the refusal is the whole output: {report:#}"
    );
    assert!(!report["errors"].as_array().unwrap().is_empty());
    // Refusal moved nothing.
    assert!(root_a.join("team").is_dir());
    assert!(root_b.join("team").is_dir());

    // Resolution: adopt root A; root B's copy is archived read-only.
    let resolved = run_rkat(
        &temp,
        &[
            "storage",
            "migrate",
            "--apply",
            "--json",
            "--root",
            root_a.to_str().unwrap(),
            "--root",
            root_b.to_str().unwrap(),
            "--adopt-root",
            root_a.to_str().unwrap(),
        ],
    );
    assert_success(&resolved, "migrate --apply --adopt-root");
    let report = parse_json(&resolved);
    let split = report["split_brain"].as_array().expect("split_brain array");
    assert_eq!(split[0]["resolution"]["kind"], "archived", "{report:#}");
    let archived: Vec<PathBuf> = split[0]["resolution"]["archived"]
        .as_array()
        .expect("archived array")
        .iter()
        .map(|value| PathBuf::from(value.as_str().unwrap()))
        .collect();
    assert_eq!(archived.len(), 1, "{report:#}");
    // The archive exists under the registered backup naming; original gone.
    assert!(archived[0].is_dir(), "archive must exist: {archived:?}");
    let archive_name = archived[0].file_name().unwrap().to_str().unwrap();
    assert!(archive_name.starts_with("team.pre-"), "{archive_name}");
    assert!(archive_name.ends_with(".split-brain"), "{archive_name}");
    assert!(!root_b.join("team").exists(), "original copy must be gone");
    assert!(
        archived[0].join("sessions.sqlite3").is_file(),
        "archive preserves the database"
    );
    // The adopted copy stays where it lies, untouched.
    assert!(root_a.join("team").is_dir());
    assert!(root_a.join("team/sessions.sqlite3").is_file());
    assert!(root_a.join("team/realm_manifest.json").is_file());
    // The adopted realm then went through cases 1/4 in the same run.
    let realms = report["realms"].as_array().expect("realms array");
    assert_eq!(realms.len(), 1, "{report:#}");
    assert_eq!(realms[0]["realm"], "team");
    assert_eq!(
        realms[0]["root"].as_str().unwrap(),
        root_a.join("team").to_str().unwrap()
    );

    // A subsequent doctor sweep shows no split-brain.
    let doctor = run_rkat(
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
    assert_success(&doctor, "doctor after split-brain resolution");
    let diagnosis = parse_json(&doctor);
    assert!(
        findings_with_code(&diagnosis, "split-brain-realm").is_empty(),
        "{diagnosis:#}"
    );
    // The archive is inventoried as a backup artifact instead.
    assert!(
        !findings_with_code(&diagnosis, "backup-artifact").is_empty(),
        "{diagnosis:#}"
    );
}

fn backdate(path: &Path, days: u64) {
    let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    file.set_times(
        FileTimes::new()
            .set_modified(SystemTime::now() - Duration::from_secs(days * 86_400 + 3_600)),
    )
    .unwrap();
}

#[test]
fn prune_lists_then_deletes_only_registered_artifacts() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("realms");
    let paths = write_manifest(&root, "artifacts");
    // Registered artifacts: an old backup, a young backup, an old
    // quarantine, and a root-level archived realm directory.
    let old_backup = paths.root.join("sessions.sqlite3.pre-0.0.1-1700000000");
    std::fs::write(&old_backup, b"old-backup").unwrap();
    backdate(&old_backup, 90);
    let young_backup = paths.root.join("runtime.sqlite3.pre-0.0.1-1750000000");
    std::fs::write(&young_backup, b"young-backup").unwrap();
    let jsonl_dir = paths.root.join("sessions_jsonl");
    std::fs::create_dir_all(&jsonl_dir).unwrap();
    let quarantine = jsonl_dir.join("session_index.sqlite3.corrupt-42");
    std::fs::write(&quarantine, b"quarantine").unwrap();
    backdate(&quarantine, 90);
    let archived_dir = root.join("old-team.pre-0.0.1-1700000000.split-brain");
    std::fs::create_dir_all(&archived_dir).unwrap();
    std::fs::write(archived_dir.join("sessions.sqlite3"), b"frozen").unwrap();
    // Distractors that must never be touched.
    let live_db = paths.root.join("sessions.sqlite3");
    std::fs::write(&live_db, b"live").unwrap();
    let notes = paths.root.join("notes.txt");
    std::fs::write(&notes, b"keep me").unwrap();

    // Dry-run: lists with sizes/ages; deletes nothing.
    let dry = run_rkat(
        &temp,
        &[
            "storage",
            "prune",
            "--json",
            "--root",
            root.to_str().unwrap(),
        ],
    );
    assert_success(&dry, "prune dry-run");
    let report = parse_json(&dry);
    assert_eq!(report["mode"], "dry_run");
    assert_eq!(report["older_than_days"], 30);
    let artifacts = report["artifacts"].as_array().expect("artifacts array");
    assert_eq!(artifacts.len(), 4, "{report:#}");
    let action_of = |suffix: &str| {
        artifacts
            .iter()
            .find(|artifact| artifact["path"].as_str().unwrap().ends_with(suffix))
            .unwrap_or_else(|| panic!("artifact {suffix} missing: {report:#}"))["action"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(
        action_of("sessions.sqlite3.pre-0.0.1-1700000000"),
        "would-delete"
    );
    assert_eq!(
        action_of("session_index.sqlite3.corrupt-42"),
        "would-delete"
    );
    assert_eq!(action_of("runtime.sqlite3.pre-0.0.1-1750000000"), "kept");
    assert!(old_backup.is_file(), "dry-run deletes nothing");

    // Apply with the default 30-day threshold: old artifacts deleted, young
    // kept, distractors untouched.
    let apply = run_rkat(
        &temp,
        &[
            "storage",
            "prune",
            "--apply",
            "--json",
            "--root",
            root.to_str().unwrap(),
        ],
    );
    assert_success(&apply, "prune --apply");
    let report = parse_json(&apply);
    assert_eq!(report["mode"], "apply");
    assert!(!old_backup.exists(), "old backup must be deleted");
    assert!(!quarantine.exists(), "old quarantine must be deleted");
    assert!(young_backup.is_file(), "young backup must be kept");
    assert!(archived_dir.is_dir(), "young archive dir must be kept");
    assert!(live_db.is_file(), "live database must never be touched");
    assert!(notes.is_file(), "unregistered files must never be touched");

    // --older-than-days 0 deletes every registered artifact.
    let all = run_rkat(
        &temp,
        &[
            "storage",
            "prune",
            "--apply",
            "--older-than-days",
            "0",
            "--json",
            "--root",
            root.to_str().unwrap(),
        ],
    );
    assert_success(&all, "prune --apply --older-than-days 0");
    assert!(!young_backup.exists());
    assert!(!archived_dir.exists());
    assert!(live_db.is_file());
    assert!(notes.is_file());
}

#[test]
fn foreign_fence_holder_fails_migrate_apply_typed() {
    let temp = TempDir::new().unwrap();
    let state_root = temp.path().join("realms");
    let database = create_legacy_realm(&state_root, "fenced");
    let before = std::fs::read(&database).unwrap();

    // THIS process plays the foreign maintenance holder: a raw exclusive
    // lock on the fence lock file. The rkat child process must fail typed.
    let lock_path = state_root.join("fenced").join("sessions.sqlite3.mfence");
    let holder = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .unwrap();
    holder.try_lock().unwrap();

    let output = run_rkat(
        &temp,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "storage",
            "migrate",
            "--apply",
            "--fence-wait-secs",
            "0",
            "--json",
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(1),
        "held fence must fail migrate --apply\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = parse_json(&output);
    let realms = report["realms"].as_array().expect("realms array");
    assert_eq!(realms.len(), 1, "{report:#}");
    let errors = realms[0]["errors"].as_array().expect("realm errors");
    assert_eq!(errors.len(), 1, "{report:#}");
    let message = errors[0].as_str().unwrap();
    assert!(
        message.contains("maintenance fence"),
        "error must name the fence: {message}"
    );
    assert!(
        message.contains("sessions.sqlite3"),
        "error must name the fenced database: {message}"
    );
    drop(holder);

    // Nothing was migrated under the refused fence.
    let after = std::fs::read(&database).unwrap();
    assert_eq!(before, after, "refused migrate must not touch the database");
}

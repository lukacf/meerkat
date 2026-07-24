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
    let jobs = ledger_entries(&realms[0], "jobs", "jobs.sqlite3");
    assert_eq!(jobs.len(), 1, "{report:#}");
    assert_eq!(jobs[0]["action"], "stamped");
    assert_eq!(jobs[0]["after"], 1);
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
    let jobs = ledger_entries(&realms[0], "jobs", "jobs.sqlite3");
    assert_eq!(jobs[0]["action"], "already-current");
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
    // Fence-before-compare evidence: every copy is fenced for the whole
    // compare-to-archive interval, and the archived copy's released fence
    // lock files ride into the archive.
    assert!(
        archived[0].join("realm.mfence").is_file(),
        "archive must carry the realm write-admission lock file"
    );
    assert!(
        archived[0].join("sessions.sqlite3.mfence").is_file(),
        "archive must carry the per-database fence lock files"
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
    let young_stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs()
        - 86_400;
    let young_name = format!("runtime.sqlite3.pre-0.0.1-{young_stamp}");
    let young_backup = paths.root.join(&young_name);
    std::fs::write(&young_backup, b"young-backup").unwrap();
    let jsonl_dir = paths.root.join("sessions_jsonl");
    std::fs::create_dir_all(&jsonl_dir).unwrap();
    let quarantine = jsonl_dir.join("session_index.sqlite3.corrupt-42");
    std::fs::write(&quarantine, b"quarantine").unwrap();
    backdate(&quarantine, 90);
    let archived_dir = root.join(format!("old-team.pre-0.0.1-{young_stamp}.split-brain"));
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
    assert_eq!(action_of(&young_name), "kept");
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
fn prune_realm_filter_deletes_only_that_realms_artifacts() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("realms");
    let alpha = write_manifest(&root, "alpha");
    let beta = write_manifest(&root, "beta");
    let alpha_backup = alpha.root.join("sessions.sqlite3.pre-0.0.1-1700000000");
    std::fs::write(&alpha_backup, b"alpha-backup").unwrap();
    let beta_backup = beta.root.join("sessions.sqlite3.pre-0.0.1-1700000000");
    std::fs::write(&beta_backup, b"beta-backup").unwrap();
    // Root-level whole-realm archives carrying each realm's identity.
    let alpha_archive = root.join("alpha.pre-0.0.1-1700000000.split-brain");
    std::fs::create_dir_all(&alpha_archive).unwrap();
    std::fs::write(alpha_archive.join("sessions.sqlite3"), b"frozen").unwrap();
    let beta_archive = root.join("beta.pre-0.0.1-1700000000.split-brain");
    std::fs::create_dir_all(&beta_archive).unwrap();
    // Root-level file archive with no realm identity: a scoped prune must
    // never delete an unattributed preserved copy.
    let orphan_archive = root.join("orphan.sqlite3.pre-0.0.1-1700000000");
    std::fs::write(&orphan_archive, b"orphan").unwrap();

    let output = run_rkat(
        &temp,
        &[
            "--realm",
            "alpha",
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
    assert_success(&output, "realm-scoped prune --apply");
    let report = parse_json(&output);
    let artifacts = report["artifacts"].as_array().expect("artifacts array");
    assert_eq!(artifacts.len(), 2, "only alpha's artifacts: {report:#}");
    assert!(
        artifacts
            .iter()
            .all(|artifact| artifact["action"] == "deleted"),
        "{report:#}"
    );
    assert!(!alpha_backup.exists(), "alpha's backup must be deleted");
    assert!(!alpha_archive.exists(), "alpha's archive must be deleted");
    assert!(beta_backup.is_file(), "beta's backup must survive");
    assert!(beta_archive.is_dir(), "beta's archive must survive");
    assert!(
        orphan_archive.is_file(),
        "unattributed root-level archive must survive a scoped prune"
    );
}

#[test]
fn explicit_roots_skip_the_ambient_legacy_home_probe() {
    let temp = TempDir::new().unwrap();
    // A legacy pre-realm sessions directory under the (overridden) home.
    let legacy_sessions = temp.path().join(".rkat").join("sessions");
    std::fs::create_dir_all(&legacy_sessions).unwrap();
    std::fs::write(legacy_sessions.join("old.jsonl"), b"{}").unwrap();
    let state_root = temp.path().join("realms");
    create_healthy_realm(&state_root, "scoped");

    // Explicit scope sweeps ONLY the given roots — no ambient home probe.
    let scoped = run_rkat(
        &temp,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "storage",
            "migrate",
            "--json",
        ],
    );
    assert_success(&scoped, "explicitly scoped migrate");
    let report = parse_json(&scoped);
    assert!(
        findings_with_code(&report, "legacy-home-sessions-dir").is_empty(),
        "explicit roots must not probe the ambient home: {report:#}"
    );

    // The ambient dual-root sweep still reports the probe (report-only).
    let ambient = run_rkat(&temp, &["storage", "migrate", "--json"]);
    assert_success(&ambient, "ambient migrate");
    let report = parse_json(&ambient);
    assert_eq!(
        findings_with_code(&report, "legacy-home-sessions-dir").len(),
        1,
        "{report:#}"
    );
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

#[test]
fn foreign_fence_holder_blocks_split_brain_resolution_before_any_compare_or_archive() {
    let temp = TempDir::new().unwrap();
    let root_a = temp.path().join("root-a");
    let root_b = temp.path().join("root-b");
    create_healthy_realm(&root_a, "team");
    create_healthy_realm(&root_b, "team");

    // Foreign holder on ONE copy's per-database fence: resolution fences
    // every copy BEFORE comparing, so it must refuse with no divergence
    // computed and nothing archived.
    let lock_path = root_b.join("team").join("sessions.sqlite3.mfence");
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
            "--fence-wait-secs",
            "0",
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(1),
        "held fence must refuse split-brain resolution\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = parse_json(&output);
    let split = report["split_brain"].as_array().expect("split_brain array");
    assert_eq!(split.len(), 1, "{report:#}");
    assert_eq!(split[0]["resolution"]["kind"], "refused", "{report:#}");
    let reason = split[0]["resolution"]["reason"].as_str().unwrap();
    assert!(
        reason.contains("maintenance fence"),
        "refusal must name the fence: {reason}"
    );
    // The comparison never ran under a broken fence: no divergence entries.
    assert!(split[0]["sessions"].as_array().unwrap().is_empty());
    assert!(split[0]["files"].as_array().unwrap().is_empty());
    // Nothing moved.
    assert!(root_a.join("team").is_dir());
    assert!(root_b.join("team").is_dir());
    drop(holder);
}

#[test]
fn ledger_baseline_read_failures_and_future_versions_are_refusals_not_would_stamp() {
    let temp = TempDir::new().unwrap();
    let state_root = temp.path().join("realms");
    let paths = write_manifest(&state_root, "poisoned");
    // An unreadable database must surface as a per-realm error, never as a
    // missing ledger the dry-run could report as would-stamp.
    std::fs::write(paths.root.join("tasks.db"), b"this is not sqlite").unwrap();
    // A future-versioned domain must be reported as a refusal exactly as
    // `--apply`'s guarded constructor would refuse it.
    let jsonl_dir = paths.root.join("sessions_jsonl");
    std::fs::create_dir_all(&jsonl_dir).unwrap();
    {
        let conn = Connection::open(jsonl_dir.join("session_index.sqlite3")).unwrap();
        conn.execute_batch(
            "CREATE TABLE meerkat_schema (domain TEXT PRIMARY KEY, version INTEGER NOT NULL);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO meerkat_schema VALUES ('jsonl-index', 9223372036854775807)",
            [],
        )
        .unwrap();
    }

    let output = run_rkat(
        &temp,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "storage",
            "migrate",
            "--json",
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(1),
        "baseline read failures and future versions must exit 1\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = parse_json(&output);
    let realms = report["realms"].as_array().expect("realms array");
    assert_eq!(realms.len(), 1, "{report:#}");
    let errors: Vec<&str> = realms[0]["errors"]
        .as_array()
        .expect("realm errors")
        .iter()
        .map(|error| error.as_str().unwrap())
        .collect();
    assert!(
        errors
            .iter()
            .any(|error| error.contains("ledger unreadable") && error.contains("tasks.db")),
        "unreadable database must surface typed: {errors:?}"
    );
    assert!(
        errors
            .iter()
            .any(|error| error.contains("from the future") && error.contains("jsonl-index")),
        "future version must surface as a refusal: {errors:?}"
    );
    // The unreadable database contributes no ledger rows (no would-stamp
    // laundering)...
    assert!(
        ledger_entries(&realms[0], "tools-tasks", "tasks.db").is_empty(),
        "{report:#}"
    );
    // ...and the future domain's row is report-only, never would-stamp.
    let future_rows = ledger_entries(&realms[0], "jsonl-index", "session_index.sqlite3");
    assert_eq!(future_rows.len(), 1, "{report:#}");
    assert_eq!(future_rows[0]["action"], "report-only", "{report:#}");
    assert_eq!(future_rows[0]["before"], 9_223_372_036_854_775_807_i64);
}

#[cfg(unix)]
#[test]
fn partial_archive_failure_reports_archive_failed_with_completed_archives_visible() {
    use std::os::unix::fs::PermissionsExt as _;
    let temp = TempDir::new().unwrap();
    let root_a = temp.path().join("root-a");
    let root_b = temp.path().join("root-b");
    let root_c = temp.path().join("root-c");
    create_healthy_realm(&root_a, "team");
    create_healthy_realm(&root_b, "team");
    create_healthy_realm(&root_c, "team");
    // root-c refuses renames (its realm directory cannot be moved within
    // it), so the third copy's archive fails after root-b's succeeded.
    std::fs::set_permissions(&root_c, std::fs::Permissions::from_mode(0o555)).unwrap();

    let output = run_rkat(
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
            "--root",
            root_c.to_str().unwrap(),
            "--adopt-root",
            root_a.to_str().unwrap(),
        ],
    );
    // Restore write permission before asserting so tempdir cleanup works
    // even when an assertion fails.
    std::fs::set_permissions(&root_c, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(
        output.status.code(),
        Some(1),
        "partial archive must exit 1\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = parse_json(&output);
    let split = report["split_brain"].as_array().expect("split_brain array");
    assert_eq!(split.len(), 1, "{report:#}");
    let resolution = &split[0]["resolution"];
    assert_eq!(resolution["kind"], "archive_failed", "{report:#}");
    assert_eq!(
        resolution["adopted"].as_str().unwrap(),
        root_a.join("team").to_str().unwrap()
    );
    // The archive that succeeded before the failure stays visible AND on
    // disk — a partial archive must never be reported as "nothing moved".
    let archived = resolution["archived"].as_array().expect("archived array");
    assert_eq!(archived.len(), 1, "{report:#}");
    let archive_path = PathBuf::from(archived[0].as_str().unwrap());
    assert!(
        archive_path.is_dir(),
        "completed archive must exist: {archive_path:?}"
    );
    let reason = resolution["reason"].as_str().unwrap();
    assert!(
        reason.contains("root-c"),
        "reason names the failure: {reason}"
    );
    assert!(
        !root_b.join("team").exists(),
        "root-b's copy was archived before the failure"
    );
    assert!(root_c.join("team").is_dir(), "the failed copy stays put");
    assert!(
        root_a.join("team").is_dir(),
        "the adopted copy is untouched"
    );
    // Partial resolution is a hard error; the realm is not migrated in the
    // same run.
    assert!(!report["errors"].as_array().unwrap().is_empty());
    assert!(
        report["realms"].as_array().unwrap().is_empty(),
        "{report:#}"
    );
}

#[cfg(unix)]
#[test]
fn adopt_root_refuses_when_the_comparison_is_inconclusive() {
    let temp = TempDir::new().unwrap();
    let root_a = temp.path().join("root-a");
    let root_b = temp.path().join("root-b");
    create_healthy_realm(&root_a, "team");
    create_healthy_realm(&root_b, "team");
    // A symlink inside an authoritative tree poisons its entry as Unknown:
    // the comparison cannot account for it, so no archive decision may rest
    // on the report.
    let blobs = root_b.join("team").join("blobs");
    std::fs::create_dir_all(&blobs).unwrap();
    std::os::unix::fs::symlink("missing-target", blobs.join("ghost")).unwrap();

    let output = run_rkat(
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
    assert_eq!(
        output.status.code(),
        Some(1),
        "inconclusive comparison must refuse the archive decision\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = parse_json(&output);
    let split = report["split_brain"].as_array().expect("split_brain array");
    assert_eq!(split.len(), 1, "{report:#}");
    assert_eq!(split[0]["resolution"]["kind"], "refused", "{report:#}");
    let reason = split[0]["resolution"]["reason"].as_str().unwrap();
    assert!(
        reason.contains("inconclusive"),
        "refusal names the inconclusive comparison: {reason}"
    );
    // The poisoned entry is visible as unknown, and nothing moved.
    assert!(
        split[0]["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|file| file["status"]["kind"] == "unknown"),
        "{report:#}"
    );
    assert!(root_a.join("team").is_dir());
    assert!(root_b.join("team").is_dir());
}

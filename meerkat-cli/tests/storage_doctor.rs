//! End-to-end coverage for `rkat storage doctor` (Phase 1 of the storage
//! unification arc): read-only diagnosis over explicit state roots.
//!
//! Hermeticity: every invocation passes explicit roots (`--state-root` /
//! `--root`) plus overridden `HOME`/`XDG_DATA_HOME`, so the developer's real
//! data dir is never read.

#![cfg(feature = "session-store")]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use meerkat_core::{BlobId, ContentBlock, ImageData, Message, Session, UserMessage};
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

fn run_doctor(temp: &TempDir, args: &[&str]) -> Output {
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
    assert!(
        domains
            .iter()
            .any(|pair| pair[0] == "session-store" && pair[1] == 1),
        "session-store domain must be ledger-stamped at v1: {domains:?}"
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

#[test]
fn legacy_realm_reports_no_ledger_and_census_without_mutating() {
    let temp = TempDir::new().unwrap();
    let state_root = temp.path().join("realms");
    let database = create_legacy_realm(&state_root, "legacy-shape");
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
        "info/warning findings must exit 0\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = parse_json(&output);

    let no_ledger = findings_with_code(&report, "no-schema-ledger");
    assert_eq!(no_ledger.len(), 1, "{report:#}");
    assert_eq!(no_ledger[0]["severity"], "info");

    let census = findings_with_code(&report, "legacy-unverified-sessions");
    assert_eq!(census.len(), 1, "{report:#}");
    assert_eq!(census[0]["severity"], "warning");
    assert_eq!(census[0]["realm"], "legacy-shape");
    assert!(
        census[0]["message"]
            .as_str()
            .unwrap()
            .starts_with("1 legacy-unverified"),
        "{census:?}"
    );

    // Read-only proof: the database bytes are untouched (no ledger was
    // created, no pragmas rewritten).
    let after = std::fs::read(&database).unwrap();
    assert_eq!(before, after, "doctor must not mutate the database");
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

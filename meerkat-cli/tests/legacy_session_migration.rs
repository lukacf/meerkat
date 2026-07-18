#![cfg(feature = "session-store")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use rusqlite::{Connection, params};
use serde::Deserialize;
use tempfile::TempDir;

const REALM_ID: &str = "legacy-v0-6-34";
const SESSION_ID: &str = "018f0000-0000-7000-8000-000000000001";
const INPUT_ID: &str = "018f0000-0000-7000-8000-000000000002";
const RUN_ID: &str = "018f0000-0000-7000-8000-000000000003";

fn fixture(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../meerkat-runtime/tests/fixtures/v0_6_34_completed_idle")
        .join(name);
    let mut bytes = std::fs::read(path).unwrap();
    // Repository text fixtures end with LF, while v0.6.34 persisted
    // the exact compact bytes emitted by `serde_json::to_vec`.
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    bytes
}

#[derive(Deserialize)]
struct LegacySessionRow {
    session_id: String,
    created_at_ms: i64,
    updated_at_ms: i64,
    message_count: i64,
    total_tokens: i64,
    metadata_json: String,
}

fn create_legacy_realm(state_root: &Path) -> PathBuf {
    let paths = meerkat_store::realm_paths_in(state_root, REALM_ID);
    std::fs::create_dir_all(&paths.root).unwrap();
    std::fs::write(&paths.manifest_path, fixture("realm_manifest.json")).unwrap();
    let conn = Connection::open(&paths.sessions_sqlite_path).unwrap();
    conn.execute_batch(std::str::from_utf8(&fixture("schema.sql")).unwrap())
        .unwrap();
    let row: LegacySessionRow = serde_json::from_slice(&fixture("session_row.json")).unwrap();
    let runtime_id = format!("rt:session:{}", row.session_id);
    conn.execute(
        "INSERT INTO sessions (session_id, created_at_ms, updated_at_ms, message_count, total_tokens, metadata_json, session_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            row.session_id,
            row.created_at_ms,
            row.updated_at_ms,
            row.message_count,
            row.total_tokens,
            row.metadata_json,
            fixture("session_snapshot.json"),
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO runtime_states (runtime_id, runtime_state_json) VALUES (?1, ?2)",
        params![runtime_id, fixture("runtime_state.json")],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO runtime_session_snapshots (runtime_id, session_snapshot) VALUES (?1, ?2)",
        params![runtime_id, fixture("session_snapshot.json")],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO runtime_input_states (runtime_id, input_id, state_json) VALUES (?1, ?2, ?3)",
        params![runtime_id, INPUT_ID, fixture("input_state_consumed.json")],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO runtime_boundary_receipts (runtime_id, run_id, sequence, receipt_json) VALUES (?1, ?2, 1, ?3)",
        params![runtime_id, RUN_ID, fixture("boundary_receipt.json")],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO runtime_ops_lifecycle (runtime_id, state_json) VALUES (?1, ?2)",
        params![runtime_id, fixture("ops_lifecycle_empty.json")],
    )
    .unwrap();
    drop(conn);
    paths.sessions_sqlite_path
}

fn run_rkat(temp: &TempDir, project: &Path, state_root: &Path, args: &[&str]) -> Output {
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rkat"));
    let mut command = Command::new(binary);
    command
        .current_dir(project)
        .env("HOME", temp.path())
        .env("XDG_DATA_HOME", temp.path().join("data"))
        .env("RKAT_TEST_CLIENT", "1")
        .arg("--context-root")
        .arg(project)
        .arg("--state-root")
        .arg(state_root)
        .arg("--realm")
        .arg(REALM_ID)
        .args(args);
    command.output().unwrap()
}

fn assert_success(output: &Output, operation: &str) {
    assert!(
        output.status.success(),
        "{operation} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn cli_migrates_complete_v0_6_34_realm_then_lists_and_shows_session() {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("project");
    std::fs::create_dir_all(project.join(".rkat")).unwrap();
    let config = toml::to_string_pretty(&meerkat::Config::default()).unwrap();
    std::fs::write(project.join(".rkat/config.toml"), config).unwrap();
    let state_root = temp.path().join("realms");
    let database = create_legacy_realm(&state_root);

    let dry_run = run_rkat(
        &temp,
        &project,
        &state_root,
        &["session", "migrate", "--json"],
    );
    assert_success(&dry_run, "migration dry-run");
    let report: serde_json::Value = serde_json::from_slice(&dry_run.stdout).unwrap();
    assert_eq!(report["applied"], false);
    assert_eq!(report["items"][0]["disposition"], "would_migrate");
    let conn = Connection::open(&database).unwrap();
    let before: Vec<u8> = conn
        .query_row("SELECT runtime_state_json FROM runtime_states", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(before, b"\"idle\"");
    drop(conn);

    let lease_dir =
        meerkat_store::realm_lease_dir(&meerkat_store::realm_paths_in(&state_root, REALM_ID));
    std::fs::create_dir_all(&lease_dir).unwrap();
    let unreadable_lease = lease_dir.join("unknown-live-process.json");
    std::fs::write(&unreadable_lease, b"not-json").unwrap();
    let blocked_apply = run_rkat(
        &temp,
        &project,
        &state_root,
        &["session", "migrate", "--apply", "--json"],
    );
    assert!(
        !blocked_apply.status.success(),
        "unreadable liveness evidence must block apply"
    );
    std::fs::remove_file(unreadable_lease).unwrap();

    let apply = run_rkat(
        &temp,
        &project,
        &state_root,
        &["session", "migrate", "--apply", "--json"],
    );
    assert_success(&apply, "migration apply");
    let report: serde_json::Value = serde_json::from_slice(&apply.stdout).unwrap();
    assert_eq!(report["applied"], true);
    assert_eq!(report["items"][0]["disposition"], "migrated");

    let list = run_rkat(&temp, &project, &state_root, &["session", "list"]);
    assert_success(&list, "session list after migration");
    assert!(String::from_utf8_lossy(&list.stdout).contains(SESSION_ID));

    let show = run_rkat(
        &temp,
        &project,
        &state_root,
        &["session", "show", SESSION_ID],
    );
    assert_success(&show, "session show after migration");
    assert!(String::from_utf8_lossy(&show.stdout).contains(&format!("Session: {SESSION_ID}")));
}

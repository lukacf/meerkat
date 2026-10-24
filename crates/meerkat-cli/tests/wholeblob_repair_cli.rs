//! `rkat session repair-wholeblob` opens only the realm's runtime database.
//!
//! An operator diagnose must leave the realm directory as it found it: no
//! realm manifest, no session/jobs/workgraph stores, and a typed refusal when
//! the database is missing or the realm id is not a directory name.
//!
//! `rkat session resolve-held-tail` follows the same operator rules: with
//! `--runtime-store` the id must be a full session UUID, and a missing
//! database or realm manifest refuses typed without creating anything.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::disallowed_methods
)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use meerkat_core::Session;
use meerkat_core::types::{Message, UserMessage};
use meerkat_runtime::identifiers::LogicalRuntimeId;
use meerkat_runtime::store::SqliteRuntimeStore;
use tempfile::TempDir;

fn run_rkat(temp: &TempDir, args: &[&str]) -> Output {
    let binary = std::env::var_os("CARGO_BIN_EXE_rkat")
        .map(PathBuf::from)
        .expect("Cargo or Nextest must provide the rkat binary path");
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

async fn seed_whole_blob_realm(realms_root: &Path, realm: &str) -> (String, usize) {
    let realm_dir = realms_root.join(realm);
    std::fs::create_dir_all(&realm_dir).unwrap();
    let store = SqliteRuntimeStore::new_whole_blob(realm_dir.join("runtime.sqlite3"))
        .expect("whole-blob sqlite store");
    let mut session = Session::new();
    session.append_system_message("system prompt");
    for turn in 0..3 {
        session.push(Message::User(UserMessage::text(format!("question {turn}"))));
    }
    let rows = session.messages().len();
    let session_id = session.id().clone();
    let runtime_id = LogicalRuntimeId::for_session(&session_id);
    let bytes = serde_json::to_vec(&session).expect("plain serde encodes");
    store
        .inject_wedged_whole_blob_for_test(&runtime_id, &session_id, bytes, Vec::new())
        .await
        .expect("commit a body");
    drop(store);
    (session_id.to_string(), rows)
}

fn directory_entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn diagnose_opens_only_the_runtime_database_and_reports_the_row_count() {
    let temp = TempDir::new().unwrap();
    let realms_root = temp.path().join("realms");
    let (session_id, rows) = seed_whole_blob_realm(&realms_root, "gateway").await;
    let realm_dir = realms_root.join("gateway");
    let before = directory_entries(&realm_dir);

    let output = run_rkat(
        &temp,
        &[
            "--state-root",
            realms_root.to_str().unwrap(),
            "--realm",
            "gateway",
            "session",
            "repair-wholeblob",
            &session_id,
            "--json",
        ],
    );
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["action"], "no_repair_needed");
    assert_eq!(report["live_row_count"], rows);

    let after = directory_entries(&realm_dir);
    for forbidden in [
        "realm_manifest.json",
        "sessions.sqlite3",
        "jobs.sqlite3",
        "workgraph.sqlite3",
        "sessions_jsonl",
    ] {
        assert!(
            !after.iter().any(|name| name == forbidden),
            "diagnose created {forbidden}; before {before:?}, after {after:?}"
        );
    }
    for name in &after {
        assert!(
            name.starts_with("runtime.sqlite3"),
            "unexpected file {name} after diagnose; before {before:?}, after {after:?}"
        );
    }
}

#[test]
fn missing_runtime_database_is_a_typed_refusal_without_side_effects() {
    let temp = TempDir::new().unwrap();
    let realms_root = temp.path().join("realms");
    std::fs::create_dir_all(realms_root.join("empty")).unwrap();
    let output = run_rkat(
        &temp,
        &[
            "--state-root",
            realms_root.to_str().unwrap(),
            "--realm",
            "empty",
            "session",
            "repair-wholeblob",
            "01a000bb-b69e-7570-933d-ffd5d61d51ee",
        ],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no runtime database at"), "{stderr}");
    assert!(stderr.contains("runtime.sqlite3"), "{stderr}");
    assert!(
        directory_entries(&realms_root.join("empty")).is_empty(),
        "a refused diagnose must not create anything"
    );
}

#[test]
fn mob_scope_realm_id_is_refused_with_the_directory_name_rule() {
    let temp = TempDir::new().unwrap();
    let realms_root = temp.path().join("realms");
    std::fs::create_dir_all(&realms_root).unwrap();
    let output = run_rkat(
        &temp,
        &[
            "--state-root",
            realms_root.to_str().unwrap(),
            "--realm",
            "mob.example",
            "session",
            "repair-wholeblob",
            "01a000bb-b69e-7570-933d-ffd5d61d51ee",
        ],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid explicit realm id: mob.example"),
        "{stderr}"
    );
    assert!(stderr.contains("`mobkit`"), "{stderr}");
    assert!(stderr.contains("mob scope, not a store realm"), "{stderr}");
    assert!(
        directory_entries(&realms_root).is_empty(),
        "a refused realm id must not create a realm directory"
    );
}

#[test]
fn resolve_held_tail_with_a_runtime_store_takes_a_full_session_uuid() {
    let temp = TempDir::new().unwrap();
    let host = temp.path().join("host");
    std::fs::create_dir_all(&host).unwrap();
    let database = host.join("runtime.sqlite");
    let output = run_rkat(
        &temp,
        &[
            "session",
            "resolve-held-tail",
            "gateway:not-a-session-uuid",
            "--apply",
            "--runtime-store",
            database.to_str().unwrap(),
            "--json",
        ],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--runtime-store takes a full session UUID"),
        "{stderr}"
    );
    assert!(
        output.stdout.is_empty(),
        "a refused id prints no report: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        directory_entries(&host).is_empty(),
        "a refused id must not open or create the runtime database"
    );
}

#[test]
fn resolve_held_tail_over_a_missing_runtime_store_is_a_typed_refusal_without_side_effects() {
    let temp = TempDir::new().unwrap();
    let host = temp.path().join("host");
    std::fs::create_dir_all(&host).unwrap();
    let database = host.join("runtime.sqlite");
    let output = run_rkat(
        &temp,
        &[
            "session",
            "resolve-held-tail",
            "01a000bb-b69e-7570-933d-ffd5d61d51ee",
            "--runtime-store",
            database.to_str().unwrap(),
        ],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cannot open the runtime store"), "{stderr}");
    assert!(stderr.contains("no runtime database at"), "{stderr}");
    assert!(
        directory_entries(&host).is_empty(),
        "a refused open must not create the runtime database"
    );
}

#[test]
fn resolve_held_tail_in_a_realm_without_a_manifest_refuses_without_side_effects() {
    let temp = TempDir::new().unwrap();
    let realms_root = temp.path().join("realms");
    std::fs::create_dir_all(realms_root.join("empty")).unwrap();
    let output = run_rkat(
        &temp,
        &[
            "--state-root",
            realms_root.to_str().unwrap(),
            "--realm",
            "empty",
            "session",
            "resolve-held-tail",
            "01a000bb-b69e-7570-933d-ffd5d61d51ee",
        ],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot read the manifest of realm 'empty'"),
        "{stderr}"
    );
    assert!(stderr.contains("--runtime-store"), "{stderr}");
    assert!(
        directory_entries(&realms_root.join("empty")).is_empty(),
        "a refused resolve must not create anything in the realm"
    );
}

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
    write_manifest_with_backend(state_root, realm_id, "sqlite")
}

fn write_manifest_with_backend(
    state_root: &Path,
    realm_id: &str,
    backend: &str,
) -> meerkat_store::RealmPaths {
    let paths = meerkat_store::realm_paths_in(state_root, realm_id);
    std::fs::create_dir_all(&paths.root).unwrap();
    std::fs::write(
        &paths.manifest_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "realm_id": realm_id,
            "backend": backend,
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

/// An unsupported unledgered owned-object fixture: raw `sessions` table, one
/// session row, and no `meerkat_schema` ledger.
fn create_unledgered_owned_fixture_realm(state_root: &Path, realm_id: &str) -> PathBuf {
    let paths = write_manifest(state_root, realm_id);
    let conn = Connection::open(&paths.sessions_sqlite_path).unwrap();
    conn.execute_batch(SESSIONS_DDL).unwrap();
    let mut session = Session::new();
    session.push(Message::User(UserMessage::text(
        "unledgered owned-object fixture",
    )));
    insert_session(&conn, &session);
    paths.sessions_sqlite_path
}

struct ExactPreFloorFixture {
    database: PathBuf,
    session_id: String,
    session_source: Vec<u8>,
    runtime_id: String,
    input_id: String,
    input_source: Vec<u8>,
}

/// Exact pre-floor session-store and runtime-store v1 schemas in their
/// shared database. The valid variant carries a frozen released-v2 Session
/// and one unversioned queued text prompt. The malformed variant proves that
/// the explicit bridge rolls back DDL, row conversion, and the ledger stamp.
fn create_exact_pre_floor_session_realm(
    state_root: &Path,
    realm_id: &str,
    malformed: bool,
) -> ExactPreFloorFixture {
    let paths = write_manifest(state_root, realm_id);
    let mut conn = Connection::open(&paths.sessions_sqlite_path).unwrap();
    let tx = conn.transaction().unwrap();
    let session_v1 = meerkat_store::sqlite_store::SESSION_STORE_DOMAIN
        .migrations
        .first()
        .expect("session v1 migration");
    (session_v1.apply)(&tx).unwrap();
    let runtime_v1 = meerkat_runtime::store::sqlite::RUNTIME_STORE_DOMAIN
        .migrations
        .first()
        .expect("runtime v1 migration");
    (runtime_v1.apply)(&tx).unwrap();
    tx.commit().unwrap();

    let mut session = Session::new();
    session.push(Message::User(UserMessage::text(
        "exact pre-floor session fixture",
    )));
    let session_id = session.id().to_string();
    let mut document = serde_json::to_value(&session).unwrap();
    document["version"] = serde_json::json!(2);
    if malformed {
        document["messages"] = serde_json::json!("not-an-array");
    } else {
        let _released_session =
            meerkat_core::import_released_0810_session(&serde_json::to_vec(&document).unwrap())
                .expect("fixture must be an exact released-v2 session document");
    }
    let source = serde_json::to_vec(&document).unwrap();
    conn.execute(
        "INSERT INTO sessions (session_id, created_at_ms, updated_at_ms, message_count, \
         total_tokens, metadata_json, session_json) VALUES (?1, 0, 0, 1, 0, ?2, ?3)",
        rusqlite::params![
            &session_id,
            serde_json::to_string(session.metadata()).unwrap(),
            &source
        ],
    )
    .unwrap();

    // Frozen pre-v0.8.10 wire bytes. This fixture is deliberately authored
    // without today's StoredInputState serializer so current defaults or
    // field additions cannot make the historical bridge test pass by drift.
    let input_id = "019e20e6-b011-7000-8000-000000000001".to_string();
    let legacy = serde_json::json!({
        "input_id": input_id,
        "current_state": "queued",
        "policy": {
            "version": 1,
            "decision": {
                "apply_mode": "stage_run_start",
                "wake_mode": "wake_if_idle",
                "queue_mode": "fifo",
                "consume_point": "on_run_complete",
                "drain_policy": "queue_next_turn",
                "routing_disposition": "queue",
                "record_transcript": true,
                "emit_operator_content": true,
                "policy_version": 1
            }
        },
        "runtime_semantics": {
            "boundary": "run_start",
            "execution_kind": "content_turn",
            "peer_response_terminal_apply_intent": null
        },
        "durability": "durable",
        "idempotency_key": "queued-pre-floor-idempotency",
        "attempt_count": 0,
        "recovery_count": 0,
        "history": [{
            "timestamp": "2026-05-13T10:00:00.000200Z",
            "from": "accepted",
            "to": "queued",
            "reason": "QueueAccepted"
        }],
        "persisted_input": {
            "input_type": "prompt",
            "header": {
                "id": input_id,
                "timestamp": "2026-05-13T10:00:00.000000Z",
                "source": { "type": "operator" },
                "durability": "durable",
                "visibility": {
                    "transcript_eligible": true,
                    "operator_eligible": true
                },
                "idempotency_key": "queued-pre-floor-idempotency",
                "correlation_id": "019e20e6-b011-7000-8000-100000000001"
            },
            "text": "queued pre-floor prompt",
            "turn_metadata": {}
        },
        "created_at": "2026-05-13T10:00:00.000100Z",
        "updated_at": "2026-05-13T10:00:00.000200Z"
    });
    assert!(legacy.get("stored_input_state_version").is_none());
    assert!(legacy.get("admission_sequence").is_none());
    assert!(legacy.get("recovery_lane").is_none());
    assert!(legacy["persisted_input"].get("blocks").is_none());
    assert_eq!(
        legacy["input_id"],
        legacy["persisted_input"]["header"]["id"]
    );
    assert_eq!(
        legacy["durability"],
        legacy["persisted_input"]["header"]["durability"]
    );
    assert_eq!(
        legacy["idempotency_key"],
        legacy["persisted_input"]["header"]["idempotency_key"]
    );
    assert_eq!(
        legacy["policy"]["version"],
        legacy["policy"]["decision"]["policy_version"]
    );
    assert_eq!(legacy["policy"]["decision"]["routing_disposition"], "queue");
    assert_eq!(legacy["runtime_semantics"]["boundary"], "run_start");
    assert_eq!(
        legacy["runtime_semantics"]["execution_kind"],
        "content_turn"
    );
    assert_eq!(legacy["history"].as_array().unwrap().len(), 1);
    assert_eq!(legacy["history"][0]["from"], "accepted");
    assert_eq!(legacy["history"][0]["to"], "queued");
    assert_eq!(legacy["history"][0]["reason"], "QueueAccepted");
    let input_source = serde_json::to_vec(&legacy).unwrap();
    let runtime_id =
        meerkat_runtime::identifiers::LogicalRuntimeId::for_session(session.id()).to_string();
    let runtime_state_source = serde_json::to_vec(&serde_json::json!({
        "record_version": 1,
        "runtime_state": "idle",
        "binding": {
            "agent_runtime_id": null,
            "fence_token": null,
            "runtime_generation": null,
            "runtime_epoch_id": null
        }
    }))
    .unwrap();
    conn.execute(
        "INSERT INTO runtime_states (runtime_id, runtime_state_json) VALUES (?1, ?2)",
        rusqlite::params![&runtime_id, &runtime_state_source],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO runtime_session_snapshots (runtime_id, session_snapshot) VALUES (?1, ?2)",
        rusqlite::params![&runtime_id, &source],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO runtime_input_states (runtime_id, input_id, state_json) \
         VALUES (?1, ?2, ?3)",
        rusqlite::params![&runtime_id, &input_id, &input_source],
    )
    .unwrap();

    ExactPreFloorFixture {
        database: paths.sessions_sqlite_path,
        session_id,
        session_source: source,
        runtime_id,
        input_id,
        input_source,
    }
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
fn migrate_refuses_unledgered_owned_sessions_without_mutation() {
    let temp = TempDir::new().unwrap();
    let state_root = temp.path().join("realms");
    let database = create_unledgered_owned_fixture_realm(&state_root, "unledgered");
    let before = std::fs::read(&database).unwrap();

    // Dry-run: pending ledger baseline and a byte-identical database.
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
    assert_eq!(realms[0]["realm"], "unledgered");
    assert_eq!(realms[0]["backend"], "sqlite");
    let pending = ledger_entries(&realms[0], "session-store", "sessions.sqlite3");
    assert_eq!(pending.len(), 1, "{report:#}");
    assert_eq!(pending[0]["action"], "missing-row");
    assert!(pending[0].get("before").is_none(), "no ledger row yet");
    assert!(
        realms[0].get("adoption").is_none(),
        "the retired checkpoint-adoption report must not survive: {report:#}"
    );
    let after_dry = std::fs::read(&database).unwrap();
    assert_eq!(before, after_dry, "dry-run must not mutate the database");

    // Apply refuses the unversioned owned schema; it is not inferred or
    // baseline-stamped.
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
    assert_eq!(
        apply.status.code(),
        Some(1),
        "unledgered owned schema must refuse\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    let report = parse_json(&apply);
    assert_eq!(report["mode"], "apply");
    let realms = report["realms"].as_array().expect("realms array");
    assert_eq!(realms.len(), 1, "{report:#}");
    assert!(
        realms[0]["errors"]
            .as_array()
            .expect("errors")
            .iter()
            .any(|error| error
                .as_str()
                .is_some_and(|error| error.contains("no ledger row"))),
        "{report:#}"
    );
    assert!(
        ledger_entries(&realms[0], "session-store", "sessions.sqlite3").is_empty(),
        "{report:#}"
    );
    assert_eq!(
        std::fs::read(&database).unwrap(),
        before,
        "refusal must leave the database byte-identical"
    );
    let conn = Connection::open(&database).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'table' AND name = 'meerkat_schema'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        0,
        "refusal created the ledger"
    );
}

#[test]
fn fresh_workspace_runs_use_durable_sqlite_fallback_without_touching_legacy_realm() {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let state_root = temp.path().join("realms");
    let workspace_realm = meerkat_core::derive_workspace_realm_id(&project);
    let legacy_database = create_unledgered_owned_fixture_realm(&state_root, &workspace_realm);
    let legacy_before = std::fs::read(&legacy_database).unwrap();
    let state_root_arg = state_root.to_str().unwrap();

    let invocations = [
        (
            vec![
                "--state-root",
                state_root_arg,
                "fresh fallback prompt",
                "--json",
            ],
            true,
        ),
        (
            vec![
                "--state-root",
                state_root_arg,
                "help",
                "Give me a WorkGraph example",
                "--json",
            ],
            true,
        ),
        (
            vec!["--state-root", state_root_arg, "plain fallback prompt"],
            false,
        ),
        (
            vec![
                "--state-root",
                state_root_arg,
                "help",
                "Give me a plain WorkGraph example",
            ],
            false,
        ),
    ];
    let mut fallback_realms = Vec::new();
    for (index, (args, json_output)) in invocations.iter().enumerate() {
        let output = run_rkat(&temp, args);
        assert_success(&output, &format!("fresh fallback invocation {index}"));
        let warning = String::from_utf8_lossy(&output.stderr);
        assert!(
            warning.contains(&format!(
                "historical sessions from original workspace realm '{workspace_realm}' were not loaded into this fresh run"
            )),
            "warning did not identify the original realm excluded from the fresh run:\n{warning}"
        );
        assert!(
            warning.contains("fresh-run storage fell back to generated realm")
                && warning.contains("durable SQLite")
                && warning.contains("workspace configuration and auth policy remain in force"),
            "warning did not state the fallback durability and config boundary:\n{warning}"
        );
        assert!(
            warning.contains(&format!(
                "rkat --state-root '{}' --realm '{workspace_realm}' storage migrate --apply --bridge-pre-0-8-10",
                state_root.display()
            )),
            "warning did not provide the explicit recovery command:\n{warning}"
        );

        let (session_ref, reported_session_id) = if *json_output {
            let result = parse_json(&output);
            (
                result["session_ref"]
                    .as_str()
                    .expect("fresh fallback JSON must expose a realm-qualified session ref")
                    .to_string(),
                result["session_id"]
                    .as_str()
                    .expect("fresh fallback JSON must expose a session id")
                    .to_string(),
            )
        } else {
            let summary = warning
                .lines()
                .find(|line| line.starts_with("[Session: ") && line.contains(" | Ref: "))
                .expect("plain fallback output must print a full realm-qualified session ref");
            let reported_session_id = summary
                .strip_prefix("[Session: ")
                .and_then(|rest| rest.split_once(" | Ref: "))
                .map(|(session_id, _)| session_id.to_string())
                .expect("plain fallback summary must expose the full session id");
            let session_ref = summary
                .split_once(" | Ref: ")
                .and_then(|(_, rest)| rest.split_once(" | "))
                .map(|(session_ref, _)| session_ref.to_string())
                .expect("plain fallback summary must expose the session ref");
            (session_ref, reported_session_id)
        };
        let (fallback_realm, session_id) = session_ref
            .split_once(':')
            .expect("session ref must contain realm and session identity");
        assert!(fallback_realm.starts_with("realm-"), "{session_ref}");
        assert_ne!(fallback_realm, workspace_realm, "{session_ref}");
        assert_eq!(reported_session_id, session_id, "{session_ref}");

        let fallback_paths = meerkat_store::realm_paths_in(&state_root, fallback_realm);
        let manifest: meerkat_store::RealmManifest = serde_json::from_slice(
            &std::fs::read(&fallback_paths.manifest_path).expect("fallback manifest"),
        )
        .expect("valid fallback manifest");
        assert_eq!(manifest.backend, meerkat_store::RealmBackend::Sqlite);
        assert_eq!(manifest.origin, meerkat_store::RealmOrigin::Generated);
        assert_eq!(manifest.realm.as_str(), fallback_realm);

        let list = run_rkat(
            &temp,
            &[
                "--state-root",
                state_root_arg,
                "--realm",
                fallback_realm,
                "session",
                "list",
            ],
        );
        assert_success(&list, "list session in generated fallback realm");
        assert!(
            String::from_utf8_lossy(&list.stdout).contains(session_id),
            "fallback session was not durable\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&list.stdout),
            String::from_utf8_lossy(&list.stderr)
        );
        fallback_realms.push(fallback_realm.to_string());
        assert_eq!(
            std::fs::read(&legacy_database).unwrap(),
            legacy_before,
            "fresh fallback invocation mutated the failed workspace database"
        );
    }
    assert_eq!(
        fallback_realms
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        invocations.len(),
        "each failed fresh open must mint a new isolated storage realm"
    );

    let explicit = run_rkat(
        &temp,
        &[
            "--state-root",
            state_root_arg,
            "--realm",
            &workspace_realm,
            "explicit realms stay strict",
            "--json",
        ],
    );
    assert_eq!(explicit.status.code(), Some(1));
    let explicit_stderr = String::from_utf8_lossy(&explicit.stderr);
    assert!(
        explicit_stderr.contains("Failed to open realm persistence backend"),
        "explicit realm did not report its original open failure:\n{explicit_stderr}"
    );
    assert!(
        !explicit_stderr.contains("fresh-run storage fell back"),
        "explicit --realm must never fall back:\n{explicit_stderr}"
    );
    assert_eq!(std::fs::read(&legacy_database).unwrap(), legacy_before);

    let strict_workspace_invocations = [
        vec![
            "--state-root",
            state_root_arg,
            "run",
            "--resume",
            "last",
            "resume stays strict",
            "--json",
        ],
        vec!["--state-root", state_root_arg, "session", "list"],
    ];
    for args in &strict_workspace_invocations {
        let output = run_rkat(&temp, args);
        assert_eq!(
            output.status.code(),
            Some(1),
            "resume and session commands must fail closed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !String::from_utf8_lossy(&output.stderr).contains("fresh-run storage fell back"),
            "resume and session commands must never enter fresh-run fallback"
        );
        assert_eq!(std::fs::read(&legacy_database).unwrap(), legacy_before);
    }
}

#[test]
fn explicit_pre_floor_bridge_migrates_then_reopens_idempotently() {
    let temp = TempDir::new().unwrap();
    let state_root = temp.path().join("realms");
    let fixture = create_exact_pre_floor_session_realm(&state_root, "pre-floor", false);

    let apply = run_rkat(
        &temp,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "--realm",
            "pre-floor",
            "storage",
            "migrate",
            "--apply",
            "--bridge-pre-0-8-10",
            "--json",
        ],
    );
    assert_success(&apply, "explicit pre-floor bridge");
    let report = parse_json(&apply);
    let realms = report["realms"].as_array().expect("realms array");
    assert_eq!(realms.len(), 1, "{report:#}");
    assert!(
        realms[0]["notes"]
            .as_array()
            .expect("notes")
            .iter()
            .any(|note| note.as_str().is_some_and(|note| {
                note.contains("pre-v0.8.10 bridge") && note.contains("session-store")
            })),
        "{report:#}"
    );
    assert!(
        realms[0]["notes"]
            .as_array()
            .expect("notes")
            .iter()
            .any(|note| note.as_str().is_some_and(|note| {
                note.contains("pre-v0.8.10 bridge")
                    && note.contains("runtime-store")
                    && note.contains("rewrote 1 legacy durable payload row(s)")
            })),
        "{report:#}"
    );
    let session_ledger = ledger_entries(&realms[0], "session-store", "sessions.sqlite3");
    assert_eq!(session_ledger.len(), 1, "{report:#}");
    assert_eq!(session_ledger[0]["action"], "stamped", "{report:#}");
    assert_eq!(session_ledger[0]["after"], 3, "{report:#}");
    let runtime_ledger = ledger_entries(&realms[0], "runtime-store", "sessions.sqlite3");
    assert_eq!(runtime_ledger.len(), 1, "{report:#}");
    assert_eq!(runtime_ledger[0]["action"], "stamped", "{report:#}");
    assert_eq!(runtime_ledger[0]["after"], 2, "{report:#}");

    let list = run_rkat(
        &temp,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "--realm",
            "pre-floor",
            "session",
            "list",
        ],
    );
    assert_success(&list, "strict session list after pre-floor bridge");
    assert!(
        String::from_utf8_lossy(&list.stdout).contains(&fixture.session_id),
        "session list omitted imported session\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&list.stdout),
        String::from_utf8_lossy(&list.stderr)
    );

    let conn = Connection::open(&fixture.database).unwrap();
    let queued_bytes: Vec<u8> = conn
        .query_row(
            "SELECT state_json FROM runtime_input_states \
             WHERE runtime_id = ?1 AND input_id = ?2",
            rusqlite::params![&fixture.runtime_id, &fixture.input_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_ne!(
        queued_bytes, fixture.input_source,
        "bridge left the legacy unversioned queued payload untouched"
    );
    let queued: serde_json::Value = serde_json::from_slice(&queued_bytes).unwrap();
    assert_eq!(queued["current_state"], "queued", "{queued:#}");
    assert_eq!(queued["attempt_count"], 0, "{queued:#}");
    assert!(
        queued["admission_sequence"].as_u64().is_some(),
        "{queued:#}"
    );
    assert_eq!(queued["recovery_lane"], "queue", "{queued:#}");
    assert!(queued["terminal_outcome"].is_null(), "{queued:#}");
    assert_eq!(
        queued["persisted_input"]["content"], "queued pre-floor prompt",
        "{queued:#}"
    );
    assert!(
        queued["persisted_input"].get("text").is_none(),
        "{queued:#}"
    );
    assert!(
        queued["stored_input_state_version"].as_u64().is_some(),
        "{queued:#}"
    );
    drop(conn);

    let again = run_rkat(
        &temp,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "--realm",
            "pre-floor",
            "storage",
            "migrate",
            "--apply",
            "--bridge-pre-0-8-10",
            "--json",
        ],
    );
    assert_success(&again, "idempotent pre-floor bridge rerun");
    let report = parse_json(&again);
    let realm = &report["realms"][0];
    assert!(
        realm["notes"]
            .as_array()
            .expect("notes")
            .iter()
            .any(|note| note
                .as_str()
                .is_some_and(|note| note.contains("no domain required pre-floor import"))),
        "{report:#}"
    );
    assert_eq!(
        ledger_entries(realm, "session-store", "sessions.sqlite3")[0]["action"],
        "already-current",
        "{report:#}"
    );
    let conn = Connection::open(&fixture.database).unwrap();
    let queued_after_rerun: Vec<u8> = conn
        .query_row(
            "SELECT state_json FROM runtime_input_states \
             WHERE runtime_id = ?1 AND input_id = ?2",
            rusqlite::params![&fixture.runtime_id, &fixture.input_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        queued_after_rerun, queued_bytes,
        "idempotent bridge rerun changed or replayed queued work"
    );
}

#[test]
fn explicit_pre_floor_bridge_rolls_back_malformed_session() {
    let temp = TempDir::new().unwrap();
    let state_root = temp.path().join("realms");
    let fixture = create_exact_pre_floor_session_realm(&state_root, "malformed-pre-floor", true);

    let apply = run_rkat(
        &temp,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "--realm",
            "malformed-pre-floor",
            "storage",
            "migrate",
            "--apply",
            "--bridge-pre-0-8-10",
            "--json",
        ],
    );
    assert_eq!(
        apply.status.code(),
        Some(1),
        "malformed bridge must fail\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    let report = parse_json(&apply);
    assert!(
        report["realms"][0]["errors"]
            .as_array()
            .expect("errors")
            .iter()
            .any(|error| error
                .as_str()
                .is_some_and(|error| error.contains("pre-v0.8.10 bridge failed"))),
        "{report:#}"
    );

    let conn = Connection::open(&fixture.database).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'meerkat_schema'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        0,
        "failed bridge stamped a schema ledger"
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master \
             WHERE type = 'table' AND name = 'session_strand_links'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        0,
        "failed bridge committed migration DDL"
    );
    let stored: Vec<u8> = conn
        .query_row(
            "SELECT session_json FROM sessions WHERE session_id = ?1",
            rusqlite::params![&fixture.session_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        stored, fixture.session_source,
        "failed bridge changed the source row"
    );
    let queued: Vec<u8> = conn
        .query_row(
            "SELECT state_json FROM runtime_input_states \
             WHERE runtime_id = ?1 AND input_id = ?2",
            rusqlite::params![&fixture.runtime_id, &fixture.input_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        queued, fixture.input_source,
        "failed bridge converted queued runtime work after an earlier domain refusal"
    );
}

#[test]
fn explicit_pre_floor_bridge_rejects_non_sqlite_realms() {
    for backend in ["jsonl", "memory"] {
        let temp = TempDir::new().unwrap();
        let state_root = temp.path().join("realms");
        let realm_id = format!("pre-floor-{backend}");
        write_manifest_with_backend(&state_root, &realm_id, backend);

        let apply = run_rkat(
            &temp,
            &[
                "--state-root",
                state_root.to_str().unwrap(),
                "--realm",
                &realm_id,
                "storage",
                "migrate",
                "--apply",
                "--bridge-pre-0-8-10",
                "--json",
            ],
        );
        assert_eq!(
            apply.status.code(),
            Some(1),
            "{backend} bridge must fail\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&apply.stdout),
            String::from_utf8_lossy(&apply.stderr)
        );
        let report = parse_json(&apply);
        let realm = &report["realms"][0];
        assert_eq!(realm["backend"], backend, "{report:#}");
        assert!(
            realm["errors"]
                .as_array()
                .expect("errors")
                .iter()
                .any(|error| error.as_str().is_some_and(|error| {
                    error.contains("--bridge-pre-0-8-10")
                        && error.contains("backend 'sqlite'")
                        && error.contains(&format!("backend '{backend}'"))
                })),
            "{report:#}"
        );
    }
}

#[test]
fn sqlite_apply_does_not_open_inactive_jsonl_session_index() {
    let temp = TempDir::new().unwrap();
    let state_root = temp.path().join("realms");
    let paths = write_manifest(&state_root, "sqlite-with-inactive-jsonl");
    meerkat_store::SqliteSessionStore::open(&paths.sessions_sqlite_path).unwrap();
    let jsonl_dir = paths.root.join("sessions_jsonl");
    std::fs::create_dir_all(&jsonl_dir).unwrap();
    let index = jsonl_dir.join("session_index.sqlite3");
    Connection::open(&index).unwrap();

    let apply = run_rkat(
        &temp,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "--realm",
            "sqlite-with-inactive-jsonl",
            "storage",
            "migrate",
            "--apply",
            "--json",
        ],
    );
    assert_success(&apply, "sqlite migration with inactive JSONL index");

    let conn = Connection::open(index).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'session_index'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        0,
        "SQLite migration opened the inactive JSONL session index"
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'meerkat_schema'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        0,
        "SQLite migration stamped the inactive JSONL session index"
    );
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
    // The adopted realm then went through the structural-ledger case in the
    // same run.
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
    let database = create_unledgered_owned_fixture_realm(&state_root, "fenced");
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
fn ledger_baseline_read_failures_and_future_versions_are_refusals_not_missing_rows() {
    let temp = TempDir::new().unwrap();
    let state_root = temp.path().join("realms");
    let paths = write_manifest(&state_root, "poisoned");
    // An unreadable database must surface as a per-realm error, never as a
    // missing ledger the dry-run reports as missing-row.
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
    // The unreadable database contributes no ledger rows (no missing-row
    // laundering)...
    assert!(
        ledger_entries(&realms[0], "tools-tasks", "tasks.db").is_empty(),
        "{report:#}"
    );
    // ...and the future domain's row is report-only, never missing-row.
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

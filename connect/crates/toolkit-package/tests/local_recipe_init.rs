#![cfg(feature = "local-recipes")]

use rusqlite::{Connection, types::Value as SqlValue};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::Stdio;
use std::process::{Command, Output};
use std::sync::{Arc, Barrier};
use tempfile::TempDir;
use toolkit_store::{
    AdmissionRequest, EffectIntent, EventWrite, RecordData, ReplaySafety, SqliteStore, StoreKey,
    StoreScope, ToolkitStore,
};

const PLUGIN: &[u8] =
    include_bytes!("../integrations/connectors/github-readonly/0.1.0/plugin.json");
const MCP: &[u8] = include_bytes!("../integrations/connectors/github-readonly/0.1.0/mcp.json");
const RECIPES: &[u8] =
    include_bytes!("../integrations/connectors/github-readonly/0.1.0/recipes.json");

struct Fixture {
    _temp: TempDir,
    database: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().canonicalize().unwrap().join("recipes.sqlite");
        Self {
            _temp: temp,
            database,
        }
    }

    fn args(&self, verb: &str, realm: &str, owner: &str) -> Vec<String> {
        vec![
            verb.into(),
            "--database".into(),
            self.database.to_str().unwrap().into(),
            "--realm".into(),
            realm.into(),
            "--owner".into(),
            owner.into(),
        ]
    }

    fn init(&self) -> Vec<String> {
        self.args("init", "host-realm", "alice")
    }

    fn save(&self, owner: &str) -> Vec<String> {
        let mut args = self.args("save", "host-realm", owner);
        for (flag, name, bytes) in [
            ("--plugin", "plugin.json", PLUGIN),
            ("--mcp", "mcp.json", MCP),
            ("--recipes", "recipes.json", RECIPES),
        ] {
            let path = self.database.parent().unwrap().join(name);
            fs::write(&path, bytes).unwrap();
            args.extend([flag.into(), path.to_str().unwrap().into()]);
        }
        args.extend([
            "--decision-owner".into(),
            "init-test".into(),
            "--decision-record".into(),
            format!("choice-{owner}"),
            "--decision-revision".into(),
            "1".into(),
        ]);
        args
    }

    fn read(&self, realm: &str, owner: &str, digest: &str) -> Vec<String> {
        let mut args = self.args("read", realm, owner);
        args.extend([
            "--publisher".into(),
            "meerkat-toolkit".into(),
            "--name".into(),
            "github-readonly".into(),
            "--version".into(),
            "0.1.0".into(),
            "--digest".into(),
            digest.into(),
        ]);
        args
    }
}

fn command(args: &[String]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_toolkit-local-recipes"));
    command.args(args).env_clear().env("PATH", "/usr/bin:/bin");
    command
}

fn result(output: Output, error: Option<&str>) -> Value {
    assert_eq!(output.status.success(), error.is_none(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["scope"], "retained_recipe_metadata");
    if let Some(error) = error {
        assert_eq!(
            value,
            json!({"schema_version": 1, "scope": "retained_recipe_metadata",
                "status": "error", "error": error})
        );
    }
    value
}

fn success(args: &[String]) -> Value {
    result(command(args).output().unwrap(), None)
}

fn failure(args: &[String], error: &str) {
    result(command(args).output().unwrap(), Some(error));
}

fn initialized(args: &[String]) {
    assert_eq!(
        success(args),
        json!({"schema_version": 1, "scope": "retained_recipe_metadata",
            "status": "initialized"})
    );
}

// Observe all persisted fields, including fences and claims, without recreating
// schema. The production store owner is the only initializer in these tests.
fn snapshot(path: &Path) -> Vec<Vec<Vec<SqlValue>>> {
    let connection = Connection::open(path).unwrap();
    [
        "toolkit_metadata",
        "toolkit_records",
        "toolkit_admissions",
        "toolkit_events",
        "toolkit_effects",
    ]
    .into_iter()
    .map(|table| {
        let mut statement = connection
            .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
            .unwrap();
        let columns = statement.column_count();
        statement
            .query_map([], |row| {
                (0..columns).map(|column| row.get(column)).collect()
            })
            .unwrap()
            .collect::<Result<Vec<Vec<SqlValue>>, _>>()
            .unwrap()
    })
    .collect()
}

#[test]
fn initializes_without_recipe_or_domain_records_and_reopens_empty() {
    let fixture = Fixture::new();
    initialized(&fixture.init());
    let store = SqliteStore::open_existing(&fixture.database).unwrap();
    store.capabilities().require_durable_contract().unwrap();
    let contents = snapshot(&fixture.database);
    assert_eq!(contents[0].len(), 1);
    assert!(contents[1..].iter().all(Vec::is_empty));
    for (realm, owner) in [("host-realm", "alice"), ("other-realm", "bob")] {
        let listed = success(&fixture.args("list", realm, owner));
        assert_eq!(listed["recipes"], json!([]));
        assert_eq!(listed["unverified"], json!([]));
    }
    initialized(&fixture.init());
    assert_eq!(snapshot(&fixture.database), contents);
}

#[tokio::test]
async fn replay_preserves_recipes_admissions_events_effects_and_live_record_claims() {
    let fixture = Fixture::new();
    initialized(&fixture.init());
    let alice = success(&fixture.save("alice"));
    let bob = success(&fixture.save("bob"));
    let alice_digest = alice["recipe"]["digest"].as_str().unwrap();
    let bob_digest = bob["recipe"]["digest"].as_str().unwrap();
    let alice_read = success(&fixture.read("host-realm", "alice", alice_digest));
    let bob_read = success(&fixture.read("host-realm", "bob", bob_digest));
    let store = SqliteStore::open_existing(&fixture.database).unwrap();
    let data = RecordData {
        schema: "test.init-preservation".into(),
        version: 1,
        body: json!({"observation": "retained"}),
    };
    let admission = AdmissionRequest {
        key: StoreKey {
            realm: "host-realm".into(),
            owner: "record-owner".into(),
            id: "work".into(),
        },
        admission_key: "work-admission".into(),
        data: data.clone(),
        events: vec![EventWrite {
            kind: "observed".into(),
            data: data.clone(),
        }],
        effects: vec![EffectIntent {
            id: "effect".into(),
            kind: "test.effect".into(),
            request: data,
            idempotency_key: "effect-key".into(),
            replay_safety: ReplaySafety::ReconcileBeforeRetry,
        }],
    };
    let record = store
        .admit(admission.clone())
        .await
        .unwrap()
        .record()
        .clone();
    let lease = store
        .claim_record(&record.key, record.revision, "worker", 300_000)
        .await
        .unwrap();
    let before = snapshot(&fixture.database);
    for (realm, owner) in [
        ("host-realm", "alice"),
        ("host-realm", "bob"),
        ("new-realm", "alice"),
    ] {
        initialized(&fixture.args("init", realm, owner));
    }
    assert_eq!(snapshot(&fixture.database), before);
    assert_eq!(store.recover_record_claim(&lease).await.unwrap(), lease);
    assert_eq!(store.admit(admission).await.unwrap().record(), &record);
    assert_eq!(
        success(&fixture.read("host-realm", "alice", alice_digest)),
        alice_read
    );
    assert_eq!(
        success(&fixture.read("host-realm", "bob", bob_digest)),
        bob_read
    );
    for (realm, owner) in [("host-realm", "charlie"), ("other-realm", "alice")] {
        assert_eq!(
            success(&fixture.args("list", realm, owner))["recipes"],
            json!([])
        );
        assert_eq!(
            success(&fixture.read(realm, owner, alice_digest))["status"],
            "not_found"
        );
        let scope = StoreScope {
            realm: realm.into(),
            owner: owner.into(),
        };
        assert!(
            store
                .list_records(&scope, None, 100)
                .await
                .unwrap()
                .is_empty()
        );
    }
    assert_eq!(snapshot(&fixture.database), before);
}

#[test]
fn strict_arguments_and_scope_fail_before_creation_without_echoing_values() {
    let fixture = Fixture::new();
    let canary = "PRIVATE_VALUE_MUST_NOT_APPEAR";
    for flag in ["--owner", "--token", "--plugin", "--decision-owner"] {
        let mut args = fixture.init();
        args.extend([flag.into(), canary.into()]);
        failure(&args, "invalid_arguments");
    }
    let mut missing = fixture.init();
    missing.truncate(5);
    failure(&missing, "invalid_arguments");
    let mut dangling_flag = fixture.init();
    dangling_flag.push("--token".into());
    failure(&dangling_flag, "invalid_arguments");
    for field in [4, 6] {
        for invalid in [String::new(), format!(" {canary}"), "x".repeat(513)] {
            let mut args = fixture.init();
            args[field] = invalid;
            failure(&args, "invalid_scope");
        }
    }
    let mut relative = fixture.init();
    relative[2] = format!("{canary}.sqlite");
    failure(&relative, "absolute_path_required");
    assert!(!fixture.database.exists());
}

#[test]
fn missing_parent_and_nonregular_database_paths_are_refused() {
    let fixture = Fixture::new();
    let mut missing = fixture.init();
    let parent = fixture.database.parent().unwrap().join("absent");
    missing[2] = parent.join("recipes.sqlite").to_str().unwrap().into();
    failure(&missing, "database_parent_unavailable");
    assert!(!parent.exists());
    fs::create_dir(&fixture.database).unwrap();
    failure(&fixture.init(), "database_regular_file_required");
    assert!(fixture.database.is_dir());
}

#[cfg(unix)]
#[test]
fn final_symlinks_are_refused_without_touching_existing_or_missing_targets() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let target = fixture.database.with_extension("target");
    fs::write(&target, b"untouched").unwrap();
    symlink(&target, &fixture.database).unwrap();
    failure(&fixture.init(), "database_regular_file_required");
    assert_eq!(fs::read(&target).unwrap(), b"untouched");
    fs::remove_file(&target).unwrap();
    failure(&fixture.init(), "database_regular_file_required");
    assert!(!target.exists());
}

#[test]
fn corrupt_and_unsupported_databases_do_not_report_initialization() {
    let corrupt = Fixture::new();
    fs::write(&corrupt.database, b"PRIVATE_CORRUPT_CONTENT").unwrap();
    failure(&corrupt.init(), "database_unavailable");
    let future = Fixture::new();
    let connection = Connection::open(&future.database).unwrap();
    connection.pragma_update(None, "user_version", 999).unwrap();
    drop(connection);
    failure(&future.init(), "database_unavailable");
    let connection = Connection::open(&future.database).unwrap();
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        999
    );
    let count: i64 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name LIKE 'toolkit_%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    // WAL settings can change before version rejection; raw byte equality is
    // intentionally not part of the existing store owner's failure contract.
}

#[test]
fn initialization_inherits_version_zero_adoption_without_deleting_existing_data() {
    let fixture = Fixture::new();
    let connection = Connection::open(&fixture.database).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE existing_data(value TEXT);
             INSERT INTO existing_data VALUES('preserved');",
        )
        .unwrap();
    drop(connection);
    initialized(&fixture.init());
    let connection = Connection::open(&fixture.database).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT value FROM existing_data", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "preserved"
    );
    assert!(snapshot(&fixture.database)[1..].iter().all(Vec::is_empty));
}

#[test]
fn observation_still_refuses_to_initialize_a_missing_or_empty_database() {
    let fixture = Fixture::new();
    failure(
        &fixture.args("list", "host-realm", "alice"),
        "database_not_found",
    );
    failure(
        &fixture.read("host-realm", "alice", &"0".repeat(64)),
        "database_not_found",
    );
    assert!(!fixture.database.exists());
    fs::write(&fixture.database, []).unwrap();
    failure(
        &fixture.args("list", "host-realm", "alice"),
        "database_unavailable",
    );
    assert!(fs::read(&fixture.database).unwrap().is_empty());
    initialized(&fixture.init());
}

#[test]
fn concurrent_initializers_converge_without_replacing_the_store() {
    let fixture = Fixture::new();
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let args = fixture.init();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                command(&args).output().unwrap()
            })
        })
        .collect();
    let mut successes = 0;
    for thread in threads {
        let output = thread.join().unwrap();
        if output.status.success() {
            assert_eq!(result(output, None)["status"], "initialized");
            successes += 1;
        } else {
            // A bounded SQLite lock failure is not an initialization receipt.
            result(output, Some("database_unavailable"));
        }
    }
    assert!(successes >= 1);
    let before = snapshot(&fixture.database);
    initialized(&fixture.init());
    assert_eq!(snapshot(&fixture.database), before);
    assert_eq!(before[0].len(), 1);
    assert!(before[1..].iter().all(Vec::is_empty));
}

#[cfg(unix)]
#[test]
fn failed_acknowledgement_after_commit_is_recovered_by_exact_init_replay() {
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;

    let fixture = Fixture::new();
    // Close the receiving end before launch so writes report EPIPE. An invalid
    // stdout descriptor can be silently ignored by the Rust standard library.
    let (receiver, sender) = UnixStream::pair().unwrap();
    drop(receiver);
    let sender: OwnedFd = sender.into();
    let output = command(&fixture.init())
        .stdout(Stdio::from(sender))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(output.stderr, b"output_unconfirmed\n");
    let before = snapshot(&fixture.database);
    SqliteStore::open_existing(&fixture.database).unwrap();
    initialized(&fixture.init());
    assert_eq!(snapshot(&fixture.database), before);
}

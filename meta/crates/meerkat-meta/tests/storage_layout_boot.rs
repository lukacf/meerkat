#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args
)]

//! M2 `MetaStorageLayout` boot coverage: the dual-name fixture matrix per
//! surface. Every surface (builder, `rpc_gateway`, `meta_gateway`) must
//! keep booting against a legacy-named state directory (files used where
//! they lie, the resolver never creates a twin) and converge fresh
//! directories on the canonical spellings.
//!
//! Sessions live in Core's pair on `runtime.sqlite`. No surface creates a
//! sessions database, so a directory a fresh boot leaves starts again. A
//! sessions database in any spelling, file-name twins included, is the
//! 0.9.3 session projection: every surface refuses it typed before any
//! store opens, names `meta_gateway storage-migrate-native`, and leaves it
//! where it lies.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use async_trait::async_trait;
use meerkat_meta::identity_first::contracts::RosterProvider;
use meerkat_meta::identity_first::{DurableAgentSpec, RosterContext, RosterError};
use meerkat_meta::unified_runtime::UnifiedRuntimeBuilder;
use serde_json::{Value, json};
use tempfile::TempDir;

/// Per-test mob id counter: 0.8.23's fail-closed in-proc registration
/// means concurrently running tests must not share a supervisor route.
static NEXT_TEST_MOB_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn test_definition() -> meerkat_mob::MobDefinition {
    meerkat_mob::MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "storage-layout-boot-test-{}"

[profiles.default]
model = "gpt-5.5"
"#,
        NEXT_TEST_MOB_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
    .expect("parse test mob definition")
}

fn touch(path: &Path) {
    std::fs::write(path, b"").expect("seed fixture file");
}

/// No surface creates a sessions database in any spelling: sessions live in
/// Core's pair on `runtime.sqlite`.
fn assert_no_sessions_database(state_dir: &Path) {
    let slot = meerkat_meta::storage_layout::DatabaseSlot::Sessions;
    for spelling in
        std::iter::once(slot.canonical_name()).chain(slot.legacy_names().iter().copied())
    {
        assert!(
            !state_dir.join(spelling).exists(),
            "no surface may create a separate sessions database, found {spelling}"
        );
    }
}

/// The refusal a sessions database gets: the directory must be migrated,
/// the message names the native migration verb and every file it found.
fn assert_native_migration_refusal(message: &str, files: &[&str]) {
    assert!(
        message.contains("must be migrated before this version opens it")
            && message.contains(meerkat_meta::storage_layout::NATIVE_STORAGE_MIGRATION_VERB),
        "a sessions database must refuse with the native migration: {message}"
    );
    for file in files {
        assert!(
            message.contains(file),
            "the refusal must name {file}: {message}"
        );
    }
}

/// A refused boot opens no store: every seeded file is still empty and
/// Core's `runtime.sqlite` was never created.
fn assert_refused_boot_left_the_directory(state_dir: &Path, seeded: &[&str]) {
    for file in seeded {
        assert_eq!(
            std::fs::metadata(state_dir.join(file))
                .expect("seeded file")
                .len(),
            0,
            "a refused boot must leave {file} as it was"
        );
    }
    assert!(
        !state_dir.join("runtime.sqlite").exists(),
        "a refused boot must not open the runtime store"
    );
}

/// An empty desired roster: enough to drive the identity-first arm (and its
/// continuity-store open) without materializing any member.
struct EmptyRosterProvider;

#[async_trait]
impl RosterProvider for EmptyRosterProvider {
    async fn roster(&self, _context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        Ok(Vec::new())
    }
}

// ---------------------------------------------------------------------------
// Builder (library surface)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn builder_fresh_state_dir_converges_on_canonical_names() {
    let tmp = TempDir::new().expect("temp dir");
    let state = tmp.path().join("state");

    let runtime = Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(test_definition())
            .persistent_state(&state)
            .roster_provider(Arc::new(EmptyRosterProvider))
            .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
            .build(),
    )
    .await
    .expect("fresh persistent build");

    for canonical in [
        "runtime.sqlite",
        "continuity.sqlite3",
        "meta_metadata.sqlite3",
        "meta_console.sqlite3",
    ] {
        assert!(
            state.join(canonical).exists(),
            "fresh boot must create canonical {canonical}"
        );
    }
    assert_no_sessions_database(&state);
    for legacy in ["continuity.db", "identity_continuity.sqlite"] {
        assert!(
            !state.join(legacy).exists(),
            "fresh boot must not create legacy {legacy}"
        );
    }

    runtime.mob_handle().stop().await.expect("stop");
}

/// The second start of a directory a fresh boot left: it holds no sessions
/// database, so after a clean shutdown it reads as native and the same mob
/// builds on it again.
#[tokio::test]
async fn builder_starts_a_fresh_state_dir_again_after_a_clean_shutdown() {
    let tmp = TempDir::new().expect("temp dir");
    let state = tmp.path().join("state");
    // One mob throughout: the directory's topology state belongs to it.
    let definition = test_definition();
    let build = || {
        Box::pin(
            UnifiedRuntimeBuilder::default()
                .definition(definition.clone())
                .persistent_state(&state)
                .roster_provider(Arc::new(EmptyRosterProvider))
                .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
                .build(),
        )
    };

    let first = build()
        .await
        .expect("first boot of a fresh state directory");
    first.shutdown().await;
    drop(first);
    assert_no_sessions_database(&state);
    assert_eq!(
        meerkat_meta::storage_migrate_native::native_layout_status(&state),
        meerkat_meta::storage_migrate_native::NativeLayoutStatus::Native,
        "the directory a fresh boot leaves must read as native"
    );

    let again = build()
        .await
        .expect("a directory this version created must start again");
    assert_no_sessions_database(&state);
    again.shutdown().await;
}

/// One Meta runtime per state directory: a second builder on a directory a
/// live runtime holds is refused typed before it opens any store. A clean
/// shutdown releases the directory, and once the first runtime is gone the
/// same mob builds on it again.
#[tokio::test]
async fn builder_refuses_a_state_directory_another_runtime_holds() {
    let tmp = TempDir::new().expect("temp dir");
    let state = tmp.path().join("state");
    // One mob throughout: the directory's topology state belongs to it.
    let definition = test_definition();
    let build = || {
        Box::pin(
            UnifiedRuntimeBuilder::default()
                .definition(definition.clone())
                .persistent_state(&state)
                .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
                .build(),
        )
    };
    let first = build().await.expect("first runtime");
    let Err(refused) = build().await else {
        panic!("a second runtime on a held state directory must be refused");
    };
    assert!(
        matches!(
            refused,
            meerkat_meta::UnifiedRuntimeBuilderError::StorageLayout(
                meerkat_meta::storage_layout::StorageLayoutError::StateDirInUse { .. }
            )
        ),
        "{refused}"
    );
    assert!(
        refused
            .to_string()
            .ends_with("is in use by another Meta runtime"),
        "{refused}"
    );
    first.shutdown().await;
    drop(
        meerkat_meta::state_dir_claim::StateDirClaim::acquire(&state)
            .expect("a clean shutdown releases the state directory"),
    );
    drop(first);
    let again = build()
        .await
        .expect("the same mob builds on the directory once the first runtime is gone");
    again.shutdown().await;
}

/// The builder claims its state directory only once the configuration is
/// coherent: a configuration it refuses leaves the directory untouched.
#[tokio::test]
async fn a_refused_builder_configuration_leaves_the_state_directory_untouched() {
    let tmp = TempDir::new().expect("temp dir");
    let state = tmp.path().join("state");
    let Err(refused) = Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(test_definition())
            .persistent_state(&state)
            .scratch_dir(tmp.path().join("scratch"))
            .build(),
    )
    .await
    else {
        panic!("persistent_state() with scratch_dir() must be refused");
    };
    assert!(
        matches!(
            refused,
            meerkat_meta::UnifiedRuntimeBuilderError::ConflictingConfiguration(_)
        ),
        "{refused}"
    );
    assert!(
        !state.exists(),
        "a refused configuration created {}",
        state.display()
    );
}

#[tokio::test]
async fn builder_keeps_using_a_legacy_named_state_dir_where_it_lies() {
    let tmp = TempDir::new().expect("temp dir");
    let state = tmp.path().join("state");
    std::fs::create_dir_all(&state).expect("state dir");
    // The builder's own historical continuity spelling (recon census):
    // SQLite opens the zero-byte seed as an empty database and initializes
    // it in place. Its historical sessions spelling refuses instead
    // (`builder_refuses_a_legacy_sessions_database_until_it_is_migrated`).
    touch(&state.join("identity_continuity.sqlite"));

    let runtime = Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(test_definition())
            .persistent_state(&state)
            .roster_provider(Arc::new(EmptyRosterProvider))
            .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
            .build(),
    )
    .await
    .expect("legacy-named persistent build");

    let (legacy, canonical) = ("identity_continuity.sqlite", "continuity.sqlite3");
    assert!(
        state.join(legacy).exists(),
        "legacy {legacy} must keep being used where it lies"
    );
    assert!(
        !state.join(canonical).exists(),
        "boot must not create a canonical twin {canonical} beside {legacy}"
    );
    assert!(
        std::fs::metadata(state.join(legacy))
            .expect("metadata")
            .len()
            > 0,
        "legacy {legacy} must have been opened and initialized in place"
    );
    assert_no_sessions_database(&state);

    runtime.mob_handle().stop().await.expect("stop");
}

/// A legacy sessions database is the 0.9.3 session projection: the builder
/// refuses it typed before any store opens and leaves it where it lies for
/// the native migration to move.
#[tokio::test]
async fn builder_refuses_a_legacy_sessions_database_until_it_is_migrated() {
    let tmp = TempDir::new().expect("temp dir");
    let state = tmp.path().join("state");
    std::fs::create_dir_all(&state).expect("state dir");
    touch(&state.join("sessions.db"));

    let error = Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(test_definition())
            .persistent_state(&state)
            .roster_provider(Arc::new(EmptyRosterProvider))
            .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
            .build(),
    )
    .await
    .err()
    .expect("a legacy sessions database must refuse to boot");
    assert!(
        matches!(
            error,
            meerkat_meta::UnifiedRuntimeBuilderError::StorageLayout(
                meerkat_meta::storage_layout::StorageLayoutError::NativeStorageMigrationRequired { .. }
            )
        ),
        "{error}"
    );
    assert_native_migration_refusal(&error.to_string(), &["sessions.db"]);
    assert_refused_boot_left_the_directory(&state, &["sessions.db"]);
}

/// Session file-name twins are two copies of the 0.9.3 session projection:
/// the builder refuses them like a single one, naming both.
#[tokio::test]
async fn builder_refuses_session_file_name_twins() {
    let tmp = TempDir::new().expect("temp dir");
    let state = tmp.path().join("state");
    std::fs::create_dir_all(&state).expect("state dir");
    touch(&state.join("sessions.db"));
    touch(&state.join("sessions.sqlite3"));

    let error = Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(test_definition())
            .persistent_state(&state)
            .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
            .build(),
    )
    .await
    .err()
    .expect("twin spellings must refuse to boot");
    assert!(
        matches!(
            error,
            meerkat_meta::UnifiedRuntimeBuilderError::StorageLayout(
                meerkat_meta::storage_layout::StorageLayoutError::NativeStorageMigrationRequired { .. }
            )
        ),
        "{error}"
    );
    assert_native_migration_refusal(&error.to_string(), &["sessions.db", "sessions.sqlite3"]);
    assert_refused_boot_left_the_directory(&state, &["sessions.db", "sessions.sqlite3"]);
}

/// The predecessor (pre-0.9) metadata and console file names are not
/// legacy spellings: the builder refuses typed instead of starting renamed
/// stores empty beside them, and names the one-shot adoption verb.
#[tokio::test]
async fn builder_refuses_unadopted_predecessor_state() {
    let tmp = TempDir::new().expect("temp dir");
    let state = tmp.path().join("state");
    std::fs::create_dir_all(&state).expect("state dir");
    let predecessor =
        state.join(meerkat_meta::storage_predecessor::PREDECESSOR_METADATA_DB_FILE_NAMES[0]);
    touch(&predecessor);

    let error = Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(test_definition())
            .persistent_state(&state)
            .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
            .build(),
    )
    .await
    .err()
    .expect("predecessor state must refuse to boot");
    let message = error.to_string();
    assert!(
        message.contains("predecessor") && message.contains("storage-adopt-predecessor"),
        "predecessor refusal must name the adoption verb: {message}"
    );
    assert!(
        message.contains(&format!("--state-dir {} --apply", state.display())),
        "the refusal must name the exact state directory to adopt: {message}"
    );
    assert!(
        !state.join("meta_metadata.sqlite3").exists(),
        "a refused boot must not create the renamed store beside the predecessor"
    );
}

/// The predecessor memory ledger file alone refuses the builder too, with
/// no memory backend configured: detection never depends on memory being
/// enabled, and a refused boot leaves the file as it was.
#[tokio::test]
async fn builder_refuses_an_unadopted_predecessor_memory_ledger() {
    let tmp = TempDir::new().expect("temp dir");
    let state = tmp.path().join("state");
    std::fs::create_dir_all(&state).expect("state dir");
    let ledger = state.join(meerkat_meta::storage_predecessor::PREDECESSOR_MEMORY_LEDGER_FILE_NAME);
    let bytes = b"{\"assertions\": [], \"conflicts\": []}";
    std::fs::write(&ledger, bytes).expect("seed the predecessor ledger");

    let error = Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(test_definition())
            .persistent_state(&state)
            .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
            .build(),
    )
    .await
    .err()
    .expect("a predecessor ledger must refuse to boot");
    let message = error.to_string();
    assert!(
        message.contains(&ledger.display().to_string())
            && message.contains(&format!("--state-dir {} --apply", state.display())),
        "the refusal must name the ledger and the exact adoption command: {message}"
    );
    assert_eq!(std::fs::read(&ledger).expect("predecessor ledger"), bytes);
    assert!(
        !state
            .join(meerkat_meta::storage_layout::MEMORY_LEDGER_FILE_NAME)
            .exists()
    );
    assert!(!state.join("meta_metadata.sqlite3").exists());
}

// ---------------------------------------------------------------------------
// rpc_gateway (SDK stdio surface)
// ---------------------------------------------------------------------------

/// Per-call mob id: 0.8.23's fail-closed in-proc registration means
/// concurrently running tests must not share a supervisor route.
fn rpc_mob_config() -> String {
    format!(
        r#"
[mob]
id = "storage-layout-rpc-test-{}"

[profiles.default]
model = "gpt-5.5"
"#,
        NEXT_TEST_MOB_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

/// Spawn the real `rpc_gateway --persistent`, send one `meta/init` with
/// `persistent_state` and `mob_config`, and return the init response.
fn rpc_gateway_init(state_dir: &Path, mob_config: &str) -> Value {
    rpc_gateway_init_with(
        &json!({
            "persistent_state": state_dir,
            "mob_config": mob_config,
        }),
        &[],
    )
}

/// [`rpc_gateway_init`] with the init `params` given, and `env` set in the
/// gateway's environment.
fn rpc_gateway_init_with(params: &Value, env: &[(&str, &Path)]) -> Value {
    rpc_gateway_session(params, env, &[])
        .into_iter()
        .next()
        .expect("the init response")
}

/// Spawn the real `rpc_gateway --persistent` with `env` set, send `meta/init`
/// with `params`, then each of `requests` once init has answered, and return
/// the init response followed by each request's response, each matched by
/// its id. A refused init ends the session.
fn rpc_gateway_session(params: &Value, env: &[(&str, &Path)], requests: &[Value]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rpc_gateway"))
        .arg("--persistent")
        .envs(env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn rpc_gateway");
    let mut stdin = child.stdin.take().expect("gateway stdin");
    let stdout = child.stdout.take().expect("gateway stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let init = json!({
        "jsonrpc": "2.0",
        "id": "init",
        "method": "meta/init",
        "params": params,
    });
    let mut responses = Vec::new();
    for request in std::iter::once(&init).chain(requests) {
        writeln!(
            stdin,
            "{}",
            serde_json::to_string(request).expect("request json")
        )
        .expect("write request");
        stdin.flush().expect("flush request");
        let response = loop {
            let line = rx
                .recv_timeout(Duration::from_secs(90))
                .unwrap_or_else(|_| panic!("rpc_gateway did not answer {}", request["method"]));
            let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            // Notifications carry no id; a refusal of an unreadable line
            // carries a null one.
            if message["id"] == request["id"]
                || (message["id"].is_null() && message.get("error").is_some())
            {
                break message;
            }
        };
        let init_refused = request["method"] == "meta/init" && response.get("error").is_some();
        responses.push(response);
        if init_refused {
            break;
        }
    }
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    responses
}

/// `meta/mobpacks/deploy` with `execute`: a deploy that runs `rkat`, which a
/// runtime holding a named provider credential store refuses before reading
/// any document.
fn executing_deploy() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": "deploy",
        "method": "meta/mobpacks/deploy",
        "params": { "execute": true },
    })
}

/// The deploy reply is the typed refusal of a runtime holding a named
/// provider credential store.
fn assert_named_store_refusal(launch: &str, deploy: &Value) {
    assert_eq!(
        (
            deploy["error"]["code"].clone(),
            deploy["error"]["data"]["kind"].clone()
        ),
        (
            json!(meerkat_meta::CAPABILITY_UNAVAILABLE_CODE),
            json!("provider_credentials_not_carried")
        ),
        "{launch}: the runtime must refuse an executing deploy: {deploy}"
    );
}

/// POST `body` as JSON to `path` on a gateway's loopback HTTP listener and
/// return the JSON reply.
fn post_json(base_url: &str, path: &str, body: &Value) -> Value {
    let authority = base_url
        .strip_prefix("http://")
        .unwrap_or_else(|| panic!("a plain http base url: {base_url}"));
    let mut stream =
        std::net::TcpStream::connect(authority).expect("connect to the gateway listener");
    stream
        .set_read_timeout(Some(Duration::from_secs(90)))
        .expect("read timeout");
    let body = body.to_string();
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: {authority}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .expect("write the request");
    let mut reply = String::new();
    stream.read_to_string(&mut reply).expect("read the reply");
    let (_head, json) = reply
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("an HTTP reply: {reply}"));
    serde_json::from_str(json).unwrap_or_else(|error| panic!("a JSON reply ({error}): {reply}"))
}

/// A launch that names a provider credentials root opens it at boot, for a
/// persistent and a process-local launch alike, and records it: the
/// runtime then refuses an executing deploy, whose `rkat` process could not
/// carry the store. An unusable root refuses init with the storage
/// resolution code. Nothing reaches the process default credential store,
/// which lies in a scratch directory.
#[test]
fn rpc_gateway_opens_the_named_credentials_root_at_boot() {
    let scratch = TempDir::new().expect("scratch dir");
    let config_home = scratch.path().join("config");
    let home = scratch.path().join("home");
    let env = [
        ("XDG_CONFIG_HOME", config_home.as_path()),
        ("HOME", home.as_path()),
    ];
    let state = TempDir::new().expect("state dir");
    let mob_config = rpc_mob_config();
    for (launch, persistent) in [("persistent", true), ("process-local", false)] {
        let root = scratch.path().join(launch).join("credentials");
        let mut params = json!({
            "mob_config": mob_config,
            "runtime_options": { "provider_credentials_root": root },
        });
        if persistent {
            params["persistent_state"] = json!(state.path());
        }
        let responses = rpc_gateway_session(&params, &env, &[executing_deploy()]);
        let response = &responses[0];
        assert!(
            response["result"]["contract_version"].is_string(),
            "{launch}: init must succeed: {response}"
        );
        assert!(root.is_dir(), "{launch}: the named root is opened at boot");
        assert_named_store_refusal(launch, &responses[1]);
    }

    let unusable = scratch.path().join("not-a-directory");
    std::fs::write(&unusable, b"").expect("a regular file");
    let refused = rpc_gateway_init_with(
        &json!({
            "mob_config": mob_config,
            "runtime_options": { "provider_credentials_root": unusable },
        }),
        &env,
    );
    assert_eq!(
        refused["error"]["code"],
        json!(meerkat_meta::STORAGE_RESOLUTION_CODE),
        "{refused}"
    );

    for default_store in [
        config_home.join("meerkat").join("credentials"),
        home.join(".config").join("meerkat").join("credentials"),
        home.join("Library")
            .join("Application Support")
            .join("meerkat")
            .join("credentials"),
    ] {
        assert!(
            !default_store.exists(),
            "nothing may reach the process default store {}",
            default_store.display()
        );
    }
}

#[test]
fn rpc_gateway_fresh_state_dir_converges_on_canonical_names() {
    let state = TempDir::new().expect("state dir");
    // One mob throughout: the directory's composition record belongs to it.
    let mob_config = rpc_mob_config();
    let response = rpc_gateway_init(state.path(), &mob_config);
    assert!(
        response["result"]["contract_version"].is_string(),
        "fresh init must succeed: {response}"
    );
    for canonical in [
        "runtime.sqlite",
        "meta_metadata.sqlite3",
        "meta_console.sqlite3",
    ] {
        assert!(
            state.path().join(canonical).exists(),
            "fresh boot must create canonical {canonical}"
        );
    }
    assert_no_sessions_database(state.path());

    // The second start of the directory the first one left (killed after
    // init, as a crash would) reads it as native and starts.
    let again = rpc_gateway_init(state.path(), &mob_config);
    assert!(
        again["result"]["contract_version"].is_string(),
        "a directory this version created must start again: {again}"
    );
    assert_no_sessions_database(state.path());
}

/// `rpc_gateway`'s historical sessions spelling is the 0.9.3 session
/// projection: init refuses it with the storage-resolution code before any
/// store opens and leaves it where it lies.
#[test]
fn rpc_gateway_refuses_a_legacy_sessions_database_until_it_is_migrated() {
    let state = TempDir::new().expect("state dir");
    touch(&state.path().join("sessions.db"));

    let response = rpc_gateway_init(state.path(), &rpc_mob_config());
    assert_eq!(
        response["error"]["code"],
        json!(meerkat_meta::STORAGE_RESOLUTION_CODE),
        "{response}"
    );
    let message = response["error"]["message"].as_str().unwrap_or_default();
    assert_native_migration_refusal(message, &["sessions.db"]);
    assert_refused_boot_left_the_directory(state.path(), &["sessions.db"]);
}

#[test]
fn rpc_gateway_refuses_session_file_name_twins() {
    let state = TempDir::new().expect("state dir");
    touch(&state.path().join("sessions.db"));
    touch(&state.path().join("sessions.sqlite3"));

    let response = rpc_gateway_init(state.path(), &rpc_mob_config());
    assert_eq!(
        response["error"]["code"],
        json!(meerkat_meta::STORAGE_RESOLUTION_CODE),
        "{response}"
    );
    let message = response["error"]["message"].as_str().unwrap_or_default();
    assert_native_migration_refusal(message, &["sessions.db", "sessions.sqlite3"]);
    assert_refused_boot_left_the_directory(state.path(), &["sessions.db", "sessions.sqlite3"]);
}

#[test]
fn rpc_gateway_refuses_unadopted_predecessor_state() {
    let state = TempDir::new().expect("state dir");
    touch(
        &state
            .path()
            .join(meerkat_meta::storage_predecessor::PREDECESSOR_CONSOLE_DB_FILE_NAMES[0]),
    );

    let response = rpc_gateway_init(state.path(), &rpc_mob_config());
    let message = response["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("predecessor") && message.contains("storage-adopt-predecessor"),
        "predecessor state must fail init with the adoption pointer: {response}"
    );
    assert!(
        message.contains(&format!("--state-dir {} --apply", state.path().display())),
        "the pointer must name the gateway's persistent_state: {response}"
    );
    assert!(!state.path().join("meta_console.sqlite3").exists());
}

// ---------------------------------------------------------------------------
// meta_gateway (console/HTTP surface)
// ---------------------------------------------------------------------------

/// Spawn the real `meta_gateway`, send one `meta/init` pointed at
/// `store_dir` with `persistent_sessions: true`, and return the init
/// response. XDG state is isolated per test (shared-home CI flake class).
fn meta_gateway_init(workspace: &Path, store_dir: &Path) -> Value {
    with_meta_gateway(
        workspace,
        &json!({
            "workspace_root": workspace,
            "store_path": store_dir,
            "persistent_sessions": true,
        }),
        &[],
        |_| (),
    )
    .0
}

/// Spawn the real `meta_gateway` in `workspace` with `env` set, send
/// `meta/init` with `params`, run `while_up` with the init response while the
/// gateway still serves, then stop it.
fn with_meta_gateway<T>(
    workspace: &Path,
    params: &Value,
    env: &[(&str, &Path)],
    while_up: impl FnOnce(&Value) -> T,
) -> (Value, T) {
    let init = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "meta/init",
        "params": params,
    });
    let mut child = Command::new(env!("CARGO_BIN_EXE_meta_gateway"))
        .current_dir(workspace)
        // Dummy provider secrets: the fallback mob's LLM client is created
        // at bootstrap but never called (mirrors meta_gateway_bootstrap).
        .env("ANTHROPIC_API_KEY", "sk-ant-regression-test")
        .env("OPENAI_API_KEY", "sk-regression-test")
        .env("XDG_STATE_HOME", workspace.join("xdg-state"))
        .envs(env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn meta_gateway");

    let mut stdin = child.stdin.take().expect("stdin");
    writeln!(
        stdin,
        "{}",
        serde_json::to_string(&init).expect("init json")
    )
    .expect("write init");
    stdin.flush().expect("flush");

    let stdout = child.stdout.take().expect("stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let _ = reader.read_line(&mut line);
        let _ = tx.send(line);
    });
    let line = rx
        .recv_timeout(Duration::from_secs(90))
        .expect("meta_gateway did not answer meta/init");
    let response: Value = serde_json::from_str(line.trim()).expect("init response json");
    let outcome = while_up(&response);
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    (response, outcome)
}

/// meta_gateway's top-level `provider_credentials_root` is opened at boot and
/// recorded: its Flow Editor refuses an executing deploy, whose `rkat`
/// process could not carry the store. An unusable root refuses init with
/// the storage resolution code. Nothing reaches the process default
/// credential store, which lies in a scratch directory.
#[test]
fn meta_gateway_opens_the_named_credentials_root_at_boot() {
    let workspace = TempDir::new().expect("workspace");
    let store = TempDir::new().expect("store");
    let scratch = TempDir::new().expect("scratch dir");
    let config_home = scratch.path().join("config");
    let home = scratch.path().join("home");
    let env = [
        ("XDG_CONFIG_HOME", config_home.as_path()),
        ("HOME", home.as_path()),
    ];
    let root = scratch.path().join("credentials");
    let (response, deploy) = with_meta_gateway(
        workspace.path(),
        &json!({
            "workspace_root": workspace.path(),
            "store_path": store.path().join("store"),
            "persistent_sessions": true,
            "provider_credentials_root": root,
        }),
        &env,
        |response| {
            response["result"]["http_base_url"]
                .as_str()
                .map(|base_url| post_json(base_url, "/flow-editor/rpc", &executing_deploy()))
        },
    );
    assert!(
        response.get("error").is_none(),
        "init must succeed: {response}"
    );
    assert!(root.is_dir(), "the named root is opened at boot");
    assert_named_store_refusal(
        "meta_gateway",
        &deploy.expect("init reports its http_base_url"),
    );

    let unusable = scratch.path().join("not-a-directory");
    std::fs::write(&unusable, b"").expect("a regular file");
    let refusing = TempDir::new().expect("workspace");
    let (refused, ()) = with_meta_gateway(
        refusing.path(),
        &json!({
            "workspace_root": refusing.path(),
            "store_path": store.path().join("refused-store"),
            "persistent_sessions": true,
            "provider_credentials_root": unusable,
        }),
        &env,
        |_| (),
    );
    assert_eq!(
        refused["error"]["code"],
        json!(meerkat_meta::STORAGE_RESOLUTION_CODE),
        "{refused}"
    );

    for default_store in [
        config_home.join("meerkat").join("credentials"),
        home.join(".config").join("meerkat").join("credentials"),
        home.join("Library")
            .join("Application Support")
            .join("meerkat")
            .join("credentials"),
    ] {
        assert!(
            !default_store.exists(),
            "nothing may reach the process default store {}",
            default_store.display()
        );
    }
}

#[test]
fn meta_gateway_fresh_store_dir_converges_on_canonical_names() {
    let workspace = TempDir::new().expect("workspace");
    let store = TempDir::new().expect("store");
    let store_dir = store.path().join("store");

    let response = meta_gateway_init(workspace.path(), &store_dir);
    assert!(
        response.get("error").is_none(),
        "fresh init must succeed: {response}"
    );
    for canonical in ["runtime.sqlite", "continuity.sqlite3"] {
        assert!(
            store_dir.join(canonical).exists(),
            "fresh boot must create canonical {canonical}"
        );
    }
    assert_no_sessions_database(&store_dir);
    assert!(
        !store_dir.join("continuity.db").exists(),
        "fresh boot must not create legacy continuity.db"
    );

    // The second start of the store directory the first one left (killed
    // after init, as a crash would) reads it as native and creates a new
    // runtime on it.
    let again = meta_gateway_init(workspace.path(), &store_dir);
    assert_eq!(
        again["result"]["launch_state"],
        json!("created"),
        "a directory this version created must start again: {again}"
    );
    assert_no_sessions_database(&store_dir);
}

#[test]
fn meta_gateway_keeps_using_a_legacy_named_store_dir_where_it_lies() {
    let workspace = TempDir::new().expect("workspace");
    let store = TempDir::new().expect("store");
    let store_dir = store.path().join("store");
    std::fs::create_dir_all(&store_dir).expect("store dir");
    // meta_gateway's own historical continuity spelling (recon census). Its
    // historical sessions spelling refuses instead
    // (`meta_gateway_refuses_a_legacy_sessions_database_until_it_is_migrated`).
    touch(&store_dir.join("continuity.db"));

    let response = meta_gateway_init(workspace.path(), &store_dir);
    assert!(
        response.get("error").is_none(),
        "legacy-named init must succeed: {response}"
    );
    let (legacy, canonical) = ("continuity.db", "continuity.sqlite3");
    assert!(
        store_dir.join(legacy).exists(),
        "legacy {legacy} must keep being used where it lies"
    );
    assert!(
        !store_dir.join(canonical).exists(),
        "boot must not create a canonical twin {canonical} beside {legacy}"
    );
    assert_no_sessions_database(&store_dir);
}

/// `meta_gateway`'s historical sessions spelling is the 0.9.3 session
/// projection: init refuses it with the storage-resolution code before any
/// store opens and leaves it where it lies.
#[test]
fn meta_gateway_refuses_a_legacy_sessions_database_until_it_is_migrated() {
    let workspace = TempDir::new().expect("workspace");
    let store = TempDir::new().expect("store");
    let store_dir = store.path().join("store");
    std::fs::create_dir_all(&store_dir).expect("store dir");
    touch(&store_dir.join("sessions.sqlite"));

    let response = meta_gateway_init(workspace.path(), &store_dir);
    assert_eq!(
        response["error"]["code"],
        json!(meerkat_meta::STORAGE_RESOLUTION_CODE),
        "{response}"
    );
    let message = response["error"]["message"].as_str().unwrap_or_default();
    assert_native_migration_refusal(message, &["sessions.sqlite"]);
    assert_refused_boot_left_the_directory(&store_dir, &["sessions.sqlite"]);
}

/// A gateway home that only exists under its predecessor (pre-0.9) name
/// refuses init instead of minting a new peer identity.
#[test]
fn meta_gateway_refuses_unadopted_predecessor_gateway_home() {
    let workspace = TempDir::new().expect("workspace");
    let store = TempDir::new().expect("store");
    let predecessor_home = workspace
        .path()
        .join("xdg-state")
        .join(meerkat_meta::storage_predecessor::PREDECESSOR_GATEWAY_HOME_DIR_NAME);
    std::fs::create_dir_all(&predecessor_home).expect("predecessor home");
    touch(&predecessor_home.join("peer_key.ed25519"));

    let response = meta_gateway_init(workspace.path(), &store.path().join("store"));
    let message = response["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("predecessor") && message.contains("storage-adopt-predecessor"),
        "predecessor home must fail init with the adoption pointer: {response}"
    );
    assert!(
        !workspace
            .path()
            .join("xdg-state")
            .join("meerkat-meta")
            .exists(),
        "a refused boot must not mint the renamed gateway home"
    );
}

/// A predecessor environment variable is configuration the renamed binary
/// would silently ignore; the gateway refuses to start instead.
#[test]
fn rpc_gateway_refuses_predecessor_environment() {
    let variable = meerkat_meta::storage_predecessor::PREDECESSOR_ENV_VARS
        .iter()
        .find(|name| name.ends_with("HTTP_LISTEN_ADDR"))
        .expect("the predecessor listen variable")
        .to_string();
    let output = Command::new(env!("CARGO_BIN_EXE_rpc_gateway"))
        .arg("--persistent")
        .env(&variable, "127.0.0.1:0")
        .stdin(Stdio::null())
        .output()
        .expect("run rpc_gateway");
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(&variable), "{stderr}");
}

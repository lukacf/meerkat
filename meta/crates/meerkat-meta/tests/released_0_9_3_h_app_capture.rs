//! The released Meta 0.9.3 H-app state directories under
//! `tests/fixtures/released_0_9_3_h_app/` are the bytes the published 0.9.3
//! crates wrote when an application embedded `UnifiedRuntimeBuilder` with
//! `persistent_state`, and the staging every consumer uses rebuilds the minted
//! final state from them exactly. See tests/fixtures/README.md and the mint
//! script beside the variants. The layout checks open staged copies of the
//! stores read-only.

#![allow(clippy::expect_used, clippy::panic)]

#[path = "support/released_0_9_3_h_app.rs"]
mod released_0_9_3_h_app;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use released_0_9_3_h_app::{DRIVER_FILES, MINT_SCRIPT, Variant, sha256_hex, stage_variant};
use serde_json::Value;

/// The mob composition manifest 0.9.3 writes beside a persistent mob store.
const COMPOSITION_SUFFIX: &str = ".composition.json";

#[test]
fn every_h_app_variant_stages_to_the_final_state_the_release_wrote() {
    for variant in Variant::ALL {
        let name = variant.dir_name();
        let temp = tempfile::tempdir().expect("temp dir");
        // stage_variant checks the committed bytes against checksums.sha256
        // and the staged tree against MANIFEST.json files.state.
        let state = stage_variant(variant, temp.path());
        assert!(
            state.join("runtime.sqlite").is_file() && state.join("sessions.sqlite3").is_file(),
            "{name} stages a 0.9.3 state directory"
        );
        assert_eq!(
            state.join("continuity.sqlite3").is_file(),
            variant.has_roster(),
            "{name}: 0.9.3 keeps continuity.sqlite3 exactly when the app serves a roster"
        );
        let composition_manifests: BTreeSet<String> = std::fs::read_dir(&state)
            .expect("read the staged state")
            .map(|entry| {
                entry
                    .expect("staged entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .filter(|file| file.ends_with(COMPOSITION_SUFFIX))
            .collect();
        let expected: BTreeSet<String> = variant
            .mob_store()
            .map(|store| format!("{store}{COMPOSITION_SUFFIX}"))
            .into_iter()
            .collect();
        assert_eq!(
            composition_manifests, expected,
            "{name}: a persistent mob store, and only one, leaves its composition manifest"
        );
        for store in ["event_log.sqlite3", "mob.sqlite3"] {
            assert_eq!(
                state.join(store).is_file(),
                variant.mob_store() == Some(store),
                "{name}: {store} is the app's mob store or absent"
            );
        }
        let mut key_files = Vec::new();
        collect_named(&state, "identity.key", &mut key_files);
        assert!(
            key_files.is_empty(),
            "{name}: the session comms keys live outside the state directory: {key_files:?}"
        );
    }
}

fn collect_named(dir: &Path, file: &str, out: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).expect("read a staged directory") {
        let path = entry.expect("staged entry").path();
        if path.is_dir() {
            collect_named(&path, file, out);
        } else if path.file_name().is_some_and(|name| name == file) {
            out.push(path.display().to_string());
        }
    }
}

#[test]
fn the_driver_and_script_beside_the_variants_are_the_ones_that_minted_them() {
    let root = released_0_9_3_h_app::fixture_root();
    let script_sha = sha256_hex(&std::fs::read(root.join(MINT_SCRIPT)).expect("read the script"));
    let driver_shas: BTreeMap<&str, String> = DRIVER_FILES
        .iter()
        .map(|file| {
            let bytes = std::fs::read(root.join("driver").join(file))
                .unwrap_or_else(|error| panic!("read driver/{file}: {error}"));
            (*file, sha256_hex(&bytes))
        })
        .collect();
    let mut binaries = BTreeSet::new();
    for variant in Variant::ALL {
        let name = variant.dir_name();
        let manifest = variant.manifest();
        let producer = &manifest["producer"];
        assert_eq!(manifest["variant"].as_str(), Some(name));
        assert_eq!(producer["wrapper"].as_str(), Some(MINT_SCRIPT));
        assert_eq!(
            producer["wrapper_sha256"].as_str(),
            Some(script_sha.as_str()),
            "{name} was minted by a different script; re-mint every variant or restore it"
        );
        for (file, sha) in &driver_shas {
            assert_eq!(
                producer["driver_files"][*file].as_str(),
                Some(sha.as_str()),
                "{name} was minted by a different driver/{file}; re-mint every variant or \
                 restore it"
            );
        }
        let crates = producer["meerkat_crates"]
            .as_object()
            .expect("meerkat_crates is an object");
        assert!(
            crates.contains_key("meerkat-meta") && crates.contains_key("meerkat"),
            "{name}: the lockfile resolves the published Meta and Meerkat crates"
        );
        for (krate, pin) in crates {
            assert_eq!(
                (pin["version"].as_str(), pin["source"].as_str()),
                (Some("0.9.3"), Some("registry")),
                "{name}: {krate} is not the published 0.9.3 crate"
            );
        }
        let mob_storage = producer["mob_storage"].as_str().expect("mob storage");
        match variant.mob_store() {
            Some(store) => assert_eq!(mob_storage, format!("persistent, state/{store}")),
            None => assert!(
                mob_storage.starts_with("in-memory"),
                "{name}: {mob_storage}"
            ),
        }
        binaries.insert(
            producer["binary_sha256"]
                .as_str()
                .expect("binary sha")
                .to_string(),
        );
    }
    assert_eq!(binaries.len(), 1, "one driver binary minted every variant");
}

/// A staged copy of one of the variant's databases, opened read-only.
fn open_staged(state: &Path, file: &str) -> rusqlite::Connection {
    rusqlite::Connection::open_with_flags(
        state.join(file),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap_or_else(|error| panic!("open the staged {file} read-only: {error}"))
}

fn strings(connection: &rusqlite::Connection, sql: &str) -> Vec<String> {
    let mut statement = connection
        .prepare(sql)
        .unwrap_or_else(|error| panic!("prepare {sql}: {error}"));
    let rows = statement
        .query_map([], |row| row.get(0))
        .unwrap_or_else(|error| panic!("run {sql}: {error}"));
    rows.map(|row| row.expect("a text row")).collect()
}

/// The messages of every committed WholeBlob session, by session id.
fn runtime_messages(runtime: &rusqlite::Connection) -> BTreeMap<String, Value> {
    let mut statement = runtime
        .prepare(
            "SELECT authority.session_id, bodies.session_snapshot \
             FROM runtime_whole_blob_authority AS authority \
             JOIN runtime_whole_blob_bodies AS bodies \
               ON bodies.blob_sha256 = authority.blob_sha256",
        )
        .expect("prepare the WholeBlob body read");
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .expect("read the WholeBlob bodies");
    rows.map(|row| {
        let (session, body) = row.expect("a WholeBlob body row");
        let body: Value = serde_json::from_slice(&body).expect("the body is JSON");
        (session, body["messages"].clone())
    })
    .collect()
}

/// The messages of every session in sessions.sqlite3, by session id.
fn projected_messages(sessions: &rusqlite::Connection) -> BTreeMap<String, Value> {
    let mut statement = sessions
        .prepare("SELECT session_id, session_json FROM sessions")
        .expect("prepare the sessions read");
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .expect("read the sessions");
    rows.map(|row| {
        let (session, json) = row.expect("a sessions row");
        let session_json: Value = serde_json::from_slice(&json).expect("the session is JSON");
        (session, session_json["messages"].clone())
    })
    .collect()
}

/// An application that embedded the 0.9.3 builder with a roster keeps every
/// session body in sessions.sqlite3, the same as the plain app, and only
/// the identity bindings in continuity.sqlite3: a schema-1 store holding
/// continuity_records and an empty session_snapshots, with none of the
/// head, strand or rewrite tables the gateway's store gains at schema 2.
/// storage-migrate-native needs a shape of its own for this layout, and the
/// roster captures are its inputs. The roster's last boot dropped
/// `app:app-06`, whose record stays.
#[test]
fn the_builder_keeps_bodies_in_the_sessions_store_and_only_bindings_in_continuity() {
    for variant in Variant::ALL {
        let name = variant.dir_name();
        let temp = tempfile::tempdir().expect("temp dir");
        let state = stage_variant(variant, temp.path());
        let runtime = runtime_messages(&open_staged(&state, "runtime.sqlite"));
        let projection = projected_messages(&open_staged(&state, "sessions.sqlite3"));
        assert_eq!(
            projection, runtime,
            "{name}: sessions.sqlite3 projects every committed session with its messages"
        );
        if !variant.has_roster() {
            assert_eq!(runtime.len(), 4, "{name}: two members over two boots");
            continue;
        }
        let continuity = open_staged(&state, "continuity.sqlite3");
        let schema: Vec<(String, i64)> = {
            let mut statement = continuity
                .prepare("SELECT * FROM meerkat_schema")
                .expect("prepare the schema read");
            let rows = statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .expect("read the schema");
            rows.map(|row| row.expect("a schema row")).collect()
        };
        assert_eq!(schema, [("meta-continuity".to_string(), 1)], "{name}");
        assert_eq!(
            strings(
                &continuity,
                "SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name"
            ),
            ["continuity_records", "meerkat_schema", "session_snapshots"],
            "{name}: continuity.sqlite3 has no head, strand or rewrite table"
        );
        assert!(
            strings(&continuity, "SELECT session_id FROM session_snapshots").is_empty(),
            "{name}: continuity.sqlite3 holds no session body"
        );
        let mut statement = continuity
            .prepare("SELECT identity, session_id FROM continuity_records")
            .expect("prepare the record read");
        let records: BTreeMap<String, String> = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("read the records")
            .map(|row| row.expect("a record"))
            .collect();
        let identities: Vec<&str> = records.keys().map(String::as_str).collect();
        assert_eq!(
            identities,
            [
                "app:app-01",
                "app:app-02",
                "app:app-03",
                "app:app-04",
                "app:app-05",
                "app:app-06"
            ],
            "{name}: one record per identity the roster ever listed"
        );
        for (identity, session) in &records {
            assert!(
                runtime.contains_key(session),
                "{name}: {identity}'s record binds {session}, which has no committed body"
            );
        }
        assert_eq!(
            runtime.len(),
            records.len() + 2,
            "{name}: the bound sessions plus the ones app-04's reset and app-05's delete left \
             behind"
        );
    }
}

/// Each refusal the 0.9.3 runtime answered the driver, as (boot, op,
/// identity, message).
fn refusals(manifest: &Value) -> Vec<(u64, String, String, String)> {
    manifest["refusals"]
        .as_array()
        .expect("refusals is an array")
        .iter()
        .map(|row| {
            (
                row["boot"].as_u64().expect("refusal boot"),
                row["op"].as_str().expect("refusal op").to_string(),
                row["identity"].as_str().unwrap_or_default().to_string(),
                row["message"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The 0.9.3 outcomes the upgrade acceptance tests start from. A re-mint that
/// stops reproducing one of them, or adds another, changes what those tests
/// prove, so it fails here first.
#[test]
fn the_manifests_record_the_093_outcomes_the_upgrade_tests_start_from() {
    for variant in Variant::ALL {
        let name = variant.dir_name();
        let manifest = variant.manifest();
        assert_eq!(
            manifest["restart_probe_093"]["started"].as_bool(),
            Some(true),
            "{name}: 0.9.3 builds again on its own final state"
        );
        let comms = &manifest["outside_state"]["session_comms_identity"];
        assert_eq!(comms["stored"].as_bool(), Some(false), "{name}");
        let committed = manifest["committed_sessions"]
            .as_object()
            .expect("committed_sessions is an object");
        let listed = comms["sessions"].as_object().expect("listed comms keys");
        assert_eq!(
            listed.len(),
            committed.len(),
            "{name}: 0.9.3 kept one comms keypair per session, outside the state directory"
        );
        let recorded = refusals(&manifest);
        if !variant.has_roster() {
            assert!(recorded.is_empty(), "{name}: {recorded:?}");
            continue;
        }
        let refused: BTreeSet<(u64, &str, &str)> = recorded
            .iter()
            .map(|(boot, op, identity, message)| {
                assert!(
                    message.contains("unknown identity"),
                    "{name}: the refusal of {op} on {identity} at boot {boot} changed: \
                     {message}"
                );
                (*boot, op.as_str(), identity.as_str())
            })
            .collect();
        assert_eq!(
            refused,
            BTreeSet::from([
                (1, "status", "app:app-05"),
                (1, "turn", "app:app-05"),
                (3, "status", "app:app-06"),
            ]),
            "{name}: app-05 is unknown after its delete until the next roster compile, and \
             app-06 once the roster drops it"
        );
        let last = manifest["statuses"]["3"]
            .as_object()
            .expect("boot 3 statuses");
        let bound: Vec<&str> = last.keys().map(String::as_str).collect();
        assert_eq!(
            bound,
            [
                "app:app-01",
                "app:app-02",
                "app:app-03",
                "app:app-04",
                "app:app-05"
            ],
            "{name}"
        );
        for (identity, status) in last {
            assert_eq!(
                status["state"].as_str(),
                Some("active"),
                "{name}: {identity}"
            );
            // Only app-04 was reset, to generation 1.
            let generation = u64::from(identity == "app:app-04");
            assert_eq!(
                status["generation"].as_u64(),
                Some(generation),
                "{name}: {identity}'s generation; only app-04 was reset"
            );
            for boot in ["1", "2"] {
                let earlier = &manifest["statuses"][boot][identity]["session_id"];
                let same = earlier == &status["session_id"];
                assert_eq!(
                    same,
                    !(identity == "app:app-05" && boot == "1"),
                    "{name}: {identity} at boot {boot}; only app-05's re-add moved it to a \
                     new session"
                );
            }
        }
        let readded = last["app:app-05"]["session_id"]
            .as_str()
            .expect("app-05 session");
        let body = &committed[readded];
        assert_eq!(
            (body["store_revision"].as_u64(), body["messages"].as_u64()),
            (Some(1), Some(1)),
            "{name}: app-05's re-added session never took a turn"
        );
    }
}

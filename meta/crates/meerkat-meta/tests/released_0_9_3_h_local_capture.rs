//! The released Meta 0.9.3 H-local state directories under
//! `tests/fixtures/released_0_9_3_h_local/` are the bytes the published 0.9.3
//! rpc_gateway wrote, and the staging every consumer uses rebuilds the minted
//! final state from them exactly. See tests/fixtures/README.md and the mint
//! scripts beside the variants. These tests read the committed files as bytes
//! only; the re-anchored and re-materialized session checks open staged copies
//! of runtime.sqlite and continuity.sqlite3 read-only.

#![allow(clippy::expect_used, clippy::panic)]

#[path = "support/released_0_9_3_h_local.rs"]
mod released_0_9_3_h_local;

use std::collections::{BTreeMap, BTreeSet};

use released_0_9_3_h_local::{Variant, sha256_hex, stage_variant};
use serde_json::Value;

#[test]
fn every_h_local_variant_stages_to_the_final_state_the_release_wrote() {
    for variant in Variant::ALL {
        let temp = tempfile::tempdir().expect("temp dir");
        // stage_variant checks the committed bytes against checksums.sha256
        // and the staged tree against MANIFEST.json files.state.
        let state = stage_variant(variant, temp.path());
        assert!(
            state.join("runtime.sqlite").is_file() && state.join("continuity.sqlite3").is_file(),
            "{} stages a 0.9.3 state directory",
            variant.dir_name()
        );
        assert!(
            !state.join("peer_key.ed25519").exists(),
            "the peer key is left out of every capture"
        );
    }
}

#[test]
fn the_mint_script_beside_the_variants_is_the_one_that_minted_them() {
    for variant in Variant::ALL {
        let script =
            std::fs::read(released_0_9_3_h_local::fixture_root().join(variant.driver_file()))
                .expect("read the mint script");
        let script_sha = sha256_hex(&script);
        let manifest = variant.manifest();
        assert_eq!(
            manifest["producer"]["driver_sha256"].as_str(),
            Some(script_sha.as_str()),
            "{} was minted by a different script; re-mint every variant with the released \
             binary or restore the script",
            variant.dir_name()
        );
        assert_eq!(manifest["variant"].as_str(), Some(variant.dir_name()));
        assert_eq!(
            manifest["producer"]["release_asset_sha256"].as_str(),
            Some("b416df1ba5648ca3ed849dec2692f664c37b0e7277232a6d2421ac53961a18b0"),
            "every variant comes from the published 0.9.3 rpc_gateway archive"
        );
        // The script hashes the binary it ran, so this one is measured, not
        // typed by the operator.
        assert_eq!(
            manifest["producer"]["binary_sha256"].as_str(),
            Some("757519ed1210d16d865e2e37de3fba098a1cca6ccba6a7e1dc646963fb04a335"),
            "every variant was minted by the rpc_gateway inside that archive"
        );
    }
}

/// Each refusal the 0.9.3 gateway answered, as (boot, op, identity, message).
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

/// The 0.9.3 bridge rejected the identity's resume and left it degraded.
const RESUME_REJECTED: &str = "bridge resume_session rejected (identity degraded";
/// The compact_member maintenance turn never reached a terminal.
const MAINTENANCE_TURN_TIMED_OUT: &str = "did not complete within";

/// `recorded` holds the refusal of `op` on `identity` at `boot`, and its
/// message still names `cause`: the cause decides the durable state the
/// upgrade tests start from.
fn assert_refused(
    name: &str,
    recorded: &[(u64, String, String, String)],
    (boot, op, identity): (u64, &str, &str),
    cause: &str,
    what: &str,
) {
    let row = recorded
        .iter()
        .find(|(b, o, i, _)| *b == boot && o == op && i == identity)
        .unwrap_or_else(|| {
            panic!(
                "{name}: {what}: no refusal of {op} on {identity} at boot {boot} in {recorded:?}"
            )
        });
    assert!(
        row.3.contains(cause),
        "{name}: {what}: the refusal of {op} on {identity} at boot {boot} no longer carries \
         {cause:?}: {}",
        row.3
    );
}

/// The 0.9.3 outcomes the upgrade acceptance tests start from. A re-mint that
/// stops reproducing one of them, or adds another, changes what those tests
/// prove, so it fails here first. rematerialized/ follows a plan of its own,
/// and its test pins its outcomes.
#[test]
fn the_manifests_record_the_093_outcomes_the_upgrade_tests_start_from() {
    for variant in Variant::H_LOCAL_PLAN {
        let manifest = variant.manifest();
        let name = variant.dir_name();
        let rosters = manifest["rosters"].as_object().expect("rosters");
        let first_roster = rosters["1"].as_array().expect("first roster");
        assert_eq!(
            first_roster.len(),
            18,
            "{name}: the first roster lists every identity"
        );
        assert!(
            first_roster.contains(&Value::from("agent:hl-18")),
            "{name}: agent:hl-18 starts on the roster"
        );
        let last_boot = rosters
            .keys()
            .map(|boot| boot.parse::<u64>().expect("boot number"))
            .max()
            .expect("at least one boot");
        let last_roster = rosters[&last_boot.to_string()]
            .as_array()
            .expect("last roster");
        assert_eq!(
            last_roster.len(),
            17,
            "{name}: the final roster drops one identity"
        );
        assert!(
            !last_roster.contains(&Value::from("agent:hl-18")),
            "{name}: agent:hl-18 is the identity the roster drops"
        );
        assert_eq!(
            manifest["identities"]["agent:hl-18"]["final_status"]["state"].as_str(),
            Some("broken"),
            "{name}: 0.9.3 reports the dropped identity Broken"
        );
        let recorded = refusals(&manifest);
        assert_refused(
            name,
            &recorded,
            (1, "turn_refused", "agent:hl-04"),
            RESUME_REJECTED,
            "0.9.3 refuses the boot-1 wake after retire",
        );
        assert_refused(
            name,
            &recorded,
            (2, "meta/compact_member", "agent:hl-11"),
            RESUME_REJECTED,
            "0.9.3 fails compact_member on agent:hl-11",
        );
        let probe = &manifest["restart_probe_093"];
        let killed_last = manifest["shutdowns"][last_boot.to_string()]["killed"].as_bool();
        match variant {
            Variant::Bootable | Variant::Reanchored => {
                assert_eq!(probe["started"].as_bool(), Some(true), "{name}: {probe}");
                assert_eq!(killed_last, Some(false), "{name}");
                assert_eq!(recorded.len(), 2, "{name}: {recorded:?}");
            }
            Variant::Stuck => {
                assert_eq!(probe["started"].as_bool(), Some(false), "{name}: {probe}");
                assert!(
                    probe["message"]
                        .as_str()
                        .is_some_and(|message| message.contains("mob actor command channel closed")),
                    "{name}: 0.9.3 cannot start the stuck state: {probe}"
                );
                assert_eq!(killed_last, Some(false), "{name}");
                assert_eq!(recorded.len(), 2, "{name}: {recorded:?}");
            }
            Variant::Killed => {
                assert_eq!(probe["started"].as_bool(), Some(true), "{name}: {probe}");
                assert_eq!(killed_last, Some(true), "{name}: the last boot is killed");
                assert_refused(
                    name,
                    &recorded,
                    (3, "meta/compact_member", "agent:hl-14"),
                    MAINTENANCE_TURN_TIMED_OUT,
                    "compact_member on agent:hl-14 never completes",
                );
                assert_eq!(recorded.len(), 3, "{name}: {recorded:?}");
            }
            Variant::Rematerialized | Variant::DirectMember => {
                unreachable!("{name} is not a capture of the H-local plan")
            }
        }
    }
}

/// A staged copy of one of the variant's databases, opened read-only.
fn open_staged(state: &std::path::Path, file: &str) -> rusqlite::Connection {
    rusqlite::Connection::open_with_flags(
        state.join(file),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap_or_else(|error| panic!("open the staged {file} read-only: {error}"))
}

/// A committed WholeBlob session: its store revision and the session body
/// the 0.9.3 gateway stored.
struct CommittedBody {
    store_revision: i64,
    body: Value,
}

/// Every committed WholeBlob session, by session id.
fn committed_bodies(runtime: &rusqlite::Connection) -> BTreeMap<String, CommittedBody> {
    let mut statement = runtime
        .prepare(
            "SELECT authority.session_id, authority.store_revision, bodies.session_snapshot \
             FROM runtime_whole_blob_authority AS authority \
             JOIN runtime_whole_blob_bodies AS bodies \
               ON bodies.blob_sha256 = authority.blob_sha256",
        )
        .expect("prepare the WholeBlob body read");
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })
        .expect("read the WholeBlob bodies");
    rows.map(|row| {
        let (session, store_revision, body) = row.expect("a WholeBlob body row");
        let body: Value = serde_json::from_slice(&body).expect("the body is JSON");
        (
            session,
            CommittedBody {
                store_revision,
                body,
            },
        )
    })
    .collect()
}

/// Every committed WholeBlob session with the transcript graph the 0.9.3
/// gateway stored in its body (`session_transcript_history_state_v1`, Null
/// for a session that never rewrote its transcript), by session id.
fn committed_transcript_graphs(runtime: &rusqlite::Connection) -> BTreeMap<String, Value> {
    committed_bodies(runtime)
        .into_iter()
        .map(|(session, committed)| {
            (
                session,
                committed.body["metadata"]["session_transcript_history_state_v1"].clone(),
            )
        })
        .collect()
}

/// The rewrite generations of a graph's retained edges or retired commits.
fn rewrite_generations(entries: &Value) -> Vec<u64> {
    entries
        .as_array()
        .expect("an array of rewrites")
        .iter()
        .map(|entry| {
            entry
                .get("commit")
                .unwrap_or(entry)
                .get("rewrite_generation")
                .and_then(Value::as_u64)
                .expect("rewrite generation")
        })
        .collect()
}

/// The upgrade has to convert sessions whose transcript graph no longer
/// starts at the pre-rewrite transcript, bound or not. In reanchored/,
/// `agent:hl-16` and `agent:hl-17` each compacted six times under the 0.9.3
/// retention, so each committed graph kept the newest four rewrites and
/// retired the two oldest. hl-16 stays bound to its session; hl-17's outlived
/// a delete_identity in runtime.sqlite only. No other session is re-anchored.
#[test]
fn the_reanchored_variant_holds_a_bound_and_a_runtime_only_reanchored_session() {
    let variant = Variant::Reanchored;
    let manifest = variant.manifest();
    // The compaction config the mint used: the [ct] trigger reports more than
    // the pinned threshold, and retention and cadence are not overridden.
    let compaction = &manifest["producer"]["compaction"];
    let threshold = compaction["auto_compact_threshold"]
        .as_u64()
        .expect("compaction threshold");
    assert_eq!(threshold, 50_000, "{compaction}");
    assert!(
        compaction["reported_prompt_tokens"]
            .as_u64()
            .is_some_and(|reported| reported > threshold),
        "{compaction}"
    );
    assert_eq!(
        compaction["recent_turn_budget"].as_u64(),
        Some(1),
        "{compaction}"
    );
    assert!(
        compaction.get("history_retained_rewrites").is_none()
            && compaction.get("min_turns_between_compactions").is_none(),
        "the mint leaves retention and cadence at the 0.9.3 defaults: {compaction}"
    );

    let rows = manifest["reanchored"].as_array().expect("reanchored rows");
    let identities: Vec<&str> = rows
        .iter()
        .map(|row| row["identity"].as_str().expect("row identity"))
        .collect();
    assert_eq!(identities, ["agent:hl-16", "agent:hl-17"]);

    let temp = tempfile::tempdir().expect("temp dir");
    let state = stage_variant(variant, temp.path());
    let graphs = committed_transcript_graphs(&open_staged(&state, "runtime.sqlite"));
    let continuity = open_staged(&state, "continuity.sqlite3");
    for row in rows {
        let identity = row["identity"].as_str().expect("row identity");
        let session = row["session_id"].as_str().expect("row session");
        let bound = identity == "agent:hl-16";
        let facts = &manifest["identities"][identity];
        assert_eq!(
            facts["sessions_seen"][0].as_str(),
            Some(session),
            "{identity}: the re-anchored session is the identity's first"
        );
        assert_eq!(
            facts["final_status"]["session_id"].as_str() == Some(session),
            bound,
            "{identity}: only hl-16 is still bound to its re-anchored session: {facts}"
        );
        assert_eq!(row["bound"].as_bool(), Some(bound), "{row}");

        // Only runtime.sqlite still holds hl-17's session: 0.9.3 dropped the
        // continuity copy at delete_identity and kept the runtime body.
        let heads: i64 = continuity
            .query_row(
                "SELECT count(*) FROM continuity_session_heads WHERE session_id = ?1",
                [session],
                |count| count.get(0),
            )
            .expect("count continuity heads");
        assert_eq!(
            heads,
            i64::from(bound),
            "{identity}: continuity.sqlite3 holds a head for {session} only while it is bound"
        );
        assert_eq!(row["continuity_copy"].as_bool(), Some(bound), "{row}");

        // The committed body itself, not only the manifest's account of it.
        let graph = graphs
            .get(session)
            .unwrap_or_else(|| panic!("{identity}: no committed body for {session}"));
        let retained = rewrite_generations(&graph["edges"]);
        let retired = rewrite_generations(&graph["retired"]["commits"]);
        assert_eq!(
            graph["rewrite_prefix"]["occurrence_count"].as_u64(),
            Some(6),
            "{identity}: six compactions"
        );
        // Retention read from the data: of six rewrites, the newest four keep
        // their bodies.
        assert_eq!(
            retained,
            [3, 4, 5, 6],
            "{identity}: the graph keeps the bodies of the newest 4 rewrites"
        );
        assert_eq!(
            retired,
            [1, 2],
            "{identity}: the two oldest rewrites are retired"
        );
        let recorded = &row["transcript_history"];
        assert_eq!(
            recorded["retained_generations"],
            serde_json::json!(retained),
            "{row}"
        );
        assert_eq!(
            recorded["retired_generations"],
            serde_json::json!(retired),
            "{row}"
        );
        assert_eq!(
            recorded["retired"].as_u64(),
            Some(retired.len() as u64),
            "{row}"
        );
    }
    let reanchored: Vec<&str> = graphs
        .iter()
        .filter(|(_, graph)| !graph["retired"].is_null())
        .map(|(session, _)| session.as_str())
        .collect();
    let mut expected: Vec<&str> = rows
        .iter()
        .map(|row| row["session_id"].as_str().expect("row session"))
        .collect();
    expected.sort_unstable();
    assert_eq!(
        reanchored, expected,
        "exactly the two rows' bodies are re-anchored"
    );
}

/// Each identity's continuity record, as the session it binds.
fn continuity_bindings(continuity: &rusqlite::Connection) -> BTreeMap<String, String> {
    let mut statement = continuity
        .prepare("SELECT identity, session_id FROM continuity_records")
        .expect("prepare the continuity record read");
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("read the continuity records");
    rows.map(|row| row.expect("a continuity record")).collect()
}

/// The sessions with at least one row in a continuity table.
fn sessions_in(continuity: &rusqlite::Connection, table: &str) -> BTreeSet<String> {
    let mut statement = continuity
        .prepare(&format!("SELECT DISTINCT session_id FROM {table}"))
        .unwrap_or_else(|error| panic!("prepare the {table} read: {error}"));
    let rows = statement
        .query_map([], |row| row.get(0))
        .unwrap_or_else(|error| panic!("read {table}: {error}"));
    rows.map(|row| row.expect("a session id")).collect()
}

/// The messages of each session's snapshot row, by session id.
fn snapshot_messages(continuity: &rusqlite::Connection) -> BTreeMap<String, Value> {
    let mut statement = continuity
        .prepare("SELECT session_id, data FROM session_snapshots")
        .expect("prepare the snapshot read");
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .expect("read the snapshot rows");
    rows.map(|row| {
        let (session, data) = row.expect("a snapshot row");
        let snapshot: Value = serde_json::from_slice(&data).expect("the snapshot is JSON");
        (session, snapshot["messages"].clone())
    })
    .collect()
}

/// The upgrade has to carry over identities bound to a session that never
/// took a turn. 0.9.3 writes a session's continuity head at its first turn,
/// so until then continuity.sqlite3 holds the session only as the snapshot
/// written when it was materialized. In rematerialized/, `agent:hl-02` is
/// deleted and materialized again and `agent:hl-03` is materialized for the
/// first time, neither taking a turn: each is bound to a one-message session
/// at revision 1 with a snapshot row and no head. hl-02's pre-delete session
/// outlives the delete in runtime.sqlite only, and `agent:hl-01`, which took
/// a turn, keeps a head.
#[test]
fn the_rematerialized_variant_binds_two_sessions_without_a_continuity_head() {
    let variant = Variant::Rematerialized;
    let manifest = variant.manifest();
    // The plan's own 0.9.3 outcomes: three clean boots, hl-03 joins the
    // roster at the last one, nothing is refused, and the 0.9.3 gateway
    // starts on the final state again.
    let rosters = &manifest["rosters"];
    let two = serde_json::json!(["agent:hl-01", "agent:hl-02"]);
    let three = serde_json::json!(["agent:hl-01", "agent:hl-02", "agent:hl-03"]);
    assert_eq!(
        rosters,
        &serde_json::json!({"1": two, "2": two, "3": three}),
        "the rosters of the three boots"
    );
    let shutdowns = manifest["shutdowns"].as_object().expect("shutdowns");
    assert_eq!(shutdowns.len(), 3, "{shutdowns:?}");
    for (boot, shutdown) in shutdowns {
        assert_eq!(
            shutdown["killed"].as_bool(),
            Some(false),
            "boot {boot} shuts down cleanly: {shutdown}"
        );
    }
    let recorded = refusals(&manifest);
    assert!(recorded.is_empty(), "0.9.3 refuses nothing: {recorded:?}");
    let probe = &manifest["restart_probe_093"];
    assert_eq!(probe["started"].as_bool(), Some(true), "{probe}");

    let rows = manifest["rematerialized"]
        .as_array()
        .expect("rematerialized rows");
    let identities: Vec<&str> = rows
        .iter()
        .map(|row| row["identity"].as_str().expect("row identity"))
        .collect();
    assert_eq!(identities, ["agent:hl-02", "agent:hl-03"]);

    let temp = tempfile::tempdir().expect("temp dir");
    let state = stage_variant(variant, temp.path());
    let bodies = committed_bodies(&open_staged(&state, "runtime.sqlite"));
    let continuity = open_staged(&state, "continuity.sqlite3");
    let bindings = continuity_bindings(&continuity);
    let heads = sessions_in(&continuity, "continuity_session_heads");
    let strands = sessions_in(&continuity, "continuity_strand_messages");
    let snapshots = snapshot_messages(&continuity);
    for row in rows {
        let identity = row["identity"].as_str().expect("row identity");
        let session = row["session_id"].as_str().expect("row session");
        let facts = &manifest["identities"][identity];
        assert_eq!(
            facts["final_status"]["session_id"].as_str(),
            Some(session),
            "{identity}: the row's session is the one the identity is bound to: {facts}"
        );
        let seen: Vec<&str> = facts["sessions_seen"]
            .as_array()
            .expect("sessions seen")
            .iter()
            .map(|seen| seen.as_str().expect("a session id"))
            .collect();
        assert_eq!(seen.last(), Some(&session), "{identity}: {seen:?}");

        // The committed body itself, not only the manifest's account of it:
        // the one message a materialization writes, at revision 1.
        let committed = bodies
            .get(session)
            .unwrap_or_else(|| panic!("{identity}: no committed body for {session}"));
        let messages = &committed.body["messages"];
        assert_eq!(
            committed.store_revision, 1,
            "{identity}: the session never committed a turn"
        );
        assert_eq!(
            messages.as_array().map(Vec::len),
            Some(1),
            "{identity}: one message"
        );

        // continuity.sqlite3: the record binds the session and the snapshot
        // row holds the same message, but there is no head and no strand row.
        assert_eq!(
            bindings.get(identity).map(String::as_str),
            Some(session),
            "{identity}: the continuity record binds {session}"
        );
        assert_eq!(
            snapshots.get(session),
            Some(messages),
            "{identity}: the snapshot row holds the body's message"
        );
        assert!(
            !heads.contains(session),
            "{identity}: {session} has no continuity head"
        );
        assert!(
            !strands.contains(session),
            "{identity}: {session} has no strand rows"
        );

        // The manifest's account of the same facts.
        for (field, value) in [
            ("store_revision", Value::from(1)),
            ("messages", Value::from(1)),
            ("continuity_head", Value::from(false)),
            ("strand_messages", Value::from(0)),
            ("snapshot_messages", Value::from(1)),
            ("snapshot_equals_body", Value::from(true)),
            ("record_binds", Value::from(true)),
        ] {
            assert_eq!(row[field], value, "{identity}: {field}: {row}");
        }

        if identity == "agent:hl-02" {
            // delete_identity dropped the pre-delete session's continuity
            // copy and kept its runtime body, which nothing is bound to: the
            // boot-1 turn, three messages at revision 3.
            let [previous, _] = seen[..] else {
                panic!("{identity}: two sessions, before and after the delete: {seen:?}");
            };
            assert_eq!(row["previous_session_id"].as_str(), Some(previous), "{row}");
            assert_eq!(row["previous_runtime_only"].as_bool(), Some(true), "{row}");
            let kept = bodies.get(previous).unwrap_or_else(|| {
                panic!("{identity}: runtime.sqlite keeps the pre-delete session {previous}")
            });
            assert_eq!(
                (
                    kept.store_revision,
                    kept.body["messages"].as_array().map(Vec::len)
                ),
                (3, Some(3)),
                "{identity}: the pre-delete session holds the boot-1 turn"
            );
            assert_eq!(row["previous_store_revision"], Value::from(3), "{row}");
            assert_eq!(row["previous_messages"], Value::from(3), "{row}");
            assert!(
                !bindings.values().any(|bound| bound == previous)
                    && !heads.contains(previous)
                    && !strands.contains(previous)
                    && !snapshots.contains_key(previous),
                "{identity}: continuity.sqlite3 holds nothing of {previous}"
            );
        } else {
            assert_eq!(seen, [session], "{identity}: one session");
        }
    }

    // The control: hl-01 took a turn, so its bound session has a head and
    // strand rows.
    let control = manifest["identities"]["agent:hl-01"]["final_status"]["session_id"]
        .as_str()
        .expect("hl-01 session");
    assert_eq!(
        bindings.get("agent:hl-01").map(String::as_str),
        Some(control)
    );
    assert!(
        heads.contains(control) && strands.contains(control),
        "hl-01's session {control} has a head and strand rows"
    );
    let headless: BTreeSet<&str> = bindings
        .values()
        .map(String::as_str)
        .filter(|session| !heads.contains(*session))
        .collect();
    let expected: BTreeSet<&str> = rows
        .iter()
        .map(|row| row["session_id"].as_str().expect("row session"))
        .collect();
    assert_eq!(
        headless, expected,
        "exactly the two rows' sessions are bound without a continuity head"
    );
}

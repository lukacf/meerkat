//! The console experience's identity bindings: an identity-first row's
//! session is the identity runtime's answer, a walk recorded before the
//! identity runtime moved is never served for the moved binding, and a row
//! answered pending past the refresh deadline withholds its walked facts on
//! the wire while access and visibility still decide on them.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use meerkat_client::{LlmClient, LlmError, LlmRequest};
use meerkat_core::{Message, Provider};
use meerkat_mob::MobDefinition;
use serde_json::{Value, json};

use super::{
    CONSOLE_BINDING_REFRESH_DEADLINE, ConsoleSnapshotReadModel, IdentityBinding,
    IdentityBindingCut, MEMBER_STATE_RETIRING, build_aggregator_live_snapshot, build_live_snapshot,
    join_identity_bindings, live_member_modules,
};
use crate::access::{AccessControlConfig, AccessController, AccessEffect, AccessRule};
use crate::console_aggregator::{AllowAllConsoleVisibilityPolicy, MetaConsoleAggregator};
use crate::identity_first::{
    AgentAddressability, AgentIdentity, DurableAgentSpec, IdentityLifecycleState,
    LocalContinuityStore, LocalLeaseProvider, MutableRosterProvider,
};
use crate::runtime::{ConsoleMember, ConsoleModelCapabilities, ConsoleRestJsonRequest};
use crate::unified_runtime::UnifiedRuntime;
use crate::unobserved::{UnobservedFacts, UnobservedReason};

const KEEPER: &str = "voice-keeper";
const OTHER: &str = "other-keeper";

/// The members never take a turn; the client only has to exist.
struct NeverCalledClient;

#[async_trait::async_trait]
impl LlmClient for NeverCalledClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, _request: &'a LlmRequest) -> meerkat_client::types::LlmStream<'a> {
        Box::pin(futures::stream::empty())
    }

    fn provider(&self) -> Provider {
        Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

fn row(roster_id: &str, identity_label: Option<&str>, session: Option<&str>) -> ConsoleMember {
    ConsoleMember {
        agent_identity: roster_id.to_string(),
        role: "worker".to_string(),
        state: "active".to_string(),
        model_capabilities: ConsoleModelCapabilities::default(),
        runtime_mode: None,
        session_id: session.map(str::to_string),
        wired_to: vec![],
        labels: identity_label
            .map(|identity| BTreeMap::from([("agent_identity".to_string(), identity.to_string())]))
            .unwrap_or_default(),
        progress: Some(json!({
            "run_state": "run_open",
            "in_flight_work": 1,
            "last_progress_at_ms": 7,
            "last_progress_event": "became_busy",
            "health": "healthy",
        })),
    }
}

fn cut(bindings: &[(&str, Option<&str>, IdentityLifecycleState)]) -> IdentityBindingCut {
    IdentityBindingCut {
        bindings: bindings
            .iter()
            .map(|(identity, session, state)| {
                (
                    (*identity).to_string(),
                    IdentityBinding {
                        session_id: session.map(str::to_string),
                        state: *state,
                    },
                )
            })
            .collect(),
        ..IdentityBindingCut::default()
    }
}

fn fallback_progress() -> Value {
    json!({
        "run_state": "unknown",
        "in_flight_work": 0,
        "last_progress_at_ms": 0,
        "last_progress_event": "unchanged",
        "health": "unknown",
    })
}

#[tokio::test]
async fn the_join_binds_identity_first_rows_to_the_owner_session_and_leaves_plain_members() {
    // An answering walk joins with bindings equal to its own. The walk read
    // the keeper's binding (s1) but its roster copy names no session.
    let bindings = cut(&[(KEEPER, Some("s1"), IdentityLifecycleState::Active)]);
    let mut members = vec![
        row(KEEPER, Some(KEEPER), None),
        // A plain mob member: the identity runtime never knew it.
        row("worker-1", None, Some("p0")),
        // A row whose label names the keeper but whose roster id is another
        // member is not the keeper's roster row.
        row("imposter", Some(KEEPER), Some("p1")),
    ];
    let untouched = members[1..].to_vec();

    let pending = join_identity_bindings(
        &mut members,
        &bindings,
        &bindings,
        true,
        &BTreeSet::new(),
        None,
    )
    .await;

    assert!(pending.is_empty(), "the walk answers: nothing is pending");
    assert_eq!(
        members[0].session_id.as_deref(),
        Some("s1"),
        "the owner's session, never the walk's copy"
    );
    assert_eq!(
        members[0].progress,
        Some(fallback_progress()),
        "progress the walk observed for no session is never shown beside s1"
    );
    assert_eq!(members[0].state, "active", "state stays the walk's");
    assert_eq!(&members[1..], untouched.as_slice());
}

#[tokio::test]
async fn the_join_names_a_moved_binding_pending_only_when_the_walk_does_not_answer() {
    let walked = cut(&[
        (KEEPER, Some("s0"), IdentityLifecycleState::Active),
        (OTHER, Some("o0"), IdentityLifecycleState::Active),
    ]);

    // A reset: the session moved.
    let mut members = vec![
        row(KEEPER, Some(KEEPER), Some("s0")),
        row(OTHER, Some(OTHER), Some("o0")),
    ];
    let reset = cut(&[
        (KEEPER, Some("s1"), IdentityLifecycleState::Active),
        (OTHER, Some("o0"), IdentityLifecycleState::Active),
    ]);
    let pending =
        join_identity_bindings(&mut members, &walked, &reset, false, &BTreeSet::new(), None).await;
    assert_eq!(pending, BTreeSet::from([KEEPER.to_string()]));
    assert_eq!(members[0].session_id.as_deref(), Some("s1"));
    assert_eq!(members[0].progress, Some(fallback_progress()));
    assert_eq!(
        members[1],
        row(OTHER, Some(OTHER), Some("o0")),
        "an unchanged binding answers from the walk"
    );

    // A retire: the session stays, the lifecycle state moved.
    let mut members = vec![row(KEEPER, Some(KEEPER), Some("s0"))];
    let retire = cut(&[
        (KEEPER, Some("s0"), IdentityLifecycleState::Dormant),
        (OTHER, Some("o0"), IdentityLifecycleState::Active),
    ]);
    let pending = join_identity_bindings(
        &mut members,
        &walked,
        &retire,
        false,
        &BTreeSet::new(),
        None,
    )
    .await;
    assert_eq!(pending, BTreeSet::from([KEEPER.to_string()]));
    assert_eq!(members[0].session_id.as_deref(), Some("s0"));
    assert_eq!(members[0].progress, Some(fallback_progress()));

    // A delete: the identity runtime no longer knows the identity.
    let mut members = vec![row(KEEPER, Some(KEEPER), Some("s0"))];
    let delete = cut(&[(OTHER, Some("o0"), IdentityLifecycleState::Active)]);
    let pending = join_identity_bindings(
        &mut members,
        &walked,
        &delete,
        false,
        &BTreeSet::new(),
        None,
    )
    .await;
    assert_eq!(pending, BTreeSet::from([KEEPER.to_string()]));
    assert_eq!(members[0].session_id, None);

    // The same moves with a walk that answers are joined, never pending.
    for current in [&reset, &retire, &delete] {
        let mut members = vec![row(KEEPER, Some(KEEPER), Some("s0"))];
        let pending =
            join_identity_bindings(&mut members, &walked, current, true, &BTreeSet::new(), None)
                .await;
        assert!(pending.is_empty());
    }
}

/// A cut whose read held positions `started` and `started + 1` on the one
/// read counter.
fn cut_read_at(
    bindings: &[(&str, Option<&str>, IdentityLifecycleState)],
    started: u64,
) -> IdentityBindingCut {
    IdentityBindingCut {
        read_started: started,
        read_completed: started + 1,
        ..cut(bindings)
    }
}

#[test]
fn a_walk_answers_for_the_same_bindings_or_a_cut_read_after_the_request() {
    let successor = [(KEEPER, Some("s1"), IdentityLifecycleState::Active)];
    let predecessor = [(KEEPER, Some("s0"), IdentityLifecycleState::Active)];
    // The request's read held positions 10 and 11.
    let request = cut_read_at(&successor, 10);

    let older_same = cut_read_at(&successor, 2);
    assert!(older_same.answers_for(&request), "the same bindings answer");
    assert!(std::ptr::eq(older_same.newer(&request), &raw const request));

    let older_moved = cut_read_at(&predecessor, 2);
    assert!(
        !older_moved.answers_for(&request),
        "a walk read before the binding moved never answers"
    );
    assert!(std::ptr::eq(
        older_moved.newer(&request),
        &raw const request
    ));

    let newer_moved = cut_read_at(&predecessor, 12);
    assert!(
        newer_moved.answers_for(&request),
        "a walk whose cut was read after the request's answers for it"
    );
    assert!(
        std::ptr::eq(newer_moved.newer(&request), &raw const newer_moved),
        "the request joins with the newer cut"
    );

    let overlapping = cut_read_at(&predecessor, 9);
    assert!(
        !overlapping.answers_for(&request),
        "a cut read while the request was reading is not after it"
    );
}

#[tokio::test(start_paused = true)]
async fn binding_reads_order_exactly_where_the_clock_stands_still() {
    let first = IdentityBindingCut {
        bindings: cut(&[(KEEPER, Some("s0"), IdentityLifecycleState::Active)]).bindings,
        ..IdentityBindingCut::read(None).await
    };
    let second = IdentityBindingCut {
        bindings: cut(&[(KEEPER, Some("s1"), IdentityLifecycleState::Active)]).bindings,
        ..IdentityBindingCut::read(None).await
    };
    assert!(
        second.answers_for(&first),
        "a read that started after the other completed answers for it"
    );
    assert!(
        !first.answers_for(&second),
        "an earlier read never answers for a later one, though the paused clock reads the same"
    );
    assert!(std::ptr::eq(second.newer(&first), &raw const second));
    assert!(std::ptr::eq(first.newer(&second), &raw const second));
}

#[tokio::test]
async fn a_moving_binding_vouches_for_no_walked_fact() {
    // Control: an Active binding with the walked session answers from the walk.
    let active = cut(&[(KEEPER, Some("s1"), IdentityLifecycleState::Active)]);
    let mut members = vec![row(KEEPER, Some(KEEPER), Some("s1"))];
    let pending =
        join_identity_bindings(&mut members, &active, &active, true, &BTreeSet::new(), None).await;
    assert!(pending.is_empty());
    assert_eq!(members[0], row(KEEPER, Some(KEEPER), Some("s1")));

    // A binding that moved while the walk ran (read again after it).
    let moved = BTreeSet::from([KEEPER.to_string()]);
    let mut members = vec![row(KEEPER, Some(KEEPER), Some("s1"))];
    let pending = join_identity_bindings(&mut members, &active, &active, true, &moved, None).await;
    assert_eq!(pending, BTreeSet::from([KEEPER.to_string()]));
}

#[test]
fn a_walk_records_the_bindings_that_moved_while_it_ran() {
    let before = cut(&[
        (KEEPER, Some("s1"), IdentityLifecycleState::Active),
        (OTHER, Some("o0"), IdentityLifecycleState::Active),
    ]);
    assert!(before.moved_in(&before).is_empty(), "nothing moved");
    let reset = cut(&[
        (KEEPER, Some("s2"), IdentityLifecycleState::Active),
        (OTHER, Some("o0"), IdentityLifecycleState::Active),
    ]);
    assert_eq!(
        before.moved_in(&reset),
        BTreeSet::from([KEEPER.to_string()])
    );
    let retired = cut(&[
        (KEEPER, Some("s1"), IdentityLifecycleState::Dormant),
        (OTHER, Some("o0"), IdentityLifecycleState::Active),
    ]);
    assert_eq!(
        before.moved_in(&retired),
        BTreeSet::from([KEEPER.to_string()])
    );
    let deleted = cut(&[(OTHER, Some("o0"), IdentityLifecycleState::Active)]);
    assert_eq!(
        before.moved_in(&deleted),
        BTreeSet::from([KEEPER.to_string()])
    );
}

/// A pending keeper whose walked facts are sentinels, beside a plain peer
/// wired to it.
fn pending_route_snapshot() -> crate::runtime::ConsoleLiveSnapshot {
    let mut keeper = row(KEEPER, Some(KEEPER), Some("s1"));
    keeper.role = "sentinel-role".to_string();
    keeper.wired_to = vec![OTHER.to_string()];
    keeper.model_capabilities = ConsoleModelCapabilities { image_input: true };
    keeper.labels.extend([
        ("tier".to_string(), "restricted-sentinel".to_string()),
        ("display_name".to_string(), "Keeper Sentinel".to_string()),
        ("group".to_string(), "group-sentinel".to_string()),
    ]);
    keeper.progress = Some(fallback_progress());
    let mut other = row(OTHER, Some(OTHER), Some("o0"));
    other.wired_to = vec![KEEPER.to_string()];
    crate::runtime::ConsoleLiveSnapshot::new(
        Some("console-binding".to_string()),
        Some(true),
        vec![],
        vec![],
        vec![keeper, other],
        true,
    )
}

const WALKED_SENTINELS: &[&str] = &[
    "sentinel-role",
    "restricted-sentinel",
    "Keeper Sentinel",
    "group-sentinel",
];

/// Every object in `value` that names `identity` (as identity, agent_id or
/// member_id).
fn records_naming<'a>(value: &'a Value, identity: &str, found: &mut Vec<&'a Value>) {
    match value {
        Value::Object(object) => {
            if ["identity", "agent_id", "member_id"]
                .iter()
                .any(|key| object.get(*key).and_then(Value::as_str) == Some(identity))
            {
                found.push(value);
            }
            for child in object.values() {
                records_naming(child, identity, found);
            }
        }
        Value::Array(items) => {
            for item in items {
                records_naming(item, identity, found);
            }
        }
        _ => {}
    }
}

/// The whole body carries none of the pending keeper's walked facts: no
/// sentinel anywhere, and every record naming it (sidebar row, identity
/// status row, health row, topology node) holds the withheld shape and the
/// marker.
fn assert_pending_keeper_withheld_everywhere(body: &Value) {
    let text = body.to_string();
    for sentinel in WALKED_SENTINELS {
        assert!(
            !text.contains(sentinel),
            "the walked fact {sentinel:?} is served for the pending keeper: {body:#}"
        );
    }
    let mut records = Vec::new();
    records_naming(body, KEEPER, &mut records);
    assert_eq!(
        records.len(),
        4,
        "sidebar, identity status, health and topology each name the keeper: {records:#?}"
    );
    for record in records {
        assert_eq!(
            record["unobserved"]["reason"], "refresh_pending",
            "{record:#}"
        );
        assert_eq!(record["state"], "unknown", "{record:#}");
        for withheld in ["role", "model_capabilities", "display_name", "group"] {
            assert!(
                record.get(withheld).is_none(),
                "{withheld} served: {record:#}"
            );
        }
        if let Some(wired_to) = record.get("wired_to") {
            assert_eq!(wired_to, &json!([]), "{record:#}");
        }
        if let Some(labels) = record.get("labels") {
            assert_eq!(labels, &json!({"agent_identity": KEEPER}), "{record:#}");
        }
        if let Some(label) = record.get("label") {
            assert_eq!(label, KEEPER, "{record:#}");
        }
    }
}

/// The experience body the console route builds for `snapshot`, with
/// `pending` identities answered pending.
fn experience_body(
    snapshot: &crate::runtime::ConsoleLiveSnapshot,
    pending: &BTreeSet<String>,
    access: Option<&AccessController>,
) -> Value {
    let response = crate::runtime::handle_console_rest_json_route_with_pending_rows(
        &open_console_decisions(),
        &crate::ConsoleSessionFences::detached(),
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: "/console/experience".to_string(),
            auth: None,
        },
        Some(snapshot),
        access,
        false,
        false,
        pending,
    );
    assert_eq!(response.status, 200, "{}", response.body);
    response.body
}

/// The record naming `identity` in the array at `pointer`.
fn record_at<'a>(body: &'a Value, pointer: &str, identity: &str) -> &'a Value {
    body.pointer(pointer)
        .and_then(Value::as_array)
        .and_then(|rows| rows.iter().find(|row| row["identity"] == identity))
        .unwrap_or_else(|| panic!("{pointer} names {identity}: {body:#}"))
}

#[test]
fn a_pending_row_withholds_its_walked_facts_in_every_panel_and_fails_affordances_closed() {
    let snapshot = pending_route_snapshot();
    let pending = BTreeSet::from([KEEPER.to_string()]);
    let body = experience_body(&snapshot, &pending, None);
    assert_pending_keeper_withheld_everywhere(&body);

    let keeper = record_at(&body, "/agent_sidebar/live_snapshot/agents", KEEPER);
    assert_eq!(keeper["session_id"], "s1", "the owner's session stays");
    assert_eq!(
        keeper["progress"],
        fallback_progress(),
        "the bounded run state stays"
    );
    assert_eq!(keeper["addressable"], false);
    assert_eq!(keeper["affordances"]["addressable"], false);
    assert_eq!(keeper["affordances"]["can_send_message"], false);
    assert_eq!(keeper["affordances"]["can_retire"], false);
    assert_eq!(
        keeper["affordances"]["can_respawn"], true,
        "respawn resolves its target live"
    );
    assert!(
        keeper["unobserved"]["fields"]
            .as_array()
            .unwrap()
            .contains(&json!("labels"))
    );
    let keeper_status = record_at(&body, "/identity_status/rows", KEEPER);
    assert_eq!(keeper_status["session_id"], "s1");
    assert_eq!(keeper_status["addressability"], "internal_only");
    assert_eq!(
        body["health_overview"]["live_snapshot"]["identities"], body["identity_status"]["rows"],
        "health lists the same rows"
    );
    let node = record_at(&body, "/topology/live_snapshot/nodes", KEEPER);
    assert_eq!(node["addressable"], false);

    // The peer's records stay as walked, and the profile hints count only
    // the rows the walk answers for.
    let answered = experience_body(&snapshot, &BTreeSet::new(), None);
    for pointer in [
        "/agent_sidebar/live_snapshot/agents",
        "/identity_status/rows",
        "/topology/live_snapshot/nodes",
    ] {
        assert_eq!(
            record_at(&body, pointer, OTHER),
            record_at(&answered, pointer, OTHER),
            "{pointer}"
        );
    }
    assert_eq!(
        body["runtime_capabilities"]["profile_capabilities"],
        json!({"worker": {"instance_count": 1, "addressable": true, "has_wiring": true}}),
        "the pending row's walked profile no longer counts"
    );

    // Nothing pending: the body is the public route's, unchanged.
    let public =
        crate::runtime::handle_console_rest_json_route_with_snapshot_access_memory_and_workgraph(
            &open_console_decisions(),
            &ConsoleRestJsonRequest {
                method: "GET".to_string(),
                path: "/console/experience".to_string(),
                auth: None,
            },
            Some(&snapshot),
            None,
            false,
            false,
        );
    assert_eq!(public.body, answered);
}

fn spec(identity: &str, labels: BTreeMap<String, String>) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: AgentIdentity::parse(identity).unwrap(),
        profile: "worker".into(),
        addressability: AgentAddressability::Addressable,
        display_name: None,
        labels,
        context: None,
        additional_instructions: Vec::new(),
        initial_message: None,
        runtime_mode_override: Some(meerkat_mob::MobRuntimeMode::AutonomousHost),
        backend: None,
        binding: None,
        placement: None,
    }
}

async fn identity_first_runtime(
    keeper_labels: BTreeMap<String, String>,
) -> (UnifiedRuntime, tempfile::TempDir) {
    let scratch = tempfile::tempdir().unwrap();
    let definition = MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "console-binding-{}"
[profiles.worker]
model = "gpt-5.5"
runtime_mode = "autonomous_host"
external_addressable = true
[profiles.worker.tools]
comms = true
"#,
        uuid::Uuid::new_v4()
    ))
    .unwrap();
    let roster = Arc::new(MutableRosterProvider::new(vec![
        spec(KEEPER, keeper_labels),
        spec(OTHER, BTreeMap::new()),
    ]));
    let runtime = UnifiedRuntime::builder()
        .definition(definition)
        .continuity_store(Arc::new(LocalContinuityStore::in_memory().unwrap()))
        .lease_provider(Arc::new(LocalLeaseProvider::new()))
        .roster_provider(roster)
        .scratch_dir(scratch.path())
        .ephemeral_runtime_store(true)
        .identity_runtime_instance_id("console-binding")
        .default_llm_client(Arc::new(NeverCalledClient))
        .build()
        .await
        .unwrap();
    (runtime, scratch)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_binding_change_past_the_refresh_deadline_answers_pending_without_predecessor_facts() {
    let (runtime, _scratch) = identity_first_runtime(BTreeMap::from([(
        "group".to_string(),
        "keepers".to_string(),
    )]))
    .await;
    let mob_runtime = runtime.mob_runtime().clone();
    let identity_runtime = runtime.identity_runtime().unwrap();
    let identity_runtime = identity_runtime.runtime();
    let read_model = ConsoleSnapshotReadModel::default();
    let policy = AllowAllConsoleVisibilityPolicy;

    // A primed read model, as a console that was open before the reset.
    let (snapshot, pending) = build_live_snapshot(
        &mob_runtime,
        &[],
        None,
        &policy,
        &read_model,
        Some(identity_runtime),
    )
    .await;
    assert!(pending.is_empty());
    let predecessor = member(&snapshot, KEEPER).unwrap().clone();
    let other_before = member(&snapshot, OTHER).unwrap().clone();

    // Every walk from here on waits for the gate.
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    read_model.set_walk_gate(Some(Arc::clone(&gate)));
    let successor = identity_runtime
        .reset(&AgentIdentity::parse(KEEPER).unwrap())
        .await
        .unwrap();

    let started = tokio::time::Instant::now();
    let (snapshot, pending) = build_live_snapshot(
        &mob_runtime,
        &[],
        None,
        &policy,
        &read_model,
        Some(identity_runtime),
    )
    .await;
    let waited = started.elapsed();
    assert!(
        waited >= CONSOLE_BINDING_REFRESH_DEADLINE
            && waited < CONSOLE_BINDING_REFRESH_DEADLINE + Duration::from_secs(2),
        "the read waits for the held walk until the deadline, then answers: {waited:?}"
    );
    assert_eq!(pending, BTreeSet::from([KEEPER.to_string()]));
    let keeper = member(&snapshot, KEEPER).unwrap();
    assert_eq!(
        keeper.session_id.as_deref(),
        successor.session_id().map(ToString::to_string).as_deref(),
        "the owner's session, never the walk's"
    );
    assert_ne!(keeper.session_id, predecessor.session_id);
    assert_ne!(
        keeper.progress, predecessor.progress,
        "the predecessor's progress is never served for the successor"
    );
    assert_eq!(
        keeper.progress.as_ref().unwrap()["last_progress_event"],
        "unchanged"
    );
    assert_eq!(
        member(&snapshot, OTHER).unwrap(),
        &other_before,
        "an unchanged binding answers from the walk"
    );

    // The held walk lands once released; the next read is the fresh walk.
    gate.add_permits(64);
    let (snapshot, pending) = build_live_snapshot(
        &mob_runtime,
        &[],
        None,
        &policy,
        &read_model,
        Some(identity_runtime),
    )
    .await;
    assert!(pending.is_empty());
    let keeper = member(&snapshot, KEEPER).unwrap();
    assert_eq!(keeper.state, "active");
    assert_eq!(
        keeper.session_id.as_deref(),
        successor.session_id().map(ToString::to_string).as_deref()
    );
    assert_eq!(
        keeper.labels.get("group").map(String::as_str),
        Some("keepers")
    );
    runtime.mob_handle().stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_joins_with_a_walk_cut_read_after_its_own() {
    let (runtime, _scratch) = identity_first_runtime(BTreeMap::new()).await;
    let mob_runtime = runtime.mob_runtime().clone();
    let identity_runtime = runtime.identity_runtime().unwrap();
    let identity_runtime = Arc::clone(identity_runtime.runtime());
    let read_model = ConsoleSnapshotReadModel::default();
    let keeper = AgentIdentity::parse(KEEPER).unwrap();
    build_live_snapshot(
        &mob_runtime,
        &[],
        None,
        &AllowAllConsoleVisibilityPolicy,
        &read_model,
        Some(&identity_runtime),
    )
    .await;

    // Hold the walks and reset (s0 to s1). The deadline is pinned by its
    // own test; here it outlasts any host load, so the read always waits
    // for the walk this test releases.
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    read_model.set_walk_gate(Some(Arc::clone(&gate)));
    read_model.set_refresh_deadline(Duration::from_mins(2));
    identity_runtime.reset(&keeper).await.unwrap();

    // A read takes its cut (s1) and waits for a walk.
    let read = tokio::spawn({
        let mob_runtime = mob_runtime.clone();
        let read_model = read_model.clone();
        let identity_runtime = Arc::clone(&identity_runtime);
        async move {
            build_live_snapshot(
                &mob_runtime,
                &[],
                None,
                &AllowAllConsoleVisibilityPolicy,
                &read_model,
                Some(&identity_runtime),
            )
            .await
        }
    });
    // A walk held at the gate holds a clone of it, and only the read
    // starts one: once it is there, the read has taken its cut.
    let held = tokio::time::Instant::now();
    while Arc::strong_count(&gate) < 3 {
        assert!(
            held.elapsed() < Duration::from_mins(1),
            "the read never started its walk"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // Reset again (s1 to s2), then release the walk: its cut reads s2,
    // after the read's.
    let successor = identity_runtime.reset(&keeper).await.unwrap();
    gate.add_permits(64);

    let (snapshot, pending) = read.await.unwrap();
    assert!(
        pending.is_empty(),
        "the walk read after the request answers it"
    );
    assert_eq!(
        member(&snapshot, KEEPER).unwrap().session_id.as_deref(),
        successor.session_id().map(ToString::to_string).as_deref(),
        "the row joins with the walk's newer cut, never the read's own older one"
    );
    runtime.mob_handle().stop().await.unwrap();
}

/// A binding that moves while a walk runs, after the walk read its cut, is
/// recorded on the walk, and a read the walk answers (its cut equals the
/// read's) answers that identity pending: the walk cannot vouch for facts of
/// a binding that moved under it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_binding_that_moves_while_a_walk_runs_answers_its_row_pending() {
    let (runtime, _scratch) = identity_first_runtime(BTreeMap::new()).await;
    let mob_runtime = runtime.mob_runtime().clone();
    let identity_runtime = runtime.identity_runtime().unwrap();
    let identity_runtime = Arc::clone(identity_runtime.runtime());
    let read_model = ConsoleSnapshotReadModel::default();
    let keeper = AgentIdentity::parse(KEEPER).unwrap();
    build_live_snapshot(
        &mob_runtime,
        &[],
        None,
        &AllowAllConsoleVisibilityPolicy,
        &read_model,
        Some(&identity_runtime),
    )
    .await;

    // Reset (s0 to s1), so the next read needs a fresh walk, and hold every
    // walk after its roster walk, before the cut read that closes it.
    let walked = Arc::new(tokio::sync::Semaphore::new(0));
    read_model.set_walked_gate(Some(Arc::clone(&walked)));
    read_model.set_refresh_deadline(Duration::from_mins(2));
    identity_runtime.reset(&keeper).await.unwrap();

    // A read takes its cut (s1); its walk reads the same cut and walks.
    let read = tokio::spawn({
        let mob_runtime = mob_runtime.clone();
        let read_model = read_model.clone();
        let identity_runtime = Arc::clone(&identity_runtime);
        async move {
            build_live_snapshot(
                &mob_runtime,
                &[],
                None,
                &AllowAllConsoleVisibilityPolicy,
                &read_model,
                Some(&identity_runtime),
            )
            .await
        }
    });
    // A walk held at the gate holds a clone of it.
    let held = tokio::time::Instant::now();
    while Arc::strong_count(&walked) < 3 {
        assert!(
            held.elapsed() < Duration::from_mins(1),
            "the read never walked"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // The binding moves (s1 to s2) after the walk read its cut.
    identity_runtime.reset(&keeper).await.unwrap();
    walked.add_permits(64);

    let (_, pending) = read.await.unwrap();
    assert!(
        read_model
            .inner
            .read()
            .await
            .moved_during_walk
            .contains(KEEPER),
        "the walk records the binding that moved while it ran"
    );
    assert_eq!(
        pending,
        BTreeSet::from([KEEPER.to_string()]),
        "the walk vouches for no fact of a binding that moved under it"
    );
    runtime.mob_handle().stop().await.unwrap();
}

/// Whether the mob runs is its owner's answer when the snapshot is read: a mob
/// that stopped after the cached walk reads stopped, never the walk's running.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_live_snapshot_answers_the_mob_owners_running_state_not_the_cached_walks() {
    let (runtime, _scratch) = identity_first_runtime(BTreeMap::new()).await;
    let mob_runtime = runtime.mob_runtime().clone();
    let read_model = ConsoleSnapshotReadModel::default();
    let policy = AllowAllConsoleVisibilityPolicy;

    // A primed read model while the mob runs.
    let (snapshot, _) =
        build_live_snapshot(&mob_runtime, &[], None, &policy, &read_model, None).await;
    assert_eq!(snapshot.running, Some(true));
    assert_eq!(snapshot.unobserved, None);

    // Every later walk waits for the gate, so only the owner can know the
    // mob stopped.
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    read_model.set_walk_gate(Some(Arc::clone(&gate)));
    runtime.mob_handle().stop().await.unwrap();

    let (snapshot, _) =
        build_live_snapshot(&mob_runtime, &[], None, &policy, &read_model, None).await;
    assert_eq!(
        snapshot.running,
        Some(false),
        "the mob stopped after the walk; the owner says so, the cached walk does not"
    );
    gate.add_permits(64);
}

/// A console aggregator with no registered runtime has no owner to answer
/// whether a mob runs: running is null and named unobserved.
#[tokio::test]
async fn an_aggregator_without_a_registered_runtime_reports_running_unobserved() {
    let aggregator = MetaConsoleAggregator::in_memory();
    let snapshot = build_aggregator_live_snapshot(&aggregator, &[])
        .await
        .unwrap();
    assert_eq!(snapshot.running, None);
    assert_eq!(
        snapshot.unobserved,
        Some(UnobservedFacts::new(
            UnobservedReason::NotObserved,
            &["running"]
        ))
    );
}

/// An aggregator's running is its registered runtimes' owners' answer: true
/// while every registered mob runs, false once any one of them stops.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_aggregator_runs_only_while_every_registered_runtime_runs() {
    let (first, _first_scratch) = identity_first_runtime(BTreeMap::new()).await;
    let (second, _second_scratch) = identity_first_runtime(BTreeMap::new()).await;
    let aggregator = MetaConsoleAggregator::in_memory();
    for (key, runtime) in [("first", &first), ("second", &second)] {
        aggregator.register_runtime_handles_with_policy(
            key,
            key,
            runtime.mob_runtime().clone(),
            None,
            runtime.console_events(),
            Arc::new(AllowAllConsoleVisibilityPolicy),
        );
    }
    let snapshot = build_aggregator_live_snapshot(&aggregator, &[])
        .await
        .unwrap();
    assert_eq!(snapshot.running, Some(true));
    assert_eq!(snapshot.unobserved, None);

    second.mob_handle().stop().await.unwrap();
    let snapshot = build_aggregator_live_snapshot(&aggregator, &[])
        .await
        .unwrap();
    assert_eq!(
        snapshot.running,
        Some(false),
        "one registered runtime stopped, so the aggregate does not run"
    );
    first.mob_handle().stop().await.unwrap();
}

/// A pending identity's walked state does not decide whether it is a loaded
/// module: walked active or walked retiring, it is left out alike. Settled
/// rows keep the rule that a retiring member is not loaded.
#[test]
fn a_pending_identity_is_left_out_of_the_live_modules_whatever_its_walked_state() {
    let with_keeper = |state: &str| {
        let mut keeper = row(KEEPER, Some(KEEPER), Some("s1"));
        keeper.state = state.to_string();
        let mut leaving = row("leaving-keeper", Some("leaving-keeper"), Some("p1"));
        leaving.state = MEMBER_STATE_RETIRING.to_string();
        vec![keeper, row(OTHER, Some(OTHER), Some("o1")), leaving]
    };
    let pending = BTreeSet::from([KEEPER.to_string()]);
    for walked_state in ["active", MEMBER_STATE_RETIRING] {
        assert_eq!(
            live_member_modules(&with_keeper(walked_state), &pending),
            vec![OTHER.to_string()],
            "walked {walked_state}"
        );
    }
    assert_eq!(
        live_member_modules(&with_keeper("active"), &BTreeSet::new()),
        vec![OTHER.to_string(), KEEPER.to_string()],
        "a settled keeper is loaded; a settled retiring member is not"
    );
}

fn member<'a>(
    snapshot: &'a crate::runtime::ConsoleLiveSnapshot,
    roster_id: &str,
) -> Option<&'a ConsoleMember> {
    snapshot
        .members
        .iter()
        .find(|member| member.agent_identity == roster_id)
}

fn open_console_decisions() -> crate::RuntimeDecisionState {
    crate::build_runtime_decision_state(crate::RuntimeDecisionInputs {
        bigquery: crate::BigQueryNaming {
            dataset: "console_binding".to_string(),
            table: "events".to_string(),
        },
        trusted_meta_toml: "modules = []\n".to_string(),
        auth: crate::AuthPolicy {
            default_provider: crate::AuthProvider::GoogleOAuth,
            email_allowlist: vec![],
        },
        trusted_oidc: crate::TrustedOidcRuntimeConfig {
            discovery_json:
                r#"{"issuer":"https://trusted.meta.local","jwks_uri":"https://trusted.meta.local/.well-known/jwks.json"}"#
                    .to_string(),
            jwks_json: r#"{"keys":[{"kid":"kid-current","kty":"oct","alg":"HS256","k":"cGhhc2U3LXRydXN0ZWQtY3VycmVudC1zZWNyZXQ"}]}"#
                .to_string(),
            audience: "meerkat-console".to_string(),
            require_verified_email: false,
        },
        console: crate::ConsolePolicy {
            require_app_auth: false,
            ..crate::ConsolePolicy::default()
        },
        ops: crate::RuntimeOpsPolicy::default(),
        release_metadata_json: include_str!("../assets/release-targets.json").to_string(),
    })
    .expect("decision state builds")
}

fn experience_identities(
    snapshot: &crate::runtime::ConsoleLiveSnapshot,
    pending: &BTreeSet<String>,
    controller: &AccessController,
) -> (Vec<String>, Vec<String>) {
    let body = experience_body(snapshot, pending, Some(controller));
    let names = |pointer: &str| {
        body.pointer(pointer)
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| row["identity"].as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    (
        names("/agent_sidebar/live_snapshot/agents"),
        names("/identity_status/rows"),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_label_scoped_deny_still_hides_a_member_answered_pending() {
    let (runtime, _scratch) = identity_first_runtime(BTreeMap::from([(
        "tier".to_string(),
        "restricted".to_string(),
    )]))
    .await;
    let mob_runtime = runtime.mob_runtime().clone();
    let identity_runtime = runtime.identity_runtime().unwrap();
    let identity_runtime = identity_runtime.runtime();
    let read_model = ConsoleSnapshotReadModel::default();
    let policy = AllowAllConsoleVisibilityPolicy;
    let controller = AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec!["root@example.test".to_string()],
        rules: vec![
            AccessRule {
                id: "view-all".to_string(),
                actions: vec!["agent.view".to_string()],
                agents: vec!["*".to_string()],
                ..AccessRule::default()
            },
            AccessRule {
                id: "deny-restricted".to_string(),
                effect: AccessEffect::Deny,
                actions: vec!["agent.view".to_string()],
                match_labels: BTreeMap::from([("tier".to_string(), "restricted".to_string())]),
                ..AccessRule::default()
            },
        ],
        ..AccessControlConfig::default()
    })
    .expect("access controller");

    let (snapshot, pending) = build_live_snapshot(
        &mob_runtime,
        &[],
        None,
        &policy,
        &read_model,
        Some(identity_runtime),
    )
    .await;
    assert!(pending.is_empty());
    assert_eq!(
        experience_identities(&snapshot, &pending, &controller),
        (vec![OTHER.to_string()], vec![OTHER.to_string()]),
        "the deny hides the restricted keeper before the reset"
    );

    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    read_model.set_walk_gate(Some(Arc::clone(&gate)));
    identity_runtime
        .reset(&AgentIdentity::parse(KEEPER).unwrap())
        .await
        .unwrap();
    let (snapshot, pending) = build_live_snapshot(
        &mob_runtime,
        &[],
        None,
        &policy,
        &read_model,
        Some(identity_runtime),
    )
    .await;
    assert_eq!(pending, BTreeSet::from([KEEPER.to_string()]));
    assert_eq!(
        experience_identities(&snapshot, &pending, &controller),
        (vec![OTHER.to_string()], vec![OTHER.to_string()]),
        "access decides on the last observed labels while the row is pending"
    );
    gate.add_permits(64);
    runtime.mob_handle().stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_join_binds_the_runtimes_own_identities_under_a_namespaced_aggregator() {
    let (runtime, _scratch) = identity_first_runtime(BTreeMap::new()).await;
    let mob_runtime = runtime.mob_runtime().clone();
    let identity_runtime = runtime.identity_runtime().unwrap();
    let identity_runtime = identity_runtime.runtime();
    let aggregator = MetaConsoleAggregator::in_memory();
    aggregator.register_runtime_handles_with_policy(
        "default",
        "ns",
        mob_runtime.clone(),
        Some(Arc::clone(identity_runtime)),
        runtime.console_events(),
        Arc::new(AllowAllConsoleVisibilityPolicy),
    );
    let read_model = ConsoleSnapshotReadModel::default();
    let policy = AllowAllConsoleVisibilityPolicy;
    build_live_snapshot(
        &mob_runtime,
        &[],
        None,
        &policy,
        &read_model,
        Some(identity_runtime),
    )
    .await;

    // Every walk from here on waits for the gate, so the read below answers
    // from the predecessor's walk and only the join can name the successor.
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    read_model.set_walk_gate(Some(Arc::clone(&gate)));
    let successor = identity_runtime
        .reset(&AgentIdentity::parse(KEEPER).unwrap())
        .await
        .unwrap();
    let records = aggregator.list_identities().await.unwrap();
    let record = records
        .iter()
        .find(|record| record.identity == format!("ns/{KEEPER}"))
        .unwrap_or_else(|| panic!("the aggregator lists the namespaced record: {records:?}"));
    assert_eq!(
        record.session_id.as_deref(),
        successor.session_id().map(ToString::to_string).as_deref()
    );
    let (snapshot, pending) = build_live_snapshot(
        &mob_runtime,
        &[],
        None,
        &policy,
        &read_model,
        Some(identity_runtime),
    )
    .await;
    assert_eq!(pending, BTreeSet::from([KEEPER.to_string()]));
    assert_eq!(
        member(&snapshot, KEEPER).unwrap().session_id.as_deref(),
        successor.session_id().map(ToString::to_string).as_deref(),
        "the experience binds its bare row to the same session"
    );
    gate.add_permits(64);
    runtime.mob_handle().stop().await.unwrap();
}

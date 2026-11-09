//! Rows for escalations, persisted suspend markers, member reads, the status
//! object and the capability seam: the source health a host and its agents
//! can act on.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::Ordering;

use serde_json::{Value, json};

use super::owner::{AnnouncementConfig, AnnouncementService};
use super::store::AnnouncementStore;
use super::tests::{
    Harness, OWN_MOB, SOURCES_ADVANCE, SOURCES_GET, SOURCES_LIST, SOURCES_PAUSE, SOURCES_PUT,
    SOURCES_RECORD_FAILURE, SOURCES_RESUME, SUBSCRIPTIONS_PUT, ScriptedNative, advance_params,
    announce_request, batch_id, harness_with, harness_with_config, identity_first_test_builder,
    identity_first_test_runtime, member, op, owed, peer_member, policy, principal_of, put, put_for,
    source_params, subscribe_to_inbox, try_host_call, upsert, with, with_items,
};
use super::types::StartFrontier;
use super::{NativeDelivery, SuspendCause};

const MINUTE: u64 = 60_000;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;

const ESCALATIONS_LIST: &str = "meta/mob_announcements/escalations/list";
const MEMBERS_GET: &str = "meta/mob_announcements/members/get";
const MEMBERS_LIST: &str = "meta/mob_announcements/members/list";

/// Source "inbox" as its collector reads it: declared with `policy` as its
/// health policy (or the default when `None`) and live after its baseline,
/// at revision 1, version 1, at the harness clock.
async fn inbox(harness: &Harness, policy: Option<Value>) {
    let mut params = source_params("put-inbox", "inbox", "subject-1", &["mail.received"]);
    if let Some(policy) = policy {
        params = with(params, "health_policy", policy);
    }
    try_host_call(&harness.service, SOURCES_PUT, params)
        .await
        .unwrap();
    let baseline = with(
        advance_params("baseline", 1, 0, "c1"),
        "coverage",
        json!("complete"),
    );
    try_host_call(&harness.service, SOURCES_ADVANCE, baseline)
        .await
        .unwrap();
}

/// The collector of "inbox" at `version`: one successful advance at `at_ms`,
/// observing `observed` and carrying `items`. Returns the next version.
async fn advance(
    harness: &Harness,
    version: u64,
    at_ms: u64,
    observed: Option<Value>,
    items: &[Value],
) -> u64 {
    harness.clock.store(at_ms, Ordering::SeqCst);
    let mut params = advance_params(
        &format!("advance-{version}"),
        1,
        version,
        &format!("c{}", version + 1),
    );
    if let Some(observed) = observed {
        params = with(params, "observed", observed);
    }
    if !items.is_empty() {
        params = with_items(params, items);
    }
    try_host_call(&harness.service, SOURCES_ADVANCE, params)
        .await
        .unwrap();
    version + 1
}

/// One failed collection of "inbox" at `version`, at `at_ms`.
async fn fail(harness: &Harness, version: u64, at_ms: u64, attempt: u32) {
    harness.clock.store(at_ms, Ordering::SeqCst);
    try_host_call(
        &harness.service,
        SOURCES_RECORD_FAILURE,
        json!({
            "operation_id": format!("failure-{version}-{attempt}"),
            "source_id": "inbox",
            "expected_revision": 1,
            "expected_version": version,
            "principal": principal_of("subject-1"),
            "failure": { "class": "provider_unavailable" },
        }),
    )
    .await
    .unwrap();
}

async fn round_at(harness: &Harness, at_ms: u64) {
    harness.clock.store(at_ms, Ordering::SeqCst);
    harness.service.run_round().await.unwrap();
}

/// The open escalations, as `escalations/list` reads them.
async fn open_escalations(harness: &Harness) -> Vec<Value> {
    let listed = try_host_call(&harness.service, ESCALATIONS_LIST, json!({ "open": true }))
        .await
        .unwrap();
    listed["escalations"].as_array().unwrap().clone()
}

/// The open escalations of `cause`.
async fn open_of(harness: &Harness, cause: &str) -> Vec<Value> {
    open_escalations(harness)
        .await
        .into_iter()
        .filter(|escalation| escalation["cause"] == json!(cause))
        .collect()
}

/// The escalation events after `cursor`, and the cursor to read on from.
async fn events_after(harness: &Harness, cursor: u64) -> (Vec<Value>, u64) {
    let listed = try_host_call(
        &harness.service,
        ESCALATIONS_LIST,
        json!({ "after": cursor }),
    )
    .await
    .unwrap();
    (
        listed["events"].as_array().unwrap().clone(),
        listed["next"].as_u64().unwrap(),
    )
}

fn inbox_subject() -> Value {
    json!({ "kind": "source", "mob_id": OWN_MOB, "source_id": "inbox" })
}

async fn health(harness: &Harness) -> Value {
    try_host_call(
        &harness.service,
        SOURCES_GET,
        json!({ "source_id": "inbox" }),
    )
    .await
    .unwrap()["source"]["health"]
        .clone()
}

/// A live source whose advances keep succeeding but carry no item new to it
/// for its quiet window escalates "quiet", dated from the start of the
/// window, and names the evidence class of the window's latest advance.
#[tokio::test]
async fn a_source_succeeding_empty_past_its_quiet_window_escalates() {
    let harness = harness_with(ScriptedNative::new());
    inbox(&harness, Some(json!({ "quiet_after_ms": HOUR }))).await;
    let delta = json!({ "kind": "delta" });
    let mut version = 1;
    for step in 1..=6 {
        version = advance(
            &harness,
            version,
            1_000 + step * 10 * MINUTE,
            Some(delta.clone()),
            &[],
        )
        .await;
    }
    round_at(&harness, 1_000 + 59 * MINUTE).await;
    assert_eq!(
        open_of(&harness, "quiet").await,
        Vec::<Value>::new(),
        "inside the window"
    );
    version = advance(&harness, version, 1_000 + HOUR + MINUTE, Some(delta), &[]).await;
    let _ = version;
    round_at(&harness, 1_000 + HOUR + MINUTE).await;
    let quiet = open_of(&harness, "quiet").await;
    assert_eq!(quiet.len(), 1, "{quiet:?}");
    assert_eq!(quiet[0]["subject"], inbox_subject());
    assert_eq!(quiet[0]["since_ms"], json!(1_000));
    assert_eq!(quiet[0]["evidence"], json!("delta_confirmed"));
    assert_eq!(quiet[0]["renotify_count"], json!(0));
}

/// The first advance that carries an item new to the source resolves its
/// quiet escalation, under the same id.
#[tokio::test]
async fn the_first_new_item_resolves_a_quiet_escalation() {
    let harness = harness_with(ScriptedNative::new());
    inbox(&harness, Some(json!({ "quiet_after_ms": HOUR }))).await;
    let mut version = advance(
        &harness,
        1,
        1_000 + HOUR + MINUTE,
        Some(json!({ "kind": "delta" })),
        &[],
    )
    .await;
    round_at(&harness, 1_000 + HOUR + MINUTE).await;
    let quiet = open_of(&harness, "quiet").await;
    assert_eq!(quiet.len(), 1, "{quiet:?}");
    let id = quiet[0]["escalation_id"].clone();
    version = advance(
        &harness,
        version,
        1_000 + HOUR + 2 * MINUTE,
        Some(json!({ "kind": "delta" })),
        &[upsert("m-1", "v1", "a new message")],
    )
    .await;
    let _ = version;
    round_at(&harness, 1_000 + HOUR + 2 * MINUTE).await;
    assert_eq!(open_of(&harness, "quiet").await, Vec::<Value>::new());
    let (events, _) = events_after(&harness, 0).await;
    let resolved: Vec<&Value> = events
        .iter()
        .filter(|event| event["event"] == json!("resolved"))
        .collect();
    assert_eq!(resolved.len(), 1, "{events:?}");
    assert_eq!(resolved[0]["escalation_id"], id);
    assert_eq!(resolved[0]["cause"], json!("quiet"));
}

/// Replays never count as new: a collector that keeps sending the page it
/// already sent still goes quiet, dated from the last item that was new.
#[tokio::test]
async fn a_collector_replaying_the_same_page_still_escalates_quiet() {
    let harness = harness_with(ScriptedNative::new());
    inbox(&harness, Some(json!({ "quiet_after_ms": HOUR }))).await;
    let page = [upsert("m-1", "v1", "the only message")];
    let first = 1_000 + MINUTE;
    let mut version = advance(&harness, 1, first, None, &page).await;
    for step in 1..=7 {
        version = advance(&harness, version, first + step * 10 * MINUTE, None, &page).await;
    }
    round_at(&harness, first + 70 * MINUTE).await;
    let quiet = open_of(&harness, "quiet").await;
    assert_eq!(quiet.len(), 1, "{quiet:?}");
    assert_eq!(quiet[0]["since_ms"], json!(first));
    assert_eq!(quiet[0]["evidence"], json!("unreported"));
}

/// Without a quiet policy the source reads "quiet detection not configured"
/// and never escalates quiet, however long it stays empty.
#[tokio::test]
async fn a_source_without_quiet_policy_reports_quiet_detection_not_configured() {
    let harness = harness_with(ScriptedNative::new());
    inbox(&harness, None).await;
    let monitoring = health(&harness).await["quiet_monitoring"].clone();
    assert_eq!(
        monitoring,
        json!({ "configured": false, "quiet_after_ms": null, "last_new_item_at_ms": null })
    );
    let mut version = 1;
    for step in 1..=6 {
        version = advance(
            &harness,
            version,
            1_000 + step * 8 * HOUR,
            Some(json!({ "kind": "delta" })),
            &[],
        )
        .await;
        round_at(&harness, 1_000 + step * 8 * HOUR).await;
    }
    assert_eq!(open_of(&harness, "quiet").await, Vec::<Value>::new());
}

/// Without an empty coverage policy the source reads "empty coverage
/// detection not configured" and never escalates it.
#[tokio::test]
async fn a_source_without_empty_policy_reports_empty_coverage_detection_not_configured() {
    let harness = harness_with(ScriptedNative::new());
    inbox(&harness, None).await;
    let empty = json!({ "kind": "snapshot", "records": 0 });
    let mut version = 1;
    for step in 1..=6 {
        version = advance(
            &harness,
            version,
            1_000 + step * 8 * HOUR,
            Some(empty.clone()),
            &[],
        )
        .await;
        round_at(&harness, 1_000 + step * 8 * HOUR).await;
    }
    let monitoring = health(&harness).await["empty_coverage_monitoring"].clone();
    assert_eq!(
        monitoring,
        json!({
            "configured": false,
            "empty_after_ms": null,
            "empty_snapshot_since_ms": 1_000 + 8 * HOUR,
        })
    );
    assert_eq!(
        open_of(&harness, "empty_coverage").await,
        Vec::<Value>::new()
    );
}

/// A spell of empty snapshots escalates "empty_coverage" after its window,
/// inside a longer quiet window that has not passed.
#[tokio::test]
async fn an_empty_snapshot_spell_escalates_inside_the_quiet_window() {
    let harness = harness_with(ScriptedNative::new());
    inbox(
        &harness,
        Some(json!({ "quiet_after_ms": 4 * HOUR, "empty_after_ms": HOUR })),
    )
    .await;
    let empty = json!({ "kind": "snapshot", "records": 0 });
    let first = 1_000 + 10 * MINUTE;
    let mut version = advance(&harness, 1, first, Some(empty.clone()), &[]).await;
    for step in 1..=7 {
        version = advance(
            &harness,
            version,
            first + step * 10 * MINUTE,
            Some(empty.clone()),
            &[],
        )
        .await;
    }
    round_at(&harness, first + 70 * MINUTE).await;
    let empty_coverage = open_of(&harness, "empty_coverage").await;
    assert_eq!(empty_coverage.len(), 1, "{empty_coverage:?}");
    assert_eq!(empty_coverage[0]["since_ms"], json!(first));
    assert_eq!(empty_coverage[0]["subject"], inbox_subject());
    assert_eq!(open_of(&harness, "quiet").await, Vec::<Value>::new());
    let monitoring = health(&harness).await["empty_coverage_monitoring"].clone();
    assert_eq!(
        monitoring,
        json!({ "configured": true, "empty_after_ms": HOUR, "empty_snapshot_since_ms": first })
    );
}

/// A delta that confirms nothing changed is a real "no change": it never
/// raises empty coverage, however long it lasts.
#[tokio::test]
async fn a_confirmed_empty_delta_never_raises_empty_coverage() {
    let harness = harness_with(ScriptedNative::new());
    inbox(&harness, Some(json!({ "empty_after_ms": HOUR }))).await;
    let mut version = 1;
    for step in 1..=12 {
        version = advance(
            &harness,
            version,
            1_000 + step * 20 * MINUTE,
            Some(json!({ "kind": "delta" })),
            &[],
        )
        .await;
        round_at(&harness, 1_000 + step * 20 * MINUTE).await;
    }
    assert_eq!(
        open_of(&harness, "empty_coverage").await,
        Vec::<Value>::new()
    );
}

/// Three failed collections escalate "failure_streak" in the next round,
/// and the next success resolves it under the same id.
#[tokio::test]
async fn a_failure_streak_escalates_within_its_bound_and_resolves_on_success() {
    let harness = harness_with(ScriptedNative::new());
    inbox(&harness, None).await;
    fail(&harness, 1, 1_000 + MINUTE, 1).await;
    fail(&harness, 1, 1_000 + 2 * MINUTE, 2).await;
    round_at(&harness, 1_000 + 2 * MINUTE).await;
    assert_eq!(
        open_of(&harness, "failure_streak").await,
        Vec::<Value>::new(),
        "two failures"
    );
    fail(&harness, 1, 1_000 + 3 * MINUTE, 3).await;
    round_at(&harness, 1_000 + 3 * MINUTE).await;
    let streak = open_of(&harness, "failure_streak").await;
    assert_eq!(streak.len(), 1, "{streak:?}");
    assert_eq!(streak[0]["subject"], inbox_subject());
    assert_eq!(streak[0]["since_ms"], json!(1_000));
    let id = streak[0]["escalation_id"].clone();
    advance(&harness, 1, 1_000 + 4 * MINUTE, None, &[]).await;
    round_at(&harness, 1_000 + 4 * MINUTE).await;
    assert_eq!(
        open_of(&harness, "failure_streak").await,
        Vec::<Value>::new()
    );
    let (events, _) = events_after(&harness, 0).await;
    let kinds: Vec<(Value, Value)> = events
        .iter()
        .filter(|event| event["escalation_id"] == id)
        .map(|event| (event["event"].clone(), event["cause"].clone()))
        .collect();
    assert_eq!(
        kinds,
        vec![
            (json!("raised"), json!("failure_streak")),
            (json!("resolved"), json!("failure_streak")),
        ]
    );
}

/// An active source with no advance for a day escalates "stale": its
/// collector is not running.
#[tokio::test]
async fn a_stale_source_escalates_when_no_collector_runs() {
    let harness = harness_with(ScriptedNative::new());
    inbox(&harness, None).await;
    round_at(&harness, 1_000 + DAY - MINUTE).await;
    assert_eq!(open_of(&harness, "stale").await, Vec::<Value>::new());
    round_at(&harness, 1_000 + DAY + MINUTE).await;
    let stale = open_of(&harness, "stale").await;
    assert_eq!(stale.len(), 1, "{stale:?}");
    assert_eq!(stale[0]["subject"], inbox_subject());
    assert_eq!(stale[0]["since_ms"], json!(1_000));
}

/// A member's suspend marker is persisted: after a restart the same wait
/// keeps the time it was first seen, and members/get reads it.
#[tokio::test]
async fn persisted_suspend_markers_keep_their_since_across_a_restart() {
    let harness = harness_with(ScriptedNative::new());
    harness.subscribe("s1").await;
    harness.announce_burst(2).await;
    harness
        .native
        .waiting
        .lock()
        .unwrap()
        .insert("agent:a".into(), SuspendCause::MemberUnknown);
    round_at(&harness, 10_000_000).await;
    let first = harness
        .service
        .suspend_marker(&member("agent:a"))
        .expect("the wait is marked");
    assert_eq!(first.since_ms, 10_000_000);

    // A restart: a new owner over the same store and native.
    let clock = Arc::clone(&harness.clock);
    clock.store(20_000_000, Ordering::SeqCst);
    let restarted = AnnouncementService::with_clock(
        Arc::clone(&harness.store) as Arc<dyn AnnouncementStore>,
        Arc::clone(&harness.native) as Arc<dyn NativeDelivery>,
        harness.service.config().clone(),
        Arc::new(move || clock.load(Ordering::SeqCst)),
    )
    .unwrap();
    let read = try_host_call(
        &restarted,
        MEMBERS_GET,
        json!({ "member": { "mob_id": OWN_MOB, "identity": "agent:a" } }),
    )
    .await
    .unwrap();
    assert_eq!(read["marker"]["since_ms"], json!(10_000_000), "{read}");
    restarted.run_round().await.unwrap();
    let marker = restarted
        .suspend_marker(&member("agent:a"))
        .expect("the same wait is marked again");
    assert_eq!(marker.since_ms, 10_000_000, "since survives the restart");
}

/// An open escalation is notified again every day, under the same id with a
/// higher count, until it resolves; then it is notified no more.
#[tokio::test]
async fn an_unresolved_escalation_renotifies_every_day_until_it_resolves() {
    let harness = harness_with(ScriptedNative::new());
    inbox(&harness, None).await;
    for attempt in 1..=3 {
        fail(&harness, 1, 1_000 + u64::from(attempt) * MINUTE, attempt).await;
    }
    round_at(&harness, 1_000 + 3 * MINUTE).await;
    let id = open_of(&harness, "failure_streak").await[0]["escalation_id"].clone();
    round_at(&harness, 1_000 + 3 * MINUTE + DAY + MINUTE).await;
    round_at(&harness, 1_000 + 3 * MINUTE + 2 * DAY + 2 * MINUTE).await;
    let open = open_of(&harness, "failure_streak").await;
    assert_eq!(open[0]["escalation_id"], id);
    assert_eq!(open[0]["renotify_count"], json!(2));
    advance(
        &harness,
        1,
        1_000 + 3 * MINUTE + 2 * DAY + 3 * MINUTE,
        None,
        &[],
    )
    .await;
    round_at(&harness, 1_000 + 3 * MINUTE + 2 * DAY + 3 * MINUTE).await;
    round_at(&harness, 1_000 + 3 * MINUTE + 4 * DAY).await;
    let (events, _) = events_after(&harness, 0).await;
    let of_id: Vec<(Value, Value)> = events
        .iter()
        .filter(|event| event["escalation_id"] == id)
        .map(|event| (event["event"].clone(), event["renotify_count"].clone()))
        .collect();
    assert_eq!(
        of_id,
        vec![
            (json!("raised"), json!(0)),
            (json!("renotified"), json!(1)),
            (json!("renotified"), json!(2)),
            (json!("resolved"), json!(2)),
        ]
    );
}

/// A host that reconnects reads every escalation event after its last
/// cursor, and nothing it read before; a host away longer reads the open
/// set.
#[tokio::test]
async fn escalation_events_are_read_back_from_a_cursor_after_a_reconnect() {
    let harness = harness_with(ScriptedNative::new());
    inbox(&harness, None).await;
    for attempt in 1..=3 {
        fail(&harness, 1, 1_000 + u64::from(attempt) * MINUTE, attempt).await;
    }
    round_at(&harness, 1_000 + 3 * MINUTE).await;
    let (first, cursor) = events_after(&harness, 0).await;
    assert_eq!(first.len(), 1, "{first:?}");
    assert_eq!(first[0]["event"], json!("raised"));
    assert_eq!(first[0]["cursor"].as_u64(), Some(cursor));
    assert_eq!(open_escalations(&harness).await.len(), 1);

    // The host is away while the escalation resolves.
    advance(&harness, 1, 1_000 + 4 * MINUTE, None, &[]).await;
    round_at(&harness, 1_000 + 4 * MINUTE).await;
    let (missed, next) = events_after(&harness, cursor).await;
    assert_eq!(missed.len(), 1, "{missed:?}");
    assert_eq!(missed[0]["event"], json!("resolved"));
    assert_eq!(missed[0]["escalation_id"], first[0]["escalation_id"]);
    assert!(next > cursor);
    let (none, same) = events_after(&harness, next).await;
    assert_eq!(none, Vec::<Value>::new());
    assert_eq!(same, next);
    assert_eq!(open_escalations(&harness).await, Vec::<Value>::new());
}

/// members/get reads a member's backlog, its persisted marker and its
/// escalations; a marker older than an hour escalates "suspended", and
/// members/list finds the member, also among the escalated ones.
#[tokio::test]
async fn members_get_and_list_read_backlog_marker_and_escalations() {
    let harness = harness_with(ScriptedNative::new());
    harness.subscribe("s1").await;
    harness.announce_burst(3).await;
    harness
        .native
        .waiting
        .lock()
        .unwrap()
        .insert("agent:a".into(), SuspendCause::MemberUnknown);
    round_at(&harness, 10_000_000).await;
    let wire_member = json!({ "mob_id": OWN_MOB, "identity": "agent:a" });
    let read = try_host_call(
        &harness.service,
        MEMBERS_GET,
        json!({ "member": wire_member }),
    )
    .await
    .unwrap();
    assert_eq!(read["member"], wire_member);
    assert_eq!(
        read["marker"]["cause"]["kind"],
        json!("member_unknown"),
        "{read}"
    );
    assert_eq!(read["marker"]["since_ms"], json!(10_000_000));
    assert!(read["backlog"].is_object(), "{read}");
    assert_eq!(read["escalations"], json!([]));

    round_at(&harness, 10_000_000 + HOUR + MINUTE).await;
    let read = try_host_call(
        &harness.service,
        MEMBERS_GET,
        json!({ "member": wire_member }),
    )
    .await
    .unwrap();
    let escalations = read["escalations"].as_array().unwrap();
    assert_eq!(escalations.len(), 1, "{read}");
    assert_eq!(escalations[0]["cause"], json!("suspended"));
    assert_eq!(
        escalations[0]["subject"],
        json!({ "kind": "member", "member": wire_member })
    );
    assert_eq!(escalations[0]["since_ms"], json!(10_000_000));
    for params in [json!({}), json!({ "escalated": true })] {
        let listed = try_host_call(&harness.service, MEMBERS_LIST, params.clone())
            .await
            .unwrap();
        let members: Vec<Value> = listed["members"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["member"].clone())
            .collect();
        assert_eq!(members, vec![wire_member.clone()], "{params}: {listed}");
    }
}

/// meta/status's mob announcement object counts sources by state, members
/// suspended, and open escalations with the last event cursor.
#[tokio::test]
async fn the_status_object_counts_sources_members_and_escalations() {
    let harness = harness_with(ScriptedNative::new());
    inbox(&harness, None).await;
    for attempt in 1..=3 {
        fail(&harness, 1, 1_000 + u64::from(attempt) * MINUTE, attempt).await;
    }
    round_at(&harness, 1_000 + 3 * MINUTE).await;
    let (_, cursor) = events_after(&harness, 0).await;
    let status = super::rpc::status_projection(&harness.service).await;
    assert_eq!(
        status,
        json!({
            "sources": { "active": 0, "paused": 0, "failing": 1, "stale": 0 },
            "members": { "suspended": 0, "unhandled_held": 0 },
            "handling": { "pending": 0, "handled": 0, "handled_unverified": 0, "unhandled": 0 },
            "escalations": { "open": 1, "last_event_cursor": cursor },
        })
    );
}

/// meta/capabilities names source support and reads local delivery from the
/// same fact the subscription check reads.
#[tokio::test]
async fn the_capability_seam_names_sources_v1_and_local_delivery() {
    assert!(
        super::rpc::announcement_feature_capabilities()
            .contains(&super::rpc::SOURCES_FEATURE_CAPABILITY)
    );
    assert_eq!(
        super::rpc::SOURCES_FEATURE_CAPABILITY,
        "mob_announcements.sources.v1"
    );
    assert_eq!(super::rpc::capabilities_projection(None).await, Value::Null);
    let ready = harness_with(ScriptedNative::new());
    assert_eq!(
        super::rpc::capabilities_projection(Some(&ready.service)).await,
        json!({
            "sources": "v1",
            "local_delivery": { "kind": "ready" },
            "governed_admission": false,
        })
    );
    let mut governed = ScriptedNative::new();
    governed.governed = true;
    let governed = harness_with(governed);
    assert_eq!(
        super::rpc::capabilities_projection(Some(&governed.service)).await["local_delivery"],
        json!({ "kind": "refused", "cause": "governed_runtime" })
    );
}

/// sources/put answers how many live subscriptions the source's audience
/// and routes now bind, and whether this gateway can deliver locally.
#[tokio::test]
async fn sources_put_reports_affected_subscriptions_and_delivery_readiness() {
    let harness = harness_with(ScriptedNative::new());
    let created = try_host_call(
        &harness.service,
        SOURCES_PUT,
        source_params("put-1", "inbox", "subject-1", &["mail.received"]),
    )
    .await
    .unwrap();
    assert_eq!(created["affected_subscriptions"], json!(0), "{created}");
    assert_eq!(created["delivery_readiness"], json!({ "kind": "ready" }));
    subscribe_to_inbox(&harness.service, "sub-a", "agent:a").await;
    let replaced = try_host_call(
        &harness.service,
        SOURCES_PUT,
        with(
            source_params("put-2", "inbox", "subject-1", &["mail.received"]),
            "expected_revision",
            json!(1),
        ),
    )
    .await
    .unwrap();
    assert_eq!(replaced["affected_subscriptions"], json!(1), "{replaced}");
}

/// On a governed gateway, setup already says local delivery is refused:
/// sources/put answers it in `delivery_readiness` and meta/capabilities in
/// `local_delivery`, both from the fact the subscription check reads, and a
/// local member's subscription is refused `governed_runtime` as before.
#[tokio::test]
async fn a_governed_gateway_reports_refused_local_delivery_at_setup_and_in_capabilities() {
    let mut native = ScriptedNative::new();
    native.governed = true;
    let harness = harness_with(native);
    let refused = json!({ "kind": "refused", "cause": "governed_runtime" });
    let created = try_host_call(
        &harness.service,
        SOURCES_PUT,
        source_params("put-1", "inbox", "subject-1", &["mail.received"]),
    )
    .await
    .unwrap();
    assert_eq!(created["delivery_readiness"], refused, "{created}");
    assert_eq!(
        super::rpc::capabilities_projection(Some(&harness.service)).await,
        json!({
            "sources": "v1",
            "local_delivery": refused,
            "governed_admission": false,
        })
    );
    let subscription = try_host_call(
        &harness.service,
        SUBSCRIPTIONS_PUT,
        json!({
            "operation_id": "subscribe-a",
            "subscription_id": "sub-a",
            "member": { "mob_id": OWN_MOB, "identity": "agent:a" },
            "topics": ["mail.received"],
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(
        subscription.data.as_ref().and_then(|data| data.get("kind")),
        Some(&json!("governed_runtime")),
        "{subscription:?}"
    );
}

/// Sources are host work, not member admission: a governed gateway declares
/// a source, takes its baseline, advances it with an item, records a failure
/// and recovers on the next advance, as any gateway does.
#[tokio::test]
async fn a_governed_gateway_still_collects_and_advances_sources() {
    let mut native = ScriptedNative::new();
    native.governed = true;
    let harness = harness_with(native);
    inbox(&harness, None).await;
    let advanced = try_host_call(
        &harness.service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("advance-1", 1, 1, "c2"),
            &[upsert("m-1", "v1", "first")],
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        advanced["items"][0]["outcome"],
        json!("announced"),
        "{advanced}"
    );
    assert_eq!(advanced["source"]["cursor"]["version"], json!(2));
    fail(&harness, 2, 1_000 + MINUTE, 1).await;
    let failing = health(&harness).await;
    assert_eq!(failing["status"], json!("failing"), "{failing}");
    assert_eq!(failing["consecutive_failures"], json!(1));
    advance(&harness, 2, 1_000 + 2 * MINUTE, None, &[]).await;
    let recovered = health(&harness).await;
    assert_eq!(recovered["status"], json!("ok"), "{recovered}");
    assert_eq!(recovered["consecutive_failures"], json!(0));
}

/// A gateway composed with mob announcements and no governed host says so in
/// its first meta/capabilities answer: the feature capability, the new
/// methods, and local delivery ready. One composed without them names
/// neither.
#[tokio::test(flavor = "multi_thread")]
async fn an_ungoverned_gateway_reports_ready_local_delivery_at_boot() {
    async fn capabilities(runtime: &crate::UnifiedRuntime) -> Value {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "meta/capabilities",
            "params": {},
        })
        .to_string();
        let answer = crate::rpc::handle_unified_rpc_json(
            runtime,
            &request,
            std::time::Duration::from_secs(30),
            None,
            None,
        )
        .await;
        serde_json::from_str::<Value>(&answer).unwrap()["result"].clone()
    }
    let sources_v1 = json!("mob_announcements.sources.v1");
    let state = tempfile::tempdir().unwrap();
    let runtime = Box::pin(
        identity_first_test_builder(state.path(), "mob-k-boot-ready", "")
            .await
            .mob_announcements(AnnouncementConfig::default())
            .build(),
    )
    .await
    .expect("the builder composes mob announcements");
    let answer = capabilities(&runtime).await;
    assert!(
        answer["feature_capabilities"]
            .as_array()
            .unwrap()
            .contains(&sources_v1),
        "{answer}"
    );
    assert_eq!(
        answer["mob_announcements"],
        json!({
            "sources": "v1",
            "local_delivery": { "kind": "ready" },
            "governed_admission": false,
        }),
        "{answer}"
    );
    let methods = answer["methods"].as_array().unwrap();
    for method in [MEMBERS_GET, MEMBERS_LIST, ESCALATIONS_LIST] {
        assert!(methods.contains(&json!(method)), "{method}: {answer}");
    }
    runtime.shutdown().await;

    let plain_state = tempfile::tempdir().unwrap();
    let plain = identity_first_test_runtime(plain_state.path(), "mob-k-boot-plain").await;
    let answer = capabilities(&plain).await;
    assert_eq!(answer["mob_announcements"], Value::Null, "{answer}");
    assert!(
        !answer["feature_capabilities"]
            .as_array()
            .unwrap()
            .contains(&sources_v1),
        "{answer}"
    );
    plain.shutdown().await;
}

/// A source whose runner this gateway does not schedule (no schedule service
/// installed its runners) shows it in its health, so an operator does not
/// wait on a collector that never runs here.
#[tokio::test]
async fn a_runner_this_gateway_does_not_schedule_shows_in_the_source_health() {
    let harness = harness_with(ScriptedNative::new());
    try_host_call(
        &harness.service,
        SOURCES_PUT,
        with(
            source_params("put-1", "inbox", "subject-1", &["mail.received"]),
            "runner",
            json!({ "host_runnable": "collect-inbox", "every_seconds": 300 }),
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        health(&harness).await["runner_scheduled_here"],
        json!(false)
    );
    let other = harness_with(ScriptedNative::new());
    inbox(&other, None).await;
    assert_eq!(
        health(&other).await["runner_scheduled_here"],
        Value::Null,
        "no runner, nothing to schedule"
    );
}

/// A source escalation is announced to the agents its audience covers, on
/// the reserved health topic, with a typed body; a member outside the
/// audience subscribed to the same topic captures nothing.
#[tokio::test]
async fn a_source_escalation_is_announced_on_the_health_topic_to_its_audience() {
    let mut native = ScriptedNative::new();
    native.known.insert("agent:b".into());
    let harness = harness_with(native);
    inbox(&harness, None).await;
    for (id, identity) in [("health-a", "agent:a"), ("health-b", "agent:b")] {
        try_host_call(
            &harness.service,
            super::tests::SUBSCRIPTIONS_PUT,
            json!({
                "operation_id": format!("subscribe-{id}"),
                "subscription_id": id,
                "member": { "mob_id": OWN_MOB, "identity": identity },
                "topics": ["mob_announcements.health"],
            }),
        )
        .await
        .unwrap();
    }
    for attempt in 1..=3 {
        fail(&harness, 1, 1_000 + u64::from(attempt) * MINUTE, attempt).await;
    }
    round_at(&harness, 1_000 + 3 * MINUTE).await;
    let owed = owed(&harness).await;
    assert_eq!(owed.get("agent:a"), Some(&1), "{owed:?}");
    assert_eq!(owed.get("agent:b"), None, "{owed:?}");
}

/// Every escalation event reaches the host's error sink as it is written:
/// raised as MobAnnouncementEscalated, resolved as
/// MobAnnouncementEscalationResolved, both under one id and with the event
/// cursor.
#[tokio::test]
async fn escalation_events_reach_the_error_sink_with_their_cursor() {
    let harness = harness_with(ScriptedNative::new());
    let seen: Arc<std::sync::Mutex<Vec<Value>>> = Arc::default();
    let sink = Arc::clone(&seen);
    harness
        .service
        .set_error_sink(Arc::new(move |event: crate::ErrorEvent| {
            sink.lock()
                .unwrap()
                .push(serde_json::to_value(&event).unwrap());
        }));
    inbox(&harness, None).await;
    for attempt in 1..=3 {
        fail(&harness, 1, 1_000 + u64::from(attempt) * MINUTE, attempt).await;
    }
    round_at(&harness, 1_000 + 3 * MINUTE).await;
    advance(&harness, 1, 1_000 + 4 * MINUTE, None, &[]).await;
    round_at(&harness, 1_000 + 4 * MINUTE).await;
    let seen = seen.lock().unwrap().clone();
    let categories: Vec<Value> = seen.iter().map(|event| event["category"].clone()).collect();
    assert_eq!(
        categories,
        vec![
            json!("mob_announcement_escalated"),
            json!("mob_announcement_escalation_resolved"),
        ],
        "{seen:?}"
    );
    assert_eq!(
        seen[0]["escalation"]["escalation_id"],
        seen[1]["escalation_id"]
    );
    assert_eq!(seen[0]["escalation"]["cause"], json!("failure_streak"));
    assert!(seen[1]["cursor"].as_u64() > seen[0]["cursor"].as_u64());
}

/// The defaults the owner runs with: a day between notifications of an open
/// escalation, and no health audience for member escalations.
#[test]
fn escalation_options_default_to_a_day_and_no_health_audience() {
    let config = AnnouncementConfig::default();
    assert_eq!(config.renotify_after_ms, DAY);
    assert!(config.health_audience.is_empty());
    let parsed = super::parse_options(
        &json!({
            "enabled": true,
            "renotify_after_ms": 2 * DAY,
            "health_audience": [
                { "kind": "member", "member": { "mob_id": OWN_MOB, "identity": "agent:a" } }
            ],
        }),
        "mob_announcements",
    )
    .unwrap()
    .unwrap();
    assert_eq!(parsed.renotify_after_ms, 2 * DAY);
    assert_eq!(parsed.health_audience.len(), 1);
}

/// The raise and resolve the declaration and handling slices call inside
/// their own store transactions are idempotent: a second raise of an open
/// escalation keeps it as it is and writes no event, a resolve without an
/// open one writes nothing, and each write is one event at the next cursor.
#[tokio::test]
async fn the_escalation_api_is_idempotent_inside_a_store_transaction() {
    use super::declarations::DeclarationRecordKind;
    use super::escalations::{EscalationCause, EscalationEventKind, EscalationSubject};
    use super::sqlite::{SqliteAnnouncementStore, raise_escalation_in, resolve_escalation_in};

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mob_announcements.sqlite3");
    let store = SqliteAnnouncementStore::open(&path).unwrap();
    let subject = EscalationSubject::Declaration {
        record_kind: DeclarationRecordKind::Subscription,
        id: "sub-1".to_string(),
    };
    let pending = EscalationCause::DeclarationPending;
    let mut db = rusqlite::Connection::open(&path).unwrap();
    let transaction = db.transaction().unwrap();
    raise_escalation_in(&transaction, pending, &subject, 10, 20).unwrap();
    raise_escalation_in(&transaction, pending, &subject, 15, 25).unwrap();
    resolve_escalation_in(&transaction, EscalationCause::Stale, &subject, 25).unwrap();
    transaction.commit().unwrap();
    let open = store.open_escalations(None).await.unwrap();
    assert_eq!(open.len(), 1, "{open:?}");
    assert_eq!(open[0].since_ms, 10, "the second raise kept the open one");
    assert_eq!(open[0].raised_at_ms, 20);

    let transaction = db.transaction().unwrap();
    resolve_escalation_in(&transaction, pending, &subject, 30).unwrap();
    resolve_escalation_in(&transaction, pending, &subject, 35).unwrap();
    transaction.commit().unwrap();
    assert!(store.open_escalations(None).await.unwrap().is_empty());
    let (events, next) = store.escalation_events(0, 10).await.unwrap();
    let kinds: Vec<_> = events.iter().map(|event| event.event).collect();
    assert_eq!(
        kinds,
        [EscalationEventKind::Raised, EscalationEventKind::Resolved]
    );
    assert!(
        events
            .iter()
            .all(|event| event.escalation_id == open[0].escalation_id)
    );
    assert_eq!(next, events[1].cursor);
    assert!(events[0].cursor < events[1].cursor);
}

/// A rebaseline raises "source_gap" even when its pages finish before the
/// dispatcher's next round, dated from its gap notice, and the escalation
/// stays open for a day after the source's last gap.
#[tokio::test]
async fn a_rebaseline_raises_a_source_gap_that_resolves_a_day_after_the_last_gap() {
    let harness = harness_with(ScriptedNative::new());
    inbox(&harness, None).await;
    let at = 1_000 + MINUTE;
    harness.clock.store(at, Ordering::SeqCst);
    let rebaseline = with(
        with(
            advance_params("rebaseline:v1", 1, 1, "r1"),
            "rebaseline",
            json!({ "reason": "cursor_expired" }),
        ),
        "page",
        json!({ "index": 0, "final": false }),
    );
    try_host_call(&harness.service, SOURCES_ADVANCE, rebaseline)
        .await
        .unwrap();
    let last = with(
        with(
            advance_params("rebaseline-last:v2", 1, 2, "c9"),
            "page",
            json!({ "index": 1, "final": true }),
        ),
        "coverage",
        json!("complete"),
    );
    let advanced = try_host_call(&harness.service, SOURCES_ADVANCE, last)
        .await
        .unwrap();
    assert_eq!(advanced["source"]["collection"], json!("live"));
    round_at(&harness, at + MINUTE).await;
    let gap = open_of(&harness, "source_gap").await;
    assert_eq!(gap.len(), 1, "{gap:?}");
    assert_eq!(gap[0]["subject"], inbox_subject());
    assert_eq!(gap[0]["since_ms"], json!(at));
    round_at(&harness, at + DAY - MINUTE).await;
    assert_eq!(
        open_of(&harness, "source_gap").await.len(),
        1,
        "open inside the day"
    );
    round_at(&harness, at + DAY).await;
    assert!(
        open_of(&harness, "source_gap").await.is_empty(),
        "resolved a day after the last gap"
    );
}

/// A batch that reports gaps raises "member_gap" for its member, though the
/// gap left the member's pending gaps when the batch was sealed, dated from
/// the batch; the escalation stays open for a day after the member's last
/// batch with gaps.
#[tokio::test]
async fn a_batch_reporting_gaps_raises_a_member_gap_that_resolves_a_day_later() {
    let harness = harness_with(ScriptedNative::new());
    let store = &harness.store;
    store
        .put_subscription(
            &op("o1"),
            put("s1", "agent:a", &["ios"], StartFrontier::Now),
            1_000,
        )
        .await
        .unwrap();
    store
        .announce(announce_request(OWN_MOB, "k1", &["ios"], "old"), 1_000)
        .await
        .unwrap();
    let report = store.expire(1_000 + HOUR, HOUR, 0, 100).await.unwrap();
    assert_eq!(report.captures, 1, "{report:?}");
    let sealed_at = 1_000 + HOUR + MINUTE;
    store
        .announce(
            announce_request(OWN_MOB, "k2", &["ios"], "new"),
            sealed_at - 1_000,
        )
        .await
        .unwrap();
    let batch = store
        .seal_batch(&member("agent:a"), &batch_id("b1"), &policy(), sealed_at)
        .await
        .unwrap()
        .expect("the new announcement is due");
    assert!(!batch.gaps.is_empty(), "{batch:?}");
    round_at(&harness, sealed_at + MINUTE).await;
    let gap = open_of(&harness, "member_gap").await;
    assert_eq!(gap.len(), 1, "{gap:?}");
    assert_eq!(
        gap[0]["subject"],
        json!({ "kind": "member", "member": { "mob_id": OWN_MOB, "identity": "agent:a" } })
    );
    assert_eq!(gap[0]["since_ms"], json!(sealed_at));
    round_at(&harness, sealed_at + DAY).await;
    assert!(
        open_of(&harness, "member_gap").await.is_empty(),
        "resolved a day after the last batch with gaps"
    );
}

/// A capture withheld because the source's audience no longer covers its
/// member raises "audience_hold" for that member, as held items do, dated
/// from the first withheld capture; it resolves a day after the last one.
#[tokio::test]
async fn a_capture_withheld_for_coverage_raises_an_audience_hold() {
    let harness = harness_with(ScriptedNative::new());
    inbox(&harness, None).await;
    subscribe_to_inbox(&harness.service, "sub-a", "agent:a").await;
    let narrowed = with(
        with(
            source_params("put-narrow", "inbox", "subject-1", &["mail.received"]),
            "expected_revision",
            json!(1),
        ),
        "audience",
        json!([{ "kind": "member", "member": { "mob_id": OWN_MOB, "identity": "agent:b" } }]),
    );
    try_host_call(&harness.service, SOURCES_PUT, narrowed)
        .await
        .unwrap();
    let at = 1_000 + MINUTE;
    harness.clock.store(at, Ordering::SeqCst);
    let advanced = try_host_call(
        &harness.service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("withheld:v1", 2, 1, "c2"),
            &[upsert("m-1", "v1", "first")],
        ),
    )
    .await
    .unwrap();
    assert_eq!(advanced["items"][0]["captured"], json!(0), "{advanced}");
    round_at(&harness, at + MINUTE).await;
    let hold = open_of(&harness, "audience_hold").await;
    assert_eq!(hold.len(), 1, "{hold:?}");
    assert_eq!(
        hold[0]["subject"],
        json!({ "kind": "member", "member": { "mob_id": OWN_MOB, "identity": "agent:a" } })
    );
    assert_eq!(hold[0]["since_ms"], json!(at));
    round_at(&harness, at + DAY).await;
    assert!(
        open_of(&harness, "audience_hold").await.is_empty(),
        "resolved a day after the last withheld capture"
    );
}

/// sources/list with "escalated" reads on past sources without an open
/// escalation until its page is full, so an escalated source behind a page
/// of others is still listed.
#[tokio::test]
async fn sources_list_escalated_reads_on_until_its_page_is_full() {
    let harness = harness_with(ScriptedNative::new());
    try_host_call(
        &harness.service,
        SOURCES_PUT,
        source_params("put-calm", "a-calm", "subject-calm", &["calm.received"]),
    )
    .await
    .unwrap();
    inbox(&harness, None).await;
    for attempt in 1..=3 {
        fail(&harness, 1, 1_000 + u64::from(attempt) * MINUTE, attempt).await;
    }
    round_at(&harness, 1_000 + 3 * MINUTE).await;
    assert_eq!(open_of(&harness, "failure_streak").await.len(), 1);
    let listed = try_host_call(
        &harness.service,
        SOURCES_LIST,
        json!({ "escalated": true, "limit": 1 }),
    )
    .await
    .unwrap();
    let ids: Vec<Value> = listed["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|source| source["source_id"].clone())
        .collect();
    assert_eq!(ids, vec![json!("inbox")], "{listed}");
}

/// The health topic is the escalation pass's own: a host announcement on it
/// is refused as invalid params, and so is a member's.
#[tokio::test]
async fn the_health_topic_is_reserved_for_the_escalation_pass() {
    use super::types::{AnnounceRequest, Announcer, MemberIdentity, SourceKey, Topic};

    let harness = harness_with(ScriptedNative::new());
    let refused = try_host_call(
        &harness.service,
        "meta/mob_announcements/announce",
        json!({
            "source_key": "k-1",
            "topics": ["mob_announcements.health"],
            "body": "not a health event",
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(refused.code, -32602, "{refused:?}");
    let member_announce = harness
        .service
        .announce(AnnounceRequest {
            mob_id: super::tests::mob(OWN_MOB),
            announced_by: Announcer::Member {
                identity: MemberIdentity::new("agent:a").unwrap(),
            },
            source_key: SourceKey::new("k-2").unwrap(),
            topics: [Topic::new("mob_announcements.health").unwrap()].into(),
            body: "not a health event either".to_string(),
            correlation_id: None,
            causes: Vec::new(),
        })
        .await;
    assert!(
        matches!(
            member_announce,
            Err(super::owner::AnnouncementError::Store(
                super::types::AnnouncementStoreError::Invalid(_)
            ))
        ),
        "{member_announce:?}"
    );
}

/// Conditions that happened while no round ran (the gateway was down for
/// days) are raised by the next round, dated from when they happened: the
/// window reads back to the last round that evaluated escalations. A
/// condition last seen more than a day ago is then resolved by the round
/// after.
#[tokio::test]
async fn conditions_that_happened_while_no_round_ran_are_raised_by_the_next_round() {
    let mut native = ScriptedNative::new();
    native.known.insert("agent:b".into());
    let harness = harness_with(native);
    let service = &harness.service;
    inbox(&harness, None).await;
    let last_round = 10 * HOUR;
    round_at(&harness, last_round).await;
    assert!(open_escalations(&harness).await.is_empty());

    let at = last_round + MINUTE;
    harness.clock.store(at, Ordering::SeqCst);
    // A conflict: the same item with other content.
    for (operation, version, cursor, body) in [
        ("conflict:v1", 1, "c2", "the original"),
        ("conflict:v2", 2, "c3", "other content"),
    ] {
        try_host_call(
            service,
            SOURCES_ADVANCE,
            with_items(
                advance_params(operation, 1, version, cursor),
                &[upsert("m-1", "v1", body)],
            ),
        )
        .await
        .unwrap();
    }
    // The audience narrows past agent:a, whose next capture is withheld.
    subscribe_to_inbox(service, "sub-a", "agent:a").await;
    let narrowed = with(
        with(
            source_params("put-narrow", "inbox", "subject-1", &["mail.received"]),
            "expected_revision",
            json!(1),
        ),
        "audience",
        json!([{ "kind": "member", "member": { "mob_id": OWN_MOB, "identity": "agent:b" } }]),
    );
    try_host_call(service, SOURCES_PUT, narrowed).await.unwrap();
    try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("withheld:v3", 2, 3, "c4"),
            &[upsert("m-2", "v1", "second")],
        ),
    )
    .await
    .unwrap();
    // A rebaseline whose pages finish at once.
    let rebaseline = with(
        with(
            advance_params("rebaseline:v4", 2, 4, "r1"),
            "rebaseline",
            json!({ "reason": "cursor_expired" }),
        ),
        "page",
        json!({ "index": 0, "final": false }),
    );
    try_host_call(service, SOURCES_ADVANCE, rebaseline)
        .await
        .unwrap();
    let last_page = with(
        with(
            advance_params("rebaseline:v5", 2, 5, "c9"),
            "page",
            json!({ "index": 1, "final": true }),
        ),
        "coverage",
        json!("complete"),
    );
    try_host_call(service, SOURCES_ADVANCE, last_page)
        .await
        .unwrap();
    // A batch of agent:b that reports gaps.
    let store = &harness.store;
    store
        .put_subscription(
            &op("o1"),
            put("s1", "agent:b", &["ios"], StartFrontier::Now),
            at,
        )
        .await
        .unwrap();
    store
        .announce(announce_request(OWN_MOB, "k1", &["ios"], "old"), at)
        .await
        .unwrap();
    store.expire(at + HOUR, HOUR, 0, 100).await.unwrap();
    let sealed_at = at + HOUR + 2 * MINUTE;
    store
        .announce(
            announce_request(OWN_MOB, "k2", &["ios"], "new"),
            sealed_at - MINUTE,
        )
        .await
        .unwrap();
    let batch = store
        .seal_batch(&member("agent:b"), &batch_id("b1"), &policy(), sealed_at)
        .await
        .unwrap()
        .expect("the new announcement is due");
    assert!(!batch.gaps.is_empty(), "{batch:?}");

    // Down for three days: the next round raises all four.
    let back = sealed_at + 3 * DAY;
    round_at(&harness, back).await;
    for (cause, since) in [
        ("conflicts", at),
        ("audience_hold", at),
        ("source_gap", at),
        ("member_gap", sealed_at),
    ] {
        let open = open_of(&harness, cause).await;
        assert_eq!(open.len(), 1, "{cause}: {open:?}");
        assert_eq!(open[0]["since_ms"], json!(since), "{cause}");
    }
    // None of them happened again within a day of this round.
    round_at(&harness, back + MINUTE).await;
    for cause in ["conflicts", "audience_hold", "source_gap", "member_gap"] {
        assert!(open_of(&harness, cause).await.is_empty(), "{cause}");
    }
}

/// The SQL catalog of the store at `path`, and its ledger version.
fn catalog_of(path: &std::path::Path) -> (Vec<(String, String, Option<String>)>, i64) {
    let connection = rusqlite::Connection::open(path).unwrap();
    let mut statement = connection
        .prepare("SELECT type, name, sql FROM sqlite_master ORDER BY type, name")
        .unwrap();
    let objects = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    let version = connection
        .query_row(
            "SELECT version FROM meerkat_schema WHERE domain = 'meta-mob-announcements'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    (objects, version)
}

/// A store opened without the index of batches that reported gaps (one that
/// 0.9.4 left at version 1) gets it in the upgrade (version 3 creates it) and
/// reads its member gaps; a second open changes nothing.
#[tokio::test]
async fn a_store_without_the_gap_batch_index_gets_it_on_open_and_keeps_it() {
    use super::escalations::EscalationPass;
    use super::sources::RosterFacts;

    // A batch that reported gaps, as this release seals it.
    let source_dir = tempfile::tempdir().unwrap();
    let source_path = source_dir.path().join("mob_announcements.sqlite3");
    let current = super::storage::open_disk_store(source_path.clone()).unwrap();
    current
        .put_subscription(
            &op("o1"),
            put("s1", "agent:a", &["ios"], StartFrontier::Now),
            1_000,
        )
        .await
        .unwrap();
    current
        .announce(announce_request(OWN_MOB, "k1", &["ios"], "old"), 1_000)
        .await
        .unwrap();
    current.expire(1_000 + HOUR, HOUR, 0, 100).await.unwrap();
    let sealed_at = 1_000 + HOUR + 2 * MINUTE;
    current
        .announce(
            announce_request(OWN_MOB, "k2", &["ios"], "new"),
            sealed_at - MINUTE,
        )
        .await
        .unwrap();
    let batch = current
        .seal_batch(&member("agent:a"), &batch_id("b1"), &policy(), sealed_at)
        .await
        .unwrap()
        .expect("the new announcement is due");
    assert!(!batch.gaps.is_empty(), "{batch:?}");
    drop(current);
    let (batch_member, state, record): (String, String, String) =
        rusqlite::Connection::open(&source_path)
            .unwrap()
            .query_row(
                "SELECT member, state, record FROM announcement_batches WHERE id = 'b1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();

    // The same batch in a store as 0.9.4 left it: version 1, no such index.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mob_announcements.sqlite3");
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch(super::sqlite::SCHEMA).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE main.meerkat_schema (
                     domain TEXT PRIMARY KEY,
                     version INTEGER NOT NULL
                 );
                 INSERT INTO main.meerkat_schema (domain, version)
                     VALUES ('meta-mob-announcements', 1);",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO announcement_batches
                 (id, member, state, sealed_at_ms, updated_at_ms, record)
                 VALUES ('b1', ?1, ?2, ?3, ?3, ?4)",
                rusqlite::params![
                    batch_member,
                    state,
                    i64::try_from(sealed_at).unwrap(),
                    record
                ],
            )
            .unwrap();
    }
    let has_index = |path: &std::path::Path| {
        catalog_of(path)
            .0
            .iter()
            .any(|(kind, name, _)| kind == "index" && name == "announcement_batches_reporting_gaps")
    };
    assert!(!has_index(&path));
    let store = super::storage::open_disk_store(path.clone()).unwrap();
    assert!(has_index(&path), "the upgrade created it");
    let pass = EscalationPass {
        renotify_after_ms: DAY,
        health_audience: Vec::new(),
        retention_ms: 7 * DAY,
        handled_unverified_threshold: 3,
        hosted: std::collections::BTreeSet::new(),
    };
    store
        .escalation_pass(pass, &RosterFacts::default(), sealed_at + MINUTE)
        .await
        .unwrap();
    let open = store.open_escalations(None).await.unwrap();
    assert_eq!(open.len(), 1, "{open:?}");
    assert_eq!(open[0].cause.as_str(), "member_gap");
    assert_eq!(open[0].since_ms, sealed_at);
    drop(store);
    let before = catalog_of(&path);
    assert_eq!(before.1, 3);
    drop(super::storage::open_disk_store(path.clone()).unwrap());
    assert_eq!(catalog_of(&path), before, "a second open changes nothing");
}

/// A release that knows only version 1 (0.9.4) refuses a store this release
/// opened, as a schema from the future, before it reads or changes
/// anything: no object of a later version, the gap batch index among them,
/// is its to read.
#[test]
fn a_release_that_knows_only_version_one_refuses_a_newer_store() {
    fn released_version_one(
        tx: &rusqlite::Transaction<'_>,
    ) -> std::result::Result<(), rusqlite::Error> {
        tx.execute_batch(super::sqlite::SCHEMA)
    }
    static RELEASED: meerkat_sqlite::SchemaDomain = meerkat_sqlite::SchemaDomain {
        name: "meta-mob-announcements",
        migrations: &[meerkat_sqlite::Migration {
            version: 1,
            name: "announcements-subscriptions-batches",
            apply: released_version_one,
        }],
        initialize_current: released_version_one,
        allowed_existing_versions: &[1],
        bridge_recoverable_versions: &[],
        released_predecessors: &[],
        owned_objects: &[meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "announcements",
        }],
        retired_objects: &[],
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mob_announcements.sqlite3");
    drop(super::storage::open_disk_store(path.clone()).unwrap());
    let before = catalog_of(&path);
    let mut connection = rusqlite::Connection::open(&path).unwrap();
    let refused = meerkat_sqlite::apply_domain_migrations(&mut connection, &RELEASED).unwrap_err();
    assert!(
        matches!(
            refused,
            meerkat_sqlite::SqliteStoreError::SchemaFromTheFuture {
                found: 3,
                supported: 1,
                ..
            }
        ),
        "{refused:?}"
    );
    assert!(
        refused
            .to_string()
            .contains("is from the future: file has version 3, this binary supports up to 1"),
        "{refused}"
    );
    drop(connection);
    assert_eq!(catalog_of(&path), before, "the refusal changed nothing");
}

/// A build that knows versions 1 and 2 only (the first 0.9.5 builds) refuses
/// a store this build opened, as a schema from the future, before it reads
/// or changes anything.
#[test]
fn a_build_that_knows_only_version_two_refuses_a_version_three_store() {
    fn version_one(tx: &rusqlite::Transaction<'_>) -> std::result::Result<(), rusqlite::Error> {
        tx.execute_batch(super::sqlite::SCHEMA)
    }
    fn version_two(tx: &rusqlite::Transaction<'_>) -> std::result::Result<(), rusqlite::Error> {
        tx.execute_batch(super::sqlite::SCHEMA_V2)
    }
    fn current_two(tx: &rusqlite::Transaction<'_>) -> std::result::Result<(), rusqlite::Error> {
        tx.execute_batch(super::sqlite::SCHEMA)?;
        tx.execute_batch(super::sqlite::SCHEMA_V2)
    }
    fn verify_one(_: &rusqlite::Connection) -> std::result::Result<(), String> {
        Ok(())
    }
    static EARLIER: meerkat_sqlite::SchemaDomain = meerkat_sqlite::SchemaDomain {
        name: "meta-mob-announcements",
        migrations: &[
            meerkat_sqlite::Migration {
                version: 1,
                name: "announcements-subscriptions-batches",
                apply: version_one,
            },
            meerkat_sqlite::Migration {
                version: 2,
                name: "announcement-sources",
                apply: version_two,
            },
        ],
        initialize_current: current_two,
        allowed_existing_versions: &[1, 2],
        bridge_recoverable_versions: &[],
        released_predecessors: &[meerkat_sqlite::SchemaPredecessor {
            version: 1,
            verify: verify_one,
        }],
        owned_objects: &[meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "announcements",
        }],
        retired_objects: &[],
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mob_announcements.sqlite3");
    drop(super::storage::open_disk_store(path.clone()).unwrap());
    let before = catalog_of(&path);
    let mut connection = rusqlite::Connection::open(&path).unwrap();
    let refused = meerkat_sqlite::apply_domain_migrations(&mut connection, &EARLIER).unwrap_err();
    assert!(
        matches!(
            refused,
            meerkat_sqlite::SqliteStoreError::SchemaFromTheFuture {
                found: 3,
                supported: 2,
                ..
            }
        ),
        "{refused:?}"
    );
    drop(connection);
    assert_eq!(catalog_of(&path), before, "the refusal changed nothing");
}

/// `identity` of this gateway's mob subscribes to the health topic.
async fn subscribe_to_health(harness: &Harness, identity: &str) {
    let id = identity.replace(':', "-");
    try_host_call(
        &harness.service,
        SUBSCRIPTIONS_PUT,
        json!({
            "operation_id": format!("subscribe-health-{id}"),
            "subscription_id": format!("health-{id}"),
            "member": { "mob_id": OWN_MOB, "identity": identity },
            "topics": ["mob_announcements.health"],
        }),
    )
    .await
    .unwrap();
}

/// Whether the open escalation of `cause` was announced on the health topic
/// of this gateway's mob under its own key.
async fn announced(harness: &Harness, cause: &str) -> bool {
    let open = harness.store.open_escalations(None).await.unwrap();
    let escalation = open
        .iter()
        .find(|escalation| escalation.cause.as_str() == cause)
        .expect("an open escalation of the cause");
    let key = super::types::SourceKey::new(super::escalations::health_key(escalation)).unwrap();
    harness
        .store
        .announcement(&super::tests::mob(OWN_MOB), &key)
        .await
        .unwrap()
        .is_some()
}

/// A health audience of one member of this gateway's mob.
fn health_audience_of(identity: &str) -> AnnouncementConfig {
    AnnouncementConfig {
        health_audience: vec![
            serde_json::from_value(json!({
                "kind": "member",
                "member": { "mob_id": OWN_MOB, "identity": identity },
            }))
            .unwrap(),
        ],
        ..AnnouncementConfig::default()
    }
}

/// A source subject's key keeps its parts apart: mob "a" with source "b:c"
/// and mob "a:b" with source "c" are two subjects, and an escalation of one
/// is never the other's.
#[tokio::test]
async fn two_subjects_that_once_shared_a_key_are_kept_apart() {
    use super::escalations::{EscalationCause, EscalationSubject};
    use super::sqlite::{SqliteAnnouncementStore, raise_escalation_in};
    use super::types::{MobId, SourceId};

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mob_announcements.sqlite3");
    let store = SqliteAnnouncementStore::open(&path).unwrap();
    let one = EscalationSubject::Source {
        mob_id: MobId::new("a").unwrap(),
        source_id: SourceId::new("b:c").unwrap(),
    };
    let other = EscalationSubject::Source {
        mob_id: MobId::new("a:b").unwrap(),
        source_id: SourceId::new("c").unwrap(),
    };
    assert_ne!(one.key(), other.key());
    let mut db = rusqlite::Connection::open(&path).unwrap();
    let transaction = db.transaction().unwrap();
    raise_escalation_in(&transaction, EscalationCause::Stale, &one, 10, 20).unwrap();
    raise_escalation_in(&transaction, EscalationCause::Stale, &other, 15, 25).unwrap();
    transaction.commit().unwrap();
    for subject in [&one, &other] {
        let open = store.open_escalations(Some(subject)).await.unwrap();
        assert_eq!(open.len(), 1, "{open:?}");
        assert_eq!(&open[0].subject, subject);
    }
}

/// A member escalation of a mob this gateway does not host (a peer
/// gateway's member) reaches the host, but is not announced into that mob:
/// no announcement log is opened under a mob hosted elsewhere.
#[tokio::test]
async fn health_is_announced_only_into_mobs_hosted_here() {
    let remote = peer_member("agent:x", "mob-b");
    let config = AnnouncementConfig {
        health_audience: vec![
            serde_json::from_value(json!({
                "kind": "member",
                "member": { "mob_id": "mob-b", "identity": "agent:x" },
            }))
            .unwrap(),
        ],
        ..AnnouncementConfig::default()
    };
    let harness = harness_with_config(ScriptedNative::new(), config);
    let store = &harness.store;
    store
        .put_subscription(&op("o1"), put_for("s1", remote.clone(), &["ios"]), 1_000)
        .await
        .unwrap();
    store
        .announce(announce_request(OWN_MOB, "k1", &["ios"], "old"), 1_000)
        .await
        .unwrap();
    store.expire(1_000 + HOUR, HOUR, 0, 100).await.unwrap();
    let sealed_at = 1_000 + HOUR + 2 * MINUTE;
    store
        .announce(
            announce_request(OWN_MOB, "k2", &["ios"], "new"),
            sealed_at - MINUTE,
        )
        .await
        .unwrap();
    let batch = store
        .seal_batch(&remote, &batch_id("b1"), &policy(), sealed_at)
        .await
        .unwrap()
        .expect("the new announcement is due");
    assert!(!batch.gaps.is_empty(), "{batch:?}");
    round_at(&harness, sealed_at + MINUTE).await;
    let gap = open_of(&harness, "member_gap").await;
    assert_eq!(gap.len(), 1, "the host hears of it: {gap:?}");
    assert!(
        !store
            .known_mobs()
            .await
            .unwrap()
            .contains(&super::tests::mob("mob-b")),
        "no log under a mob hosted elsewhere"
    );
}

/// The health topic is the escalation pass's own: a source naming it as a
/// topic, or a triage route naming it as a forward topic, is refused as
/// invalid params, so no adapter text reaches it.
#[tokio::test]
async fn a_source_naming_the_health_topic_is_refused() {
    let harness = harness_with(ScriptedNative::new());
    let as_topic = try_host_call(
        &harness.service,
        SOURCES_PUT,
        source_params("put-1", "inbox", "subject-1", &["mob_announcements.health"]),
    )
    .await
    .unwrap_err();
    assert_eq!(as_topic.code, -32602, "{as_topic:?}");
    let as_forward = try_host_call(
        &harness.service,
        SOURCES_PUT,
        with(
            source_params("put-2", "inbox", "subject-1", &["mail.received"]),
            "routes",
            json!([{
                "kind": "triage",
                "member": { "mob_id": OWN_MOB, "identity": "agent:a" },
                "forward_topics": ["mob_announcements.health"],
            }]),
        ),
    )
    .await
    .unwrap_err();
    assert_eq!(as_forward.code, -32602, "{as_forward:?}");
}

/// A stale source paused and resumed raises a new stale escalation with the
/// same since: its health announcement's key carries the new escalation's
/// id, so it is announced again and the pass goes on.
#[tokio::test]
async fn a_stale_source_resumed_after_a_pause_is_announced_again() {
    let harness = harness_with(ScriptedNative::new());
    inbox(&harness, None).await;
    subscribe_to_health(&harness, "agent:a").await;
    round_at(&harness, 1_000 + DAY).await;
    assert_eq!(open_of(&harness, "stale").await.len(), 1);
    assert!(announced(&harness, "stale").await);
    harness.clock.store(1_000 + DAY + MINUTE, Ordering::SeqCst);
    try_host_call(
        &harness.service,
        SOURCES_PAUSE,
        json!({ "operation_id": "pause", "source_id": "inbox", "expected_revision": 1 }),
    )
    .await
    .unwrap();
    round_at(&harness, 1_000 + DAY + 2 * MINUTE).await;
    assert!(
        open_of(&harness, "stale").await.is_empty(),
        "a paused source never escalates"
    );
    try_host_call(
        &harness.service,
        SOURCES_RESUME,
        json!({ "operation_id": "resume", "source_id": "inbox", "expected_revision": 2 }),
    )
    .await
    .unwrap();
    round_at(&harness, 1_000 + DAY + 3 * MINUTE).await;
    let again = open_of(&harness, "stale").await;
    assert_eq!(again.len(), 1, "raised again: {again:?}");
    assert!(announced(&harness, "stale").await, "announced again");
}

/// A quiet source whose collector fails for longer than the quiet window and
/// then recovers with nothing new raises quiet again with the same since:
/// announced again under the new escalation's key, and the pass goes on.
#[tokio::test]
async fn a_quiet_source_recovering_from_a_failure_spell_is_announced_again() {
    let harness = harness_with(ScriptedNative::new());
    inbox(&harness, Some(json!({ "quiet_after_ms": HOUR }))).await;
    subscribe_to_health(&harness, "agent:a").await;
    let version = advance(&harness, 1, 1_000 + HOUR + MINUTE, None, &[]).await;
    round_at(&harness, 1_000 + HOUR + 2 * MINUTE).await;
    assert_eq!(open_of(&harness, "quiet").await.len(), 1);
    assert!(announced(&harness, "quiet").await);
    for attempt in 1..=3 {
        fail(
            &harness,
            version,
            1_000 + 2 * HOUR + u64::from(attempt) * MINUTE,
            attempt,
        )
        .await;
    }
    round_at(&harness, 1_000 + 3 * HOUR).await;
    assert!(
        open_of(&harness, "quiet").await.is_empty(),
        "no quiet while failing"
    );
    advance(&harness, version, 1_000 + 3 * HOUR + MINUTE, None, &[]).await;
    round_at(&harness, 1_000 + 3 * HOUR + 2 * MINUTE).await;
    let again = open_of(&harness, "quiet").await;
    assert_eq!(again.len(), 1, "raised again: {again:?}");
    assert!(announced(&harness, "quiet").await, "announced again");
}

/// A member's captures withheld for coverage stop for a day and then
/// resume: audience_hold is raised again with the same since (one counted
/// entry keeps its first time), announced again to the health audience
/// under the new escalation's key, and the pass goes on.
#[tokio::test]
async fn an_audience_hold_raised_again_is_announced_again() {
    let harness = harness_with_config(ScriptedNative::new(), health_audience_of("agent:a"));
    let service = &harness.service;
    inbox(&harness, None).await;
    subscribe_to_inbox(service, "sub-a", "agent:a").await;
    subscribe_to_health(&harness, "agent:a").await;
    let narrowed = with(
        with(
            source_params("put-narrow", "inbox", "subject-1", &["mail.received"]),
            "expected_revision",
            json!(1),
        ),
        "audience",
        json!([{ "kind": "member", "member": { "mob_id": OWN_MOB, "identity": "agent:b" } }]),
    );
    try_host_call(service, SOURCES_PUT, narrowed).await.unwrap();
    let at = 1_000 + MINUTE;
    for (version, at_ms, item) in [(1, at, "m-1"), (2, at + DAY + MINUTE, "m-2")] {
        if version == 2 {
            round_at(&harness, at + DAY).await;
            assert!(
                open_of(&harness, "audience_hold").await.is_empty(),
                "a day without a withheld capture"
            );
        }
        harness.clock.store(at_ms, Ordering::SeqCst);
        try_host_call(
            service,
            SOURCES_ADVANCE,
            with_items(
                advance_params(
                    &format!("withheld:v{version}"),
                    2,
                    version,
                    &format!("c{}", version + 1),
                ),
                &[upsert(item, "v1", "withheld")],
            ),
        )
        .await
        .unwrap();
        round_at(&harness, at_ms + MINUTE).await;
        let hold = open_of(&harness, "audience_hold").await;
        assert_eq!(hold.len(), 1, "raised at v{version}: {hold:?}");
        assert_eq!(hold[0]["since_ms"], json!(at));
        assert!(
            announced(&harness, "audience_hold").await,
            "announced at v{version}"
        );
    }
}

/// A source no collector advanced for its stale window shows stale, computed
/// at read, in sources/get and in meta/status's counts.
#[tokio::test]
async fn a_stale_source_shows_in_meta_status_and_sources_get() {
    let harness = harness_with(ScriptedNative::new());
    inbox(&harness, None).await;
    harness.clock.store(1_000 + DAY, Ordering::SeqCst);
    assert_eq!(health(&harness).await["status"], json!("stale"));
    let status = super::rpc::status_projection(&harness.service).await;
    assert_eq!(
        status["sources"],
        json!({ "active": 0, "paused": 0, "failing": 0, "stale": 1 }),
        "{status}"
    );
}

/// A source whose collector keeps failing is failing, in any collector
/// phase, and never stale; failure_streak escalates it. Paused, it neither
/// counts as failing nor escalates.
#[tokio::test]
async fn a_failing_source_is_failing_not_stale_in_every_phase() {
    let harness = harness_with(ScriptedNative::new());
    try_host_call(
        &harness.service,
        SOURCES_PUT,
        source_params("put-inbox", "inbox", "subject-1", &["mail.received"]),
    )
    .await
    .unwrap();
    fail(&harness, 0, 1_000 + MINUTE, 1).await;
    assert_eq!(
        health(&harness).await["status"],
        json!("failing"),
        "failing while it baselines"
    );
    round_at(&harness, 1_000 + DAY + MINUTE).await;
    assert!(open_of(&harness, "stale").await.is_empty());
    assert_eq!(open_of(&harness, "failure_streak").await.len(), 1);
    assert_eq!(
        super::rpc::status_projection(&harness.service).await["sources"],
        json!({ "active": 0, "paused": 0, "failing": 1, "stale": 0 })
    );
    try_host_call(
        &harness.service,
        SOURCES_PAUSE,
        json!({ "operation_id": "pause", "source_id": "inbox", "expected_revision": 1 }),
    )
    .await
    .unwrap();
    round_at(&harness, 1_000 + DAY + 2 * MINUTE).await;
    assert!(open_escalations(&harness).await.is_empty());
    assert_eq!(
        super::rpc::status_projection(&harness.service).await["sources"],
        json!({ "active": 0, "paused": 1, "failing": 0, "stale": 0 })
    );
}

/// A health announcement reaches only the audience the pass chose when it
/// announced: a later subscription to the health topic starts after it,
/// whatever its start frontier says.
#[tokio::test]
async fn health_announcements_are_not_replayed_to_a_new_subscription() {
    let mut native = ScriptedNative::new();
    native.known.insert("agent:b".into());
    let harness = harness_with(native);
    inbox(&harness, None).await;
    subscribe_to_health(&harness, "agent:a").await;
    round_at(&harness, 1_000 + DAY).await;
    assert!(announced(&harness, "stale").await);
    try_host_call(
        &harness.service,
        SUBSCRIPTIONS_PUT,
        json!({
            "operation_id": "subscribe-health-b",
            "subscription_id": "health-b",
            "member": { "mob_id": OWN_MOB, "identity": "agent:b" },
            "topics": ["mob_announcements.health"],
            "start": { "kind": "after", "frontier": { "payments": 0 } },
        }),
    )
    .await
    .unwrap();
    assert_eq!(owed(&harness).await.get("agent:b"), None);
}

/// Items of a source whose id looks like another source's gap keys are no
/// gap of that other source: a gap is read from the source's own gap
/// notices.
#[tokio::test]
async fn items_of_a_source_named_like_a_gap_key_are_no_gap() {
    let harness = harness_with(ScriptedNative::new());
    inbox(&harness, None).await;
    let lookalike = "inbox:gap:1";
    try_host_call(
        &harness.service,
        SOURCES_PUT,
        source_params("put-lookalike", lookalike, "subject-2", &["mail.other"]),
    )
    .await
    .unwrap();
    for (operation, version, cursor, items) in [
        ("lookalike:baseline", 0, "c1", Vec::new()),
        (
            "lookalike:v1",
            1,
            "c2",
            vec![json!({
                "item_id": "m-1",
                "version": "v1",
                "change": "upsert",
                "topics": ["mail.other"],
                "body": "an item",
            })],
        ),
    ] {
        let mut params = json!({
            "operation_id": operation,
            "source_id": lookalike,
            "expected_revision": 1,
            "expected_version": version,
            "principal": principal_of("subject-2"),
            "next_cursor": cursor,
        });
        if items.is_empty() {
            params = with(params, "coverage", json!("complete"));
        } else {
            params = with_items(params, &items);
        }
        try_host_call(&harness.service, SOURCES_ADVANCE, params)
            .await
            .unwrap();
    }
    round_at(&harness, 1_000 + MINUTE).await;
    assert!(
        open_of(&harness, "source_gap").await.is_empty(),
        "{:?}",
        open_escalations(&harness).await
    );
}

/// members/list clamps its limit to a page: 0 still answers one member.
#[tokio::test]
async fn members_list_clamps_its_limit() {
    let mut native = ScriptedNative::new();
    native.known.insert("agent:b".into());
    let harness = harness_with(native);
    inbox(&harness, None).await;
    subscribe_to_inbox(&harness.service, "sub-a", "agent:a").await;
    subscribe_to_health(&harness, "agent:b").await;
    let listed = try_host_call(&harness.service, MEMBERS_LIST, json!({ "limit": 0 }))
        .await
        .unwrap();
    assert_eq!(
        listed["members"].as_array().map(Vec::len),
        Some(1),
        "{listed}"
    );
}

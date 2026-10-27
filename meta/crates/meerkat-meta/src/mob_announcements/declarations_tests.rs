//! Declarations and role-selected subscriptions: what a host declares is
//! reconciled into the records it manages and nothing else, and a role
//! template captures for the members that hold its role when an
//! announcement is made.

use super::*;

const DECLARATIONS_APPLY: &str = "meta/mob_announcements/declarations/apply";
const DECLARATIONS_GET: &str = "meta/mob_announcements/declarations/get";
const SUBSCRIPTIONS_GET: &str = "meta/mob_announcements/subscriptions/get";

/// Apply `declarations` at `revision`, keeping a refusal.
async fn try_apply(
    service: &AnnouncementService,
    revision: u64,
    sources: serde_json::Value,
    subscriptions: serde_json::Value,
) -> std::result::Result<serde_json::Value, crate::rpc::JsonRpcError> {
    try_host_call(
        service,
        DECLARATIONS_APPLY,
        serde_json::json!({
            "revision": revision,
            "declarations": {
                "revision": revision,
                "sources": sources,
                "subscriptions": subscriptions,
            },
        }),
    )
    .await
}

/// What `declarations/get` reports.
async fn declared(service: &AnnouncementService) -> serde_json::Value {
    try_host_call(service, DECLARATIONS_GET, serde_json::json!({}))
        .await
        .unwrap()
}

/// The record of `kind` and `id` in a `declarations/get` result.
fn record<'a>(view: &'a serde_json::Value, kind: &str, id: &str) -> Option<&'a serde_json::Value> {
    view["records"]
        .as_array()?
        .iter()
        .find(|record| record["kind"] == kind && record["id"] == id)
}

/// The role children of template `template` a `declarations/get` result
/// lists.
fn role_children(view: &serde_json::Value, template: &str) -> Vec<serde_json::Value> {
    let prefix = format!("{template}/");
    view["records"]
        .as_array()
        .map(|records| {
            records
                .iter()
                .filter(|record| {
                    record["kind"] == "role_child"
                        && record["id"]
                            .as_str()
                            .is_some_and(|id| id.starts_with(&prefix))
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// A subscription's state, or `None` when no subscription has the id.
async fn subscription_state(service: &AnnouncementService, id: &str) -> Option<String> {
    let found = try_host_call(
        service,
        SUBSCRIPTIONS_GET,
        serde_json::json!({ "subscription_id": id }),
    )
    .await
    .unwrap();
    found["subscription"]["state"].as_str().map(str::to_string)
}

/// A declared source: "inbox" on "mail.received" with this audience and a
/// direct route.
fn declared_inbox(audience: serde_json::Value) -> serde_json::Value {
    let mut source = with(
        with(
            source_params("unused", "inbox", "subject-1", &["mail.received"]),
            "audience",
            audience,
        ),
        "routes",
        serde_json::json!([{ "kind": "direct" }]),
    );
    source.as_object_mut().unwrap().remove("operation_id");
    source
}

/// A declared subscription of this mob's member `identity` to `topics`.
fn declared_subscription(id: &str, identity: &str, topics: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "subscription_id": id,
        "member": { "mob_id": OWN_MOB, "identity": identity },
        "topics": topics,
        "start": { "kind": "now" },
    })
}

/// A declared template: every member of this mob with `role`, on `topics`.
fn declared_template(id: &str, role: &str, topics: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "subscription_id": id,
        "members": { "mob_id": OWN_MOB, "role": role },
        "topics": topics,
        "start": { "kind": "now" },
    })
}

/// The host announces `key` on `topics` into this gateway's mob.
async fn host_announces(service: &AnnouncementService, key: &str, topics: &[&str]) {
    try_host_call(
        service,
        ANNOUNCE,
        serde_json::json!({ "source_key": key, "topics": topics, "body": key }),
    )
    .await
    .unwrap();
}

/// A declared subscription whose member its source's audience does not
/// cover is never made: the record is pending with the audience's refusal,
/// and nothing captures for the member.
#[tokio::test]
async fn a_declared_subscription_outside_its_source_audience_is_refused() {
    let harness = harness_with(native_serving(&["agent:b"]));
    let service = &harness.service;
    try_apply(
        service,
        1,
        serde_json::json!([declared_inbox(serde_json::json!([member_grant("agent:a")]))]),
        serde_json::json!([declared_subscription(
            "b-mail",
            "agent:b",
            &["mail.received"]
        )]),
    )
    .await
    .unwrap();
    let view = declared(service).await;
    assert_eq!(
        record(&view, "source", "inbox").map(|record| &record["status"]),
        Some(&serde_json::json!("applied")),
        "{view}"
    );
    let refused = record(&view, "subscription", "b-mail")
        .unwrap_or_else(|| panic!("the declared subscription is listed: {view}"));
    assert_eq!(refused["status"], "pending", "{view}");
    assert_eq!(refused["refusal"]["kind"], "audience_not_covered", "{view}");
    assert_eq!(subscription_state(service, "b-mail").await, None);
}

/// A role template takes a member that gets the role after the template
/// was declared: its first matching announcement is captured for it, under
/// a child named for the template and the member, and a member without
/// the role captures nothing.
#[tokio::test]
async fn a_role_template_captures_for_a_member_spawned_after_it() {
    let harness = harness_with(native_serving(&["agent:b", "agent:c"]));
    let service = &harness.service;
    try_apply(
        service,
        1,
        serde_json::json!([]),
        serde_json::json!([declared_template("reviewers", "reviewer", &["deploy.done"])]),
    )
    .await
    .unwrap();
    // The member is spawned with the role after the template.
    harness.native.set_roster_facts("agent:b", "reviewer", &[]);
    harness.native.set_roster_facts("agent:c", "author", &[]);
    host_announces(service, "deploy-1", &["deploy.done"]).await;
    let owed_now = owed(&harness).await;
    assert_eq!(owed_now.get("agent:b"), Some(&1), "{owed_now:?}");
    assert_eq!(owed_now.get("agent:c"), None, "{owed_now:?}");
    let view = declared(service).await;
    assert_eq!(
        record(&view, "subscription", "reviewers").map(|record| &record["status"]),
        Some(&serde_json::json!("applied")),
        "{view}"
    );
    let children = role_children(&view, "reviewers");
    assert_eq!(children.len(), 1, "{view}");
    let child = children[0]["id"].as_str().unwrap().to_string();
    let digest = child.strip_prefix("reviewers/").unwrap();
    assert!(
        digest.len() == 16 && digest.chars().all(|c| c.is_ascii_hexdigit()),
        "{child}"
    );
    assert_eq!(children[0]["status"], "applied", "{view}");
    assert_eq!(
        subscription_state(service, &child).await.as_deref(),
        Some("active")
    );
}

/// A member that loses the role stops capturing at once and keeps what it
/// already owes, which is delivered: its child stays active, capturing
/// nothing, and leaving the role never cancels it. A member given the role
/// again captures again, under the same child.
#[tokio::test]
async fn a_member_that_leaves_the_role_stops_capturing() {
    let harness = harness_with(native_serving(&["agent:b"]));
    let service = &harness.service;
    harness.native.set_roster_facts("agent:b", "reviewer", &[]);
    try_apply(
        service,
        1,
        serde_json::json!([]),
        serde_json::json!([declared_template("reviewers", "reviewer", &["deploy.done"])]),
    )
    .await
    .unwrap();
    host_announces(service, "deploy-1", &["deploy.done"]).await;
    assert_eq!(owed(&harness).await.get("agent:b"), Some(&1));
    let child = role_children(&declared(service).await, "reviewers")[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    // The member is respawned under another role.
    harness.native.set_roster_facts("agent:b", "author", &[]);
    host_announces(service, "deploy-2", &["deploy.done"]).await;
    assert_eq!(
        owed(&harness).await.get("agent:b"),
        Some(&1),
        "the second announcement is not captured; the first is still owed"
    );
    service.reconcile_mobs().await.unwrap();
    assert_eq!(owed(&harness).await.get("agent:b"), Some(&1));
    // What it owes is delivered.
    harness.clock.store(10_000_000, Ordering::SeqCst);
    service.run_round().await.unwrap();
    let delivered = service
        .deliveries(&BatchQuery {
            member: Some(member("agent:b")),
            limit: 10,
            ..BatchQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(delivered.len(), 1, "{delivered:?}");
    assert_eq!(delivered[0].items.len(), 1, "{delivered:?}");
    assert_eq!(
        delivered[0].state.tag(),
        BatchStateTag::Delivered,
        "{delivered:?}"
    );
    service.reconcile_mobs().await.unwrap();
    assert_eq!(
        subscription_state(service, &child).await.as_deref(),
        Some("active"),
        "leaving the role never cancels the child"
    );
    let children = role_children(&declared(service).await, "reviewers");
    assert_eq!(children.len(), 1, "{children:?}");
    assert_eq!(children[0]["status"], "left", "{children:?}");
    host_announces(service, "deploy-3", &["deploy.done"]).await;
    assert_eq!(
        owed(&harness).await.get("agent:b"),
        None,
        "a member without the role captures nothing"
    );
    // The member is respawned with the role again.
    harness.native.set_roster_facts("agent:b", "reviewer", &[]);
    host_announces(service, "deploy-4", &["deploy.done"]).await;
    assert_eq!(owed(&harness).await.get("agent:b"), Some(&1));
    let children = role_children(&declared(service).await, "reviewers");
    assert_eq!(children.len(), 1, "{children:?}");
    assert_eq!(children[0]["id"], child.as_str());
    assert_eq!(children[0]["status"], "applied", "{children:?}");
    assert_eq!(
        subscription_state(service, &child).await.as_deref(),
        Some("active")
    );
}

/// The reconciler changes only the records a declaration made: a record
/// dropped from the declaration is cancelled, and a subscription made at
/// runtime is never touched.
#[tokio::test]
async fn declarations_reconcile_only_what_they_manage() {
    let harness = harness_with(native_serving(&["agent:b"]));
    let service = &harness.service;
    try_subscribe(service, "made-at-runtime", "agent:a", &["ops.runtime"])
        .await
        .unwrap();
    try_apply(
        service,
        1,
        serde_json::json!([]),
        serde_json::json!([
            declared_subscription("declared-a", "agent:a", &["ops.declared"]),
            declared_subscription("declared-b", "agent:b", &["ops.declared"]),
        ]),
    )
    .await
    .unwrap();
    assert_eq!(
        subscription_state(service, "declared-a").await.as_deref(),
        Some("active")
    );
    try_apply(
        service,
        2,
        serde_json::json!([]),
        serde_json::json!([declared_subscription(
            "declared-b",
            "agent:b",
            &["ops.declared"]
        )]),
    )
    .await
    .unwrap();
    assert_eq!(
        subscription_state(service, "declared-a").await.as_deref(),
        Some("cancelled"),
        "a record dropped from the declaration is cancelled"
    );
    assert_eq!(
        subscription_state(service, "declared-b").await.as_deref(),
        Some("active")
    );
    assert_eq!(
        subscription_state(service, "made-at-runtime")
            .await
            .as_deref(),
        Some("active"),
        "a runtime record is never touched"
    );
    let view = declared(service).await;
    assert!(
        record(&view, "subscription", "declared-a").is_none(),
        "{view}"
    );
    assert!(
        record(&view, "subscription", "made-at-runtime").is_none(),
        "{view}"
    );
    assert_eq!(view["revision"], 2, "{view}");
}

/// A lower revision is refused as a rollback, the applied revision with
/// other declarations as a digest conflict, and the applied revision with
/// the same declarations is a no-op.
#[tokio::test]
async fn a_declaration_rollback_or_digest_conflict_is_refused() {
    let harness = harness_with(native_serving(&["agent:b"]));
    let service = &harness.service;
    let first = serde_json::json!([declared_subscription("declared-a", "agent:a", &["ops.one"])]);
    try_apply(service, 2, serde_json::json!([]), first.clone())
        .await
        .unwrap();
    let digest = declared(service).await["digest"].clone();
    assert!(
        digest.as_str().is_some_and(|digest| !digest.is_empty()),
        "{digest}"
    );
    let rollback = try_apply(service, 1, serde_json::json!([]), first.clone())
        .await
        .unwrap_err();
    assert_eq!(
        rollback.data,
        Some(serde_json::json!({ "kind": "declaration_rollback", "applied": 2 }))
    );
    let conflict = try_apply(
        service,
        2,
        serde_json::json!([]),
        serde_json::json!([declared_subscription("declared-a", "agent:a", &["ops.two"])]),
    )
    .await
    .unwrap_err();
    assert_eq!(
        conflict.data,
        Some(serde_json::json!({ "kind": "declaration_digest_conflict", "revision": 2 }))
    );
    try_apply(service, 2, serde_json::json!([]), first)
        .await
        .unwrap();
    let view = declared(service).await;
    assert_eq!(view["revision"], 2, "{view}");
    assert_eq!(view["digest"], digest, "{view}");
}

/// An apply whose declarations the static checks refuse is refused whole:
/// nothing it declares is made, nothing it drops is cancelled, and the
/// applied revision stands.
#[tokio::test]
async fn an_apply_the_static_checks_refuse_changes_nothing() {
    let harness = harness_with(native_serving(&["agent:b"]));
    let service = &harness.service;
    try_apply(
        service,
        1,
        serde_json::json!([]),
        serde_json::json!([declared_subscription("declared-a", "agent:a", &["ops.one"])]),
    )
    .await
    .unwrap();
    let before = declared(service).await;
    // Revision 2 drops declared-a and names declared-b twice.
    let refused = try_apply(
        service,
        2,
        serde_json::json!([]),
        serde_json::json!([
            declared_subscription("declared-b", "agent:b", &["ops.two"]),
            declared_subscription("declared-b", "agent:b", &["ops.three"]),
        ]),
    )
    .await
    .unwrap_err();
    assert_eq!(refused.code, -32602, "{refused:?}");
    assert_eq!(declared(service).await, before);
    assert_eq!(
        subscription_state(service, "declared-a").await.as_deref(),
        Some("active")
    );
    assert_eq!(subscription_state(service, "declared-b").await, None);
}

/// A declared template whose replace is refused is pending with the
/// refusal, and the children it made before are still listed: they still
/// capture.
#[tokio::test]
async fn a_pending_template_replace_keeps_its_children_listed() {
    let harness = harness_with(native_serving(&["agent:b"]));
    let service = &harness.service;
    harness.native.set_roster_facts("agent:b", "reviewer", &[]);
    try_apply(
        service,
        1,
        serde_json::json!([]),
        serde_json::json!([declared_template("reviewers", "reviewer", &["deploy.done"])]),
    )
    .await
    .unwrap();
    assert_eq!(
        role_children(&declared(service).await, "reviewers").len(),
        1
    );
    // Revision 2 names a mob this gateway does not host: the replace is
    // refused.
    let moved = with(
        declared_template("reviewers", "reviewer", &["deploy.done"]),
        "mobs",
        serde_json::json!(["mob-not-hosted"]),
    );
    try_apply(
        service,
        2,
        serde_json::json!([]),
        serde_json::json!([moved]),
    )
    .await
    .unwrap();
    let view = declared(service).await;
    assert_eq!(
        record(&view, "subscription", "reviewers").map(|record| &record["status"]),
        Some(&serde_json::json!("pending")),
        "{view}"
    );
    let children = role_children(&view, "reviewers");
    assert_eq!(children.len(), 1, "{view}");
    assert_eq!(children[0]["status"], "applied", "{view}");
}

/// Pause, resume or cancel the template `id` at `revision` as an operator
/// does, through the subscription verbs.
async fn template_change(
    service: &AnnouncementService,
    verb: &str,
    id: &str,
    revision: u64,
) -> serde_json::Value {
    try_host_call(
        service,
        &format!("meta/mob_announcements/subscriptions/{verb}"),
        serde_json::json!({
            "operation_id": format!("operator-{verb}-{id}-{revision}"),
            "subscription_id": id,
            "expected_revision": revision,
        }),
    )
    .await
    .unwrap()
}

/// The subscription verbs take a template's id and cascade to its children
/// in one step, the template's revision rising each time. A paused
/// template's children capture nothing and keep what they owe; a resumed
/// one's capture again; a cancelled one's end as a cancel ends a
/// subscription, and a declared template cancelled this way is suppressed.
#[tokio::test]
async fn a_template_pause_resume_and_cancel_cascade_to_its_children() {
    let harness = harness_with(native_serving(&["agent:b", "agent:c"]));
    let service = &harness.service;
    harness.native.set_roster_facts("agent:b", "reviewer", &[]);
    harness.native.set_roster_facts("agent:c", "reviewer", &[]);
    try_apply(
        service,
        1,
        serde_json::json!([]),
        serde_json::json!([declared_template("reviewers", "reviewer", &["deploy.done"])]),
    )
    .await
    .unwrap();
    host_announces(service, "deploy-1", &["deploy.done"]).await;
    let children: Vec<String> = role_children(&declared(service).await, "reviewers")
        .iter()
        .map(|child| child["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(children.len(), 2, "{children:?}");

    let paused = template_change(service, "pause", "reviewers", 1).await;
    assert_eq!(paused["template"]["state"], "paused", "{paused}");
    assert_eq!(paused["template"]["revision"], 2, "{paused}");
    for child in &children {
        assert_eq!(
            subscription_state(service, child).await.as_deref(),
            Some("paused")
        );
    }
    host_announces(service, "deploy-2", &["deploy.done"]).await;

    let resumed = template_change(service, "resume", "reviewers", 2).await;
    assert_eq!(resumed["template"]["state"], "active", "{resumed}");
    assert_eq!(resumed["template"]["revision"], 3, "{resumed}");
    for child in &children {
        assert_eq!(
            subscription_state(service, child).await.as_deref(),
            Some("active")
        );
    }
    let owed_now = owed(&harness).await;
    assert_eq!(
        (owed_now.get("agent:b"), owed_now.get("agent:c")),
        (Some(&1), Some(&1)),
        "nothing was captured while the template was paused: {owed_now:?}"
    );
    host_announces(service, "deploy-3", &["deploy.done"]).await;
    let owed_now = owed(&harness).await;
    assert_eq!(
        (owed_now.get("agent:b"), owed_now.get("agent:c")),
        (Some(&2), Some(&2)),
        "{owed_now:?}"
    );

    let cancelled = template_change(service, "cancel", "reviewers", 3).await;
    assert_eq!(cancelled["template"]["state"], "cancelled", "{cancelled}");
    assert_eq!(cancelled["template"]["revision"], 4, "{cancelled}");
    for child in &children {
        assert_eq!(
            subscription_state(service, child).await.as_deref(),
            Some("cancelled")
        );
    }
    assert_eq!(owed(&harness).await, BTreeMap::new(), "a cancel settles");
    service.reconcile_mobs().await.unwrap();
    let view = declared(service).await;
    assert_eq!(
        record(&view, "subscription", "reviewers").map(|record| &record["status"]),
        Some(&serde_json::json!("suppressed")),
        "{view}"
    );
}

/// A template replace re-admits each child with the checks of a put: a
/// child whose member the new selector's source audience leaves out is
/// pending with the refusal and captures nothing, while what it owes stays
/// owed; once its member is covered, the next trigger admits it again.
#[tokio::test]
async fn a_template_replace_re_admits_its_children() {
    let harness = harness_with(native_serving(&["agent:b", "agent:c"]));
    let service = &harness.service;
    harness
        .native
        .set_roster_facts("agent:b", "reviewer", &[("mail", "yes")]);
    harness.native.set_roster_facts("agent:c", "reviewer", &[]);
    let inbox = declared_inbox(serde_json::json!([
        { "kind": "label", "mob_id": OWN_MOB, "key": "mail", "value": "yes" }
    ]));
    try_apply(
        service,
        1,
        serde_json::json!([inbox]),
        serde_json::json!([declared_template("reviewers", "reviewer", &["deploy.done"])]),
    )
    .await
    .unwrap();
    host_announces(service, "deploy-1", &["deploy.done"]).await;
    let child_of = |identity: &str| {
        super::super::declarations::role_child_id(&sid("reviewers"), &member(identity))
            .unwrap()
            .to_string()
    };
    let status_of = |view: &serde_json::Value, identity: &str| {
        record(view, "role_child", &child_of(identity))
            .unwrap_or_else(|| panic!("the child of {identity} is listed: {view}"))
            .clone()
    };

    // Revision 2 adds the inbox's topic, whose audience covers b only.
    try_apply(
        service,
        2,
        serde_json::json!([inbox]),
        serde_json::json!([declared_template(
            "reviewers",
            "reviewer",
            &["deploy.done", "mail.received"]
        )]),
    )
    .await
    .unwrap();
    let view = declared(service).await;
    assert_eq!(status_of(&view, "agent:b")["status"], "applied", "{view}");
    let refused = status_of(&view, "agent:c");
    assert_eq!(refused["status"], "pending", "{view}");
    assert_eq!(refused["refusal"]["kind"], "audience_not_covered", "{view}");
    host_announces(service, "deploy-2", &["deploy.done"]).await;
    let owed_now = owed(&harness).await;
    assert_eq!(
        (owed_now.get("agent:b"), owed_now.get("agent:c")),
        (Some(&2), Some(&1)),
        "the pending child captures nothing and keeps what it owes: {owed_now:?}"
    );

    // c is labelled: the next trigger admits its child again.
    harness
        .native
        .set_roster_facts("agent:c", "reviewer", &[("mail", "yes")]);
    service.reconcile_mobs().await.unwrap();
    assert_eq!(
        status_of(&declared(service).await, "agent:c")["status"],
        "applied"
    );
    host_announces(service, "deploy-3", &["deploy.done"]).await;
    let owed_now = owed(&harness).await;
    assert_eq!(
        (owed_now.get("agent:b"), owed_now.get("agent:c")),
        (Some(&3), Some(&2)),
        "{owed_now:?}"
    );
}

/// A declared source an operator ended, whose changed declaration's next
/// generation is refused at runtime, stays pending at every trigger, and
/// starts once the refusal is gone.
#[tokio::test]
async fn a_declared_source_whose_restart_is_refused_stays_pending_and_is_retried() {
    let harness = harness_with(native_serving(&["agent:b"]));
    let service = &harness.service;
    try_apply(
        service,
        1,
        serde_json::json!([declared_inbox(serde_json::json!([member_grant("agent:a")]))]),
        serde_json::json!([]),
    )
    .await
    .unwrap();
    // An operator ends it, and a source made at runtime takes its identity.
    let revision = inbox(service).await["revision"].clone();
    try_host_call(
        service,
        SOURCES_END,
        serde_json::json!({
            "operation_id": "operator-end-inbox",
            "source_id": "inbox",
            "expected_revision": revision,
        }),
    )
    .await
    .unwrap();
    try_host_call(
        service,
        SOURCES_PUT,
        source_params("put-other", "other", "subject-1", &["mail.other"]),
    )
    .await
    .unwrap();
    // Revision 2 changes the declaration: its next generation is refused
    // while the other source holds the identity.
    try_apply(
        service,
        2,
        serde_json::json!([declared_inbox(serde_json::json!([
            member_grant("agent:a"),
            member_grant("agent:b"),
        ]))]),
        serde_json::json!([]),
    )
    .await
    .unwrap();
    for _ in 0..2 {
        service.reconcile_mobs().await.unwrap();
        let view = declared(service).await;
        let waiting = record(&view, "source", "inbox")
            .unwrap_or_else(|| panic!("the declared source is listed: {view}"));
        assert_eq!(waiting["status"], "pending", "{view}");
        assert_eq!(
            waiting["refusal"]["kind"], "source_identity_taken",
            "{view}"
        );
    }
    // Once the other source ends, the next trigger starts the declared one.
    let other = try_host_call(
        service,
        SOURCES_GET,
        serde_json::json!({ "source_id": "other" }),
    )
    .await
    .unwrap();
    try_host_call(
        service,
        SOURCES_END,
        serde_json::json!({
            "operation_id": "operator-end-other",
            "source_id": "other",
            "expected_revision": other["source"]["revision"],
        }),
    )
    .await
    .unwrap();
    service.reconcile_mobs().await.unwrap();
    let view = declared(service).await;
    assert_eq!(
        record(&view, "source", "inbox").map(|record| &record["status"]),
        Some(&serde_json::json!("applied")),
        "{view}"
    );
    let source = inbox(service).await;
    assert_eq!(source["generation"], 2, "{source}");
    assert_eq!(source["state"], "active", "{source}");
}

/// A role template's id is at most 200 bytes whichever way it is put, so
/// each child's id fits; a capture on its topics goes on.
#[tokio::test]
async fn a_role_template_id_over_the_bound_is_refused_and_captures_go_on() {
    let harness = harness_with(native_serving(&["agent:b"]));
    let service = &harness.service;
    harness.native.set_roster_facts("agent:b", "reviewer", &[]);
    let refused = try_host_call(
        service,
        SUBSCRIPTIONS_PUT,
        serde_json::json!({
            "operation_id": "put-long-template",
            "subscription_id": "t".repeat(250),
            "members": { "mob_id": OWN_MOB, "role": "reviewer" },
            "topics": ["deploy.done"],
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(refused.code, -32602, "{refused:?}");
    host_announces(service, "deploy-1", &["deploy.done"]).await;
}

/// A source advance resolves only the role templates its items can reach:
/// those on the source's own or forward topics.
#[tokio::test]
async fn a_source_advance_resolves_only_the_templates_its_topics_reach() {
    let harness = harness_with(native_serving(&["agent:b"]));
    let service = &harness.service;
    harness.native.set_roster_facts("agent:b", "reviewer", &[]);
    live_inbox(service).await;
    try_apply(
        service,
        1,
        serde_json::json!([]),
        serde_json::json!([declared_template("reviewers", "reviewer", &["deploy.done"])]),
    )
    .await
    .unwrap();
    let before = harness.native.role_reads();
    let revision = inbox(service).await["revision"].as_u64().unwrap();
    try_host_call(
        service,
        SOURCES_ADVANCE,
        with_items(
            advance_params("advance-1", revision, 1, "c2"),
            &[upsert("item-1", "v1", "a message")],
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        harness.native.role_reads(),
        before,
        "no roster read for a template the advance cannot reach"
    );
}

/// `subscriptions/get` reads a role template by its id: the template beside
/// no subscription.
#[tokio::test]
async fn subscriptions_get_reads_a_role_template() {
    let harness = harness_with(native_serving(&["agent:b"]));
    let service = &harness.service;
    try_host_call(
        service,
        SUBSCRIPTIONS_PUT,
        serde_json::json!({
            "operation_id": "put-reviewers",
            "subscription_id": "reviewers",
            "members": { "mob_id": OWN_MOB, "role": "reviewer" },
            "topics": ["deploy.done"],
        }),
    )
    .await
    .unwrap();
    let found = try_host_call(
        service,
        SUBSCRIPTIONS_GET,
        serde_json::json!({ "subscription_id": "reviewers" }),
    )
    .await
    .unwrap();
    assert!(found["subscription"].is_null(), "{found}");
    assert_eq!(found["template"]["id"], "reviewers", "{found}");
    assert_eq!(found["template"]["state"], "active", "{found}");
    assert_eq!(found["template"]["members"]["role"], "reviewer", "{found}");
}

/// A template's resume resumes only the children its own pause paused: a
/// child an operator paused by itself stays paused.
#[tokio::test]
async fn a_template_resume_leaves_a_child_its_operator_paused() {
    let harness = harness_with(native_serving(&["agent:b", "agent:c"]));
    let service = &harness.service;
    harness.native.set_roster_facts("agent:b", "reviewer", &[]);
    harness.native.set_roster_facts("agent:c", "reviewer", &[]);
    try_apply(
        service,
        1,
        serde_json::json!([]),
        serde_json::json!([declared_template("reviewers", "reviewer", &["deploy.done"])]),
    )
    .await
    .unwrap();
    host_announces(service, "deploy-1", &["deploy.done"]).await;
    let child_of = |identity: &str| {
        super::super::declarations::role_child_id(&sid("reviewers"), &member(identity))
            .unwrap()
            .to_string()
    };
    operator_changes(service, "pause", &child_of("agent:b")).await;
    template_change(service, "pause", "reviewers", 1).await;
    template_change(service, "resume", "reviewers", 2).await;
    assert_eq!(
        subscription_state(service, &child_of("agent:b"))
            .await
            .as_deref(),
        Some("paused"),
        "the operator's own pause holds"
    );
    assert_eq!(
        subscription_state(service, &child_of("agent:c"))
            .await
            .as_deref(),
        Some("active")
    );
}

/// Change the subscription `id` at its revision as an operator does:
/// `verb` is "pause", "resume" or "cancel".
async fn operator_changes(service: &AnnouncementService, verb: &str, id: &str) {
    let found = try_host_call(
        service,
        SUBSCRIPTIONS_GET,
        serde_json::json!({ "subscription_id": id }),
    )
    .await
    .unwrap();
    try_host_call(
        service,
        &format!("meta/mob_announcements/subscriptions/{verb}"),
        serde_json::json!({
            "operation_id": format!("operator-{verb}-{id}"),
            "subscription_id": id,
            "expected_revision": found["subscription"]["revision"],
        }),
    )
    .await
    .unwrap();
}

/// An operator's pause of a declared subscription holds. An operator's
/// cancel of it, or end of a declared source, holds as suppressed while the
/// declaration of that record is unchanged. A changed declaration starts
/// the source's next generation; the cancelled subscription stays
/// cancelled and its declaration waits as pending until retention removes
/// the cancelled record.
#[tokio::test]
async fn an_operators_pause_and_cancel_of_a_declared_record_hold() {
    let harness = harness_with(native_serving(&["agent:b"]));
    let service = &harness.service;
    let first_inbox = declared_inbox(serde_json::json!([member_grant("agent:a")]));
    let first = serde_json::json!([declared_subscription("declared-a", "agent:a", &["ops.one"])]);
    try_apply(service, 1, serde_json::json!([first_inbox]), first.clone())
        .await
        .unwrap();

    // A pause holds across the same declarations applied again and a
    // reconcile.
    operator_changes(service, "pause", "declared-a").await;
    try_apply(service, 1, serde_json::json!([first_inbox]), first)
        .await
        .unwrap();
    service.reconcile_mobs().await.unwrap();
    assert_eq!(
        subscription_state(service, "declared-a").await.as_deref(),
        Some("paused")
    );

    // A cancel and an end hold as suppressed.
    operator_changes(service, "cancel", "declared-a").await;
    let revision = inbox(service).await["revision"].clone();
    try_host_call(
        service,
        SOURCES_END,
        serde_json::json!({
            "operation_id": "operator-end-inbox",
            "source_id": "inbox",
            "expected_revision": revision,
        }),
    )
    .await
    .unwrap();
    service.reconcile_mobs().await.unwrap();
    let view = declared(service).await;
    for (kind, id) in [("source", "inbox"), ("subscription", "declared-a")] {
        assert_eq!(
            record(&view, kind, id).map(|record| &record["status"]),
            Some(&serde_json::json!("suppressed")),
            "{view}"
        );
    }
    assert_eq!(
        subscription_state(service, "declared-a").await.as_deref(),
        Some("cancelled")
    );
    assert_eq!(inbox(service).await["state"], "ended");

    // Revision 2 changes both declarations.
    try_apply(
        service,
        2,
        serde_json::json!([declared_inbox(serde_json::json!([
            member_grant("agent:a"),
            member_grant("agent:b"),
        ]))]),
        serde_json::json!([declared_subscription("declared-a", "agent:a", &["ops.two"])]),
    )
    .await
    .unwrap();
    let view = declared(service).await;
    assert_eq!(
        record(&view, "source", "inbox").map(|record| &record["status"]),
        Some(&serde_json::json!("applied")),
        "{view}"
    );
    let source = inbox(service).await;
    assert_eq!(source["generation"], 2, "{source}");
    assert_eq!(source["state"], "active", "{source}");
    let waiting = record(&view, "subscription", "declared-a")
        .unwrap_or_else(|| panic!("the declared subscription is listed: {view}"));
    assert_eq!(waiting["status"], "pending", "{view}");
    assert_eq!(
        waiting["refusal"]["kind"], "subscription_cancelled",
        "{view}"
    );
    assert_eq!(
        subscription_state(service, "declared-a").await.as_deref(),
        Some("cancelled")
    );

    // The next trigger retries the changed declaration: it is still pending,
    // never held as suppressed.
    service.reconcile_mobs().await.unwrap();
    let view = declared(service).await;
    let waiting = record(&view, "subscription", "declared-a")
        .unwrap_or_else(|| panic!("the declared subscription is listed: {view}"));
    assert_eq!(waiting["status"], "pending", "{view}");
    assert_eq!(
        waiting["refusal"]["kind"], "subscription_cancelled",
        "{view}"
    );
}

/// A managed subscription whose declaration changed is replaced in place
/// at its revision: what it captured before stays owed, the new selector
/// captures from now, and the old one captures no more.
#[tokio::test]
async fn a_replaced_managed_subscription_keeps_what_it_owes() {
    let harness = harness_with(native_serving(&[]));
    let service = &harness.service;
    try_apply(
        service,
        1,
        serde_json::json!([]),
        serde_json::json!([declared_subscription("declared-a", "agent:a", &["ops.one"])]),
    )
    .await
    .unwrap();
    host_announces(service, "one-1", &["ops.one"]).await;
    assert_eq!(owed(&harness).await.get("agent:a"), Some(&1));
    try_apply(
        service,
        2,
        serde_json::json!([]),
        serde_json::json!([declared_subscription("declared-a", "agent:a", &["ops.two"])]),
    )
    .await
    .unwrap();
    let found = try_host_call(
        service,
        SUBSCRIPTIONS_GET,
        serde_json::json!({ "subscription_id": "declared-a" }),
    )
    .await
    .unwrap();
    assert_eq!(found["subscription"]["revision"], 2, "{found}");
    assert_eq!(found["subscription"]["state"], "active", "{found}");
    assert_eq!(owed(&harness).await.get("agent:a"), Some(&1), "still owed");
    host_announces(service, "two-1", &["ops.two"]).await;
    host_announces(service, "one-2", &["ops.one"]).await;
    assert_eq!(
        owed(&harness).await.get("agent:a"),
        Some(&2),
        "the new selector captures; the old one no more"
    );
}

/// A declared subscription refused for a runtime reason is pending and is
/// retried at the next trigger: once its member is on the roster with the
/// role the audience grants, it is made.
#[tokio::test]
async fn a_pending_declared_subscription_is_retried_when_its_member_registers() {
    let harness = harness_with(native_serving(&["agent:b"]));
    let service = &harness.service;
    let audience = serde_json::json!([{ "kind": "role", "mob_id": OWN_MOB, "role": "triager" }]);
    try_apply(
        service,
        1,
        serde_json::json!([declared_inbox(audience)]),
        serde_json::json!([declared_subscription(
            "b-mail",
            "agent:b",
            &["mail.received"]
        )]),
    )
    .await
    .unwrap();
    let view = declared(service).await;
    let pending = record(&view, "subscription", "b-mail")
        .unwrap_or_else(|| panic!("the declared subscription is listed: {view}"));
    assert_eq!(pending["status"], "pending", "{view}");
    assert_eq!(subscription_state(service, "b-mail").await, None);
    // The member registers on the roster with the role.
    harness.native.set_roster_facts("agent:b", "triager", &[]);
    service.reconcile_mobs().await.unwrap();
    let view = declared(service).await;
    assert_eq!(
        record(&view, "subscription", "b-mail").map(|record| &record["status"]),
        Some(&serde_json::json!("applied")),
        "{view}"
    );
    assert_eq!(
        subscription_state(service, "b-mail").await.as_deref(),
        Some("active")
    );
}

/// The options take declarations inline or from a sidecar file the host
/// names, read once at init, never both; a sidecar that cannot be read or
/// parsed refuses the configuration.
#[test]
fn the_options_take_declarations_inline_or_from_a_sidecar() {
    let declarations = serde_json::json!({
        "revision": 3,
        "subscriptions": [declared_subscription("declared-a", "agent:a", &["ops.one"])],
    });
    let inline = super::super::parse_runtime_options(&serde_json::json!({
        "enabled": true,
        "declarations": declarations,
    }))
    .unwrap()
    .unwrap();
    let source = inline
        .declarations
        .as_deref()
        .expect("inline declarations are kept");
    assert_eq!(source.declarations().revision, 3);
    assert!(source.sidecar().is_none());

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("declarations.json");
    std::fs::write(&path, declarations.to_string()).unwrap();
    let sidecar = super::super::parse_runtime_options(&serde_json::json!({
        "enabled": true,
        "declarations_path": path,
    }))
    .unwrap()
    .unwrap();
    let source = sidecar
        .declarations
        .as_deref()
        .expect("the sidecar is read at init");
    assert_eq!(source.declarations().revision, 3);
    assert_eq!(source.sidecar(), Some(path.as_path()));

    let both = super::super::parse_runtime_options(&serde_json::json!({
        "enabled": true,
        "declarations": declarations,
        "declarations_path": path,
    }));
    assert!(both.is_err(), "one source of declarations per gateway");
    std::fs::write(&path, "{ not declarations").unwrap();
    let unparsed = super::super::parse_runtime_options(&serde_json::json!({
        "enabled": true,
        "declarations_path": path,
    }));
    assert!(
        unparsed.is_err(),
        "a sidecar that cannot be parsed fails closed"
    );
    let missing = super::super::parse_runtime_options(&serde_json::json!({
        "enabled": true,
        "declarations_path": dir.path().join("absent.json"),
    }));
    assert!(
        missing.is_err(),
        "a sidecar that cannot be read fails closed"
    );
}

/// The static checks refuse a whole declaration, whatever the runtime
/// holds: a stream route selector, a triage member its source's audience
/// leaves out, a topic two declared sources own (a forward topic too), a
/// member whose declared subscriptions together reach a declared source
/// both directly and through its forward, and a conversation member
/// subscribed to its own scope.
#[test]
fn a_declaration_the_static_checks_refuse_is_refused_whole() {
    let own = mob(OWN_MOB);
    let check = |value: serde_json::Value| {
        let declarations: super::super::declarations::Declarations =
            serde_json::from_value(value).unwrap();
        super::super::declarations::validate(&declarations, &own)
    };
    let streams = with(
        declared_inbox(serde_json::json!([member_grant("agent:a")])),
        "routes",
        serde_json::json!([{ "kind": "direct", "streams": ["INBOX"] }]),
    );
    let triage_outside = with(
        declared_inbox(serde_json::json!([member_grant("agent:a")])),
        "routes",
        serde_json::json!([
            { "kind": "direct" },
            triage_by("agent:t", serde_json::json!([member_grant("agent:a")])),
        ]),
    );
    // "agent:t" triages "mail.received" and forwards on "mail.triaged".
    let forwarding = with(
        declared_inbox(serde_json::json!([
            member_grant("agent:a"),
            member_grant("agent:t")
        ])),
        "routes",
        serde_json::json!([
            { "kind": "direct" },
            triage_by("agent:t", serde_json::json!([member_grant("agent:a")])),
        ]),
    );
    let mut triaged = with(
        source_params("unused", "triaged", "subject-2", &["mail.triaged"]),
        "routes",
        serde_json::json!([{ "kind": "direct" }]),
    );
    triaged.as_object_mut().unwrap().remove("operation_id");
    let conversation = with(
        declared_inbox(serde_json::json!([member_grant("agent:a")])),
        "conversation_members",
        serde_json::json!([{ "mob_id": OWN_MOB, "identity": "agent:a" }]),
    );
    let refusals = [
        (
            "a stream route selector",
            matches!(
                check(serde_json::json!({ "revision": 1, "sources": [streams] })),
                Err(AnnouncementStoreError::RouteSelectorUnsupported { .. })
            ),
        ),
        (
            "a triage member outside its source's audience",
            matches!(
                check(serde_json::json!({ "revision": 1, "sources": [triage_outside] })),
                Err(AnnouncementStoreError::AudienceNotCovered { .. })
            ),
        ),
        (
            "a forward topic another declared source owns",
            matches!(
                check(serde_json::json!({
                    "revision": 1,
                    "sources": [forwarding, triaged],
                })),
                Err(AnnouncementStoreError::TopicOwned { .. })
            ),
        ),
        (
            "a member reaching a declared source directly and through its forward",
            matches!(
                check(serde_json::json!({
                    "revision": 1,
                    "sources": [forwarding],
                    "subscriptions": [
                        declared_subscription("direct", "agent:a", &["mail.received"]),
                        declared_subscription("forwarded", "agent:a", &["mail.triaged"]),
                    ],
                })),
                Err(AnnouncementStoreError::RouteOverlap { .. })
            ),
        ),
        (
            "a conversation member subscribed to its own scope",
            matches!(
                check(serde_json::json!({
                    "revision": 1,
                    "sources": [conversation],
                    "subscriptions": [
                        declared_subscription("own-scope", "agent:a", &["mail.received"]),
                    ],
                })),
                Err(AnnouncementStoreError::ConversationMember { .. })
            ),
        ),
    ];
    let missed: Vec<&str> = refusals
        .iter()
        .filter(|(_, refused)| !refused)
        .map(|(what, _)| *what)
        .collect();
    assert!(missed.is_empty(), "the static checks admit: {missed:?}");
    let admitted = check(serde_json::json!({
        "revision": 1,
        "sources": [forwarding],
        "subscriptions": [
            declared_subscription("forwarded", "agent:a", &["mail.triaged"]),
            declared_subscription("triage", "agent:t", &["mail.received"]),
        ],
    }));
    assert!(
        admitted.is_ok(),
        "a declaration the checks admit: {admitted:?}"
    );
}

/// A declared subscription that names no mobs takes this gateway's own, as
/// `subscriptions/put` does, whichever mob its member or role is of.
#[test]
fn a_declared_subscription_names_this_gateways_mob_by_default() {
    let own = mob(OWN_MOB);
    let decode = |value: serde_json::Value| -> super::super::declarations::DeclaredSubscription {
        serde_json::from_value(value).unwrap()
    };
    let one = decode(serde_json::json!({
        "subscription_id": "child-member",
        "member": { "mob_id": "payments-child", "identity": "agent:c" },
        "topics": ["ops.one"],
    }));
    assert_eq!(one.mob_set(&own), BTreeSet::from([own.clone()]));
    let template = decode(serde_json::json!({
        "subscription_id": "child-role",
        "members": { "mob_id": "payments-child", "role": "reviewer" },
        "topics": ["ops.one"],
    }));
    assert_eq!(template.mob_set(&own), BTreeSet::from([own.clone()]));
}

/// A service on `harness`'s store, native owner and clock, whose gateway
/// takes its declarations from `source`, as a restarted gateway would.
fn service_on(
    harness: &Harness,
    source: super::super::declarations::DeclarationsSource,
) -> AnnouncementService {
    let reader = Arc::clone(&harness.clock);
    let config = AnnouncementConfig {
        policy: SealingPolicy {
            max_count: 256,
            max_bytes: 1_048_576,
            max_delay_ms: 60_000,
            min_wake_interval_ms: 60_000,
        },
        declarations: Some(Arc::new(source)),
        ..AnnouncementConfig::default()
    };
    AnnouncementService::with_clock(
        Arc::clone(&harness.store) as Arc<dyn AnnouncementStore>,
        Arc::clone(&harness.native) as Arc<dyn NativeDelivery>,
        config,
        Arc::new(move || reader.load(Ordering::SeqCst)),
    )
    .unwrap()
}

/// A harness whose gateway takes its declarations from `source`, before
/// anything reconciled them.
fn harness_configured(
    native: ScriptedNative,
    source: super::super::declarations::DeclarationsSource,
) -> Harness {
    let plain = harness_with(native);
    let service = service_on(&plain, source);
    Harness { service, ..plain }
}

/// A harness whose gateway takes its declarations from `source`, composed
/// as `compose` composes it: the declarations are reconciled before
/// anything runs.
async fn harness_declaring(
    native: ScriptedNative,
    source: super::super::declarations::DeclarationsSource,
) -> Harness {
    let harness = harness_configured(native, source);
    harness.service.reconcile_declarations().await.unwrap();
    harness
}

/// An apply follows the gateway's source of declarations. A gateway that
/// declares none, or declares them inline, applies what the call carries,
/// a revision and declarations both; one with a sidecar takes no
/// declarations in the call and reads the file again at each apply.
#[tokio::test]
async fn an_apply_follows_the_gateways_source_of_declarations() {
    let first = serde_json::json!({
        "revision": 1,
        "subscriptions": [declared_subscription("declared-a", "agent:a", &["ops.one"])],
    });
    let none = harness_with(native_serving(&[]));
    let empty = try_host_call(&none.service, DECLARATIONS_APPLY, serde_json::json!({}))
        .await
        .unwrap_err();
    assert_eq!(empty.code, -32602, "nothing to apply: {empty:?}");

    let inline = harness_declaring(
        native_serving(&[]),
        super::super::declarations::DeclarationsSource::Inline(
            serde_json::from_value(first.clone()).unwrap(),
        ),
    )
    .await;
    assert_eq!(
        subscription_state(&inline.service, "declared-a")
            .await
            .as_deref(),
        Some("active"),
        "inline declarations are reconciled at compose"
    );
    let empty = try_host_call(&inline.service, DECLARATIONS_APPLY, serde_json::json!({}))
        .await
        .unwrap_err();
    assert_eq!(
        empty.code, -32602,
        "an inline gateway's apply carries its declarations: {empty:?}"
    );

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("declarations.json");
    std::fs::write(&path, first.to_string()).unwrap();
    let harness = harness_declaring(
        native_serving(&[]),
        super::super::declarations::DeclarationsSource::Sidecar {
            path: path.clone(),
            declarations: serde_json::from_value(first).unwrap(),
        },
    )
    .await;
    let service = &harness.service;
    assert_eq!(
        subscription_state(service, "declared-a").await.as_deref(),
        Some("active"),
        "the sidecar read at init is reconciled at compose"
    );
    let carried = try_apply(service, 2, serde_json::json!([]), serde_json::json!([]))
        .await
        .unwrap_err();
    assert_eq!(
        carried.code, -32602,
        "a sidecar gateway's apply carries no declarations: {carried:?}"
    );
    assert_eq!(declared(service).await["revision"], 1);
    std::fs::write(
        &path,
        serde_json::json!({
            "revision": 2,
            "subscriptions": [declared_subscription("declared-b", "agent:a", &["ops.two"])],
        })
        .to_string(),
    )
    .unwrap();
    try_host_call(service, DECLARATIONS_APPLY, serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(declared(service).await["revision"], 2);
    assert_eq!(
        subscription_state(service, "declared-a").await.as_deref(),
        Some("cancelled"),
        "the file read again drops what it no longer declares"
    );
    assert_eq!(
        subscription_state(service, "declared-b").await.as_deref(),
        Some("active")
    );
}

/// The dispatcher starts once the declarations are reconciled, so its first
/// round already runs on what they made: while the reconcile is held the
/// dispatcher waits; a reconcile still running after the bound lets it
/// start anyway; and a stop asked for meanwhile holds.
#[tokio::test]
async fn the_dispatcher_starts_once_the_declarations_are_reconciled() {
    let declarations = serde_json::json!({
        "revision": 1,
        "subscriptions": [declared_subscription("declared-a", "agent:a", &["ops.one"])],
    });
    let inline = || {
        super::super::declarations::DeclarationsSource::Inline(
            serde_json::from_value(declarations.clone()).unwrap(),
        )
    };
    let harness = harness_configured(native_serving(&[]), inline());
    let held = harness.service.hold_declarations().await;
    let starting = tokio::spawn({
        let service = harness.service.clone();
        async move {
            drop(
                service
                    .start_after_reconcile(std::time::Duration::from_secs(30))
                    .await,
            );
        }
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(
        !starting.is_finished(),
        "the dispatcher waits for the reconcile"
    );
    drop(held);
    starting.await.unwrap();
    assert_eq!(
        subscription_state(&harness.service, "declared-a")
            .await
            .as_deref(),
        Some("active"),
        "the first round sees the declared subscription"
    );
    assert!(!harness.service.is_stopped());

    let bounded = harness_configured(native_serving(&[]), inline());
    let held = bounded.service.hold_declarations().await;
    let started = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        bounded
            .service
            .start_after_reconcile(std::time::Duration::from_millis(100)),
    )
    .await;
    assert!(
        started.is_ok(),
        "a reconcile past the bound does not hold the dispatcher"
    );
    assert!(!bounded.service.is_stopped());
    drop(held);

    let stopping = harness_configured(native_serving(&[]), inline());
    let held = stopping.service.hold_declarations().await;
    let starting = tokio::spawn({
        let service = stopping.service.clone();
        async move {
            drop(
                service
                    .start_after_reconcile(std::time::Duration::from_secs(30))
                    .await,
            );
        }
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    stopping.service.stop();
    drop(held);
    starting.await.unwrap();
    assert!(
        stopping.service.is_stopped(),
        "a stop asked for before the dispatcher started holds"
    );
}

/// Options naming the applied revision with other declarations change
/// nothing, as an apply at runtime moved the gateway past them: the gateway
/// warns once per boot, naming the revision and both digests, and keeps the
/// applied declarations.
#[tokio::test]
async fn options_at_the_applied_revision_with_other_content_warn_once() {
    let harness = harness_with(native_serving(&[]));
    try_apply(
        &harness.service,
        2,
        serde_json::json!([]),
        serde_json::json!([declared_subscription("declared-a", "agent:a", &["ops.one"])]),
    )
    .await
    .unwrap();
    let applied = declared(&harness.service).await["digest"]
        .as_str()
        .unwrap()
        .to_string();
    let options: super::super::declarations::Declarations =
        serde_json::from_value(serde_json::json!({
            "revision": 2,
            "subscriptions": [declared_subscription("declared-a", "agent:a", &["ops.two"])],
        }))
        .unwrap();
    let offered = options.digest(&mob(OWN_MOB)).unwrap();
    assert_ne!(offered, applied);
    let log = CapturedLog::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_writer(move || writer.clone())
        .finish();
    let _logging = tracing::subscriber::set_default(subscriber);
    let restarted = service_on(
        &harness,
        super::super::declarations::DeclarationsSource::Inline(options),
    );
    restarted.reconcile_declarations().await.unwrap();
    restarted.reconcile_declarations().await.unwrap();
    let logged = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    let warnings: Vec<&str> = logged
        .lines()
        .filter(|line| line.contains(&offered))
        .collect();
    assert_eq!(warnings.len(), 1, "one warning per boot: {logged}");
    assert!(
        warnings[0].contains("WARN") && warnings[0].contains(&applied),
        "{logged}"
    );
    assert_eq!(declared(&restarted).await["digest"], applied.as_str());
    let found = try_host_call(
        &restarted,
        SUBSCRIPTIONS_GET,
        serde_json::json!({ "subscription_id": "declared-a" }),
    )
    .await
    .unwrap();
    assert_eq!(
        found["subscription"]["selector"]["topics"],
        serde_json::json!(["ops.one"]),
        "{found}"
    );
}

/// A subscription and a role template never share an id: each refuses the
/// id the other holds, so the subscription verbs never meet a collision.
#[tokio::test]
async fn a_subscription_and_a_role_template_never_share_an_id() {
    let harness = harness_with(native_serving(&[]));
    let service = &harness.service;
    try_host_call(
        service,
        SUBSCRIPTIONS_PUT,
        serde_json::json!({
            "operation_id": "template",
            "subscription_id": "shared",
            "members": { "mob_id": OWN_MOB, "role": "reviewer" },
            "topics": ["ops.one"],
        }),
    )
    .await
    .unwrap();
    let refused = try_host_call(
        service,
        SUBSCRIPTIONS_PUT,
        serde_json::json!({
            "operation_id": "subscription",
            "subscription_id": "shared",
            "member": { "mob_id": OWN_MOB, "identity": "agent:a" },
            "topics": ["ops.one"],
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(refused.code, -32047, "{refused:?}");
    assert_eq!(
        refused.data,
        Some(serde_json::json!({ "kind": "subscription_id_taken", "subscription_id": "shared" }))
    );
    try_subscribe(service, "plain", "agent:a", &["ops.two"])
        .await
        .unwrap();
    let reverse = try_host_call(
        service,
        SUBSCRIPTIONS_PUT,
        serde_json::json!({
            "operation_id": "template-2",
            "subscription_id": "plain",
            "members": { "mob_id": OWN_MOB, "role": "reviewer" },
            "topics": ["ops.two"],
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(reverse.code, -32047, "{reverse:?}");
    assert_eq!(
        reverse.data,
        Some(serde_json::json!({ "kind": "subscription_id_taken", "subscription_id": "plain" }))
    );
}

/// Two declared sources reading one scope of one principal are refused
/// whole, as two declared sources owning one topic are; the refusal names
/// the source declared first, which holds the scope.
#[test]
fn two_declared_sources_on_one_identity_scope_are_refused_whole() {
    let first = declared_inbox(serde_json::json!([member_grant("agent:a")]));
    let mut second = with(
        source_params("unused", "inbox-2", "subject-1", &["mail.other"]),
        "routes",
        serde_json::json!([{ "kind": "direct" }]),
    );
    second.as_object_mut().unwrap().remove("operation_id");
    let declarations: super::super::declarations::Declarations =
        serde_json::from_value(serde_json::json!({
            "revision": 1,
            "sources": [first, second],
        }))
        .unwrap();
    let refused = super::super::declarations::validate(&declarations, &mob(OWN_MOB));
    assert!(
        matches!(
            &refused,
            Err(AnnouncementStoreError::SourceIdentityTaken { source_id })
                if source_id.as_str() == "inbox"
        ),
        "{refused:?}"
    );
}

/// A declared record whose id a record of the other kind made at runtime
/// holds is pending as unmanaged, as one of its own kind would be, and the
/// runtime record is never touched.
#[tokio::test]
async fn a_declared_record_on_an_id_a_runtime_record_of_the_other_kind_holds_is_unmanaged() {
    let harness = harness_with(native_serving(&[]));
    let service = &harness.service;
    try_host_call(
        service,
        SUBSCRIPTIONS_PUT,
        serde_json::json!({
            "operation_id": "template",
            "subscription_id": "held-by-template",
            "members": { "mob_id": OWN_MOB, "role": "reviewer" },
            "topics": ["ops.one"],
        }),
    )
    .await
    .unwrap();
    try_subscribe(service, "held-by-subscription", "agent:a", &["ops.two"])
        .await
        .unwrap();
    try_apply(
        service,
        1,
        serde_json::json!([]),
        serde_json::json!([
            declared_subscription("held-by-template", "agent:a", &["ops.one"]),
            declared_template("held-by-subscription", "reviewer", &["ops.two"]),
        ]),
    )
    .await
    .unwrap();
    let view = declared(service).await;
    for id in ["held-by-template", "held-by-subscription"] {
        let held =
            record(&view, "subscription", id).unwrap_or_else(|| panic!("{id} is listed: {view}"));
        assert_eq!(held["status"], "pending", "{id}: {view}");
        assert_eq!(held["refusal"]["kind"], "unmanaged_record", "{id}: {view}");
    }
    assert_eq!(
        subscription_state(service, "held-by-template").await,
        None,
        "no subscription takes the template's id"
    );
    assert_eq!(
        subscription_state(service, "held-by-subscription")
            .await
            .as_deref(),
        Some("active"),
        "the runtime subscription is never touched"
    );
}

/// A declared record pending with a runtime refusal raises one
/// `declaration_pending` escalation about it, and applying the record
/// resolves it.
#[tokio::test]
async fn a_pending_declared_record_raises_declaration_pending_until_it_applies() {
    let harness = harness_with(native_serving(&["agent:b"]));
    let service = &harness.service;
    let audience = serde_json::json!([{ "kind": "role", "mob_id": OWN_MOB, "role": "triager" }]);
    try_apply(
        service,
        1,
        serde_json::json!([declared_inbox(audience)]),
        serde_json::json!([declared_subscription(
            "b-mail",
            "agent:b",
            &["mail.received"]
        )]),
    )
    .await
    .unwrap();
    let subject = serde_json::json!({
        "kind": "declaration",
        "record_kind": "subscription",
        "id": "b-mail",
    });
    let open = |escalations: &serde_json::Value| -> Vec<serde_json::Value> {
        escalations["escalations"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|escalation| {
                escalation["cause"] == "declaration_pending" && escalation["subject"] == subject
            })
            .collect()
    };
    let escalations = try_host_call(
        service,
        "meta/mob_announcements/escalations/list",
        serde_json::json!({ "open": true }),
    )
    .await
    .unwrap();
    assert_eq!(open(&escalations).len(), 1, "{escalations}");
    // The member registers on the roster with the role, and the record applies.
    harness.native.set_roster_facts("agent:b", "triager", &[]);
    service.reconcile_mobs().await.unwrap();
    let escalations = try_host_call(
        service,
        "meta/mob_announcements/escalations/list",
        serde_json::json!({ "open": true }),
    )
    .await
    .unwrap();
    assert!(open(&escalations).is_empty(), "{escalations}");
}

/// A declared source naming the reserved health topic, as a topic or as a
/// triage route's forward topic, is refused whole, and nothing applies.
#[tokio::test]
async fn a_declared_source_naming_the_health_topic_is_refused_whole() {
    let harness = harness_with(native_serving(&[]));
    let service = &harness.service;
    let mut on_health = declared_inbox(serde_json::json!([member_grant("agent:a")]));
    on_health["topics"] = serde_json::json!(["mob_announcements.health"]);
    let refused = try_apply(
        service,
        1,
        serde_json::json!([on_health]),
        serde_json::json!([]),
    )
    .await
    .unwrap_err();
    assert_eq!(refused.code, -32602, "{refused:?}");
    let mut forwarding = with(
        declared_inbox(serde_json::json!([
            member_grant("agent:a"),
            member_grant("agent:t")
        ])),
        "routes",
        serde_json::json!([
            { "kind": "direct" },
            triage_by("agent:t", serde_json::json!([member_grant("agent:a")])),
        ]),
    );
    forwarding["routes"][1]["forward_topics"] = serde_json::json!(["mob_announcements.health"]);
    let refused = try_apply(
        service,
        1,
        serde_json::json!([forwarding]),
        serde_json::json!([]),
    )
    .await
    .unwrap_err();
    assert_eq!(refused.code, -32602, "{refused:?}");
    assert_eq!(declared(service).await["revision"], 0);
}

/// A pending declared record is retried at the health pass every round
/// runs: once its member registers with the role, the next round makes it.
#[tokio::test]
async fn a_pending_declared_record_is_retried_at_the_health_pass() {
    let harness = harness_with(native_serving(&["agent:b"]));
    let service = &harness.service;
    let audience = serde_json::json!([{ "kind": "role", "mob_id": OWN_MOB, "role": "triager" }]);
    try_apply(
        service,
        1,
        serde_json::json!([declared_inbox(audience)]),
        serde_json::json!([declared_subscription(
            "b-mail",
            "agent:b",
            &["mail.received"]
        )]),
    )
    .await
    .unwrap();
    assert_eq!(subscription_state(service, "b-mail").await, None);
    harness.native.set_roster_facts("agent:b", "triager", &[]);
    service.run_round().await.unwrap();
    let view = declared(service).await;
    assert_eq!(
        record(&view, "subscription", "b-mail").map(|record| &record["status"]),
        Some(&serde_json::json!("applied")),
        "{view}"
    );
    assert_eq!(
        subscription_state(service, "b-mail").await.as_deref(),
        Some("active")
    );
}

/// A declared source's collection reads its health as it stands at the
/// read: a live source no step has advanced for longer than its stale
/// window is stale.
#[tokio::test]
async fn a_declared_source_past_its_stale_window_reads_stale() {
    let harness = harness_with(native_serving(&[]));
    let service = &harness.service;
    try_apply(
        service,
        1,
        serde_json::json!([declared_inbox(serde_json::json!([member_grant("agent:a")]))]),
        serde_json::json!([]),
    )
    .await
    .unwrap();
    let baseline = with(
        advance_params("baseline", 1, 0, "cursor-1"),
        "coverage",
        serde_json::json!("complete"),
    );
    try_host_call(service, SOURCES_ADVANCE, baseline)
        .await
        .unwrap();
    let view = declared(service).await;
    assert_eq!(
        record(&view, "source", "inbox").map(|record| &record["collection"]),
        Some(&serde_json::json!("live")),
        "{view}"
    );
    // Two days on, past the default one-day stale window.
    harness
        .clock
        .fetch_add(2 * 24 * 60 * 60 * 1000, Ordering::SeqCst);
    let view = declared(service).await;
    assert_eq!(
        record(&view, "source", "inbox").map(|record| &record["collection"]),
        Some(&serde_json::json!("stale")),
        "{view}"
    );
}

/// A declared subscription's delivery reaches the subscription it makes:
/// `{"await_handling": true}` as it is, and a delivery naming nothing (`{}`)
/// the same as none, the default, with the same digest.
#[tokio::test]
async fn a_declared_subscription_carries_its_delivery() {
    let harness = harness_with(native_serving(&["agent:b"]));
    let service = &harness.service;
    let mut waiting = declared_subscription("waits", "agent:a", &["ops.one"]);
    waiting["delivery"] = serde_json::json!({ "await_handling": true });
    let mut empty = declared_subscription("plain", "agent:b", &["ops.two"]);
    empty["delivery"] = serde_json::json!({});
    try_apply(
        service,
        1,
        serde_json::json!([]),
        serde_json::json!([waiting.clone(), empty]),
    )
    .await
    .unwrap();
    let found = try_host_call(
        service,
        SUBSCRIPTIONS_GET,
        serde_json::json!({ "subscription_id": "waits" }),
    )
    .await
    .unwrap();
    assert_eq!(
        found["subscription"]["delivery"],
        serde_json::json!({ "await_handling": true }),
        "{found}"
    );
    let found = try_host_call(
        service,
        SUBSCRIPTIONS_GET,
        serde_json::json!({ "subscription_id": "plain" }),
    )
    .await
    .unwrap();
    assert!(
        found["subscription"].get("delivery").is_none(),
        "an empty delivery is the default: {found}"
    );
    // The same revision with the empty delivery left out is the same
    // declaration: no digest conflict.
    try_apply(
        service,
        1,
        serde_json::json!([]),
        serde_json::json!([
            waiting,
            declared_subscription("plain", "agent:b", &["ops.two"])
        ]),
    )
    .await
    .unwrap();
}

/// A declared role template's delivery reaches every child it makes.
#[tokio::test]
async fn a_declared_template_gives_its_children_its_delivery() {
    let harness = harness_with(native_serving(&["agent:b"]));
    let service = &harness.service;
    harness.native.set_roster_facts("agent:b", "reviewer", &[]);
    let mut template = declared_template("reviewers", "reviewer", &["deploy.done"]);
    template["delivery"] = serde_json::json!({ "await_handling": false });
    try_apply(
        service,
        1,
        serde_json::json!([]),
        serde_json::json!([template]),
    )
    .await
    .unwrap();
    host_announces(service, "deploy-1", &["deploy.done"]).await;
    let child = role_children(&declared(service).await, "reviewers")[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let found = try_host_call(
        service,
        SUBSCRIPTIONS_GET,
        serde_json::json!({ "subscription_id": child }),
    )
    .await
    .unwrap();
    assert_eq!(
        found["subscription"]["delivery"],
        serde_json::json!({ "await_handling": false }),
        "{found}"
    );
}

/// The host's `subscriptions/put` with `members` takes a delivery beside
/// them: the template keeps it, and its children carry it.
#[tokio::test]
async fn a_host_template_put_gives_its_children_its_delivery() {
    let harness = harness_with(native_serving(&["agent:b"]));
    let service = &harness.service;
    harness.native.set_roster_facts("agent:b", "reviewer", &[]);
    let put = try_host_call(
        service,
        SUBSCRIPTIONS_PUT,
        serde_json::json!({
            "operation_id": "template",
            "subscription_id": "reviewers",
            "members": { "mob_id": OWN_MOB, "role": "reviewer" },
            "topics": ["deploy.done"],
            "delivery": { "await_handling": true },
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        put["subscription"]["delivery"],
        serde_json::json!({ "await_handling": true }),
        "{put}"
    );
    host_announces(service, "deploy-1", &["deploy.done"]).await;
    let found = try_host_call(
        service,
        SUBSCRIPTIONS_GET,
        serde_json::json!({ "subscription_id": "reviewers" }),
    )
    .await
    .unwrap();
    assert_eq!(
        found["template"]["delivery"],
        serde_json::json!({ "await_handling": true }),
        "{found}"
    );
    let listed = try_host_call(
        service,
        "meta/mob_announcements/subscriptions/list",
        serde_json::json!({}),
    )
    .await
    .unwrap();
    let child = listed["subscriptions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|subscription| {
            subscription["id"]
                .as_str()
                .is_some_and(|id| id.starts_with("reviewers/"))
        })
        .cloned()
        .unwrap_or_else(|| panic!("a child is made: {listed}"));
    assert_eq!(
        child["delivery"],
        serde_json::json!({ "await_handling": true }),
        "{child}"
    );
}

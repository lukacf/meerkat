//! `agent.view` on stored timeline frames, decided by the member provenance
//! the console owner captured with each frame (most restrictive rule).

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use super::*;
use crate::access::{AccessControlConfig, AccessController, AccessEffect, AccessRule};

const ROOT: &str = "root@example.test";
const CALLER: &str = "caller@example.test";

/// Everyone may view every agent; `tier=secret` members are hidden from
/// everyone but the admin.
fn secret_label_deny() -> AccessRule {
    AccessRule {
        id: "hide-secret".to_string(),
        effect: AccessEffect::Deny,
        actions: vec!["agent.view".to_string()],
        match_labels: BTreeMap::from([("tier".to_string(), "secret".to_string())]),
        ..AccessRule::default()
    }
}

fn controller(deny: Option<AccessRule>) -> AccessController {
    let mut rules = vec![AccessRule {
        id: "everyone-views".to_string(),
        actions: vec!["agent.view".to_string()],
        agents: vec!["*".to_string()],
        ..AccessRule::default()
    }];
    rules.extend(deny);
    AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules,
        ..AccessControlConfig::default()
    })
    .expect("controller")
}

fn link(identity: &str, tier: Option<&str>) -> ConsoleFrameLineageLink {
    ConsoleFrameLineageLink {
        identity: identity.to_string(),
        agent_id: tier.map(|_| identity.to_string()),
        role: tier.map(|_| "worker".to_string()),
        labels: tier.map(|tier| BTreeMap::from([("tier".to_string(), tier.to_string())])),
    }
}

fn provenance(
    runtime_key: &str,
    identity: &str,
    tier: &str,
    ancestors: Vec<ConsoleFrameLineageLink>,
) -> ConsoleFrameMemberProvenance {
    let record = ConsoleIdentityRecord {
        identity: identity.to_string(),
        display_name: identity.to_string(),
        runtime_key: runtime_key.to_string(),
        runtime_member_id: identity.to_string(),
        session_id: Some(format!("{identity}-session")),
        member_id: None,
        visibility: ConsoleVisibility::RetiredReadable,
        addressable: false,
        health: "retired".to_string(),
        topology_peers: Vec::new(),
        labels: BTreeMap::from([("tier".to_string(), tier.to_string())]),
    };
    let mut lineage = vec![link(identity, Some(tier))];
    lineage.extend(ancestors);
    ConsoleFrameMemberProvenance {
        member: ConsoleMember {
            agent_identity: identity.to_string(),
            role: "worker".to_string(),
            state: "retired".to_string(),
            model_capabilities: Default::default(),
            runtime_mode: None,
            session_id: record.session_id.clone(),
            wired_to: Vec::new(),
            labels: record.labels.clone(),
            progress: None,
        },
        primary_mob_id: "access-mob".to_string(),
        source_mob_id: "access-mob".to_string(),
        lineage,
        identity: record,
    }
}

fn member_frame(
    runtime_key: &str,
    key: &str,
    identity: &str,
    provenance: Option<ConsoleFrameMemberProvenance>,
) -> NewConsoleFrame {
    NewConsoleFrame {
        id: None,
        dedupe_key: key.to_string(),
        timestamp_ms: 1,
        runtime_key: runtime_key.to_string(),
        identity: identity.to_string(),
        conversation_id: Some(identity.to_string()),
        session_id: Some(format!("{identity}-session")),
        kind: "text_complete".to_string(),
        status: ConsoleFrameStatus::Completed,
        payload: json!({ "text": key }),
        source: ConsoleFrameSource {
            member_provenance: provenance,
            kind: ConsoleFrameSourceKind::SessionHistory,
            source_cursor: None,
        },
        source_event_id: None,
        interaction_id: None,
        turn_id: None,
        run_id: None,
        parent_frame_id: None,
        caused_by_frame_id: None,
    }
}

fn recent(limit: usize) -> ConsoleTimelineWindowQuery {
    ConsoleTimelineWindowQuery {
        mode: ConsoleTimelineMode::Recent,
        limit,
        ..ConsoleTimelineWindowQuery::default()
    }
}

/// The identities of the frames `subject` reads from one window page.
async fn window_identities(
    aggregator: &MetaConsoleAggregator,
    controller: &AccessController,
    subject: Option<&str>,
) -> Vec<String> {
    let decider = TimelineFrameDecider::new(controller.view_for_subject(subject));
    let filter: &TimelineFrameFilter<'_> = &|frame: &ConsoleFrame| decider.visible(frame);
    aggregator
        .query_timeline_windowed_for(recent(200), Some(filter))
        .await
        .expect("window")
        .frames
        .into_iter()
        .map(|frame| frame.identity)
        .collect()
}

/// The stream's decision on every stored frame, as the stream makes it on
/// each snapshot frame and live broadcast before projecting it.
async fn stream_identities(
    aggregator: &MetaConsoleAggregator,
    controller: &AccessController,
    subject: Option<&str>,
) -> Vec<String> {
    let view = controller.view_for_subject(subject);
    aggregator
        .query_timeline_windowed_stored_for(recent(200), None)
        .await
        .expect("stored window")
        .frames
        .into_iter()
        .filter(|frame| timeline_frame_visible_to(&view, frame))
        .map(|frame| frame.identity)
        .collect()
}

/// Restart path: a member labelled into the denied class wrote frames and
/// left; the store reopens under a new owner whose access cache knows
/// nothing of it. Its frames stay hidden from the denied caller on the
/// window (and the RPC page, which runs the same query) and the stream.
#[tokio::test]
async fn a_retired_denied_member_stays_hidden_after_the_store_reopens() {
    let temp = tempfile::tempdir().expect("temp dir");
    let path = temp.path().join("console.sqlite");
    {
        let store = SqliteConsoleLogStore::open(&path).expect("store");
        store
            .append_if_absent(member_frame(
                "runtime-a",
                "secret-frame",
                "gone-secret",
                Some(provenance("runtime-a", "gone-secret", "secret", Vec::new())),
            ))
            .await
            .expect("secret frame");
        store
            .append_if_absent(member_frame(
                "runtime-a",
                "public-frame",
                "gone-public",
                Some(provenance("runtime-a", "gone-public", "public", Vec::new())),
            ))
            .await
            .expect("public frame");
    }
    let aggregator = MetaConsoleAggregator::new(Arc::new(
        SqliteConsoleLogStore::open(&path).expect("reopened store"),
    ));
    let controller = controller(Some(secret_label_deny()));
    assert!(!controller.view_for_subject(None).knows_agent("gone-secret"));
    for subject in [None, Some(CALLER)] {
        assert_eq!(
            window_identities(&aggregator, &controller, subject).await,
            vec!["gone-public".to_string()]
        );
        assert_eq!(
            stream_identities(&aggregator, &controller, subject).await,
            vec!["gone-public".to_string()]
        );
    }
    // Not vacuous: the admin reads both members' frames.
    assert_eq!(
        window_identities(&aggregator, &controller, Some(ROOT)).await,
        vec!["gone-secret".to_string(), "gone-public".to_string()]
    );
}

/// A member frame without captured provenance fails closed under a label
/// deny, and keeps its identity decision under an allow-only policy.
#[tokio::test]
async fn an_uncaptured_member_frame_fails_closed_only_under_an_undecidable_deny() {
    let aggregator = MetaConsoleAggregator::in_memory();
    aggregator
        .store()
        .append_if_absent(member_frame("runtime-a", "legacy", "legacy-member", None))
        .await
        .expect("legacy frame");
    let denying = controller(Some(secret_label_deny()));
    assert!(
        window_identities(&aggregator, &denying, None)
            .await
            .is_empty()
    );
    assert!(
        stream_identities(&aggregator, &denying, None)
            .await
            .is_empty()
    );
    let allow_only = controller(None);
    assert_eq!(
        window_identities(&aggregator, &allow_only, None).await,
        vec!["legacy-member".to_string()]
    );
    assert_eq!(
        stream_identities(&aggregator, &allow_only, None).await,
        vec!["legacy-member".to_string()]
    );
}

/// Q-a in both directions on the surface: a relabel never reveals a frame
/// its captured attributes hide, and a member relabelled into the denied
/// class hides its earlier frames.
#[tokio::test]
async fn relabelled_members_are_decided_by_captured_and_current_attributes() {
    let aggregator = MetaConsoleAggregator::in_memory();
    for (key, identity, tier) in [
        ("into-denied", "moved-in", "public"),
        ("out-of-denied", "moved-out", "secret"),
        ("unchanged", "steady", "public"),
    ] {
        aggregator
            .store()
            .append_if_absent(member_frame(
                "runtime-a",
                key,
                identity,
                Some(provenance("runtime-a", identity, tier, Vec::new())),
            ))
            .await
            .expect("frame");
    }
    let controller = controller(Some(secret_label_deny()));
    let current = |identity: &str, tier: &str| crate::access::AgentResourceAttributes {
        identity: identity.to_string(),
        agent_id: Some(identity.to_string()),
        role: Some("worker".to_string()),
        labels: BTreeMap::from([("tier".to_string(), tier.to_string())]),
    };
    controller.record_agent_attributes(current("moved-in", "secret"));
    controller.record_agent_attributes(current("moved-out", "public"));
    controller.record_agent_attributes(current("steady", "public"));
    assert_eq!(
        window_identities(&aggregator, &controller, None).await,
        vec!["steady".to_string()]
    );
    assert_eq!(
        stream_identities(&aggregator, &controller, None).await,
        vec!["steady".to_string()]
    );
}

/// C3 (1)/(2): a parent in the denied class spawned a neutral child; both
/// left, and the store reopened. The child's frames inherit the parent's
/// deny from the captured lineage.
#[tokio::test]
async fn a_captured_denied_parent_hides_its_child_after_both_left() {
    let temp = tempfile::tempdir().expect("temp dir");
    let path = temp.path().join("console.sqlite");
    {
        let store = SqliteConsoleLogStore::open(&path).expect("store");
        store
            .append_if_absent(member_frame(
                "runtime-a",
                "child-of-secret",
                "child-a",
                Some(provenance(
                    "runtime-a",
                    "child-a",
                    "public",
                    vec![link("secret-parent", Some("secret"))],
                )),
            ))
            .await
            .expect("child frame");
        store
            .append_if_absent(member_frame(
                "runtime-a",
                "child-of-public",
                "child-b",
                Some(provenance(
                    "runtime-a",
                    "child-b",
                    "public",
                    vec![link("public-parent", Some("public"))],
                )),
            ))
            .await
            .expect("control frame");
    }
    let aggregator = MetaConsoleAggregator::new(Arc::new(
        SqliteConsoleLogStore::open(&path).expect("reopened store"),
    ));
    let controller = controller(Some(secret_label_deny()));
    assert_eq!(
        window_identities(&aggregator, &controller, None).await,
        vec!["child-b".to_string()]
    );
    assert_eq!(
        stream_identities(&aggregator, &controller, None).await,
        vec!["child-b".to_string()]
    );
}

/// C3 (3): the child's lineage names a parent incarnation the owner could
/// not confirm (it left; a neutral member now holds the same identity). The
/// unconfirmed link fails closed under the label deny, and the current
/// neutral namesake cannot reveal the child.
#[tokio::test]
async fn an_unconfirmed_parent_link_fails_closed_despite_a_neutral_namesake() {
    let aggregator = MetaConsoleAggregator::in_memory();
    aggregator
        .store()
        .append_if_absent(member_frame(
            "runtime-a",
            "child-of-unconfirmed",
            "child",
            Some(provenance(
                "runtime-a",
                "child",
                "public",
                vec![link("parent", None)],
            )),
        ))
        .await
        .expect("child frame");
    let denying = controller(Some(secret_label_deny()));
    denying.record_agent_attributes(crate::access::AgentResourceAttributes {
        identity: "parent".to_string(),
        agent_id: Some("parent".to_string()),
        role: Some("worker".to_string()),
        labels: BTreeMap::from([("tier".to_string(), "public".to_string())]),
    });
    assert!(
        window_identities(&aggregator, &denying, None)
            .await
            .is_empty()
    );
    assert!(
        stream_identities(&aggregator, &denying, None)
            .await
            .is_empty()
    );
    // Without an applicable deny the child stays visible.
    assert_eq!(
        window_identities(&aggregator, &controller(None), None).await,
        vec!["child".to_string()]
    );
}

/// An embedder's namespaced runtime: identities, agent ids and lineage carry
/// the registration's namespace, and the captured attributes decide.
#[tokio::test]
async fn an_embedder_namespaced_member_is_decided_by_its_captured_lineage() {
    let aggregator = MetaConsoleAggregator::in_memory();
    aggregator
        .store()
        .append_if_absent(member_frame(
            "embedder-runtime",
            "namespaced-secret",
            "embedder/worker",
            Some(provenance(
                "embedder-runtime",
                "embedder/worker",
                "secret",
                Vec::new(),
            )),
        ))
        .await
        .expect("secret frame");
    aggregator
        .store()
        .append_if_absent(member_frame(
            "embedder-runtime",
            "namespaced-child",
            "embedder/helper",
            Some(provenance(
                "embedder-runtime",
                "embedder/helper",
                "public",
                vec![link("embedder/lead", Some("public"))],
            )),
        ))
        .await
        .expect("child frame");
    let label_deny = controller(Some(secret_label_deny()));
    assert_eq!(
        window_identities(&aggregator, &label_deny, None).await,
        vec!["embedder/helper".to_string()]
    );
    // A deny naming the namespaced parent reaches its child through the
    // captured lineage, with nothing in the shared cache.
    let parent_deny = controller(Some(AccessRule {
        id: "hide-embedder-lead".to_string(),
        effect: AccessEffect::Deny,
        actions: vec!["agent.view".to_string()],
        agents: vec!["embedder/lead".to_string()],
        ..AccessRule::default()
    }));
    assert_eq!(
        window_identities(&aggregator, &parent_deny, None).await,
        vec!["embedder/worker".to_string()]
    );
}

/// C4 + R-g: a caller whose view hides every frame the scan budget reaches
/// gets the empty-page shape (a before cursor to continue from, not
/// exhausted), never an error that tells it hidden frames exist.
#[tokio::test]
async fn a_fully_hidden_budget_returns_the_empty_page_shape() {
    let aggregator = MetaConsoleAggregator::in_memory();
    aggregator
        .store()
        .append_if_absent(member_frame(
            "runtime-a",
            "visible-old",
            "visible",
            Some(provenance("runtime-a", "visible", "public", Vec::new())),
        ))
        .await
        .expect("visible frame");
    for index in 0..(TIMELINE_RAW_SCAN_PAGE_LIMIT + 5) {
        aggregator
            .store()
            .append_if_absent(member_frame(
                "runtime-a",
                &format!("hidden-{index}"),
                "hidden",
                Some(provenance("runtime-a", "hidden", "secret", Vec::new())),
            ))
            .await
            .expect("hidden frame");
    }
    let controller = controller(Some(secret_label_deny()));
    let decider = TimelineFrameDecider::new(controller.view_for_subject(None));
    let filter: &TimelineFrameFilter<'_> = &|frame: &ConsoleFrame| decider.visible(frame);
    let page = aggregator
        .query_timeline_windowed_for_with_scan_budget(
            recent(10),
            Some(filter),
            TIMELINE_RAW_SCAN_PAGE_LIMIT,
        )
        .await
        .expect("a fully hidden budget is not an error");
    assert!(page.frames.is_empty());
    assert!(!page.exhausted);
    let before = page.next_cursor.expect("a cursor to continue from");
    // Continuing from the cursor reaches the older visible frame.
    let older = aggregator
        .query_timeline_windowed_for_with_scan_budget(
            ConsoleTimelineWindowQuery {
                before: Some(before),
                ..recent(10)
            },
            Some(filter),
            TIMELINE_RAW_SCAN_PAGE_LIMIT,
        )
        .await
        .expect("older page");
    assert_eq!(
        older
            .frames
            .iter()
            .map(|frame| frame.identity.as_str())
            .collect::<Vec<_>>(),
        vec!["visible"]
    );
}

/// R-d: only the owner writes update markers. A frame embedded in any
/// other frame's payload is ingested data, and provenance inside it is not
/// accepted: the embedded frame is decided as uncaptured.
#[tokio::test]
async fn provenance_inside_an_ingested_payload_is_never_trusted() {
    let view_controller = controller(Some(secret_label_deny()));
    let view = view_controller.view_for_subject(None);
    let embedded = ConsoleFrame {
        id: "embedded".to_string(),
        cursor: ConsoleCursor::from_seq(1),
        dedupe_key: "embedded".to_string(),
        timestamp_ms: 1,
        runtime_key: "runtime-a".to_string(),
        identity: "claimed".to_string(),
        conversation_id: None,
        session_id: None,
        kind: "text_complete".to_string(),
        status: ConsoleFrameStatus::Completed,
        frame_version: 1,
        updated_at_ms: None,
        payload: json!({}),
        source: ConsoleFrameSource {
            member_provenance: Some(provenance("runtime-a", "claimed", "public", Vec::new())),
            kind: ConsoleFrameSourceKind::ConsoleEvent,
            source_cursor: None,
        },
        source_event_id: None,
        interaction_id: None,
        turn_id: None,
        run_id: None,
        parent_frame_id: None,
        caused_by_frame_id: None,
    };
    let marker = |kind: ConsoleFrameSourceKind| ConsoleFrame {
        id: "marker".to_string(),
        cursor: ConsoleCursor::from_seq(2),
        dedupe_key: "marker".to_string(),
        kind: "frame_updated".to_string(),
        payload: json!({ "frame": embedded.clone() }),
        source: ConsoleFrameSource {
            member_provenance: Some(provenance("runtime-a", "claimed", "public", Vec::new())),
            kind,
            source_cursor: None,
        },
        ..embedded.clone()
    };
    // The owner's marker: its embedded frame keeps the owner's capture.
    assert!(timeline_frame_visible_to(
        &view,
        &marker(ConsoleFrameSourceKind::Synthetic)
    ));
    // Ingested data shaped like a marker: the embedded capture is ignored,
    // so the label deny fails it closed.
    assert!(!timeline_frame_visible_to(
        &view,
        &marker(ConsoleFrameSourceKind::ConsoleEvent)
    ));
}

/// O1-5: the console's own frames keep their identity decision. A label
/// deny, under which an uncaptured member frame fails closed, does not hide
/// the runtime-plane rows (identity-less memory events and gap markers).
#[tokio::test]
async fn runtime_plane_frames_keep_their_identity_decision_under_a_label_deny() {
    let aggregator = MetaConsoleAggregator::in_memory();
    for (key, identity) in [
        (
            "memory-row",
            crate::console_contracts::SYSTEM_EVENT_IDENTITY,
        ),
        ("gap-row", crate::console_contracts::CONSOLE_GAP_IDENTITY),
    ] {
        aggregator
            .store()
            .append_if_absent(member_frame("runtime-a", key, identity, None))
            .await
            .expect("runtime-plane frame");
    }
    let denying = controller(Some(secret_label_deny()));
    assert_eq!(
        window_identities(&aggregator, &denying, None).await,
        vec![
            crate::console_contracts::SYSTEM_EVENT_IDENTITY.to_string(),
            crate::console_contracts::CONSOLE_GAP_IDENTITY.to_string(),
        ]
    );
    assert_eq!(
        stream_identities(&aggregator, &denying, None).await,
        vec![
            crate::console_contracts::SYSTEM_EVENT_IDENTITY.to_string(),
            crate::console_contracts::CONSOLE_GAP_IDENTITY.to_string(),
        ]
    );
}

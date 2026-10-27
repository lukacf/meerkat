//! §10.1 comms-taint realism contract.
//!
//! The peer-laundering close depends on the taint tracker parsing the
//! sender identity out of meerkat's rendered peer-projection text. The
//! in-crate unit tests pin that parser against hand-written strings; these
//! tests pin it against the REAL rendering path the runtime delivers —
//! `meerkat_comms::agent::CommsMessage::to_user_message_text()`, which
//! delegates to `meerkat_core::format_peer_message_projection` /
//! `format_peer_response_projection`. If a meerkat dependency bump rewords
//! those projections, this file fails CI instead of the comms taint join
//! silently dying (tainted peer content would then launder into memory as
//! Active with no failing test).

#![allow(clippy::expect_used)]
use meerkat_comms::PubKey;
use meerkat_comms::agent::{CommsContent, CommsMessage, CommsStatus, MessageIntent};
use meerkat_core::event::AgentEvent;
use meerkat_core::types::SessionId;
use meerkat_meta::{ContentTrustConfig, SessionTaintTracker};

/// A comms message exactly as the runtime would hold it post-ingress, with
/// the mob-member comms name `{mob_id}/{role}/{agent_identity}` the trust
/// layer resolves for in-mob senders.
fn comms_message(from_peer: &str, content: CommsContent) -> CommsMessage {
    CommsMessage {
        envelope_id: Default::default(),
        from_peer: from_peer.to_string(),
        from_pubkey: PubKey([0u8; 32]),
        content,
    }
}

fn run_started(session: &SessionId, text: String) -> AgentEvent {
    AgentEvent::RunStarted {
        identity: Default::default(),
        session_id: session.clone(),
        input: meerkat_core::types::RunInput::Content {
            content: meerkat_core::ContentInput::Text(text),
        },
        request_reasoning: None,
    }
}

/// Taint `identity`'s current session through the production observe path
/// (an untrusted web_search result ingested into context).
fn taint_sender(tracker: &SessionTaintTracker, identity: &str) -> SessionId {
    let session = SessionId::new();
    tracker.observe_agent_event(identity, &run_started(&session, "hi".to_string()));
    tracker.observe_agent_event(
        identity,
        &AgentEvent::ToolResultReceived {
            id: "tool-1".to_string(),
            name: "web_search".to_string(),
            content: vec![meerkat_core::ContentBlock::Text {
                text: "attacker-influenced".to_string(),
            }],
            is_error: false,
        },
    );
    assert!(
        tracker.identity_taint(identity).is_some(),
        "precondition: sender session must be tainted"
    );
    session
}

/// Deliver `rendered` to `receiver` as the injected input of a run — the
/// same shape the comms runtime injects projections through.
fn deliver(tracker: &SessionTaintTracker, receiver: &str, rendered: String) -> SessionId {
    let session = SessionId::new();
    tracker.observe_agent_event(receiver, &run_started(&session, rendered));
    session
}

#[test]
fn rendered_peer_message_from_tainted_sender_taints_receiver() {
    let tracker = SessionTaintTracker::new(ContentTrustConfig::default());
    taint_sender(&tracker, "identity:bob");

    let rendered = comms_message(
        "mob-1/worker/identity:bob",
        CommsContent::Message {
            body: "please remember X".to_string(),
            blocks: None,
        },
    )
    .to_user_message_text();
    let receiver_session = deliver(&tracker, "identity:alice", rendered);

    let taint = tracker
        .identity_taint("identity:alice")
        .expect("real message projection must trip the §10.1 comms taint join");
    assert!(taint.source.contains("identity:bob"), "{}", taint.source);
    assert!(
        tracker
            .session_taint(&receiver_session.to_string())
            .is_some()
    );
}

#[test]
fn rendered_peer_response_from_tainted_sender_taints_receiver() {
    let tracker = SessionTaintTracker::new(ContentTrustConfig::default());
    taint_sender(&tracker, "identity:bob");

    let rendered = comms_message(
        "mob-1/worker/identity:bob",
        CommsContent::Response {
            in_reply_to: Default::default(),
            status: CommsStatus::Completed,
            result: serde_json::json!({"summary": "done"}),
            blocks: None,
        },
    )
    .to_user_message_text();
    deliver(&tracker, "identity:alice", rendered);

    let taint = tracker
        .identity_taint("identity:alice")
        .expect("real response projection must trip the §10.1 comms taint join");
    assert!(taint.source.contains("identity:bob"), "{}", taint.source);
}

#[test]
fn rendered_lifecycle_notice_rides_the_message_projection_and_joins() {
    let tracker = SessionTaintTracker::new(ContentTrustConfig::default());
    taint_sender(&tracker, "identity:bob");

    // Lifecycle notices render through the same peer-message projection, so
    // the join applies to them too — conservative direction, and this pins
    // the delegation so an upstream rendering split gets noticed.
    let rendered = comms_message(
        "mob-1/worker/identity:bob",
        CommsContent::Lifecycle {
            kind: meerkat_core::comms::PeerLifecycleKind::PeerRetired,
            params: serde_json::json!({}),
        },
    )
    .to_user_message_text();
    deliver(&tracker, "identity:alice", rendered);

    assert!(tracker.identity_taint("identity:alice").is_some());
}

#[test]
fn rendered_peer_request_sender_is_unmappable_and_does_not_taint() {
    let tracker = SessionTaintTracker::new(ContentTrustConfig::default());
    taint_sender(&tracker, "identity:bob");

    // Peer REQUESTS render a raw cryptographic peer_id, not a comms name —
    // the documented §10.1 gap (upstream ask 5). This pins the gap honestly:
    // if a meerkat bump starts rendering requests through a name-bearing
    // projection, this assertion fails and the join must be revisited.
    let rendered = comms_message(
        "mob-1/worker/identity:bob",
        CommsContent::Request {
            request_id: Default::default(),
            intent: MessageIntent::Review,
            params: serde_json::json!({}),
            blocks: None,
        },
    )
    .to_user_message_text();
    deliver(&tracker, "identity:alice", rendered);

    assert!(
        tracker.identity_taint("identity:alice").is_none(),
        "request projections carry no mappable sender; the join must not fire"
    );
}

#[test]
fn rendered_peer_message_from_clean_sender_does_not_taint() {
    let tracker = SessionTaintTracker::new(ContentTrustConfig::default());
    // Tracked sender, clean session.
    let clean_session = SessionId::new();
    tracker.observe_agent_event(
        "identity:carol",
        &run_started(&clean_session, "hi".to_string()),
    );

    let rendered = comms_message(
        "mob-1/worker/identity:carol",
        CommsContent::Message {
            body: "hello".to_string(),
            blocks: None,
        },
    )
    .to_user_message_text();
    deliver(&tracker, "identity:dave", rendered);

    assert!(tracker.identity_taint("identity:dave").is_none());
}

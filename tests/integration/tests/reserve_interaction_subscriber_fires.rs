//! Integration-scope test for the coverage-matrix cell:
//!   TurnDriven × NonRealtime × Request × ReserveInteraction × SubscriberStream
//!
//! Covers the reservation-contract end-to-end between two mutually-trusted
//! in-process `CommsRuntime` instances. Uses the public
//! `CommsRuntime::inproc_pair_with_mutual_trust` helper landed alongside
//! W1-C so integration tests can drive real peer traffic without reaching
//! into private trust-seeding paths.
//!
//! Scenario:
//!   1. Peer A sends a `PeerRequest` with `InputStreamMode::ReserveInteraction`.
//!   2. Peer B drains its inbox, observes the request, replies with a
//!      `PeerResponse` whose `in_reply_to` is the request's envelope id.
//!   3. Peer A drains its inbox, observes the terminal response, and its
//!      reserved subscriber is correlated via the same envelope id.
//!
//! Assertion surface: the one-shot subscriber reservation on A returns
//! `Some(sender)` when looked up by the correlating `InteractionId`.
//! That is the integration-scope analogue of
//! `meerkat_comms::runtime::comms_runtime::tests::\
//!  test_peer_request_reserved_stream_correlates_via_request_envelope_id`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use meerkat_comms::runtime::comms_runtime::CommsRuntime;
use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
use meerkat_core::comms::{CommsCommand, InputStreamMode, PeerName, PeerRoute, SendReceipt};
use meerkat_core::{
    ClassifiedInboxInteraction, HandlingMode, InteractionContent, InteractionId, ResponseStatus,
};
use uuid::Uuid;

#[tokio::test]
async fn reserve_interaction_subscriber_fires_on_matching_response() {
    let suffix = Uuid::new_v4().simple().to_string();
    let name_a = format!("cov-a-{suffix}");
    let name_b = format!("cov-b-{suffix}");

    let (a, b) = CommsRuntime::inproc_pair_with_mutual_trust(&name_a, &name_b)
        .await
        .expect("inproc pair with mutual trust");

    // 1. A sends a reserved-stream PeerRequest to B.
    let receipt = CoreCommsRuntime::send(
        a.as_ref(),
        CommsCommand::PeerRequest {
            to: PeerRoute::with_display_name(
                b.public_key().to_peer_id(),
                PeerName::new(name_b.clone()).expect("peer_name valid"),
            ),
            intent: "reservation-contract-probe".to_string(),
            params: serde_json::json!({"probe": true}),
            handling_mode: HandlingMode::Queue,
            stream: InputStreamMode::ReserveInteraction,
        },
    )
    .await
    .expect("peer request send should succeed");

    let envelope_id = match receipt {
        SendReceipt::PeerRequestSent {
            envelope_id,
            stream_reserved,
            ..
        } => {
            assert!(
                stream_reserved,
                "ReserveInteraction must produce stream_reserved=true",
            );
            envelope_id
        }
        other => panic!("expected PeerRequestSent, got {other:?}"),
    };

    // 2. B drains its inbox, observes the request, replies with a terminal
    //    PeerResponse. Drain with a short retry to tolerate inproc delivery.
    let request_at_b = drain_until_nonempty(b.as_ref(), 50, 20).await;
    assert_eq!(
        request_at_b.len(),
        1,
        "B should observe exactly one request"
    );
    let request = &request_at_b[0].interaction;
    assert!(
        matches!(request.content, InteractionContent::Request { .. }),
        "B's drained interaction should be a Request, got {:?}",
        request.content,
    );
    assert_eq!(
        request.id,
        InteractionId(envelope_id),
        "request envelope id at B must equal A's send receipt envelope id",
    );

    let _response_receipt = CoreCommsRuntime::send(
        b.as_ref(),
        CommsCommand::PeerResponse {
            to: PeerRoute::with_display_name(
                a.public_key().to_peer_id(),
                PeerName::new(name_a.clone()).expect("peer_name valid"),
            ),
            in_reply_to: InteractionId(envelope_id),
            status: ResponseStatus::Completed,
            result: serde_json::json!({"probe_reply": true}),
            handling_mode: Some(HandlingMode::Queue),
        },
    )
    .await
    .expect("peer response send should succeed");

    // 3. A drains its inbox, observes the terminal response, and the
    //    reservation on A correlates back to the envelope id.
    let response_at_a = drain_until_nonempty(a.as_ref(), 50, 20).await;
    assert_eq!(
        response_at_a.len(),
        1,
        "A should observe exactly one response",
    );
    let response = &response_at_a[0].interaction;
    match &response.content {
        InteractionContent::Response { in_reply_to, .. } => {
            assert_eq!(
                *in_reply_to,
                InteractionId(envelope_id),
                "response in_reply_to on A must match the original envelope id",
            );
        }
        other => panic!("expected response interaction, got {other:?}"),
    }

    // Assertion surface: the reserved subscriber on A correlates under the
    // request envelope id carried via `in_reply_to`. One-shot take returns
    // `Some` the first time, `None` the second.
    let subscriber =
        CoreCommsRuntime::interaction_subscriber(a.as_ref(), &InteractionId(envelope_id));
    assert!(
        subscriber.is_some(),
        "reserved peer-request stream on A should correlate via the request \
         envelope id carried in the response in_reply_to",
    );
    let second = CoreCommsRuntime::interaction_subscriber(a.as_ref(), &InteractionId(envelope_id));
    assert!(second.is_none(), "subscriber should be one-shot");
}

async fn drain_until_nonempty(
    runtime: &CommsRuntime,
    poll_ms: u64,
    attempts: u32,
) -> Vec<ClassifiedInboxInteraction> {
    for _ in 0..attempts {
        let batch = CoreCommsRuntime::drain_classified_inbox_interactions(runtime)
            .await
            .unwrap_or_default();
        if !batch.is_empty() {
            return batch;
        }
        tokio::time::sleep(std::time::Duration::from_millis(poll_ms)).await;
    }
    Vec::new()
}

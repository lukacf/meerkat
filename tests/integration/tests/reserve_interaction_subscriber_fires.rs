//! Integration-scope test for the coverage-matrix cell:
//!   TurnDriven × NonRealtime × Request × ReserveInteraction × SubscriberStream
//!
//! Covers the reservation-contract end-to-end between two in-process
//! `CommsRuntime` instances whose mutual trust is installed through generated
//! MeerkatMachine authority.
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
use meerkat_core::PeerInputClass;
use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
use meerkat_core::comms::{
    CommsCommand, CommsTrustMutation, InputStreamMode, PeerAddress, PeerName, PeerRoute,
    PeerTransport, SendReceipt, TrustedPeerDescriptor,
};
use meerkat_core::{
    ClassifiedInboxInteraction, HandlingMode, InteractionContent, InteractionId, PeerCorrelationId,
    ResponseStatus,
};
use meerkat_runtime::handles::{
    HandleDslAuthority, RuntimeInteractionStreamHandle, RuntimePeerCommsHandle,
    RuntimePeerInteractionHandle,
};
use meerkat_runtime::meerkat_machine::dsl as mm_dsl;
use std::sync::Arc;
use uuid::Uuid;

#[tokio::test]
async fn reserve_interaction_subscriber_fires_on_matching_response() {
    let suffix = Uuid::new_v4().simple().to_string();
    let name_a = format!("cov-a-{suffix}");
    let name_b = format!("cov-b-{suffix}");

    let (a, b) = inproc_pair_with_generated_trust(&name_a, &name_b)
        .await
        .expect("inproc pair with mutual trust");

    // 1. A sends a reserved-stream PeerRequest to B.
    let receipt = CoreCommsRuntime::send(
        a.as_ref(),
        CommsCommand::PeerRequest {
            content_taint: None,
            to: PeerRoute::with_display_name(
                b.public_key().to_peer_id(),
                PeerName::new(name_b.clone()).expect("peer_name valid"),
            ),
            intent: "reservation-contract-probe".to_string(),
            params: serde_json::json!({"probe": true}),
            blocks: None,
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
    b.peer_interaction_handle()
        .expect("B should have explicit peer interaction authority")
        .request_received(
            PeerCorrelationId::from_uuid(envelope_id),
            HandlingMode::Queue,
        )
        .expect("direct comms-drain bypass must seed inbound request state");

    let _response_receipt = CoreCommsRuntime::send(
        b.as_ref(),
        CommsCommand::PeerResponse {
            content_taint: None,
            to: PeerRoute::with_display_name(
                a.public_key().to_peer_id(),
                PeerName::new(name_a.clone()).expect("peer_name valid"),
            ),
            in_reply_to: InteractionId(envelope_id),
            status: ResponseStatus::Completed,
            result: serde_json::json!({"probe_reply": true}),
            blocks: None,
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
    assert_eq!(
        response_at_a[0].class(),
        PeerInputClass::ResponseTerminal,
        "terminal response class must be machine-owned at ingress"
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

async fn inproc_pair_with_generated_trust(
    name_a: &str,
    name_b: &str,
) -> Result<(Arc<CommsRuntime>, Arc<CommsRuntime>), String> {
    let a = Arc::new(CommsRuntime::inproc_only(name_a).map_err(|err| err.to_string())?);
    let b = Arc::new(CommsRuntime::inproc_only(name_b).map_err(|err| err.to_string())?);
    let descriptor_a = descriptor_for_runtime(a.as_ref())?;
    let descriptor_b = descriptor_for_runtime(b.as_ref())?;
    let authority_a =
        install_ephemeral_peer_request_response_authority(&a, "integration-generated-trust-a");
    let authority_b =
        install_ephemeral_peer_request_response_authority(&b, "integration-generated-trust-b");
    publish_local_endpoint(&authority_a, &descriptor_a)?;
    publish_local_endpoint(&authority_b, &descriptor_b)?;
    add_generated_trust(a.as_ref(), &authority_a, descriptor_b.clone()).await?;
    add_generated_trust(b.as_ref(), &authority_b, descriptor_a).await?;
    Ok((a, b))
}

fn descriptor_for_runtime(runtime: &CommsRuntime) -> Result<TrustedPeerDescriptor, String> {
    Ok(TrustedPeerDescriptor {
        peer_id: runtime.public_key().to_peer_id(),
        name: PeerName::new(runtime.participant_name().to_string())?,
        address: PeerAddress::new(PeerTransport::Inproc, runtime.participant_name()),
        pubkey: *runtime.public_key().as_bytes(),
    })
}

async fn add_generated_trust(
    runtime: &CommsRuntime,
    authority: &HandleDslAuthority,
    peer: TrustedPeerDescriptor,
) -> Result<(), String> {
    let authority = generated_trust_add_authority(authority, &peer)?;
    CoreCommsRuntime::apply_trust_mutation(
        runtime,
        CommsTrustMutation::AddTrustedPeer { peer, authority },
    )
    .await
    .map(|_| ())
    .map_err(|err| err.to_string())
}

fn generated_trust_add_authority(
    authority: &HandleDslAuthority,
    peer: &TrustedPeerDescriptor,
) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {
    let endpoint = mm_dsl::PeerEndpoint::from(peer);
    let transition = authority
        .apply_input_with_transition(
            mm_dsl::MeerkatMachineInput::AddDirectPeerEndpoint {
                endpoint: endpoint.clone(),
            },
            "test::add_direct_peer_endpoint",
        )
        .map_err(|err| err.to_string())?;
    let mut obligations =
        meerkat_runtime::protocol_comms_trust_reconcile::extract_obligations_with_freshness(
            &transition,
            authority.peer_projection_freshness_authority(),
        );
    let obligation = obligations
        .pop()
        .ok_or_else(|| "generated trust reconcile obligation missing".to_string())?;
    meerkat_runtime::protocol_comms_trust_reconcile::authority_for_endpoint(&obligation, &endpoint)
}

fn publish_local_endpoint(
    authority: &HandleDslAuthority,
    local: &TrustedPeerDescriptor,
) -> Result<(), String> {
    authority
        .apply_input(
            mm_dsl::MeerkatMachineInput::PublishLocalEndpoint {
                endpoint: mm_dsl::PeerEndpoint::from(local),
            },
            "test::publish_local_endpoint",
        )
        .map_err(|err| err.to_string())
}

fn install_ephemeral_peer_request_response_authority(
    runtime: &Arc<CommsRuntime>,
    session: &str,
) -> Arc<HandleDslAuthority> {
    let dsl = Arc::new(HandleDslAuthority::ephemeral());
    dsl.apply_signal(mm_dsl::MeerkatMachineSignal::Initialize, "test::initialize")
        .expect("Initialize signal");
    dsl.apply_input(
        mm_dsl::MeerkatMachineInput::RegisterSession {
            session_id: mm_dsl::SessionId::from(session.to_string()),
        },
        "test::register_session",
    )
    .expect("RegisterSession input");
    dsl.apply_input(
        mm_dsl::MeerkatMachineInput::PrepareBindings {
            agent_runtime_id: mm_dsl::AgentRuntimeId::from(format!("{session}-runtime")),
            fence_token: mm_dsl::FenceToken(1),
            generation: Some(mm_dsl::Generation(0)),
            runtime_epoch_id: None,
            session_id: mm_dsl::SessionId::from(session.to_string()),
        },
        "test::prepare_bindings",
    )
    .expect("PrepareBindings input");

    RuntimePeerCommsHandle::install_generated_on(Arc::clone(&dsl), runtime.as_ref())
        .expect("install generated peer-comms handle");
    runtime.install_peer_request_response_authority(
        meerkat_comms::PeerRequestResponseAuthority::new(
            Arc::new(RuntimePeerInteractionHandle::new(Arc::clone(&dsl))),
            Arc::new(RuntimeInteractionStreamHandle::new(Arc::clone(&dsl))),
        ),
    );
    dsl
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

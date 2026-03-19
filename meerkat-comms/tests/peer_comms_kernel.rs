#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::BTreeMap;

use meerkat_machine_kernels::generated::peer_comms;
use meerkat_machine_kernels::{KernelInput, KernelValue};

fn string(value: &str) -> KernelValue {
    KernelValue::String(value.to_string())
}

fn input(variant: &str, fields: Vec<(&str, KernelValue)>) -> KernelInput {
    KernelInput {
        variant: variant.to_string(),
        fields: fields
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect::<BTreeMap<_, _>>(),
    }
}

fn map_value<'a>(
    state: &'a meerkat_machine_kernels::KernelState,
    field: &str,
    key: &str,
) -> Option<&'a KernelValue> {
    match state.fields.get(field) {
        Some(KernelValue::Map(values)) => values.get(&string(key)),
        _ => None,
    }
}

#[test]
fn peer_comms_kernel_preserves_reservation_and_trust_snapshot_for_trusted_requests() {
    let state = peer_comms::initial_state().expect("initial state");
    let trusted = peer_comms::transition(
        &state,
        &input("TrustPeer", vec![("peer_id", string("peer-a"))]),
    )
    .expect("trust peer")
    .next_state;

    let received = peer_comms::transition(
        &trusted,
        &input(
            "ReceivePeerEnvelope",
            vec![
                ("raw_item_id", string("raw-1")),
                ("peer_id", string("peer-a")),
                ("raw_kind", string("request")),
                ("text_projection", string("hello from peer")),
                ("content_shape", string("text")),
                ("request_id", string("req-1")),
                ("reservation_key", string("reservation-1")),
            ],
        ),
    )
    .expect("receive trusted request")
    .next_state;

    assert_eq!(received.phase, "Received");
    assert_eq!(
        map_value(&received, "classified_as", "raw-1"),
        Some(&string("ActionableRequest"))
    );
    assert_eq!(
        map_value(&received, "trusted_snapshot", "raw-1"),
        Some(&KernelValue::Bool(true))
    );
    assert_eq!(
        map_value(&received, "reservation_key", "raw-1"),
        Some(&string("reservation-1"))
    );

    let delivered = peer_comms::transition(
        &received,
        &input(
            "SubmitTypedPeerInput",
            vec![("raw_item_id", string("raw-1"))],
        ),
    )
    .expect("submit typed peer input");

    assert_eq!(delivered.transition, "SubmitTypedPeerInputDelivered");
    assert_eq!(delivered.next_state.phase, "Delivered");
    assert_eq!(delivered.effects.len(), 1);
    assert_eq!(delivered.effects[0].variant, "SubmitPeerInputCandidate");
    assert_eq!(
        delivered.effects[0].fields.get("request_id"),
        Some(&string("req-1"))
    );
    assert_eq!(
        delivered.effects[0].fields.get("reservation_key"),
        Some(&string("reservation-1"))
    );
}

#[test]
fn peer_comms_kernel_classifies_inline_terminal_without_child_lifecycle_leakage() {
    let state = peer_comms::initial_state().expect("initial state");
    let trusted = peer_comms::transition(
        &state,
        &input("TrustPeer", vec![("peer_id", string("peer-inline"))]),
    )
    .expect("trust peer")
    .next_state;

    let received = peer_comms::transition(
        &trusted,
        &input(
            "ReceivePeerEnvelope",
            vec![
                ("raw_item_id", string("raw-inline")),
                ("peer_id", string("peer-inline")),
                ("raw_kind", string("response_terminal")),
                ("text_projection", string("done")),
                ("content_shape", string("text")),
                ("request_id", KernelValue::None),
                ("reservation_key", KernelValue::None),
            ],
        ),
    )
    .expect("receive trusted inline response")
    .next_state;

    let delivered = peer_comms::transition(
        &received,
        &input(
            "SubmitTypedPeerInput",
            vec![("raw_item_id", string("raw-inline"))],
        ),
    )
    .expect("submit inline-only input");

    assert_eq!(
        delivered.effects[0].fields.get("peer_input_class"),
        Some(&string("Response"))
    );
    assert_ne!(
        delivered.effects[0].fields.get("peer_input_class"),
        Some(&string("SubagentResult"))
    );
    assert_eq!(
        map_value(&received, "trusted_snapshot", "raw-inline"),
        Some(&KernelValue::Bool(true))
    );
}

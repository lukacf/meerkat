#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::BTreeMap;

use meerkat_machine_kernels::generated::peer_directory_reachability;
use meerkat_machine_kernels::{KernelInput, KernelValue};

fn string(value: &str) -> KernelValue {
    KernelValue::String(value.to_string())
}

fn named_variant(enum_name: &str, variant: &str) -> KernelValue {
    KernelValue::NamedVariant {
        enum_name: enum_name.to_string(),
        variant: variant.to_string(),
    }
}

fn key(name: &str, peer_id: &str) -> KernelValue {
    string(&format!("{name}|{peer_id}"))
}

fn input(variant: &str, fields: Vec<(&str, KernelValue)>) -> KernelInput {
    KernelInput {
        variant: variant.to_string(),
        fields: fields
            .into_iter()
            .map(|(field, value)| (field.to_string(), value))
            .collect(),
    }
}

fn map_value<'a>(
    state: &'a meerkat_machine_kernels::KernelState,
    field: &str,
    key: &KernelValue,
) -> Option<&'a KernelValue> {
    match state.fields.get(field) {
        Some(KernelValue::Map(values)) => values.get(key),
        _ => None,
    }
}

#[test]
fn peer_directory_reachability_kernel_reconcile_replaces_directory_snapshot() {
    let state = peer_directory_reachability::initial_state().expect("initial state");
    let peer_key = key("agent-a", "ed25519:a");
    let reconciled = peer_directory_reachability::transition(
        &state,
        &input(
            "ReconcileResolvedDirectory",
            vec![
                (
                    "keys",
                    KernelValue::Set(vec![peer_key.clone()].into_iter().collect()),
                ),
                (
                    "reachability",
                    KernelValue::Map(BTreeMap::from([(peer_key.clone(), string("Unknown"))])),
                ),
                (
                    "last_reason",
                    KernelValue::Map(BTreeMap::from([(peer_key.clone(), KernelValue::None)])),
                ),
            ],
        ),
    )
    .expect("reconcile directory")
    .next_state;

    assert_eq!(reconciled.phase, "Tracking");
    assert_eq!(
        map_value(&reconciled, "reachability", &peer_key),
        Some(&string("Unknown"))
    );
    assert_eq!(
        map_value(&reconciled, "last_reason", &peer_key),
        Some(&KernelValue::None)
    );
}

#[test]
fn peer_directory_reachability_kernel_records_send_failures_for_resolved_peers() {
    let state = peer_directory_reachability::initial_state().expect("initial state");
    let peer_key = key("agent-a", "ed25519:a");
    let reconciled = peer_directory_reachability::transition(
        &state,
        &input(
            "ReconcileResolvedDirectory",
            vec![
                (
                    "keys",
                    KernelValue::Set(vec![peer_key.clone()].into_iter().collect()),
                ),
                (
                    "reachability",
                    KernelValue::Map(BTreeMap::from([(peer_key.clone(), string("Unknown"))])),
                ),
                (
                    "last_reason",
                    KernelValue::Map(BTreeMap::from([(peer_key.clone(), KernelValue::None)])),
                ),
            ],
        ),
    )
    .expect("reconcile directory")
    .next_state;

    let failed = peer_directory_reachability::transition(
        &reconciled,
        &input(
            "RecordSendFailed",
            vec![
                ("key", peer_key.clone()),
                ("reason", string("TransportError")),
            ],
        ),
    )
    .expect("record send failure")
    .next_state;

    assert_eq!(
        map_value(&failed, "reachability", &peer_key),
        Some(&named_variant("PeerReachability", "Unreachable"))
    );
    assert_eq!(
        map_value(&failed, "last_reason", &peer_key),
        Some(&string("TransportError"))
    );
}

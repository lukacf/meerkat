#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::BTreeMap;

use meerkat_machine_kernels::generated::meerkat;
use meerkat_machine_kernels::{KernelInput, KernelSignal, KernelValue};

fn string(value: &str) -> KernelValue {
    KernelValue::String(value.to_string())
}

fn named_variant(enum_name: &str, variant: &str) -> KernelValue {
    KernelValue::NamedVariant {
        enum_name: enum_name.to_string(),
        variant: variant.to_string(),
    }
}

fn some(value: KernelValue) -> KernelValue {
    KernelValue::Map(BTreeMap::from([(string("value"), value)]))
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

fn signal(variant: &str, fields: Vec<(&str, KernelValue)>) -> KernelSignal {
    KernelSignal {
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

fn attached_meerkat_state() -> meerkat_machine_kernels::KernelState {
    let initialized = meerkat::transition_signal(
        &meerkat::initial_state().expect("initial state"),
        &signal("Initialize", vec![]),
    )
    .expect("initialize")
    .next_state;
    let registered = meerkat::transition(
        &initialized,
        &input("RegisterSession", vec![("session_id", string("session-1"))]),
    )
    .expect("register session")
    .next_state;
    meerkat::transition(
        &registered,
        &input(
            "PrepareBindings",
            vec![
                ("agent_runtime_id", string("runtime-7")),
                ("fence_token", KernelValue::U64(3)),
                ("generation", KernelValue::U64(1)),
            ],
        ),
    )
    .expect("prepare bindings")
    .next_state
}

#[test]
fn peer_directory_reachability_kernel_reconcile_replaces_directory_snapshot() {
    let state = attached_meerkat_state();
    let peer_key = key("agent-a", "ed25519:a");
    let reconciled = meerkat::transition_signal(
        &state,
        &signal(
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

    assert_eq!(reconciled.phase, "Attached");
    assert_eq!(
        map_value(&reconciled, "peer_reachability", &peer_key),
        Some(&string("Unknown"))
    );
    assert_eq!(
        map_value(&reconciled, "peer_last_reason", &peer_key),
        Some(&KernelValue::None)
    );
}

#[test]
fn peer_directory_reachability_kernel_records_send_failures_for_resolved_peers() {
    let state = attached_meerkat_state();
    let peer_key = key("agent-a", "ed25519:a");
    let reconciled = meerkat::transition_signal(
        &state,
        &signal(
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

    let failed = meerkat::transition_signal(
        &reconciled,
        &signal(
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
        map_value(&failed, "peer_reachability", &peer_key),
        Some(&named_variant("PeerReachability", "Unreachable"))
    );
    assert_eq!(
        map_value(&failed, "peer_last_reason", &peer_key),
        Some(&some(string("TransportError")))
    );
}

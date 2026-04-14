#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::BTreeMap;

use meerkat_machine_kernels::generated::meerkat;
use meerkat_machine_kernels::{KernelSignal, KernelValue};

fn string(value: &str) -> KernelValue {
    KernelValue::String(value.to_string())
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

#[test]
fn peer_directory_reachability_kernel_reconcile_signal_removed() {
    let state = meerkat::initial_state().expect("initial state");
    let initialized = meerkat::transition_signal(&state, &signal("Initialize", vec![]))
        .expect("initialize")
        .next_state;

    // ReconcileResolvedDirectory was removed from the schema (unimplemented).
    let result = meerkat::transition_signal(
        &initialized,
        &signal(
            "ReconcileResolvedDirectory",
            vec![
                ("keys", KernelValue::Set(Default::default())),
                ("reachability", KernelValue::Map(BTreeMap::new())),
                ("last_reason", KernelValue::Map(BTreeMap::new())),
            ],
        ),
    );
    assert!(
        result.is_err(),
        "ReconcileResolvedDirectory should be rejected as an unknown signal"
    );
}

#[test]
fn peer_directory_reachability_kernel_fields_removed_from_state() {
    let state = meerkat::initial_state().expect("initial state");
    assert!(
        !state.fields.contains_key("resolved_peer_keys"),
        "resolved_peer_keys field should not exist"
    );
    assert!(
        !state.fields.contains_key("peer_reachability"),
        "peer_reachability field should not exist"
    );
    assert!(
        !state.fields.contains_key("peer_last_reason"),
        "peer_last_reason field should not exist"
    );
}

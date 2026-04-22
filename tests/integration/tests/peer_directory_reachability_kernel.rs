#![allow(clippy::expect_used, clippy::unwrap_used)]

use meerkat_machine_kernels::generated::meerkat;

#[test]
fn peer_directory_reachability_kernel_initializes_with_typed_signal() {
    let state = meerkat::initial_state();
    let initialized = meerkat::transition_signal(
        &state,
        meerkat::Signal::Initialize(meerkat::signals::Initialize {}),
        &meerkat::EmptyContext,
    )
    .expect("initialize")
    .next_state;

    assert_ne!(initialized.phase, meerkat::Phase::Initializing);
}

#[test]
fn peer_directory_reachability_kernel_fields_removed_from_state() {
    let state = meerkat::initial_state();
    let value = serde_json::to_value(&state).expect("serialize typed state");
    let object = value.as_object().expect("typed state serializes as object");

    assert!(
        !object.contains_key("resolved_peer_keys"),
        "resolved_peer_keys field should not exist"
    );
    assert!(
        !object.contains_key("peer_reachability"),
        "peer_reachability field should not exist"
    );
    assert!(
        !object.contains_key("peer_last_reason"),
        "peer_last_reason field should not exist"
    );
}

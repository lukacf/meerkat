//! Track-B (R5) peer-projection DSL transition coverage.
//!
//! These tests exercise the Track-B `MeerkatMachine` peer-projection
//! state + inputs + effects at the DSL-authority layer directly. They
//! pin:
//!
//! 1. `PublishLocalEndpoint` / `ClearLocalEndpoint` — session
//!    self-endpoint publication.
//! 2. `AddDirectPeerEndpoint` / `RemoveDirectPeerEndpoint` — non-mob
//!    direct peering.
//! 3. `ApplyMobPeerOverlay { epoch, endpoints }` — composition-driven
//!    mob overlay, with stale-epoch guard rejection.
//! 4. `peer_projection_epoch` advances monotonically on every Track-B
//!    mutation.
//! 5. `LocalEndpointChanged` / `PeerProjectionChanged` /
//!    `CommsTrustReconcileRequested` effects carry the right payload
//!    for downstream Commit 4 consumers.
//!
//! The peer-projection state is the endpoint-level projection that the
//! `RecomputeMobPeerOverlay` composition driver (Commit 4) writes onto
//! each MeerkatMachine session from the identity-level wiring graph
//! owned by MobMachine (Commit 2).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use meerkat_runtime::handles::HandleDslAuthority;
use meerkat_runtime::meerkat_machine::dsl as mm_dsl;

/// Build an ephemeral authority bootstrapped into `Idle` (via
/// `Initialize` + `RegisterSession`). The peer-projection transitions
/// are `Idle`-and-later valid — running them from `Initializing`
/// would be rejected.
fn new_authority() -> Arc<HandleDslAuthority> {
    let dsl = Arc::new(HandleDslAuthority::ephemeral());
    dsl.apply_signal(mm_dsl::MeerkatMachineSignal::Initialize, "test::initialize")
        .expect("Initialize signal");
    dsl.apply_input(
        mm_dsl::MeerkatMachineInput::RegisterSession {
            session_id: mm_dsl::SessionId::from("peer-projection-test".to_string()),
        },
        "test::register_session",
    )
    .expect("RegisterSession input");
    dsl
}

fn endpoint(name: &str, peer_id: &str, address: &str) -> mm_dsl::PeerEndpoint {
    mm_dsl::PeerEndpoint {
        name: name.into(),
        peer_id: peer_id.into(),
        address: address.into(),
        signing_key: mm_dsl::PeerSigningKey([name.as_bytes()[0]; 32]),
    }
}

#[test]
fn publish_local_endpoint_sets_field_and_emits_local_endpoint_changed() {
    let dsl = new_authority();
    let ep = endpoint("local", "ed25519:local", "inproc://local");

    let effects = dsl
        .apply_input_with_effects(
            mm_dsl::MeerkatMachineInput::PublishLocalEndpoint {
                endpoint: ep.clone(),
            },
            "test::publish_local",
        )
        .expect("publish accepted");

    let state = dsl.snapshot_state();
    assert_eq!(state.local_endpoint.as_ref(), Some(&ep));

    let changed = effects.iter().find_map(|e| match e {
        mm_dsl::MeerkatMachineEffect::LocalEndpointChanged { endpoint } => Some(endpoint.clone()),
        _ => None,
    });
    assert_eq!(
        changed,
        Some(Some(ep)),
        "PublishLocalEndpoint must emit LocalEndpointChanged with the new endpoint",
    );
}

#[test]
fn clear_local_endpoint_removes_field_and_emits_local_endpoint_changed_with_none() {
    let dsl = new_authority();
    let ep = endpoint("local", "ed25519:local", "inproc://local");
    dsl.apply_input(
        mm_dsl::MeerkatMachineInput::PublishLocalEndpoint { endpoint: ep },
        "test::publish",
    )
    .expect("publish accepted");

    let effects = dsl
        .apply_input_with_effects(
            mm_dsl::MeerkatMachineInput::ClearLocalEndpoint,
            "test::clear",
        )
        .expect("clear accepted");

    assert!(dsl.snapshot_state().local_endpoint.is_none());
    let changed = effects.iter().find_map(|e| match e {
        mm_dsl::MeerkatMachineEffect::LocalEndpointChanged { endpoint } => Some(endpoint.clone()),
        _ => None,
    });
    assert_eq!(
        changed,
        Some(None),
        "ClearLocalEndpoint must emit LocalEndpointChanged(None)",
    );
}

#[test]
fn clear_local_endpoint_rejects_when_no_local_endpoint_is_present() {
    let dsl = new_authority();
    let result = dsl.apply_input(
        mm_dsl::MeerkatMachineInput::ClearLocalEndpoint,
        "test::clear_missing",
    );
    assert!(
        result.is_err(),
        "ClearLocalEndpoint with no endpoint set must be rejected by the guard",
    );
}

#[test]
fn add_direct_peer_endpoint_inserts_bumps_epoch_and_emits_trust_reconcile() {
    let dsl = new_authority();
    assert_eq!(dsl.snapshot_state().peer_projection_epoch, 0);

    let ep = endpoint("peer-1", "ed25519:peer-1", "inproc://peer-1");
    let effects = dsl
        .apply_input_with_effects(
            mm_dsl::MeerkatMachineInput::AddDirectPeerEndpoint {
                endpoint: ep.clone(),
            },
            "test::add_direct",
        )
        .expect("add accepted");

    let state = dsl.snapshot_state();
    assert!(state.direct_peer_endpoints.contains(&ep));
    assert_eq!(state.peer_projection_epoch, 1);

    let projection_effect = effects.iter().find_map(|e| match e {
        mm_dsl::MeerkatMachineEffect::PeerProjectionChanged {
            peer_projection_epoch,
        } => Some(*peer_projection_epoch),
        _ => None,
    });
    assert_eq!(projection_effect, Some(1));

    let reconcile_epoch = effects.iter().find_map(|e| match e {
        mm_dsl::MeerkatMachineEffect::CommsTrustReconcileRequested {
            peer_projection_epoch,
        } => Some(*peer_projection_epoch),
        _ => None,
    });
    assert_eq!(
        reconcile_epoch,
        Some(1),
        "must emit CommsTrustReconcileRequested carrying the post-transition epoch",
    );
}

#[test]
fn add_direct_peer_endpoint_rejects_duplicate_without_bumping_epoch() {
    let dsl = new_authority();
    let ep = endpoint("peer-1", "ed25519:peer-1", "inproc://peer-1");
    dsl.apply_input(
        mm_dsl::MeerkatMachineInput::AddDirectPeerEndpoint {
            endpoint: ep.clone(),
        },
        "test::first",
    )
    .expect("first accepted");
    assert_eq!(dsl.snapshot_state().peer_projection_epoch, 1);

    let result = dsl.apply_input(
        mm_dsl::MeerkatMachineInput::AddDirectPeerEndpoint { endpoint: ep },
        "test::dup",
    );
    assert!(
        result.is_err(),
        "adding an already-present direct endpoint must be rejected",
    );
    assert_eq!(dsl.snapshot_state().peer_projection_epoch, 1);
}

#[test]
fn remove_direct_peer_endpoint_removes_bumps_epoch_and_emits_trust_reconcile() {
    let dsl = new_authority();
    let ep = endpoint("peer-1", "ed25519:peer-1", "inproc://peer-1");
    dsl.apply_input(
        mm_dsl::MeerkatMachineInput::AddDirectPeerEndpoint {
            endpoint: ep.clone(),
        },
        "test::add",
    )
    .expect("add accepted");
    assert_eq!(dsl.snapshot_state().peer_projection_epoch, 1);

    let effects = dsl
        .apply_input_with_effects(
            mm_dsl::MeerkatMachineInput::RemoveDirectPeerEndpoint {
                endpoint: ep.clone(),
            },
            "test::remove",
        )
        .expect("remove accepted");

    let state = dsl.snapshot_state();
    assert!(!state.direct_peer_endpoints.contains(&ep));
    // Not in overlay either — effective = direct ∪ overlay is the
    // empty union after removal.
    assert!(!state.mob_overlay_peer_endpoints.contains(&ep));
    assert_eq!(state.peer_projection_epoch, 2);

    let projection_effect = effects.iter().find_map(|e| match e {
        mm_dsl::MeerkatMachineEffect::PeerProjectionChanged {
            peer_projection_epoch,
        } => Some(*peer_projection_epoch),
        _ => None,
    });
    assert_eq!(projection_effect, Some(2));

    let reconcile_effect = effects.iter().any(|e| {
        matches!(
            e,
            mm_dsl::MeerkatMachineEffect::CommsTrustReconcileRequested { .. }
        )
    });
    assert!(reconcile_effect);
}

#[test]
fn remove_direct_peer_endpoint_rejects_absent_endpoint() {
    let dsl = new_authority();
    let ep = endpoint("peer-1", "ed25519:peer-1", "inproc://peer-1");
    let result = dsl.apply_input(
        mm_dsl::MeerkatMachineInput::RemoveDirectPeerEndpoint { endpoint: ep },
        "test::remove_absent",
    );
    assert!(
        result.is_err(),
        "removing an absent direct endpoint must be rejected",
    );
    assert_eq!(dsl.snapshot_state().peer_projection_epoch, 0);
}

#[test]
fn remove_direct_peer_endpoint_preserves_overlay_when_also_in_overlay() {
    // Endpoint is both direct AND in mob overlay. Removing the direct
    // entry leaves the overlay intact — the peer remains trust-
    // reachable via the mob path. `effective = direct ∪ overlay` is
    // a read-side projection, so the test asserts the two underlying
    // sets directly.
    let dsl = new_authority();
    let ep = endpoint("peer-1", "ed25519:peer-1", "inproc://peer-1");
    dsl.apply_input(
        mm_dsl::MeerkatMachineInput::AddDirectPeerEndpoint {
            endpoint: ep.clone(),
        },
        "test::add_direct",
    )
    .expect("add direct accepted");
    // Apply a mob overlay that includes the same endpoint. The overlay
    // epoch must be >= the current `peer_projection_epoch` (which was
    // bumped to 1 by AddDirectPeerEndpoint); supply epoch = 10 so the
    // overlay application wins.
    let mut overlay = std::collections::BTreeSet::new();
    overlay.insert(ep.clone());
    dsl.apply_input(
        mm_dsl::MeerkatMachineInput::ApplyMobPeerOverlay {
            epoch: 10,
            endpoints: overlay.clone(),
        },
        "test::apply_overlay",
    )
    .expect("overlay accepted");

    dsl.apply_input(
        mm_dsl::MeerkatMachineInput::RemoveDirectPeerEndpoint {
            endpoint: ep.clone(),
        },
        "test::remove_direct",
    )
    .expect("remove direct accepted");

    let state = dsl.snapshot_state();
    assert!(
        !state.direct_peer_endpoints.contains(&ep),
        "direct set must no longer contain the endpoint",
    );
    assert!(
        state.mob_overlay_peer_endpoints.contains(&ep),
        "overlay set must still carry the endpoint — effective = direct ∪ overlay remains non-empty",
    );
}

#[test]
fn apply_mob_peer_overlay_replaces_overlay_set_and_bumps_epoch() {
    let dsl = new_authority();
    let ep_a = endpoint("peer-a", "ed25519:a", "inproc://a");
    let ep_b = endpoint("peer-b", "ed25519:b", "inproc://b");

    let mut first = std::collections::BTreeSet::new();
    first.insert(ep_a.clone());
    let effects_1 = dsl
        .apply_input_with_effects(
            mm_dsl::MeerkatMachineInput::ApplyMobPeerOverlay {
                epoch: 1,
                endpoints: first,
            },
            "test::overlay_1",
        )
        .expect("first overlay accepted");
    let state = dsl.snapshot_state();
    assert!(state.mob_overlay_peer_endpoints.contains(&ep_a));
    assert!(!state.mob_overlay_peer_endpoints.contains(&ep_b));
    assert_eq!(state.peer_projection_epoch, 1);
    assert!(
        effects_1.iter().any(|e| matches!(
            e,
            mm_dsl::MeerkatMachineEffect::PeerProjectionChanged { .. }
        )),
        "first overlay must emit PeerProjectionChanged",
    );

    // Second overlay: replace with {ep_b}. ep_a must leave, ep_b must
    // enter the overlay. The effective set (direct ∪ overlay) is a
    // read-side projection, so these assertions target the overlay
    // directly.
    let mut second = std::collections::BTreeSet::new();
    second.insert(ep_b.clone());
    dsl.apply_input(
        mm_dsl::MeerkatMachineInput::ApplyMobPeerOverlay {
            epoch: 2,
            endpoints: second,
        },
        "test::overlay_2",
    )
    .expect("second overlay accepted");
    let state = dsl.snapshot_state();
    assert!(
        !state.mob_overlay_peer_endpoints.contains(&ep_a),
        "ep_a must be dropped from overlay",
    );
    assert!(
        state.mob_overlay_peer_endpoints.contains(&ep_b),
        "ep_b must be present in overlay",
    );
    // `peer_projection_epoch` bumps by 1 per overlay apply (general
    // freshness counter); `mob_overlay_epoch` matches the supplied
    // driver epoch (overlay-specific watermark).
    assert_eq!(state.peer_projection_epoch, 2);
    assert_eq!(state.mob_overlay_epoch, 2);
}

#[test]
fn apply_mob_peer_overlay_rejects_stale_epoch() {
    let dsl = new_authority();
    let ep = endpoint("peer-a", "ed25519:a", "inproc://a");
    let mut endpoints = std::collections::BTreeSet::new();
    endpoints.insert(ep);
    dsl.apply_input(
        mm_dsl::MeerkatMachineInput::ApplyMobPeerOverlay {
            epoch: 5,
            endpoints: endpoints.clone(),
        },
        "test::overlay_5",
    )
    .expect("overlay accepted");
    assert_eq!(dsl.snapshot_state().mob_overlay_epoch, 5);

    // Strictly-less epoch must be rejected.
    let stale = dsl.apply_input(
        mm_dsl::MeerkatMachineInput::ApplyMobPeerOverlay {
            epoch: 4,
            endpoints: endpoints.clone(),
        },
        "test::overlay_stale",
    );
    assert!(
        stale.is_err(),
        "stale overlay epoch (4 < 5) must be rejected by the stale-epoch guard",
    );

    // Equal epoch is also rejected (strictly-greater guard). The
    // driver's monotonic counter suppresses no-op dispatches, so any
    // re-delivery at the same epoch is a retry-bug on the transport
    // layer and surfaces as an error.
    let equal = dsl.apply_input(
        mm_dsl::MeerkatMachineInput::ApplyMobPeerOverlay {
            epoch: 5,
            endpoints,
        },
        "test::overlay_equal_epoch",
    );
    assert!(
        equal.is_err(),
        "equal overlay epoch must be rejected by the strictly-greater-epoch guard",
    );
    assert_eq!(dsl.snapshot_state().mob_overlay_epoch, 5);
}

#[test]
fn apply_mob_peer_overlay_empty_set_clears_overlay_but_preserves_direct() {
    let dsl = new_authority();
    let ep_direct = endpoint("direct", "ed25519:d", "inproc://d");
    let ep_overlay = endpoint("overlay", "ed25519:o", "inproc://o");
    dsl.apply_input(
        mm_dsl::MeerkatMachineInput::AddDirectPeerEndpoint {
            endpoint: ep_direct.clone(),
        },
        "test::add_direct",
    )
    .expect("add direct");
    let mut first = std::collections::BTreeSet::new();
    first.insert(ep_overlay.clone());
    dsl.apply_input(
        mm_dsl::MeerkatMachineInput::ApplyMobPeerOverlay {
            epoch: 1,
            endpoints: first,
        },
        "test::apply_overlay",
    )
    .expect("apply overlay");
    let state = dsl.snapshot_state();
    assert!(state.direct_peer_endpoints.contains(&ep_direct));
    assert!(state.mob_overlay_peer_endpoints.contains(&ep_overlay));

    // Clear overlay. Apply epoch = 10 (> current epoch of 1 after
    // overlay apply) so the stale-epoch guard accepts; the current
    // overlay had epoch 1 from the first apply.
    dsl.apply_input(
        mm_dsl::MeerkatMachineInput::ApplyMobPeerOverlay {
            epoch: 10,
            endpoints: std::collections::BTreeSet::new(),
        },
        "test::clear_overlay",
    )
    .expect("clear overlay");

    let state = dsl.snapshot_state();
    assert!(
        state.mob_overlay_peer_endpoints.is_empty(),
        "overlay must be empty after clear",
    );
    assert!(
        state.direct_peer_endpoints.contains(&ep_direct),
        "direct set must survive overlay clear",
    );
    // Effective = direct ∪ overlay = {ep_direct} ∪ {} = {ep_direct}.
    assert!(
        !state.mob_overlay_peer_endpoints.contains(&ep_overlay),
        "overlay-only endpoint must be dropped by the clearing overlay",
    );
}

#[test]
fn peer_projection_epoch_advances_monotonically_across_mixed_inputs() {
    // Post-review-fix: `peer_projection_epoch` and `mob_overlay_epoch`
    // live in separate namespaces. `peer_projection_epoch` bumps by 1
    // on every Track-B mutation; `mob_overlay_epoch` bumps only on
    // successful overlay apply and takes the supplied driver epoch
    // verbatim.
    let dsl = new_authority();
    let ep_a = endpoint("peer-a", "ed25519:a", "inproc://a");
    let ep_b = endpoint("peer-b", "ed25519:b", "inproc://b");

    dsl.apply_input(
        mm_dsl::MeerkatMachineInput::AddDirectPeerEndpoint {
            endpoint: ep_a.clone(),
        },
        "test::add_a",
    )
    .expect("add a");
    assert_eq!(dsl.snapshot_state().peer_projection_epoch, 1);
    assert_eq!(
        dsl.snapshot_state().mob_overlay_epoch,
        0,
        "direct-endpoint mutation must NOT advance mob_overlay_epoch",
    );

    let mut overlay = std::collections::BTreeSet::new();
    overlay.insert(ep_b);
    dsl.apply_input(
        mm_dsl::MeerkatMachineInput::ApplyMobPeerOverlay {
            epoch: 10,
            endpoints: overlay,
        },
        "test::overlay",
    )
    .expect("overlay");
    assert_eq!(
        dsl.snapshot_state().peer_projection_epoch,
        2,
        "overlay apply bumps peer_projection_epoch by 1",
    );
    assert_eq!(
        dsl.snapshot_state().mob_overlay_epoch,
        10,
        "overlay apply takes the supplied driver epoch verbatim",
    );

    dsl.apply_input(
        mm_dsl::MeerkatMachineInput::RemoveDirectPeerEndpoint { endpoint: ep_a },
        "test::remove_a",
    )
    .expect("remove a");
    assert_eq!(
        dsl.snapshot_state().peer_projection_epoch,
        3,
        "direct-endpoint mutations bump peer_projection_epoch by 1",
    );
    assert_eq!(
        dsl.snapshot_state().mob_overlay_epoch,
        10,
        "direct-endpoint mutations MUST NOT regress the overlay epoch",
    );
}

#[test]
fn publish_local_endpoint_does_not_advance_peer_projection_epoch() {
    // `LocalEndpointChanged` fires on publish/clear but does NOT touch
    // the effective peer set — the local endpoint is this session's
    // self-address, not a remote peer. The projection epoch therefore
    // stays put across local-endpoint lifecycle.
    let dsl = new_authority();
    assert_eq!(dsl.snapshot_state().peer_projection_epoch, 0);

    dsl.apply_input(
        mm_dsl::MeerkatMachineInput::PublishLocalEndpoint {
            endpoint: endpoint("local", "ed25519:local", "inproc://local"),
        },
        "test::publish",
    )
    .expect("publish");
    assert_eq!(dsl.snapshot_state().peer_projection_epoch, 0);

    dsl.apply_input(
        mm_dsl::MeerkatMachineInput::ClearLocalEndpoint,
        "test::clear",
    )
    .expect("clear");
    assert_eq!(dsl.snapshot_state().peer_projection_epoch, 0);
}

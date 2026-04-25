//! T13 (Phase 5): mid-turn reauth notice surface — top-down observable
//! proof that the MeerkatMachine DSL auth lifecycle drives the
//! `AuthReauthRequired` system notice at the next CallingLlm boundary.
//!
//! This test verifies the integration at the handle → DSL authority →
//! snapshot observation level: the runtime-backed [`AuthLeaseHandle`]
//! transitions the shared DSL authority through the legal state sequence
//! and every transition is visible through [`AuthLeaseHandle::snapshot`].
//!
//! The plan's choke point C12 reads: "lease expires mid-turn → system
//! notice → next CallingLlm". This file exercises the state-transition
//! half of that contract; the agent-runner half is exercised by the
//! e2e-auth lane's `e2e_auth_mid_turn_reauth_notice_surface` test
//! (`tests/integration/tests/e2e_auth_lane.rs`).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use meerkat_core::connection::{BindingId, RealmId};
use meerkat_core::handles::{AuthLeaseHandle, AuthLeasePhase, LeaseKey};
use meerkat_runtime::RuntimeAuthLeaseHandle;

fn lease(realm: &str, binding: &str) -> LeaseKey {
    LeaseKey::new(
        RealmId::parse(realm).expect("valid realm"),
        BindingId::parse(binding).expect("valid binding"),
        None,
    )
}

/// Given an ephemeral handle and a binding key, acquiring the lease then
/// driving it into `reauth_required` (via `MarkReauthRequired`) yields a
/// snapshot where `state == Some("reauth_required")` — this is the
/// precise precondition the agent runner's CallingLlm arm checks
/// (meerkat-core/src/agent/state.rs).
#[test]
fn mark_reauth_required_transitions_to_reauth_required_state() {
    let handle = RuntimeAuthLeaseHandle::ephemeral();
    let key = lease("test-realm", "test-binding");

    // Acquire → state=valid.
    handle.acquire_lease(&key, 9_999_999_999).unwrap();
    assert_eq!(
        handle.snapshot(&key).phase,
        Some(AuthLeasePhase::Valid),
        "acquire_lease must land in valid"
    );

    // Drive directly to reauth_required (short-circuit mid-turn).
    handle.mark_reauth_required(&key).unwrap();
    assert_eq!(
        handle.snapshot(&key).phase,
        Some(AuthLeasePhase::ReauthRequired),
        "mark_reauth_required must land in reauth_required"
    );

    // Expires-at is preserved across state transitions.
    assert_eq!(
        handle.snapshot(&key).expires_at,
        Some(9_999_999_999),
        "expires_at must be preserved across reauth transition"
    );
}

/// Sequence: Acquire → MarkAuthExpiring → BeginAuthRefresh → simulated
/// permanent failure → reauth_required. Each step's DSL guard is
/// enforced and each intermediate state is observable via snapshot.
#[test]
fn refresh_path_terminates_in_reauth_required_on_permanent_failure() {
    let handle = RuntimeAuthLeaseHandle::ephemeral();
    let key = lease("realm-x", "binding-y");

    handle.acquire_lease(&key, 1_000).unwrap();
    handle.mark_expiring(&key).unwrap();
    assert_eq!(handle.snapshot(&key).phase, Some(AuthLeasePhase::Expiring));

    handle.begin_refresh(&key).unwrap();
    assert_eq!(
        handle.snapshot(&key).phase,
        Some(AuthLeasePhase::Refreshing)
    );

    // Permanent refresh failure → reauth_required.
    handle.refresh_failed(&key, true).unwrap();
    assert_eq!(
        handle.snapshot(&key).phase,
        Some(AuthLeasePhase::ReauthRequired),
        "permanent refresh failure must land in reauth_required (runner snapshot-polls this state to emit a session notice; dogma §1/§19: the DSL state is the sole canonical truth, no redundant effect emission)"
    );
}

/// DSL-level refresh dedup invariant: once a binding is in `refreshing`,
/// a second `BeginAuthRefresh` is rejected by the DSL guard. This is the
/// machine-owned guarantee that at most one concurrent refresh per
/// binding_key is possible — the shell cannot violate it.
#[test]
fn begin_refresh_rejected_from_refreshing_state() {
    let handle = RuntimeAuthLeaseHandle::ephemeral();
    let key = lease("dedup", "binding");

    handle.acquire_lease(&key, 1_000).unwrap();
    handle.begin_refresh(&key).unwrap();
    let err = handle
        .begin_refresh(&key)
        .expect_err("second begin_refresh must be rejected");
    assert!(
        err.to_string().contains("BeginRefresh") || err.to_string().contains("begin_refresh"),
        "error must mention BeginRefresh / begin_refresh: {err}"
    );
}

/// Transient failure path: refresh_failed(permanent=false) returns the
/// binding to `expiring`, not `reauth_required` — the runner can retry.
#[test]
fn transient_refresh_failure_returns_to_expiring() {
    let handle = RuntimeAuthLeaseHandle::ephemeral();
    let key = lease("transient", "binding");

    handle.acquire_lease(&key, 1_000).unwrap();
    handle.mark_expiring(&key).unwrap();
    handle.begin_refresh(&key).unwrap();
    handle.refresh_failed(&key, false).unwrap();
    assert_eq!(
        handle.snapshot(&key).phase,
        Some(AuthLeasePhase::Expiring),
        "transient failure must return binding to expiring"
    );

    // And begin_refresh is legal again from expiring.
    handle.begin_refresh(&key).unwrap();
    assert_eq!(
        handle.snapshot(&key).phase,
        Some(AuthLeasePhase::Refreshing)
    );
}

/// Release removes the binding from all state sets.
#[test]
fn release_clears_all_state() {
    let handle = RuntimeAuthLeaseHandle::ephemeral();
    let key = lease("release", "binding");

    handle.acquire_lease(&key, 1_000).unwrap();
    handle.release_lease(&key).unwrap();
    assert_eq!(handle.snapshot(&key).phase, None);
    assert_eq!(handle.snapshot(&key).expires_at, None);
}

/// AuthLeaseSnapshot preserves expires_at through the acquire →
/// snapshot round-trip. This is the read-path the agent runner's
/// TTL sampler consults to decide whether to call mark_expiring.
#[test]
fn snapshot_preserves_expires_at_for_ttl_sampler() {
    let handle = RuntimeAuthLeaseHandle::ephemeral();
    let key = lease("ttl", "binding");

    // Lease with a specific expiry timestamp.
    handle.acquire_lease(&key, 1_700_000_000).unwrap();
    let snap = handle.snapshot(&key);
    assert_eq!(snap.phase, Some(AuthLeasePhase::Valid));
    assert_eq!(snap.expires_at, Some(1_700_000_000));

    // After CompleteAuthRefresh, expires_at updates.
    handle.mark_expiring(&key).unwrap();
    handle.begin_refresh(&key).unwrap();
    handle
        .complete_refresh(&key, 1_800_000_000, 1_750_000_000)
        .unwrap();
    let snap = handle.snapshot(&key);
    assert_eq!(snap.phase, Some(AuthLeasePhase::Valid));
    assert_eq!(snap.expires_at, Some(1_800_000_000));
}

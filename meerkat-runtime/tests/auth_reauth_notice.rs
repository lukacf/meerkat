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

use meerkat_core::handles::AuthLeaseHandle;
use meerkat_runtime::RuntimeAuthLeaseHandle;

/// Given an ephemeral handle and a binding key, acquiring the lease then
/// driving it into `reauth_required` (via `MarkReauthRequired`) yields a
/// snapshot where `state == Some("reauth_required")` — this is the
/// precise precondition the agent runner's CallingLlm arm checks
/// (meerkat-core/src/agent/state.rs).
#[test]
fn mark_reauth_required_transitions_to_reauth_required_state() {
    let handle = RuntimeAuthLeaseHandle::ephemeral();
    let key = "test-realm:test-binding";

    // Acquire → state=valid.
    handle.acquire_lease(key, 9_999_999_999).unwrap();
    assert_eq!(
        handle.snapshot(key).state.as_deref(),
        Some("valid"),
        "acquire_lease must land in valid"
    );

    // Drive directly to reauth_required (short-circuit mid-turn).
    handle.mark_reauth_required(key).unwrap();
    assert_eq!(
        handle.snapshot(key).state.as_deref(),
        Some("reauth_required"),
        "mark_reauth_required must land in reauth_required"
    );

    // Expires-at is preserved across state transitions.
    assert_eq!(
        handle.snapshot(key).expires_at,
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
    let key = "realm-x:binding-y";

    handle.acquire_lease(key, 1_000).unwrap();
    handle.mark_expiring(key).unwrap();
    assert_eq!(handle.snapshot(key).state.as_deref(), Some("expiring"));

    handle.begin_refresh(key).unwrap();
    assert_eq!(handle.snapshot(key).state.as_deref(), Some("refreshing"));

    // Permanent refresh failure → reauth_required.
    handle.refresh_failed(key, true).unwrap();
    assert_eq!(
        handle.snapshot(key).state.as_deref(),
        Some("reauth_required"),
        "permanent refresh failure must land in reauth_required (emits EmitAuthReauthNotice effect)"
    );
}

/// DSL-level refresh dedup invariant: once a binding is in `refreshing`,
/// a second `BeginAuthRefresh` is rejected by the DSL guard. This is the
/// machine-owned guarantee that at most one concurrent refresh per
/// binding_key is possible — the shell cannot violate it.
#[test]
fn begin_refresh_rejected_from_refreshing_state() {
    let handle = RuntimeAuthLeaseHandle::ephemeral();
    let key = "dedup:binding";

    handle.acquire_lease(key, 1_000).unwrap();
    handle.begin_refresh(key).unwrap();
    let err = handle
        .begin_refresh(key)
        .expect_err("second begin_refresh must be rejected");
    assert!(
        err.to_string().contains("BeginAuthRefresh"),
        "error must mention BeginAuthRefresh: {err}"
    );
}

/// Transient failure path: refresh_failed(permanent=false) returns the
/// binding to `expiring`, not `reauth_required` — the runner can retry.
#[test]
fn transient_refresh_failure_returns_to_expiring() {
    let handle = RuntimeAuthLeaseHandle::ephemeral();
    let key = "transient:binding";

    handle.acquire_lease(key, 1_000).unwrap();
    handle.mark_expiring(key).unwrap();
    handle.begin_refresh(key).unwrap();
    handle.refresh_failed(key, false).unwrap();
    assert_eq!(
        handle.snapshot(key).state.as_deref(),
        Some("expiring"),
        "transient failure must return binding to expiring"
    );

    // And begin_refresh is legal again from expiring.
    handle.begin_refresh(key).unwrap();
    assert_eq!(handle.snapshot(key).state.as_deref(), Some("refreshing"));
}

/// Release removes the binding from all state sets.
#[test]
fn release_clears_all_state() {
    let handle = RuntimeAuthLeaseHandle::ephemeral();
    let key = "release:binding";

    handle.acquire_lease(key, 1_000).unwrap();
    handle.release_lease(key).unwrap();
    assert_eq!(handle.snapshot(key).state, None);
    assert_eq!(handle.snapshot(key).expires_at, None);
}

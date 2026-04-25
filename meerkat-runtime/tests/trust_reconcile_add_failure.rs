//! Wave-c C-T §6 #3 — `CommsTrustReconciler` add-failure contract.
//!
//! Ports the blocker stub that was originally scaffolded in
//! `meerkat-comms/tests/trust_reconcile_add_failure.rs` at c.0. See
//! the sibling `trust_reconcile_concurrency.rs` for the rationale on
//! why the tests live in `meerkat-runtime/tests/` instead — the
//! reconciler lives in `meerkat-runtime`, and `meerkat-comms` cannot
//! dev-dep on it without introducing a circular crate dependency.
//!
//! Invariant pinned (§6 #3): when the trust store's `add_trusted_peer`
//! returns an error, the reconciler must
//!
//!   (a) surface the failure as
//!       `CommsTrustReconcileError::AddTrustFailed` with the typed
//!       peer id echoed back,
//!   (b) NOT mutate the canonical trust store — later reconciles
//!       re-read the same "empty" state, so a retry will try to add
//!       the same peer again rather than the reconciler leaking a
//!       phantom "already applied" entry.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use meerkat_core::agent::{CommsCapabilityError, CommsRuntime};
use meerkat_core::comms::{SendError, TrustedPeerDescriptor};
use meerkat_core::{
    PeerIngressAuthorityPhase, PeerIngressQueueSnapshot, PeerIngressRuntimeSnapshot,
};
use meerkat_runtime::comms_trust_reconcile::{CommsTrustReconcileError, CommsTrustReconciler};
use meerkat_runtime::meerkat_machine::dsl::{
    PeerAddress, PeerEndpoint, PeerId, PeerName, PeerSigningKey,
};

const UUID_A: &str = "aaaaaaaa-0000-4000-8000-000000000001";

fn endpoint(name: &str, peer_id_uuid: &str) -> PeerEndpoint {
    PeerEndpoint {
        name: PeerName(format!("ep-{name}")),
        peer_id: PeerId(peer_id_uuid.to_string()),
        address: PeerAddress(format!("inproc://{name}")),
        signing_key: PeerSigningKey([name.as_bytes()[0]; 32]),
    }
}

/// `CommsRuntime` mock whose next `add_trusted_peer` call returns
/// `SendError::Unsupported`. Once fired, the flag resets so a
/// subsequent retry succeeds — this lets us assert the retry path
/// sees the peer as still absent from the applied view.
#[derive(Default)]
struct AddFailingCommsRuntime {
    fail_next_add: AtomicBool,
    successful_adds: std::sync::Mutex<Vec<TrustedPeerDescriptor>>,
}

impl std::fmt::Debug for AddFailingCommsRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AddFailingCommsRuntime").finish()
    }
}

#[async_trait]
impl CommsRuntime for AddFailingCommsRuntime {
    async fn drain_messages(&self) -> Vec<String> {
        Vec::new()
    }

    fn inbox_notify(&self) -> Arc<tokio::sync::Notify> {
        Arc::new(tokio::sync::Notify::new())
    }

    async fn add_trusted_peer(&self, peer: TrustedPeerDescriptor) -> Result<(), SendError> {
        if self.fail_next_add.swap(false, Ordering::SeqCst) {
            return Err(SendError::Unsupported("synthetic add failure".into()));
        }
        self.successful_adds
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(peer);
        Ok(())
    }

    async fn remove_trusted_peer(&self, _peer_id: &str) -> Result<bool, SendError> {
        Ok(true)
    }

    async fn peer_ingress_runtime_snapshot(
        &self,
    ) -> Result<PeerIngressRuntimeSnapshot, CommsCapabilityError> {
        Ok(PeerIngressRuntimeSnapshot {
            self_peer_id: meerkat_core::comms::PeerId::parse(
                "00000000-0000-4000-8000-000000000000",
            )
            .expect("valid test peer id"),
            auth_required: true,
            authority_phase: PeerIngressAuthorityPhase::Received,
            trusted_peers: self.successful_add_calls(),
            submission_queue_len: 0,
            queue: PeerIngressQueueSnapshot::default(),
        })
    }
}

impl AddFailingCommsRuntime {
    fn successful_add_calls(&self) -> Vec<TrustedPeerDescriptor> {
        self.successful_adds
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// Add failure surfaces a typed `AddTrustFailed` error. The
/// canonical trust store is NOT mutated — a subsequent retry with the
/// same peer set re-reads the peer as absent and tries the add again.
#[tokio::test]
async fn add_failure_surfaces_typed_error_and_preserves_canonical_store() {
    let comms = Arc::new(AddFailingCommsRuntime::default());
    comms.fail_next_add.store(true, Ordering::SeqCst);
    let reconciler = CommsTrustReconciler::new(comms.clone());

    // First reconcile: add fails.
    let err = reconciler
        .reconcile(1, BTreeSet::from([endpoint("A", UUID_A)]))
        .await
        .expect_err("add_trust failure must surface");
    match err {
        CommsTrustReconcileError::AddTrustFailed { peer_id, .. } => {
            assert_eq!(
                peer_id, UUID_A,
                "typed error must echo the peer_id that failed",
            );
        }
        other => panic!("expected AddTrustFailed, got {other:?}"),
    }

    // Canonical store must still be empty; the only attempted add
    // errored before it could commit.
    assert!(
        comms.successful_add_calls().is_empty(),
        "no successful trust-store adds yet — the only attempt errored",
    );

    // Retry the same reconcile at the same epoch. Now the failure
    // flag is cleared; the retry succeeds because the reconciler
    // re-reads the canonical store and still sees the peer absent.
    let retry = reconciler
        .reconcile(1, BTreeSet::from([endpoint("A", UUID_A)]))
        .await
        .expect("retry succeeds with flag cleared");
    assert_eq!(retry.applied_epoch, 1);
    assert_eq!(retry.added.len(), 1, "retry treats peer A as newly added");
    assert!(retry.removed.is_empty());
    assert_eq!(
        comms.successful_add_calls().len(),
        1,
        "trust store now carries peer A after the retry",
    );
    let trusted_after_retry = comms.successful_add_calls();
    assert_eq!(trusted_after_retry.len(), 1);
    assert_eq!(trusted_after_retry[0].peer_id.to_string(), UUID_A);
}

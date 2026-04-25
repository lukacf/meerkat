//! Wave-c C-T §6 #1 + #2 — `CommsTrustReconciler` concurrency invariants.
//!
//! Ports the blocker stub that was originally scaffolded in
//! `meerkat-comms/tests/trust_reconcile_concurrency.rs` at c.0. The
//! stub expected a free `meerkat_comms::trust_reconcile::reconcile`
//! entry point that was never built — the post-PR #340 architecture
//! routes trust reconciliation through the DSL-owned
//! `CommsTrustReconcileRequested` effect consumed by
//! `meerkat_runtime::comms_trust_reconcile::CommsTrustReconciler`.
//! The tests live next to the reconciler they exercise (in
//! `meerkat-runtime/tests/`) rather than in `meerkat-comms/tests/`
//! because `meerkat-comms` cannot dev-dep on `meerkat-runtime`
//! without introducing a circular crate dependency.
//!
//! Invariants pinned:
//!
//! * §6 #1 — Concurrent reconciles serialize through the reconciler's
//!   async mutex; there is no race between overlapping `reconcile()`
//!   calls.
//! * §6 #2 — A stale-epoch reconcile (arriving after a newer one has
//!   committed) short-circuits inside the critical section without
//!   mutating the trust store. The applied-epoch watermark never
//!   regresses.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use meerkat_core::agent::CommsRuntime;
use meerkat_core::comms::{SendError, TrustedPeerDescriptor};
use meerkat_runtime::comms_trust_reconcile::{CommsTrustReconciler, ReconcileReport};
use meerkat_runtime::meerkat_machine::dsl::{
    PeerAddress, PeerEndpoint, PeerId, PeerName, PeerSigningKey,
};

const UUID_A: &str = "aaaaaaaa-0000-4000-8000-000000000001";
const UUID_B: &str = "bbbbbbbb-0000-4000-8000-000000000002";

fn endpoint(name: &str, peer_id_uuid: &str) -> PeerEndpoint {
    PeerEndpoint {
        name: PeerName(format!("ep-{name}")),
        peer_id: PeerId(peer_id_uuid.to_string()),
        address: PeerAddress(format!("inproc://{name}")),
        signing_key: PeerSigningKey([name.as_bytes()[0]; 32]),
    }
}

/// Mock `CommsRuntime` that records every trust-store interaction and
/// exposes the accumulated history for assertion. The calls-vector
/// mutex is `std::sync::Mutex` because we never hold it across an
/// `.await`; the reconciler's own async mutex guards the critical
/// section we're testing.
#[derive(Default)]
struct RecordingCommsRuntime {
    adds: std::sync::Mutex<Vec<TrustedPeerDescriptor>>,
    removes: std::sync::Mutex<Vec<String>>,
    add_count: AtomicUsize,
    remove_count: AtomicUsize,
}

impl std::fmt::Debug for RecordingCommsRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecordingCommsRuntime").finish()
    }
}

#[async_trait]
impl CommsRuntime for RecordingCommsRuntime {
    async fn drain_messages(&self) -> Vec<String> {
        Vec::new()
    }

    fn inbox_notify(&self) -> Arc<tokio::sync::Notify> {
        Arc::new(tokio::sync::Notify::new())
    }

    async fn add_trusted_peer(&self, peer: TrustedPeerDescriptor) -> Result<(), SendError> {
        self.add_count.fetch_add(1, Ordering::SeqCst);
        self.adds
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(peer);
        Ok(())
    }

    async fn remove_trusted_peer(&self, peer_id: &str) -> Result<bool, SendError> {
        self.remove_count.fetch_add(1, Ordering::SeqCst);
        self.removes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(peer_id.to_string());
        Ok(true)
    }
}

impl RecordingCommsRuntime {
    fn add_calls(&self) -> Vec<TrustedPeerDescriptor> {
        self.adds
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// §6 #1 — concurrent reconciles are serialized end-to-end.
///
/// Two reconciles spawn in parallel; whichever wins the mutex first
/// runs to completion (including all trust-store awaits) before the
/// other observes any state. After both complete, the applied view
/// reflects the newer epoch exactly, and the trust store is
/// consistent with the winner's peer set.
#[tokio::test]
async fn concurrent_reconciles_are_serialised_and_stale_short_circuits() {
    let comms = Arc::new(RecordingCommsRuntime::default());
    let reconciler = Arc::new(CommsTrustReconciler::new(comms.clone()));

    let reconciler_older = reconciler.clone();
    let reconciler_newer = reconciler.clone();
    let older_task = tokio::spawn(async move {
        reconciler_older
            .reconcile(1, BTreeSet::from([endpoint("A", UUID_A)]))
            .await
    });
    let newer_task = tokio::spawn(async move {
        reconciler_newer
            .reconcile(2, BTreeSet::from([endpoint("B", UUID_B)]))
            .await
    });
    let older_res: ReconcileReport = older_task
        .await
        .expect("older task joins")
        .expect("older reconcile ok");
    let newer_res: ReconcileReport = newer_task
        .await
        .expect("newer task joins")
        .expect("newer reconcile ok");

    // Final applied view reflects the newer epoch, always.
    let (applied_epoch, applied_peers) = reconciler.applied_snapshot().await;
    assert_eq!(applied_epoch, 2);
    assert_eq!(applied_peers, BTreeSet::from([endpoint("B", UUID_B)]));

    // The newer reconcile's applied_epoch is 2 regardless of ordering.
    assert_eq!(
        newer_res.applied_epoch, 2,
        "newer reconcile must apply its epoch (never stale against older): got {}",
        newer_res.applied_epoch,
    );
    // The older one either reports 1 (ran first) or 2 (stale short-
    // circuit after newer committed). Both are legal outcomes of
    // serialization.
    assert!(
        older_res.applied_epoch == 1 || older_res.applied_epoch == 2,
        "older reconcile's applied_epoch must be 1 or 2: got {}",
        older_res.applied_epoch,
    );
}

/// §6 #2 — a stale-epoch reconcile arriving after a newer one has
/// committed short-circuits inside the critical section. Trust store
/// is NOT touched for the stale reconcile's peer set; applied
/// watermark does not regress.
///
/// Uses a sequential form because serialization is now strict —
/// overlapping reconciles wait on the mutex, so the concurrency
/// invariant reduces to the sequential one after ordering resolves.
#[tokio::test]
async fn stale_reconcile_after_newer_commit_does_not_touch_trust_store() {
    let comms = Arc::new(RecordingCommsRuntime::default());
    let reconciler = CommsTrustReconciler::new(comms.clone());

    // Newer commits first at epoch=5 with peer set {A}.
    let newer = reconciler
        .reconcile(5, BTreeSet::from([endpoint("A", UUID_A)]))
        .await
        .expect("newer reconcile");
    assert_eq!(newer.applied_epoch, 5);
    assert_eq!(comms.add_count.load(Ordering::SeqCst), 1);
    assert_eq!(comms.remove_count.load(Ordering::SeqCst), 0);

    // Stale arrives at epoch=3 with a different set {B}. Expected:
    // empty report, applied_epoch stays at 5, no trust-store calls.
    let stale = reconciler
        .reconcile(3, BTreeSet::from([endpoint("B", UUID_B)]))
        .await
        .expect("stale reconcile accepted as no-op");
    assert!(stale.added.is_empty(), "stale added must be empty");
    assert!(stale.removed.is_empty(), "stale removed must be empty");
    assert_eq!(
        stale.applied_epoch, 5,
        "stale report must reflect the newer watermark",
    );

    // Trust store was not touched by the stale reconcile.
    assert_eq!(
        comms.add_count.load(Ordering::SeqCst),
        1,
        "stale reconcile must not leak add_trusted_peer calls",
    );
    assert_eq!(
        comms.remove_count.load(Ordering::SeqCst),
        0,
        "stale reconcile must not leak remove_trusted_peer calls",
    );
    let added_peer_ids: Vec<String> = comms
        .add_calls()
        .into_iter()
        .map(|d| d.peer_id.to_string())
        .collect();
    assert_eq!(
        added_peer_ids,
        vec![UUID_A.to_string()],
        "only the newer reconcile's peer A must be in the trust store",
    );

    // Applied view reflects only the newer reconcile.
    let (applied_epoch, applied_peers) = reconciler.applied_snapshot().await;
    assert_eq!(applied_epoch, 5);
    assert_eq!(applied_peers, BTreeSet::from([endpoint("A", UUID_A)]));
}

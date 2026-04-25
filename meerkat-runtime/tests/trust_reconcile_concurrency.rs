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
//! * §6 #1 — Concurrent reconciles read from the canonical trust store;
//!   there is no helper-local applied view for overlapping calls to
//!   corrupt.
//! * §6 #2 — Epoch is reported for observability only. A lower-epoch
//!   reconcile still diffs against canonical runtime trust, not against
//!   a reconciler-local watermark.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use meerkat_core::agent::{CommsCapabilityError, CommsRuntime};
use meerkat_core::comms::{SendError, TrustedPeerDescriptor};
use meerkat_core::{
    PeerIngressAuthorityPhase, PeerIngressQueueSnapshot, PeerIngressRuntimeSnapshot,
};
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
/// exposes the accumulated history and current canonical trust store for
/// assertion. The mutexes are `std::sync::Mutex` because we never hold them
/// across an `.await`.
#[derive(Default)]
struct RecordingCommsRuntime {
    adds: std::sync::Mutex<Vec<TrustedPeerDescriptor>>,
    removes: std::sync::Mutex<Vec<String>>,
    trusted: std::sync::Mutex<Vec<TrustedPeerDescriptor>>,
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
            .push(peer.clone());
        self.trusted
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
        let mut trusted = self
            .trusted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let before = trusted.len();
        trusted.retain(|peer| peer.peer_id.to_string() != peer_id);
        Ok(before != trusted.len())
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
            trusted_peers: self
                .trusted
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
            submission_queue_len: 0,
            queue: PeerIngressQueueSnapshot::default(),
        })
    }
}

impl RecordingCommsRuntime {
    fn add_calls(&self) -> Vec<TrustedPeerDescriptor> {
        self.adds
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn trusted_peer_ids(&self) -> BTreeSet<String> {
        self.trusted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|peer| peer.peer_id.to_string())
            .collect()
    }
}

/// §6 #1 — concurrent reconciles use canonical trust as their only state.
///
/// Two reconciles spawn in parallel. Their exact interleaving is not
/// semantically owned by the reconciler; the invariant is that no helper-local
/// applied view exists for either call to corrupt.
#[tokio::test]
async fn concurrent_reconciles_complete_against_canonical_store() {
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

    assert_eq!(older_res.applied_epoch, 1);
    assert_eq!(newer_res.applied_epoch, 2);
    let trusted = comms.trusted_peer_ids();
    assert!(
        !trusted.is_empty(),
        "at least one reconcile must update canonical trust",
    );
    assert!(
        trusted.is_subset(&BTreeSet::from([UUID_A.to_string(), UUID_B.to_string()])),
        "canonical trust must contain only reconciled peers, got {trusted:?}",
    );
}

/// §6 #2 — lower-epoch reconciles still diff against canonical trust.
#[tokio::test]
async fn lower_epoch_reconcile_reads_canonical_store() {
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

    // Lower epoch arrives with a different set {B}. Expected: canonical diff
    // removes A and adds B; no helper-local watermark short-circuits it.
    let lower = reconciler
        .reconcile(3, BTreeSet::from([endpoint("B", UUID_B)]))
        .await
        .expect("lower-epoch reconcile");
    assert_eq!(lower.applied_epoch, 3);
    assert_eq!(lower.added, vec![endpoint("B", UUID_B)]);
    assert_eq!(lower.removed, vec![endpoint("A", UUID_A)]);

    assert_eq!(
        comms.add_count.load(Ordering::SeqCst),
        2,
        "lower-epoch reconcile adds peer B from canonical diff",
    );
    assert_eq!(
        comms.remove_count.load(Ordering::SeqCst),
        1,
        "lower-epoch reconcile removes peer A from canonical diff",
    );
    let added_peer_ids: BTreeSet<String> = comms
        .add_calls()
        .into_iter()
        .map(|d| d.peer_id.to_string())
        .collect();
    assert_eq!(
        added_peer_ids,
        BTreeSet::from([UUID_A.to_string(), UUID_B.to_string()]),
        "both successful adds should be recorded",
    );
    assert_eq!(
        comms.trusted_peer_ids(),
        BTreeSet::from([UUID_B.to_string()])
    );
}

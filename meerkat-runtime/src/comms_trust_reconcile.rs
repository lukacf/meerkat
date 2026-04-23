//! Comms trust reconciliation handler.
//!
//! Track-B (R5) Commit 4: mechanically reconciles a
//! `meerkat-comms` trust store against the effective peer set
//! declared by the MeerkatMachine DSL.
//!
//! Ownership split:
//!
//! * The DSL (`meerkat-machine-schema::catalog::dsl::meerkat_machine`)
//!   owns the declarative facts:
//!   `direct_peer_endpoints`, `mob_overlay_peer_endpoints`,
//!   `peer_projection_epoch`.
//! * The `CommsTrustReconcileRequested` effect fires whenever the
//!   effective set could have changed. The effect carries only the
//!   post-transition `peer_projection_epoch` — the DSL emits the fact
//!   "reconcile needed at epoch N", the shell does the mechanical
//!   diff.
//! * This handler owns the mechanical reconciliation: it maintains an
//!   "applied trust store" view and, given a fresh effective peer
//!   set, computes the add / remove delta and calls
//!   `CommsRuntime::add_trusted_peer` / `remove_trusted_peer`
//!   mechanically. No semantic decisions live here; failures surface
//!   through typed errors.
//!
//! The handler is stateless about the DSL — it does not read machine
//! snapshots directly. Callers supply the effective peer set
//! (`direct ∪ overlay`) as a single `BTreeSet<PeerEndpoint>` from
//! the DSL's state snapshot.

use std::collections::BTreeSet;
use std::sync::Arc;

use meerkat_core::agent::CommsRuntime;
use meerkat_core::comms::{SendError, TrustedPeerSpec};
// `tokio::sync::Mutex` (async) rather than `std::sync::Mutex` (sync)
// so the lock can be held across `.await` points. This lets the
// reconcile path serialize end-to-end — trust-store mutations AND
// the applied-view commit run inside the same critical section,
// closing the TOCTOU class where a stale reconcile's mutations
// could orphan peers in the trust store while its commit was
// skipped (PR #340 re-review item).
use tokio::sync::Mutex;

use crate::meerkat_machine::dsl::PeerEndpoint;

/// Typed error surfaced by the reconciliation handler.
#[derive(Debug, thiserror::Error)]
pub enum CommsTrustReconcileError {
    /// `add_trusted_peer` failed at the trust store. The handler
    /// surfaces the underlying `SendError` verbatim.
    #[error("add_trusted_peer for `{peer_id}` failed: {source}")]
    AddTrustFailed {
        peer_id: String,
        #[source]
        source: SendError,
    },
    /// `remove_trusted_peer` failed at the trust store.
    #[error("remove_trusted_peer for `{peer_id}` failed: {source}")]
    RemoveTrustFailed {
        peer_id: String,
        #[source]
        source: SendError,
    },
}

/// Structured summary of a single reconciliation pass.
///
/// Tests and observability consumers read this to understand what
/// the reconciler actually asked of the trust store.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    /// Peers that were newly registered on this pass.
    pub added: Vec<PeerEndpoint>,
    /// Peers that were unregistered on this pass.
    pub removed: Vec<PeerEndpoint>,
    /// Epoch watermark the reconciler applied.
    pub applied_epoch: u64,
}

/// Mechanical trust reconciliation handler.
///
/// Holds a reference to a `CommsRuntime` and an applied view the
/// reconciler uses to compute deltas. Thread-safe; the applied
/// view is guarded by a single `Mutex`.
pub struct CommsTrustReconciler {
    comms: Arc<dyn CommsRuntime>,
    applied: Mutex<AppliedView>,
}

impl std::fmt::Debug for CommsTrustReconciler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `dyn CommsRuntime` does not implement `Debug`; print a
        // structural placeholder. Reading the applied view requires
        // an async lock acquisition, which is not available from
        // `Debug::fmt` — tests use `applied_snapshot()` for the
        // view contents instead.
        f.debug_struct("CommsTrustReconciler")
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Default)]
struct AppliedView {
    /// Peers currently registered in the trust store (per the
    /// reconciler's view; failures are self-correcting on the next
    /// pass).
    peers: BTreeSet<PeerEndpoint>,
    /// Highest epoch applied so far. Reconciles with strictly-lower
    /// epoch are rejected to prevent out-of-order deliveries
    /// regressing the trust view.
    epoch: u64,
}

impl CommsTrustReconciler {
    /// Construct a reconciler bound to the given comms runtime.
    /// Initial applied view is empty at epoch 0.
    pub fn new(comms: Arc<dyn CommsRuntime>) -> Self {
        Self {
            comms,
            applied: Mutex::new(AppliedView::default()),
        }
    }

    /// Reconcile the trust store against `effective_peers`.
    ///
    /// Returns a [`ReconcileReport`] describing the add / remove
    /// calls the reconciler made. Out-of-order epochs (epoch <
    /// currently-applied) return `Ok` with an empty report — the
    /// reconciler treats them as stale.
    ///
    /// Trust-store failures surface as
    /// [`CommsTrustReconcileError`]; the applied view is only
    /// advanced for peers the trust store acknowledged, so failed
    /// adds/removes are retried on the next pass.
    ///
    /// Concurrency (PR #340 re-review fix): the entire reconcile
    /// path — stale-epoch check, trust-store add/remove calls, and
    /// applied-view commit — runs inside a single `tokio::sync::Mutex`
    /// critical section. Stale reconciles fail the epoch check at
    /// the TOP of the critical section and short-circuit WITHOUT
    /// touching the trust store, preventing the earlier TOCTOU
    /// class where a stale reconcile's mutations could orphan
    /// peers in the trust store while its commit was skipped.
    ///
    /// The lock is tokio's async mutex (not `std::sync::Mutex`) so
    /// it can be held across `.await` points for the trust-store
    /// calls. The critical section is short relative to mob
    /// topology changes — overlay reconciliation fires at the
    /// rate of topology-change events, not per-message — so
    /// holding the lock across awaits is well within the
    /// reconciler's latency budget.
    pub async fn reconcile(
        &self,
        epoch: u64,
        effective_peers: BTreeSet<PeerEndpoint>,
    ) -> Result<ReconcileReport, CommsTrustReconcileError> {
        let mut guard = self.applied.lock().await;

        if epoch < guard.epoch {
            // Stale delivery — short-circuit INSIDE the lock.
            // The trust store is not touched, so no orphaned
            // mutations.
            return Ok(ReconcileReport {
                added: Vec::new(),
                removed: Vec::new(),
                applied_epoch: guard.epoch,
            });
        }

        let previous_peers: BTreeSet<PeerEndpoint> = guard.peers.clone();

        let to_add: Vec<PeerEndpoint> = effective_peers
            .iter()
            .filter(|ep| !previous_peers.contains(*ep))
            .cloned()
            .collect();
        let to_remove: Vec<PeerEndpoint> = previous_peers
            .iter()
            .filter(|ep| !effective_peers.contains(*ep))
            .cloned()
            .collect();

        let mut added = Vec::new();
        let mut removed = Vec::new();

        // Perform adds first so a concurrent peer-send from the same
        // session sees the new peer available even if a remove for
        // an older session hasn't completed. Both adds and removes
        // happen while the async mutex is held — concurrent
        // reconciles wait their turn.
        for endpoint in to_add {
            let spec = TrustedPeerSpec {
                name: endpoint.name.clone(),
                peer_id: endpoint.peer_id.clone(),
                address: endpoint.address.clone(),
            };
            self.comms.add_trusted_peer(spec).await.map_err(|source| {
                CommsTrustReconcileError::AddTrustFailed {
                    peer_id: endpoint.peer_id.clone(),
                    source,
                }
            })?;
            added.push(endpoint);
        }

        for endpoint in to_remove {
            self.comms
                .remove_trusted_peer(&endpoint.peer_id)
                .await
                .map_err(|source| CommsTrustReconcileError::RemoveTrustFailed {
                    peer_id: endpoint.peer_id.clone(),
                    source,
                })?;
            removed.push(endpoint);
        }

        // Commit the applied view under the same lock. No race
        // possible — we've held the lock across the entire
        // trust-store interaction.
        guard.peers = effective_peers;
        guard.epoch = epoch;
        let applied_epoch = epoch;

        Ok(ReconcileReport {
            added,
            removed,
            applied_epoch,
        })
    }

    /// Snapshot of the reconciler's current applied view (tests).
    #[cfg(test)]
    pub(crate) async fn applied_snapshot(&self) -> (u64, BTreeSet<PeerEndpoint>) {
        let guard = self.applied.lock().await;
        (guard.epoch, guard.peers.clone())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn endpoint(name: &str) -> PeerEndpoint {
        PeerEndpoint {
            name: format!("ep-{name}"),
            peer_id: format!("ed25519:{name}"),
            address: format!("inproc://{name}"),
        }
    }

    /// Records every `add_trusted_peer` / `remove_trusted_peer` call
    /// for assertion in tests. Uses `std::sync::Mutex` for the
    /// recorder state (not the async tokio Mutex used by the
    /// reconciler's applied view) — the recorder only needs the
    /// sync form to guard its accumulated calls vector.
    #[derive(Default)]
    struct RecordingCommsRuntime {
        adds: std::sync::Mutex<Vec<TrustedPeerSpec>>,
        removes: std::sync::Mutex<Vec<String>>,
        fail_next_add: AtomicBool,
        fail_next_remove: AtomicBool,
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

        async fn add_trusted_peer(&self, peer: TrustedPeerSpec) -> Result<(), SendError> {
            if self.fail_next_add.swap(false, Ordering::SeqCst) {
                return Err(SendError::Unsupported("synthetic failure".into()));
            }
            self.adds
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(peer);
            Ok(())
        }

        async fn remove_trusted_peer(&self, peer_id: &str) -> Result<bool, SendError> {
            if self.fail_next_remove.swap(false, Ordering::SeqCst) {
                return Err(SendError::Unsupported("synthetic failure".into()));
            }
            self.removes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(peer_id.to_string());
            Ok(true)
        }
    }

    impl RecordingCommsRuntime {
        fn add_calls(&self) -> Vec<TrustedPeerSpec> {
            self.adds
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }

        fn remove_calls(&self) -> Vec<String> {
            self.removes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    #[tokio::test]
    async fn first_reconcile_registers_all_effective_peers() {
        let comms = Arc::new(RecordingCommsRuntime::default());
        let reconciler = CommsTrustReconciler::new(comms.clone());
        let peers = BTreeSet::from([endpoint("A"), endpoint("B")]);

        let report = reconciler
            .reconcile(1, peers.clone())
            .await
            .expect("first reconcile succeeds");

        assert_eq!(report.applied_epoch, 1);
        assert_eq!(report.added.len(), 2);
        assert!(report.removed.is_empty());

        let add_calls = comms.add_calls();
        assert_eq!(add_calls.len(), 2);
        assert_eq!(comms.remove_calls().len(), 0);
        assert!(
            add_calls.iter().any(|spec| spec.peer_id == "ed25519:A"
                && spec.name == "ep-A"
                && spec.address == "inproc://A"),
            "add_trusted_peer must be called with the canonical (name, peer_id, address) triple",
        );
    }

    #[tokio::test]
    async fn subsequent_reconcile_adds_new_and_removes_departed() {
        let comms = Arc::new(RecordingCommsRuntime::default());
        let reconciler = CommsTrustReconciler::new(comms.clone());

        let peers_v1 = BTreeSet::from([endpoint("A"), endpoint("B")]);
        reconciler
            .reconcile(1, peers_v1)
            .await
            .expect("v1 reconcile");

        let peers_v2 = BTreeSet::from([endpoint("A"), endpoint("C")]);
        let report = reconciler
            .reconcile(2, peers_v2)
            .await
            .expect("v2 reconcile");

        // Added: C. Removed: B. Retained: A.
        assert_eq!(report.added, vec![endpoint("C")]);
        assert_eq!(report.removed, vec![endpoint("B")]);
        assert_eq!(report.applied_epoch, 2);

        // Trust-store calls: v1 {A,B} adds, v2 {C} add + {B} remove.
        // Across both passes the add calls are A, B, C and the remove
        // calls are B.
        assert_eq!(comms.add_calls().len(), 3);
        assert_eq!(comms.remove_calls(), vec!["ed25519:B"]);
    }

    #[tokio::test]
    async fn stale_epoch_reconcile_is_accepted_as_no_op() {
        let comms = Arc::new(RecordingCommsRuntime::default());
        let reconciler = CommsTrustReconciler::new(comms.clone());

        reconciler
            .reconcile(5, BTreeSet::from([endpoint("A")]))
            .await
            .expect("first reconcile");

        let report = reconciler
            .reconcile(4, BTreeSet::from([endpoint("B")]))
            .await
            .expect("stale reconcile accepted");
        assert!(report.added.is_empty());
        assert!(report.removed.is_empty());
        assert_eq!(
            report.applied_epoch, 5,
            "applied_epoch must remain at the newer watermark",
        );

        // Only the first reconcile's add made it to the trust store.
        assert_eq!(comms.add_calls().len(), 1);
        assert_eq!(comms.remove_calls().len(), 0);
    }

    #[tokio::test]
    async fn add_failure_surfaces_typed_error_and_does_not_update_applied_view() {
        let comms = Arc::new(RecordingCommsRuntime::default());
        comms.fail_next_add.store(true, Ordering::SeqCst);
        let reconciler = CommsTrustReconciler::new(comms.clone());

        let err = reconciler
            .reconcile(1, BTreeSet::from([endpoint("A")]))
            .await
            .expect_err("add_trust failure must surface");
        match err {
            CommsTrustReconcileError::AddTrustFailed { peer_id, .. } => {
                assert_eq!(peer_id, "ed25519:A");
            }
            other => panic!("expected AddTrustFailed, got {other:?}"),
        }
        let (epoch, applied) = reconciler.applied_snapshot().await;
        assert_eq!(
            epoch, 0,
            "applied epoch must NOT advance past a failed reconcile",
        );
        assert!(applied.is_empty());
    }

    #[tokio::test]
    async fn empty_effective_set_clears_all_previously_trusted_peers() {
        let comms = Arc::new(RecordingCommsRuntime::default());
        let reconciler = CommsTrustReconciler::new(comms.clone());

        reconciler
            .reconcile(1, BTreeSet::from([endpoint("A"), endpoint("B")]))
            .await
            .expect("seed");

        let report = reconciler
            .reconcile(2, BTreeSet::new())
            .await
            .expect("clear all");
        assert_eq!(report.removed.len(), 2);
        assert!(report.added.is_empty());

        let mut removes = comms.remove_calls();
        removes.sort();
        assert_eq!(removes, vec!["ed25519:A", "ed25519:B"]);
    }

    /// PR #340 re-review item: serialize reconciles end-to-end so
    /// trust-store mutations are inside the same critical section
    /// as the applied-view commit. Earlier TOCTOU fix re-checked
    /// the epoch before commit but didn't gate the trust-store
    /// calls, so a stale reconcile could leak add/remove mutations
    /// into the comms trust store even though its commit was
    /// skipped (orphaned trust = authorization bypass).
    ///
    /// The async mutex now serializes reconciles strictly. A
    /// stale reconcile acquiring the lock AFTER a newer one
    /// commits sees `epoch < applied.epoch` and short-circuits
    /// WITHOUT touching the trust store.
    ///
    /// This test pins two invariants at once:
    ///   1. The stale reconcile does not mutate the trust store
    ///      (no orphaned `add_trusted_peer` / `remove_trusted_peer`
    ///      calls).
    ///   2. The stale reconcile's report reflects the newer
    ///      applied epoch (watermark never regresses).
    ///
    /// The scenario uses sequential calls (rather than a gated
    /// mock) because serialization is now strict: a concurrent
    /// stale reconcile would simply wait for the newer one to
    /// release the lock, then short-circuit. The sequential form
    /// is the same invariant tested identically.
    #[tokio::test]
    async fn stale_reconcile_under_serialization_does_not_leak_trust_store_mutations() {
        let comms = Arc::new(RecordingCommsRuntime::default());
        let reconciler = CommsTrustReconciler::new(comms.clone());

        // Newer reconcile lands first at epoch=5 with peer set {A}.
        let newer = reconciler
            .reconcile(5, BTreeSet::from([endpoint("A")]))
            .await
            .expect("newer reconcile");
        assert_eq!(newer.applied_epoch, 5);
        assert_eq!(comms.add_calls().len(), 1);
        assert_eq!(comms.remove_calls().len(), 0);

        // Stale reconcile attempts to install a different peer set
        // at an earlier epoch. The serialized critical section sees
        // the stale epoch at the TOP and short-circuits. Crucially,
        // `add_trusted_peer` / `remove_trusted_peer` must NOT be
        // called for the stale reconcile's peer set — doing so
        // would leak orphaned trust entries into the comms store.
        let stale = reconciler
            .reconcile(3, BTreeSet::from([endpoint("B"), endpoint("C")]))
            .await
            .expect("stale reconcile accepted as no-op");
        assert!(stale.added.is_empty());
        assert!(stale.removed.is_empty());
        assert_eq!(
            stale.applied_epoch, 5,
            "stale report must reflect the newer applied watermark",
        );

        // Trust store end-state: only the newer reconcile's adds
        // landed. The stale reconcile's `{B, C}` additions must
        // NOT appear, AND no removes were issued.
        assert_eq!(
            comms.add_calls().len(),
            1,
            "stale reconcile must not leak `add_trusted_peer` calls into the trust store",
        );
        assert_eq!(
            comms.remove_calls().len(),
            0,
            "stale reconcile must not leak `remove_trusted_peer` calls",
        );
        let added_peer_ids: Vec<String> = comms
            .add_calls()
            .into_iter()
            .map(|spec| spec.peer_id)
            .collect();
        assert_eq!(
            added_peer_ids,
            vec!["ed25519:A".to_string()],
            "only the newer reconcile's peer A must be in the trust store",
        );

        // Applied view reflects only the newer reconcile.
        let (applied_epoch, applied_peers) = reconciler.applied_snapshot().await;
        assert_eq!(applied_epoch, 5);
        assert_eq!(applied_peers, BTreeSet::from([endpoint("A")]));
    }

    /// Concurrent reconciles are strictly serialized by the async
    /// mutex: an "older" reconcile spawned first holds the lock
    /// across its trust-store awaits; a "newer" reconcile waits
    /// for the lock. When the older one finishes (committing
    /// epoch=1), the newer one acquires the lock and sees
    /// epoch=2 > applied=1, so it proceeds normally.
    ///
    /// If the order is reversed — newer spawns first, commits
    /// epoch=2, stale runs after — the stale one sees
    /// epoch=1 < applied=2 and short-circuits without touching
    /// the trust store. This is the serialization guarantee; this
    /// test proves concurrent-task scheduling doesn't break it.
    #[tokio::test]
    async fn concurrent_reconciles_are_serialized_and_stale_short_circuits() {
        let comms = Arc::new(RecordingCommsRuntime::default());
        let reconciler = Arc::new(CommsTrustReconciler::new(comms.clone()));

        // Spawn two concurrent reconciles: older (epoch=1) and newer
        // (epoch=2). Because of the async mutex, whichever acquires
        // the lock first runs to completion; the other waits.
        let reconciler_older = reconciler.clone();
        let reconciler_newer = reconciler.clone();
        let older_task = tokio::spawn(async move {
            reconciler_older
                .reconcile(1, BTreeSet::from([endpoint("A")]))
                .await
        });
        let newer_task = tokio::spawn(async move {
            reconciler_newer
                .reconcile(2, BTreeSet::from([endpoint("B")]))
                .await
        });
        let older_res = older_task.await.expect("older joins").expect("older ok");
        let newer_res = newer_task.await.expect("newer joins").expect("newer ok");

        // One of them runs first; the other runs second. If older
        // runs first, it commits epoch=1 / {A}, then newer commits
        // epoch=2 / {B}. If newer runs first, it commits epoch=2 /
        // {B}, then older short-circuits as stale. In either
        // ordering, the final applied state is epoch=2 / {B} and
        // the trust store ends at {B} (either via add-then-replace
        // or just add).
        let (applied_epoch, applied_peers) = reconciler.applied_snapshot().await;
        assert_eq!(applied_epoch, 2);
        assert_eq!(applied_peers, BTreeSet::from([endpoint("B")]));

        // At least one of the reconciles reports epoch=2 (the
        // winner). The other either also reports epoch=2 (if it
        // short-circuited as stale after the winner committed) or
        // epoch=1 (if it ran first and committed before being
        // superseded).
        assert!(
            older_res.applied_epoch == 1 || older_res.applied_epoch == 2,
            "older reconcile's applied_epoch must be 1 (ran first) or 2 (stale short-circuit): got {}",
            older_res.applied_epoch,
        );
        assert_eq!(
            newer_res.applied_epoch, 2,
            "newer reconcile must apply its epoch (never stale against older)",
        );
    }
}

//! Comms trust reconciliation handler.
//!
//! Track-B (R5) Commit 4 (ported into wave-c `dogma/wave-c-c-t`): mechanically
//! reconciles a `meerkat-comms` trust store against the effective peer set
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
//! * This handler owns the mechanical reconciliation: it reads the
//!   canonical trust store, computes the add / remove delta against a
//!   fresh effective peer set, and calls
//!   `CommsRuntime::add_trusted_peer` / `remove_trusted_peer`
//!   mechanically. No semantic decisions live here; failures surface
//!   through typed errors.
//!
//! The handler is stateless about the DSL — it does not read machine
//! snapshots directly. Callers supply the effective peer set
//! (`direct ∪ overlay`) as a single `BTreeSet<PeerEndpoint>` from
//! the DSL's state snapshot.
//!
//! Wave-c retype (C-T port): the sibling branch's `TrustedPeerSpec`
//! 3-field struct was superseded by
//! `meerkat_core::comms::TrustedPeerDescriptor` (typed `PeerName` /
//! `PeerId` / `PeerAddress` atoms + `pubkey: [u8; 32]` signing key).
//! The DSL `PeerEndpoint` carries typed mirrors of those atoms plus
//! the peer signing key; the reconciler parses the string-shaped
//! identity atoms at the trust-store boundary and forwards the
//! machine-owned key bytes unchanged.

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::meerkat_machine::dsl::PeerEndpoint;
use meerkat_core::agent::{CommsCapabilityError, CommsRuntime};
use meerkat_core::comms::{
    PeerAddress, PeerId, PeerName, PeerTransport, SendError, TrustedPeerDescriptor,
};

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
    /// A `PeerEndpoint` carried invalid slug payload (malformed
    /// peer_id UUID or address without transport scheme). The DSL
    /// should never emit these; surfacing as a typed error makes
    /// the contract visible.
    #[error("invalid peer endpoint `{peer_id}`: {detail}")]
    InvalidEndpoint { peer_id: String, detail: String },
    /// The bound runtime does not expose its canonical trust-store snapshot.
    #[error("canonical trust-store snapshot unavailable: {0}")]
    TrustSnapshotUnavailable(#[from] CommsCapabilityError),
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
/// Holds a reference to a `CommsRuntime`. Deltas are computed from the
/// runtime's canonical trust-store snapshot on every pass.
pub struct CommsTrustReconciler {
    comms: Arc<dyn CommsRuntime>,
}

impl std::fmt::Debug for CommsTrustReconciler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommsTrustReconciler")
            .finish_non_exhaustive()
    }
}

impl CommsTrustReconciler {
    /// Construct a reconciler bound to the given comms runtime.
    pub fn new(comms: Arc<dyn CommsRuntime>) -> Self {
        Self { comms }
    }

    /// Reconcile the trust store against `effective_peers`.
    ///
    /// Returns a [`ReconcileReport`] describing the add / remove
    /// calls the reconciler made.
    ///
    /// Trust-store failures surface as
    /// [`CommsTrustReconcileError`]. There is no helper-local applied
    /// truth: if a prior mutation failed, the next pass re-reads the
    /// canonical trust store and retries whatever delta remains.
    pub async fn reconcile(
        &self,
        epoch: u64,
        effective_peers: BTreeSet<PeerEndpoint>,
    ) -> Result<ReconcileReport, CommsTrustReconcileError> {
        let previous_peers = self.canonical_trusted_peer_snapshot().await?;

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
        // an older session hasn't completed.
        for endpoint in to_add {
            let descriptor = endpoint_to_descriptor(&endpoint)?;
            self.comms
                .add_trusted_peer(descriptor)
                .await
                .map_err(|source| CommsTrustReconcileError::AddTrustFailed {
                    peer_id: endpoint.peer_id.0.clone(),
                    source,
                })?;
            added.push(endpoint);
        }

        for endpoint in to_remove {
            self.comms
                .remove_trusted_peer(endpoint.peer_id.0.as_str())
                .await
                .map_err(|source| CommsTrustReconcileError::RemoveTrustFailed {
                    peer_id: endpoint.peer_id.0.clone(),
                    source,
                })?;
            removed.push(endpoint);
        }

        Ok(ReconcileReport {
            added,
            removed,
            applied_epoch: epoch,
        })
    }

    async fn canonical_trusted_peer_snapshot(
        &self,
    ) -> Result<BTreeSet<PeerEndpoint>, CommsTrustReconcileError> {
        let snapshot = self.comms.peer_ingress_runtime_snapshot().await?;
        snapshot
            .trusted_peers
            .iter()
            .map(descriptor_to_endpoint)
            .collect()
    }
}

/// Parse a DSL `PeerEndpoint` into a core `TrustedPeerDescriptor`.
///
/// The DSL carries string-backed identity atoms so the schema
/// validator sees opaque newtype shapes; the trust store requires
/// parsed typed atoms. The signing key is already part of the
/// MeerkatMachine-owned projection and is forwarded unchanged.
fn endpoint_to_descriptor(
    endpoint: &PeerEndpoint,
) -> Result<TrustedPeerDescriptor, CommsTrustReconcileError> {
    let name = PeerName::new(endpoint.name.0.as_str()).map_err(|detail| {
        CommsTrustReconcileError::InvalidEndpoint {
            peer_id: endpoint.peer_id.0.clone(),
            detail: format!("invalid name: {detail}"),
        }
    })?;
    let peer_id = PeerId::parse(endpoint.peer_id.0.as_str()).map_err(|err| {
        CommsTrustReconcileError::InvalidEndpoint {
            peer_id: endpoint.peer_id.0.clone(),
            detail: format!("invalid peer_id: {err}"),
        }
    })?;
    let address = parse_peer_address(endpoint.address.0.as_str()).map_err(|detail| {
        CommsTrustReconcileError::InvalidEndpoint {
            peer_id: endpoint.peer_id.0.clone(),
            detail,
        }
    })?;
    Ok(TrustedPeerDescriptor {
        name,
        peer_id,
        address,
        pubkey: endpoint.signing_key.0,
    })
}

fn parse_peer_address(raw: &str) -> Result<PeerAddress, String> {
    let (scheme, endpoint) = raw
        .split_once("://")
        .ok_or_else(|| format!("address missing transport scheme: {raw}"))?;
    let transport = match scheme {
        "inproc" => PeerTransport::Inproc,
        "uds" => PeerTransport::Uds,
        "tcp" => PeerTransport::Tcp,
        other => return Err(format!("unknown transport `{other}` in address `{raw}`")),
    };
    Ok(PeerAddress::new(transport, endpoint))
}

fn descriptor_to_endpoint(
    descriptor: &TrustedPeerDescriptor,
) -> Result<PeerEndpoint, CommsTrustReconcileError> {
    Ok(PeerEndpoint {
        name: crate::meerkat_machine::dsl::PeerName(descriptor.name.as_str().to_owned()),
        peer_id: crate::meerkat_machine::dsl::PeerId(descriptor.peer_id.to_string()),
        address: crate::meerkat_machine::dsl::PeerAddress(descriptor.address.to_string()),
        signing_key: crate::meerkat_machine::dsl::PeerSigningKey(descriptor.pubkey),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use meerkat_core::{
        PeerIngressAuthorityPhase, PeerIngressQueueSnapshot, PeerIngressRuntimeSnapshot,
    };
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Build a `PeerEndpoint` with a valid UUID `peer_id` and an
    /// inproc address. The name is a display slug; the `peer_id`
    /// must parse as a UUID because core's `PeerId::parse` requires
    /// a hyphenated UUID.
    fn endpoint(name: &str, peer_id_uuid: &str) -> PeerEndpoint {
        PeerEndpoint {
            name: crate::meerkat_machine::dsl::PeerName(format!("ep-{name}")),
            peer_id: crate::meerkat_machine::dsl::PeerId(peer_id_uuid.to_string()),
            address: crate::meerkat_machine::dsl::PeerAddress(format!("inproc://{name}")),
            signing_key: crate::meerkat_machine::dsl::PeerSigningKey([name.as_bytes()[0]; 32]),
        }
    }

    const UUID_A: &str = "aaaaaaaa-0000-4000-8000-000000000001";
    const UUID_B: &str = "bbbbbbbb-0000-4000-8000-000000000002";
    const UUID_C: &str = "cccccccc-0000-4000-8000-000000000003";

    /// Records every `add_trusted_peer` / `remove_trusted_peer` call
    /// for assertion in tests. Uses `std::sync::Mutex` for the
    /// recorder state (not the async tokio Mutex used by the
    /// reconciler's applied view) — the recorder only needs the
    /// sync form to guard its accumulated calls vector.
    #[derive(Default)]
    struct RecordingCommsRuntime {
        adds: std::sync::Mutex<Vec<TrustedPeerDescriptor>>,
        removes: std::sync::Mutex<Vec<String>>,
        trusted: std::sync::Mutex<BTreeSet<PeerEndpoint>>,
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

        async fn add_trusted_peer(&self, peer: TrustedPeerDescriptor) -> Result<(), SendError> {
            if self.fail_next_add.swap(false, Ordering::SeqCst) {
                return Err(SendError::Unsupported("synthetic failure".into()));
            }
            self.adds
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(peer.clone());
            self.trusted
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(descriptor_to_endpoint(&peer).expect("test descriptor should map"));
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
            self.trusted
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .retain(|endpoint| endpoint.peer_id.0 != peer_id);
            Ok(true)
        }

        async fn peer_ingress_runtime_snapshot(
            &self,
        ) -> Result<PeerIngressRuntimeSnapshot, CommsCapabilityError> {
            let trusted_peers = self
                .trusted
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .map(endpoint_to_descriptor)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|err| CommsCapabilityError::Unsupported(err.to_string()))?;
            Ok(PeerIngressRuntimeSnapshot {
                self_peer_id: PeerId::parse("00000000-0000-4000-8000-000000000000")
                    .expect("valid test peer id"),
                auth_required: true,
                authority_phase: PeerIngressAuthorityPhase::Received,
                trusted_peers,
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
        let peers = BTreeSet::from([endpoint("A", UUID_A), endpoint("B", UUID_B)]);

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
            add_calls
                .iter()
                .any(|d| d.peer_id.to_string() == UUID_A && d.name.as_str() == "ep-A"),
            "add_trusted_peer must be called with the parsed (name, peer_id, address) triple",
        );
        assert!(
            add_calls
                .iter()
                .any(|d| d.peer_id.to_string() == UUID_A && d.pubkey == [b'A'; 32]),
            "reconciler must forward the machine-owned signing key for peer A",
        );
        assert!(
            add_calls
                .iter()
                .any(|d| d.peer_id.to_string() == UUID_B && d.pubkey == [b'B'; 32]),
            "reconciler must forward the machine-owned signing key for peer B",
        );
    }

    #[tokio::test]
    async fn subsequent_reconcile_adds_new_and_removes_departed() {
        let comms = Arc::new(RecordingCommsRuntime::default());
        let reconciler = CommsTrustReconciler::new(comms.clone());

        let peers_v1 = BTreeSet::from([endpoint("A", UUID_A), endpoint("B", UUID_B)]);
        reconciler
            .reconcile(1, peers_v1)
            .await
            .expect("v1 reconcile");

        let peers_v2 = BTreeSet::from([endpoint("A", UUID_A), endpoint("C", UUID_C)]);
        let report = reconciler
            .reconcile(2, peers_v2)
            .await
            .expect("v2 reconcile");

        // Added: C. Removed: B. Retained: A.
        assert_eq!(report.added, vec![endpoint("C", UUID_C)]);
        assert_eq!(report.removed, vec![endpoint("B", UUID_B)]);
        assert_eq!(report.applied_epoch, 2);

        // Trust-store calls: v1 {A,B} adds, v2 {C} add + {B} remove.
        // Across both passes the add calls are A, B, C and the remove
        // calls are B.
        assert_eq!(comms.add_calls().len(), 3);
        assert_eq!(comms.remove_calls(), vec![UUID_B.to_string()]);
    }

    #[tokio::test]
    async fn reconcile_reads_canonical_store_not_local_applied_view() {
        let comms = Arc::new(RecordingCommsRuntime::default());
        let reconciler = CommsTrustReconciler::new(comms.clone());

        reconciler
            .reconcile(5, BTreeSet::from([endpoint("A", UUID_A)]))
            .await
            .expect("first reconcile");

        // Mutate the canonical trust store outside the reconciler. A helper-
        // local applied view would still believe A is present and B absent;
        // the correct reconciler re-reads canonical truth and restores A while
        // removing B.
        comms
            .trusted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        comms
            .trusted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(endpoint("B", UUID_B));

        let report = reconciler
            .reconcile(4, BTreeSet::from([endpoint("A", UUID_A)]))
            .await
            .expect("reconcile should read canonical trust store");
        assert_eq!(report.added, vec![endpoint("A", UUID_A)]);
        assert_eq!(report.removed, vec![endpoint("B", UUID_B)]);
        assert_eq!(report.applied_epoch, 4);

        assert_eq!(comms.add_calls().len(), 2);
        assert_eq!(comms.remove_calls(), vec![UUID_B.to_string()]);
    }

    #[tokio::test]
    async fn empty_effective_set_clears_all_previously_trusted_peers() {
        let comms = Arc::new(RecordingCommsRuntime::default());
        let reconciler = CommsTrustReconciler::new(comms.clone());

        reconciler
            .reconcile(
                1,
                BTreeSet::from([endpoint("A", UUID_A), endpoint("B", UUID_B)]),
            )
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
        assert_eq!(removes, vec![UUID_A.to_string(), UUID_B.to_string()]);
    }

    /// Invalid peer endpoint (non-UUID peer_id) surfaces
    /// `InvalidEndpoint` rather than reaching the trust store.
    #[tokio::test]
    async fn invalid_endpoint_surfaces_typed_error_without_touching_trust_store() {
        let comms = Arc::new(RecordingCommsRuntime::default());
        let reconciler = CommsTrustReconciler::new(comms.clone());
        let bad = PeerEndpoint {
            name: crate::meerkat_machine::dsl::PeerName("ep-bad".into()),
            peer_id: crate::meerkat_machine::dsl::PeerId("not-a-uuid".into()),
            address: crate::meerkat_machine::dsl::PeerAddress("inproc://bad".into()),
            signing_key: crate::meerkat_machine::dsl::PeerSigningKey([9u8; 32]),
        };
        let err = reconciler
            .reconcile(1, BTreeSet::from([bad]))
            .await
            .expect_err("invalid endpoint must surface a typed error");
        assert!(
            matches!(err, CommsTrustReconcileError::InvalidEndpoint { .. }),
            "expected InvalidEndpoint, got {err:?}",
        );
        assert!(comms.add_calls().is_empty());
    }
}

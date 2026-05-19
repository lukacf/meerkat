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
//! * §6 #2 — A lower-epoch generated reconcile handoff fails closed at
//!   the trust mutation seam instead of mutating current canonical trust.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use meerkat_core::agent::{CommsCapabilityError, CommsRuntime};
use meerkat_core::comms::{
    CommsTrustMutation, CommsTrustMutationAuthority, CommsTrustMutationResult,
    GeneratedCommsTrustAuthoritySourceKind, SendError, TrustedPeerDescriptor,
};
use meerkat_core::{
    PeerIngressAuthorityPhase, PeerIngressQueueSnapshot, PeerIngressRuntimeSnapshot,
};
use meerkat_runtime::comms_trust_reconcile::{CommsTrustReconciler, ReconcileReport};
use meerkat_runtime::meerkat_machine::dsl::{
    MeerkatMachineAuthority, MeerkatMachineInput, MeerkatMachineMutator, MeerkatMachineSignal,
    PeerAddress, PeerEndpoint, PeerId, PeerName, PeerSigningKey, SessionId,
};
use meerkat_runtime::protocol_comms_trust_reconcile::CommsTrustReconcileObligation;

const UUID_A: &str = "f805a14c-4089-5328-b4cb-39ede8b4464d";
const UUID_B: &str = "a576ebe3-ccd6-565d-8f48-5f29c0db055d";

fn endpoint(name: &str, peer_id_uuid: &str) -> PeerEndpoint {
    PeerEndpoint {
        name: PeerName(format!("ep-{name}")),
        peer_id: PeerId(peer_id_uuid.to_string()),
        address: PeerAddress(format!("inproc://{name}")),
        signing_key: PeerSigningKey([name.as_bytes()[0]; 32]),
    }
}

fn obligation(
    epoch: u64,
    direct_peer_endpoints: BTreeSet<PeerEndpoint>,
) -> CommsTrustReconcileObligation {
    let mut authority = MeerkatMachineAuthority::new();
    authority
        .apply_signal(MeerkatMachineSignal::Initialize)
        .expect("Initialize signal");
    MeerkatMachineMutator::apply(
        &mut authority,
        MeerkatMachineInput::RegisterSession {
            session_id: SessionId::from("trust-reconcile-concurrency-test"),
        },
    )
    .expect("RegisterSession input");
    let projection_epoch = epoch.max(1);
    let mut transition = None;
    for overlay_epoch in 1..=projection_epoch {
        transition = Some(
            MeerkatMachineMutator::apply(
                &mut authority,
                MeerkatMachineInput::ApplyMobPeerOverlay {
                    epoch: overlay_epoch,
                    endpoints: direct_peer_endpoints.clone(),
                },
            )
            .expect("ApplyMobPeerOverlay input"),
        );
    }
    let transition = transition.expect("projection epoch loop produces transition");
    meerkat_runtime::protocol_comms_trust_reconcile::extract_obligations(&transition)
        .into_iter()
        .next()
        .expect("generated reconcile obligation")
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
    epoch_watermarks: std::sync::Mutex<HashMap<GeneratedCommsTrustAuthoritySourceKind, u64>>,
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

    async fn apply_trust_mutation(
        &self,
        mutation: CommsTrustMutation,
    ) -> Result<CommsTrustMutationResult, SendError> {
        match mutation {
            CommsTrustMutation::AddTrustedPeer { peer, authority } => {
                authority
                    .validate_public_add(&peer)
                    .map_err(SendError::Validation)?;
                self.validate_authority_epoch(&authority)?;
                self.add_count.fetch_add(1, Ordering::SeqCst);
                self.adds
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(peer.clone());
                self.trusted
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(peer);
                Ok(CommsTrustMutationResult::Added)
            }
            CommsTrustMutation::RemoveTrustedPeer { peer_id, authority } => {
                let parsed_peer_id = meerkat_core::comms::PeerId::parse(&peer_id)
                    .map_err(|error| SendError::Validation(error.to_string()))?;
                authority
                    .validate_public_remove(parsed_peer_id)
                    .map_err(SendError::Validation)?;
                self.validate_authority_epoch(&authority)?;
                self.remove_count.fetch_add(1, Ordering::SeqCst);
                self.removes
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(peer_id.clone());
                let mut trusted = self
                    .trusted
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let before = trusted.len();
                trusted.retain(|peer| peer.peer_id.to_string() != peer_id);
                Ok(CommsTrustMutationResult::Removed {
                    removed: before != trusted.len(),
                })
            }
            _ => Err(SendError::Unsupported(
                "test runtime only supports generated public trust mutations".into(),
            )),
        }
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

    async fn public_trusted_peer_projection_snapshot(
        &self,
    ) -> Result<Vec<TrustedPeerDescriptor>, CommsCapabilityError> {
        Ok(self
            .trusted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone())
    }
}

impl RecordingCommsRuntime {
    fn validate_authority_epoch(
        &self,
        authority: &CommsTrustMutationAuthority,
    ) -> Result<(), SendError> {
        let source_kind = authority.source_kind();
        let epoch = authority.epoch();
        let mut watermarks = self
            .epoch_watermarks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = watermarks.entry(source_kind).or_insert(epoch);
        if epoch < *previous {
            return Err(SendError::Validation(format!(
                "stale generated comms trust authority for {source_kind:?}: epoch {epoch} is older than observed epoch {previous}"
            )));
        }
        if epoch > *previous {
            *previous = epoch;
        }
        Ok(())
    }

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
            .reconcile(&obligation(1, BTreeSet::from([endpoint("A", UUID_A)])))
            .await
    });
    let newer_task = tokio::spawn(async move {
        reconciler_newer
            .reconcile(&obligation(2, BTreeSet::from([endpoint("B", UUID_B)])))
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

/// §6 #2 — lower-epoch reconciles fail closed against canonical trust.
#[tokio::test]
async fn lower_epoch_reconcile_is_rejected_as_stale() {
    let comms = Arc::new(RecordingCommsRuntime::default());
    let reconciler = CommsTrustReconciler::new(comms.clone());

    // Newer commits first at epoch=5 with peer set {A}.
    let newer = reconciler
        .reconcile(&obligation(5, BTreeSet::from([endpoint("A", UUID_A)])))
        .await
        .expect("newer reconcile");
    assert_eq!(newer.applied_epoch, 5);
    assert_eq!(comms.add_count.load(Ordering::SeqCst), 1);
    assert_eq!(comms.remove_count.load(Ordering::SeqCst), 0);

    // Lower epoch arrives with a different set {B}. Expected: the generated
    // authority replay guard rejects it before stale trust can mutate.
    let lower = reconciler
        .reconcile(&obligation(3, BTreeSet::from([endpoint("B", UUID_B)])))
        .await
        .expect_err("lower-epoch reconcile must fail closed");
    assert!(
        matches!(
            lower,
            meerkat_runtime::comms_trust_reconcile::CommsTrustReconcileError::AddTrustFailed {
                source: SendError::Validation(ref message),
                ..
            } if message.contains("stale generated comms trust authority")
        ),
        "unexpected stale reconcile rejection: {lower:?}",
    );

    assert_eq!(
        comms.add_count.load(Ordering::SeqCst),
        1,
        "stale lower-epoch reconcile must not add peer B",
    );
    assert_eq!(
        comms.remove_count.load(Ordering::SeqCst),
        0,
        "stale lower-epoch reconcile must not remove peer A",
    );
    let added_peer_ids: BTreeSet<String> = comms
        .add_calls()
        .into_iter()
        .map(|d| d.peer_id.to_string())
        .collect();
    assert_eq!(
        added_peer_ids,
        BTreeSet::from([UUID_A.to_string()]),
        "only the current generated add should be recorded",
    );
    assert_eq!(
        comms.trusted_peer_ids(),
        BTreeSet::from([UUID_A.to_string()])
    );
}

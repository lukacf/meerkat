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
//! * §6 #2 — A lower-epoch overlay fails closed in generated MeerkatMachine
//!   authority instead of mutating current canonical trust.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use meerkat_core::agent::{CommsCapabilityError, CommsRuntime};
use meerkat_core::comms::{
    CommsTrustMutation, CommsTrustMutationResult, GeneratedCommsTrustAuthoritySourceKind,
    SendError, TrustedPeerDescriptor,
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
    meerkat_runtime::protocol_comms_trust_reconcile::extract_obligations_with_freshness(
        &transition,
        meerkat_runtime::protocol_comms_trust_reconcile::PeerProjectionFreshnessAuthority::from_authority(
            Arc::new(std::sync::Mutex::new(authority)),
        ),
    )
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
    add_count: AtomicUsize,
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

    async fn trusted_peer_projection_snapshot_for_source(
        &self,
        source_kind: GeneratedCommsTrustAuthoritySourceKind,
    ) -> Result<Vec<TrustedPeerDescriptor>, CommsCapabilityError> {
        if source_kind == GeneratedCommsTrustAuthoritySourceKind::MeerkatMachinePeerProjection {
            Ok(self
                .trusted
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone())
        } else {
            Ok(Vec::new())
        }
    }
}

impl RecordingCommsRuntime {
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

/// §6 #2 — lower-epoch overlays fail closed in generated machine authority.
#[tokio::test]
async fn lower_epoch_overlay_is_rejected_by_generated_machine() {
    let mut authority = MeerkatMachineAuthority::new();
    authority
        .apply_signal(MeerkatMachineSignal::Initialize)
        .expect("Initialize signal");
    MeerkatMachineMutator::apply(
        &mut authority,
        MeerkatMachineInput::RegisterSession {
            session_id: SessionId::from("trust-reconcile-lower-epoch-test"),
        },
    )
    .expect("RegisterSession input");

    let current = MeerkatMachineMutator::apply(
        &mut authority,
        MeerkatMachineInput::ApplyMobPeerOverlay {
            epoch: 5,
            endpoints: BTreeSet::from([endpoint("A", UUID_A)]),
        },
    )
    .expect("current overlay applies");
    let current_obligation =
        meerkat_runtime::protocol_comms_trust_reconcile::extract_obligations(&current)
            .into_iter()
            .next()
            .expect("generated reconcile obligation");
    assert_eq!(current_obligation.peer_projection_epoch(), 1);

    let stale = MeerkatMachineMutator::apply(
        &mut authority,
        MeerkatMachineInput::ApplyMobPeerOverlay {
            epoch: 3,
            endpoints: BTreeSet::from([endpoint("B", UUID_B)]),
        },
    )
    .expect_err("lower overlay epoch must fail closed in generated authority");
    let stale_debug = format!("{stale:?}");
    assert!(
        stale_debug.contains("GuardRejected") && stale_debug.contains("ApplyMobPeerOverlay"),
        "unexpected stale overlay rejection: {stale_debug}",
    );
}

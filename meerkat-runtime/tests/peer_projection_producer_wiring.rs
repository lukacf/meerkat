#![allow(clippy::implicit_clone, clippy::redundant_clone)]

//! D-track-b · Peer-projection producer wiring end-to-end contract.
//!
//! Pins the wave-d closure of the emitter→consumer gap documented in
//! `docs/wave-d-prep/track-b-producer-wiring.md`. The three seams being
//! tested are:
//!
//! 1. **Stager helpers** on [`MeerkatMachine`] — `stage_add_direct_peer_endpoint`,
//!    `stage_remove_direct_peer_endpoint`,
//!    `stage_authorized_supervisor_mob_peer_overlay` —
//!    apply the matching DSL input and drive the trust-store
//!    [`CommsTrustReconciler`] for the current [`CommsRuntime`].
//! 2. **Rebind correctness** — reconciliation is stateless aside from the
//!    caller-supplied runtime. When a session rebinds to a fresh runtime, the
//!    next producer call reconciles that runtime's canonical trust snapshot
//!    instead of pinning the first runtime observed by the session.
//! 3. **End-to-end producer → consumer** — an
//!    `AddDirectPeerEndpoint` stager call causes `add_trusted_peer` on
//!    the underlying [`CommsRuntime`], and a follow-up
//!    `RemoveDirectPeerEndpoint` causes `remove_trusted_peer`.
//!
//! Wire shape (WireMember / UnwireMember bridge handlers) is covered by
//! the existing bridge handler tests in
//! `meerkat-runtime/src/comms_drain.rs`; this file pins the stager →
//! reconciler → trust-store chain at the integration layer.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::agent::{CommsCapabilityError, CommsRuntime};
use meerkat_core::comms::{
    CommsTrustMutation, CommsTrustMutationResult, GeneratedCommsTrustAuthoritySourceKind,
    SendError, TrustedPeerDescriptor,
};
use meerkat_core::types::SessionId;
use meerkat_core::{
    PeerIngressAuthorityPhase, PeerIngressQueueSnapshot, PeerIngressRuntimeSnapshot,
};
use meerkat_runtime::MeerkatMachine;
use meerkat_runtime::meerkat_machine::dsl::{
    MobPeerOverlayCommandKind, PeerAddress, PeerEndpoint, PeerId, PeerName, PeerSigningKey,
};

const UUID_A: &str = "f805a14c-4089-5328-b4cb-39ede8b4464d";
const UUID_B: &str = "a576ebe3-ccd6-565d-8f48-5f29c0db055d";
const UUID_C: &str = "76f30618-7a02-578b-a8bc-6d82ae7ba8cf";
const UUID_LOCAL: &str = "e7599cdf-1397-5ff3-95cb-f2cdcf337c7f";
const UUID_SUPERVISOR: &str = "eeeeeeee-0000-4000-8000-000000000005";

fn endpoint(name: &str, peer_id_uuid: &str) -> PeerEndpoint {
    PeerEndpoint {
        name: PeerName(format!("ep-{name}")),
        peer_id: PeerId(peer_id_uuid.to_string()),
        address: PeerAddress(format!("inproc://{name}")),
        signing_key: PeerSigningKey([name.as_bytes()[0]; 32]),
    }
}

#[derive(Default)]
struct RecordingCommsRuntime {
    adds: std::sync::Mutex<Vec<TrustedPeerDescriptor>>,
    removes: std::sync::Mutex<Vec<String>>,
    trusted: std::sync::Mutex<Vec<TrustedPeerDescriptor>>,
}

impl std::fmt::Debug for RecordingCommsRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecordingCommsRuntime").finish()
    }
}

#[async_trait]
impl CommsRuntime for RecordingCommsRuntime {
    fn peer_id(&self) -> Option<meerkat_core::comms::PeerId> {
        meerkat_core::comms::PeerId::parse(UUID_LOCAL).ok()
    }

    fn public_key_bytes(&self) -> Option<[u8; 32]> {
        Some([b'L'; 32])
    }

    fn comms_name(&self) -> Option<String> {
        Some("local".to_string())
    }

    fn advertised_address(&self) -> Option<String> {
        Some("inproc://local".to_string())
    }

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
                    .validate_public_add(self.peer_id(), &peer)
                    .map_err(SendError::Validation)?;
                self.adds
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(peer.clone());
                let mut trusted = self
                    .trusted
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let created = !trusted
                    .iter()
                    .any(|existing| existing.peer_id == peer.peer_id);
                trusted.retain(|existing| existing.peer_id != peer.peer_id);
                trusted.push(peer);
                Ok(CommsTrustMutationResult::Added { created })
            }
            CommsTrustMutation::RemoveTrustedPeer { peer_id, authority } => {
                let parsed_peer_id = meerkat_core::comms::PeerId::parse(peer_id.as_str())
                    .map_err(|err| SendError::Validation(err.to_string()))?;
                authority
                    .validate_public_remove(self.peer_id(), parsed_peer_id)
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
                trusted.retain(|peer| peer.peer_id.as_str() != peer_id);
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
            self_peer_id: self.peer_id().expect("valid test peer id"),
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
            self.public_trusted_peer_projection_snapshot().await
        } else {
            Ok(Vec::new())
        }
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

    fn clear_projection(&self) {
        self.trusted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    fn seed_projection(&self, endpoint: &PeerEndpoint) {
        let descriptor = TrustedPeerDescriptor::unsigned_with_pubkey(
            endpoint.name.0.as_str(),
            endpoint.peer_id.0.clone(),
            endpoint.signing_key.0,
            endpoint.address.0.as_str(),
        )
        .expect("valid endpoint descriptor");
        self.trusted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(descriptor);
    }
}

async fn register(machine: &Arc<MeerkatMachine>, sid: &SessionId) {
    machine
        .register_session(sid.clone())
        .await
        .expect("register session");
}

#[tokio::test]
async fn stage_add_direct_peer_endpoint_drives_trust_store() {
    let machine = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    register(&machine, &sid).await;
    let recorder = Arc::new(RecordingCommsRuntime::default());
    let comms: Arc<dyn CommsRuntime> = recorder.clone();

    machine
        .stage_add_direct_peer_endpoint(&sid, endpoint("A", UUID_A), comms)
        .await
        .expect("stage_add_direct_peer_endpoint accepted");

    let add_calls = recorder.add_calls();
    assert_eq!(
        add_calls.len(),
        1,
        "add_trusted_peer must fire exactly once"
    );
    assert_eq!(add_calls[0].peer_id.as_str(), UUID_A);
    assert_eq!(
        add_calls[0].pubkey, [b'A'; 32],
        "stager-to-reconciler path must preserve the endpoint signing key",
    );
    assert!(recorder.remove_calls().is_empty());
}

#[tokio::test]
async fn stage_remove_direct_peer_endpoint_drives_trust_store() {
    let machine = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    register(&machine, &sid).await;
    let recorder = Arc::new(RecordingCommsRuntime::default());

    let ep = endpoint("A", UUID_A);
    machine
        .stage_add_direct_peer_endpoint(&sid, ep.clone(), recorder.clone())
        .await
        .expect("add accepted");
    machine
        .stage_remove_direct_peer_endpoint(&sid, ep, recorder.clone())
        .await
        .expect("remove accepted");

    assert_eq!(recorder.add_calls().len(), 1);
    assert_eq!(recorder.remove_calls(), vec![UUID_A.to_string()]);
}

#[tokio::test]
async fn direct_peer_endpoint_repair_retry_reconciles_stale_projection() {
    let machine = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    register(&machine, &sid).await;
    let recorder = Arc::new(RecordingCommsRuntime::default());
    let ep = endpoint("A", UUID_A);

    machine
        .stage_add_direct_peer_endpoint(&sid, ep.clone(), recorder.clone())
        .await
        .expect("first add accepted");
    recorder.clear_projection();
    machine
        .stage_add_direct_peer_endpoint(&sid, ep.clone(), recorder.clone())
        .await
        .expect("duplicate add repair accepted");
    assert_eq!(
        recorder.add_calls().len(),
        2,
        "duplicate add repair must re-emit reconcile when projection is stale",
    );

    machine
        .stage_remove_direct_peer_endpoint(&sid, ep.clone(), recorder.clone())
        .await
        .expect("first remove accepted");
    recorder.seed_projection(&ep);
    machine
        .stage_remove_direct_peer_endpoint(&sid, ep, recorder.clone())
        .await
        .expect("absent remove repair accepted");
    assert_eq!(
        recorder.remove_calls(),
        vec![UUID_A.to_string(), UUID_A.to_string()],
        "absent remove repair must re-emit reconcile when projection is stale",
    );
}

#[tokio::test]
async fn stage_authorized_supervisor_mob_peer_overlay_drives_trust_store_with_delta() {
    let machine = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    register(&machine, &sid).await;
    let recorder = Arc::new(RecordingCommsRuntime::default());
    machine
        .stage_supervisor_bind(
            &sid,
            "supervisor".to_string(),
            UUID_SUPERVISOR.to_string(),
            "inproc://supervisor".to_string(),
            "supervisor-key".to_string(),
            1,
        )
        .await
        .expect("supervisor bound");

    let mut v1 = BTreeSet::new();
    let ep_a = endpoint("A", UUID_A);
    v1.insert(ep_a.clone());
    v1.insert(endpoint("B", UUID_B));
    machine
        .stage_authorized_supervisor_mob_peer_overlay(
            &sid,
            UUID_SUPERVISOR.to_string(),
            1,
            UUID_LOCAL.to_string(),
            1,
            v1,
            2,
            UUID_A.to_string(),
            ep_a,
            MobPeerOverlayCommandKind::Wire,
            recorder.clone(),
        )
        .await
        .expect("overlay v1 accepted");

    // Replace {A,B} with {A,C} at a fresh epoch.
    let mut v2 = BTreeSet::new();
    v2.insert(endpoint("A", UUID_A));
    let ep_c = endpoint("C", UUID_C);
    v2.insert(ep_c.clone());
    machine
        .stage_authorized_supervisor_mob_peer_overlay(
            &sid,
            UUID_SUPERVISOR.to_string(),
            1,
            UUID_LOCAL.to_string(),
            2,
            v2,
            2,
            UUID_C.to_string(),
            ep_c,
            MobPeerOverlayCommandKind::Wire,
            recorder.clone(),
        )
        .await
        .expect("overlay v2 accepted");
    // v1 registered A and B; v2 added C and removed B. Reconciler
    // runs add-before-remove within a pass, so the add vector is
    // [A, B, C] and removes are [B].
    let adds: Vec<String> = recorder
        .add_calls()
        .into_iter()
        .map(|d| d.peer_id.as_str().to_owned())
        .collect();
    let mut adds_sorted = adds.clone();
    adds_sorted.sort();
    let mut expected_adds = vec![UUID_A.to_string(), UUID_B.to_string(), UUID_C.to_string()];
    expected_adds.sort();
    assert_eq!(
        adds_sorted, expected_adds,
        "overlay producer must register A, B, C across the two passes",
    );
    assert_eq!(recorder.remove_calls(), vec![UUID_B.to_string()]);
}

#[tokio::test]
async fn reconciler_uses_current_runtime_after_rebind() {
    // Regression pin for runtime rebinds: the stager must not cache the
    // first runtime it sees. The reconciler reads the supplied runtime's
    // canonical trust store on every call, so a fresh runtime receives the
    // full effective peer projection.
    let machine = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    register(&machine, &sid).await;
    let first_runtime = Arc::new(RecordingCommsRuntime::default());
    let rebound_runtime = Arc::new(RecordingCommsRuntime::default());

    let ep_a = endpoint("A", UUID_A);
    machine
        .stage_add_direct_peer_endpoint(&sid, ep_a, first_runtime.clone())
        .await
        .expect("add A");

    let ep_b = endpoint("B", UUID_B);
    machine
        .stage_add_direct_peer_endpoint(&sid, ep_b, rebound_runtime.clone())
        .await
        .expect("add B after runtime rebind");

    let first_adds: Vec<_> = first_runtime
        .add_calls()
        .into_iter()
        .map(|d| d.peer_id.as_str().to_owned())
        .collect();
    assert_eq!(
        first_adds,
        vec![UUID_A.to_string()],
        "rebound reconcile must not keep mutating the first runtime",
    );

    let mut rebound_adds: Vec<_> = rebound_runtime
        .add_calls()
        .into_iter()
        .map(|d| d.peer_id.as_str().to_owned())
        .collect();
    rebound_adds.sort();
    let mut expected_rebound_adds = vec![UUID_A.to_string(), UUID_B.to_string()];
    expected_rebound_adds.sort();
    assert_eq!(
        rebound_adds, expected_rebound_adds,
        "rebound runtime must be reconciled to the full DSL-owned effective peer set",
    );
}

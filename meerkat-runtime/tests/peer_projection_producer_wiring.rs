#![allow(clippy::implicit_clone, clippy::redundant_clone)]

//! D-track-b · Peer-projection producer wiring end-to-end contract.
//!
//! Pins the wave-d closure of the emitter→consumer gap documented in
//! `docs/wave-d-prep/track-b-producer-wiring.md`. The three seams being
//! tested are:
//!
//! 1. **Stager helpers** on [`MeerkatMachine`] — `stage_add_direct_peer_endpoint`,
//!    `stage_remove_direct_peer_endpoint`, `stage_apply_mob_peer_overlay` —
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
use meerkat_core::comms::{SendError, TrustedPeerDescriptor};
use meerkat_core::types::SessionId;
use meerkat_core::{
    PeerIngressAuthorityPhase, PeerIngressQueueSnapshot, PeerIngressRuntimeSnapshot,
};
use meerkat_runtime::MeerkatMachine;
use meerkat_runtime::meerkat_machine::dsl::{
    PeerAddress, PeerEndpoint, PeerId, PeerName, PeerSigningKey,
};

const UUID_A: &str = "aaaaaaaa-0000-4000-8000-000000000001";
const UUID_B: &str = "bbbbbbbb-0000-4000-8000-000000000002";
const UUID_C: &str = "cccccccc-0000-4000-8000-000000000003";

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
    async fn drain_messages(&self) -> Vec<String> {
        Vec::new()
    }

    fn inbox_notify(&self) -> Arc<tokio::sync::Notify> {
        Arc::new(tokio::sync::Notify::new())
    }

    async fn add_trusted_peer(&self, peer: TrustedPeerDescriptor) -> Result<(), SendError> {
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
        self.removes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(peer_id.to_string());
        let mut trusted = self
            .trusted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let before = trusted.len();
        trusted.retain(|peer| peer.peer_id.as_str() != peer_id);
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

    fn remove_calls(&self) -> Vec<String> {
        self.removes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

async fn register(machine: &Arc<MeerkatMachine>, sid: &SessionId) {
    machine.register_session(sid.clone()).await;
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
async fn stage_apply_mob_peer_overlay_drives_trust_store_with_delta() {
    let machine = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    register(&machine, &sid).await;
    let recorder = Arc::new(RecordingCommsRuntime::default());

    let mut v1 = BTreeSet::new();
    v1.insert(endpoint("A", UUID_A));
    v1.insert(endpoint("B", UUID_B));
    machine
        .stage_apply_mob_peer_overlay(&sid, 1, v1, recorder.clone())
        .await
        .expect("overlay v1 accepted");

    // Replace {A,B} with {A,C} at a fresh epoch.
    let mut v2 = BTreeSet::new();
    v2.insert(endpoint("A", UUID_A));
    v2.insert(endpoint("C", UUID_C));
    machine
        .stage_apply_mob_peer_overlay(&sid, 2, v2, recorder.clone())
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
    assert_eq!(
        adds_sorted,
        vec![UUID_A.to_string(), UUID_B.to_string(), UUID_C.to_string()],
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
    assert_eq!(
        rebound_adds,
        vec![UUID_A.to_string(), UUID_B.to_string()],
        "rebound runtime must be reconciled to the full DSL-owned effective peer set",
    );
}

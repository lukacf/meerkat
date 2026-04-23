//! D-track-b · Peer-projection producer wiring end-to-end contract.
//!
//! Pins the wave-d closure of the emitter→consumer gap documented in
//! `docs/wave-d-prep/track-b-producer-wiring.md`. The three seams being
//! tested are:
//!
//! 1. **Stager helpers** on [`MeerkatMachine`] — `stage_add_direct_peer_endpoint`,
//!    `stage_remove_direct_peer_endpoint`, `stage_apply_mob_peer_overlay` —
//!    apply the matching DSL input and drive the session-scoped
//!    [`CommsTrustReconciler`] under the same critical section.
//! 2. **Session-scoped reconciler lifetime** — one
//!    [`CommsTrustReconciler`] per registered session, created on first
//!    stager call and re-used across subsequent producer calls so the
//!    applied-view watermark is monotonically advanced.
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
use meerkat_core::agent::CommsRuntime;
use meerkat_core::comms::{SendError, TrustedPeerDescriptor};
use meerkat_core::types::SessionId;
use meerkat_runtime::MeerkatMachine;
use meerkat_runtime::meerkat_machine::dsl::{PeerAddress, PeerEndpoint, PeerId, PeerName};

const UUID_A: &str = "aaaaaaaa-0000-4000-8000-000000000001";
const UUID_B: &str = "bbbbbbbb-0000-4000-8000-000000000002";
const UUID_C: &str = "cccccccc-0000-4000-8000-000000000003";

fn endpoint(name: &str, peer_id_uuid: &str) -> PeerEndpoint {
    PeerEndpoint {
        name: PeerName(format!("ep-{name}")),
        peer_id: PeerId(peer_id_uuid.to_string()),
        address: PeerAddress(format!("inproc://{name}")),
    }
}

#[derive(Default)]
struct RecordingCommsRuntime {
    adds: std::sync::Mutex<Vec<TrustedPeerDescriptor>>,
    removes: std::sync::Mutex<Vec<String>>,
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
            .push(peer);
        Ok(())
    }

    async fn remove_trusted_peer(&self, peer_id: &str) -> Result<bool, SendError> {
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
async fn reconciler_is_shared_across_stager_calls_on_the_same_session() {
    // Regression pin for finding 3 — the reconciler has a per-session
    // lifetime; consecutive stager calls must reuse the same
    // reconciler so the applied-view watermark is monotonic. If a
    // second reconciler were constructed per-call, the applied view
    // would start empty on every call and `remove_trusted_peer`
    // could never fire (the reconciler would see "no previous
    // applied peers" and compute a no-op remove-set).
    let machine = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    register(&machine, &sid).await;
    let recorder = Arc::new(RecordingCommsRuntime::default());

    let ep_a = endpoint("A", UUID_A);
    machine
        .stage_add_direct_peer_endpoint(&sid, ep_a.clone(), recorder.clone())
        .await
        .expect("add A");
    // Without a shared reconciler this remove would be a no-op
    // (empty applied view) and `remove_trusted_peer` would not fire.
    machine
        .stage_remove_direct_peer_endpoint(&sid, ep_a, recorder.clone())
        .await
        .expect("remove A");

    assert_eq!(
        recorder.remove_calls(),
        vec![UUID_A.to_string()],
        "per-session reconciler must retain applied view across calls",
    );
}

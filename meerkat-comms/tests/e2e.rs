//! End-to-end tests for meerkat-comms.
//!
//! These tests verify the full system working together with realistic scenarios.

#![cfg(feature = "integration-real-tests")]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use async_trait::async_trait;
use meerkat_comms::{
    CommsConfig, CommsRuntime, Inbox, Keypair, MessageKind, PeerRequestResponseAuthority, PubKey,
    Router, RuntimeCommsCommandHandle, ToolContext, TrustedPeer, TrustedPeers, handle_connection,
    handle_tools_call_with_context,
};
use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
use meerkat_core::comms::TrustedPeerDescriptor;
use meerkat_core::handles::PeerInteractionHandle;
use meerkat_core::{
    BlobId, BlobPayload, BlobRef, BlobStore, BlobStoreError, ContentBlock, ContentInput,
    DslTransitionError, ImageData, InboundPeerRequestState, InteractionStreamState,
    OutboundPeerRequestState, PeerCorrelationId, ResponseStatus, ToolDispatchContext,
};
use parking_lot::RwLock;
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::net::{TcpListener, UnixListener};
use uuid::Uuid;

static INPROC_REGISTRY_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Helper to create a keypair
fn make_keypair() -> Keypair {
    Keypair::generate()
}

#[derive(Default)]
struct TestBlobStore {
    blobs: tokio::sync::RwLock<HashMap<BlobId, BlobPayload>>,
}

#[async_trait]
impl BlobStore for TestBlobStore {
    async fn put_image(&self, media_type: &str, data: &str) -> Result<BlobRef, BlobStoreError> {
        let blob_id = BlobId::new(format!("sha256:e2e-{media_type}-{data}"));
        let payload = BlobPayload {
            blob_id: blob_id.clone(),
            media_type: media_type.to_string(),
            data: data.to_string(),
        };
        self.blobs.write().await.insert(blob_id.clone(), payload);
        Ok(BlobRef {
            blob_id,
            media_type: media_type.to_string(),
        })
    }

    async fn get(&self, blob_id: &BlobId) -> Result<BlobPayload, BlobStoreError> {
        self.blobs
            .read()
            .await
            .get(blob_id)
            .cloned()
            .ok_or_else(|| BlobStoreError::NotFound(blob_id.clone()))
    }

    async fn delete(&self, blob_id: &BlobId) -> Result<(), BlobStoreError> {
        self.blobs.write().await.remove(blob_id);
        Ok(())
    }

    fn is_persistent(&self) -> bool {
        false
    }
}

#[derive(Default)]
struct TestPeerInteractionHandle {
    outbound: parking_lot::Mutex<HashMap<PeerCorrelationId, OutboundPeerRequestState>>,
    inbound: parking_lot::Mutex<HashMap<PeerCorrelationId, InboundPeerRequestState>>,
}

impl TestPeerInteractionHandle {
    fn rejection(context: &'static str, corr_id: PeerCorrelationId) -> DslTransitionError {
        DslTransitionError::guard_rejected(context, format!("e2e rejected corr_id {corr_id}"))
    }
}

impl meerkat_core::handles::PeerInteractionHandle for TestPeerInteractionHandle {
    fn request_sent(
        &self,
        corr_id: PeerCorrelationId,
        _to: String,
    ) -> Result<(), DslTransitionError> {
        self.outbound
            .lock()
            .insert(corr_id, OutboundPeerRequestState::Sent);
        Ok(())
    }

    fn response_progress(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError> {
        if !self.outbound.lock().contains_key(&corr_id) {
            return Err(Self::rejection(
                "PeerInteractionHandle::response_progress",
                corr_id,
            ));
        }
        self.outbound
            .lock()
            .insert(corr_id, OutboundPeerRequestState::AcceptedProgress);
        Ok(())
    }

    fn response_terminal(
        &self,
        corr_id: PeerCorrelationId,
        _disposition: meerkat_core::handles::PeerTerminalDisposition,
    ) -> Result<(), DslTransitionError> {
        if self.outbound.lock().remove(&corr_id).is_none() {
            return Err(Self::rejection(
                "PeerInteractionHandle::response_terminal",
                corr_id,
            ));
        }
        Ok(())
    }

    fn request_timed_out(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError> {
        if self.outbound.lock().remove(&corr_id).is_none() {
            return Err(Self::rejection(
                "PeerInteractionHandle::request_timed_out",
                corr_id,
            ));
        }
        Ok(())
    }

    fn request_received(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError> {
        self.inbound
            .lock()
            .insert(corr_id, InboundPeerRequestState::Received);
        Ok(())
    }

    fn response_replied(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError> {
        if self.inbound.lock().remove(&corr_id).is_none() {
            return Err(Self::rejection(
                "PeerInteractionHandle::response_replied",
                corr_id,
            ));
        }
        Ok(())
    }

    fn outbound_state(&self, corr_id: PeerCorrelationId) -> Option<OutboundPeerRequestState> {
        self.outbound.lock().get(&corr_id).copied()
    }

    fn inbound_state(&self, corr_id: PeerCorrelationId) -> Option<InboundPeerRequestState> {
        self.inbound.lock().get(&corr_id).copied()
    }

    fn install_cleanup_observer(
        &self,
        _observer: Arc<dyn meerkat_core::handles::PeerInteractionCleanupObserver>,
    ) {
    }
}

#[derive(Default)]
struct TestInteractionStreamHandle {
    states: parking_lot::Mutex<HashMap<PeerCorrelationId, InteractionStreamState>>,
}

impl meerkat_core::handles::InteractionStreamHandle for TestInteractionStreamHandle {
    fn reserved(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError> {
        self.states
            .lock()
            .insert(corr_id, InteractionStreamState::Reserved);
        Ok(())
    }

    fn attached(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError> {
        self.states
            .lock()
            .insert(corr_id, InteractionStreamState::Attached);
        Ok(())
    }

    fn completed(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError> {
        self.states.lock().remove(&corr_id);
        Ok(())
    }

    fn expired(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError> {
        self.states.lock().remove(&corr_id);
        Ok(())
    }

    fn closed_early(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError> {
        self.states.lock().remove(&corr_id);
        Ok(())
    }

    fn state(&self, corr_id: PeerCorrelationId) -> Option<InteractionStreamState> {
        self.states.lock().get(&corr_id).copied()
    }

    fn install_cleanup_observer(
        &self,
        _observer: Arc<dyn meerkat_core::handles::InteractionStreamCleanupObserver>,
    ) {
    }
}

fn install_test_authority(
    runtime: &Arc<CommsRuntime>,
    peer_handle: Arc<TestPeerInteractionHandle>,
) {
    runtime.install_peer_request_response_authority(PeerRequestResponseAuthority::new(
        peer_handle,
        Arc::new(TestInteractionStreamHandle::default()),
    ));
}

fn tool_context(runtime: &Arc<CommsRuntime>) -> ToolContext {
    let material = runtime.tool_material();
    let router = material.router().clone();
    let trusted_peers = material.trusted_peers_shared();
    let runtime = material.into_runtime();
    ToolContext {
        router,
        trusted_peers,
        runtime: Some(RuntimeCommsCommandHandle::new(runtime)),
    }
}

fn bind_uds_or_skip(path: &Path) -> Option<UnixListener> {
    match UnixListener::bind(path) {
        Ok(listener) => Some(listener),
        Err(e) => {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                return None;
            }
            panic!("UnixListener::bind failed: {e}");
        }
    }
}

async fn bind_tcp_or_skip(addr: &str) -> Option<TcpListener> {
    match TcpListener::bind(addr).await {
        Ok(listener) => Some(listener),
        Err(e) => {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                return None;
            }
            panic!("TcpListener::bind failed: {e}");
        }
    }
}

/// Helper to set up mutual trust between two peers.
///
/// Returns shared `Arc<RwLock<TrustedPeers>>` handles — the same shape the
/// router owns and `handle_connection` now reads through.
fn setup_mutual_trust(
    name_a: &str,
    pubkey_a: &PubKey,
    addr_a: &str,
    name_b: &str,
    pubkey_b: &PubKey,
    addr_b: &str,
) -> (Arc<RwLock<TrustedPeers>>, Arc<RwLock<TrustedPeers>>) {
    // Peer A trusts Peer B
    let a_trusts = Arc::new(RwLock::new(TrustedPeers {
        peers: vec![TrustedPeer {
            name: name_b.to_string(),
            pubkey: *pubkey_b,
            addr: addr_b.to_string(),
            meta: meerkat_comms::PeerMeta::default(),
        }],
    }));

    // Peer B trusts Peer A
    let b_trusts = Arc::new(RwLock::new(TrustedPeers {
        peers: vec![TrustedPeer {
            name: name_a.to_string(),
            pubkey: *pubkey_a,
            addr: addr_a.to_string(),
            meta: meerkat_comms::PeerMeta::default(),
        }],
    }));

    (a_trusts, b_trusts)
}

/// E2E test harness that sets up two Meerkat instances
struct E2EHarness {
    peer_a_keypair: Keypair,
    peer_b_keypair: Keypair,
    peer_a_trust: Arc<RwLock<TrustedPeers>>,
    peer_b_trust: Arc<RwLock<TrustedPeers>>,
}

impl E2EHarness {
    fn new_uds(tmp_a: &TempDir, tmp_b: &TempDir) -> Self {
        let peer_a_keypair = make_keypair();
        let peer_b_keypair = make_keypair();

        let addr_a = format!("uds://{}", tmp_a.path().join("peer_a.sock").display());
        let addr_b = format!("uds://{}", tmp_b.path().join("peer_b.sock").display());

        let (peer_a_trust, peer_b_trust) = setup_mutual_trust(
            "peer-a",
            &peer_a_keypair.public_key(),
            &addr_a,
            "peer-b",
            &peer_b_keypair.public_key(),
            &addr_b,
        );

        Self {
            peer_a_keypair,
            peer_b_keypair,
            peer_a_trust,
            peer_b_trust,
        }
    }
}

#[test]
fn test_e2e_harness() {
    let tmp_a = TempDir::new().unwrap();
    let tmp_b = TempDir::new().unwrap();
    let harness = E2EHarness::new_uds(&tmp_a, &tmp_b);

    // Verify mutual trust is set up correctly
    assert!(
        harness
            .peer_a_trust
            .read()
            .is_trusted(&harness.peer_b_keypair.public_key())
    );
    assert!(
        harness
            .peer_b_trust
            .read()
            .is_trusted(&harness.peer_a_keypair.public_key())
    );

    // Verify names
    assert_eq!(
        harness
            .peer_a_trust
            .read()
            .get_by_name("peer-b")
            .map(|p| p.pubkey),
        Some(harness.peer_b_keypair.public_key())
    );
    assert_eq!(
        harness
            .peer_b_trust
            .read()
            .get_by_name("peer-a")
            .map(|p| p.pubkey),
        Some(harness.peer_a_keypair.public_key())
    );
}

#[test]
fn test_e2e_mutual_trust_setup() {
    let peer_a_keypair = make_keypair();
    let peer_b_keypair = make_keypair();

    let (a_trusts, b_trusts) = setup_mutual_trust(
        "alice",
        &peer_a_keypair.public_key(),
        "uds:///tmp/alice.sock",
        "bob",
        &peer_b_keypair.public_key(),
        "uds:///tmp/bob.sock",
    );

    // Alice trusts Bob
    assert!(a_trusts.read().is_trusted(&peer_b_keypair.public_key()));
    assert!(!a_trusts.read().is_trusted(&peer_a_keypair.public_key()));

    // Bob trusts Alice
    assert!(b_trusts.read().is_trusted(&peer_a_keypair.public_key()));
    assert!(!b_trusts.read().is_trusted(&peer_b_keypair.public_key()));
}

#[tokio::test]
#[ignore = "integration-real: real socket I/O"]
async fn integration_real_uds_message_exchange() {
    let _tmp_a = TempDir::new().unwrap();
    let tmp_b = TempDir::new().unwrap();

    let sock_b = tmp_b.path().join("peer_b.sock");
    let addr_b = format!("uds://{}", sock_b.display());

    let peer_a_keypair = make_keypair();
    let peer_b_keypair = make_keypair();
    let peer_b_pubkey = peer_b_keypair.public_key();

    let (peer_a_trust, peer_b_trust) = setup_mutual_trust(
        "peer-a",
        &peer_a_keypair.public_key(),
        "uds:///unused",
        "peer-b",
        &peer_b_keypair.public_key(),
        &addr_b,
    );

    // Start peer B listening
    let Some(listener_b) = bind_uds_or_skip(&sock_b) else {
        return;
    };
    let (_inbox_b, inbox_sender_b) = Inbox::new();

    // Spawn B's IO task
    let handle_b = tokio::spawn(async move {
        let (stream, _) = listener_b.accept().await.unwrap();
        handle_connection(
            stream,
            true,
            &peer_b_keypair,
            &peer_b_trust,
            &inbox_sender_b,
        )
        .await
        .unwrap();
    });

    // Peer A sends a message
    let (_, inbox_sender_a) = Inbox::new();
    let router_a = Router::with_shared_peers(
        peer_a_keypair,
        peer_a_trust,
        CommsConfig::default(),
        inbox_sender_a,
        true,
    );
    router_a
        .send(
            meerkat_comms::router::peer_id_from_pubkey(&peer_b_pubkey),
            MessageKind::Message {
                body: "Hello from A!".to_string(),
                blocks: None,
                handling_mode: None,
            },
        )
        .await
        .unwrap();

    // Wait for B to process
    handle_b.await.unwrap();

    // Note: The important thing is that the message was sent successfully and ack'd.
    // If we got here without error, the exchange worked. The inbox verification
    // is implicit - if B didn't ack, A's send would have failed.
}

#[tokio::test]
#[ignore = "lane:e2e-smoke"]
async fn e2e_smoke_mcp_multimodal_blob_current_turn_request_response_loop() {
    let _lock = INPROC_REGISTRY_LOCK.lock().await;
    meerkat_comms::InprocRegistry::global().clear();

    let suffix = Uuid::new_v4().simple().to_string();
    let name_a = format!("e2e-image-a-{suffix}");
    let name_b = format!("e2e-image-b-{suffix}");

    let mut runtime_a = CommsRuntime::inproc_only(&name_a).expect("runtime A");
    let mut runtime_b = CommsRuntime::inproc_only(&name_b).expect("runtime B");
    let blob_store_a = Arc::new(TestBlobStore::default());
    let blob_store_b = Arc::new(TestBlobStore::default());
    let message_blob = blob_store_a
        .put_image("image/png", "bWVzc2FnZS1ibG9i")
        .await
        .expect("message blob");
    let response_blob = blob_store_b
        .put_image("image/png", "cmVzcG9uc2UtYmxvYg==")
        .await
        .expect("response blob");
    runtime_a.set_blob_store(blob_store_a);
    runtime_b.set_blob_store(blob_store_b);

    let runtime_a = Arc::new(runtime_a);
    let runtime_b = Arc::new(runtime_b);
    let peer_a = Arc::new(TestPeerInteractionHandle::default());
    let peer_b = Arc::new(TestPeerInteractionHandle::default());
    install_test_authority(&runtime_a, peer_a);
    install_test_authority(&runtime_b, peer_b.clone());
    let peer_b_id = runtime_b.public_key().to_peer_id();
    let peer_a_id = runtime_a.public_key().to_peer_id();

    CoreCommsRuntime::add_trusted_peer(
        runtime_a.as_ref(),
        TrustedPeerDescriptor::unsigned_with_pubkey(
            name_b.clone(),
            peer_b_id.to_string(),
            *runtime_b.public_key().as_bytes(),
            format!("inproc://{name_b}"),
        )
        .expect("B trusted descriptor"),
    )
    .await
    .expect("A trusts B");
    CoreCommsRuntime::add_trusted_peer(
        runtime_b.as_ref(),
        TrustedPeerDescriptor::unsigned_with_pubkey(
            name_a.clone(),
            peer_a_id.to_string(),
            *runtime_a.public_key().as_bytes(),
            format!("inproc://{name_a}"),
        )
        .expect("A trusted descriptor"),
    )
    .await
    .expect("B trusts A");

    let ctx_a = tool_context(&runtime_a);
    let ctx_b = tool_context(&runtime_b);

    handle_tools_call_with_context(
        &ctx_a,
        "send_message",
        &json!({
            "peer_id": peer_b_id,
            "display_name": name_b,
            "body": "A->B blob prelude",
            "blocks": [
                {"type": "image_ref", "source": "blob", "blob_id": message_blob.blob_id, "media_type": "image/png"},
                {"type": "text", "text": "Prelude after the image so body synthesis has to decide what to do."}
            ],
            "handling_mode": "queue"
        }),
        &ToolDispatchContext::default(),
    )
    .await
    .expect("send blob-backed message");

    let b_messages = CoreCommsRuntime::drain_inbox_interactions(runtime_b.as_ref()).await;
    assert_eq!(b_messages.len(), 1);
    match &b_messages[0].content {
        meerkat_core::InteractionContent::Message { body, blocks } => {
            assert_eq!(body, "A->B blob prelude");
            let blocks = blocks.as_ref().expect("message blocks");
            assert!(matches!(
                &blocks[0],
                ContentBlock::Text { text } if text == "A->B blob prelude"
            ));
            assert!(matches!(
                &blocks[1],
                ContentBlock::Image {
                    media_type,
                    data: ImageData::Inline { data }
                } if media_type == "image/png" && data == "bWVzc2FnZS1ibG9i"
            ));
        }
        other => panic!("expected B message, got {other:?}"),
    }

    let request_context =
        ToolDispatchContext::from_current_turn_input(&ContentInput::Blocks(vec![
            ContentBlock::Text {
                text: "not an image".to_string(),
            },
            ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: ImageData::Inline {
                    data: "Y3VycmVudC10dXJuLWltYWdl".to_string(),
                },
            },
        ]));

    handle_tools_call_with_context(
        &ctx_a,
        "send_request",
        &json!({
            "peer_id": peer_b_id,
            "display_name": name_b,
            "intent": "checksum_token",
            "params": {"subject": "describe-current-turn-and-answer-with-image"},
            "blocks": [
                {"type": "text", "text": "Request body text block"},
                {"type": "image_ref", "source": "current_turn", "index": 0}
            ],
            "handling_mode": "steer"
        }),
        &request_context,
    )
    .await
    .expect("send current-turn image request");

    let b_requests = CoreCommsRuntime::drain_inbox_interactions(runtime_b.as_ref()).await;
    assert_eq!(b_requests.len(), 1);
    let request_id = b_requests[0].id;
    match &b_requests[0].content {
        meerkat_core::InteractionContent::Request {
            intent,
            params,
            blocks,
        } => {
            assert_eq!(intent, "checksum_token");
            assert_eq!(
                params["subject"],
                "describe-current-turn-and-answer-with-image"
            );
            let blocks = blocks.as_ref().expect("request blocks");
            assert!(matches!(
                &blocks[1],
                ContentBlock::Image {
                    data: ImageData::Inline { data },
                    ..
                } if data == "Y3VycmVudC10dXJuLWltYWdl"
            ));
        }
        other => panic!("expected B request, got {other:?}"),
    }
    peer_b
        .request_received(PeerCorrelationId::from_uuid(request_id.0))
        .expect("seed B inbound request authority");

    handle_tools_call_with_context(
        &ctx_b,
        "send_response",
        &json!({
            "peer_id": peer_a_id,
            "display_name": name_a,
            "in_reply_to": request_id.0.to_string(),
            "status": "completed",
            "result": {
                "request_intent": "checksum_token",
                "request_subject": "describe-current-turn-and-answer-with-image",
                "token": "blob-current-turn-response-ok"
            },
            "blocks": [
                {"type": "text", "text": "B response text block"},
                {"type": "image_ref", "source": "blob", "blob_id": response_blob.blob_id, "media_type": "image/png"}
            ],
            "handling_mode": "steer"
        }),
        &ToolDispatchContext::default(),
    )
    .await
    .expect("send blob-backed response");

    let a_responses = CoreCommsRuntime::drain_inbox_interactions(runtime_a.as_ref()).await;
    assert_eq!(a_responses.len(), 1);
    match &a_responses[0].content {
        meerkat_core::InteractionContent::Response {
            in_reply_to,
            status,
            result,
            blocks,
        } => {
            assert_eq!(*in_reply_to, request_id);
            assert_eq!(*status, ResponseStatus::Completed);
            assert_eq!(result["token"], "blob-current-turn-response-ok");
            let blocks = blocks.as_ref().expect("response blocks");
            assert!(matches!(
                &blocks[1],
                ContentBlock::Image {
                    media_type,
                    data: ImageData::Inline { data }
                } if media_type == "image/png" && data == "cmVzcG9uc2UtYmxvYg=="
            ));
        }
        other => panic!("expected A response, got {other:?}"),
    }
}

#[tokio::test]
#[ignore = "integration-real: real socket I/O"]
async fn integration_real_tcp_message_exchange() {
    // Find an available port
    let Some(listener) = bind_tcp_or_skip("127.0.0.1:0").await else {
        return;
    };
    let addr_b = listener.local_addr().unwrap();

    let peer_a_keypair = make_keypair();
    let peer_b_keypair = make_keypair();
    let peer_b_pubkey = peer_b_keypair.public_key();

    let (peer_a_trust, peer_b_trust) = setup_mutual_trust(
        "peer-a",
        &peer_a_keypair.public_key(),
        "tcp://127.0.0.1:0",
        "peer-b",
        &peer_b_keypair.public_key(),
        &format!("tcp://{addr_b}"),
    );

    // Keep inbox_b alive through the test (must not be dropped before handle_connection completes)
    let (mut inbox_b, inbox_sender_b) = Inbox::new();

    // Spawn B's IO task
    let handle_b = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        handle_connection(
            stream,
            true,
            &peer_b_keypair,
            &peer_b_trust,
            &inbox_sender_b,
        )
        .await
        .unwrap();
    });

    // Peer A sends a message
    let (_, inbox_sender_a) = Inbox::new();
    let router_a = Router::with_shared_peers(
        peer_a_keypair,
        peer_a_trust,
        CommsConfig::default(),
        inbox_sender_a,
        true,
    );
    router_a
        .send(
            meerkat_comms::router::peer_id_from_pubkey(&peer_b_pubkey),
            MessageKind::Message {
                body: "Hello via TCP!".to_string(),
                blocks: None,
                handling_mode: None,
            },
        )
        .await
        .unwrap();

    // Wait for B to process
    handle_b.await.unwrap();

    // Verify B received the message
    let items = inbox_b.try_drain();
    assert_eq!(items.len(), 1, "B should have received one message");
}

#[tokio::test]
#[ignore = "integration-real: real socket I/O"]
async fn integration_real_request_response_flow() {
    let tmp = TempDir::new().unwrap();
    let sock_b = tmp.path().join("peer_b.sock");
    let sock_a = tmp.path().join("peer_a.sock");
    let addr_b = format!("uds://{}", sock_b.display());
    let addr_a = format!("uds://{}", sock_a.display());

    let peer_a_keypair = make_keypair();
    let peer_b_keypair = make_keypair();

    let peer_a_pubkey = peer_a_keypair.public_key();
    let peer_b_pubkey = peer_b_keypair.public_key();

    let (peer_a_trust, peer_b_trust) = setup_mutual_trust(
        "peer-a",
        &peer_a_pubkey,
        &addr_a,
        "peer-b",
        &peer_b_pubkey,
        &addr_b,
    );

    // Start peer B listening for the Request
    let Some(listener_b) = bind_uds_or_skip(&sock_b) else {
        return;
    };
    // Keep inbox_b alive through the test
    let (mut inbox_b, inbox_sender_b) = Inbox::new();

    // Spawn B's receiver
    let peer_b_trust_clone = peer_b_trust.clone();
    let handle_b = tokio::spawn(async move {
        let (stream, _) = listener_b.accept().await.unwrap();
        handle_connection(
            stream,
            true,
            &peer_b_keypair,
            &peer_b_trust_clone,
            &inbox_sender_b,
        )
        .await
        .unwrap();
    });

    // A sends a Request to B
    let (_, inbox_sender_a) = Inbox::new();
    let router_a = Router::with_shared_peers(
        peer_a_keypair,
        peer_a_trust.clone(),
        CommsConfig::default(),
        inbox_sender_a,
        true,
    );
    router_a
        .send(
            meerkat_comms::router::peer_id_from_pubkey(&peer_b_pubkey),
            MessageKind::Request {
                intent: "review-pr".to_string(),
                params: json!({"pr": 42}),
                blocks: None,
                handling_mode: Some(meerkat_core::types::HandlingMode::Queue),
            },
        )
        .await
        .unwrap();

    // B receives the request
    handle_b.await.unwrap();

    // Verify B received the Request
    let items = inbox_b.try_drain();
    assert_eq!(items.len(), 1, "B should have received the request");

    // The request was received (ack was sent) - that's the core flow verified
    // In a real scenario, B would now process and send a Response back
}

#[tokio::test]
#[ignore = "integration-real: real socket I/O"]
async fn integration_real_untrusted_rejected() {
    let tmp = TempDir::new().unwrap();
    let sock_b = tmp.path().join("peer_b.sock");
    let addr_b = format!("uds://{}", sock_b.display());

    let peer_a_keypair = make_keypair();
    let peer_b_keypair = make_keypair();
    let peer_b_pubkey = peer_b_keypair.public_key();
    let _untrusted_keypair = make_keypair();

    // A trusts B, but B does NOT trust A (empty trust list)
    let peer_a_trust = Arc::new(RwLock::new(TrustedPeers {
        peers: vec![TrustedPeer {
            name: "peer-b".to_string(),
            pubkey: peer_b_keypair.public_key(),
            addr: addr_b.clone(),
            meta: meerkat_comms::PeerMeta::default(),
        }],
    }));

    let peer_b_trust = Arc::new(RwLock::new(TrustedPeers { peers: vec![] })); // B trusts no one

    // Start peer B listening
    let Some(listener_b) = bind_uds_or_skip(&sock_b) else {
        return;
    };
    let (_, inbox_sender_b) = Inbox::new();

    let handle_b = tokio::spawn(async move {
        let (stream, _) = listener_b.accept().await.unwrap();
        // This will reject the message due to untrusted sender
        // The connection handler should complete (possibly with error due to untrusted sender)
        // What matters is the message is NOT delivered to inbox
        handle_connection(
            stream,
            true,
            &peer_b_keypair,
            &peer_b_trust,
            &inbox_sender_b,
        )
        .await
    });

    // A sends a message (will connect, but B will reject it as untrusted)
    let (_, inbox_sender_a) = Inbox::new();
    let router_a = Router::with_shared_peers(
        peer_a_keypair,
        peer_a_trust,
        CommsConfig::default(),
        inbox_sender_a,
        true,
    );
    let send_result = router_a
        .send(
            meerkat_comms::router::peer_id_from_pubkey(&peer_b_pubkey),
            MessageKind::Message {
                body: "Hello!".to_string(),
                blocks: None,
                handling_mode: None,
            },
        )
        .await;

    // The send should fail (no ack from B due to untrusted rejection)
    // Either timeout or immediate rejection
    assert!(send_result.is_err());

    // Wait for B's handler to complete
    let _ = handle_b.await;
}

#[tokio::test]
#[ignore = "integration-real: real socket I/O"]
async fn integration_real_concurrent_multi_peer() {
    let tmp = TempDir::new().unwrap();

    // Three peers: A, B, C
    let peer_a_keypair = make_keypair();
    let peer_b_keypair = make_keypair();
    let peer_b_pubkey = peer_b_keypair.public_key();
    let peer_c_keypair = make_keypair();
    let peer_c_pubkey = peer_c_keypair.public_key();

    let sock_a = tmp.path().join("peer_a.sock");
    let sock_b = tmp.path().join("peer_b.sock");
    let sock_c = tmp.path().join("peer_c.sock");

    let addr_a = format!("uds://{}", sock_a.display());
    let addr_b = format!("uds://{}", sock_b.display());
    let addr_c = format!("uds://{}", sock_c.display());

    // A trusts B and C
    let peer_a_trust = Arc::new(RwLock::new(TrustedPeers {
        peers: vec![
            TrustedPeer {
                name: "peer-b".to_string(),
                pubkey: peer_b_keypair.public_key(),
                addr: addr_b.clone(),
                meta: meerkat_comms::PeerMeta::default(),
            },
            TrustedPeer {
                name: "peer-c".to_string(),
                pubkey: peer_c_keypair.public_key(),
                addr: addr_c.clone(),
                meta: meerkat_comms::PeerMeta::default(),
            },
        ],
    }));

    // B trusts A
    let peer_b_trust = Arc::new(RwLock::new(TrustedPeers {
        peers: vec![TrustedPeer {
            name: "peer-a".to_string(),
            pubkey: peer_a_keypair.public_key(),
            addr: addr_a.clone(),
            meta: meerkat_comms::PeerMeta::default(),
        }],
    }));

    // C trusts A
    let peer_c_trust = Arc::new(RwLock::new(TrustedPeers {
        peers: vec![TrustedPeer {
            name: "peer-a".to_string(),
            pubkey: peer_a_keypair.public_key(),
            addr: addr_a.clone(),
            meta: meerkat_comms::PeerMeta::default(),
        }],
    }));

    // Start listeners for B and C
    let Some(listener_b) = bind_uds_or_skip(&sock_b) else {
        return;
    };
    let Some(listener_c) = bind_uds_or_skip(&sock_c) else {
        return;
    };

    // Keep inboxes alive through the test
    let (mut inbox_b, inbox_sender_b) = Inbox::new();
    let (mut inbox_c, inbox_sender_c) = Inbox::new();

    // Spawn B's and C's handlers
    let handle_b = tokio::spawn(async move {
        let (stream, _) = listener_b.accept().await.unwrap();
        handle_connection(
            stream,
            true,
            &peer_b_keypair,
            &peer_b_trust,
            &inbox_sender_b,
        )
        .await
        .unwrap();
    });

    let handle_c = tokio::spawn(async move {
        let (stream, _) = listener_c.accept().await.unwrap();
        handle_connection(
            stream,
            true,
            &peer_c_keypair,
            &peer_c_trust,
            &inbox_sender_c,
        )
        .await
        .unwrap();
    });

    // A sends messages to both B and C concurrently
    let (_, inbox_sender_a) = Inbox::new();
    let router_a = Router::with_shared_peers(
        peer_a_keypair,
        peer_a_trust,
        CommsConfig::default(),
        inbox_sender_a,
        true,
    );

    let send_b = router_a.send(
        meerkat_comms::router::peer_id_from_pubkey(&peer_b_pubkey),
        MessageKind::Message {
            body: "Hello B!".to_string(),
            blocks: None,
            handling_mode: None,
        },
    );
    let send_c = router_a.send(
        meerkat_comms::router::peer_id_from_pubkey(&peer_c_pubkey),
        MessageKind::Message {
            body: "Hello C!".to_string(),
            blocks: None,
            handling_mode: None,
        },
    );

    // Both sends should succeed
    let (result_b, result_c) = tokio::join!(send_b, send_c);
    assert!(result_b.is_ok(), "Send to B should succeed");
    assert!(result_c.is_ok(), "Send to C should succeed");

    // Wait for handlers
    handle_b.await.unwrap();
    handle_c.await.unwrap();

    // Verify both B and C received messages
    let items_b = inbox_b.try_drain();
    let items_c = inbox_c.try_drain();
    assert_eq!(items_b.len(), 1, "B should have received one message");
    assert_eq!(items_c.len(), 1, "C should have received one message");
}

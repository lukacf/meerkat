//! End-to-end tests for meerkat-comms.
//!
//! These tests verify the full system working together with realistic scenarios.

#![cfg(feature = "integration-real-tests")]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use async_trait::async_trait;
use meerkat_comms::{
    CommsRuntime, PeerRequestResponseAuthority, RuntimeCommsCommandHandle, ToolContext,
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
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

static INPROC_REGISTRY_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

async fn add_generated_peer_projection_trust(
    runtime: &CommsRuntime,
    peer: TrustedPeerDescriptor,
    context: &'static str,
) {
    let endpoint = meerkat_runtime::meerkat_machine::dsl::PeerEndpoint::from(&peer);
    let state = meerkat_runtime::meerkat_machine::dsl::MeerkatMachineState {
        lifecycle_phase: meerkat_runtime::meerkat_machine::dsl::MeerkatPhase::Attached,
        session_id: Some(meerkat_runtime::meerkat_machine::dsl::SessionId::from(
            "comms-e2e-test-reconcile",
        )),
        ..Default::default()
    };
    let authority = Arc::new(std::sync::Mutex::new(
        meerkat_runtime::meerkat_machine::dsl::MeerkatMachineAuthority::recover_from_state(state)
            .expect("test MeerkatMachine state must be recoverable"),
    ));
    let dsl = Arc::new(meerkat_runtime::handles::HandleDslAuthority::from_shared(
        Arc::clone(&authority),
    ));
    meerkat_runtime::handles::RuntimePeerCommsHandle::install_generated_on(dsl, runtime)
        .unwrap_or_else(|error| panic!("{context}: install generated peer comms owner: {error}"));
    let transition = {
        let mut guard = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        meerkat_runtime::meerkat_machine::dsl::MeerkatMachineMutator::apply(
            &mut *guard,
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::ApplyMobPeerOverlay {
                epoch: 1,
                endpoints: std::collections::BTreeSet::from([endpoint.clone()]),
            },
        )
        .expect("ApplyMobPeerOverlay input")
    };
    let obligation =
        meerkat_runtime::protocol_comms_trust_reconcile::extract_obligations_with_freshness(
            &transition,
            meerkat_runtime::protocol_comms_trust_reconcile::PeerProjectionFreshnessAuthority::from_authority(
                Arc::clone(&authority),
            ),
        )
        .pop()
        .expect("generated reconcile obligation");
    CoreCommsRuntime::apply_trust_mutation(
        runtime,
        meerkat_core::comms::CommsTrustMutation::AddTrustedPeer {
            authority: meerkat_runtime::protocol_comms_trust_reconcile::authority_for_endpoint(
                &obligation,
                &endpoint,
            )
            .expect("generated peer projection add authority"),
            peer,
        },
    )
    .await
    .unwrap_or_else(|error| panic!("{context}: {error}"));
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
    inbound_modes:
        parking_lot::Mutex<HashMap<PeerCorrelationId, meerkat_core::types::HandlingMode>>,
}

impl TestPeerInteractionHandle {
    fn rejection(context: &'static str, corr_id: PeerCorrelationId) -> DslTransitionError {
        DslTransitionError::guard_rejected(context, format!("e2e rejected corr_id {corr_id}"))
    }
}

impl meerkat_core::handles::PeerInteractionHandle for TestPeerInteractionHandle {
    fn request_sent(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError> {
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

    fn response_rejected(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError> {
        if self.outbound.lock().remove(&corr_id).is_none() {
            return Err(Self::rejection(
                "PeerInteractionHandle::response_rejected",
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

    fn request_send_failed(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError> {
        if self.outbound.lock().remove(&corr_id).is_none() {
            return Err(Self::rejection(
                "PeerInteractionHandle::request_send_failed",
                corr_id,
            ));
        }
        Ok(())
    }

    fn request_received(
        &self,
        corr_id: PeerCorrelationId,
        handling_mode: meerkat_core::types::HandlingMode,
    ) -> Result<(), DslTransitionError> {
        self.inbound
            .lock()
            .insert(corr_id, InboundPeerRequestState::Received);
        self.inbound_modes.lock().insert(corr_id, handling_mode);
        Ok(())
    }

    fn classify_response_reply(
        &self,
        status: meerkat_core::ResponseStatus,
    ) -> Result<meerkat_core::TerminalityClass, DslTransitionError> {
        let generated = meerkat_runtime::handles::RuntimePeerInteractionHandle::ephemeral();
        meerkat_core::handles::PeerInteractionHandle::classify_response_reply(&generated, status)
    }

    fn response_replied(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError> {
        if self.inbound.lock().remove(&corr_id).is_none() {
            return Err(Self::rejection(
                "PeerInteractionHandle::response_replied",
                corr_id,
            ));
        }
        self.inbound_modes.lock().remove(&corr_id);
        Ok(())
    }

    fn outbound_state(&self, corr_id: PeerCorrelationId) -> Option<OutboundPeerRequestState> {
        self.outbound.lock().get(&corr_id).copied()
    }

    fn inbound_state(&self, corr_id: PeerCorrelationId) -> Option<InboundPeerRequestState> {
        self.inbound.lock().get(&corr_id).copied()
    }

    fn inbound_handling_mode(
        &self,
        corr_id: PeerCorrelationId,
    ) -> Option<meerkat_core::types::HandlingMode> {
        self.inbound_modes.lock().get(&corr_id).copied()
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

    fn abandoned(
        &self,
        corr_id: PeerCorrelationId,
        _reason: meerkat_core::InteractionStreamAbandonReason,
    ) -> Result<(), DslTransitionError> {
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

    add_generated_peer_projection_trust(
        runtime_a.as_ref(),
        TrustedPeerDescriptor::unsigned_with_pubkey(
            name_b.clone(),
            peer_b_id.to_string(),
            *runtime_b.public_key().as_bytes(),
            format!("inproc://{name_b}"),
        )
        .expect("B trusted descriptor"),
        "A trusts B",
    )
    .await;
    add_generated_peer_projection_trust(
        runtime_b.as_ref(),
        TrustedPeerDescriptor::unsigned_with_pubkey(
            name_a.clone(),
            peer_a_id.to_string(),
            *runtime_a.public_key().as_bytes(),
            format!("inproc://{name_a}"),
        )
        .expect("A trusted descriptor"),
        "B trusts A",
    )
    .await;

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
            // Single content authority: the caller supplied a text block, so the
            // explicit `body` argument is ignored and the carried body is the
            // projection of the text blocks (mcp/tools.rs `project_body_from_blocks`).
            assert_eq!(
                body,
                "Prelude after the image so body synthesis has to decide what to do."
            );
            // Blocks keep caller order: [image, text].
            let blocks = blocks.as_ref().expect("message blocks");
            assert_eq!(blocks.len(), 2);
            assert!(matches!(
                &blocks[0],
                ContentBlock::Image {
                    media_type,
                    data: ImageData::Inline { data }
                } if media_type == "image/png" && data == "bWVzc2FnZS1ibG9i"
            ));
            assert!(matches!(
                &blocks[1],
                ContentBlock::Text { text } if text == "Prelude after the image so body synthesis has to decide what to do."
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
        .request_received(
            PeerCorrelationId::from_uuid(request_id.0),
            meerkat_core::types::HandlingMode::Queue,
        )
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

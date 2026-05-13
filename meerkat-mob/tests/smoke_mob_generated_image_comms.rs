#![cfg(all(feature = "integration-real-tests", not(target_arch = "wasm32")))]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use async_trait::async_trait;
use futures::stream;
use meerkat::{AgentFactory, Config, FactoryAgentBuilder};
use meerkat_client::{
    ImageGenerationExecutor, LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest,
    ProviderGeneratedImage, ProviderImageGenerationOutput, ProviderImageGenerationRequest,
};
use meerkat_core::types::{
    AssistantBlock, ContentBlock, ContentInput, HandlingMode, ImageData, Message,
    SystemNoticeBlock, SystemNoticeDirection, SystemNoticeKind, SystemNoticeMessage, text_content,
};
use meerkat_core::{
    AssistantImageRef, BlobRef, ImageOperationTerminalClass, MediaType, ProviderImageMetadata,
    RevisedPromptDisposition,
};
use meerkat_mob::definition::{RoleWiringRule, WiringRules};
use meerkat_mob::{
    AgentIdentity, MobBuilder, MobDefinition, MobHandle, MobId, MobRuntimeMode, MobSessionService,
    MobStorage, Profile, ProfileBinding, ProfileName, SpawnMemberSpec, ToolConfig,
};
use meerkat_runtime::MeerkatMachine;
use meerkat_session::PersistentSessionService;
use meerkat_store::{JsonlStore, MemoryBlobStore, StoreAdapter};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::TempDir;
use tokio::time::{Duration, Instant, sleep};

fn first_env(vars: &[&str]) -> Option<String> {
    vars.iter().find_map(|name| std::env::var(name).ok())
}

fn gemini_api_key() -> Option<String> {
    first_env(&["RKAT_GEMINI_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY"])
}

fn image_comms_model() -> String {
    first_env(&[
        "RKAT_MOB_IMAGE_COMMS_MODEL",
        "SMOKE_MODEL_ANTHROPIC",
        "SMOKE_MODEL",
    ])
    .unwrap_or_else(|| "claude-sonnet-4-6".to_string())
}

fn is_provider_empty_output_attempt_error(error: &(dyn std::error::Error + 'static)) -> bool {
    error
        .to_string()
        .contains("completed without user-visible text")
}

fn generated_image_comms_profile(
    model: &str,
    peer_description: &str,
    image_generation: bool,
) -> Profile {
    Profile {
        model: model.to_string(),
        skills: vec![],
        tools: ToolConfig {
            builtins: image_generation,
            comms: true,
            image_generation,
            ..Default::default()
        },
        peer_description: peer_description.to_string(),
        external_addressable: true,
        backend: None,
        runtime_mode: MobRuntimeMode::TurnDriven,
        max_inline_peer_notifications: None,
        output_schema: None,
        provider_params: None,
    }
}

fn generated_image_comms_definition(model: &str) -> MobDefinition {
    let mut profiles = BTreeMap::new();
    profiles.insert(
        ProfileName::from("maker"),
        ProfileBinding::Inline(generated_image_comms_profile(
            model,
            "maker - generates the initial image and asks reviewer to confirm receipt",
            true,
        )),
    );
    profiles.insert(
        ProfileName::from("reviewer"),
        ProfileBinding::Inline(generated_image_comms_profile(
            model,
            "reviewer - replies to checksum_token image-check requests with a generated image receipt",
            true,
        )),
    );

    let mut definition = MobDefinition::explicit(MobId::from("generated-image-comms"));
    definition.profiles = profiles;
    definition.wiring = WiringRules {
        auto_wire_orchestrator: false,
        role_wiring: vec![RoleWiringRule {
            a: ProfileName::from("maker"),
            b: ProfileName::from("reviewer"),
        }],
    };
    definition
}

#[derive(Default)]
struct ScriptedGeneratedImageCommsClient {
    call_id: AtomicUsize,
}

struct DeterministicImageExecutor;

#[async_trait]
impl ImageGenerationExecutor for DeterministicImageExecutor {
    async fn execute_image_generation(
        &self,
        request: ProviderImageGenerationRequest,
    ) -> Result<ProviderImageGenerationOutput, LlmError> {
        Ok(ProviderImageGenerationOutput {
            operation_id: request.operation_id,
            terminal: ImageOperationTerminalClass::Generated,
            images: vec![ProviderGeneratedImage {
                media_type: MediaType::new("image/png"),
                base64_data: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/p9sAAAAASUVORK5CYII=".to_string(),
                width: 1,
                height: 1,
            }],
            provider_text: None,
            revised_prompt: RevisedPromptDisposition::NotRequested,
            native_metadata: ProviderImageMetadata::NotEmitted,
            warnings: Vec::new(),
        })
    }
}

#[async_trait]
impl LlmClient for ScriptedGeneratedImageCommsClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let events = scripted_generated_image_comms_events(
            request,
            self.call_id.fetch_add(1, Ordering::SeqCst),
        );
        Box::pin(stream::iter(events.into_iter().map(Ok)))
    }

    fn provider(&self) -> &'static str {
        "scripted-generated-image-comms"
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

fn scripted_generated_image_comms_events(request: &LlmRequest, call_id: usize) -> Vec<LlmEvent> {
    if let Some(results) = current_boundary_tool_results(&request.messages) {
        let has_peer_request = request_text(request).contains("Peer request from peer_id");
        if let Some(peers) = peers_result(results) {
            let Some(image) = latest_generated_image(&request.messages) else {
                return text_events("script error: missing maker image before send_request");
            };
            let peer_id = peer_id_from_peers(&peers, "reviewer")
                .unwrap_or_else(|| first_peer_id(&peers).unwrap_or_default());
            return tool_events(
                call_id,
                "send_request",
                json!({
                    "peer_id": peer_id,
                    "display_name": "reviewer",
                    "intent": "checksum_token",
                    "params": {"subject": "image_receipt_check"},
                    "blocks": [
                        {"type": "text", "text": "Please confirm receipt and answer with a generated image receipt."},
                        {"type": "image_ref", "source": "blob", "blob_id": image.blob_ref.blob_id.as_str(), "media_type": image.media_type.as_str()}
                    ],
                    "handling_mode": "steer"
                }),
            );
        }

        if let Some(image) = generated_image_from_results(results) {
            if has_peer_request {
                let text = request_text(request);
                let Some((peer_id, display_name, request_id)) = peer_request_route(&text) else {
                    return text_events(
                        "script error: missing peer request route before send_response",
                    );
                };
                return tool_events(
                    call_id,
                    "send_response",
                    json!({
                        "peer_id": peer_id,
                        "display_name": display_name,
                        "in_reply_to": request_id,
                        "status": "completed",
                        "result": {
                            "request_intent": "checksum_token",
                            "request_subject": "image_receipt_check",
                            "token": "generated-image-response-ok"
                        },
                        "blocks": [
                            {"type": "image_ref", "source": "blob", "blob_id": image.blob_ref.blob_id.as_str(), "media_type": image.media_type.as_str()}
                        ],
                        "handling_mode": "steer"
                    }),
                );
            }
            return text_events("generated image ready");
        }

        return text_events("peer tool call complete");
    }

    let text = request_text(request);
    if text.contains("Peer terminal response") {
        return text_events("response image received");
    }
    if text.contains("Peer request from peer_id") && text.contains("image_receipt_check") {
        if has_tool_use(&request.messages, "send_response") {
            return text_events("response sent");
        }
        if let Some(image) = latest_generated_image(&request.messages) {
            let Some((peer_id, display_name, request_id)) = peer_request_route(&text) else {
                return text_events(
                    "script error: missing peer request route before send_response",
                );
            };
            return tool_events(
                call_id,
                "send_response",
                json!({
                    "peer_id": peer_id,
                    "display_name": display_name,
                    "in_reply_to": request_id,
                    "status": "completed",
                    "result": {
                        "request_intent": "checksum_token",
                        "request_subject": "image_receipt_check",
                        "token": "generated-image-response-ok"
                    },
                    "blocks": [
                        {"type": "image_ref", "source": "blob", "blob_id": image.blob_ref.blob_id.as_str(), "media_type": image.media_type.as_str()}
                    ],
                    "handling_mode": "steer"
                }),
            );
        }
        return tool_events(call_id, "generate_image", receipt_image_args());
    }
    if text.contains("Turn 2 of the generated-image comms smoke") {
        if has_tool_use(&request.messages, "send_request") {
            return text_events("request sent");
        }
        return tool_events(call_id, "peers", json!({}));
    }
    if text.contains("Turn 1 of the generated-image comms smoke") {
        if latest_generated_image(&request.messages).is_some() {
            return text_events("generated image ready");
        }
        return tool_events(call_id, "generate_image", maker_image_args());
    }
    if text.contains("REVIEWER-GENERATED-IMAGE-COMMS-READY") {
        return text_events("REVIEWER-GENERATED-IMAGE-COMMS-READY");
    }
    if text.contains("MAKER-GENERATED-IMAGE-COMMS-READY") {
        return text_events("MAKER-GENERATED-IMAGE-COMMS-READY");
    }

    text_events("scripted generated-image comms idle")
}

fn tool_events(call_id: usize, name: &str, args: Value) -> Vec<LlmEvent> {
    vec![
        LlmEvent::ToolCallComplete {
            id: format!("scripted-{name}-{call_id}"),
            name: name.to_string(),
            args,
            meta: None,
        },
        LlmEvent::Done {
            outcome: LlmDoneOutcome::Success {
                stop_reason: meerkat_core::StopReason::ToolUse,
            },
        },
    ]
}

fn text_events(text: &str) -> Vec<LlmEvent> {
    vec![
        LlmEvent::TextDelta {
            delta: text.to_string(),
            meta: None,
        },
        LlmEvent::Done {
            outcome: LlmDoneOutcome::Success {
                stop_reason: meerkat_core::StopReason::EndTurn,
            },
        },
    ]
}

fn maker_image_args() -> Value {
    json!({
        "request": {
            "intent": "generate",
            "prompt": "a simple cyan square with a small magenta dot, no text",
            "provider": "gemini",
            "model": "gemini-3.1-flash-image-preview",
            "size": "1024x1024",
            "quality": "low",
            "format": "png",
            "count": 1
        }
    })
}

fn receipt_image_args() -> Value {
    json!({
        "request": {
            "intent": "generate",
            "prompt": "a tiny generated receipt image with one green check shape, no text",
            "provider": "gemini",
            "model": "gemini-3.1-flash-image-preview",
            "size": "1024x1024",
            "quality": "low",
            "format": "png",
            "count": 1
        }
    })
}

fn current_boundary_tool_results(messages: &[Message]) -> Option<&[meerkat_core::ToolResult]> {
    match messages.last()? {
        Message::ToolResults { results, .. } => Some(results),
        _ => None,
    }
}

fn request_text(request: &LlmRequest) -> String {
    request
        .messages
        .iter()
        .map(message_projection_text)
        .collect::<Vec<_>>()
        .join("\n")
}

fn message_projection_text(message: &Message) -> String {
    match message {
        Message::System(system) => system.content.clone(),
        Message::SystemNotice(notice) => notice.model_projection_text(),
        Message::User(user) => user.text_content(),
        Message::Assistant(assistant) => assistant.content.clone(),
        Message::BlockAssistant(assistant) => assistant.to_string(),
        Message::ToolResults { results, .. } => results
            .iter()
            .map(|result| text_content(&result.content))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn peers_result(results: &[meerkat_core::ToolResult]) -> Option<Value> {
    results.iter().find_map(|result| {
        let value: Value = serde_json::from_str(&text_content(&result.content)).ok()?;
        value.get("peers")?;
        Some(value)
    })
}

fn first_peer_id(peers: &Value) -> Option<String> {
    peers
        .get("peers")?
        .as_array()?
        .iter()
        .find_map(|peer| peer.get("peer_id")?.as_str().map(ToOwned::to_owned))
}

fn peer_id_from_peers(peers: &Value, role: &str) -> Option<String> {
    peers
        .get("peers")?
        .as_array()?
        .iter()
        .find(|peer| {
            let name_matches = peer
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| name.contains(role));
            let description_matches = peer
                .pointer("/meta/description")
                .and_then(Value::as_str)
                .is_some_and(|description| description.contains(role));
            name_matches || description_matches
        })?
        .get("peer_id")?
        .as_str()
        .map(ToOwned::to_owned)
}

fn generated_image_from_results(results: &[meerkat_core::ToolResult]) -> Option<AssistantImageRef> {
    results
        .iter()
        .find_map(|result| {
            if result.is_error {
                return None;
            }
            serde_json::from_str::<meerkat_core::ImageGenerationToolResult>(&text_content(
                &result.content,
            ))
            .ok()
        })
        .and_then(|result| result.images.into_iter().next())
}

fn latest_generated_image(messages: &[Message]) -> Option<AssistantImageRef> {
    messages
        .iter()
        .rev()
        .find_map(successful_generate_image_result)
        .and_then(|result| result.images.into_iter().next())
}

fn peer_request_route(text: &str) -> Option<(String, Option<String>, String)> {
    let peer_id = text
        .split_once("Peer request from peer_id ")?
        .1
        .split_whitespace()
        .next()?
        .to_string();
    let request_id = text
        .split_once("Request ID: ")?
        .1
        .lines()
        .next()?
        .trim()
        .to_string();
    let display_name = text
        .split_once("(display_name: ")
        .and_then(|(_, rest)| {
            rest.split_once(')')
                .map(|(name, _)| name.trim().to_string())
        })
        .filter(|name| !name.is_empty());
    Some((peer_id, display_name, request_id))
}

fn has_tool_use(messages: &[Message], tool_name: &str) -> bool {
    messages.iter().any(|message| {
        tool_uses(message)
            .iter()
            .any(|(name, _)| *name == tool_name)
    })
}

async fn setup_generated_image_comms_mob(
    _api_key: String,
    model: &str,
) -> Result<(MobHandle, Arc<dyn MobSessionService>, TempDir), Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;
    let root = temp_dir.path();
    let runtime_store = Arc::new(meerkat_runtime::InMemoryRuntimeStore::default());
    let blob_store: Arc<dyn meerkat_core::BlobStore> = Arc::new(MemoryBlobStore::default());
    let runtime_adapter = Arc::new(MeerkatMachine::persistent(
        runtime_store.clone(),
        blob_store.clone(),
    ));

    let factory = AgentFactory::new(root.join("factory-store"))
        .user_config_root(root.join("user-config"))
        .runtime_root(root.join("runtime-root"))
        .project_root(root.join("project-root"))
        .context_root(root.join("context-root"))
        .builtins(true)
        .comms(true)
        .with_image_generation_machine(runtime_adapter.clone());
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    builder.default_image_generation_executor = Some(Arc::new(DeterministicImageExecutor));
    let store = Arc::new(JsonlStore::new(root.join("sessions-jsonl")));
    builder.default_session_store = Some(Arc::new(StoreAdapter::new(store.clone())));
    builder.default_llm_client = Some(Arc::new(ScriptedGeneratedImageCommsClient::default()));

    let store_dyn: Arc<dyn meerkat::SessionStore> = store;
    let session_service = Arc::new(PersistentSessionService::new(
        builder,
        8,
        store_dyn,
        Some(runtime_store),
        blob_store,
    ));
    let mob_service: Arc<dyn MobSessionService> = session_service.clone();
    let handle = MobBuilder::new(
        generated_image_comms_definition(model),
        MobStorage::in_memory(),
    )
    .with_session_service(mob_service.clone())
    .with_runtime_adapter(runtime_adapter)
    .create()
    .await?;

    Ok((handle, mob_service, temp_dir))
}

async fn spawn_generated_image_comms_members(
    handle: &MobHandle,
) -> Result<(), Box<dyn std::error::Error>> {
    handle
        .spawn_spec(
            SpawnMemberSpec::new("maker", AgentIdentity::from("maker"))
                .with_initial_message(ContentInput::Text(
                    "Stand by for the two-turn generated-image comms smoke. \
                     For this spawn turn, do not call tools and do not contact peers. \
                     Reply exactly MAKER-GENERATED-IMAGE-COMMS-READY."
                        .to_string(),
                ))
                .with_additional_instructions(vec![
                    "When asked to run the generated-image comms smoke, use tools before prose. \
                     If a user says 'Turn 1', your only valid tool action is one generate_image tool call with \
                     a nested request object: {\"request\":{\"intent\":\"generate\",\"provider\":\"gemini\",\
                     \"model\":\"gemini-3.1-flash-image-preview\",\"prompt\":\"...\",\"size\":\"1024x1024\",\
                     \"quality\":\"low\",\"format\":\"png\",\"count\":1}}; after that tool returns, reply exactly \
                     MAKER-GENERATED-IMAGE-TURN1-DONE. \
                     If a user says 'Turn 2', do not generate a new image; send the prior generated blob. \
                     Generate images with request.target={\"target\":\"model\",\"provider\":\"gemini\",\"model\":\"gemini-3.1-flash-image-preview\"}. \
                     When sending a generated image through comms, use the comms tool's blob-backed image_ref block support."
                        .to_string(),
                ]),
        )
        .await?;
    handle
        .spawn_spec(
            SpawnMemberSpec::new("reviewer", AgentIdentity::from("reviewer"))
                .with_initial_message(ContentInput::Text(
                    "Stand by as reviewer for the generated-image comms smoke. \
                     For this spawn turn, do not call tools and do not contact peers. \
                     Reply exactly REVIEWER-GENERATED-IMAGE-COMMS-READY."
                        .to_string(),
                ))
                .with_additional_instructions(vec![
                    "You are reviewer. When maker sends a checksum_token request about image_receipt_check that includes an image, \
                     generate a tiny receipt image with generate_image using request.target={\"target\":\"model\",\"provider\":\"gemini\",\"model\":\"gemini-3.1-flash-image-preview\"}, then call send_response to maker. Complete the checksum_token request \
                     with token generated-image-response-ok and include your generated receipt image using the comms tool's \
                     blob-backed image_ref block support. \
                     Do not answer with prose only. Do not send any peer message until maker sends you a request."
                        .to_string(),
                    "For checksum_token requests about image_receipt_check, the only successful terminal action is send_response with a blob-backed image_ref block, then reply exactly REVIEWER-RESPONSE-SENT."
                        .to_string(),
                ]),
        )
        .await?;
    handle.wait_for_ready(Some(Duration::from_secs(60))).await?;
    Ok(())
}

async fn wait_for_member_histories_to_settle(
    handle: &MobHandle,
    service: &dyn MobSessionService,
    members: &[&str],
    timeout: Duration,
    stable_for: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    let mut last_counts: Option<Vec<usize>> = None;
    let mut stable_since = Instant::now();
    loop {
        let mut counts = Vec::new();
        for member in members {
            counts.push(member_messages(handle, service, member).await.len());
        }
        if Some(&counts) == last_counts.as_ref() {
            if stable_since.elapsed() >= stable_for {
                return Ok(());
            }
        } else {
            last_counts = Some(counts);
            stable_since = Instant::now();
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "member histories did not settle; last_counts={last_counts:?}"
            ));
        }
        sleep(Duration::from_secs(1)).await;
    }
}

fn system_notice_block_has_image(block: &SystemNoticeBlock) -> bool {
    match block {
        SystemNoticeBlock::Comms { content, .. }
        | SystemNoticeBlock::ExternalEvent { content, .. } => meerkat_core::has_images(content),
        _ => false,
    }
}

fn message_has_image(message: &Message) -> bool {
    match message {
        Message::User(user) => meerkat_core::has_images(&user.content),
        Message::SystemNotice(notice) => notice.blocks.iter().any(system_notice_block_has_image),
        _ => false,
    }
}

#[test]
fn message_has_image_detects_typed_comms_notice_images() {
    let message = Message::SystemNotice(SystemNoticeMessage::with_block(
        SystemNoticeKind::Comms,
        Some("Peer message".to_string()),
        SystemNoticeBlock::Comms {
            kind: "message".to_string(),
            direction: SystemNoticeDirection::Incoming,
            peer: None,
            request_id: None,
            intent: None,
            status: None,
            summary: Some("Peer message".to_string()),
            payload: None,
            content: vec![ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: ImageData::Blob {
                    blob_id: "blob-smoke".into(),
                },
            }],
        },
    ));

    assert!(
        message_has_image(&message),
        "generated-image comms smoke must count typed comms notice media as received image"
    );
}

fn tool_uses(message: &Message) -> Vec<(&str, Value)> {
    let Message::BlockAssistant(blocks) = message else {
        return Vec::new();
    };
    blocks
        .blocks
        .iter()
        .filter_map(|block| {
            let AssistantBlock::ToolUse { name, args, .. } = block else {
                return None;
            };
            let args = serde_json::from_str(args.get()).ok()?;
            Some((name.as_str(), args))
        })
        .collect()
}

fn successful_generate_image_result(
    message: &Message,
) -> Option<meerkat_core::ImageGenerationToolResult> {
    let Message::ToolResults { results, .. } = message else {
        return None;
    };
    results.iter().find_map(|result| {
        if result.is_error {
            return None;
        }
        let text = text_content(&result.content);
        serde_json::from_str::<meerkat_core::ImageGenerationToolResult>(&text)
            .ok()
            .filter(|decoded| !decoded.images.is_empty())
    })
}

fn first_generated_image(messages: &[Message]) -> Option<AssistantImageRef> {
    messages
        .iter()
        .find_map(successful_generate_image_result)
        .and_then(|result| result.images.into_iter().next())
}

fn generated_blob_ref_tool_use(message: &Message, tool_name: &str) -> bool {
    tool_uses(message).into_iter().any(|(name, args)| {
        name == tool_name
            && args
                .get("blocks")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .any(|block| {
                    block.get("type").and_then(Value::as_str) == Some("image_ref")
                        && block.get("source").and_then(Value::as_str) == Some("blob")
                        && block.get("blob_id").and_then(Value::as_str).is_some()
                        && block.get("media_type").and_then(Value::as_str).is_some()
                })
    })
}

async fn member_messages(
    handle: &MobHandle,
    service: &dyn MobSessionService,
    member: &str,
) -> Vec<Message> {
    let Some(session_id) = handle
        .resolve_bridge_session_id(&AgentIdentity::from(member))
        .await
    else {
        return Vec::new();
    };
    service
        .read_history(
            &session_id,
            meerkat_core::SessionHistoryQuery {
                offset: 0,
                limit: None,
            },
        )
        .await
        .map(|page| page.messages)
        .unwrap_or_default()
}

fn message_summary(message: &Message) -> String {
    match message {
        Message::System(_) => "system".to_string(),
        Message::SystemNotice(notice) => format!(
            "system_notice:{:?} body={}",
            notice.kind,
            notice
                .body
                .as_deref()
                .unwrap_or_default()
                .chars()
                .take(240)
                .collect::<String>()
        ),
        Message::User(user) => {
            let text = text_content(&user.content);
            format!(
                "user blocks={} images={} text={}",
                user.content.len(),
                meerkat_core::has_images(&user.content),
                text.chars().take(160).collect::<String>()
            )
        }
        Message::Assistant(assistant) => format!(
            "assistant tools={} text={}",
            assistant.tool_calls.len(),
            assistant.content.chars().take(160).collect::<String>()
        ),
        Message::BlockAssistant(blocks) => {
            let uses = blocks
                .blocks
                .iter()
                .filter_map(|block| match block {
                    AssistantBlock::ToolUse { name, .. } => Some(name.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(",");
            format!(
                "block_assistant blocks={} tools=[{}]",
                blocks.blocks.len(),
                uses
            )
        }
        Message::ToolResults { results, .. } => {
            let rendered = results
                .iter()
                .map(|result| {
                    format!(
                        "{} error={} text={}",
                        result.tool_use_id,
                        result.is_error,
                        text_content(&result.content)
                            .chars()
                            .take(240)
                            .collect::<String>()
                    )
                })
                .collect::<Vec<_>>()
                .join(" ; ");
            format!("tool_results count={} {rendered}", results.len())
        }
    }
}

fn history_summary(messages: &[Message]) -> String {
    messages
        .iter()
        .enumerate()
        .map(|(idx, message)| format!("#{idx}: {}", message_summary(message)))
        .collect::<Vec<_>>()
        .join(" | ")
}

async fn wait_for_generated_image_comms_success(
    handle: &MobHandle,
    service: &dyn MobSessionService,
    timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        let maker = member_messages(handle, service, "maker").await;
        let reviewer = member_messages(handle, service, "reviewer").await;
        let maker_generated = first_generated_image(&maker).is_some();
        let maker_sent_blob_request = maker
            .iter()
            .any(|m| generated_blob_ref_tool_use(m, "send_request"));
        let reviewer_received_image = reviewer.iter().any(message_has_image);
        let reviewer_generated = first_generated_image(&reviewer).is_some();
        let reviewer_sent_blob_response = reviewer
            .iter()
            .any(|m| generated_blob_ref_tool_use(m, "send_response"));
        let maker_received_response_image = maker.iter().any(message_has_image);

        if maker_generated
            && maker_sent_blob_request
            && reviewer_received_image
            && reviewer_generated
            && reviewer_sent_blob_response
            && maker_received_response_image
        {
            return Ok(());
        }

        if Instant::now() >= deadline {
            return Err(format!(
                "timed out waiting for generated-image comms loop: maker_generated={maker_generated}, \
                 maker_sent_blob_request={maker_sent_blob_request}, reviewer_received_image={reviewer_received_image}, \
                 reviewer_generated={reviewer_generated}, reviewer_sent_blob_response={reviewer_sent_blob_response}, \
                 maker_received_response_image={maker_received_response_image}; maker_messages={}, reviewer_messages={}",
                maker.len(),
                reviewer.len()
            ));
        }
        sleep(Duration::from_secs(3)).await;
    }
}

async fn wait_for_first_generated_image(
    handle: &MobHandle,
    service: &dyn MobSessionService,
    member: &str,
    timeout: Duration,
) -> Result<AssistantImageRef, String> {
    let deadline = Instant::now() + timeout;
    loop {
        let messages = member_messages(handle, service, member).await;
        if let Some(image) = first_generated_image(&messages) {
            return Ok(image);
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "timed out waiting for {member} to generate an image; messages={}; history={}",
                messages.len(),
                history_summary(&messages)
            ));
        }
        sleep(Duration::from_secs(3)).await;
    }
}

fn image_ref_matches_blob(message: &Message, tool_name: &str, expected: &BlobRef) -> bool {
    tool_uses(message).into_iter().any(|(name, args)| {
        name == tool_name
            && args
                .get("blocks")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .any(|block| {
                    block.get("type").and_then(Value::as_str) == Some("image_ref")
                        && block.get("source").and_then(Value::as_str) == Some("blob")
                        && block.get("blob_id").and_then(Value::as_str)
                            == Some(expected.blob_id.as_str())
                        && block.get("media_type").and_then(Value::as_str)
                            == Some(expected.media_type.as_str())
                })
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "lane:e2e-smoke"]
async fn e2e_smoke_mob_generated_image_comms_blob_request_response() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();

    let Some(api_key) = gemini_api_key() else {
        eprintln!("Skipping generated-image comms mob smoke: missing GEMINI_API_KEY");
        return;
    };
    let model = image_comms_model();
    let mut last_error = None;
    for attempt in 1..=3 {
        match run_generated_image_comms_blob_request_response_once(api_key.clone(), &model).await {
            Ok(()) => return,
            Err(error) => {
                if attempt < 3 && is_provider_empty_output_attempt_error(error.as_ref()) {
                    eprintln!(
                        "[generated-image-comms] retrying isolated smoke attempt after provider empty-output turn on attempt {attempt}: {error}"
                    );
                    last_error = Some(error.to_string());
                    sleep(Duration::from_secs(5 * attempt)).await;
                    continue;
                }
                panic!("generated image comms mob smoke failed: {error}");
            }
        }
    }

    panic!(
        "generated image comms mob smoke exhausted retries after provider empty-output turns: {}",
        last_error.unwrap_or_else(|| "unknown error".to_string())
    );
}

async fn run_generated_image_comms_blob_request_response_once(
    api_key: String,
    model: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let (handle, service, _tmp) = setup_generated_image_comms_mob(api_key, model).await?;
    spawn_generated_image_comms_members(&handle).await?;
    wait_for_member_histories_to_settle(
        &handle,
        service.as_ref(),
        &["maker", "reviewer"],
        Duration::from_secs(120),
        Duration::from_secs(5),
    )
    .await
    .map_err(std::io::Error::other)?;

    let maker = handle.member(&AgentIdentity::from("maker")).await?;
    maker
        .send(
            ContentInput::Text(
                 "Turn 1 of the generated-image comms smoke. \
                 You MUST call the generate_image tool exactly once now. \
                 Pass request.target={\"target\":\"model\",\"provider\":\"gemini\",\"model\":\"gemini-3.1-flash-image-preview\"}, \
                 request.intent=\"generate\", request.prompt=\"a simple cyan square with a small magenta dot, no text\", \
                 request.size=\"1024x1024\", request.quality=\"low\", request.format=\"png\", request.count=1. \
                 After the tool returns, reply exactly \
                 MAKER-GENERATED-IMAGE-TURN1-DONE. Do not call send_request, \
                 send_message, or send_response in this turn. Do not answer with prose before the tool call."
                    .to_string(),
            ),
            HandlingMode::Queue,
        )
        .await?;

    let maker_image = wait_for_first_generated_image(
        &handle,
        service.as_ref(),
        "maker",
        Duration::from_secs(120),
    )
    .await
    .map_err(std::io::Error::other)?;

    maker
        .send(
            ContentInput::Text(format!(
                "Turn 2 of the generated-image comms smoke. Use the image generated in your previous turn. \
                 Its blob_id is {}, media_type is {}. \
                 Step 1: call peers if needed to find reviewer. \
                 Step 2: call send_request to reviewer as a checksum_token request about subject image_receipt_check, \
                 handling_mode steer, and blocks containing one text block plus one blob-backed image_ref exactly for \
                 that previous-turn generated image. \
                 The image_ref source must be blob. Do not use source current_turn. After send_request returns, \
                 reply exactly MAKER-REQUEST-SENT.",
                maker_image.blob_ref.blob_id.as_str(),
                maker_image.media_type.as_str()
            )),
            HandlingMode::Queue,
        )
        .await?;

    wait_for_generated_image_comms_success(&handle, service.as_ref(), Duration::from_secs(360))
        .await
        .map_err(std::io::Error::other)?;

    let maker_messages = member_messages(&handle, service.as_ref(), "maker").await;
    if !maker_messages
        .iter()
        .any(|message| image_ref_matches_blob(message, "send_request", &maker_image.blob_ref))
    {
        return Err(
            "maker should send the image generated in the previous turn as a blob image_ref".into(),
        );
    }
    Ok(())
}

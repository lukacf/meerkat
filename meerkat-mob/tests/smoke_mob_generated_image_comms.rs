#![cfg(all(feature = "integration-real-tests", not(target_arch = "wasm32")))]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use meerkat::{AgentFactory, Config, FactoryAgentBuilder};
use meerkat_core::types::{
    AssistantBlock, ContentBlock, ContentInput, HandlingMode, Message, SystemNoticeBlock,
    text_content,
};
use meerkat_core::{AssistantImageRef, BlobRef};
use meerkat_mob::definition::{RoleWiringRule, WiringRules};
use meerkat_mob::{
    AgentIdentity, MobBuilder, MobDefinition, MobHandle, MobId, MobRuntimeMode, MobSessionService,
    MobStorage, Profile, ProfileBinding, ProfileName, SpawnMemberSpec, ToolConfig,
};
use meerkat_runtime::MeerkatMachine;
use meerkat_session::PersistentSessionService;
use meerkat_store::{JsonlStore, MemoryBlobStore, StoreAdapter};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::time::{Duration, Instant, sleep};

fn first_env(vars: &[&str]) -> Option<String> {
    vars.iter().find_map(|name| std::env::var(name).ok())
}

fn gemini_api_key() -> Option<String> {
    first_env(&["RKAT_GEMINI_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY"])
}

fn openai_api_key() -> Option<String> {
    first_env(&["RKAT_OPENAI_API_KEY", "OPENAI_API_KEY"])
}

fn anthropic_api_key() -> Option<String> {
    first_env(&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"])
}

fn image_comms_model() -> String {
    std::env::var("RKAT_MOB_IMAGE_COMMS_MODEL").unwrap_or_else(|_| "claude-sonnet-4-5".to_string())
}

fn image_relay_maker_model() -> String {
    std::env::var("RKAT_MOB_IMAGE_RELAY_MAKER_MODEL").unwrap_or_else(|_| "gpt-5.5".to_string())
}

fn image_relay_relay_model() -> String {
    std::env::var("RKAT_MOB_IMAGE_RELAY_RELAY_MODEL")
        .unwrap_or_else(|_| "claude-sonnet-4-5".to_string())
}

fn image_relay_reader_model() -> String {
    std::env::var("RKAT_MOB_IMAGE_RELAY_READER_MODEL")
        .unwrap_or_else(|_| "gemini-3.1-pro-preview".to_string())
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
            builtins: false,
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

fn image_relay_profile(model: &str, peer_description: &str, image_generation: bool) -> Profile {
    Profile {
        model: model.to_string(),
        skills: vec![],
        tools: ToolConfig {
            builtins: false,
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

fn image_relay_definition(
    maker_model: &str,
    relay_model: &str,
    reader_model: &str,
) -> MobDefinition {
    let mut profiles = BTreeMap::new();
    profiles.insert(
        ProfileName::from("maker"),
        ProfileBinding::Inline(image_relay_profile(
            maker_model,
            "maker - generates a text-bearing image and sends it to relay",
            true,
        )),
    );
    profiles.insert(
        ProfileName::from("relay"),
        ProfileBinding::Inline(image_relay_profile(
            relay_model,
            "relay - forwards maker images to reader and returns the readout",
            false,
        )),
    );
    profiles.insert(
        ProfileName::from("reader"),
        ProfileBinding::Inline(image_relay_profile(
            reader_model,
            "reader - reads text from relayed images",
            false,
        )),
    );

    let mut definition = MobDefinition::explicit(MobId::from("image-relay"));
    definition.profiles = profiles;
    definition.wiring = WiringRules {
        auto_wire_orchestrator: false,
        role_wiring: vec![
            RoleWiringRule {
                a: ProfileName::from("maker"),
                b: ProfileName::from("relay"),
            },
            RoleWiringRule {
                a: ProfileName::from("relay"),
                b: ProfileName::from("reader"),
            },
        ],
    };
    definition
}

async fn setup_generated_image_comms_mob(
    api_key: String,
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
    builder.default_image_generation_executor =
        Some(Arc::new(meerkat_client::GeminiClient::new(api_key)));
    let store = Arc::new(JsonlStore::new(root.join("sessions-jsonl")));
    builder.default_session_store = Some(Arc::new(StoreAdapter::new(store.clone())));

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

async fn setup_image_relay_mob(
    openai_key: String,
    maker_model: &str,
    relay_model: &str,
    reader_model: &str,
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
    builder.default_image_generation_executor =
        Some(Arc::new(meerkat_client::OpenAiClient::new(openai_key)));
    let store = Arc::new(JsonlStore::new(root.join("sessions-jsonl")));
    builder.default_session_store = Some(Arc::new(StoreAdapter::new(store.clone())));

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
        image_relay_definition(maker_model, relay_model, reader_model),
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
                .with_additional_instructions(vec![
                    "When asked to run the generated-image comms smoke, use tools, not prose. \
                     If a user says 'Turn 1', your only valid action is one generate_image tool call. \
                     If a user says 'Turn 2', do not generate a new image; send the prior generated blob. \
                     Generate images with provider gemini and model gemini-3.1-flash-image-preview. \
                     When sending a generated image through comms, use the comms tool's blob-backed image_ref block support."
                        .to_string(),
                ]),
        )
        .await?;
    handle
        .spawn_spec(
            SpawnMemberSpec::new("reviewer", AgentIdentity::from("reviewer"))
                .with_additional_instructions(vec![
                    "You are reviewer. When maker sends a checksum_token request about image_receipt_check that includes an image, \
                     generate a tiny receipt image with generate_image using provider gemini and model \
                     gemini-3.1-flash-image-preview, then call send_response to maker. Complete the checksum_token request \
                     with token generated-image-response-ok and include your generated receipt image using the comms tool's \
                     blob-backed image_ref block support. \
                     Do not answer with prose only. Do not send any peer message until maker sends you a request."
                        .to_string(),
                    "For checksum_token requests about image_receipt_check, the only successful terminal action is send_response with a blob-backed image_ref block."
                        .to_string(),
                ]),
        )
        .await?;
    handle.wait_for_ready(Some(Duration::from_secs(60))).await?;
    Ok(())
}

async fn spawn_image_relay_members(handle: &MobHandle) -> Result<(), Box<dyn std::error::Error>> {
    handle
        .spawn_spec(
            SpawnMemberSpec::new("relay", AgentIdentity::from("relay"))
                .with_additional_instructions(vec![
                    "When maker sends an image relay task, do not read or describe the image yourself. \
                     Extract the blob_id and media_type from maker's message. Then call send_message to \
                     the reader member's canonical peer_id with handling_mode steer, a body asking reader \
                     to read the visible text, and a blob-backed image_ref block using exactly that blob_id \
                     and media_type. If needed, call peers first to find reader's peer_id. When reader replies, call send_message back to maker with body \
                     READOUT: <reader's text>. Never invent the readout yourself."
                        .to_string(),
                ]),
        )
        .await?;
    handle
        .spawn_spec(
            SpawnMemberSpec::new("reader", AgentIdentity::from("reader"))
                .with_additional_instructions(vec![
                    "When relay sends you an image, inspect the image pixels and read the visible text. \
                     Reply to relay using send_message with handling_mode steer and body exactly \
                     READOUT: <the text you see>. If you cannot see text, reply READOUT: NO_TEXT. \
                     Do not use prior context or relay body text as the readout."
                        .to_string(),
                ]),
        )
        .await?;
    handle
        .spawn_spec(
            SpawnMemberSpec::new("maker", AgentIdentity::from("maker"))
                .with_additional_instructions(vec![
                    "When asked to run the relay smoke, use tools rather than prose. Generate the image first. \
                     After generate_image returns, send the generated image to relay with a blob-backed image_ref. \
                     The send_message body may include the blob_id and media_type, but it must not reveal the \
                     text that the image generation prompt asked the image to contain."
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

fn message_has_image(message: &Message) -> bool {
    match message {
        Message::User(user) => meerkat_core::has_images(&user.content),
        Message::SystemNotice(notice) => notice.blocks.iter().any(|block| match block {
            SystemNoticeBlock::Comms { content, .. }
            | SystemNoticeBlock::ExternalEvent { content, .. } => meerkat_core::has_images(content),
            _ => false,
        }),
        _ => false,
    }
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

async fn member_peer_id(handle: &MobHandle, member: &str) -> Option<String> {
    handle
        .get_member(&AgentIdentity::from(member))
        .await
        .and_then(|entry| entry.peer_id().map(|peer_id| peer_id.to_string()))
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

fn image_ref_matches_blob_to_peer(
    message: &Message,
    tool_name: &str,
    peer_id: &str,
    expected: &BlobRef,
) -> bool {
    tool_uses(message).into_iter().any(|(name, args)| {
        name == tool_name
            && args.get("peer_id").and_then(Value::as_str) == Some(peer_id)
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

fn image_ref_matches_blob_to_any_peer(
    message: &Message,
    tool_name: &str,
    peer_ids: &[String],
    expected: &BlobRef,
) -> bool {
    peer_ids
        .iter()
        .any(|peer_id| image_ref_matches_blob_to_peer(message, tool_name, peer_id, expected))
}

fn tool_use_leaks_readout_token(message: &Message, tool_name: &str) -> bool {
    tool_uses(message).into_iter().any(|(name, args)| {
        if name != tool_name {
            return false;
        }
        let mut text = args
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        for block in args
            .get("blocks")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(block_text) = block.get("text").and_then(Value::as_str) {
                text.push('\n');
                text.push_str(block_text);
            }
        }
        readout_mentions_target(&text)
    })
}

fn tool_use_to_any_peer_leaks_readout_token(
    message: &Message,
    tool_name: &str,
    peer_ids: &[String],
) -> bool {
    tool_uses(message).into_iter().any(|(name, args)| {
        if name != tool_name {
            return false;
        }
        let Some(peer_id) = args.get("peer_id").and_then(Value::as_str) else {
            return false;
        };
        if !peer_ids.iter().any(|expected| expected == peer_id) {
            return false;
        }
        let mut text = args
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        for block in args
            .get("blocks")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(block_text) = block.get("text").and_then(Value::as_str) {
                text.push('\n');
                text.push_str(block_text);
            }
        }
        readout_mentions_target(&text)
    })
}

fn value_mentions_reader(value: &Value) -> bool {
    match value {
        Value::String(text) => {
            text.eq_ignore_ascii_case("reader")
                || text.contains("image-relay/reader/reader")
                || text.to_ascii_lowercase().contains(" reader")
        }
        Value::Array(values) => values.iter().any(value_mentions_reader),
        Value::Object(map) => map.iter().any(|(key, value)| {
            key.to_ascii_lowercase().contains("reader") || value_mentions_reader(value)
        }),
        _ => false,
    }
}

fn collect_reader_peer_ids_from_value(value: &Value, ids: &mut BTreeSet<String>) {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_reader_peer_ids_from_value(value, ids);
            }
        }
        Value::Object(map) => {
            let object_mentions_reader = map.iter().any(|(key, value)| {
                key.to_ascii_lowercase().contains("reader") || value_mentions_reader(value)
            });
            if object_mentions_reader {
                for key in ["id", "peer_id", "peerId", "name", "address", "endpoint"] {
                    if let Some(peer_id) = map.get(key).and_then(Value::as_str) {
                        ids.insert(peer_id.to_string());
                    }
                }
            }
            for (key, value) in map {
                if key.to_ascii_lowercase().contains("reader")
                    && let Some(peer_id) = value.as_str()
                {
                    ids.insert(peer_id.to_string());
                }
                collect_reader_peer_ids_from_value(value, ids);
            }
        }
        _ => {}
    }
}

fn reader_peer_ids_from_history(messages: &[Message]) -> Vec<String> {
    let mut ids = BTreeSet::from([
        "image-relay/reader/reader".to_string(),
        "reader".to_string(),
    ]);
    for message in messages {
        let Message::ToolResults { results, .. } = message else {
            continue;
        };
        for result in results {
            let text = text_content(&result.content);
            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                collect_reader_peer_ids_from_value(&value, &mut ids);
            }
        }
    }
    ids.into_iter().collect()
}

fn comms_content_from_peer<'a>(
    message: &'a Message,
    expected_peer_id: &str,
) -> Option<&'a [ContentBlock]> {
    let Message::SystemNotice(notice) = message else {
        return None;
    };
    notice.blocks.iter().find_map(|block| {
        let meerkat_core::SystemNoticeBlock::Comms { peer, content, .. } = block else {
            return None;
        };
        let matches_peer = peer.as_ref().is_some_and(|peer| {
            peer.id.as_str() == expected_peer_id
                || peer.display_name.as_deref() == Some(expected_peer_id)
        });
        matches_peer.then_some(content.as_slice())
    })
}

fn comms_text_from_peer(message: &Message, expected_peer_id: &str) -> Option<String> {
    comms_content_from_peer(message, expected_peer_id)
        .map(text_content)
        .filter(|text| !text.trim().is_empty())
}

fn member_received_image_from(messages: &[Message], expected_peer_id: &str) -> bool {
    messages.iter().any(|message| {
        comms_content_from_peer(message, expected_peer_id).is_some_and(meerkat_core::has_images)
    })
}

fn content_has_blob_ref(content: &[ContentBlock], expected: &BlobRef) -> bool {
    content.iter().any(|block| {
        block.image_blob_ref().is_some_and(|(media_type, blob_id)| {
            media_type == expected.media_type && blob_id == &expected.blob_id
        })
    })
}

fn member_received_blob_from(
    messages: &[Message],
    expected_peer_id: &str,
    expected: &BlobRef,
) -> bool {
    messages.iter().any(|message| {
        comms_content_from_peer(message, expected_peer_id)
            .is_some_and(|content| content_has_blob_ref(content, expected))
    })
}

fn member_received_readout_from(messages: &[Message], expected_peer_id: &str) -> bool {
    messages.iter().any(|message| {
        comms_text_from_peer(message, expected_peer_id).is_some_and(|text| {
            text.to_ascii_uppercase().contains("READOUT") && readout_mentions_target(&text)
        })
    })
}

fn readout_mentions_target(text: &str) -> bool {
    let normalized = text.to_ascii_lowercase();
    normalized.contains("rkat") && normalized.contains("7319")
}

fn relay_forwarded_blob_to_reader(
    relay_messages: &[Message],
    reader_messages: &[Message],
    expected: &BlobRef,
) -> bool {
    let reader_peer_ids = reader_peer_ids_from_history(relay_messages);
    let relay_targeted_reader = relay_messages.iter().any(|message| {
        image_ref_matches_blob_to_any_peer(message, "send_message", &reader_peer_ids, expected)
    });
    let reader_received_same_blob =
        member_received_blob_from(reader_messages, "image-relay/relay/relay", expected);
    let relay_sent_same_blob = relay_messages
        .iter()
        .any(|message| image_ref_matches_blob(message, "send_message", expected));
    relay_targeted_reader || (relay_sent_same_blob && reader_received_same_blob)
}

async fn wait_for_image_relay_readout_success(
    handle: &MobHandle,
    service: &dyn MobSessionService,
    timeout: Duration,
) -> Result<BlobRef, String> {
    let deadline = Instant::now() + timeout;
    let relay_peer_ids = {
        let mut ids = vec!["image-relay/relay/relay".to_string(), "relay".to_string()];
        if let Some(peer_id) = member_peer_id(handle, "relay").await {
            ids.push(peer_id);
        }
        ids
    };
    loop {
        let maker = member_messages(handle, service, "maker").await;
        let relay = member_messages(handle, service, "relay").await;
        let reader = member_messages(handle, service, "reader").await;

        let maker_image = first_generated_image(&maker);
        let maker_sent_blob_to_relay = maker_image.as_ref().is_some_and(|image| {
            maker.iter().any(|message| {
                image_ref_matches_blob_to_any_peer(
                    message,
                    "send_message",
                    &relay_peer_ids,
                    &image.blob_ref,
                )
            })
        });
        let relay_received_image = member_received_image_from(&relay, "image-relay/maker/maker");
        let relay_sent_blob_to_reader = maker_image
            .as_ref()
            .is_some_and(|image| relay_forwarded_blob_to_reader(&relay, &reader, &image.blob_ref));
        let reader_received_image = maker_image.as_ref().is_some_and(|image| {
            member_received_blob_from(&reader, "image-relay/relay/relay", &image.blob_ref)
        });
        let relay_received_readout =
            member_received_readout_from(&relay, "image-relay/reader/reader");
        let maker_received_readout =
            member_received_readout_from(&maker, "image-relay/relay/relay");

        if let Some(image) = maker_image.as_ref()
            && maker_sent_blob_to_relay
            && relay_received_image
            && relay_sent_blob_to_reader
            && reader_received_image
            && relay_received_readout
            && maker_received_readout
        {
            return Ok(image.blob_ref.clone());
        }

        if Instant::now() >= deadline {
            return Err(format!(
                "timed out waiting for image relay readout: maker_generated={}, \
                 maker_sent_blob_to_relay={maker_sent_blob_to_relay}, relay_received_image={relay_received_image}, \
                 relay_sent_blob_to_reader={relay_sent_blob_to_reader}, reader_received_image={reader_received_image}, \
                 relay_received_readout={relay_received_readout}, maker_received_readout={maker_received_readout}; \
                 maker={}; relay={}; reader={}",
                maker_image.is_some(),
                history_summary(&maker),
                history_summary(&relay),
                history_summary(&reader)
            ));
        }

        sleep(Duration::from_secs(3)).await;
    }
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
    let (handle, service, _tmp) = setup_generated_image_comms_mob(api_key, &model)
        .await
        .expect("generated image comms mob setup");
    spawn_generated_image_comms_members(&handle)
        .await
        .expect("spawn generated image comms members");
    wait_for_member_histories_to_settle(
        &handle,
        service.as_ref(),
        &["maker", "reviewer"],
        Duration::from_secs(120),
        Duration::from_secs(5),
    )
    .await
    .expect("spawn turns should settle before queued two-turn smoke");

    let maker = handle
        .member(&AgentIdentity::from("maker"))
        .await
        .expect("maker member");
    maker
        .send(
            ContentInput::Text(
                "Turn 1 of the generated-image comms smoke. \
                 Call generate_image exactly once now with this JSON argument: \
                 {\"request\":{\"intent\":\"generate\",\"provider\":\"gemini\",\
                 \"model\":\"gemini-3.1-flash-image-preview\",\"prompt\":\"a simple cyan square with a small magenta dot, no text\",\
                 \"size\":\"1024x1024\",\"quality\":\"low\",\"format\":\"png\",\"count\":1}}. \
                 After the tool returns, stop. Do not call send_request, send_message, or send_response in this turn. \
                 Do not answer with prose instead of the tool call."
                    .to_string(),
            ),
            HandlingMode::Queue,
        )
        .await
        .expect("kick off maker image generation turn");

    let maker_image = wait_for_first_generated_image(
        &handle,
        service.as_ref(),
        "maker",
        Duration::from_secs(120),
    )
    .await
    .expect("maker should generate an image in turn 1");

    maker
        .send(
            ContentInput::Text(format!(
                "Turn 2 of the generated-image comms smoke. Use the image generated in your previous turn. \
                 Its blob_id is {}, media_type is {}. \
                 Step 1: call peers if needed to find reviewer. \
                 Step 2: call send_request to reviewer as a checksum_token request about subject image_receipt_check, \
                 handling_mode steer, and blocks containing one text block plus one blob-backed image_ref exactly for \
                 that previous-turn generated image. \
                 The image_ref source must be blob. Do not use source current_turn.",
                maker_image.blob_ref.blob_id.as_str(),
                maker_image.media_type.as_str()
            )),
            HandlingMode::Queue,
        )
        .await
        .expect("kick off maker generated-image comms send turn");

    wait_for_generated_image_comms_success(&handle, service.as_ref(), Duration::from_secs(360))
        .await
        .expect("generated image request/response comms loop should complete");

    let maker_messages = member_messages(&handle, service.as_ref(), "maker").await;
    assert!(
        maker_messages.iter().any(|message| image_ref_matches_blob(
            message,
            "send_request",
            &maker_image.blob_ref
        )),
        "maker should send the image generated in the previous turn as a blob image_ref"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "lane:e2e-smoke"]
async fn e2e_smoke_s86_mob_provider_image_relay_readout() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();

    let Some(openai_key) = openai_api_key() else {
        eprintln!("Skipping image relay smoke: missing OPENAI_API_KEY");
        return;
    };
    if anthropic_api_key().is_none() {
        eprintln!("Skipping image relay smoke: missing ANTHROPIC_API_KEY");
        return;
    }
    if gemini_api_key().is_none() {
        eprintln!("Skipping image relay smoke: missing GEMINI_API_KEY");
        return;
    }

    let maker_model = image_relay_maker_model();
    let relay_model = image_relay_relay_model();
    let reader_model = image_relay_reader_model();
    let (handle, service, _tmp) =
        setup_image_relay_mob(openai_key, &maker_model, &relay_model, &reader_model)
            .await
            .expect("image relay mob setup");
    spawn_image_relay_members(&handle)
        .await
        .expect("spawn image relay members");
    wait_for_member_histories_to_settle(
        &handle,
        service.as_ref(),
        &["maker", "relay", "reader"],
        Duration::from_secs(120),
        Duration::from_secs(5),
    )
    .await
    .expect("spawn turns should settle before relay smoke");

    let relay_peer_id = member_peer_id(&handle, "relay")
        .await
        .expect("relay should expose a canonical comms peer id");
    let maker = handle
        .member(&AgentIdentity::from("maker"))
        .await
        .expect("maker member");
    maker
        .send(
            ContentInput::Text(format!(
                "Run the provider image relay smoke now. Use exactly this tool sequence, in this order:\n\
                 1. Call generate_image exactly once. The request must use provider openai, model gpt-image-2, \
                 prompt \"Create a simple white poster with only the exact large black text RKAT 7319 centered. \
                 No other letters, numbers, watermarks, captions, or symbols.\", size 1024x1024, quality low, \
                 format png, count 1.\n\
                 2. After generate_image returns, call send_message to peer_id \"{relay_peer_id}\" with \
                 handling_mode \"steer\". Attach the generated image as a blob-backed image_ref using the exact \
                 blob_id and media_type returned by generate_image. The body must tell relay to forward that same \
                 blob-backed image to reader, ask reader to read visible text, and then return reader's READOUT to you. \
                 The body and any text block you send to relay MUST NOT contain the target text from the image prompt. \
                 It may contain only the blob_id, media_type, routing instructions, and the word READOUT.\n\
                 Do not answer with prose until the tools are done."
            )),
            HandlingMode::Queue,
        )
        .await
        .expect("kick off image relay smoke");

    let relayed_blob =
        wait_for_image_relay_readout_success(&handle, service.as_ref(), Duration::from_secs(420))
            .await
            .expect("image relay readout should complete");

    let maker_messages = member_messages(&handle, service.as_ref(), "maker").await;
    let relay_messages = member_messages(&handle, service.as_ref(), "relay").await;
    let reader_messages = member_messages(&handle, service.as_ref(), "reader").await;
    let reader_peer_ids = reader_peer_ids_from_history(&relay_messages);
    assert!(
        !maker_messages
            .iter()
            .any(|message| tool_use_leaks_readout_token(message, "send_message")),
        "maker must not reveal the target text in its relay message"
    );
    assert!(
        !relay_messages
            .iter()
            .any(|message| tool_use_to_any_peer_leaks_readout_token(
                message,
                "send_message",
                &reader_peer_ids
            )),
        "relay must not reveal the target text when asking reader to OCR the image"
    );
    assert!(
        relay_forwarded_blob_to_reader(&relay_messages, &reader_messages, &relayed_blob),
        "relay should forward maker's generated image blob to reader"
    );
}

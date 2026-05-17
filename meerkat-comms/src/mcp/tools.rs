//! MCP tool implementations for Meerkat comms.
//!
//! Exposes agent-facing comms tools: `send_message`, `send_request`,
//! `send_response`, and `peers`.
//!
//! The tool inputs deserialize into typed per-tool input structs; peer
//! request/response traffic then projects through the generated comms
//! contract before it becomes a [`CommsCommand`] domain envelope.

use parking_lot::RwLock;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::Arc;

#[cfg(test)]
use crate::{CommsConfig, Keypair};
use crate::{Router, TrustedPeers};
use meerkat_contracts::{
    CommsPeerRequestIntent, CommsPeerRequestParams, CommsPeerResponseResult, CommsPeersResult,
    CommsSendResult,
};
use meerkat_core::BlobId;
use meerkat_core::ToolDispatchContext;
use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
use meerkat_core::comms::{
    CommsCommand, InputStreamMode, PeerAddress, PeerCapabilitySet, PeerDirectoryEntry,
    PeerDirectorySource, PeerId, PeerName, PeerReachability, PeerRoute, PeerSendability, SendError,
    SendReceipt,
};
use meerkat_core::interaction::{InteractionId, ResponseStatus};
use meerkat_core::tool_catalog::ToolUnavailableReason;
use meerkat_core::types::{ContentBlock, HandlingMode, ImageData};

const RUNTIME_COMMAND_AUTHORITY_UNAVAILABLE_CODE: &str = "runtime_command_authority_unavailable";

const COMMS_BLOCKS_DESCRIPTION: &str = "\n\nMultimodal blocks:\n- Use blocks to send text and images alongside the body/request/response.\n- {\"type\":\"image_ref\",\"source\":\"current_turn\",\"index\":0} refers only to an image attached to the current admitted user input turn.\n- {\"type\":\"image_ref\",\"source\":\"blob\",\"blob_id\":\"sha256:...\",\"media_type\":\"image/png\"} refers to a generated or otherwise blob-backed image, such as an image returned by generate_image earlier in this assistant turn or a previous turn.\n- Do not use source=current_turn for generated images; generated images must be sent with source=blob, blob_id, and media_type.";

const SEND_REQUEST_CONTRACTS_DESCRIPTION: &str = "\n\nSupported request contracts:\n- checksum_token: Use for a simple correlated check/ack/review. params must be {\"subject\":\"<subject>\"}. The responder should send_response with status completed and result {\"request_intent\":\"checksum_token\",\"request_subject\":\"<same subject>\",\"token\":\"<token or checksum>\"}.\n- supervisor.bridge: Use for supervisor bridge control. params include the bridge command payload, for example command observe_member with supervisor, epoch, and protocol_version fields.";

const SEND_RESPONSE_CONTRACTS_DESCRIPTION: &str = "\n\nResponse result contracts:\n- For checksum_token requests, completed responses must use result {\"request_intent\":\"checksum_token\",\"request_subject\":\"<same subject from request params.subject>\",\"token\":\"<token or checksum>\"}.\n- For supervisor.bridge requests, completed acknowledgements use result {\"result\":\"ack\",\"ok\":true}; failed responses use result {\"result\":\"rejected\",\"cause\":\"unsupported\",\"reason\":\"<reason>\"}.";

fn schema_for<T: JsonSchema>() -> Value {
    let schema = schemars::schema_for!(T);
    let mut value = serde_json::to_value(&schema).unwrap_or(Value::Null);

    if let Value::Object(ref mut obj) = value
        && obj.get("type").and_then(Value::as_str) == Some("object")
    {
        obj.entry("properties".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        obj.entry("required".to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
    }

    value
}

// ---------------------------------------------------------------------------
// Per-tool input schemas
// ---------------------------------------------------------------------------

/// Send a message to a peer.
///
/// Example: `{"peer_id": "<peer-id-from-peers>", "body": "What is the current time?", "handling_mode": "steer"}`
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendMessageInput {
    /// Canonical peer ID from the `peers` tool
    pub peer_id: PeerId,
    /// Optional display name retained only for diagnostics
    #[serde(default)]
    pub display_name: Option<String>,
    /// Message body
    pub body: String,
    /// Optional multimodal blocks. Use image_ref entries such as
    /// {"type":"image_ref","source":"current_turn","index":0} to forward
    /// images from the current admitted user turn, or
    /// {"type":"image_ref","source":"blob","blob_id":"sha256:...","media_type":"image/png"}
    /// to forward a generated/blob-backed image without inlining bytes in the tool call.
    #[serde(default)]
    pub blocks: Option<Vec<CommsToolContentBlock>>,
    /// "steer" for immediate processing (normal), "queue" for next turn boundary
    pub handling_mode: HandlingMode,
}

/// Send a structured request to a peer and expect a correlated response.
///
/// Example: `{"peer_id": "<peer-id-from-peers>", "intent": "supervisor.bridge", "params": {"command": "observe_member", ...}, "handling_mode": "steer"}`
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendRequestInput {
    /// Canonical peer ID from the `peers` tool
    pub peer_id: PeerId,
    /// Optional display name retained only for diagnostics
    #[serde(default)]
    pub display_name: Option<String>,
    /// Request intent
    pub intent: CommsPeerRequestIntent,
    /// "steer" for immediate processing (normal), "queue" for next turn boundary
    pub handling_mode: HandlingMode,
    /// Request parameters. Shape is validated against the selected intent at
    /// the typed dispatch boundary; model-facing schema stays compact so
    /// internal bridge wire variants do not dominate ordinary comms turns.
    #[schemars(
        with = "Value",
        description = "Request parameters for the selected intent. For checksum_token use {\"subject\":\"image_receipt_check\"}. For supervisor.bridge use the bridge command payload described in the tool text."
    )]
    pub params: CommsPeerRequestParams,
    /// Optional multimodal blocks. Use image_ref entries such as
    /// {"type":"image_ref","source":"current_turn","index":0} to forward
    /// images from the current admitted user turn, or
    /// {"type":"image_ref","source":"blob","blob_id":"sha256:...","media_type":"image/png"}
    /// to forward a generated/blob-backed image without inlining bytes in the tool call.
    #[serde(default)]
    pub blocks: Option<Vec<CommsToolContentBlock>>,
}

/// Tool-level content block references for comms sends.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CommsToolContentBlock {
    /// Plain text content.
    Text { text: String },
    /// Reference to an image in the current admitted turn.
    ImageRef {
        /// Source collection for the image reference.
        source: CommsToolImageReferenceSource,
        /// Zero-based image index within the current admitted turn. Required for
        /// source=current_turn; forbidden for source=blob.
        #[serde(default)]
        index: Option<usize>,
        /// Blob ID for a generated or otherwise blob-backed image. Required for
        /// source=blob; forbidden for source=current_turn.
        #[serde(default)]
        blob_id: Option<BlobId>,
        /// MIME type for a blob-backed image. Required for source=blob; forbidden
        /// for source=current_turn.
        #[serde(default)]
        media_type: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommsToolImageReferenceSource {
    CurrentTurn,
    Blob,
}

/// Send a response to a previous peer request.
///
/// Example: `{"peer_id": "<peer-id-from-peers>", "in_reply_to": "<request-id>", "status": "completed", "result": {"result": "ack", "ok": true}}`
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendResponseInput {
    /// Canonical peer ID from the `peers` tool
    pub peer_id: PeerId,
    /// Optional display name retained only for diagnostics
    #[serde(default)]
    pub display_name: Option<String>,
    /// ID of the request being responded to (from the original request)
    pub in_reply_to: String,
    /// Response status: "accepted", "completed", or "failed"
    pub status: ResponseStatus,
    /// Response result data (optional). Shape is validated against the
    /// original request contract at the typed dispatch boundary.
    #[serde(default)]
    #[schemars(
        with = "Option<Value>",
        description = "Typed response payload. For checksum_token use {\"request_intent\":\"checksum_token\",\"request_subject\":\"image_receipt_check\",\"token\":\"generated-image-response-ok\"}. For supervisor.bridge use simple bridge reply objects such as {\"result\":\"ack\",\"ok\":true}."
    )]
    pub result: Option<CommsPeerResponseResult>,
    /// Optional multimodal blocks. Use image_ref entries such as
    /// {"type":"image_ref","source":"current_turn","index":0} to forward
    /// images from the current admitted user turn, or
    /// {"type":"image_ref","source":"blob","blob_id":"sha256:...","media_type":"image/png"}
    /// to forward a generated/blob-backed image without inlining bytes in the tool call.
    #[serde(default)]
    pub blocks: Option<Vec<CommsToolContentBlock>>,
    /// Handling mode override for terminal responses: "steer" or "queue" (optional)
    #[serde(default)]
    pub handling_mode: Option<HandlingMode>,
}

/// Input schema for `peers` tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct PeersInput {}

/// Context for comms tool execution
#[derive(Clone)]
pub struct ToolContext {
    pub router: Arc<Router>,
    pub trusted_peers: Arc<RwLock<TrustedPeers>>,
    pub runtime: Option<RuntimeCommsCommandHandle>,
}

/// Runtime-bound authority for semantic comms command execution.
#[derive(Clone)]
pub struct RuntimeCommsCommandHandle {
    runtime: Arc<dyn CoreCommsRuntime>,
}

impl RuntimeCommsCommandHandle {
    pub fn new(runtime: Arc<dyn CoreCommsRuntime>) -> Self {
        Self { runtime }
    }

    pub async fn send(&self, command: CommsCommand) -> Result<SendReceipt, SendError> {
        self.runtime.send(command).await
    }

    pub async fn peers(&self) -> Vec<PeerDirectoryEntry> {
        self.runtime.peers().await
    }
}

pub fn requires_runtime_command_authority(name: &str) -> bool {
    matches!(name, "send_request" | "send_response")
}

pub fn comms_tool_unavailable_reason(
    ctx: &ToolContext,
    name: &str,
) -> Option<ToolUnavailableReason> {
    (requires_runtime_command_authority(name) && ctx.runtime.is_none())
        .then_some(ToolUnavailableReason::RuntimeCommandAuthorityUnavailable)
}

/// Returns the list of comms tools.
pub fn tools_list() -> Vec<Value> {
    vec![
        json!({
            "name": "send_message",
            "description": format!("{}{}{}", "Send a fire-and-forget message to a peer. No response is expected.\n\nWhen to use: Use send_message for one-way collaboration — status updates, notifications, sharing results, or any case where you do not need the peer to reply with structured data. If you need a correlated reply, use send_request instead.\n\nhandling_mode:\n- \"steer\": The peer processes your message immediately, interrupting its current work. Use for urgent or time-sensitive collaboration.\n- \"queue\": The message is delivered at the peer's next turn boundary. Use for non-urgent follow-ups where you do not want to interrupt the peer's current task.", COMMS_BLOCKS_DESCRIPTION, "\n\nExamples:\n1. Fire-and-forget collaboration:\n   {\"peer_id\": \"<peer-id-from-peers>\", \"body\": \"FYI: the database migration completed successfully.\", \"handling_mode\": \"steer\"}\n2. Send a generated image without expecting a reply:\n   {\"peer_id\": \"<peer-id-from-peers>\", \"body\": \"Here is the generated mockup.\", \"blocks\": [{\"type\":\"image_ref\",\"source\":\"blob\",\"blob_id\":\"sha256:generated-image\",\"media_type\":\"image/png\"}], \"handling_mode\": \"steer\"}\n3. Queued follow-up (non-urgent):\n   {\"peer_id\": \"<peer-id-from-peers>\", \"display_name\": \"reporter\", \"body\": \"When you finish, include the error counts from section 3.\", \"handling_mode\": \"queue\"}\n\nFailure handling:\n- peer_not_found_or_not_trusted: The peer_id does not match a trusted peer. Call peers first to pick a peer_id.\n- peer_unreachable: The peer exists but is offline or the transport failed. Retry after a delay or inform the user."),
            "inputSchema": schema_for::<SendMessageInput>()
        }),
        json!({
            "name": "send_request",
            "description": format!("{}{}{}{}", "Send a typed structured request to a peer and expect a correlated response. The peer will reply using send_response with the same request ID.\n\nWhen to use: Use send_request for typed comms request contracts such as checksum_token or supervisor.bridge. The response will arrive as an incoming message with the original request ID in its in_reply_to field, so you can match it. If you just need to share information without expecting a reply, use send_message instead.\n\nhandling_mode:\n- \"steer\": The peer processes your request immediately, interrupting its current work. Use for requests that block your own progress.\n- \"queue\": The request is delivered at the peer's next turn boundary. Use when the peer can handle it after finishing its current task.", SEND_REQUEST_CONTRACTS_DESCRIPTION, COMMS_BLOCKS_DESCRIPTION, "\n\nExamples:\n1. checksum_token image review request:\n   {\"peer_id\":\"<peer-id-from-peers>\",\"display_name\":\"reviewer\",\"intent\":\"checksum_token\",\"params\":{\"subject\":\"image_receipt_check\"},\"blocks\":[{\"type\":\"text\",\"text\":\"Please inspect this generated image and return a receipt token with an image receipt.\"},{\"type\":\"image_ref\",\"source\":\"blob\",\"blob_id\":\"sha256:generated-image\",\"media_type\":\"image/png\"}],\"handling_mode\":\"steer\"}\n2. supervisor bridge request/reply:\n  {\"peer_id\": \"<peer-id-from-peers>\", \"display_name\": \"member\", \"intent\": \"supervisor.bridge\", \"params\": {\"command\": \"observe_member\", \"supervisor\": {\"name\": \"supervisor\", \"peer_id\": \"<supervisor-peer-id>\", \"address\": \"inproc://supervisor\", \"pubkey\": [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]}, \"epoch\": 1, \"protocol_version\": 2}, \"handling_mode\": \"steer\"}\n  The peer receives this and sends back with your peer_id:\n  {\"peer_id\": \"<your-peer-id>\", \"in_reply_to\": \"<request-id>\", \"status\": \"completed\", \"result\": {\"result\": \"ack\", \"ok\": true}}\n\nFailure handling:\n- peer_not_found_or_not_trusted: The peer_id does not match a trusted peer. Call peers first to pick a peer_id.\n- peer_unreachable: The peer exists but is offline or the transport failed. Retry after a delay or inform the user.\n- Missing response: There is no built-in timeout. If the peer does not respond, it may have failed or dropped the request. Re-send or check with the peer via send_message."),
            "inputSchema": schema_for::<SendRequestInput>()
        }),
        json!({
            "name": "send_response",
            "description": format!("{}{}{}{}", "Send a typed response to a previous peer request. The in_reply_to field must match the request ID from the original send_request message you received.\n\nWhen to use: Use send_response after receiving a typed comms request from a peer. The requester is waiting for a correlated reply.\n\nstatus values:\n- \"accepted\": Acknowledge receipt; you will send a \"completed\" or \"failed\" response later. Do not include handling_mode on accepted progress responses.\n- \"completed\": The request succeeded. Include a typed result when the request contract requires one.\n- \"failed\": The request could not be fulfilled. Include a typed rejection result when the request contract requires one.\n\nhandling_mode (optional): Override how the requester processes this terminal response. Defaults to the original request's mode. Use \"steer\" to interrupt the requester immediately with your result, or \"queue\" to deliver at their next turn boundary.", SEND_RESPONSE_CONTRACTS_DESCRIPTION, COMMS_BLOCKS_DESCRIPTION, "\n\nExamples:\n1. Completed checksum_token response with a generated image receipt:\n   {\"peer_id\":\"<peer-id-from-peers>\",\"display_name\":\"requester\",\"in_reply_to\":\"<request-id>\",\"status\":\"completed\",\"result\":{\"request_intent\":\"checksum_token\",\"request_subject\":\"image_receipt_check\",\"token\":\"generated-image-response-ok\"},\"blocks\":[{\"type\":\"image_ref\",\"source\":\"blob\",\"blob_id\":\"sha256:receipt-image\",\"media_type\":\"image/png\"}],\"handling_mode\":\"steer\"}\n2. Acceptance then later completion:\n   {\"peer_id\": \"<peer-id-from-peers>\", \"in_reply_to\": \"<request-id>\", \"status\": \"accepted\"}\n   ...later...\n   {\"peer_id\": \"<peer-id-from-peers>\", \"in_reply_to\": \"<request-id>\", \"status\": \"completed\", \"result\": {\"result\": \"ack\", \"ok\": true}}\n3. Failure response:\n   {\"peer_id\": \"<peer-id-from-peers>\", \"in_reply_to\": \"<request-id>\", \"status\": \"failed\", \"result\": {\"result\": \"rejected\", \"cause\": \"unsupported\", \"reason\": \"unsupported command\"}}\n\nFailure handling:\n- peer_not_found_or_not_trusted / peer_unreachable: Same as send_message. The requester will not receive your response — they may re-send the request.\n- Invalid in_reply_to: If the ID is not a valid UUID or does not match a known request, the call fails with a validation error."),
            "inputSchema": schema_for::<SendResponseInput>()
        }),
        json!({
            "name": "peers",
            "description": "List all visible peers with connection info and optional metadata (description, labels, capabilities, reachability).\n\nAlways call peers before sending any message to pick the canonical peer_id. Names are display labels and may not be unique. The returned list includes:\n- peer_id: Unique cryptographic identity to use in send_message / send_request / send_response.\n- name: Display label only.\n- address: Transport address.\n- reachability: Whether the peer is currently reachable.\n- capabilities / meta: What the peer can do and its role description.\n\nExample output:\n{\"peers\": [{\"name\": \"helper-1\", \"peer_id\": \"018f...\", \"address\": \"tcp://...\", \"reachability\": \"reachable\", \"meta\": {\"description\": \"Code review helper\"}}]}",
            "inputSchema": schema_for::<PeersInput>()
        }),
    ]
}

/// Handle a comms tool call.
pub async fn handle_tools_call(
    ctx: &ToolContext,
    name: &str,
    args: &Value,
) -> Result<Value, String> {
    handle_tools_call_with_context(ctx, name, args, &ToolDispatchContext::default()).await
}

/// Handle a comms tool call with dispatch-time turn context.
pub async fn handle_tools_call_with_context(
    ctx: &ToolContext,
    name: &str,
    args: &Value,
    dispatch_context: &ToolDispatchContext,
) -> Result<Value, String> {
    if comms_tool_unavailable_reason(ctx, name).is_some() {
        return Err(runtime_command_authority_unavailable_message());
    }

    match name {
        "send_message" => {
            let input: SendMessageInput = serde_json::from_value(args.clone())
                .map_err(|e| format!("Invalid arguments: {e}"))?;
            let to = peer_route(ctx, input.peer_id, input.display_name.as_deref())?;
            let blocks = resolve_message_blocks(&input.body, input.blocks, dispatch_context)?;
            let command = CommsCommand::PeerMessage {
                to,
                body: input.body,
                blocks,
                handling_mode: input.handling_mode,
            };
            dispatch(ctx, command).await
        }
        "send_request" => {
            let input: SendRequestInput = serde_json::from_value(args.clone())
                .map_err(|e| format!("Invalid arguments: {e}"))?;
            let to = peer_route(ctx, input.peer_id, input.display_name.as_deref())?;
            let blocks = resolve_tool_blocks(input.blocks, dispatch_context)?;
            let typed_request = meerkat_contracts::CommsCommandRequest::PeerRequest {
                to: input.peer_id,
                intent: input.intent,
                params: input.params,
                blocks,
                handling_mode: Some(input.handling_mode),
                stream: Some(InputStreamMode::ReserveInteraction),
            };
            let command = project_peer_request_command(typed_request, to)?;
            dispatch(ctx, command).await
        }
        "send_response" => {
            let input: SendResponseInput = serde_json::from_value(args.clone())
                .map_err(|e| format!("Invalid arguments: {e}"))?;
            let to = peer_route(ctx, input.peer_id, input.display_name.as_deref())?;
            let in_reply_to_uuid = uuid::Uuid::parse_str(&input.in_reply_to)
                .map_err(|_| format!("invalid UUID for in_reply_to: {}", input.in_reply_to))?;
            // Accepted progress responses reject a handling_mode override
            // at the tool boundary — the receiver's admission gate would
            // drop them, so refuse to build a command that can't succeed.
            if matches!(input.status, ResponseStatus::Accepted) && input.handling_mode.is_some() {
                return Err("handling_mode is forbidden on accepted peer responses".to_string());
            }
            let blocks = resolve_tool_blocks(input.blocks, dispatch_context)?;
            let typed_request = meerkat_contracts::CommsCommandRequest::PeerResponse {
                to: input.peer_id,
                in_reply_to: InteractionId(in_reply_to_uuid),
                status: input.status,
                result: input.result,
                blocks,
                handling_mode: input.handling_mode,
            };
            let command = project_peer_response_command(typed_request, to)?;
            dispatch(ctx, command).await
        }
        "peers" => {
            let _input: PeersInput = serde_json::from_value(args.clone())
                .map_err(|e| format!("Invalid arguments: {e}"))?;
            handle_peers(ctx).await
        }
        _ => Err(format!("Unknown tool: {name}")),
    }
}

fn resolve_tool_blocks(
    blocks: Option<Vec<CommsToolContentBlock>>,
    dispatch_context: &ToolDispatchContext,
) -> Result<Option<Vec<ContentBlock>>, String> {
    blocks
        .map(|blocks| {
            blocks
                .into_iter()
                .map(|block| resolve_tool_block(block, dispatch_context))
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()
}

fn resolve_message_blocks(
    body: &str,
    blocks: Option<Vec<CommsToolContentBlock>>,
    dispatch_context: &ToolDispatchContext,
) -> Result<Option<Vec<ContentBlock>>, String> {
    let Some(mut blocks) = resolve_tool_blocks(blocks, dispatch_context)? else {
        return Ok(None);
    };
    let body = body.trim();
    if !body.is_empty() && !blocks_text_contains(&blocks, body) {
        blocks.insert(
            0,
            ContentBlock::Text {
                text: body.to_string(),
            },
        );
    }
    Ok(Some(blocks))
}

fn blocks_text_contains(blocks: &[ContentBlock], expected: &str) -> bool {
    if blocks
        .iter()
        .any(|block| matches!(block, ContentBlock::Text { text } if text.trim() == expected))
    {
        return true;
    }
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.trim()),
            _ => None,
        })
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
        == expected
}

fn resolve_tool_block(
    block: CommsToolContentBlock,
    dispatch_context: &ToolDispatchContext,
) -> Result<ContentBlock, String> {
    match block {
        CommsToolContentBlock::Text { text } => Ok(ContentBlock::Text { text }),
        CommsToolContentBlock::ImageRef {
            source,
            index,
            blob_id,
            media_type,
        } => resolve_image_ref(source, index, blob_id, media_type, dispatch_context),
    }
}

fn resolve_image_ref(
    source: CommsToolImageReferenceSource,
    index: Option<usize>,
    blob_id: Option<BlobId>,
    media_type: Option<String>,
    dispatch_context: &ToolDispatchContext,
) -> Result<ContentBlock, String> {
    match source {
        CommsToolImageReferenceSource::CurrentTurn => {
            if blob_id.is_some() || media_type.is_some() {
                return Err("invalid image_ref: source current_turn only accepts index".to_string());
            }
            let index = index.ok_or_else(|| {
                "invalid image_ref: source current_turn requires index".to_string()
            })?;
            dispatch_context
                .current_turn_image(index)
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "image_ref_unavailable: current_turn image {index} did not resolve to a current-turn image"
                    )
                })
        }
        CommsToolImageReferenceSource::Blob => {
            if index.is_some() {
                return Err("invalid image_ref: source blob does not accept index".to_string());
            }
            let blob_id = blob_id
                .ok_or_else(|| "invalid image_ref: source blob requires blob_id".to_string())?;
            let media_type = media_type
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| "invalid image_ref: source blob requires media_type".to_string())?;
            Ok(ContentBlock::Image {
                media_type,
                data: ImageData::Blob { blob_id },
            })
        }
    }
}

fn project_peer_request_command(
    request: meerkat_contracts::CommsCommandRequest,
    to: PeerRoute,
) -> Result<CommsCommand, String> {
    // Compatibility JSON is produced only after the generated comms contract
    // has accepted the typed intent and params.
    let core_request = request
        .into_core_request()
        .map_err(|err| format!("Invalid arguments: {err}"))?;
    let meerkat_core::comms::CommsCommandRequest::PeerRequest {
        intent,
        params,
        blocks,
        handling_mode,
        stream,
        ..
    } = core_request
    else {
        return Err("Invalid arguments: expected peer_request comms command".to_string());
    };
    Ok(CommsCommand::PeerRequest {
        to,
        intent,
        params,
        blocks,
        handling_mode: handling_mode.unwrap_or_default(),
        stream: stream.unwrap_or(InputStreamMode::None),
    })
}

fn project_peer_response_command(
    request: meerkat_contracts::CommsCommandRequest,
    to: PeerRoute,
) -> Result<CommsCommand, String> {
    // Compatibility JSON is produced only after the generated comms contract
    // has accepted the typed result payload.
    let core_request = request
        .into_core_request()
        .map_err(|err| format!("Invalid arguments: {err}"))?;
    let meerkat_core::comms::CommsCommandRequest::PeerResponse {
        in_reply_to,
        status,
        result,
        blocks,
        handling_mode,
        ..
    } = core_request
    else {
        return Err("Invalid arguments: expected peer_response comms command".to_string());
    };
    Ok(CommsCommand::PeerResponse {
        to,
        in_reply_to,
        status,
        result,
        blocks,
        handling_mode,
    })
}

fn peer_name(value: &str, field: &str) -> Result<PeerName, String> {
    PeerName::new(value).map_err(|err| format!("invalid {field}: {err}"))
}

fn peer_route(
    ctx: &ToolContext,
    peer_id: PeerId,
    display_name: Option<&str>,
) -> Result<PeerRoute, String> {
    ensure_peer_id_is_trusted(ctx, &peer_id)?;
    match display_name {
        Some(name) => Ok(PeerRoute::with_display_name(
            peer_id,
            peer_name(name, "display_name")?,
        )),
        None => Ok(PeerRoute::new(peer_id)),
    }
}

async fn dispatch(ctx: &ToolContext, command: CommsCommand) -> Result<Value, String> {
    // Capture peer display label for error normalization before consuming the command.
    let peer_for_errors = match &command {
        CommsCommand::PeerMessage { to, .. }
        | CommsCommand::PeerLifecycle { to, .. }
        | CommsCommand::PeerRequest { to, .. }
        | CommsCommand::PeerResponse { to, .. } => Some(to.label()),
        CommsCommand::Input { .. } => None,
    };
    let cmd_kind = command.command_kind().to_string();

    if let Some(runtime) = &ctx.runtime {
        let receipt = runtime.send(command).await.map_err(|error| match error {
            SendError::PeerNotFound(p) => {
                format!("peer_not_found_or_not_trusted: peer '{p}' is not found or not trusted")
            }
            SendError::PeerOffline => format!(
                "peer_unreachable: peer '{}' is unreachable: offline_or_no_ack",
                peer_for_errors.as_deref().unwrap_or("<unknown>")
            ),
            SendError::Internal(inner) if is_transport_internal(&inner) => {
                format!(
                    "peer_unreachable: peer '{}' is unreachable: transport_error ({inner})",
                    peer_for_errors.as_deref().unwrap_or("<unknown>")
                )
            }
            other => other.to_string(),
        })?;
        return Ok(sent_result(&cmd_kind, receipt));
    }

    if matches!(
        command,
        CommsCommand::PeerRequest { .. } | CommsCommand::PeerResponse { .. }
    ) {
        return Err(runtime_command_authority_unavailable_message());
    }

    // Fallback path (no runtime): dispatch directly through the router.
    // This is transport-only and intentionally excludes semantic
    // request/response traffic, which must be owned by a runtime command
    // authority.
    let dest_display = peer_for_errors
        .as_deref()
        .unwrap_or("<unknown>")
        .to_string();
    let dest_peer_id = match &command {
        CommsCommand::PeerMessage { to, .. }
        | CommsCommand::PeerLifecycle { to, .. }
        | CommsCommand::PeerRequest { to, .. }
        | CommsCommand::PeerResponse { to, .. } => to.peer_id,
        CommsCommand::Input { .. } => {
            return Err("input command is not supported by MCP send".to_string());
        }
    };

    match command {
        CommsCommand::Input { .. } => Err("input command is not supported by MCP send".to_string()),
        CommsCommand::PeerMessage {
            body,
            blocks,
            handling_mode,
            ..
        } => {
            ctx.router
                .send(
                    dest_peer_id,
                    crate::types::MessageKind::Message {
                        body,
                        blocks,
                        handling_mode: Some(handling_mode),
                    },
                )
                .await
                .map_err(|e| format_router_send_error(&dest_display, e))?;
            Ok(json!({ "status": "sent", "kind": cmd_kind }))
        }
        CommsCommand::PeerLifecycle { kind, params, .. } => {
            ctx.router
                .send(
                    dest_peer_id,
                    crate::types::MessageKind::Lifecycle { kind, params },
                )
                .await
                .map_err(|e| format_router_send_error(&dest_display, e))?;
            Ok(json!({ "status": "sent", "kind": cmd_kind }))
        }
        CommsCommand::PeerRequest { .. } => Err(runtime_command_authority_unavailable_message()),
        CommsCommand::PeerResponse { .. } => Err(runtime_command_authority_unavailable_message()),
    }
}

fn runtime_command_authority_unavailable_message() -> String {
    format!("tool_unavailable: {RUNTIME_COMMAND_AUTHORITY_UNAVAILABLE_CODE}")
}

fn sent_result(kind: &str, receipt: SendReceipt) -> Value {
    json!({
        "status": "sent",
        "kind": kind,
        "receipt": CommsSendResult::from(receipt),
    })
}

/// Validate a canonical [`PeerId`] against the runtime-facing trust set.
fn ensure_peer_id_is_trusted(ctx: &ToolContext, peer_id: &PeerId) -> Result<(), String> {
    let peers = ctx.trusted_peers.read();
    match peers.find_by_peer_id(peer_id) {
        Some(_) => Ok(()),
        None => Err(format!(
            "peer_not_found_or_not_trusted: peer '{peer_id}' is not found or not trusted",
        )),
    }
}

fn format_router_send_error(peer_name: &str, error: crate::router::SendError) -> String {
    match error {
        crate::router::SendError::PeerNotFound(_) => {
            format!("peer_not_found_or_not_trusted: peer '{peer_name}' is not found or not trusted")
        }
        crate::router::SendError::PeerOffline => {
            format!("peer_unreachable: peer '{peer_name}' is unreachable: offline_or_no_ack")
        }
        crate::router::SendError::AdmissionDropped { reason } => {
            // Distinct from `peer_unreachable`: the peer's transport was
            // live, ingress policy refused us. Surface the typed reason
            // (untrusted_sender / inbox_full / …) so clients can tell
            // policy failures from connectivity failures.
            let code: meerkat_core::comms::AdmissionDropReason = reason.into();
            format!(
                "peer_admission_dropped: peer '{peer_name}' rejected envelope at ingress: {}",
                code.as_code()
            )
        }
        crate::router::SendError::Transport(inner) => {
            format!(
                "peer_unreachable: peer '{peer_name}' is unreachable: transport_error ({inner})"
            )
        }
        crate::router::SendError::Io(inner) => {
            format!(
                "peer_unreachable: peer '{peer_name}' is unreachable: transport_error ({inner})"
            )
        }
    }
}

fn is_transport_internal(message: &str) -> bool {
    message.starts_with("Transport error:") || message.starts_with("IO error:")
}

async fn handle_peers(ctx: &ToolContext) -> Result<Value, String> {
    let entries = if let Some(runtime) = &ctx.runtime {
        runtime.peers().await
    } else {
        runtime_less_peer_directory(ctx)
    };

    serde_json::to_value(CommsPeersResult::from_entries(&entries))
        .map_err(|err| format!("failed to serialize peer directory: {err}"))
}

fn runtime_less_peer_directory(ctx: &ToolContext) -> Vec<PeerDirectoryEntry> {
    let self_pubkey = ctx.router.keypair_arc().public_key();
    let peers = ctx.trusted_peers.read();
    let sendable_kinds = peer_sendability_authorized_by_tools(ctx);
    let peer_id_counts: HashMap<PeerId, usize> = peers
        .peers
        .iter()
        .filter(|p| !p.pubkey.is_zero() && p.pubkey != self_pubkey)
        .fold(HashMap::new(), |mut counts, peer| {
            *counts.entry(peer.pubkey.to_peer_id()).or_default() += 1;
            counts
        });
    peers
        .peers
        .iter()
        .filter(|p| p.pubkey != self_pubkey)
        .filter_map(|p| {
            if p.pubkey.is_zero() {
                tracing::warn!(
                    peer_name = %p.name,
                    "skipping zero-pubkey trusted peer in MCP peer directory"
                );
                return None;
            }
            let peer_id = p.pubkey.to_peer_id();
            if peer_id_counts.get(&peer_id).copied().unwrap_or(0) != 1 {
                tracing::warn!(
                    peer_name = %p.name,
                    peer_id = %peer_id,
                    "skipping duplicate trusted peer id in MCP peer directory"
                );
                return None;
            }
            let name = match PeerName::new(p.name.clone()) {
                Ok(name) => name,
                Err(err) => {
                    tracing::warn!(
                        peer_name = %p.name,
                        peer_id = %peer_id,
                        error = %err,
                        "skipping trusted peer with invalid name in MCP peer directory"
                    );
                    return None;
                }
            };
            let address = match PeerAddress::parse(&p.addr) {
                Ok(address) => address,
                Err(err) => {
                    tracing::warn!(
                        peer_name = %p.name,
                        peer_id = %peer_id,
                        address = %p.addr,
                        error = %err,
                        "skipping trusted peer with invalid address in MCP peer directory"
                    );
                    return None;
                }
            };
            Some(PeerDirectoryEntry {
                name,
                peer_id,
                address,
                source: PeerDirectorySource::Trusted,
                sendable_kinds: sendable_kinds.clone(),
                capabilities: PeerCapabilitySet::default(),
                reachability: PeerReachability::Unknown,
                last_unreachable_reason: None,
                meta: p.meta.clone(),
            })
        })
        .collect()
}

fn peer_sendability_authorized_by_tools(ctx: &ToolContext) -> Vec<PeerSendability> {
    PeerSendability::directory_defaults()
        .into_iter()
        .filter(|kind| {
            comms_tool_unavailable_reason(ctx, peer_sendability_tool_name(*kind)).is_none()
        })
        .collect()
}

fn peer_sendability_tool_name(kind: PeerSendability) -> &'static str {
    match kind {
        PeerSendability::PeerMessage => "send_message",
        PeerSendability::PeerRequest => "send_request",
        PeerSendability::PeerResponse => "send_response",
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::{PubKey, TrustedPeer};
    use meerkat_contracts::wire::supervisor_bridge::{
        BridgeAck, BridgeCommand, BridgePeerSpec, BridgeReply, BridgeSupervisorPayload,
        supervisor_bridge_current_protocol_version,
    };
    use meerkat_core::comms::{
        PeerAddress, PeerCapabilitySet, PeerDirectorySource, PeerReachability, PeerSendability,
        PeerTransport,
    };
    use parking_lot::Mutex;
    use std::sync::LazyLock;
    use tokio::sync::Notify;

    static INPROC_REGISTRY_LOCK: LazyLock<tokio::sync::Mutex<()>> =
        LazyLock::new(|| tokio::sync::Mutex::new(()));

    fn extract_projection_send_response_args(text: &str) -> Value {
        let marker = "send_response with arguments ";
        let start = text
            .find(marker)
            .map(|idx| idx + marker.len())
            .expect("projection must include schema-owned send_response argument JSON");
        let json_start = text[start..]
            .find('{')
            .map(|idx| start + idx)
            .expect("projection must include a JSON argument object");
        let json_end = text[json_start..]
            .find("}.")
            .map(|idx| json_start + idx)
            .expect("projection JSON argument object must end before the sentence");
        serde_json::from_str(&text[json_start..=json_end])
            .expect("projection argument object must parse as JSON")
    }

    fn bridge_peer_spec() -> BridgePeerSpec {
        BridgePeerSpec {
            name: "supervisor".to_string(),
            peer_id: PeerId::parse("11111111-1111-4111-8111-111111111111")
                .expect("valid peer id")
                .to_string(),
            address: "inproc://supervisor".to_string(),
            pubkey: [7u8; 32],
        }
    }

    fn bridge_command_json() -> Value {
        serde_json::to_value(BridgeCommand::ObserveMember(BridgeSupervisorPayload {
            supervisor: bridge_peer_spec(),
            epoch: 1,
            protocol_version: supervisor_bridge_current_protocol_version(),
        }))
        .expect("bridge command should serialize")
    }

    fn bridge_reply_json() -> Value {
        serde_json::to_value(BridgeReply::Ack(BridgeAck { ok: true }))
            .expect("bridge reply should serialize")
    }

    fn checksum_token_reply_json(subject: &str, token: &str) -> Value {
        json!({
            "request_intent": "checksum_token",
            "request_subject": subject,
            "token": token
        })
    }

    struct RecordingRuntime {
        sent: Mutex<Vec<CommsCommand>>,
        peers: Vec<PeerDirectoryEntry>,
        notify: Arc<Notify>,
    }

    impl RecordingRuntime {
        fn new() -> Self {
            Self {
                sent: Mutex::new(Vec::new()),
                peers: Vec::new(),
                notify: Arc::new(Notify::new()),
            }
        }

        fn with_peers(peers: Vec<PeerDirectoryEntry>) -> Self {
            Self {
                sent: Mutex::new(Vec::new()),
                peers,
                notify: Arc::new(Notify::new()),
            }
        }

        fn sent_len(&self) -> usize {
            self.sent.lock().len()
        }
    }

    #[async_trait::async_trait]
    impl CoreCommsRuntime for RecordingRuntime {
        async fn send(
            &self,
            cmd: CommsCommand,
        ) -> Result<meerkat_core::comms::SendReceipt, meerkat_core::comms::SendError> {
            let receipt = match &cmd {
                CommsCommand::PeerMessage { .. } => {
                    meerkat_core::comms::SendReceipt::PeerMessageSent {
                        envelope_id: uuid::Uuid::from_u128(5),
                        acked: false,
                    }
                }
                CommsCommand::PeerRequest { stream, .. } => {
                    meerkat_core::comms::SendReceipt::PeerRequestSent {
                        envelope_id: uuid::Uuid::from_u128(1),
                        interaction_id: InteractionId(uuid::Uuid::from_u128(2)),
                        stream_reserved: *stream == InputStreamMode::ReserveInteraction,
                    }
                }
                CommsCommand::PeerResponse { in_reply_to, .. } => {
                    meerkat_core::comms::SendReceipt::PeerResponseSent {
                        envelope_id: uuid::Uuid::from_u128(3),
                        in_reply_to: *in_reply_to,
                    }
                }
                other => {
                    return Err(meerkat_core::comms::SendError::Unsupported(format!(
                        "unexpected command in test: {}",
                        other.command_kind()
                    )));
                }
            };
            self.sent.lock().push(cmd);
            Ok(receipt)
        }

        async fn drain_messages(&self) -> Vec<String> {
            Vec::new()
        }

        fn inbox_notify(&self) -> Arc<Notify> {
            Arc::clone(&self.notify)
        }

        async fn peers(&self) -> Vec<PeerDirectoryEntry> {
            self.peers.clone()
        }
    }

    fn make_trusted_runtime_less_context(
        peer_keypair: &Keypair,
    ) -> (ToolContext, meerkat_core::comms::PeerId) {
        let sender_keypair = Keypair::generate();
        let peer_id = peer_keypair.public_key().to_peer_id();
        let trusted_peers = TrustedPeers {
            peers: vec![TrustedPeer {
                name: "test-peer".to_string(),
                pubkey: peer_keypair.public_key(),
                addr: "inproc://test-peer".to_string(),
                meta: crate::PeerMeta::default(),
            }],
        };
        let trusted_peers = Arc::new(RwLock::new(trusted_peers));
        let (_, inbox_sender) = crate::Inbox::new();
        let router = Arc::new(Router::with_shared_peers(
            sender_keypair,
            trusted_peers.clone(),
            CommsConfig::default(),
            inbox_sender,
            true,
        ));

        (
            ToolContext {
                router,
                trusted_peers,
                runtime: None,
            },
            peer_id,
        )
    }

    async fn make_runtime_less_inproc_context() -> (ToolContext, meerkat_core::comms::PeerId) {
        let _lock = INPROC_REGISTRY_LOCK.lock().await;
        crate::InprocRegistry::global().clear();

        let peer_keypair = Keypair::generate();
        let (_receiver_inbox, receiver_sender) = crate::Inbox::new();
        crate::InprocRegistry::global().register(
            "test-peer",
            peer_keypair.public_key(),
            receiver_sender,
        );

        make_trusted_runtime_less_context(&peer_keypair)
    }

    #[test]
    fn test_tools_list_has_four_tools() {
        let tools = tools_list();
        assert_eq!(tools.len(), 4);
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"send_message"));
        assert!(names.contains(&"send_request"));
        assert!(names.contains(&"send_response"));
        assert!(names.contains(&"peers"));
    }

    #[test]
    fn test_tool_descriptions_document_image_refs_and_checksum_contracts() {
        let tools = tools_list();
        let description = |name: &str| -> String {
            tools
                .iter()
                .find(|tool| tool["name"].as_str() == Some(name))
                .and_then(|tool| tool["description"].as_str())
                .expect("tool description")
                .to_string()
        };

        for name in ["send_message", "send_request", "send_response"] {
            let text = description(name);
            assert!(
                text.contains("\"source\":\"blob\""),
                "{name} should document blob-backed generated image refs"
            );
            assert!(
                text.contains("\"source\":\"current_turn\""),
                "{name} should document current-turn user image refs"
            );
            assert!(
                text.contains("generated images must be sent with source=blob"),
                "{name} should distinguish generated images from current-turn input"
            );
        }

        let request = description("send_request");
        assert!(request.contains("checksum_token"));
        assert!(request.contains("\"params\":{\"subject\""));

        let response = description("send_response");
        assert!(response.contains("\"request_intent\":\"checksum_token\""));
        assert!(response.contains("\"request_subject\""));
    }

    #[test]
    fn test_send_message_schema_requires_handling_mode() {
        let schema = schema_for::<SendMessageInput>();
        let required = schema["required"].as_array().unwrap();
        let required_names: Vec<&str> = required.iter().map(|v| v.as_str().unwrap()).collect();
        assert!(
            required_names.contains(&"handling_mode"),
            "send_message must require handling_mode, got required: {required_names:?}"
        );
        assert!(required_names.contains(&"peer_id"));
        assert!(!required_names.contains(&"display_name"));
        assert!(required_names.contains(&"body"));
        assert!(!required_names.contains(&"blocks"));
        assert!(
            schema["properties"]
                .as_object()
                .unwrap()
                .contains_key("blocks")
        );
    }

    #[test]
    fn test_send_request_schema_requires_handling_mode() {
        let schema = schema_for::<SendRequestInput>();
        let required = schema["required"].as_array().unwrap();
        let required_names: Vec<&str> = required.iter().map(|v| v.as_str().unwrap()).collect();
        assert!(
            required_names.contains(&"handling_mode"),
            "send_request must require handling_mode, got required: {required_names:?}"
        );
        assert!(required_names.contains(&"peer_id"));
        assert!(!required_names.contains(&"display_name"));
        assert!(required_names.contains(&"intent"));
        assert!(required_names.contains(&"params"));
        assert!(!required_names.contains(&"blocks"));
        assert!(
            schema["properties"]
                .as_object()
                .unwrap()
                .contains_key("blocks")
        );
    }

    #[tokio::test]
    async fn send_message_resolves_current_turn_image_ref_to_peer_message_blocks() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair);
        let runtime = Arc::new(RecordingRuntime::new());
        ctx.runtime = Some(RuntimeCommsCommandHandle::new(runtime.clone()));
        let dispatch_context = meerkat_core::ToolDispatchContext::from_current_turn_input(
            &meerkat_core::ContentInput::Blocks(vec![
                meerkat_core::ContentBlock::Text {
                    text: "please forward this".to_string(),
                },
                meerkat_core::ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: "iVBORw0KGgo=".into(),
                },
            ]),
        );

        handle_tools_call_with_context(
            &ctx,
            "send_message",
            &json!({
                "peer_id": peer_id,
                "body": "Please describe the attached image.",
                "blocks": [
                    {"type": "text", "text": "Please describe the attached image."},
                    {"type": "image_ref", "source": "current_turn", "index": 0}
                ],
                "handling_mode": "steer"
            }),
            &dispatch_context,
        )
        .await
        .expect("send_message should resolve current-turn image refs");

        let sent = runtime.sent.lock();
        let [
            CommsCommand::PeerMessage {
                body,
                blocks: Some(blocks),
                ..
            },
        ] = sent.as_slice()
        else {
            panic!("expected one peer message with blocks, got {sent:?}");
        };
        assert_eq!(body, "Please describe the attached image.");
        assert!(matches!(
            &blocks[0],
            meerkat_core::ContentBlock::Text { text }
                if text == "Please describe the attached image."
        ));
        assert!(matches!(
            &blocks[1],
            meerkat_core::ContentBlock::Image {
                media_type,
                data: meerkat_core::ImageData::Inline { data }
            } if media_type == "image/png" && data == "iVBORw0KGgo="
        ));
    }

    #[tokio::test]
    async fn send_request_resolves_current_turn_image_ref_to_peer_request_blocks() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair);
        let runtime = Arc::new(RecordingRuntime::new());
        ctx.runtime = Some(RuntimeCommsCommandHandle::new(runtime.clone()));
        let dispatch_context = meerkat_core::ToolDispatchContext::from_current_turn_input(
            &meerkat_core::ContentInput::Blocks(vec![meerkat_core::ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: "iVBORw0KGgo=".into(),
            }]),
        );

        handle_tools_call_with_context(
            &ctx,
            "send_request",
            &json!({
                "peer_id": peer_id,
                "intent": "checksum_token",
                "params": {"subject": "describe-image"},
                "blocks": [
                    {"type": "text", "text": "Please describe the attached image."},
                    {"type": "image_ref", "source": "current_turn", "index": 0}
                ],
                "handling_mode": "steer"
            }),
            &dispatch_context,
        )
        .await
        .expect("send_request should resolve current-turn image refs");

        let sent = runtime.sent.lock();
        let [
            CommsCommand::PeerRequest {
                blocks: Some(blocks),
                ..
            },
        ] = sent.as_slice()
        else {
            panic!("expected one peer request with blocks, got {sent:?}");
        };
        assert!(matches!(
            &blocks[0],
            meerkat_core::ContentBlock::Text { text }
                if text == "Please describe the attached image."
        ));
        assert!(matches!(
            &blocks[1],
            meerkat_core::ContentBlock::Image {
                media_type,
                data: meerkat_core::ImageData::Inline { data }
            } if media_type == "image/png" && data == "iVBORw0KGgo="
        ));
    }

    #[tokio::test]
    async fn send_message_resolves_blob_image_ref_to_peer_message_blocks() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair);
        let runtime = Arc::new(RecordingRuntime::new());
        ctx.runtime = Some(RuntimeCommsCommandHandle::new(runtime.clone()));

        handle_tools_call_with_context(
            &ctx,
            "send_message",
            &json!({
                "peer_id": peer_id,
                "body": "Please review the generated image.",
                "blocks": [
                    {"type": "image_ref", "source": "blob", "blob_id": "sha256:generated-image", "media_type": "image/png"}
                ],
                "handling_mode": "steer"
            }),
            &ToolDispatchContext::default(),
        )
        .await
        .expect("send_message should resolve blob-backed image refs");

        let sent = runtime.sent.lock();
        let [
            CommsCommand::PeerMessage {
                body,
                blocks: Some(blocks),
                ..
            },
        ] = sent.as_slice()
        else {
            panic!("expected one peer message with blocks, got {sent:?}");
        };
        assert_eq!(body, "Please review the generated image.");
        assert_eq!(
            blocks.first(),
            Some(&meerkat_core::ContentBlock::Text {
                text: "Please review the generated image.".into()
            })
        );
        assert!(matches!(
            &blocks[1],
            meerkat_core::ContentBlock::Image {
                media_type,
                data: meerkat_core::ImageData::Blob { blob_id }
            } if media_type == "image/png" && blob_id.as_str() == "sha256:generated-image"
        ));
    }

    #[tokio::test]
    async fn send_request_resolves_blob_image_ref_to_peer_request_blocks() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair);
        let runtime = Arc::new(RecordingRuntime::new());
        ctx.runtime = Some(RuntimeCommsCommandHandle::new(runtime.clone()));

        handle_tools_call_with_context(
            &ctx,
            "send_request",
            &json!({
                "peer_id": peer_id,
                "intent": "checksum_token",
                "params": {"subject": "describe-generated-image"},
                "blocks": [
                    {"type": "text", "text": "Please describe the generated image."},
                    {"type": "image_ref", "source": "blob", "blob_id": "sha256:generated-request-image", "media_type": "image/png"}
                ],
                "handling_mode": "steer"
            }),
            &ToolDispatchContext::default(),
        )
        .await
        .expect("send_request should resolve blob-backed image refs");

        let sent = runtime.sent.lock();
        let [
            CommsCommand::PeerRequest {
                blocks: Some(blocks),
                ..
            },
        ] = sent.as_slice()
        else {
            panic!("expected one peer request with blocks, got {sent:?}");
        };
        assert!(matches!(
            &blocks[0],
            meerkat_core::ContentBlock::Text { text }
                if text == "Please describe the generated image."
        ));
        assert!(matches!(
            &blocks[1],
            meerkat_core::ContentBlock::Image {
                media_type,
                data: meerkat_core::ImageData::Blob { blob_id }
            } if media_type == "image/png" && blob_id.as_str() == "sha256:generated-request-image"
        ));
    }

    #[tokio::test]
    async fn blob_image_ref_rejects_current_turn_index() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair);
        let runtime = Arc::new(RecordingRuntime::new());
        ctx.runtime = Some(RuntimeCommsCommandHandle::new(runtime));

        let err = handle_tools_call_with_context(
            &ctx,
            "send_message",
            &json!({
                "peer_id": peer_id,
                "body": "bad mixed image ref",
                "blocks": [
                    {"type": "image_ref", "source": "blob", "index": 0, "blob_id": "sha256:generated-image", "media_type": "image/png"}
                ],
                "handling_mode": "steer"
            }),
            &ToolDispatchContext::default(),
        )
        .await
        .expect_err("mixed blob/current_turn fields should be rejected");

        assert_eq!(err, "invalid image_ref: source blob does not accept index");
    }

    #[tokio::test]
    async fn send_message_synthesizes_body_text_when_blocks_only_reference_image() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair);
        let runtime = Arc::new(RecordingRuntime::new());
        ctx.runtime = Some(RuntimeCommsCommandHandle::new(runtime.clone()));
        let dispatch_context = meerkat_core::ToolDispatchContext::from_current_turn_input(
            &meerkat_core::ContentInput::Blocks(vec![meerkat_core::ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: "iVBORw0KGgo=".into(),
            }]),
        );

        handle_tools_call_with_context(
            &ctx,
            "send_message",
            &json!({
                "peer_id": peer_id,
                "body": "Please describe the attached image.",
                "blocks": [
                    {"type": "image_ref", "source": "current_turn", "index": 0}
                ],
                "handling_mode": "steer"
            }),
            &dispatch_context,
        )
        .await
        .expect("send_message should resolve image-only blocks");

        let sent = runtime.sent.lock();
        let [
            CommsCommand::PeerMessage {
                blocks: Some(blocks),
                ..
            },
        ] = sent.as_slice()
        else {
            panic!("expected one peer message with blocks, got {sent:?}");
        };
        assert_eq!(
            blocks.first(),
            Some(&meerkat_core::ContentBlock::Text {
                text: "Please describe the attached image.".into()
            })
        );
        assert!(matches!(
            &blocks[1],
            meerkat_core::ContentBlock::Image {
                media_type,
                data: meerkat_core::ImageData::Inline { data }
            } if media_type == "image/png" && data == "iVBORw0KGgo="
        ));
    }

    #[tokio::test]
    async fn send_message_does_not_duplicate_body_when_text_blocks_split_it() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair);
        let runtime = Arc::new(RecordingRuntime::new());
        ctx.runtime = Some(RuntimeCommsCommandHandle::new(runtime.clone()));

        handle_tools_call_with_context(
            &ctx,
            "send_message",
            &json!({
                "peer_id": peer_id,
                "body": "Please describe\nthe attached image.",
                "blocks": [
                    {"type": "text", "text": "Please describe"},
                    {"type": "text", "text": "the attached image."}
                ],
                "handling_mode": "steer"
            }),
            &ToolDispatchContext::default(),
        )
        .await
        .expect("send_message should accept split text blocks");

        let sent = runtime.sent.lock();
        let [
            CommsCommand::PeerMessage {
                blocks: Some(blocks),
                ..
            },
        ] = sent.as_slice()
        else {
            panic!("expected one peer message with blocks, got {sent:?}");
        };
        assert_eq!(blocks.len(), 2);
        assert!(matches!(
            &blocks[0],
            meerkat_core::ContentBlock::Text { text } if text == "Please describe"
        ));
        assert!(matches!(
            &blocks[1],
            meerkat_core::ContentBlock::Text { text } if text == "the attached image."
        ));
    }

    #[test]
    fn test_send_response_schema_does_not_require_handling_mode() {
        let schema = schema_for::<SendResponseInput>();
        let required = schema["required"].as_array().unwrap();
        let required_names: Vec<&str> = required.iter().map(|v| v.as_str().unwrap()).collect();
        assert!(
            !required_names.contains(&"handling_mode"),
            "send_response must not require handling_mode"
        );
        assert!(required_names.contains(&meerkat_core::SendResponseCallProjection::PEER_ID_FIELD));
        assert!(
            !required_names.contains(&meerkat_core::SendResponseCallProjection::DISPLAY_NAME_FIELD)
        );
        assert!(
            required_names.contains(&meerkat_core::SendResponseCallProjection::IN_REPLY_TO_FIELD)
        );
        assert!(required_names.contains(&meerkat_core::SendResponseCallProjection::STATUS_FIELD));
        assert!(!schema["properties"].as_object().unwrap().contains_key("to"));
        assert!(!required_names.contains(&"blocks"));
        assert!(
            schema["properties"]
                .as_object()
                .unwrap()
                .contains_key("blocks")
        );
    }

    #[test]
    fn peer_request_projection_send_response_args_match_schema() {
        let peer_id = PeerId::parse("11111111-1111-4111-8111-111111111111").expect("valid peer id");
        let request_id =
            uuid::Uuid::parse_str("22222222-2222-4222-8222-222222222222").expect("valid uuid");
        let projection = meerkat_core::format_peer_request_projection(
            peer_id,
            Some("incident-command-center/scribe/primary"),
            request_id,
            "ping",
            &json!({"nonce": 7}),
        );

        assert!(projection.contains("peer_id"));
        assert!(!projection.contains("to=\""));

        let args = extract_projection_send_response_args(&projection);
        let parsed: SendResponseInput =
            serde_json::from_value(args.clone()).expect("projection args must match schema");

        assert_eq!(parsed.peer_id, peer_id);
        assert_eq!(
            parsed.display_name.as_deref(),
            Some("incident-command-center/scribe/primary")
        );
        assert_eq!(parsed.in_reply_to, request_id.to_string());
        assert_eq!(parsed.status, ResponseStatus::Completed);
        assert!(parsed.result.is_none());
        assert!(
            !args
                .as_object()
                .expect("projection args object")
                .contains_key("to")
        );
    }

    #[tokio::test]
    async fn ping_request_projection_send_response_is_accepted_by_tool_boundary() {
        let requester = Keypair::generate();
        let requester_peer_id = requester.public_key().to_peer_id();
        let request_id =
            uuid::Uuid::parse_str("33333333-3333-4333-8333-333333333333").expect("valid uuid");
        let projection = meerkat_core::format_peer_request_projection(
            requester_peer_id,
            Some("operator"),
            request_id,
            "ping",
            &json!({}),
        );
        let args = extract_projection_send_response_args(&projection);

        let trusted_peers = Arc::new(RwLock::new(TrustedPeers {
            peers: vec![TrustedPeer {
                name: "operator".to_string(),
                pubkey: requester.public_key(),
                addr: "inproc://operator".to_string(),
                meta: crate::PeerMeta::default(),
            }],
        }));
        let (_, inbox_sender) = crate::Inbox::new();
        let router = Arc::new(Router::with_shared_peers(
            Keypair::generate(),
            trusted_peers.clone(),
            CommsConfig::default(),
            inbox_sender,
            true,
        ));
        let runtime = Arc::new(RecordingRuntime::new());
        let ctx = ToolContext {
            router,
            trusted_peers,
            runtime: Some(RuntimeCommsCommandHandle::new(runtime.clone())),
        };

        let result = handle_tools_call(&ctx, "send_response", &args)
            .await
            .expect("projection response args should be accepted by send_response");

        assert_eq!(result["status"], "sent");
        let sent = runtime.sent.lock();
        let [
            CommsCommand::PeerResponse {
                to,
                in_reply_to,
                status,
                result,
                ..
            },
        ] = sent.as_slice()
        else {
            panic!("expected one peer response command, got {sent:?}");
        };
        assert_eq!(to.peer_id, requester_peer_id);
        assert_eq!(
            to.display_name.as_ref().map(meerkat_core::PeerName::as_str),
            Some("operator")
        );
        assert_eq!(in_reply_to.0, request_id);
        assert_eq!(*status, ResponseStatus::Completed);
        assert_eq!(*result, Value::Null);
    }

    #[test]
    fn test_send_response_input_accepts_completed_steer_terminal_shape() {
        let peer_id = PeerId::new();
        let request_id = uuid::Uuid::new_v4();

        let input: SendResponseInput = serde_json::from_value(json!({
            "peer_id": peer_id,
            "in_reply_to": request_id.to_string(),
            "status": "completed",
            "result": bridge_reply_json(),
            "handling_mode": "steer"
        }))
        .expect("valid completed terminal response should deserialize");

        assert_eq!(input.peer_id, peer_id);
        assert_eq!(input.in_reply_to, request_id.to_string());
        assert_eq!(input.status, ResponseStatus::Completed);
        assert_eq!(input.handling_mode, Some(HandlingMode::Steer));
        assert_eq!(
            input
                .result
                .map(serde_json::to_value)
                .transpose()
                .expect("typed result should serialize"),
            Some(bridge_reply_json())
        );
    }

    #[tokio::test]
    async fn test_handle_peers() {
        let keypair = Keypair::generate();
        let trusted_peers = TrustedPeers {
            peers: vec![
                TrustedPeer {
                    name: "test-peer".to_string(),
                    pubkey: PubKey::new([1u8; 32]),
                    addr: "tcp://127.0.0.1:4200".to_string(),
                    meta: crate::PeerMeta::default(),
                },
                TrustedPeer {
                    name: "test-peer".to_string(),
                    pubkey: PubKey::new([2u8; 32]),
                    addr: "tcp://127.0.0.1:4201".to_string(),
                    meta: crate::PeerMeta::default(),
                },
            ],
        };
        let trusted_peers = Arc::new(RwLock::new(trusted_peers));
        let (_, inbox_sender) = crate::Inbox::new();
        let router = Arc::new(Router::with_shared_peers(
            keypair,
            trusted_peers.clone(),
            CommsConfig::default(),
            inbox_sender,
            true,
        ));

        let ctx = ToolContext {
            router,
            trusted_peers,
            runtime: None,
        };

        let result = handle_tools_call(&ctx, "peers", &json!({})).await;
        assert!(result.is_ok());
        let val = result.unwrap();
        let peers = val["peers"].as_array().expect("peers should be array");
        assert!(peers.iter().any(|p| p["name"] == "test-peer"));
        assert_eq!(
            peers.iter().filter(|p| p["name"] == "test-peer").count(),
            2,
            "peers must preserve duplicate display names; peer_id is the routing key",
        );
        assert!(
            peers.iter().all(|p| p["peer_id"].is_string()),
            "peers must expose canonical peer_id for routing",
        );
    }

    #[tokio::test]
    async fn test_handle_peers_runtime_payload_uses_typed_directory_contract() {
        let peer_id = PeerId::new();
        let entry = PeerDirectoryEntry {
            peer_id,
            name: PeerName::new("runtime-peer").expect("valid peer name"),
            address: PeerAddress::new(PeerTransport::Inproc, "runtime-peer"),
            source: PeerDirectorySource::Inproc,
            sendable_kinds: vec![PeerSendability::PeerMessage, PeerSendability::PeerRequest],
            capabilities: PeerCapabilitySet::default()
                .with_extension("vendor.echo", json!({ "enabled": true })),
            reachability: PeerReachability::Reachable,
            last_unreachable_reason: None,
            meta: crate::PeerMeta::default(),
        };
        let peer_keypair = Keypair::generate();
        let (mut ctx, _) = make_trusted_runtime_less_context(&peer_keypair);
        let runtime = Arc::new(RecordingRuntime::with_peers(vec![entry]));
        ctx.runtime = Some(RuntimeCommsCommandHandle::new(runtime));

        let value = handle_tools_call(&ctx, "peers", &json!({}))
            .await
            .expect("runtime-backed peers should serialize");
        let peer = &value["peers"][0];

        assert_eq!(peer["peer_id"], peer_id.to_string());
        assert_eq!(peer["source"], "inproc");
        assert_ne!(peer["source"], "Inproc");
        assert_eq!(
            peer["sendable_kinds"],
            json!(["peer_message", "peer_request"])
        );
        assert_eq!(peer["capabilities"]["version"], 1);
        assert_eq!(
            peer["capabilities"]["extensions"]["vendor.echo"]["enabled"],
            true
        );
        assert_eq!(peer["reachability"], "reachable");
    }

    #[tokio::test]
    async fn test_handle_peers_runtime_less_payload_uses_authorized_sendability() {
        let peer_keypair = Keypair::generate();
        let (ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair);

        let value = handle_tools_call(&ctx, "peers", &json!({}))
            .await
            .expect("runtime-less peers should serialize");
        let peer = &value["peers"][0];

        assert_eq!(peer["peer_id"], peer_id.to_string());
        assert_eq!(peer["source"], "trusted");
        assert_eq!(peer["sendable_kinds"], json!(["peer_message"]));
        assert_eq!(peer["capabilities"]["version"], 1);
        assert_eq!(peer["capabilities"]["extensions"], json!({}));
        assert_eq!(peer["reachability"], "unknown");
        assert_eq!(peer["last_unreachable_reason"], Value::Null);
        assert!(peer["meta"].is_object());
    }

    #[test]
    fn test_runtime_less_peer_directory_skips_duplicate_canonical_peer_ids() {
        let sender_keypair = Keypair::generate();
        let peer_keypair = Keypair::generate();
        let peer_pubkey = peer_keypair.public_key();
        let peer_id = peer_pubkey.to_peer_id();
        let trusted_peers = Arc::new(RwLock::new(TrustedPeers::new()));
        let (_, inbox_sender) = crate::Inbox::new();
        let router = Arc::new(Router::with_shared_peers(
            sender_keypair,
            trusted_peers.clone(),
            CommsConfig::default(),
            inbox_sender,
            true,
        ));
        trusted_peers.write().peers.extend([
            TrustedPeer {
                name: "runtime-less-primary".to_string(),
                pubkey: peer_pubkey,
                addr: "inproc://runtime-less-primary".to_string(),
                meta: crate::PeerMeta::default(),
            },
            TrustedPeer {
                name: "runtime-less-shadow".to_string(),
                pubkey: peer_pubkey,
                addr: "inproc://runtime-less-shadow".to_string(),
                meta: crate::PeerMeta::default(),
            },
        ]);
        let ctx = ToolContext {
            router,
            trusted_peers,
            runtime: None,
        };

        assert!(
            ensure_peer_id_is_trusted(&ctx, &peer_id).is_err(),
            "duplicate canonical trust is not send-resolvable"
        );

        let peers = runtime_less_peer_directory(&ctx);
        assert!(
            peers.iter().all(|peer| peer.peer_id != peer_id),
            "runtime-less peer directory must not advertise duplicate canonical ids: {peers:?}"
        );
    }

    #[tokio::test]
    async fn test_send_message_fails_when_recipient_is_not_trusted() {
        let sender_keypair = Keypair::generate();
        let receiver_id = PeerId::new();

        let trusted_peers = Arc::new(RwLock::new(TrustedPeers::new()));
        let (_, router_inbox_sender) = crate::Inbox::new();
        let router = Arc::new(Router::with_shared_peers(
            sender_keypair,
            trusted_peers.clone(),
            CommsConfig::default(),
            router_inbox_sender,
            true,
        ));

        let ctx = ToolContext {
            router,
            trusted_peers,
            runtime: None,
        };

        let result = handle_tools_call(
            &ctx,
            "send_message",
            &json!({
                "peer_id": receiver_id,
                "body": "hello",
                "handling_mode": "steer"
            }),
        )
        .await;

        let error = result.expect_err("send should fail for an unreachable peer");
        assert!(
            error.starts_with("peer_not_found_or_not_trusted:"),
            "expected stable sender-facing code, got: {error}"
        );
    }

    #[tokio::test]
    async fn test_send_message_invalid_handling_mode_fails_at_serde_boundary() {
        let keypair = Keypair::generate();
        let trusted_peers = Arc::new(RwLock::new(TrustedPeers::new()));
        let (_, inbox_sender) = crate::Inbox::new();
        let router = Arc::new(Router::with_shared_peers(
            keypair,
            trusted_peers.clone(),
            CommsConfig::default(),
            inbox_sender,
            true,
        ));
        let ctx = ToolContext {
            router,
            trusted_peers,
            runtime: None,
        };

        // "almost" is not a valid HandlingMode — typed enum rejects it at
        // deserialization, never reaches the runtime.
        let result = handle_tools_call(
            &ctx,
            "send_message",
            &json!({
                "peer_id": PeerId::new(),
                "body": "hello",
                "handling_mode": "almost"
            }),
        )
        .await;

        let error = result.expect_err("invalid handling_mode must be rejected");
        assert!(
            error.contains("Invalid arguments")
                && (error.contains("handling_mode") || error.contains("almost")),
            "expected serde error mentioning handling_mode, got: {error}"
        );
    }

    #[tokio::test]
    async fn test_unknown_tool_returns_error() {
        let keypair = Keypair::generate();
        let trusted_peers = Arc::new(RwLock::new(TrustedPeers::new()));
        let (_, inbox_sender) = crate::Inbox::new();
        let router = Arc::new(Router::with_shared_peers(
            keypair,
            trusted_peers.clone(),
            CommsConfig::default(),
            inbox_sender,
            true,
        ));
        let ctx = ToolContext {
            router,
            trusted_peers,
            runtime: None,
        };

        assert!(
            handle_tools_call(&ctx, "nonexistent", &json!({}))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn test_send_request_without_runtime_command_authority_fails_closed() {
        let (ctx, peer_id) = make_runtime_less_inproc_context().await;

        let error = handle_tools_call(
            &ctx,
            "send_request",
            &json!({
                "peer_id": peer_id,
                "intent": "supervisor.bridge",
                "params": bridge_command_json(),
                "handling_mode": "steer"
            }),
        )
        .await
        .expect_err("runtime-less send_request must not fall back to router send");

        assert!(
            error.contains("runtime_command_authority_unavailable"),
            "expected runtime command authority error, got: {error}"
        );

        crate::InprocRegistry::global().clear();
    }

    #[tokio::test]
    async fn test_send_response_without_runtime_command_authority_fails_closed() {
        let (ctx, peer_id) = make_runtime_less_inproc_context().await;
        let request_id = uuid::Uuid::new_v4();

        let error = handle_tools_call(
            &ctx,
            "send_response",
            &json!({
                "peer_id": peer_id,
                "in_reply_to": request_id.to_string(),
                "status": "completed",
                "result": bridge_reply_json()
            }),
        )
        .await
        .expect_err("runtime-less send_response must not fall back to router send");

        assert!(
            error.contains("runtime_command_authority_unavailable"),
            "expected runtime command authority error, got: {error}"
        );

        crate::InprocRegistry::global().clear();
    }

    #[tokio::test]
    async fn test_send_request_unknown_intent_fails_at_typed_boundary() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair);
        let runtime = Arc::new(RecordingRuntime::new());
        ctx.runtime = Some(RuntimeCommsCommandHandle::new(runtime.clone()));

        let error = handle_tools_call(
            &ctx,
            "send_request",
            &json!({
                "peer_id": peer_id,
                "intent": "review",
                "params": bridge_command_json(),
                "handling_mode": "queue"
            }),
        )
        .await
        .expect_err("unknown public comms intent must fail before dispatch");

        assert!(
            error.contains("Invalid arguments") && error.contains("review"),
            "expected typed intent serde error, got: {error}"
        );
        assert_eq!(runtime.sent_len(), 0);
    }

    #[tokio::test]
    async fn test_send_request_malformed_params_fail_at_typed_boundary() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair);
        let runtime = Arc::new(RecordingRuntime::new());
        ctx.runtime = Some(RuntimeCommsCommandHandle::new(runtime.clone()));

        let error = handle_tools_call(
            &ctx,
            "send_request",
            &json!({
                "peer_id": peer_id,
                "intent": "supervisor.bridge",
                "params": {"file": "main.rs"},
                "handling_mode": "queue"
            }),
        )
        .await
        .expect_err("malformed public comms request params must fail before dispatch");

        assert!(
            error.contains("Invalid arguments")
                && (error.contains("command") || error.contains("did not match any variant")),
            "expected typed params serde error, got: {error}"
        );
        assert_eq!(runtime.sent_len(), 0);
    }

    #[tokio::test]
    async fn test_send_response_malformed_result_fails_at_typed_boundary() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair);
        let runtime = Arc::new(RecordingRuntime::new());
        ctx.runtime = Some(RuntimeCommsCommandHandle::new(runtime.clone()));
        let request_id = uuid::Uuid::from_u128(4);

        let error = handle_tools_call(
            &ctx,
            "send_response",
            &json!({
                "peer_id": peer_id,
                "in_reply_to": request_id.to_string(),
                "status": "completed",
                "result": {"ok": true}
            }),
        )
        .await
        .expect_err("malformed public comms response result must fail before dispatch");

        assert!(
            error.contains("Invalid arguments")
                && (error.contains("result") || error.contains("did not match any variant")),
            "expected typed result serde error, got: {error}"
        );
        assert_eq!(runtime.sent_len(), 0);
    }

    #[tokio::test]
    async fn test_send_response_checksum_token_requires_request_subject() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair);
        let runtime = Arc::new(RecordingRuntime::new());
        ctx.runtime = Some(RuntimeCommsCommandHandle::new(runtime.clone()));
        let request_id = uuid::Uuid::from_u128(4);

        let error = handle_tools_call(
            &ctx,
            "send_response",
            &json!({
                "peer_id": peer_id,
                "in_reply_to": request_id.to_string(),
                "status": "completed",
                "result": {
                    "request_intent": "checksum_token",
                    "token": "birch seventeen"
                }
            }),
        )
        .await
        .expect_err("checksum token response without request_subject must fail before dispatch");

        assert!(
            error.contains("Invalid arguments"),
            "expected typed result serde error, got: {error}"
        );
        assert_eq!(runtime.sent_len(), 0);
    }

    #[tokio::test]
    async fn test_runtime_bound_send_request_returns_typed_receipt() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair);
        let runtime = Arc::new(RecordingRuntime::new());
        ctx.runtime = Some(RuntimeCommsCommandHandle::new(runtime.clone()));

        let result = handle_tools_call(
            &ctx,
            "send_request",
            &json!({
                "peer_id": peer_id,
                "intent": "supervisor.bridge",
                "params": bridge_command_json(),
                "handling_mode": "queue"
            }),
        )
        .await
        .expect("runtime-backed send_request should succeed");

        assert_eq!(runtime.sent_len(), 1);
        assert_eq!(result["status"], "sent");
        assert_eq!(result["kind"], "peer_request");
        assert_eq!(result["receipt"]["kind"], "peer_request_sent");
        assert_eq!(
            result["receipt"]["envelope_id"],
            uuid::Uuid::from_u128(1).to_string()
        );
        assert_eq!(
            result["receipt"]["interaction_id"],
            uuid::Uuid::from_u128(2).to_string()
        );
        assert_eq!(result["receipt"]["stream_reserved"], true);
        let sent = runtime.sent.lock();
        let [CommsCommand::PeerRequest { intent, params, .. }] = sent.as_slice() else {
            panic!("expected one peer request command, got {sent:?}");
        };
        assert_eq!(intent, meerkat_core::comms::SUPERVISOR_BRIDGE_INTENT);
        assert_eq!(params, &bridge_command_json());
    }

    #[tokio::test]
    async fn test_runtime_bound_send_response_returns_typed_receipt() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair);
        let runtime = Arc::new(RecordingRuntime::new());
        ctx.runtime = Some(RuntimeCommsCommandHandle::new(runtime.clone()));
        let request_id = uuid::Uuid::from_u128(4);

        let result = handle_tools_call(
            &ctx,
            "send_response",
            &json!({
                "peer_id": peer_id,
                "in_reply_to": request_id.to_string(),
                "status": "completed",
                "result": bridge_reply_json()
            }),
        )
        .await
        .expect("runtime-backed send_response should succeed");

        assert_eq!(runtime.sent_len(), 1);
        assert_eq!(result["status"], "sent");
        assert_eq!(result["kind"], "peer_response");
        assert_eq!(result["receipt"]["kind"], "peer_response_sent");
        assert_eq!(
            result["receipt"]["envelope_id"],
            uuid::Uuid::from_u128(3).to_string()
        );
        assert_eq!(result["receipt"]["in_reply_to"], request_id.to_string());
        let sent = runtime.sent.lock();
        let [CommsCommand::PeerResponse { result, .. }] = sent.as_slice() else {
            panic!("expected one peer response command, got {sent:?}");
        };
        assert_eq!(result, &bridge_reply_json());
    }

    #[tokio::test]
    async fn send_response_resolves_blob_image_ref_to_peer_response_blocks() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair);
        let runtime = Arc::new(RecordingRuntime::new());
        ctx.runtime = Some(RuntimeCommsCommandHandle::new(runtime.clone()));
        let request_id = uuid::Uuid::from_u128(44);

        handle_tools_call_with_context(
            &ctx,
            "send_response",
            &json!({
                "peer_id": peer_id,
                "in_reply_to": request_id.to_string(),
                "status": "completed",
                "result": bridge_reply_json(),
                "blocks": [
                    {"type": "text", "text": "Generated image attached."},
                    {"type": "image_ref", "source": "blob", "blob_id": "sha256:response-image", "media_type": "image/png"}
                ]
            }),
            &ToolDispatchContext::default(),
        )
        .await
        .expect("send_response should resolve blob-backed image refs");

        let sent = runtime.sent.lock();
        let [
            CommsCommand::PeerResponse {
                blocks: Some(blocks),
                ..
            },
        ] = sent.as_slice()
        else {
            panic!("expected one peer response command with blocks, got {sent:?}");
        };
        assert!(matches!(
            &blocks[0],
            meerkat_core::ContentBlock::Text { text } if text == "Generated image attached."
        ));
        assert!(matches!(
            &blocks[1],
            meerkat_core::ContentBlock::Image {
                media_type,
                data: meerkat_core::ImageData::Blob { blob_id }
            } if media_type == "image/png" && blob_id.as_str() == "sha256:response-image"
        ));
    }

    #[tokio::test]
    async fn test_runtime_bound_send_response_accepts_checksum_token_subject() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair);
        let runtime = Arc::new(RecordingRuntime::new());
        ctx.runtime = Some(RuntimeCommsCommandHandle::new(runtime.clone()));
        let request_id = uuid::Uuid::from_u128(4);

        let result = handle_tools_call(
            &ctx,
            "send_response",
            &json!({
                "peer_id": peer_id,
                "in_reply_to": request_id.to_string(),
                "status": "completed",
                "result": checksum_token_reply_json("alpha beta gamma", "birch seventeen")
            }),
        )
        .await
        .expect("runtime-backed checksum token send_response should succeed");

        assert_eq!(runtime.sent_len(), 1);
        assert_eq!(result["status"], "sent");
        assert_eq!(result["kind"], "peer_response");
        let sent = runtime.sent.lock();
        let [CommsCommand::PeerResponse { result, .. }] = sent.as_slice() else {
            panic!("expected one peer response command, got {sent:?}");
        };
        assert_eq!(
            result,
            &checksum_token_reply_json("alpha beta gamma", "birch seventeen")
        );
    }
}

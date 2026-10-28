//! MCP tool implementations for Meerkat comms.
//!
//! Exposes agent-facing comms tools: `send_message`, `send_request`,
//! `send_response`, and `peers`.
//!
//! The tool inputs deserialize into typed per-tool input structs; peer
//! request/response traffic then projects through the generated comms
//! contract before it becomes a [`CommsCommand`] domain envelope.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::sync::Arc;

#[cfg(test)]
use crate::{CommsConfig, Keypair};
use crate::{Router, TrustedPeersView};
use meerkat_contracts::{
    CommsPeerRequestIntent, CommsPeerRequestParams, CommsPeerResponseResult, CommsPeersResult,
    CommsSendResult,
};
use meerkat_core::BlobId;
use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
use meerkat_core::comms::{
    CommsCommand, InputStreamMode, PeerCapabilitySet, PeerDirectoryEntry, PeerDirectorySource,
    PeerId, PeerName, PeerReplyCapability, PeerReplyDispatchContext, PeerRoute, PeerSendability,
    SendError, SendReceipt,
};
use meerkat_core::error::ToolError;
use meerkat_core::interaction::{InteractionId, ResponseStatus};
use meerkat_core::tool_catalog::ToolUnavailableReason;
use meerkat_core::types::{ContentBlock, HandlingMode, ImageData};
use meerkat_core::{CurrentTurnImageRef, ToolDispatchContext};
use meerkat_core::{PeerSendKind, ToolOperationTarget};

const RUNTIME_COMMAND_AUTHORITY_UNAVAILABLE_CODE: &str = "runtime_command_authority_unavailable";

const SEND_REQUEST_CONTRACTS_DESCRIPTION: &str = "\n\nSupported request contracts:\n- checksum_token: Use for a simple correlated check/ack/review. params must be {\"subject\":\"<subject>\"}. The responder should send_response with status completed and result {\"request_intent\":\"checksum_token\",\"request_subject\":\"<same subject>\",\"token\":\"<token or checksum>\"}.\n- supervisor.bridge: Use for supervisor bridge control. params include the bridge command payload, for example command observe_member with supervisor, epoch, and protocol_version fields.";

const SEND_RESPONSE_CONTRACTS_DESCRIPTION: &str = "\n\nResponse result contracts:\n- For checksum_token requests, completed responses must use result {\"request_intent\":\"checksum_token\",\"request_subject\":\"<same subject from request params.subject>\",\"token\":\"<token or checksum>\"}.\n- For supervisor.bridge requests, completed acknowledgements use result {\"result\":\"ack\",\"ok\":true}; failed responses use result {\"result\":\"rejected\",\"cause\":\"unsupported\",\"reason\":\"<reason>\"}.";

fn schema_for<T: JsonSchema>() -> Value {
    let schema = schemars::schema_for!(T);
    let mut value = schema.to_value();

    if let Value::Object(ref mut obj) = value {
        // The root `title`/`description` come from the input struct's name and
        // rustdoc; the tool description already owns that text, so repeating
        // it in every request's tool schema only costs tokens.
        obj.remove("title");
        obj.remove("description");
    }

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
/// Example: `{"peer_id": "<peer-id-from-peers>", "body": "What is the current time?", "handling_mode": "queue"}`
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendMessageInput {
    /// Canonical peer ID from the `peers` tool
    pub peer_id: PeerId,
    /// Optional display name retained only for diagnostics
    #[serde(default)]
    pub display_name: Option<String>,
    /// Message body
    pub body: String,
    /// Optional multimodal blocks (text and images) sent alongside the body.
    /// {"type":"image_ref","source":"current_turn","index":0} refers only to
    /// an image attached to the current admitted user input turn; index counts
    /// only that turn's image blocks (0 = the first image). Generated or other
    /// blob-backed images, such as an image returned by generate_image, must
    /// use {"type":"image_ref","source":"blob","blob_id":"sha256:...","media_type":"image/png"};
    /// generated images must be sent with source=blob, never source=current_turn.
    #[serde(default)]
    pub blocks: Option<Vec<CommsToolContentBlock>>,
    /// "queue" for next turn boundary (normal), "steer" for urgent preemption
    pub handling_mode: HandlingMode,
}

/// Reply to a peer message delivered in the current turn.
///
/// Pre-addressed: the runtime minted a typed reply capability for this turn's
/// incoming peer message(s), so no `peer_id` is supplied — only content.
///
/// Example: `{"body": "Done — the migration completed.", "handling_mode": "queue"}`
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplyToPeerInput {
    /// Optional delivery selector, only needed when more than one peer
    /// message arrived in this turn. Set it to one of the delivery ids listed
    /// by an `ambiguous_reply` error.
    #[serde(default)]
    pub reply_to: Option<String>,
    /// Reply body
    pub body: String,
    /// Optional multimodal blocks (text and images) sent alongside the body.
    /// {"type":"image_ref","source":"current_turn","index":0} refers only to
    /// an image attached to the current admitted user input turn; index counts
    /// only that turn's image blocks (0 = the first image). Generated or other
    /// blob-backed images, such as an image returned by generate_image, must
    /// use {"type":"image_ref","source":"blob","blob_id":"sha256:...","media_type":"image/png"};
    /// generated images must be sent with source=blob, never source=current_turn.
    #[serde(default)]
    pub blocks: Option<Vec<CommsToolContentBlock>>,
    /// "queue" for next turn boundary (normal), "steer" for urgent preemption
    pub handling_mode: HandlingMode,
}

/// Send a structured request to a peer and expect a correlated response.
///
/// Example: `{"peer_id": "<peer-id-from-peers>", "intent": "supervisor.bridge", "params": {"command": "observe_member", ...}, "handling_mode": "queue"}`
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendRequestInput {
    /// Canonical peer ID from the `peers` tool
    pub peer_id: PeerId,
    /// Optional display name retained only for diagnostics
    #[serde(default)]
    pub display_name: Option<String>,
    /// Request intent
    pub intent: CommsPeerRequestIntent,
    /// "queue" for next turn boundary (normal), "steer" for urgent preemption
    pub handling_mode: HandlingMode,
    /// Request parameters. Shape is validated against the selected intent at
    /// the typed dispatch boundary; model-facing schema stays compact so
    /// internal bridge wire variants do not dominate ordinary comms turns.
    #[schemars(
        with = "Value",
        description = "Request parameters for the selected intent. For checksum_token use {\"subject\":\"image_receipt_check\"}. For supervisor.bridge use the bridge command payload described in the tool text."
    )]
    pub params: CommsPeerRequestParams,
    /// Optional multimodal blocks (text and images) sent alongside the body.
    /// {"type":"image_ref","source":"current_turn","index":0} refers only to
    /// an image attached to the current admitted user input turn; index counts
    /// only that turn's image blocks (0 = the first image). Generated or other
    /// blob-backed images, such as an image returned by generate_image, must
    /// use {"type":"image_ref","source":"blob","blob_id":"sha256:...","media_type":"image/png"};
    /// generated images must be sent with source=blob, never source=current_turn.
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
        /// Zero-based index into the current admitted turn's image blocks
        /// (the filtered image stream: index 0 is the turn's first image,
        /// skipping non-image blocks). Required for source=current_turn;
        /// forbidden for source=blob. Wire shape is a bare integer.
        #[serde(default)]
        #[schemars(with = "Option<usize>")]
        index: Option<CurrentTurnImageRef>,
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
    /// Optional multimodal blocks (text and images) sent alongside the body.
    /// {"type":"image_ref","source":"current_turn","index":0} refers only to
    /// an image attached to the current admitted user input turn; index counts
    /// only that turn's image blocks (0 = the first image). Generated or other
    /// blob-backed images, such as an image returned by generate_image, must
    /// use {"type":"image_ref","source":"blob","blob_id":"sha256:...","media_type":"image/png"};
    /// generated images must be sent with source=blob, never source=current_turn.
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
    pub trusted_peers: TrustedPeersView,
    pub runtime: Option<RuntimeCommsCommandHandle>,
}

/// Runtime-bound authority for semantic comms command execution.
#[derive(Clone)]
pub struct RuntimeCommsCommandHandle {
    runtime: Arc<dyn CoreCommsRuntime>,
    post_commit_hooks: Option<Arc<meerkat_core::PostCommitHookDispatcher>>,
}

impl RuntimeCommsCommandHandle {
    pub fn new(runtime: Arc<dyn CoreCommsRuntime>) -> Self {
        Self {
            runtime,
            post_commit_hooks: None,
        }
    }

    pub fn with_post_commit_hooks(
        mut self,
        post_commit_hooks: Arc<meerkat_core::PostCommitHookDispatcher>,
    ) -> Self {
        self.post_commit_hooks = Some(post_commit_hooks);
        self
    }

    pub async fn send(&self, command: CommsCommand) -> Result<SendReceipt, SendError> {
        let observation_command = command.clone();
        let receipt = self.runtime.send(command).await?;
        if let Some(dispatcher) = &self.post_commit_hooks
            && let Some(observation) = meerkat_core::HookObservation::from_committed_peer_send(
                &observation_command,
                &receipt,
            )
        {
            dispatcher.dispatch(observation);
        }
        Ok(receipt)
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
            "description": format!("{}{}", "Send a fire-and-forget message to a peer. No response is expected.\n\nWhen to use: Use send_message for one-way collaboration - status updates, notifications, sharing results, or any case where you do not need the peer to reply with structured data. If you need a correlated reply, use send_request instead.\n\nhandling_mode:\n- \"queue\": The message is delivered at the peer's next turn boundary. Use for ordinary collaboration where you do not want to preempt the peer's current task.\n- \"steer\": The peer processes your message immediately, interrupting its current work. Use only for urgent or time-sensitive collaboration.", "\n\nExamples:\n1. Fire-and-forget collaboration:\n   {\"peer_id\": \"<peer-id-from-peers>\", \"body\": \"FYI: the database migration completed successfully.\", \"handling_mode\": \"queue\"}\n2. Send a generated image without expecting a reply:\n   {\"peer_id\": \"<peer-id-from-peers>\", \"body\": \"Here is the generated mockup.\", \"blocks\": [{\"type\":\"image_ref\",\"source\":\"blob\",\"blob_id\":\"sha256:generated-image\",\"media_type\":\"image/png\"}], \"handling_mode\": \"queue\"}\n3. Urgent preemption:\n   {\"peer_id\": \"<peer-id-from-peers>\", \"display_name\": \"reporter\", \"body\": \"Stop before editing section 3; the requirement changed.\", \"handling_mode\": \"steer\"}\n\nFailure handling:\n- peer_not_found_or_not_trusted: The peer_id does not match a trusted peer. Call peers first to pick a peer_id.\n- peer_unreachable: The peer exists but is offline or the transport failed. Retry after a delay or inform the user."),
            "inputSchema": schema_for::<SendMessageInput>()
        }),
        json!({
            "name": "reply_to_peer",
            "description": format!("{}{}", "Reply to a peer message delivered in the current turn: the one that started it, or one that joined it while it ran. The reply is pre-addressed: the runtime already knows which peer sent each delivered message, so you do not supply a peer_id.\n\nWhen to use: Use reply_to_peer to answer a peer message you received this turn. Use send_message with an explicit peer_id for unsolicited outreach, and use send_response only for typed request/response contracts (send_request traffic).\n\nreply_to (optional): Only needed when more than one peer message arrived in this turn. Each peer message notice shows its Delivery ID. On an ambiguous call the error lists the available delivery ids with their senders; retry with reply_to set to one of them.\n\nhandling_mode:\n- \"queue\": The reply is delivered at the peer's next turn boundary. Use for ordinary collaboration.\n- \"steer\": The peer processes your reply immediately, interrupting its current work. Use only for urgent or time-sensitive collaboration.", "\n\nExamples:\n1. Reply to the turn's peer message:\n   {\"body\": \"Done - the database migration completed successfully.\", \"handling_mode\": \"queue\"}\n2. Disambiguate between multiple deliveries in one turn:\n   {\"reply_to\": \"<delivery-id-from-ambiguous_reply-error>\", \"body\": \"Acknowledged.\", \"handling_mode\": \"queue\"}\n\nFailure handling:\n- no_reply_capability: No peer message was delivered in this turn; there is nothing to reply to. Use peers + send_message instead.\n- ambiguous_reply: Multiple peer messages arrived this turn, or the reply_to id belongs to messages from more than one peer. Nothing was sent. Retry with reply_to set to one of the listed delivery ids, or use send_message to answer a specific peer.\n- unknown_reply_to: The reply_to value does not match any of this turn's deliveries. Use one of the listed delivery ids.\n- peer_not_found_or_not_trusted / peer_unreachable: Same as send_message - the sending peer is no longer trusted or reachable."),
            "inputSchema": schema_for::<ReplyToPeerInput>()
        }),
        json!({
            "name": "send_request",
            "description": format!("{}{}{}", "Send a typed structured request to a peer and expect a correlated response. The peer will reply using send_response with the same request ID.\n\nWhen to use: Use send_request for typed comms request contracts such as checksum_token or supervisor.bridge. The response will arrive as an incoming message with the original request ID in its in_reply_to field, so you can match it. If you just need to share information without expecting a reply, use send_message instead.\n\nhandling_mode:\n- \"queue\": The request is delivered at the peer's next turn boundary. Use when the peer can handle it after finishing its current task.\n- \"steer\": The peer processes your request immediately, interrupting its current work. Use only for requests that block your own progress.", SEND_REQUEST_CONTRACTS_DESCRIPTION, "\n\nExamples:\n1. checksum_token image review request:\n   {\"peer_id\":\"<peer-id-from-peers>\",\"display_name\":\"reviewer\",\"intent\":\"checksum_token\",\"params\":{\"subject\":\"image_receipt_check\"},\"blocks\":[{\"type\":\"text\",\"text\":\"Please inspect this generated image and return a receipt token with an image receipt.\"},{\"type\":\"image_ref\",\"source\":\"blob\",\"blob_id\":\"sha256:generated-image\",\"media_type\":\"image/png\"}],\"handling_mode\":\"queue\"}\n2. supervisor bridge request/reply:\n  {\"peer_id\": \"<peer-id-from-peers>\", \"display_name\": \"member\", \"intent\": \"supervisor.bridge\", \"params\": {\"command\": \"observe_member\", \"supervisor\": {\"name\": \"supervisor\", \"peer_id\": \"<supervisor-peer-id>\", \"address\": \"inproc://supervisor\", \"pubkey\": [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]}, \"epoch\": 1, \"protocol_version\": 2}, \"handling_mode\": \"queue\"}\n  The peer receives this and sends back with your peer_id:\n  {\"peer_id\": \"<your-peer-id>\", \"in_reply_to\": \"<request-id>\", \"status\": \"completed\", \"result\": {\"result\": \"ack\", \"ok\": true}}\n\nFailure handling:\n- peer_not_found_or_not_trusted: The peer_id does not match a trusted peer. Call peers first to pick a peer_id.\n- peer_unreachable: The peer exists but is offline or the transport failed. Retry after a delay or inform the user.\n- Missing response: There is no built-in timeout. If the peer does not respond, it may have failed or dropped the request. Re-send or check with the peer via send_message."),
            "inputSchema": schema_for::<SendRequestInput>()
        }),
        json!({
            "name": "send_response",
            "description": format!("{}{}{}", "Send a typed response to a previous peer request. The in_reply_to field must match the request ID from the original send_request message you received, and peer_id must be the peer that sent that request.\n\nWhen to use: Use send_response after receiving a typed comms request from a peer. The requester is waiting for a correlated reply.\n\nstatus values:\n- \"accepted\": Acknowledge receipt; you will send a \"completed\" or \"failed\" response later. Do not include handling_mode on accepted progress responses.\n- \"completed\": The request succeeded. Include a typed result when the request contract requires one.\n- \"failed\": The request could not be fulfilled. Include a typed rejection result when the request contract requires one.\n\nhandling_mode (optional): Override how the requester processes this terminal response. Defaults to the original request's mode. Use \"steer\" only for urgent preemption, or \"queue\" to deliver at the requester's next turn boundary.", SEND_RESPONSE_CONTRACTS_DESCRIPTION, "\n\nExamples:\n1. Completed checksum_token response with a generated image receipt:\n   {\"peer_id\":\"<peer-id-from-peers>\",\"display_name\":\"requester\",\"in_reply_to\":\"<request-id>\",\"status\":\"completed\",\"result\":{\"request_intent\":\"checksum_token\",\"request_subject\":\"image_receipt_check\",\"token\":\"generated-image-response-ok\"},\"blocks\":[{\"type\":\"image_ref\",\"source\":\"blob\",\"blob_id\":\"sha256:receipt-image\",\"media_type\":\"image/png\"}],\"handling_mode\":\"queue\"}\n2. Acceptance then later completion:\n   {\"peer_id\": \"<peer-id-from-peers>\", \"in_reply_to\": \"<request-id>\", \"status\": \"accepted\"}\n   ...later...\n   {\"peer_id\": \"<peer-id-from-peers>\", \"in_reply_to\": \"<request-id>\", \"status\": \"completed\", \"result\": {\"result\": \"ack\", \"ok\": true}}\n3. Failure response:\n   {\"peer_id\": \"<peer-id-from-peers>\", \"in_reply_to\": \"<request-id>\", \"status\": \"failed\", \"result\": {\"result\": \"rejected\", \"cause\": \"unsupported\", \"reason\": \"unsupported command\"}}\n\nFailure handling:\n- peer_not_found_or_not_trusted / peer_unreachable: Same as send_message. The requester will not receive your response - they may re-send the request.\n- Invalid in_reply_to: If the ID is not a valid UUID or does not match a known request, the call fails with a validation error.\n- invalid_arguments (not the requester): The peer_id is not the peer that sent the request. Nothing was sent and the request is still open: send the response to the requester's peer_id."),
            "inputSchema": schema_for::<SendResponseInput>()
        }),
        json!({
            "name": "peers",
            "description": "List all visible peers with connection info and optional metadata (description, labels, capabilities).\n\nAlways call peers before sending any message to pick the canonical peer_id. Names are display labels and may not be unique. The returned list includes:\n- peer_id: Unique cryptographic identity to use in send_message / send_request / send_response.\n- name: Display label only.\n- address: Transport address.\n- capabilities / meta: What the peer can do and its role description.\n\nExample output:\n{\"peers\": [{\"name\": \"helper-1\", \"peer_id\": \"018f...\", \"address\": \"tcp://...\", \"meta\": {\"description\": \"Code review helper\"}}]}",
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
    handle_tools_call_typed(ctx, name, args, dispatch_context)
        .await
        .map_err(CommsToolCallError::into_message)
}

/// Why a comms tool call was refused, by the tool error class a dispatcher
/// reports for it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum CommsToolCallError {
    /// The arguments name a delivery the runtime refuses for this session:
    /// a `send_response` addressed to a peer that did not send the request
    /// (`invalid_arguments`).
    #[error("{0}")]
    InvalidArguments(String),
    /// Any other refusal or failure (`execution_failed`).
    #[error("{0}")]
    Failed(String),
}

impl CommsToolCallError {
    /// The refusal as the dispatcher of `tool_name` reports it.
    pub fn into_tool_error(self, tool_name: &str) -> ToolError {
        match self {
            Self::InvalidArguments(reason) => ToolError::invalid_arguments(tool_name, reason),
            Self::Failed(message) => ToolError::ExecutionFailed { message },
        }
    }

    /// The refusal's message, without its class.
    pub fn into_message(self) -> String {
        match self {
            Self::InvalidArguments(message) | Self::Failed(message) => message,
        }
    }
}

impl From<String> for CommsToolCallError {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

/// Handle a comms tool call with dispatch-time turn context; a refusal keeps
/// its tool error class ([`CommsToolCallError`]). The comms tool dispatchers
/// report through this.
pub async fn handle_tools_call_typed(
    ctx: &ToolContext,
    name: &str,
    args: &Value,
    dispatch_context: &ToolDispatchContext,
) -> Result<Value, CommsToolCallError> {
    if comms_tool_unavailable_reason(ctx, name).is_some() {
        return Err(runtime_command_authority_unavailable_message().into());
    }

    match name {
        "reply_to_peer" => {
            let input: ReplyToPeerInput = serde_json::from_value(args.clone())
                .map_err(|e| format!("Invalid arguments: {e}"))?;
            let reply_context = peer_reply_dispatch_context(dispatch_context)?;
            let capability =
                select_reply_capability(&reply_context.deliveries, input.reply_to.as_deref())?;
            // Trust is re-validated at dispatch time: a capability minted at
            // turn admission does not outlive a trust revocation.
            let to = peer_route(ctx, capability.peer_id, capability.display_name.as_deref())?;
            let blocks = resolve_message_blocks(&input.body, input.blocks, dispatch_context)?;
            // Single content authority: mirrors send_message — when blocks are
            // present they own the content and `body` is projected from their
            // text blocks so the two cannot diverge.
            let body = match &blocks {
                Some(blocks) => project_body_from_blocks(blocks),
                None => input.body,
            };
            let command = CommsCommand::PeerMessage {
                to,
                body,
                blocks,
                // Agent tools carry no per-send taint override: the outbound
                // declaration is inherited from the runtime-level host config.
                content_taint: None,
                handling_mode: input.handling_mode,
                objective_id: None,
                // The reply names the delivery it answers, signed in the
                // envelope, so the receiver can link it.
                in_reply_to: Some(capability.in_reply_to),
            }
            .with_objective_id(objective_id_from_dispatch_context(dispatch_context)?);
            dispatch(ctx, command).await
        }
        "send_message" => {
            let input: SendMessageInput = serde_json::from_value(args.clone())
                .map_err(|e| format!("Invalid arguments: {e}"))?;
            let to = peer_route(ctx, input.peer_id, input.display_name.as_deref())?;
            let blocks = resolve_message_blocks(&input.body, input.blocks, dispatch_context)?;
            // Single content authority: when blocks are present they own the
            // content, and `body` is derived as a projection of their text
            // blocks so the two cannot diverge. With no blocks, `body` is the
            // sole content owner and stands alone.
            let body = match &blocks {
                Some(blocks) => project_body_from_blocks(blocks),
                None => input.body,
            };
            let command = CommsCommand::PeerMessage {
                to,
                body,
                blocks,
                // Agent tools carry no per-send taint override: the outbound
                // declaration is inherited from the runtime-level host config.
                content_taint: None,
                handling_mode: input.handling_mode,
                objective_id: None,
                // A fresh message answers no delivery.
                in_reply_to: None,
            }
            .with_objective_id(objective_id_from_dispatch_context(dispatch_context)?);
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
                // Agent tools carry no per-send taint override: the outbound
                // declaration is inherited from the runtime-level host config.
                content_taint: None,
                handling_mode: Some(input.handling_mode),
                stream: Some(InputStreamMode::ReserveInteraction),
            };
            let command = project_peer_request_command(typed_request, to)?
                .with_objective_id(objective_id_from_dispatch_context(dispatch_context)?);
            dispatch(ctx, command).await
        }
        "send_response" => {
            let input: SendResponseInput = serde_json::from_value(args.clone())
                .map_err(|e| format!("Invalid arguments: {e}"))?;
            let to = peer_route(ctx, input.peer_id, input.display_name.as_deref())?;
            let in_reply_to_uuid = uuid::Uuid::parse_str(&input.in_reply_to)
                .map_err(|_| format!("invalid UUID for in_reply_to: {}", input.in_reply_to))?;
            let blocks = resolve_tool_blocks(input.blocks, dispatch_context)?;
            let typed_request = meerkat_contracts::CommsCommandRequest::PeerResponse {
                to: input.peer_id,
                in_reply_to: InteractionId(in_reply_to_uuid),
                status: input.status,
                result: input.result,
                blocks,
                // Agent tools carry no per-send taint override: the outbound
                // declaration is inherited from the runtime-level host config.
                content_taint: None,
                handling_mode: input.handling_mode,
            };
            let command = project_peer_response_command(typed_request, to)?
                .with_objective_id(objective_id_from_dispatch_context(dispatch_context)?);
            dispatch(ctx, command).await
        }
        "peers" => {
            let _input: PeersInput = serde_json::from_value(args.clone())
                .map_err(|e| format!("Invalid arguments: {e}"))?;
            Ok(handle_peers(ctx).await?)
        }
        _ => Err(format!("Unknown tool: {name}").into()),
    }
}

/// The typed operation target of one comms tool call, derived from the same
/// typed input and turn context its handler routes by: the one peer a
/// delivery reaches (the `peer_id` of `send_message`, `send_request` and
/// `send_response`; for `reply_to_peer`, the sender of the turn's peer
/// message the reply selects), or the session's trusted-peer directory for
/// `peers`. It is a fact for an operation policy owner, never a permission:
/// the handler still checks trust, runtime command authority and delivery.
/// The `send_response` addressee is the caller's own `peer_id`; at send the
/// runtime delivers it only when that peer is the recorded requester of a
/// request this session received and can still answer.
///
/// # Errors
/// The reason the call names no single addressee: arguments that do not
/// parse as the tool's input, or a reply that selects no single delivery of
/// this turn. These are the refusals the handler itself returns.
pub fn comms_tool_operation_target(
    name: &str,
    args: &Value,
    dispatch_context: &ToolDispatchContext,
) -> Result<ToolOperationTarget, String> {
    fn parse_input<T: serde::de::DeserializeOwned>(args: &Value) -> Result<T, String> {
        serde_json::from_value(args.clone()).map_err(|error| error.to_string())
    }
    let delivery = |peer, kind| ToolOperationTarget::PeerSend { peer, kind };
    match name {
        "send_message" => {
            let input: SendMessageInput = parse_input(args)?;
            Ok(delivery(input.peer_id, PeerSendKind::Message))
        }
        "reply_to_peer" => {
            let input: ReplyToPeerInput = parse_input(args)?;
            let reply_context =
                peer_reply_dispatch_context(dispatch_context).map_err(|error| error.to_string())?;
            let capability =
                select_reply_capability(&reply_context.deliveries, input.reply_to.as_deref())
                    .map_err(|error| error.to_string())?;
            Ok(delivery(capability.peer_id, PeerSendKind::Message))
        }
        "send_request" => {
            let input: SendRequestInput = parse_input(args)?;
            Ok(delivery(input.peer_id, PeerSendKind::Request))
        }
        "send_response" => {
            let input: SendResponseInput = parse_input(args)?;
            Ok(delivery(input.peer_id, PeerSendKind::Response))
        }
        "peers" => Ok(ToolOperationTarget::PeerDirectoryRead),
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
    // Single content authority: when the caller supplies their own text
    // block(s), those blocks own the message content and the convenience
    // `body` field is ignored — the carried body is later projected from the
    // blocks (`project_body_from_blocks`) so the two cannot diverge. The
    // `body` is only lifted into a leading text block when the caller supplied
    // no text of their own (e.g. body + image blocks), so its text is not
    // silently dropped.
    let has_text_block = blocks
        .iter()
        .any(|block| matches!(block, ContentBlock::Text { .. }));
    if !body.is_empty() && !has_text_block {
        blocks.insert(
            0,
            ContentBlock::Text {
                text: body.to_string(),
            },
        );
    }
    Ok(Some(blocks))
}

/// Project a message body from the text content of resolved blocks.
///
/// When blocks are the single content authority, the body is a deterministic
/// function of their text blocks: the text-block contents joined with newlines.
/// Non-text blocks (images) contribute no body text. This guarantees that the
/// carried `body` cannot diverge from the block content it is meant to mirror.
fn project_body_from_blocks(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn objective_id_from_dispatch_context(
    dispatch_context: &ToolDispatchContext,
) -> Result<Option<meerkat_core::interaction::ObjectiveId>, String> {
    dispatch_context
        .turn_metadata(meerkat_core::agent::TOOL_DISPATCH_OBJECTIVE_ID_KEY)
        .and_then(serde_json::Value::as_str)
        .map(uuid::Uuid::parse_str)
        .transpose()
        .map_err(|error| format!("invalid objective correlation in dispatch context: {error}"))
        .map(|objective_id| objective_id.map(meerkat_core::interaction::ObjectiveId))
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
    index: Option<CurrentTurnImageRef>,
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
        content_taint,
        handling_mode,
        stream,
        ..
    } = core_request
    else {
        return Err("Invalid arguments: expected peer_request comms command".to_string());
    };
    Ok(CommsCommand::PeerRequest {
        to,
        // #101: downgrade the typed closed public-request intent to the domain
        // envelope's wider open `String` intent vocabulary at this boundary.
        intent: intent.as_str().to_string(),
        params,
        blocks,
        content_taint,
        handling_mode: handling_mode.unwrap_or_default(),
        stream: stream.unwrap_or(InputStreamMode::None),
        objective_id: None,
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
        content_taint,
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
        content_taint,
        handling_mode,
        objective_id: None,
    })
}

/// Why `reply_to_peer` cannot pick the delivery it answers. Each message
/// starts with the stable code the tool contract documents, and nothing is
/// sent for any of them.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReplySelectionError {
    /// The turn carries no peer message delivery to answer.
    NoReplyCapability,
    /// The turn's runtime-minted reply context does not decode.
    InvalidReplyContext(meerkat_core::comms::PeerReplyContextError),
    /// `reply_to` names no delivery of this turn.
    UnknownReplyTo {
        selector: String,
        available: Vec<PeerReplyCapability>,
    },
    /// The selection does not name exactly one sender's delivery: either
    /// `reply_to` matches deliveries from more than one sender (each sender
    /// chooses its own ids, so two can share one), or no `reply_to` was
    /// given and the turn carries more than one delivery.
    AmbiguousReply {
        reply_to: Option<String>,
        candidates: Vec<PeerReplyCapability>,
    },
}

/// One delivery as an error names it: its id and its sender.
struct DeliveryLabel<'a>(&'a PeerReplyCapability);

impl std::fmt::Display for DeliveryLabel<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let capability = self.0;
        write!(formatter, "{} from ", capability.in_reply_to)?;
        match capability.display_name.as_deref() {
            Some(name) => write!(formatter, "{name} ({})", capability.peer_id),
            None => write!(formatter, "{}", capability.peer_id),
        }
    }
}

/// A list of deliveries as an error names them.
struct DeliveryLabels<'a>(&'a [PeerReplyCapability]);

impl std::fmt::Display for DeliveryLabels<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (index, capability) in self.0.iter().enumerate() {
            if index > 0 {
                formatter.write_str(", ")?;
            }
            write!(formatter, "{}", DeliveryLabel(capability))?;
        }
        Ok(())
    }
}

impl std::fmt::Display for ReplySelectionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoReplyCapability => formatter.write_str(
                "no_reply_capability: this turn carries no peer message delivery; \
                 use peers + send_message instead",
            ),
            Self::InvalidReplyContext(error) => write!(formatter, "invalid_reply_context: {error}"),
            Self::UnknownReplyTo {
                selector,
                available,
            } => write!(
                formatter,
                "unknown_reply_to: no peer delivery in this turn matches '{selector}'; \
                 available delivery ids: [{}]",
                DeliveryLabels(available)
            ),
            Self::AmbiguousReply {
                reply_to: Some(selector),
                candidates,
            } => write!(
                formatter,
                "ambiguous_reply: delivery id {selector} arrived from more than one peer \
                 this turn: [{}]; nothing was sent. reply_to cannot tell them apart; \
                 use peers + send_message to answer one of them",
                DeliveryLabels(candidates)
            ),
            Self::AmbiguousReply {
                reply_to: None,
                candidates,
            } => write!(
                formatter,
                "ambiguous_reply: multiple peer messages arrived this turn; \
                 set reply_to to one of the delivery ids: [{}]",
                DeliveryLabels(candidates)
            ),
        }
    }
}

impl std::error::Error for ReplySelectionError {}

impl From<ReplySelectionError> for CommsToolCallError {
    fn from(error: ReplySelectionError) -> Self {
        Self::Failed(error.to_string())
    }
}

/// Every peer delivery this turn can answer: the turn-start deliveries the
/// runtime minted and the ones that joined the running turn at a model
/// boundary ([`ToolDispatchContext::peer_reply_deliveries`]). A malformed
/// turn-start value fails closed with its own code instead of being coerced
/// or ignored.
fn peer_reply_dispatch_context(
    dispatch_context: &ToolDispatchContext,
) -> Result<&PeerReplyDispatchContext, ReplySelectionError> {
    let context = dispatch_context
        .peer_reply_deliveries()
        .map_err(ReplySelectionError::InvalidReplyContext)?;
    if context.deliveries.is_empty() {
        return Err(ReplySelectionError::NoReplyCapability);
    }
    Ok(context)
}

/// Select the delivery to reply to. One delivery with no selector resolves to
/// it; multiple deliveries require an explicit `reply_to`; a selector must
/// match the deliveries of exactly one sender. Deliveries are keyed by
/// (sender, delivery id), and the sender chooses the id, so a selector that
/// matches more than one sender is ambiguous: it never takes the first match.
fn select_reply_capability<'a>(
    deliveries: &'a [PeerReplyCapability],
    reply_to: Option<&str>,
) -> Result<&'a PeerReplyCapability, ReplySelectionError> {
    match reply_to {
        Some(selector) => {
            let candidates = deliveries
                .iter()
                .filter(|capability| capability.in_reply_to.to_string() == selector)
                .collect::<Vec<_>>();
            match candidates.as_slice() {
                [] => Err(ReplySelectionError::UnknownReplyTo {
                    selector: selector.to_string(),
                    available: deliveries.to_vec(),
                }),
                [first, rest @ ..]
                    if rest
                        .iter()
                        .all(|capability| capability.peer_id == first.peer_id) =>
                {
                    Ok(*first)
                }
                _ => Err(ReplySelectionError::AmbiguousReply {
                    reply_to: Some(selector.to_string()),
                    candidates: candidates.into_iter().cloned().collect(),
                }),
            }
        }
        None => match deliveries {
            [single] => Ok(single),
            [] => Err(ReplySelectionError::NoReplyCapability),
            _ => Err(ReplySelectionError::AmbiguousReply {
                reply_to: None,
                candidates: deliveries.to_vec(),
            }),
        },
    }
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

async fn dispatch(ctx: &ToolContext, command: CommsCommand) -> Result<Value, CommsToolCallError> {
    // Capture peer display label for error normalization before consuming the command.
    let peer_for_errors = match &command {
        CommsCommand::PeerMessage { to, .. }
        | CommsCommand::IncarnationFencedPeerMessage { to, .. }
        | CommsCommand::PeerLifecycle { to, .. }
        | CommsCommand::PeerRequest { to, .. }
        | CommsCommand::PeerResponse { to, .. } => Some(to.label()),
        CommsCommand::Input { .. } => None,
    };
    let cmd_kind = command.command_kind().to_string();

    if let Some(runtime) = &ctx.runtime {
        let receipt = runtime.send(command).await.map_err(|error| match error {
            // The runtime refused a response addressed to a peer that did not
            // send the request: the call's `peer_id` is the invalid argument.
            error @ SendError::ResponseAddresseeNotRequester { .. } => {
                CommsToolCallError::InvalidArguments(error.to_string())
            }
            SendError::PeerNotFound(p) => {
                format!("peer_not_found_or_not_trusted: peer '{p}' is not found or not trusted")
                    .into()
            }
            SendError::PeerOffline => format!(
                "peer_unreachable: peer '{}' is unreachable: offline_or_no_ack",
                peer_for_errors.as_deref().unwrap_or("<unknown>")
            )
            .into(),
            SendError::Transport(inner) => format!(
                "peer_unreachable: peer '{}' is unreachable: transport_error ({inner})",
                peer_for_errors.as_deref().unwrap_or("<unknown>")
            )
            .into(),
            SendError::AdmissionDropped { reason } => format!(
                "peer_admission_dropped: peer '{}' rejected envelope at ingress: {}",
                peer_for_errors.as_deref().unwrap_or("<unknown>"),
                reason.as_code()
            )
            .into(),
            SendError::DurableAdmissionRejected { envelope_id } => format!(
                "peer_input_rejected: peer '{}' durably rejected envelope {envelope_id}",
                peer_for_errors.as_deref().unwrap_or("<unknown>")
            )
            .into(),
            SendError::PeerWakeRefused {
                envelope_id,
                refusal,
                ..
            } => peer_wake_refused_message(
                peer_for_errors.as_deref().unwrap_or("<unknown>"),
                envelope_id,
                &refusal,
            )
            .into(),
            other => other.to_string().into(),
        })?;
        return Ok(sent_result(&cmd_kind, receipt));
    }

    Ok(dispatch_without_runtime(ctx, command, peer_for_errors, cmd_kind).await?)
}

/// Transport-only dispatch for a tool context without runtime command
/// authority.
async fn dispatch_without_runtime(
    ctx: &ToolContext,
    command: CommsCommand,
    peer_for_errors: Option<String>,
    cmd_kind: String,
) -> Result<Value, String> {
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
        | CommsCommand::IncarnationFencedPeerMessage { to, .. }
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
            content_taint,
            handling_mode,
            objective_id,
            in_reply_to,
            ..
        } => {
            reject_runtime_less_blob_blocks(blocks.as_deref())?;
            // The per-send taint override rides to the router's stamping
            // choke point even on the runtime-less fallback path.
            let outcome = ctx
                .router
                .send_with_id(
                    dest_peer_id,
                    uuid::Uuid::new_v4(),
                    crate::types::MessageKind::Message {
                        body,
                        blocks,
                        content_taint: None,
                        handling_mode: Some(handling_mode),
                        objective_id,
                        in_reply_to: in_reply_to.map(|delivery| delivery.0),
                    },
                    content_taint,
                )
                .await
                .map_err(|e| format_router_send_error(&dest_display, e))?;
            Ok(sent_result(
                &cmd_kind,
                SendReceipt::PeerMessageSent {
                    envelope_id: outcome.envelope_id,
                    delivery: outcome.delivery,
                },
            ))
        }
        CommsCommand::IncarnationFencedPeerMessage {
            body,
            blocks,
            content_taint,
            handling_mode,
            objective_id,
            expected_recipient,
            ..
        } => {
            reject_runtime_less_blob_blocks(blocks.as_deref())?;
            let outcome = ctx
                .router
                .send_with_id(
                    dest_peer_id,
                    uuid::Uuid::new_v4(),
                    crate::types::MessageKind::IncarnationFencedMessage {
                        body,
                        blocks,
                        content_taint: None,
                        handling_mode: Some(handling_mode),
                        objective_id,
                        expected_recipient,
                    },
                    content_taint,
                )
                .await
                .map_err(|e| format_router_send_error(&dest_display, e))?;
            Ok(sent_result(
                &cmd_kind,
                SendReceipt::PeerMessageSent {
                    envelope_id: outcome.envelope_id,
                    delivery: outcome.delivery,
                },
            ))
        }
        CommsCommand::PeerLifecycle { kind, params, .. } => {
            let outcome = ctx
                .router
                .send(
                    dest_peer_id,
                    crate::types::MessageKind::Lifecycle { kind, params },
                )
                .await
                .map_err(|e| format_router_send_error(&dest_display, e))?;
            Ok(sent_result(
                &cmd_kind,
                SendReceipt::PeerLifecycleSent {
                    envelope_id: outcome.envelope_id,
                    delivery: outcome.delivery,
                },
            ))
        }
        CommsCommand::PeerRequest { .. } => Err(runtime_command_authority_unavailable_message()),
        CommsCommand::PeerResponse { .. } => Err(runtime_command_authority_unavailable_message()),
    }
}

fn reject_runtime_less_blob_blocks(blocks: Option<&[ContentBlock]>) -> Result<(), String> {
    let has_sender_local_blob = blocks.is_some_and(|blocks| {
        blocks.iter().any(|block| {
            matches!(
                block,
                ContentBlock::Image {
                    data: ImageData::Blob { .. },
                    ..
                }
            )
        })
    });
    if has_sender_local_blob {
        return Err(
            "tool_unavailable: runtime_less_blob_transport_unavailable: blob-backed comms blocks require a runtime blob store"
                .to_string(),
        );
    }
    Ok(())
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

/// Validate a canonical [`PeerId`] against the runtime-facing trust store.
fn ensure_peer_id_is_trusted(ctx: &ToolContext, peer_id: &PeerId) -> Result<(), String> {
    if ctx.trusted_peers.contains(peer_id) {
        Ok(())
    } else {
        Err(format!(
            "peer_not_found_or_not_trusted: peer '{peer_id}' is not found or not trusted",
        ))
    }
}

/// Tool text for a peer that is a seated member whose wake was refused. Not
/// delivered, and never `peer_not_found_or_not_trusted`: the peer is a member.
/// The text names whether a retry can succeed (nothing was handed off, so a
/// retry never duplicates the message).
fn peer_wake_refused_message(
    peer_name: &str,
    envelope_id: uuid::Uuid,
    refusal: &meerkat_core::comms::PeerWakeRefusal,
) -> String {
    let retry = if refusal.is_retryable() {
        "not delivered; retry later"
    } else {
        "not delivered; retrying will not help"
    };
    format!(
        "peer_wake_refused: peer '{peer_name}' was not woken for envelope {envelope_id} ({refusal}): {retry}"
    )
}

fn format_router_send_error(peer_name: &str, error: crate::router::SendError) -> String {
    match error {
        crate::router::SendError::PeerWakeRefused {
            envelope_id,
            refusal,
        } => peer_wake_refused_message(peer_name, envelope_id, &refusal),
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
        crate::router::SendError::DurableAdmissionRejected { envelope_id } => format!(
            "peer_input_rejected: peer '{peer_name}' durably rejected envelope {envelope_id}"
        ),
        // Neither `peer_unreachable` (the peer was present and took the item)
        // nor `peer_admission_dropped` (admission refused it): the envelope is
        // on the peer's queue and no drain confirmed it inside the bound.
        crate::router::SendError::InprocDeliveryUnconfirmed {
            envelope_id,
            park_bound,
        } => format!(
            "peer_delivery_unconfirmed: peer '{peer_name}' admitted envelope {envelope_id} but did not confirm it within {}s",
            park_bound.as_secs_f64()
        ),
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
    let sendable_kinds = peer_sendability_authorized_by_tools(ctx);
    // The trust store is PeerId-keyed and every entry is validated typed
    // material — duplicate identities, zero pubkeys, and invalid names or
    // addresses are structurally impossible here.
    ctx.trusted_peers
        .entries()
        .into_iter()
        .filter(|entry| entry.pubkey != self_pubkey)
        .map(|entry| PeerDirectoryEntry {
            name: entry.name,
            peer_id: entry.peer_id,
            address: entry.address,
            source: PeerDirectorySource::Trusted,
            sendable_kinds: sendable_kinds.clone(),
            capabilities: PeerCapabilitySet::default(),
            meta: entry.meta,
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
    use crate::{CommsRuntime, PubKey};
    use meerkat_contracts::wire::supervisor_bridge::{
        BridgeAck, BridgeCommand, BridgePeerSpec, BridgeReply, BridgeSupervisorPayload,
        supervisor_bridge_current_protocol_version,
    };
    use meerkat_core::comms::{
        COMMS_PEER_REPLY_DISPATCH_CONTEXT_KEY, CommsTrustMutation, PeerAddress, PeerCapabilitySet,
        PeerDirectorySource, PeerSendability, PeerTransport, TrustedPeerDescriptor,
    };
    use parking_lot::Mutex;
    use tokio::sync::Notify;

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

    #[test]
    fn router_admission_drop_error_keeps_typed_reason_code() {
        let message = format_router_send_error(
            "peer-a",
            crate::router::SendError::AdmissionDropped {
                reason: crate::inbox::DropReason::InboxFull,
            },
        );

        assert_eq!(
            message,
            "peer_admission_dropped: peer 'peer-a' rejected envelope at ingress: inbox_full"
        );
    }

    struct RecordingRuntime {
        sent: Mutex<Vec<CommsCommand>>,
        peers: Vec<PeerDirectoryEntry>,
        notify: Arc<Notify>,
    }

    struct RecordingPostCommitHook {
        sender: tokio::sync::mpsc::UnboundedSender<meerkat_core::HookInvocation>,
    }

    #[async_trait::async_trait]
    impl meerkat_core::HookEngine for RecordingPostCommitHook {
        fn mandatory_topology(
            &self,
            _overrides: Option<&meerkat_core::HookRunOverrides>,
        ) -> Result<meerkat_core::HookMandatoryTopology, meerkat_core::HookEngineError> {
            Ok(meerkat_core::HookMandatoryTopology::none())
        }

        async fn execute(
            &self,
            invocation: meerkat_core::HookInvocation,
            _overrides: Option<&meerkat_core::HookRunOverrides>,
        ) -> Result<meerkat_core::HookExecutionReport, meerkat_core::HookEngineError> {
            let _ = self.sender.send(invocation);
            Ok(meerkat_core::HookExecutionReport::empty())
        }
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
                        delivery: meerkat_core::comms::PeerDeliveryOutcome::VolatileHandedOff,
                    }
                }
                CommsCommand::PeerRequest { stream, .. } => {
                    meerkat_core::comms::SendReceipt::PeerRequestSent {
                        envelope_id: uuid::Uuid::from_u128(1),
                        interaction_id: InteractionId(uuid::Uuid::from_u128(2)),
                        stream_reserved: *stream == InputStreamMode::ReserveInteraction,
                        delivery: meerkat_core::comms::PeerDeliveryOutcome::VolatileHandedOff,
                    }
                }
                CommsCommand::PeerResponse { in_reply_to, .. } => {
                    meerkat_core::comms::SendReceipt::PeerResponseSent {
                        envelope_id: uuid::Uuid::from_u128(3),
                        in_reply_to: *in_reply_to,
                        delivery: meerkat_core::comms::PeerDeliveryOutcome::VolatileHandedOff,
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

        fn inbox_notify(&self) -> Arc<Notify> {
            Arc::clone(&self.notify)
        }

        async fn peers(&self) -> Vec<PeerDirectoryEntry> {
            self.peers.clone()
        }
    }

    #[cfg(feature = "workspace-tests")]
    #[allow(clippy::expect_used)]
    fn test_peer_projection_reconcile_obligation(
        session_id: impl Into<String>,
        local_peer_id: meerkat_core::comms::PeerId,
        endpoints: std::collections::BTreeSet<meerkat_runtime::meerkat_machine::dsl::PeerEndpoint>,
    ) -> (
        Arc<meerkat_runtime::HandleDslAuthority>,
        meerkat_runtime::protocol_comms_trust_reconcile::CommsTrustReconcileObligation,
    ) {
        let session_id = session_id.into();
        let dsl = Arc::new(meerkat_runtime::HandleDslAuthority::ephemeral());
        dsl.apply_signal(
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineSignal::Initialize,
            "test::initialize",
        )
        .expect("Initialize signal");
        dsl.apply_input(
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::RegisterSession {
                session_id: meerkat_runtime::meerkat_machine::dsl::SessionId::from(
                    session_id.clone(),
                ),
                runtime_epoch_id: None,
                initial_run_start_holds: std::collections::BTreeSet::new(),
            },
            "test::register_session",
        )
        .expect("RegisterSession input");
        dsl.apply_input(
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::PrepareBindings {
                agent_runtime_id: meerkat_runtime::meerkat_machine::dsl::AgentRuntimeId::from(
                    format!("test-runtime-{session_id}"),
                ),
                fence_token: meerkat_runtime::meerkat_machine::dsl::FenceToken::from(0),
                generation: Some(meerkat_runtime::meerkat_machine::dsl::Generation::from(0)),
                runtime_epoch_id: None,
                session_id: meerkat_runtime::meerkat_machine::dsl::SessionId::from(session_id),
                residency_owner:
                    meerkat_runtime::meerkat_machine::dsl::SessionResidencyOwner::Session,
            },
            "test::prepare_bindings",
        )
        .expect("PrepareBindings input");
        dsl.apply_input(
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::PublishLocalEndpoint {
                endpoint: meerkat_runtime::meerkat_machine::dsl::PeerEndpoint::new(
                    "local",
                    local_peer_id.to_string(),
                    "inproc://local",
                    [0x7f; 32],
                ),
            },
            "test::publish_local_endpoint",
        )
        .expect("PublishLocalEndpoint input");
        let transition = dsl
            .apply_input_with_transition(
                meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::ApplyMobPeerOverlay {
                    epoch: 1,
                    endpoints,
                },
                "test::apply_mob_peer_overlay",
            )
            .expect("ApplyMobPeerOverlay input");
        let mut obligations =
            meerkat_runtime::protocol_comms_trust_reconcile::extract_obligations_with_freshness(
                &transition,
                dsl.peer_projection_freshness_authority(),
            );
        assert_eq!(
            obligations.len(),
            1,
            "test reconcile effect must produce one generated obligation"
        );
        (dsl, obligations.pop().expect("obligation count checked"))
    }

    #[cfg(feature = "workspace-tests")]
    async fn add_generated_peer_projection_trust(
        runtime: &CommsRuntime,
        peer: TrustedPeerDescriptor,
        context: &'static str,
    ) {
        add_generated_peer_projection_trust_batch(runtime, vec![peer], context).await;
    }

    #[cfg(feature = "workspace-tests")]
    async fn add_generated_peer_projection_trust_batch(
        runtime: &CommsRuntime,
        peers: Vec<TrustedPeerDescriptor>,
        context: &'static str,
    ) {
        let endpoints = peers
            .iter()
            .map(meerkat_runtime::meerkat_machine::dsl::PeerEndpoint::from)
            .collect::<std::collections::BTreeSet<_>>();
        let (dsl, obligation) = test_peer_projection_reconcile_obligation(
            "comms-mcp-tools-test-reconcile",
            runtime
                .peer_id()
                .unwrap_or_else(|| panic!("{context}: runtime peer_id unavailable")),
            endpoints,
        );
        meerkat_runtime::RuntimePeerCommsHandle::install_generated_on(dsl, runtime)
            .expect("install generated peer-comms handle");
        for peer in peers {
            let endpoint = meerkat_runtime::meerkat_machine::dsl::PeerEndpoint::from(&peer);
            CoreCommsRuntime::apply_trust_mutation(
                runtime,
                CommsTrustMutation::AddTrustedPeer {
                    authority:
                        meerkat_runtime::protocol_comms_trust_reconcile::authority_for_endpoint(
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
    }

    #[cfg(feature = "workspace-tests")]
    async fn make_trusted_runtime_less_context(
        peer_keypair: &Keypair,
    ) -> (ToolContext, meerkat_core::comms::PeerId) {
        let peer_id = peer_keypair.public_key().to_peer_id();
        let runtime = Arc::new(
            CommsRuntime::inproc_only(&format!(
                "mcp-tools-runtime-{}",
                uuid::Uuid::new_v4().simple()
            ))
            .expect("runtime"),
        );
        add_generated_peer_projection_trust(
            runtime.as_ref(),
            TrustedPeerDescriptor::unsigned_with_pubkey(
                "test-peer",
                peer_id.to_string(),
                *peer_keypair.public_key().as_bytes(),
                "inproc://test-peer",
            )
            .expect("trusted peer descriptor"),
            "install test peer trust",
        )
        .await;

        (
            ToolContext {
                router: runtime.router_arc(),
                trusted_peers: runtime.trusted_peers_shared(),
                runtime: None,
            },
            peer_id,
        )
    }

    #[cfg(feature = "workspace-tests")]
    async fn make_runtime_less_trust_only_context() -> (ToolContext, meerkat_core::comms::PeerId) {
        let peer_keypair = Keypair::generate();
        make_trusted_runtime_less_context(&peer_keypair).await
    }

    #[cfg(feature = "workspace-tests")]
    async fn make_runtime_less_inproc_delivery_pair() -> (
        ToolContext,
        meerkat_core::comms::PeerId,
        Arc<CommsRuntime>,
        Arc<CommsRuntime>,
        Arc<meerkat_runtime::TestPeerIngressRuntimeFinalizer>,
    ) {
        let nonce = uuid::Uuid::new_v4().simple();
        let sender_name = format!("mcp-tools-runtime-less-sender-{nonce}");
        let receiver_name = format!("mcp-tools-runtime-less-receiver-{nonce}");
        let sender = Arc::new(CommsRuntime::inproc_only(&sender_name).expect("sender runtime"));
        let receiver =
            Arc::new(CommsRuntime::inproc_only(&receiver_name).expect("receiver runtime"));
        add_generated_peer_projection_trust(
            sender.as_ref(),
            TrustedPeerDescriptor::unsigned_with_pubkey(
                &receiver_name,
                receiver.public_key().to_peer_id().to_string(),
                *receiver.public_key().as_bytes(),
                format!("inproc://{receiver_name}"),
            )
            .expect("receiver trusted peer descriptor"),
            "install receiver trust",
        )
        .await;
        add_generated_peer_projection_trust(
            receiver.as_ref(),
            TrustedPeerDescriptor::unsigned_with_pubkey(
                &sender_name,
                sender.public_key().to_peer_id().to_string(),
                *sender.public_key().as_bytes(),
                format!("inproc://{sender_name}"),
            )
            .expect("sender trusted peer descriptor"),
            "install sender trust",
        )
        .await;

        let (peer_comms_handle, finalizer) =
            meerkat_runtime::test_peer_comms_handle_and_runtime_finalizer();
        crate::runtime::comms_runtime::test_install_peer_comms_handle(
            receiver.as_ref(),
            peer_comms_handle,
        );

        let receiver_peer_id = receiver.public_key().to_peer_id();
        let context = ToolContext {
            router: sender.router_arc(),
            trusted_peers: sender.trusted_peers_shared(),
            runtime: None,
        };
        (
            context,
            receiver_peer_id,
            sender,
            receiver,
            Arc::new(finalizer),
        )
    }

    #[cfg(feature = "workspace-tests")]
    fn spawn_runtime_finalization(
        receiver: Arc<CommsRuntime>,
        finalizer: Arc<meerkat_runtime::TestPeerIngressRuntimeFinalizer>,
    ) -> tokio::task::JoinHandle<meerkat_core::interaction::PeerInputCandidate> {
        tokio::spawn(async move {
            let claim = loop {
                if let Some(claim) =
                    CoreCommsRuntime::claim_classified_inbox_interaction(receiver.as_ref())
                        .await
                        .expect("test runtime claim surface must be available")
                {
                    break claim;
                }
                tokio::task::yield_now().await;
            };
            finalizer
                .finalize(claim)
                .await
                .expect("real MeerkatMachine must durably finalize the exact tool-send claim")
        })
    }

    #[test]
    fn test_tools_list_has_five_tools() {
        let tools = tools_list();
        assert_eq!(tools.len(), 5);
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"send_message"));
        assert!(names.contains(&"reply_to_peer"));
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

        // Image reference rules are documented once per tool, on the `blocks`
        // argument, rather than repeated in the tool description.
        let blocks_description = |name: &str| -> String {
            tools
                .iter()
                .find(|tool| tool["name"].as_str() == Some(name))
                .and_then(|tool| {
                    tool["inputSchema"]["properties"]["blocks"]["description"].as_str()
                })
                .expect("blocks description")
                .to_string()
        };
        for name in [
            "send_message",
            "reply_to_peer",
            "send_request",
            "send_response",
        ] {
            let text = blocks_description(name);
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
            assert!(
                !description(name).contains("Multimodal blocks:"),
                "{name} must not repeat the blocks documentation in its description"
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

    /// The pre-addressed reply tool takes no peer identity: body and
    /// handling_mode are required, reply_to and blocks optional, and unknown
    /// fields (e.g. a smuggled peer_id) are rejected at the schema level.
    #[test]
    fn test_reply_to_peer_schema_is_pre_addressed() {
        let schema = schema_for::<ReplyToPeerInput>();
        let required = schema["required"].as_array().unwrap();
        let required_names: Vec<&str> = required.iter().map(|v| v.as_str().unwrap()).collect();
        assert!(
            required_names.contains(&"handling_mode"),
            "reply_to_peer must require handling_mode, got required: {required_names:?}"
        );
        assert!(required_names.contains(&"body"));
        assert!(!required_names.contains(&"reply_to"));
        assert!(!required_names.contains(&"blocks"));
        let properties = schema["properties"].as_object().unwrap();
        assert!(properties.contains_key("reply_to"));
        assert!(properties.contains_key("blocks"));
        assert!(
            !properties.contains_key("peer_id"),
            "reply_to_peer is pre-addressed; it must not expose a peer_id input"
        );
        assert_eq!(
            schema["additionalProperties"],
            Value::Bool(false),
            "deny_unknown_fields must project as additionalProperties: false"
        );
    }

    fn reply_capability(peer_id: PeerId, interaction: u128) -> PeerReplyCapability {
        PeerReplyCapability {
            in_reply_to: InteractionId(uuid::Uuid::from_u128(interaction)),
            peer_id,
            display_name: Some("sender-agent".to_string()),
            kind: meerkat_core::comms::PeerReplyDeliveryKind::Message,
        }
    }

    fn reply_dispatch_context(deliveries: Vec<PeerReplyCapability>) -> ToolDispatchContext {
        let context = PeerReplyDispatchContext { deliveries };
        ToolDispatchContext::default().with_turn_metadata(std::collections::BTreeMap::from([(
            COMMS_PEER_REPLY_DISPATCH_CONTEXT_KEY.to_string(),
            serde_json::to_value(&context).expect("reply context serializes"),
        )]))
    }

    /// Each comms tool's typed target comes from the same input and turn
    /// context its handler routes by: the explicit peer of a send, the
    /// sender of the turn's peer message for a reply, the trusted-peer
    /// directory for the roster read. A call naming no single addressee has
    /// no target, with the handler's own refusal as the reason.
    #[test]
    fn comms_tool_operation_targets_name_the_peer_the_handler_routes_to() {
        let peer = PeerId::new();
        let none = ToolDispatchContext::default();
        let send = |kind| ToolOperationTarget::PeerSend { peer, kind };
        assert_eq!(
            comms_tool_operation_target(
                "send_message",
                &json!({"peer_id": peer, "body": "hi", "handling_mode": "queue"}),
                &none
            ),
            Ok(send(PeerSendKind::Message))
        );
        assert_eq!(
            comms_tool_operation_target(
                "send_request",
                &json!({
                    "peer_id": peer,
                    "intent": "checksum_token",
                    "params": {"subject": "check"},
                    "handling_mode": "queue"
                }),
                &none
            ),
            Ok(send(PeerSendKind::Request))
        );
        assert_eq!(
            comms_tool_operation_target(
                "send_response",
                &json!({
                    "peer_id": peer,
                    "in_reply_to": uuid::Uuid::from_u128(5).to_string(),
                    "status": "accepted"
                }),
                &none
            ),
            Ok(send(PeerSendKind::Response))
        );
        assert_eq!(
            comms_tool_operation_target("peers", &json!({}), &none),
            Ok(ToolOperationTarget::PeerDirectoryRead)
        );

        let sender = PeerId::new();
        let reply = json!({"body": "done", "handling_mode": "queue"});
        assert_eq!(
            comms_tool_operation_target(
                "reply_to_peer",
                &reply,
                &reply_dispatch_context(vec![reply_capability(sender, 7)])
            ),
            Ok(ToolOperationTarget::PeerSend {
                peer: sender,
                kind: PeerSendKind::Message,
            })
        );
        let two =
            reply_dispatch_context(vec![reply_capability(sender, 7), reply_capability(peer, 8)]);
        assert_eq!(
            comms_tool_operation_target(
                "reply_to_peer",
                &json!({
                    "reply_to": uuid::Uuid::from_u128(8).to_string(),
                    "body": "done",
                    "handling_mode": "queue"
                }),
                &two
            ),
            Ok(send(PeerSendKind::Message)),
            "an explicit selector reaches the sender of that delivery"
        );

        for (name, args, context, refusal) in [
            (
                "reply_to_peer",
                reply.clone(),
                none.clone(),
                "no_reply_capability",
            ),
            ("reply_to_peer", reply, two, "ambiguous_reply"),
        ] {
            let error = comms_tool_operation_target(name, &args, &context)
                .expect_err("a reply without one selected delivery has no addressee");
            assert!(error.starts_with(refusal), "{error}");
        }
        assert!(
            comms_tool_operation_target(
                "send_message",
                &json!({"body": "hi", "handling_mode": "queue"}),
                &none
            )
            .is_err(),
            "a send without a peer has no addressee"
        );
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn reply_to_peer_dispatches_peer_message_to_capability_peer() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair).await;
        let runtime = Arc::new(RecordingRuntime::new());
        ctx.runtime = Some(RuntimeCommsCommandHandle::new(runtime.clone()));
        let dispatch_context = reply_dispatch_context(vec![reply_capability(peer_id, 7)]);

        handle_tools_call_with_context(
            &ctx,
            "reply_to_peer",
            &json!({"body": "on it", "handling_mode": "queue"}),
            &dispatch_context,
        )
        .await
        .expect("single-delivery reply must dispatch without a selector");

        let sent = runtime.sent.lock();
        let [
            CommsCommand::PeerMessage {
                to,
                body,
                blocks,
                handling_mode,
                ..
            },
        ] = sent.as_slice()
        else {
            panic!("expected one peer message, got {sent:?}");
        };
        assert_eq!(to.peer_id, peer_id);
        assert_eq!(body, "on it");
        assert!(blocks.is_none());
        assert_eq!(*handling_mode, HandlingMode::Queue);
    }

    /// The comms runtime signs a message's `in_reply_to` into its envelope.
    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn runtime_send_signs_the_peer_message_in_reply_to_onto_the_wire() {
        let (_ctx, peer_id, sender, receiver, finalizer) =
            make_runtime_less_inproc_delivery_pair().await;
        let finalize = spawn_runtime_finalization(receiver, finalizer);
        let answered = meerkat_core::InteractionId(uuid::Uuid::new_v4());

        CoreCommsRuntime::send(
            sender.as_ref(),
            CommsCommand::PeerMessage {
                to: meerkat_core::comms::PeerRoute::new(peer_id),
                body: "reply through the runtime".to_string(),
                blocks: None,
                content_taint: None,
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                objective_id: None,
                in_reply_to: Some(answered),
            },
        )
        .await
        .expect("the runtime sends the reply");

        let candidate = finalize.await.expect("finalization task");
        assert!(
            matches!(
                &candidate.interaction.content,
                meerkat_core::InteractionContent::Message { in_reply_to: Some(id), .. }
                    if *id == answered
            ),
            "the receiver sees the delivery the reply answers: {:?}",
            candidate.interaction.content
        );
    }

    /// A reply names the delivery it answers: `reply_to_peer` stamps the
    /// selected capability's id as `in_reply_to`, which the comms runtime
    /// signs into the envelope. `send_message` answers nothing and stamps
    /// nothing.
    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn test_runtime_less_reply_to_peer_names_the_delivery_it_answers() {
        let (ctx, peer_id, _sender, receiver, finalizer) =
            make_runtime_less_inproc_delivery_pair().await;
        let finalize = spawn_runtime_finalization(receiver, finalizer);
        let capability = reply_capability(peer_id, 7);
        let dispatch_context = reply_dispatch_context(vec![capability.clone()]);

        handle_tools_call_with_context(
            &ctx,
            "reply_to_peer",
            &json!({"body": "on it", "handling_mode": "queue"}),
            &dispatch_context,
        )
        .await
        .expect("the runtime-less reply dispatches");

        let candidate = finalize.await.expect("finalization task");
        assert!(
            matches!(
                &candidate.interaction.content,
                meerkat_core::InteractionContent::Message { in_reply_to: Some(id), .. }
                    if *id == capability.in_reply_to
            ),
            "the receiver sees the delivery the reply answers: {:?}",
            candidate.interaction.content
        );
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn reply_to_peer_names_the_delivery_it_answers_and_send_message_names_none() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair).await;
        let runtime = Arc::new(RecordingRuntime::new());
        ctx.runtime = Some(RuntimeCommsCommandHandle::new(runtime.clone()));
        let capability = reply_capability(peer_id, 7);
        let dispatch_context = reply_dispatch_context(vec![capability.clone()]);

        handle_tools_call_with_context(
            &ctx,
            "reply_to_peer",
            &json!({"body": "on it", "handling_mode": "queue"}),
            &dispatch_context,
        )
        .await
        .expect("single-delivery reply must dispatch without a selector");
        handle_tools_call_with_context(
            &ctx,
            "send_message",
            &json!({"peer_id": peer_id.to_string(), "body": "fresh", "handling_mode": "queue"}),
            &dispatch_context,
        )
        .await
        .expect("send_message dispatches");

        let sent = runtime.sent.lock();
        let [
            CommsCommand::PeerMessage {
                in_reply_to: reply_in_reply_to,
                ..
            },
            CommsCommand::PeerMessage {
                in_reply_to: send_in_reply_to,
                ..
            },
        ] = sent.as_slice()
        else {
            panic!("expected two peer messages, got {sent:?}");
        };
        assert_eq!(*reply_in_reply_to, Some(capability.in_reply_to));
        assert_eq!(*send_in_reply_to, None);
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn reply_to_peer_without_capability_returns_no_reply_capability() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, _peer_id) = make_trusted_runtime_less_context(&peer_keypair).await;
        let runtime = Arc::new(RecordingRuntime::new());
        ctx.runtime = Some(RuntimeCommsCommandHandle::new(runtime.clone()));

        let err = handle_tools_call_with_context(
            &ctx,
            "reply_to_peer",
            &json!({"body": "on it", "handling_mode": "queue"}),
            &ToolDispatchContext::default(),
        )
        .await
        .expect_err("a turn without a peer delivery must not reply");

        assert!(
            err.starts_with("no_reply_capability"),
            "expected typed no_reply_capability error, got: {err}"
        );
        assert_eq!(runtime.sent_len(), 0, "nothing may be dispatched");
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn reply_to_peer_with_multiple_deliveries_requires_selector() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair).await;
        let runtime = Arc::new(RecordingRuntime::new());
        ctx.runtime = Some(RuntimeCommsCommandHandle::new(runtime.clone()));
        let first = reply_capability(peer_id, 1);
        let second = reply_capability(peer_id, 2);
        let dispatch_context = reply_dispatch_context(vec![first.clone(), second.clone()]);

        let err = handle_tools_call_with_context(
            &ctx,
            "reply_to_peer",
            &json!({"body": "on it", "handling_mode": "queue"}),
            &dispatch_context,
        )
        .await
        .expect_err("two deliveries without a selector must be ambiguous");

        assert!(
            err.starts_with("ambiguous_reply"),
            "expected typed ambiguous_reply error, got: {err}"
        );
        assert!(
            err.contains(&first.in_reply_to.to_string())
                && err.contains(&second.in_reply_to.to_string()),
            "ambiguous error must list the delivery ids so the caller can retry: {err}"
        );
        assert_eq!(runtime.sent_len(), 0, "nothing may be dispatched");

        // Retrying with an explicit selector resolves the ambiguity.
        handle_tools_call_with_context(
            &ctx,
            "reply_to_peer",
            &json!({
                "reply_to": second.in_reply_to.to_string(),
                "body": "on it",
                "handling_mode": "queue"
            }),
            &dispatch_context,
        )
        .await
        .expect("explicit selector must resolve the ambiguity");
        assert_eq!(runtime.sent_len(), 1);
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn reply_to_peer_with_unknown_selector_returns_unknown_reply_to() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair).await;
        let runtime = Arc::new(RecordingRuntime::new());
        ctx.runtime = Some(RuntimeCommsCommandHandle::new(runtime.clone()));
        let dispatch_context = reply_dispatch_context(vec![reply_capability(peer_id, 7)]);

        let err = handle_tools_call_with_context(
            &ctx,
            "reply_to_peer",
            &json!({
                "reply_to": uuid::Uuid::from_u128(999).to_string(),
                "body": "on it",
                "handling_mode": "queue"
            }),
            &dispatch_context,
        )
        .await
        .expect_err("a selector outside the turn's deliveries must fail");

        assert!(
            err.starts_with("unknown_reply_to"),
            "expected typed unknown_reply_to error, got: {err}"
        );
        assert_eq!(runtime.sent_len(), 0, "nothing may be dispatched");
    }

    /// A delivery id is chosen by its sender, so two senders can carry the
    /// same one into a turn. A selector matching both must answer ambiguous,
    /// name both senders and send nothing; before 0.9.4 it routed to
    /// whichever delivery came first. The governed target resolution uses
    /// the same selector, so it cannot authorize the first match either.
    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn reply_to_peer_selector_matching_two_senders_is_ambiguous_and_sends_nothing() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, first_sender) = make_trusted_runtime_less_context(&peer_keypair).await;
        let runtime = Arc::new(RecordingRuntime::new());
        ctx.runtime = Some(RuntimeCommsCommandHandle::new(runtime.clone()));
        let second_sender = PeerId::new();
        let first = reply_capability(first_sender, 31);
        let mut second = reply_capability(second_sender, 31);
        second.display_name = Some("other-agent".to_string());
        let dispatch_context = reply_dispatch_context(vec![first.clone(), second.clone()]);
        let args = json!({
            "reply_to": first.in_reply_to.to_string(),
            "body": "on it",
            "handling_mode": "queue"
        });

        let err = handle_tools_call_with_context(&ctx, "reply_to_peer", &args, &dispatch_context)
            .await
            .expect_err("a selector matching two senders must not pick one");

        assert!(
            err.starts_with("ambiguous_reply"),
            "expected typed ambiguous_reply error, got: {err}"
        );
        assert!(
            err.contains(&first_sender.to_string())
                && err.contains(&second_sender.to_string())
                && err.contains("other-agent"),
            "the ambiguity must name both senders, by id and display name: {err}"
        );
        assert_eq!(runtime.sent_len(), 0, "nothing may be dispatched");
        let target = comms_tool_operation_target("reply_to_peer", &args, &dispatch_context)
            .expect_err("the governed target of an ambiguous reply has no addressee");
        assert!(target.starts_with("ambiguous_reply"), "{target}");
        assert_eq!(
            select_reply_capability(
                &dispatch_context
                    .peer_reply_deliveries()
                    .expect("well-formed reply context")
                    .deliveries,
                Some(&first.in_reply_to.to_string()),
            ),
            Err(ReplySelectionError::AmbiguousReply {
                reply_to: Some(first.in_reply_to.to_string()),
                candidates: vec![first, second],
            })
        );
    }

    /// The same sender's delivery id appears once, so a selector that matches
    /// only one sender still routes even when that sender's capability was
    /// recorded twice (turn start and a later boundary).
    #[test]
    fn reply_selector_routes_a_single_sender_matched_twice() {
        let sender = PeerId::new();
        let capability = reply_capability(sender, 41);
        let deliveries = [capability.clone(), capability.clone()];
        assert_eq!(
            select_reply_capability(&deliveries, Some(&capability.in_reply_to.to_string())),
            Ok(&capability)
        );
    }

    /// A malformed turn-start reply context fails closed with its own code,
    /// typed, and never coerces into "no capability".
    #[test]
    fn malformed_reply_context_fails_closed_typed() {
        let context =
            ToolDispatchContext::default().with_turn_metadata(std::collections::BTreeMap::from([
                (
                    COMMS_PEER_REPLY_DISPATCH_CONTEXT_KEY.to_string(),
                    json!({"deliveries": "not a list"}),
                ),
            ]));
        let error = peer_reply_dispatch_context(&context).expect_err("malformed context");
        assert!(
            matches!(error, ReplySelectionError::InvalidReplyContext(_)),
            "{error:?}"
        );
        assert!(
            error.to_string().starts_with("invalid_reply_context"),
            "{error}"
        );
    }

    /// Distinct delivery ids from two senders each select their own sender,
    /// in the selector and in the typed operation target the governed path
    /// authorizes.
    #[test]
    fn reply_selector_routes_distinct_ids_to_their_own_senders() {
        let first = reply_capability(PeerId::new(), 51);
        let second = reply_capability(PeerId::new(), 52);
        let deliveries = [first, second];
        let dispatch_context = reply_dispatch_context(deliveries.to_vec());
        for capability in &deliveries {
            let selector = capability.in_reply_to.to_string();
            assert_eq!(
                select_reply_capability(&deliveries, Some(&selector)),
                Ok(capability)
            );
            assert_eq!(
                comms_tool_operation_target(
                    "reply_to_peer",
                    &json!({"reply_to": selector, "body": "on it", "handling_mode": "queue"}),
                    &dispatch_context,
                ),
                Ok(ToolOperationTarget::PeerSend {
                    peer: capability.peer_id,
                    kind: PeerSendKind::Message,
                })
            );
        }
    }

    /// Counts the authorizations governed work prepares, allowing each.
    struct CountingAllow(Arc<std::sync::atomic::AtomicUsize>);
    struct CountedAllowed;

    impl meerkat_core::WorkAuthorization for CountingAllow {
        fn prepare(
            &self,
            _: &meerkat_core::PreparedAuthorizationBinding,
        ) -> Result<
            Arc<dyn meerkat_core::PreparedOperationAuthorization>,
            meerkat_core::OperationAuthorizationError,
        > {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Arc::new(CountedAllowed))
        }
    }

    impl meerkat_core::PreparedOperationAuthorization for CountedAllowed {
        fn review_tier(&self) -> meerkat_core::OperationReviewTier {
            meerkat_core::OperationReviewTier::R1
        }

        fn check_current(
            &self,
            _: &meerkat_core::PreparedAuthorizationBinding,
        ) -> Result<(), meerkat_core::OperationAuthorizationError> {
            Ok(())
        }
    }

    /// Governed work that names no single addressee is refused before
    /// authorization: a `reply_to` matching two senders' deliveries fails
    /// plan resolution with the typed `ambiguous_reply` reason, so no
    /// authorization is prepared and nothing is sent. In the same governed
    /// context, a selector naming one sender's delivery prepares one
    /// authorization and delivers once (control).
    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn governed_ambiguous_reply_is_refused_before_authorization() {
        let peer_keypair = Keypair::generate();
        let (ctx, sender) = make_trusted_runtime_less_context(&peer_keypair).await;
        let runtime = Arc::new(RecordingRuntime::new());
        let dispatcher: Arc<dyn meerkat_core::AgentToolDispatcher> = Arc::new(
            crate::agent::dispatcher::CommsToolDispatcher::new_with_runtime(
                ctx.router.clone(),
                ctx.trusted_peers.clone(),
                runtime.clone(),
            ),
        );
        let prepared = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let shared = reply_capability(sender, 61);
        let mut other = reply_capability(PeerId::new(), 61);
        other.display_name = Some("other-agent".to_string());
        let only = reply_capability(sender, 62);
        let governed = reply_dispatch_context(vec![shared.clone(), other, only.clone()])
            .with_work_authorization(Some(meerkat_core::WorkAuthorizationContext::new(
                Arc::new(CountingAllow(Arc::clone(&prepared))),
                meerkat_core::OperationExecutionScope::Domain,
            )));
        let resolution = meerkat_core::ToolExecutionResolutionContext::new(
            meerkat_core::ToolDeadlineChain::new(vec![
                meerkat_core::ToolDeadlineContributor::finite(
                    meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
                    std::time::Duration::from_mins(10),
                ),
            ])
            .expect("valid test deadline chain"),
        );
        let reply_args = |capability: &PeerReplyCapability| {
            serde_json::value::RawValue::from_string(
                json!({
                    "reply_to": capability.in_reply_to.to_string(),
                    "body": "on it",
                    "handling_mode": "queue"
                })
                .to_string(),
            )
            .expect("valid args")
        };

        let ambiguous = reply_args(&shared);
        let call = meerkat_core::ToolCallView {
            id: "reply-ambiguous",
            name: "reply_to_peer",
            args: &ambiguous,
        };
        let resolved = meerkat_core::agent::resolve_tool_execution_plan_fenced(
            &dispatcher,
            call,
            &governed,
            &resolution,
        );
        assert!(
            matches!(
                &resolved,
                Err(meerkat_core::ToolExecutionResolutionError::InvalidArguments { reason, .. })
                    if reason.starts_with("ambiguous_reply")
            ),
            "{resolved:?}"
        );
        assert_eq!(
            prepared.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "no authorization is prepared for an ambiguous reply"
        );
        assert_eq!(runtime.sent_len(), 0, "nothing may be dispatched");

        let single = reply_args(&only);
        let call = meerkat_core::ToolCallView {
            id: "reply-single",
            name: "reply_to_peer",
            args: &single,
        };
        let plan = meerkat_core::agent::resolve_tool_execution_plan_fenced(
            &dispatcher,
            call,
            &governed,
            &resolution,
        )
        .expect("a selector naming one sender resolves");
        meerkat_core::agent::dispatch_tool_execution_plan_fenced(
            &dispatcher,
            call,
            &governed,
            &plan,
        )
        .await
        .expect("the governed reply to one sender delivers");
        assert_eq!(prepared.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(runtime.sent_len(), 1);
    }

    /// Trust is re-validated at dispatch time: a capability naming a peer that
    /// is not (or no longer) trusted must fail closed, never route on the
    /// minted identity alone.
    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn reply_to_peer_revalidates_trust_at_dispatch() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, _trusted_peer) = make_trusted_runtime_less_context(&peer_keypair).await;
        let runtime = Arc::new(RecordingRuntime::new());
        ctx.runtime = Some(RuntimeCommsCommandHandle::new(runtime.clone()));
        let untrusted_peer = PeerId::new();
        let dispatch_context = reply_dispatch_context(vec![reply_capability(untrusted_peer, 7)]);

        let err = handle_tools_call_with_context(
            &ctx,
            "reply_to_peer",
            &json!({"body": "on it", "handling_mode": "queue"}),
            &dispatch_context,
        )
        .await
        .expect_err("an untrusted capability peer must fail closed");

        assert!(
            err.starts_with("peer_not_found_or_not_trusted"),
            "expected typed trust error, got: {err}"
        );
        assert_eq!(runtime.sent_len(), 0, "nothing may be dispatched");
    }

    /// A present-but-malformed reply context is a distinct fail-closed error,
    /// never coerced into "no capability" or a successful dispatch.
    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn reply_to_peer_rejects_malformed_reply_context() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, _peer_id) = make_trusted_runtime_less_context(&peer_keypair).await;
        let runtime = Arc::new(RecordingRuntime::new());
        ctx.runtime = Some(RuntimeCommsCommandHandle::new(runtime.clone()));
        let dispatch_context =
            ToolDispatchContext::default().with_turn_metadata(std::collections::BTreeMap::from([
                (
                    COMMS_PEER_REPLY_DISPATCH_CONTEXT_KEY.to_string(),
                    json!({"deliveries": [], "unexpected": true}),
                ),
            ]));

        let err = handle_tools_call_with_context(
            &ctx,
            "reply_to_peer",
            &json!({"body": "on it", "handling_mode": "queue"}),
            &dispatch_context,
        )
        .await
        .expect_err("unknown fields in the reply context must fail closed");

        assert!(
            err.starts_with("invalid_reply_context"),
            "expected typed invalid_reply_context error, got: {err}"
        );
        assert_eq!(runtime.sent_len(), 0, "nothing may be dispatched");
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

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn send_message_resolves_current_turn_image_ref_to_peer_message_blocks() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair).await;
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

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn send_request_resolves_current_turn_image_ref_to_peer_request_blocks() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair).await;
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

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn send_message_resolves_blob_image_ref_to_peer_message_blocks() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair).await;
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

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn send_request_resolves_blob_image_ref_to_peer_request_blocks() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair).await;
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

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn blob_image_ref_rejects_current_turn_index() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair).await;
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

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn send_message_synthesizes_body_text_when_blocks_only_reference_image() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair).await;
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

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn send_message_does_not_duplicate_body_when_text_blocks_split_it() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair).await;
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

    /// ROW #94 gate: when a comms message carries blocks, blocks are the single
    /// content authority and the carried `body` is provably a projection of the
    /// text blocks — it cannot diverge from a contradicting `body` input.
    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn send_message_body_is_projection_of_text_blocks_when_blocks_present() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair).await;
        let runtime = Arc::new(RecordingRuntime::new());
        ctx.runtime = Some(RuntimeCommsCommandHandle::new(runtime.clone()));

        handle_tools_call_with_context(
            &ctx,
            "send_message",
            &json!({
                "peer_id": peer_id,
                // A body that contradicts the block content.
                "body": "TOTALLY DIFFERENT BODY",
                "blocks": [
                    {"type": "text", "text": "authoritative line one"},
                    {"type": "text", "text": "authoritative line two"}
                ],
                "handling_mode": "queue"
            }),
            &ToolDispatchContext::default(),
        )
        .await
        .expect("send_message should accept text blocks");

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
        // Body is a function of the text blocks, NOT the contradicting input
        // body. Helper synthesis and ingress classification read one owner.
        let expected = super::project_body_from_blocks(blocks);
        assert_eq!(body, &expected);
        assert_eq!(body, "authoritative line one\nauthoritative line two");
        assert_ne!(body, "TOTALLY DIFFERENT BODY");
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

    #[cfg(feature = "workspace-tests")]
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

        let (mut ctx, _) = make_trusted_runtime_less_context(&requester).await;
        let runtime = Arc::new(RecordingRuntime::new());
        ctx.runtime = Some(RuntimeCommsCommandHandle::new(runtime.clone()));

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

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn test_handle_peers() {
        let runtime = Arc::new(
            CommsRuntime::inproc_only(&format!(
                "mcp-tools-peers-{}",
                uuid::Uuid::new_v4().simple()
            ))
            .expect("runtime"),
        );
        let duplicate_name_peers = [
            (PubKey::new([1u8; 32]), "tcp://127.0.0.1:4200"),
            (PubKey::new([2u8; 32]), "tcp://127.0.0.1:4201"),
        ]
        .into_iter()
        .map(|(pubkey, addr)| {
            TrustedPeerDescriptor::unsigned_with_pubkey(
                "test-peer",
                pubkey.to_peer_id().to_string(),
                *pubkey.as_bytes(),
                addr,
            )
            .expect("trusted peer descriptor")
        })
        .collect();
        add_generated_peer_projection_trust_batch(
            runtime.as_ref(),
            duplicate_name_peers,
            "install duplicate-name peer trust",
        )
        .await;

        let ctx = ToolContext {
            router: runtime.router_arc(),
            trusted_peers: runtime.trusted_peers_shared(),
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

    #[cfg(feature = "workspace-tests")]
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
            meta: crate::PeerMeta::default(),
        };
        let peer_keypair = Keypair::generate();
        let (mut ctx, _) = make_trusted_runtime_less_context(&peer_keypair).await;
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
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn test_handle_peers_runtime_less_payload_uses_authorized_sendability() {
        let peer_keypair = Keypair::generate();
        let (ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair).await;

        let value = handle_tools_call(&ctx, "peers", &json!({}))
            .await
            .expect("runtime-less peers should serialize");
        let peer = &value["peers"][0];

        assert_eq!(peer["peer_id"], peer_id.to_string());
        assert_eq!(peer["source"], "trusted");
        assert_eq!(peer["sendable_kinds"], json!(["peer_message"]));
        assert_eq!(peer["capabilities"]["version"], 1);
        assert_eq!(peer["capabilities"]["extensions"], json!({}));
        assert!(peer["meta"].is_object());
    }

    #[tokio::test]
    async fn test_send_message_fails_when_recipient_is_not_trusted() {
        let sender_keypair = Keypair::generate();
        let receiver_id = PeerId::new();

        let (_, router_inbox_sender) = crate::Inbox::new_transport_only();
        let router = Arc::new(Router::new(
            sender_keypair,
            CommsConfig::default(),
            router_inbox_sender,
            true,
        ));
        let trusted_peers = router.trusted_peers_view();

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

    /// An unwired member is not a trusted peer: `send_request` to it fails at
    /// once with the typed, model-visible refusal, never a silent wait. A
    /// voice executor told to ask an unwired member reports exactly this
    /// (Turbo S S102 round 1).
    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn test_send_request_fails_typed_when_recipient_is_not_trusted() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, _trusted_peer) = make_trusted_runtime_less_context(&peer_keypair).await;
        let runtime = Arc::new(RecordingRuntime::new());
        ctx.runtime = Some(RuntimeCommsCommandHandle::new(runtime.clone()));
        let unwired_peer = PeerId::new();

        let error = handle_tools_call(
            &ctx,
            "send_request",
            &json!({
                "peer_id": unwired_peer,
                "intent": "checksum_token",
                "params": {"subject": "what time is it"},
                "handling_mode": "queue"
            }),
        )
        .await
        .expect_err("send_request to an untrusted peer must fail");

        assert!(
            error.starts_with("peer_not_found_or_not_trusted:"),
            "expected the typed trust refusal, got: {error}"
        );
        assert_eq!(runtime.sent_len(), 0, "nothing may be dispatched");
    }

    #[tokio::test]
    async fn test_send_message_invalid_handling_mode_fails_at_serde_boundary() {
        let keypair = Keypair::generate();
        let (_, inbox_sender) = crate::Inbox::new_transport_only();
        let router = Arc::new(Router::new(
            keypair,
            CommsConfig::default(),
            inbox_sender,
            true,
        ));
        let trusted_peers = router.trusted_peers_view();
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
        let (_, inbox_sender) = crate::Inbox::new_transport_only();
        let router = Arc::new(Router::new(
            keypair,
            CommsConfig::default(),
            inbox_sender,
            true,
        ));
        let trusted_peers = router.trusted_peers_view();
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

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn test_send_request_without_runtime_command_authority_fails_closed() {
        let (ctx, peer_id) = make_runtime_less_trust_only_context().await;

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
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn test_send_response_without_runtime_command_authority_fails_closed() {
        let (ctx, peer_id) = make_runtime_less_trust_only_context().await;
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
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn test_send_request_unknown_intent_fails_at_typed_boundary() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair).await;
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

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn test_send_request_malformed_params_fail_at_typed_boundary() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair).await;
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

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn test_send_response_malformed_result_fails_at_typed_boundary() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair).await;
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

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn test_send_response_checksum_token_requires_request_subject() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair).await;
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

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn test_runtime_bound_send_message_returns_typed_delivery_outcome() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair).await;
        let runtime = Arc::new(RecordingRuntime::new());
        let (hook_tx, mut hook_rx) = tokio::sync::mpsc::unbounded_channel();
        let hooks = Arc::new(meerkat_core::PostCommitHookDispatcher::new(
            meerkat_core::SessionId::new(),
        ));
        hooks
            .configure(
                Some(Arc::new(RecordingPostCommitHook { sender: hook_tx })),
                meerkat_core::HookRunOverrides::default(),
            )
            .expect("configure test post-commit hooks");
        ctx.runtime = Some(RuntimeCommsCommandHandle::new(runtime).with_post_commit_hooks(hooks));

        let result = handle_tools_call(
            &ctx,
            "send_message",
            &json!({"peer_id": peer_id, "body": "hello", "handling_mode": "queue"}),
        )
        .await
        .expect("send_message should return a typed receipt");

        assert_eq!(result["receipt"]["kind"], "peer_message_sent");
        assert_eq!(result["receipt"]["delivery"], "volatile_handed_off");
        assert!(result["receipt"].get("acked").is_none());
        let hook = tokio::time::timeout(std::time::Duration::from_secs(1), hook_rx.recv())
            .await
            .expect("peer-egress hook should be dispatched")
            .expect("hook dispatcher remains connected");
        assert_eq!(hook.point, meerkat_core::HookPoint::PeerEgressCommitted);
        assert!(matches!(
            hook.observation,
            Some(meerkat_core::HookObservation::PeerEgressCommitted(
                meerkat_core::HookPeerEgressCommitted {
                    kind: meerkat_core::HookPeerEgressKind::Message,
                    peer_id: observed_peer,
                    ..
                }
            )) if observed_peer == peer_id
        ));
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn test_runtime_less_inproc_send_message_returns_typed_delivery_outcome() {
        let (ctx, peer_id, _sender, receiver, finalizer) =
            make_runtime_less_inproc_delivery_pair().await;
        let finalize = spawn_runtime_finalization(receiver, finalizer);

        let result = handle_tools_call(
            &ctx,
            "send_message",
            &json!({"peer_id": peer_id, "body": "hello", "handling_mode": "queue"}),
        )
        .await
        .expect("runtime-less send_message should return a typed receipt");

        assert_eq!(result["status"], "sent");
        assert_eq!(result["kind"], "peer_message");
        assert_eq!(result["receipt"]["kind"], "peer_message_sent");
        assert_eq!(
            result["receipt"]["delivery"],
            json!({"durably_resolved": {"outcome": "accepted"}})
        );
        finalize
            .await
            .expect("tool-send finalization task must complete");
        let envelope_id = result["receipt"]["envelope_id"]
            .as_str()
            .expect("receipt envelope_id must be a string");
        assert_ne!(
            uuid::Uuid::parse_str(envelope_id).expect("receipt envelope_id must be a UUID"),
            uuid::Uuid::nil()
        );
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn test_runtime_less_send_message_rejects_sender_local_blob_before_transport() {
        let (ctx, peer_id, _sender, _receiver, _finalizer) =
            make_runtime_less_inproc_delivery_pair().await;

        let error = handle_tools_call_with_context(
            &ctx,
            "send_message",
            &json!({
                "peer_id": peer_id,
                "body": "generated image",
                "blocks": [{
                    "type": "image_ref",
                    "source": "blob",
                    "blob_id": "sha256:sender-local",
                    "media_type": "image/png"
                }],
                "handling_mode": "queue"
            }),
            &ToolDispatchContext::default(),
        )
        .await
        .expect_err("runtime-less transport cannot resolve a sender-local blob id");

        assert!(error.contains("runtime_less_blob_transport_unavailable"));
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn test_runtime_less_inproc_fenced_send_has_peer_message_receipt_parity() {
        let (ctx, peer_id, _sender, receiver, finalizer) =
            make_runtime_less_inproc_delivery_pair().await;
        let finalize = spawn_runtime_finalization(receiver, finalizer);

        let error = dispatch(
            &ctx,
            CommsCommand::IncarnationFencedPeerMessage {
                to: PeerRoute::new(peer_id),
                body: "fenced hello".to_string(),
                blocks: None,
                content_taint: None,
                handling_mode: HandlingMode::Queue,
                objective_id: None,
                expected_recipient: meerkat_core::comms::PeerRecipientIncarnation {
                    mob_id: "mob".to_string(),
                    agent_identity: "worker".to_string(),
                    host_id: "host".to_string(),
                    binding_generation: 2,
                    member_session_id: "session".to_string(),
                    generation: 3,
                    fence_token: 5,
                },
            },
        )
        .await
        .expect_err("runtime-less fenced send should surface durable rejection");

        assert!(
            matches!(&error, CommsToolCallError::Failed(message) if message.starts_with("peer_input_rejected:")),
            "{error}"
        );
        finalize
            .await
            .expect("fenced tool-send finalization task must complete");
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn test_runtime_bound_send_request_returns_typed_receipt() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair).await;
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

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn test_runtime_bound_send_response_returns_typed_receipt() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair).await;
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

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn send_response_resolves_blob_image_ref_to_peer_response_blocks() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair).await;
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

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn test_runtime_bound_send_response_accepts_checksum_token_subject() {
        let peer_keypair = Keypair::generate();
        let (mut ctx, peer_id) = make_trusted_runtime_less_context(&peer_keypair).await;
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

//! MCP tool implementations for Meerkat comms.
//!
//! Exposes agent-facing comms tools: `send_message`, `send_request`,
//! `send_response`, and `peers`.
//!
//! The tool inputs deserialize into typed per-tool input structs; from
//! there they are assembled directly into [`CommsCommand`] domain
//! envelopes and dispatched through the runtime. There is no wire-layer
//! [`CommsCommandRequest`] variant for peer traffic after Wave-B: the
//! agent-facing MCP surface is internal, so typed enums (`HandlingMode`,
//! `ResponseStatus`) remain the only serde-gated boundary.

use parking_lot::RwLock;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::sync::Arc;

#[cfg(test)]
use crate::{CommsConfig, Keypair};
use crate::{Router, TrustedPeers};
use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
use meerkat_core::comms::{
    CommsCommand, InputStreamMode, PeerDirectoryEntry, PeerId, PeerName, PeerRoute, SendError,
    SendReceipt,
};
use meerkat_core::interaction::{InteractionId, ResponseStatus};
use meerkat_core::tool_catalog::ToolUnavailableReason;
use meerkat_core::types::HandlingMode;

const RUNTIME_COMMAND_AUTHORITY_UNAVAILABLE_CODE: &str = "runtime_command_authority_unavailable";

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
    /// "steer" for immediate processing (normal), "queue" for next turn boundary
    pub handling_mode: HandlingMode,
}

/// Send a structured request to a peer and expect a correlated response.
///
/// Example: `{"peer_id": "<peer-id-from-peers>", "intent": "review", "params": {"file": "main.rs"}, "handling_mode": "steer"}`
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendRequestInput {
    /// Canonical peer ID from the `peers` tool
    pub peer_id: PeerId,
    /// Optional display name retained only for diagnostics
    #[serde(default)]
    pub display_name: Option<String>,
    /// Request intent (e.g. "review", "analyze")
    pub intent: String,
    /// "steer" for immediate processing (normal), "queue" for next turn boundary
    pub handling_mode: HandlingMode,
    /// Request parameters (optional, defaults to {})
    #[serde(default)]
    pub params: Option<Value>,
}

/// Send a response to a previous peer request.
///
/// Example: `{"peer_id": "<peer-id-from-peers>", "in_reply_to": "<request-id>", "status": "completed", "result": {"answer": 42}}`
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
    /// Response result data (optional)
    #[serde(default)]
    pub result: Option<Value>,
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
            "description": "Send a fire-and-forget message to a peer. No response is expected.\n\nWhen to use: Use send_message for one-way collaboration — status updates, notifications, sharing results, or any case where you do not need the peer to reply with structured data. If you need a correlated reply, use send_request instead.\n\nhandling_mode:\n- \"steer\": The peer processes your message immediately, interrupting its current work. Use for urgent or time-sensitive collaboration.\n- \"queue\": The message is delivered at the peer's next turn boundary. Use for non-urgent follow-ups where you do not want to interrupt the peer's current task.\n\nExamples:\n1. Fire-and-forget collaboration:\n   {\"peer_id\": \"<peer-id-from-peers>\", \"body\": \"FYI: the database migration completed successfully.\", \"handling_mode\": \"steer\"}\n2. Queued follow-up (non-urgent):\n   {\"peer_id\": \"<peer-id-from-peers>\", \"display_name\": \"reporter\", \"body\": \"When you finish, include the error counts from section 3.\", \"handling_mode\": \"queue\"}\n\nFailure handling:\n- peer_not_found_or_not_trusted: The peer_id does not match a trusted peer. Call peers first to pick a peer_id.\n- peer_unreachable: The peer exists but is offline or the transport failed. Retry after a delay or inform the user.",
            "inputSchema": schema_for::<SendMessageInput>()
        }),
        json!({
            "name": "send_request",
            "description": "Send a structured request to a peer and expect a correlated response. The peer will reply using send_response with the same request ID.\n\nWhen to use: Use send_request when you need the peer to perform work and return a structured result. The response will arrive as an incoming message with the original request ID in its in_reply_to field, so you can match it. If you just need to share information without expecting a reply, use send_message instead.\n\nhandling_mode:\n- \"steer\": The peer processes your request immediately, interrupting its current work. Use for requests that block your own progress.\n- \"queue\": The request is delivered at the peer's next turn boundary. Use when the peer can handle it after finishing its current task.\n\nExample — structured request/reply:\n  {\"peer_id\": \"<peer-id-from-peers>\", \"display_name\": \"analyzer\", \"intent\": \"review\", \"params\": {\"file\": \"main.rs\", \"focus\": \"error handling\"}, \"handling_mode\": \"steer\"}\n  The peer receives this, performs the review, and sends back with your peer_id:\n  {\"peer_id\": \"<your-peer-id>\", \"in_reply_to\": \"<request-id>\", \"status\": \"completed\", \"result\": {\"issues\": [...]}}\n\nFailure handling:\n- peer_not_found_or_not_trusted: The peer_id does not match a trusted peer. Call peers first to pick a peer_id.\n- peer_unreachable: The peer exists but is offline or the transport failed. Retry after a delay or inform the user.\n- Missing response: There is no built-in timeout. If the peer does not respond, it may have failed or dropped the request. Re-send or check with the peer via send_message.",
            "inputSchema": schema_for::<SendRequestInput>()
        }),
        json!({
            "name": "send_response",
            "description": "Send a response to a previous peer request. The in_reply_to field must match the request ID from the original send_request message you received.\n\nWhen to use: Use send_response after receiving a send_request from a peer. The requester is waiting for a correlated reply.\n\nstatus values:\n- \"accepted\": Acknowledge receipt; you will send a \"completed\" or \"failed\" response later.\n- \"completed\": The request succeeded. Include the result in the result field.\n- \"failed\": The request could not be fulfilled. Include error details in the result field.\n\nhandling_mode (optional): Override how the requester processes this response. Defaults to the original request's mode. Use \"steer\" to interrupt the requester immediately with your result, or \"queue\" to deliver at their next turn boundary.\n\nExamples:\n1. Completed response:\n   {\"peer_id\": \"<peer-id-from-peers>\", \"display_name\": \"requester\", \"in_reply_to\": \"<request-id>\", \"status\": \"completed\", \"result\": {\"answer\": 42}}\n2. Acceptance then later completion:\n   {\"peer_id\": \"<peer-id-from-peers>\", \"in_reply_to\": \"<request-id>\", \"status\": \"accepted\"}\n   ...later...\n   {\"peer_id\": \"<peer-id-from-peers>\", \"in_reply_to\": \"<request-id>\", \"status\": \"completed\", \"result\": {\"report\": \"done\"}}\n3. Failure response:\n   {\"peer_id\": \"<peer-id-from-peers>\", \"in_reply_to\": \"<request-id>\", \"status\": \"failed\", \"result\": {\"error\": \"file not found\"}}\n\nFailure handling:\n- peer_not_found_or_not_trusted / peer_unreachable: Same as send_message. The requester will not receive your response — they may re-send the request.\n- Invalid in_reply_to: If the ID is not a valid UUID or does not match a known request, the call fails with a validation error.",
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
    if comms_tool_unavailable_reason(ctx, name).is_some() {
        return Err(runtime_command_authority_unavailable_message());
    }

    match name {
        "send_message" => {
            let input: SendMessageInput = serde_json::from_value(args.clone())
                .map_err(|e| format!("Invalid arguments: {e}"))?;
            let to = peer_route(ctx, input.peer_id, input.display_name.as_deref())?;
            let command = CommsCommand::PeerMessage {
                to,
                body: input.body,
                blocks: None,
                handling_mode: input.handling_mode,
            };
            dispatch(ctx, command).await
        }
        "send_request" => {
            let input: SendRequestInput = serde_json::from_value(args.clone())
                .map_err(|e| format!("Invalid arguments: {e}"))?;
            let to = peer_route(ctx, input.peer_id, input.display_name.as_deref())?;
            let command = CommsCommand::PeerRequest {
                to,
                intent: input.intent,
                params: input.params.unwrap_or_else(|| json!({})),
                handling_mode: input.handling_mode,
                stream: InputStreamMode::None,
            };
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
            let command = CommsCommand::PeerResponse {
                to,
                in_reply_to: InteractionId(in_reply_to_uuid),
                status: input.status,
                result: input.result.unwrap_or_else(|| json!({})),
                handling_mode: input.handling_mode,
            };
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
        "receipt": receipt_to_json(receipt),
    })
}

fn receipt_to_json(receipt: SendReceipt) -> Value {
    match receipt {
        SendReceipt::InputAccepted {
            interaction_id,
            stream_reserved,
        } => json!({
            "type": "input_accepted",
            "interaction_id": interaction_id.0,
            "stream_reserved": stream_reserved,
        }),
        SendReceipt::PeerMessageSent { envelope_id, acked } => json!({
            "type": "peer_message_sent",
            "envelope_id": envelope_id,
            "acked": acked,
        }),
        SendReceipt::PeerLifecycleSent { envelope_id } => json!({
            "type": "peer_lifecycle_sent",
            "envelope_id": envelope_id,
        }),
        SendReceipt::PeerRequestSent {
            envelope_id,
            interaction_id,
            stream_reserved,
        } => json!({
            "type": "peer_request_sent",
            "envelope_id": envelope_id,
            "interaction_id": interaction_id.0,
            "stream_reserved": stream_reserved,
        }),
        SendReceipt::PeerResponseSent {
            envelope_id,
            in_reply_to,
        } => json!({
            "type": "peer_response_sent",
            "envelope_id": envelope_id,
            "in_reply_to": in_reply_to.0,
        }),
    }
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
    if let Some(runtime) = &ctx.runtime {
        let peer_list: Vec<Value> = runtime
            .peers()
            .await
            .into_iter()
            .map(|peer| {
                json!({
                    "name": peer.name.to_string(),
                    "peer_id": peer.peer_id,
                    "address": peer.address,
                    "source": format!("{:?}", peer.source),
                    "sendable_kinds": peer.sendable_kinds,
                    "capabilities": peer.capabilities,
                    "reachability": peer.reachability,
                    "last_unreachable_reason": peer.last_unreachable_reason,
                    "meta": peer.meta,
                })
            })
            .collect();
        return Ok(json!({ "peers": peer_list }));
    }

    let self_pubkey = ctx.router.keypair_arc().public_key();
    let peers = ctx.trusted_peers.read();
    let peer_list: Vec<Value> = peers
        .peers
        .iter()
        .filter(|p| p.pubkey != self_pubkey)
        .map(|p| {
            let mut entry = json!({
                "name": p.name,
                "peer_id": p.pubkey.to_peer_id(),
                "address": p.addr
            });
            if let Some(desc) = &p.meta.description {
                entry["description"] = json!(desc);
            }
            if !p.meta.labels.is_empty() {
                entry["labels"] = json!(p.meta.labels);
            }
            entry
        })
        .collect();
    drop(peers);

    Ok(json!({ "peers": peer_list }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::{PubKey, TrustedPeer};
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

    struct RecordingRuntime {
        sent: Mutex<Vec<CommsCommand>>,
        notify: Arc<Notify>,
    }

    impl RecordingRuntime {
        fn new() -> Self {
            Self {
                sent: Mutex::new(Vec::new()),
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
                CommsCommand::PeerRequest { .. } => {
                    meerkat_core::comms::SendReceipt::PeerRequestSent {
                        envelope_id: uuid::Uuid::from_u128(1),
                        interaction_id: InteractionId(uuid::Uuid::from_u128(2)),
                        stream_reserved: false,
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
        let mut args = extract_projection_send_response_args(&projection);
        args["result"] = json!({"pong": true});

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
            to.display_name.as_ref().map(|name| name.as_str()),
            Some("operator")
        );
        assert_eq!(in_reply_to.0, request_id);
        assert_eq!(*status, ResponseStatus::Completed);
        assert_eq!(result["pong"], true);
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
                "intent": "review",
                "params": {"file": "main.rs"},
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
                "result": {"ok": true}
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
                "intent": "review",
                "params": {"file": "main.rs"},
                "handling_mode": "queue"
            }),
        )
        .await
        .expect("runtime-backed send_request should succeed");

        assert_eq!(runtime.sent_len(), 1);
        assert_eq!(result["status"], "sent");
        assert_eq!(result["kind"], "peer_request");
        assert_eq!(result["receipt"]["type"], "peer_request_sent");
        assert_eq!(
            result["receipt"]["envelope_id"],
            uuid::Uuid::from_u128(1).to_string()
        );
        assert_eq!(
            result["receipt"]["interaction_id"],
            uuid::Uuid::from_u128(2).to_string()
        );
        assert_eq!(result["receipt"]["stream_reserved"], false);
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
                "result": {"ok": true}
            }),
        )
        .await
        .expect("runtime-backed send_response should succeed");

        assert_eq!(runtime.sent_len(), 1);
        assert_eq!(result["status"], "sent");
        assert_eq!(result["kind"], "peer_response");
        assert_eq!(result["receipt"]["type"], "peer_response_sent");
        assert_eq!(
            result["receipt"]["envelope_id"],
            uuid::Uuid::from_u128(3).to_string()
        );
        assert_eq!(result["receipt"]["in_reply_to"], request_id.to_string());
    }
}

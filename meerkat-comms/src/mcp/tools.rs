//! MCP tool implementations for Meerkat comms.
//!
//! Exposes agent-facing comms tools: `send_message`, `send_request`,
//! `send_response`, and `peers`.

use parking_lot::RwLock;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

#[cfg(test)]
use crate::{CommsConfig, Keypair};
use crate::{Router, Status, TrustedPeers};
use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;

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
/// Example: `{"to": "helper-1", "body": "What is the current time?", "handling_mode": "steer"}`
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendMessageInput {
    /// Peer name to send to
    pub to: String,
    /// Message body
    pub body: String,
    /// "steer" for immediate processing (normal), "queue" for next turn boundary
    pub handling_mode: String,
}

/// Send a structured request to a peer and expect a correlated response.
///
/// Example: `{"to": "analyzer", "intent": "review", "params": {"file": "main.rs"}, "handling_mode": "steer"}`
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendRequestInput {
    /// Peer name to send to
    pub to: String,
    /// Request intent (e.g. "review", "analyze")
    pub intent: String,
    /// "steer" for immediate processing (normal), "queue" for next turn boundary
    pub handling_mode: String,
    /// Request parameters (optional, defaults to {})
    #[serde(default)]
    pub params: Option<Value>,
}

/// Send a response to a previous peer request.
///
/// Example: `{"to": "requester", "in_reply_to": "<request-id>", "status": "completed", "result": {"answer": 42}}`
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendResponseInput {
    /// Peer name to send to
    pub to: String,
    /// ID of the request being responded to (from the original request)
    pub in_reply_to: String,
    /// Response status: "accepted", "completed", or "failed"
    pub status: String,
    /// Response result data (optional)
    #[serde(default)]
    pub result: Option<Value>,
    /// Handling mode override for terminal responses: "steer" or "queue" (optional)
    #[serde(default)]
    pub handling_mode: Option<String>,
}

/// Input schema for `peers` tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct PeersInput {}

/// Backward-compatible unified send input (used by `normalize_comms_call`).
#[derive(Debug, Deserialize)]
pub struct SendInput {
    pub kind: String,
    pub to: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub blocks: Option<Vec<meerkat_core::types::ContentBlock>>,
    #[serde(default)]
    pub intent: Option<String>,
    #[serde(default)]
    pub params: Option<Value>,
    #[serde(default)]
    pub in_reply_to: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub handling_mode: Option<String>,
}

/// Context for comms tool execution
#[derive(Clone)]
pub struct ToolContext {
    pub router: Arc<Router>,
    pub trusted_peers: Arc<RwLock<TrustedPeers>>,
    pub runtime: Option<Arc<dyn CoreCommsRuntime>>,
}

/// Returns the list of comms tools.
pub fn tools_list() -> Vec<Value> {
    vec![
        json!({
            "name": "send_message",
            "description": "Send a fire-and-forget message to a peer. No response is expected.\n\nWhen to use: Use send_message for one-way collaboration — status updates, notifications, sharing results, or any case where you do not need the peer to reply with structured data. If you need a correlated reply, use send_request instead.\n\nhandling_mode:\n- \"steer\": The peer processes your message immediately, interrupting its current work. Use for urgent or time-sensitive collaboration.\n- \"queue\": The message is delivered at the peer's next turn boundary. Use for non-urgent follow-ups where you do not want to interrupt the peer's current task.\n\nExamples:\n1. Fire-and-forget collaboration:\n   {\"to\": \"helper-1\", \"body\": \"FYI: the database migration completed successfully.\", \"handling_mode\": \"steer\"}\n2. Queued follow-up (non-urgent):\n   {\"to\": \"reporter\", \"body\": \"When you finish, include the error counts from section 3.\", \"handling_mode\": \"queue\"}\n\nFailure handling:\n- peer_not_found_or_not_trusted: The peer name does not match any known peer. Call peers first to verify available names.\n- peer_unreachable: The peer exists but is offline or the transport failed. Retry after a delay or inform the user.",
            "inputSchema": schema_for::<SendMessageInput>()
        }),
        json!({
            "name": "send_request",
            "description": "Send a structured request to a peer and expect a correlated response. The peer will reply using send_response with the same request ID.\n\nWhen to use: Use send_request when you need the peer to perform work and return a structured result. The response will arrive as an incoming message with the original request ID in its in_reply_to field, so you can match it. If you just need to share information without expecting a reply, use send_message instead.\n\nhandling_mode:\n- \"steer\": The peer processes your request immediately, interrupting its current work. Use for requests that block your own progress.\n- \"queue\": The request is delivered at the peer's next turn boundary. Use when the peer can handle it after finishing its current task.\n\nExample — structured request/reply:\n  {\"to\": \"analyzer\", \"intent\": \"review\", \"params\": {\"file\": \"main.rs\", \"focus\": \"error handling\"}, \"handling_mode\": \"steer\"}\n  The peer receives this, performs the review, and sends back:\n  {\"to\": \"<your-name>\", \"in_reply_to\": \"<request-id>\", \"status\": \"completed\", \"result\": {\"issues\": [...]}}\n\nFailure handling:\n- peer_not_found_or_not_trusted: The peer name does not match any known peer. Call peers first to verify available names.\n- peer_unreachable: The peer exists but is offline or the transport failed. Retry after a delay or inform the user.\n- Missing response: There is no built-in timeout. If the peer does not respond, it may have failed or dropped the request. Re-send or check with the peer via send_message.",
            "inputSchema": schema_for::<SendRequestInput>()
        }),
        json!({
            "name": "send_response",
            "description": "Send a response to a previous peer request. The in_reply_to field must match the request ID from the original send_request message you received.\n\nWhen to use: Use send_response after receiving a send_request from a peer. The requester is waiting for a correlated reply.\n\nstatus values:\n- \"accepted\": Acknowledge receipt; you will send a \"completed\" or \"failed\" response later.\n- \"completed\": The request succeeded. Include the result in the result field.\n- \"failed\": The request could not be fulfilled. Include error details in the result field.\n\nhandling_mode (optional): Override how the requester processes this response. Defaults to the original request's mode. Use \"steer\" to interrupt the requester immediately with your result, or \"queue\" to deliver at their next turn boundary.\n\nExamples:\n1. Completed response:\n   {\"to\": \"requester\", \"in_reply_to\": \"<request-id>\", \"status\": \"completed\", \"result\": {\"answer\": 42}}\n2. Acceptance then later completion:\n   {\"to\": \"requester\", \"in_reply_to\": \"<request-id>\", \"status\": \"accepted\"}\n   ...later...\n   {\"to\": \"requester\", \"in_reply_to\": \"<request-id>\", \"status\": \"completed\", \"result\": {\"report\": \"done\"}}\n3. Failure response:\n   {\"to\": \"requester\", \"in_reply_to\": \"<request-id>\", \"status\": \"failed\", \"result\": {\"error\": \"file not found\"}}\n\nFailure handling:\n- peer_not_found_or_not_trusted / peer_unreachable: Same as send_message. The requester will not receive your response — they may re-send the request.\n- Invalid in_reply_to: If the ID is not a valid UUID or does not match a known request, the call fails with a validation error.",
            "inputSchema": schema_for::<SendResponseInput>()
        }),
        json!({
            "name": "peers",
            "description": "List all visible peers with connection info and optional metadata (description, labels, capabilities, reachability).\n\nAlways call peers before sending any message to verify the peer name exists and is reachable. The returned list includes:\n- name: The peer name to use in the \"to\" field of send_message / send_request / send_response.\n- peer_id: Unique cryptographic identity.\n- address: Transport address.\n- reachability: Whether the peer is currently reachable.\n- capabilities / meta: What the peer can do and its role description.\n\nExample output:\n{\"peers\": [{\"name\": \"helper-1\", \"peer_id\": \"abc123\", \"address\": \"tcp://...\", \"reachability\": \"reachable\", \"meta\": {\"description\": \"Code review helper\"}}]}",
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
    match name {
        "send_message" => {
            let input: SendMessageInput = serde_json::from_value(args.clone())
                .map_err(|e| format!("Invalid arguments: {e}"))?;
            handle_send_unified(
                ctx,
                "peer_message",
                input.to,
                Some(input.body),
                None,
                None,
                None,
                None,
                None,
                Some(input.handling_mode),
                None,
            )
            .await
        }
        "send_request" => {
            let input: SendRequestInput = serde_json::from_value(args.clone())
                .map_err(|e| format!("Invalid arguments: {e}"))?;
            handle_send_unified(
                ctx,
                "peer_request",
                input.to,
                None,
                None,
                Some(input.intent),
                input.params,
                None,
                None,
                Some(input.handling_mode),
                None,
            )
            .await
        }
        "send_response" => {
            let input: SendResponseInput = serde_json::from_value(args.clone())
                .map_err(|e| format!("Invalid arguments: {e}"))?;
            handle_send_unified(
                ctx,
                "peer_response",
                input.to,
                None,
                None,
                None,
                None,
                Some(input.in_reply_to),
                Some(input.status),
                input.handling_mode,
                input.result,
            )
            .await
        }
        // Backward compatibility: the old unified "send" tool still works
        // for programmatic callers that use the kind discriminator.
        "send" => {
            let input: SendInput = serde_json::from_value(args.clone())
                .map_err(|e| format!("Invalid arguments: {e}"))?;
            handle_send_unified(
                ctx,
                &input.kind,
                input.to,
                input.body,
                input.blocks,
                input.intent,
                input.params,
                input.in_reply_to,
                input.status,
                input.handling_mode,
                input.result,
            )
            .await
        }
        "peers" => {
            let _input: PeersInput = serde_json::from_value(args.clone())
                .map_err(|e| format!("Invalid arguments: {e}"))?;
            handle_peers(ctx).await
        }
        _ => Err(format!("Unknown tool: {name}")),
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_send_unified(
    ctx: &ToolContext,
    kind: &str,
    to: String,
    body: Option<String>,
    blocks: Option<Vec<meerkat_core::types::ContentBlock>>,
    intent: Option<String>,
    params: Option<Value>,
    in_reply_to: Option<String>,
    status: Option<String>,
    handling_mode: Option<String>,
    result: Option<Value>,
) -> Result<Value, String> {
    let request = meerkat_core::comms::CommsCommandRequest {
        kind: kind.to_string(),
        to: Some(to),
        body,
        blocks,
        intent,
        params,
        in_reply_to,
        status,
        result,
        source: None,
        stream: None,
        allow_self_session: None,
        handling_mode,
    };
    let command = request
        .parse(&meerkat_core::SessionId::new())
        .map_err(format_comms_command_error)?;

    let cmd_kind = command.command_kind().to_string();
    if let Some(runtime) = &ctx.runtime {
        runtime.send(command).await.map_err(|error| match error {
            meerkat_core::comms::SendError::PeerNotFound(peer_name) => {
                format!(
                    "peer_not_found_or_not_trusted: peer '{peer_name}' is not found or not trusted"
                )
            }
            meerkat_core::comms::SendError::PeerOffline => format!(
                "peer_unreachable: peer '{}' is unreachable: offline_or_no_ack",
                request.to.as_deref().unwrap_or("<unknown>")
            ),
            meerkat_core::comms::SendError::Internal(inner) if is_transport_internal(&inner) => {
                format!(
                    "peer_unreachable: peer '{}' is unreachable: transport_error ({inner})",
                    request.to.as_deref().unwrap_or("<unknown>")
                )
            }
            other => other.to_string(),
        })?;
        return Ok(json!({ "status": "sent", "kind": cmd_kind }));
    }

    match command {
        meerkat_core::comms::CommsCommand::Input { .. } => {
            Err("input command is not supported by MCP send".to_string())
        }
        meerkat_core::comms::CommsCommand::PeerMessage {
            to,
            body,
            blocks,
            handling_mode,
        } => {
            ctx.router
                .send(
                    to.as_str(),
                    crate::types::MessageKind::Message {
                        body,
                        blocks,
                        handling_mode: Some(handling_mode),
                    },
                )
                .await
                .map_err(|e| format_router_send_error(to.as_str(), e))?;
            Ok(json!({ "status": "sent", "kind": cmd_kind }))
        }
        meerkat_core::comms::CommsCommand::PeerRequest {
            to,
            intent,
            params,
            handling_mode,
            ..
        } => {
            ctx.router
                .send(
                    to.as_str(),
                    crate::types::MessageKind::Request {
                        intent,
                        params,
                        handling_mode: Some(handling_mode),
                    },
                )
                .await
                .map_err(|e| format_router_send_error(to.as_str(), e))?;
            Ok(json!({ "status": "sent", "kind": cmd_kind }))
        }
        meerkat_core::comms::CommsCommand::PeerResponse {
            to,
            in_reply_to,
            status,
            result,
            handling_mode,
        } => {
            let status = match status {
                meerkat_core::ResponseStatus::Accepted => Status::Accepted,
                meerkat_core::ResponseStatus::Completed => Status::Completed,
                meerkat_core::ResponseStatus::Failed => Status::Failed,
            };
            ctx.router
                .send(
                    to.as_str(),
                    crate::types::MessageKind::Response {
                        in_reply_to: in_reply_to.0,
                        status,
                        result,
                        handling_mode,
                    },
                )
                .await
                .map_err(|e| format_router_send_error(to.as_str(), e))?;
            Ok(json!({ "status": "sent", "kind": cmd_kind }))
        }
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

fn format_comms_command_error(
    errors: Vec<meerkat_core::comms::CommsCommandValidationError>,
) -> String {
    let errors = meerkat_core::comms::CommsCommandRequest::validation_errors_to_json(&errors);
    if let Some(first) = errors.first() {
        let field = first["field"].as_str().unwrap_or("command");
        let issue = first["issue"].as_str().unwrap_or("invalid");
        let got = first["got"].as_str();
        match (field, issue) {
            ("body", "required_field") => "peer_message requires body".to_string(),
            ("to", "required_field") => "to is required".to_string(),
            ("intent", "required_field") => "peer_request requires intent".to_string(),
            ("in_reply_to", "required_field") => "peer_response requires in_reply_to".to_string(),
            ("in_reply_to", "invalid_uuid") => got.map_or_else(
                || "invalid in_reply_to".to_string(),
                |value| format!("invalid UUID for in_reply_to: {value}"),
            ),
            ("status", "invalid_value") => got.map_or_else(
                || "invalid status".to_string(),
                |value| format!("invalid status: {value}"),
            ),
            ("to", "invalid_value") => got.map_or_else(
                || "invalid peer name".to_string(),
                |value| format!("invalid to: {value}"),
            ),
            ("source", "invalid_value") => got.map_or_else(
                || "invalid source".to_string(),
                |value| format!("invalid source: {value}"),
            ),
            ("stream", "removed_unsupported_field") => got.map_or_else(
                || "stream field has been removed".to_string(),
                |value| format!("stream field has been removed (got: {value})"),
            ),
            ("kind", "unknown_kind") => got.map_or_else(
                || "unknown kind".to_string(),
                |value| format!("unknown kind: {value}"),
            ),
            ("handling_mode", "required_field") => {
                "handling_mode is required: use \"steer\" for normal collaboration or \"queue\" for next-turn processing".to_string()
            }
            _ => issue.to_string(),
        }
    } else {
        "invalid command".to_string()
    }
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
    let peer_map: BTreeMap<String, Value> = peers
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
            (p.name.clone(), entry)
        })
        .collect();
    drop(peers);

    let peer_list: Vec<Value> = peer_map.into_values().collect();
    Ok(json!({ "peers": peer_list }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::{PubKey, TrustedPeer};

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
        assert!(required_names.contains(&"to"));
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
        assert!(required_names.contains(&"to"));
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
        assert!(required_names.contains(&"to"));
        assert!(required_names.contains(&"in_reply_to"));
        assert!(required_names.contains(&"status"));
    }

    #[tokio::test]
    async fn test_handle_peers() {
        let keypair = Keypair::generate();
        let trusted_peers = TrustedPeers {
            peers: vec![TrustedPeer {
                name: "test-peer".to_string(),
                pubkey: PubKey::new([1u8; 32]),
                addr: "tcp://127.0.0.1:4200".to_string(),
                meta: crate::PeerMeta::default(),
            }],
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
    }

    #[tokio::test]
    async fn test_send_message_fails_when_recipient_is_not_trusted() {
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let receiver_name = format!("receiver-{suffix}");
        let sender_keypair = Keypair::generate();

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
                "to": receiver_name,
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
    async fn test_legacy_send_still_works() {
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let receiver_name = format!("receiver-{suffix}");
        let sender_keypair = Keypair::generate();
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

        // The old "send" tool with kind discriminator still works
        let result = handle_tools_call(
            &ctx,
            "send",
            &json!({
                "kind": "peer_message",
                "to": receiver_name,
                "body": "hello",
                "handling_mode": "steer"
            }),
        )
        .await;
        // Will fail because peer is not trusted, but the parsing should succeed
        let error = result.expect_err("send should fail for an unreachable peer");
        assert!(error.contains("not found or not trusted"));
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
}

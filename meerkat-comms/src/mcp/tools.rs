//! MCP tool implementations for Meerkat comms.
//!
//! Exposes agent-facing comms tools: `send_message`, `send_request`,
//! `send_response`, and `peers`.
//!
//! All comms-send tools serialize into the typed
//! [`meerkat_core::comms::CommsCommandRequest`] enum at the deserialization
//! boundary. Invalid discriminators (`source`, `stream`, `handling_mode`,
//! `status`) become serde errors rather than runtime string-match failures.

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
use meerkat_core::comms::{CommsCommand, CommsCommandRequest, PeerName};
use meerkat_core::interaction::{InteractionId, ResponseStatus};
use meerkat_core::types::HandlingMode;

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
    pub handling_mode: HandlingMode,
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
    pub handling_mode: HandlingMode,
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
            let to = peer_name(&input.to)?;
            let request = CommsCommandRequest::PeerMessage {
                to,
                body: Some(input.body),
                blocks: None,
                handling_mode: Some(input.handling_mode),
            };
            dispatch(ctx, request).await
        }
        "send_request" => {
            let input: SendRequestInput = serde_json::from_value(args.clone())
                .map_err(|e| format!("Invalid arguments: {e}"))?;
            let to = peer_name(&input.to)?;
            let request = CommsCommandRequest::PeerRequest {
                to,
                intent: input.intent,
                params: input.params,
                body: None,
                handling_mode: Some(input.handling_mode),
                stream: None,
            };
            dispatch(ctx, request).await
        }
        "send_response" => {
            let input: SendResponseInput = serde_json::from_value(args.clone())
                .map_err(|e| format!("Invalid arguments: {e}"))?;
            let to = peer_name(&input.to)?;
            let in_reply_to_uuid = uuid::Uuid::parse_str(&input.in_reply_to)
                .map_err(|_| format!("invalid UUID for in_reply_to: {}", input.in_reply_to))?;
            let request = CommsCommandRequest::PeerResponse {
                to,
                in_reply_to: InteractionId(in_reply_to_uuid),
                status: input.status,
                result: input.result,
                handling_mode: input.handling_mode,
            };
            dispatch(ctx, request).await
        }
        "peers" => {
            let _input: PeersInput = serde_json::from_value(args.clone())
                .map_err(|e| format!("Invalid arguments: {e}"))?;
            handle_peers(ctx).await
        }
        _ => Err(format!("Unknown tool: {name}")),
    }
}

fn peer_name(value: &str) -> Result<PeerName, String> {
    PeerName::new(value).map_err(|err| format!("invalid to: {err}"))
}

async fn dispatch(ctx: &ToolContext, request: CommsCommandRequest) -> Result<Value, String> {
    // Capture peer name for error normalization before consuming the request.
    let peer_for_errors = match &request {
        CommsCommandRequest::PeerMessage { to, .. }
        | CommsCommandRequest::PeerRequest { to, .. }
        | CommsCommandRequest::PeerResponse { to, .. } => Some(to.as_string()),
        CommsCommandRequest::Input { .. } => None,
    };

    let command = request
        // Per-session id is irrelevant here — the agent-facing tools only
        // reach peer_* commands, never the local `Input` variant.
        .into_command(&meerkat_core::SessionId::new())
        .map_err(|e| e.to_string())?;
    let cmd_kind = command.command_kind().to_string();

    if let Some(runtime) = &ctx.runtime {
        runtime.send(command).await.map_err(|error| match error {
            meerkat_core::comms::SendError::PeerNotFound(p) => {
                format!("peer_not_found_or_not_trusted: peer '{p}' is not found or not trusted")
            }
            meerkat_core::comms::SendError::PeerOffline => format!(
                "peer_unreachable: peer '{}' is unreachable: offline_or_no_ack",
                peer_for_errors.as_deref().unwrap_or("<unknown>")
            ),
            meerkat_core::comms::SendError::Internal(inner) if is_transport_internal(&inner) => {
                format!(
                    "peer_unreachable: peer '{}' is unreachable: transport_error ({inner})",
                    peer_for_errors.as_deref().unwrap_or("<unknown>")
                )
            }
            other => other.to_string(),
        })?;
        return Ok(json!({ "status": "sent", "kind": cmd_kind }));
    }

    match command {
        CommsCommand::Input { .. } => Err("input command is not supported by MCP send".to_string()),
        CommsCommand::PeerMessage {
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
        CommsCommand::PeerLifecycle { to, kind, params } => {
            ctx.router
                .send(
                    to.as_str(),
                    crate::types::MessageKind::Lifecycle { kind, params },
                )
                .await
                .map_err(|e| format_router_send_error(to.as_str(), e))?;
            Ok(json!({ "status": "sent", "kind": cmd_kind }))
        }
        CommsCommand::PeerRequest {
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
        CommsCommand::PeerResponse {
            to,
            in_reply_to,
            status,
            result,
            handling_mode,
        } => {
            let status = match status {
                ResponseStatus::Accepted => Status::Accepted,
                ResponseStatus::Completed => Status::Completed,
                ResponseStatus::Failed => Status::Failed,
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
                "to": "alice",
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
}

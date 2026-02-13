//! MCP tool implementations for Meerkat comms.
//!
//! Exposes exactly two tools: `send` and `peers`.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

#[cfg(test)]
use crate::{CommsConfig, Keypair};
use crate::{Router, Status, TrustedPeers};

fn schema_for<T: JsonSchema>() -> Value {
    let schema = schemars::schema_for!(T);
    let mut value = serde_json::to_value(&schema).unwrap_or(Value::Null);

    if let Value::Object(ref mut obj) = value {
        if obj.get("type").and_then(Value::as_str) == Some("object") {
            obj.entry("properties".to_string())
                .or_insert_with(|| Value::Object(Map::new()));
            obj.entry("required".to_string())
                .or_insert_with(|| Value::Array(Vec::new()));
        }
    }

    value
}

/// Input schema for the unified `send` tool.
///
/// Uses a flat `kind` discriminator with dispatch-time validation.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendInput {
    /// Command kind: "peer_message", "peer_request", or "peer_response"
    pub kind: String,
    /// Peer name to send to
    pub to: String,
    /// Message body (required for peer_message)
    #[serde(default)]
    pub body: Option<String>,
    /// Request intent (required for peer_request)
    #[serde(default)]
    pub intent: Option<String>,
    /// Request parameters (optional, defaults to {})
    #[serde(default)]
    pub params: Option<Value>,
    /// ID of the request being responded to (required for peer_response)
    #[serde(default)]
    pub in_reply_to: Option<String>,
    /// Response status: "accepted", "completed", or "failed" (for peer_response)
    #[serde(default)]
    pub status: Option<String>,
    /// Response result data (optional for peer_response)
    #[serde(default)]
    pub result: Option<Value>,
}

/// Input schema for `peers` tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct PeersInput {}

/// Context for comms tool execution
#[derive(Clone)]
pub struct ToolContext {
    pub router: Arc<Router>,
    pub trusted_peers: Arc<RwLock<TrustedPeers>>,
}

/// Returns the list of comms tools: exactly `send` and `peers`.
pub fn tools_list() -> Vec<Value> {
    vec![
        json!({
            "name": "send",
            "description": "Send a message, request, or response to a peer. Use `kind` to select the command type.",
            "inputSchema": schema_for::<SendInput>()
        }),
        json!({
            "name": "peers",
            "description": "List all visible peers and their connection info",
            "inputSchema": schema_for::<PeersInput>()
        }),
    ]
}

/// Handle a comms tool call. Only `send` and `peers` are valid.
pub async fn handle_tools_call(
    ctx: &ToolContext,
    name: &str,
    args: &Value,
) -> Result<Value, String> {
    match name {
        "send" => {
            let input: SendInput = serde_json::from_value(args.clone())
                .map_err(|e| format!("Invalid arguments: {e}"))?;
            handle_send(ctx, input).await
        }
        "peers" => {
            let _input: PeersInput = serde_json::from_value(args.clone())
                .map_err(|e| format!("Invalid arguments: {e}"))?;
            handle_peers(ctx).await
        }
        _ => Err(format!("Unknown tool: {name}")),
    }
}

async fn handle_send(ctx: &ToolContext, input: SendInput) -> Result<Value, String> {
    match input.kind.as_str() {
        "peer_message" => {
            let body = input.body.ok_or("peer_message requires 'body' field")?;
            ctx.router
                .send(&input.to, crate::types::MessageKind::Message { body })
                .await
                .map_err(|e| e.to_string())?;
            Ok(json!({ "status": "sent", "kind": "peer_message" }))
        }
        "peer_request" => {
            let intent = input.intent.ok_or("peer_request requires 'intent' field")?;
            let params = input.params.unwrap_or(json!({}));
            ctx.router
                .send(
                    &input.to,
                    crate::types::MessageKind::Request { intent, params },
                )
                .await
                .map_err(|e| e.to_string())?;
            Ok(json!({ "status": "sent", "kind": "peer_request" }))
        }
        "peer_response" => {
            let in_reply_to_str = input
                .in_reply_to
                .ok_or("peer_response requires 'in_reply_to' field")?;
            let in_reply_to: Uuid = in_reply_to_str
                .parse()
                .map_err(|_| format!("invalid UUID for in_reply_to: {in_reply_to_str}"))?;
            let status_str = input.status.as_deref().unwrap_or("completed");
            let status = match status_str {
                "accepted" => Status::Accepted,
                "completed" => Status::Completed,
                "failed" => Status::Failed,
                other => return Err(format!("invalid status: {other}")),
            };
            let result = input.result.unwrap_or(Value::Null);
            ctx.router
                .send(
                    &input.to,
                    crate::types::MessageKind::Response {
                        in_reply_to,
                        status,
                        result,
                    },
                )
                .await
                .map_err(|e| e.to_string())?;
            Ok(json!({ "status": "sent", "kind": "peer_response" }))
        }
        other => Err(format!("unknown send kind: {other}")),
    }
}

async fn handle_peers(ctx: &ToolContext) -> Result<Value, String> {
    let self_pubkey = ctx.router.keypair_arc().public_key();
    let peers = ctx.trusted_peers.read().await;
    let peer_map: BTreeMap<String, Value> = peers
        .peers
        .iter()
        .filter(|p| p.pubkey != self_pubkey)
        .map(|p| {
            (
                p.name.clone(),
                json!({
                    "name": p.name,
                    "peer_id": p.pubkey.to_peer_id(),
                    "address": p.addr
                }),
            )
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
    fn test_tools_list_is_exactly_send_and_peers() {
        let tools = tools_list();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0]["name"], "send");
        assert_eq!(tools[1]["name"], "peers");
    }

    #[test]
    fn test_send_schema_has_kind_field() {
        let schema = schema_for::<SendInput>();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["kind"].is_object());
        assert!(schema["properties"]["to"].is_object());
    }

    #[tokio::test]
    async fn test_handle_peers() {
        let keypair = Keypair::generate();
        let trusted_peers = TrustedPeers {
            peers: vec![TrustedPeer {
                name: "test-peer".to_string(),
                pubkey: PubKey::new([1u8; 32]),
                addr: "tcp://127.0.0.1:4200".to_string(),
            }],
        };
        let trusted_peers = Arc::new(RwLock::new(trusted_peers));
        let (_, inbox_sender) = crate::Inbox::new();
        let router = Arc::new(Router::with_shared_peers(
            keypair,
            trusted_peers.clone(),
            CommsConfig::default(),
            inbox_sender,
        ));

        let ctx = ToolContext {
            router,
            trusted_peers,
        };

        let result = handle_tools_call(&ctx, "peers", &json!({})).await;
        assert!(result.is_ok());
        let val = result.unwrap();
        let peers = val["peers"].as_array().expect("peers should be array");
        assert!(peers.iter().any(|p| p["name"] == "test-peer"));
    }

    #[tokio::test]
    async fn test_send_fails_when_recipient_is_not_trusted() {
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
        ));

        let ctx = ToolContext {
            router,
            trusted_peers,
        };

        let result = handle_tools_call(
            &ctx,
            "send",
            &json!({
                "kind": "peer_message",
                "to": receiver_name,
                "body": "hello"
            }),
        )
        .await;

        assert!(result.is_err());
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
        ));
        let ctx = ToolContext {
            router,
            trusted_peers,
        };

        // Legacy tool names should no longer be recognized
        assert!(
            handle_tools_call(&ctx, "send_message", &json!({}))
                .await
                .is_err()
        );
        assert!(
            handle_tools_call(&ctx, "list_peers", &json!({}))
                .await
                .is_err()
        );
    }
}

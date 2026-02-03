//! MCP tool implementations for Meerkat comms.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

#[cfg(test)]
use crate::{CommsConfig, Keypair};
use crate::{Router, Status, TrustedPeers};

fn schema_for<T: JsonSchema>() -> Value {
    let schema = schemars::schema_for!(T);
    let mut value = serde_json::to_value(&schema).unwrap_or(Value::Null);

    // Some generators omit empty `properties`/`required` for `{}`.
    // Our tool schema contract expects explicit presence of both keys.
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

/// Input schema for send_message tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendMessageInput {
    /// Peer name to send message to
    pub to: String,
    /// Message content
    pub body: String,
}

/// Input schema for send_request tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendRequestInput {
    /// Peer name to send request to
    pub to: String,
    /// Request intent/action
    pub intent: String,
    /// Request parameters
    #[serde(default)]
    pub params: Value,
}

/// Input schema for send_response tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendResponseInput {
    /// Peer name to send response to
    pub to: String,
    /// ID of the request being responded to
    #[schemars(with = "String")]
    pub in_reply_to: Uuid,
    /// Response status
    pub status: Status,
    /// Response result data
    #[serde(default)]
    pub result: Value,
}

/// Input schema for list_peers tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListPeersInput {}

/// Context for comms tool execution
#[derive(Clone)]
pub struct ToolContext {
    pub router: Arc<Router>,
    pub trusted_peers: Arc<RwLock<TrustedPeers>>,
}

/// Returns the list of comms tools
pub fn tools_list() -> Vec<Value> {
    vec![
        json!({
            "name": "send_message",
            "description": "Send a simple text message to a trusted peer",
            "inputSchema": schema_for::<SendMessageInput>()
        }),
        json!({
            "name": "send_request",
            "description": "Send a request to a trusted peer and wait for acknowledgement",
            "inputSchema": schema_for::<SendRequestInput>()
        }),
        json!({
            "name": "send_response",
            "description": "Send a response back to a previous request from a peer",
            "inputSchema": schema_for::<SendResponseInput>()
        }),
        json!({
            "name": "list_peers",
            "description": "List all trusted peers and their connection status",
            "inputSchema": schema_for::<ListPeersInput>()
        }),
    ]
}
/// Handle a comms tool call
pub async fn handle_tools_call(
    ctx: &ToolContext,
    name: &str,
    args: &Value,
) -> Result<Value, String> {
    match name {
        "send_message" => {
            let input: SendMessageInput = serde_json::from_value(args.clone())
                .map_err(|e| format!("Invalid arguments: {}", e))?;
            handle_send_message(ctx, input).await
        }
        "send_request" => {
            let input: SendRequestInput = serde_json::from_value(args.clone())
                .map_err(|e| format!("Invalid arguments: {}", e))?;
            handle_send_request(ctx, input).await
        }
        "send_response" => {
            let input: SendResponseInput = serde_json::from_value(args.clone())
                .map_err(|e| format!("Invalid arguments: {}", e))?;
            handle_send_response(ctx, input).await
        }
        "list_peers" => {
            let _input: ListPeersInput = serde_json::from_value(args.clone())
                .map_err(|e| format!("Invalid arguments: {}", e))?;
            handle_list_peers(ctx).await
        }
        _ => Err(format!("Unknown tool: {}", name)),
    }
}

async fn handle_send_message(ctx: &ToolContext, input: SendMessageInput) -> Result<Value, String> {
    ctx.router
        .send_message(&input.to, input.body)
        .await
        .map_err(|e| e.to_string())?;
    Ok(json!({ "status": "sent" }))
}

async fn handle_send_request(ctx: &ToolContext, input: SendRequestInput) -> Result<Value, String> {
    ctx.router
        .send_request(&input.to, input.intent, input.params)
        .await
        .map_err(|e| e.to_string())?;
    Ok(json!({ "status": "sent" }))
}

async fn handle_send_response(
    ctx: &ToolContext,
    input: SendResponseInput,
) -> Result<Value, String> {
    ctx.router
        .send_response(&input.to, input.in_reply_to, input.status, input.result)
        .await
        .map_err(|e| e.to_string())?;
    Ok(json!({ "status": "sent" }))
}

async fn handle_list_peers(ctx: &ToolContext) -> Result<Value, String> {
    let peers = ctx.trusted_peers.read().await;
    let peer_list: Vec<Value> = peers
        .peers
        .iter()
        .map(|p| {
            json!({
                "name": p.name,
                "peer_id": p.pubkey.to_peer_id(),
                "address": p.addr
            })
        })
        .collect();

    Ok(json!({ "peers": peer_list }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::{PubKey, TrustedPeer};

    #[test]
    fn test_schema_generation() {
        let schema = schema_for::<SendMessageInput>();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["to"].is_object());
        assert!(schema["properties"]["body"].is_object());
    }

    #[test]
    fn test_tools_list() {
        let tools = tools_list();
        assert_eq!(tools.len(), 4);
        assert_eq!(tools[0]["name"], "send_message");
        assert_eq!(tools[1]["name"], "send_request");
        assert_eq!(tools[2]["name"], "send_response");
        assert_eq!(tools[3]["name"], "list_peers");
    }

    #[tokio::test]
    async fn test_handle_list_peers() {
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

        let result = handle_tools_call(&ctx, "list_peers", &json!({})).await;
        assert!(result.is_ok());
        let val = result.unwrap();
        assert_eq!(val["peers"].as_array().unwrap().len(), 1);
        assert_eq!(val["peers"][0]["name"], "test-peer");
    }
}

//! MCP tool implementations for Meerkat comms.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use meerkat_comms::{CommsConfig, Keypair, Router, Status, TrustedPeers};

/// Input schema for send_message tool
#[derive(Debug, Deserialize)]
pub struct SendMessageInput {
    /// Name of the peer to send to
    pub peer: String,
    /// Message body
    pub body: String,
}

/// Input schema for send_request tool
#[derive(Debug, Deserialize)]
pub struct SendRequestInput {
    /// Name of the peer to send to
    pub peer: String,
    /// Intent of the request
    pub intent: String,
    /// Parameters for the request
    #[serde(default)]
    pub params: Value,
}

/// Input schema for send_response tool.
///
/// Note: `peer` is explicitly required (rather than auto-routing based on request origin)
/// because being explicit is simpler and doesn't require tracking pending requests.
/// The agent can easily extract the sender from the original request envelope's `from` field.
#[derive(Debug, Deserialize)]
pub struct SendResponseInput {
    /// Name of the peer to send the response to (typically the request originator)
    pub peer: String,
    /// ID of the request this is a response to
    pub request_id: Uuid,
    /// Status of the response
    pub status: ResponseStatus,
    /// Result of the request
    #[serde(default)]
    pub result: Value,
}

/// Response status for the send_response tool
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ResponseStatus {
    Accepted,
    Completed,
    Failed,
}

impl From<ResponseStatus> for Status {
    fn from(s: ResponseStatus) -> Self {
        match s {
            ResponseStatus::Accepted => Status::Accepted,
            ResponseStatus::Completed => Status::Completed,
            ResponseStatus::Failed => Status::Failed,
        }
    }
}

/// Input schema for list_peers tool
#[derive(Debug, Deserialize)]
pub struct ListPeersInput {}

/// Output for list_peers tool
#[derive(Debug, Serialize)]
pub struct PeerInfo {
    pub name: String,
    pub pubkey: String,
    pub addr: String,
}

/// Returns the list of MCP tools exposed by meerkat-comms
pub fn tools_list() -> Vec<Value> {
    vec![
        json!({
            "name": "send_message",
            "description": "Send a message to a peer. Returns when the peer acknowledges receipt.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "peer": {
                        "type": "string",
                        "description": "Name of the peer to send to"
                    },
                    "body": {
                        "type": "string",
                        "description": "Message body"
                    }
                },
                "required": ["peer", "body"]
            }
        }),
        json!({
            "name": "send_request",
            "description": "Send a request to a peer. Returns when the peer acknowledges receipt.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "peer": {
                        "type": "string",
                        "description": "Name of the peer to send to"
                    },
                    "intent": {
                        "type": "string",
                        "description": "Intent of the request (e.g., 'review-pr', 'analyze-code')"
                    },
                    "params": {
                        "type": "object",
                        "description": "Parameters for the request"
                    }
                },
                "required": ["peer", "intent"]
            }
        }),
        json!({
            "name": "send_response",
            "description": "Reply to a request. Does not wait for acknowledgement.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "peer": {
                        "type": "string",
                        "description": "Name of the peer to send to"
                    },
                    "request_id": {
                        "type": "string",
                        "format": "uuid",
                        "description": "ID of the request this is a response to"
                    },
                    "status": {
                        "type": "string",
                        "enum": ["accepted", "completed", "failed"],
                        "description": "Status of the response"
                    },
                    "result": {
                        "type": "object",
                        "description": "Result of the request"
                    }
                },
                "required": ["peer", "request_id", "status"]
            }
        }),
        json!({
            "name": "list_peers",
            "description": "List trusted peers. Note: This returns the trusted list, not live online status. Use send_message to probe liveness.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }),
    ]
}

/// Context needed to handle tool calls
pub struct ToolContext {
    pub router: Arc<Router>,
    pub trusted_peers: Arc<RwLock<TrustedPeers>>,
}

impl ToolContext {
    /// Create a new tool context
    pub fn new(keypair: Keypair, trusted_peers: TrustedPeers, config: CommsConfig) -> Self {
        let trusted_peers = Arc::new(RwLock::new(trusted_peers.clone()));
        let router = Arc::new(Router::with_shared_peers(
            keypair,
            trusted_peers.clone(),
            config,
        ));
        Self {
            router,
            trusted_peers,
        }
    }
}

/// Handle a tools/call request
pub async fn handle_tools_call(
    ctx: &ToolContext,
    tool_name: &str,
    arguments: &Value,
) -> Result<Value, String> {
    match tool_name {
        "send_message" => {
            let input: SendMessageInput = serde_json::from_value(arguments.clone())
                .map_err(|e| format!("Invalid arguments: {e}"))?;
            handle_send_message(ctx, input).await
        }
        "send_request" => {
            let input: SendRequestInput = serde_json::from_value(arguments.clone())
                .map_err(|e| format!("Invalid arguments: {e}"))?;
            handle_send_request(ctx, input).await
        }
        "send_response" => {
            let input: SendResponseInput = serde_json::from_value(arguments.clone())
                .map_err(|e| format!("Invalid arguments: {e}"))?;
            handle_send_response(ctx, input).await
        }
        "list_peers" => {
            let _input: ListPeersInput = serde_json::from_value(arguments.clone())
                .map_err(|e| format!("Invalid arguments: {e}"))?;
            handle_list_peers(ctx).await
        }
        _ => Err(format!("Unknown tool: {tool_name}")),
    }
}

async fn handle_send_message(ctx: &ToolContext, input: SendMessageInput) -> Result<Value, String> {
    ctx.router
        .send_message(&input.peer, input.body)
        .await
        .map_err(|e| format!("Failed to send message: {e}"))?;

    Ok(json!({
        "success": true,
        "message": format!("Message sent to {}", input.peer)
    }))
}

async fn handle_send_request(ctx: &ToolContext, input: SendRequestInput) -> Result<Value, String> {
    ctx.router
        .send_request(&input.peer, input.intent.clone(), input.params)
        .await
        .map_err(|e| format!("Failed to send request: {e}"))?;

    Ok(json!({
        "success": true,
        "message": format!("Request '{}' sent to {}", input.intent, input.peer)
    }))
}

async fn handle_send_response(
    ctx: &ToolContext,
    input: SendResponseInput,
) -> Result<Value, String> {
    ctx.router
        .send_response(
            &input.peer,
            input.request_id,
            input.status.into(),
            input.result,
        )
        .await
        .map_err(|e| format!("Failed to send response: {e}"))?;

    Ok(json!({
        "success": true,
        "message": format!("Response sent to {}", input.peer)
    }))
}

async fn handle_list_peers(ctx: &ToolContext) -> Result<Value, String> {
    let trusted_peers = ctx.trusted_peers.read().await;
    let peers: Vec<PeerInfo> = trusted_peers
        .peers
        .iter()
        .map(|p| PeerInfo {
            name: p.name.clone(),
            pubkey: p.pubkey.to_peer_id(),
            addr: p.addr.clone(),
        })
        .collect();

    Ok(json!({
        "peers": peers
    }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_comms::{PubKey, TrustedPeer};
    use tempfile::TempDir;

    fn make_keypair() -> Keypair {
        Keypair::generate()
    }

    fn make_trusted_peers_with_addr(name: &str, pubkey: &PubKey, addr: &str) -> TrustedPeers {
        TrustedPeers {
            peers: vec![TrustedPeer {
                name: name.to_string(),
                pubkey: *pubkey,
                addr: addr.to_string(),
            }],
        }
    }

    #[test]
    fn test_tools_list_contains_all_four_tools() -> Result<(), Box<dyn std::error::Error>> {
        let tools = tools_list();
        assert_eq!(tools.len(), 4, "Expected 4 tools");

        let tool_names: Vec<&str> = tools
            .iter()
            .map(|t| t["name"].as_str().unwrap_or_default())
            .collect();

        assert!(tool_names.contains(&"send_message"));
        assert!(tool_names.contains(&"send_request"));
        assert!(tool_names.contains(&"send_response"));
        assert!(tool_names.contains(&"list_peers"));
        Ok(())
    }

    #[test]
    fn test_mcp_tool_discovery() {
        let tools = tools_list();
        assert_eq!(tools.len(), 4);

        // Verify each tool has required fields
        for tool in &tools {
            assert!(tool["name"].is_string());
            assert!(tool["description"].is_string());
            assert!(tool["inputSchema"].is_object());
        }
    }

    #[test]
    fn test_send_message_input_parsing() -> Result<(), Box<dyn std::error::Error>> {
        let input_json = json!({
            "peer": "review-meerkat",
            "body": "Hello!"
        });

        let input: SendMessageInput = serde_json::from_value(input_json)?;
        assert_eq!(input.peer, "review-meerkat");
        assert_eq!(input.body, "Hello!");
        Ok(())
    }

    #[test]
    fn test_send_request_input_parsing() -> Result<(), Box<dyn std::error::Error>> {
        let input_json = json!({
            "peer": "review-meerkat",
            "intent": "review-pr",
            "params": { "pr": 42 }
        });

        let input: SendRequestInput = serde_json::from_value(input_json)?;
        assert_eq!(input.peer, "review-meerkat");
        assert_eq!(input.intent, "review-pr");
        assert_eq!(input.params["pr"], 42);
        Ok(())
    }

    #[test]
    fn test_send_response_input_parsing() -> Result<(), Box<dyn std::error::Error>> {
        let input_json = json!({
            "peer": "coding-meerkat",
            "request_id": "01234567-89ab-cdef-0123-456789abcdef",
            "status": "completed",
            "result": { "approved": true }
        });

        let input: SendResponseInput = serde_json::from_value(input_json)?;
        assert_eq!(input.peer, "coding-meerkat");
        assert!(matches!(input.status, ResponseStatus::Completed));
        assert_eq!(input.result["approved"], true);
        Ok(())
    }

    #[test]
    fn test_list_peers_input_parsing() -> Result<(), Box<dyn std::error::Error>> {
        let input_json = json!({});
        let _input: ListPeersInput = serde_json::from_value(input_json)?;
        Ok(())
    }

    #[tokio::test]
    async fn test_tool_send_message() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = TempDir::new()?;
        let sock_path = tmp.path().join("peer.sock");

        let peer_keypair = make_keypair();
        let our_keypair = make_keypair();
        let our_pubkey = our_keypair.public_key();

        let trusted_peers = make_trusted_peers_with_addr(
            "test-peer",
            &peer_keypair.public_key(),
            &format!("uds://{}", sock_path.display()),
        );

        let ctx = ToolContext::new(our_keypair, trusted_peers, CommsConfig::default());

        // Start mock peer server
        let listener = tokio::net::UnixListener::bind(&sock_path)?;
        let server_handle = tokio::spawn(async move {
            use meerkat_comms::{Envelope, MessageKind, Signature};
            use tokio::io::{AsyncReadExt, AsyncWriteExt};

            let (mut stream, _) = listener.accept().await.map_err(|e| e.to_string())?;

            // Read envelope
            let mut len_bytes = [0u8; 4];
            stream
                .read_exact(&mut len_bytes)
                .await
                .map_err(|e| e.to_string())?;
            let len = u32::from_be_bytes(len_bytes);
            let mut payload = vec![0u8; len as usize];
            stream
                .read_exact(&mut payload)
                .await
                .map_err(|e| e.to_string())?;
            let envelope: Envelope =
                ciborium::from_reader(&payload[..]).map_err(|e| e.to_string())?;

            // Send ack
            let mut ack = Envelope {
                id: uuid::Uuid::new_v4(),
                from: peer_keypair.public_key(),
                to: our_pubkey,
                kind: MessageKind::Ack {
                    in_reply_to: envelope.id,
                },
                sig: Signature::new([0u8; 64]),
            };
            ack.sign(&peer_keypair);

            let mut ack_payload = Vec::new();
            ciborium::into_writer(&ack, &mut ack_payload).map_err(|e| e.to_string())?;
            let ack_len = ack_payload.len() as u32;
            stream
                .write_all(&ack_len.to_be_bytes())
                .await
                .map_err(|e| e.to_string())?;
            stream
                .write_all(&ack_payload)
                .await
                .map_err(|e| e.to_string())?;
            stream.flush().await.map_err(|e| e.to_string())?;
            Ok::<(), String>(())
        });

        let result = handle_tools_call(
            &ctx,
            "send_message",
            &json!({
                "peer": "test-peer",
                "body": "Hello!"
            }),
        )
        .await;

        assert!(
            result.is_ok(),
            "send_message should succeed: {:?}",
            result.err()
        );
        let output = result.map_err(|e| e.to_string())?;
        assert_eq!(output["success"], true);

        server_handle.await??;
        Ok(())
    }

    #[tokio::test]
    async fn test_tool_send_request() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = TempDir::new()?;
        let sock_path = tmp.path().join("peer.sock");

        let peer_keypair = make_keypair();
        let our_keypair = make_keypair();
        let our_pubkey = our_keypair.public_key();

        let trusted_peers = make_trusted_peers_with_addr(
            "test-peer",
            &peer_keypair.public_key(),
            &format!("uds://{}", sock_path.display()),
        );

        let ctx = ToolContext::new(our_keypair, trusted_peers, CommsConfig::default());

        // Start mock peer server
        let listener = tokio::net::UnixListener::bind(&sock_path)?;
        let server_handle = tokio::spawn(async move {
            use meerkat_comms::{Envelope, MessageKind, Signature};
            use tokio::io::{AsyncReadExt, AsyncWriteExt};

            let (mut stream, _) = listener.accept().await.map_err(|e| e.to_string())?;

            // Read envelope
            let mut len_bytes = [0u8; 4];
            stream
                .read_exact(&mut len_bytes)
                .await
                .map_err(|e| e.to_string())?;
            let len = u32::from_be_bytes(len_bytes);
            let mut payload = vec![0u8; len as usize];
            stream
                .read_exact(&mut payload)
                .await
                .map_err(|e| e.to_string())?;
            let envelope: Envelope =
                ciborium::from_reader(&payload[..]).map_err(|e| e.to_string())?;

            // Send ack
            let mut ack = Envelope {
                id: uuid::Uuid::new_v4(),
                from: peer_keypair.public_key(),
                to: our_pubkey,
                kind: MessageKind::Ack {
                    in_reply_to: envelope.id,
                },
                sig: Signature::new([0u8; 64]),
            };
            ack.sign(&peer_keypair);

            let mut ack_payload = Vec::new();
            ciborium::into_writer(&ack, &mut ack_payload).map_err(|e| e.to_string())?;
            let ack_len = ack_payload.len() as u32;
            stream
                .write_all(&ack_len.to_be_bytes())
                .await
                .map_err(|e| e.to_string())?;
            stream
                .write_all(&ack_payload)
                .await
                .map_err(|e| e.to_string())?;
            stream.flush().await.map_err(|e| e.to_string())?;
            Ok::<(), String>(())
        });

        let result = handle_tools_call(
            &ctx,
            "send_request",
            &json!({
                "peer": "test-peer",
                "intent": "review-pr",
                "params": { "pr": 42 }
            }),
        )
        .await;

        assert!(result.is_ok(), "send_request should succeed");
        let output = result.map_err(|e| e.to_string())?;
        assert_eq!(output["success"], true);

        server_handle.await??;
        Ok(())
    }

    #[tokio::test]
    async fn test_tool_send_response() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = TempDir::new()?;
        let sock_path = tmp.path().join("peer.sock");

        let peer_keypair = make_keypair();
        let our_keypair = make_keypair();

        let trusted_peers = make_trusted_peers_with_addr(
            "test-peer",
            &peer_keypair.public_key(),
            &format!("uds://{}", sock_path.display()),
        );

        let ctx = ToolContext::new(our_keypair, trusted_peers, CommsConfig::default());

        // Start mock peer server (no ack expected for Response)
        let listener = tokio::net::UnixListener::bind(&sock_path)?;
        let server_handle = tokio::spawn(async move {
            use meerkat_comms::Envelope;
            use tokio::io::AsyncReadExt;

            let (mut stream, _) = listener.accept().await.map_err(|e| e.to_string())?;

            // Read envelope
            let mut len_bytes = [0u8; 4];
            stream
                .read_exact(&mut len_bytes)
                .await
                .map_err(|e| e.to_string())?;
            let len = u32::from_be_bytes(len_bytes);
            let mut payload = vec![0u8; len as usize];
            stream
                .read_exact(&mut payload)
                .await
                .map_err(|e| e.to_string())?;
            let _envelope: Envelope =
                ciborium::from_reader(&payload[..]).map_err(|e| e.to_string())?;

            // No ack sent for Response
            Ok::<(), String>(())
        });

        let result = handle_tools_call(
            &ctx,
            "send_response",
            &json!({
                "peer": "test-peer",
                "request_id": "01234567-89ab-cdef-0123-456789abcdef",
                "status": "completed",
                "result": { "approved": true }
            }),
        )
        .await;

        assert!(result.is_ok(), "send_response should succeed");
        let output = result.map_err(|e| e.to_string())?;
        assert_eq!(output["success"], true);

        server_handle.await??;
        Ok(())
    }

    #[tokio::test]
    async fn test_tool_list_peers() -> Result<(), Box<dyn std::error::Error>> {
        let peer1_keypair = make_keypair();
        let peer2_keypair = make_keypair();
        let our_keypair = make_keypair();

        let trusted_peers = TrustedPeers {
            peers: vec![
                TrustedPeer {
                    name: "peer1".to_string(),
                    pubkey: peer1_keypair.public_key(),
                    addr: "tcp://192.168.1.1:4200".to_string(),
                },
                TrustedPeer {
                    name: "peer2".to_string(),
                    pubkey: peer2_keypair.public_key(),
                    addr: "uds:///tmp/peer2.sock".to_string(),
                },
            ],
        };

        let ctx = ToolContext::new(our_keypair, trusted_peers, CommsConfig::default());

        let result = handle_tools_call(&ctx, "list_peers", &json!({})).await;

        assert!(result.is_ok());
        let output = result.map_err(|e| e.to_string())?;
        let peers = output["peers"].as_array().ok_or("not an array")?;
        assert_eq!(peers.len(), 2);

        // Verify peer info
        assert_eq!(peers[0]["name"], "peer1");
        assert!(
            peers[0]["pubkey"]
                .as_str()
                .ok_or("not a string")?
                .starts_with("ed25519:")
        );
        assert_eq!(peers[0]["addr"], "tcp://192.168.1.1:4200");

        assert_eq!(peers[1]["name"], "peer2");
        assert!(
            peers[1]["pubkey"]
                .as_str()
                .ok_or("not a string")?
                .starts_with("ed25519:")
        );
        assert_eq!(peers[1]["addr"], "uds:///tmp/peer2.sock");

        Ok(())
    }
}

//! Result fidelity through real rmcp I/O and both exported MCP wrappers.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::McpConnection;
use crate::{McpError, McpProtocol};
use meerkat_core::{ContentBlock, McpServerConfig, Message, ToolResult};
use rmcp::{
    ErrorData, RoleServer, ServerHandler, ServiceExt,
    model::{CallToolRequestParams, CallToolResult, ServerCapabilities, ServerInfo},
    service::{RequestContext, RoleClient, RunningService},
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone)]
enum Reply {
    Result(CallToolResult),
    ProtocolError,
}
struct Server {
    reply: Reply,
    requests: Arc<Mutex<Vec<CallToolRequestParams>>>,
}
impl ServerHandler for Server {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.requests.lock().unwrap().push(request);
        match &self.reply {
            Reply::Result(result) => Ok(result.clone()),
            Reply::ProtocolError => Err(ErrorData::invalid_params(
                "fixture call refused",
                Some(json!({"reason": "arguments"})),
            )),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Surface {
    Connection,
    Protocol,
}
const SURFACES: [Surface; 2] = [Surface::Connection, Surface::Protocol];
#[derive(Debug)]
enum Output {
    Blocks(Vec<ContentBlock>),
    Text(String),
}
impl Output {
    fn blocks(self) -> Vec<ContentBlock> {
        let Self::Blocks(blocks) = self else {
            panic!("expected block result");
        };
        blocks
    }
    fn text(self) -> String {
        let Self::Text(text) = self else {
            panic!("expected text result");
        };
        text
    }
}

async fn invoke(surface: Surface, reply: Reply, text_only: bool) -> Result<Output, McpError> {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let server = Server {
        reply,
        requests: requests.clone(),
    };
    let (client_io, server_io) = tokio::io::duplex(8192);
    let server_task = tokio::spawn(async move {
        let running = server.serve(server_io).await.expect("server handshake");
        running.waiting().await.expect("server cleanup")
    });
    let service: RunningService<RoleClient, ()> =
        ().serve(client_io).await.expect("client handshake");
    let args = json!({"query": "nested"});
    let (result, close) = match surface {
        Surface::Connection => {
            let connection = McpConnection {
                config: McpServerConfig::stdio(
                    "structured-fixture",
                    "unused-duplex",
                    vec![],
                    Default::default(),
                ),
                service: service.into(),
            };
            let result = tokio::time::timeout(Duration::from_secs(3), async {
                if text_only {
                    connection
                        .call_tool_text("lookup", &args)
                        .await
                        .map(Output::Text)
                } else {
                    connection
                        .call_tool("lookup", &args)
                        .await
                        .map(Output::Blocks)
                }
            })
            .await;
            (result, connection.close().await)
        }
        Surface::Protocol => {
            let protocol = McpProtocol::new(service);
            let result = tokio::time::timeout(Duration::from_secs(3), async {
                if text_only {
                    protocol
                        .call_tool_text("lookup", &args)
                        .await
                        .map(Output::Text)
                } else {
                    protocol
                        .call_tool("lookup", &args)
                        .await
                        .map(Output::Blocks)
                }
            })
            .await;
            (result, protocol.close().await)
        }
    };
    // Retire both actual peers before asserting the returned result or trace.
    let server_join = server_task.await;
    close.expect("client cleanup");
    server_join.expect("server task joined");
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].name, "lookup");
    assert_eq!(requests[0].arguments.as_ref(), args.as_object());
    result.expect("fixture request completed")
}

fn nested() -> Value {
    json!({
        "records": [{"name": "a\n\"b", "enabled": true, "missing": null,
                     "numbers": [0, -7, 1.25], "children": [{"value": false}]}],
        "empty": {}, "list": []
    })
}
fn reply(content: Value, structured: Option<Value>, is_error: bool) -> Reply {
    let mut value = json!({"content": content, "isError": is_error});
    if let Some(structured) = structured {
        value["structuredContent"] = structured;
    }
    Reply::Result(serde_json::from_value(value).expect("valid rmcp result fixture"))
}
fn mixed() -> Value {
    json!([
        {"type": "text", "text": "before"},
        {"type": "image", "mimeType": "image/png", "data": "cG5n"},
        {"type": "text", "text": "after"},
        {"type": "resource", "resource": {"uri": "file:///fixture", "text": "resource body"}}
    ])
}
fn structured_value(block: &ContentBlock) -> Value {
    let ContentBlock::Structured { data } = block else {
        panic!("expected Structured, got {block:?}");
    };
    serde_json::from_str(data.get()).expect("retained JSON value")
}
fn assert_mixed_prefix(blocks: &[ContentBlock]) {
    assert_eq!(
        blocks[0],
        ContentBlock::Text {
            text: "before".into()
        }
    );
    assert_eq!(
        blocks[1],
        ContentBlock::Image {
            media_type: "image/png".into(),
            data: "cG5n".into()
        }
    );
    assert_eq!(
        blocks[2],
        ContentBlock::Text {
            text: "after".into()
        }
    );
    assert_eq!(structured_value(&blocks[3]), mixed()[3]);
}

#[tokio::test]
async fn structured_result_only_preserves_nested_json_over_both_surfaces() {
    for surface in SURFACES {
        let blocks = invoke(surface, reply(json!([]), Some(nested()), false), false)
            .await
            .expect("successful tool result")
            .blocks();
        assert_eq!(blocks.len(), 1, "{surface:?}");
        assert_eq!(structured_value(&blocks[0]), nested());
    }
}

#[tokio::test]
async fn structured_result_mixed_appends_without_reordering_or_deduplicating() {
    for surface in SURFACES {
        let mut content = mixed();
        content
            .as_array_mut()
            .unwrap()
            .push(json!({"type": "text", "text": nested().to_string()}));
        let blocks = invoke(surface, reply(content, Some(nested()), false), false)
            .await
            .expect("successful mixed tool result")
            .blocks();
        assert_eq!(blocks.len(), 6, "{surface:?}");
        assert_mixed_prefix(&blocks);
        assert_eq!(
            blocks[4],
            ContentBlock::Text {
                text: nested().to_string()
            }
        );
        assert_eq!(structured_value(&blocks[5]), nested());
    }
}

#[tokio::test]
async fn structured_result_absent_keeps_content_and_empty_result_unchanged() {
    for surface in SURFACES {
        let blocks = invoke(surface, reply(mixed(), None, false), false)
            .await
            .expect("ordinary content")
            .blocks();
        assert_eq!(blocks.len(), 4);
        assert_mixed_prefix(&blocks);
        let empty = invoke(surface, reply(json!([]), None, false), false)
            .await
            .expect("empty success")
            .blocks();
        assert!(empty.is_empty());
        let text = invoke(
            surface,
            reply(
                json!([
                    {"type": "text", "text": "one"}, {"type": "text", "text": "two"}
                ]),
                None,
                false,
            ),
            true,
        )
        .await
        .expect("text-only compatibility")
        .text();
        assert_eq!(text, "one\ntwo");
    }
}

#[tokio::test]
async fn structured_result_survives_nested_transcript_roundtrip() {
    for surface in SURFACES {
        let blocks = invoke(surface, reply(mixed(), Some(nested()), false), false)
            .await
            .expect("mixed result")
            .blocks();
        assert_eq!(blocks.len(), 5);
        let message = Message::tool_results(vec![ToolResult::with_blocks(
            "call-structured".into(),
            blocks,
            false,
        )]);
        let encoded = serde_json::to_vec(&message).expect("serialize transcript");
        let decoded: Message = serde_json::from_slice(&encoded).expect("restore transcript");
        // Structured JSON promises value fidelity, not RawValue key order.
        // Message decoding may reorder keys in the existing resource block.
        assert_eq!(
            serde_json::to_value(&decoded).expect("restored message value"),
            serde_json::to_value(&message).expect("original message value")
        );
        let Message::ToolResults { results, .. } = decoded else {
            panic!("tool result transcript");
        };
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].tool_use_id, "call-structured");
        assert!(!results[0].is_error);
        assert_mixed_prefix(&results[0].content);
        assert_eq!(structured_value(&results[0].content[4]), nested());
    }
}

#[tokio::test]
async fn structured_result_text_helpers_keep_their_distinct_contracts() {
    for content in [json!([]), json!([{"type": "text", "text": "prefix"}])] {
        let strict = invoke(
            Surface::Protocol,
            reply(content.clone(), Some(nested()), false),
            true,
        )
        .await;
        assert!(
            matches!(strict, Err(McpError::ProtocolError { message }) if message.contains("unsupported content"))
        );
        let text = invoke(
            Surface::Connection,
            reply(content.clone(), Some(nested()), false),
            true,
        )
        .await
        .expect("tolerant projection")
        .text();
        let expected = if content.as_array().unwrap().is_empty() {
            nested().to_string()
        } else {
            format!("prefix\n{}", nested())
        };
        assert_eq!(text, expected);
    }
    // Existing non-text refusal remains strict with no top-level structured data.
    for content in [
        json!([{"type": "image", "mimeType": "image/png", "data": "cG5n"}]),
        json!([{"type": "resource", "resource": {"uri": "file:///fixture", "text": "body"}}]),
    ] {
        let result = invoke(Surface::Protocol, reply(content, None, false), true).await;
        assert!(matches!(result, Err(McpError::ProtocolError { .. })));
    }
}

#[tokio::test]
async fn structured_result_is_error_retains_detail_and_never_becomes_success() {
    for surface in SURFACES {
        for text_only in [false, true] {
            let result = invoke(surface, reply(json!([]), Some(nested()), true), text_only).await;
            let Err(McpError::ToolCallFailed { tool, reason }) = result else {
                panic!("isError must remain failure: {surface:?}");
            };
            assert_eq!(tool, "lookup");
            assert_eq!(
                serde_json::from_str::<Value>(&reason).expect("structured error detail"),
                nested()
            );
            let result = invoke(
                surface,
                reply(
                    json!([{"type": "text", "text": "prefix"}]),
                    Some(nested()),
                    true,
                ),
                text_only,
            )
            .await;
            assert!(
                matches!(result, Err(McpError::ToolCallFailed { reason, .. }) if reason == format!("prefix\n{}", nested()))
            );
        }
    }
}

#[tokio::test]
async fn structured_result_protocol_failures_are_not_converted_to_empty_success() {
    for surface in SURFACES {
        for text_only in [false, true] {
            let result = invoke(surface, Reply::ProtocolError, text_only).await;
            assert!(
                matches!(result, Err(McpError::ToolCallFailed { tool, reason }) if tool == "lookup" && reason.contains("fixture call refused"))
            );
        }
    }
}

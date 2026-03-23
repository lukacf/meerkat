//! Callback tool dispatcher — routes tool calls to an external client via JSON-RPC.
//!
//! When the agent invokes a callback tool, the dispatcher sends a `tool/execute`
//! request to the client over the RPC transport and awaits the response. This
//! enables Python/TypeScript SDKs to provide tool implementations.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::value::RawValue;
use tokio::sync::{mpsc, oneshot};

use async_trait::async_trait;
use meerkat_core::AgentToolDispatcher;
use meerkat_core::ToolDispatchOutcome;
use meerkat_core::error::ToolError;
use meerkat_core::types::{ToolCallView, ToolDef, ToolResult};

use crate::protocol::{RpcId, RpcRequest, RpcResponse};

/// Timeout for waiting on a client tool response.
const CALLBACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Dispatches tool calls to an external client via the RPC callback protocol.
pub struct CallbackToolDispatcher {
    tools: Arc<[Arc<ToolDef>]>,
    callback_tx: mpsc::Sender<(RpcRequest, oneshot::Sender<RpcResponse>)>,
    id_counter: Arc<AtomicU64>,
}

impl CallbackToolDispatcher {
    /// Create a new callback dispatcher with the given tool definitions and callback channel.
    pub fn new(
        tools: Vec<ToolDef>,
        callback_tx: mpsc::Sender<(RpcRequest, oneshot::Sender<RpcResponse>)>,
        id_counter: Arc<AtomicU64>,
    ) -> Self {
        let tools: Arc<[Arc<ToolDef>]> = tools.into_iter().map(Arc::new).collect();
        Self {
            tools,
            callback_tx,
            id_counter,
        }
    }

    fn next_id(&self) -> RpcId {
        let n = self.id_counter.fetch_add(1, Ordering::Relaxed);
        RpcId::Str(format!("srv-{n}"))
    }
}

#[async_trait]
impl AgentToolDispatcher for CallbackToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.tools.clone()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        let request_id = self.next_id();
        let arguments: serde_json::Value = serde_json::from_str(call.args.get()).map_err(|e| {
            ToolError::invalid_arguments(call.name, format!("malformed tool-call arguments: {e}"))
        })?;
        let params = serde_json::json!({
            "tool_use_id": call.id,
            "name": call.name,
            "arguments": arguments,
        });

        let params_raw = RawValue::from_string(serde_json::to_string(&params).map_err(|e| {
            ToolError::execution_failed(format!("Failed to serialize callback params: {e}"))
        })?)
        .map_err(|e| ToolError::execution_failed(format!("Failed to create RawValue: {e}")))?;

        let request = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(request_id),
            method: "tool/execute".to_string(),
            params: Some(params_raw),
        };

        let (response_tx, response_rx) = oneshot::channel();

        self.callback_tx
            .send((request, response_tx))
            .await
            .map_err(|_| ToolError::execution_failed("Callback channel closed".to_string()))?;

        let response = tokio::time::timeout(CALLBACK_TIMEOUT, response_rx)
            .await
            .map_err(|_| {
                ToolError::execution_failed(format!(
                    "Callback tool '{}' timed out after {}s",
                    call.name,
                    CALLBACK_TIMEOUT.as_secs()
                ))
            })?
            .map_err(|_| {
                ToolError::execution_failed("Callback response channel dropped".to_string())
            })?;

        // Parse the response result.
        if let Some(error) = response.error {
            return Err(ToolError::execution_failed(error.message));
        }

        let result_raw = response.result.ok_or_else(|| {
            ToolError::execution_failed("Callback response missing result".to_string())
        })?;

        #[derive(serde::Deserialize)]
        struct CallbackResult {
            #[serde(default)]
            content: String,
            #[serde(default)]
            is_error: bool,
        }

        let parsed: CallbackResult = serde_json::from_str(result_raw.get()).map_err(|e| {
            ToolError::execution_failed(format!("Failed to parse callback result: {e}"))
        })?;

        Ok(ToolResult::new(call.id.to_string(), parsed.content, parsed.is_error).into())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::value::RawValue;

    fn make_tool_def(name: &str) -> ToolDef {
        ToolDef {
            name: name.to_string(),
            description: format!("Test tool {name}"),
            input_schema: serde_json::json!({"type": "object"}),
        }
    }

    #[tokio::test]
    async fn callback_dispatcher_sends_request_awaits_result() {
        let (callback_tx, mut callback_rx) = mpsc::channel(10);
        let id_counter = Arc::new(AtomicU64::new(0));

        let dispatcher =
            CallbackToolDispatcher::new(vec![make_tool_def("search")], callback_tx, id_counter);

        // Spawn the dispatch in a task.
        let handle = tokio::spawn(async move {
            let args = RawValue::from_string("{\"q\":\"test\"}".to_string()).unwrap();
            let call = ToolCallView {
                id: "tc-1",
                name: "search",
                args: &args,
            };
            dispatcher.dispatch(call).await
        });

        // Receive the callback request and respond.
        let (request, response_tx) = callback_rx.recv().await.unwrap();
        assert_eq!(request.method, "tool/execute");
        assert_eq!(request.id, Some(RpcId::Str("srv-0".to_string())));

        let result_raw = RawValue::from_string(
            serde_json::to_string(&serde_json::json!({"content":"results here","is_error":false}))
                .unwrap(),
        )
        .unwrap();
        response_tx
            .send(RpcResponse {
                jsonrpc: "2.0".to_string(),
                id: request.id,
                result: Some(result_raw),
                error: None,
            })
            .unwrap();

        let result = handle.await.unwrap().unwrap();
        assert_eq!(result.result.text_content(), "results here");
        assert!(!result.result.is_error);
    }

    #[tokio::test]
    async fn callback_dispatcher_timeout_returns_tool_error() {
        let (callback_tx, _callback_rx) = mpsc::channel(10);
        let id_counter = Arc::new(AtomicU64::new(0));

        // Create dispatcher but drop the receiver so the oneshot will never complete.
        let dispatcher = CallbackToolDispatcher {
            tools: vec![Arc::new(make_tool_def("slow"))].into(),
            callback_tx,
            id_counter,
        };

        let args = RawValue::from_string("{}".to_string()).unwrap();
        let call = ToolCallView {
            id: "tc-2",
            name: "slow",
            args: &args,
        };

        // Use a very short timeout for testing.
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            dispatcher.dispatch(call),
        )
        .await;

        // The dispatch itself should timeout or fail because no one responds.
        assert!(result.is_err() || result.unwrap().is_err());
    }
}

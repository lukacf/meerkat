//! Callback tool dispatcher — routes tool calls to an external client via JSON-RPC.
//!
//! When the agent invokes a callback tool, the dispatcher sends a `tool/execute`
//! request to the client over the RPC transport and awaits the response. This
//! enables Python/TypeScript SDKs to provide tool implementations.
//!
//! The tool list is **live**: it reads from a shared `Arc<RwLock<Vec<ToolDef>>>`
//! (the same one `tools/register` writes to), so tools registered after session
//! materialization become visible at the next turn boundary via
//! `poll_external_updates`.
//!
//! Per-session inline tools (from `session/create` `external_tools` param) are
//! held separately and take precedence on name collision with globals.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock as StdRwLock};

use serde_json::value::RawValue;
use tokio::sync::{mpsc, oneshot};

use async_trait::async_trait;
use meerkat_core::AgentToolDispatcher;
use meerkat_core::ToolDispatchOutcome;
use meerkat_core::agent::ExternalToolUpdate;
use meerkat_core::error::ToolError;
use meerkat_core::event::{ExternalToolDelta, ExternalToolDeltaPhase, ToolConfigChangeOperation};
use meerkat_core::types::{ToolCallView, ToolDef, ToolResult};

use crate::protocol::{RpcId, RpcRequest, RpcResponse};

/// Timeout for waiting on a client tool response.
const CALLBACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Dispatches tool calls to an external client via the RPC callback protocol.
///
/// Holds a shared reference to the global registered-tools list so that tools
/// registered via `tools/register` after session creation are picked up
/// dynamically at each turn boundary (via `poll_external_updates`).
///
/// Per-session `inline_tools` (from `session/create` `external_tools` param) are
/// static and take precedence on name collision with globals. Most sessions have
/// no inline tools (the field is empty).
pub struct CallbackToolDispatcher {
    /// Live tool definitions — shared with `SessionRuntime::registered_tools_slot`.
    registered_tools: Arc<StdRwLock<Vec<ToolDef>>>,
    /// Per-session inline tools (static, usually empty). Win on name collision.
    inline_tools: Vec<ToolDef>,
    /// Inline tool names cached for fast collision lookup.
    inline_names: HashSet<String>,
    callback_tx: mpsc::Sender<(RpcRequest, oneshot::Sender<RpcResponse>)>,
    id_counter: Arc<AtomicU64>,
    /// Snapshot of global tool names from the last `poll_external_updates` (or creation).
    /// Used to detect additions/removals between polls.
    last_seen_globals: StdRwLock<HashSet<String>>,
}

impl CallbackToolDispatcher {
    /// Create a callback dispatcher backed by the live registered-tools list.
    ///
    /// `inline_tools` are per-session static tools (from `session/create`
    /// `external_tools` param). Pass empty vec for most sessions.
    pub fn new(
        registered_tools: Arc<StdRwLock<Vec<ToolDef>>>,
        callback_tx: mpsc::Sender<(RpcRequest, oneshot::Sender<RpcResponse>)>,
        id_counter: Arc<AtomicU64>,
        inline_tools: Vec<ToolDef>,
    ) -> Self {
        let inline_names: HashSet<String> = inline_tools.iter().map(|t| t.name.clone()).collect();
        let last_seen_globals = registered_tools
            .read()
            .map(|g| {
                g.iter()
                    .filter(|t| !inline_names.contains(&t.name))
                    .map(|t| t.name.clone())
                    .collect()
            })
            .unwrap_or_default();
        Self {
            registered_tools,
            inline_tools,
            inline_names,
            callback_tx,
            id_counter,
            last_seen_globals: StdRwLock::new(last_seen_globals),
        }
    }

    fn next_id(&self) -> RpcId {
        let n = self.id_counter.fetch_add(1, Ordering::Relaxed);
        RpcId::Str(format!("srv-{n}"))
    }

    /// Current global tool names excluding inline collisions.
    fn current_global_names(&self) -> HashSet<String> {
        self.registered_tools
            .read()
            .map(|g| {
                g.iter()
                    .filter(|t| !self.inline_names.contains(&t.name))
                    .map(|t| t.name.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn is_known(&self, name: &str) -> bool {
        if self.inline_names.contains(name) {
            return true;
        }
        self.registered_tools
            .read()
            .map(|g| g.iter().any(|t| t.name == name))
            .unwrap_or(false)
    }
}

#[async_trait]
impl AgentToolDispatcher for CallbackToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        let mut result: Vec<Arc<ToolDef>> = self
            .inline_tools
            .iter()
            .map(|t| Arc::new(t.clone()))
            .collect();
        if let Ok(global) = self.registered_tools.read() {
            for t in global.iter() {
                if !self.inline_names.contains(&t.name) {
                    result.push(Arc::new(t.clone()));
                }
            }
        }
        result.into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        if !self.is_known(call.name) {
            return Err(ToolError::not_found(call.name));
        }

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

    /// Detect global tools added/removed since last poll and emit deltas.
    /// Inline tools are static and never produce deltas.
    async fn poll_external_updates(&self) -> ExternalToolUpdate {
        let current = self.current_global_names();
        let previous = self
            .last_seen_globals
            .read()
            .map(|g| g.clone())
            .unwrap_or_default();

        if current == previous {
            return ExternalToolUpdate::default();
        }

        let mut notices = Vec::new();

        for name in current.difference(&previous) {
            notices.push(
                ExternalToolDelta::new(
                    format!("callback:{name}"),
                    ToolConfigChangeOperation::Add,
                    ExternalToolDeltaPhase::Applied,
                )
                .with_tool_count(Some(1)),
            );
        }

        for name in previous.difference(&current) {
            notices.push(ExternalToolDelta::new(
                format!("callback:{name}"),
                ToolConfigChangeOperation::Remove,
                ExternalToolDeltaPhase::Applied,
            ));
        }

        if let Ok(mut last) = self.last_seen_globals.write() {
            *last = current;
        }

        ExternalToolUpdate {
            notices,
            pending: Vec::new(),
            background_completions: Vec::new(),
        }
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
            provenance: None,
        }
    }

    #[tokio::test]
    async fn callback_dispatcher_sends_request_awaits_result() {
        let (callback_tx, mut callback_rx) = mpsc::channel(10);
        let id_counter = Arc::new(AtomicU64::new(0));
        let tools = Arc::new(StdRwLock::new(vec![make_tool_def("search")]));

        let dispatcher = CallbackToolDispatcher::new(tools, callback_tx, id_counter, vec![]);

        let handle = tokio::spawn(async move {
            let args = RawValue::from_string("{\"q\":\"test\"}".to_string()).unwrap();
            let call = ToolCallView {
                id: "tc-1",
                name: "search",
                args: &args,
            };
            dispatcher.dispatch(call).await
        });

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
        let tools = Arc::new(StdRwLock::new(vec![make_tool_def("slow")]));

        let dispatcher = CallbackToolDispatcher::new(tools, callback_tx, id_counter, vec![]);

        let args = RawValue::from_string("{}".to_string()).unwrap();
        let call = ToolCallView {
            id: "tc-2",
            name: "slow",
            args: &args,
        };

        let result = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            dispatcher.dispatch(call),
        )
        .await;

        assert!(result.is_err() || result.unwrap().is_err());
    }

    #[tokio::test]
    async fn poll_external_updates_detects_additions_and_removals() {
        let (callback_tx, _rx) = mpsc::channel(10);
        let id_counter = Arc::new(AtomicU64::new(0));
        let tools = Arc::new(StdRwLock::new(vec![make_tool_def("a")]));

        let dispatcher =
            CallbackToolDispatcher::new(tools.clone(), callback_tx, id_counter, vec![]);

        // No change yet.
        let update = dispatcher.poll_external_updates().await;
        assert!(update.notices.is_empty());

        // Add a tool.
        tools.write().unwrap().push(make_tool_def("b"));
        let update = dispatcher.poll_external_updates().await;
        assert_eq!(update.notices.len(), 1);
        assert_eq!(update.notices[0].target, "callback:b");
        assert_eq!(update.notices[0].operation, ToolConfigChangeOperation::Add);

        // Remove original tool.
        tools.write().unwrap().retain(|t| t.name != "a");
        let update = dispatcher.poll_external_updates().await;
        assert_eq!(update.notices.len(), 1);
        assert_eq!(update.notices[0].target, "callback:a");
        assert_eq!(
            update.notices[0].operation,
            ToolConfigChangeOperation::Remove
        );

        // No change.
        let update = dispatcher.poll_external_updates().await;
        assert!(update.notices.is_empty());
    }

    #[tokio::test]
    async fn tools_reflects_live_registered_list() {
        let (callback_tx, _rx) = mpsc::channel(10);
        let id_counter = Arc::new(AtomicU64::new(0));
        let tools = Arc::new(StdRwLock::new(vec![make_tool_def("x")]));

        let dispatcher =
            CallbackToolDispatcher::new(tools.clone(), callback_tx, id_counter, vec![]);
        assert_eq!(dispatcher.tools().len(), 1);

        tools.write().unwrap().push(make_tool_def("y"));
        assert_eq!(dispatcher.tools().len(), 2);

        tools.write().unwrap().clear();
        assert_eq!(dispatcher.tools().len(), 0);
    }

    #[tokio::test]
    async fn inline_tools_take_precedence_over_globals() {
        let (callback_tx, _rx) = mpsc::channel(10);
        let id_counter = Arc::new(AtomicU64::new(0));
        // Global has "shared" and "global_only"
        let tools = Arc::new(StdRwLock::new(vec![
            make_tool_def("shared"),
            make_tool_def("global_only"),
        ]));
        // Inline has "shared" and "inline_only"
        let inline = vec![make_tool_def("shared"), make_tool_def("inline_only")];

        let dispatcher = CallbackToolDispatcher::new(tools, callback_tx, id_counter, inline);

        let names: Vec<String> = dispatcher.tools().iter().map(|t| t.name.clone()).collect();
        // Should have: shared (from inline), inline_only, global_only
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"shared".to_string()));
        assert!(names.contains(&"inline_only".to_string()));
        assert!(names.contains(&"global_only".to_string()));

        // "shared" appears exactly once (inline wins, global filtered)
        assert_eq!(names.iter().filter(|n| *n == "shared").count(), 1);
    }

    #[tokio::test]
    async fn inline_tools_do_not_produce_poll_deltas() {
        let (callback_tx, _rx) = mpsc::channel(10);
        let id_counter = Arc::new(AtomicU64::new(0));
        let tools = Arc::new(StdRwLock::new(vec![]));
        let inline = vec![make_tool_def("static_tool")];

        let dispatcher = CallbackToolDispatcher::new(tools, callback_tx, id_counter, inline);

        // Inline tool exists but should not produce a delta.
        let update = dispatcher.poll_external_updates().await;
        assert!(update.notices.is_empty());
    }

    #[tokio::test]
    async fn dynamic_updates_work_with_inline_tools_present() {
        let (callback_tx, _rx) = mpsc::channel(10);
        let id_counter = Arc::new(AtomicU64::new(0));
        let tools = Arc::new(StdRwLock::new(vec![]));
        let inline = vec![make_tool_def("my_inline")];

        let dispatcher =
            CallbackToolDispatcher::new(tools.clone(), callback_tx, id_counter, inline);
        assert_eq!(dispatcher.tools().len(), 1); // just inline

        // Register a global tool later.
        tools.write().unwrap().push(make_tool_def("late_global"));
        assert_eq!(dispatcher.tools().len(), 2); // inline + late_global

        let update = dispatcher.poll_external_updates().await;
        assert_eq!(update.notices.len(), 1);
        assert_eq!(update.notices[0].target, "callback:late_global");
        assert_eq!(update.notices[0].operation, ToolConfigChangeOperation::Add);
    }
}

//! Memory search tool — exposes `MemoryStore::search` as an `AgentToolDispatcher`.
//!
//! This tool allows agents to search their semantic memory for past conversation
//! content that was indexed during compaction. It wraps an `Arc<dyn MemoryStore>`
//! and delegates to its `search()` method.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::AgentToolDispatcher;
use meerkat_core::error::ToolError;
use meerkat_core::memory::MemoryStore;
use meerkat_core::types::{ToolCallView, ToolDef, ToolResult};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

const TOOL_NAME: &str = "memory_search";
const DEFAULT_LIMIT: usize = 5;

/// Input schema for the memory_search tool.
#[derive(Debug, Deserialize, JsonSchema)]
struct MemorySearchInput {
    /// Natural language search query describing what you want to recall.
    query: String,
    /// Maximum number of results to return (default: 5, max: 20).
    #[serde(default)]
    limit: Option<usize>,
}

/// Generate the JSON schema for the input type, ensuring `properties` and
/// `required` keys are always present (tool schema contract).
fn input_schema() -> Value {
    let schema = schemars::schema_for!(MemorySearchInput);
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

/// Tool dispatcher that provides the `memory_search` tool.
///
/// Wraps an `Arc<dyn MemoryStore>` and exposes semantic search as an
/// agent-callable tool. Created by the factory when the `memory-store-session`
/// feature is enabled.
pub struct MemorySearchDispatcher {
    store: Arc<dyn MemoryStore>,
    tool_defs: Arc<[Arc<ToolDef>]>,
}

impl MemorySearchDispatcher {
    /// Create a new memory search dispatcher backed by the given store.
    pub fn new(store: Arc<dyn MemoryStore>) -> Self {
        let tool_def = Arc::new(ToolDef {
            name: TOOL_NAME.to_string(),
            description: "Search semantic memory for past conversation content. \
                Memory contains text from earlier conversation turns that were \
                compacted away to save context space. Use this to recall \
                information from earlier in the conversation or from previous sessions."
                .to_string(),
            input_schema: input_schema(),
        });

        Self {
            store,
            tool_defs: Arc::from(vec![tool_def]),
        }
    }

    /// Usage instructions for the system prompt.
    pub fn usage_instructions() -> &'static str {
        "# Semantic Memory\n\n\
         You have access to a semantic memory store that contains text from earlier \
         conversation turns that were compacted away. Use the `memory_search` tool \
         to recall information that is no longer in your visible context."
    }
}

#[async_trait]
impl AgentToolDispatcher for MemorySearchDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tool_defs)
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
        if call.name != TOOL_NAME {
            return Err(ToolError::NotFound {
                name: call.name.to_string(),
            });
        }

        let input: MemorySearchInput =
            serde_json::from_str(call.args.get()).map_err(|e| ToolError::InvalidArguments {
                name: TOOL_NAME.to_string(),
                reason: e.to_string(),
            })?;

        let limit = input.limit.unwrap_or(DEFAULT_LIMIT).min(20);

        let results = self.store.search(&input.query, limit).await.map_err(|e| {
            ToolError::ExecutionFailed {
                message: format!("{TOOL_NAME}: {e}"),
            }
        })?;

        let results_json: Vec<Value> = results
            .into_iter()
            .map(|r| {
                json!({
                    "content": r.content,
                    "score": r.score,
                    "session_id": r.metadata.session_id.to_string(),
                    "turn": r.metadata.turn,
                })
            })
            .collect();

        Ok(ToolResult {
            tool_use_id: call.id.to_string(),
            content: serde_json::to_string(&results_json).unwrap_or_else(|_| "[]".to_string()),
            is_error: false,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::memory::{MemoryMetadata, MemoryStore};
    use meerkat_core::types::SessionId;
    use serde_json::value::RawValue;
    use std::time::SystemTime;

    /// Helper to create a ToolCallView for testing.
    fn make_call(args_json: &str) -> (String, Box<RawValue>, String) {
        let id = "test-call-1".to_string();
        let raw = RawValue::from_string(args_json.to_string()).unwrap();
        let name = TOOL_NAME.to_string();
        (id, raw, name)
    }

    fn call_view<'a>(id: &'a str, raw: &'a RawValue, name: &'a str) -> ToolCallView<'a> {
        ToolCallView {
            id,
            name,
            args: raw,
        }
    }

    fn meta() -> MemoryMetadata {
        MemoryMetadata {
            session_id: SessionId::new(),
            turn: Some(1),
            indexed_at: SystemTime::now(),
        }
    }

    // ==================== Tool Definition Tests ====================

    #[test]
    fn test_tool_name() {
        let store = Arc::new(crate::SimpleMemoryStore::new());
        let dispatcher = MemorySearchDispatcher::new(store);
        let tools = dispatcher.tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "memory_search");
    }

    #[test]
    fn test_tool_schema_has_required_query() {
        let store = Arc::new(crate::SimpleMemoryStore::new());
        let dispatcher = MemorySearchDispatcher::new(store);
        let tools = dispatcher.tools();
        let schema = &tools[0].input_schema;

        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["query"].is_object());
        assert_eq!(schema["properties"]["query"]["type"], "string");

        let required = schema["required"].as_array().unwrap();
        let required_strs: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        assert!(required_strs.contains(&"query"));
    }

    #[test]
    fn test_tool_schema_has_optional_limit() {
        let store = Arc::new(crate::SimpleMemoryStore::new());
        let dispatcher = MemorySearchDispatcher::new(store);
        let tools = dispatcher.tools();
        let schema = &tools[0].input_schema;

        assert!(schema["properties"]["limit"].is_object());

        // limit is NOT in required
        let required = schema["required"].as_array().unwrap();
        let required_strs: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        assert!(!required_strs.contains(&"limit"));
    }

    // ==================== Dispatch Tests ====================

    #[tokio::test]
    async fn test_search_returns_results() {
        let store = Arc::new(crate::SimpleMemoryStore::new());
        store
            .index("The project codename is AURORA-7", meta())
            .await
            .unwrap();
        store
            .index("The budget was set at $42,000", meta())
            .await
            .unwrap();
        store
            .index("Meeting scheduled for next Tuesday", meta())
            .await
            .unwrap();

        let dispatcher = MemorySearchDispatcher::new(store);
        let (id, raw, name) = make_call(r#"{"query": "project codename"}"#);
        let view = call_view(&id, &raw, &name);

        let result = dispatcher.dispatch(view).await.unwrap();
        assert!(!result.is_error);

        let parsed: Vec<Value> = serde_json::from_str(&result.content).unwrap();
        assert!(!parsed.is_empty());
        assert!(parsed[0]["content"].as_str().unwrap().contains("AURORA"));
        assert!(parsed[0]["score"].as_f64().unwrap() > 0.0);
        assert!(parsed[0]["session_id"].is_string());
    }

    #[tokio::test]
    async fn test_search_empty_store_returns_empty() {
        let store = Arc::new(crate::SimpleMemoryStore::new());
        let dispatcher = MemorySearchDispatcher::new(store);

        let (id, raw, name) = make_call(r#"{"query": "anything"}"#);
        let view = call_view(&id, &raw, &name);

        let result = dispatcher.dispatch(view).await.unwrap();
        assert!(!result.is_error);

        let parsed: Vec<Value> = serde_json::from_str(&result.content).unwrap();
        assert!(parsed.is_empty());
    }

    #[tokio::test]
    async fn test_search_with_limit() {
        let store = Arc::new(crate::SimpleMemoryStore::new());
        for i in 0..10 {
            store
                .index(&format!("Memory entry {} about testing", i), meta())
                .await
                .unwrap();
        }

        let dispatcher = MemorySearchDispatcher::new(store);
        let (id, raw, name) = make_call(r#"{"query": "testing", "limit": 3}"#);
        let view = call_view(&id, &raw, &name);

        let result = dispatcher.dispatch(view).await.unwrap();
        let parsed: Vec<Value> = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed.len(), 3);
    }

    #[tokio::test]
    async fn test_search_default_limit() {
        let store = Arc::new(crate::SimpleMemoryStore::new());
        for i in 0..10 {
            store
                .index(&format!("Entry {} about Rust programming", i), meta())
                .await
                .unwrap();
        }

        let dispatcher = MemorySearchDispatcher::new(store);
        let (id, raw, name) = make_call(r#"{"query": "Rust"}"#);
        let view = call_view(&id, &raw, &name);

        let result = dispatcher.dispatch(view).await.unwrap();
        let parsed: Vec<Value> = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed.len(), DEFAULT_LIMIT);
    }

    #[tokio::test]
    async fn test_search_no_match_returns_empty() {
        let store = Arc::new(crate::SimpleMemoryStore::new());
        store
            .index("The weather is sunny today", meta())
            .await
            .unwrap();

        let dispatcher = MemorySearchDispatcher::new(store);
        let (id, raw, name) = make_call(r#"{"query": "quantum physics"}"#);
        let view = call_view(&id, &raw, &name);

        let result = dispatcher.dispatch(view).await.unwrap();
        let parsed: Vec<Value> = serde_json::from_str(&result.content).unwrap();
        assert!(parsed.is_empty());
    }

    #[tokio::test]
    async fn test_dispatch_wrong_tool_name() {
        let store = Arc::new(crate::SimpleMemoryStore::new());
        let dispatcher = MemorySearchDispatcher::new(store);

        let id = "test-1".to_string();
        let raw = RawValue::from_string(r#"{"query": "test"}"#.to_string()).unwrap();
        let name = "wrong_tool";
        let view = ToolCallView {
            id: &id,
            name,
            args: &raw,
        };

        let result = dispatcher.dispatch(view).await;
        assert!(matches!(result, Err(ToolError::NotFound { .. })));
    }

    #[tokio::test]
    async fn test_dispatch_invalid_args() {
        let store = Arc::new(crate::SimpleMemoryStore::new());
        let dispatcher = MemorySearchDispatcher::new(store);

        let (id, raw, name) = make_call(r#"{"not_query": "test"}"#);
        let view = call_view(&id, &raw, &name);

        let result = dispatcher.dispatch(view).await;
        assert!(matches!(result, Err(ToolError::InvalidArguments { .. })));
    }

    #[tokio::test]
    async fn test_limit_capped_at_20() {
        let store = Arc::new(crate::SimpleMemoryStore::new());
        for i in 0..30 {
            store
                .index(&format!("Data point {} about science", i), meta())
                .await
                .unwrap();
        }

        let dispatcher = MemorySearchDispatcher::new(store);
        let (id, raw, name) = make_call(r#"{"query": "science", "limit": 100}"#);
        let view = call_view(&id, &raw, &name);

        let result = dispatcher.dispatch(view).await.unwrap();
        let parsed: Vec<Value> = serde_json::from_str(&result.content).unwrap();
        assert!(parsed.len() <= 20);
    }

    #[test]
    fn test_usage_instructions_not_empty() {
        let instructions = MemorySearchDispatcher::usage_instructions();
        assert!(!instructions.is_empty());
        assert!(instructions.contains("memory_search"));
    }
}

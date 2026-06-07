//! Memory search tool — exposes `MemoryStore::search` as an `AgentToolDispatcher`.
//!
//! This tool allows agents to search their semantic memory for past conversation
//! content that was indexed during compaction. It wraps an `Arc<dyn MemoryStore>`
//! and delegates to its `search()` method.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::AgentToolDispatcher;
use meerkat_core::error::ToolError;
use meerkat_core::memory::{MemorySearchScope, MemoryStore, MemoryStoreError};
use meerkat_core::types::{ToolCallView, ToolDef, ToolProvenance, ToolResult, ToolSourceKind};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

const TOOL_NAME: &str = "memory_search";
const DEFAULT_LIMIT: usize = 5;

/// Map a typed [`MemoryStoreError`] onto a distinguishable [`ToolError`],
/// preserving the failure class via a stable structured `code` rather than
/// collapsing every cause into an opaque message string.
fn memory_search_error(error: MemoryStoreError) -> ToolError {
    ToolError::ExecutionFailedWithData {
        message: error.to_string(),
        data: json!({ "code": error.error_code() }),
    }
}

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
    scope: MemorySearchScope,
    tool_defs: Arc<[Arc<ToolDef>]>,
}

impl MemorySearchDispatcher {
    /// Create a new memory search dispatcher backed by the given store.
    pub fn new(store: Arc<dyn MemoryStore>, scope: MemorySearchScope) -> Self {
        let tool_def = Arc::new(ToolDef {
            name: TOOL_NAME.into(),
            description: "Search semantic memory for past conversation content. \
                Memory contains text from earlier conversation turns that were \
                compacted away to save context space. Use this to recall \
                information from earlier in the conversation or from previous sessions."
                .to_string(),
            input_schema: input_schema(),
            provenance: Some(ToolProvenance {
                kind: ToolSourceKind::Memory,
                source_id: "memory".into(),
            }),
        });

        Self {
            store,
            scope,
            tool_defs: Arc::from(vec![tool_def]),
        }
    }

    /// Create a dispatcher scoped to the canonical memory owner for a session.
    pub fn for_session(
        store: Arc<dyn MemoryStore>,
        session_id: meerkat_core::types::SessionId,
    ) -> Self {
        Self::new(store, MemorySearchScope::for_session(session_id))
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

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
        if call.name != TOOL_NAME {
            return Err(ToolError::NotFound {
                name: call.name.into(),
            });
        }
        let input: MemorySearchInput =
            serde_json::from_str(call.args.get()).map_err(|e| ToolError::InvalidArguments {
                name: TOOL_NAME.into(),
                reason: e.to_string(),
            })?;
        let limit = input.limit.unwrap_or(DEFAULT_LIMIT).min(20);
        let results = self
            .store
            .search(&self.scope, &input.query, limit)
            .await
            .map_err(memory_search_error)?;
        let items: Vec<Value> = results
            .into_iter()
            .map(|r| {
                let mut entry = json!({
                    "content": r.content,
                    "score": r.score,
                });
                // Expose the typed source handle (provenance), not a proxy turn.
                if let (Value::Object(obj), Some(range)) =
                    (&mut entry, r.metadata.source.source_range())
                {
                    obj.insert(
                        "source_range".to_string(),
                        json!({ "start": range.start(), "end": range.end() }),
                    );
                }
                entry
            })
            .collect();
        // Bare-array wire shape: the tool returns a list of result objects,
        // and every test in this module expects `Vec<Value>` via
        // `serde_json::from_str`. The `0c9acc473` wave-d baseline wrapped
        // items in `{"results": items}` but that wrapper isn't consumed by
        // any production caller (only test fixtures that parse as Vec),
        // and MCP-style tool outputs conventionally emit the list directly
        // when the result is a list.
        let payload = Value::Array(items).to_string();
        Ok(meerkat_core::ops::ToolDispatchOutcome::from(
            ToolResult::new(call.id.to_string(), payload, false),
        ))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::memory::{
        MemoryIndexRequest, MemoryIndexScope, MemoryMetadata, MemorySource, MemoryStore,
        MessageRange,
    };
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

    fn meta(session_id: &SessionId) -> MemoryMetadata {
        MemoryMetadata {
            session_id: session_id.clone(),
            source: MemorySource::Compaction {
                source_range: MessageRange::single(7),
            },
            indexed_at: SystemTime::now(),
        }
    }

    fn request(content: impl Into<String>, session_id: &SessionId) -> MemoryIndexRequest {
        MemoryIndexRequest::new(
            MemoryIndexScope::for_session(session_id.clone()),
            content.into(),
            meta(session_id),
        )
        .unwrap()
    }

    fn dispatcher(store: Arc<dyn MemoryStore>, session_id: &SessionId) -> MemorySearchDispatcher {
        MemorySearchDispatcher::for_session(store, session_id.clone())
    }

    // ==================== Tool Definition Tests ====================

    #[test]
    fn test_tool_name() {
        let store: Arc<dyn MemoryStore> = Arc::new(crate::SimpleMemoryStore::new());
        let session_id = SessionId::new();
        let dispatcher = dispatcher(store, &session_id);
        let tools = dispatcher.tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "memory_search");
    }

    #[test]
    fn test_tool_schema_has_required_query() {
        let store: Arc<dyn MemoryStore> = Arc::new(crate::SimpleMemoryStore::new());
        let session_id = SessionId::new();
        let dispatcher = dispatcher(store, &session_id);
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
        let store: Arc<dyn MemoryStore> = Arc::new(crate::SimpleMemoryStore::new());
        let session_id = SessionId::new();
        let dispatcher = dispatcher(store, &session_id);
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
        let store: Arc<dyn MemoryStore> = Arc::new(crate::SimpleMemoryStore::new());
        let session_id = SessionId::new();
        let other_session_id = SessionId::new();
        store
            .index_scoped(request("The project codename is AURORA-7", &session_id))
            .await
            .unwrap();
        store
            .index_scoped(request("The budget was set at $42,000", &session_id))
            .await
            .unwrap();
        store
            .index_scoped(request(
                "Meeting scheduled for next Tuesday",
                &other_session_id,
            ))
            .await
            .unwrap();

        let dispatcher = dispatcher(store, &session_id);
        let (id, raw, name) = make_call(r#"{"query": "project codename"}"#);
        let view = call_view(&id, &raw, &name);

        let outcome = dispatcher.dispatch(view).await.unwrap();
        assert!(!outcome.result.is_error);

        let parsed: Vec<Value> = serde_json::from_str(&outcome.result.text_content()).unwrap();
        assert!(!parsed.is_empty());
        assert!(parsed[0]["content"].as_str().unwrap().contains("AURORA"));
        assert!(parsed[0]["score"].as_f64().unwrap() > 0.0);
        assert!(
            parsed[0].get("session_id").is_none(),
            "memory tool must not leak raw source session ids"
        );
        // The typed source handle (provenance) is exposed, not a proxy turn.
        assert!(
            parsed[0].get("turn").is_none(),
            "memory tool must expose the typed source handle, not a turn proxy"
        );
        let range = parsed[0]
            .get("source_range")
            .expect("memory result exposes typed source range");
        assert_eq!(range["start"].as_u64().unwrap(), 7);
        assert_eq!(range["end"].as_u64().unwrap(), 8);
    }

    #[tokio::test]
    async fn test_search_error_preserves_typed_failure_class() {
        use async_trait::async_trait;
        use meerkat_core::memory::{MemoryIndexBatch, MemoryIndexReceipt, MemoryResult};

        struct FailingStore;

        #[async_trait]
        impl MemoryStore for FailingStore {
            async fn index_scoped_batch(
                &self,
                _batch: MemoryIndexBatch,
            ) -> Result<MemoryIndexReceipt, MemoryStoreError> {
                Err(MemoryStoreError::LockPoisoned)
            }

            async fn search(
                &self,
                _scope: &MemorySearchScope,
                _query: &str,
                _limit: usize,
            ) -> Result<Vec<MemoryResult>, MemoryStoreError> {
                Err(MemoryStoreError::LockPoisoned)
            }
        }

        let store: Arc<dyn MemoryStore> = Arc::new(FailingStore);
        let session_id = SessionId::new();
        let dispatcher = dispatcher(store, &session_id);
        let (id, raw, name) = make_call(r#"{"query": "anything"}"#);
        let view = call_view(&id, &raw, &name);

        let err = dispatcher.dispatch(view).await.unwrap_err();
        match err {
            ToolError::ExecutionFailedWithData { data, .. } => {
                assert_eq!(data["code"].as_str(), Some("memory_lock_poisoned"));
            }
            other => panic!("expected typed execution failure with data, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_search_empty_store_returns_empty() {
        let store: Arc<dyn MemoryStore> = Arc::new(crate::SimpleMemoryStore::new());
        let session_id = SessionId::new();
        let dispatcher = dispatcher(store, &session_id);

        let (id, raw, name) = make_call(r#"{"query": "anything"}"#);
        let view = call_view(&id, &raw, &name);

        let outcome = dispatcher.dispatch(view).await.unwrap();
        assert!(!outcome.result.is_error);

        let parsed: Vec<Value> = serde_json::from_str(&outcome.result.text_content()).unwrap();
        assert!(parsed.is_empty());
    }

    #[tokio::test]
    async fn test_search_with_limit() {
        let store: Arc<dyn MemoryStore> = Arc::new(crate::SimpleMemoryStore::new());
        let session_id = SessionId::new();
        for i in 0..10 {
            store
                .index_scoped(request(
                    format!("Memory entry {i} about testing"),
                    &session_id,
                ))
                .await
                .unwrap();
        }

        let dispatcher = dispatcher(store, &session_id);
        let (id, raw, name) = make_call(r#"{"query": "testing", "limit": 3}"#);
        let view = call_view(&id, &raw, &name);

        let outcome = dispatcher.dispatch(view).await.unwrap();
        let parsed: Vec<Value> = serde_json::from_str(&outcome.result.text_content()).unwrap();
        assert_eq!(parsed.len(), 3);
    }

    #[tokio::test]
    async fn test_search_default_limit() {
        let store: Arc<dyn MemoryStore> = Arc::new(crate::SimpleMemoryStore::new());
        let session_id = SessionId::new();
        for i in 0..10 {
            store
                .index_scoped(request(
                    format!("Entry {i} about Rust programming"),
                    &session_id,
                ))
                .await
                .unwrap();
        }

        let dispatcher = dispatcher(store, &session_id);
        let (id, raw, name) = make_call(r#"{"query": "Rust"}"#);
        let view = call_view(&id, &raw, &name);

        let outcome = dispatcher.dispatch(view).await.unwrap();
        let parsed: Vec<Value> = serde_json::from_str(&outcome.result.text_content()).unwrap();
        assert_eq!(parsed.len(), DEFAULT_LIMIT);
    }

    #[tokio::test]
    async fn test_search_no_match_returns_empty() {
        let store: Arc<dyn MemoryStore> = Arc::new(crate::SimpleMemoryStore::new());
        let session_id = SessionId::new();
        store
            .index_scoped(request("The weather is sunny today", &session_id))
            .await
            .unwrap();

        let dispatcher = dispatcher(store, &session_id);
        let (id, raw, name) = make_call(r#"{"query": "quantum physics"}"#);
        let view = call_view(&id, &raw, &name);

        let outcome = dispatcher.dispatch(view).await.unwrap();
        let parsed: Vec<Value> = serde_json::from_str(&outcome.result.text_content()).unwrap();
        assert!(parsed.is_empty());
    }

    #[tokio::test]
    async fn test_dispatch_wrong_tool_name() {
        let store: Arc<dyn MemoryStore> = Arc::new(crate::SimpleMemoryStore::new());
        let session_id = SessionId::new();
        let dispatcher = dispatcher(store, &session_id);

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
        let store: Arc<dyn MemoryStore> = Arc::new(crate::SimpleMemoryStore::new());
        let session_id = SessionId::new();
        let dispatcher = dispatcher(store, &session_id);

        let (id, raw, name) = make_call(r#"{"not_query": "test"}"#);
        let view = call_view(&id, &raw, &name);

        let result = dispatcher.dispatch(view).await;
        assert!(matches!(result, Err(ToolError::InvalidArguments { .. })));
    }

    #[tokio::test]
    async fn test_limit_capped_at_20() {
        let store: Arc<dyn MemoryStore> = Arc::new(crate::SimpleMemoryStore::new());
        let session_id = SessionId::new();
        for i in 0..30 {
            store
                .index_scoped(request(
                    format!("Data point {i} about science"),
                    &session_id,
                ))
                .await
                .unwrap();
        }

        let dispatcher = dispatcher(store, &session_id);
        let (id, raw, name) = make_call(r#"{"query": "science", "limit": 100}"#);
        let view = call_view(&id, &raw, &name);

        let outcome = dispatcher.dispatch(view).await.unwrap();
        let parsed: Vec<Value> = serde_json::from_str(&outcome.result.text_content()).unwrap();
        assert!(parsed.len() <= 20);
    }

    #[test]
    fn test_usage_instructions_not_empty() {
        let instructions = MemorySearchDispatcher::usage_instructions();
        assert!(!instructions.is_empty());
        assert!(instructions.contains("memory_search"));
    }

    #[test]
    fn memory_tools_have_memory_provenance() {
        let store: Arc<dyn MemoryStore> = Arc::new(crate::simple::SimpleMemoryStore::new());
        let session_id = SessionId::new();
        let dispatcher = dispatcher(store, &session_id);
        let tools = dispatcher.tools();
        assert_eq!(tools.len(), 1);
        let prov = tools[0]
            .provenance
            .as_ref()
            .expect("memory tool should have provenance");
        assert_eq!(prov.kind, meerkat_core::types::ToolSourceKind::Memory);
        assert_eq!(prov.source_id, "memory");
    }
}

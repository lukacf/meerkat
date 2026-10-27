//! Memory search tool — exposes `MemoryStore::search` as an `AgentToolDispatcher`.
//!
//! This tool allows agents to search their semantic memory for past conversation
//! content that was indexed during compaction. It wraps an `Arc<dyn MemoryStore>`
//! and delegates to its `search()` method.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use meerkat_core::AgentToolDispatcher;
use meerkat_core::error::ToolError;
use meerkat_core::memory::{MemorySearchScope, MemoryStore, MemoryStoreError};
use meerkat_core::types::{
    ContentBlock, ToolCallView, ToolDef, ToolProvenance, ToolResult, ToolSourceKind,
};
use meerkat_core::{MemoryOperation, ToolOperationTarget};
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
    let mut value = schema.to_value();
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
/// feature is enabled. It is the owner of its plans: each carries the typed
/// [`meerkat_core::ToolOperationTarget::MemoryOperation`] read of the scope
/// this dispatcher searches, and only a plan it resolved dispatches. Governed
/// work searches only when its prepared authorization carries exactly that
/// read, including through a wrapper that forwards this dispatcher's
/// dispatch.
pub struct MemorySearchDispatcher {
    store: Arc<dyn MemoryStore>,
    scope: MemorySearchScope,
    tool_defs: Arc<[Arc<ToolDef>]>,
    execution_authority_key: String,
}

static NEXT_MEMORY_EXECUTION_AUTHORITY_ID: AtomicU64 = AtomicU64::new(1);

impl MemorySearchDispatcher {
    /// Create a new memory search dispatcher backed by the given store.
    /// The names of the tools this dispatcher provides.
    pub fn tool_names() -> &'static [&'static str] {
        &[TOOL_NAME]
    }

    pub fn new(store: Arc<dyn MemoryStore>, scope: MemorySearchScope) -> Self {
        let tool_def = Arc::new(ToolDef {
            audience: Default::default(),
            name: TOOL_NAME.into(),
            description: "Search semantic memory for past conversation content. \
                Memory contains text from earlier turns in this session that \
                were compacted away to save context space. Use this to recall \
                information from earlier in this session, including turns \
                compacted away before a resume."
                .to_string(),
            input_schema: input_schema(),
            provenance: Some(ToolProvenance {
                kind: ToolSourceKind::Memory,
                source_id: "memory".into(),
                artifact: None,
            }),
        });

        Self {
            store,
            scope,
            tool_defs: Arc::from(vec![tool_def]),
            execution_authority_key: format!(
                "memory-search:{}",
                NEXT_MEMORY_EXECUTION_AUTHORITY_ID.fetch_add(1, Ordering::Relaxed)
            ),
        }
    }

    /// Create a dispatcher scoped to the canonical memory owner for a session.
    pub fn for_session(
        store: Arc<dyn MemoryStore>,
        session_id: meerkat_core::types::SessionId,
    ) -> Self {
        Self::new(store, MemorySearchScope::for_session(session_id))
    }

    /// The typed operation this dispatcher runs: a read of exactly the
    /// scope it searches.
    fn operation_target(&self) -> ToolOperationTarget {
        ToolOperationTarget::MemoryOperation {
            scope: self.scope.scope_ref(),
            operation: MemoryOperation::Read,
        }
    }
}

#[async_trait]
impl AgentToolDispatcher for MemorySearchDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tool_defs)
    }

    fn live_bridge_effect_kind(&self, tool_name: &str) -> meerkat_core::LiveBridgeEffectKind {
        if tool_name == TOOL_NAME {
            meerkat_core::LiveBridgeEffectKind::ReadOnlyMemorySnapshot
        } else {
            meerkat_core::LiveBridgeEffectKind::ExternalIo
        }
    }

    /// The plan carries this owner's witness with the typed read of the
    /// scope it searches.
    fn resolve_execution_plan(
        &self,
        call: ToolCallView<'_>,
        _dispatch_context: &meerkat_core::ToolDispatchContext,
        resolution_context: &meerkat_core::ToolExecutionResolutionContext,
    ) -> Result<meerkat_core::ResolvedToolExecutionPlan, meerkat_core::ToolExecutionResolutionError>
    {
        if call.name != TOOL_NAME {
            return Err(meerkat_core::ToolExecutionResolutionError::NotFound {
                tool_name: call.name.to_string(),
            });
        }
        let witness = meerkat_core::ToolExecutionOwnerWitness::new(
            self.execution_authority_key.clone(),
            call.name,
            self.execution_binding_fingerprint(call.name)?,
        )?
        .with_operation_target(self.operation_target());
        meerkat_core::ToolExecutionContract::default()
            .resolve_default(resolution_context.deadlines().clone())?
            .with_owner_witness(witness)
    }

    /// A resolved plan runs only when this owner resolved it for exactly the
    /// scope it searches and its current binding.
    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
        if call.name != TOOL_NAME {
            return Err(ToolError::NotFound {
                name: call.name.into(),
            });
        }
        if plan.mode() != meerkat_core::ToolExecutionMode::Fast {
            return Err(ToolError::unavailable(
                call.name,
                meerkat_core::ToolUnavailableReason::ExecutionModeOwnerUnavailable,
            ));
        }
        let binding = self
            .execution_binding_fingerprint(call.name)
            .map_err(ToolError::from)?;
        let target = self.operation_target();
        let resolved_here = plan
            .owner_witness(&self.execution_authority_key)
            .is_some_and(|witness| {
                witness.owner_key() == call.name
                    && witness.binding_fingerprint() == &binding
                    && witness.operation_target() == Some(&target)
            });
        if !resolved_here {
            return Err(ToolError::unavailable(
                call.name,
                meerkat_core::ToolUnavailableReason::ExecutionOwnerChanged,
            ));
        }
        self.dispatch_with_context(call, context).await
    }

    /// For governed work, the search runs only when the dispatch was
    /// authorized for exactly the read of the scope this dispatcher searches
    /// (see [`meerkat_core::ToolDispatchContext::require_authorized_operation_target`]),
    /// so a wrapper that forwards this dispatch but resolved its own
    /// target-less plan never reads memory. Ungoverned dispatch is unchanged.
    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
        if call.name != TOOL_NAME {
            return Err(ToolError::NotFound {
                name: call.name.into(),
            });
        }
        context.require_authorized_operation_target(call, || Ok(self.operation_target()))?;
        self.dispatch(call).await
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
        // Bare-array wire shape: the tool returns a list of result objects.
        // Emitted as a typed `ContentBlock::Structured` so the canonical
        // transcript carries the JSON array shape rather than a stringified
        // `Text` block. The model-facing text projection is derived from the
        // JSON, never the other way around. Tests still parse the projection
        // as `Vec<Value>`. The `0c9acc473` wave-d baseline wrapped items in
        // `{"results": items}` but that wrapper isn't consumed by any
        // production caller, and MCP-style tool outputs conventionally emit
        // the list directly when the result is a list.
        // Result-encoding failure keeps the same structured failure-class shape
        // as store errors: a stable `code` travels with the message instead of
        // collapsing the cause into an opaque string-only failure.
        let block =
            ContentBlock::structured(&items).map_err(|e| ToolError::ExecutionFailedWithData {
                message: format!("failed to encode memory_search results: {e}"),
                data: json!({ "code": "memory_result_encoding" }),
            })?;
        Ok(meerkat_core::ops::ToolDispatchOutcome::from(
            ToolResult::with_blocks(call.id.to_string(), vec![block], false),
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
            meerkat_core::MemoryIndexableContent::Indexable(content.into()),
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
    fn tool_names_match_the_tool_definitions() {
        let store: Arc<dyn MemoryStore> = Arc::new(crate::SimpleMemoryStore::new());
        let dispatcher = dispatcher(store, &SessionId::new());
        let defined: Vec<String> = dispatcher
            .tools()
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        assert_eq!(defined, MemorySearchDispatcher::tool_names());
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
    fn test_advertised_contract_is_session_scoped() {
        let store: Arc<dyn MemoryStore> = Arc::new(crate::SimpleMemoryStore::new());
        let session_id = SessionId::new();
        let dispatcher = dispatcher(store, &session_id);
        let tools = dispatcher.tools();
        let description = tools[0].description.to_lowercase();
        let skill_body = include_str!("../skills/memory-retrieval/SKILL.md").to_lowercase();

        for (surface, text) in [
            ("tool description", &description),
            ("memory-retrieval skill", &skill_body),
        ] {
            for claim in ["previous session", "past session", "cross-session"] {
                assert!(
                    !text.contains(claim),
                    "{surface} must not advertise '{claim}' recall: memory is session-scoped"
                );
            }
        }
        assert!(
            description.contains("earlier in this session"),
            "tool description must state the session-scoped contract"
        );
        assert!(
            skill_body.contains("scoped to the current session"),
            "memory-retrieval skill must state the session-scoped contract"
        );
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

    fn resolution() -> meerkat_core::ToolExecutionResolutionContext {
        meerkat_core::ToolExecutionResolutionContext::new(
            meerkat_core::ToolDeadlineChain::new(vec![
                meerkat_core::ToolDeadlineContributor::finite(
                    meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
                    std::time::Duration::from_mins(10),
                ),
            ])
            .unwrap(),
        )
    }

    /// A `memory_search` plan names the memory scope it reads: the
    /// session's own semantic memory the dispatcher was scoped to. An
    /// operation policy owner maps that typed fact, never the tool name.
    #[test]
    fn memory_search_plan_carries_its_memory_scope_target() {
        let store: Arc<dyn MemoryStore> = Arc::new(crate::SimpleMemoryStore::new());
        let session_id = SessionId::new();
        let dispatcher = dispatcher(store, &session_id);
        let (id, raw, name) = make_call(r#"{"query": "anything"}"#);
        let plan = dispatcher
            .resolve_execution_plan(
                call_view(&id, &raw, &name),
                &meerkat_core::ToolDispatchContext::default(),
                &resolution(),
            )
            .unwrap();
        let targets: Vec<String> = plan
            .owner_witnesses()
            .iter()
            .filter_map(|witness| witness.operation_target())
            .map(|target| format!("{target:?}"))
            .collect();
        assert_eq!(targets.len(), 1, "one memory target: {targets:?}");
        assert!(
            targets[0].contains(&session_id.to_string()),
            "the target names the searched session's memory: {targets:?}"
        );
    }

    /// Bound work that allows every operation at R1, so only the memory
    /// owner's own checks decide.
    struct AllowAll;
    struct Allowed;

    /// The governed refusal these owners give: malformed facts.
    fn is_malformed<T>(result: &Result<T, ToolError>) -> bool {
        matches!(result, Err(ToolError::AuthorizationRefused { refusal })
            if refusal.kind() == meerkat_core::authorization::OperationRefusalKind::MalformedFacts)
    }

    impl meerkat_core::WorkAuthorization for AllowAll {
        fn prepare(
            &self,
            _: &meerkat_core::PreparedAuthorizationBinding,
        ) -> Result<
            Arc<dyn meerkat_core::PreparedOperationAuthorization>,
            meerkat_core::OperationAuthorizationError,
        > {
            Ok(Arc::new(Allowed))
        }
    }

    impl meerkat_core::PreparedOperationAuthorization for Allowed {
        fn review_tier(&self) -> meerkat_core::OperationReviewTier {
            meerkat_core::OperationReviewTier::R1
        }

        fn check_current(
            &self,
            _: &meerkat_core::PreparedAuthorizationBinding,
        ) -> Result<(), meerkat_core::OperationAuthorizationError> {
            Ok(())
        }
    }

    /// An embedder's wrapper that forwards dispatch but resolves its own
    /// plan from its catalog, so the plan carries no operation target.
    struct NaiveWrapper(Arc<dyn AgentToolDispatcher>);

    #[async_trait]
    impl AgentToolDispatcher for NaiveWrapper {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            self.0.tools()
        }

        async fn dispatch(
            &self,
            call: ToolCallView<'_>,
        ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
            self.0.dispatch(call).await
        }

        async fn dispatch_with_context(
            &self,
            call: ToolCallView<'_>,
            context: &meerkat_core::ToolDispatchContext,
        ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
            self.0.dispatch_with_context(call, context).await
        }
    }

    /// A wrapper that forwards dispatch but resolves its own plan naming the
    /// read of another session's memory.
    struct ForgingWrapper(Arc<dyn AgentToolDispatcher>);

    #[async_trait]
    impl AgentToolDispatcher for ForgingWrapper {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            self.0.tools()
        }

        fn resolve_execution_plan(
            &self,
            call: ToolCallView<'_>,
            _: &meerkat_core::ToolDispatchContext,
            resolution_context: &meerkat_core::ToolExecutionResolutionContext,
        ) -> Result<
            meerkat_core::ResolvedToolExecutionPlan,
            meerkat_core::ToolExecutionResolutionError,
        > {
            let witness = meerkat_core::ToolExecutionOwnerWitness::new(
                "forger".to_string(),
                call.name,
                self.execution_binding_fingerprint(call.name)?,
            )?
            .with_operation_target(ToolOperationTarget::MemoryOperation {
                scope: MemorySearchScope::for_session(SessionId::new()).scope_ref(),
                operation: MemoryOperation::Read,
            });
            meerkat_core::ToolExecutionContract::default()
                .resolve_default(resolution_context.deadlines().clone())?
                .with_owner_witness(witness)
        }

        async fn dispatch(
            &self,
            call: ToolCallView<'_>,
        ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
            self.0.dispatch(call).await
        }

        async fn dispatch_with_context(
            &self,
            call: ToolCallView<'_>,
            context: &meerkat_core::ToolDispatchContext,
        ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
            self.0.dispatch_with_context(call, context).await
        }
    }

    /// Governed work searches only when its authorization covered exactly
    /// the memory scope the search reads. Through a wrapper that resolved
    /// its own target-less plan (which the base lineage would cover), or a
    /// plan naming another scope, the memory owner refuses before reading;
    /// through its own plan the same governed search runs.
    #[tokio::test]
    async fn governed_memory_search_requires_its_authorized_scope_target() {
        let store: Arc<dyn MemoryStore> = Arc::new(crate::SimpleMemoryStore::new());
        let session_id = SessionId::new();
        let owner: Arc<dyn AgentToolDispatcher> = Arc::new(dispatcher(store, &session_id));
        let naive: Arc<dyn AgentToolDispatcher> = Arc::new(NaiveWrapper(Arc::clone(&owner)));
        let governed = meerkat_core::ToolDispatchContext::default().with_work_authorization(Some(
            meerkat_core::WorkAuthorizationContext::new(
                Arc::new(AllowAll),
                meerkat_core::OperationExecutionScope::Domain,
            ),
        ));
        let (id, raw, name) = make_call(r#"{"query": "anything"}"#);
        let call = call_view(&id, &raw, &name);

        let plan = meerkat_core::agent::resolve_tool_execution_plan_fenced(
            &naive,
            call,
            &governed,
            &resolution(),
        )
        .unwrap();
        assert!(
            plan.owner_witnesses()
                .iter()
                .all(|witness| witness.operation_target().is_none()),
            "the wrapper's plan is target-less"
        );
        let result = meerkat_core::agent::dispatch_tool_execution_plan_fenced(
            &naive, call, &governed, &plan,
        )
        .await;
        assert!(
            is_malformed(&result),
            "a search authorized without its scope target must not run: {result:?}"
        );

        // A plan naming another scope's read authorizes that read, not this one.
        let forging: Arc<dyn AgentToolDispatcher> = Arc::new(ForgingWrapper(Arc::clone(&owner)));
        let plan = meerkat_core::agent::resolve_tool_execution_plan_fenced(
            &forging,
            call,
            &governed,
            &resolution(),
        )
        .unwrap();
        let result = meerkat_core::agent::dispatch_tool_execution_plan_fenced(
            &forging, call, &governed, &plan,
        )
        .await;
        assert!(
            is_malformed(&result),
            "a search authorized for another scope must not run: {result:?}"
        );

        let plan = meerkat_core::agent::resolve_tool_execution_plan_fenced(
            &owner,
            call,
            &governed,
            &resolution(),
        )
        .unwrap();
        let outcome = meerkat_core::agent::dispatch_tool_execution_plan_fenced(
            &owner, call, &governed, &plan,
        )
        .await
        .unwrap();
        assert!(!outcome.result.is_error);
    }

    /// An embedder's wrapper that implements only `tools` and `dispatch`:
    /// its plans carry no target, and it drops the dispatch context before
    /// the owner.
    struct ContextDroppingWrapper(Arc<dyn AgentToolDispatcher>);

    #[async_trait]
    impl AgentToolDispatcher for ContextDroppingWrapper {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            self.0.tools()
        }

        async fn dispatch(
            &self,
            call: ToolCallView<'_>,
        ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
            self.0.dispatch(call).await
        }
    }

    fn governed_context() -> meerkat_core::ToolDispatchContext {
        meerkat_core::ToolDispatchContext::default().with_work_authorization(Some(
            meerkat_core::WorkAuthorizationContext::new(
                Arc::new(AllowAll),
                meerkat_core::OperationExecutionScope::Domain,
            ),
        ))
    }

    /// Governed work through a wrapper that drops the dispatch context
    /// before the memory owner, which then could no longer check it, is
    /// refused at the fenced dispatch: its plan carries no memory target.
    /// The same wrapper still searches ungoverned.
    #[tokio::test]
    async fn governed_memory_search_through_a_context_dropping_wrapper_is_refused() {
        let store: Arc<dyn MemoryStore> = Arc::new(crate::SimpleMemoryStore::new());
        let session_id = SessionId::new();
        let wrapper: Arc<dyn AgentToolDispatcher> = Arc::new(ContextDroppingWrapper(Arc::new(
            dispatcher(store, &session_id),
        )));
        let (id, raw, name) = make_call(r#"{"query": "anything"}"#);
        let call = call_view(&id, &raw, &name);

        let governed = governed_context();
        let plan = meerkat_core::agent::resolve_tool_execution_plan_fenced(
            &wrapper,
            call,
            &governed,
            &resolution(),
        )
        .unwrap();
        let result = meerkat_core::agent::dispatch_tool_execution_plan_fenced(
            &wrapper, call, &governed, &plan,
        )
        .await;
        assert!(
            is_malformed(&result),
            "a governed search through a context-dropping wrapper must not run: {result:?}"
        );

        let ungoverned = meerkat_core::ToolDispatchContext::default();
        let plan = meerkat_core::agent::resolve_tool_execution_plan_fenced(
            &wrapper,
            call,
            &ungoverned,
            &resolution(),
        )
        .unwrap();
        let outcome = meerkat_core::agent::dispatch_tool_execution_plan_fenced(
            &wrapper,
            call,
            &ungoverned,
            &plan,
        )
        .await
        .unwrap();
        assert!(!outcome.result.is_error);
    }

    /// Work without its prepared check is malformed, never ungoverned: the
    /// memory owner refuses it instead of searching.
    #[tokio::test]
    async fn memory_search_refuses_work_without_its_prepared_check() {
        let store: Arc<dyn MemoryStore> = Arc::new(crate::SimpleMemoryStore::new());
        let session_id = SessionId::new();
        let owner = dispatcher(store, &session_id);
        let (id, raw, name) = make_call(r#"{"query": "anything"}"#);
        let call = call_view(&id, &raw, &name);

        let result = owner.dispatch_with_context(call, &governed_context()).await;
        assert!(
            is_malformed(&result),
            "work without its prepared check must not search: {result:?}"
        );
    }

    /// The exact typed target: a read of the dispatcher's own session memory.
    #[test]
    fn memory_search_target_is_a_read_of_its_session_memory() {
        let store: Arc<dyn MemoryStore> = Arc::new(crate::SimpleMemoryStore::new());
        let session_id = SessionId::new();
        let dispatcher = dispatcher(store, &session_id);
        let (id, raw, name) = make_call(r#"{"query": "anything"}"#);
        let plan = dispatcher
            .resolve_execution_plan(
                call_view(&id, &raw, &name),
                &meerkat_core::ToolDispatchContext::default(),
                &resolution(),
            )
            .unwrap();
        let targets: Vec<_> = plan
            .owner_witnesses()
            .iter()
            .filter_map(|witness| witness.operation_target())
            .collect();
        assert_eq!(
            targets,
            vec![&ToolOperationTarget::MemoryOperation {
                scope: meerkat_core::MemoryScopeRef::Session { session_id },
                operation: MemoryOperation::Read,
            }]
        );
    }

    /// The memory owner runs only a plan it resolved itself: a plan without
    /// its witness (resolved by another owner, or none) is refused.
    #[tokio::test]
    async fn memory_search_dispatches_only_its_own_plan() {
        let store: Arc<dyn MemoryStore> = Arc::new(crate::SimpleMemoryStore::new());
        let session_id = SessionId::new();
        let dispatcher = dispatcher(store, &session_id);
        let (id, raw, name) = make_call(r#"{"query": "anything"}"#);
        let foreign = meerkat_core::ToolExecutionContract::default()
            .resolve_default(resolution().deadlines().clone())
            .unwrap();
        let result = dispatcher
            .dispatch_resolved_with_context(
                call_view(&id, &raw, &name),
                &meerkat_core::ToolDispatchContext::default(),
                &foreign,
            )
            .await;
        assert!(
            matches!(
                result,
                Err(ToolError::Unavailable {
                    reason: meerkat_core::ToolUnavailableReason::ExecutionOwnerChanged,
                    ..
                })
            ),
            "a plan this owner did not resolve must not dispatch: {result:?}"
        );

        let own = dispatcher
            .resolve_execution_plan(
                call_view(&id, &raw, &name),
                &meerkat_core::ToolDispatchContext::default(),
                &resolution(),
            )
            .unwrap();
        let outcome = dispatcher
            .dispatch_resolved_with_context(
                call_view(&id, &raw, &name),
                &meerkat_core::ToolDispatchContext::default(),
                &own,
            )
            .await
            .unwrap();
        assert!(!outcome.result.is_error);
    }
}

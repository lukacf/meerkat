#![allow(
    clippy::expect_used,
    clippy::unnecessary_literal_bound,
    clippy::unwrap_used
)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use meerkat::AgentBuilder as FacadeAgentBuilder;
use meerkat_core::types::{AssistantBlock, StopReason, ToolProvenance, ToolSourceKind, Usage};
use meerkat_core::{
    AgentError, AgentLlmClient, AgentSessionStore, AgentToolDispatcher, DynamicToolComposite,
    LlmStreamResult, Provider, Session, ToolCatalogCapabilities, ToolCatalogEntry, ToolDef,
    ToolResult,
};
use meerkat_tools::{CatalogControlDispatcher, CatalogControlVisibilityProvider};
use serde_json::value::RawValue;

const LOAD_TOOL_NAME: &str = "tool_catalog_load";

struct ExactCatalogDispatcher {
    tools: Arc<[Arc<ToolDef>]>,
    catalog: Arc<[ToolCatalogEntry]>,
}

#[async_trait]
impl AgentToolDispatcher for ExactCatalogDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tools)
    }

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        ToolCatalogCapabilities {
            exact_catalog: true,
            may_require_catalog_control_plane: false,
        }
    }

    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        Arc::clone(&self.catalog)
    }

    async fn dispatch(
        &self,
        call: meerkat_core::ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        Ok(ToolResult::new(
            call.id.to_string(),
            serde_json::json!({ "loaded": call.name }).to_string(),
            false,
        )
        .into())
    }
}

struct NoopAgentStore;

#[async_trait]
impl AgentSessionStore for NoopAgentStore {
    async fn save(&self, _session: &Session) -> Result<(), AgentError> {
        Ok(())
    }

    async fn load(&self, _id: &str) -> Result<Option<Session>, AgentError> {
        Ok(None)
    }
}

struct CatalogLoadRouteClient {
    calls: Mutex<u32>,
    seen_tools: Mutex<Vec<Vec<String>>>,
}

impl CatalogLoadRouteClient {
    fn new() -> Self {
        Self {
            calls: Mutex::new(0),
            seen_tools: Mutex::new(Vec::new()),
        }
    }

    fn seen_tools(&self) -> Vec<Vec<String>> {
        self.seen_tools.lock().unwrap().clone()
    }
}

#[async_trait]
impl AgentLlmClient for CatalogLoadRouteClient {
    async fn stream_response(
        &self,
        _messages: &[meerkat_core::Message],
        tools: &[Arc<ToolDef>],
        _max_tokens: u32,
        _temperature: Option<f32>,
        _provider_params: Option<&meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
    ) -> Result<LlmStreamResult, AgentError> {
        self.seen_tools.lock().unwrap().push(
            tools
                .iter()
                .map(|tool| tool.name.to_string())
                .collect::<Vec<_>>(),
        );
        let mut calls = self.calls.lock().unwrap();
        let response = if *calls == 0 {
            LlmStreamResult::new(
                vec![AssistantBlock::ToolUse {
                    id: "load-deferred".to_string(),
                    name: LOAD_TOOL_NAME.into(),
                    args: RawValue::from_string(
                        serde_json::json!({ "names": ["deferred_mcp_tool"] }).to_string(),
                    )
                    .unwrap(),
                    meta: None,
                }],
                StopReason::ToolUse,
                Usage::default(),
            )
        } else {
            LlmStreamResult::new(
                vec![AssistantBlock::Text {
                    text: "done".to_string(),
                    meta: None,
                }],
                StopReason::EndTurn,
                Usage::default(),
            )
        };
        *calls += 1;
        Ok(response)
    }

    fn provider(&self) -> &'static str {
        "mock"
    }

    fn model(&self) -> &str {
        "mock-model"
    }
}

fn session_tool(name: &str, description: &str) -> Arc<ToolDef> {
    Arc::new(ToolDef {
        name: name.into(),
        description: description.to_string(),
        input_schema: serde_json::json!({ "type": "object" }),
        provenance: Some(ToolProvenance {
            kind: ToolSourceKind::Callback,
            source_id: "test".into(),
        }),
    })
}

#[tokio::test]
async fn agent_boundary_routes_tool_catalog_load_authority_from_control_plane() {
    let deferred = session_tool("deferred_mcp_tool", "Deferred MCP tool");
    let deferred_two = session_tool("deferred_mcp_tool_two", "Second deferred MCP tool");
    let session_dispatcher: Arc<dyn AgentToolDispatcher> = Arc::new(ExactCatalogDispatcher {
        tools: vec![Arc::clone(&deferred), Arc::clone(&deferred_two)].into(),
        catalog: vec![
            ToolCatalogEntry::session_deferred(
                Arc::clone(&deferred),
                true,
                "callback:test".to_string(),
            ),
            ToolCatalogEntry::session_deferred(
                Arc::clone(&deferred_two),
                true,
                "callback:test".to_string(),
            ),
        ]
        .into(),
    });
    let visibility_provider = Arc::new(CatalogControlVisibilityProvider::new());
    let control_dispatcher: Arc<dyn AgentToolDispatcher> = Arc::new(CatalogControlDispatcher::new(
        Arc::clone(&session_dispatcher),
        Arc::clone(&visibility_provider),
    ));
    let tools: Arc<dyn AgentToolDispatcher> = Arc::new(DynamicToolComposite::new(vec![
        session_dispatcher,
        control_dispatcher,
    ]));
    let client = Arc::new(CatalogLoadRouteClient::new());
    let build_client: Arc<dyn AgentLlmClient> = client.clone();
    let store: Arc<dyn AgentSessionStore> = Arc::new(NoopAgentStore);

    let mut agent = FacadeAgentBuilder::new()
        .model("mock-model")
        .provider(Provider::Other)
        .build(build_client, tools, store)
        .await
        .expect("facade builder should route through AgentFactory");
    visibility_provider.set_scope(agent.tool_scope().clone());

    let result = agent.run("prompt".to_string().into()).await.unwrap();
    assert_eq!(result.text, "done");

    let seen_tools = client.seen_tools();
    assert!(
        seen_tools.len() >= 2,
        "expected load route to continue into a post-boundary LLM request, saw {seen_tools:?}"
    );
    assert!(
        seen_tools[0].iter().any(|name| name == LOAD_TOOL_NAME),
        "control load tool must be visible before deferred tools are loaded: {seen_tools:?}"
    );
    assert!(
        !seen_tools[0].iter().any(|name| name == "deferred_mcp_tool"),
        "deferred tool must stay hidden until the real catalog-load route is accepted"
    );
    assert!(
        seen_tools[1].iter().any(|name| name == "deferred_mcp_tool"),
        "real catalog-load route should reveal the requested deferred tool on the next boundary"
    );

    let visibility_state = agent
        .session()
        .tool_visibility_state()
        .expect("agent visibility state should decode")
        .expect("agent boundary should persist canonical visibility state");
    assert!(
        visibility_state
            .active_requested_deferred_names
            .contains("deferred_mcp_tool"),
        "catalog-load route should commit the requested deferred name at the boundary"
    );
    let witness = visibility_state
        .requested_witnesses
        .get("deferred_mcp_tool")
        .expect("catalog-load route should persist the provenance witness");
    assert_eq!(witness.stable_owner_key.as_deref(), Some("callback:test"));
    assert_eq!(
        witness.last_seen_provenance.as_ref(),
        deferred.provenance.as_ref(),
        "authority must come from the catalog entry provenance, not only the requested name"
    );
}

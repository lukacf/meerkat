#![allow(
    clippy::expect_used,
    clippy::panic,
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

fn normalized_catalog_usage() -> Usage {
    meerkat_core::TurnUsage::host_declared(Provider::Other, "mock-model", Usage::default())
        .into_inner()
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
                normalized_catalog_usage(),
            )
        } else {
            LlmStreamResult::new(
                vec![AssistantBlock::Text {
                    text: "done".to_string(),
                    meta: None,
                }],
                StopReason::EndTurn,
                normalized_catalog_usage(),
            )
        };
        *calls += 1;
        Ok(response)
    }

    fn provider(&self) -> meerkat::Provider {
        meerkat::Provider::Other
    }

    fn model(&self) -> &str {
        "mock-model"
    }
}

fn session_tool(name: &str, description: &str) -> Arc<ToolDef> {
    Arc::new(ToolDef {
        audience: Default::default(),
        name: name.into(),
        description: description.to_string(),
        input_schema: serde_json::json!({ "type": "object" }),
        provenance: Some(ToolProvenance {
            kind: ToolSourceKind::Callback,
            source_id: "test".into(),
            artifact: None,
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
                ToolProvenance {
                    kind: ToolSourceKind::Callback,
                    source_id: "test".into(),
                    artifact: None,
                },
            ),
            ToolCatalogEntry::session_deferred(
                Arc::clone(&deferred_two),
                true,
                ToolProvenance {
                    kind: ToolSourceKind::Callback,
                    source_id: "test".into(),
                    artifact: None,
                },
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
    assert_eq!(
        witness.last_seen_provenance.as_ref(),
        deferred.provenance.as_ref(),
        "authority must come from the catalog entry provenance, not only the requested name"
    );
}

const SEARCH_TOOL_NAME: &str = "tool_catalog_search";
const BLOCKED_TOOL: &str = "blocked_mcp_tool";
const BLOCKED_DESCRIPTION: &str = "Deferred MCP tool the session's filter blocks";
const UNKNOWN_TOOL: &str = "never_registered_tool";

/// A scripted model that, in its first response, searches the whole catalog,
/// asks to load the blocked tool, asks to load a name no source registered
/// and calls the blocked tool; then it finishes.
struct BlockedToolProbeClient {
    calls: Mutex<u32>,
    seen_tools: Mutex<Vec<Vec<String>>>,
}

impl BlockedToolProbeClient {
    fn new() -> Self {
        Self {
            calls: Mutex::new(0),
            seen_tools: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl AgentLlmClient for BlockedToolProbeClient {
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
        let tool_use = |id: &str, name: &str, args: serde_json::Value| AssistantBlock::ToolUse {
            id: id.to_string(),
            name: name.into(),
            args: RawValue::from_string(args.to_string()).unwrap(),
            meta: None,
        };
        let response = if *calls == 0 {
            LlmStreamResult::new(
                vec![
                    tool_use("search-all", SEARCH_TOOL_NAME, serde_json::json!({})),
                    tool_use(
                        "load-blocked",
                        LOAD_TOOL_NAME,
                        serde_json::json!({ "names": [BLOCKED_TOOL] }),
                    ),
                    tool_use(
                        "load-unknown",
                        LOAD_TOOL_NAME,
                        serde_json::json!({ "names": [UNKNOWN_TOOL] }),
                    ),
                    tool_use("call-blocked", BLOCKED_TOOL, serde_json::json!({})),
                ],
                StopReason::ToolUse,
                normalized_catalog_usage(),
            )
        } else {
            LlmStreamResult::new(
                vec![AssistantBlock::Text {
                    text: "done".to_string(),
                    meta: None,
                }],
                StopReason::EndTurn,
                normalized_catalog_usage(),
            )
        };
        *calls += 1;
        Ok(response)
    }

    fn provider(&self) -> meerkat::Provider {
        meerkat::Provider::Other
    }

    fn model(&self) -> &str {
        "mock-model"
    }
}

/// What the probe's session holds afterwards, and what the provider was
/// offered in each request.
struct BlockedToolProbe {
    session: Session,
    seen_tools: Vec<Vec<String>>,
}

impl BlockedToolProbe {
    fn result_of(&self, id: &str) -> ToolResult {
        self.session
            .messages()
            .iter()
            .find_map(|message| match message {
                meerkat_core::Message::ToolResults { results, .. } => {
                    results.iter().find(|result| result.tool_use_id == id)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("a tool result for {id}"))
            .clone()
    }

    fn deferred_catalog_notices(&self) -> Vec<String> {
        self.session
            .messages()
            .iter()
            .filter_map(|message| match message {
                meerkat_core::Message::SystemNotice(notice) => notice.body.clone(),
                _ => None,
            })
            .filter(|body| body.starts_with("Deferred catalog changed at turn boundary:"))
            .collect()
    }

    /// The names each deferred-catalog notice carries in its typed delta
    /// (added and removed), one list per notice, in order.
    fn deferred_catalog_delta_names(&self) -> Vec<Vec<String>> {
        self.session
            .messages()
            .iter()
            .filter_map(|message| match message {
                meerkat_core::Message::SystemNotice(notice) => Some(notice),
                _ => None,
            })
            .flat_map(|notice| notice.blocks.iter())
            .filter_map(|block| match block {
                meerkat_core::types::SystemNoticeBlock::ToolConfig { payload } => {
                    payload.deferred_catalog_delta.as_ref()
                }
                _ => None,
            })
            .map(|delta| {
                delta
                    .added_hidden_names
                    .iter()
                    .chain(delta.removed_hidden_names.iter())
                    .map(ToString::to_string)
                    .collect()
            })
            .collect()
    }
}

/// One session through the composed control plane and the agent loop: three
/// deferrable tools, of which the session's filter (a staged deny list)
/// blocks one, and the scripted probe above as its model.
async fn run_blocked_tool_probe() -> BlockedToolProbe {
    let tools = [
        session_tool("deferred_mcp_tool", "Deferred MCP tool"),
        session_tool("deferred_mcp_tool_two", "Second deferred MCP tool"),
        session_tool(BLOCKED_TOOL, BLOCKED_DESCRIPTION),
    ];
    let session_dispatcher: Arc<dyn AgentToolDispatcher> = Arc::new(ExactCatalogDispatcher {
        tools: tools.to_vec().into(),
        catalog: tools
            .iter()
            .map(|tool| {
                ToolCatalogEntry::session_deferred(
                    Arc::clone(tool),
                    true,
                    ToolProvenance {
                        kind: ToolSourceKind::Callback,
                        source_id: "test".into(),
                        artifact: None,
                    },
                )
            })
            .collect::<Vec<_>>()
            .into(),
    });
    let visibility_provider = Arc::new(CatalogControlVisibilityProvider::new());
    let control_dispatcher: Arc<dyn AgentToolDispatcher> = Arc::new(CatalogControlDispatcher::new(
        Arc::clone(&session_dispatcher),
        Arc::clone(&visibility_provider),
    ));
    let composed: Arc<dyn AgentToolDispatcher> = Arc::new(DynamicToolComposite::new(vec![
        session_dispatcher,
        control_dispatcher,
    ]));
    let client = Arc::new(BlockedToolProbeClient::new());
    let build_client: Arc<dyn AgentLlmClient> = client.clone();
    let store: Arc<dyn AgentSessionStore> = Arc::new(NoopAgentStore);

    let mut agent = FacadeAgentBuilder::new()
        .model("mock-model")
        .provider(Provider::Other)
        .build(build_client, composed, store)
        .await
        .expect("facade builder should route through AgentFactory");
    visibility_provider.set_scope(agent.tool_scope().clone());
    agent
        .stage_external_tool_filter(meerkat_core::ToolFilter::Deny(
            [BLOCKED_TOOL.to_string()].into_iter().collect(),
        ))
        .expect("stage the deny list");

    let result = agent.run("prompt".to_string().into()).await.unwrap();
    assert_eq!(result.text, "done");

    BlockedToolProbe {
        session: agent.session().clone(),
        seen_tools: client.seen_tools.lock().unwrap().clone(),
    }
}

/// A tool the session's filter blocks is not shown to the model by the
/// catalog: a search returns and counts only the tools the filter allows,
/// with neither the blocked tool's name nor its description, and the
/// deferred-catalog notice does not name it.
#[tokio::test]
async fn a_filter_blocked_deferred_tool_is_not_named_by_search_or_by_the_notice() {
    let probe = run_blocked_tool_probe().await;

    let search = probe.result_of("search-all");
    assert!(!search.is_error);
    let search_text = search.text_content();
    let found: serde_json::Value = serde_json::from_str(&search_text).unwrap();
    assert_eq!(
        found["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["deferred_mcp_tool", "deferred_mcp_tool_two"],
        "search returns only what the session's filter allows: {search_text}"
    );
    assert_eq!(found["total_matches"], 2, "{search_text}");
    assert!(
        !search_text.contains(BLOCKED_TOOL) && !search_text.contains(BLOCKED_DESCRIPTION),
        "neither the name nor the description reaches the model: {search_text}"
    );

    let notices = probe.deferred_catalog_notices();
    assert_eq!(
        notices.first().map(String::as_str),
        Some(
            "Deferred catalog changed at turn boundary: new deferred tools available: \
             deferred_mcp_tool, deferred_mcp_tool_two"
        ),
        "{notices:?}"
    );
    assert!(
        notices.iter().all(|body| !body.contains(BLOCKED_TOOL)),
        "the notice never names the blocked tool: {notices:?}"
    );

    // The notice's typed delta carries the same names as its text.
    let deltas = probe.deferred_catalog_delta_names();
    assert_eq!(
        deltas.first().map(Vec::as_slice),
        Some(
            &[
                "deferred_mcp_tool".to_string(),
                "deferred_mcp_tool_two".to_string()
            ][..]
        ),
        "{deltas:?}"
    );
    assert!(
        deltas.iter().flatten().all(|name| name != BLOCKED_TOOL),
        "the typed delta never names the blocked tool: {deltas:?}"
    );
}

/// CONTROL: tool load and call behavior is unchanged. A tool the session's
/// filter blocks is in no provider request, its load is refused and its
/// call is refused by the session's tool policy; the row pins the answers
/// as they were.
#[tokio::test]
async fn a_filter_blocked_deferred_tool_is_refused_at_load_and_at_call() {
    let probe = run_blocked_tool_probe().await;

    assert!(
        probe
            .seen_tools
            .iter()
            .all(|request| !request.iter().any(|name| name == BLOCKED_TOOL)),
        "the blocked tool is never in a provider request: {:?}",
        probe.seen_tools
    );

    let blocked_load = probe.result_of("load-blocked");
    assert!(!blocked_load.is_error);
    assert_eq!(
        blocked_load.text_content(),
        format!(
            r#"{{"catalog_exact":true,"accepted_names":[],"noop_names":[],"resolutions":[{{"name":"{BLOCKED_TOOL}","accepted":false,"rejected_reason":"not_filterable"}}]}}"#
        ),
        "a load of the blocked tool is refused"
    );

    let unknown_load = probe.result_of("load-unknown");
    assert!(!unknown_load.is_error);
    assert_eq!(
        unknown_load.text_content(),
        format!(
            r#"{{"catalog_exact":true,"accepted_names":[],"noop_names":[],"resolutions":[{{"name":"{UNKNOWN_TOOL}","accepted":false,"rejected_reason":"unknown_key"}}]}}"#
        ),
        "a load of a name no source registered is refused"
    );

    let call = probe.result_of("call-blocked");
    assert!(call.is_error, "a call of the blocked tool is refused");
    assert!(
        call.text_content()
            .contains(&format!("Tool '{BLOCKED_TOOL}' is not allowed by policy")),
        "the refusal is the tool policy's: {}",
        call.text_content()
    );
}

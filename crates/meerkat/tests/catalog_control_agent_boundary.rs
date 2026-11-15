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
    // The second tool alone is over the catalog's byte budget, so the
    // session defers both.
    let deferred_two = session_tool(
        "deferred_mcp_tool_two",
        &format!(
            "Second deferred MCP tool {}",
            "x".repeat(meerkat_core::tool_catalog::DEFERRED_CATALOG_SCHEMA_VOLUME_THRESHOLD)
        ),
    );
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
const DENIED_TOOL: &str = "denied_mcp_tool";
const UNKNOWN_TOOL: &str = "never_registered_tool";

/// A model that searches the whole catalog, loads the hidden tool, loads a
/// name no source ever registered and calls the hidden tool, all in its
/// first response, then finishes.
struct HiddenToolProbeClient {
    hidden: &'static str,
    provider: Provider,
    model: &'static str,
    calls: Mutex<u32>,
    seen_tools: Mutex<Vec<Vec<String>>>,
}

impl HiddenToolProbeClient {
    fn new(hidden: &'static str) -> Self {
        Self::for_model(hidden, Provider::Other, "mock-model")
    }

    /// The same probe as the client of a session on `provider`/`model`.
    fn for_model(hidden: &'static str, provider: Provider, model: &'static str) -> Self {
        Self {
            hidden,
            provider,
            model,
            calls: Mutex::new(0),
            seen_tools: Mutex::new(Vec::new()),
        }
    }

    fn usage(&self) -> Usage {
        meerkat_core::TurnUsage::host_declared(self.provider, self.model, Usage::default())
            .into_inner()
    }
}

#[async_trait]
impl AgentLlmClient for HiddenToolProbeClient {
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
                        "load-hidden",
                        LOAD_TOOL_NAME,
                        serde_json::json!({ "names": [self.hidden] }),
                    ),
                    tool_use(
                        "load-unknown",
                        LOAD_TOOL_NAME,
                        serde_json::json!({ "names": [UNKNOWN_TOOL] }),
                    ),
                    tool_use("call-hidden", self.hidden, serde_json::json!({})),
                ],
                StopReason::ToolUse,
                self.usage(),
            )
        } else {
            LlmStreamResult::new(
                vec![AssistantBlock::Text {
                    text: "done".to_string(),
                    meta: None,
                }],
                StopReason::EndTurn,
                self.usage(),
            )
        };
        *calls += 1;
        Ok(response)
    }

    fn provider(&self) -> meerkat::Provider {
        self.provider
    }

    fn model(&self) -> &str {
        self.model
    }
}

/// Two deferrable tools every session of these rows may use, a third one
/// (`hidden`) its filters do not allow, and the catalog control plane
/// composed over them as the factory composes it. The first tool's
/// definition is over the deferred catalog's byte budget, so the session
/// uses the deferred catalog and its model is given the control tools.
fn probe_dispatchers(
    hidden: &str,
    hidden_description: &str,
) -> (
    Arc<dyn AgentToolDispatcher>,
    Arc<CatalogControlVisibilityProvider>,
) {
    probe_dispatchers_with(
        &over_budget_description("Deferred MCP tool."),
        hidden,
        hidden_description,
    )
}

/// A description long enough that its tool's definition alone is over the
/// deferred catalog's byte budget.
fn over_budget_description(opening: &str) -> String {
    let mut over_budget = opening.to_string();
    while over_budget.len() <= meerkat_core::tool_catalog::DEFERRED_CATALOG_SCHEMA_VOLUME_THRESHOLD
    {
        over_budget
            .push_str(" Its description is long, so its definition alone is over the budget.");
    }
    over_budget
}

/// [`probe_dispatchers`] with the first allowed tool's description given.
fn probe_dispatchers_with(
    first_description: &str,
    hidden: &str,
    hidden_description: &str,
) -> (
    Arc<dyn AgentToolDispatcher>,
    Arc<CatalogControlVisibilityProvider>,
) {
    let tools = [
        session_tool("deferred_mcp_tool", first_description),
        session_tool("deferred_mcp_tool_two", "Second deferred MCP tool"),
        session_tool(hidden, hidden_description),
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
    (composed, visibility_provider)
}

/// What the model read in the probe's run: the hidden tool is in no provider
/// request, its call is refused, the search neither returns nor counts it,
/// its load answers as the load of an unregistered name, and the
/// deferred-catalog notice does not name it.
fn assert_hidden_tool_is_absent_for_the_model(
    session: &Session,
    seen_tools: &[Vec<String>],
    hidden: &str,
    hidden_description: &str,
) {
    assert!(
        seen_tools
            .iter()
            .all(|request| !request.iter().any(|name| name == hidden)),
        "the hidden tool is never in a provider request: {seen_tools:?}"
    );

    let result_of = |id: &str| {
        session
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
    };

    // Call: refused by the session's tool policy. This holds before and
    // after the catalog change, so it is checked first.
    let call = result_of("call-hidden");
    assert!(call.is_error, "a call of the hidden tool is refused");
    assert!(
        call.text_content()
            .contains(&format!("Tool '{hidden}' is not allowed by policy")),
        "the refusal is the tool policy's: {}",
        call.text_content()
    );

    // Search: only the tools the filters allow, and a total of those.
    let search = result_of("search-all");
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
        "search returns only what the session's filters allow: {search_text}"
    );
    assert_eq!(found["total_matches"], 2, "{search_text}");
    assert!(
        !search_text.contains(hidden) && !search_text.contains(hidden_description),
        "neither the name nor the description reaches the model: {search_text}"
    );

    // Load: the hidden tool answers exactly as the unregistered name.
    let hidden_load = result_of("load-hidden");
    let unknown_load = result_of("load-unknown");
    assert!(!hidden_load.is_error && !unknown_load.is_error);
    assert_eq!(
        unknown_load.text_content(),
        format!(
            r#"{{"catalog_exact":true,"accepted_names":[],"noop_names":[],"resolutions":[{{"name":"{UNKNOWN_TOOL}","accepted":false,"rejected_reason":"unknown_key"}}]}}"#
        )
    );
    assert_eq!(
        hidden_load.text_content().replace(hidden, UNKNOWN_TOOL),
        unknown_load.text_content(),
        "a load of the hidden tool differs from a load of an unregistered name only by the \
         name asked for"
    );

    // Notice: the deferred catalog offers only the allowed tools.
    let notices = session
        .messages()
        .iter()
        .filter_map(|message| match message {
            meerkat_core::Message::SystemNotice(notice) => notice.body.clone(),
            _ => None,
        })
        .filter(|body| body.starts_with("Deferred catalog changed at turn boundary:"))
        .collect::<Vec<_>>();
    assert_eq!(
        notices.first().map(String::as_str),
        Some(
            "Deferred catalog changed at turn boundary: new deferred tools available: \
             deferred_mcp_tool, deferred_mcp_tool_two"
        ),
        "{notices:?}"
    );
    assert!(
        notices.iter().all(|body| !body.contains(hidden)),
        "the notice never names the hidden tool: {notices:?}"
    );
}

/// A session with a deny list over a deferrable tool, through the composed
/// control plane and the agent loop: what the model reads from a search, a
/// load and the deferred-catalog notice never shows the denied tool, a load
/// of it answers as a load of a name no source registered, and a call of it
/// is refused as it was.
#[tokio::test]
async fn a_denied_deferred_tool_is_absent_from_search_load_and_notice_and_its_call_is_refused() {
    const DENIED_DESCRIPTION: &str = "Deferred MCP tool the deny list names";
    let (composed, visibility_provider) = probe_dispatchers(DENIED_TOOL, DENIED_DESCRIPTION);
    let client = Arc::new(HiddenToolProbeClient::new(DENIED_TOOL));
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
            [DENIED_TOOL.to_string()].into_iter().collect(),
        ))
        .expect("stage the deny list");

    let result = agent.run("prompt".to_string().into()).await.unwrap();
    assert_eq!(result.text, "done");

    let seen_tools = client.seen_tools.lock().unwrap().clone();
    assert_hidden_tool_is_absent_for_the_model(
        agent.session(),
        &seen_tools,
        DENIED_TOOL,
        DENIED_DESCRIPTION,
    );
}

/// A turn whose own overlay blocks a deferrable tool. The turn's tool list
/// would not hold the tool, so the turn's catalog does not either: the
/// model's search, load and notice in that turn do not show it, and a call
/// of it is refused.
#[tokio::test]
async fn a_turn_blocked_deferred_tool_is_absent_from_search_load_and_notice_in_that_turn() {
    const TURN_BLOCKED: &str = "turn_blocked_mcp_tool";
    const TURN_BLOCKED_DESCRIPTION: &str = "Deferred MCP tool this turn blocks";
    let (composed, visibility_provider) = probe_dispatchers(TURN_BLOCKED, TURN_BLOCKED_DESCRIPTION);
    let client = Arc::new(HiddenToolProbeClient::new(TURN_BLOCKED));
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
        .set_turn_tool_overlay(Some(meerkat_core::service::TurnToolOverlay {
            blocked_tools: Some(vec![TURN_BLOCKED.into()]),
            ..Default::default()
        }))
        .expect("set the turn overlay");

    let result = agent.run("prompt".to_string().into()).await.unwrap();
    assert_eq!(result.text, "done");

    let seen_tools = client.seen_tools.lock().unwrap().clone();
    assert_hidden_tool_is_absent_for_the_model(
        agent.session(),
        &seen_tools,
        TURN_BLOCKED,
        TURN_BLOCKED_DESCRIPTION,
    );
}

/// A deferrable tool the session's model cannot use. The capability filter
/// is the model's own: a model that cannot read an image in a tool result
/// does not get the image viewer. Built through the factory, so the filter
/// comes from the model's profile and the generated authority holds it. The
/// model's search, load and notice do not show the tool.
#[tokio::test]
async fn a_capability_filtered_deferred_tool_is_absent_from_search_load_and_notice() {
    const MODEL: &str = "text-only-model";
    const VIEWER_DESCRIPTION: &str = "Image viewer the model cannot use";
    let hidden = meerkat_core::VIEW_IMAGE_TOOL_NAME;

    // A configured model that declares no vision: it cannot take an image
    // in a tool result.
    let mut config = meerkat::Config::default();
    config.models.custom.insert(
        MODEL.to_string(),
        serde_json::from_value(serde_json::json!({ "provider": "openai" }))
            .expect("a configured model entry"),
    );

    let (composed, visibility_provider) = probe_dispatchers(hidden, VIEWER_DESCRIPTION);
    let client = Arc::new(HiddenToolProbeClient::for_model(
        hidden,
        Provider::OpenAI,
        MODEL,
    ));
    let build_client: Arc<dyn AgentLlmClient> = client.clone();
    let store: Arc<dyn AgentSessionStore> = Arc::new(NoopAgentStore);

    let mut agent = FacadeAgentBuilder::new()
        .with_config(config)
        .model(MODEL)
        .provider(Provider::OpenAI)
        .build(build_client, composed, store)
        .await
        .expect("facade builder should route through AgentFactory");
    visibility_provider.set_scope(agent.tool_scope().clone());
    assert_eq!(
        agent
            .tool_scope()
            .visibility_state()
            .expect("visibility state")
            .capability_base_filter,
        meerkat_core::capability_base_filter_for_image_tool_results(false),
        "precondition: the session holds its model's capability filter"
    );

    let result = agent.run("prompt".to_string().into()).await.unwrap();
    assert_eq!(result.text, "done");

    let seen_tools = client.seen_tools.lock().unwrap().clone();
    assert_hidden_tool_is_absent_for_the_model(
        agent.session(),
        &seen_tools,
        hidden,
        VIEWER_DESCRIPTION,
    );
}

/// One run of the probe model in a session that holds two small deferrable
/// tools and a deny list over a third one, whose description is given: the
/// session after the run, and the tool names of each provider request.
async fn run_probe_with_a_denied_tool(denied_description: &str) -> (Session, Vec<Vec<String>>) {
    let (composed, visibility_provider) =
        probe_dispatchers_with("Deferred MCP tool", DENIED_TOOL, denied_description);
    let client = Arc::new(HiddenToolProbeClient::new(DENIED_TOOL));
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
            [DENIED_TOOL.to_string()].into_iter().collect(),
        ))
        .expect("stage the deny list");

    let result = agent.run("prompt".to_string().into()).await.unwrap();
    assert_eq!(result.text, "done");

    let mut seen_tools = client.seen_tools.lock().unwrap().clone();
    for request in &mut seen_tools {
        request.sort();
    }
    (agent.session().clone(), seen_tools)
}

/// Known residue, pinned as it is today. Whether a session's own deferrable
/// tools are in its requests or wait behind the catalog can depend on a tool
/// its filters do not allow. Two sessions hold the same two small tools and
/// the same deny list over a third one. Where the denied tool is small too,
/// the session's tools are in its request and it is given no control tool.
/// Where the denied tool is large, the session is given the two control
/// tools in their place and is told that its own two tools are available
/// behind the catalog. That session is still told no name, no description
/// and no count of what it may not use: its search, its load and its notice
/// are the ones the rows above assert. It can tell that something is
/// withheld from it, and roughly how much. Closing this is a decision of its
/// own change, which then turns this row over knowingly.
#[tokio::test]
async fn a_withheld_tool_can_move_a_sessions_own_tools_behind_the_catalog() {
    const DENIED_DESCRIPTION: &str = "Deferred MCP tool the deny list names.";

    let (inline_session, inline_requests) = run_probe_with_a_denied_tool(DENIED_DESCRIPTION).await;
    assert_eq!(
        inline_requests[0],
        vec!["deferred_mcp_tool", "deferred_mcp_tool_two"],
        "with a small denied tool the session's own tools are in its request, with no control \
         tool: {inline_requests:?}"
    );
    assert!(
        inline_session.messages().iter().all(|message| {
            !matches!(
                message,
                meerkat_core::Message::SystemNotice(notice)
                    if notice.body.as_deref().is_some_and(|body| {
                        body.starts_with("Deferred catalog changed at turn boundary:")
                    })
            )
        }),
        "that session gets no deferred-catalog notice"
    );

    let large_description = over_budget_description(DENIED_DESCRIPTION);
    let (deferred_session, deferred_requests) =
        run_probe_with_a_denied_tool(&large_description).await;
    assert_eq!(
        deferred_requests[0],
        vec![LOAD_TOOL_NAME, SEARCH_TOOL_NAME],
        "with a large denied tool the session's own tools wait behind the catalog and its \
         request holds the two control tools: {deferred_requests:?}"
    );
    assert_hidden_tool_is_absent_for_the_model(
        &deferred_session,
        &deferred_requests,
        DENIED_TOOL,
        &large_description,
    );
}

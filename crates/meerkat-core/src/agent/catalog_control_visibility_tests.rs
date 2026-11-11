//! When the deferred catalog's two control tools are in a model request.
//!
//! `tool_catalog_search` and `tool_catalog_load` are useful only while
//! something can be searched or loaded. A session whose tool source reports
//! an exact catalog mounts them when it has a deferrable tool, and they are
//! in a request only when
//!
//! - a deferrable tool waits behind the catalog,
//! - a tool source is still connecting, or
//! - a tool source declares that it needs them whatever the mode
//!   (`may_require_catalog_control_plane`).
//!
//! Otherwise they are left out of the request, and a call of one is refused
//! like a call of any tool the session does not show.
#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::too_many_lines
)]

use crate as meerkat_core;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use meerkat_core::tool_catalog::DEFERRED_CATALOG_SCHEMA_VOLUME_THRESHOLD;
use meerkat_core::{
    AgentBuilder, AgentError, AgentLlmClient, AgentSessionStore, AgentToolDispatcher,
    AssistantBlock, LlmStreamResult, Message, Provider, StopReason, ToolCallView,
    ToolCatalogCapabilities, ToolCatalogEntry, ToolCatalogMode, ToolCatalogVisibility, ToolDef,
    ToolProvenance, ToolResult, ToolSourceKind, TurnUsage, Usage,
};
use serde_json::json;

const SEARCH: &str = "tool_catalog_search";
const LOAD: &str = "tool_catalog_load";
const MODEL: &str = "test-model";

fn provenance() -> ToolProvenance {
    ToolProvenance {
        kind: ToolSourceKind::Callback,
        source_id: "test".into(),
        artifact: None,
    }
}

fn tool(name: &str, description: String) -> Arc<ToolDef> {
    Arc::new(ToolDef {
        audience: Default::default(),
        name: name.into(),
        description,
        input_schema: json!({ "type": "object" }),
        provenance: Some(provenance()),
    })
}

/// A deferrable tool far under the deferred catalog's byte budget.
fn small(name: &str) -> Arc<ToolDef> {
    tool(name, format!("small tool {name}"))
}

/// A deferrable tool that alone is over the deferred catalog's byte budget.
fn large(name: &str) -> Arc<ToolDef> {
    tool(
        name,
        format!(
            "large tool {name} {}",
            "x".repeat(DEFERRED_CATALOG_SCHEMA_VOLUME_THRESHOLD)
        ),
    )
}

fn control(name: &str) -> Arc<ToolDef> {
    Arc::new(ToolDef {
        audience: Default::default(),
        name: name.into(),
        description: format!("control tool {name}"),
        input_schema: json!({ "type": "object" }),
        provenance: None,
    })
}

/// A tool source with an exact catalog of deferrable tools that a test can
/// change between requests, as a client that registers tools or a server
/// that connects does.
struct Source {
    session: Mutex<Vec<Arc<ToolDef>>>,
    /// Whether the two control tools are mounted beside the session tools.
    control_plane: bool,
    /// Whether the source declares that it needs the control tools whatever
    /// the mode.
    declares_control_plane: bool,
    connecting: Mutex<Vec<String>>,
    dispatched: Mutex<Vec<String>>,
}

impl Source {
    fn new(session: Vec<Arc<ToolDef>>) -> Self {
        Self {
            session: Mutex::new(session),
            control_plane: true,
            declares_control_plane: false,
            connecting: Mutex::new(Vec::new()),
            dispatched: Mutex::new(Vec::new()),
        }
    }

    fn add(&self, tool: Arc<ToolDef>) {
        self.session.lock().unwrap().push(tool);
    }

    fn dispatched(&self) -> Vec<String> {
        self.dispatched.lock().unwrap().clone()
    }
}

#[async_trait]
impl AgentToolDispatcher for Source {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        let mut tools = self.session.lock().unwrap().clone();
        if self.control_plane {
            tools.push(control(SEARCH));
            tools.push(control(LOAD));
        }
        tools.into()
    }

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        ToolCatalogCapabilities {
            exact_catalog: true,
            may_require_catalog_control_plane: self.declares_control_plane,
        }
    }

    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        let mut entries = self
            .session
            .lock()
            .unwrap()
            .iter()
            .map(|tool| ToolCatalogEntry::session_deferred(Arc::clone(tool), true, provenance()))
            .collect::<Vec<_>>();
        if self.control_plane {
            entries.push(ToolCatalogEntry::control_inline(control(SEARCH), true));
            entries.push(ToolCatalogEntry::control_inline(control(LOAD), true));
        }
        entries.into()
    }

    fn pending_catalog_sources(&self) -> Arc<[String]> {
        self.connecting.lock().unwrap().clone().into()
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        self.dispatched.lock().unwrap().push(call.name.to_string());
        let mut outcome = meerkat_core::ToolDispatchOutcome::sync_result(ToolResult::new(
            call.id.to_string(),
            format!("ran {}", call.name),
            false,
        ));
        if call.name == LOAD {
            let args: serde_json::Value = serde_json::from_str(call.args.get()).unwrap();
            let authorities = args["names"]
                .as_array()
                .unwrap()
                .iter()
                .map(|name| {
                    meerkat_core::DeferredToolLoadAuthority::new(
                        name.as_str().unwrap(),
                        meerkat_core::ToolVisibilityWitness {
                            last_seen_provenance: Some(provenance()),
                        },
                    )
                })
                .collect();
            outcome
                .session_effects
                .push(meerkat_core::ops::SessionEffect::RequestDeferredTools { authorities });
        }
        Ok(outcome)
    }
}

/// What one model request carried: its tool names in order, and the tool
/// results in its messages as `(call id, is_error, text)`.
#[derive(Clone, Debug)]
struct Request {
    tools: Vec<String>,
    results: Vec<(String, bool, String)>,
}

/// A model that answers each request with the next scripted tool call, and
/// with text once the script is used up.
struct Model {
    calls: Mutex<VecDeque<(&'static str, serde_json::Value)>>,
    requests: Mutex<Vec<Request>>,
}

impl Model {
    fn new() -> Self {
        Self {
            calls: Mutex::new(VecDeque::new()),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn will_call(&self, name: &'static str, args: serde_json::Value) {
        self.calls.lock().unwrap().push_back((name, args));
    }

    fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl AgentLlmClient for Model {
    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[Arc<ToolDef>],
        _max_tokens: u32,
        _temperature: Option<f32>,
        _provider_params: Option<&meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
    ) -> Result<LlmStreamResult, AgentError> {
        let request = Request {
            tools: tools.iter().map(|tool| tool.name.to_string()).collect(),
            results: messages
                .iter()
                .flat_map(|message| match message {
                    Message::ToolResults { results, .. } => results.as_slice(),
                    _ => &[],
                })
                .map(|result| {
                    (
                        result.tool_use_id.clone(),
                        result.is_error,
                        result.text_content(),
                    )
                })
                .collect(),
        };
        let number = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(request);
            requests.len()
        };
        let usage = TurnUsage::host_declared(Provider::Other, MODEL, Usage::default()).into_inner();
        Ok(match self.calls.lock().unwrap().pop_front() {
            Some((name, args)) => LlmStreamResult::new(
                vec![AssistantBlock::ToolUse {
                    id: format!("call-{number}"),
                    name: name.into(),
                    args: serde_json::value::RawValue::from_string(args.to_string()).unwrap(),
                    meta: None,
                }],
                StopReason::ToolUse,
                usage,
            ),
            None => LlmStreamResult::new(
                vec![AssistantBlock::Text {
                    text: "done".to_string(),
                    meta: None,
                }],
                StopReason::EndTurn,
                usage,
            ),
        })
    }

    fn provider(&self) -> Provider {
        Provider::Other
    }

    fn model(&self) -> &'static str {
        MODEL
    }
}

struct NoopStore;

#[async_trait]
impl AgentSessionStore for NoopStore {
    async fn save(&self, _session: &meerkat_core::Session) -> Result<(), AgentError> {
        Ok(())
    }

    async fn load(&self, _id: &str) -> Result<Option<meerkat_core::Session>, AgentError> {
        Ok(None)
    }
}

type SourceAgent = meerkat_core::Agent<Model, Source, NoopStore>;

async fn agent(model: &Arc<Model>, source: &Arc<Source>) -> SourceAgent {
    let mut session = meerkat_core::Session::new();
    session
        .set_build_state(meerkat_core::SessionBuildState::default())
        .expect("test session build state should serialize");
    let owner: Arc<dyn meerkat_core::ToolVisibilityOwner> =
        Arc::new(crate::tool_scope::GeneratedTestToolVisibilityOwner::new());
    let mut agent = AgentBuilder::new()
        .resume_session(session)
        .with_turn_state_handle(Arc::new(
            crate::agent::test_turn_state_handle::TestTurnStateHandle::new(),
        ))
        .with_runtime_execution_kind_for_test(
            meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
        )
        .with_tool_visibility_owner(
            crate::tool_scope::generated_test_tool_visibility_owner_from(owner),
        )
        .build_standalone(Arc::clone(model), Arc::clone(source), Arc::new(NoopStore))
        .await;
    agent.config.max_turns = Some(8);
    agent
}

/// Run one prompt to its text answer and return the requests it made.
async fn run(agent: &mut SourceAgent, model: &Model) -> Vec<Request> {
    let before = model.requests().len();
    let result = agent
        .run("prompt".to_string().into())
        .await
        .expect("the run completes");
    assert_eq!(result.text, "done");
    model.requests()[before..].to_vec()
}

fn names(tools: &[&str]) -> Vec<String> {
    tools.iter().map(|name| (*name).to_string()).collect()
}

fn view_visibility(agent: &SourceAgent, name: &str) -> Option<ToolCatalogVisibility> {
    agent
        .tool_catalog_view()
        .expect("catalog view")
        .entries
        .iter()
        .find(|entry| entry.name == name)
        .map(|entry| entry.visibility)
}

/// A session whose deferrable tools are under the byte budget sends them
/// and nothing else: the control tools are mounted and left out, and the
/// host's catalog view says so.
#[tokio::test]
async fn an_inline_session_sends_neither_control_tool() {
    let model = Arc::new(Model::new());
    let source = Arc::new(Source::new(vec![small("alpha"), small("beta")]));
    let mut agent = agent(&model, &source).await;

    let view = agent.tool_catalog_view().expect("catalog view");
    assert_eq!(view.mode, ToolCatalogMode::Inline);
    for name in [SEARCH, LOAD] {
        assert_eq!(
            view_visibility(&agent, name),
            Some(ToolCatalogVisibility::Hidden),
            "{name} has nothing to search or load"
        );
    }

    let requests = run(&mut agent, &model).await;
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].tools,
        names(&["alpha", "beta"]),
        "the request carries the session's tools, in order, and no control tool"
    );
}

/// A session that starts with a deferrable tool mounts the control tools
/// hidden. When a later tool takes it over the byte budget, the next request
/// carries both control tools, the new tool waits behind the catalog, and a
/// load brings it into the request after.
#[tokio::test]
async fn a_session_that_grows_past_the_budget_gets_both_control_tools_and_loads() {
    let model = Arc::new(Model::new());
    let source = Arc::new(Source::new(vec![small("alpha")]));
    let mut agent = agent(&model, &source).await;

    let requests = run(&mut agent, &model).await;
    assert_eq!(
        requests[0].tools,
        names(&["alpha"]),
        "under the budget the control tools are left out"
    );

    source.add(large("big"));
    model.will_call(LOAD, json!({ "names": ["big"] }));
    let requests = run(&mut agent, &model).await;
    assert_eq!(requests.len(), 2);
    for name in [SEARCH, LOAD] {
        assert!(
            requests[0].tools.iter().any(|tool| tool == name),
            "over the budget {name} is in the request: {:?}",
            requests[0].tools
        );
    }
    assert!(
        !requests[0].tools.iter().any(|tool| tool == "big"),
        "the tool that arrived over the budget waits behind the catalog"
    );
    assert!(
        requests[1].tools.iter().any(|tool| tool == "big"),
        "the loaded tool is in the next request: {:?}",
        requests[1].tools
    );
    assert_eq!(
        requests[1].results,
        vec![("call-2".to_string(), false, format!("ran {LOAD}"))],
        "the load was dispatched and answered"
    );
}

/// A session that starts with nothing to defer has no control plane. Tools
/// that arrive later stay in every request for its lifetime, whatever their
/// size: nothing is hidden and nothing has to be loaded.
#[tokio::test]
async fn a_session_that_starts_with_nothing_to_defer_keeps_later_tools_inline() {
    let model = Arc::new(Model::new());
    let source = Arc::new(Source {
        control_plane: false,
        ..Source::new(Vec::new())
    });
    let mut agent = agent(&model, &source).await;

    let requests = run(&mut agent, &model).await;
    assert_eq!(requests[0].tools, names(&[]));

    source.add(large("big"));
    source.add(large("bigger"));
    let requests = run(&mut agent, &model).await;
    assert_eq!(
        requests[0].tools,
        names(&["big", "bigger"]),
        "every later tool is offered directly"
    );
    assert_eq!(
        agent.tool_catalog_view().expect("catalog view").mode,
        ToolCatalogMode::Inline
    );
}

/// A source that declares it needs the control tools keeps both in every
/// request: while the session is inline, while tools wait, and once every
/// tool is loaded.
#[tokio::test]
async fn a_source_that_declares_the_control_plane_keeps_both_control_tools() {
    let model = Arc::new(Model::new());
    let source = Arc::new(Source {
        declares_control_plane: true,
        ..Source::new(vec![small("alpha")])
    });
    let mut agent = agent(&model, &source).await;

    let requests = run(&mut agent, &model).await;
    assert_eq!(
        requests[0].tools,
        names(&["alpha", SEARCH, LOAD]),
        "inline, the declared control tools are in the request"
    );

    source.add(large("big"));
    model.will_call(LOAD, json!({ "names": ["big"] }));
    let requests = run(&mut agent, &model).await;
    assert_eq!(requests.len(), 2);
    for request in &requests {
        for name in [SEARCH, LOAD] {
            assert!(
                request.tools.iter().any(|tool| tool == name),
                "{name} stays in the request: {:?}",
                request.tools
            );
        }
    }
    assert!(
        requests[1].tools.iter().any(|tool| tool == "big"),
        "the waiting tool is loaded: {:?}",
        requests[1].tools
    );

    let requests = run(&mut agent, &model).await;
    for name in [SEARCH, LOAD] {
        assert!(
            requests[0].tools.iter().any(|tool| tool == name),
            "{name} stays in the request on a later run: {:?}",
            requests[0].tools
        );
    }
}

/// A deferred session whose every tool is loaded sends its tools and no
/// control tool: nothing is left to search or load.
#[tokio::test]
async fn a_deferred_session_with_every_tool_loaded_sends_neither_control_tool() {
    let model = Arc::new(Model::new());
    let source = Arc::new(Source::new(vec![large("big")]));
    let mut agent = agent(&model, &source).await;

    model.will_call(LOAD, json!({ "names": ["big"] }));
    let requests = run(&mut agent, &model).await;
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].tools,
        names(&[SEARCH, LOAD]),
        "the tool waits behind the catalog, so both control tools are offered"
    );
    assert_eq!(
        requests[1].tools,
        names(&["big"]),
        "with every tool loaded the control tools are left out"
    );

    let requests = run(&mut agent, &model).await;
    assert_eq!(
        requests[0].tools,
        names(&["big"]),
        "and stay out afterwards"
    );
}

/// While a tool source is still connecting both control tools are in the
/// request, so the model can search what arrives. Once it has connected and
/// the session is under the budget they are left out again.
#[tokio::test]
async fn a_connecting_source_shows_both_control_tools_until_it_has_connected() {
    let model = Arc::new(Model::new());
    let source = Arc::new(Source::new(vec![small("alpha")]));
    source.connecting.lock().unwrap().push("late".to_string());
    let mut agent = agent(&model, &source).await;

    let requests = run(&mut agent, &model).await;
    for name in [SEARCH, LOAD] {
        assert!(
            requests[0].tools.iter().any(|tool| tool == name),
            "{name} is offered while a source connects: {:?}",
            requests[0].tools
        );
    }

    source.connecting.lock().unwrap().clear();
    let requests = run(&mut agent, &model).await;
    for name in [SEARCH, LOAD] {
        assert!(
            !requests[0].tools.iter().any(|tool| tool == name),
            "{name} is left out once every source has connected: {:?}",
            requests[0].tools
        );
    }
}

/// A control tool that is left out of the request cannot be called: the
/// call is refused like a call of any tool the session does not show, and
/// the tool source never sees it.
#[tokio::test]
async fn a_control_tool_left_out_of_the_request_cannot_be_called() {
    let model = Arc::new(Model::new());
    let source = Arc::new(Source::new(vec![small("alpha")]));
    let mut agent = agent(&model, &source).await;

    model.will_call(LOAD, json!({ "names": ["alpha"] }));
    let requests = run(&mut agent, &model).await;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].tools, names(&["alpha"]));

    let refusal = meerkat_core::ops::terminal_tool_outcome_for_error(
        "call-1".to_string(),
        meerkat_core::ToolError::access_denied(LOAD),
    )
    .result;
    assert_eq!(
        requests[1].results,
        vec![("call-1".to_string(), true, refusal.text_content())],
        "the call is answered with the refusal of a tool that is not shown"
    );
    assert!(
        source.dispatched().is_empty(),
        "a refused call never reaches the tool source: {:?}",
        source.dispatched()
    );
}

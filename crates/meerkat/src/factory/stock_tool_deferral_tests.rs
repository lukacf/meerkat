//! A host's stock tool deferral policy on sessions the factory builds.
//!
//! The rows build real sessions: the comms tools are the comms crate's own,
//! mounted through a comms runtime, and memory search is the memory crate's
//! own. The mob tools are stand-ins with the provenance kind and source the
//! mob crates give theirs (the facade does not depend on them). A stand-in
//! mounted as the agent mob surface reports no exact catalog, as the mob
//! crates' tools do.
//!
//! These rows live under `factory` because they read the factory's private
//! prompt guidance text.

use std::collections::BTreeSet;
use std::pin::Pin;
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use futures::stream;
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::types::{ToolCall, ToolCallView};
use meerkat_core::{
    AgentToolDispatcher, Config, Message, Provider, RuntimeBuildMode, Session,
    StockToolDeferralPolicy, StockToolSourceKind, StopReason, ToolCatalogDeferredEligibility,
    ToolCatalogEntry, ToolCatalogMode, ToolCatalogVisibility, ToolCategoryOverride, ToolDef,
    ToolProvenance, ToolSourceKind,
};

use crate::{AgentBuildConfig, AgentFactory, DynAgent};

const COMMS_TOOLS: [&str; 5] = [
    "send_message",
    "reply_to_peer",
    "send_request",
    "send_response",
    "peers",
];
const COMMS_TOOLS_THAT_WAIT: [&str; 3] = ["send_message", "send_request", "peers"];
const CONTROL_TOOLS: [&str; 2] = ["tool_catalog_search", "tool_catalog_load"];
const HOST_ALPHA: &str = "host_alpha";
const HOST_BETA: &str = "host_beta";

type Requests = Arc<StdMutex<Vec<LlmRequest>>>;

/// A model that answers every request with one word and records the request.
struct RecordingClient {
    requests: Requests,
}

#[async_trait]
impl LlmClient for RecordingClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        self.requests.lock().unwrap().push(request.clone());
        Box::pin(stream::iter(vec![
            Ok(LlmEvent::TextDelta {
                delta: "ok".to_string(),
                meta: None,
            }),
            Ok(LlmEvent::UsageUpdate {
                usage: meerkat_core::TurnUsage::host_declared(
                    Provider::OpenAI,
                    &request.model,
                    meerkat_core::Usage::default(),
                ),
            }),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::success(StopReason::EndTurn),
            }),
        ]))
    }

    fn provider(&self) -> Provider {
        Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

/// A tool source standing in for a host's own tools and for the mob tool
/// sources. Its catalog is exact unless it stands in for the agent mob
/// surface.
struct StandInTools {
    catalog: Arc<[ToolCatalogEntry]>,
    exact: bool,
}

impl StandInTools {
    /// A source that reports an exact catalog.
    fn new(catalog: Vec<ToolCatalogEntry>) -> Self {
        Self {
            catalog: catalog.into(),
            exact: true,
        }
    }

    /// A source that reports no exact catalog.
    fn inexact(catalog: Vec<ToolCatalogEntry>) -> Self {
        Self {
            catalog: catalog.into(),
            exact: false,
        }
    }
}

#[async_trait]
impl AgentToolDispatcher for StandInTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.catalog
            .iter()
            .map(|entry| Arc::clone(&entry.tool))
            .collect::<Vec<_>>()
            .into()
    }

    fn tool_catalog_capabilities(&self) -> meerkat_core::ToolCatalogCapabilities {
        meerkat_core::ToolCatalogCapabilities {
            exact_catalog: self.exact,
            may_require_catalog_control_plane: false,
        }
    }

    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        Arc::clone(&self.catalog)
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::error::ToolError> {
        Ok(meerkat_core::types::ToolResult::new(
            call.id.to_string(),
            format!("ran {}", call.name),
            false,
        )
        .into())
    }
}

struct StandInMobToolsFactory {
    dispatcher: Arc<dyn AgentToolDispatcher>,
}

#[async_trait]
impl meerkat_core::service::MobToolsFactory for StandInMobToolsFactory {
    async fn build_mob_tools(
        &self,
        _args: meerkat_core::service::MobToolsBuildArgs,
    ) -> Result<Arc<dyn AgentToolDispatcher>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Arc::clone(&self.dispatcher))
    }
}

fn mob_provenance() -> ToolProvenance {
    ToolProvenance {
        kind: ToolSourceKind::Mob,
        source_id: "mob".into(),
        artifact: None,
    }
}

fn comms_provenance() -> ToolProvenance {
    ToolProvenance {
        kind: ToolSourceKind::Comms,
        source_id: "comms".into(),
        artifact: None,
    }
}

fn host_provenance() -> ToolProvenance {
    ToolProvenance {
        kind: ToolSourceKind::Callback,
        source_id: "host-tools".into(),
        artifact: None,
    }
}

fn tool(name: &str, provenance: ToolProvenance) -> Arc<ToolDef> {
    let mut tool = ToolDef::new(
        name,
        "A stand-in tool.",
        serde_json::json!({ "type": "object" }),
    );
    tool.provenance = Some(provenance);
    Arc::new(tool)
}

/// An inline mob tool, as the mob tool sources list theirs.
fn mob_tool(name: &str) -> ToolCatalogEntry {
    ToolCatalogEntry::session_inline(tool(name, mob_provenance()), true)
}

/// One small inline host tool from a source with an exact catalog: with it
/// among a session's external tools the session's composed catalog is exact.
fn exact_host_tool() -> Vec<ToolCatalogEntry> {
    vec![ToolCatalogEntry::session_inline(
        tool("host_inline", host_provenance()),
        true,
    )]
}

/// Two host tools that wait behind the deferred catalog. They reach the
/// catalog's tool count, so a session with an exact catalog that mounts
/// them defers with or without a policy.
fn two_deferred_host_tools() -> Vec<ToolCatalogEntry> {
    [HOST_ALPHA, HOST_BETA]
        .into_iter()
        .map(|name| {
            ToolCatalogEntry::session_deferred(
                tool(name, host_provenance()),
                true,
                host_provenance(),
            )
        })
        .collect()
}

fn comms_policy() -> StockToolDeferralPolicy {
    StockToolDeferralPolicy::none().deferring(StockToolSourceKind::Comms)
}

/// A factory rooted in `root` with no builtin tools and, when `comms` names
/// one, a comms runtime of that name that mounts the comms tools.
fn host_factory(root: &std::path::Path, comms: Option<&str>) -> AgentFactory {
    let factory = AgentFactory::new(root.join("sessions")).builtins(false);
    match comms {
        Some(name) => factory.with_comms_runtime(Arc::new(
            meerkat_comms::CommsRuntime::inproc_only(name).expect("an in-process comms runtime"),
        )),
        None => factory,
    }
}

fn comms_name(label: &str) -> String {
    format!("stock-deferral-{label}-{}", meerkat_core::SessionId::new())
}

/// A session the factory builds on its own runtime registration, continuing
/// `session`, with `external` as the host's tools.
async fn build(
    factory: &AgentFactory,
    session: Session,
    external: Vec<ToolCatalogEntry>,
    requests: &Requests,
) -> DynAgent {
    build_with(factory, session, external, requests, |_| {}).await
}

async fn build_with(
    factory: &AgentFactory,
    session: Session,
    external: Vec<ToolCatalogEntry>,
    requests: &Requests,
    configure: impl FnOnce(&mut AgentBuildConfig),
) -> DynAgent {
    let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
    let bindings = runtime
        .prepare_bindings(session.id().clone())
        .await
        .expect("runtime bindings");
    let mut build = AgentBuildConfig::new("gpt-5.6-sol");
    build.provider = Some(Provider::OpenAI);
    build.llm_client_override = Some(Arc::new(RecordingClient {
        requests: Arc::clone(requests),
    }));
    build.resume_session = Some(session);
    build.runtime_build_mode = RuntimeBuildMode::SessionOwned(bindings);
    build.override_builtins = ToolCategoryOverride::Disable;
    if !external.is_empty() {
        build.external_tools = Some(Arc::new(StandInTools::new(external)));
    }
    configure(&mut build);
    factory
        .build_agent(build, &Config::default())
        .await
        .expect("the session builds")
}

/// One turn, so the agent crosses a boundary; returns the tool names its
/// model request listed.
async fn listed_in_a_turn(agent: &mut DynAgent, text: &str, requests: &Requests) -> Vec<String> {
    let before = requests.lock().unwrap().len();
    agent.set_runtime_execution_kind(Some(
        meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
    ));
    agent
        .run(meerkat_core::ContentInput::Text(text.into()))
        .await
        .expect("an agent turn");
    let requests = requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        before + 1,
        "the turn made one model request"
    );
    requests[before]
        .tools
        .iter()
        .map(|tool| tool.name.as_str().to_string())
        .collect()
}

async fn call(
    agent: &mut DynAgent,
    name: &str,
    args: serde_json::Value,
) -> meerkat_core::ToolDispatchOutcome {
    agent
        .dispatch_external_tool_call(ToolCall::new(
            format!("{name}-call"),
            name.to_string(),
            args,
        ))
        .await
        .expect("an external call outcome")
}

/// Ask for deferred tools through the catalog control plane; returns the
/// load resolution of each name.
async fn load(agent: &mut DynAgent, names: &[&str]) -> Vec<serde_json::Value> {
    let outcome = call(
        agent,
        "tool_catalog_load",
        serde_json::json!({ "names": names }),
    )
    .await;
    let text = outcome.result.text_content();
    let response: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("a catalog load response ({error}): {text}"));
    response["resolutions"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("load resolutions: {text}"))
}

fn resolution<'a>(resolutions: &'a [serde_json::Value], name: &str) -> &'a serde_json::Value {
    resolutions
        .iter()
        .find(|resolution| resolution["name"] == name)
        .unwrap_or_else(|| panic!("a load resolution for {name}: {resolutions:?}"))
}

fn view(agent: &DynAgent) -> meerkat_core::ToolCatalogView {
    agent.tool_catalog_view().expect("a catalog view")
}

fn visibility(agent: &DynAgent, name: &str) -> Option<ToolCatalogVisibility> {
    view(agent)
        .entries
        .iter()
        .find(|entry| entry.name.as_str() == name)
        .map(|entry| entry.visibility)
}

fn eligibility(agent: &DynAgent, name: &str) -> ToolCatalogDeferredEligibility {
    view(agent)
        .entries
        .iter()
        .find(|entry| entry.name.as_str() == name)
        .unwrap_or_else(|| panic!("no catalog entry named {name}"))
        .deferred_eligibility
        .clone()
}

/// The names the session's catalog reports as deferred-eligible.
fn eligible(agent: &DynAgent) -> BTreeSet<String> {
    view(agent)
        .entries
        .iter()
        .filter(|entry| {
            matches!(
                entry.deferred_eligibility,
                ToolCatalogDeferredEligibility::DeferredEligible { .. }
            )
        })
        .map(|entry| entry.name.as_str().to_string())
        .collect()
}

fn names(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

fn system_prompt(agent: &DynAgent) -> String {
    match agent.session().messages().first() {
        Some(Message::System(message)) => message.content.clone(),
        other => panic!("a system prompt as the first message, got {other:?}"),
    }
}

/// The tool names the deferred catalog notice of the session's transcript
/// offered as newly deferred, across its boundaries.
fn announced_as_deferred(agent: &DynAgent) -> BTreeSet<String> {
    const CLAUSE: &str = "new deferred tools available: ";
    let transcript =
        serde_json::to_string(agent.session().messages()).expect("a serializable transcript");
    let mut announced = BTreeSet::new();
    let mut rest = transcript.as_str();
    while let Some(start) = rest.find(CLAUSE) {
        rest = &rest[start + CLAUSE.len()..];
        let end = rest.find([';', '"', '\\']).unwrap_or(rest.len());
        announced.extend(rest[..end].split(", ").map(str::to_string));
    }
    announced
}

fn listed(tools: &[String], name: &str) -> bool {
    tools.iter().any(|tool| tool == name)
}

/// The names the typed deltas of the session's deferred-catalog notices
/// carry as newly deferred, across its boundaries.
fn announced_in_typed_deltas(agent: &DynAgent) -> BTreeSet<String> {
    agent
        .session()
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::SystemNotice(notice) => Some(notice),
            _ => None,
        })
        .flat_map(|notice| notice.blocks.iter())
        .filter_map(|block| match block {
            meerkat_core::types::SystemNoticeBlock::ToolConfig { payload } => {
                payload.deferred_catalog_delta.as_ref()
            }
            _ => None,
        })
        .flat_map(|delta| delta.added_hidden_names.iter())
        .map(ToString::to_string)
        .collect()
}

/// What a built session shows of itself after its first turn: its catalog
/// view, its system prompt, its first model request's tool definitions
/// (serialized) and the tools its notices offered as newly deferred.
#[derive(Debug, PartialEq)]
struct BuiltSession {
    view: meerkat_core::ToolCatalogView,
    prompt: String,
    request_tools: String,
    announced: BTreeSet<String>,
}

impl BuiltSession {
    fn entry(&self, name: &str) -> &meerkat_core::ToolCatalogViewEntry {
        self.view
            .entries
            .iter()
            .find(|entry| entry.name.as_str() == name)
            .unwrap_or_else(|| panic!("no catalog entry named {name}"))
    }
}

/// A new session of a factory with a comms runtime named `comms`, through
/// its first turn. Two sessions compared with each other are built one after
/// the other under one comms name, so their prompts cannot differ by it.
async fn built_session(
    comms: &str,
    configure_factory: impl FnOnce(AgentFactory) -> AgentFactory,
    external: Vec<ToolCatalogEntry>,
    configure_build: impl FnOnce(&mut AgentBuildConfig),
) -> BuiltSession {
    let temp = tempfile::tempdir().expect("temp root");
    let factory = configure_factory(host_factory(temp.path(), Some(comms)));
    let requests = Requests::default();
    let mut agent = build_with(
        &factory,
        Session::new(),
        external,
        &requests,
        configure_build,
    )
    .await;
    let prompt = system_prompt(&agent);
    listed_in_a_turn(&mut agent, "first", &requests).await;
    let request_tools = serde_json::to_string(&requests.lock().unwrap()[0].tools)
        .expect("serializable tool definitions");
    let built = BuiltSession {
        view: view(&agent),
        prompt,
        request_tools,
        announced: announced_as_deferred(&agent),
    };
    agent.release_owned_resources().await;
    built
}

/// `factory` with a stand-in agent mob surface mounted. Like the mob
/// crates' tools, the stand-in reports no exact catalog.
fn with_mob_surface(factory: AgentFactory) -> AgentFactory {
    let surface: Arc<dyn AgentToolDispatcher> = Arc::new(StandInTools::inexact(vec![
        mob_tool("delegate"),
        mob_tool("mob_spawn_member"),
    ]));
    factory
        .mob(true)
        .mob_tools_factory(Arc::new(StandInMobToolsFactory {
            dispatcher: surface,
        }))
}

/// The authority a build needs for the agent mob surface to be mounted.
fn mob_authority(build: &mut AgentBuildConfig) {
    build.mob_tool_authority_context = Some(
        meerkat_runtime::mob_operator_authority::create_only_mob_operator_authority()
            .expect("generated create-only mob authority"),
    );
}

/// A factory given the empty policy builds the session a factory never
/// given one builds. The catalog view, the system prompt, the first
/// request's tool definitions and the notices are equal.
#[tokio::test]
async fn an_empty_policy_changes_nothing() {
    const NAME: &str = "stock-deferral-empty-policy";
    let external = || {
        let mut external = two_deferred_host_tools();
        external.push(mob_tool("spawn_member"));
        external
    };
    let plain = built_session(NAME, |factory| factory, external(), |_| {}).await;
    let empty = built_session(
        NAME,
        |factory| factory.stock_tool_deferral(StockToolDeferralPolicy::none()),
        external(),
        |_| {},
    )
    .await;

    assert_eq!(plain.view.mode, ToolCatalogMode::Deferred, "precondition");
    for name in COMMS_TOOLS.into_iter().chain(["spawn_member"]) {
        assert_eq!(
            plain.entry(name).deferred_eligibility,
            ToolCatalogDeferredEligibility::InlineOnly,
            "precondition: without a policy {name} is inline only"
        );
    }
    assert_eq!(empty, plain);
}

/// On a built session the comms policy makes `send_message`,
/// `send_request` and `peers` deferred-eligible, owned by the comms
/// provenance. `reply_to_peer` and `send_response` stay inline only, and so
/// do a mob tool and the host's own inline tool beside them.
#[tokio::test]
async fn comms_policy_defers_only_unpinned_comms_tools() {
    let temp = tempfile::tempdir().expect("temp root");
    let factory = host_factory(temp.path(), Some(&comms_name("eligibility")))
        .stock_tool_deferral(comms_policy());
    let requests = Requests::default();
    let mut external = exact_host_tool();
    external.push(mob_tool("spawn_member"));
    let agent = build(&factory, Session::new(), external, &requests).await;

    assert_eq!(eligible(&agent), names(&COMMS_TOOLS_THAT_WAIT));
    for name in COMMS_TOOLS_THAT_WAIT {
        assert_eq!(
            eligibility(&agent, name),
            ToolCatalogDeferredEligibility::DeferredEligible {
                provenance: comms_provenance()
            },
            "{name} is owned behind the catalog by the comms provenance"
        );
    }
    for name in [
        "reply_to_peer",
        "send_response",
        "spawn_member",
        "host_inline",
    ] {
        assert_eq!(
            eligibility(&agent, name),
            ToolCatalogDeferredEligibility::InlineOnly,
            "{name} stays in every request"
        );
    }
}

/// In a session that defers, the first request leaves out the comms tools
/// that may wait and the notice names them. An unloaded call is refused; a
/// load is accepted; the next request carries the loaded tools and they run
/// as they do in a session without the policy.
#[tokio::test]
async fn a_deferred_stock_tool_loads_and_runs() {
    let temp = tempfile::tempdir().expect("temp root");
    let factory =
        host_factory(temp.path(), Some(&comms_name("load"))).stock_tool_deferral(comms_policy());
    let requests = Requests::default();
    let mut agent = build(
        &factory,
        Session::new(),
        two_deferred_host_tools(),
        &requests,
    )
    .await;
    assert_eq!(view(&agent).mode, ToolCatalogMode::Deferred);

    let first = listed_in_a_turn(&mut agent, "first", &requests).await;
    for name in COMMS_TOOLS_THAT_WAIT {
        assert!(!listed(&first, name), "{name} waits: {first:?}");
        assert_eq!(
            visibility(&agent, name),
            Some(ToolCatalogVisibility::Deferred)
        );
    }
    for name in CONTROL_TOOLS {
        assert!(listed(&first, name), "{name} is in the request: {first:?}");
    }
    let announced = announced_as_deferred(&agent);
    for name in COMMS_TOOLS_THAT_WAIT {
        assert!(
            announced.contains(name),
            "the notice names {name}: {announced:?}"
        );
    }
    for name in ["reply_to_peer", "send_response"] {
        assert!(
            !announced.contains(name),
            "the notice does not offer {name}: {announced:?}"
        );
    }

    let unloaded = call(&mut agent, "peers", serde_json::json!({})).await;
    assert!(
        unloaded.result.is_error,
        "a call of a tool that waits is refused: {}",
        unloaded.result.text_content()
    );

    let loaded = load(&mut agent, &["peers", "send_message"]).await;
    for name in ["peers", "send_message"] {
        assert_eq!(resolution(&loaded, name)["accepted"], true, "{loaded:?}");
    }
    let second = listed_in_a_turn(&mut agent, "second", &requests).await;
    for name in ["peers", "send_message"] {
        assert!(listed(&second, name), "{name} is loaded: {second:?}");
        assert_eq!(
            visibility(&agent, name),
            Some(ToolCatalogVisibility::Visible)
        );
        assert_eq!(
            eligibility(&agent, name),
            ToolCatalogDeferredEligibility::DeferredEligible {
                provenance: comms_provenance()
            }
        );
    }
    assert!(!listed(&second, "send_request"), "{second:?}");

    // The same calls in a session without the policy.
    let plain_temp = tempfile::tempdir().expect("temp root");
    let plain_factory = host_factory(plain_temp.path(), Some(&comms_name("load-plain")));
    let mut plain = build(
        &plain_factory,
        Session::new(),
        two_deferred_host_tools(),
        &requests,
    )
    .await;
    let nobody = serde_json::json!({
        "peer_id": "00000000-0000-0000-0000-000000000000",
        "body": "hello",
        "handling_mode": "queue"
    });
    for (name, args) in [("peers", serde_json::json!({})), ("send_message", nobody)] {
        let through_the_catalog = call(&mut agent, name, args.clone()).await;
        let inline = call(&mut plain, name, args).await;
        assert_eq!(
            through_the_catalog.result.is_error,
            inline.result.is_error,
            "{name} loaded: {}; inline: {}",
            through_the_catalog.result.text_content(),
            inline.result.text_content()
        );
    }
    let peers = call(&mut agent, "peers", serde_json::json!({})).await;
    assert!(
        !peers.result.is_error,
        "the loaded peers tool runs: {}",
        peers.result.text_content()
    );
}

/// In a session that defers its comms tools, the two tools that answer a
/// peer are in the first request, a load of them is refused as not
/// deferred-eligible, and a call of `reply_to_peer` reaches the comms tool
/// with no load (it answers that no peer message arrived in this turn).
#[tokio::test]
async fn a_peer_is_answered_in_the_first_request() {
    let temp = tempfile::tempdir().expect("temp root");
    let factory = host_factory(temp.path(), Some(&comms_name("peer-answer")))
        .stock_tool_deferral(comms_policy());
    let requests = Requests::default();
    let mut agent = build(
        &factory,
        Session::new(),
        two_deferred_host_tools(),
        &requests,
    )
    .await;

    let first = listed_in_a_turn(&mut agent, "first", &requests).await;
    assert!(!listed(&first, "send_message"), "precondition: {first:?}");
    for name in meerkat_comms::PEER_ANSWER_TOOL_NAMES {
        assert!(listed(&first, name), "{name} is in the request: {first:?}");
        assert_eq!(
            visibility(&agent, name),
            Some(ToolCatalogVisibility::Visible)
        );
    }
    let refused = load(&mut agent, &meerkat_comms::PEER_ANSWER_TOOL_NAMES).await;
    for name in meerkat_comms::PEER_ANSWER_TOOL_NAMES {
        assert_eq!(resolution(&refused, name)["accepted"], false);
        assert_eq!(
            resolution(&refused, name)["rejected_reason"],
            "not_deferred_eligible",
            "{refused:?}"
        );
    }
    let reply = call(
        &mut agent,
        "reply_to_peer",
        serde_json::json!({ "body": "done", "handling_mode": "queue" }),
    )
    .await;
    let text = reply.result.text_content();
    assert!(
        text.contains("no_reply_capability"),
        "the call reaches the comms tool, which has no peer message to answer: {text}"
    );
}

/// The catalog's ordinary rule decides, and the comms family meets it on
/// its own. With the three comms tools that may wait as its only
/// deferred-eligible tools, a session with an exact catalog is at the
/// rule's tool count and defers from its first request. The same session
/// without the policy is inline, and so is a session under the policy whose
/// catalog is not exact: there the comms tools stay in the request and the
/// policy is not applied.
#[tokio::test]
async fn the_comms_tools_that_may_wait_reach_the_deferred_mode_on_their_own() {
    /// The catalog view, the deferred-eligible names and the first
    /// request's tools of a session with the comms tools mounted.
    async fn session(
        label: &str,
        policy: Option<StockToolDeferralPolicy>,
        external: Vec<ToolCatalogEntry>,
    ) -> (meerkat_core::ToolCatalogView, BTreeSet<String>, Vec<String>) {
        let temp = tempfile::tempdir().expect("temp root");
        let mut factory = host_factory(temp.path(), Some(&comms_name(label)));
        if let Some(policy) = policy {
            factory = factory.stock_tool_deferral(policy);
        }
        let requests = Requests::default();
        let mut agent = build(&factory, Session::new(), external, &requests).await;
        let tools = listed_in_a_turn(&mut agent, "first", &requests).await;
        (view(&agent), eligible(&agent), tools)
    }

    let (view, eligible, tools) =
        session("rule-deferred", Some(comms_policy()), exact_host_tool()).await;
    assert!(view.exact_catalog, "precondition: an exact catalog");
    assert_eq!(eligible, names(&COMMS_TOOLS_THAT_WAIT));
    assert!(
        COMMS_TOOLS_THAT_WAIT.len()
            >= meerkat_core::tool_catalog::DEFERRED_CATALOG_TOOL_COUNT_THRESHOLD,
        "the comms tools that may wait reach the rule's tool count"
    );
    assert_eq!(view.mode, ToolCatalogMode::Deferred);
    for name in COMMS_TOOLS_THAT_WAIT {
        assert!(!listed(&tools, name), "{name} waits: {tools:?}");
    }
    for name in CONTROL_TOOLS
        .into_iter()
        .chain(meerkat_comms::PEER_ANSWER_TOOL_NAMES)
        .chain(["host_inline"])
    {
        assert!(listed(&tools, name), "{name} is in the request: {tools:?}");
    }

    let (view, eligible, tools) = session("rule-plain", None, exact_host_tool()).await;
    assert!(view.exact_catalog, "precondition: an exact catalog");
    assert!(eligible.is_empty(), "no policy: {eligible:?}");
    assert_eq!(view.mode, ToolCatalogMode::Inline);
    for name in COMMS_TOOLS {
        assert!(listed(&tools, name), "{name} is in the request: {tools:?}");
    }
    for name in CONTROL_TOOLS {
        assert!(
            !listed(&tools, name),
            "{name} is not in the request: {tools:?}"
        );
    }

    let (view, eligible, tools) = session("rule-inexact", Some(comms_policy()), Vec::new()).await;
    assert!(
        !view.exact_catalog,
        "precondition: a catalog that is not exact"
    );
    assert!(
        eligible.is_empty(),
        "the policy is not applied: {eligible:?}"
    );
    assert_eq!(view.mode, ToolCatalogMode::Inline);
    for name in COMMS_TOOLS {
        assert!(listed(&tools, name), "{name} is in the request: {tools:?}");
    }
    for name in CONTROL_TOOLS {
        assert!(
            !listed(&tools, name),
            "{name} is not in the request: {tools:?}"
        );
    }
}

/// A stock tool a session loaded stays loaded when the
/// session is resumed under the same policy.
#[tokio::test]
async fn a_loaded_stock_tool_stays_loaded_across_a_resume() {
    let temp = tempfile::tempdir().expect("temp root");
    let requests = Requests::default();
    let first_factory = host_factory(temp.path(), Some(&comms_name("stays-loaded-first")))
        .stock_tool_deferral(comms_policy());
    let mut first = build(
        &first_factory,
        Session::new(),
        two_deferred_host_tools(),
        &requests,
    )
    .await;
    let loaded = load(&mut first, &["peers"]).await;
    assert_eq!(resolution(&loaded, "peers")["accepted"], true, "{loaded:?}");
    let tools = listed_in_a_turn(&mut first, "first life", &requests).await;
    assert!(listed(&tools, "peers"), "precondition: {tools:?}");
    assert!(!listed(&tools, "send_message"), "precondition: {tools:?}");
    let session = first.session().clone();
    first.release_owned_resources().await;
    drop(first);

    let resumed_factory = host_factory(temp.path(), Some(&comms_name("stays-loaded-resumed")))
        .stock_tool_deferral(comms_policy());
    let mut resumed = build(
        &resumed_factory,
        session,
        two_deferred_host_tools(),
        &requests,
    )
    .await;
    let tools = listed_in_a_turn(&mut resumed, "second life", &requests).await;
    assert!(
        listed(&tools, "peers"),
        "the loaded tool is in the resumed session's first request: {tools:?}"
    );
    assert!(
        !listed(&tools, "send_message"),
        "a tool it never loaded still waits: {tools:?}"
    );
}

/// A session that ran with the comms tools inline is
/// resumed by a host that opted in. A comms tool it called inline now waits
/// behind the catalog and is loaded once: the second load is a no-op.
#[tokio::test]
async fn a_resumed_session_loads_a_formerly_inline_stock_tool_once() {
    let temp = tempfile::tempdir().expect("temp root");
    let requests = Requests::default();
    let first_factory = host_factory(temp.path(), Some(&comms_name("opt-in-first")));
    let mut first = build(
        &first_factory,
        Session::new(),
        two_deferred_host_tools(),
        &requests,
    )
    .await;
    let tools = listed_in_a_turn(&mut first, "first life", &requests).await;
    assert!(
        listed(&tools, "send_message"),
        "precondition: inline without the policy: {tools:?}"
    );
    let session = first.session().clone();
    first.release_owned_resources().await;
    drop(first);

    let resumed_factory = host_factory(temp.path(), Some(&comms_name("opt-in-resumed")))
        .stock_tool_deferral(comms_policy());
    let mut resumed = build(
        &resumed_factory,
        session,
        two_deferred_host_tools(),
        &requests,
    )
    .await;
    let tools = listed_in_a_turn(&mut resumed, "second life", &requests).await;
    assert!(
        !listed(&tools, "send_message"),
        "after the host opted in the tool waits: {tools:?}"
    );
    assert!(
        listed(&tools, "reply_to_peer"),
        "the peer answer tool stays: {tools:?}"
    );
    let loaded = load(&mut resumed, &["send_message"]).await;
    assert_eq!(
        resolution(&loaded, "send_message")["accepted"],
        true,
        "{loaded:?}"
    );
    assert_ne!(
        resolution(&loaded, "send_message")["accepted_noop"],
        true,
        "the first load is a real load: {loaded:?}"
    );
    let tools = listed_in_a_turn(&mut resumed, "after the load", &requests).await;
    assert!(listed(&tools, "send_message"), "{tools:?}");
    let again = load(&mut resumed, &["send_message"]).await;
    assert_eq!(
        resolution(&again, "send_message")["accepted_noop"],
        true,
        "a second load has nothing to do: {again:?}"
    );
}

/// A session that loaded a comms tool under the policy is resumed by a
/// host without it. The comms tools are inline again and in the request;
/// the loaded name the session recorded changes nothing.
#[tokio::test]
async fn removing_the_policy_returns_the_tools_inline() {
    let temp = tempfile::tempdir().expect("temp root");
    let requests = Requests::default();
    let first_factory = host_factory(temp.path(), Some(&comms_name("opt-out-first")))
        .stock_tool_deferral(comms_policy());
    let mut first = build(
        &first_factory,
        Session::new(),
        two_deferred_host_tools(),
        &requests,
    )
    .await;
    let loaded = load(&mut first, &["peers"]).await;
    assert_eq!(resolution(&loaded, "peers")["accepted"], true, "{loaded:?}");
    let tools = listed_in_a_turn(&mut first, "first life", &requests).await;
    assert!(listed(&tools, "peers"), "precondition: {tools:?}");
    assert!(!listed(&tools, "send_message"), "precondition: {tools:?}");
    let session = first.session().clone();
    first.release_owned_resources().await;
    drop(first);

    let resumed_factory = host_factory(temp.path(), Some(&comms_name("opt-out-resumed")));
    let mut resumed = build(
        &resumed_factory,
        session,
        two_deferred_host_tools(),
        &requests,
    )
    .await;
    let tools = listed_in_a_turn(&mut resumed, "second life", &requests).await;
    for name in COMMS_TOOLS {
        assert!(listed(&tools, name), "{name} is in the request: {tools:?}");
        assert_eq!(
            eligibility(&resumed, name),
            ToolCatalogDeferredEligibility::InlineOnly,
            "{name} is inline only again"
        );
    }
    let refused = load(&mut resumed, &["send_message"]).await;
    assert_eq!(
        resolution(&refused, "send_message")["rejected_reason"],
        "not_deferred_eligible",
        "{refused:?}"
    );
    let peers = call(&mut resumed, "peers", serde_json::json!({})).await;
    assert!(
        !peers.result.is_error,
        "the tool it had loaded runs inline: {}",
        peers.result.text_content()
    );
}

/// The limit of a resume without the policy, pinned as it is. A session
/// keeps the system prompt it was created with: created under the policy
/// and resumed by a host without it, its requests list the comms tools
/// inline and offer no `tool_catalog_load`, while its prompt still says to
/// load them with it. The prompt's tool name grants nothing: a call of the
/// load tool is refused.
#[tokio::test]
async fn a_session_resumed_without_the_policy_keeps_its_load_first_prompt() {
    let load_first = "are in the deferred tool catalog";
    let temp = tempfile::tempdir().expect("temp root");
    let requests = Requests::default();
    let first_factory = host_factory(temp.path(), Some(&comms_name("stale-prompt-first")))
        .stock_tool_deferral(comms_policy());
    let mut first = build(&first_factory, Session::new(), exact_host_tool(), &requests).await;
    let created_prompt = system_prompt(&first);
    assert!(
        created_prompt.contains(load_first) && created_prompt.contains("tool_catalog_load"),
        "precondition: created under the policy, the prompt says to load: {created_prompt}"
    );
    let tools = listed_in_a_turn(&mut first, "first life", &requests).await;
    assert!(
        listed(&tools, "tool_catalog_load"),
        "precondition: {tools:?}"
    );
    assert!(!listed(&tools, "send_message"), "precondition: {tools:?}");
    let session = first.session().clone();
    first.release_owned_resources().await;
    drop(first);

    let resumed_factory = host_factory(temp.path(), Some(&comms_name("stale-prompt-resumed")));
    let mut resumed = build(&resumed_factory, session, exact_host_tool(), &requests).await;
    let tools = listed_in_a_turn(&mut resumed, "second life", &requests).await;
    for name in COMMS_TOOLS {
        assert!(listed(&tools, name), "{name} is in the request: {tools:?}");
    }
    for name in CONTROL_TOOLS {
        assert!(
            !listed(&tools, name),
            "{name} is not in the request: {tools:?}"
        );
    }
    assert_eq!(
        system_prompt(&resumed),
        created_prompt,
        "the resumed session keeps the prompt it was created with"
    );
    let called = call(
        &mut resumed,
        "tool_catalog_load",
        serde_json::json!({ "names": ["peers"] }),
    )
    .await;
    assert!(
        called.result.is_error,
        "a tool the prompt names and the request does not offer is not callable: {}",
        called.result.text_content()
    );
}

/// A comms tool the session's own filter hides is not loadable and
/// stays hidden; the policy widens nothing.
#[tokio::test]
async fn a_filtered_stock_tool_cannot_be_loaded() {
    let temp = tempfile::tempdir().expect("temp root");
    let factory = host_factory(temp.path(), Some(&comms_name("filtered")))
        .stock_tool_deferral(comms_policy());
    let requests = Requests::default();
    let mut agent = build(
        &factory,
        Session::new(),
        two_deferred_host_tools(),
        &requests,
    )
    .await;
    agent
        .stage_external_tool_filter(meerkat_core::ToolFilter::Deny(
            ["send_message".to_string()].into_iter().collect(),
        ))
        .expect("stage the session's own deny");
    listed_in_a_turn(&mut agent, "the filter applies", &requests).await;
    assert_eq!(
        visibility(&agent, "send_message"),
        Some(ToolCatalogVisibility::Hidden)
    );

    let refused = load(&mut agent, &["send_message", "peers"]).await;
    assert_eq!(
        resolution(&refused, "send_message")["accepted"],
        false,
        "{refused:?}"
    );
    assert_eq!(
        resolution(&refused, "send_message")["rejected_reason"],
        "not_filterable",
        "the load is refused by the session's filter: {refused:?}"
    );
    assert_eq!(
        resolution(&refused, "peers")["accepted"],
        true,
        "{refused:?}"
    );
    let tools = listed_in_a_turn(&mut agent, "after the load", &requests).await;
    assert!(!listed(&tools, "send_message"), "{tools:?}");
    assert!(listed(&tools, "peers"), "{tools:?}");
}

/// Under the policy a comms tool the session's own filter blocks is not
/// shown to the model by the catalog. While the filter allows it, a search
/// of the whole catalog returns it with its description. Once the filter
/// blocks it, the deferred-catalog notice (its text and its typed delta)
/// names only the comms tools that wait and the filter allows, and a search
/// returns and counts the same tools, with neither the blocked tool's row
/// nor its description. Tool load and call behavior is unchanged.
#[tokio::test]
async fn a_filtered_stock_tool_is_not_named_by_search_or_by_the_notice() {
    /// The rows of a search of the whole catalog (name and description),
    /// its match count and its whole text.
    async fn search_all(
        agent: &mut DynAgent,
    ) -> (Vec<(String, String)>, serde_json::Value, String) {
        let outcome = call(agent, "tool_catalog_search", serde_json::json!({})).await;
        let text = outcome.result.text_content();
        assert!(!outcome.result.is_error, "a catalog search: {text}");
        let found: serde_json::Value = serde_json::from_str(&text)
            .unwrap_or_else(|error| panic!("a catalog search response ({error}): {text}"));
        let rows = found["results"]
            .as_array()
            .unwrap_or_else(|| panic!("search results: {text}"))
            .iter()
            .map(|row| {
                (
                    row["name"].as_str().unwrap_or_default().to_string(),
                    row["description"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        (rows, found["total_matches"].clone(), text)
    }

    const BLOCKED: &str = "send_message";
    let temp = tempfile::tempdir().expect("temp root");
    let factory = host_factory(temp.path(), Some(&comms_name("filtered-unnamed")))
        .stock_tool_deferral(comms_policy());
    let requests = Requests::default();
    let mut agent = build(&factory, Session::new(), exact_host_tool(), &requests).await;
    assert_eq!(
        eligible(&agent),
        names(&COMMS_TOOLS_THAT_WAIT),
        "precondition: the tool to block is one the policy lets wait"
    );

    // While the filter allows it, the search returns it.
    let (rows, total, text) = search_all(&mut agent).await;
    assert_eq!(
        rows.iter()
            .map(|(name, _)| name.clone())
            .collect::<BTreeSet<_>>(),
        names(&COMMS_TOOLS_THAT_WAIT),
        "precondition: {text}"
    );
    assert_eq!(total, 3, "precondition: {text}");
    let description = rows
        .iter()
        .find(|(name, _)| name == BLOCKED)
        .map(|(_, description)| description.clone())
        .expect("precondition: the search returns the tool");
    let first_line = description.lines().next().unwrap_or_default().to_string();
    assert!(
        first_line.len() > 20,
        "precondition: a description to look for: {first_line:?}"
    );

    agent
        .stage_external_tool_filter(meerkat_core::ToolFilter::Deny(
            [BLOCKED.to_string()].into_iter().collect(),
        ))
        .expect("stage the session's own deny");
    let first = listed_in_a_turn(&mut agent, "first", &requests).await;
    assert!(!listed(&first, BLOCKED), "{first:?}");
    let allowed = names(&["send_request", "peers"]);

    assert_eq!(
        announced_as_deferred(&agent),
        allowed,
        "the notice's text names what the filter allows"
    );
    assert_eq!(
        announced_in_typed_deltas(&agent),
        allowed,
        "the notice's typed delta names what the filter allows"
    );

    let (rows, total, text) = search_all(&mut agent).await;
    assert_eq!(
        rows.iter()
            .map(|(name, _)| name.clone())
            .collect::<BTreeSet<_>>(),
        allowed,
        "the search returns only what the filter allows: {text}"
    );
    assert_eq!(total, 2, "{text}");
    assert!(
        rows.iter()
            .all(|(_, row_description)| *row_description != description)
            && !text.contains(&first_line),
        "the blocked tool's description does not reach the model: {text}"
    );

    // Tool load and call behavior is unchanged. The blocked tool and an
    // allowed one are loaded and called side by side: the allowed tool
    // loads and runs, and the blocked one is refused each time by the
    // session's own filter and tool policy, not by a failed send.
    let loads = load(&mut agent, &[BLOCKED, "peers"]).await;
    assert_eq!(resolution(&loads, BLOCKED)["accepted"], false, "{loads:?}");
    assert_eq!(
        resolution(&loads, BLOCKED)["rejected_reason"],
        "not_filterable",
        "the load is refused by the session's filter: {loads:?}"
    );
    assert_eq!(resolution(&loads, "peers")["accepted"], true, "{loads:?}");
    let second = listed_in_a_turn(&mut agent, "after the load", &requests).await;
    assert!(!listed(&second, BLOCKED), "{second:?}");
    assert!(listed(&second, "peers"), "{second:?}");

    let called = call(
        &mut agent,
        BLOCKED,
        serde_json::json!({
            "peer_id": "00000000-0000-0000-0000-000000000000",
            "body": "hello",
            "handling_mode": "queue"
        }),
    )
    .await;
    let refusal = called.result.text_content();
    assert!(
        called.result.is_error,
        "a call of the blocked tool is refused: {refusal}"
    );
    assert!(
        refusal.contains(&format!("Tool '{BLOCKED}' is not allowed by policy")),
        "the refusal is the tool policy's: {refusal}"
    );
    let allowed_call = call(&mut agent, "peers", serde_json::json!({})).await;
    assert!(
        !allowed_call.result.is_error,
        "the allowed tool, loaded, runs: {}",
        allowed_call.result.text_content()
    );
}

/// The announcement: a session whose comms tools defer from its first
/// request has a system prompt that explains the catalog control tools.
/// Without the policy the same session defers nothing and its prompt does
/// not mention them.
#[tokio::test]
async fn the_system_prompt_explains_the_catalog_when_stock_tools_defer_from_the_first_request() {
    async fn prompt_and_first_request(
        label: &str,
        policy: Option<StockToolDeferralPolicy>,
    ) -> (String, Vec<String>) {
        let temp = tempfile::tempdir().expect("temp root");
        let mut factory = host_factory(temp.path(), Some(&comms_name(label)));
        if let Some(policy) = policy {
            factory = factory.stock_tool_deferral(policy);
        }
        let requests = Requests::default();
        let mut agent = build(&factory, Session::new(), exact_host_tool(), &requests).await;
        let prompt = system_prompt(&agent);
        let tools = listed_in_a_turn(&mut agent, "first", &requests).await;
        (prompt, tools)
    }

    let guidance = super::deferred_catalog_guidance();
    let (plain_prompt, plain_tools) = prompt_and_first_request("announce-plain", None).await;
    assert!(
        !plain_prompt.contains(guidance),
        "precondition: without the policy nothing waits and nothing is explained"
    );
    assert!(listed(&plain_tools, "send_message"), "{plain_tools:?}");
    assert!(
        !listed(&plain_tools, "tool_catalog_load"),
        "{plain_tools:?}"
    );

    let (prompt, tools) = prompt_and_first_request("announce-deferred", Some(comms_policy())).await;
    assert!(
        prompt.contains(guidance),
        "the prompt explains the control tools: {prompt}"
    );
    assert!(!listed(&tools, "send_message"), "{tools:?}");
    for name in CONTROL_TOOLS {
        assert!(
            listed(&tools, name),
            "{name} is in the first request: {tools:?}"
        );
    }
}

/// The exclusion of sessions with the agent mob tools. Such a session has
/// no exact catalog and defers nothing, and a host's policy is not applied
/// to it: under the comms policy it is the session a factory without the
/// policy builds. The catalog view, the system prompt, the first request's
/// tool definitions and the notices are equal. The same policy defers the
/// comms tools of the same session without the mob surface, so the policy
/// is live in this row.
#[tokio::test]
async fn a_session_with_mob_tools_is_built_as_without_the_policy() {
    const NAME: &str = "stock-deferral-mob-exclusion";
    let plain = built_session(NAME, with_mob_surface, exact_host_tool(), mob_authority).await;
    let under_the_policy = built_session(
        NAME,
        |factory| with_mob_surface(factory).stock_tool_deferral(comms_policy()),
        exact_host_tool(),
        mob_authority,
    )
    .await;

    assert!(
        !plain.view.exact_catalog,
        "precondition: the mob surface makes the catalog not exact"
    );
    assert_eq!(plain.view.mode, ToolCatalogMode::Inline, "precondition");
    for name in COMMS_TOOLS
        .into_iter()
        .chain(["delegate", "mob_spawn_member", "host_inline"])
    {
        assert_eq!(
            plain.entry(name).deferred_eligibility,
            ToolCatalogDeferredEligibility::InlineOnly,
            "precondition: {name} is mounted, inline only"
        );
        assert_eq!(
            plain.entry(name).visibility,
            ToolCatalogVisibility::Visible,
            "precondition: {name} is in the request"
        );
    }
    assert_eq!(under_the_policy, plain);

    let without_the_surface = built_session(
        NAME,
        |factory| factory.stock_tool_deferral(comms_policy()),
        exact_host_tool(),
        |_| {},
    )
    .await;
    assert_eq!(
        without_the_surface.view.mode,
        ToolCatalogMode::Deferred,
        "the policy defers the comms tools of the session without the mob surface"
    );
}

/// The exclusion of sessions with memory search. Memory search is composed
/// over the session's other tools and reports no exact catalog, so such a
/// session defers nothing, and a host's policy is not applied to it: under
/// the comms policy it is the session a factory without the policy builds.
/// The catalog view, the system prompt, the first request's tool
/// definitions and the notices are equal. The same policy defers the comms
/// tools of the same session without memory search, so the policy is live
/// in this row.
#[cfg(feature = "memory-store-session")]
#[tokio::test]
async fn a_session_with_memory_search_is_built_as_without_the_policy() {
    const NAME: &str = "stock-deferral-memory-exclusion";
    let plain = built_session(
        NAME,
        |factory| factory.memory(true),
        exact_host_tool(),
        |_| {},
    )
    .await;
    let under_the_policy = built_session(
        NAME,
        |factory| factory.memory(true).stock_tool_deferral(comms_policy()),
        exact_host_tool(),
        |_| {},
    )
    .await;

    assert!(
        !plain.view.exact_catalog,
        "precondition: memory search makes the catalog not exact"
    );
    assert_eq!(plain.view.mode, ToolCatalogMode::Inline, "precondition");
    for name in COMMS_TOOLS
        .into_iter()
        .chain(["memory_search", "host_inline"])
    {
        assert_eq!(
            plain.entry(name).deferred_eligibility,
            ToolCatalogDeferredEligibility::InlineOnly,
            "precondition: {name} is mounted, inline only"
        );
        assert_eq!(
            plain.entry(name).visibility,
            ToolCatalogVisibility::Visible,
            "precondition: {name} is in the request"
        );
    }
    assert_eq!(under_the_policy, plain);

    let without_memory = built_session(
        NAME,
        |factory| factory.stock_tool_deferral(comms_policy()),
        exact_host_tool(),
        |_| {},
    )
    .await;
    assert_eq!(
        without_memory.view.mode,
        ToolCatalogMode::Deferred,
        "the policy defers the comms tools of the session without memory search"
    );
}

/// The names the factory pins are data: the two comms tools that answer a
/// peer.
#[test]
fn the_factory_pins_the_tools_that_answer_by_name() {
    let pinned = super::stock_tool_deferral_pins()
        .into_iter()
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    assert_eq!(pinned, names(&["reply_to_peer", "send_response"]));
    assert_eq!(
        meerkat_comms::PEER_ANSWER_TOOL_NAMES,
        ["reply_to_peer", "send_response"]
    );
}

/// A tool of a kind the policy does not name is outside it. In a session
/// that defers its comms tools, a mob tool with the default execution
/// contract is in the first request, the notice does not offer it, and a
/// load of it is refused as not deferred-eligible.
#[tokio::test]
async fn a_tool_of_a_kind_the_policy_does_not_name_never_waits() {
    let temp = tempfile::tempdir().expect("temp root");
    let factory = host_factory(temp.path(), Some(&comms_name("other-kind")))
        .stock_tool_deferral(comms_policy());
    let requests = Requests::default();
    let mut agent = build(
        &factory,
        Session::new(),
        vec![mob_tool("conclude_objective"), mob_tool("delegate")],
        &requests,
    )
    .await;

    assert_eq!(view(&agent).mode, ToolCatalogMode::Deferred, "precondition");
    let tools = listed_in_a_turn(&mut agent, "first", &requests).await;
    assert!(!listed(&tools, "send_message"), "precondition: {tools:?}");
    let announced = announced_as_deferred(&agent);
    let refused = load(&mut agent, &["conclude_objective", "delegate"]).await;
    for name in ["conclude_objective", "delegate"] {
        assert_eq!(
            eligibility(&agent, name),
            ToolCatalogDeferredEligibility::InlineOnly
        );
        assert!(listed(&tools, name), "{name} is in the request: {tools:?}");
        assert!(
            !announced.contains(name),
            "the notice does not offer {name}: {announced:?}"
        );
        assert_eq!(
            resolution(&refused, name)["rejected_reason"],
            "not_deferred_eligible",
            "{refused:?}"
        );
    }
}

/// A session that starts with its comms tools behind the catalog is told to
/// load them, not to call them: its system prompt carries one sentence in
/// place of the comms usage section, which names `send_message`,
/// `send_request` and `peers` as tools to call. Every other session keeps
/// the usage section: one without the policy, and one with the comms policy
/// whose catalog is not exact (it defers nothing, so the comms tools are in
/// its requests).
#[tokio::test]
async fn a_session_that_defers_its_comms_tools_is_told_to_load_them() {
    /// The system prompt and the first request's tools of a session with
    /// the comms tools mounted.
    async fn session(
        label: &str,
        policy: Option<StockToolDeferralPolicy>,
        external: Vec<ToolCatalogEntry>,
    ) -> (String, Vec<String>) {
        let temp = tempfile::tempdir().expect("temp root");
        let mut factory = host_factory(temp.path(), Some(&comms_name(label)));
        if let Some(policy) = policy {
            factory = factory.stock_tool_deferral(policy);
        }
        let requests = Requests::default();
        let mut agent = build(&factory, Session::new(), external, &requests).await;
        let prompt = system_prompt(&agent);
        let tools = listed_in_a_turn(&mut agent, "first", &requests).await;
        (prompt, tools)
    }
    let usage = meerkat_tools::CommsToolSurface::usage_instructions();
    let load_first = "are in the deferred tool catalog";

    let (prompt, tools) = session("prompt-deferred", Some(comms_policy()), exact_host_tool()).await;
    for name in COMMS_TOOLS_THAT_WAIT {
        assert!(
            !listed(&tools, name),
            "precondition: {name} waits: {tools:?}"
        );
    }
    assert!(
        !prompt.contains(usage),
        "the prompt does not tell the model to call tools that wait: {prompt}"
    );
    assert!(
        prompt.contains(load_first) && prompt.contains("tool_catalog_load"),
        "the prompt says the comms tools are loaded first: {prompt}"
    );
    for name in COMMS_TOOLS {
        assert!(
            prompt.contains(&format!("`{name}`")),
            "the prompt names {name}"
        );
    }

    let (prompt, tools) = session("prompt-plain", None, exact_host_tool()).await;
    assert!(listed(&tools, "send_message"), "{tools:?}");
    assert!(prompt.contains(usage), "no policy: the usage section stays");
    assert!(!prompt.contains(load_first));

    let (prompt, tools) = session("prompt-inexact", Some(comms_policy()), Vec::new()).await;
    assert!(
        listed(&tools, "send_message"),
        "precondition: a catalog that is not exact defers nothing: {tools:?}"
    );
    assert!(
        prompt.contains(usage),
        "a session whose comms tools are in its requests keeps the usage section"
    );
    assert!(!prompt.contains(load_first));
}

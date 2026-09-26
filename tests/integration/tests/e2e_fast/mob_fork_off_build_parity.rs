//! A fork-derived member is built exactly as its source, recorded at the
//! provider boundary.
//!
//! HomeCore (run-32) saw a `fork_off` child of `domain:calendar` built by the
//! host with bare mob labels, no application context and no source reference:
//! the host resolved a generic member, 92 of calendar's 150 tools. The child
//! could not do calendar work, and because its `tools` block differed, it
//! could not reuse the forker's cached prefix either.
//!
//! These lanes compose the production persistent session service behind a
//! host build callback (`SessionAgentBuilder` wrapping the factory, the shape
//! MobKit's `callback/build_agent` gateway has) that resolves tools from
//! `app_context`, the way HomeCore does, and runs the members on the real
//! OpenAI Responses client against a loopback server that records every
//! request body. A child and its source must send byte-identical `tools`
//! arrays and the same transcript prefix up to the fork boundary.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{DefaultBodyLimit, State};
use axum::response::{IntoResponse, Response, Sse, sse::Event};
use axum::routing::post;
use meerkat::{AgentFactory, Config, FactoryAgent, FactoryAgentBuilder};
use meerkat_client::LlmClient;
use meerkat_core::service::{CreateSessionRequest, SessionError};
use meerkat_core::types::SessionId;
use meerkat_core::{AgentEvent, ContentInput, HandlingMode, SessionLlmIdentity};
use meerkat_mob::{
    AgentIdentity, BoundedResultSpec, MobControlPrincipal, MobDefinition, MobHandle, MobId,
    ProfileName, SpawnMemberSpec, WorkOrigin, WorkSpec,
};
use meerkat_mob_mcp::MobMcpState;
use meerkat_mob_mcp::temporary_council::{
    MergeBackPolicy, TemporaryCouncilBounds, TemporaryCouncilParticipantSpec,
    TemporaryCouncilRequest,
};
use meerkat_session::SessionAgentBuilder;
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};
use tokio::sync::mpsc;

const MODEL: &str = "gpt-6-astra";
const PROFILE: &str = "domain";
const SOURCE: &str = "domain-calendar";
const FORK_PROMPT: &str = "FORK-NOW-8M fork a child for tomorrow's agenda";
const CHILD: &str = "calendar-fork";
const CHILD_TASK: &str = "CHILD-TASK-2P list tomorrow's events";
const CHILD_REPLY: &str = "CHILD-REPLY-6T";
const FORK_DONE: &str = "FORK-DONE-1K";
const SOURCE_TURN: &str = "SOURCE-TURN-4W note the agenda";
const SEAT: &str = "calendar-seat";
const WAIT: Duration = Duration::from_secs(60);

/// Host tools the build callback grants a calendar member.
const CALENDAR_HOST_TOOLS: [&str; 2] = ["calendar_create_event", "display_publish"];
/// The host tool the build callback grants a member it cannot place.
const GENERIC_HOST_TOOL: &str = "generic_note";
/// The source's per-spawn overlay (MobKit's `memory` recorder).
const OVERLAY_TOOL: &str = "memory";

// ===========================================================================
// Loopback OpenAI Responses server: records every request body verbatim
// ===========================================================================

#[derive(Clone, Default)]
struct RecordedBodies(Arc<Mutex<Vec<String>>>);

impl RecordedBodies {
    fn all(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

/// The parts of a Responses request body this lane compares, borrowed as the
/// exact bytes the client sent.
#[derive(Deserialize)]
struct BodyParts<'a> {
    #[serde(borrow)]
    tools: Option<&'a RawValue>,
    #[serde(borrow)]
    input: Vec<&'a RawValue>,
}

fn body_parts(body: &str) -> BodyParts<'_> {
    serde_json::from_str(body).unwrap_or_else(|error| panic!("request body ({error}): {body}"))
}

fn input_items(body: &str) -> Vec<Value> {
    let value: Value = serde_json::from_str(body).unwrap();
    value["input"].as_array().cloned().unwrap_or_default()
}

/// The text of the last user message in a request body.
fn last_user_text(body: &str) -> String {
    input_items(body)
        .iter()
        .rev()
        .find(|item| item["role"] == "user")
        .map(Value::to_string)
        .unwrap_or_default()
}

fn has_tool_output(body: &str) -> bool {
    input_items(body)
        .iter()
        .any(|item| item["type"] == "function_call_output")
}

fn tool_names(body: &str) -> Vec<String> {
    let value: Value = serde_json::from_str(body).unwrap();
    value["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| tool["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn text_output(text: &str) -> Value {
    json!([{
        "type": "message", "id": "msg_local", "role": "assistant", "status": "completed",
        "content": [{"type": "output_text", "text": text, "annotations": []}]
    }])
}

/// The forker calls `fork_off` once, then finishes; the child replies with its
/// token; every other turn acknowledges.
fn scripted_output(body: &str) -> Value {
    let last_user = last_user_text(body);
    if last_user.contains(CHILD_TASK) {
        return text_output(CHILD_REPLY);
    }
    if has_tool_output(body) {
        return text_output(FORK_DONE);
    }
    if last_user.contains(FORK_PROMPT) {
        let arguments = json!({"member_id": CHILD, "task": CHILD_TASK}).to_string();
        return json!([{
            "type": "function_call", "id": "fc_fork", "call_id": "call_fork",
            "name": "fork_off", "arguments": arguments, "status": "completed"
        }]);
    }
    text_output("ACK")
}

async fn responses(State(bodies): State<RecordedBodies>, body: String) -> Response {
    let output = scripted_output(&body);
    bodies.0.lock().unwrap().push(body);
    let completed = json!({
        "type": "response.completed",
        "response": {
            "id": "resp_local", "status": "completed", "model": MODEL, "output": output,
            "usage": {"input_tokens": 10, "output_tokens": 3, "total_tokens": 13}
        }
    });
    Sse::new(futures::stream::iter([Ok::<_, Infallible>(
        Event::default()
            .event("response.completed")
            .json_data(completed)
            .unwrap(),
    )]))
    .into_response()
}

// ===========================================================================
// Host build callback: resolves tools from `app_context`, records each build
// ===========================================================================

#[derive(Debug, Clone)]
struct RecordedBuild {
    member: Option<String>,
    labels: BTreeMap<String, String>,
    app_context: Option<Value>,
    fork_source: Option<meerkat_core::ForkBuildSource>,
}

struct NamedTools(Vec<&'static str>);

#[async_trait::async_trait]
impl meerkat_core::AgentToolDispatcher for NamedTools {
    fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> {
        self.0
            .iter()
            .map(|name| {
                Arc::new(meerkat_core::ToolDef {
                    name: (*name).into(),
                    description: format!("{name} host tool"),
                    input_schema: json!({"type": "object", "properties": {}}),
                    provenance: None,
                })
            })
            .collect::<Vec<_>>()
            .into()
    }

    async fn dispatch(
        &self,
        call: meerkat_core::ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        Ok(meerkat_core::ToolResult::new(call.id.to_string(), "ok".to_string(), false).into())
    }
}

/// The host side of `callback/build_agent`: grants tools by the member's
/// `app_context["domain"]` and composes them over whatever the mob already
/// installed (the per-spawn overlay and mob-owned tools), as MobKit does.
struct HostBuildCallback {
    inner: FactoryAgentBuilder,
    builds: Arc<Mutex<Vec<RecordedBuild>>>,
}

#[async_trait::async_trait]
impl SessionAgentBuilder for HostBuildCallback {
    type Agent = FactoryAgent;

    async fn model_supports_inline_video(&self, identity: &SessionLlmIdentity) -> Option<bool> {
        self.inner.model_supports_inline_video(identity).await
    }

    async fn abort_absent_session_compaction_stages(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        self.inner
            .abort_absent_session_compaction_stages(session_id)
            .await
    }

    async fn build_agent(
        &self,
        req: &CreateSessionRequest,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<Self::Agent, SessionError> {
        let mut build = req.build.clone().unwrap_or_default();
        self.builds.lock().unwrap().push(RecordedBuild {
            member: build
                .mob_member_binding
                .as_ref()
                .map(|binding| binding.member.clone()),
            labels: build
                .peer_meta
                .as_ref()
                .map(|meta| meta.labels.clone())
                .unwrap_or_default(),
            app_context: build.app_context.clone(),
            fork_source: build.fork_source.clone(),
        });
        let domain = build
            .app_context
            .as_ref()
            .and_then(|context| context["domain"].as_str());
        let host_tools: Arc<dyn meerkat_core::AgentToolDispatcher> = match domain {
            Some("calendar") => Arc::new(NamedTools(CALENDAR_HOST_TOOLS.to_vec())),
            _ => Arc::new(NamedTools(vec![GENERIC_HOST_TOOL])),
        };
        let mut layers = vec![host_tools];
        layers.extend(build.external_tools.take());
        build.external_tools = Some(Arc::new(meerkat_core::DynamicToolComposite::new(layers)));
        let request = CreateSessionRequest {
            model: req.model.clone(),
            prompt: req.prompt.clone(),
            system_prompt: req.system_prompt.clone(),
            max_tokens: req.max_tokens,
            event_tx: req.event_tx.clone(),
            initial_turn: req.initial_turn,
            build: Some(build),
            labels: req.labels.clone(),
            deferred_prompt_policy: req.deferred_prompt_policy,
            injected_context: req.injected_context.clone(),
        };
        self.inner.build_agent(&request, event_tx).await
    }
}

// ===========================================================================
// Stack
// ===========================================================================

struct Stack {
    state: Arc<MobMcpState>,
    bodies: RecordedBodies,
    builds: Arc<Mutex<Vec<RecordedBuild>>>,
    server: tokio::task::JoinHandle<()>,
    _temp: tempfile::TempDir,
}

impl Stack {
    async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let bodies = RecordedBodies::default();
        let app = axum::Router::new()
            .route("/v1/responses", post(responses))
            .layer(DefaultBodyLimit::disable())
            .with_state(bodies.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client: Arc<dyn LlmClient> = Arc::new(meerkat_client::OpenAiClient::new_with_base_url(
            "loopback-key".to_string(),
            base_url,
        ));

        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path();
        let project_root = root.join("project-root");
        let context_root = root.join("context-root");
        for dir in [&project_root, &context_root] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("AGENTS.md"), "# Fork build parity\n").unwrap();
        }
        let factory = AgentFactory::new(root.join("factory-store"))
            .user_config_root(root.join("user-config"))
            .runtime_root(root.join("runtime-root"))
            .project_root(project_root)
            .context_root(context_root)
            .builtins(false)
            .shell(false)
            .comms(true)
            .mob(true);
        let mut inner = FactoryAgentBuilder::new(factory, Config::default());
        inner.default_llm_client = Some(Arc::clone(&client));
        let store = Arc::new(meerkat::MemoryStore::new());
        inner.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(Arc::clone(
            &store,
        ))));
        let mob_tools_slot = Arc::clone(&inner.default_mob_tools);
        let builds = Arc::new(Mutex::new(Vec::new()));
        let host = HostBuildCallback {
            inner,
            builds: Arc::clone(&builds),
        };
        let store: Arc<dyn meerkat::SessionStore> = store;
        let service = Arc::new(meerkat_session::PersistentSessionService::new(
            host,
            32,
            store,
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            Arc::new(meerkat_store::MemoryBlobStore::default()),
        ));
        let state = MobMcpState::new(service, MobControlPrincipal::Owner)
            .with_default_llm_client(Some(client))
            .try_with_persistent_storage_root(Some(root.join("state")))
            .expect("rooted mob custody")
            .into_shared();
        *mob_tools_slot.write().unwrap() = Some(Arc::new(
            meerkat_mob_mcp::AgentMobToolSurfaceFactory::new(Arc::clone(&state)),
        ));
        Self {
            state,
            bodies,
            builds,
            server,
            _temp: temp,
        }
    }

    /// Create a mob and seat the calendar source with every per-build input
    /// its host resolves it from. Returns the mob handle and source session.
    async fn seed_source(&self, mob_id: &MobId) -> (MobHandle, SessionId) {
        self.state
            .mob_create_definition(definition(mob_id.as_str()))
            .await
            .expect("create mob");
        let handle = self.state.handle_for(mob_id).await.expect("mob handle");
        let mut spec = SpawnMemberSpec::new(PROFILE, AgentIdentity::from(SOURCE));
        spec.runtime_mode = Some(meerkat_mob::MobRuntimeMode::TurnDriven);
        spec.context = Some(source_app_context());
        spec.labels = Some(source_app_labels());
        spec.external_tools = Some(Arc::new(NamedTools(vec![OVERLAY_TOOL])));
        handle.spawn_spec(spec).await.expect("spawn the source");
        let session = handle
            .resolve_bridge_session_id(&AgentIdentity::from(SOURCE))
            .await
            .expect("source session");
        (handle, session)
    }

    /// The last recorded build of `member`.
    fn last_build(&self, member: &str) -> RecordedBuild {
        self.builds
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|build| build.member.as_deref() == Some(member))
            .cloned()
            .unwrap_or_else(|| panic!("no recorded build for '{member}'"))
    }

    /// The only recorded request body matching `pick`.
    fn only_body(&self, what: &str, pick: impl Fn(&str) -> bool) -> String {
        let matching: Vec<String> = self
            .bodies
            .all()
            .into_iter()
            .filter(|body| pick(body))
            .collect();
        assert_eq!(
            matching.len(),
            1,
            "expected exactly one {what} request, got {}",
            matching.len()
        );
        matching.into_iter().next().unwrap()
    }

    async fn teardown(self) {
        let handles = self.state.mob_handles_snapshot().await.unwrap_or_default();
        for (mob_id, _) in handles {
            let _ = self.state.mob_destroy(&mob_id).await;
        }
        self.server.abort();
    }
}

fn definition(mob_id: &str) -> MobDefinition {
    serde_json::from_value(json!({
        "id": mob_id,
        "profiles": {
            PROFILE: {
                "model": MODEL,
                "tools": { "comms": true, "mob": true },
                "peer_description": "Calendar domain member",
                "runtime_mode": "turn_driven"
            }
        },
        "wiring": { "auto_wire_orchestrator": false, "role_wiring": [] }
    }))
    .expect("fork build parity mob definition")
}

fn source_app_context() -> Value {
    json!({"domain": "calendar", "identity": "domain:calendar"})
}

fn source_app_labels() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("domain".to_string(), "calendar".to_string()),
        ("home_role".to_string(), "domain".to_string()),
    ])
}

async fn bounded_turn(handle: &MobHandle, identity: &str, prompt: &str) -> String {
    let spec = BoundedResultSpec::new("turn", 16 * 1024).expect("bounded result spec");
    let work = handle
        .start_work_for_identity_bounded(
            AgentIdentity::from(identity),
            WorkSpec::new(ContentInput::Text(prompt.to_string()), WorkOrigin::Internal),
            HandlingMode::Queue,
            spec.clone(),
        )
        .await
        .unwrap_or_else(|error| panic!("start a turn for {identity}: {error:?}"));
    let Ok(outcome) = tokio::time::timeout(WAIT, work.wait_bounded(spec)).await else {
        panic!("turn for {identity} ({prompt:?}) timed out");
    };
    outcome
        .unwrap_or_else(|error| panic!("turn for {identity} ({prompt:?}) failed: {error:?}"))
        .result()
        .result()
        .text()
        .to_string()
}

/// The child's build is the source's, bar the child's own member identity
/// and the mob it is seated in.
fn assert_built_as_source(
    source: &RecordedBuild,
    child: &RecordedBuild,
    child_identity: &str,
    child_mob: &MobId,
    expected_source: &meerkat_core::ForkBuildSource,
) {
    assert_eq!(source.app_context, Some(source_app_context()));
    assert_eq!(
        child.app_context, source.app_context,
        "the child's build carries the source's application context verbatim"
    );
    let mut expected_labels = source.labels.clone();
    for (key, value) in source_app_labels() {
        assert_eq!(
            expected_labels.get(&key),
            Some(&value),
            "source label {key}"
        );
    }
    expected_labels.insert("mob_id".to_string(), child_mob.to_string());
    expected_labels.insert("agent_identity".to_string(), child_identity.to_string());
    expected_labels.insert("meerkat_id".to_string(), child_identity.to_string());
    assert_eq!(
        child.labels, expected_labels,
        "the child's labels are the source's, with its own member identity"
    );
    assert_eq!(source.fork_source, None, "the source is no fork");
    assert_eq!(
        child.fork_source.as_ref(),
        Some(expected_source),
        "the child's build names its source member and source session"
    );
}

// ===========================================================================
// Lanes
// ===========================================================================

/// The recorded-request contract. The forker calls the real `fork_off` tool
/// from its own turn; the child's first request must carry the forker's exact
/// `tools` array (order included) and the forker's transcript up to the fork
/// boundary, byte for byte. Without the fix the host builds the child as a
/// generic member: its tools diverge right after the shared prefix.
#[tokio::test(flavor = "multi_thread")]
async fn e2e_fast_mob_fork_off_child_request_matches_the_forker_byte_for_byte() {
    let stack = Stack::new().await;
    let mob_id = MobId::from(format!("fork-parity-{}", uuid::Uuid::new_v4().simple()));
    let (handle, source_session) = stack.seed_source(&mob_id).await;

    // A committed exchange before the fork, so the inherited prefix is a real
    // conversation and not only the system prompt.
    assert_eq!(bounded_turn(&handle, SOURCE, SOURCE_TURN).await, "ACK");
    assert_eq!(bounded_turn(&handle, SOURCE, FORK_PROMPT).await, FORK_DONE);

    let forker = stack.only_body("forker fork_off", |body| {
        last_user_text(body).contains(FORK_PROMPT) && !has_tool_output(body)
    });
    let child = stack.only_body("child first", |body| {
        last_user_text(body).contains(CHILD_TASK)
    });
    let forker_parts = body_parts(&forker);
    let child_parts = body_parts(&child);

    let forker_tools = forker_parts.tools.expect("the forker sends tools").get();
    let child_tools = child_parts.tools.expect("the child sends tools").get();
    let forker_names = tool_names(&forker);
    for tool in CALENDAR_HOST_TOOLS.iter().chain([&OVERLAY_TOOL]) {
        assert!(
            forker_names.iter().any(|name| name == tool),
            "the forker carries '{tool}': {forker_names:?}"
        );
    }
    assert!(forker_names.iter().any(|name| name == "fork_off"));
    assert_eq!(
        child_tools,
        forker_tools,
        "the child's tools array must be the forker's byte for byte; child tools: {:?}",
        tool_names(&child)
    );

    // The child's input is the forker's committed transcript at the fork
    // boundary followed by its own task. `fork_off` runs inside the forker's
    // turn and branches at the last committed boundary, which is everything
    // the forker sent except the in-flight user message that asked for the
    // fork: the system prompt and the earlier exchange.
    let task_at = child_parts
        .input
        .iter()
        .position(|item| item.get().contains(CHILD_TASK))
        .expect("the child's request carries its task");
    let inherited = &child_parts.input[..task_at];
    assert_eq!(
        inherited.len() + 1,
        forker_parts.input.len(),
        "the child inherits the forker's request up to its in-flight fork prompt"
    );
    assert!(
        inherited
            .iter()
            .any(|item| item.get().contains(SOURCE_TURN)),
        "the inherited prefix carries the forker's earlier exchange"
    );
    for (index, (child_item, forker_item)) in
        inherited.iter().zip(forker_parts.input.iter()).enumerate()
    {
        assert_eq!(
            child_item.get(),
            forker_item.get(),
            "input item {index} differs before the fork boundary"
        );
    }
    // Everything but the transcript is the same request: model, tools,
    // reasoning and output settings, byte for byte.
    let forker_fields: BTreeMap<&str, &RawValue> = serde_json::from_str(&forker).unwrap();
    let child_fields: BTreeMap<&str, &RawValue> = serde_json::from_str(&child).unwrap();
    assert_eq!(
        forker_fields.keys().collect::<Vec<_>>(),
        child_fields.keys().collect::<Vec<_>>(),
        "the child's request has the forker's top-level fields"
    );
    for (field, value) in &forker_fields {
        if *field == "input" {
            continue;
        }
        assert_eq!(
            child_fields[field].get(),
            value.get(),
            "top-level request field '{field}' differs between forker and child"
        );
    }

    let expected_source = meerkat_core::ForkBuildSource::new(
        meerkat_core::MobMemberBinding {
            mob_id: mob_id.to_string(),
            role: PROFILE.to_string(),
            member: SOURCE.to_string(),
        },
        source_session,
    );
    assert_built_as_source(
        &stack.last_build(SOURCE),
        &stack.last_build(CHILD),
        CHILD,
        &mob_id,
        &expected_source,
    );

    stack.teardown().await;
}

/// A temporary-council participant is a fork of its convener's member and is
/// built the same way: the source's application context and labels, the
/// typed source reference, and the source's exact tool set.
#[tokio::test(flavor = "multi_thread")]
async fn e2e_fast_mob_fork_off_council_participant_is_built_as_its_source() {
    let stack = Stack::new().await;
    let scope = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
    let source_mob = MobId::from(format!("fork-parity-src-{scope}"));
    let (handle, _source_session) = stack.seed_source(&source_mob).await;
    assert_eq!(bounded_turn(&handle, SOURCE, SOURCE_TURN).await, "ACK");

    let council_id =
        meerkat_mob::temporary_council::TemporaryCouncilId::new(format!("fork-parity-{scope}"))
            .expect("canonical council id");
    let temporary_mob = council_id.temporary_mob_id();
    let request = TemporaryCouncilRequest::new(
        council_id,
        definition("template-is-replaced"),
        vec![TemporaryCouncilParticipantSpec::new(
            0,
            "planner",
            source_mob.clone(),
            AgentIdentity::from(SOURCE),
            AgentIdentity::from(SEAT),
            ProfileName::from(PROFILE),
        )],
        "What goes on tomorrow's agenda?",
        TemporaryCouncilBounds::relative(WAIT, 1, 4096),
        MergeBackPolicy::NoMerge,
    );
    let outcome = stack
        .state
        .temporary_council()
        .run(request)
        .await
        .expect("council runs to a terminal outcome");
    let participant = outcome
        .result
        .participants
        .first()
        .expect("the participant's provenance");
    assert!(participant.seated, "the participant was seated");
    let capability = participant
        .capability
        .as_ref()
        .expect("a seated participant carries capability provenance");

    let expected_source = meerkat_core::ForkBuildSource::new(
        meerkat_core::MobMemberBinding {
            mob_id: source_mob.to_string(),
            role: PROFILE.to_string(),
            member: SOURCE.to_string(),
        },
        capability.source_provenance.source_session_id.clone(),
    );
    assert_built_as_source(
        &stack.last_build(SOURCE),
        &stack.last_build(SEAT),
        SEAT,
        &temporary_mob,
        &expected_source,
    );

    let source_body = stack.only_body("source turn", |body| {
        last_user_text(body).contains(SOURCE_TURN)
    });
    let seat_body = stack
        .bodies
        .all()
        .into_iter()
        .find(|body| {
            let value: Value = serde_json::from_str(body).unwrap();
            value.to_string().contains("What goes on tomorrow")
        })
        .expect("the participant's council turn reached the provider");
    let seat_names = tool_names(&seat_body);
    for tool in CALENDAR_HOST_TOOLS.iter().chain([&OVERLAY_TOOL]) {
        assert!(
            seat_names.iter().any(|name| name == tool),
            "the participant carries the source's '{tool}': {seat_names:?}"
        );
    }
    assert!(
        !seat_names.iter().any(|name| name == GENERIC_HOST_TOOL),
        "the participant is not built as a generic member: {seat_names:?}"
    );
    assert_eq!(
        body_parts(&seat_body).tools.map(RawValue::get),
        body_parts(&source_body).tools.map(RawValue::get),
        "the participant's tools array is the source's byte for byte"
    );

    stack.teardown().await;
}

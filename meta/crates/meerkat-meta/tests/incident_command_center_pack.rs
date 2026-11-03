#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args,
    clippy::collapsible_if,
    clippy::redundant_clone,
    clippy::needless_raw_string_hashes,
    clippy::single_match,
    clippy::redundant_closure_for_method_calls,
    clippy::redundant_pattern_matching,
    clippy::ignored_unit_patterns,
    clippy::clone_on_copy,
    clippy::manual_assert,
    clippy::unwrap_in_result,
    clippy::useless_vec
)]

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use futures::{Stream, stream};
use meerkat::AgentToolDispatcher;
use meerkat_client::{LlmError, LlmEvent, LlmRequest};
use meerkat_core::lifecycle::run_primitive::ModelId;
use meerkat_core::types::{ContentInput, HandlingMode};
use meerkat_core::{AgentEvent, Provider, StopReason};
use meerkat_meta::mob_handle_runtime::MobRuntimeError;
use meerkat_meta::{
    DiscoverySpec, MemberTurnOptions, MetaConfig, MobBootstrapOptions, MobBootstrapSpec,
    UnifiedRuntime,
};
use meerkat_mob::{MobDefinition, MobError, MobSessionService, MobStorage, SpawnMemberSpec};
use serde_json::{Value, json};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;

#[path = "../../../examples/001-incident-command-center-pack/incident_command_center.rs"]
mod incident_command_center;

/// The one definition of the normalized-provider-accounting contract every
/// Meta LLM double must satisfy under meerkat 0.8.22. See the module docs.
#[path = "support/llm_usage.rs"]
mod llm_usage;

use incident_command_center::{
    IncidentSessionHook, IncidentToolDispatcher, build_runtime_bundle_with_default_client,
    scenario_path,
};

/// Per-test mob id counter: 0.8.23's fail-closed in-proc registration
/// means concurrently running tests must not share a supervisor route.
static NEXT_TEST_MOB_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The shipped example pack pins `id = "incident-command-center"`, so tests
/// that build the stock bundle share ONE supervisor route name. Serialize
/// them and stand each runtime down (terminal shutdown releases the route)
/// so successive bootstraps succeed under 0.8.23's fail-closed registration.
static STOCK_BUNDLE_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

#[derive(Clone, Default)]
struct IncidentPackTestClient {
    requested_models: Arc<std::sync::Mutex<Vec<String>>>,
    provider: Option<Provider>,
}

impl IncidentPackTestClient {
    fn for_provider(provider: Provider) -> Self {
        Self {
            provider: Some(provider),
            ..Self::default()
        }
    }

    fn requested_models(&self) -> Vec<String> {
        self.requested_models
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl meerkat_client::LlmClient for IncidentPackTestClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        self.requested_models
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(request.model.clone());
        // meerkat 0.8.22 rejects a turn whose stream carried no normalized
        // provider accounting, so the terminal `Done` never travels alone.
        // This double is deliberately pointed at several models across the
        // pack's profiles, so the accounting identity is derived per request
        // rather than restated as one literal.
        let provider = meerkat_client::LlmClient::provider(self);
        let [usage, done] = llm_usage::usage_then_done(request, provider, StopReason::EndTurn);
        Box::pin(stream::iter([
            Ok(LlmEvent::TextDelta {
                delta: "ok".to_string(),
                meta: None,
            }),
            Ok(usage),
            Ok(done),
        ]))
    }

    // meerkat 0.7: LlmClient::provider returns the typed Provider.
    fn provider(&self) -> meerkat::Provider {
        self.provider.unwrap_or(meerkat::Provider::OpenAI)
    }

    fn health_check<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

#[derive(Clone)]
struct SelfHostedRouteStubState {
    response_text: &'static str,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

async fn self_hosted_route_stub(
    axum::extract::State(state): axum::extract::State<SelfHostedRouteStubState>,
) -> impl axum::response::IntoResponse {
    state
        .calls
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let payload = format!(
        concat!(
            "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{}\"}}}}]}}\n\n",
            "data: {{\"choices\":[{{\"finish_reason\":\"stop\"}}],",
            "\"usage\":{{\"prompt_tokens\":1,\"completion_tokens\":1}}}}\n\n",
            "data: [DONE]\n\n"
        ),
        state.response_text
    );
    ([("content-type", "text/event-stream")], payload)
}

async fn self_hosted_route_stub_models() -> impl axum::response::IntoResponse {
    axum::Json(json!({"data": []}))
}

async fn spawn_self_hosted_route_stub(
    response_text: &'static str,
) -> (
    String,
    Arc<std::sync::atomic::AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let app = axum::Router::new()
        .route(
            "/v1/chat/completions",
            axum::routing::post(self_hosted_route_stub),
        )
        .route(
            "/v1/models",
            axum::routing::get(self_hosted_route_stub_models),
        )
        .with_state(SelfHostedRouteStubState {
            response_text,
            calls: Arc::clone(&calls),
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind self-hosted route stub");
    let address = listener
        .local_addr()
        .expect("read self-hosted route stub address");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve self-hosted route stub");
    });
    (format!("http://{address}"), calls, server)
}

async fn json_response(app: axum::Router, request: Request<Body>) -> Value {
    let response = app.oneshot(request).await.expect("router response");
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body bytes");
    serde_json::from_slice(&body).expect("json body")
}

#[test]
fn incident_local_tools_are_witnessed_for_delegate_inheritance() {
    // meerkat 0.7 made meerkat_mob::snapshot private; the same provenance
    // contract is asserted through the public meerkat_core::tool_scope
    // witness derivation the snapshot used internally.
    let tools = IncidentToolDispatcher { inner: None }.tools();
    let defs: Vec<meerkat_core::types::ToolDef> =
        tools.iter().map(|tool| (**tool).clone()).collect();
    let filter = meerkat_core::tool_scope::ToolFilter::Allow(
        defs.iter().map(|def| def.name.to_string()).collect(),
    );
    let witnesses = meerkat_core::tool_scope::filter_witnesses_for_tool_defs(&defs, &filter);

    assert!(
        witnesses.contains_key("inspect_service"),
        "inspect_service must carry provenance so delegate can inherit it"
    );
    assert!(
        witnesses.contains_key("analyze_customer_impact"),
        "analyze_customer_impact must carry provenance so delegate can inherit it"
    );
}

// Regression: the incident tool dispatcher must COMPOSE over any external
// tools already installed on the build (the agent-memory recorder's `memory`
// tool), not replace them. The original hook overwrote build.external_tools,
// silently dropping the recorder so `memory` never reached the model and the
// Memory panel stayed empty.
#[tokio::test]
async fn incident_dispatcher_composes_inner_external_tools() {
    use meerkat_core::types::{ToolCallView, ToolDef, ToolResult};
    use meerkat_core::{ToolDispatchOutcome, ToolError};

    struct FakeInner;
    #[async_trait::async_trait]
    impl AgentToolDispatcher for FakeInner {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            vec![Arc::new(ToolDef {
                audience: Default::default(),
                name: "memory".into(),
                description: "stand-in recorder".to_string(),
                input_schema: json!({"type": "object"}),
                provenance: None,
            })]
            .into()
        }
        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
            Ok(ToolResult::new(call.id.to_string(), "recorded".to_string(), false).into())
        }
    }

    let dispatcher = IncidentToolDispatcher {
        inner: Some(Arc::new(FakeInner)),
    };
    let names: Vec<String> = dispatcher
        .tools()
        .iter()
        .map(|t| t.name.to_string())
        .collect();
    assert!(names.iter().any(|n| n == "inspect_service"));
    assert!(names.iter().any(|n| n == "analyze_customer_impact"));
    assert!(
        names.iter().any(|n| n == "memory"),
        "composed inner tool (memory recorder) must surface alongside incident tools: {names:?}"
    );

    // And an unknown-to-us call must delegate to the inner dispatcher (rather
    // than NotFound) — proving the recorder's tool is actually callable.
    let empty_args = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
    dispatcher
        .dispatch(ToolCallView {
            id: "call-1",
            name: "memory",
            args: &empty_args,
        })
        .await
        .expect("unknown tool must route to the inner recorder, not NotFound");

    // With no inner, an unknown tool is genuinely absent.
    let bare = IncidentToolDispatcher { inner: None };
    let miss = bare
        .dispatch(ToolCallView {
            id: "call-2",
            name: "memory",
            args: &empty_args,
        })
        .await;
    assert!(matches!(miss, Err(ToolError::NotFound { .. })));
}

// Regression: before_create must COMPOSE both external_tools AND
// additional_instructions over what the agent-memory customizer already
// installed on the build. Overwriting additional_instructions dropped the
// build-time memory-recall injection, so a respawned/fresh member never
// recalled its stored memories (the "respawn forgets my name" report).
#[tokio::test]
async fn incident_hook_composes_memory_injection_and_tools() {
    use meerkat_core::config::SystemPromptOverride;
    use meerkat_core::service::{CreateSessionRequest, InitialTurnPolicy, SessionBuildOptions};
    use meerkat_meta::SessionHook;

    // Stand in for the agent-memory customizer's pre-installed build state:
    // a recalled-memory injection line + a recorder external-tool dispatcher.
    let build = SessionBuildOptions {
        additional_instructions: Some(vec!["MEMORY-RECALL: operator name is Luka.".to_string()]),
        external_tools: Some(Arc::new(RecorderMarkerDispatcher)),
        ..SessionBuildOptions::default()
    };

    let mut req = CreateSessionRequest {
        model: "gpt-5.5".to_string(),
        prompt: meerkat_core::ContentInput::Text(String::new()),
        system_prompt: SystemPromptOverride::Inherit,
        max_tokens: None,
        event_tx: None,
        initial_turn: InitialTurnPolicy::Defer,
        build: Some(build),
        labels: None,
        deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::default(),
        injected_context: Vec::new(),
    };

    IncidentSessionHook
        .before_create(&mut req)
        .await
        .expect("before_create");

    let build = req.build.expect("build present");
    let instructions = build.additional_instructions.expect("instructions present");
    assert!(
        instructions
            .iter()
            .any(|line| line.contains("operator name is Luka")),
        "the pre-installed memory-recall injection must survive (composed, not overwritten): {instructions:?}"
    );
    assert!(
        instructions
            .iter()
            .any(|line| line.contains("incident command center")),
        "the incident framing must also be present"
    );
    let tool_names: Vec<String> = build
        .external_tools
        .expect("external tools present")
        .tools()
        .iter()
        .map(|t| t.name.to_string())
        .collect();
    assert!(
        tool_names.iter().any(|n| n == "memory-marker"),
        "the pre-installed recorder tool must survive alongside incident tools: {tool_names:?}"
    );
    assert!(tool_names.iter().any(|n| n == "inspect_service"));
}

struct RecorderMarkerDispatcher;

#[async_trait::async_trait]
impl AgentToolDispatcher for RecorderMarkerDispatcher {
    fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
        vec![Arc::new(meerkat_core::types::ToolDef {
            audience: Default::default(),
            name: "memory-marker".into(),
            description: "stand-in recorder".to_string(),
            input_schema: json!({"type": "object"}),
            provenance: None,
        })]
        .into()
    }
    async fn dispatch(
        &self,
        call: meerkat_core::types::ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        Ok(
            meerkat_core::types::ToolResult::new(call.id.to_string(), "ok".to_string(), false)
                .into(),
        )
    }
}

#[tokio::test]
async fn incident_pack_exposes_seeded_stock_console_state() {
    let _serial = STOCK_BUNDLE_LOCK.lock().await;
    let bundle = Box::pin(build_runtime_bundle_with_default_client(
        &scenario_path().expect("incident scenario path"),
        Arc::new(IncidentPackTestClient::default()),
    ))
    .await
    .expect("incident runtime bundle");
    let app = bundle
        .runtime
        .build_reference_app_router(bundle.decisions.clone());

    let experience = json_response(
        app.clone(),
        Request::builder()
            .uri("/console/experience")
            .body(Body::empty())
            .expect("experience request"),
    )
    .await;

    assert_eq!(experience["contract_version"], json!("0.5.0"));
    let agents = experience["agent_sidebar"]["live_snapshot"]["agents"]
        .as_array()
        .expect("agent rows");
    let commander = agents
        .iter()
        .find(|entry| entry["identity"] == json!("incident-commander"))
        .expect("incident commander row");
    assert_eq!(commander["watched"], json!(true));
    assert_eq!(commander["alertLevel"], json!("critical"));

    let health_monitor = agents
        .iter()
        .find(|entry| entry["identity"] == json!("health-monitor"))
        .expect("health monitor row");
    assert_eq!(health_monitor["degraded"], json!(true));
    assert_eq!(health_monitor["degradedReason"], json!("peer_unreachable"));

    let topology_nodes = experience["topology"]["live_snapshot"]["nodes"]
        .as_array()
        .expect("topology nodes");
    let commander_node = topology_nodes
        .iter()
        .find(|entry| entry["identity"] == json!("incident-commander"))
        .expect("incident commander topology node");
    let wired_to = commander_node["wired_to"]
        .as_array()
        .expect("wired_to array");
    assert!(
        wired_to.contains(&json!("payments-sre")) || wired_to.contains(&json!("merchant-comms")),
        "incident commander should have seeded runtime wiring"
    );

    let filter_presets = experience["activity_feed"]["filter_presets"]
        .as_array()
        .expect("activity filter presets");
    assert!(
        filter_presets
            .iter()
            .any(|preset| preset["id"] == json!("watched-only")),
        "watched-only filter preset should be projected"
    );

    let routes = json_response(
        app.clone(),
        Request::builder()
            .method("POST")
            .uri("/console/rpc")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "jsonrpc": "2.0",
                    "id": "incident-pack-routes",
                    "method": "meta/routing/routes/list",
                    "params": {},
                })
                .to_string(),
            ))
            .expect("routes request"),
    )
    .await;
    assert!(
        routes.get("error").is_none() || routes["error"].is_null(),
        "routing rpc should succeed: {routes:?}"
    );
    let route_rows = routes["result"]["routes"]
        .as_array()
        .unwrap_or_else(|| panic!("routes array missing in response: {routes:?}"));
    assert!(
        route_rows
            .iter()
            .any(|route| route["route_key"] == json!("incident-statuspage")),
        "seeded statuspage route should exist"
    );

    let pending = json_response(
        app,
        Request::builder()
            .method("POST")
            .uri("/console/rpc")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "jsonrpc": "2.0",
                    "id": "incident-pack-gating",
                    "method": "meta/gating/pending",
                    "params": {},
                })
                .to_string(),
            ))
            .expect("gating request"),
    )
    .await;
    assert!(
        pending.get("error").is_none() || pending["error"].is_null(),
        "gating rpc should succeed: {pending:?}"
    );
    let pending_rows = pending["result"]["pending"]
        .as_array()
        .unwrap_or_else(|| panic!("pending array missing in response: {pending:?}"));
    assert!(
        !pending_rows.is_empty(),
        "seeded gating pending entry should exist"
    );
    // Retire the roster through the ordinary reconcile path before terminal
    // shutdown: an autonomous kickoff caught mid-attach cannot be interrupted
    // by the machine Shutdown path ("Runtime not ready: attached"), which
    // would leave the stock supervisor route occupied for the next test.
    let _ = bundle.runtime.reconcile(Vec::new()).await;
    let _ = bundle.runtime.shutdown().await;
}

/// Regression test: the seeded R3 gating action must deliver its approval
/// notification even though the bundle is built inside an active tokio
/// runtime. `UnifiedRuntime::evaluate_gating_action` previously ran the
/// router/delivery MCP boundary calls directly on the runtime thread, so the
/// notification silently failed with
/// `cannot execute blocking MCP boundary call inside an active tokio runtime`
/// and the pending request timed out to safe_draft.
#[tokio::test]
async fn incident_pack_gating_approval_notification_fires_from_async_context() {
    let _serial = STOCK_BUNDLE_LOCK.lock().await;
    let bundle = Box::pin(build_runtime_bundle_with_default_client(
        &scenario_path().expect("incident scenario path"),
        Arc::new(IncidentPackTestClient::default()),
    ))
    .await
    .expect("incident runtime bundle");
    let app = bundle
        .runtime
        .build_reference_app_router(bundle.decisions.clone());

    let pending = json_response(
        app.clone(),
        Request::builder()
            .method("POST")
            .uri("/console/rpc")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "jsonrpc": "2.0",
                    "id": "incident-pack-gating-pending",
                    "method": "meta/gating/pending",
                    "params": {},
                })
                .to_string(),
            ))
            .expect("gating pending request"),
    )
    .await;
    let pending_rows = pending["result"]["pending"]
        .as_array()
        .unwrap_or_else(|| panic!("pending array missing in response: {pending:?}"));
    let seeded = pending_rows
        .iter()
        .find(|entry| entry["action"] == json!("publish_status_update"))
        .unwrap_or_else(|| panic!("seeded publish_status_update pending entry: {pending:?}"));
    assert!(
        seeded["approval_route_id"].is_string(),
        "approval route must be resolved through the router module: {seeded:?}"
    );
    assert!(
        seeded["approval_delivery_id"].is_string(),
        "approval notification must be delivered through the delivery module: {seeded:?}"
    );

    let audit = json_response(
        app.clone(),
        Request::builder()
            .method("POST")
            .uri("/console/rpc")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "jsonrpc": "2.0",
                    "id": "incident-pack-gating-audit",
                    "method": "meta/gating/audit",
                    "params": { "limit": 100 },
                })
                .to_string(),
            ))
            .expect("gating audit request"),
    )
    .await;
    let entries = audit["result"]["entries"]
        .as_array()
        .unwrap_or_else(|| panic!("audit entries missing in response: {audit:?}"));
    let pending_created = entries
        .iter()
        .find(|entry| {
            entry["event_type"] == json!("pending_created")
                && entry["action_id"] == seeded["action_id"]
        })
        .unwrap_or_else(|| panic!("pending_created audit entry: {audit:?}"));
    assert!(
        pending_created["detail"]["approval_notification_error"].is_null(),
        "approval notification must not fail: {pending_created:?}"
    );

    // Settle the pending action through the real approval lifecycle before
    // teardown: the gated member turn stays parked (runtime session
    // "attached") while the decision is outstanding, and terminal shutdown
    // cannot interrupt a parked turn - the supervisor route would stay live
    // and block the next stock-bundle bootstrap in this process.
    let decided = json_response(
        app,
        Request::builder()
            .method("POST")
            .uri("/console/rpc")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "jsonrpc": "2.0",
                    "id": "incident-pack-gating-decide",
                    "method": "meta/gating/decide",
                    "params": {
                        "pending_id": seeded["pending_id"],
                        // The seeded pending pins its approver; a mismatched
                        // principal is refused.
                        "approver_id": seeded["requested_approver"]
                            .as_str()
                            .unwrap_or("incident-pack-test-operator"),
                        "decision": "reject",
                        "reason": "test teardown: settle the seeded pending action",
                    },
                })
                .to_string(),
            ))
            .expect("gating decide request"),
    )
    .await;
    assert!(
        decided["error"].is_null(),
        "seeded pending action must be decidable: {decided:#?}"
    );
    // Retire the roster through the ordinary reconcile path before terminal
    // shutdown: an autonomous kickoff caught mid-attach cannot be interrupted
    // by the machine Shutdown path ("Runtime not ready: attached"), which
    // would leave the stock supervisor route occupied for the next test.
    let _ = bundle.runtime.reconcile(Vec::new()).await;
    let _ = bundle.runtime.shutdown().await;
}

async fn assert_public_ephemeral_constructor_supports_live_llm_switching(with_hook: bool) {
    let constructor_name = if with_hook {
        "ephemeral_with_hook"
    } else {
        "ephemeral"
    };
    let definition = MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "public-{constructor_name}-llm-switch-test"

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "turn_driven"
external_addressable = true

[profiles.worker.tools]
comms = true
"#,
    ))
    .unwrap_or_else(|error| panic!("parse {constructor_name} test definition: {error}"));
    let temp_dir = tempfile::tempdir()
        .unwrap_or_else(|error| panic!("create {constructor_name} tempdir: {error}"));
    let hook_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let spec = if with_hook {
        let hook_calls = Arc::clone(&hook_calls);
        MobBootstrapSpec::ephemeral_with_hook(
            definition,
            MobStorage::in_memory(),
            temp_dir.path().join("sessions"),
            16,
            None,
            move |_request| {
                let hook_calls = Arc::clone(&hook_calls);
                Box::pin(async move {
                    hook_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Ok(())
                })
            },
        )
    } else {
        MobBootstrapSpec::ephemeral(
            definition,
            MobStorage::in_memory(),
            temp_dir.path().join("sessions"),
            16,
            None,
        )
    }
    .unwrap_or_else(|error| panic!("build {constructor_name} ephemeral spec: {error}"));
    let llm_client = Arc::new(IncidentPackTestClient::for_provider(Provider::Other));
    let spec = spec.with_options(MobBootstrapOptions {
        allow_ephemeral_sessions: true,
        notify_orchestrator_on_resume: true,
        default_llm_client: Some(llm_client.clone()),
    });
    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .mob_spec(spec)
            .module_config(MetaConfig {
                modules: Vec::new(),
                discovery: DiscoverySpec {
                    namespace: format!("public-{constructor_name}-llm-switch-test"),
                    modules: Vec::new(),
                },
                pre_spawn: Vec::new(),
            })
            .timeout(Duration::from_secs(5))
            .build(),
    )
    .await
    .unwrap_or_else(|error| panic!("build {constructor_name} runtime: {error}"));
    runtime
        .spawn(SpawnMemberSpec::from_wire(
            "worker".to_string(),
            "switch-worker".to_string(),
            None,
            None,
            None,
        ))
        .await
        .unwrap_or_else(|error| panic!("spawn {constructor_name} worker: {error}"));

    if with_hook {
        assert!(
            hook_calls.load(std::sync::atomic::Ordering::SeqCst) > 0,
            "ephemeral_with_hook must retain its pre-build hook while installing the LLM reconfiguration host"
        );
    }

    for (model, provider) in [
        ("gpt-5.6", Provider::OpenAI),
        ("claude-opus-4-8", Provider::Anthropic),
    ] {
        let mut admission = runtime
            .start_member_turn(
                "switch-worker",
                ContentInput::Text(format!("run with {provider:?}:{model}")),
                HandlingMode::Queue,
                MemberTurnOptions::new()
                    .with_model(ModelId::new(model))
                    .with_provider(provider),
                None,
            )
            .await
            .unwrap_or_else(|error| {
                panic!("{constructor_name} must admit {provider:?}:{model}: {error}")
            });
        let applied_identity = tokio::time::timeout(
            Duration::from_secs(2),
            admission.turn.wait_for_applied_llm_identity(),
        )
        .await
        .unwrap_or_else(|error| {
            panic!("{constructor_name} identity acknowledgement timed out: {error}")
        })
        .unwrap_or_else(|error| {
            panic!("{constructor_name} failed to apply {provider:?}:{model}: {error}")
        })
        .unwrap_or_else(|| panic!("{constructor_name} dropped the requested identity"));
        assert_eq!(applied_identity.model, model);
        assert_eq!(applied_identity.provider, provider);
        tokio::time::timeout(Duration::from_secs(2), admission.turn.wait())
            .await
            .unwrap_or_else(|error| panic!("{constructor_name} turn timed out: {error}"))
            .unwrap_or_else(|error| panic!("{constructor_name} turn failed: {error}"));
    }

    assert_eq!(
        llm_client.requested_models(),
        vec!["gpt-5.6".to_string(), "claude-opus-4-8".to_string()],
        "the applied model/provider identities must reach the live executor in order"
    );

    let requested_models_before_rejection = llm_client.requested_models();
    let mut rejected = runtime
        .start_member_turn(
            "switch-worker",
            ContentInput::Text("do not silently use the old identity".to_string()),
            HandlingMode::Queue,
            MemberTurnOptions::new()
                .with_model(ModelId::new("claude-opus-4-8"))
                .with_provider(Provider::OpenAI),
            None,
        )
        .await
        .unwrap_or_else(|error| {
            panic!("{constructor_name} should represent the rejected turn admission: {error}")
        });
    let rejection = tokio::time::timeout(
        Duration::from_secs(2),
        rejected.turn.wait_for_applied_llm_identity(),
    )
    .await
    .unwrap_or_else(|error| panic!("{constructor_name} rejection timed out: {error}"))
    .expect_err("provider/model mismatch must fail closed at the installed host");
    assert!(
        rejection.to_string().contains("provider") || rejection.to_string().contains("model"),
        "rejection must explain the invalid provider/model identity, got {rejection}"
    );
    assert_eq!(
        llm_client.requested_models(),
        requested_models_before_rejection,
        "a rejected identity must not fall back to the previous live client"
    );

    let _ = runtime.shutdown().await;
}

#[tokio::test]
async fn public_ephemeral_constructor_supports_live_model_and_provider_switching() {
    assert_public_ephemeral_constructor_supports_live_llm_switching(false).await;
}

#[tokio::test]
async fn public_ephemeral_with_hook_constructor_supports_live_model_and_provider_switching() {
    assert_public_ephemeral_constructor_supports_live_llm_switching(true).await;
}

#[tokio::test]
async fn unified_runtime_member_turn_returns_exact_session_and_committed_completion() {
    let definition = MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "member-turn-wrapper-test-{}"

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "turn_driven"
external_addressable = true

[profiles.worker.tools]
comms = true
"#,
        NEXT_TEST_MOB_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
    .expect("parse turn-driven test definition");
    let llm_client = Arc::new(IncidentPackTestClient::default());
    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .definition(definition)
            .default_llm_client(llm_client.clone())
            .timeout(Duration::from_secs(5))
            .build(),
    )
    .await
    .expect("build turn-driven unified runtime");
    runtime
        .spawn(SpawnMemberSpec::from_wire(
            "worker".to_string(),
            "console-worker".to_string(),
            None,
            None,
            None,
        ))
        .await
        .expect("spawn turn-driven console worker");

    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(64);
    let mut admission = runtime
        .start_member_turn(
            "console-worker",
            ContentInput::Text("report status".to_string()),
            HandlingMode::Queue,
            MemberTurnOptions::new()
                .with_model(ModelId::new("gpt-5.6"))
                .with_provider(Provider::OpenAI),
            Some(event_tx),
        )
        .await
        .expect("admit completion-bearing member turn");
    assert_eq!(
        admission.turn.session_id().map(ToString::to_string),
        Some(admission.session_id.clone()),
        "the wrapper must expose the exact bridge session captured by admission"
    );
    let applied_identity = tokio::time::timeout(
        Duration::from_secs(2),
        admission.turn.wait_for_applied_llm_identity(),
    )
    .await
    .expect("applied identity acknowledgement should resolve")
    .expect("stock runtime should apply the requested identity")
    .expect("turn requested a non-default identity");
    assert_eq!(applied_identity.model, "gpt-5.6");
    assert_eq!(applied_identity.provider, Provider::OpenAI);

    let receipt = tokio::time::timeout(Duration::from_secs(2), admission.turn.wait())
        .await
        .expect("turn completion should resolve")
        .expect("turn should commit successfully");
    assert_eq!(receipt.handling_mode, HandlingMode::Queue);

    let mut observed_text_output = false;
    let mut observed_committed_completion = false;
    let mut observed_events = Vec::new();
    while !observed_committed_completion {
        let event = tokio::time::timeout(Duration::from_secs(2), event_rx.recv())
            .await
            .expect("live member-turn event should arrive")
            .expect("member-turn event channel should remain open through completion");
        observed_events.push(format!("{:?}", event.payload));
        match event.payload {
            AgentEvent::TextDelta { delta, .. } => {
                observed_text_output |= delta == "ok";
            }
            AgentEvent::TextComplete { content, .. } => {
                observed_text_output |= content == "ok";
            }
            AgentEvent::RunCompleted { session_id, .. } => {
                assert_eq!(
                    session_id.to_string(),
                    admission.session_id,
                    "the committed terminal must carry the exact admitted session"
                );
                observed_committed_completion = true;
            }
            AgentEvent::RunFailed { error_report, .. } => {
                panic!("member turn unexpectedly failed: {error_report:?}")
            }
            _ => {}
        }
    }
    assert!(
        observed_text_output,
        "the stock incident-console path must forward model output before completion; observed {observed_events:?}"
    );
    assert!(
        llm_client
            .requested_models()
            .iter()
            .any(|model| model == "gpt-5.6"),
        "the stock incident-console constructor must deliver the applied model to LlmRequest"
    );

    let _ = runtime.shutdown().await;
}

#[tokio::test]
async fn unified_runtime_member_turn_preserves_exact_self_hosted_route_at_executor_boundary() {
    const INFERRED_MODEL: &str = "meta-route-model-a";
    const PINNED_MODEL: &str = "meta-route-model-b";
    const INFERRED_SERVER_ID: &str = "local-a";
    const PINNED_SERVER_ID: &str = "local-b";

    let (inferred_base_url, inferred_calls, inferred_server) =
        spawn_self_hosted_route_stub("from-local-a").await;
    let (pinned_base_url, pinned_calls, pinned_server) =
        spawn_self_hosted_route_stub("from-local-b").await;

    let definition = MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "member-turn-self-hosted-route-test-{}"

[profiles.worker]
model = "{INFERRED_MODEL}"
provider = "self_hosted"
runtime_mode = "turn_driven"
external_addressable = true

[profiles.worker.tools]
comms = true
"#,
        NEXT_TEST_MOB_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
    ))
    .expect("parse self-hosted route test definition");

    let mut config = meerkat::Config::default();
    for (server_id, base_url) in [
        (INFERRED_SERVER_ID, inferred_base_url),
        (PINNED_SERVER_ID, pinned_base_url),
    ] {
        config.self_hosted.servers.insert(
            server_id.to_string(),
            meerkat_core::SelfHostedServerConfig {
                base_url,
                ..Default::default()
            },
        );
    }
    for (model, server_id) in [
        (INFERRED_MODEL, INFERRED_SERVER_ID),
        (PINNED_MODEL, PINNED_SERVER_ID),
    ] {
        config.self_hosted.models.insert(
            model.to_string(),
            meerkat_core::SelfHostedModelConfig {
                server: server_id.to_string(),
                // Both hosts expose the same raw model name. The alias selects
                // a registered target; self_hosted_server_id remains the exact
                // route witness and fail-closed mismatch guard.
                remote_model: "route-test:latest".to_string(),
                display_name: format!("Meta route test via {server_id}"),
                family: "route-test".to_string(),
                ..Default::default()
            },
        );
    }
    config.self_hosted.default_model = Some(INFERRED_MODEL.to_string());

    // One authless provider binding is shared by both servers and deliberately
    // carries no base URL. That leaves the registered server selected by the
    // exact session identity as the transport authority.
    let mut realm = meerkat_core::RealmConfigSection {
        default_binding: Some("local-self-hosted".to_string()),
        ..Default::default()
    };
    realm.backend.insert(
        "local-self-hosted-backend".to_string(),
        meerkat_core::BackendProfileConfig {
            provider: "self_hosted".to_string(),
            backend_kind: "self_hosted".to_string(),
            base_url: None,
            options: Value::Null,
            // Deliberately unpinned: this fixture asserts that the server is
            // selected by the exact session identity, so binding one here
            // would defeat what the test exists to prove.
            server: None,
        },
    );
    realm.auth.insert(
        "local-self-hosted-auth".to_string(),
        meerkat_core::AuthProfileConfig {
            provider: "self_hosted".to_string(),
            auth_method: "none".to_string(),
            source: meerkat_core::CredentialSourceSpec::ManagedStore,
            constraints: Default::default(),
            metadata_defaults: Default::default(),
        },
    );
    realm.binding.insert(
        "local-self-hosted".to_string(),
        meerkat_core::ProviderBindingConfig {
            // meerkat 0.8.31 added this field. `None` is the behaviour-preserving
            // value: ProviderBindingConfig::credential_identity falls back to
            // AuthCredentialIdentity::from_auth_binding when the account is absent,
            // which is exactly what this binding did before the field existed.
            credential_account: None,
            backend_profile: "local-self-hosted-backend".to_string(),
            auth_profile: "local-self-hosted-auth".to_string(),
            default_model: Some("route-test:latest".to_string()),
            policy: Default::default(),
            provider_default: true,
        },
    );
    config.realm.insert("global".to_string(), realm);

    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .definition(definition)
            .meerkat_config(config)
            .timeout(Duration::from_secs(5))
            .build(),
    )
    .await
    .expect("build runtime with the stock installed LLM reconfiguration host");
    runtime
        .spawn(SpawnMemberSpec::from_wire(
            "worker".to_string(),
            "route-worker".to_string(),
            None,
            None,
            None,
        ))
        .await
        .expect("spawn self-hosted route worker");

    let inferred_admission = runtime
        .start_member_turn(
            "route-worker",
            ContentInput::Text("use the profile-inferred local route".to_string()),
            HandlingMode::Queue,
            MemberTurnOptions::new(),
            None,
        )
        .await
        .expect("admit profile-inferred self-hosted member turn");
    tokio::time::timeout(Duration::from_secs(2), inferred_admission.turn.wait())
        .await
        .expect("profile-inferred turn completion should resolve")
        .expect("profile-inferred turn should commit successfully");
    assert_eq!(
        inferred_calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the profile model must initially infer local-a"
    );
    assert_eq!(
        pinned_calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "local-b must remain untouched before the explicit route override"
    );

    let mut admission = runtime
        .start_member_turn(
            "route-worker",
            ContentInput::Text("use the exact local route".to_string()),
            HandlingMode::Queue,
            MemberTurnOptions::new()
                .with_model(ModelId::new(PINNED_MODEL))
                .with_provider(Provider::SelfHosted)
                .with_self_hosted_server_id(PINNED_SERVER_ID),
            None,
        )
        .await
        .expect("admit exact self-hosted member turn");
    let applied_identity = tokio::time::timeout(
        Duration::from_secs(2),
        admission.turn.wait_for_applied_llm_identity(),
    )
    .await
    .expect("executor-boundary identity acknowledgement should resolve")
    .expect("the installed runtime host should apply the requested identity")
    .expect("the turn requested an explicit identity");
    assert_eq!(applied_identity.model, PINNED_MODEL);
    assert_eq!(applied_identity.provider, Provider::SelfHosted);
    assert_eq!(
        applied_identity.self_hosted_server_id.as_deref(),
        Some(PINNED_SERVER_ID),
        "the exact host route must survive UnifiedRuntime admission, member-turn metadata, and the serialized executor boundary"
    );

    tokio::time::timeout(Duration::from_secs(2), admission.turn.wait())
        .await
        .expect("self-hosted route turn completion should resolve")
        .expect("self-hosted route turn should commit successfully");
    assert_eq!(
        inferred_calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the explicit local-b route must not fall back to the profile-inferred local-a host"
    );
    assert_eq!(
        pinned_calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the exact local-b host must execute the explicitly pinned turn"
    );

    let mut mismatched = runtime
        .start_member_turn(
            "route-worker",
            ContentInput::Text("do not discard a conflicting exact route".to_string()),
            HandlingMode::Queue,
            MemberTurnOptions::new()
                .with_model(ModelId::new(INFERRED_MODEL))
                .with_provider(Provider::SelfHosted)
                .with_self_hosted_server_id(PINNED_SERVER_ID),
            None,
        )
        .await
        .expect("represent the rejected mismatched route as an admitted turn");
    let mismatch = tokio::time::timeout(
        Duration::from_secs(2),
        mismatched.turn.wait_for_applied_llm_identity(),
    )
    .await
    .expect("mismatched route validation should resolve")
    .expect_err("model-inferred local-a plus explicit local-b must fail closed");
    let mismatch = mismatch.to_string();
    assert!(
        mismatch.contains(INFERRED_MODEL)
            && mismatch.contains(INFERRED_SERVER_ID)
            && mismatch.contains(PINNED_SERVER_ID),
        "typed mismatch evidence must name the model, inferred host, and explicit host: {mismatch}"
    );
    assert_eq!(
        inferred_calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a rejected exact-route mismatch must not execute the model-inferred host"
    );
    assert_eq!(
        pinned_calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a rejected exact-route mismatch must not execute the conflicting explicit host"
    );

    let _ = runtime.shutdown().await;
    inferred_server.abort();
    pinned_server.abort();
}

#[tokio::test]
async fn unified_runtime_fails_closed_without_installed_session_llm_reconfigure_host() {
    let definition = MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "member-turn-missing-llm-host-test-{}"

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "turn_driven"
external_addressable = true

[profiles.worker.tools]
comms = true
"#,
        NEXT_TEST_MOB_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
    .expect("parse missing-host test definition");
    let temp_dir = tempfile::tempdir().expect("missing-host test tempdir");
    let factory = meerkat::AgentFactory::new(temp_dir.path().join("sessions")).comms(true);
    let session_service = Arc::new(meerkat::build_ephemeral_service(
        factory,
        meerkat::Config::default(),
        16,
    ));
    assert!(
        MobSessionService::supports_runtime_turn_apply(session_service.as_ref()),
        "the stock ephemeral service must support generic runtime-turn apply"
    );
    let runtime_adapter =
        MobSessionService::acquire_runtime_adapter(session_service.as_ref(), None)
            .expect("acquire the stock ephemeral service's runtime adapter")
            .expect("the stock ephemeral service exposes its runtime adapter");
    assert!(
        !runtime_adapter.has_session_llm_reconfigure_host(),
        "this fixture deliberately leaves the session LLM reconfiguration host uninstalled"
    );

    let spec = MobBootstrapSpec::new(definition, MobStorage::in_memory(), session_service)
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(IncidentPackTestClient::default())),
        });
    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .mob_spec(spec)
            .module_config(MetaConfig {
                modules: Vec::new(),
                discovery: DiscoverySpec {
                    namespace: "missing-llm-host-test".to_string(),
                    modules: Vec::new(),
                },
                pre_spawn: Vec::new(),
            })
            .timeout(Duration::from_secs(5))
            .build(),
    )
    .await
    .expect("build custom runtime without an LLM reconfiguration host");
    runtime
        .spawn(SpawnMemberSpec::from_wire(
            "worker".to_string(),
            "no-host-worker".to_string(),
            None,
            None,
            None,
        ))
        .await
        .expect("spawn worker backed by generic runtime-turn apply");

    let error = runtime
        .start_member_turn(
            "no-host-worker",
            ContentInput::Text("must not run on an unapplied model".to_string()),
            HandlingMode::Queue,
            MemberTurnOptions::new().with_model(ModelId::new("gpt-5.6")),
            None,
        )
        .await
        .expect_err("runtime-turn support alone must not advertise LLM reconfiguration");
    assert!(
        matches!(
            error,
            MobRuntimeError::Mob(MobError::MissingMemberCapability {
                capability: meerkat_mob::error::MobMemberCapability::SessionLlmReconfigure,
                context: "member turn LLM identity override",
                ..
            })
        ),
        "missing installed LLM host must fail closed before turn admission, got {error}"
    );

    let _ = runtime.shutdown().await;
}

#[tokio::test]
async fn stock_incident_commander_rejects_unrepresentable_tracked_turn() {
    let _serial = STOCK_BUNDLE_LOCK.lock().await;
    let bundle = Box::pin(build_runtime_bundle_with_default_client(
        &scenario_path().expect("incident scenario path"),
        Arc::new(IncidentPackTestClient::default()),
    ))
    .await
    .expect("incident runtime bundle");
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(1);

    let error = bundle
        .runtime
        .start_member_turn(
            "incident-commander",
            ContentInput::Text("tracked console turn".to_string()),
            HandlingMode::Queue,
            MemberTurnOptions::default(),
            Some(event_tx),
        )
        .await
        .expect_err("stock autonomous profiles must not silently drop tracking semantics");
    assert!(
        matches!(
            error,
            MobRuntimeError::Mob(MobError::UnsupportedForMode { .. })
        ),
        "expected typed unsupported-mode rejection, got {error}"
    );

    // Retire the roster through the ordinary reconcile path before terminal
    // shutdown: an autonomous kickoff caught mid-attach cannot be interrupted
    // by the machine Shutdown path ("Runtime not ready: attached"), which
    // would leave the stock supervisor route occupied for the next test.
    let _ = bundle.runtime.reconcile(Vec::new()).await;
    let _ = bundle.runtime.shutdown().await;
}

//! Released MobKit console composition with synthetic auth/model and optional PairedHost.
//! No Connect runtime, approval owner, live account or provider is constructed.

use std::{
    collections::{BTreeMap, BTreeSet},
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::State,
    http::{Request, StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::{Stream, StreamExt};
use hmac::{Hmac, Mac};
use meerkat::{Config, LlmClient, LlmError, LlmEvent, LlmRequest};
use meerkat_connect::{
    PAIRED_HOST_PREPARATION_TOOLS, PairedHostPreparationTools, RECIPE_TOOLS, RecipeTools,
    console::{ConsoleResearchHook, ConsoleSetupCustomizer},
    paired_host::{PairedHostClient, PairingCode},
    paired_host_tool::{
        PAIRED_HOST_FRONTEND_INVENTORY, PAIRED_HOST_FRONTEND_PREPARE, PAIRED_HOST_FRONTEND_SUBMIT,
        PAIRED_HOST_SECRET_CANCEL, PAIRED_HOST_SECRET_CREATE, PAIRED_HOST_SECRET_STATUS,
        TOOLKIT_FRONTEND_CATALOG,
    },
};
use meerkat_core::{
    AgentError, AgentEvent, AgentToolDispatcher, Message, Provider, ToolCategoryOverride,
    ToolError, ToolResult,
    comms::EventStream,
    event::AgentErrorReport,
    service::{CreateSessionRequest, SessionError, SessionHistoryQuery},
    types::ToolCallView,
};
use meerkat_mobkit::{
    AccessControlConfig, AccessController, AccessRule, AuthPolicy, AuthProvider, BigQueryNaming,
    ConsolePolicy, DiscoverySpec, MetaConfig, RuntimeDecisionInputs, RuntimeDecisionState,
    RuntimeOpsPolicy, SessionHook, TrustedOidcRuntimeConfig, UnifiedRuntime, UnifiedRuntimeBuilder,
    build_runtime_decision_state,
    identity_first::{
        AgentBuildContext, AgentBuildDraft, AgentCustomizer, AgentIdentity, CustomizerError,
        DurableAgentSpec, LocalExternalToolOverlay, MutableRosterProvider,
    },
};
use serde_json::{Value, json};
use sha2::Sha256;
use tokio::{
    sync::{Notify, oneshot},
    time::timeout,
};
use toolkit_package::{
    CatalogEntry, PackageIdentity, bundled_adopted_catalog, bundled_recipe_catalog,
};
use tower::ServiceExt;

const BOUND: Duration = Duration::from_secs(45);
const SETUP: &str = "setup:operator";
const OTHER: &str = "other:operator";
const OPERATOR: &str = "operator@example.test";
const OUTSIDER: &str = "outsider@example.test";
const DEVICE: &str = "private-fixture-console-device";
const PAIR: &str = "private-fixture-console-pair";
const CODE: &str = "421735";
const SIGNING_KEY: &[u8] = b"synthetic-console-test-key-not-a-real-credential";
const FIRST: &str = "Please inspect and prepare the Telegram frontend for this paired host.";
const FOLLOWUP: &str = "Have you installed it, or only prepared the candidate?";
const OTHER_PROMPT: &str = "Check whether this other member can borrow the setup member's tools.";
const BOOTSTRAP: &str = "Acknowledge this fixture member's startup without calling any tools.";
const BOOTSTRAP_ANSWER: &str = "Fixture member startup acknowledged.";
const PREPARED: &str = "The candidate is prepared. Nothing has been installed or activated.";
const FOLLOWUP_ANSWER: &str = "Only preparation is available in this console conversation.";
const RECIPE_FIRST: &str = "Investigate Gmail in the bundled recipes. Read its exact adopted entry; do not install or authenticate anything.";
const RECIPE_FOLLOWUP: &str =
    "Which recipe did you just inspect, and have you installed or authenticated it?";
const RECIPE_ANSWER: &str =
    "I inspected the exact adopted Gmail recipe. This is public metadata only.";
const RECIPE_FOLLOWUP_ANSWER: &str = "The adopted Gmail recipe was inspected in this conversation. No integration was installed or authenticated.";
const RECIPE_INSTRUCTIONS: &str = "Investigate the person's complete request using bundled recipe evidence. Preserve exact identity, digest, snapshot and cursor. Metadata is not installation, account authorization or runtime readiness. Frontend roles still require a compatible persistent host.";
const RECIPE_PAGE_SIZE: usize = 20;

fn inventory() -> Value {
    json!({"current":{"activation_id":"210","policy_digest":"sha256:current-fixture"},
        "extensions":[{"extension_id":"telegram","enabled":true,"wheel_present":true,
            "channels":[{"channel_id":"telegram:primary","ingress":["polling","webhook"],
                "secrets":[{"name":"telegram_bot_token","source":"vault","present":false,
                    "generation":null,"binding_status":null}],
                "approved_groups":{"declared":["-1001234567890"],"discovery":"unavailable"},
                "senders":{"identities":["identity:parent-1"],"informational":true}}]}],
        "routing":"sender_identity"})
}

fn candidate() -> Value {
    json!({"extension_id":"telegram","channel_id":"telegram:primary","ingress":"polling",
        "participants":{"approved_groups":["-1001234567890"]}})
}

fn prepared() -> Value {
    json!({"valid":true,"problems":[],
        "missing_prerequisites":[{"kind":"secret","name":"telegram_bot_token"}],
        "current":{"activation_id":"210","policy_digest":"sha256:current-fixture"},
        "candidate":{"channel_row_digest":"sha256:row-fixture",
            "policy_digest":"hc-policy-content-v1:sha256:policy-fixture","compile_depth":"policy_bundle"},
        "compiled_is_not_ready":true})
}

#[tokio::test]
async fn public_console_customizer_preserves_host_draft_and_refuses_existing_overlay() {
    let selected = AgentIdentity::parse(SETUP).unwrap();
    let customizer = ConsoleSetupCustomizer::recipes(selected.clone());
    let context = AgentBuildContext {
        identity: selected,
        active_peers: vec![],
        managed_edges: vec![],
        runtime_services: Default::default(),
    };
    let spec: DurableAgentSpec = serde_json::from_value(json!({
        "identity":SETUP,"profile":"setup","display_name":"Setup",
        "labels":{"host":"retained"}
    }))
    .unwrap();
    let draft = AgentBuildDraft {
        model: Some("host-selected-model".into()),
        system_prompt: Some("Host system instructions".into()),
        additional_instructions: vec!["Retain host instructions".into()],
        labels: BTreeMap::from([("host".into(), "retained".into())]),
        app_context: Some(json!({"host":"retained"})),
        external_tools: vec![
            serde_json::from_value(json!({
                "name":"host-declared-tool","description":"Host declaration",
                "input_schema":{"type":"object"}
            }))
            .unwrap(),
        ],
        local_external_tools: LocalExternalToolOverlay::empty(),
        provider_params: None,
        compaction_curator: Default::default(),
    };
    for mismatch_context in [true, false] {
        let mut other_context = context.clone();
        let mut other_spec = spec.clone();
        if mismatch_context {
            other_context.identity = AgentIdentity::parse(OTHER).unwrap();
        } else {
            other_spec.identity = AgentIdentity::parse(OTHER).unwrap();
        }
        let mut actual = draft.clone();
        customizer
            .customize_build(&other_context, &other_spec, &mut actual)
            .await
            .unwrap();
        assert_eq!(actual, draft, "both native identity carriers must match");
    }
    let existing: Arc<dyn AgentToolDispatcher> = Arc::new(RecipeTools::new());
    let mut occupied = draft.clone();
    occupied.local_external_tools = LocalExternalToolOverlay::new(existing.clone());
    let before = occupied.clone();
    assert!(matches!(
        customizer
            .customize_build(&context, &spec, &mut occupied)
            .await,
        Err(CustomizerError::BuildFailed(_))
    ));
    assert_eq!(occupied, before);
    assert!(Arc::ptr_eq(
        &existing,
        &occupied.local_external_tools.dispatcher().unwrap()
    ));

    let mut actual = draft.clone();
    customizer
        .customize_build(&context, &spec, &mut actual)
        .await
        .unwrap();
    let dispatcher = actual.local_external_tools.dispatcher().unwrap();
    assert_eq!(
        dispatcher
            .tools()
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        RECIPE_TOOLS
    );
    let mut expected = draft;
    expected.local_external_tools = actual.local_external_tools.clone();
    expected
        .additional_instructions
        .push(RECIPE_INSTRUCTIONS.into());
    assert_eq!(
        actual, expected,
        "only the leaf and canonical instructions are added"
    );
}

fn research_hook_request(binding: Option<meerkat_core::MobMemberBinding>) -> CreateSessionRequest {
    CreateSessionRequest {
        model: RESEARCH_MODEL.into(),
        prompt: "Public investigation only".into(),
        injected_context: vec![],
        system_prompt: Default::default(),
        max_tokens: Some(512),
        event_tx: None,
        initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
        deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
        build: Some(meerkat_core::service::SessionBuildOptions {
            mob_member_binding: binding,
            realm_id: Some(meerkat_core::mob_realm_id(RESEARCH_MOB).unwrap()),
            override_builtins: ToolCategoryOverride::Disable,
            override_web_search: ToolCategoryOverride::Disable,
            override_shell: ToolCategoryOverride::Disable,
            ..Default::default()
        }),
        labels: Some(BTreeMap::from([("display_name".into(), SETUP.into())])),
    }
}

#[tokio::test]
async fn public_console_research_hook_preserves_identity_and_host_policy_intent() {
    use meerkat_core::{ToolExecutionPolicy, ToolFilter, ToolMutationClass, ops::ToolAccessPolicy};
    let selected = meerkat_core::MobMemberBinding {
        mob_id: RESEARCH_MOB.into(),
        role: "setup".into(),
        member: meerkat_mobkit::member_comms_id::roster_member_id_for_identity(SETUP).to_string(),
    };
    let auth = meerkat_core::AuthBindingRef {
        realm: meerkat_core::mob_realm_id(RESEARCH_MOB).unwrap(),
        binding: meerkat_core::BindingId::parse("member").unwrap(),
        profile: None,
        origin: meerkat_core::BindingOrigin::Configured,
    };
    let hook = ConsoleResearchHook::new(selected.clone(), auth.clone());
    let mut wrong_mob = selected.clone();
    wrong_mob.mob_id = "another-mob".into();
    let mut wrong_role = selected.clone();
    wrong_role.role = "another-role".into();
    let mut wrong_member = selected.clone();
    wrong_member.member = "another-member".into();
    for binding in [None, Some(wrong_mob), Some(wrong_role), Some(wrong_member)] {
        let mut request = research_hook_request(binding.clone());
        hook.before_create(&mut request).await.unwrap();
        let build = request.build.unwrap();
        assert_eq!(build.mob_member_binding, binding);
        assert!(
            build.auth_binding.is_none()
                && build.initial_tool_filter.is_none()
                && build.tool_access_policy.is_none()
        );
        assert_eq!(build.override_builtins, ToolCategoryOverride::Disable);
        assert_eq!(build.override_web_search, ToolCategoryOverride::Disable);
    }
    for conflict in ["auth", "filter", "policy"] {
        let mut request = research_hook_request(Some(selected.clone()));
        let build = request.build.as_mut().unwrap();
        match conflict {
            "auth" => {
                let mut other_auth = auth.clone();
                other_auth.binding = meerkat_core::BindingId::parse("other-account").unwrap();
                build.auth_binding = Some(other_auth);
            }
            "filter" => {
                build.initial_tool_filter =
                    Some(ToolFilter::Allow(["host-tool"].into_iter().collect()))
            }
            _ => build.tool_access_policy = Some(ToolAccessPolicy::Inherit),
        }
        let before = build.clone();
        assert!(matches!(
            hook.before_create(&mut request).await,
            Err(SessionError::Unsupported(_))
        ));
        let build = request.build.unwrap();
        assert_eq!(build.auth_binding, before.auth_binding);
        assert_eq!(build.initial_tool_filter, before.initial_tool_filter);
        assert_eq!(build.tool_access_policy, before.tool_access_policy);
        assert_eq!(build.override_builtins, before.override_builtins);
        assert_eq!(build.override_web_search, before.override_web_search);
    }
    let mut request = research_hook_request(Some(selected.clone()));
    request.build.as_mut().unwrap().auth_binding = Some(auth.clone());
    request.build.as_mut().unwrap().tool_access_policy = Some(
        ToolAccessPolicy::ReadOnly
            .conjoin(ToolAccessPolicy::DenyList(
                ["web_search"].into_iter().collect(),
            ))
            .unwrap(),
    );
    hook.before_create(&mut request).await.unwrap();
    assert_eq!(request.model, RESEARCH_MODEL);
    assert_eq!(request.max_tokens, Some(512));
    let build = request.build.unwrap();
    assert_eq!(build.mob_member_binding, Some(selected));
    assert_eq!(
        build.realm_id,
        Some(meerkat_core::mob_realm_id(RESEARCH_MOB).unwrap())
    );
    assert_eq!(build.auth_binding, Some(auth));
    assert_eq!(build.override_shell, ToolCategoryOverride::Disable);
    assert_eq!(
        build.initial_tool_filter,
        Some(ToolFilter::Allow(
            RESEARCH_VISIBLE_TOOLS.into_iter().collect()
        ))
    );
    let policy = ToolExecutionPolicy::resolve(build.tool_access_policy.unwrap()).unwrap();
    assert!(policy.permits_call(RECIPE_TOOLS[0], ToolMutationClass::ReadOnly));
    assert!(!policy.permits_call(RECIPE_TOOLS[0], ToolMutationClass::Unknown));
    for name in [
        "web_search",
        "task_list",
        "send_message",
        PAIRED_HOST_FRONTEND_PREPARE,
    ] {
        assert!(
            !policy.permits_call(name, ToolMutationClass::ReadOnly),
            "host and setup ceilings must both hold for {name}"
        );
    }
}

#[derive(Clone)]
struct HostState {
    origin: String,
    requests: Arc<Mutex<Vec<(String, String, Value)>>>,
}

async fn host_request(State(state): State<HostState>, request: Request<Body>) -> Response {
    let method = request.method().as_str().to_owned();
    let path = request.uri().path().to_owned();
    let headers = request.headers().clone();
    assert!(!headers.contains_key("x-paired-host-dev-token"));
    assert!(!headers.contains_key("x-paired-host-harness-token"));
    assert!(!headers.contains_key(header::AUTHORIZATION));
    if method == "POST" {
        assert_eq!(headers[header::ORIGIN], state.origin);
    }
    if path.starts_with("/console/extensions/") {
        assert!(headers[header::COOKIE].to_str().unwrap().contains(DEVICE));
    }
    let body = to_bytes(request.into_body(), 16 * 1024).await.unwrap();
    let body: Value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap()
    };
    state
        .requests
        .lock()
        .unwrap()
        .push((method.clone(), path.clone(), body.clone()));
    let (value, cookies) = match (method.as_str(), path.as_str()) {
        ("POST", "/console/pair/request") => (
            json!({"ok":true}),
            vec![format!(
                "hc_console_pair={PAIR}; Path=/console/pair; HttpOnly; SameSite=Strict"
            )],
        ),
        ("POST", "/console/pair/confirm") => {
            assert!(headers[header::COOKIE].to_str().unwrap().contains(PAIR));
            (
                json!({"ok":true}),
                vec![
                    format!("hc_console_device={DEVICE}; Path=/; HttpOnly; SameSite=Lax"),
                    "hc_console_pair=; Max-Age=0; Path=/console/pair".into(),
                ],
            )
        }
        ("GET", "/console/extensions/frontend/inventory") => (inventory(), vec![]),
        ("POST", "/console/extensions/frontend/prepare") => {
            assert_eq!(body, candidate());
            (prepared(), vec![])
        }
        _ => panic!("unexpected PairedHost effect or route: {method} {path}"),
    };
    let mut response = axum::Json(value).into_response();
    if path.starts_with("/console/extensions/") {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    }
    for cookie in cookies {
        response
            .headers_mut()
            .append(header::SET_COOKIE, cookie.parse().unwrap());
    }
    response
}

struct Host {
    state: HostState,
    stop: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl Host {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let state = HostState {
            origin: format!("http://{}", listener.local_addr().unwrap()),
            requests: Arc::new(Mutex::new(Vec::new())),
        };
        let app = Router::new()
            .fallback(host_request)
            .with_state(state.clone());
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        });
        Self {
            state,
            stop: Some(stop),
            task: Some(task),
        }
    }
    async fn client(&self) -> PairedHostClient {
        let mut client = PairedHostClient::new(&self.state.origin).unwrap();
        client
            .request_pairing("Console preparation fixture")
            .await
            .unwrap();
        client
            .confirm_pairing(PairingCode::new(CODE.into()).unwrap())
            .await
            .unwrap();
        client
    }
    fn count(&self) -> usize {
        self.state.requests.lock().unwrap().len()
    }
    async fn finish(mut self) {
        self.stop.take().unwrap().send(()).unwrap();
        timeout(BOUND, self.task.take().unwrap())
            .await
            .unwrap()
            .unwrap();
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

#[derive(Default)]
struct Probe {
    calls: AtomicUsize,
    observed: Mutex<BTreeMap<String, ToolResult>>,
}
fn no_private(value: &str) {
    for private in [DEVICE, PAIR, CODE, "/secrets/intents/"] {
        assert!(
            !value.contains(private),
            "private PairedHost material entered ordinary output"
        );
    }
}
fn safe_tool_error_code(result: &ToolResult) -> &'static str {
    let text = result.text_content();
    if text.len() > 4096 {
        return "diagnostic_size_limit";
    }
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return "non_json_result";
    };
    // Emit only fixed native classifications, never the response body or message.
    match value.get("error").and_then(Value::as_str) {
        Some("tool_not_found") => "tool_not_found",
        Some("tool_unavailable") => "tool_unavailable",
        Some("invalid_arguments") => "invalid_arguments",
        Some("execution_failed") => "execution_failed",
        Some("timeout") => "timeout",
        Some("inactivity_timeout") => "inactivity_timeout",
        Some("access_denied") => "access_denied",
        Some("policy_denied") => "policy_denied",
        Some("policy_indeterminate") => "policy_indeterminate",
        Some("tool_error") => "tool_error",
        Some("callback_pending") => "callback_pending",
        Some(_) => "unrecognized_error_code",
        None => "no_error_code",
    }
}
fn response_events(
    request: &LlmRequest,
    call: Option<(&str, &str, Value)>,
    text: &str,
) -> Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send>> {
    let tool_call = call.is_some();
    let event = match call {
        Some((id, name, args)) => LlmEvent::ToolCallComplete {
            id: id.into(),
            name: name.into(),
            args,
            meta: None,
        },
        None => LlmEvent::TextDelta {
            delta: text.into(),
            meta: None,
        },
    };
    Box::pin(futures_util::stream::iter([
        Ok(event),
        Ok(LlmEvent::UsageUpdate {
            usage: meerkat_core::TurnUsage::host_declared(
                Provider::OpenAI,
                &request.model,
                meerkat_core::Usage::default(),
            ),
        }),
        Ok(LlmEvent::Done {
            outcome: meerkat::LlmDoneOutcome::success(if tool_call {
                meerkat::StopReason::ToolUse
            } else {
                meerkat::StopReason::EndTurn
            }),
        }),
    ]))
}
#[async_trait]
impl LlmClient for Probe {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        meerkat_client::TestClient::for_provider(Provider::OpenAI).project_replay_messages(messages)
    }
    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        no_private(&serde_json::to_string(&request.messages).unwrap());
        no_private(&serde_json::to_string(&request.tools).unwrap());
        let offered: BTreeSet<_> = request
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        for name in &offered {
            assert!(
                PAIRED_HOST_PREPARATION_TOOLS.contains(name)
                    || [
                        "send_message",
                        "reply_to_peer",
                        "send_request",
                        "send_response",
                        "peers"
                    ]
                    .contains(name),
                "unexpected helper, provider or tool family: {name}"
            );
        }
        for excluded in [
            PAIRED_HOST_FRONTEND_SUBMIT,
            PAIRED_HOST_SECRET_CREATE,
            PAIRED_HOST_SECRET_STATUS,
            PAIRED_HOST_SECRET_CANCEL,
            "web_search",
            "shell",
            "mob_spawn_member",
            "spawn_member",
            "mob_create",
            "mob_fork",
            "mob_run_flow",
        ] {
            assert!(
                !offered.contains(excluded),
                "excluded tool is advertised: {excluded}"
            );
        }
        let mut results = BTreeMap::new();
        for message in &request.messages {
            if let Message::ToolResults { results: items, .. } = message {
                for result in items {
                    results.insert(result.tool_use_id.as_str(), result);
                    self.observed
                        .lock()
                        .unwrap()
                        .insert(result.tool_use_id.clone(), result.clone());
                }
            }
        }
        let literal = request
            .messages
            .iter()
            .rev()
            .find_map(|message| match message {
                Message::User(message) => Some(message.text_content()),
                _ => None,
            })
            .unwrap();
        if literal == BOOTSTRAP {
            return response_events(request, None, BOOTSTRAP_ANSWER);
        }
        if literal.contains(OTHER_PROMPT) {
            for name in PAIRED_HOST_PREPARATION_TOOLS {
                assert!(!offered.contains(name));
            }
            assert!(
                !results.contains_key("wrong-member"),
                "unadvertised tool call must terminate natively"
            );
            return response_events(
                request,
                Some(("wrong-member", PAIRED_HOST_FRONTEND_INVENTORY, json!({}))),
                "",
            );
        }
        for name in PAIRED_HOST_PREPARATION_TOOLS {
            assert!(offered.contains(name), "missing {name}");
        }
        if literal.contains(FOLLOWUP) {
            assert!(
                serde_json::to_string(&request.messages)
                    .unwrap()
                    .contains(FIRST)
            );
            return response_events(request, None, FOLLOWUP_ANSWER);
        }
        assert!(literal.contains(FIRST));
        let sequence = [
            ("catalog", TOOLKIT_FRONTEND_CATALOG, json!({})),
            ("inventory", PAIRED_HOST_FRONTEND_INVENTORY, json!({})),
            ("prepare", PAIRED_HOST_FRONTEND_PREPARE, candidate()),
            (
                "forged-send",
                "send_message",
                json!({"peer_id":"00000000-0000-4000-8000-000000000001",
                "body":"must not be delivered","handling_mode":"queue"}),
            ),
        ];
        for (id, name, args) in sequence {
            if let Some(result) = results.get(id) {
                assert_eq!(
                    result.is_error,
                    id.starts_with("forged-"),
                    "unexpected result for {id}; native_error_code={}",
                    safe_tool_error_code(result),
                );
                if id == "forged-send" {
                    let value: Value = serde_json::from_str(&result.text_content()).unwrap();
                    assert_eq!(
                        value["error"], "access_denied",
                        "native policy, not argument/peer validation, must refuse"
                    );
                }
                if id == "inventory" {
                    assert_eq!(
                        serde_json::from_str::<Value>(&result.text_content()).unwrap(),
                        inventory()
                    );
                }
                if id == "prepare" {
                    let value: Value = serde_json::from_str(&result.text_content()).unwrap();
                    assert_eq!(value["valid"], true);
                    assert_eq!(value["compiled_is_not_ready"], true);
                }
            } else {
                return response_events(request, Some((id, name, args)), "");
            }
        }
        response_events(request, None, PREPARED)
    }
    fn provider(&self) -> Provider {
        Provider::OpenAI
    }
    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

struct RecipeProbe {
    calls: AtomicUsize,
    observed: Mutex<BTreeMap<String, ToolResult>>,
    entries: BTreeMap<PackageIdentity, CatalogEntry>,
    gmail: CatalogEntry,
}

impl RecipeProbe {
    fn new() -> Self {
        let adopted = bundled_adopted_catalog().unwrap();
        let gmail = adopted
            .entries()
            .find(|entry| entry.package.identity.name.as_str() == "gmail")
            .unwrap()
            .clone();
        Self {
            calls: AtomicUsize::new(0),
            observed: Mutex::new(BTreeMap::new()),
            entries: bundled_recipe_catalog()
                .unwrap()
                .entries()
                .map(|entry| (entry.package.identity.clone(), entry.clone()))
                .collect(),
            gmail,
        }
    }
}

#[async_trait]
impl LlmClient for RecipeProbe {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        meerkat_client::TestClient::for_provider(Provider::OpenAI).project_replay_messages(messages)
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        no_private(&serde_json::to_string(&request.messages).unwrap());
        let offered: BTreeSet<_> = request
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        for name in &offered {
            assert!(
                RECIPE_TOOLS.contains(name)
                    || [
                        "send_message",
                        "reply_to_peer",
                        "send_request",
                        "send_response",
                        "peers",
                    ]
                    .contains(name),
                "unexpected recipe-member tool: {name}"
            );
        }
        let results: BTreeMap<_, _> = request
            .messages
            .iter()
            .filter_map(|message| {
                if let Message::ToolResults { results, .. } = message {
                    Some(results)
                } else {
                    None
                }
            })
            .flatten()
            .map(|result| {
                self.observed
                    .lock()
                    .unwrap()
                    .insert(result.tool_use_id.clone(), result.clone());
                (result.tool_use_id.as_str(), result)
            })
            .collect();
        let literal = request
            .messages
            .iter()
            .rev()
            .find_map(|message| match message {
                Message::User(message) => Some(message.text_content()),
                _ => None,
            })
            .unwrap();
        if literal == BOOTSTRAP {
            return response_events(request, None, BOOTSTRAP_ANSWER);
        }
        if literal.contains(OTHER_PROMPT) {
            assert!(RECIPE_TOOLS.iter().all(|name| !offered.contains(name)));
            assert!(
                !results.contains_key("wrong-member"),
                "native unknown tool must terminate the turn"
            );
            return response_events(
                request,
                Some(("wrong-member", RECIPE_TOOLS[0], json!({"limit":1}))),
                "",
            );
        }
        assert!(RECIPE_TOOLS.iter().all(|name| offered.contains(name)));
        if literal.contains(RECIPE_FOLLOWUP) {
            let history = serde_json::to_string(&request.messages).unwrap();
            assert!(history.contains(RECIPE_FIRST) && history.contains(RECIPE_ANSWER));
            assert_eq!(
                serde_json::from_str::<Value>(&results["gmail"].text_content()).unwrap(),
                serde_json::to_value(&self.gmail).unwrap()
            );
            return response_events(request, None, RECIPE_FOLLOWUP_ANSWER);
        }
        assert!(literal.contains(RECIPE_FIRST));
        let mut args = json!({"limit":RECIPE_PAGE_SIZE});
        let mut snapshot = None;
        let mut seen = BTreeSet::new();
        let mut selected = None;
        let mut paged = false;
        // Follow only returned cursors. Stop after finding Gmail and observing
        // at least two pages; exhaustive inventory traversal is covered elsewhere.
        for index in 0..self.entries.len().div_ceil(RECIPE_PAGE_SIZE) {
            let id = format!("recipes-{index}");
            let Some(result) = results.get(id.as_str()) else {
                return response_events(request, Some((&id, RECIPE_TOOLS[0], args)), "");
            };
            assert!(
                !result.is_error,
                "catalog read failed: {}",
                safe_tool_error_code(result)
            );
            let page: Value = serde_json::from_str(&result.text_content()).unwrap();
            if let Some(expected) = &snapshot {
                assert_eq!(&page["snapshot"], expected);
            } else {
                snapshot = Some(page["snapshot"].clone());
            }
            let entries = page["entries"].as_array().unwrap();
            assert!(!entries.is_empty() && entries.len() <= RECIPE_PAGE_SIZE);
            let mut previous = args
                .get("after")
                .map(|after| serde_json::from_value::<PackageIdentity>(after.clone()).unwrap());
            for summary in entries {
                let identity: PackageIdentity =
                    serde_json::from_value(summary["identity"].clone()).unwrap();
                if let Some(prior) = &previous {
                    assert!(&identity > prior);
                }
                previous = Some(identity.clone());
                assert!(seen.insert(identity.clone()), "cursor repeated an entry");
                let canonical = &self.entries[&identity];
                assert_eq!(
                    summary["digest"],
                    serde_json::to_value(&canonical.digest).unwrap()
                );
                assert_eq!(
                    summary,
                    &json!({
                        "identity": canonical.package.identity,
                        "digest": canonical.digest,
                        "display_name": canonical.package.display_name,
                        "description": canonical.package.description,
                        "roles": canonical.package.known_exports.iter()
                            .map(|export| export.role).collect::<Vec<_>>(),
                    })
                );
                if identity == self.gmail.package.identity {
                    selected = Some(summary.clone());
                }
            }
            if index >= 1 && selected.is_some() {
                paged = true;
                break;
            }
            assert!(
                !page["next_after"].is_null(),
                "Gmail must be reachable by returned cursor"
            );
            assert_eq!(page["next_after"], entries.last().unwrap()["identity"]);
            args = json!({"limit":RECIPE_PAGE_SIZE,"snapshot":page["snapshot"],"after":page["next_after"]});
        }
        assert!(paged);
        let summary = selected.unwrap();
        if let Some(result) = results.get("gmail") {
            assert!(!result.is_error);
            let full: Value = serde_json::from_str(&result.text_content()).unwrap();
            assert_eq!(
                full["origin"],
                serde_json::to_value(&self.gmail.origin).unwrap()
            );
            assert_eq!(
                full["package"]["assessments"],
                serde_json::to_value(&self.gmail.package.assessments).unwrap()
            );
            assert_eq!(full, serde_json::to_value(&self.gmail).unwrap());
        } else {
            return response_events(
                request,
                Some((
                    "gmail",
                    RECIPE_TOOLS[1],
                    json!({"identity":summary["identity"],"digest":summary["digest"]}),
                )),
                "",
            );
        }
        if let Some(result) = results.get("forged-send") {
            assert!(result.is_error);
            assert_eq!(safe_tool_error_code(result), "access_denied");
        } else {
            return response_events(
                request,
                Some((
                    "forged-send",
                    "send_message",
                    json!({"peer_id":"00000000-0000-4000-8000-000000000001",
                    "body":"must not be delivered","handling_mode":"queue"}),
                )),
                "",
            );
        }
        response_events(request, None, RECIPE_ANSWER)
    }

    fn provider(&self) -> Provider {
        Provider::OpenAI
    }
    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

fn decisions() -> RuntimeDecisionState {
    build_runtime_decision_state(RuntimeDecisionInputs {
        bigquery: BigQueryNaming {
            dataset: "console_preparation".into(),
            table: "rows".into(),
        },
        trusted_meta_toml: String::new(),
        auth: AuthPolicy {
            default_provider: AuthProvider::GoogleOAuth,
            email_allowlist: vec![OPERATOR.into(), OUTSIDER.into()],
        },
        trusted_oidc: TrustedOidcRuntimeConfig {
            discovery_json: json!({"issuer":"https://trusted.mobkit.localhost",
                "jwks_uri":"https://trusted.mobkit.localhost/.well-known/jwks.json"})
            .to_string(),
            jwks_json: json!({"keys":[{"kid":"fixture","kty":"oct","alg":"HS256",
                "k":URL_SAFE_NO_PAD.encode(SIGNING_KEY)}]})
            .to_string(),
            audience: "meerkat-console".into(),
            require_verified_email: false,
        },
        console: ConsolePolicy {
            require_app_auth: true,
            ..Default::default()
        },
        ops: RuntimeOpsPolicy::default(),
        release_metadata_json: json!({"targets":["crates.io","npm","pypi","github-releases"],
            "support_matrix":"same-as-meerkat"})
        .to_string(),
    })
    .unwrap()
}
fn bearer(email: &str) -> String {
    let header = URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&json!({"alg":"HS256","kid":"fixture","typ":"JWT"})).unwrap());
    let body = URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&json!({"iss":"https://trusted.mobkit.localhost",
        "aud":"meerkat-console","sub":email,"email":email,"email_verified":true,"provider":"google_oauth",
        "exp":SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()+600}))
        .unwrap(),
    );
    let unsigned = format!("{header}.{body}");
    let mut mac = Hmac::<Sha256>::new_from_slice(SIGNING_KEY).unwrap();
    mac.update(unsigned.as_bytes());
    format!(
        "Bearer {unsigned}.{}",
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    )
}
async fn rpc(
    app: &Router,
    bearer: Option<&str>,
    method: &str,
    params: Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri("/console/rpc")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(bearer) = bearer {
        request = request.header(header::AUTHORIZATION, bearer);
    }
    let response = timeout(
        BOUND,
        app.clone().oneshot(
            request
                .body(Body::from(
                    serde_json::to_vec(
                        &json!({"jsonrpc":"2.0","id":"fixture","method":method,"params":params}),
                    )
                    .unwrap(),
                ))
                .unwrap(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    no_private(std::str::from_utf8(&bytes).unwrap());
    (status, serde_json::from_slice(&bytes).unwrap())
}
fn send(identity: &str, text: &str, key: &str) -> Value {
    json!({"identity":identity,"content":text,"origin":"console","origin_kind":"operator",
        "idempotency_key":key})
}
async fn completed(events: &mut EventStream, expected: &str) {
    timeout(BOUND, async {
        let mut run = None;
        while let Some(event) = events.next().await {
            match event.payload {
                AgentEvent::RunStarted { identity, .. } => run = identity.run_id,
                AgentEvent::RunCompleted {
                    identity,
                    result,
                    extraction_required,
                    ..
                } => {
                    assert!(run.is_some());
                    assert_eq!(identity.run_id, run);
                    assert!(!extraction_required);
                    assert_eq!(result, expected);
                    return;
                }
                AgentEvent::RunFailed { .. } => panic!("native setup conversation failed"),
                _ => {}
            }
        }
        panic!("native event stream ended before completion");
    })
    .await
    .unwrap();
}

async fn unknown_tool_failure(events: &mut EventStream, name: &str) {
    tool_failure(events, ToolError::not_found(name)).await;
}

async fn tool_failure(events: &mut EventStream, expected: ToolError) {
    timeout(BOUND, async {
        let mut run = None;
        while let Some(event) = events.next().await {
            match event.payload {
                AgentEvent::RunStarted { identity, .. } => run = identity.run_id,
                AgentEvent::RunFailed {
                    identity,
                    error_report,
                    ..
                } => {
                    assert!(run.is_some());
                    assert_eq!(identity.run_id, run);
                    assert_eq!(
                        error_report,
                        AgentErrorReport::from_agent_error(&AgentError::tool(expected))
                    );
                    return;
                }
                AgentEvent::RunCompleted { .. } => {
                    panic!("refused tool call unexpectedly completed")
                }
                _ => {}
            }
        }
        panic!("native stream ended before its exact tool refusal");
    })
    .await
    .unwrap();
}

async fn effect_names_are_closed(tools: &PairedHostPreparationTools) {
    let args = serde_json::value::RawValue::from_string("{}".into()).unwrap();
    for name in [
        PAIRED_HOST_FRONTEND_SUBMIT,
        PAIRED_HOST_SECRET_CREATE,
        PAIRED_HOST_SECRET_STATUS,
        PAIRED_HOST_SECRET_CANCEL,
        "web_search",
        "send_message",
    ] {
        assert!(
            matches!(
                tools
                    .dispatcher()
                    .dispatch(ToolCallView {
                        id: "forged",
                        name,
                        args: &args
                    })
                    .await,
                Err(ToolError::NotFound { .. })
            ),
            "preparation dispatcher accepted an excluded name: {name}"
        );
    }
}

async fn bootstrap_settled(
    service: &dyn meerkat_mob::MobSessionService,
    id: &meerkat_core::SessionId,
) {
    timeout(BOUND, async {
        loop {
            let history = service
                .read_history(id, SessionHistoryQuery::default())
                .await
                .unwrap();
            let view = service.read(id).await.unwrap();
            if !view.state.is_active
                && serde_json::to_string(&history)
                    .unwrap()
                    .contains(BOOTSTRAP_ANSWER)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

async fn setup_runtime(
    mob_id: meerkat_mob::MobId,
    directory: &std::path::Path,
    probe: Arc<dyn LlmClient>,
    customizer: ConsoleSetupCustomizer,
) -> UnifiedRuntime {
    let mut config = Config::default();
    config.retry.max_retries = 0;
    config.model_fallback.enabled = Some(false);
    let builder = setup_builder(mob_id, directory, customizer, "gpt-5.4")
        .await
        .default_llm_client(probe)
        .meerkat_config(config);
    timeout(BOUND, Box::pin(builder.build()))
        .await
        .unwrap()
        .unwrap()
}

async fn setup_builder(
    mob_id: meerkat_mob::MobId,
    directory: &std::path::Path,
    customizer: ConsoleSetupCustomizer,
    model: &str,
) -> UnifiedRuntimeBuilder {
    let mut definition = meerkat_mob::MobDefinition::from_toml(&format!(
        r#"
[mob]
id="connect-console-preparation"
[profiles.setup]
model="{model}"
external_addressable=true
[profiles.setup.tools]
builtins=false
shell=false
comms=true
memory=false
workgraph=false
mob=false
schedule=false
image_generation=false
read_only=true
deny=["send_message","reply_to_peer","send_request","send_response"]
"#
    ))
    .unwrap();
    // Isolated state directories do not separate concurrent participant names.
    definition.id = mob_id;
    let roster = [SETUP, OTHER]
        .into_iter()
        .map(|identity| {
            serde_json::from_value::<DurableAgentSpec>(
                json!({"identity":identity,"profile":"setup","addressability":"addressable",
            "display_name":null,"context":null,"initial_message":BOOTSTRAP}),
            )
            .unwrap()
        })
        .collect();
    let access = AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec!["fixture-admin@example.test".into()],
        groups: BTreeMap::new(),
        approvers: BTreeMap::new(),
        rules: vec![AccessRule {
            id: "selected-host-operator".into(),
            subjects: vec![OPERATOR.into()],
            actions: vec!["agent.view".into(), "agent.send".into()],
            agents: vec![SETUP.into(), OTHER.into()],
            ..Default::default()
        }],
    })
    .unwrap();
    UnifiedRuntime::builder()
        .definition(definition)
        .persistent_state(directory)
        .continuity_from_state_dir(directory)
        .await
        .unwrap()
        .roster_provider(Arc::new(MutableRosterProvider::new(roster)))
        .agent_customizer(Arc::new(customizer))
        .access_controller(access)
        .module_config(MetaConfig {
            modules: vec![],
            discovery: DiscoverySpec {
                namespace: "connect-fixture".into(),
                modules: vec![],
            },
            pre_spawn: vec![],
        })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stock_console_prepares_for_one_admitted_member_without_effect_authority() {
    let host = Host::start().await;
    let preparation = PairedHostPreparationTools::new(host.client().await).unwrap();
    effect_names_are_closed(&preparation).await;
    assert_eq!(host.count(), 2);
    let directory = tempfile::tempdir().unwrap();
    let probe = Arc::new(Probe::default());
    let runtime = setup_runtime(
        meerkat_mob::MobId::from("connect-console-preparation"),
        directory.path(),
        probe.clone(),
        ConsoleSetupCustomizer::paired_host_preparation(
            AgentIdentity::parse(SETUP).unwrap(),
            &preparation,
        ),
    )
    .await;
    let app = runtime.build_reference_app_router(decisions());
    let operator = bearer(OPERATOR);
    let outsider = bearer(OUTSIDER);
    let service = runtime.mob_runtime().session_service().unwrap();
    let setup_id = runtime
        .identity_runtime()
        .unwrap()
        .status(&AgentIdentity::parse(SETUP).unwrap())
        .await
        .unwrap()
        .session_id
        .unwrap();
    let other_id = runtime
        .identity_runtime()
        .unwrap()
        .status(&AgentIdentity::parse(OTHER).unwrap())
        .await
        .unwrap()
        .session_id
        .unwrap();
    assert_ne!(setup_id, other_id);
    // Autonomous members have a real native kickoff. Settle both known
    // startup turns before measuring whether denied viewers trigger work.
    bootstrap_settled(service.as_ref(), &setup_id).await;
    bootstrap_settled(service.as_ref(), &other_id).await;
    let bootstrap_calls = probe.calls.load(Ordering::SeqCst);
    assert_eq!(bootstrap_calls, 2);
    assert_eq!(host.count(), 2, "bootstrap must perform no PairedHost call");
    let mut setup_events =
        meerkat_mob::MobSessionService::subscribe_session_events(service.as_ref(), &setup_id)
            .await
            .unwrap();
    let mut other_events =
        meerkat_mob::MobSessionService::subscribe_session_events(service.as_ref(), &other_id)
            .await
            .unwrap();

    let (status, _) = rpc(
        &app,
        None,
        "mobkit/console/send",
        send(SETUP, FIRST, "unauthenticated"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (_, denied) = rpc(
        &app,
        Some(&outsider),
        "mobkit/console/send",
        send(SETUP, FIRST, "wrong-viewer"),
    )
    .await;
    assert_eq!(denied["error"]["code"], -32030, "{denied}");
    assert_eq!(denied["error"]["data"]["kind"], "access_denied");
    assert_eq!(probe.calls.load(Ordering::SeqCst), bootstrap_calls);
    assert_eq!(host.count(), 2);

    let (_, accepted) = rpc(
        &app,
        Some(&operator),
        "mobkit/console/send",
        send(SETUP, FIRST, "prepare-once"),
    )
    .await;
    assert!(accepted.get("error").is_none(), "{accepted}");
    assert_eq!(accepted["result"]["identity"], SETUP);
    completed(&mut setup_events, PREPARED).await;
    assert_eq!(
        host.count(),
        4,
        "pair twice, inventory once, preparation once; no effect or research"
    );
    let (_, accepted) = rpc(
        &app,
        Some(&operator),
        "mobkit/console/send",
        send(SETUP, FOLLOWUP, "followup-once"),
    )
    .await;
    assert!(accepted.get("error").is_none(), "{accepted}");
    completed(&mut setup_events, FOLLOWUP_ANSWER).await;
    assert_eq!(host.count(), 4);

    let (_, accepted) = rpc(
        &app,
        Some(&operator),
        "mobkit/console/send",
        send(OTHER, OTHER_PROMPT, "other-once"),
    )
    .await;
    assert!(accepted.get("error").is_none(), "{accepted}");
    unknown_tool_failure(&mut other_events, PAIRED_HOST_FRONTEND_INVENTORY).await;
    assert_eq!(
        host.count(),
        4,
        "another member sharing the profile cannot borrow the captured client"
    );
    effect_names_are_closed(&preparation).await;
    assert_eq!(
        host.count(),
        4,
        "excluded effects remain closed after preparation"
    );

    let history = service
        .read_history(&setup_id, SessionHistoryQuery::default())
        .await
        .unwrap();
    let serialized = serde_json::to_string(&history).unwrap();
    no_private(&serialized);
    assert!(serialized.contains(FIRST) && serialized.contains(FOLLOWUP));
    assert!(serialized.contains(PREPARED) && serialized.contains(FOLLOWUP_ANSWER));
    assert!(!serialized.contains(OTHER_PROMPT));
    {
        let observed = probe.observed.lock().unwrap();
        assert!(observed["forged-send"].is_error);
    }
    // The stock query reads its current projection and schedules backfill
    // asynchronously. Read that same admitted route until it projects native
    // history; never inject frames or force a private refresh path.
    let mut visible_observations = [false; 4];
    let visible = timeout(BOUND, async {
        loop {
            let (_, timeline) = rpc(
                &app,
                Some(&operator),
                "mobkit/console/query_timeline",
                json!({"identity":SETUP,"limit":200}),
            )
            .await;
            assert!(timeline.get("error").is_none(), "{timeline}");
            let visible = serde_json::to_string(&timeline["result"]).unwrap();
            visible_observations = [FIRST, FOLLOWUP, PREPARED, FOLLOWUP_ANSWER]
                .map(|expected| visible.contains(expected));
            if visible_observations.iter().all(|present| *present) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(
        visible.is_ok(),
        "ordinary console transcript did not expose both prompts and answers within the bound: {visible_observations:?}"
    );
    let (_, denied_history) = rpc(
        &app,
        Some(&outsider),
        "mobkit/console/query_timeline",
        json!({"identity":SETUP,"limit":200}),
    )
    .await;
    assert!(denied_history.get("error").is_none(), "{denied_history}");
    assert_eq!(denied_history["result"]["frames"], json!([]));
    let denied_history = serde_json::to_string(&denied_history).unwrap();
    assert!(!denied_history.contains(PREPARED) && !denied_history.contains(FIRST));
    let report = timeout(BOUND, Box::pin(runtime.shutdown())).await.unwrap();
    assert!(
        report.cleanup_completed(),
        "native runtime did not settle: {report:?}"
    );
    drop(app);
    drop(runtime);
    host.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stock_console_discovers_bundled_recipes_without_host_or_resources() {
    let tools = Arc::new(RecipeTools::default());
    assert_eq!(
        serde_json::to_value(tools.tools()).unwrap(),
        serde_json::to_value(RecipeTools::new().tools()).unwrap()
    );
    assert_eq!(
        tools
            .tools()
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        RECIPE_TOOLS
    );
    assert!(tools.tool_catalog_capabilities().exact_catalog);
    assert!(
        !tools
            .tool_catalog_capabilities()
            .may_require_catalog_control_plane
    );
    for name in RECIPE_TOOLS {
        assert_eq!(
            tools.tool_mutation_class(name),
            meerkat_core::ToolMutationClass::ReadOnly
        );
    }
    let probe = Arc::new(RecipeProbe::new());
    for (name, args) in [
        (RECIPE_TOOLS[0], json!({"limit":0})),
        (
            RECIPE_TOOLS[0],
            json!({"limit":1,"snapshot":"0".repeat(64)}),
        ),
        (RECIPE_TOOLS[1], json!({})),
        (
            RECIPE_TOOLS[1],
            json!({"identity":probe.gmail.package.identity,"digest":"0".repeat(64)}),
        ),
    ] {
        let args = serde_json::value::to_raw_value(&args).unwrap();
        assert!(matches!(
            tools
                .dispatch(ToolCallView {
                    id: "invalid",
                    name,
                    args: &args
                })
                .await,
            Err(ToolError::InvalidArguments { .. })
        ));
    }
    for name in [
        "toolkit_prepare_services",
        "setup_prepare",
        "setup_install",
        "install",
        "send_message",
        "web_search",
        PAIRED_HOST_FRONTEND_INVENTORY,
        PAIRED_HOST_FRONTEND_PREPARE,
        PAIRED_HOST_FRONTEND_SUBMIT,
        PAIRED_HOST_SECRET_CREATE,
    ] {
        let args = serde_json::value::RawValue::from_string("{}".into()).unwrap();
        assert!(
            matches!(
                tools
                    .dispatch(ToolCallView {
                        id: "excluded",
                        name,
                        args: &args
                    })
                    .await,
                Err(ToolError::NotFound { .. })
            ),
            "recipe dispatcher accepted {name}"
        );
    }

    // Only the existing native runtime is built. There is no PairedHost client,
    // resource locator/preparer, MCP server or additional session facade.
    let directory = tempfile::tempdir().unwrap();
    let runtime = setup_runtime(
        meerkat_mob::MobId::from("connect-console-recipes"),
        directory.path(),
        probe.clone(),
        ConsoleSetupCustomizer::recipes(AgentIdentity::parse(SETUP).unwrap()),
    )
    .await;
    let app = runtime.build_reference_app_router(decisions());
    let operator = bearer(OPERATOR);
    let outsider = bearer(OUTSIDER);
    let service = runtime.mob_runtime().session_service().unwrap();
    let identity = AgentIdentity::parse(SETUP).unwrap();
    let setup_id = runtime
        .identity_runtime()
        .unwrap()
        .status(&identity)
        .await
        .unwrap()
        .session_id
        .unwrap();
    let other_id = runtime
        .identity_runtime()
        .unwrap()
        .status(&AgentIdentity::parse(OTHER).unwrap())
        .await
        .unwrap()
        .session_id
        .unwrap();
    assert_ne!(setup_id, other_id);
    bootstrap_settled(service.as_ref(), &setup_id).await;
    bootstrap_settled(service.as_ref(), &other_id).await;
    assert_eq!(probe.calls.load(Ordering::SeqCst), 2);
    let mut setup_events =
        meerkat_mob::MobSessionService::subscribe_session_events(service.as_ref(), &setup_id)
            .await
            .unwrap();
    let mut other_events =
        meerkat_mob::MobSessionService::subscribe_session_events(service.as_ref(), &other_id)
            .await
            .unwrap();
    let (status, _) = rpc(
        &app,
        None,
        "mobkit/console/send",
        send(SETUP, RECIPE_FIRST, "unauthenticated"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (_, denied) = rpc(
        &app,
        Some(&outsider),
        "mobkit/console/send",
        send(SETUP, RECIPE_FIRST, "wrong-viewer"),
    )
    .await;
    assert_eq!(denied["error"]["code"], -32030);
    assert_eq!(denied["error"]["data"]["kind"], "access_denied");
    assert_eq!(
        probe.calls.load(Ordering::SeqCst),
        2,
        "denied callers cannot trigger the model"
    );

    for (prompt, key, answer) in [
        (RECIPE_FIRST, "recipe-once", RECIPE_ANSWER),
        (RECIPE_FOLLOWUP, "recipe-followup", RECIPE_FOLLOWUP_ANSWER),
    ] {
        let (_, accepted) = rpc(
            &app,
            Some(&operator),
            "mobkit/console/send",
            send(SETUP, prompt, key),
        )
        .await;
        assert!(accepted.get("error").is_none(), "{accepted}");
        assert_eq!(accepted["result"]["identity"], SETUP);
        completed(&mut setup_events, answer).await;
        assert_eq!(
            runtime
                .identity_runtime()
                .unwrap()
                .status(&identity)
                .await
                .unwrap()
                .session_id
                .as_ref(),
            Some(&setup_id)
        );
    }
    let (_, accepted) = rpc(
        &app,
        Some(&operator),
        "mobkit/console/send",
        send(OTHER, OTHER_PROMPT, "other-once"),
    )
    .await;
    assert!(accepted.get("error").is_none(), "{accepted}");
    unknown_tool_failure(&mut other_events, RECIPE_TOOLS[0]).await;
    let history = service
        .read_history(&setup_id, SessionHistoryQuery::default())
        .await
        .unwrap();
    let history = serde_json::to_string(&history).unwrap();
    no_private(&history);
    for text in [
        RECIPE_FIRST,
        RECIPE_ANSWER,
        RECIPE_FOLLOWUP,
        RECIPE_FOLLOWUP_ANSWER,
    ] {
        assert!(history.contains(text));
    }
    assert!(!history.contains(OTHER_PROMPT));
    let other_history = service
        .read_history(&other_id, SessionHistoryQuery::default())
        .await
        .unwrap();
    let other_history = serde_json::to_string(&other_history).unwrap();
    assert!(!other_history.contains(RECIPE_FIRST) && !other_history.contains(RECIPE_ANSWER));
    {
        let observed = probe.observed.lock().unwrap();
        assert!(observed.contains_key("recipes-0") && observed.contains_key("recipes-1"));
        assert_eq!(
            serde_json::from_str::<Value>(&observed["gmail"].text_content()).unwrap(),
            serde_json::to_value(&probe.gmail).unwrap()
        );
        assert_eq!(
            safe_tool_error_code(&observed["forged-send"]),
            "access_denied"
        );
    }
    // Exercise the stock console's projection of the same native history.
    timeout(BOUND, async {
        loop {
            let (_, timeline) = rpc(
                &app,
                Some(&operator),
                "mobkit/console/query_timeline",
                json!({"identity":SETUP,"limit":200}),
            )
            .await;
            assert!(timeline.get("error").is_none(), "{timeline}");
            let visible = serde_json::to_string(&timeline["result"]).unwrap();
            if [
                RECIPE_FIRST,
                RECIPE_ANSWER,
                RECIPE_FOLLOWUP,
                RECIPE_FOLLOWUP_ANSWER,
            ]
            .iter()
            .all(|text| visible.contains(*text))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    let (_, denied) = rpc(
        &app,
        Some(&outsider),
        "mobkit/console/query_timeline",
        json!({"identity":SETUP,"limit":200}),
    )
    .await;
    assert!(denied.get("error").is_none(), "{denied}");
    assert_eq!(denied["result"]["frames"], json!([]));
    let denied = serde_json::to_string(&denied).unwrap();
    assert!(!denied.contains(RECIPE_FIRST) && !denied.contains(RECIPE_ANSWER));
    let report = timeout(BOUND, Box::pin(runtime.shutdown())).await.unwrap();
    assert!(
        report.cleanup_completed(),
        "native runtime did not settle: {report:?}"
    );
    drop(app);
    drop(runtime);
}

const RESEARCH_MOB: &str = "connect-console-research";
const RESEARCH_MODEL: &str = "gpt-5.3-codex";
const MEMBER_KEY: &str = "synthetic-console-member-key";
const SEARCH_KEY: &str = "synthetic-console-search-key";
const RESEARCH_FIRST: &str = "Investigate an open source wiki whose content is arranged in shelves, books and chapters. Public research only; do not install or sign in.";
const RESEARCH_QUERY: &str = "open source wiki shelves books chapters BookStack";
const RESEARCH_ANSWER: &str = "BookStack matches the clue, according to the recorded public search. No installation was performed.";
const RESEARCH_FOLLOWUP: &str = "Which wiki did the search identify, and did you install it?";
const RESEARCH_FOLLOWUP_ANSWER: &str =
    "The same search identified BookStack. Nothing was installed or authenticated.";
const RESEARCH_BUILTIN: &str =
    "Check whether this research member can bypass its tool boundary to list native tasks.";
const RESEARCH_CANCEL: &str = "Begin one more public wiki search, which I will cancel.";
const CANCEL_QUERY: &str = "held public wiki research";
const SEARCH_TEXT: &str = "BookStack organizes documentation into shelves, books and chapters.";
const SEARCH_URL: &str = "https://www.bookstackapp.com/";
const RESEARCH_VISIBLE_TOOLS: [&str; 7] = [
    RECIPE_TOOLS[0],
    RECIPE_TOOLS[1],
    "web_search",
    "send_message",
    "reply_to_peer",
    "send_request",
    "send_response",
];

// Real service hook, not SessionHookCustomizerAdapter's synthetic request.
// The native producer has already stamped the typed member binding. The host
// selects each known member's auth route; only SETUP gains research visibility.
struct ResearchHook {
    research: ConsoleResearchHook,
    selected: meerkat_core::MobMemberBinding,
    other: meerkat_core::MobMemberBinding,
    member_auth: meerkat_core::AuthBindingRef,
    selected_builds: AtomicUsize,
    other_builds: AtomicUsize,
}
#[async_trait]
impl SessionHook for ResearchHook {
    async fn before_create(&self, request: &mut CreateSessionRequest) -> Result<(), SessionError> {
        let Some(build) = request.build.as_mut() else {
            return Ok(());
        };
        let selected = build.mob_member_binding.as_ref() == Some(&self.selected);
        let other = build.mob_member_binding.as_ref() == Some(&self.other);
        if selected || other {
            assert_eq!(request.model, RESEARCH_MODEL);
            assert_eq!(
                build.realm_id.as_ref(),
                Some(&meerkat_core::mob_realm_id(RESEARCH_MOB).unwrap())
            );
            assert!(build.llm_client_override.is_none());
            if other
                && build
                    .auth_binding
                    .as_ref()
                    .is_some_and(|binding| binding != &self.member_auth)
            {
                return Err(SessionError::Unsupported(
                    "conflicting host-selected member auth binding".into(),
                ));
            }
            if other {
                // This second member's separate auth choice is application
                // policy, not a side effect of the public setup hook.
                build.auth_binding = Some(self.member_auth.clone());
            }
        }
        self.research.before_create(request).await?;
        let build = request.build.as_ref().unwrap();
        if selected || other {
            assert_eq!(build.auth_binding.as_ref(), Some(&self.member_auth));
            assert_eq!(request.model, RESEARCH_MODEL);
            assert_eq!(
                build.realm_id.as_ref(),
                Some(&meerkat_core::mob_realm_id(RESEARCH_MOB).unwrap())
            );
        }
        if selected {
            assert_eq!(build.override_builtins, ToolCategoryOverride::Enable);
            assert_eq!(build.override_web_search, ToolCategoryOverride::Enable);
            self.selected_builds.fetch_add(1, Ordering::SeqCst);
        } else if other {
            assert_ne!(build.override_web_search, ToolCategoryOverride::Enable);
            self.other_builds.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    }
}

#[derive(Clone)]
struct ResearchProviderState {
    requests: Arc<Mutex<Vec<(String, Value)>>>,
    held: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
    started: Arc<Notify>,
}

fn response_text(text: &str) -> Value {
    json!({"type":"message","role":"assistant","status":"completed",
        "content":[{"type":"output_text","text":text,"annotations":[]}]})
}
fn response_call(id: &str, name: &str, args: Value) -> Value {
    json!({"type":"function_call","id":format!("fc_{id}"),"call_id":id,
        "name":name,"arguments":args.to_string(),"status":"completed"})
}
fn native_response(output: Vec<Value>, input_tokens: u64, output_tokens: u64) -> Response {
    // Exercise the published provider's Responses decoder and typed search
    // events, not a mocked WebSearchExecutor or injected agent client.
    let payload = json!({"type":"response.completed","response":{
        "status":"completed","output":output,
        "usage":{"input_tokens":input_tokens,"output_tokens":output_tokens}}});
    (
        [(header::CONTENT_TYPE, "text/event-stream")],
        format!("data: {payload}\n\ndata: [DONE]\n\n"),
    )
        .into_response()
}

async fn research_provider(
    State(state): State<ResearchProviderState>,
    request: Request<Body>,
) -> Response {
    assert_eq!(request.method(), "POST");
    let path = request.uri().path().to_owned();
    let key = match path.as_str() {
        "/member/v1/responses" => MEMBER_KEY,
        "/search/v1/responses" => SEARCH_KEY,
        _ => panic!("unexpected native provider route"),
    };
    assert_eq!(
        request.headers()[header::AUTHORIZATION],
        format!("Bearer {key}")
    );
    let bytes = to_bytes(request.into_body(), 256 * 1024).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    let encoded = body.to_string();
    assert!(!encoded.contains(MEMBER_KEY) && !encoded.contains(SEARCH_KEY));
    no_private(&encoded);
    assert_eq!(body["stream"], true);
    state
        .requests
        .lock()
        .unwrap()
        .push((path.clone(), body.clone()));
    if path == "/search/v1/responses" {
        assert_eq!(
            body["model"],
            meerkat_models::default_model(Provider::OpenAI).unwrap()
        );
        assert_eq!(body["max_output_tokens"], 2048);
        assert_eq!(body["tools"], json!([{"type":"web_search"}]));
        if encoded.contains(CANCEL_QUERY) {
            let held = state
                .held
                .lock()
                .unwrap()
                .take()
                .expect("only one held search");
            state.started.notify_one();
            held.await.unwrap();
        } else {
            assert!(encoded.contains(RESEARCH_QUERY));
        }
        let mut answer = response_text(SEARCH_TEXT);
        answer["content"][0]["annotations"] = json!([{"type":"url_citation",
            "url":SEARCH_URL,"title":"BookStack","start_index":0,"end_index":9}]);
        return native_response(
            vec![
                json!({"type":"web_search_call","id":"ws_fixture","status":"completed",
                "action":{"type":"search","queries":[RESEARCH_QUERY]}}),
                answer,
            ],
            31,
            17,
        );
    }
    assert_eq!(body["model"], RESEARCH_MODEL);
    let tools = body["tools"].as_array().unwrap();
    assert!(
        tools.iter().all(|tool| tool["type"] == "function"),
        "restricted member must not receive provider-native search"
    );
    let offered: BTreeSet<_> = tools
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    if offered.contains(RECIPE_TOOLS[0]) {
        assert_eq!(offered, RESEARCH_VISIBLE_TOOLS.into_iter().collect());
    }
    for forbidden in [
        "shell",
        "mob_spawn_member",
        "setup_install",
        PAIRED_HOST_FRONTEND_SUBMIT,
    ] {
        assert!(!offered.contains(forbidden));
    }
    let input = body["input"].as_array().unwrap();
    let literal = input
        .iter()
        .rev()
        .find(|item| item["role"] == "user")
        .expect("native user input")
        .to_string();
    let results: BTreeMap<_, _> = input
        .iter()
        .filter(|item| item["type"] == "function_call_output")
        .map(|item| {
            (
                item["call_id"].as_str().unwrap(),
                item["output"].as_str().unwrap(),
            )
        })
        .collect();
    let output = if literal.contains(BOOTSTRAP) {
        response_text(BOOTSTRAP_ANSWER)
    } else if literal.contains(OTHER_PROMPT) {
        assert!(!offered.contains("web_search"));
        response_call(
            "wrong-search-member",
            "web_search",
            json!({"query":RESEARCH_QUERY}),
        )
    } else if literal.contains(RESEARCH_FOLLOWUP) {
        assert!(encoded.contains(RESEARCH_FIRST) && encoded.contains(RESEARCH_ANSWER));
        response_text(RESEARCH_FOLLOWUP_ANSWER)
    } else if literal.contains(RESEARCH_BUILTIN) {
        assert_eq!(offered, RESEARCH_VISIBLE_TOOLS.into_iter().collect());
        response_call("forged-task-list", "task_list", json!({}))
    } else if literal.contains(RESEARCH_CANCEL) {
        assert!(offered.contains("web_search"));
        assert!(
            !results.contains_key("held-search"),
            "canceled search must not resume the model"
        );
        response_call("held-search", "web_search", json!({"query":CANCEL_QUERY}))
    } else {
        assert!(literal.contains(RESEARCH_FIRST));
        assert!(offered.contains("web_search"));
        if let Some(result) = results.get("native-search") {
            let result: Value = serde_json::from_str(result).unwrap();
            assert_eq!(result["status"], "completed");
            assert_eq!(result["provider"], "openai");
            assert_eq!(
                result["model"],
                meerkat_models::default_model(Provider::OpenAI).unwrap()
            );
            assert_eq!(result["answer"], SEARCH_TEXT);
            assert!(
                result["native_events"]
                    .as_array()
                    .is_some_and(|events| !events.is_empty())
            );
            assert!(
                result["evidence"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|entry| entry["url"] == SEARCH_URL)
            );
            if let Some(denied) = results.get("forged-send") {
                let denied: Value = serde_json::from_str(denied).unwrap();
                assert_eq!(denied["error"], "access_denied");
                response_text(RESEARCH_ANSWER)
            } else {
                response_call(
                    "forged-send",
                    "send_message",
                    json!({
                    "peer_id":"00000000-0000-4000-8000-000000000001",
                    "body":"must not be delivered","handling_mode":"queue"}),
                )
            }
        } else {
            response_call(
                "native-search",
                "web_search",
                json!({"query":RESEARCH_QUERY}),
            )
        }
    };
    native_response(vec![output], 10, 5)
}

struct ResearchProvider {
    state: ResearchProviderState,
    origin: String,
    release: Option<oneshot::Sender<()>>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl ResearchProvider {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let (release, held) = oneshot::channel();
        let state = ResearchProviderState {
            requests: Arc::default(),
            held: Arc::new(Mutex::new(Some(held))),
            started: Arc::new(Notify::new()),
        };
        let app = Router::new()
            .fallback(research_provider)
            .with_state(state.clone());
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        });
        Self {
            state,
            origin,
            release: Some(release),
            stop: Some(stop),
            task: Some(task),
        }
    }
    fn count(&self, path: &str) -> usize {
        self.state
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(route, _)| route == path)
            .count()
    }
    fn config(&self) -> Config {
        let realm = meerkat_core::mob_realm_id(RESEARCH_MOB).unwrap();
        let mut config = serde_json::to_value(Config::default()).unwrap();
        config["agent"]["model"] = json!(RESEARCH_MODEL);
        config["realm"] = json!({(realm.as_str()): {
            "default_binding":"research",
            "backend":{
                "member":{"provider":"openai","backend_kind":"openai_api","base_url":format!("{}/member", self.origin)},
                "research":{"provider":"openai","backend_kind":"openai_api","base_url":format!("{}/search", self.origin)}},
            "auth":{
                "member":{"provider":"openai","auth_method":"api_key","source":{"kind":"inline_secret","secret":MEMBER_KEY}},
                "research":{"provider":"openai","auth_method":"api_key","source":{"kind":"inline_secret","secret":SEARCH_KEY}}},
            "binding":{
                "member":{"backend_profile":"member","auth_profile":"member","credential_account":"member-account","default_model":RESEARCH_MODEL},
                "research":{"backend_profile":"research","auth_profile":"research","credential_account":"research-account"}}
        }});
        let mut config: Config = serde_json::from_value(config).unwrap();
        config.retry.max_retries = 0;
        config.model_fallback.enabled = Some(false);
        config.provider_tools.openai.web_search = true;
        config
    }
    async fn finish(mut self) {
        // Cancellation may already have disposed the HTTP receiver.
        let _ = self.release.take().unwrap().send(());
        self.stop.take().unwrap().send(()).unwrap();
        timeout(BOUND, self.task.take().unwrap())
            .await
            .unwrap()
            .unwrap();
    }
}
impl Drop for ResearchProvider {
    fn drop(&mut self) {
        // Failed assertions retain ordinary test cleanup; passing acceptance
        // requires the explicit graceful join in finish().
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stock_console_research_uses_explicit_native_helper_and_exact_member() {
    let provider = ResearchProvider::start().await;
    let directory = tempfile::tempdir().unwrap();
    let member = meerkat_mobkit::member_comms_id::roster_member_id_for_identity(SETUP);
    let selected = meerkat_core::MobMemberBinding {
        mob_id: RESEARCH_MOB.into(),
        role: "setup".into(),
        member: member.to_string(),
    };
    let member_auth = meerkat_core::AuthBindingRef {
        realm: meerkat_core::mob_realm_id(RESEARCH_MOB).unwrap(),
        binding: meerkat_core::BindingId::parse("member").unwrap(),
        profile: None,
        origin: meerkat_core::BindingOrigin::Configured,
    };
    let hook = Arc::new(ResearchHook {
        research: ConsoleResearchHook::new(selected.clone(), member_auth.clone()),
        selected,
        other: meerkat_core::MobMemberBinding {
            mob_id: RESEARCH_MOB.into(),
            role: "setup".into(),
            member: meerkat_mobkit::member_comms_id::roster_member_id_for_identity(OTHER)
                .to_string(),
        },
        member_auth,
        selected_builds: AtomicUsize::new(0),
        other_builds: AtomicUsize::new(0),
    });
    let config = provider.config();
    let registry =
        meerkat_core::ModelRegistry::from_config(&config, meerkat_models::canonical()).unwrap();
    assert!(
        !registry
            .profile_for_provider(Provider::OpenAI, RESEARCH_MODEL)
            .unwrap()
            .supports_web_search
    );
    assert!(
        registry
            .profile_for_provider(
                Provider::OpenAI,
                registry.default_model(Provider::OpenAI).unwrap()
            )
            .unwrap()
            .supports_web_search
    );
    // No default_llm_client, host decorator, executor override, environment change,
    // PairedHost client or prepared service resource is used by this scenario.
    let builder = setup_builder(
        meerkat_mob::MobId::from(RESEARCH_MOB),
        directory.path(),
        ConsoleSetupCustomizer::recipes(AgentIdentity::parse(SETUP).unwrap()),
        RESEARCH_MODEL,
    )
    .await
    .meerkat_config(config)
    .session_hook(hook.clone());
    let runtime = timeout(BOUND, Box::pin(builder.build()))
        .await
        .unwrap()
        .unwrap();
    let app = runtime.build_reference_app_router(decisions());
    let service = runtime.mob_runtime().session_service().unwrap();
    let identity = AgentIdentity::parse(SETUP).unwrap();
    // Builder return may precede background identity materialization. The
    // native barrier distinguishes a terminal Broken member from readiness.
    let (bootstrap, timed_out) = runtime
        .identity_runtime()
        .unwrap()
        .wait_identity_bootstrap_terminal(BOUND)
        .await;
    if timed_out || !bootstrap.ready {
        let diagnostic = serde_json::to_string(&bootstrap).unwrap();
        no_private(&diagnostic);
        assert!(
            !diagnostic.contains(MEMBER_KEY) && !diagnostic.contains(SEARCH_KEY),
            "native bootstrap diagnostic contained a fixture credential"
        );
        panic!("native research bootstrap not ready (timed_out={timed_out}): {diagnostic}");
    }
    let setup_id = runtime
        .identity_runtime()
        .unwrap()
        .status(&identity)
        .await
        .unwrap()
        .session_id
        .expect("native ready research member has a session");
    let other_id = runtime
        .identity_runtime()
        .unwrap()
        .status(&AgentIdentity::parse(OTHER).unwrap())
        .await
        .unwrap()
        .session_id
        .unwrap();
    assert_ne!(setup_id, other_id);
    bootstrap_settled(service.as_ref(), &setup_id).await;
    bootstrap_settled(service.as_ref(), &other_id).await;
    assert_eq!(hook.selected_builds.load(Ordering::SeqCst), 1);
    assert_eq!(hook.other_builds.load(Ordering::SeqCst), 1);
    assert_eq!(provider.count("/member/v1/responses"), 2);
    assert_eq!(provider.count("/search/v1/responses"), 0);
    let mut events =
        meerkat_mob::MobSessionService::subscribe_session_events(service.as_ref(), &setup_id)
            .await
            .unwrap();
    let mut other_events =
        meerkat_mob::MobSessionService::subscribe_session_events(service.as_ref(), &other_id)
            .await
            .unwrap();
    let operator = bearer(OPERATOR);
    let outsider = bearer(OUTSIDER);
    let (status, _) = rpc(
        &app,
        None,
        "mobkit/console/send",
        send(SETUP, RESEARCH_FIRST, "research-no-auth"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (_, denied) = rpc(
        &app,
        Some(&outsider),
        "mobkit/console/send",
        send(SETUP, RESEARCH_FIRST, "research-outsider"),
    )
    .await;
    assert_eq!(denied["error"]["data"]["kind"], "access_denied");
    assert_eq!(provider.count("/member/v1/responses"), 2);
    assert_eq!(provider.count("/search/v1/responses"), 0);
    for (prompt, answer, key) in [
        (RESEARCH_FIRST, RESEARCH_ANSWER, "research-first"),
        (
            RESEARCH_FOLLOWUP,
            RESEARCH_FOLLOWUP_ANSWER,
            "research-followup",
        ),
    ] {
        let (_, accepted) = rpc(
            &app,
            Some(&operator),
            "mobkit/console/send",
            send(SETUP, prompt, key),
        )
        .await;
        assert!(accepted.get("error").is_none(), "{accepted}");
        completed(&mut events, answer).await;
        assert_eq!(
            runtime
                .identity_runtime()
                .unwrap()
                .status(&identity)
                .await
                .unwrap()
                .session_id
                .as_ref(),
            Some(&setup_id)
        );
        assert_eq!(
            service.read(&setup_id).await.unwrap().state.model,
            RESEARCH_MODEL
        );
    }
    assert_eq!(provider.count("/search/v1/responses"), 1);
    let (_, accepted) = rpc(
        &app,
        Some(&operator),
        "mobkit/console/send",
        send(OTHER, OTHER_PROMPT, "research-other"),
    )
    .await;
    assert!(accepted.get("error").is_none(), "{accepted}");
    unknown_tool_failure(&mut other_events, "web_search").await;
    assert_eq!(provider.count("/search/v1/responses"), 1);
    let history = serde_json::to_string(
        &service
            .read_history(&setup_id, SessionHistoryQuery::default())
            .await
            .unwrap(),
    )
    .unwrap();
    for text in [
        RESEARCH_FIRST,
        RESEARCH_ANSWER,
        RESEARCH_FOLLOWUP,
        RESEARCH_FOLLOWUP_ANSWER,
        SEARCH_URL,
    ] {
        assert!(history.contains(text));
    }
    assert!(
        !history.contains(MEMBER_KEY)
            && !history.contains(SEARCH_KEY)
            && !history.contains(OTHER_PROMPT)
    );
    timeout(BOUND, async {
        loop {
            let (_, timeline) = rpc(
                &app,
                Some(&operator),
                "mobkit/console/query_timeline",
                json!({"identity":SETUP,"limit":200}),
            )
            .await;
            assert!(timeline.get("error").is_none());
            let visible = timeline["result"].to_string();
            assert!(!visible.contains(MEMBER_KEY) && !visible.contains(SEARCH_KEY));
            if [
                RESEARCH_FIRST,
                RESEARCH_ANSWER,
                RESEARCH_FOLLOWUP,
                RESEARCH_FOLLOWUP_ANSWER,
            ]
            .iter()
            .all(|text| visible.contains(text))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    let (_, denied) = rpc(
        &app,
        Some(&outsider),
        "mobkit/console/query_timeline",
        json!({"identity":SETUP,"limit":200}),
    )
    .await;
    assert_eq!(denied["result"]["frames"], json!([]));

    let member_calls = provider.count("/member/v1/responses");
    let (_, accepted) = rpc(
        &app,
        Some(&operator),
        "mobkit/console/send",
        send(SETUP, RESEARCH_BUILTIN, "research-denied-builtin"),
    )
    .await;
    assert!(accepted.get("error").is_none(), "{accepted}");
    tool_failure(&mut events, ToolError::access_denied("task_list")).await;
    assert_eq!(provider.count("/search/v1/responses"), 1);
    assert_eq!(provider.count("/member/v1/responses"), member_calls + 1);

    let (_, accepted) = rpc(
        &app,
        Some(&operator),
        "mobkit/console/send",
        send(SETUP, RESEARCH_CANCEL, "research-cancel"),
    )
    .await;
    assert!(accepted.get("error").is_none(), "{accepted}");
    timeout(BOUND, provider.state.started.notified())
        .await
        .unwrap();
    let run_id = timeout(BOUND, async {
        while let Some(event) = events.next().await {
            if let AgentEvent::RunStarted { identity, .. } = event.payload {
                return identity.run_id.unwrap();
            }
        }
        panic!("missing exact started run");
    })
    .await
    .unwrap();
    let calls_before_stop = provider.count("/member/v1/responses");
    let receipt = timeout(
        BOUND,
        runtime.mob_handle().stop_member_run(
            meerkat_mob::MobControlPrincipal::Owner,
            member,
            run_id.clone(),
            "operator canceled public research",
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let receipt = serde_json::to_value(receipt).unwrap();
    assert_eq!(receipt["outcome"], "stopped");
    assert_eq!(receipt["run_id"], run_id.to_string());
    let contributors = receipt["contributors"].as_array().unwrap();
    assert_eq!(contributors.len(), 1);
    assert_eq!(contributors[0]["terminal"], "cancelled");
    assert_eq!(contributors[0]["completion"], "cancelled");
    assert!(!service.read(&setup_id).await.unwrap().state.is_active);
    assert_eq!(
        provider.count("/search/v1/responses"),
        2,
        "one completed search plus one canceled request, no retry"
    );
    assert_eq!(
        provider.count("/member/v1/responses"),
        calls_before_stop,
        "no model continuation after stop"
    );
    let report = timeout(BOUND, Box::pin(runtime.shutdown())).await.unwrap();
    assert!(report.cleanup_completed(), "{report:?}");
    drop(app);
    drop(runtime);
    // Remote HTTP handler disposal is separate from native terminality.
    // Release the fixture response and join its server, without a new request.
    assert_eq!(provider.count("/member/v1/responses"), calls_before_stop);
    provider.finish().await;
}

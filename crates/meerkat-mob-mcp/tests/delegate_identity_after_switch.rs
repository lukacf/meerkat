//! `delegate` after an in-place switch of the delegating session's LLM
//! identity.
//!
//! A session's identity (model, provider, self-hosted server, auth binding)
//! can change in place: a turn that names `model`, `provider`,
//! `self_hosted_server_id` or `auth_binding` (JSON-RPC `turn/start`, a mob
//! member's `MemberTurnOptions`) reconfigures the live session, and its
//! later turns run on the new identity. A `delegate` the session makes after
//! the switch should run its helper on the identity the session has when the
//! call runs.
//!
//! Every test here runs over the production runtime-backed session service
//! with the real agent mob tools and no LLM client override: each build and
//! each in-place switch builds its client from the realm's config, and the
//! config's endpoints are loopback OpenAI Responses servers that record
//! every request they receive. The tests assert on the requests the helper
//! actually sent (which endpoint, which model) and on the route the
//! session's implicit delegation mob records for its helpers (its
//! `delegate` profile). The helper's own session is archived when it
//! retires, so its durable metadata is not readable afterwards; the implicit
//! mob's profile is the identity the helper was built from.
//!
//! The helper is delegated with the tool's default tooling (it inherits its
//! parent's tool categories), so the test build enables the facade's
//! `memory-store-session` feature and the mob state carries a WorkGraph
//! service: the categories it opens build.
//!
//! The model registry needs every model id to be unique, so two endpoints
//! serving the same model name appear in two shapes: two self-hosted aliases
//! on two servers that both serve the remote model [`SHARED_REMOTE_MODEL`],
//! and one catalog model reached through two realm bindings whose backends
//! are two endpoints.
#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat_core::lifecycle::run_primitive::{ModelId, TurnMetadataOverride};
use meerkat_core::{
    AuthBindingRef, BindingId, ContentInput, HandlingMode, Provider, RealmConfigSection, RealmId,
};
use meerkat_mob::{
    AgentIdentity, MemberTurnOptions, MobBackendKind, MobDefinition, MobId, Profile,
    ProfileBinding, ProfileName,
};
use meerkat_mob_mcp::MobMcpState;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use support::participant_profile;

/// The remote model both self-hosted servers serve.
const SHARED_REMOTE_MODEL: &str = "mirror-model";
/// The alias of [`SHARED_REMOTE_MODEL`] on the server `one`.
const MIRROR_ON_ONE: &str = "mirror-on-one";
/// The alias of [`SHARED_REMOTE_MODEL`] on the server `two`.
const MIRROR_ON_TWO: &str = "mirror-on-two";
/// A second model on the server `one`.
const OTHER_ON_ONE: &str = "other-on-one";
/// The remote model [`OTHER_ON_ONE`] names.
const OTHER_REMOTE_MODEL: &str = "other-model";
const SERVER_ONE: &str = "one";
const SERVER_TWO: &str = "two";

/// The catalog model the auth-binding case runs on, behind both bindings.
const CATALOG_MODEL: &str = "gpt-5.4";
const REALM: &str = "global";
const BINDING_ONE: &str = "endpoint_one";
const BINDING_TWO: &str = "endpoint_two";

const SWITCH_TASK: &str = "IDENTITY-SWITCH-TURN";
const DELEGATE_TASK: &str = "IDENTITY-DELEGATE-TASK";
const HELPER_TASK: &str = "IDENTITY-HELPER-TASK";
const HELPER_ANSWER: &str = "IDENTITY-HELPER-ANSWER";

// ===========================================================================
// Loopback OpenAI Responses endpoint
// ===========================================================================

/// Who sent a request, read from its content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Caller {
    /// The delegating session.
    Parent,
    /// The delegated helper.
    Helper,
}

/// One request an endpoint received.
#[derive(Debug, Clone)]
struct WireRequest {
    caller: Caller,
    model: String,
    /// The request's `authorization` header: which credential sent it.
    authorization: String,
}

/// A loopback Responses endpoint: the delegating session's delegate turn
/// calls `delegate`, the helper answers its task, every other turn answers
/// `ok`.
struct Endpoint {
    base_url: String,
    requests: Arc<Mutex<Vec<WireRequest>>>,
    /// The `delegate` call's result, as the delegating session's model got
    /// it back on this endpoint.
    delegate_result: Arc<Mutex<Option<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Endpoint {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the loopback endpoint");
        let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let delegate_result = Arc::new(Mutex::new(None));
        let task = {
            let requests = Arc::clone(&requests);
            let delegate_result = Arc::clone(&delegate_result);
            tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        return;
                    };
                    let requests = Arc::clone(&requests);
                    let delegate_result = Arc::clone(&delegate_result);
                    tokio::spawn(serve_one(stream, requests, delegate_result));
                }
            })
        };
        Self {
            base_url,
            requests,
            delegate_result,
            task,
        }
    }

    fn requests(&self) -> Vec<WireRequest> {
        self.requests.lock().unwrap().clone()
    }

    /// The models of the requests `caller` sent here.
    fn models_of(&self, caller: Caller) -> Vec<String> {
        self.requests()
            .into_iter()
            .filter(|request| request.caller == caller)
            .map(|request| request.model)
            .collect()
    }

    /// The `authorization` headers of the requests `caller` sent here.
    fn authorizations_of(&self, caller: Caller) -> Vec<String> {
        self.requests()
            .into_iter()
            .filter(|request| request.caller == caller)
            .map(|request| request.authorization)
            .collect()
    }

    fn delegate_result(&self) -> Option<String> {
        self.delegate_result.lock().unwrap().clone()
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Reads one HTTP request from `stream`, answers it, and closes.
async fn serve_one(
    mut stream: tokio::net::TcpStream,
    requests: Arc<Mutex<Vec<WireRequest>>>,
    delegate_result: Arc<Mutex<Option<String>>>,
) {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 8192];
    let header_end = loop {
        let Ok(read) = stream.read(&mut chunk).await else {
            return;
        };
        if read == 0 {
            return;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
    };
    let head = String::from_utf8_lossy(&buffer[..header_end]).to_string();
    let content_length = head
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let authorization = head
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("authorization"))
        .map(|(_, value)| value.trim().to_string())
        .unwrap_or_default();
    while buffer.len() < header_end + content_length {
        let Ok(read) = stream.read(&mut chunk).await else {
            return;
        };
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let response = if head.starts_with("POST") {
        let body: Value = serde_json::from_slice(&buffer[header_end..]).unwrap_or(Value::Null);
        let (content_type, payload) = answer(&body, &authorization, &requests, &delegate_result);
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\n\
             connection: close\r\n\r\n{payload}",
            payload.len()
        )
    } else {
        let payload = r#"{"object":"list","data":[]}"#;
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
             connection: close\r\n\r\n{payload}",
            payload.len()
        )
    };
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

/// The `output` of the `function_call_output` item for `call_id` in a
/// Responses request body, if the request carries one.
fn function_call_output(body: &Value, call_id: &str) -> Option<String> {
    body["input"].as_array()?.iter().find_map(|item| {
        (item["type"] == "function_call_output" && item["call_id"] == call_id).then(|| {
            item["output"]
                .as_str()
                .map_or_else(|| item["output"].to_string(), str::to_string)
        })
    })
}

/// Records the request and scripts its answer as a Responses event stream.
fn answer(
    body: &Value,
    authorization: &str,
    requests: &Mutex<Vec<WireRequest>>,
    delegate_result: &Mutex<Option<String>>,
) -> (&'static str, String) {
    let model = body["model"].as_str().unwrap_or_default().to_string();
    let input = body["input"].to_string();
    let record = |caller| WireRequest {
        caller,
        model: model.clone(),
        authorization: authorization.to_string(),
    };
    let item = if let Some(result) = function_call_output(body, "call-delegate") {
        requests.lock().unwrap().push(record(Caller::Parent));
        *delegate_result.lock().unwrap() = Some(result);
        text_item("delegated")
    } else if input.contains(DELEGATE_TASK) {
        requests.lock().unwrap().push(record(Caller::Parent));
        json!({
            "type": "function_call",
            "id": "fc_delegate",
            "call_id": "call-delegate",
            "name": "delegate",
            "status": "completed",
            "arguments": json!({
                "task": HELPER_TASK,
                "member_id": "helper",
                "result_label": "helper_result",
                "max_text_bytes": 4096,
            })
            .to_string(),
        })
    } else if input.contains(HELPER_TASK) {
        requests.lock().unwrap().push(record(Caller::Helper));
        text_item(HELPER_ANSWER)
    } else {
        requests.lock().unwrap().push(record(Caller::Parent));
        text_item("ok")
    };
    let events = [
        json!({"type": "response.created", "response": {"id": "resp_1"}}),
        json!({"type": "response.output_item.done", "output_index": 0, "item": item}),
        json!({
            "type": "response.completed",
            "response": {
                "id": "resp_1",
                "status": "completed",
                "output": [item],
                "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2},
            },
        }),
    ];
    let mut payload = String::new();
    for event in events {
        payload.push_str(&format!("data: {event}\n\n"));
    }
    payload.push_str("data: [DONE]\n\n");
    ("text/event-stream", payload)
}

fn text_item(text: &str) -> Value {
    json!({
        "type": "message",
        "id": "msg_1",
        "role": "assistant",
        "content": [{"type": "output_text", "text": text, "annotations": []}],
    })
}

// ===========================================================================
// Realm configs
// ===========================================================================

fn self_hosted_model(server: &str, remote_model: &str) -> meerkat_core::SelfHostedModelConfig {
    meerkat_core::SelfHostedModelConfig {
        server: server.to_string(),
        remote_model: remote_model.to_string(),
        display_name: remote_model.to_string(),
        family: remote_model.to_string(),
        tier: meerkat_core::model_profile::catalog::ModelTier::Supported,
        context_window: Some(128_000),
        max_input_tokens: None,
        max_output_tokens: Some(4_096),
        vision: false,
        image_tool_results: false,
        inline_video: false,
        supports_temperature: true,
        supports_thinking: false,
        supports_reasoning: false,
        supports_web_search: false,
        call_timeout_secs: Some(60),
        stream_inactivity_timeout_secs: None,
    }
}

/// Two self-hosted servers, `one` at `one` and `two` at `two`, that both
/// serve [`SHARED_REMOTE_MODEL`] (as [`MIRROR_ON_ONE`] and
/// [`MIRROR_ON_TWO`]); `one` also serves [`OTHER_REMOTE_MODEL`] as
/// [`OTHER_ON_ONE`]. One realm binding with no server reaches both, so the
/// session's self-hosted server alone picks the endpoint.
fn self_hosted_config(one: &Endpoint, two: &Endpoint) -> meerkat::Config {
    let mut config = meerkat::Config::default();
    for (name, endpoint) in [(SERVER_ONE, one), (SERVER_TWO, two)] {
        config.self_hosted.servers.insert(
            name.to_string(),
            meerkat_core::SelfHostedServerConfig {
                transport: meerkat_core::SelfHostedTransport::OpenAiCompatible,
                base_url: endpoint.base_url.clone(),
                api_style: meerkat_core::SelfHostedApiStyle::Responses,
            },
        );
    }
    config.self_hosted.default_model = Some(MIRROR_ON_ONE.to_string());
    // One realm binding reaches the self-hosted servers; it names no
    // server, so the model's server is the endpoint.
    let mut section = RealmConfigSection::default();
    let id = "self_hosted".to_string();
    section.backend.insert(
        id.clone(),
        meerkat_core::BackendProfileConfig {
            provider: Provider::SelfHosted.as_str().to_string(),
            backend_kind: "self_hosted".to_string(),
            base_url: None,
            options: Value::Null,
            server: None,
        },
    );
    section.auth.insert(
        id.clone(),
        meerkat_core::AuthProfileConfig {
            provider: Provider::SelfHosted.as_str().to_string(),
            auth_method: "none".to_string(),
            source: meerkat_core::CredentialSourceSpec::PlatformDefault,
            constraints: Default::default(),
            metadata_defaults: Default::default(),
        },
    );
    section.binding.insert(
        id.clone(),
        meerkat_core::ProviderBindingConfig {
            backend_profile: id.clone(),
            auth_profile: id,
            credential_account: None,
            default_model: None,
            policy: Default::default(),
            provider_default: true,
        },
    );
    config.realm.insert(REALM.to_string(), section);
    for (alias, server, remote_model) in [
        (MIRROR_ON_ONE, SERVER_ONE, SHARED_REMOTE_MODEL),
        (MIRROR_ON_TWO, SERVER_TWO, SHARED_REMOTE_MODEL),
        (OTHER_ON_ONE, SERVER_ONE, OTHER_REMOTE_MODEL),
    ] {
        config
            .self_hosted
            .models
            .insert(alias.to_string(), self_hosted_model(server, remote_model));
    }
    config
}

/// The realm `global` (the one every realm inherits) with two OpenAI bindings whose backends are `one` and
/// `two`; [`BINDING_ONE`] is the provider's default.
fn two_binding_config(one: &Endpoint, two: &Endpoint) -> meerkat::Config {
    let mut config = meerkat::Config::default();
    let mut section = RealmConfigSection::default();
    for (binding, endpoint, provider_default) in
        [(BINDING_ONE, one, true), (BINDING_TWO, two, false)]
    {
        section.backend.insert(
            binding.to_string(),
            meerkat_core::BackendProfileConfig {
                provider: Provider::OpenAI.as_str().to_string(),
                backend_kind: "openai_api".to_string(),
                base_url: Some(endpoint.base_url.clone()),
                options: Value::Null,
                server: None,
            },
        );
        section.auth.insert(
            binding.to_string(),
            meerkat_core::AuthProfileConfig {
                provider: Provider::OpenAI.as_str().to_string(),
                auth_method: "api_key".to_string(),
                source: meerkat_core::CredentialSourceSpec::InlineSecret {
                    secret: format!("synthetic-key-{binding}"),
                },
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            },
        );
        section.binding.insert(
            binding.to_string(),
            meerkat_core::ProviderBindingConfig {
                backend_profile: binding.to_string(),
                auth_profile: binding.to_string(),
                credential_account: None,
                default_model: None,
                policy: Default::default(),
                provider_default,
            },
        );
    }
    section.default_binding = Some(BINDING_ONE.to_string());
    config.realm.insert(REALM.to_string(), section);
    config
}

fn binding(binding: &str) -> AuthBindingRef {
    AuthBindingRef {
        realm: RealmId::parse(REALM).unwrap(),
        binding: BindingId::parse(binding).unwrap(),
        profile: None,
        origin: meerkat_core::BindingOrigin::Configured,
    }
}

// ===========================================================================
// The delegating session
// ===========================================================================

/// One member on `model` that mounts the agent mob tools, on `provider` and
/// the self-hosted `server`.
fn parent_definition(
    mob_id: &MobId,
    model: &str,
    provider: Provider,
    server: Option<&str>,
) -> MobDefinition {
    let mut profile = participant_profile("delegating parent");
    profile.model = model.to_string();
    profile.provider = Some(provider);
    profile.self_hosted_server_id = server.map(str::to_string);
    profile.tools.mob = true;
    profile.external_addressable = true;
    let mut definition = MobDefinition::explicit(mob_id.clone());
    definition.profiles.insert(
        ProfileName::from("participant"),
        ProfileBinding::Inline(Box::new(profile)),
    );
    definition
}

/// The delegating session, a mob member built by the production factory.
struct Delegator {
    state: Arc<MobMcpState>,
    mob_id: MobId,
    _temp: tempfile::TempDir,
}

impl Delegator {
    async fn spawn(
        config: meerkat::Config,
        model: &str,
        provider: Provider,
        server: Option<&str>,
    ) -> Self {
        let temp = tempfile::tempdir().expect("temp dir");
        let state = support::agent_mob_tools_state_on_configured_providers(temp.path(), config);
        let mob_id = MobId::from(format!("identity-switch-{}", uuid::Uuid::new_v4().simple()));
        state
            .mob_create_definition(parent_definition(&mob_id, model, provider, server))
            .await
            .expect("create the mob");
        state
            .mob_spawn(
                &mob_id,
                ProfileName::from("participant"),
                AgentIdentity::from("parent"),
                Some(meerkat_mob::MobRuntimeMode::TurnDriven),
                Some(MobBackendKind::Session),
                None,
            )
            .await
            .expect("spawn the delegating session");
        Self {
            state,
            mob_id,
            _temp: temp,
        }
    }

    /// Runs one turn of the delegating session with `options` to completion.
    async fn turn(&self, prompt: &str, options: MemberTurnOptions) {
        let turn = self
            .state
            .handle_for(&self.mob_id)
            .await
            .expect("mob handle")
            .member(&AgentIdentity::from("parent"))
            .await
            .expect("member handle")
            .start_turn(
                ContentInput::Text(prompt.to_string()),
                HandlingMode::Queue,
                options,
                None,
            )
            .await
            .expect("turn admitted");
        tokio::time::timeout(Duration::from_mins(2), turn.wait())
            .await
            .expect("the turn completes")
            .expect("the turn succeeds");
    }

    /// Switches the session's identity in place with a turn that names the
    /// new identity, then runs the delegating turn with no override.
    async fn switch_then_delegate(&self, switch: MemberTurnOptions) {
        self.turn(SWITCH_TASK, switch).await;
        self.turn(DELEGATE_TASK, MemberTurnOptions::default()).await;
    }

    async fn session_id(&self) -> meerkat_core::SessionId {
        self.state
            .handle_for(&self.mob_id)
            .await
            .expect("mob handle")
            .resolve_bridge_session_id(&AgentIdentity::from("parent"))
            .await
            .expect("the delegating member's session")
    }

    /// The delegating session's durable metadata.
    async fn metadata(&self) -> meerkat_core::SessionMetadata {
        let session = self.session_id().await;
        self.state
            .session_service()
            .load_persisted_session_metadata(&session)
            .await
            .expect("read the delegating session's metadata")
            .and_then(|view| view.session_metadata)
            .expect("the delegating session has metadata")
    }

    /// The `delegate` profile of the session's implicit delegation mob: the
    /// route its helpers run on.
    async fn implicit_delegate_profile(&self) -> Profile {
        let session = self.session_id().await;
        let implicit = self
            .state
            .find_implicit_mob_for_bridge_session(&session.to_string())
            .await
            .expect("the session's implicit delegation mob");
        self.state
            .handle_for(&implicit)
            .await
            .expect("implicit mob handle")
            .definition()
            .resolve_inline_profile(&ProfileName::from("delegate"))
            .cloned()
            .expect("the implicit mob's delegate profile")
    }

    async fn destroy(self) {
        let _ = self.state.mob_destroy(&self.mob_id).await;
    }
}

/// The delegate result on whichever endpoint the delegating session's last
/// turn ran, checked to be a completed helper run.
fn completed_delegate_result(endpoints: &[&Endpoint]) -> String {
    let result = endpoints
        .iter()
        .find_map(|endpoint| endpoint.delegate_result())
        .expect("the delegating session's model got the delegate result back");
    assert!(
        result.contains("\"status\":\"completed\"") && result.contains(HELPER_ANSWER),
        "the helper ran to completion: {result}"
    );
    result
}

fn self_hosted_switch(model: &str, server: &str) -> MemberTurnOptions {
    MemberTurnOptions::new()
        .with_model(ModelId::new(model))
        .with_provider(Provider::SelfHosted)
        .with_self_hosted_server_id(server)
}

// ===========================================================================
// 1. Model switch on one server
// ===========================================================================

/// Runs the model-switch scenario: a session on [`MIRROR_ON_ONE`] switches
/// in place to [`OTHER_ON_ONE`] (same provider, same server) and delegates.
/// Returns the endpoint and the implicit mob's `delegate` profile.
async fn model_switch_scenario() -> (Endpoint, Endpoint, Profile) {
    let one = Endpoint::start().await;
    let two = Endpoint::start().await;
    let delegator = Delegator::spawn(
        self_hosted_config(&one, &two),
        MIRROR_ON_ONE,
        Provider::SelfHosted,
        Some(SERVER_ONE),
    )
    .await;
    delegator
        .switch_then_delegate(self_hosted_switch(OTHER_ON_ONE, SERVER_ONE))
        .await;

    // The switch took: the session itself now runs on the new model.
    let metadata = delegator.metadata().await;
    assert_eq!(
        metadata.model, OTHER_ON_ONE,
        "the in-place switch moved the session"
    );
    let parent_models = one.models_of(Caller::Parent);
    assert_eq!(
        parent_models.last().map(String::as_str),
        Some(OTHER_REMOTE_MODEL),
        "the delegating session's turns after the switch run on the new model: {parent_models:?}"
    );
    completed_delegate_result(&[&one, &two]);
    let delegate = delegator.implicit_delegate_profile().await;
    delegator.destroy().await;
    (one, two, delegate)
}

/// Intended behaviour. A session built on [`MIRROR_ON_ONE`] and switched in
/// place to [`OTHER_ON_ONE`] (same provider and server) delegates: the
/// helper runs on the session's current model, so its request to the
/// endpoint names [`OTHER_REMOTE_MODEL`], and the implicit mob records
/// [`OTHER_ON_ONE`] as the helpers' model. The session's identity is the
/// one its turns run on now; a helper delegated from it should not run on a
/// model the session has left.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "delegate keeps the session's build-time identity after an in-place switch; see the PR"]
async fn delegate_after_an_in_place_model_switch_runs_the_helper_on_the_new_model() {
    let (one, two, delegate) = model_switch_scenario().await;

    assert_eq!(
        one.models_of(Caller::Helper),
        [OTHER_REMOTE_MODEL],
        "the helper's request names the session's current model"
    );
    assert!(two.requests().is_empty());
    assert_eq!(
        (delegate.model.as_str(), delegate.provider),
        (OTHER_ON_ONE, Some(Provider::SelfHosted)),
        "the implicit mob records the session's current model for its helpers"
    );
}

/// Records today's behaviour; delete it when
/// [`delegate_after_an_in_place_model_switch_runs_the_helper_on_the_new_model`]
/// (which states the intended behaviour) passes. The delegate tool keeps
/// the model the session was built with, so after the switch the helper's
/// request names the build-time [`SHARED_REMOTE_MODEL`] and the implicit
/// mob records [`MIRROR_ON_ONE`].
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn today_delegate_after_an_in_place_model_switch_runs_the_helper_on_the_build_model() {
    let (one, _two, delegate) = model_switch_scenario().await;

    assert_eq!(one.models_of(Caller::Helper), [SHARED_REMOTE_MODEL]);
    assert_eq!(delegate.model, MIRROR_ON_ONE);
}

// ===========================================================================
// 2. Endpoint switch: two servers serving the same model
// ===========================================================================

/// Runs the endpoint-switch scenario: a session on [`MIRROR_ON_ONE`]
/// (server `one`) switches in place to [`MIRROR_ON_TWO`] (server `two`),
/// both serving [`SHARED_REMOTE_MODEL`], and delegates.
async fn endpoint_switch_scenario() -> (Endpoint, Endpoint, Profile) {
    let one = Endpoint::start().await;
    let two = Endpoint::start().await;
    let delegator = Delegator::spawn(
        self_hosted_config(&one, &two),
        MIRROR_ON_ONE,
        Provider::SelfHosted,
        Some(SERVER_ONE),
    )
    .await;
    delegator
        .switch_then_delegate(self_hosted_switch(MIRROR_ON_TWO, SERVER_TWO))
        .await;

    let metadata = delegator.metadata().await;
    assert_eq!(
        (
            metadata.model.as_str(),
            metadata.self_hosted_server_id.as_deref()
        ),
        (MIRROR_ON_TWO, Some(SERVER_TWO)),
        "the in-place switch moved the session to server two"
    );
    assert!(
        one.models_of(Caller::Parent).is_empty(),
        "the switch turn itself already runs on server two: {:?}",
        one.requests()
    );
    assert!(
        two.models_of(Caller::Parent).len() >= 2,
        "the switch turn and the delegate turn run on server two: {:?}",
        two.requests()
    );
    completed_delegate_result(&[&one, &two]);
    let delegate = delegator.implicit_delegate_profile().await;
    delegator.destroy().await;
    (one, two, delegate)
}

/// Intended behaviour. Two self-hosted servers serve the same model
/// ([`SHARED_REMOTE_MODEL`]); a session built on server `one` and switched
/// in place to server `two` delegates. The helper's request arrives at
/// server `two`, where the session now runs, and not at server `one`; the
/// implicit mob records server `two`. The model name alone cannot tell the
/// two routes apart, so this is the case where a helper on the build-time
/// route is visibly on the wrong endpoint.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "delegate keeps the session's build-time identity after an in-place switch; see the PR"]
async fn delegate_after_an_in_place_endpoint_switch_sends_the_helper_to_the_new_endpoint() {
    let (one, two, delegate) = endpoint_switch_scenario().await;

    assert_eq!(
        two.models_of(Caller::Helper),
        [SHARED_REMOTE_MODEL],
        "the helper's request arrives at the session's current server"
    );
    assert!(
        one.models_of(Caller::Helper).is_empty(),
        "no helper request reaches the server the session left"
    );
    assert_eq!(
        (
            delegate.model.as_str(),
            delegate.self_hosted_server_id.as_deref()
        ),
        (MIRROR_ON_TWO, Some(SERVER_TWO)),
        "the implicit mob records the session's current server for its helpers"
    );
}

/// Records today's behaviour; delete it when
/// [`delegate_after_an_in_place_endpoint_switch_sends_the_helper_to_the_new_endpoint`]
/// (which states the intended behaviour) passes. The delegate tool keeps
/// the route the session was built with, so after the switch to server
/// `two` the helper's request still arrives at server `one`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn today_delegate_after_an_in_place_endpoint_switch_sends_the_helper_to_the_build_endpoint() {
    let (one, two, delegate) = endpoint_switch_scenario().await;

    assert_eq!(one.models_of(Caller::Helper), [SHARED_REMOTE_MODEL]);
    assert!(two.models_of(Caller::Helper).is_empty());
    assert_eq!(
        (
            delegate.model.as_str(),
            delegate.self_hosted_server_id.as_deref()
        ),
        (MIRROR_ON_ONE, Some(SERVER_ONE))
    );
}

// ===========================================================================
// 3. Auth binding switch: a record, not a requirement
// ===========================================================================

/// A record of today's behaviour, not a requirement: whether a helper should
/// follow its session's auth binding is an open decision upstream.
///
/// One catalog model ([`CATALOG_MODEL`]) is served by two realm bindings
/// whose backends are two endpoints; [`BINDING_ONE`] is the provider's
/// default. A session built with no binding (so on [`BINDING_ONE`]'s
/// endpoint) is switched in place to [`BINDING_TWO`] and delegates.
///
/// Today the switch moves the session to endpoint two, while the helper's
/// request arrives at endpoint one: the delegate tool names no binding for
/// its helpers, so the helper resolves its own credentials through the
/// realm's default binding. If the decision is that a helper follows the
/// session's binding, this test inverts (the helper's request arrives at
/// endpoint two) and should be renamed to state that requirement.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn record_delegate_after_an_auth_binding_switch_resolves_the_helper_on_the_default_binding() {
    let one = Endpoint::start().await;
    let two = Endpoint::start().await;
    let delegator = Delegator::spawn(
        two_binding_config(&one, &two),
        CATALOG_MODEL,
        Provider::OpenAI,
        None,
    )
    .await;
    delegator
        .switch_then_delegate(
            MemberTurnOptions::new()
                .with_auth_binding(TurnMetadataOverride::Set(binding(BINDING_TWO))),
        )
        .await;

    let metadata = delegator.metadata().await;
    assert_eq!(
        metadata.auth_binding.as_ref().map(|b| b.binding.as_str()),
        Some(BINDING_TWO),
        "the in-place switch moved the session to binding two"
    );
    assert!(
        one.models_of(Caller::Parent).is_empty(),
        "the session's turns from the switch on run on endpoint two: {:?}",
        one.requests()
    );
    assert!(two.models_of(Caller::Parent).len() >= 2);
    completed_delegate_result(&[&one, &two]);

    // Today: the helper is on the realm's default binding, endpoint one.
    assert_eq!(
        one.models_of(Caller::Helper),
        [CATALOG_MODEL],
        "today the helper resolves its own credentials (the realm default), not the session's \
         binding"
    );
    assert!(two.models_of(Caller::Helper).is_empty());
    assert_eq!(
        one.authorizations_of(Caller::Helper),
        [format!("Bearer synthetic-key-{BINDING_ONE}")],
        "today the helper authenticates with the default binding's credential"
    );
    assert!(
        two.authorizations_of(Caller::Parent)
            .iter()
            .all(|authorization| authorization == &format!("Bearer synthetic-key-{BINDING_TWO}")),
        "the session itself authenticates with the binding it switched to"
    );
    let delegate = delegator.implicit_delegate_profile().await;
    assert_eq!(
        (delegate.model.as_str(), delegate.provider),
        (CATALOG_MODEL, Some(Provider::OpenAI)),
        "the implicit mob records the model and provider; a profile carries no auth binding"
    );
    delegator.destroy().await;
}

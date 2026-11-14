//! `delegate` after an in-place switch of the delegating session's LLM
//! identity.
//!
//! A session's identity (model, provider, self-hosted server, auth binding)
//! can change in place: a turn that names `model`, `provider`,
//! `self_hosted_server_id` or `auth_binding` (JSON-RPC `turn/start`, a mob
//! member's `MemberTurnOptions`) reconfigures the live session, and its
//! later turns run on the new identity. A `delegate` the session makes after
//! the switch runs its helper on the identity the session has when the call
//! runs, its auth binding included, and a session whose route changed since
//! its last delegate gets its implicit mob replaced on the new route.
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
//! are two endpoints. The provider switch needs no alias: the self-hosted
//! alias and the OpenAI catalog model are different ids.
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
/// A second delegate of the same session, in a later turn. None of these
/// contains its first-delegate counterpart, so each turn's answer is told
/// apart by content alone.
const SECOND_DELEGATE_TASK: &str = "IDENTITY-SECOND-DELEGATE-TASK";
const SECOND_HELPER_TASK: &str = "IDENTITY-SECOND-HELPER-TASK";
const SECOND_HELPER_ANSWER: &str = "IDENTITY-SECOND-HELPER-ANSWER";

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
    /// The helper of the session's second delegate.
    SecondHelper,
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
    /// The second `delegate` call's result, likewise.
    second_delegate_result: Arc<Mutex<Option<String>>>,
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
        let second_delegate_result = Arc::new(Mutex::new(None));
        let task = {
            let requests = Arc::clone(&requests);
            let delegate_result = Arc::clone(&delegate_result);
            let second_delegate_result = Arc::clone(&second_delegate_result);
            tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        return;
                    };
                    let results = DelegateResults {
                        first: Arc::clone(&delegate_result),
                        second: Arc::clone(&second_delegate_result),
                    };
                    tokio::spawn(serve_one(stream, Arc::clone(&requests), results));
                }
            })
        };
        Self {
            base_url,
            requests,
            delegate_result,
            second_delegate_result,
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

    fn second_delegate_result(&self) -> Option<String> {
        self.second_delegate_result.lock().unwrap().clone()
    }
}

/// Where an endpoint records the delegate results the delegating session's
/// model got back.
struct DelegateResults {
    first: Arc<Mutex<Option<String>>>,
    second: Arc<Mutex<Option<String>>>,
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
    results: DelegateResults,
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
        let (content_type, payload) = answer(&body, &authorization, &requests, &results);
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
///
/// A turn's request carries the session's whole history, so a later turn
/// still holds the first delegate's call and result. The second delegate is
/// therefore matched first: its result, then its task.
fn answer(
    body: &Value,
    authorization: &str,
    requests: &Mutex<Vec<WireRequest>>,
    results: &DelegateResults,
) -> (&'static str, String) {
    let model = body["model"].as_str().unwrap_or_default().to_string();
    let input = body["input"].to_string();
    let record = |caller| WireRequest {
        caller,
        model: model.clone(),
        authorization: authorization.to_string(),
    };
    let delegate_call = |item_id: &str, call_id: &str, task: &str, member_id: &str| {
        json!({
            "type": "function_call",
            "id": item_id,
            "call_id": call_id,
            "name": "delegate",
            "status": "completed",
            "arguments": json!({
                "task": task,
                "member_id": member_id,
                "result_label": "helper_result",
                "max_text_bytes": 4096,
            })
            .to_string(),
        })
    };
    let item = if let Some(result) = function_call_output(body, "call-delegate-second") {
        requests.lock().unwrap().push(record(Caller::Parent));
        *results.second.lock().unwrap() = Some(result);
        text_item("delegated")
    } else if input.contains(SECOND_DELEGATE_TASK) {
        requests.lock().unwrap().push(record(Caller::Parent));
        delegate_call(
            "fc_delegate_second",
            "call-delegate-second",
            SECOND_HELPER_TASK,
            "second-helper",
        )
    } else if let Some(result) = function_call_output(body, "call-delegate") {
        requests.lock().unwrap().push(record(Caller::Parent));
        *results.first.lock().unwrap() = Some(result);
        text_item("delegated")
    } else if input.contains(DELEGATE_TASK) {
        requests.lock().unwrap().push(record(Caller::Parent));
        delegate_call("fc_delegate", "call-delegate", HELPER_TASK, "helper")
    } else if input.contains(SECOND_HELPER_TASK) {
        requests.lock().unwrap().push(record(Caller::SecondHelper));
        text_item(SECOND_HELPER_ANSWER)
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
        reasoning_effort_levels: None,
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
        self.implicit_handle()
            .await
            .definition()
            .resolve_inline_profile(&ProfileName::from("delegate"))
            .cloned()
            .expect("the implicit mob's delegate profile")
    }

    /// The incarnation the session's implicit delegation mob runs at: a
    /// replaced mob keeps its id and runs at a new incarnation.
    async fn implicit_incarnation(&self) -> meerkat_core::service::MobIncarnationId {
        self.implicit_handle().await.incarnation()
    }

    async fn implicit_handle(&self) -> meerkat_mob::MobHandle {
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

/// The second delegate's result on whichever endpoint the delegating
/// session's turn ran, checked to be a completed helper run.
fn completed_second_delegate_result(endpoints: &[&Endpoint]) -> String {
    let result = endpoints
        .iter()
        .find_map(|endpoint| endpoint.second_delegate_result())
        .expect("the delegating session's model got the second delegate's result back");
    assert!(
        result.contains("\"status\":\"completed\"") && result.contains(SECOND_HELPER_ANSWER),
        "the second helper ran to completion: {result}"
    );
    result
}

/// The note a delegate result carries when the call created the session's
/// implicit mob, a replacement included.
const IMPLICIT_MOB_CREATED: &str = "Implicit delegation mob created";

/// The auth binding a `delegate` profile records, in its persisted form: the
/// form the implicit mob's profile is rebuilt from when the mob is restored.
fn recorded_auth_binding(profile: &Profile) -> Value {
    serde_json::to_value(profile).expect("the profile serializes")["auth_binding"].clone()
}

/// A binding in the same persisted form, for comparison.
fn persisted_auth_binding(binding: Option<&AuthBindingRef>) -> Value {
    serde_json::to_value(binding).expect("the binding serializes")
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

/// A session built on [`MIRROR_ON_ONE`] and switched in place to
/// [`OTHER_ON_ONE`] (same provider and server) delegates: the helper runs on
/// the session's current model, so its request to the endpoint names
/// [`OTHER_REMOTE_MODEL`], and the implicit mob records [`OTHER_ON_ONE`] as
/// the helpers' model. The session's identity is the one its turns run on
/// now; a helper delegated from it should not run on a model the session has
/// left.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
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

/// Two self-hosted servers serve the same model ([`SHARED_REMOTE_MODEL`]); a
/// session built on server `one` and switched in place to server `two`
/// delegates. The helper's request arrives at server `two`, where the session
/// now runs, and not at server `one`; the implicit mob records server `two`.
/// The model name alone cannot tell the two routes apart, so this is the case
/// where a helper on the build-time route is visibly on the wrong endpoint.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
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

// ===========================================================================
// 3. Auth binding switch
// ===========================================================================

/// One catalog model ([`CATALOG_MODEL`]) is served by two realm bindings
/// whose backends are two endpoints; [`BINDING_ONE`] is the provider's
/// default. A session built with no binding (so on [`BINDING_ONE`]'s
/// endpoint) is switched in place to [`BINDING_TWO`] and delegates.
///
/// The helper follows the session's binding: its request reaches endpoint
/// two with endpoint two's key, and none reaches endpoint one. The implicit
/// mob's `delegate` profile records the binding, so the helper's credentials
/// are the session's, not the realm's default.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delegate_after_an_auth_binding_switch_sends_the_helper_to_the_new_binding_with_its_key() {
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

    assert_eq!(
        two.models_of(Caller::Helper),
        [CATALOG_MODEL],
        "the helper's request reaches the endpoint of the session's current binding"
    );
    assert!(
        one.models_of(Caller::Helper).is_empty(),
        "no helper request reaches the binding the session left"
    );
    assert_eq!(
        two.authorizations_of(Caller::Helper),
        [format!("Bearer synthetic-key-{BINDING_TWO}")],
        "the helper authenticates with the session's binding's credential"
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
        "the implicit mob records the session's model and provider"
    );
    assert_eq!(
        recorded_auth_binding(&delegate),
        persisted_auth_binding(metadata.auth_binding.as_ref()),
        "the implicit mob records the session's current binding for its helpers"
    );
    delegator.destroy().await;
}

// ===========================================================================
// 4. Provider switch: self-hosted to OpenAI
// ===========================================================================

/// The OpenAI binding the provider-switch case moves to.
const BINDING_OPENAI: &str = "openai_endpoint";

/// [`self_hosted_config`] plus one OpenAI binding in the realm `global`
/// whose backend is `openai`, the provider's default. The session's
/// provider alone picks between the self-hosted servers and `openai`.
fn cross_provider_config(one: &Endpoint, two: &Endpoint, openai: &Endpoint) -> meerkat::Config {
    let mut config = self_hosted_config(one, two);
    let section = config
        .realm
        .get_mut(REALM)
        .expect("the self-hosted config's realm section");
    section.backend.insert(
        BINDING_OPENAI.to_string(),
        meerkat_core::BackendProfileConfig {
            provider: Provider::OpenAI.as_str().to_string(),
            backend_kind: "openai_api".to_string(),
            base_url: Some(openai.base_url.clone()),
            options: Value::Null,
            server: None,
        },
    );
    section.auth.insert(
        BINDING_OPENAI.to_string(),
        meerkat_core::AuthProfileConfig {
            provider: Provider::OpenAI.as_str().to_string(),
            auth_method: "api_key".to_string(),
            source: meerkat_core::CredentialSourceSpec::InlineSecret {
                secret: format!("synthetic-key-{BINDING_OPENAI}"),
            },
            constraints: Default::default(),
            metadata_defaults: Default::default(),
        },
    );
    section.binding.insert(
        BINDING_OPENAI.to_string(),
        meerkat_core::ProviderBindingConfig {
            backend_profile: BINDING_OPENAI.to_string(),
            auth_profile: BINDING_OPENAI.to_string(),
            credential_account: None,
            default_model: None,
            policy: Default::default(),
            provider_default: true,
        },
    );
    config
}

/// Runs the provider-switch scenario: a session on [`MIRROR_ON_ONE`]
/// (provider self-hosted, server `one`) switches in place to
/// [`CATALOG_MODEL`] on provider OpenAI, whose binding's backend is the
/// endpoint `openai`, and delegates. Returns the endpoints (server `one`,
/// server `two`, `openai`) and the implicit mob's `delegate` profile.
async fn provider_switch_scenario() -> (Endpoint, Endpoint, Endpoint, Profile) {
    let one = Endpoint::start().await;
    let two = Endpoint::start().await;
    let openai = Endpoint::start().await;
    let delegator = Delegator::spawn(
        cross_provider_config(&one, &two, &openai),
        MIRROR_ON_ONE,
        Provider::SelfHosted,
        Some(SERVER_ONE),
    )
    .await;
    delegator
        .switch_then_delegate(
            MemberTurnOptions::new()
                .with_model(ModelId::new(CATALOG_MODEL))
                .with_provider(Provider::OpenAI),
        )
        .await;

    // The switch took: the session itself now runs on OpenAI.
    let metadata = delegator.metadata().await;
    assert_eq!(
        (metadata.model.as_str(), metadata.provider),
        (CATALOG_MODEL, Provider::OpenAI),
        "the in-place switch moved the session to OpenAI"
    );
    assert!(
        one.models_of(Caller::Parent).is_empty(),
        "the switch turn itself already runs on OpenAI: {:?}",
        one.requests()
    );
    assert!(
        openai.models_of(Caller::Parent).len() >= 2,
        "the switch turn and the delegate turn run on the OpenAI endpoint: {:?}",
        openai.requests()
    );
    assert!(two.requests().is_empty());
    completed_delegate_result(&[&one, &two, &openai]);
    let delegate = delegator.implicit_delegate_profile().await;
    delegator.destroy().await;
    (one, two, openai, delegate)
}

/// A session built on the self-hosted [`MIRROR_ON_ONE`] (server `one`) and
/// switched in place to [`CATALOG_MODEL`] on provider OpenAI delegates. The
/// helper's request arrives at the OpenAI endpoint and names
/// [`CATALOG_MODEL`], and the implicit mob records the OpenAI route with no
/// self-hosted server. The model, provider and server are separate parts of
/// the route, so a helper that took the session's model and server but kept
/// the build-time provider would fail this case.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delegate_after_an_in_place_provider_switch_sends_the_helper_to_the_new_provider() {
    let (one, two, openai, delegate) = provider_switch_scenario().await;

    assert_eq!(
        openai.models_of(Caller::Helper),
        [CATALOG_MODEL],
        "the helper's request arrives at the session's current provider and names its model"
    );
    assert!(
        one.models_of(Caller::Helper).is_empty(),
        "no helper request reaches the self-hosted server the session left"
    );
    assert!(two.requests().is_empty());
    assert_eq!(
        (
            delegate.model.as_str(),
            delegate.provider,
            delegate.self_hosted_server_id.as_deref()
        ),
        (CATALOG_MODEL, Some(Provider::OpenAI), None),
        "the implicit mob records the session's current provider for its helpers"
    );
}

// ===========================================================================
// 5. A second delegate: the implicit mob follows the session's route
// ===========================================================================

/// A session on server `one` delegates, which creates its implicit mob on
/// that route. It then switches in place to server `two` and delegates
/// again: the implicit mob is replaced (a new incarnation, and the call
/// reports a created mob), the replacement records server `two`, and the
/// second helper's request arrives at server `two` while the first helper's
/// stayed at server `one`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delegate_after_a_switch_replaces_the_implicit_mob_and_runs_the_helper_on_the_new_route()
{
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
        .turn(DELEGATE_TASK, MemberTurnOptions::default())
        .await;
    let first = completed_delegate_result(&[&one, &two]);
    assert!(
        first.contains(IMPLICIT_MOB_CREATED),
        "the first delegate creates the implicit mob: {first}"
    );
    let before = delegator.implicit_incarnation().await;
    let profile = delegator.implicit_delegate_profile().await;
    assert_eq!(
        (
            profile.model.as_str(),
            profile.self_hosted_server_id.as_deref()
        ),
        (MIRROR_ON_ONE, Some(SERVER_ONE))
    );

    delegator
        .turn(SWITCH_TASK, self_hosted_switch(MIRROR_ON_TWO, SERVER_TWO))
        .await;
    delegator
        .turn(SECOND_DELEGATE_TASK, MemberTurnOptions::default())
        .await;

    let second = completed_second_delegate_result(&[&one, &two]);
    assert!(
        second.contains(IMPLICIT_MOB_CREATED),
        "the second delegate replaces the implicit mob the session's route left: {second}"
    );
    assert_ne!(
        delegator.implicit_incarnation().await,
        before,
        "the replacement runs at a new incarnation"
    );
    let profile = delegator.implicit_delegate_profile().await;
    assert_eq!(
        (
            profile.model.as_str(),
            profile.self_hosted_server_id.as_deref()
        ),
        (MIRROR_ON_TWO, Some(SERVER_TWO)),
        "the replacement records the session's current route"
    );
    assert_eq!(
        one.models_of(Caller::Helper),
        [SHARED_REMOTE_MODEL],
        "the first helper ran on the route the session had then"
    );
    assert_eq!(
        two.models_of(Caller::SecondHelper),
        [SHARED_REMOTE_MODEL],
        "the second helper runs where the session runs now"
    );
    assert!(
        one.models_of(Caller::SecondHelper).is_empty(),
        "no second-helper request reaches the server the session left"
    );
    delegator.destroy().await;
}

/// Without a switch nothing changes: the session's second delegate reuses
/// its implicit mob (the same incarnation, and the call reports no created
/// mob), and both helpers run on the route the session was built with.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_a_switch_a_second_delegate_reuses_the_implicit_mob_on_the_same_route() {
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
        .turn(DELEGATE_TASK, MemberTurnOptions::default())
        .await;
    let first = completed_delegate_result(&[&one, &two]);
    assert!(
        first.contains(IMPLICIT_MOB_CREATED),
        "the first delegate creates the implicit mob: {first}"
    );
    let before = delegator.implicit_incarnation().await;

    delegator
        .turn(SECOND_DELEGATE_TASK, MemberTurnOptions::default())
        .await;

    let second = completed_second_delegate_result(&[&one, &two]);
    assert!(
        !second.contains(IMPLICIT_MOB_CREATED),
        "the second delegate reuses the implicit mob: {second}"
    );
    assert_eq!(
        delegator.implicit_incarnation().await,
        before,
        "the implicit mob keeps its incarnation"
    );
    let profile = delegator.implicit_delegate_profile().await;
    assert_eq!(
        (
            profile.model.as_str(),
            profile.provider,
            profile.self_hosted_server_id.as_deref()
        ),
        (MIRROR_ON_ONE, Some(Provider::SelfHosted), Some(SERVER_ONE))
    );
    assert_eq!(one.models_of(Caller::Helper), [SHARED_REMOTE_MODEL]);
    assert_eq!(one.models_of(Caller::SecondHelper), [SHARED_REMOTE_MODEL]);
    assert!(two.requests().is_empty(), "nothing reaches server two");
    delegator.destroy().await;
}

/// A binding change alone replaces the implicit mob. A session on
/// [`CATALOG_MODEL`] that names no binding runs on the realm's default,
/// [`BINDING_ONE`], and delegates: its implicit mob records the session's
/// binding, and the helper reaches endpoint one with endpoint one's key. The
/// session then switches in place to [`BINDING_TWO`], model and provider
/// unchanged, and delegates again: the implicit mob is replaced, the
/// replacement records binding two, and the second helper reaches endpoint
/// two with endpoint two's key.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_binding_change_alone_replaces_the_implicit_mob() {
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
        .turn(DELEGATE_TASK, MemberTurnOptions::default())
        .await;
    let first = completed_delegate_result(&[&one, &two]);
    assert!(
        first.contains(IMPLICIT_MOB_CREATED),
        "the first delegate creates the implicit mob: {first}"
    );
    let before = delegator.implicit_incarnation().await;
    let metadata = delegator.metadata().await;
    assert_eq!(
        metadata.auth_binding.as_ref().map(|b| b.binding.as_str()),
        Some(BINDING_ONE),
        "a session that names no binding runs on the realm's default"
    );
    assert_eq!(
        recorded_auth_binding(&delegator.implicit_delegate_profile().await),
        persisted_auth_binding(metadata.auth_binding.as_ref()),
        "the implicit mob records the session's binding"
    );
    assert_eq!(
        one.authorizations_of(Caller::Helper),
        [format!("Bearer synthetic-key-{BINDING_ONE}")]
    );

    delegator
        .turn(
            SWITCH_TASK,
            MemberTurnOptions::new()
                .with_auth_binding(TurnMetadataOverride::Set(binding(BINDING_TWO))),
        )
        .await;
    delegator
        .turn(SECOND_DELEGATE_TASK, MemberTurnOptions::default())
        .await;

    let metadata = delegator.metadata().await;
    assert_eq!(
        (
            metadata.model.as_str(),
            metadata.provider,
            metadata.auth_binding.as_ref().map(|b| b.binding.as_str())
        ),
        (CATALOG_MODEL, Provider::OpenAI, Some(BINDING_TWO)),
        "the switch changed the session's binding alone"
    );
    let second = completed_second_delegate_result(&[&one, &two]);
    assert!(
        second.contains(IMPLICIT_MOB_CREATED),
        "the binding change replaced the implicit mob: {second}"
    );
    assert_ne!(
        delegator.implicit_incarnation().await,
        before,
        "the replacement runs at a new incarnation"
    );
    let profile = delegator.implicit_delegate_profile().await;
    assert_eq!(
        (profile.model.as_str(), profile.provider),
        (CATALOG_MODEL, Some(Provider::OpenAI))
    );
    assert_eq!(
        recorded_auth_binding(&profile),
        persisted_auth_binding(metadata.auth_binding.as_ref()),
        "the replacement records the session's current binding"
    );
    assert_eq!(
        two.authorizations_of(Caller::SecondHelper),
        [format!("Bearer synthetic-key-{BINDING_TWO}")],
        "the second helper reaches endpoint two with endpoint two's key"
    );
    assert!(
        one.models_of(Caller::SecondHelper).is_empty(),
        "no second-helper request reaches the binding the session left"
    );
    delegator.destroy().await;
}

/// The switch and the delegate in one turn: a turn that names server `two`
/// and delegates. The switch is committed before the turn runs, so the
/// helper of that same turn already runs on server `two`, and nothing
/// reaches server `one`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delegate_in_the_switching_turn_runs_the_helper_on_the_new_route() {
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
        .turn(DELEGATE_TASK, self_hosted_switch(MIRROR_ON_TWO, SERVER_TWO))
        .await;

    completed_delegate_result(&[&one, &two]);
    assert!(
        one.requests().is_empty(),
        "the switching turn and its helper run on server two: {:?}",
        one.requests()
    );
    assert_eq!(
        two.models_of(Caller::Helper),
        [SHARED_REMOTE_MODEL],
        "the helper runs where the switched session runs"
    );
    let profile = delegator.implicit_delegate_profile().await;
    assert_eq!(
        (
            profile.model.as_str(),
            profile.self_hosted_server_id.as_deref()
        ),
        (MIRROR_ON_TWO, Some(SERVER_TWO)),
        "the implicit mob records the route the turn switched to"
    );
    delegator.destroy().await;
}

//! Sessions on realm models outside the catalog delegate.
//!
//! The realm's config defines a model the catalog does not know: a
//! `[models.<id>]` entry, or a self-hosted model. A session runs on it, and
//! its model turn calls `delegate`. The session's implicit delegation mob
//! runs its `delegate` profile on the session's model, so the mob's
//! definition must name the provider the session's build resolved for that
//! model: a definition naming an uncatalogued model alone does not validate,
//! and `delegate` fails. For a self-hosted model it must also name the server
//! the session's build resolved, or the helpers would run on the model's
//! configured server rather than the one the session is pinned to.
//!
//! End to end over the production runtime-backed session service with the
//! real agent mob tools: the delegating session is a mob member built by the
//! production factory, as every surface builds its sessions. Only the LLM is
//! scripted; it serves the realm's model. The call names its helper's tools
//! with an inline profile on the realm's model, as this crate's other
//! production-composition delegate rows do: a helper that inherits its
//! parent's tool categories opens every category, which needs host features
//! (semantic memory) this test build does not compile.
#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat_client::LlmRequest;
use meerkat_core::{ContentInput, HandlingMode, Message, Provider};
use meerkat_mob::{
    AgentIdentity, MobBackendKind, MobDefinition, MobId, Profile, ProfileBinding, ProfileName,
};
use meerkat_mob_mcp::MobMcpState;
use support::{ScriptedCouncilClient, ScriptedTurn, last_user_text, participant_profile};

/// A model id outside the catalog, defined only by the realm's config.
const REALM_MODEL: &str = "realm-gateway-model";
/// A self-hosted model outside the catalog, configured on the server
/// `local`.
const SELF_HOSTED_MODEL: &str = "gemma-4-31b";
/// The server the delegating session on [`SELF_HOSTED_MODEL`] is pinned to.
const PINNED_SERVER: &str = "other";
const DELEGATE_TASK: &str = "REALM-MODEL-DELEGATE-TASK";
const HELPER_TASK: &str = "REALM-MODEL-HELPER-TASK";
const HELPER_ANSWER: &str = "REALM-MODEL-HELPER-ANSWER";

/// The realm's config: one `[models.<id>]` entry the scripted provider
/// serves.
fn realm_config() -> meerkat::Config {
    let mut config = meerkat::Config::default();
    config.models.custom.insert(
        REALM_MODEL.to_string(),
        meerkat_core::config::CustomModelConfig {
            provider: Provider::Anthropic,
            display_name: None,
            context_window: Some(200_000),
            max_input_tokens: None,
            max_output_tokens: Some(8_192),
            vision: None,
            web_search: None,
            call_timeout_secs: None,
        },
    );
    config
}

/// The realm's config: two self-hosted servers, and one self-hosted model
/// configured on `local`.
fn self_hosted_config() -> meerkat::Config {
    let mut config = meerkat::Config::default();
    for (name, base_url) in [
        ("local", "http://127.0.0.1:11434"),
        (PINNED_SERVER, "http://127.0.0.1:11435"),
    ] {
        config.self_hosted.servers.insert(
            name.to_string(),
            meerkat_core::SelfHostedServerConfig {
                transport: meerkat_core::SelfHostedTransport::OpenAiCompatible,
                base_url: base_url.to_string(),
                api_style: meerkat_core::SelfHostedApiStyle::Responses,
            },
        );
    }
    config.self_hosted.models.insert(
        SELF_HOSTED_MODEL.to_string(),
        meerkat_core::SelfHostedModelConfig {
            server: "local".to_string(),
            remote_model: "gemma4:31b".to_string(),
            display_name: "Gemma 4 31B".into(),
            family: "gemma-4".to_string(),
            tier: meerkat_core::model_profile::catalog::ModelTier::Supported,
            context_window: Some(256_000),
            max_input_tokens: None,
            max_output_tokens: Some(8_192),
            vision: false,
            image_tool_results: false,
            inline_video: false,
            supports_temperature: true,
            supports_thinking: false,
            supports_reasoning: false,
            supports_web_search: false,
            call_timeout_secs: Some(600),
            stream_inactivity_timeout_secs: None,
            reasoning_effort_levels: None,
        },
    );
    config
}

/// One member on `model` that mounts the agent mob tools. Its own mob names
/// the provider, and the server it is pinned to, as a mob definition must
/// for a model outside the catalog.
fn parent_definition(
    mob_id: &MobId,
    model: &str,
    provider: Provider,
    self_hosted_server_id: Option<&str>,
) -> MobDefinition {
    let mut profile = participant_profile("delegating parent");
    profile.model = model.to_string();
    profile.provider = Some(provider);
    profile.self_hosted_server_id = self_hosted_server_id.map(str::to_string);
    profile.tools.mob = true;
    // The test drives the member's own turn from outside the mob.
    profile.external_addressable = true;
    let mut definition = MobDefinition::explicit(mob_id.clone());
    definition.profiles.insert(
        ProfileName::from("participant"),
        ProfileBinding::Inline(Box::new(profile)),
    );
    definition
}

/// What the scripted provider saw.
#[derive(Default, Clone)]
struct Observed {
    /// The model of each request the delegating session made.
    parent_models: Arc<Mutex<Vec<String>>>,
    /// The model of each request the helper made.
    helper_models: Arc<Mutex<Vec<String>>>,
    /// The `delegate` call's result, as the delegating session's model got
    /// it back.
    delegate_result: Arc<Mutex<Option<String>>>,
}

/// The tool-result text of each call in `messages`, keyed by call id.
fn tool_results(messages: &[Message]) -> BTreeMap<String, String> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::ToolResults { results, .. } => Some(results),
            _ => None,
        })
        .flatten()
        .map(|result| (result.tool_use_id.clone(), result.text_content()))
        .collect()
}

/// The delegating session calls `delegate` once, with an inline helper
/// profile on `model`, and answers with its result; the helper answers its
/// task.
fn script(
    observed: Observed,
    model: &'static str,
) -> impl Fn(&LlmRequest) -> ScriptedTurn + Send + Sync + 'static {
    move |request| {
        let user = last_user_text(request);
        if user.contains(HELPER_TASK) {
            observed
                .helper_models
                .lock()
                .unwrap()
                .push(request.model.clone());
            return ScriptedTurn::Text(HELPER_ANSWER.to_string());
        }
        if !user.contains(DELEGATE_TASK) {
            return ScriptedTurn::Text("ok".to_string());
        }
        observed
            .parent_models
            .lock()
            .unwrap()
            .push(request.model.clone());
        if let Some(result) = tool_results(&request.messages).remove("call-delegate") {
            *observed.delegate_result.lock().unwrap() = Some(result);
            return ScriptedTurn::Text("delegated".to_string());
        }
        ScriptedTurn::ToolCall {
            id: "call-delegate".to_string(),
            name: "delegate".to_string(),
            args: serde_json::json!({
                "task": HELPER_TASK,
                "member_id": "helper",
                "result_label": "helper_result",
                "max_text_bytes": 4096,
                "tooling": {
                    "mode": "profile",
                    "source": {
                        "type": "inline",
                        "model": model,
                        "tools": { "comms": true }
                    }
                },
            }),
        }
    }
}

/// The scripted client, claiming the self-hosted provider, so that a build
/// on a self-hosted model accepts it as its client override.
struct SelfHostedScripted(ScriptedCouncilClient);

#[async_trait::async_trait]
impl meerkat_client::LlmClient for SelfHostedScripted {
    fn project_replay_messages(
        &self,
        messages: &[Message],
    ) -> Result<Vec<Message>, meerkat_client::LlmError> {
        self.0.project_replay_messages(messages)
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> meerkat_client::types::LlmStream<'a> {
        self.0.stream(request)
    }

    fn provider(&self) -> Provider {
        Provider::SelfHosted
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        self.0.health_check().await
    }
}

/// Spawns the member `parent` of `mob_id` and runs its delegating turn to
/// completion.
async fn run_the_delegating_turn(state: &MobMcpState, mob_id: &MobId) {
    state
        .mob_spawn(
            mob_id,
            ProfileName::from("participant"),
            AgentIdentity::from("parent"),
            Some(meerkat_mob::MobRuntimeMode::TurnDriven),
            Some(MobBackendKind::Session),
            None,
        )
        .await
        .expect("spawn the session on the realm's model");
    let turn = state
        .handle_for(mob_id)
        .await
        .expect("mob handle")
        .member(&AgentIdentity::from("parent"))
        .await
        .expect("member handle")
        .start_turn(
            ContentInput::Text(DELEGATE_TASK.to_string()),
            HandlingMode::Queue,
            meerkat_mob::MemberTurnOptions::default(),
            None,
        )
        .await
        .expect("turn admitted");
    tokio::time::timeout(Duration::from_mins(2), turn.wait())
        .await
        .expect("the turn completes")
        .expect("the delegating turn succeeds");
}

/// The `delegate` profile of the implicit delegation mob of `parent`'s
/// session.
async fn implicit_delegate_profile(state: &MobMcpState, mob_id: &MobId) -> Profile {
    let session = state
        .handle_for(mob_id)
        .await
        .expect("mob handle")
        .resolve_bridge_session_id(&AgentIdentity::from("parent"))
        .await
        .expect("the delegating member's session");
    let implicit = state
        .find_implicit_mob_for_bridge_session(&session.to_string())
        .await
        .expect("the session's implicit delegation mob");
    state
        .handle_for(&implicit)
        .await
        .expect("implicit mob handle")
        .definition()
        .resolve_inline_profile(&ProfileName::from("delegate"))
        .cloned()
        .expect("the implicit mob's delegate profile")
}

/// The delegate result the delegating session's model got back, once it
/// ran its helper to completion.
fn completed_delegate_result(observed: &Observed) -> String {
    let result = observed
        .delegate_result
        .lock()
        .unwrap()
        .clone()
        .expect("the delegating session's model got the delegate result back");
    assert!(
        result.contains("\"status\":\"completed\"") && result.contains(HELPER_ANSWER),
        "a delegate from a session on a realm model outside the catalog runs its helper to \
         completion: {result}"
    );
    result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_on_a_realm_custom_model_delegates_to_a_helper_on_that_model() {
    let observed = Observed::default();
    let temp = tempfile::tempdir().expect("temp dir");
    let client = Arc::new(ScriptedCouncilClient::new(script(
        observed.clone(),
        REALM_MODEL,
    )));
    let state =
        support::agent_mob_tools_state_with_config(temp.path(), client, realm_config(), |state| {
            state
        });
    let mob_id = MobId::from(format!("realm-model-{}", uuid::Uuid::new_v4().simple()));
    state
        .mob_create_definition(parent_definition(
            &mob_id,
            REALM_MODEL,
            Provider::Anthropic,
            None,
        ))
        .await
        .expect("create the mob");
    run_the_delegating_turn(&state, &mob_id).await;

    let parent_models = observed.parent_models.lock().unwrap().clone();
    assert!(
        !parent_models.is_empty() && parent_models.iter().all(|model| model == REALM_MODEL),
        "the delegating session runs on the realm's model: {parent_models:?}"
    );
    completed_delegate_result(&observed);
    let helper_models = observed.helper_models.lock().unwrap().clone();
    assert_eq!(
        helper_models,
        [REALM_MODEL],
        "the helper runs on the delegating session's model"
    );
    // The session's implicit mob runs its `delegate` profile on the
    // session's model and on the provider its build resolved for it.
    let delegate = implicit_delegate_profile(&state, &mob_id).await;
    assert_eq!(
        (delegate.model.as_str(), delegate.provider),
        (REALM_MODEL, Some(Provider::Anthropic))
    );

    let _ = state.mob_destroy(&mob_id).await;
}

/// A session on a self-hosted model, pinned to a server other than the
/// model's configured one, delegates through the production factory-built
/// surface, and its implicit mob names the session's provider and the
/// pinned server, so its helpers run on the server the session runs on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_on_a_self_hosted_model_delegates_on_its_pinned_server() {
    let observed = Observed::default();
    let temp = tempfile::tempdir().expect("temp dir");
    let client = Arc::new(SelfHostedScripted(ScriptedCouncilClient::new(script(
        observed.clone(),
        SELF_HOSTED_MODEL,
    ))));
    let state = support::agent_mob_tools_state_with_config(
        temp.path(),
        client,
        self_hosted_config(),
        |state| state,
    );
    let mob_id = MobId::from(format!("self-hosted-{}", uuid::Uuid::new_v4().simple()));
    state
        .mob_create_definition(parent_definition(
            &mob_id,
            SELF_HOSTED_MODEL,
            Provider::SelfHosted,
            Some(PINNED_SERVER),
        ))
        .await
        .expect("create the mob");
    run_the_delegating_turn(&state, &mob_id).await;

    completed_delegate_result(&observed);
    let delegate = implicit_delegate_profile(&state, &mob_id).await;
    assert_eq!(
        (
            delegate.model.as_str(),
            delegate.provider,
            delegate.self_hosted_server_id.as_deref()
        ),
        (
            SELF_HOSTED_MODEL,
            Some(Provider::SelfHosted),
            Some(PINNED_SERVER)
        ),
        "the implicit mob names the session's provider and its pinned server"
    );

    let _ = state.mob_destroy(&mob_id).await;
}

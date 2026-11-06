//! A session on a realm custom model delegates.
//!
//! The realm's config defines a model the catalog does not know (a
//! `[models.<id>]` entry). A session runs on it, and its model turn calls
//! `delegate`. The session's implicit delegation mob runs its `delegate`
//! profile on the session's model, so the mob's definition must name the
//! provider the session's build resolved for that model: a definition naming
//! an uncatalogued model alone does not validate, and `delegate` fails.
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
use meerkat_core::{ContentInput, HandlingMode, Message};
use meerkat_mob::{
    AgentIdentity, MobBackendKind, MobDefinition, MobId, ProfileBinding, ProfileName,
};
use support::{ScriptedCouncilClient, ScriptedTurn, last_user_text, participant_profile};

/// A model id outside the catalog, defined only by the realm's config.
const REALM_MODEL: &str = "realm-gateway-model";
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
            provider: meerkat_core::Provider::Anthropic,
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

/// One member on the realm's model that mounts the agent mob tools. Its own
/// mob names the provider, as a mob definition must for a model outside the
/// catalog.
fn parent_definition(mob_id: &MobId) -> MobDefinition {
    let mut profile = participant_profile("delegating parent");
    profile.model = REALM_MODEL.to_string();
    profile.provider = Some(meerkat_core::Provider::Anthropic);
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

/// The delegating session calls `delegate` once and answers with its
/// result; the helper answers its task.
fn script(observed: Observed) -> impl Fn(&LlmRequest) -> ScriptedTurn + Send + Sync + 'static {
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
                        "model": REALM_MODEL,
                        "tools": { "comms": true }
                    }
                },
            }),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_on_a_realm_custom_model_delegates_to_a_helper_on_that_model() {
    let observed = Observed::default();
    let temp = tempfile::tempdir().expect("temp dir");
    let client = Arc::new(ScriptedCouncilClient::new(script(observed.clone())));
    let state =
        support::agent_mob_tools_state_with_config(temp.path(), client, realm_config(), |state| {
            state
        });
    let mob_id = MobId::from(format!("realm-model-{}", uuid::Uuid::new_v4().simple()));
    state
        .mob_create_definition(parent_definition(&mob_id))
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
        .expect("spawn the session on the realm's model");

    let turn = state
        .handle_for(&mob_id)
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

    let parent_models = observed.parent_models.lock().unwrap().clone();
    assert!(
        !parent_models.is_empty() && parent_models.iter().all(|model| model == REALM_MODEL),
        "the delegating session runs on the realm's model: {parent_models:?}"
    );
    let result = observed
        .delegate_result
        .lock()
        .unwrap()
        .clone()
        .expect("the delegating session's model got the delegate result back");
    assert!(
        result.contains("\"status\":\"completed\"") && result.contains(HELPER_ANSWER),
        "a delegate from a session on a realm custom model runs its helper to completion: \
         {result}"
    );
    let helper_models = observed.helper_models.lock().unwrap().clone();
    assert_eq!(
        helper_models,
        [REALM_MODEL],
        "the helper runs on the delegating session's model"
    );
    // The session's implicit mob runs its `delegate` profile on the
    // session's model and on the provider its build resolved for it.
    let handle = state.handle_for(&mob_id).await.expect("mob handle");
    let session = handle
        .resolve_bridge_session_id(&AgentIdentity::from("parent"))
        .await
        .expect("the delegating member's session");
    let implicit = state
        .find_implicit_mob_for_bridge_session(&session.to_string())
        .await
        .expect("the session's implicit delegation mob");
    let delegate = state
        .handle_for(&implicit)
        .await
        .expect("implicit mob handle")
        .definition()
        .resolve_inline_profile(&ProfileName::from("delegate"))
        .cloned()
        .expect("the implicit mob's delegate profile");
    assert_eq!(
        (delegate.model.as_str(), delegate.provider),
        (REALM_MODEL, Some(meerkat_core::Provider::Anthropic))
    );

    let _ = state.mob_destroy(&mob_id).await;
}

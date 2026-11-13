//! A mob profile sets its members' reasoning effort level.
//!
//! A profile names the provider-neutral level in its provider params
//! (`provider_params.reasoning_effort`). The member's session is built with
//! it, every provider request the member makes is sent with the level
//! resolved for the route that serves it (the provider's own field, never
//! the neutral one), and a level the route does not accept refuses the
//! member's build with the same typed data every other surface answers
//! with, before any provider request.
//!
//! End to end over the production runtime-backed session service: the
//! member is built by the production agent factory, as every surface builds
//! its sessions. Only the LLM is scripted; it reports the direct Anthropic
//! route, whose levels the model catalog states.
#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat_client::types::LlmStream;
use meerkat_client::{LlmClient, LlmError, LlmRequest};
use meerkat_core::lifecycle::run_primitive::ProviderTag;
use meerkat_core::{ContentInput, HandlingMode, Message, Provider};
use meerkat_mob::{
    AgentIdentity, MobBackendKind, MobDefinition, MobId, ProfileBinding, ProfileName,
};
use meerkat_mob_mcp::MobMcpState;
use support::{ScriptedCouncilClient, ScriptedTurn, last_user_text, participant_profile};

/// A catalog model whose direct route accepts `low`, `medium` and `high`.
const MODEL: &str = "claude-opus-4-5";
const FIRST_TASK: &str = "PROFILE-LEVEL-FIRST-TASK";
const SECOND_TASK: &str = "PROFILE-LEVEL-SECOND-TASK";

/// The scripted client on the direct Anthropic route. It records the
/// provider tag of every request that carries one of the member's tasks.
struct DirectRouteScripted {
    inner: ScriptedCouncilClient,
    member_request_tags: Arc<Mutex<Vec<Option<ProviderTag>>>>,
}

#[async_trait::async_trait]
impl LlmClient for DirectRouteScripted {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        self.inner.project_replay_messages(messages)
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        let user = last_user_text(request);
        if user.contains(FIRST_TASK) || user.contains(SECOND_TASK) {
            self.member_request_tags
                .lock()
                .unwrap()
                .push(request.provider_params.clone());
        }
        self.inner.stream(request)
    }

    fn provider(&self) -> Provider {
        Provider::Anthropic
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        self.inner.health_check().await
    }

    fn reasoning_route(&self) -> Option<meerkat_core::ReasoningRouteObservation> {
        meerkat_core::direct_route_observation(Provider::Anthropic)
    }
}

/// A mob whose `participant` profile is on [`MODEL`] and sets `level` the
/// way a profile does: in its provider params.
fn definition_with_profile_level(mob_id: &MobId, level: &str) -> MobDefinition {
    let mut profile = participant_profile("member with a reasoning effort level");
    profile.model = MODEL.to_string();
    profile.provider = Some(Provider::Anthropic);
    profile.provider_params = Some(
        serde_json::from_value(serde_json::json!({ "reasoning_effort": level }))
            .expect("a profile's provider params name the level"),
    );
    // The test drives the member's turns from outside the mob.
    profile.external_addressable = true;
    let mut definition = MobDefinition::explicit(mob_id.clone());
    definition.profiles.insert(
        ProfileName::from("participant"),
        ProfileBinding::Inline(Box::new(profile)),
    );
    definition
}

struct Fixture {
    state: Arc<MobMcpState>,
    mob_id: MobId,
    member_request_tags: Arc<Mutex<Vec<Option<ProviderTag>>>>,
    provider_calls: Arc<std::sync::atomic::AtomicUsize>,
    _temp: tempfile::TempDir,
}

async fn mob_with_profile_level(level: &str) -> Fixture {
    let temp = tempfile::tempdir().expect("temp dir");
    let inner = ScriptedCouncilClient::new(|_request| ScriptedTurn::Text("ok".to_string()));
    let provider_calls = inner.calls();
    let member_request_tags = Arc::new(Mutex::new(Vec::new()));
    let client = Arc::new(DirectRouteScripted {
        inner,
        member_request_tags: Arc::clone(&member_request_tags),
    });
    let state = support::agent_mob_tools_state(temp.path(), client, |state| state);
    let mob_id = MobId::from(format!("profile-level-{}", uuid::Uuid::new_v4().simple()));
    state
        .mob_create_definition(definition_with_profile_level(&mob_id, level))
        .await
        .expect("create the mob");
    Fixture {
        state,
        mob_id,
        member_request_tags,
        provider_calls,
        _temp: temp,
    }
}

async fn spawn_member(
    fixture: &Fixture,
) -> Result<meerkat_mob::SpawnResult, meerkat_mob::MobError> {
    fixture
        .state
        .mob_spawn(
            &fixture.mob_id,
            ProfileName::from("participant"),
            AgentIdentity::from("member"),
            Some(meerkat_mob::MobRuntimeMode::TurnDriven),
            Some(MobBackendKind::Session),
            None,
        )
        .await
}

async fn run_member_turn(fixture: &Fixture, task: &str) {
    let turn = fixture
        .state
        .handle_for(&fixture.mob_id)
        .await
        .expect("mob handle")
        .member(&AgentIdentity::from("member"))
        .await
        .expect("member handle")
        .start_turn(
            ContentInput::Text(task.to_string()),
            HandlingMode::Queue,
            meerkat_mob::MemberTurnOptions::default(),
            None,
        )
        .await
        .expect("turn admitted");
    tokio::time::timeout(Duration::from_mins(2), turn.wait())
        .await
        .expect("the turn completes")
        .expect("the member's turn succeeds");
}

/// A level the member's route accepts is sent on every request the member
/// makes, as the provider's own field: each request is resolved for the
/// route that serves it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_profile_level_is_resolved_for_each_of_the_members_requests() {
    let fixture = mob_with_profile_level("high").await;
    spawn_member(&fixture)
        .await
        .expect("a level the route accepts builds the member");
    run_member_turn(&fixture, FIRST_TASK).await;
    run_member_turn(&fixture, SECOND_TASK).await;

    let tags = fixture.member_request_tags.lock().unwrap().clone();
    assert_eq!(
        tags.len(),
        2,
        "one provider request for each of the member's two turns: {tags:?}"
    );
    for tag in &tags {
        let tag = serde_json::to_value(tag).expect("a provider tag serializes");
        assert_eq!(
            (tag["provider"].as_str(), tag["effort"].as_str()),
            (Some("anthropic"), Some("high")),
            "every request of the member carries the profile's level in the provider's own \
             field: {tag}"
        );
    }

    let _ = fixture.state.mob_destroy(&fixture.mob_id).await;
}

/// A level the member's route does not accept refuses the member with the
/// typed refusal, and no provider request is made.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_profile_level_the_route_refuses_is_the_typed_refusal() {
    let fixture = mob_with_profile_level("max").await;
    let error = match spawn_member(&fixture).await {
        Err(error) => error,
        Ok(_) => panic!("a member whose profile names a level its route refuses must not build"),
    };
    assert_eq!(
        error.structured_data(),
        Some(serde_json::json!({
            "kind": "reasoning_effort_refused",
            "retryable": false,
            "reasoning_effort_refusal": {
                "requested": "max", "provider": "anthropic", "model": MODEL,
                "route": {"backend": "anthropic_api", "api": "messages"},
                "reason": "unsupported_level",
                "supported": ["low", "medium", "high"], "facts": "catalog",
            },
        })),
        "the refused member build carries the typed refusal: {error}"
    );
    assert_eq!(
        fixture.provider_calls.load(Ordering::SeqCst),
        0,
        "a refused level makes no provider request"
    );

    let _ = fixture.state.mob_destroy(&fixture.mob_id).await;
}

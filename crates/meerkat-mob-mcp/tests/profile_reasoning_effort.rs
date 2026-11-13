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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat_client::types::LlmStream;
use meerkat_client::{LlmClient, LlmError, LlmRequest};
use meerkat_core::lifecycle::run_primitive::ProviderTag;
use meerkat_core::model_profile::capabilities::EffortLevel;
use meerkat_core::{ContentInput, HandlingMode, Message, Provider, SessionId};
use meerkat_mob::{
    AgentIdentity, BoundedResultSpec, MemberRespawn, MemberRestorePolicy, MobBackendKind,
    MobControlPrincipal, MobDefinition, MobError, MobId, MobMemberStatus, MobState, ProfileBinding,
    ProfileName, WorkOrigin, WorkSpec,
};
use meerkat_mob_mcp::{MobMcpState, MobRestoreRetryOutcome};
use support::{
    ScriptedCouncilClient, ScriptedTurn, last_user_text, participant_profile, user_text,
};

/// A catalog model whose direct route accepts `low`, `medium` and `high`.
const MODEL: &str = "claude-opus-4-5";
const FIRST_TASK: &str = "PROFILE-LEVEL-FIRST-TASK";
const SECOND_TASK: &str = "PROFILE-LEVEL-SECOND-TASK";

/// The scripted client on the direct Anthropic route. It records the
/// provider tag of every request that carries one of the member's tasks.
struct DirectRouteScripted {
    inner: ScriptedCouncilClient,
    member_request_tags: Arc<Mutex<Vec<Option<ProviderTag>>>>,
    /// The user text of each recorded request: the member's conversation as
    /// the provider is sent it.
    member_request_histories: Arc<Mutex<Vec<String>>>,
    /// While set, the client reports a route whose account states `low`
    /// only, as an account whose model list changed does.
    route_refuses: Arc<AtomicBool>,
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
            self.member_request_histories
                .lock()
                .unwrap()
                .push(user_text(request));
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
        let mut route = meerkat_core::direct_route_observation(Provider::Anthropic)?;
        if self.route_refuses.load(Ordering::SeqCst) {
            route.backend = meerkat_core::ReasoningBackend::Anthropic(
                meerkat_core::provider_matrix::AnthropicBackendKind::Copilot,
            );
            route.account = meerkat_core::AccountLevelStatement::Stated(vec![EffortLevel::Low]);
        }
        Some(route)
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
        member_request_histories: Arc::default(),
        route_refuses: Arc::default(),
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

/// One process lifetime over the durable stores of a restart row: a fresh
/// session service and mob state, as a restarted process composes them.
struct Lifetime {
    state: Arc<MobMcpState>,
    service: Arc<meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder>>,
}

/// Start a lifetime over `root` and `runtime_store`: the production agent
/// factory and persistent session service, with only the LLM scripted, and a
/// mob state whose mobs are stored under `root`.
fn start_lifetime(
    root: &std::path::Path,
    runtime_store: Arc<dyn meerkat_runtime::RuntimeStore>,
    client: Arc<dyn LlmClient>,
    restore_policy: MemberRestorePolicy,
) -> Lifetime {
    let project_root = root.join("project-root");
    let context_root = root.join("context-root");
    for dir in [&project_root, &context_root] {
        std::fs::create_dir_all(dir).expect("create the project and context roots");
    }
    let factory = meerkat::AgentFactory::new(root.join("factory-store"))
        .user_config_root(root.join("user-config"))
        .runtime_root(root.join("runtime-root"))
        .project_root(project_root)
        .context_root(context_root)
        .builtins(false)
        .comms(true);
    let mut builder = meerkat::FactoryAgentBuilder::new(factory, meerkat::Config::default());
    builder.default_llm_client = Some(client);
    let store = Arc::new(meerkat_store::JsonlStore::new(root.join("sessions-jsonl")));
    builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(store.clone())));
    let store: Arc<dyn meerkat::SessionStore> = store;
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::default());
    let service = Arc::new(meerkat_session::PersistentSessionService::new(
        builder,
        32,
        store,
        runtime_store,
        blob_store,
    ));
    let state = MobMcpState::new(service.clone(), MobControlPrincipal::Owner)
        .expect("mob state")
        .with_member_restore_policy(restore_policy)
        .try_with_persistent_storage_root(Some(root.join("state")))
        .expect("open the persistent mob root")
        .into_shared();
    Lifetime { state, service }
}

/// End a lifetime as a stopping process does: its mobs are shut down, not
/// destroyed, and its sessions are shut down.
async fn end_lifetime(lifetime: Lifetime) {
    let handles = lifetime
        .state
        .mob_handles_snapshot()
        .await
        .expect("the lifetime's mobs");
    for (_, handle) in handles {
        handle.shutdown().await.expect("shut the mob down");
    }
    lifetime
        .service
        .try_shutdown()
        .await
        .expect("shut the sessions down");
}

/// Run one turn of the member on `task`. `Err` is the refusal of the turn's
/// admission; an admitted turn must succeed.
async fn run_member_work(state: &MobMcpState, mob_id: &MobId, task: &str) -> Result<(), MobError> {
    let spec = BoundedResultSpec::new("turn", 4096).expect("bounded result spec");
    let work = state
        .handle_for(mob_id)
        .await?
        .start_work_for_identity_bounded(
            AgentIdentity::from("member"),
            WorkSpec::new(ContentInput::Text(task.to_string()), WorkOrigin::Internal),
            HandlingMode::Queue,
            spec.clone(),
        )
        .await?;
    tokio::time::timeout(Duration::from_mins(2), work.wait_bounded(spec))
        .await
        .expect("the turn completes")
        .expect("the member's turn succeeds");
    Ok(())
}

/// The mob is restored and managed in this lifetime: listed as running, with
/// a running actor, and no mob store is quarantined.
async fn assert_mob_restored(state: &MobMcpState, mob_id: &MobId, when: &str) {
    let listed = state.mob_list().await.expect("list the mobs");
    let Some((_, observation)) = listed.iter().find(|(id, _)| id == mob_id) else {
        panic!("{when}: the mob is restored and managed, got: {listed:?}");
    };
    assert_eq!(observation.phase, MobState::Running, "{when}: the mob runs");
    assert!(
        observation.failure.is_none(),
        "{when}: the mob's actor runs: {:?}",
        observation.failure
    );
    let quarantined = state
        .quarantined_mobs()
        .await
        .expect("read the quarantined mobs");
    assert!(
        quarantined.is_empty(),
        "{when}: no mob store is quarantined: {quarantined:?}"
    );
}

/// The member's status and recorded error in the mob's member list.
async fn member_standing(state: &MobMcpState, mob_id: &MobId) -> (MobMemberStatus, Option<String>) {
    let row = state
        .handle_for(mob_id)
        .await
        .expect("the mob's handle")
        .list_members()
        .await
        .into_iter()
        .find(|row| row.agent_identity.as_str() == "member")
        .expect("the member is in the mob's member list");
    (row.status, row.error)
}

/// The typed refusal of the profile's `high` level on the route whose
/// account states `low` only.
fn refusal_on_the_changed_route() -> serde_json::Value {
    serde_json::json!({
        "kind": "reasoning_effort_refused",
        "retryable": false,
        "reasoning_effort_refusal": {
            "requested": "high", "provider": "anthropic", "model": MODEL,
            "route": {"backend": "copilot", "api": "messages"},
            "reason": "unsupported_level",
            "supported": ["low"], "facts": "account",
        },
    })
}

/// `error` is the member's restore failure for the refused level: it names
/// the member and its own session, is no durable resume hold, and keeps the
/// typed refusal as its cause data, while its own structured data is empty.
/// Returns the failure's reason text.
fn assert_restore_refused(error: &MobError, session: &SessionId, when: &str) -> String {
    let MobError::MemberRestoreFailed {
        member_id,
        session_id,
        reason,
        hold,
        cause_data,
    } = error
    else {
        panic!("{when}: expected the member's restore failure, got: {error:?}");
    };
    assert_eq!(
        member_id,
        &AgentIdentity::from("member"),
        "{when}: the failure names the member: {error}"
    );
    assert_eq!(
        session_id.as_ref(),
        Some(session),
        "{when}: the failure names the member's own session: {error}"
    );
    assert!(
        hold.is_none(),
        "{when}: the failure is no durable resume hold: {hold:?}"
    );
    assert_eq!(
        cause_data.as_ref(),
        Some(&refusal_on_the_changed_route()),
        "{when}: the failure keeps the typed refusal as its cause data: {reason}"
    );
    assert_eq!(
        error.structured_data(),
        None,
        "{when}: the typed refusal is the failure's cause data only"
    );
    reason.clone()
}

/// A level the member's route accepted at the spawn and refuses at the
/// resume (the account's model list changed between the two lifetimes)
/// breaks that member only, with the typed refusal, and nothing else:
///
/// - the restarted mob is restored and runs, under both restore policies;
///   its store is not quarantined, and the member keeps its seat and its
///   session;
/// - with members restored on demand the member is dormant until its first
///   wake, and that wake answers `MobError::MemberRestoreFailed` whose
///   `cause_data` is the typed refusal; with every member restored at the
///   start the member is recorded with the same failure at the start;
/// - no provider request is made for the refused build;
/// - the failure is answered from the member's record for the rest of the
///   lifetime: once the route accepts the level again, a wake, a retry of
///   the mob's restore and the mob's stop and resume all leave it;
/// - a restart while the route accepts the level seats the member on its
///   own session, with its conversation and its level, and so does, without
///   a restart, a respawn of the member on its own session
///   (`MobHandle::respawn_member`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_profile_level_the_route_refuses_at_resume_breaks_only_that_member() {
    let temp = tempfile::tempdir().expect("temp dir");
    let root = temp.path();
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
    let inner = ScriptedCouncilClient::new(|_request| ScriptedTurn::Text("ok".to_string()));
    let provider_calls = inner.calls();
    let tags = Arc::new(Mutex::new(Vec::new()));
    let histories = Arc::new(Mutex::new(Vec::new()));
    let route_refuses = Arc::new(AtomicBool::new(false));
    let client: Arc<dyn LlmClient> = Arc::new(DirectRouteScripted {
        inner,
        member_request_tags: Arc::clone(&tags),
        member_request_histories: Arc::clone(&histories),
        route_refuses: Arc::clone(&route_refuses),
    });
    let mob_id = MobId::from(format!("profile-level-{}", uuid::Uuid::new_v4().simple()));
    let member = AgentIdentity::from("member");
    let start = |restore_policy| {
        start_lifetime(root, runtime_store.clone(), client.clone(), restore_policy)
    };

    // First lifetime: the route accepts the level. The member is spawned and
    // runs one turn, so its session is stored with the level.
    let life = start(MemberRestorePolicy::OnDemand);
    life.state
        .mob_create_definition(definition_with_profile_level(&mob_id, "high"))
        .await
        .expect("create the mob");
    life.state
        .mob_spawn(
            &mob_id,
            ProfileName::from("participant"),
            member.clone(),
            Some(meerkat_mob::MobRuntimeMode::TurnDriven),
            Some(MobBackendKind::Session),
            None,
        )
        .await
        .expect("a level the route accepts builds the member");
    let session = life
        .state
        .handle_for(&mob_id)
        .await
        .expect("the mob's handle")
        .resolve_bridge_session_id(&member)
        .await
        .expect("the member's session");
    if let Err(error) = run_member_work(&life.state, &mob_id, FIRST_TASK).await {
        panic!("the member's first turn is admitted: {error:?}");
    }
    assert_eq!(provider_calls.load(Ordering::SeqCst), 1);
    end_lifetime(life).await;

    // Second lifetime: the route refuses the level, members restored on
    // demand. The mob is restored whole and the member is dormant.
    route_refuses.store(true, Ordering::SeqCst);
    let life = start(MemberRestorePolicy::OnDemand);
    assert_mob_restored(&life.state, &mob_id, "a restart while the route refuses").await;
    let handle = life
        .state
        .handle_for(&mob_id)
        .await
        .expect("the restored mob's handle");
    assert!(
        handle.member_unmaterialized(&member),
        "the restored member is dormant until its first wake"
    );
    assert_eq!(
        member_standing(&life.state, &mob_id).await,
        (MobMemberStatus::Active, None),
        "a dormant member is listed as before the restart"
    );
    assert_eq!(
        handle.resolve_bridge_session_id(&member).await,
        Some(session.clone()),
        "the restored member keeps its session"
    );

    // Its wake is the member's own failure, with the typed refusal.
    let error = match handle.ensure_member_live(&member).await {
        Err(error) => error,
        Ok(()) => panic!("a member whose route refuses its level must not be built"),
    };
    let reason = assert_restore_refused(&error, &session, "the member's first wake");
    assert_eq!(
        member_standing(&life.state, &mob_id).await,
        (MobMemberStatus::Broken, Some(reason)),
        "the member is listed as broken, with the failure's reason"
    );
    assert!(
        !handle.member_unmaterialized(&member),
        "a broken member is not a dormant one"
    );
    // Every later answer for the member is the same failure, from its record.
    let error = match handle.member(&member).await {
        Err(error) => error,
        Ok(_) => panic!("no member handle is given for a broken member"),
    };
    assert_restore_refused(&error, &session, "a member handle");
    let error = match run_member_work(&life.state, &mob_id, SECOND_TASK).await {
        Err(error) => error,
        Ok(()) => panic!("no turn is admitted for a broken member"),
    };
    assert_restore_refused(&error, &session, "a turn's admission");
    // Nothing else changed: the mob runs, unquarantined, the member has its
    // seat and its session, and no provider request was made.
    assert_mob_restored(&life.state, &mob_id, "after the refused wake").await;
    assert_eq!(
        handle.resolve_bridge_session_id(&member).await,
        Some(session.clone()),
        "the broken member keeps its seat and its session"
    );
    assert_eq!(
        provider_calls.load(Ordering::SeqCst),
        1,
        "a refused level makes no provider request"
    );

    // The route accepts the level again in the same lifetime: the member's
    // recorded failure stays. A wake, a retry of the mob's restore and the
    // mob's stop and resume do not build the member again.
    route_refuses.store(false, Ordering::SeqCst);
    let error = match handle.ensure_member_live(&member).await {
        Err(error) => error,
        Ok(()) => panic!("a wake of a broken member answers its recorded failure"),
    };
    assert_restore_refused(&error, &session, "a wake once the route accepts again");
    assert_eq!(
        life.state
            .mob_restore_retry(&mob_id)
            .await
            .expect("the restore retry answers"),
        MobRestoreRetryOutcome::NotQuarantined,
        "the mob was never quarantined, so there is no restore to retry"
    );
    life.state.mob_stop(&mob_id).await.expect("stop the mob");
    life.state
        .mob_resume(&mob_id)
        .await
        .expect("the mob's resume succeeds beside a broken member");
    let error = match handle.ensure_member_live(&member).await {
        Err(error) => error,
        Ok(()) => panic!("the mob's stop and resume do not rebuild a broken member"),
    };
    assert_restore_refused(&error, &session, "a wake after the mob's stop and resume");
    assert_eq!(provider_calls.load(Ordering::SeqCst), 1);
    drop(handle);
    end_lifetime(life).await;

    // Third lifetime: the route refuses the level, every member restored at
    // the start. The mob still starts whole; the member is broken at once.
    route_refuses.store(true, Ordering::SeqCst);
    let life = start(MemberRestorePolicy::Eager);
    assert_mob_restored(
        &life.state,
        &mob_id,
        "an eager restart while the route refuses",
    )
    .await;
    let handle = life
        .state
        .handle_for(&mob_id)
        .await
        .expect("the restored mob's handle");
    let error = match handle.member(&member).await {
        Err(error) => error,
        Ok(_) => panic!("a member whose route refuses its level is not built at the start"),
    };
    let reason = assert_restore_refused(&error, &session, "an eager start");
    assert_eq!(
        member_standing(&life.state, &mob_id).await,
        (MobMemberStatus::Broken, Some(reason)),
        "the member is listed as broken at the start"
    );
    assert_eq!(provider_calls.load(Ordering::SeqCst), 1);
    drop(handle);
    end_lifetime(life).await;

    // Fourth lifetime: the route accepts the level again. The restart seats
    // the member on its own session, with its conversation and its level.
    route_refuses.store(false, Ordering::SeqCst);
    let life = start(MemberRestorePolicy::OnDemand);
    assert_mob_restored(
        &life.state,
        &mob_id,
        "a restart once the route accepts again",
    )
    .await;
    let handle = life
        .state
        .handle_for(&mob_id)
        .await
        .expect("the restored mob's handle");
    assert_eq!(
        member_standing(&life.state, &mob_id).await,
        (MobMemberStatus::Active, None),
        "the earlier failure is not carried into the next lifetime"
    );
    assert_eq!(
        handle.resolve_bridge_session_id(&member).await,
        Some(session.clone()),
        "the member is still seated on its own session"
    );
    if let Err(error) = handle.ensure_member_live(&member).await {
        panic!("the member is built once its route accepts the level: {error:?}");
    }
    if let Err(error) = run_member_work(&life.state, &mob_id, SECOND_TASK).await {
        panic!("the restored member's turn is admitted: {error:?}");
    }
    assert_eq!(
        *histories.lock().unwrap(),
        [
            FIRST_TASK.to_string(),
            format!("{FIRST_TASK}\n{SECOND_TASK}")
        ],
        "the restored member's request carries the conversation of its first lifetime"
    );
    let sent = tags.lock().unwrap().clone();
    assert_eq!(
        sent.len(),
        2,
        "one provider request for each turn: {sent:?}"
    );
    for tag in &sent {
        let tag = serde_json::to_value(tag).expect("a provider tag serializes");
        assert_eq!(
            (tag["provider"].as_str(), tag["effort"].as_str()),
            (Some("anthropic"), Some("high")),
            "the member's requests carry the profile's level before and after: {tag}"
        );
    }
    drop(handle);
    end_lifetime(life).await;

    // Fifth lifetime: the route refuses the level, the member's wake fails,
    // and the route then accepts the level again. Without a restart, a
    // respawn of the member on its own session builds it again.
    route_refuses.store(true, Ordering::SeqCst);
    let life = start(MemberRestorePolicy::OnDemand);
    let handle = life
        .state
        .handle_for(&mob_id)
        .await
        .expect("the restored mob's handle");
    let error = match handle.ensure_member_live(&member).await {
        Err(error) => error,
        Ok(()) => panic!("a member whose route refuses its level must not be built"),
    };
    assert_restore_refused(&error, &session, "the wake before the respawn");
    route_refuses.store(false, Ordering::SeqCst);
    let receipt = match handle
        .respawn_member(member.clone(), MemberRespawn::new().with_wake(true))
        .await
    {
        Ok(receipt) => receipt,
        Err(error) => panic!("a broken member is respawned on its own session: {error:?}"),
    };
    assert!(
        receipt.wake_error().is_none(),
        "the respawned member is built: {:?}",
        receipt.wake_error()
    );
    assert_eq!(
        member_standing(&life.state, &mob_id).await,
        (MobMemberStatus::Active, None),
        "the respawn ends the member's recorded failure"
    );
    assert_eq!(
        handle.resolve_bridge_session_id(&member).await,
        Some(session),
        "the respawned member keeps its session"
    );
    if let Err(error) = run_member_work(&life.state, &mob_id, SECOND_TASK).await {
        panic!("the respawned member's turn is admitted: {error:?}");
    }
    assert_eq!(
        histories.lock().unwrap().last(),
        Some(&format!("{FIRST_TASK}\n{SECOND_TASK}\n{SECOND_TASK}")),
        "the respawned member's request carries its whole conversation"
    );
    assert_eq!(provider_calls.load(Ordering::SeqCst), 3);
    drop(handle);

    let _ = life.state.mob_destroy(&mob_id).await;
}

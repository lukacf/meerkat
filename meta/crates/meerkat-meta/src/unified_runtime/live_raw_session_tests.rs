//! Rows of a live request that names its member by a raw `session_id`, on a
//! host with two mobs.
//!
//! The host has one live transport, composed for the launch mob. A request
//! that names a mob is judged before the live arm runs; a raw session id
//! names none, so the session's own stored member binding says whose member
//! it is. The fixture is a launch that declares its mob to its mob set: the
//! set builds a second mob of the host through the launch's composition, so
//! the members of both mobs have their sessions in the launch's one session
//! service and one runtime machine. The external live door is composed over
//! that pair by hand, with the scripted realtime session factory: no row
//! sends a request to a provider.

#![allow(clippy::expect_used)]

use std::sync::Arc;
use std::time::Duration;

use meerkat::test_fixtures::realtime::ScriptedRealtimeSessionFactory;
use meerkat_core::service::SessionService as _;
use meerkat_core::types::SessionId;
use meerkat_mob::{AgentIdentity, MobDefinition, MobId, SpawnMemberSpec};
use serde_json::{Value, json};

use super::UnifiedRuntime;
use super::mob_entries_tests::{Sessions, launch_composed, second_mob, set_of, shut_down};
use crate::console_voice::live_host::tests::{SHARED_HOST_TEST_LOCK, config};
use crate::live_wiring::LiveRpcHandler;

/// The one name both mobs' members carry.
const MEMBER: &str = "desk";

/// A definition with one profile under an id of its own.
fn definition(tag: &str) -> MobDefinition {
    MobDefinition::from_toml(&format!(
        "[mob]\nid = \"live-raw-session-{tag}-{}\"\n\n[profiles.agent]\nmodel = \"gpt-5.5\"\nexternal_addressable = true\n\n[profiles.agent.tools]\ncomms = true\n",
        uuid::Uuid::new_v4().simple()
    ))
    .expect("a definition with one profile")
}

/// A host with its launch mob and a second mob its mob set built, a member
/// of one name in each, and the external live door over the launch's
/// session service and runtime machine.
struct TwoMobs {
    _temp: tempfile::TempDir,
    host: Arc<UnifiedRuntime>,
    sessions: Sessions,
    machine: Arc<meerkat_runtime::MeerkatMachine>,
    live: LiveRpcHandler,
    realtime: Arc<ScriptedRealtimeSessionFactory>,
    launch_mob: MobId,
    second_mob: MobId,
    launch_session: SessionId,
    second_session: SessionId,
}

/// The mob the stored member binding of `session` names, if the session
/// has one.
async fn stored_mob_of(host: &UnifiedRuntime, session: &SessionId) -> Option<String> {
    host.mob_runtime()
        .session_service()
        .expect("the launch's session service")
        .load_persisted_session_metadata(session)
        .await
        .expect("the session's stored metadata reads")
        .filter(|view| view.session_id == *session)
        .and_then(|view| view.session_metadata)
        .and_then(|metadata| metadata.mob_member_binding)
        .map(|binding| binding.mob_id)
}

impl TwoMobs {
    /// The host: its requests resolve the second mob, as every launch's
    /// do, and that mob's member is seated through the resolved mob.
    async fn launch(tag: &str) -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let (host, sessions, machine, ()) = launch_composed(
            &temp,
            definition(&format!("{tag}-launch")),
            config(),
            |_| (),
            |spec| spec,
        )
        .await;
        let launch_mob = host.mob_handle().mob_id().clone();
        let second_mob = second_mob(&host, definition(&format!("{tag}-second"))).await;
        let second = set_of(&host)
            .handle_for(&second_mob)
            .await
            .expect("setup: the set serves the second mob");
        Box::pin(host.spawn(SpawnMemberSpec::new("agent", MEMBER)))
            .await
            .expect("setup: the launch mob's member");
        let member = AgentIdentity::from(MEMBER);
        let resolved = host
            .resolve_mob(Some(&second_mob))
            .await
            .expect("setup: the second mob of the host resolves");
        Box::pin(resolved.spawn(SpawnMemberSpec::new("agent", MEMBER)))
            .await
            .expect("setup: the second mob's member");
        let launch_session = host
            .mob_handle()
            .resolve_bridge_session_id(&member)
            .await
            .expect("setup: the launch mob's member has a session");
        let second_session = second
            .resolve_bridge_session_id(&member)
            .await
            .expect("setup: the second mob's member has a session");
        assert_ne!(
            launch_session, second_session,
            "setup: two members, two sessions"
        );
        assert_eq!(
            stored_mob_of(&host, &launch_session).await.as_deref(),
            Some(launch_mob.as_str()),
            "setup: the launch mob's member is bound to the launch mob in its stored session"
        );
        assert_eq!(
            stored_mob_of(&host, &second_session).await.as_deref(),
            Some(second_mob.as_str()),
            "setup: the second mob's member is bound to the second mob in its stored session"
        );

        let realtime = Arc::new(ScriptedRealtimeSessionFactory::new());
        let factory = meerkat::AgentFactory::new(temp.path()).builtins(false);
        let mut context = crate::live_wiring::attach_live(
            Arc::clone(&sessions),
            Arc::clone(&machine),
            &factory,
            config(),
            "ws://127.0.0.1/live-raw-session".to_string(),
            None,
        );
        context.session_factory = Arc::clone(&realtime)
            as Arc<dyn meerkat_client::realtime_session::RealtimeSessionFactory>;
        let live = crate::live_wiring::live_rpc_handler(
            Arc::new(context),
            Arc::clone(&sessions),
            Arc::clone(&machine),
        );
        Self {
            _temp: temp,
            host: Arc::new(host),
            sessions,
            machine,
            live,
            realtime,
            launch_mob,
            second_mob,
            launch_session,
            second_session,
        }
    }

    /// One request on the stdio dispatch of the host, with its live door,
    /// as the gateway's writer sends it: the answer.
    async fn stdio(&self, method: &str, params: Value) -> Value {
        let request =
            json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string();
        let mut delivery = crate::rpc::handle_unified_rpc_json_with_live_arc_delivery(
            &self.host,
            &request,
            Duration::from_secs(30),
            None,
            None,
            Some(&self.live),
        )
        .await;
        let answer = serde_json::from_str(&delivery.response).expect("a JSON-RPC answer");
        delivery
            .settle_delivery(true)
            .await
            .expect("the writer settles");
        answer
    }

    /// A live open that names its member by `session` alone.
    async fn open_by_session(&self, session: &SessionId) -> Value {
        self.stdio(
            "meta/live/open",
            json!({"session_id": session.to_string(), "model": "gpt-realtime-2"}),
        )
        .await
    }

    /// A mob that the member whose session is `owner` created, with a member
    /// of its own: the mob's id and that member's session. The mob set
    /// records the mob as created by a member of the owner's mob.
    async fn child_mob_of(&self, owner: &SessionId, tag: &str) -> (MobId, SessionId) {
        let set = set_of(&self.host);
        let child = Box::pin(set.mob_create_definition_with_owner_bridge_session(
            definition(tag),
            owner.clone(),
            true,
            false,
        ))
        .await
        .expect("setup: a mob the member owns");
        let handle = set
            .handle_for(&child)
            .await
            .expect("setup: the set manages the member's mob");
        let member = AgentIdentity::from(MEMBER);
        Box::pin(handle.spawn_spec(SpawnMemberSpec::new("agent", MEMBER)))
            .await
            .expect("setup: a member of the member's mob");
        handle
            .ensure_member_live(&member)
            .await
            .expect("setup: that member has a live session");
        let session = handle
            .resolve_bridge_session_id(&member)
            .await
            .expect("setup: that member has a session");
        assert_eq!(
            stored_mob_of(&self.host, &session).await.as_deref(),
            Some(child.as_str()),
            "setup: the member is bound to the member's mob in its stored session"
        );
        (child, session)
    }

    /// Close the channel `answer` opened, if it opened one, and shut the
    /// host down. Nothing here is a row's assertion.
    async fn end(self, answer: &Value) {
        self.end_with(answer, &[]).await;
    }

    /// [`Self::end`], for a row that also made the mobs `others`.
    async fn end_with(self, answer: &Value, others: &[&MobId]) {
        if let Some(channel) = answer["result"]["channel_id"].as_str() {
            let _ = self
                .stdio("meta/live/close", json!({"channel_id": channel}))
                .await;
        }
        let mut mobs = others.to_vec();
        mobs.push(&self.second_mob);
        shut_down(&self.host, &mobs).await;
    }
}

/// The whole error of the stdio dispatch for a live method asked for a mob
/// that is not the launch mob: the refusal of a mob id with both ids, the
/// member `hosted`, and a text that says the method serves the launch mob
/// alone. A request that names the mob by `mob_id` is answered this.
fn serves_the_launch_mob_only(method: &str, requested: &MobId, launch_mob: &MobId) -> Value {
    let (requested, launch_mob) = (requested.as_str(), launch_mob.as_str());
    json!({
        "code": -32602,
        "message": format!(
            "mob '{requested}' is hosted on this gateway, but {method} serves the launch mob \
             '{launch_mob}' only"
        ),
        "data": {
            "kind": "foreign_mob_target",
            "requested_mob_id": requested,
            "gateway_mob_id": launch_mob,
            "hosted": true,
        },
    })
}

/// A live open that names no mob and the session of a member of the host's
/// second mob is refused as a live open that names that mob is: with the
/// refusal of a method that serves the launch mob alone. Nothing is opened:
/// the realtime lane sees no open and the session owns no channel.
#[tokio::test]
async fn a_live_open_by_the_session_of_another_mobs_member_is_refused() {
    let _guard = SHARED_HOST_TEST_LOCK.lock().await;
    let host = TwoMobs::launch("refused").await;

    let by_mob_id = host
        .stdio(
            "meta/live/open",
            json!({"mob_id": host.second_mob.as_str(), "identity": MEMBER, "model": "gpt-realtime-2"}),
        )
        .await;
    let before = host.realtime.open_count();
    let answer = host.open_by_session(&host.second_session).await;
    let after = host.realtime.open_count();
    let owns_a_channel = host
        .machine
        .session_live_channel_active(&host.second_session)
        .await;
    let expected = serves_the_launch_mob_only("meta/live/open", &host.second_mob, &host.launch_mob);
    let second_session = host.second_session.clone();
    host.end(&answer).await;

    assert_eq!(
        by_mob_id["error"], expected,
        "control: the open that names the second mob is refused: {by_mob_id}"
    );
    assert_eq!(
        answer["error"], expected,
        "the open that names the second mob's member by its session {second_session} is \
         refused in the same words: {answer}"
    );
    assert_eq!(
        (before, after),
        (0, 0),
        "the realtime lane saw no open: {answer}"
    );
    assert!(
        !owns_a_channel,
        "the second mob's member session owns no live channel: {answer}"
    );
}

/// The same refusal when the request also names a member of the launch mob:
/// the raw session id is what the open would act on, so it is what is
/// judged.
#[tokio::test]
async fn a_live_open_by_another_mobs_session_is_refused_beside_a_launch_mob_identity() {
    let _guard = SHARED_HOST_TEST_LOCK.lock().await;
    let host = TwoMobs::launch("refused-beside-identity").await;

    let answer = host
        .stdio(
            "meta/live/open",
            json!({
                "identity": MEMBER,
                "session_id": host.second_session.to_string(),
                "model": "gpt-realtime-2",
            }),
        )
        .await;
    let opens = host.realtime.open_count();
    let (second_owns, launch_owns) = (
        host.machine
            .session_live_channel_active(&host.second_session)
            .await,
        host.machine
            .session_live_channel_active(&host.launch_session)
            .await,
    );
    let expected = serves_the_launch_mob_only("meta/live/open", &host.second_mob, &host.launch_mob);
    host.end(&answer).await;

    assert_eq!(
        answer["error"], expected,
        "the open is refused for the session it would act on: {answer}"
    );
    assert_eq!(opens, 0, "the realtime lane saw no open: {answer}");
    assert!(
        !second_owns && !launch_owns,
        "neither member's session owns a live channel: {answer}"
    );
}

/// The launch mob's own member is still opened by its raw session id.
#[tokio::test]
async fn a_live_open_by_the_session_of_a_launch_mob_member_is_served() {
    let _guard = SHARED_HOST_TEST_LOCK.lock().await;
    let host = TwoMobs::launch("served-launch").await;

    let answer = host.open_by_session(&host.launch_session).await;
    let opens = host.realtime.open_count();
    let owns_a_channel = host
        .machine
        .session_live_channel_active(&host.launch_session)
        .await;
    host.end(&answer).await;

    assert!(
        answer["error"].is_null() && answer["result"]["channel_id"].is_string(),
        "the launch mob's member is opened by its session: {answer}"
    );
    assert_eq!(opens, 1, "the realtime lane opened one session: {answer}");
    assert!(owns_a_channel, "the member's session owns the channel");
}

/// A session id the launch's session service holds no record of passes the
/// judgement of its mob as well: the live handler is asked, and answers.
#[tokio::test]
async fn a_live_open_by_a_session_id_with_no_stored_record_passes_the_judgement_of_its_mob() {
    let _guard = SHARED_HOST_TEST_LOCK.lock().await;
    let host = TwoMobs::launch("no-record").await;
    let unknown = SessionId::new();
    assert_eq!(
        stored_mob_of(&host.host, &unknown).await,
        None,
        "setup: the session id has no stored record"
    );

    let answer = host.open_by_session(&unknown).await;
    let opens = host.realtime.open_count();
    host.end(&answer).await;
    eprintln!("a session id with no stored record: opens={opens}; {answer}");

    assert_ne!(
        answer["error"]["data"]["kind"],
        json!("foreign_mob_target"),
        "a session id with no stored record is not refused as another mob's: {answer}"
    );
    assert!(
        !answer["error"]["message"]
            .as_str()
            .is_some_and(|message| message.starts_with("live target resolution failed")),
        "the judgement of the session's mob did not fail: {answer}"
    );
    assert!(
        answer["result"]["channel_id"].is_string() || answer["error"].is_object(),
        "the live handler answered the open: {answer}"
    );
}

/// A mob that a member of the launch mob created is in the launch mob's own
/// reach: a live open by the session of a member of such a mob is served as
/// it was before the host could host a second mob.
#[tokio::test]
async fn a_live_open_by_the_session_of_a_launch_mobs_child_mob_member_is_served() {
    let _guard = SHARED_HOST_TEST_LOCK.lock().await;
    let host = TwoMobs::launch("served-child").await;
    let (child, child_session) = host.child_mob_of(&host.launch_session, "child").await;

    let answer = host.open_by_session(&child_session).await;
    let opens = host.realtime.open_count();
    let owns_a_channel = host
        .machine
        .session_live_channel_active(&child_session)
        .await;
    host.end_with(&answer, &[&child]).await;

    assert!(
        answer["error"].is_null() && answer["result"]["channel_id"].is_string(),
        "a member of a mob the launch mob's member created is opened by its session: {answer}"
    );
    assert_eq!(opens, 1, "the realtime lane opened one session: {answer}");
    assert!(owns_a_channel, "that member's session owns the channel");
}

/// A mob that a member of the second mob created is in the second mob's
/// reach: a live open by the session of a member of such a mob is refused
/// with the refusal that names the second mob, the mob the host hosts.
#[tokio::test]
async fn a_live_open_by_the_session_of_a_hosted_mobs_child_mob_member_is_refused() {
    let _guard = SHARED_HOST_TEST_LOCK.lock().await;
    let host = TwoMobs::launch("refused-hosted-child").await;
    let (child, child_session) = host
        .child_mob_of(&host.second_session, "hosted-child")
        .await;

    let answer = host.open_by_session(&child_session).await;
    let opens = host.realtime.open_count();
    let owns_a_channel = host
        .machine
        .session_live_channel_active(&child_session)
        .await;
    let expected = serves_the_launch_mob_only("meta/live/open", &host.second_mob, &host.launch_mob);
    host.end_with(&answer, &[&child]).await;

    assert_eq!(
        answer["error"], expected,
        "the open by the session of a member of the second mob's child mob is refused, \
         naming the second mob: {answer}"
    );
    assert_eq!(opens, 0, "the realtime lane saw no open: {answer}");
    assert!(
        !owns_a_channel,
        "that member's session owns no live channel: {answer}"
    );
}

/// A session of the launch's session service that is no member's (it has no
/// stored member binding) passes the judgement of its mob: a live open by
/// its raw session id is handed to the live handler as before, and is
/// answered by it. The row prints what the handler answered.
#[tokio::test]
async fn a_live_open_by_a_session_that_is_no_members_passes_the_judgement_of_its_mob() {
    let _guard = SHARED_HOST_TEST_LOCK.lock().await;
    let host = TwoMobs::launch("served-no-member").await;
    let session = meerkat_core::Session::new();
    let session_id = session.id().clone();
    let bindings = host
        .machine
        .prepare_bindings(session_id.clone())
        .await
        .expect("setup: runtime bindings for a session of no member");
    host.sessions
        .create_session(meerkat_core::service::CreateSessionRequest {
            model: "gpt-5.5".to_string(),
            prompt: meerkat_core::ContentInput::Text("hello".to_string()),
            injected_context: Vec::new(),
            system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Stage,
            build: Some(meerkat_core::service::SessionBuildOptions {
                resume_session: Some(session),
                runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
                // The mob's own spawn hands each member the launch's test
                // client; a session made on the service directly names its
                // own, so that no provider credential is asked for.
                llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                    Arc::new(meerkat_client::TestClient::default())
                        as Arc<dyn meerkat_client::LlmClient>,
                )),
                ..meerkat_core::service::SessionBuildOptions::default()
            }),
            labels: None,
        })
        .await
        .expect("setup: a session of no member");
    assert_eq!(
        stored_mob_of(&host.host, &session_id).await,
        None,
        "setup: the session has no stored member binding"
    );

    let answer = host.open_by_session(&session_id).await;
    let opens = host.realtime.open_count();
    host.end(&answer).await;
    eprintln!("a session of no member, opened by its session id: opens={opens}; {answer}");

    assert_ne!(
        answer["error"]["data"]["kind"],
        json!("foreign_mob_target"),
        "a session of no member is not refused as another mob's: {answer}"
    );
    assert!(
        !answer["error"]["message"]
            .as_str()
            .is_some_and(|message| message.starts_with("live target resolution failed")),
        "the judgement of the session's mob did not fail: {answer}"
    );
    assert!(
        answer["result"]["channel_id"].is_string() || answer["error"].is_object(),
        "the live handler answered the open: {answer}"
    );
}

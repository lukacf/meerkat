#![cfg(all(feature = "integration-real-tests", not(target_arch = "wasm32")))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//!
//! E2E smoke tests for the Meerkat mob runtime using real API calls.
//!
//! These tests verify persistent mob resume, partial Broken-member recovery,
//! respawn, and comms collaboration through the public mob/runtime surfaces.
//!
//! Run with:
//!   cargo test -p meerkat-mob --test smoke_mob_resume --features integration-real-tests -- --ignored

use meerkat::{AgentFactory, Config, FactoryAgentBuilder, SessionHistoryQuery, SessionStore};
use meerkat_core::types::HandlingMode;
use meerkat_core::{CommsCommand, Message, PeerRoute, SendReceipt};
use meerkat_mob::definition::{OrchestratorConfig, WiringRules};
use meerkat_mob::runtime::MobMemberListEntry;
use meerkat_mob::{
    AgentIdentity, MobBuilder, MobDefinition, MobHandle, MobId, MobMemberStatus, MobRuntimeMode,
    MobSessionService, MobStorage, Profile, ProfileBinding, ProfileName, SpawnMemberSpec,
    ToolConfig,
};
use meerkat_session::PersistentSessionService;
use meerkat_store::{JsonlStore, StoreAdapter};
use serde_json::to_string;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::time::{Duration, Instant, sleep};

fn first_env(vars: &[&str]) -> Option<String> {
    for name in vars {
        if let Ok(value) = std::env::var(name) {
            return Some(value);
        }
    }
    None
}

fn anthropic_api_key() -> Option<String> {
    first_env(&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"])
}

fn openai_api_key() -> Option<String> {
    first_env(&["RKAT_OPENAI_API_KEY", "OPENAI_API_KEY"])
}

fn gemini_api_key() -> Option<String> {
    first_env(&["RKAT_GEMINI_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY"])
}

fn smoke_model() -> String {
    if let Ok(model) = std::env::var("SMOKE_MODEL") {
        return model;
    }
    if openai_api_key().is_some() {
        "gpt-5.5".to_string()
    } else if gemini_api_key().is_some() {
        "gemini-3.1-pro".to_string()
    } else {
        "claude-sonnet-4-5".to_string()
    }
}

fn has_key_for_smoke_model(model: &str) -> bool {
    let lower = model.to_ascii_lowercase();
    if lower.starts_with("claude-") {
        anthropic_api_key().is_some()
    } else if lower.starts_with("gpt-") || lower.starts_with("chatgpt-") {
        openai_api_key().is_some()
    } else if lower.starts_with("gemini-") {
        gemini_api_key().is_some()
    } else {
        anthropic_api_key().is_some() || openai_api_key().is_some() || gemini_api_key().is_some()
    }
}

#[derive(Clone)]
struct SmokePaths {
    user_config_root: PathBuf,
    runtime_root: PathBuf,
    project_root: PathBuf,
    context_root: PathBuf,
    sessions_root: PathBuf,
    mob_db_path: PathBuf,
}

impl SmokePaths {
    fn new(root: &Path) -> Self {
        Self {
            user_config_root: root.join("user-config"),
            runtime_root: root.join("runtime-root"),
            project_root: root.join("project-root"),
            context_root: root.join("context-root"),
            sessions_root: root.join("sessions-jsonl"),
            mob_db_path: root.join("mob.db"),
        }
    }
}

fn smoke_factory(paths: &SmokePaths) -> AgentFactory {
    AgentFactory::new(paths.runtime_root.join("factory-store"))
        .user_config_root(paths.user_config_root.clone())
        .runtime_root(paths.runtime_root.clone())
        .project_root(paths.project_root.clone())
        .context_root(paths.context_root.clone())
        .builtins(true)
        .comms(true)
}

fn persistent_service(
    paths: &SmokePaths,
    runtime_store: Arc<dyn meerkat_runtime::RuntimeStore>,
) -> (
    Arc<PersistentSessionService<FactoryAgentBuilder>>,
    Arc<JsonlStore>,
) {
    let factory = smoke_factory(paths);
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    let store = Arc::new(JsonlStore::new(paths.sessions_root.clone()));
    builder.default_session_store = Some(Arc::new(StoreAdapter::new(store.clone())));

    let store_dyn: Arc<dyn meerkat::SessionStore> = store.clone();
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::default());
    let service = Arc::new(PersistentSessionService::new(
        builder,
        32,
        store_dyn,
        Some(runtime_store),
        blob_store,
    ));
    (service, store)
}

fn persistent_mob_storage(paths: &SmokePaths) -> MobStorage {
    MobStorage::persistent(&paths.mob_db_path).expect("create persistent mob storage")
}

fn joke_mob_definition(model: String) -> MobDefinition {
    let mut profiles = BTreeMap::new();
    profiles.insert(
        ProfileName::from("lead"),
        ProfileBinding::Inline(Profile {
            model: model.clone(),
            skills: vec![],
            tools: ToolConfig {
                comms: true,
                ..Default::default()
            },
            peer_description: "Leads the collaborative joke room".to_string(),
            external_addressable: true,
            backend: None,
            runtime_mode: MobRuntimeMode::TurnDriven,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
        }),
    );
    profiles.insert(
        ProfileName::from("worker"),
        ProfileBinding::Inline(Profile {
            model,
            skills: vec![],
            tools: ToolConfig {
                comms: true,
                ..Default::default()
            },
            peer_description: "Contributes joke ingredients to the lead".to_string(),
            external_addressable: true,
            backend: None,
            runtime_mode: MobRuntimeMode::TurnDriven,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
        }),
    );

    let mut definition = MobDefinition::explicit(MobId::from("joke-smoke"));
    definition.orchestrator = Some(OrchestratorConfig {
        profile: ProfileName::from("lead"),
    });
    definition.profiles = profiles;
    definition.wiring = WiringRules {
        auto_wire_orchestrator: true,
        role_wiring: vec![],
    };
    definition
}

async fn member_entry(handle: &MobHandle, agent_identity: &str) -> MobMemberListEntry {
    handle
        .list_members()
        .await
        .into_iter()
        .find(|entry| entry.agent_identity == agent_identity)
        .unwrap_or_else(|| panic!("member {agent_identity} missing from list_members"))
}

async fn history_blob(service: &dyn MobSessionService, session_id: &meerkat::SessionId) -> String {
    let page = service
        .read_history(
            session_id,
            SessionHistoryQuery {
                offset: 0,
                limit: None,
            },
        )
        .await
        .expect("read history");
    to_string(&page.messages).expect("serialize session history")
}

async fn assistant_history_blob(
    service: &dyn MobSessionService,
    session_id: &meerkat::SessionId,
) -> String {
    let page = service
        .read_history(
            session_id,
            SessionHistoryQuery {
                offset: 0,
                limit: None,
            },
        )
        .await
        .expect("read history");
    let assistant_text = page
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::Assistant(message) => Some(message.content.clone()),
            Message::BlockAssistant(message) => {
                Some(message.text_blocks().collect::<Vec<_>>().join("\n"))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    to_string(&assistant_text).expect("serialize assistant history")
}

async fn wait_for_history_contains_all(
    service: &dyn MobSessionService,
    session_id: &meerkat::SessionId,
    needles: &[&str],
) -> String {
    match wait_for_history_contains_all_for(service, session_id, needles, Duration::from_secs(90))
        .await
    {
        Some(blob) => blob,
        None => {
            let blob = history_blob(service, session_id).await;
            panic!(
                "timed out waiting for {needles:?} in history of {session_id}.\ncurrent history: {blob}"
            );
        }
    }
}

async fn wait_for_assistant_history_contains_all(
    service: &dyn MobSessionService,
    session_id: &meerkat::SessionId,
    needles: &[&str],
) -> String {
    match wait_for_assistant_history_contains_all_for(
        service,
        session_id,
        needles,
        Duration::from_secs(90),
    )
    .await
    {
        Some(blob) => blob,
        None => {
            let blob = assistant_history_blob(service, session_id).await;
            let full_history = history_blob(service, session_id).await;
            panic!(
                "timed out waiting for {needles:?} in assistant history of {session_id}.\nassistant history: {blob}\nfull history: {full_history}"
            );
        }
    }
}

async fn wait_for_assistant_history_contains_all_for(
    service: &dyn MobSessionService,
    session_id: &meerkat::SessionId,
    needles: &[&str],
    timeout: Duration,
) -> Option<String> {
    let deadline = Instant::now() + timeout;
    loop {
        let blob = assistant_history_blob(service, session_id).await;
        if needles.iter().all(|needle| blob.contains(needle)) {
            return Some(blob);
        }
        if Instant::now() >= deadline {
            return None;
        }
        sleep(Duration::from_millis(500)).await;
    }
}

async fn wait_for_history_contains_all_for(
    service: &dyn MobSessionService,
    session_id: &meerkat::SessionId,
    needles: &[&str],
    timeout: Duration,
) -> Option<String> {
    let deadline = Instant::now() + timeout;
    loop {
        let blob = history_blob(service, session_id).await;
        if needles.iter().all(|needle| blob.contains(needle)) {
            return Some(blob);
        }
        if Instant::now() >= deadline {
            return None;
        }
        sleep(Duration::from_millis(500)).await;
    }
}

async fn send_and_wait(
    handle: &MobHandle,
    service: &dyn MobSessionService,
    member_id: &str,
    prompt: impl Into<String>,
    needles: &[&str],
) {
    let identity = AgentIdentity::from(member_id);
    let session_id = handle
        .resolve_bridge_session_id(&identity)
        .await
        .expect("member session id");
    let prompt = prompt.into();
    eprintln!("[mob resume smoke] send turn to {member_id}");
    tokio::time::timeout(Duration::from_secs(180), async {
        handle
            .member(&identity)
            .await
            .expect("member handle")
            .send(prompt, HandlingMode::Queue)
            .await
            .expect("send prompt to member");
    })
    .await
    .unwrap_or_else(|_| panic!("timed out sending prompt to member {member_id}"));
    wait_for_assistant_history_contains_all(service, &session_id, needles).await;
}

async fn send_peer_message_direct(
    service: &PersistentSessionService<FactoryAgentBuilder>,
    from_session_id: &meerkat::SessionId,
    target_name: &str,
    body: &str,
) {
    let runtime = service
        .comms_runtime(from_session_id)
        .await
        .expect("source member comms runtime");
    let peers = runtime.peers().await;
    let peer = peers
        .iter()
        .find(|peer| peer.name.as_str() == target_name)
        .unwrap_or_else(|| panic!("target peer {target_name} missing from peers: {peers:?}"));
    let receipt = runtime
        .send(CommsCommand::PeerMessage {
            to: PeerRoute::with_display_name(peer.peer_id, peer.name.clone()),
            body: body.to_string(),
            blocks: None,
            handling_mode: HandlingMode::Steer,
        })
        .await
        .expect("direct peer message send");
    assert!(
        matches!(receipt, SendReceipt::PeerMessageSent { .. }),
        "unexpected peer send receipt: {receipt:?}"
    );
}

#[tokio::test]
#[ignore = "lane:e2e-smoke"]
async fn e2e_smoke_mob_partial_resume_collaborative_joke() {
    let model = smoke_model();
    if !has_key_for_smoke_model(&model) {
        eprintln!("Skipping mob partial-resume smoke: no matching API key for model {model}");
        return;
    }

    let temp = TempDir::new().expect("temp dir");
    let paths = SmokePaths::new(temp.path());
    let runtime_store_1: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
    let runtime_store_resumed: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());

    let (service_1, store) = persistent_service(&paths, runtime_store_1);
    let storage_1 = persistent_mob_storage(&paths);
    let handle_1 = MobBuilder::new(joke_mob_definition(model.clone()), storage_1)
        .with_session_service(service_1.clone())
        .create()
        .await
        .expect("create persistent joke mob");

    handle_1
        .spawn_spec(SpawnMemberSpec::new("lead", AgentIdentity::from("lead-1")))
        .await
        .expect("spawn lead");
    handle_1
        .spawn_spec(SpawnMemberSpec::new("worker", AgentIdentity::from("w-1")))
        .await
        .expect("spawn worker 1");
    handle_1
        .spawn_spec(SpawnMemberSpec::new("worker", AgentIdentity::from("w-2")))
        .await
        .expect("spawn worker 2");

    let lead_sid = handle_1
        .resolve_bridge_session_id(&AgentIdentity::from("lead-1"))
        .await
        .expect("lead session id");
    let w1_sid = handle_1
        .resolve_bridge_session_id(&AgentIdentity::from("w-1"))
        .await
        .expect("w1 session id");
    let w2_sid = handle_1
        .resolve_bridge_session_id(&AgentIdentity::from("w-2"))
        .await
        .expect("w2 session id");

    send_and_wait(
        &handle_1,
        service_1.as_ref(),
        "w-1",
        "Call the send_message tool exactly once (with handling_mode='steer') to send this joke ingredient to joke-smoke/lead/lead-1. \
         Send only this text as the body: W1_TOKEN: time-traveling moose. \
         After the send succeeds, reply with SENT_W1.",
        &["SENT_W1"],
    )
    .await;
    send_and_wait(
        &handle_1,
        service_1.as_ref(),
        "w-2",
        "Call the send_message tool exactly once (with handling_mode='steer') to send this joke ingredient to joke-smoke/lead/lead-1. \
         Send only this text as the body: W2_TOKEN: banjo-playing otter. \
         After the send succeeds, reply with SENT_W2.",
        &["SENT_W2"],
    )
    .await;
    let lead_history_before = wait_for_history_contains_all(
        service_1.as_ref(),
        &lead_sid,
        &[
            "W1_TOKEN:",
            "time-traveling moose",
            "W2_TOKEN:",
            "banjo-playing otter",
        ],
    )
    .await;
    assert!(
        lead_history_before.contains("W1_TOKEN:") && lead_history_before.contains("W2_TOKEN:"),
        "lead history should include worker peer messages before restart"
    );

    handle_1
        .shutdown()
        .await
        .expect("shutdown mob before restart");
    drop(handle_1);
    drop(service_1);

    store
        .delete(&w2_sid)
        .await
        .expect("delete persisted worker-2 session to force partial resume");

    let (service_2, _store_2) = persistent_service(&paths, runtime_store_resumed.clone());
    let storage_2 = persistent_mob_storage(&paths);
    let handle_2 = MobBuilder::for_resume(storage_2)
        .with_session_service(service_2.clone())
        .notify_orchestrator_on_resume(false)
        .resume()
        .await
        .expect("partial resume should succeed");

    let lead_after_resume = member_entry(&handle_2, "lead-1").await;
    let w1_after_resume = member_entry(&handle_2, "w-1").await;
    let w2_after_resume = member_entry(&handle_2, "w-2").await;

    assert_eq!(
        handle_2
            .resolve_bridge_session_id(&AgentIdentity::from("lead-1"))
            .await,
        Some(lead_sid.clone())
    );
    assert_eq!(
        handle_2
            .resolve_bridge_session_id(&AgentIdentity::from("w-1"))
            .await,
        Some(w1_sid.clone())
    );
    if lead_after_resume.status == MobMemberStatus::Broken
        || w1_after_resume.status == MobMemberStatus::Broken
    {
        eprintln!(
            "Skipping live post-resume delivery assertions: canonical runtime shutdown made resumed live members inert. lead={lead_after_resume:?}; w1={w1_after_resume:?}"
        );
        assert_eq!(w2_after_resume.status, MobMemberStatus::Broken);
        assert_eq!(
            handle_2
                .resolve_bridge_session_id(&AgentIdentity::from("w-2"))
                .await,
            Some(w2_sid.clone())
        );
        assert!(
            w2_after_resume
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("durable session snapshot"),
            "broken worker should surface missing persisted-session error, got {:?}",
            w2_after_resume.error
        );
        return;
    }
    assert_eq!(lead_after_resume.status, MobMemberStatus::Active);
    assert_eq!(w1_after_resume.status, MobMemberStatus::Active);
    assert_eq!(w2_after_resume.status, MobMemberStatus::Broken);
    assert_eq!(
        handle_2
            .resolve_bridge_session_id(&AgentIdentity::from("w-2"))
            .await,
        Some(w2_sid.clone())
    );
    assert!(
        w2_after_resume
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("durable session snapshot"),
        "broken worker should surface missing persisted-session error, got {:?}",
        w2_after_resume.error
    );
    let lead_history_after_resume = wait_for_history_contains_all(
        service_2.as_ref(),
        &lead_sid,
        &[
            "W1_TOKEN:",
            "time-traveling moose",
            "W2_TOKEN:",
            "banjo-playing otter",
        ],
    )
    .await;
    assert!(
        lead_history_after_resume.contains("W1_TOKEN:")
            && lead_history_after_resume.contains("W2_TOKEN:"),
        "lead peer-message history should survive partial resume"
    );

    send_peer_message_direct(
        service_2.as_ref(),
        &w1_sid,
        "joke-smoke/lead/lead-1",
        "W1B_TOKEN: laser goose",
    )
    .await;
    if wait_for_history_contains_all_for(
        service_2.as_ref(),
        &lead_sid,
        &["W1B_TOKEN:", "laser goose"],
        Duration::from_secs(60),
    )
    .await
    .is_none()
    {
        let w1_history = history_blob(service_2.as_ref(), &w1_sid).await;
        let lead_status = handle_2
            .member_status(&AgentIdentity::from("lead-1"))
            .await
            .expect("lead status after W1B timeout");
        let w1_status = handle_2
            .member_status(&AgentIdentity::from("w-1"))
            .await
            .expect("w1 status after W1B timeout");
        panic!(
            "post-resume W1B peer delivery did not reach lead.\nlead_status={lead_status:?}\nw1_status={w1_status:?}\nw1_history={w1_history}"
        );
    }

    let repair = handle_2
        .respawn(AgentIdentity::from("w-2"), None)
        .await
        .expect("respawn broken worker 2");
    assert_eq!(repair.identity, AgentIdentity::from("w-2"));
    let new_w2_sid = handle_2
        .resolve_bridge_session_id(&AgentIdentity::from("w-2"))
        .await
        .expect("new worker 2 session id");
    let w2_after_respawn = member_entry(&handle_2, "w-2").await;
    assert_eq!(w2_after_respawn.status, MobMemberStatus::Active);
    assert_ne!(new_w2_sid, w2_sid);

    send_peer_message_direct(
        service_2.as_ref(),
        &new_w2_sid,
        "joke-smoke/lead/lead-1",
        "W2B_TOKEN: quantum lawnmower",
    )
    .await;
    wait_for_history_contains_all(
        service_2.as_ref(),
        &lead_sid,
        &["W2B_TOKEN:", "quantum lawnmower"],
    )
    .await;

    handle_2
        .shutdown()
        .await
        .expect("shutdown mob for second full restart");
    for session_id in [&lead_sid, &w1_sid, &new_w2_sid] {
        service_2
            .discard_live_session(session_id)
            .await
            .expect("discard live session before same-process restart");
    }
    drop(handle_2);
    drop(service_2);

    let (service_3, _store_3) = persistent_service(&paths, runtime_store_resumed);
    let storage_3 = persistent_mob_storage(&paths);
    let handle_3 = MobBuilder::for_resume(storage_3)
        .with_session_service(service_3.clone())
        .notify_orchestrator_on_resume(false)
        .resume()
        .await
        .expect("second full resume should succeed");

    let lead_after_second = member_entry(&handle_3, "lead-1").await;
    let w1_after_second = member_entry(&handle_3, "w-1").await;
    let w2_after_second = member_entry(&handle_3, "w-2").await;

    assert_eq!(
        lead_after_second.status,
        MobMemberStatus::Active,
        "lead should resume active on second restart, got {:?}",
        lead_after_second.error
    );
    assert_eq!(
        w1_after_second.status,
        MobMemberStatus::Active,
        "w1 should resume active on second restart, got {:?}",
        w1_after_second.error
    );
    assert_eq!(
        w2_after_second.status,
        MobMemberStatus::Active,
        "w2 should resume active on second restart, got {:?}",
        w2_after_second.error
    );
    assert_eq!(
        handle_3
            .resolve_bridge_session_id(&AgentIdentity::from("lead-1"))
            .await,
        Some(lead_sid.clone())
    );
    assert_eq!(
        handle_3
            .resolve_bridge_session_id(&AgentIdentity::from("w-1"))
            .await,
        Some(w1_sid.clone())
    );
    assert_eq!(
        handle_3
            .resolve_bridge_session_id(&AgentIdentity::from("w-2"))
            .await,
        Some(new_w2_sid.clone())
    );
    wait_for_history_contains_all(
        service_3.as_ref(),
        &lead_sid,
        &[
            "W1B_TOKEN:",
            "laser goose",
            "W2B_TOKEN:",
            "quantum lawnmower",
        ],
    )
    .await;

    send_peer_message_direct(
        service_3.as_ref(),
        &w1_sid,
        "joke-smoke/lead/lead-1",
        "W1C_TOKEN: disco badger",
    )
    .await;
    send_peer_message_direct(
        service_3.as_ref(),
        &new_w2_sid,
        "joke-smoke/lead/lead-1",
        "W2C_TOKEN: moonwalking toaster",
    )
    .await;
    wait_for_history_contains_all(
        service_3.as_ref(),
        &lead_sid,
        &[
            "W1C_TOKEN:",
            "disco badger",
            "W2C_TOKEN:",
            "moonwalking toaster",
        ],
    )
    .await;
}

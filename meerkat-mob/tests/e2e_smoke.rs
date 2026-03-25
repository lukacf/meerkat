#![cfg(all(feature = "integration-real-tests", not(target_arch = "wasm32")))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//!
//! E2E smoke tests for the Meerkat mob runtime using real API calls.
//!
//! These tests verify persistent mob resume, partial Broken-member recovery,
//! respawn, and comms collaboration through the public mob/runtime surfaces.
//!
//! Run with:
//!   cargo test -p meerkat-mob --test e2e_smoke --features integration-real-tests -- --ignored --test-threads=1

use meerkat::{AgentFactory, Config, FactoryAgentBuilder, SessionHistoryQuery, SessionStore};
use meerkat_core::types::HandlingMode;
use meerkat_mob::definition::{OrchestratorConfig, WiringRules};
use meerkat_mob::runtime::MobMemberListEntry;
use meerkat_mob::{
    MeerkatId, MobBuilder, MobDefinition, MobHandle, MobId, MobMemberStatus, MobRuntimeMode,
    MobSessionService, MobStorage, Profile, ProfileName, ToolConfig,
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
    std::env::var("SMOKE_MODEL").unwrap_or_else(|_| "claude-sonnet-4-5".to_string())
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
            mob_db_path: root.join("mob.redb"),
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
) -> (
    Arc<PersistentSessionService<FactoryAgentBuilder>>,
    Arc<JsonlStore>,
) {
    let factory = smoke_factory(paths);
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    let store = Arc::new(JsonlStore::new(paths.sessions_root.clone()));
    builder.default_session_store = Some(Arc::new(StoreAdapter::new(store.clone())));

    let store_dyn: Arc<dyn meerkat::SessionStore> = store.clone();
    let service = Arc::new(PersistentSessionService::new(builder, 32, store_dyn, None));
    (service, store)
}

fn persistent_mob_storage(paths: &SmokePaths) -> MobStorage {
    MobStorage::redb(&paths.mob_db_path).expect("create redb-backed mob storage")
}

fn joke_mob_definition(model: String) -> MobDefinition {
    let mut profiles = BTreeMap::new();
    profiles.insert(
        ProfileName::from("lead"),
        Profile {
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
        },
    );
    profiles.insert(
        ProfileName::from("worker"),
        Profile {
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
        },
    );

    MobDefinition {
        id: MobId::from("joke-smoke"),
        orchestrator: Some(OrchestratorConfig {
            profile: ProfileName::from("lead"),
        }),
        profiles,
        mcp_servers: BTreeMap::new(),
        wiring: WiringRules {
            auto_wire_orchestrator: true,
            role_wiring: vec![],
        },
        skills: BTreeMap::new(),
        backend: Default::default(),
        flows: Default::default(),
        topology: None,
        supervisor: None,
        limits: None,
        spawn_policy: None,
        event_router: None,
    }
}

async fn member_entry(handle: &MobHandle, meerkat_id: &str) -> MobMemberListEntry {
    handle
        .list_members()
        .await
        .into_iter()
        .find(|entry| entry.meerkat_id == MeerkatId::from(meerkat_id))
        .unwrap_or_else(|| panic!("member {meerkat_id} missing from list_members"))
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

async fn wait_for_history_contains_all(
    service: &dyn MobSessionService,
    session_id: &meerkat::SessionId,
    needles: &[&str],
) -> String {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let blob = history_blob(service, session_id).await;
        if needles.iter().all(|needle| blob.contains(needle)) {
            return blob;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {needles:?} in history of {session_id}.\ncurrent history: {blob}"
        );
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
    let session_id = member_entry(handle, member_id)
        .await
        .current_session_id
        .expect("member session id");
    handle
        .member(&MeerkatId::from(member_id))
        .await
        .expect("member handle")
        .send(prompt.into(), HandlingMode::Queue)
        .await
        .expect("send prompt to member");
    wait_for_history_contains_all(service, &session_id, needles).await;
}

#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_smoke_mob_partial_resume_collaborative_joke() {
    let model = smoke_model();
    if !has_key_for_smoke_model(&model) {
        eprintln!("Skipping mob partial-resume smoke: no matching API key for model {model}");
        return;
    }

    let temp = TempDir::new().expect("temp dir");
    let paths = SmokePaths::new(temp.path());

    let (service_1, store) = persistent_service(&paths);
    let storage_1 = persistent_mob_storage(&paths);
    let handle_1 = MobBuilder::new(joke_mob_definition(model.clone()), storage_1)
        .with_session_service(service_1.clone())
        .create()
        .await
        .expect("create persistent joke mob");

    handle_1
        .spawn(ProfileName::from("lead"), MeerkatId::from("lead-1"), None)
        .await
        .expect("spawn lead");
    handle_1
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker 1");
    handle_1
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn worker 2");

    let lead_before = member_entry(&handle_1, "lead-1").await;
    let w1_before = member_entry(&handle_1, "w-1").await;
    let w2_before = member_entry(&handle_1, "w-2").await;
    let lead_sid = lead_before
        .current_session_id
        .clone()
        .expect("lead session id");
    let w1_sid = w1_before.current_session_id.clone().expect("w1 session id");
    let w2_sid = w2_before.current_session_id.clone().expect("w2 session id");
    let lead_peer = lead_before.peer_id.clone().expect("lead peer id");
    let w1_peer = w1_before.peer_id.clone().expect("w1 peer id");
    let w2_peer = w2_before.peer_id.clone().expect("w2 peer id");

    send_and_wait(
        &handle_1,
        service_1.as_ref(),
        "w-1",
        "Call the send tool exactly once to send this joke ingredient to joke-smoke/lead/lead-1. \
         Send only this text as the body: W1_TOKEN: time-traveling moose. \
         After the send succeeds, reply with SENT_W1.",
        &["SENT_W1"],
    )
    .await;
    send_and_wait(
        &handle_1,
        service_1.as_ref(),
        "w-2",
        "Call the send tool exactly once to send this joke ingredient to joke-smoke/lead/lead-1. \
         Send only this text as the body: W2_TOKEN: banjo-playing otter. \
         After the send succeeds, reply with SENT_W2.",
        &["SENT_W2"],
    )
    .await;
    send_and_wait(
        &handle_1,
        service_1.as_ref(),
        "lead-1",
        "Read the peer messages your collaborators already sent you. \
         Write one sentence that uses both the exact phrases 'time-traveling moose' \
         and 'banjo-playing otter'. Reply on a single line starting with FINAL_JOKE:.",
        &["FINAL_JOKE:", "time-traveling moose", "banjo-playing otter"],
    )
    .await;
    let lead_history_before = wait_for_history_contains_all(
        service_1.as_ref(),
        &lead_sid,
        &["FINAL_JOKE:", "time-traveling moose", "banjo-playing otter"],
    )
    .await;
    assert!(
        lead_history_before.contains("FINAL_JOKE:"),
        "lead history should include FINAL_JOKE before restart"
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

    let (service_2, _store_2) = persistent_service(&paths);
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

    assert_eq!(lead_after_resume.status, MobMemberStatus::Active);
    assert_eq!(lead_after_resume.current_session_id, Some(lead_sid.clone()));
    assert_eq!(
        lead_after_resume.peer_id.as_deref(),
        Some(lead_peer.as_str())
    );
    assert_eq!(w1_after_resume.status, MobMemberStatus::Active);
    assert_eq!(w1_after_resume.current_session_id, Some(w1_sid.clone()));
    assert_eq!(w1_after_resume.peer_id.as_deref(), Some(w1_peer.as_str()));
    assert_eq!(w2_after_resume.status, MobMemberStatus::Broken);
    assert_eq!(w2_after_resume.current_session_id, Some(w2_sid.clone()));
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
        &["FINAL_JOKE:", "time-traveling moose", "banjo-playing otter"],
    )
    .await;
    assert!(
        lead_history_after_resume.contains("FINAL_JOKE:"),
        "lead history should survive partial resume"
    );

    send_and_wait(
        &handle_2,
        service_2.as_ref(),
        "w-1",
        "Call the send tool exactly once to send this joke ingredient to joke-smoke/lead/lead-1. \
         Send only this text as the body: W1B_TOKEN: laser goose. \
         After the send succeeds, reply with SENT_W1B.",
        &["SENT_W1B"],
    )
    .await;
    send_and_wait(
        &handle_2,
        service_2.as_ref(),
        "lead-1",
        "Read the latest peer messages and write a stronger follow-up joke. \
         Include the exact phrase 'laser goose' and reply on a single line \
         starting with RECOVERY_JOKE:.",
        &["RECOVERY_JOKE:", "laser goose"],
    )
    .await;
    wait_for_history_contains_all(
        service_2.as_ref(),
        &lead_sid,
        &["RECOVERY_JOKE:", "laser goose"],
    )
    .await;

    let repair = handle_2
        .respawn(MeerkatId::from("w-2"), None)
        .await
        .expect("respawn broken worker 2");
    let new_w2_sid = repair
        .new_session_id
        .clone()
        .expect("new worker 2 session id");
    assert_ne!(repair.old_session_id, repair.new_session_id);
    let w2_after_respawn = member_entry(&handle_2, "w-2").await;
    let new_w2_peer = w2_after_respawn
        .peer_id
        .clone()
        .expect("new worker 2 peer id");
    assert_eq!(w2_after_respawn.status, MobMemberStatus::Active);
    assert_eq!(
        w2_after_respawn.current_session_id,
        Some(new_w2_sid.clone())
    );
    assert_ne!(new_w2_sid, w2_sid);
    assert_ne!(new_w2_peer, w2_peer);

    send_and_wait(
        &handle_2,
        service_2.as_ref(),
        "w-2",
        "Call the send tool exactly once to send this joke ingredient to joke-smoke/lead/lead-1. \
         Send only this text as the body: W2B_TOKEN: quantum lawnmower. \
         After the send succeeds, reply with SENT_W2B.",
        &["SENT_W2B"],
    )
    .await;
    send_and_wait(
        &handle_2,
        service_2.as_ref(),
        "lead-1",
        "Read the latest peer messages and produce the best collaborative version yet. \
         Include the exact phrase 'quantum lawnmower' and reply on a single line \
         starting with ULTIMATE_JOKE:.",
        &["ULTIMATE_JOKE:", "quantum lawnmower"],
    )
    .await;
    wait_for_history_contains_all(
        service_2.as_ref(),
        &lead_sid,
        &["ULTIMATE_JOKE:", "quantum lawnmower"],
    )
    .await;

    handle_2
        .shutdown()
        .await
        .expect("shutdown mob for second full restart");
    drop(handle_2);
    drop(service_2);

    let (service_3, _store_3) = persistent_service(&paths);
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

    assert_eq!(lead_after_second.current_session_id, Some(lead_sid.clone()));
    assert_eq!(
        lead_after_second.peer_id.as_deref(),
        Some(lead_peer.as_str())
    );
    assert_eq!(w1_after_second.current_session_id, Some(w1_sid.clone()));
    assert_eq!(w1_after_second.peer_id.as_deref(), Some(w1_peer.as_str()));
    assert_eq!(w2_after_second.current_session_id, Some(new_w2_sid.clone()));
    assert_eq!(
        w2_after_second.peer_id.as_deref(),
        Some(new_w2_peer.as_str())
    );
    wait_for_history_contains_all(
        service_3.as_ref(),
        &lead_sid,
        &["ULTIMATE_JOKE:", "quantum lawnmower"],
    )
    .await;

    send_and_wait(
        &handle_3,
        service_3.as_ref(),
        "w-1",
        "Call the send tool exactly once to send this joke ingredient to joke-smoke/lead/lead-1. \
         Send only this text as the body: W1C_TOKEN: disco badger. \
         After the send succeeds, reply with SENT_W1C.",
        &["SENT_W1C"],
    )
    .await;
    send_and_wait(
        &handle_3,
        service_3.as_ref(),
        "w-2",
        "Call the send tool exactly once to send this joke ingredient to joke-smoke/lead/lead-1. \
         Send only this text as the body: W2C_TOKEN: moonwalking toaster. \
         After the send succeeds, reply with SENT_W2C.",
        &["SENT_W2C"],
    )
    .await;
    send_and_wait(
        &handle_3,
        service_3.as_ref(),
        "lead-1",
        "Read the latest peer messages and close the set with your strongest version. \
         Include the exact phrases 'disco badger' and 'moonwalking toaster' and reply \
         on a single line starting with ENCORE_JOKE:.",
        &["ENCORE_JOKE:", "disco badger", "moonwalking toaster"],
    )
    .await;
    wait_for_history_contains_all(
        service_3.as_ref(),
        &lead_sid,
        &["ENCORE_JOKE:", "disco badger", "moonwalking toaster"],
    )
    .await;
}

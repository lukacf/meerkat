//! H3 repro harness: mob member revival across a cold host restart.
//!
//! Mirrors `smoke_mob_resume.rs` (the live-provider mob resume smoke) with
//! deterministic mock LLM clients so the mob resume-restore path
//! (`MobBuilder::for_resume` → `reconcile_resume` → `build_resumed_agent_config`
//! → `provision_member` → `create_session` → `persist_full_session_or_discard_live`)
//! is exercised without API keys.
//!
//! Hunting: TranscriptContinuityViolation ("save rejected: incoming transcript
//! is not a continuation of persisted revision") on the first post-restart
//! persist of a resumed mob member session.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use meerkat::{
    AgentFactory, Config, FactoryAgentBuilder, SessionHistoryQuery, SessionServiceControlExt,
    SessionStore,
};
use meerkat_core::session_store::IncrementalSessionStore as _;
use meerkat_core::types::HandlingMode;
use meerkat_core::{CommsCommand, PeerRoute, SendReceipt};
use meerkat_mob::definition::{OrchestratorConfig, WiringRules};
use meerkat_mob::runtime::MobMemberListEntry;
use meerkat_mob::{
    AgentIdentity, MobBuilder, MobDefinition, MobHandle, MobId, MobMemberStatus, MobRuntimeMode,
    MobSessionService, MobStorage, Profile, ProfileBinding, ProfileName, SpawnMemberSpec,
    ToolConfig,
};
use meerkat_session::PersistentSessionService;
use meerkat_store::{JsonlStore, SqliteSessionStore, StoreAdapter};
use serde_json::to_string;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::time::{Duration, Instant, sleep};

#[derive(Clone)]
struct Paths {
    user_config_root: PathBuf,
    runtime_root: PathBuf,
    project_root: PathBuf,
    context_root: PathBuf,
    sessions_root: PathBuf,
    realm_db_path: PathBuf,
    mob_db_path: PathBuf,
}

impl Paths {
    fn new(root: &Path) -> Self {
        Self {
            user_config_root: root.join("user-config"),
            runtime_root: root.join("runtime-root"),
            project_root: root.join("project-root"),
            context_root: root.join("context-root"),
            sessions_root: root.join("sessions-jsonl"),
            realm_db_path: root.join("realm.sqlite3"),
            mob_db_path: root.join("mob.db"),
        }
    }

    fn materialize_project_context(&self) {
        for root in [&self.project_root, &self.context_root] {
            fs::create_dir_all(root).expect("create project/context root");
            fs::write(
                root.join("AGENTS.md"),
                "# Mob Cold Restart\n\nDeterministic mock-LLM harness.\n",
            )
            .expect("write AGENTS.md");
        }
    }
}

fn factory(paths: &Paths) -> AgentFactory {
    AgentFactory::new(paths.runtime_root.join("factory-store"))
        .user_config_root(paths.user_config_root.clone())
        .runtime_root(paths.runtime_root.clone())
        .project_root(paths.project_root.clone())
        .context_root(paths.context_root.clone())
        .builtins(true)
        .comms(true)
}

fn persistent_service(
    paths: &Paths,
    runtime_store: Arc<dyn meerkat_runtime::RuntimeStore>,
) -> (
    Arc<PersistentSessionService<FactoryAgentBuilder>>,
    Arc<JsonlStore>,
) {
    paths.materialize_project_context();
    let factory = factory(paths);
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    builder.default_llm_client = Some(Arc::new(meerkat_client::TestClient::default()));
    let store = Arc::new(JsonlStore::new(paths.sessions_root.clone()));
    builder.default_session_store = Some(Arc::new(StoreAdapter::new(store.clone())));

    let store_dyn: Arc<dyn meerkat::SessionStore> = store.clone();
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::default());
    let service = Arc::new(PersistentSessionService::new(
        builder,
        32,
        store_dyn,
        runtime_store,
        blob_store,
    ));
    (service, store)
}

fn persistent_head_canonical_service(
    paths: &Paths,
    runtime_store: Arc<dyn meerkat_runtime::RuntimeStore>,
) -> (
    Arc<PersistentSessionService<FactoryAgentBuilder>>,
    Arc<SqliteSessionStore>,
) {
    paths.materialize_project_context();
    let factory = factory(paths);
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    builder.default_llm_client = Some(Arc::new(meerkat_client::TestClient::default()));
    let store = Arc::new(
        SqliteSessionStore::open(&paths.realm_db_path)
            .expect("open co-tenant SQLite session store"),
    );
    builder.default_session_store = Some(Arc::new(StoreAdapter::new(store.clone())));

    let store_dyn: Arc<dyn meerkat::SessionStore> = store.clone();
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::default());
    let service = Arc::new(PersistentSessionService::new(
        builder,
        32,
        store_dyn,
        runtime_store,
        blob_store,
    ));
    (service, store)
}

fn mob_definition(lead_mode: MobRuntimeMode) -> MobDefinition {
    let mut profiles = BTreeMap::new();
    profiles.insert(
        ProfileName::from("lead"),
        ProfileBinding::Inline(Box::new(Profile {
            model: "gpt-5.4".to_string(),
            provider: None,
            self_hosted_server_id: None,
            image_generation_provider: None,
            auto_compact_threshold: None,
            resume_overrides: Vec::new(),
            skills: vec![],
            tools: ToolConfig {
                comms: true,
                ..Default::default()
            },
            peer_description: "Leads the restart room".to_string(),
            external_addressable: true,
            backend: None,
            runtime_mode: lead_mode,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
        })),
    );
    profiles.insert(
        ProfileName::from("worker"),
        ProfileBinding::Inline(Box::new(Profile {
            model: "gpt-5.4".to_string(),
            provider: None,
            self_hosted_server_id: None,
            image_generation_provider: None,
            auto_compact_threshold: None,
            resume_overrides: Vec::new(),
            skills: vec![],
            tools: ToolConfig {
                comms: true,
                ..Default::default()
            },
            peer_description: "Sends restart tokens to the lead".to_string(),
            external_addressable: true,
            backend: None,
            runtime_mode: MobRuntimeMode::TurnDriven,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
        })),
    );

    // Every test in this binary runs in the process-global default inproc
    // namespace. Give each cold-restart scenario a distinct canonical comms
    // name so parallel test execution cannot evict another scenario's live
    // pubkey under `restart-mob/{profile}/{identity}`.
    let mut definition = MobDefinition::explicit(MobId::from(format!(
        "restart-mob-{}",
        meerkat_core::time_compat::new_uuid_v7()
    )));
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

async fn wait_for_history_contains_all(
    service: &dyn MobSessionService,
    session_id: &meerkat::SessionId,
    needles: &[&str],
    context: &str,
) -> String {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let blob = history_blob(service, session_id).await;
        if needles.iter().all(|needle| blob.contains(needle)) {
            return blob;
        }
        assert!(
            Instant::now() < deadline,
            "[{context}] timed out waiting for {needles:?} in history of {session_id}.\ncurrent history: {blob}"
        );
        sleep(Duration::from_millis(200)).await;
    }
}

async fn send_and_wait(
    handle: &MobHandle,
    service: &dyn MobSessionService,
    member_id: &str,
    prompt: &str,
    context: &str,
) {
    let identity = AgentIdentity::from(member_id);
    let session_id = handle
        .resolve_bridge_session_id(&identity)
        .await
        .expect("member session id");
    tokio::time::timeout(Duration::from_secs(30), async {
        handle
            .member(&identity)
            .await
            .expect("member handle")
            .send(prompt.to_string(), HandlingMode::Queue)
            .await
            .unwrap_or_else(|error| panic!("[{context}] send prompt to {member_id}: {error}"));
    })
    .await
    .unwrap_or_else(|_| panic!("[{context}] timed out sending prompt to member {member_id}"));
    // TestClient always answers "ok"; the unique prompt text appearing in the
    // durable history proves this specific turn was applied and persisted.
    wait_for_history_contains_all(service, &session_id, &[prompt], context).await;
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
            objective_id: None,
            content_taint: None,
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

fn assert_member_active(entry: &MobMemberListEntry, context: &str) {
    assert_eq!(
        entry.status,
        MobMemberStatus::Active,
        "[{context}] member {} should be Active; error={:?}",
        entry.agent_identity,
        entry.error
    );
}

fn assert_member_broken_after_authority_loss(entry: &MobMemberListEntry, context: &str) {
    assert_eq!(
        entry.status,
        MobMemberStatus::Broken,
        "[{context}] member {} should fail closed after RuntimeStore authority loss; error={:?}",
        entry.agent_identity,
        entry.error
    );
    assert!(
        entry
            .error
            .as_deref()
            .is_some_and(|error| error.contains("missing durable session snapshot")),
        "[{context}] member {} broke for an unexpected reason: {:?}",
        entry.agent_identity,
        entry.error
    );
}

/// Drive the full mob cold-restart shape:
///   lifetime 1: create mob, spawn lead+worker, run turns, peer message,
///               stop without archiving;
///   lifetime 2: rebuild service (runtime store per `reuse_runtime_store`),
///               MobBuilder::for_resume().resume(), assert members revive,
///               run post-restart turns + peer delivery, verify history.
async fn run_cold_restart_scenario(reuse_runtime_store: bool, lead_mode: MobRuntimeMode) {
    let temp = tempfile::tempdir().expect("temp dir");
    let paths = Paths::new(temp.path());

    let runtime_store_1: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());

    // ---------------- Lifetime 1 ----------------
    let (service_1, _store_1) = persistent_service(&paths, runtime_store_1.clone());
    let storage_1 = MobStorage::persistent(&paths.mob_db_path).expect("persistent mob storage");
    let definition = mob_definition(lead_mode);
    let lead_comms_name = format!("{}/lead/lead-1", definition.id);
    let handle_1 = MobBuilder::new(definition, storage_1)
        .with_session_service(service_1.clone())
        .with_default_llm_client(Arc::new(meerkat_client::TestClient::default()))
        .create()
        .await
        .expect("create persistent mob");

    handle_1
        .spawn_spec(SpawnMemberSpec::new("lead", AgentIdentity::from("lead-1")))
        .await
        .expect("spawn lead");
    handle_1
        .spawn_spec(SpawnMemberSpec::new("worker", AgentIdentity::from("w-1")))
        .await
        .expect("spawn worker");

    let lead_sid = handle_1
        .resolve_bridge_session_id(&AgentIdentity::from("lead-1"))
        .await
        .expect("lead session id");
    let w1_sid = handle_1
        .resolve_bridge_session_id(&AgentIdentity::from("w-1"))
        .await
        .expect("w1 session id");

    send_and_wait(
        &handle_1,
        service_1.as_ref(),
        "w-1",
        "PRE_RESTART_W1_TURN token-alpha",
        "lifetime1",
    )
    .await;

    // Append one ordinary durable System message before the cold stop. Resume
    // must preserve its exact role, position, and bytes without reinjection.
    let roster_append = meerkat_core::AppendSystemContextRequest {
        content: meerkat_core::CoreRenderable::text("peer roster: lead-1, w-1"),
        source: Some("comms:roster".to_string()),
        idempotency_key: Some("comms:roster:v1".to_string()),
    };
    service_1
        .append_system_context(&w1_sid, roster_append)
        .await
        .expect("append ordinary System message to worker");
    send_and_wait(
        &handle_1,
        service_1.as_ref(),
        "w-1",
        "PRE_RESTART_W1_TURN2 token-omega",
        "lifetime1",
    )
    .await;
    if lead_mode == MobRuntimeMode::TurnDriven {
        send_and_wait(
            &handle_1,
            service_1.as_ref(),
            "lead-1",
            "PRE_RESTART_LEAD_TURN token-beta",
            "lifetime1",
        )
        .await;
    }

    send_peer_message_direct(
        service_1.as_ref(),
        &w1_sid,
        &lead_comms_name,
        "PEER_TOKEN_GAMMA: moose",
    )
    .await;
    wait_for_history_contains_all(
        service_1.as_ref(),
        &lead_sid,
        &["PEER_TOKEN_GAMMA", "moose"],
        "lifetime1-peer",
    )
    .await;

    let lead_history_before = history_blob(service_1.as_ref(), &lead_sid).await;
    let w1_history_before = history_blob(service_1.as_ref(), &w1_sid).await;
    let w1_roster_position_before = service_1
        .load_authoritative_session(&w1_sid)
        .await
        .expect("load worker before restart")
        .expect("worker session before restart")
        .messages()
        .iter()
        .position(|message| {
            matches!(
                message,
                meerkat_core::Message::System(system)
                    if system.content == "peer roster: lead-1, w-1"
            )
        })
        .expect("interleaved roster System message before restart");
    eprintln!(
        "[lifetime1] lead history bytes={} w1 history bytes={}",
        lead_history_before.len(),
        w1_history_before.len()
    );

    // Cold stop: stop the actor without archiving/retiring anything, then drop
    // every live handle. `shutdown()` stops mob tasks; `discard_live_session`
    // drops the live agents the way a dead process would.
    handle_1
        .shutdown()
        .await
        .expect("shutdown mob before restart");
    for session_id in [&lead_sid, &w1_sid] {
        service_1
            .discard_live_session(session_id)
            .await
            .expect("discard live session before same-process restart");
    }
    drop(handle_1);
    drop(service_1);

    // ---------------- Lifetime 2 ----------------
    let runtime_store_2: Arc<dyn meerkat_runtime::RuntimeStore> = if reuse_runtime_store {
        runtime_store_1.clone()
    } else {
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new())
    };
    let (service_2, _store_2) = persistent_service(&paths, runtime_store_2);
    let storage_2 = MobStorage::persistent(&paths.mob_db_path).expect("reopen mob storage");
    let handle_2 = MobBuilder::for_resume(storage_2)
        .with_session_service(service_2.clone())
        .with_default_llm_client(Arc::new(meerkat_client::TestClient::default()))
        .notify_orchestrator_on_resume(false)
        .resume()
        .await
        .expect("mob resume after cold restart");

    let lead_after = member_entry(&handle_2, "lead-1").await;
    let w1_after = member_entry(&handle_2, "w-1").await;
    eprintln!("[lifetime2] lead={lead_after:?}");
    eprintln!("[lifetime2] w1={w1_after:?}");
    if !reuse_runtime_store {
        assert_member_broken_after_authority_loss(&lead_after, "post-reset");
        assert_member_broken_after_authority_loss(&w1_after, "post-reset");
        handle_2.shutdown().await.expect("final shutdown");
        return;
    }
    assert_member_active(&lead_after, "post-resume");
    assert_member_active(&w1_after, "post-resume");

    assert_eq!(
        handle_2
            .resolve_bridge_session_id(&AgentIdentity::from("lead-1"))
            .await,
        Some(lead_sid.clone()),
        "lead must keep its bridge session id across restart"
    );
    assert_eq!(
        handle_2
            .resolve_bridge_session_id(&AgentIdentity::from("w-1"))
            .await,
        Some(w1_sid.clone()),
        "worker must keep its bridge session id across restart"
    );

    // Pre-restart history must have survived the resume materialization.
    let lead_pre_needles: &[&str] = if lead_mode == MobRuntimeMode::TurnDriven {
        &["PRE_RESTART_LEAD_TURN", "PEER_TOKEN_GAMMA"]
    } else {
        &["PEER_TOKEN_GAMMA"]
    };
    wait_for_history_contains_all(
        service_2.as_ref(),
        &lead_sid,
        lead_pre_needles,
        "post-resume-lead-history",
    )
    .await;
    wait_for_history_contains_all(
        service_2.as_ref(),
        &w1_sid,
        &["PRE_RESTART_W1_TURN", "PRE_RESTART_W1_TURN2"],
        "post-resume-w1-history",
    )
    .await;

    // The ordinary System message must survive byte-for-byte at the same
    // interleaved transcript position; it is not a special row-zero prompt.
    let w1_resumed = service_2
        .load_authoritative_session(&w1_sid)
        .await
        .expect("load resumed worker session")
        .expect("worker session must survive restart");
    let w1_roster_position_after = w1_resumed
        .messages()
        .iter()
        .position(|message| {
            matches!(
                message,
                meerkat_core::Message::System(system)
                    if system.content == "peer roster: lead-1, w-1"
            )
        })
        .expect("interleaved roster System message after restart");
    assert_eq!(
        w1_roster_position_after, w1_roster_position_before,
        "ordinary System message must retain its exact transcript position across restart"
    );

    // First post-restart member turn: this drives the resumed projection into
    // the save guard against the persisted pre-restart row.
    send_and_wait(
        &handle_2,
        service_2.as_ref(),
        "w-1",
        "POST_RESTART_W1_TURN token-delta",
        "lifetime2",
    )
    .await;
    if lead_mode == MobRuntimeMode::TurnDriven {
        send_and_wait(
            &handle_2,
            service_2.as_ref(),
            "lead-1",
            "POST_RESTART_LEAD_TURN token-epsilon",
            "lifetime2",
        )
        .await;
    }

    // Post-restart peer delivery into the resumed lead session.
    send_peer_message_direct(
        service_2.as_ref(),
        &w1_sid,
        &lead_comms_name,
        "PEER_TOKEN_ZETA: otter",
    )
    .await;
    wait_for_history_contains_all(
        service_2.as_ref(),
        &lead_sid,
        &["PEER_TOKEN_ZETA", "otter"],
        "lifetime2-peer",
    )
    .await;

    // Full continuity check: everything from both lifetimes present.
    let lead_final = history_blob(service_2.as_ref(), &lead_sid).await;
    let lead_final_needles: &[&str] = if lead_mode == MobRuntimeMode::TurnDriven {
        &[
            "PRE_RESTART_LEAD_TURN",
            "PEER_TOKEN_GAMMA",
            "POST_RESTART_LEAD_TURN",
            "PEER_TOKEN_ZETA",
        ]
    } else {
        &["PEER_TOKEN_GAMMA", "PEER_TOKEN_ZETA"]
    };
    for needle in lead_final_needles {
        assert!(
            lead_final.contains(needle),
            "lead history lost '{needle}' across cold restart: {lead_final}"
        );
    }

    let lead_entry_final = member_entry(&handle_2, "lead-1").await;
    let w1_entry_final = member_entry(&handle_2, "w-1").await;
    assert_member_active(&lead_entry_final, "final");
    assert_member_active(&w1_entry_final, "final");

    handle_2.shutdown().await.expect("final shutdown");
}

/// Runtime store contents survive the restart (durable realm-style runtime
/// store, e.g. sqlite): the fresh runtime authority is rebuilt over the
/// pre-kill runtime rows.
#[tokio::test(flavor = "multi_thread")]
async fn mob_cold_restart_resume_with_durable_runtime_store() {
    run_cold_restart_scenario(true, MobRuntimeMode::TurnDriven).await;
}

/// RuntimeStore is the singular committed body/catalog authority. Replacing it
/// with a blank store is durable data loss, so stale non-authoritative
/// SessionStore projections must not resurrect either member.
#[tokio::test(flavor = "multi_thread")]
async fn mob_cold_restart_resume_with_reset_runtime_store() {
    run_cold_restart_scenario(false, MobRuntimeMode::TurnDriven).await;
}

/// AutonomousHost (keep-alive) lead + durable runtime store, graceful stop:
/// plain cold-restart resume coverage for the keep-alive lead mode. The
/// graceful shutdown converges the stores before restart, so this scenario
/// does NOT exercise the kill window (see
/// `mob_cold_restart_resume_after_kill_between_commit_points` for that); it
/// pins that ordinary autonomous-lead mobs revive across restarts.
#[tokio::test(flavor = "multi_thread")]
async fn mob_cold_restart_resume_autonomous_lead_durable_runtime_store() {
    run_cold_restart_scenario(true, MobRuntimeMode::AutonomousHost).await;
}

/// An explicitly resumed member may adopt its archived+Retired session after a
/// full host restart. Revival crosses the store-owned Retired terminal through
/// the exact machine lease while preserving the content-only body, bridge
/// session identity, and transcript.
#[tokio::test(flavor = "multi_thread")]
async fn mob_cold_restart_explicit_resume_revives_archived_retired_session_in_place() {
    let temp = tempfile::tempdir().expect("temp dir");
    let paths = Paths::new(temp.path());
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());

    let mut definition = mob_definition(MobRuntimeMode::TurnDriven);
    definition.id = MobId::from("retired-revival-mob");
    definition.orchestrator = None;
    definition.wiring = WiringRules {
        auto_wire_orchestrator: false,
        role_wiring: vec![],
    };

    // ---------------- Lifetime 1: create, use, then retire ----------------
    let (service_1, _store_1) = persistent_service(&paths, runtime_store.clone());
    let storage_1 = MobStorage::persistent(&paths.mob_db_path).expect("persistent mob storage");
    let handle_1 = MobBuilder::new(definition, storage_1)
        .with_session_service(service_1.clone())
        .with_default_llm_client(Arc::new(meerkat_client::TestClient::default()))
        .create()
        .await
        .expect("create persistent mob");

    let worker = AgentIdentity::from("w-1");
    handle_1
        .spawn_spec(SpawnMemberSpec::new("worker", worker.clone()))
        .await
        .expect("spawn worker before retirement");
    let original_session_id = handle_1
        .resolve_bridge_session_id(&worker)
        .await
        .expect("worker session id before retirement");
    send_and_wait(
        &handle_1,
        service_1.as_ref(),
        "w-1",
        "PRE_RETIRE_REVIVAL_TOKEN alpha",
        "before-retirement",
    )
    .await;

    handle_1
        .retire(worker.clone())
        .await
        .expect("retire worker before cold restart");

    assert!(
        service_1
            .load_persisted_session(&original_session_id)
            .await
            .expect("ordinary persisted-session read after retirement")
            .is_none(),
        "ordinary reads must continue to hide archived sessions"
    );
    let archived = service_1
        .load_revivable_retired_session(&original_session_id)
        .await
        .expect("load retired session through explicit revival seam")
        .expect("archived+Retired session must remain available for explicit revival");
    assert_eq!(archived.id(), &original_session_id);
    let archived_history = to_string(archived.messages()).expect("serialize archived history");
    assert!(
        archived_history.contains("PRE_RETIRE_REVIVAL_TOKEN alpha"),
        "retired session must retain its pre-retirement transcript: {archived_history}"
    );
    let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(&original_session_id);
    assert_eq!(
        meerkat_runtime::store::load_runtime_state(runtime_store.as_ref(), &runtime_id)
            .await
            .expect("load runtime state after retirement"),
        Some(meerkat_runtime::RuntimeState::Retired),
        "retirement must durably retire runtime authority"
    );

    assert_ne!(
        service_1
            .load_revivable_retired_session(&original_session_id)
            .await
            .expect("reload retired session")
            .expect("retired session remains revivable")
            .lifecycle_terminal(),
        Some(meerkat_core::session::SessionLifecycleTerminal::Archived),
        "archive authority must remain store-owned instead of being copied into the Session body"
    );

    handle_1
        .shutdown()
        .await
        .expect("shutdown mob before cold restart");
    drop(handle_1);
    drop(service_1);

    // ---------------- Lifetime 2: reopen, then explicitly revive ----------
    let (service_2, _store_2) = persistent_service(&paths, runtime_store.clone());
    let storage_2 = MobStorage::persistent(&paths.mob_db_path).expect("reopen mob storage");
    let handle_2 = MobBuilder::for_resume(storage_2)
        .with_session_service(service_2.clone())
        .with_default_llm_client(Arc::new(meerkat_client::TestClient::default()))
        .notify_orchestrator_on_resume(false)
        .resume()
        .await
        .expect("resume mob after cold restart");

    handle_2
        .spawn_spec(
            SpawnMemberSpec::new("worker", worker.clone())
                .with_resume_bridge_session_id(original_session_id.clone()),
        )
        .await
        .expect("explicitly revive archived+Retired worker session");

    assert_eq!(
        handle_2.resolve_bridge_session_id(&worker).await,
        Some(original_session_id.clone()),
        "revival must preserve the exact bridge-session id"
    );
    assert_member_active(&member_entry(&handle_2, "w-1").await, "after revival");

    let revived = service_2
        .load_authoritative_session(&original_session_id)
        .await
        .expect("load revived authoritative session")
        .expect("revived session must remain durable");
    assert_ne!(
        revived.lifecycle_terminal(),
        Some(meerkat_core::session::SessionLifecycleTerminal::Archived),
        "revival must preserve the content-only Session body"
    );
    assert!(
        service_2
            .load_persisted_session(&original_session_id)
            .await
            .expect("ordinary persisted-session read after revival")
            .is_some(),
        "revival must make the session visible through the ordinary active-session seam"
    );
    assert_ne!(
        meerkat_runtime::store::load_runtime_state(runtime_store.as_ref(), &runtime_id)
            .await
            .expect("load runtime state after revival"),
        Some(meerkat_runtime::RuntimeState::Retired),
        "revival must reactivate runtime authority"
    );

    wait_for_history_contains_all(
        service_2.as_ref(),
        &original_session_id,
        &["PRE_RETIRE_REVIVAL_TOKEN alpha"],
        "after-revival-history",
    )
    .await;
    send_and_wait(
        &handle_2,
        service_2.as_ref(),
        "w-1",
        "POST_REVIVAL_TOKEN omega",
        "after-revival-turn",
    )
    .await;
    wait_for_history_contains_all(
        service_2.as_ref(),
        &original_session_id,
        &["PRE_RETIRE_REVIVAL_TOKEN alpha", "POST_REVIVAL_TOKEN omega"],
        "final-revival-history",
    )
    .await;

    let after_turn = service_2
        .load_authoritative_session(&original_session_id)
        .await
        .expect("load authoritative session after revived turn")
        .expect("revived session remains authoritative after turn");
    assert_ne!(
        after_turn.lifecycle_terminal(),
        Some(meerkat_core::session::SessionLifecycleTerminal::Archived),
        "the revived live agent must preserve the content-only Session body"
    );
    assert!(
        service_2
            .load_persisted_session(&original_session_id)
            .await
            .expect("ordinary persisted-session read after revived turn")
            .is_some(),
        "the first revived turn must leave the session visible through the active read seam"
    );

    handle_2.shutdown().await.expect("final shutdown");
}

/// A broken wired member still needs its exact old-generation endpoint after
/// a full service restart. The endpoint is required to retire reciprocal trust
/// safely before spawning and wiring the replacement generation.
#[tokio::test(flavor = "multi_thread")]
async fn mob_cold_restart_partial_resume_respawns_wired_member() {
    let temp = tempfile::tempdir().expect("temp dir");
    let paths = Paths::new(temp.path());
    let runtime_store_1: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
    let (service_1, store_1) = persistent_service(&paths, runtime_store_1.clone());
    let storage_1 = MobStorage::persistent(&paths.mob_db_path).expect("persistent mob storage");
    let definition = mob_definition(MobRuntimeMode::TurnDriven);
    let replacement_w2_comms_name = format!("{}/worker/w-2", definition.id);
    let lead_comms_name = format!("{}/lead/lead-1", definition.id);
    let handle_1 = MobBuilder::new(definition, storage_1)
        .with_session_service(service_1.clone())
        .with_default_llm_client(Arc::new(meerkat_client::TestClient::default()))
        .create()
        .await
        .expect("create persistent mob");

    for (profile, identity) in [("lead", "lead-1"), ("worker", "w-1"), ("worker", "w-2")] {
        handle_1
            .spawn_spec(SpawnMemberSpec::new(profile, AgentIdentity::from(identity)))
            .await
            .unwrap_or_else(|error| panic!("spawn {identity}: {error}"));
    }

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
    let old_w2_peer_id = service_1
        .comms_runtime(&w2_sid)
        .await
        .expect("w2 comms runtime")
        .peer_id()
        .expect("w2 peer id");

    handle_1
        .shutdown()
        .await
        .expect("shutdown mob before cold restart");
    for session_id in [&lead_sid, &w1_sid, &w2_sid] {
        service_1
            .discard_live_session(session_id)
            .await
            .expect("discard live session before cold restart");
    }
    drop(handle_1);
    drop(service_1);
    store_1
        .delete(&w2_sid)
        .await
        .expect("delete stale w2 compatibility projection");
    drop(store_1);
    runtime_store_1
        .clear_session_snapshot(&meerkat_runtime::LogicalRuntimeId::for_session(&w2_sid))
        .await
        .expect("delete exact w2 RuntimeStore authority");

    let (service_2, _store_2) = persistent_service(&paths, runtime_store_1);
    let storage_2 = MobStorage::persistent(&paths.mob_db_path).expect("reopen mob storage");
    let handle_2 = MobBuilder::for_resume(storage_2)
        .with_session_service(service_2.clone())
        .with_default_llm_client(Arc::new(meerkat_client::TestClient::default()))
        .notify_orchestrator_on_resume(false)
        .resume()
        .await
        .expect("partial resume after cold restart");

    assert_member_active(&member_entry(&handle_2, "lead-1").await, "partial resume");
    assert_member_active(&member_entry(&handle_2, "w-1").await, "partial resume");
    let broken_w2 = member_entry(&handle_2, "w-2").await;
    assert_eq!(broken_w2.status, MobMemberStatus::Broken);

    let lead_before_repair = service_2
        .comms_runtime(&lead_sid)
        .await
        .expect("resumed lead comms runtime");
    assert!(
        lead_before_repair
            .peers()
            .await
            .iter()
            .all(|peer| peer.peer_id != old_w2_peer_id),
        "cold resume must not retain live trust for the missing old generation"
    );

    let repair = handle_2
        .respawn(AgentIdentity::from("w-2"), None)
        .await
        .expect("respawn broken wired w2");
    assert_eq!(repair.identity, AgentIdentity::from("w-2"));
    let new_w2_sid = handle_2
        .resolve_bridge_session_id(&AgentIdentity::from("w-2"))
        .await
        .expect("replacement w2 session id");
    assert_ne!(new_w2_sid, w2_sid);
    assert_member_active(&member_entry(&handle_2, "w-2").await, "after repair");

    let new_w2_runtime = service_2
        .comms_runtime(&new_w2_sid)
        .await
        .expect("replacement w2 comms runtime");
    let new_w2_peer_id = new_w2_runtime.peer_id().expect("replacement w2 peer id");
    assert_ne!(
        new_w2_peer_id, old_w2_peer_id,
        "replacement generation must rotate its exact comms endpoint"
    );
    let lead_peers_after_repair = lead_before_repair.peers().await;
    assert!(
        lead_peers_after_repair
            .iter()
            .all(|peer| peer.peer_id != old_w2_peer_id),
        "lead must not retain trust for the retired w2 generation"
    );
    assert!(
        lead_peers_after_repair.iter().any(|peer| {
            peer.peer_id == new_w2_peer_id
                && peer.name.as_str() == replacement_w2_comms_name.as_str()
        }),
        "lead must trust the exact replacement w2 endpoint"
    );
    assert!(
        new_w2_runtime
            .peers()
            .await
            .iter()
            .any(|peer| peer.name.as_str() == lead_comms_name.as_str()),
        "replacement w2 must trust the resumed lead"
    );

    handle_2.shutdown().await.expect("final shutdown");
}

// ===========================================================================
// Kill-window scenario: the host process dies BETWEEN the two non-atomic
// commit points of a single member turn:
//   (1) the agent loop's intra-turn best-effort checkpoint
//       (`Agent::save_session_best_effort` → injected `StoreCheckpointer`)
//       advances the canonical SQLite head with the completed turn, then
//   (2) the MeerkatMachine prepared boundary commit would atomically advance
//       the retained runtime authority plus receipt/input/lifecycle effects.
// A kill in that window leaves the physical head AHEAD of the exact retained
// runtime boundary. Cold restart must classify and recover that durable tail;
// neither projection may silently overwrite the other.
//
// The power cut is simulated by a RuntimeStore wrapper with a SURGICAL loud
// failure: input-admission writes (durable-before-ack) still land so the turn
// is admitted and runs into the window, while the prepared boundary commit
// fails loudly — exactly the durable state a host killed before that SQLite
// transaction leaves behind: the intra-turn checkpointer already advanced the
// store-owned physical head, runtime authority did not advance, and the
// admitted input did not settle. A blanket
// silent cut is wrong (it launders lost durable writes into "committed"
// in-memory state a real kill also destroys); a blanket loud cut is wrong
// (it rejects the turn at input admission, before the window).
// ===========================================================================

struct PowerCutRuntimeStore {
    inner: meerkat_runtime::store::SqliteRuntimeStore,
    cut: std::sync::atomic::AtomicBool,
    prepared_boundary_commit_rejections: std::sync::atomic::AtomicUsize,
    legacy_boundary_commit_rejections: std::sync::atomic::AtomicUsize,
}

impl PowerCutRuntimeStore {
    fn new(path: &Path) -> Self {
        Self {
            inner: meerkat_runtime::store::SqliteRuntimeStore::new_head_canonical(path)
                .expect("open co-tenant HeadCanonical runtime store"),
            cut: std::sync::atomic::AtomicBool::new(false),
            prepared_boundary_commit_rejections: std::sync::atomic::AtomicUsize::new(0),
            legacy_boundary_commit_rejections: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn set_cut(&self, cut: bool) {
        self.cut.store(cut, std::sync::atomic::Ordering::SeqCst);
    }

    fn is_cut(&self) -> bool {
        self.cut.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn prepared_boundary_commit_rejections(&self) -> usize {
        self.prepared_boundary_commit_rejections
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    fn legacy_boundary_commit_rejections(&self) -> usize {
        self.legacy_boundary_commit_rejections
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    fn record_prepared_boundary_commit_rejection(&self) {
        self.prepared_boundary_commit_rejections
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    fn record_legacy_boundary_commit_rejection(&self) {
        self.legacy_boundary_commit_rejections
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl meerkat_runtime::RuntimeStore for PowerCutRuntimeStore {
    fn session_authority_ops(&self) -> &dyn meerkat_runtime::store::RuntimeSessionAuthorityOps {
        self.inner.session_authority_ops()
    }

    fn session_persistence_profile(
        &self,
    ) -> meerkat_runtime::store::RuntimeSessionPersistenceProfile {
        meerkat_runtime::RuntimeStore::session_persistence_profile(&self.inner)
    }

    fn session_boundary_authority_read_cost(
        &self,
    ) -> meerkat_runtime::store::RuntimeSessionAuthorityReadCost {
        self.inner.session_boundary_authority_read_cost()
    }

    async fn commit_prepared_session_boundary(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        request: meerkat_runtime::store::PreparedRuntimeSessionCommit,
    ) -> Result<
        meerkat_runtime::store::PreparedRuntimeSessionCommitResult,
        meerkat_runtime::RuntimeStoreError,
    > {
        if self.is_cut() {
            self.record_prepared_boundary_commit_rejection();
            return Err(meerkat_runtime::RuntimeStoreError::WriteFailed(
                "power cut (commit_prepared_session_boundary): durable runtime-store write lost"
                    .to_string(),
            ));
        }
        self.inner
            .commit_prepared_session_boundary(runtime_id, request)
            .await
    }

    async fn load_session_boundary_authority(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
    ) -> Result<
        Option<meerkat_runtime::store::RuntimeSessionAuthority>,
        meerkat_runtime::RuntimeStoreError,
    > {
        self.inner.load_session_boundary_authority(runtime_id).await
    }

    async fn delete_runtime_session_catalog_entry(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
    ) -> Result<(), meerkat_runtime::RuntimeStoreError> {
        self.inner
            .delete_runtime_session_catalog_entry(runtime_id)
            .await
    }

    async fn load_runtime_session_catalog_entry(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
    ) -> Result<
        Option<meerkat_runtime::store::RuntimeSessionCatalogEntry>,
        meerkat_runtime::RuntimeStoreError,
    > {
        self.inner
            .load_runtime_session_catalog_entry(runtime_id)
            .await
    }

    async fn list_runtime_session_catalog_entries(
        &self,
        filter: meerkat_core::SessionFilter,
    ) -> Result<
        Vec<meerkat_runtime::store::RuntimeSessionCatalogEntry>,
        meerkat_runtime::RuntimeStoreError,
    > {
        self.inner
            .list_runtime_session_catalog_entries(filter)
            .await
    }

    async fn write_prepared_head_canonical_provisional_tail(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        prepared: meerkat_runtime::store::PreparedHeadCanonicalProvisionalTail,
    ) -> Result<
        meerkat_runtime::store::HeadCanonicalProvisionalTailAuthority,
        meerkat_runtime::RuntimeStoreError,
    > {
        self.inner
            .write_prepared_head_canonical_provisional_tail(runtime_id, prepared)
            .await
    }

    async fn load_head_canonical_provisional_tail(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
    ) -> Result<
        Option<meerkat_runtime::store::HeadCanonicalProvisionalTailAuthority>,
        meerkat_runtime::RuntimeStoreError,
    > {
        self.inner
            .load_head_canonical_provisional_tail(runtime_id)
            .await
    }

    async fn discard_head_canonical_provisional_tail(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        expected: &meerkat_runtime::store::HeadCanonicalProvisionalTailAuthority,
    ) -> Result<bool, meerkat_runtime::RuntimeStoreError> {
        self.inner
            .discard_head_canonical_provisional_tail(runtime_id, expected)
            .await
    }

    async fn load_durable_tail_recovery_source(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
    ) -> Result<
        Option<meerkat_runtime::store::PreparedDurableTailRecoverySource>,
        meerkat_runtime::RuntimeStoreError,
    > {
        self.inner
            .load_durable_tail_recovery_source(runtime_id)
            .await
    }

    async fn load_durable_tail_recovery_receipts(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        run_id: &meerkat_core::lifecycle::RunId,
    ) -> Result<
        Vec<meerkat_runtime::store::PreparedRecoveryReceiptSource>,
        meerkat_runtime::RuntimeStoreError,
    > {
        self.inner
            .load_durable_tail_recovery_receipts(runtime_id, run_id)
            .await
    }

    async fn load_committed_recovery_boundary(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        candidate_id: &str,
    ) -> Result<
        Option<meerkat_runtime::store::CommittedRecoveryBoundary>,
        meerkat_runtime::RuntimeStoreError,
    > {
        self.inner
            .load_committed_recovery_boundary(runtime_id, candidate_id)
            .await
    }

    fn supports_compaction_projection_outbox(&self) -> bool {
        meerkat_runtime::RuntimeStore::supports_compaction_projection_outbox(&self.inner)
    }

    async fn observe_machine_lifecycle(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
    ) -> Result<
        meerkat_runtime::store::MachineLifecycleObservation,
        meerkat_runtime::RuntimeStoreError,
    > {
        self.inner.observe_machine_lifecycle(runtime_id).await
    }

    async fn compare_and_swap_machine_lifecycle(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        expected: meerkat_runtime::store::MachineLifecycleExpectedVersion,
        replacement: meerkat_runtime::store::MachineLifecycleCommit,
    ) -> Result<
        meerkat_runtime::store::MachineLifecycleCasOutcome,
        meerkat_runtime::RuntimeStoreError,
    > {
        self.inner
            .compare_and_swap_machine_lifecycle(runtime_id, expected, replacement)
            .await
    }

    async fn compare_and_swap_machine_lifecycle_with_fence(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        expected: meerkat_runtime::store::MachineLifecycleExpectedVersion,
        replacement: meerkat_runtime::store::MachineLifecycleCommit,
        write_fence: Arc<dyn meerkat_runtime::store::RuntimeStoreWriteFence>,
    ) -> Result<
        meerkat_runtime::store::FencedMachineLifecycleCasOutcome,
        meerkat_runtime::RuntimeStoreError,
    > {
        self.inner
            .compare_and_swap_machine_lifecycle_with_fence(
                runtime_id,
                expected,
                replacement,
                write_fence,
            )
            .await
    }

    fn auth_authority_key(&self) -> Option<String> {
        meerkat_runtime::RuntimeStore::auth_authority_key(&self.inner)
    }

    fn persist_auth_oauth_flow_snapshot(
        &self,
        snapshot_json: &[u8],
    ) -> Result<(), meerkat_runtime::RuntimeStoreError> {
        meerkat_runtime::RuntimeStore::persist_auth_oauth_flow_snapshot(&self.inner, snapshot_json)
    }

    fn load_auth_oauth_flow_snapshot(
        &self,
    ) -> Result<Option<Vec<u8>>, meerkat_runtime::RuntimeStoreError> {
        meerkat_runtime::RuntimeStore::load_auth_oauth_flow_snapshot(&self.inner)
    }

    fn update_auth_oauth_flow_snapshot(
        &self,
        update: &mut meerkat_runtime::store::AuthOAuthFlowSnapshotUpdate<'_>,
    ) -> Result<(), meerkat_runtime::RuntimeStoreError> {
        meerkat_runtime::RuntimeStore::update_auth_oauth_flow_snapshot(&self.inner, update)
    }

    async fn commit_session_snapshot(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        session_delta: meerkat_runtime::SerializedSessionSnapshot,
    ) -> Result<(), meerkat_runtime::RuntimeStoreError> {
        if self.is_cut() {
            self.record_legacy_boundary_commit_rejection();
            return Err(meerkat_runtime::RuntimeStoreError::WriteFailed(
                "power cut (commit_session_snapshot): durable runtime-store write lost".to_string(),
            ));
        }
        self.inner
            .commit_session_snapshot(runtime_id, session_delta)
            .await
    }

    async fn commit_prepared_whole_blob_rewrite_boundary(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        boundary: meerkat_runtime::store::PreparedWholeBlobRewriteStoreParts,
    ) -> Result<meerkat_runtime::store::WholeBlobStoreAuthority, meerkat_runtime::RuntimeStoreError>
    {
        if self.is_cut() {
            self.record_legacy_boundary_commit_rejection();
            return Err(meerkat_runtime::RuntimeStoreError::WriteFailed(
                "power cut (commit_prepared_whole_blob_rewrite_boundary): durable runtime-store write lost".to_string(),
            ));
        }
        self.inner
            .commit_prepared_whole_blob_rewrite_boundary(runtime_id, boundary)
            .await
    }

    async fn atomic_apply(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        session_delta: Option<meerkat_runtime::SerializedSessionSnapshot>,
        receipt: meerkat_core::lifecycle::RunBoundaryReceipt,
        input_updates: Vec<meerkat_runtime::input_state::InputStatePersistenceRecord>,
        session_store_key: Option<meerkat_core::types::SessionId>,
    ) -> Result<(), meerkat_runtime::RuntimeStoreError> {
        // The cut is surgical: input-admission writes (no session delta)
        // still land, so the turn is admitted and runs; the run-boundary
        // commit (the write carrying the session snapshot delta) fails
        // loudly, modeling a host that dies at the boundary-commit write
        // AFTER the intra-turn checkpointer already wrote the durable row.
        if self.is_cut() && session_delta.is_some() {
            self.record_legacy_boundary_commit_rejection();
            return Err(meerkat_runtime::RuntimeStoreError::WriteFailed(
                "power cut (atomic_apply): durable runtime-store write lost".to_string(),
            ));
        }
        self.inner
            .atomic_apply(
                runtime_id,
                session_delta,
                receipt,
                input_updates,
                session_store_key,
            )
            .await
    }

    async fn load_input_states(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
    ) -> Result<Vec<meerkat_runtime::InputStateRow>, meerkat_runtime::RuntimeStoreError> {
        self.inner.load_input_states(runtime_id).await
    }
    /// Keep the legacy whole-blob lifecycle seam under the same cut as every
    /// other boundary write. Head-canonical recovery uses the dedicated
    /// prepared boundary seam; this forwarding remains part of the wrapper's
    /// truthful delegated capability surface.
    async fn atomic_apply_with_machine_lifecycle(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        session_delta: meerkat_runtime::SerializedSessionSnapshot,
        receipt: meerkat_core::lifecycle::RunBoundaryReceipt,
        machine_lifecycle: meerkat_runtime::store::MachineLifecycleCommit,
        input_updates: Vec<meerkat_runtime::input_state::InputStatePersistenceRecord>,
        session_store_key: meerkat_core::types::SessionId,
    ) -> Result<(), meerkat_runtime::RuntimeStoreError> {
        if self.is_cut() {
            self.record_legacy_boundary_commit_rejection();
            return Err(meerkat_runtime::RuntimeStoreError::WriteFailed(
                "power cut (atomic_apply_with_machine_lifecycle): durable runtime-store write lost"
                    .to_string(),
            ));
        }
        self.inner
            .atomic_apply_with_machine_lifecycle(
                runtime_id,
                session_delta,
                receipt,
                machine_lifecycle,
                input_updates,
                session_store_key,
            )
            .await
    }

    async fn load_committed_boundary_receipts(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        run_id: &meerkat_core::lifecycle::RunId,
    ) -> Result<Vec<meerkat_core::lifecycle::RunBoundaryReceipt>, meerkat_runtime::RuntimeStoreError>
    {
        self.inner
            .load_committed_boundary_receipts(runtime_id, run_id)
            .await
    }

    async fn load_input_states_with_versions(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
    ) -> Result<
        meerkat_runtime::store::PreparedRecoveryInputSnapshot,
        meerkat_runtime::RuntimeStoreError,
    > {
        self.inner.load_input_states_with_versions(runtime_id).await
    }

    async fn load_boundary_receipt(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        run_id: &meerkat_core::lifecycle::RunId,
        sequence: u64,
    ) -> Result<
        Option<meerkat_core::lifecycle::RunBoundaryReceipt>,
        meerkat_runtime::RuntimeStoreError,
    > {
        self.inner
            .load_boundary_receipt(runtime_id, run_id, sequence)
            .await
    }

    async fn load_session_snapshot(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
    ) -> Result<Option<std::sync::Arc<Vec<u8>>>, meerkat_runtime::RuntimeStoreError> {
        self.inner.load_session_snapshot(runtime_id).await
    }

    async fn load_pending_compaction_projections(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
    ) -> Result<Vec<meerkat_core::CompactionProjectionIntent>, meerkat_runtime::RuntimeStoreError>
    {
        self.inner
            .load_pending_compaction_projections(runtime_id)
            .await
    }

    async fn mark_compaction_projection_finalized(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        projection: &meerkat_core::CompactionProjectionId,
    ) -> Result<(), meerkat_runtime::RuntimeStoreError> {
        if self.is_cut() {
            return Err(meerkat_runtime::RuntimeStoreError::WriteFailed(
                "power cut (mark_compaction_projection_finalized): durable runtime-store write lost"
                    .to_string(),
            ));
        }
        self.inner
            .mark_compaction_projection_finalized(runtime_id, projection)
            .await
    }

    async fn clear_session_snapshot(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
    ) -> Result<(), meerkat_runtime::RuntimeStoreError> {
        if self.is_cut() {
            return Err(meerkat_runtime::RuntimeStoreError::WriteFailed(
                "power cut (clear_session_snapshot): durable runtime-store write lost".to_string(),
            ));
        }
        self.inner.clear_session_snapshot(runtime_id).await
    }

    async fn replace_session_snapshot_if_current(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        expected_current: &[u8],
        replacement: Vec<u8>,
    ) -> Result<bool, meerkat_runtime::RuntimeStoreError> {
        if self.is_cut() {
            return Err(meerkat_runtime::RuntimeStoreError::WriteFailed(
                "power cut (replace_session_snapshot_if_current): conditional write lost"
                    .to_string(),
            ));
        }
        self.inner
            .replace_session_snapshot_if_current(runtime_id, expected_current, replacement)
            .await
    }

    async fn clear_session_snapshot_if_current(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        expected_current: &[u8],
    ) -> Result<bool, meerkat_runtime::RuntimeStoreError> {
        if self.is_cut() {
            return Err(meerkat_runtime::RuntimeStoreError::WriteFailed(
                "power cut (clear_session_snapshot_if_current): conditional write lost".to_string(),
            ));
        }
        self.inner
            .clear_session_snapshot_if_current(runtime_id, expected_current)
            .await
    }

    async fn is_runtime_projection_quarantined(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
    ) -> Result<bool, meerkat_runtime::RuntimeStoreError> {
        self.inner
            .is_runtime_projection_quarantined(runtime_id)
            .await
    }

    async fn persist_input_state(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        state: &meerkat_runtime::input_state::InputStatePersistenceRecord,
    ) -> Result<(), meerkat_runtime::RuntimeStoreError> {
        // Input admission (durable-before-ack) still lands under the cut so
        // the turn is admitted and runs into the boundary-commit window.
        self.inner.persist_input_state(runtime_id, state).await
    }

    async fn persist_input_states_atomically(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        states: &[meerkat_runtime::input_state::InputStatePersistenceRecord],
    ) -> Result<(), meerkat_runtime::RuntimeStoreError> {
        // Batch input custody is the same durable-before-ack class as the
        // single-row operation above; it must still land in this surgical
        // boundary-commit cut harness.
        self.inner
            .persist_input_states_atomically(runtime_id, states)
            .await
    }

    fn input_state_batch_cas_implementation_profile(
        &self,
    ) -> meerkat_runtime::store::InputStateBatchCasImplementationProfile {
        self.inner.input_state_batch_cas_implementation_profile()
    }

    async fn compare_and_swap_input_states_atomically(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        expected: &[meerkat_runtime::input_state::StoredInputState],
        replacements: &[meerkat_runtime::input_state::InputStatePersistenceRecord],
    ) -> Result<meerkat_runtime::store::InputStateBatchCasOutcome, meerkat_runtime::RuntimeStoreError>
    {
        self.inner
            .compare_and_swap_input_states_atomically(runtime_id, expected, replacements)
            .await
    }

    async fn compare_and_swap_input_states_atomically_with_fence(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        expected: &[meerkat_runtime::input_state::StoredInputState],
        replacements: &[meerkat_runtime::input_state::InputStatePersistenceRecord],
        write_fence: std::sync::Arc<dyn meerkat_runtime::store::RuntimeStoreWriteFence>,
    ) -> Result<
        meerkat_runtime::store::FencedInputStateBatchCasOutcome,
        meerkat_runtime::RuntimeStoreError,
    > {
        self.inner
            .compare_and_swap_input_states_atomically_with_fence(
                runtime_id,
                expected,
                replacements,
                write_fence,
            )
            .await
    }

    async fn compare_and_swap_recovery_input_states_atomically(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        expected_revision: meerkat_runtime::store::RecoveryInputSetRevision,
        mutations: &[meerkat_runtime::store::RecoveryInputStateMutation],
    ) -> Result<meerkat_runtime::store::InputStateBatchCasOutcome, meerkat_runtime::RuntimeStoreError>
    {
        self.inner
            .compare_and_swap_recovery_input_states_atomically(
                runtime_id,
                expected_revision,
                mutations,
            )
            .await
    }

    async fn compare_and_swap_recovery_input_states_atomically_with_fence(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        expected_revision: meerkat_runtime::store::RecoveryInputSetRevision,
        mutations: &[meerkat_runtime::store::RecoveryInputStateMutation],
        write_fence: std::sync::Arc<dyn meerkat_runtime::store::RuntimeStoreWriteFence>,
    ) -> Result<
        meerkat_runtime::store::FencedInputStateBatchCasOutcome,
        meerkat_runtime::RuntimeStoreError,
    > {
        self.inner
            .compare_and_swap_recovery_input_states_atomically_with_fence(
                runtime_id,
                expected_revision,
                mutations,
                write_fence,
            )
            .await
    }

    async fn load_input_state(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        input_id: &meerkat_core::lifecycle::InputId,
    ) -> Result<
        Option<meerkat_runtime::input_state::StoredInputState>,
        meerkat_runtime::RuntimeStoreError,
    > {
        self.inner.load_input_state(runtime_id, input_id).await
    }

    async fn load_input_state_by_idempotency_key(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        key: &meerkat_runtime::IdempotencyKey,
    ) -> Result<
        Option<meerkat_runtime::store::ExactInputStateObservation>,
        meerkat_runtime::RuntimeStoreError,
    > {
        self.inner
            .load_input_state_by_idempotency_key(runtime_id, key)
            .await
    }

    async fn load_input_states_by_ids(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        input_ids: &[meerkat_core::lifecycle::InputId],
    ) -> Result<
        Vec<Option<meerkat_runtime::input_state::StoredInputState>>,
        meerkat_runtime::RuntimeStoreError,
    > {
        self.inner
            .load_input_states_by_ids(runtime_id, input_ids)
            .await
    }

    async fn load_pending_terminal_owner_ids_page(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        after: Option<&meerkat_core::lifecycle::InputId>,
        limit: usize,
    ) -> Result<Vec<meerkat_core::lifecycle::InputId>, meerkat_runtime::RuntimeStoreError> {
        self.inner
            .load_pending_terminal_owner_ids_page(runtime_id, after, limit)
            .await
    }

    async fn load_machine_lifecycle_record(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
    ) -> Result<Option<Vec<u8>>, meerkat_runtime::RuntimeStoreError> {
        self.inner.load_machine_lifecycle_record(runtime_id).await
    }

    async fn commit_machine_lifecycle(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        commit: meerkat_runtime::store::MachineLifecycleCommit,
        input_states: &[meerkat_runtime::input_state::InputStatePersistenceRecord],
    ) -> Result<(), meerkat_runtime::RuntimeStoreError> {
        if self.is_cut() {
            return Err(meerkat_runtime::RuntimeStoreError::WriteFailed(
                "power cut (commit_machine_lifecycle): durable runtime-store write lost"
                    .to_string(),
            ));
        }
        self.inner
            .commit_machine_lifecycle(runtime_id, commit, input_states)
            .await
    }

    async fn commit_unregister_finalization(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        finalization: meerkat_runtime::store::UnregisterFinalizationCommit,
    ) -> Result<(), meerkat_runtime::RuntimeStoreError> {
        if self.is_cut() {
            return Err(meerkat_runtime::RuntimeStoreError::WriteFailed(
                "power cut (commit_unregister_finalization): durable runtime-store write lost"
                    .to_string(),
            ));
        }
        self.inner
            .commit_unregister_finalization(runtime_id, finalization)
            .await
    }

    async fn persist_ops_lifecycle(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        snapshot: &meerkat_runtime::ops_lifecycle::PersistedOpsSnapshot,
    ) -> Result<(), meerkat_runtime::RuntimeStoreError> {
        if self.is_cut() {
            return Err(meerkat_runtime::RuntimeStoreError::WriteFailed(
                "power cut (persist_ops_lifecycle): durable runtime-store write lost".to_string(),
            ));
        }
        meerkat_runtime::RuntimeStore::persist_ops_lifecycle(&self.inner, runtime_id, snapshot)
            .await
    }

    async fn initialize_ops_lifecycle_if_absent(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        candidate: &meerkat_runtime::ops_lifecycle::PersistedOpsSnapshot,
    ) -> Result<
        meerkat_runtime::ops_lifecycle::PersistedOpsSnapshot,
        meerkat_runtime::RuntimeStoreError,
    > {
        if self.is_cut() {
            return Err(meerkat_runtime::RuntimeStoreError::WriteFailed(
                "power cut (initialize_ops_lifecycle_if_absent): durable runtime-store write lost"
                    .to_string(),
            ));
        }
        meerkat_runtime::RuntimeStore::initialize_ops_lifecycle_if_absent(
            &self.inner,
            runtime_id,
            candidate,
        )
        .await
    }

    async fn load_ops_lifecycle(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
    ) -> Result<
        Option<meerkat_runtime::ops_lifecycle::PersistedOpsSnapshot>,
        meerkat_runtime::RuntimeStoreError,
    > {
        meerkat_runtime::RuntimeStore::load_ops_lifecycle(&self.inner, runtime_id).await
    }

    async fn delete_ops_lifecycle(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
    ) -> Result<(), meerkat_runtime::RuntimeStoreError> {
        if self.is_cut() {
            return Err(meerkat_runtime::RuntimeStoreError::WriteFailed(
                "power cut (delete_ops_lifecycle): durable runtime-store write lost".to_string(),
            ));
        }
        meerkat_runtime::RuntimeStore::delete_ops_lifecycle(&self.inner, runtime_id).await
    }
}

async fn wait_for_row_contains(
    store: &dyn SessionStore,
    session_id: &meerkat::SessionId,
    needle: &str,
    context: &str,
) -> String {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let row = store
            .load(session_id)
            .await
            .expect("load durable session row");
        let blob = row
            .as_ref()
            .map(|session| to_string(session.messages()).expect("serialize row messages"))
            .unwrap_or_default();
        if blob.contains(needle) {
            return blob;
        }
        assert!(
            Instant::now() < deadline,
            "[{context}] timed out waiting for '{needle}' in durable row of {session_id}.\nrow: {blob}"
        );
        sleep(Duration::from_millis(100)).await;
    }
}

async fn wait_for_head_canonical_authority_convergence(
    store: &SqliteSessionStore,
    runtime_store: &dyn meerkat_runtime::RuntimeStore,
    session_id: &meerkat::SessionId,
    required_history: &str,
    context: &str,
) {
    let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(session_id);
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let physical_head = store
            .load_head(session_id)
            .await
            .expect("load physical canonical head");
        let authority = runtime_store
            .load_session_boundary_authority(&runtime_id)
            .await
            .expect("load retained runtime boundary authority");
        if let (Some(physical_head), Some(authority)) = (physical_head.as_ref(), authority.as_ref())
        {
            assert_eq!(
                authority.profile(),
                meerkat_runtime::store::RuntimeSessionPersistenceProfile::HeadCanonicalV1,
                "[{context}] runtime authority changed persistence profile"
            );
            let physical_token = meerkat_core::session_store::session_head_cas_token(physical_head)
                .expect("physical head has a canonical CAS token");
            let authority = authority
                .head_canonical()
                .expect("head-canonical runtime authority");
            if authority.boundary_head() == physical_head
                && authority.committed_head_token() == physical_token
            {
                let session = store
                    .load(session_id)
                    .await
                    .expect("load converged canonical session")
                    .expect("converged canonical session exists");
                let history =
                    to_string(session.messages()).expect("serialize converged session history");
                if history.contains(required_history) {
                    assert_eq!(
                        session.messages().len() as u64,
                        physical_head.message_count,
                        "[{context}] materialized session count differs from the authoritative physical head"
                    );
                    assert_eq!(
                        session
                            .transcript_content_digest()
                            .expect("converged session transcript digest"),
                        physical_head.head_revision,
                        "[{context}] materialized session digest differs from the authoritative physical head"
                    );
                    return;
                }
            }
        }
        assert!(
            Instant::now() < deadline,
            "[{context}] runtime authority did not converge to physical head containing \
             '{required_history}' for {session_id}; physical={physical_head:?}, authority={authority:?}"
        );
        sleep(Duration::from_millis(100)).await;
    }
}

/// Host dies between the intra-turn durable-row checkpoint and the runtime
/// boundary commit. The canonical SQLite head carries the final turn while the
/// co-tenant runtime authority still names its predecessor. Cold restart must
/// recover that exact durable tail through the real HeadCanonical transaction.
///
/// The test-support feature supplies the deliberately non-product crash-stop
/// command needed to quiesce lifetime 1 without graceful terminalization.
/// Both Cargo integration lanes invoke this exact target with the feature and
/// `--no-tests=fail`; the generated Bazel target also enables it.
#[cfg(feature = "test-support")]
#[tokio::test(flavor = "multi_thread")]
async fn mob_cold_restart_resume_after_kill_between_commit_points() {
    let temp = tempfile::tempdir().expect("temp dir");
    let paths = Paths::new(temp.path());

    let power_store = Arc::new(PowerCutRuntimeStore::new(&paths.realm_db_path));
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = power_store.clone();

    // ---------------- Lifetime 1 ----------------
    let (service_1, store_1) = persistent_head_canonical_service(&paths, runtime_store.clone());
    let storage_1 = MobStorage::persistent(&paths.mob_db_path).expect("persistent mob storage");
    let handle_1 = MobBuilder::new(mob_definition(MobRuntimeMode::TurnDriven), storage_1)
        .with_session_service(service_1.clone())
        .with_default_llm_client(Arc::new(meerkat_client::TestClient::default()))
        .create()
        .await
        .expect("create persistent mob");

    handle_1
        .spawn_spec(SpawnMemberSpec::new("lead", AgentIdentity::from("lead-1")))
        .await
        .expect("spawn lead");
    handle_1
        .spawn_spec(SpawnMemberSpec::new("worker", AgentIdentity::from("w-1")))
        .await
        .expect("spawn worker");

    let w1_sid = handle_1
        .resolve_bridge_session_id(&AgentIdentity::from("w-1"))
        .await
        .expect("w1 session id");

    // Turn A commits cleanly to the co-tenant canonical head and retained
    // runtime boundary in one SQLite resource.
    send_and_wait(
        &handle_1,
        service_1.as_ref(),
        "w-1",
        "PRE_CUT_W1_TURN token-alpha",
        "kill-window-lifetime1",
    )
    .await;
    wait_for_head_canonical_authority_convergence(
        store_1.as_ref(),
        runtime_store.as_ref(),
        &w1_sid,
        "PRE_CUT_W1_TURN",
        "pre-cut-boundary",
    )
    .await;
    let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(&w1_sid);
    let pre_cut_boundary_authority = runtime_store
        .load_session_boundary_authority(&runtime_id)
        .await
        .expect("load pre-cut runtime session authority")
        .expect("pre-cut runtime session authority present");

    // Power cut: durable-before-ack admission still lands, but the next
    // prepared session boundary is rejected as if the process died before
    // SQLite began that transaction.
    power_store.set_cut(true);

    // Turn B: the agent loop completes agent-side and the injected
    // StoreCheckpointer writes the durable row (commit point 1); the runtime
    // boundary commit (commit point 2) never becomes durable under the cut.
    let identity = AgentIdentity::from("w-1");
    let send_result = tokio::time::timeout(Duration::from_secs(30), async {
        handle_1
            .member(&identity)
            .await
            .expect("member handle")
            .send(
                "POST_CUT_W1_TURN token-omega".to_string(),
                HandlingMode::Queue,
            )
            .await
    })
    .await;
    eprintln!("[kill-window] send during cut => {send_result:?}");

    // Prove the divergence arose from real code paths: the physical canonical
    // head now carries turn B while the retained runtime boundary does not.
    let row_blob = wait_for_row_contains(
        store_1.as_ref(),
        &w1_sid,
        "POST_CUT_W1_TURN",
        "kill-window-row",
    )
    .await;
    eprintln!(
        "[kill-window] physical head advanced past retained boundary; row bytes={}",
        row_blob.len()
    );
    let committed_boundary_authority = runtime_store
        .load_session_boundary_authority(&runtime_id)
        .await
        .expect("load runtime session authority")
        .expect("runtime session authority present");
    assert_eq!(
        committed_boundary_authority, pre_cut_boundary_authority,
        "the rejected boundary commit must leave the exact pre-cut RuntimeStore authority current"
    );
    assert_eq!(
        committed_boundary_authority.profile(),
        meerkat_runtime::store::RuntimeSessionPersistenceProfile::HeadCanonicalV1
    );
    let committed_boundary_authority = committed_boundary_authority
        .head_canonical()
        .expect("head-canonical boundary authority");
    let boundary_head = committed_boundary_authority.boundary_head().clone();
    // Synchronize on the kill window actually having happened: the boundary
    // commit for turn B must have been rejected BEFORE power is restored,
    // otherwise a late commit could converge the stores and the scenario
    // degenerates to a plain resume (vacuous pass).
    {
        let deadline = Instant::now() + Duration::from_secs(20);
        while power_store.prepared_boundary_commit_rejections() == 0 {
            assert!(
                Instant::now() < deadline,
                "[kill-window] prepared boundary commit was never rejected under the cut"
            );
            sleep(Duration::from_millis(50)).await;
        }
    }
    // The durable-tail adoption precondition is store-owned: the physical head
    // is a strict continuation of the retained runtime boundary and binds the
    // exact materialized row by count, digest, and CAS token.
    let ahead_row = store_1
        .load(&w1_sid)
        .await
        .expect("load durable row before restart")
        .expect("durable row present before restart");
    let ahead_head = store_1
        .load_head(&w1_sid)
        .await
        .expect("load ahead physical head")
        .expect("ahead physical head exists");
    let ahead_token = meerkat_core::session_store::session_head_cas_token(&ahead_head)
        .expect("ahead physical head has a canonical token");
    assert_eq!(
        ahead_row.messages().len() as u64,
        ahead_head.message_count,
        "ahead physical head must bind the exact materialized row count"
    );
    assert_eq!(
        ahead_row
            .transcript_content_digest()
            .expect("ahead row transcript digest"),
        ahead_head.head_revision,
        "ahead physical head must bind the exact materialized transcript"
    );
    assert_ne!(
        ahead_token,
        committed_boundary_authority.committed_head_token(),
        "the physical head must remain strictly ahead of retained runtime authority"
    );
    assert!(
        ahead_head.message_count > boundary_head.message_count,
        "the ahead physical head must contain a strict transcript continuation"
    );

    // Quiesce every actor-owned volatile producer without writing graceful
    // cancellation. Dropping a MobHandle alone does not stop the actor because
    // internal clones retain its command channel.
    handle_1
        .crash_stop_preserving_durable_work_for_test()
        .await
        .expect("crash-stop lifetime-1 actor before restoring power");
    assert_eq!(
        power_store.legacy_boundary_commit_rejections(),
        0,
        "HeadCanonical lifetime must reach only the prepared boundary seam"
    );
    drop(handle_1);
    drop(service_1);
    drop(store_1);

    // ---------------- Lifetime 2 (power restored) ----------------
    power_store.set_cut(false);
    let (service_2, store_2) = persistent_head_canonical_service(&paths, runtime_store.clone());
    let storage_2 = MobStorage::persistent(&paths.mob_db_path).expect("reopen mob storage");
    let resume_result = MobBuilder::for_resume(storage_2)
        .with_session_service(service_2.clone())
        .with_default_llm_client(Arc::new(meerkat_client::TestClient::default()))
        .notify_orchestrator_on_resume(false)
        .resume()
        .await;

    let handle_2 = match resume_result {
        Ok(handle) => handle,
        Err(error) => {
            panic!("[kill-window] mob resume itself failed: {error}");
        }
    };

    let lead_after = member_entry(&handle_2, "lead-1").await;
    let w1_after = member_entry(&handle_2, "w-1").await;
    eprintln!("[kill-window] post-resume lead={lead_after:?}");
    eprintln!("[kill-window] post-resume w1={w1_after:?}");

    // Resume contract: the store-owned exact predecessor/head evidence is
    // classified and machine-authorized, then SQLite realizes the recovered
    // physical head, runtime authority, receipt, lifecycle, and input effects
    // in one transaction. A wrapper that merely advertises HeadCanonical or a
    // split JSONL/runtime topology cannot satisfy these assertions.
    assert_member_active(&w1_after, "kill-window-post-resume");
    wait_for_head_canonical_authority_convergence(
        store_2.as_ref(),
        runtime_store.as_ref(),
        &w1_sid,
        "POST_CUT_W1_TURN",
        "kill-window-recovered-boundary",
    )
    .await;

    // Drive a post-restart turn on the worker to force the first post-resume
    // persist through the save guard, then prove the ordinary boundary also
    // converges rather than merely advancing the physical checkpointer head.
    send_and_wait(
        &handle_2,
        service_2.as_ref(),
        "w-1",
        "POST_RESTART_W1_TURN token-sigma",
        "kill-window-lifetime2",
    )
    .await;
    wait_for_head_canonical_authority_convergence(
        store_2.as_ref(),
        runtime_store.as_ref(),
        &w1_sid,
        "POST_RESTART_W1_TURN",
        "kill-window-post-recovery-boundary",
    )
    .await;

    // No user input is lost and no completed durable tail is duplicated:
    // recovery adopts turn B exactly once and fences its input effects. All
    // three inputs land in committed history exactly once; either laundering
    // a second copy of the recovered turn or redelivering its already-settled
    // input would show up as a duplicate.
    let w1_final = wait_for_history_contains_all(
        service_2.as_ref(),
        &w1_sid,
        &[
            "PRE_CUT_W1_TURN",
            "POST_CUT_W1_TURN",
            "POST_RESTART_W1_TURN",
        ],
        "kill-window-final-history",
    )
    .await;
    for needle in [
        "PRE_CUT_W1_TURN",
        "POST_CUT_W1_TURN",
        "POST_RESTART_W1_TURN",
    ] {
        let occurrences = w1_final.matches(needle).count();
        assert_eq!(
            occurrences, 1,
            "'{needle}' must appear exactly once in committed history              (laundering or duplicate redelivery detected): {w1_final}"
        );
    }
    // The machine-authorized durable tail is the already-existing physical
    // head at resume. It must therefore precede the fresh post-resume input;
    // A,C,B would prove discard/redelivery rather than exact adoption.
    let pre_cut_at = w1_final.find("PRE_CUT_W1_TURN").expect("pre-cut present");
    let post_cut_at = w1_final.find("POST_CUT_W1_TURN").expect("post-cut present");
    let post_restart_at = w1_final
        .find("POST_RESTART_W1_TURN")
        .expect("post-restart present");
    assert!(
        pre_cut_at < post_cut_at && post_cut_at < post_restart_at,
        "durable tail must remain ordered before fresh post-restart work: {w1_final}"
    );
    assert_member_active(&member_entry(&handle_2, "w-1").await, "kill-window-final");
}

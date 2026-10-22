//! H3 repro harness: mob member revival across a cold host restart.
//!
//! Mirrors `smoke_mob_resume.rs` (the live-provider mob resume smoke) with
//! deterministic mock LLM clients so the mob resume-restore path
//! (`MobBuilder::for_resume` → `reconcile_resume` → `build_resumed_agent_config`
//! → `provision_member` → `create_session` → `persist_full_session_or_discard_live`)
//! is exercised without API keys.
//!
//! Pins transcript continuity and the generated runtime placement required by
//! Live admission. Restoring a session actor alone must not report a usable
//! member while its runtime identity, generation, and fence remain unbound.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[cfg(feature = "test-support")]
mod support;

use meerkat::{
    AgentFactory, Config, FactoryAgentBuilder, SessionHistoryQuery, SessionServiceControlExt,
    SessionStore,
};
#[cfg(feature = "test-support")]
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

#[cfg(feature = "test-support")]
use support::FailingOnceSessionService;

fn openai_test_client() -> Arc<meerkat_client::TestClient> {
    Arc::new(meerkat_client::TestClient::for_provider(
        meerkat_core::Provider::OpenAI,
    ))
}

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
    builder.default_llm_client = Some(openai_test_client());
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
    builder.default_llm_client = Some(openai_test_client());
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
            model_fallback: None,
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
            model_fallback: None,
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

async fn admit_test_live_profile(
    runtime: &meerkat_runtime::MeerkatMachine,
    session_id: &meerkat_core::SessionId,
    identity: &meerkat_core::SessionLlmIdentity,
) -> meerkat_core::LiveChannelId {
    let channel_id = meerkat_core::LiveChannelId::new("restored-member-live");
    let admission = runtime
        .resolve_live_open_admission(session_id, &channel_id, identity)
        .await
        .expect("Live open admission");
    assert!(admission.channel_open_authority().is_some());
    let selection =
        meerkat_runtime::live_execution::LiveExecutionProfileSelection::from_public_profile(
            "cold-restart-client-context",
            meerkat_core::LiveExecutionMode::ClientContext,
            meerkat_core::LiveExecutionCapabilities {
                function_bridge: false,
                client_context: true,
            },
        )
        .expect("test host profile");
    runtime
        .resolve_live_execution_profile_admission(session_id, &channel_id, &selection)
        .await
        .expect("Live execution profile admission");
    channel_id
}

#[tokio::test]
async fn live_staging_reports_missing_runtime_placement_without_logging_identity_material() {
    use tracing::instrument::WithSubscriber;

    #[derive(Clone)]
    struct LogBuffer(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for LogBuffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("log buffer").extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
    let session_id = meerkat_core::SessionId::new();
    runtime
        .prepare_local_session_bindings(session_id.clone())
        .await
        .expect("prepare local resources without member placement");
    let identity = meerkat_core::SessionLlmIdentity {
        model: "gpt-live-1".to_string(),
        provider: meerkat_core::Provider::OpenAI,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: None,
    };
    let channel_id = admit_test_live_profile(&runtime, &session_id, &identity).await;
    let logs = Arc::new(std::sync::Mutex::new(Vec::new()));
    let writer = LogBuffer(logs.clone());
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_max_level(tracing::Level::WARN)
        .with_writer(move || writer.clone())
        .finish();
    let error = runtime
        .stage_experimental_live_execution(&session_id, &channel_id, 0)
        .with_subscriber(subscriber)
        .await
        .expect_err("local resources alone cannot admit Live execution");
    assert!(matches!(
        error,
        meerkat_runtime::RuntimeDriverError::ValidationFailed { ref reason }
            if reason == "experimental live staging has no active runtime identity"
    ));
    assert!(
        runtime
            .live_execution_channel_phase(&session_id, &channel_id)
            .await
            .expect("read stage")
            .is_none(),
        "failed placement admission must not mint pending channel custody"
    );
    let logs = String::from_utf8(logs.lock().expect("log buffer").clone()).expect("UTF-8 logs");
    assert!(logs.contains("live_execution_staging"), "{logs}");
    assert!(logs.contains("missing_runtime_identity"), "{logs}");
    assert!(!logs.contains(&identity.model), "{logs}");
    assert!(!logs.contains("OpenAI"), "{logs}");
    runtime
        .abandon_live_open_admission(&session_id, &channel_id)
        .await
        .expect("release provider-free Live admission");
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
            in_reply_to: None,
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

    let runtime_store_1: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
        meerkat_runtime::store::SqliteRuntimeStore::new_whole_blob(&paths.realm_db_path)
            .expect("open durable runtime store"),
    );

    // ---------------- Lifetime 1 ----------------
    let (service_1, _store_1) = persistent_service(&paths, runtime_store_1.clone());
    let storage_1 = MobStorage::persistent(&paths.mob_db_path).expect("persistent mob storage");
    let definition = mob_definition(lead_mode);
    let lead_comms_name = format!("{}/lead/lead-1", definition.id);
    let handle_1 = MobBuilder::new(definition, storage_1)
        .with_session_service(service_1.clone())
        .with_default_llm_client(openai_test_client())
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
    let runtime_1 = service_1
        .acquire_runtime_adapter(None)
        .expect("acquire runtime authority")
        .expect("runtime adapter");
    let initial_binding = runtime_1
        .live_webrtc_runtime_binding(&w1_sid)
        .await
        .expect("initial runtime binding")
        .expect("fresh member must have a generated runtime binding");
    let initial_lead_binding = runtime_1
        .live_webrtc_runtime_binding(&lead_sid)
        .await
        .expect("initial lead runtime binding")
        .expect("fresh lead must have a generated runtime binding");
    drop(runtime_1);

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
    drop(runtime_store_1);

    // ---------------- Lifetime 2 ----------------
    let runtime_store_2: Arc<dyn meerkat_runtime::RuntimeStore> = if reuse_runtime_store {
        Arc::new(
            meerkat_runtime::store::SqliteRuntimeStore::new_whole_blob(&paths.realm_db_path)
                .expect("reopen durable runtime store"),
        )
    } else {
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new())
    };
    let (service_2, _store_2) = persistent_service(&paths, runtime_store_2);
    let runtime_2 = service_2
        .acquire_runtime_adapter(None)
        .expect("acquire runtime authority")
        .expect("new runtime adapter");
    assert!(
        runtime_2
            .current_executor_attachment_witness(&w1_sid)
            .await
            .is_none(),
        "a cold host must begin without the old executor attachment"
    );
    assert!(
        runtime_2
            .live_webrtc_runtime_binding(&w1_sid)
            .await
            .is_err(),
        "restoration must start from an unregistered runtime, not a warm binding"
    );
    let storage_2 = MobStorage::persistent(&paths.mob_db_path).expect("reopen mob storage");
    let handle_2 = MobBuilder::for_resume(storage_2)
        .with_session_service(service_2.clone())
        .with_default_llm_client(openai_test_client())
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
        // Authority is the generation/fence tuple a successful placement
        // mints. The runtime may hold a shell registration for the session
        // while the failed restore is disposed (observed once on the release
        // validation executor: `session_dsl_state` resolved after the members
        // were already Broken), so the property asserted here is "no tuple",
        // not "no registration"; the pre-resume assertion above already pins
        // that restoration starts unregistered.
        let w1_binding_after_reset = runtime_2.live_webrtc_runtime_binding(&w1_sid).await;
        eprintln!("[lifetime2] w1 runtime binding after reset={w1_binding_after_reset:?}");
        assert!(
            !matches!(w1_binding_after_reset, Ok(Some(_))),
            "an absent durable member must not acquire runtime authority: {w1_binding_after_reset:?}"
        );
        assert!(matches!(
            handle_2.member(&AgentIdentity::from("w-1")).await,
            Err(meerkat_mob::MobError::MemberRestoreFailed { .. })
        ));
        handle_2.shutdown().await.expect("final shutdown");
        return;
    }
    assert_member_active(&lead_after, "post-resume");
    assert_member_active(&w1_after, "post-resume");
    assert!(
        runtime_2
            .current_executor_attachment_witness(&w1_sid)
            .await
            .is_some(),
        "restoration must commit the exact session executor attachment as well as placement"
    );
    let resumed_binding = runtime_2
        .live_webrtc_runtime_binding(&w1_sid)
        .await
        .expect("resumed runtime binding")
        .expect("restored member must have a generated runtime binding before Live admission");
    assert_eq!(resumed_binding.generation, initial_binding.generation);
    assert_eq!(resumed_binding.fence, initial_binding.fence);
    let resumed_lead_binding = runtime_2
        .live_webrtc_runtime_binding(&lead_sid)
        .await
        .expect("resumed lead runtime binding")
        .expect("restored lead must have a generated runtime binding in either runtime mode");
    assert_eq!(
        resumed_lead_binding.generation,
        initial_lead_binding.generation
    );
    assert_eq!(resumed_lead_binding.fence, initial_lead_binding.fence);

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

    // Exercise the same generated admission/staging seam as public GPT Live,
    // without contacting a provider or granting transport custody.
    let identity = service_2
        .load_authoritative_session(&w1_sid)
        .await
        .expect("authoritative restored session")
        .expect("restored session")
        .session_metadata()
        .expect("restored session metadata")
        .llm_identity();
    let channel_id = admit_test_live_profile(&runtime_2, &w1_sid, &identity).await;
    let staged = runtime_2
        .stage_experimental_live_execution(&w1_sid, &channel_id, 0)
        .await
        .expect("restored member must admit Live with its generated runtime placement");
    assert_eq!(staged.binding().generation(), initial_binding.generation);
    assert_eq!(staged.binding().fence_token(), initial_binding.fence);
    let member_snapshot = handle_2
        .member_status(&AgentIdentity::from("w-1"))
        .await
        .expect("restored member snapshot");
    let (member_runtime_id, _) = member_snapshot
        .runtime_identity_fields()
        .expect("restored member incarnation");
    assert_eq!(
        staged.binding().runtime_id().to_string(),
        member_runtime_id.to_string()
    );
    runtime_2
        .abandon_live_open_admission(&w1_sid, &channel_id)
        .await
        .expect("release provider-free Live admission");
    handle_2.shutdown().await.expect("final shutdown");
}

/// Fresh runtime authority is rebuilt from a reopened SQLite RuntimeStore,
/// rather than retaining the previous process's in-memory store or bindings.
#[tokio::test(flavor = "multi_thread")]
async fn mob_cold_restart_resume_with_durable_runtime_store() {
    run_cold_restart_scenario(true, MobRuntimeMode::TurnDriven).await;
}

/// Collect a member's agent events until its first run completes.
async fn collect_until_run_completed(
    stream: &mut meerkat_core::comms::EventStream,
    context: &str,
) -> Vec<meerkat_core::event::EventEnvelope<meerkat_core::AgentEvent>> {
    use futures::StreamExt as _;
    let mut events = Vec::new();
    tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(envelope) = stream.next().await {
            let completed = matches!(
                envelope.payload,
                meerkat_core::AgentEvent::RunCompleted { .. }
            );
            events.push(envelope);
            if completed {
                return;
            }
        }
        panic!("[{context}] agent event stream ended before RunCompleted");
    })
    .await
    .unwrap_or_else(|_| panic!("[{context}] timed out waiting for RunCompleted: {events:?}"));
    events
}

/// Issue #1236: after an rpc_gateway-style cold restart, a host that attaches
/// to a restored member through `MobHandle` only after the member's first run
/// already ran still observes that run from its first `RunStarted` /
/// `TurnStarted`, with no gap marker. A cursor resumes after an observed
/// sequence, and a member whose session actor is gone surfaces as the typed
/// `MemberSessionNotLive` instead of `Internal(String)`.
#[tokio::test(flavor = "multi_thread")]
async fn restored_member_first_run_replays_to_a_late_subscriber() {
    use meerkat_core::comms::SessionEventCursor;
    let temp = tempfile::tempdir().expect("temp dir");
    let paths = Paths::new(temp.path());
    let worker = AgentIdentity::from("w-1");

    // ---------------- Lifetime 1 ----------------
    let runtime_store_1: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
        meerkat_runtime::store::SqliteRuntimeStore::new_whole_blob(&paths.realm_db_path)
            .expect("open durable runtime store"),
    );
    let (service_1, _store_1) = persistent_service(&paths, runtime_store_1.clone());
    let storage_1 = MobStorage::persistent(&paths.mob_db_path).expect("persistent mob storage");
    let handle_1 = MobBuilder::new(mob_definition(MobRuntimeMode::TurnDriven), storage_1)
        .with_session_service(service_1.clone())
        .with_default_llm_client(openai_test_client())
        .create()
        .await
        .expect("create persistent mob");
    handle_1
        .spawn_spec(SpawnMemberSpec::new("worker", worker.clone()))
        .await
        .expect("spawn worker");
    let w1_sid = handle_1
        .resolve_bridge_session_id(&worker)
        .await
        .expect("worker session id");
    send_and_wait(
        &handle_1,
        service_1.as_ref(),
        "w-1",
        "PRE_RESTART_REPLAY_TURN",
        "lifetime1",
    )
    .await;
    handle_1
        .shutdown()
        .await
        .expect("shutdown mob before restart");
    service_1
        .discard_live_session(&w1_sid)
        .await
        .expect("discard live session before restart");
    drop(handle_1);
    drop(service_1);
    drop(runtime_store_1);

    // ---------------- Lifetime 2 ----------------
    let runtime_store_2: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
        meerkat_runtime::store::SqliteRuntimeStore::new_whole_blob(&paths.realm_db_path)
            .expect("reopen durable runtime store"),
    );
    let (service_2, _store_2) = persistent_service(&paths, runtime_store_2);
    let storage_2 = MobStorage::persistent(&paths.mob_db_path).expect("reopen mob storage");
    let handle_2 = MobBuilder::for_resume(storage_2)
        .with_session_service(service_2.clone())
        .with_default_llm_client(openai_test_client())
        .notify_orchestrator_on_resume(false)
        .resume()
        .await
        .expect("mob resume after cold restart");
    assert_member_active(&member_entry(&handle_2, "w-1").await, "lifetime2");

    // The restored actor's first run completes before any host attaches.
    send_and_wait(
        &handle_2,
        service_2.as_ref(),
        "w-1",
        "POST_RESTART_REPLAY_TURN",
        "lifetime2",
    )
    .await;

    let replayed = handle_2
        .subscribe_agent_events_from(&worker, SessionEventCursor::Earliest)
        .await
        .expect("late earliest subscription through MobHandle");
    let restored_actor = replayed
        .actor
        .clone()
        .expect("a local member subscription names its actor incarnation");
    assert!(restored_actor.is_live());
    let epoch = replayed
        .epoch
        .expect("a local member subscription names its sequence space");
    let mut replayed = replayed.stream;
    let events = collect_until_run_completed(&mut replayed, "earliest").await;
    let payloads: Vec<&meerkat_core::AgentEvent> =
        events.iter().map(|envelope| &envelope.payload).collect();
    assert!(
        !payloads
            .iter()
            .any(|payload| matches!(payload, meerkat_core::AgentEvent::StreamTruncated { .. })),
        "the restored actor's head is retained, so no gap marker: {payloads:?}"
    );
    let run_started = payloads
        .iter()
        .position(|payload| matches!(payload, meerkat_core::AgentEvent::RunStarted { .. }))
        .unwrap_or_else(|| panic!("late subscriber missed RunStarted: {payloads:?}"));
    let turn_started = payloads
        .iter()
        .position(|payload| matches!(payload, meerkat_core::AgentEvent::TurnStarted { .. }))
        .unwrap_or_else(|| panic!("late subscriber missed TurnStarted: {payloads:?}"));
    assert!(run_started < turn_started, "{payloads:?}");
    assert!(
        events.windows(2).all(|pair| pair[0].seq < pair[1].seq),
        "replayed envelopes arrive in sequence order: {events:?}"
    );

    // A cursor resumes strictly after an observed sequence.
    let run_started_seq = events[run_started].seq;
    let mut resumed = handle_2
        .subscribe_agent_events_from(
            &worker,
            SessionEventCursor::After {
                epoch,
                seq: run_started_seq,
            },
        )
        .await
        .expect("cursor subscription through MobHandle")
        .stream;
    let resumed_events = collect_until_run_completed(&mut resumed, "after").await;
    assert_eq!(
        resumed_events
            .iter()
            .map(|envelope| envelope.seq)
            .collect::<Vec<_>>(),
        events
            .iter()
            .map(|envelope| envelope.seq)
            .filter(|seq| *seq > run_started_seq)
            .collect::<Vec<_>>(),
        "a cursor replays exactly the envelopes after it"
    );

    // A cursor from another sequence space is rejected typed.
    let tail = events.last().map_or(0, |envelope| envelope.seq);
    let ahead = match handle_2
        .subscribe_agent_events_from(
            &worker,
            SessionEventCursor::After {
                epoch,
                seq: tail + 1_000,
            },
        )
        .await
    {
        Ok(_) => panic!("a cursor ahead of the tail must be rejected"),
        Err(error) => error,
    };
    assert!(
        matches!(
            ahead,
            meerkat_mob::MobError::AgentEventCursorRejected {
                reason: meerkat_core::comms::SessionEventCursorRejection::AheadOfTail { .. },
                ..
            }
        ),
        "{ahead:?}"
    );

    // A cursor from another sequence space (a cursor carried across a
    // restart) is rejected typed rather than skipping the new space's
    // first events.
    let foreign = match handle_2
        .subscribe_agent_events_from(
            &worker,
            SessionEventCursor::After {
                epoch: meerkat_core::comms::SessionEventEpoch::new(),
                seq: 1,
            },
        )
        .await
    {
        Ok(_) => panic!("a cursor from another sequence space must be rejected"),
        Err(error) => error,
    };
    assert!(
        matches!(
            foreign,
            meerkat_mob::MobError::AgentEventCursorRejected {
                reason: meerkat_core::comms::SessionEventCursorRejection::EpochMismatch { .. },
                ..
            }
        ),
        "{foreign:?}"
    );

    // Cross-actor continuity through MobHandle: the member's actor is
    // replaced in the same process (stop, discard, resume on the same
    // service). The successor continues the session's sequence space, so
    // the predecessor's cursor stays valid and nothing the successor
    // publishes is skipped.
    handle_2
        .shutdown()
        .await
        .expect("shutdown before in-process resume");
    service_2
        .discard_live_session(&w1_sid)
        .await
        .expect("discard the restored actor");
    assert!(
        !restored_actor.is_live(),
        "discarding the actor revokes the subscription's witness"
    );
    drop(handle_2);
    let storage_3 = MobStorage::persistent(&paths.mob_db_path).expect("reopen mob storage");
    let handle_3 = MobBuilder::for_resume(storage_3)
        .with_session_service(service_2.clone())
        .with_default_llm_client(openai_test_client())
        .notify_orchestrator_on_resume(false)
        .resume()
        .await
        .expect("in-process mob resume");
    send_and_wait(
        &handle_3,
        service_2.as_ref(),
        "w-1",
        "SUCCESSOR_REPLAY_TURN",
        "successor",
    )
    .await;
    let successor = handle_3
        .subscribe_agent_events_from(&worker, SessionEventCursor::After { epoch, seq: tail })
        .await
        .expect("the predecessor's cursor is valid against the successor");
    assert_eq!(
        successor.epoch,
        Some(epoch),
        "one sequence space in-process"
    );
    let successor_actor = successor.actor.clone().expect("successor actor");
    assert!(
        successor_actor.is_live() && !restored_actor.is_live(),
        "the subscription names the new incarnation"
    );
    let mut successor_stream = successor.stream;
    let successor_events = collect_until_run_completed(&mut successor_stream, "successor").await;
    assert!(
        successor_events
            .iter()
            .filter(|envelope| envelope.seq != 0)
            .all(|envelope| envelope.seq > tail),
        "the successor never reuses the predecessor's sequences: {successor_events:?}"
    );
    assert!(
        successor_events.iter().any(|envelope| matches!(
            envelope.payload,
            meerkat_core::AgentEvent::RunStarted { .. }
        )),
        "the successor's run is replayed from RunStarted: {successor_events:?}"
    );

    // A member whose session actor is gone is a typed not-live outcome.
    service_2
        .discard_live_session(&w1_sid)
        .await
        .expect("discard the successor actor");
    let not_live = match handle_3.subscribe_agent_events(&worker).await {
        Ok(_) => panic!("a member without a live session actor cannot be subscribed"),
        Err(error) => error,
    };
    assert!(
        matches!(
            &not_live,
            meerkat_mob::MobError::MemberSessionNotLive { agent_identity, session_id }
                if agent_identity == &worker && session_id == &w1_sid
        ),
        "{not_live:?}"
    );
    assert!(not_live.is_missing_target());
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

async fn member_generation(handle: &MobHandle, identity: &AgentIdentity, label: &str) -> u64 {
    handle
        .get_member(identity)
        .await
        .unwrap_or_else(|error| panic!("{label}: read the member: {error}"))
        .unwrap_or_else(|| panic!("{label}: member is on the roster"))
        .generation
        .get()
}

/// A member generation never repeats within a mob incarnation, across a reset
/// and a restart too. Resume replays only the events after the last
/// `MobReset`, so before the generation high-water came from the whole log a
/// worker re-spawned after a reset and a cold restart started again at
/// generation 0: the generation, runtime id and member address its pre-reset
/// incarnation already had. A worker re-spawned after a reset without a
/// restart is the control: it continues its generation in-process. The
/// resume then replays that continued generation as the control's first one
/// after the reset, which the replay used to refuse, so the mob could not
/// resume at all.
#[tokio::test(flavor = "multi_thread")]
async fn mob_reset_then_cold_restart_respawn_continues_the_member_generation() {
    let temp = tempfile::tempdir().expect("temp dir");
    let paths = Paths::new(temp.path());
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());

    let mut definition = mob_definition(MobRuntimeMode::TurnDriven);
    definition.id = MobId::from("reset-generation-mob");
    definition.orchestrator = None;
    definition.wiring = WiringRules {
        auto_wire_orchestrator: false,
        role_wiring: vec![],
    };

    // ---------------- Lifetime 1: spawn, reset, control respawn ----------
    let (service_1, _store_1) = persistent_service(&paths, runtime_store.clone());
    let storage_1 = MobStorage::persistent(&paths.mob_db_path).expect("persistent mob storage");
    let handle_1 = MobBuilder::new(definition, storage_1)
        .with_session_service(service_1.clone())
        .with_default_llm_client(openai_test_client())
        .create()
        .await
        .expect("create persistent mob");

    let worker = AgentIdentity::from("w-1");
    let control = AgentIdentity::from("w-control");
    for identity in [&worker, &control] {
        handle_1
            .spawn_spec(SpawnMemberSpec::new("worker", identity.clone()))
            .await
            .expect("spawn before the reset");
    }
    let worker_before = member_generation(&handle_1, &worker, "worker before the reset").await;
    handle_1.reset().await.expect("reset the mob");
    handle_1
        .spawn_spec(SpawnMemberSpec::new("worker", control.clone()))
        .await
        .expect("re-spawn the control member without a restart");
    let control_after = member_generation(&handle_1, &control, "control after the reset").await;
    assert_eq!(
        control_after,
        worker_before + 1,
        "in-process, a member re-spawned after a reset continues its generation"
    );

    handle_1
        .shutdown()
        .await
        .expect("shutdown mob before cold restart");
    drop(handle_1);
    drop(service_1);

    // ---------------- Lifetime 2: resume, re-spawn the worker -------------
    let (service_2, _store_2) = persistent_service(&paths, runtime_store.clone());
    let storage_2 = MobStorage::persistent(&paths.mob_db_path).expect("reopen mob storage");
    let handle_2 = MobBuilder::for_resume(storage_2)
        .with_session_service(service_2.clone())
        .with_default_llm_client(openai_test_client())
        .notify_orchestrator_on_resume(false)
        .resume()
        .await
        .expect("resume mob after the reset and a cold restart");
    handle_2
        .spawn_spec(SpawnMemberSpec::new("worker", worker.clone()))
        .await
        .expect("re-spawn the worker after the restart");
    let worker_after = member_generation(&handle_2, &worker, "worker after the restart").await;
    assert_eq!(
        worker_after,
        worker_before + 1,
        "after a reset and a cold restart the worker mints the generation the \
         machine that never restarted mints, never its pre-reset one"
    );

    handle_2.shutdown().await.expect("final shutdown");
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
        .with_default_llm_client(openai_test_client())
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
        .with_default_llm_client(openai_test_client())
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

/// Destruction must fit the same worker stack as ordinary mob operation.
/// The parent-owned child operations make archive disposal inspect the real
/// operation registry beneath the actor's complete destroy call chain.
#[test]
fn mob_destroy_session_children_within_production_stack_budget() {
    let thread = std::thread::Builder::new()
        .name("mob-destroy-production-stack".into())
        .stack_size(2 * 1024 * 1024)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_stack_size(2 * 1024 * 1024)
                .enable_all()
                .build()
                .expect("production-sized destroy runtime");
            runtime.block_on(async {
                tokio::time::timeout(
                    Duration::from_secs(60),
                    Box::pin(mob_destroy_session_children_stack_budget_scenario()),
                )
                .await
                .expect("real child mob destruction must finish within the test deadline");
            });
        })
        .expect("spawn production-sized destroy driver");
    if let Err(panic) = thread.join() {
        std::panic::resume_unwind(panic);
    }
}

async fn mob_destroy_session_children_stack_budget_scenario() {
    use meerkat_core::ops_lifecycle::{OperationKind, OpsLifecycleRegistry};
    use meerkat_mob::store::{MobEventStore as _, SqliteMobStores};

    let temp = tempfile::tempdir().expect("temp dir");
    let paths = Paths::new(temp.path());
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
    let (service, _store) = persistent_service(&paths, runtime_store.clone());
    let adapter = service
        .acquire_runtime_adapter(None)
        .expect("acquire runtime authority")
        .expect("real session runtime");

    let mut parent_definition = mob_definition(MobRuntimeMode::TurnDriven);
    parent_definition.orchestrator = None;
    parent_definition.wiring.auto_wire_orchestrator = false;
    let parent = MobBuilder::new(
        parent_definition,
        MobStorage::persistent(&paths.mob_db_path).expect("parent mob storage"),
    )
    .with_session_service(service.clone())
    .create()
    .await
    .expect("create real parent mob");
    parent
        .spawn_spec(SpawnMemberSpec::host_root("worker", "owner"))
        .await
        .expect("spawn real owner session");
    let owner_session = parent
        .resolve_bridge_session_id(&AgentIdentity::from("owner"))
        .await
        .expect("owner bridge session");
    let owner_registration = adapter
        .current_session_registration_witness(&owner_session)
        .await
        .expect("owner has a current runtime registration");
    let owner_operations = adapter
        .ops_lifecycle_registry_if_current_registration(&owner_registration)
        .await
        .expect("owner's exact operation registry");

    let mut child_definition = mob_definition(MobRuntimeMode::TurnDriven);
    child_definition.orchestrator = None;
    child_definition.wiring.auto_wire_orchestrator = false;
    let child_storage_path = temp.path().join("child-mob.db");
    let child_storage = MobStorage::persistent(&child_storage_path).expect("child mob storage");
    let child_events = SqliteMobStores::open(&child_storage_path)
        .expect("child journal observer")
        .event_store();
    let child = MobBuilder::new(child_definition, child_storage)
        .with_session_service(service.clone())
        .with_owner_bridge_session_create_authority(owner_session.clone(), true, false)
        .create()
        .await
        .expect("create session-owned child mob");
    assert!(
        child
            .owner_bridge_session_lifecycle_authority()
            .expect("generated child authority")
            .destroy_on_owner_archive
    );
    let mut child_sessions = Vec::new();
    for member in ["child-one", "child-two"] {
        child
            .spawn_spec_with_generated_owner_context(
                SpawnMemberSpec::host_root("worker", member),
                owner_session.clone(),
            )
            .await
            .expect("spawn real child session with generated owner context");
        child_sessions.push(
            child
                .resolve_bridge_session_id(&AgentIdentity::from(member))
                .await
                .expect("child bridge session"),
        );
    }
    let operations = owner_operations
        .list_operations()
        .expect("read real child operations before destruction");
    let mut child_operations = Vec::new();
    for session in &child_sessions {
        let matching = operations
            .iter()
            .filter(|operation| {
                operation.kind == OperationKind::MobMemberChild
                    && operation.owner_session_id == owner_session
                    && operation.child_session_id.as_ref() == Some(session)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            matching.len(),
            1,
            "each child must have a real owned operation"
        );
        assert!(!matching[0].terminal, "child operation must still be live");
        child_operations.push(matching[0].id.clone());
    }
    assert!(
        !child_events
            .replay_all()
            .await
            .expect("populated child journal")
            .is_empty()
    );

    let report = child.destroy().await.expect("destroy real child mob");
    assert!(report.errors.is_empty(), "destroy errors: {report:?}");
    assert!(
        report.force_destroyed_members.is_empty(),
        "forced destroy: {report:?}"
    );
    assert!(
        report.orphaned_remote_members.is_empty(),
        "orphaned members: {report:?}"
    );
    assert!(
        !report.remote_cleanup_deadline_exceeded,
        "cleanup timed out: {report:?}"
    );
    assert!(report.namespace_cleaned && report.metadata_scrubbed && report.events_cleared);
    assert!(
        child_events
            .replay_all()
            .await
            .expect("cleared child journal")
            .is_empty()
    );
    for session in &child_sessions {
        let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(session);
        let retained = runtime_store
            .load_runtime_session_catalog_entry(&runtime_id)
            .await
            .expect("read retained child session catalog")
            .expect("destroy preserves the child session catalog");
        assert_eq!(retained.session_id(), session);
        // Archive authority is the durable lifecycle, not the retained content catalog.
        assert_eq!(
            meerkat_runtime::store::load_runtime_state(runtime_store.as_ref(), &runtime_id)
                .await
                .expect("read canonical child runtime state"),
            Some(meerkat_runtime::RuntimeState::Retired)
        );
        assert!(
            service
                .session_archived_by_authority_with_terminal(session, None)
                .await
                .expect("owning service confirms child archive authority"),
            "destroyed child must remain archived by runtime authority"
        );
        assert!(
            service.comms_runtime(session).await.is_none(),
            "archived child retained live comms"
        );
    }
    for operation_id in child_operations {
        assert!(
            owner_operations
                .snapshot(&operation_id)
                .expect("read child operation after destroy")
                .expect("owner retains child terminal receipt")
                .terminal,
            "child disposal must terminate its exact parent-owned operation"
        );
    }
    assert!(
        service.comms_runtime(&owner_session).await.is_some(),
        "child destroy retired its owner"
    );
    let parent_report = parent.destroy().await.expect("destroy parent mob");
    assert!(
        parent_report.errors.is_empty(),
        "parent destroy errors: {parent_report:?}"
    );
    assert!(
        parent_report.namespace_cleaned
            && parent_report.metadata_scrubbed
            && parent_report.events_cleared
    );
}

/// A wired member whose session was lost in a full service restart cannot be
/// respawned: its inherited restrictions can no longer be established, so the
/// respawn is refused before anything is retired, the Broken member is kept,
/// no trust is installed for it and its siblings keep working.
#[test]
fn mob_cold_restart_partial_resume_refuses_respawn_of_a_member_whose_session_was_lost() {
    run_cold_resume_scenario_with_stack(
        2 * 1024 * 1024,
        "production-stack",
        mob_cold_restart_partial_resume_lost_member_scenario(),
    );
}

/// The 2 MiB worker-stack canary for the full cold-resume respawn chain: a
/// wired member whose session survives the service restart is respawned,
/// retiring its old generation's reciprocal trust before spawning and wiring
/// the replacement generation.
#[test]
fn mob_cold_restart_resume_respawns_a_wired_member_whose_session_survived() {
    run_cold_resume_scenario_with_stack(
        2 * 1024 * 1024,
        "production-stack",
        mob_cold_restart_resume_surviving_member_respawn_scenario(),
    );
}

fn run_cold_resume_scenario_with_stack(
    stack_size: usize,
    label: &str,
    scenario: impl std::future::Future<Output = ()>,
) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        // Pin the requested stack explicitly. The production selector uses
        // Tokio's 2 MiB default, while the compact selector preserves margin
        // against debug-only future construction growth.
        .thread_stack_size(stack_size)
        .build()
        .unwrap_or_else(|error| panic!("build {label} cold-resume runtime: {error}"));
    runtime.block_on(scenario);
}

async fn mob_cold_restart_resume_surviving_member_respawn_scenario() {
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
        .with_default_llm_client(openai_test_client())
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
    drop(store_1);

    let (service_2, _store_2) = persistent_service(&paths, runtime_store_1);
    let storage_2 = MobStorage::persistent(&paths.mob_db_path).expect("reopen mob storage");
    let handle_2 = MobBuilder::for_resume(storage_2)
        .with_session_service(service_2.clone())
        .with_default_llm_client(openai_test_client())
        .notify_orchestrator_on_resume(false)
        .resume()
        .await
        .expect("resume after cold restart");
    for identity in ["lead-1", "w-1", "w-2"] {
        assert_member_active(&member_entry(&handle_2, identity).await, "cold resume");
    }
    let lead_runtime = service_2
        .comms_runtime(&lead_sid)
        .await
        .expect("resumed lead comms runtime");

    let respawned = handle_2
        .respawn(AgentIdentity::from("w-2"), None)
        .await
        .expect("respawn the surviving w2");
    assert_eq!(respawned.identity, AgentIdentity::from("w-2"));
    let new_w2_sid = handle_2
        .resolve_bridge_session_id(&AgentIdentity::from("w-2"))
        .await
        .expect("replacement w2 session id");
    assert_ne!(new_w2_sid, w2_sid);
    assert_member_active(&member_entry(&handle_2, "w-2").await, "after respawn");

    let new_w2_runtime = service_2
        .comms_runtime(&new_w2_sid)
        .await
        .expect("replacement w2 comms runtime");
    let new_w2_peer_id = new_w2_runtime.peer_id().expect("replacement w2 peer id");
    assert_ne!(
        new_w2_peer_id, old_w2_peer_id,
        "replacement generation must rotate its exact comms endpoint"
    );
    let lead_peers = lead_runtime.peers().await;
    assert!(
        lead_peers.iter().all(|peer| peer.peer_id != old_w2_peer_id),
        "lead must not retain trust for the retired w2 generation"
    );
    assert!(
        lead_peers.iter().any(|peer| {
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

async fn mob_cold_restart_partial_resume_lost_member_scenario() {
    let temp = tempfile::tempdir().expect("temp dir");
    let paths = Paths::new(temp.path());
    let runtime_store_1: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
    let (service_1, store_1) = persistent_service(&paths, runtime_store_1.clone());
    let storage_1 = MobStorage::persistent(&paths.mob_db_path).expect("persistent mob storage");
    let definition = mob_definition(MobRuntimeMode::TurnDriven);
    let replacement_w2_comms_name = format!("{}/worker/w-2", definition.id);
    let handle_1 = MobBuilder::new(definition, storage_1)
        .with_session_service(service_1.clone())
        .with_default_llm_client(openai_test_client())
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
        .with_default_llm_client(openai_test_client())
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

    // w-2's session, and with it the owner's evidence of the restrictions
    // it inherited, is gone. Respawn refuses before retiring anything and
    // keeps the exact Broken member; retiring it and spawning a fresh member
    // are separate, ordinary operations.
    let w2 = AgentIdentity::from("w-2");
    let before = handle_2
        .get_member(&w2)
        .await
        .expect("roster query")
        .expect("broken w2 in the roster");
    let refused = handle_2.respawn(w2.clone(), None).await;
    assert!(
        matches!(
            &refused,
            Err(meerkat_mob::MobRespawnError::Mob(
                meerkat_mob::MobError::RespawnPredecessorRestrictionsUnavailable { .. }
            ))
        ),
        "a lost predecessor session refuses the respawn, typed: {refused:?}"
    );
    let after = handle_2
        .get_member(&w2)
        .await
        .expect("roster query")
        .expect("the member was not retired");
    assert_eq!(after.generation, before.generation);
    assert_eq!(after.agent_runtime_id, before.agent_runtime_id);
    assert_eq!(after.fence_token, before.fence_token);
    assert_eq!(after.bridge_session_id(), Some(&w2_sid));
    assert_eq!(
        member_entry(&handle_2, "w-2").await.status,
        MobMemberStatus::Broken,
        "the refused respawn keeps the Broken member"
    );

    // No successor endpoint was trusted, and the old one stays untrusted.
    let lead_peers_after_refusal = lead_before_repair.peers().await;
    assert!(
        lead_peers_after_refusal
            .iter()
            .all(|peer| peer.peer_id != old_w2_peer_id
                && peer.name.as_str() != replacement_w2_comms_name.as_str()),
        "a refused respawn installs no trust for w2: {lead_peers_after_refusal:?}"
    );

    // The siblings stay usable after the refusal.
    assert_member_active(&member_entry(&handle_2, "lead-1").await, "after refusal");
    assert_member_active(&member_entry(&handle_2, "w-1").await, "after refusal");
    send_and_wait(
        &handle_2,
        service_2.as_ref(),
        "lead-1",
        "AFTER_REFUSAL_LEAD_TURN token-lead",
        "after refusal",
    )
    .await;
    send_and_wait(
        &handle_2,
        service_2.as_ref(),
        "w-1",
        "AFTER_REFUSAL_W1_TURN token-w1",
        "after refusal",
    )
    .await;

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
    prepared_boundary_commit_rejected: tokio::sync::Notify,
    /// While set, this runtime's prepared session boundary commit waits at
    /// the gate until released: a turn whose final checkpoint already wrote
    /// the canonical head, but whose runtime boundary has not committed.
    held_boundary: std::sync::Mutex<Option<meerkat_runtime::LogicalRuntimeId>>,
    boundary_hold: tokio::sync::watch::Sender<BoundaryHold>,
    /// While set, the store reports its inner store's hosting capability, so
    /// a service opened over it takes the inner store's OS-locked hosting
    /// claims (#1813). Unset, it offers no claims, as before.
    forward_hosting: std::sync::atomic::AtomicBool,
}

/// Where a held boundary commit is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BoundaryHold {
    Idle,
    Entered,
    Released,
}

impl PowerCutRuntimeStore {
    fn new(path: &Path) -> Self {
        Self::over(
            meerkat_runtime::store::SqliteRuntimeStore::new_head_canonical(path)
                .expect("open co-tenant HeadCanonical runtime store"),
        )
    }

    /// [`Self::new`] over a store whose sessions are hosted across processes
    /// with OS-locked claims under `hosting_root`, laid out as a realm lays
    /// them out. The store reports that capability once
    /// [`Self::forward_hosting_capability`] is called.
    fn with_hosting_root(path: &Path, hosting_root: &Path) -> Self {
        let hosting_lock_dir = hosting_root.join("hosting");
        let cold_delivery_lock = hosting_root.join("delivery").join("cold-delivery.lock");
        fs::create_dir_all(&hosting_lock_dir).expect("create the hosting lock dir");
        fs::create_dir_all(
            cold_delivery_lock
                .parent()
                .expect("the cold delivery lock has a parent"),
        )
        .expect("create the cold delivery dir");
        Self::over(
            meerkat_runtime::store::SqliteRuntimeStore::new_head_canonical(path)
                .expect("open co-tenant HeadCanonical runtime store")
                .with_hosting_paths(meerkat_runtime::HostingPaths {
                    hosting_lock_dir,
                    cold_delivery_lock,
                    database: None,
                }),
        )
    }

    fn over(inner: meerkat_runtime::store::SqliteRuntimeStore) -> Self {
        Self {
            inner,
            cut: std::sync::atomic::AtomicBool::new(false),
            prepared_boundary_commit_rejections: std::sync::atomic::AtomicUsize::new(0),
            legacy_boundary_commit_rejections: std::sync::atomic::AtomicUsize::new(0),
            prepared_boundary_commit_rejected: tokio::sync::Notify::new(),
            held_boundary: std::sync::Mutex::new(None),
            boundary_hold: tokio::sync::watch::channel(BoundaryHold::Idle).0,
            forward_hosting: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Report the inner store's hosting capability from now on (see
    /// [`Self::with_hosting_root`]).
    fn forward_hosting_capability(&self) {
        self.forward_hosting
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Hold `runtime_id`'s next prepared session boundary commits at the
    /// gate until [`Self::release_boundary_commits`].
    fn hold_boundary_commits(&self, runtime_id: meerkat_runtime::LogicalRuntimeId) {
        *self.held_boundary.lock().expect("boundary hold lock") = Some(runtime_id);
        self.boundary_hold.send_replace(BoundaryHold::Idle);
    }

    /// Resolve once a held boundary commit has reached the gate.
    async fn wait_boundary_commit_held(&self) {
        let mut hold = self.boundary_hold.subscribe();
        hold.wait_for(|state| *state == BoundaryHold::Entered)
            .await
            .expect("boundary hold channel");
    }

    fn release_boundary_commits(&self) {
        *self.held_boundary.lock().expect("boundary hold lock") = None;
        self.boundary_hold.send_replace(BoundaryHold::Released);
    }
}

/// Releases the held boundary commits when dropped, so a row that fails
/// before its own release never leaves a commit parked at the gate.
struct ReleaseBoundaryCommitsOnDrop(Arc<PowerCutRuntimeStore>);

impl Drop for ReleaseBoundaryCommitsOnDrop {
    fn drop(&mut self) {
        self.0.release_boundary_commits();
    }
}

impl PowerCutRuntimeStore {
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

    #[cfg(feature = "test-support")]
    fn legacy_boundary_commit_rejections(&self) -> usize {
        self.legacy_boundary_commit_rejections
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    fn record_prepared_boundary_commit_rejection(&self) {
        self.prepared_boundary_commit_rejections
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.prepared_boundary_commit_rejected.notify_waiters();
    }

    async fn wait_for_prepared_boundary_commit_rejection(&self) {
        loop {
            let notified = self.prepared_boundary_commit_rejected.notified();
            if self.prepared_boundary_commit_rejections() > 0 {
                return;
            }
            notified.await;
        }
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

    fn hosting_capability(&self) -> meerkat_runtime::HostingCapability {
        if self
            .forward_hosting
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            meerkat_runtime::RuntimeStore::hosting_capability(&self.inner)
        } else {
            meerkat_runtime::HostingCapability::None
        }
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
        let held = self
            .held_boundary
            .lock()
            .expect("boundary hold lock")
            .as_ref()
            == Some(runtime_id);
        if held {
            let mut hold = self.boundary_hold.subscribe();
            // A release that lands after the `held` read stays released: it
            // is never overwritten by Entered, so this commit cannot park.
            self.boundary_hold.send_if_modified(|state| {
                if *state == BoundaryHold::Released {
                    return false;
                }
                *state = BoundaryHold::Entered;
                true
            });
            let _ = hold
                .wait_for(|state| *state == BoundaryHold::Released)
                .await;
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

#[cfg(feature = "test-support")]
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

#[cfg(feature = "test-support")]
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
        .with_default_llm_client(openai_test_client())
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
    let lead_sid = handle_1
        .resolve_bridge_session_id(&AgentIdentity::from("lead-1"))
        .await
        .expect("lead session id");
    let mut runtime_release_witnesses = Vec::new();
    for session_id in [&lead_sid, &w1_sid] {
        let runtime = service_1
            .comms_runtime(session_id)
            .await
            .unwrap_or_else(|| panic!("comms runtime missing for {session_id}"));
        runtime_release_witnesses.push(Arc::downgrade(&runtime));
    }

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
    assert!(
        handle_1.actor_command_channel_closed_for_test(),
        "crash-stop must not acknowledge before the actor receiver and actor-owned runtimes are dropped"
    );
    assert_eq!(
        power_store.legacy_boundary_commit_rejections(),
        0,
        "HeadCanonical lifetime must reach only the prepared boundary seam"
    );
    // A real process loss destroys the session-service actors as well as the
    // mob actor. This same-process fixture must establish that boundary
    // explicitly before lifetime 2 can claim the same participant names.
    service_1
        .try_shutdown()
        .await
        .expect("shutdown first session service");
    drop(handle_1);
    drop(service_1);
    drop(store_1);
    tokio::time::timeout(Duration::from_secs(2), async {
        while runtime_release_witnesses
            .iter()
            .any(|runtime| runtime.upgrade().is_some())
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("process-loss fixture waits for every old session runtime to be released");

    // ---------------- Lifetime 2 (power restored) ----------------
    power_store.set_cut(false);
    let (service_2, store_2) = persistent_head_canonical_service(&paths, runtime_store.clone());
    let persistent_owner: Arc<dyn MobSessionService> = service_2.clone();
    let forwarding_decorator =
        FailingOnceSessionService::new(persistent_owner, None, None, 0, false);
    let storage_2 = MobStorage::persistent(&paths.mob_db_path).expect("reopen mob storage");
    let resume_result = MobBuilder::for_resume(storage_2)
        // The transparent decorator must forward the owner's combined
        // durable-tail verdict. Persistent actor creation consumes its exact
        // owner-issued receipt, so inheriting a nonpersistent default would
        // fail this power-cut recovery before materialization.
        .with_session_service(forwarding_decorator)
        .with_default_llm_client(openai_test_client())
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
    assert_member_active(&lead_after, "kill-window-post-resume-orchestrator");
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

/// A completed-boundary store failure leaves the live runtime shell in
/// ReloadRequired with its exact run still bound. Ordinary Mob retirement must
/// dispose only that degraded shell, cold-register durable authority, and then
/// execute the normal Retired/archive/unregister protocol. Routing this case
/// through ready-gated cancel reproduces the production retirement wedge.
#[tokio::test(flavor = "multi_thread")]
async fn mob_retirement_recovers_reload_required_runtime_before_normal_archive() {
    let temp = tempfile::tempdir().expect("temp dir");
    let paths = Paths::new(temp.path());
    let power_store = Arc::new(PowerCutRuntimeStore::new(&paths.realm_db_path));
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = power_store.clone();
    let (service, _session_store) =
        persistent_head_canonical_service(&paths, runtime_store.clone());
    let storage = MobStorage::persistent(&paths.mob_db_path).expect("persistent mob storage");
    let handle = MobBuilder::new(mob_definition(MobRuntimeMode::TurnDriven), storage)
        .with_session_service(service.clone())
        .with_default_llm_client(openai_test_client())
        .create()
        .await
        .expect("create persistent mob");

    let identity = AgentIdentity::from("reload-required-worker");
    handle
        .spawn_spec(SpawnMemberSpec::new("worker", identity.clone()))
        .await
        .expect("spawn reload-required worker");
    let session_id = handle
        .resolve_bridge_session_id(&identity)
        .await
        .expect("reload-required worker session id");
    let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(&session_id);
    let adapter = service
        .acquire_runtime_adapter(None)
        .expect("acquire runtime authority")
        .expect("persistent session service exposes its runtime adapter");
    let old_registration = adapter
        .current_session_registration_witness(&session_id)
        .await
        .expect("spawned member has an exact runtime registration");

    send_and_wait(
        &handle,
        service.as_ref(),
        identity.as_str(),
        "PRE_DEGRADED_RETIREMENT token-alpha",
        "reload-required-retirement-baseline",
    )
    .await;

    power_store.set_cut(true);
    let member = handle
        .member(&identity)
        .await
        .expect("reload-required member handle");
    let failed_turn = tokio::spawn(async move {
        member
            .send(
                "FAILED_BOUNDARY_RETIREMENT token-omega".to_string(),
                HandlingMode::Queue,
            )
            .await
    });
    tokio::time::timeout(
        Duration::from_secs(30),
        power_store.wait_for_prepared_boundary_commit_rejection(),
    )
    .await
    .expect("real prepared boundary failure must occur before retirement");
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if !adapter
                .session_has_live_executor_attachment(&session_id)
                .await
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("failed boundary runtime loop must quiesce before retirement");
    tokio::time::timeout(Duration::from_secs(30), failed_turn)
        .await
        .expect("faulted turn admission task must finish")
        .expect("faulted turn admission task must not panic")
        .expect("the fault must occur after exact turn admission");
    let degraded = adapter
        .meerkat_machine_archive_snapshot(&session_id)
        .await
        .expect("ReloadRequired shell remains registered before retirement");
    assert!(
        degraded.control.current_run_id.is_some(),
        "the regression must retain the exact stale run that ready-gated cancel cannot clear"
    );

    power_store.set_cut(false);
    tokio::time::timeout(Duration::from_secs(30), handle.retire(identity.clone()))
        .await
        .expect("ReloadRequired retirement must not wedge at actor_retirement_saga")
        .expect("ReloadRequired retirement must converge through normal archive");

    assert!(
        handle
            .list_members_including_retiring()
            .await
            .into_iter()
            .all(|entry| entry.agent_identity != identity.as_str()),
        "normal retirement must remove the member from the roster"
    );
    assert!(
        !adapter.contains_session(&session_id).await,
        "normal retirement must unregister the cold successor after durable Retired"
    );
    assert_eq!(
        adapter
            .recover_or_discard_reload_required_registration_if_current(&old_registration)
            .await
            .expect("stale degraded witness is an idempotent observation"),
        meerkat_runtime::ReloadRequiredRegistrationDisposition::NotCurrent,
        "the old degraded witness must not touch the retired successor"
    );
    assert_eq!(
        meerkat_runtime::store::load_runtime_state(runtime_store.as_ref(), &runtime_id)
            .await
            .expect("load durable runtime state after recovery retirement"),
        Some(meerkat_runtime::RuntimeState::Retired),
        "Discarded is not retirement: the cold successor must publish durable Retired"
    );
    let archived = service
        .load_revivable_retired_session(&session_id)
        .await
        .expect("read explicitly revivable retired session")
        .expect("normal archive must retain the retired session");
    let history = to_string(archived.messages()).expect("serialize retired transcript");
    for token in ["PRE_DEGRADED_RETIREMENT", "FAILED_BOUNDARY_RETIREMENT"] {
        assert_eq!(
            history.matches(token).count(),
            1,
            "durable recovery and retirement must retain exactly one '{token}': {history}"
        );
    }
}

/// A delivery scope captured before a mob reset names an incarnation the
/// reset ended. After a cold restart, an explicit Resume of the same session
/// mints a later incarnation (the generation high-water survives the reset),
/// so the old scope is refused as stale and retires nothing, although its
/// session is the member's binding again.
#[tokio::test(flavor = "multi_thread")]
async fn a_scope_from_before_a_reset_never_retires_the_resumed_member() {
    let temp = tempfile::tempdir().expect("temp dir");
    let paths = Paths::new(temp.path());
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());

    let mut definition = mob_definition(MobRuntimeMode::TurnDriven);
    definition.id = MobId::from("reset-scope-mob");
    definition.orchestrator = None;
    definition.wiring = WiringRules {
        auto_wire_orchestrator: false,
        role_wiring: vec![],
    };

    // ---------------- Lifetime 1: spawn, capture the scope, reset ---------
    let (service_1, _store_1) = persistent_service(&paths, runtime_store.clone());
    let storage_1 = MobStorage::persistent(&paths.mob_db_path).expect("persistent mob storage");
    let handle_1 = MobBuilder::new(definition, storage_1)
        .with_session_service(service_1.clone())
        .with_default_llm_client(openai_test_client())
        .create()
        .await
        .expect("create persistent mob");
    let worker = AgentIdentity::from("w-reset-scope");
    handle_1
        .spawn_spec(SpawnMemberSpec::new("worker", worker.clone()))
        .await
        .expect("spawn before the reset");
    let old_scope = handle_1
        .capture_member_delivery_scope(&worker)
        .await
        .expect("capture the scope before the reset");
    handle_1.reset().await.expect("reset the mob");
    handle_1
        .shutdown()
        .await
        .expect("shutdown mob before cold restart");
    drop(handle_1);
    drop(service_1);

    // ---------------- Lifetime 2: resume, Resume the old session ----------
    let (service_2, _store_2) = persistent_service(&paths, runtime_store.clone());
    let storage_2 = MobStorage::persistent(&paths.mob_db_path).expect("reopen mob storage");
    let handle_2 = MobBuilder::for_resume(storage_2)
        .with_session_service(service_2.clone())
        .with_default_llm_client(openai_test_client())
        .notify_orchestrator_on_resume(false)
        .resume()
        .await
        .expect("resume mob after the reset and a cold restart");
    handle_2
        .spawn_spec(
            SpawnMemberSpec::new("worker", worker.clone())
                .with_resume_bridge_session_id(old_scope.session_id().clone()),
        )
        .await
        .expect("resume the reset member's session");
    let resumed = handle_2
        .capture_member_delivery_scope(&worker)
        .await
        .expect("capture the resumed member's scope");
    assert_eq!(
        resumed.session_id(),
        old_scope.session_id(),
        "the resumed incarnation holds the old session"
    );
    assert!(
        resumed.generation() > old_scope.generation(),
        "the resumed incarnation is a later one: {resumed:?} after {old_scope:?}"
    );

    let error = handle_2
        .retire_at_scope(&old_scope)
        .await
        .expect_err("a scope from before the reset must not retire the resumed member");
    assert!(
        matches!(
            &error,
            meerkat_mob::MobError::StaleFenceToken { runtime_id, .. }
                if runtime_id == old_scope.runtime_id()
        ),
        "the old scope is refused as stale: {error:?}"
    );
    assert!(
        handle_2
            .get_member(&worker)
            .await
            .expect("read the roster")
            .is_some(),
        "the resumed member stays"
    );

    handle_2.shutdown().await.expect("final shutdown");
}

/// A mob whose retiring member has a durable tail ahead of its runtime
/// boundary authority restarts. The power cut leaves the canonical head
/// carrying a turn whose runtime boundary commit never became durable (the
/// window a mid-turn retirement's boundary cancel also leaves); the member's
/// retirement then starts and the process dies before it finishes. At the next
/// start the actor restores that member's local cleanup delivery before it
/// admits any command. The restore must settle the tail instead of refusing
/// to read the session at the stale boundary, or the whole mob fail-stops and
/// refuses its own restart.
#[cfg(feature = "test-support")]
#[tokio::test(flavor = "multi_thread")]
async fn a_mob_whose_retiring_member_has_a_durable_tail_restarts() {
    // nextest runs each test in its own process: the actor's startup refusal
    // is logged at ERROR, so show it.
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::ERROR)
        .with_writer(std::io::stderr)
        .try_init();
    let temp = tempfile::tempdir().expect("temp dir");
    let paths = Paths::new(temp.path());
    let power_store = Arc::new(PowerCutRuntimeStore::new(&paths.realm_db_path));
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = power_store.clone();

    // ---------------- Lifetime 1 ----------------
    let (service_1, store_1) = persistent_head_canonical_service(&paths, runtime_store.clone());
    let storage_1 = MobStorage::persistent(&paths.mob_db_path).expect("persistent mob storage");
    let handle_1 = MobBuilder::new(mob_definition(MobRuntimeMode::TurnDriven), storage_1)
        .with_session_service(service_1.clone())
        .with_default_llm_client(openai_test_client())
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
    let identity = AgentIdentity::from("w-1");
    let w1_sid = handle_1
        .resolve_bridge_session_id(&identity)
        .await
        .expect("w1 session id");
    let lead_sid = handle_1
        .resolve_bridge_session_id(&AgentIdentity::from("lead-1"))
        .await
        .expect("lead session id");
    let mut runtime_release_witnesses = Vec::new();
    for session_id in [&lead_sid, &w1_sid] {
        let runtime = service_1
            .comms_runtime(session_id)
            .await
            .unwrap_or_else(|| panic!("comms runtime missing for {session_id}"));
        runtime_release_witnesses.push(Arc::downgrade(&runtime));
    }

    send_and_wait(
        &handle_1,
        service_1.as_ref(),
        "w-1",
        "PRE_CUT_TAIL_TURN token-alpha",
        "retiring-tail-lifetime1",
    )
    .await;
    wait_for_head_canonical_authority_convergence(
        store_1.as_ref(),
        runtime_store.as_ref(),
        &w1_sid,
        "PRE_CUT_TAIL_TURN",
        "retiring-tail-pre-cut-boundary",
    )
    .await;

    // The next turn's durable row lands; its runtime boundary commit does not.
    power_store.set_cut(true);
    let send_result = tokio::time::timeout(Duration::from_secs(30), async {
        handle_1
            .member(&identity)
            .await
            .expect("member handle")
            .send(
                "POST_CUT_TAIL_TURN token-omega".to_string(),
                HandlingMode::Queue,
            )
            .await
    })
    .await;
    eprintln!("[retiring-tail] send during cut => {send_result:?}");
    wait_for_row_contains(
        store_1.as_ref(),
        &w1_sid,
        "POST_CUT_TAIL_TURN",
        "retiring-tail-row",
    )
    .await;
    {
        let deadline = Instant::now() + Duration::from_secs(20);
        while power_store.prepared_boundary_commit_rejections() == 0 {
            assert!(
                Instant::now() < deadline,
                "[retiring-tail] prepared boundary commit was never rejected under the cut"
            );
            sleep(Duration::from_millis(50)).await;
        }
    }
    let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(&w1_sid);
    let authority = runtime_store
        .load_session_boundary_authority(&runtime_id)
        .await
        .expect("load runtime session authority")
        .expect("runtime session authority present");
    let authority = authority
        .head_canonical()
        .expect("head-canonical boundary authority");
    let ahead_head = store_1
        .load_head(&w1_sid)
        .await
        .expect("load ahead physical head")
        .expect("ahead physical head exists");
    assert_ne!(
        meerkat_core::session_store::session_head_cas_token(&ahead_head)
            .expect("ahead physical head has a canonical token"),
        authority.committed_head_token(),
        "the physical head must be ahead of the retained runtime authority"
    );
    assert!(
        ahead_head.message_count > authority.boundary_head().message_count,
        "the physical head must carry a strict continuation past the boundary"
    );

    // The member's retirement starts with the tail in place (as a
    // reconciler's retirement of a member mid-turn does), then the process
    // dies before it finishes.
    let retire_result =
        tokio::time::timeout(Duration::from_secs(10), handle_1.retire(identity.clone())).await;
    eprintln!("[retiring-tail] retire during cut => {retire_result:?}");
    handle_1
        .crash_stop_preserving_durable_work_for_test()
        .await
        .expect("crash-stop lifetime-1 actor");
    service_1
        .try_shutdown()
        .await
        .expect("shutdown first session service");
    drop(handle_1);
    drop(service_1);
    drop(store_1);
    tokio::time::timeout(Duration::from_secs(2), async {
        while runtime_release_witnesses
            .iter()
            .any(|runtime| runtime.upgrade().is_some())
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("every old session runtime is released before lifetime 2");

    // ---------------- Lifetime 2 ----------------
    power_store.set_cut(false);
    let (service_2, store_2) = persistent_head_canonical_service(&paths, runtime_store.clone());
    let storage_2 = MobStorage::persistent(&paths.mob_db_path).expect("reopen mob storage");
    let handle_2 = MobBuilder::for_resume(storage_2)
        .with_session_service(service_2.clone())
        .with_default_llm_client(openai_test_client())
        .notify_orchestrator_on_resume(false)
        .resume()
        .await
        .unwrap_or_else(|error| {
            panic!("a stopped mob whose member has a durable tail must restart: {error}")
        });
    let status = handle_2.status().await;
    assert!(
        matches!(status, Ok(meerkat_mob::MobState::Running)),
        "the restarted mob must admit commands, not fail-stop before admission: {status:?}"
    );
    // The tail was settled by adoption: the runtime authority now names the
    // physical head that carries the post-cut turn, which is kept, not lost.
    wait_for_head_canonical_authority_convergence(
        store_2.as_ref(),
        runtime_store.as_ref(),
        &w1_sid,
        "POST_CUT_TAIL_TURN",
        "retiring-tail-adopted-boundary",
    )
    .await;
    let _ = handle_2.shutdown().await;
}

/// #1813: a mob start never settles a durable tail on a member session that
/// another runtime owner hosts. The retiring member's last turn leaves a
/// durable tail (a power cut between its rows and its runtime boundary), and
/// before the next start another runtime owner on the realm (another process,
/// here a second owner in this one) takes the member session's hosting
/// claim. That owner may still commit the tail itself, so the start must not
/// adopt it: it refuses, typed, naming the member's session, and leaves the
/// tail as it was. Once that owner lets go, the next start settles the tail
/// under its own claim and runs, the healthy lead included.
#[cfg(feature = "test-support")]
#[tokio::test(flavor = "multi_thread")]
async fn a_mob_start_refuses_a_durable_tail_another_runtime_owner_hosts() {
    // nextest runs each test in its own process: the actor's startup refusal
    // is logged at ERROR, so show it.
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::ERROR)
        .with_writer(std::io::stderr)
        .try_init();
    let temp = tempfile::tempdir().expect("temp dir");
    let paths = Paths::new(temp.path());
    let power_store = Arc::new(PowerCutRuntimeStore::with_hosting_root(
        &paths.realm_db_path,
        temp.path(),
    ));
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = power_store.clone();

    // ---------------- Lifetime 1 ----------------
    let (service_1, store_1) = persistent_head_canonical_service(&paths, runtime_store.clone());
    let storage_1 = MobStorage::persistent(&paths.mob_db_path).expect("persistent mob storage");
    let handle_1 = MobBuilder::new(mob_definition(MobRuntimeMode::TurnDriven), storage_1)
        .with_session_service(service_1.clone())
        .with_default_llm_client(openai_test_client())
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
    let identity = AgentIdentity::from("w-1");
    let w1_sid = handle_1
        .resolve_bridge_session_id(&identity)
        .await
        .expect("w1 session id");
    let lead_sid = handle_1
        .resolve_bridge_session_id(&AgentIdentity::from("lead-1"))
        .await
        .expect("lead session id");
    let mut runtime_release_witnesses = Vec::new();
    for session_id in [&lead_sid, &w1_sid] {
        let runtime = service_1
            .comms_runtime(session_id)
            .await
            .unwrap_or_else(|| panic!("comms runtime missing for {session_id}"));
        runtime_release_witnesses.push(Arc::downgrade(&runtime));
    }

    send_and_wait(
        &handle_1,
        service_1.as_ref(),
        "w-1",
        "PRE_CUT_TAIL_TURN token-alpha",
        "hosted-tail-lifetime1",
    )
    .await;
    wait_for_head_canonical_authority_convergence(
        store_1.as_ref(),
        runtime_store.as_ref(),
        &w1_sid,
        "PRE_CUT_TAIL_TURN",
        "hosted-tail-pre-cut-boundary",
    )
    .await;

    // The next turn's durable row lands; its runtime boundary commit does not.
    power_store.set_cut(true);
    let send_result = tokio::time::timeout(Duration::from_secs(30), async {
        handle_1
            .member(&identity)
            .await
            .expect("member handle")
            .send(
                "POST_CUT_TAIL_TURN token-omega".to_string(),
                HandlingMode::Queue,
            )
            .await
    })
    .await;
    eprintln!("[hosted-tail] send during cut => {send_result:?}");
    wait_for_row_contains(
        store_1.as_ref(),
        &w1_sid,
        "POST_CUT_TAIL_TURN",
        "hosted-tail-row",
    )
    .await;
    power_store
        .wait_for_prepared_boundary_commit_rejection()
        .await;
    let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(&w1_sid);
    let committed_token = || async {
        runtime_store
            .load_session_boundary_authority(&runtime_id)
            .await
            .expect("load runtime session authority")
            .expect("runtime session authority present")
            .head_canonical()
            .expect("head-canonical boundary authority")
            .committed_head_token()
            .to_string()
    };
    let tail_committed_token = committed_token().await;
    let ahead_head = store_1
        .load_head(&w1_sid)
        .await
        .expect("load ahead physical head")
        .expect("ahead physical head exists");
    let ahead_token = meerkat_core::session_store::session_head_cas_token(&ahead_head)
        .expect("ahead physical head has a canonical token");
    assert_ne!(
        ahead_token, tail_committed_token,
        "the physical head must be ahead of the retained runtime authority"
    );

    // The member's retirement starts with the tail in place, then the process
    // dies before it finishes.
    let retire_result =
        tokio::time::timeout(Duration::from_secs(10), handle_1.retire(identity.clone())).await;
    eprintln!("[hosted-tail] retire during cut => {retire_result:?}");
    handle_1
        .crash_stop_preserving_durable_work_for_test()
        .await
        .expect("crash-stop lifetime-1 actor");
    service_1
        .try_shutdown()
        .await
        .expect("shutdown first session service");
    drop(handle_1);
    drop(service_1);
    drop(store_1);
    tokio::time::timeout(Duration::from_secs(2), async {
        while runtime_release_witnesses
            .iter()
            .any(|runtime| runtime.upgrade().is_some())
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("every old session runtime is released before lifetime 2");

    // ---------------- Lifetime 2 ----------------
    // Power is back, and the realm's sessions are hosted with OS-locked
    // claims. Another runtime owner on the realm hosts the retiring member's
    // session before this start.
    power_store.set_cut(false);
    power_store.forward_hosting_capability();
    let capability = meerkat_runtime::RuntimeStore::hosting_capability(runtime_store.as_ref());
    assert!(
        capability.is_cross_process(),
        "the realm store must offer cross-process hosting claims: {capability:?}"
    );
    let other_owner = meerkat_runtime::SessionHostingAuthority::new(
        capability,
        meerkat_runtime::HostingOwner::mint(),
    );
    let other_owner_claim = other_owner
        .grant(&w1_sid)
        .expect("another runtime owner takes the retiring member's hosting claim");

    let (service_2, store_2) = persistent_head_canonical_service(&paths, runtime_store.clone());
    let refused = MobBuilder::for_resume(
        MobStorage::persistent(&paths.mob_db_path).expect("reopen mob storage"),
    )
    .with_session_service(service_2.clone())
    .with_default_llm_client(openai_test_client())
    .notify_orchestrator_on_resume(false)
    .resume()
    .await;
    let refused_status = match &refused {
        Ok(handle) => Some(handle.status().await),
        Err(_) => None,
    };
    let refused_error = refused.as_ref().err().map(ToString::to_string);
    eprintln!(
        "[hosted-tail] start while hosted elsewhere => error {refused_error:?}, status \
         {refused_status:?}"
    );

    // Never another owner's tail: the runtime authority and the physical head
    // are exactly as the cut left them.
    assert_eq!(
        committed_token().await,
        tail_committed_token,
        "a mob start must not settle a durable tail on a session another runtime owner hosts"
    );
    let head_after_refusal = store_2
        .load_head(&w1_sid)
        .await
        .expect("load the physical head after the refused start")
        .expect("the physical head still exists");
    assert_eq!(
        meerkat_core::session_store::session_head_cas_token(&head_after_refusal)
            .expect("the physical head has a canonical token"),
        ahead_token,
        "the refused start must leave the durable tail where the cut left it"
    );
    // The start refuses rather than run without the member's cleanup
    // delivery.
    assert!(
        !matches!(refused_status, Some(Ok(meerkat_mob::MobState::Running))),
        "a mob start that could not restore a retiring member's cleanup delivery must not run: \
         {refused_status:?}"
    );
    if let Ok(handle) = refused {
        let _ = handle.shutdown().await;
    }

    // The other owner lets go; the next start settles the tail under its own
    // claim and runs.
    drop(other_owner_claim);
    let handle_3 = MobBuilder::for_resume(
        MobStorage::persistent(&paths.mob_db_path).expect("reopen mob storage again"),
    )
    .with_session_service(service_2.clone())
    .with_default_llm_client(openai_test_client())
    .notify_orchestrator_on_resume(false)
    .resume()
    .await
    .unwrap_or_else(|error| {
        panic!("the mob must start once no other runtime owner hosts the member: {error}")
    });
    let status = handle_3.status().await;
    assert!(
        matches!(status, Ok(meerkat_mob::MobState::Running)),
        "the restarted mob must admit commands: {status:?}"
    );
    assert!(
        handle_3
            .get_member(&AgentIdentity::from("lead-1"))
            .await
            .expect("read the roster")
            .is_some(),
        "the healthy lead is still rostered after the refused start"
    );
    wait_for_head_canonical_authority_convergence(
        store_2.as_ref(),
        runtime_store.as_ref(),
        &w1_sid,
        "POST_CUT_TAIL_TURN",
        "hosted-tail-adopted-boundary",
    )
    .await;
    let _ = handle_3.shutdown().await;
}

/// What a process death leaves on an active member's session: the member's
/// last turn landed its durable rows, its runtime boundary commit did not (a
/// power cut between the two), and the process died with the member active.
/// Power is back afterwards, and the realm's sessions are hosted with
/// OS-locked claims.
#[cfg(feature = "test-support")]
struct ActiveMemberTail {
    runtime_store: Arc<dyn meerkat_runtime::RuntimeStore>,
    session_id: meerkat_core::types::SessionId,
    /// The committed head token the runtime authority retained.
    committed_token: String,
    /// The physical head's token, ahead of the committed one.
    ahead_token: String,
}

#[cfg(feature = "test-support")]
async fn committed_head_token(
    runtime_store: &dyn meerkat_runtime::RuntimeStore,
    session_id: &meerkat_core::types::SessionId,
) -> String {
    runtime_store
        .load_session_boundary_authority(&meerkat_runtime::LogicalRuntimeId::for_session(
            session_id,
        ))
        .await
        .expect("load runtime session authority")
        .expect("runtime session authority present")
        .head_canonical()
        .expect("head-canonical boundary authority")
        .committed_head_token()
        .to_string()
}

#[cfg(feature = "test-support")]
async fn physical_head_token(
    store: &SqliteSessionStore,
    session_id: &meerkat_core::types::SessionId,
) -> String {
    let head = store
        .load_head(session_id)
        .await
        .expect("load the physical head")
        .expect("the physical head still exists");
    meerkat_core::session_store::session_head_cas_token(&head)
        .expect("the physical head has a canonical token")
}

#[cfg(feature = "test-support")]
impl ActiveMemberTail {
    async fn committed_token_now(&self) -> String {
        committed_head_token(self.runtime_store.as_ref(), &self.session_id).await
    }

    async fn physical_token_now(&self, store: &SqliteSessionStore) -> String {
        physical_head_token(store, &self.session_id).await
    }

    async fn provisional_tail_recorded_now(&self) -> bool {
        self.runtime_store
            .load_head_canonical_provisional_tail(&meerkat_runtime::LogicalRuntimeId::for_session(
                &self.session_id,
            ))
            .await
            .expect("load the provisional tail")
            .is_some()
    }

    /// Another runtime owner on the realm (another process, here a second
    /// owner in this one) takes the member session's hosting claim; that
    /// owner may still commit the tail itself.
    fn another_owner_hosts_the_session(&self) -> meerkat_runtime::HostingClaim {
        let capability =
            meerkat_runtime::RuntimeStore::hosting_capability(self.runtime_store.as_ref());
        assert!(
            capability.is_cross_process(),
            "the realm store must offer cross-process hosting claims: {capability:?}"
        );
        meerkat_runtime::SessionHostingAuthority::new(
            capability,
            meerkat_runtime::HostingOwner::mint(),
        )
        .grant(&self.session_id)
        .expect("another runtime owner takes the member session's hosting claim")
    }
}

/// Leave an [`ActiveMemberTail`] on member `w-1` of a mob led by `lead-1`.
#[cfg(feature = "test-support")]
async fn crash_with_an_active_member_tail(temp: &Path, paths: &Paths) -> ActiveMemberTail {
    let power_store = Arc::new(PowerCutRuntimeStore::with_hosting_root(
        &paths.realm_db_path,
        temp,
    ));
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = power_store.clone();

    let (service_1, store_1) = persistent_head_canonical_service(paths, runtime_store.clone());
    let storage_1 = MobStorage::persistent(&paths.mob_db_path).expect("persistent mob storage");
    let handle_1 = MobBuilder::new(mob_definition(MobRuntimeMode::TurnDriven), storage_1)
        .with_session_service(service_1.clone())
        .with_default_llm_client(openai_test_client())
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
    let identity = AgentIdentity::from("w-1");
    let w1_sid = handle_1
        .resolve_bridge_session_id(&identity)
        .await
        .expect("w1 session id");
    let lead_sid = handle_1
        .resolve_bridge_session_id(&AgentIdentity::from("lead-1"))
        .await
        .expect("lead session id");
    let mut runtime_release_witnesses = Vec::new();
    for session_id in [&lead_sid, &w1_sid] {
        let runtime = service_1
            .comms_runtime(session_id)
            .await
            .unwrap_or_else(|| panic!("comms runtime missing for {session_id}"));
        runtime_release_witnesses.push(Arc::downgrade(&runtime));
    }

    send_and_wait(
        &handle_1,
        service_1.as_ref(),
        "w-1",
        "PRE_CUT_TAIL_TURN token-alpha",
        "active-tail-lifetime1",
    )
    .await;
    wait_for_head_canonical_authority_convergence(
        store_1.as_ref(),
        runtime_store.as_ref(),
        &w1_sid,
        "PRE_CUT_TAIL_TURN",
        "active-tail-pre-cut-boundary",
    )
    .await;

    // The next turn's durable row lands; its runtime boundary commit does not.
    power_store.set_cut(true);
    let send_result = tokio::time::timeout(Duration::from_secs(30), async {
        handle_1
            .member(&identity)
            .await
            .expect("member handle")
            .send(
                "POST_CUT_TAIL_TURN token-omega".to_string(),
                HandlingMode::Queue,
            )
            .await
    })
    .await;
    eprintln!("[active-tail] send during cut => {send_result:?}");
    wait_for_row_contains(
        store_1.as_ref(),
        &w1_sid,
        "POST_CUT_TAIL_TURN",
        "active-tail-row",
    )
    .await;
    power_store
        .wait_for_prepared_boundary_commit_rejection()
        .await;
    let committed_token = committed_head_token(runtime_store.as_ref(), &w1_sid).await;
    let ahead_token = physical_head_token(&store_1, &w1_sid).await;
    assert_ne!(
        ahead_token, committed_token,
        "the physical head must be ahead of the retained runtime authority"
    );

    // The process dies with the member active and its tail in place.
    handle_1
        .crash_stop_preserving_durable_work_for_test()
        .await
        .expect("crash-stop lifetime-1 actor");
    service_1
        .try_shutdown()
        .await
        .expect("shutdown first session service");
    drop(handle_1);
    drop(service_1);
    drop(store_1);
    tokio::time::timeout(Duration::from_secs(2), async {
        while runtime_release_witnesses
            .iter()
            .any(|runtime| runtime.upgrade().is_some())
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("every old session runtime is released before lifetime 2");

    power_store.set_cut(false);
    power_store.forward_hosting_capability();
    ActiveMemberTail {
        runtime_store,
        session_id: w1_sid,
        committed_token,
        ahead_token,
    }
}

/// #1813, the resume side: a mob start never settles a durable tail on an
/// active member's session while another runtime owner hosts it. The start
/// refuses, typed, before the mob runs: never a start that settles the tail
/// and then stops the mob when it binds the member. Once the other owner lets
/// go, the next start settles the tail under its own claim and runs.
#[cfg(feature = "test-support")]
#[tokio::test(flavor = "multi_thread")]
async fn a_mob_resume_never_settles_an_active_members_tail_another_owner_hosts() {
    // nextest runs each test in its own process: show refusals logged at ERROR.
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::ERROR)
        .with_writer(std::io::stderr)
        .try_init();
    let temp = tempfile::tempdir().expect("temp dir");
    let paths = Paths::new(temp.path());
    let tail = crash_with_an_active_member_tail(temp.path(), &paths).await;
    let other_owner_claim = tail.another_owner_hosts_the_session();

    let (service_2, store_2) =
        persistent_head_canonical_service(&paths, tail.runtime_store.clone());
    let refused = MobBuilder::for_resume(
        MobStorage::persistent(&paths.mob_db_path).expect("reopen mob storage"),
    )
    .with_session_service(service_2.clone())
    .with_default_llm_client(openai_test_client())
    .notify_orchestrator_on_resume(false)
    .resume()
    .await;
    let refused_error = refused.as_ref().err().map(ToString::to_string);
    let refused_status = match &refused {
        Ok(handle) => Some(handle.status().await),
        Err(_) => None,
    };
    eprintln!(
        "[active-tail] start while hosted elsewhere => error {refused_error:?}, status \
         {refused_status:?}"
    );

    // Never another owner's tail: the runtime authority and the physical head
    // are exactly as the cut left them.
    assert_eq!(
        tail.committed_token_now().await,
        tail.committed_token,
        "a mob start must not settle a durable tail on an active member's session another \
         runtime owner hosts"
    );
    assert_eq!(
        tail.physical_token_now(&store_2).await,
        tail.ahead_token,
        "the refused start must leave the durable tail where the cut left it"
    );
    match refused {
        Err(meerkat_mob::MobError::SessionError(
            meerkat_core::service::SessionError::ServedElsewhere { id },
        )) => assert_eq!(
            id, tail.session_id,
            "the refusal names the member's session"
        ),
        Err(error) => panic!("the start must refuse, typed, as served elsewhere: {error}"),
        Ok(handle) => {
            let status = handle.status().await;
            let _ = handle.shutdown().await;
            panic!(
                "a start that cannot host an active member's session must refuse rather than \
                 run: {status:?}"
            );
        }
    }

    // The other owner lets go; the next start settles the tail under its own
    // claim and runs with the member.
    drop(other_owner_claim);
    let handle = MobBuilder::for_resume(
        MobStorage::persistent(&paths.mob_db_path).expect("reopen mob storage again"),
    )
    .with_session_service(service_2.clone())
    .with_default_llm_client(openai_test_client())
    .notify_orchestrator_on_resume(false)
    .resume()
    .await
    .unwrap_or_else(|error| {
        panic!("the mob must start once no other runtime owner hosts the member: {error}")
    });
    let status = handle.status().await;
    assert!(
        matches!(status, Ok(meerkat_mob::MobState::Running)),
        "the restarted mob must admit commands: {status:?}"
    );
    assert_member_active(
        &member_entry(&handle, "w-1").await,
        "after the other owner let go",
    );
    assert_member_active(
        &member_entry(&handle, "lead-1").await,
        "the lead after the refused start",
    );
    wait_for_head_canonical_authority_convergence(
        store_2.as_ref(),
        tail.runtime_store.as_ref(),
        &tail.session_id,
        "POST_CUT_TAIL_TURN",
        "hosted-active-tail-adopted-boundary",
    )
    .await;
    send_and_wait(
        &handle,
        service_2.as_ref(),
        "w-1",
        "AFTER_START token-beta",
        "hosted-active-tail-after-start",
    )
    .await;
    let _ = handle.shutdown().await;
}

/// #1813, the spawn side: a resume spawn (here the explicit revival of a
/// retired member's session) of a session another runtime owner hosts
/// refuses, typed, naming the session, and the mob runs on. Once the other
/// owner lets go, the same spawn revives the member. A respawn rebuilds its
/// member through the same resume provision once it has retired the
/// member's incarnation.
#[cfg(feature = "test-support")]
#[tokio::test(flavor = "multi_thread")]
async fn a_resume_spawn_of_a_session_another_owner_hosts_refuses_typed() {
    let temp = tempfile::tempdir().expect("temp dir");
    let paths = Paths::new(temp.path());
    let power_store = Arc::new(PowerCutRuntimeStore::with_hosting_root(
        &paths.realm_db_path,
        temp.path(),
    ));
    power_store.forward_hosting_capability();
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = power_store.clone();
    let (service, store) = persistent_head_canonical_service(&paths, runtime_store.clone());

    let mut definition = mob_definition(MobRuntimeMode::TurnDriven);
    definition.orchestrator = None;
    definition.wiring = WiringRules {
        auto_wire_orchestrator: false,
        role_wiring: vec![],
    };
    let handle = MobBuilder::new(
        definition,
        MobStorage::persistent(&paths.mob_db_path).expect("persistent mob storage"),
    )
    .with_session_service(service.clone())
    .with_default_llm_client(openai_test_client())
    .create()
    .await
    .expect("create persistent mob");
    let worker = AgentIdentity::from("w-1");
    handle
        .spawn_spec(SpawnMemberSpec::new("worker", worker.clone()))
        .await
        .expect("spawn worker");
    let session_id = handle
        .resolve_bridge_session_id(&worker)
        .await
        .expect("worker session id");
    send_and_wait(
        &handle,
        service.as_ref(),
        "w-1",
        "BEFORE_RETIRE token-alpha",
        "hosted-revival-before-retire",
    )
    .await;
    handle.retire(worker.clone()).await.expect("retire worker");

    // Another runtime owner on the realm takes the retired session's hosting
    // claim once the retired incarnation has let it go.
    let capability = meerkat_runtime::RuntimeStore::hosting_capability(runtime_store.as_ref());
    assert!(
        capability.is_cross_process(),
        "the realm store must offer cross-process hosting claims: {capability:?}"
    );
    let other_owner = meerkat_runtime::SessionHostingAuthority::new(
        capability,
        meerkat_runtime::HostingOwner::mint(),
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        while other_owner.serving(&session_id)
            == meerkat_runtime::SessionServing::HeldByAnotherLocalOwner
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the retired incarnation releases its hosting claim");
    let other_owner_claim = other_owner
        .grant(&session_id)
        .expect("another runtime owner takes the retired session's hosting claim");
    let committed_before = committed_head_token(runtime_store.as_ref(), &session_id).await;
    let physical_before = physical_head_token(&store, &session_id).await;

    let refused = tokio::time::timeout(
        Duration::from_secs(60),
        handle.spawn_spec(
            SpawnMemberSpec::new("worker", worker.clone())
                .with_resume_bridge_session_id(session_id.clone()),
        ),
    )
    .await
    .expect("the resume spawn settles");
    eprintln!(
        "[hosted-revival] resume spawn while hosted elsewhere => {:?}",
        refused.as_ref().err().map(ToString::to_string)
    );
    match refused {
        Err(meerkat_mob::MobError::SessionError(
            meerkat_core::service::SessionError::ServedElsewhere { id },
        )) => assert_eq!(id, session_id, "the refusal names the session"),
        Err(error) => panic!("the resume spawn must refuse, typed, as served elsewhere: {error}"),
        Ok(_) => panic!("a resume spawn of a session another runtime owner hosts must refuse"),
    }
    assert_eq!(
        committed_head_token(runtime_store.as_ref(), &session_id).await,
        committed_before,
        "the refused spawn must not write the session's runtime authority"
    );
    assert_eq!(
        physical_head_token(&store, &session_id).await,
        physical_before,
        "the refused spawn must not write the session's head"
    );
    let status = handle.status().await;
    assert!(
        matches!(status, Ok(meerkat_mob::MobState::Running)),
        "a refused spawn leaves the mob running: {status:?}"
    );

    // The other owner lets go; the same spawn revives the member.
    drop(other_owner_claim);
    handle
        .spawn_spec(
            SpawnMemberSpec::new("worker", worker.clone())
                .with_resume_bridge_session_id(session_id.clone()),
        )
        .await
        .expect("revive the worker once no other runtime owner hosts its session");
    assert_member_active(&member_entry(&handle, "w-1").await, "after the revival");
    send_and_wait(
        &handle,
        service.as_ref(),
        "w-1",
        "AFTER_REVIVAL token-beta",
        "hosted-revival-after",
    )
    .await;
    let _ = handle.shutdown().await;
}

/// A spawn customizer that refuses one member's restore while `refusing`
/// holds: the start then leaves that member restore-failed.
#[cfg(feature = "test-support")]
struct RefuseOneRestore {
    member: AgentIdentity,
    refusing: std::sync::atomic::AtomicBool,
}

#[cfg(feature = "test-support")]
impl meerkat_mob::runtime::SpawnMemberCustomizer for RefuseOneRestore {
    fn customize_spawn(
        &self,
        ctx: &meerkat_mob::runtime::SpawnCustomizationContext,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), meerkat_mob::MobError> {
        if ctx.spawn_source == meerkat_mob::runtime::SpawnSource::Resume
            && spec.identity == self.member
            && self.refusing.load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(meerkat_mob::MobError::Internal(
                "the test customizer refuses this restore".to_string(),
            ));
        }
        Ok(())
    }
}

/// Resume the mob over an [`ActiveMemberTail`] with `w-1`'s restore refused
/// by a spawn customizer: the start's member reconcile never reaches `w-1`'s
/// session, so its durable tail is still in place when the actor starts.
#[cfg(feature = "test-support")]
async fn resume_with_the_tailed_member_restore_refused(
    paths: &Paths,
    tail: &ActiveMemberTail,
) -> (
    Result<MobHandle, meerkat_mob::MobError>,
    Arc<PersistentSessionService<FactoryAgentBuilder>>,
    Arc<SqliteSessionStore>,
    Arc<RefuseOneRestore>,
) {
    let (service, store) = persistent_head_canonical_service(paths, tail.runtime_store.clone());
    let customizer = Arc::new(RefuseOneRestore {
        member: AgentIdentity::from("w-1"),
        refusing: std::sync::atomic::AtomicBool::new(true),
    });
    let started = MobBuilder::for_resume(
        MobStorage::persistent(&paths.mob_db_path).expect("reopen mob storage"),
    )
    .with_session_service(service.clone())
    .with_default_llm_client(openai_test_client())
    .with_spawn_member_customizer(customizer.clone())
    .notify_orchestrator_on_resume(false)
    .resume()
    .await;
    (started, service, store, customizer)
}

/// A member whose restore failed at a start (here its spawn customizer
/// refused it) keeps the durable tail its last turn left, and the start
/// reads that member's session to restore its operation binding. That read
/// must not stop the whole mob: the start settles the tail under this
/// runtime owner's hosting claim and runs. Once the customizer admits the
/// member, a respawn rebuilds it on the settled session.
#[cfg(feature = "test-support")]
#[tokio::test(flavor = "multi_thread")]
async fn a_mob_start_settles_the_durable_tail_of_a_member_whose_restore_failed() {
    // nextest runs each test in its own process: show refusals logged at ERROR.
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::ERROR)
        .with_writer(std::io::stderr)
        .try_init();
    let temp = tempfile::tempdir().expect("temp dir");
    let paths = Paths::new(temp.path());
    let tail = crash_with_an_active_member_tail(temp.path(), &paths).await;
    let (started, service, store, customizer) =
        resume_with_the_tailed_member_restore_refused(&paths, &tail).await;
    let handle = started.expect("the mob starts without the refused member");
    let status = handle.status().await;
    assert!(
        matches!(status, Ok(meerkat_mob::MobState::Running)),
        "a member whose restore failed must not stop the mob: {status:?}"
    );
    wait_for_head_canonical_authority_convergence(
        store.as_ref(),
        tail.runtime_store.as_ref(),
        &tail.session_id,
        "POST_CUT_TAIL_TURN",
        "restore-failed-tail-settled-at-start",
    )
    .await;

    customizer
        .refusing
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let respawned = tokio::time::timeout(
        Duration::from_secs(60),
        handle.respawn(AgentIdentity::from("w-1"), None),
    )
    .await
    .expect("the respawn settles");
    eprintln!(
        "[restore-failed-tail] respawn => {:?}",
        respawned.as_ref().err().map(ToString::to_string)
    );
    respawned.expect("a respawn rebuilds the member on its settled session");
    assert_member_active(&member_entry(&handle, "w-1").await, "after the respawn");
    send_and_wait(
        &handle,
        service.as_ref(),
        "w-1",
        "AFTER_RESPAWN token-beta",
        "restore-failed-tail-after-respawn",
    )
    .await;
    let _ = handle.shutdown().await;
}

/// #1813: the same start while another runtime owner on the realm hosts that
/// member's session does not settle the other owner's tail. The start
/// refuses, logging the member, its session and the remedy at ERROR, and the
/// tail stays where it was.
#[cfg(feature = "test-support")]
#[tokio::test(flavor = "multi_thread")]
async fn a_mob_start_refuses_a_restore_failed_members_tail_another_owner_hosts() {
    // nextest runs each test in its own process: show refusals logged at ERROR.
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::ERROR)
        .with_writer(std::io::stderr)
        .try_init();
    let temp = tempfile::tempdir().expect("temp dir");
    let paths = Paths::new(temp.path());
    let tail = crash_with_an_active_member_tail(temp.path(), &paths).await;
    let _other_owner_claim = tail.another_owner_hosts_the_session();
    let (started, _service, store, _customizer) =
        resume_with_the_tailed_member_restore_refused(&paths, &tail).await;
    let status = match &started {
        Ok(handle) => Some(handle.status().await),
        Err(_) => None,
    };
    eprintln!(
        "[hosted-restore-failed-tail] start => error {:?}, status {status:?}",
        started.as_ref().err().map(ToString::to_string)
    );
    assert_eq!(
        tail.committed_token_now().await,
        tail.committed_token,
        "a mob start must not settle a durable tail on a session another runtime owner hosts"
    );
    assert_eq!(
        tail.physical_token_now(&store).await,
        tail.ahead_token,
        "the refused start must leave the durable tail where the cut left it"
    );
    assert!(
        tail.provisional_tail_recorded_now().await,
        "the refused start must leave the provisional tail recorded"
    );
    assert!(
        !matches!(status, Some(Ok(meerkat_mob::MobState::Running))),
        "a start that cannot settle a member's tail another runtime owner hosts must not run: \
         {status:?}"
    );
    if let Ok(handle) = started {
        let _ = handle.shutdown().await;
    }
}

/// A resume spawn onto a session this mob does not own (here another mob's
/// member session) whose durable tail runs past its committed runtime
/// boundary refuses, typed, as the transcript revision conflict it is, and
/// leaves the tail alone: a mob does not complete the boundary of a session
/// it is about to refuse to adopt.
#[cfg(feature = "test-support")]
#[tokio::test(flavor = "multi_thread")]
async fn a_resume_spawn_onto_a_foreign_session_tail_refuses_typed() {
    let temp = tempfile::tempdir().expect("temp dir");
    let paths = Paths::new(temp.path());
    let tail = crash_with_an_active_member_tail(temp.path(), &paths).await;
    let (service, store) = persistent_head_canonical_service(&paths, tail.runtime_store.clone());
    let adopting_mob_db_path = temp.path().join("adopting-mob.db");
    let handle = MobBuilder::new(
        mob_definition(MobRuntimeMode::TurnDriven),
        MobStorage::persistent(&adopting_mob_db_path).expect("second mob storage"),
    )
    .with_session_service(service.clone())
    .with_default_llm_client(openai_test_client())
    .create()
    .await
    .expect("create a second mob on the realm");
    handle
        .spawn_spec(SpawnMemberSpec::new("lead", AgentIdentity::from("lead-2")))
        .await
        .expect("spawn the second mob's lead");
    let adopter = AgentIdentity::from("w-adopt");
    let spawned = tokio::time::timeout(
        Duration::from_secs(60),
        handle.spawn_spec(
            SpawnMemberSpec::new("worker", adopter.clone())
                .with_resume_bridge_session_id(tail.session_id.clone()),
        ),
    )
    .await
    .expect("the resume spawn settles");
    eprintln!(
        "[foreign-tail-spawn] spawn => {:?}",
        spawned.as_ref().err().map(ToString::to_string)
    );
    assert_eq!(
        tail.committed_token_now().await,
        tail.committed_token,
        "a refused adoption must not settle the session's durable tail"
    );
    assert_eq!(
        tail.physical_token_now(&store).await,
        tail.ahead_token,
        "a refused adoption must leave the durable tail where the cut left it"
    );
    assert!(
        tail.provisional_tail_recorded_now().await,
        "a refused adoption must leave the provisional tail recorded"
    );
    match &spawned {
        Err(meerkat_mob::MobError::SessionError(meerkat_core::service::SessionError::Store(
            error,
        ))) if error
            .downcast_ref::<meerkat_core::SessionStoreError>()
            .is_some_and(|error| {
                matches!(
                    error,
                    meerkat_core::SessionStoreError::TranscriptRevisionConflict { id, .. }
                        if *id == tail.session_id
                )
            }) => {}
        other => panic!(
            "the resume spawn must refuse, typed, as the session's transcript revision \
             conflict: {:?}",
            other.as_ref().err().map(ToString::to_string)
        ),
    }
    let status = handle.status().await;
    assert!(
        matches!(status, Ok(meerkat_mob::MobState::Running)),
        "a refused spawn leaves the mob running: {status:?}"
    );
    assert!(
        handle
            .get_member(&adopter)
            .await
            .expect("read the roster")
            .is_none(),
        "the refused spawn rosters no member"
    );
    let _ = handle.shutdown().await;
}

/// A member whose session has a durable tail ahead of its runtime boundary
/// authority respawns. The power cut leaves the canonical head carrying a
/// turn whose runtime boundary commit never became durable (the same window a
/// turn between its checkpoint and its boundary commit shows). With power
/// back, the respawn reads the restrictions the member was spawned under from
/// its committed record before it retires anything; that read must not need
/// the transcript at the boundary the head has moved past, or the respawn is
/// refused and the member keeps its stale incarnation.
///
/// The member's actor has ended with its failed boundary, and a read of its
/// session refuses as a transcript revision conflict while the tail stands.
/// The retirement's runtime quiesce reloads the degraded registration, which
/// commits the tail, before the retirement's route and archive reads; so the
/// respawn succeeds and the retired session keeps the tail turn exactly once.
/// Were those reads ordered before the reload, they would refuse, and so would
/// the respawn.
#[cfg(feature = "test-support")]
#[tokio::test(flavor = "multi_thread")]
async fn a_member_whose_session_has_a_durable_tail_respawns() {
    let temp = tempfile::tempdir().expect("temp dir");
    let paths = Paths::new(temp.path());
    let power_store = Arc::new(PowerCutRuntimeStore::new(&paths.realm_db_path));
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = power_store.clone();
    let (service, store) = persistent_head_canonical_service(&paths, runtime_store.clone());
    let storage = MobStorage::persistent(&paths.mob_db_path).expect("persistent mob storage");
    let handle = MobBuilder::new(mob_definition(MobRuntimeMode::TurnDriven), storage)
        .with_session_service(service.clone())
        .with_default_llm_client(openai_test_client())
        .create()
        .await
        .expect("create persistent mob");
    handle
        .spawn_spec(SpawnMemberSpec::new("lead", AgentIdentity::from("lead-1")))
        .await
        .expect("spawn lead");
    handle
        .spawn_spec(SpawnMemberSpec::new("worker", AgentIdentity::from("w-1")))
        .await
        .expect("spawn worker");
    let identity = AgentIdentity::from("w-1");
    let w1_sid = handle
        .resolve_bridge_session_id(&identity)
        .await
        .expect("w1 session id");
    let incarnation_before = handle
        .get_member(&identity)
        .await
        .expect("read the roster")
        .expect("the member is rostered")
        .agent_runtime_id;

    send_and_wait(
        &handle,
        service.as_ref(),
        "w-1",
        "PRE_CUT_TAIL_TURN token-alpha",
        "respawn-tail-turn",
    )
    .await;
    wait_for_head_canonical_authority_convergence(
        store.as_ref(),
        runtime_store.as_ref(),
        &w1_sid,
        "PRE_CUT_TAIL_TURN",
        "respawn-tail-pre-cut-boundary",
    )
    .await;

    // The next turn's durable row lands; its runtime boundary commit does not.
    power_store.set_cut(true);
    let send_result = tokio::time::timeout(Duration::from_secs(30), async {
        handle
            .member(&identity)
            .await
            .expect("member handle")
            .send(
                "POST_CUT_TAIL_TURN token-omega".to_string(),
                HandlingMode::Queue,
            )
            .await
    })
    .await;
    eprintln!("[respawn-tail] send during cut => {send_result:?}");
    wait_for_row_contains(
        store.as_ref(),
        &w1_sid,
        "POST_CUT_TAIL_TURN",
        "respawn-tail-row",
    )
    .await;
    {
        let deadline = Instant::now() + Duration::from_secs(20);
        while power_store.prepared_boundary_commit_rejections() == 0 {
            assert!(
                Instant::now() < deadline,
                "[respawn-tail] prepared boundary commit was never rejected under the cut"
            );
            sleep(Duration::from_millis(50)).await;
        }
    }
    let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(&w1_sid);
    let authority = runtime_store
        .load_session_boundary_authority(&runtime_id)
        .await
        .expect("load runtime session authority")
        .expect("runtime session authority present");
    let authority = authority
        .head_canonical()
        .expect("head-canonical boundary authority");
    let ahead_head = store
        .load_head(&w1_sid)
        .await
        .expect("load ahead physical head")
        .expect("ahead physical head exists");
    assert_ne!(
        meerkat_core::session_store::session_head_cas_token(&ahead_head)
            .expect("ahead physical head has a canonical token"),
        authority.committed_head_token(),
        "the physical head must be ahead of the retained runtime authority"
    );
    assert!(
        ahead_head.message_count > authority.boundary_head().message_count,
        "the physical head must carry a strict continuation past the boundary"
    );

    // Power is back; the member respawns with the tail in place.
    power_store.set_cut(false);
    // The member's actor has ended with its failed boundary, and a read of its
    // session refuses while the tail stands: only the retirement's reload can
    // settle it.
    let sessions: &dyn MobSessionService = service.as_ref();
    tokio::time::timeout(Duration::from_secs(30), async {
        while sessions
            .has_live_session(&w1_sid)
            .await
            .expect("read whether the member's session is live")
        {
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the member's actor ends with its failed boundary");
    match sessions.load_persisted_session(&w1_sid).await {
        Err(meerkat_core::service::SessionError::Store(error))
            if error
                .downcast_ref::<meerkat_core::SessionStoreError>()
                .is_some_and(|error| {
                    matches!(
                        error,
                        meerkat_core::SessionStoreError::TranscriptRevisionConflict { id, .. }
                            if *id == w1_sid
                    )
                }) => {}
        other => panic!(
            "a read of the session must refuse as its transcript revision conflict while \
             the tail stands: {:?}",
            other.map(|session| session.is_some())
        ),
    }
    let respawned = tokio::time::timeout(
        Duration::from_secs(60),
        handle.respawn(identity.clone(), None),
    )
    .await
    .expect("the respawn ends within its bound");
    assert!(
        respawned.is_ok(),
        "a member whose session has a durable tail must respawn: {respawned:?}"
    );
    let incarnation_after = handle
        .get_member(&identity)
        .await
        .expect("read the roster")
        .expect("the respawned member is rostered")
        .agent_runtime_id;
    assert_ne!(
        incarnation_after, incarnation_before,
        "the respawn installs a new incarnation"
    );
    let retired = service
        .load_revivable_retired_session(&w1_sid)
        .await
        .expect("read the member's retired session")
        .expect("the respawn archives the member's previous session");
    let history = to_string(retired.messages()).expect("serialize the retired transcript");
    for token in ["PRE_CUT_TAIL_TURN", "POST_CUT_TAIL_TURN"] {
        assert_eq!(
            history.matches(token).count(),
            1,
            "the retirement's reload commits the tail, so the retired session keeps \
             '{token}' exactly once: {history}"
        );
    }
    let _ = handle.shutdown().await;
}

/// The restrictions a respawn reads from a HeadCanonical session's current
/// boundary metadata are the ones its committed body states. The member is
/// spawned under a launch policy, so an answer that loses or widens it
/// differs from the body read.
#[cfg(feature = "test-support")]
#[tokio::test(flavor = "multi_thread")]
async fn the_metadata_restriction_read_matches_the_committed_body() {
    let temp = tempfile::tempdir().expect("temp dir");
    let paths = Paths::new(temp.path());
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
        meerkat_runtime::store::SqliteRuntimeStore::new_head_canonical(&paths.realm_db_path)
            .expect("open HeadCanonical runtime store"),
    );
    let (service, store) = persistent_head_canonical_service(&paths, runtime_store.clone());
    let storage = MobStorage::persistent(&paths.mob_db_path).expect("persistent mob storage");
    let handle = MobBuilder::new(mob_definition(MobRuntimeMode::TurnDriven), storage)
        .with_session_service(service.clone())
        .with_default_llm_client(openai_test_client())
        .create()
        .await
        .expect("create persistent mob");
    let launch =
        meerkat_core::ops::ToolAccessPolicy::DenyList(["shell".to_string()].into_iter().collect());
    handle
        .spawn_spec(
            SpawnMemberSpec::new("worker", AgentIdentity::from("w-1"))
                .with_tool_access_policy(launch.clone()),
        )
        .await
        .expect("spawn worker");
    let w1_sid = handle
        .resolve_bridge_session_id(&AgentIdentity::from("w-1"))
        .await
        .expect("w1 session id");
    send_and_wait(
        &handle,
        service.as_ref(),
        "w-1",
        "RESTRICTION_TURN token-alpha",
        "restriction-read-turn",
    )
    .await;
    wait_for_head_canonical_authority_convergence(
        store.as_ref(),
        runtime_store.as_ref(),
        &w1_sid,
        "RESTRICTION_TURN",
        "restriction-read-boundary",
    )
    .await;

    let from_metadata = service
        .predecessor_tool_restrictions(&w1_sid)
        .await
        .expect("the metadata read states the restrictions");
    let body = service
        .observe_authoritative_session_body(&w1_sid)
        .await
        .expect("read the committed body")
        .expect("the committed body exists");
    let from_body = meerkat_core::PredecessorToolRestrictions::from_committed_session(&body)
        .expect("the committed body states the restrictions");
    assert_eq!(
        from_metadata, from_body,
        "the metadata read and the committed body state the same restrictions"
    );
    assert_eq!(
        from_metadata.launch_policy(),
        Some(launch),
        "the member's launch policy is part of what both reads state"
    );
    let _ = handle.shutdown().await;
}

#[cfg(feature = "test-support")]
const HELD_TURN_PROMPT: &str = "HELD_TURN token-sigma";

/// A provider whose first request for the held prompt waits until the test
/// releases it; every other request answers at once.
#[cfg(feature = "test-support")]
#[derive(Default)]
struct HeldTurnClient {
    held: std::sync::atomic::AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

#[cfg(feature = "test-support")]
#[async_trait::async_trait]
impl meerkat_client::LlmClient for HeldTurnClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a meerkat_client::LlmRequest,
    ) -> std::pin::Pin<
        Box<
            dyn futures::Stream<Item = Result<meerkat_client::LlmEvent, meerkat_client::LlmError>>
                + Send
                + 'a,
        >,
    > {
        use futures::StreamExt as _;
        let events = vec![
            meerkat_client::LlmEvent::TextDelta {
                delta: "done".to_string(),
                meta: None,
            },
            meerkat_client::LlmEvent::UsageUpdate {
                usage: meerkat_core::TurnUsage::host_declared(
                    meerkat_core::Provider::OpenAI,
                    &request.model,
                    meerkat_core::Usage::default(),
                ),
            },
            meerkat_client::LlmEvent::Done {
                outcome: meerkat_client::LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
            },
        ];
        let mentions_held_prompt = request.messages.iter().any(|message| {
            !matches!(message, meerkat_core::Message::System(_))
                && to_string(message).is_ok_and(|text| text.contains(HELD_TURN_PROMPT))
        });
        if mentions_held_prompt && !self.held.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return Box::pin(
                futures::stream::once(async move {
                    self.entered.notify_one();
                    self.release.notified().await;
                })
                .flat_map(move |()| futures::stream::iter(events.clone().into_iter().map(Ok))),
            );
        }
        Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

/// A mob restarts after a member's retirement cancelled a turn at its
/// boundary. The held provider call is released once the retirement has asked
/// the turn to cancel at its boundary, so the turn's durable rows land and the
/// run ends cancelled without committing that boundary. The retirement settles
/// that tail in process (the runtime authority names the physical head again)
/// and retires the member; the process then dies, and the next start must
/// admit commands. A crash inside the window before the settle leaves the tail
/// for the next start, whose cleanup restore settles it the same way.
#[cfg(feature = "test-support")]
#[tokio::test(flavor = "multi_thread")]
async fn a_mob_whose_member_retirement_cancelled_a_turn_at_its_boundary_restarts() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(
            "warn,meerkat_mob::runtime::actor::retirement_io=info,meerkat_runtime::recovery=info,\
             meerkat_mob::runtime::actor=info",
        ))
        .with_writer(std::io::stderr)
        .try_init();
    let temp = tempfile::tempdir().expect("temp dir");
    let paths = Paths::new(temp.path());
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
        meerkat_runtime::store::SqliteRuntimeStore::new_head_canonical(&paths.realm_db_path)
            .expect("open HeadCanonical runtime store"),
    );

    // ---------------- Lifetime 1 ----------------
    let (service_1, store_1) = persistent_head_canonical_service(&paths, runtime_store.clone());
    let client = Arc::new(HeldTurnClient::default());
    let storage_1 = MobStorage::persistent(&paths.mob_db_path).expect("persistent mob storage");
    let handle_1 = MobBuilder::new(mob_definition(MobRuntimeMode::TurnDriven), storage_1)
        .with_session_service(service_1.clone())
        .with_default_llm_client(client.clone())
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
    let identity = AgentIdentity::from("w-1");
    let w1_sid = handle_1
        .resolve_bridge_session_id(&identity)
        .await
        .expect("w1 session id");
    let lead_sid = handle_1
        .resolve_bridge_session_id(&AgentIdentity::from("lead-1"))
        .await
        .expect("lead session id");
    let mut runtime_release_witnesses = Vec::new();
    for session_id in [&lead_sid, &w1_sid] {
        let runtime = service_1
            .comms_runtime(session_id)
            .await
            .unwrap_or_else(|| panic!("comms runtime missing for {session_id}"));
        runtime_release_witnesses.push(Arc::downgrade(&runtime));
    }
    send_and_wait(
        &handle_1,
        service_1.as_ref(),
        "w-1",
        "PRE_HELD_TURN token-alpha",
        "cancelled-tail-first",
    )
    .await;
    wait_for_head_canonical_authority_convergence(
        store_1.as_ref(),
        runtime_store.as_ref(),
        &w1_sid,
        "PRE_HELD_TURN",
        "cancelled-tail-first-boundary",
    )
    .await;
    handle_1
        .member(&identity)
        .await
        .expect("member handle")
        .send(HELD_TURN_PROMPT.to_string(), HandlingMode::Queue)
        .await
        .expect("queue the held turn");
    tokio::time::timeout(Duration::from_secs(30), client.entered.notified())
        .await
        .expect("the held turn's provider call is in flight");
    let retire = tokio::spawn({
        let handle = handle_1.clone();
        let identity = identity.clone();
        async move { handle.retire(identity).await }
    });
    sleep(Duration::from_millis(300)).await;
    client.release.notify_one();
    let retired = tokio::time::timeout(Duration::from_secs(60), retire)
        .await
        .expect("the retirement ends within its bound")
        .expect("the retirement task completes");
    assert!(
        retired.is_ok(),
        "the member's retirement settles the cancelled turn's tail and retires: {retired:?}"
    );
    let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(&w1_sid);
    let authority = runtime_store
        .load_session_boundary_authority(&runtime_id)
        .await
        .expect("load runtime session authority")
        .expect("runtime session authority present");
    let authority = authority
        .head_canonical()
        .expect("head-canonical boundary authority");
    let physical_head = store_1
        .load_head(&w1_sid)
        .await
        .expect("load physical head")
        .expect("physical head exists");
    assert_eq!(
        meerkat_core::session_store::session_head_cas_token(&physical_head)
            .expect("physical head has a canonical token"),
        authority.committed_head_token(),
        "the retirement settles the cancelled turn's tail: the runtime authority names the physical head"
    );
    handle_1
        .crash_stop_preserving_durable_work_for_test()
        .await
        .expect("crash-stop lifetime-1 actor");
    service_1
        .try_shutdown()
        .await
        .expect("shutdown first session service");
    drop(handle_1);
    drop(service_1);
    drop(store_1);
    tokio::time::timeout(Duration::from_secs(2), async {
        while runtime_release_witnesses
            .iter()
            .any(|runtime| runtime.upgrade().is_some())
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("every old session runtime is released before lifetime 2");

    // ---------------- Lifetime 2 ----------------
    let (service_2, _store_2) = persistent_head_canonical_service(&paths, runtime_store.clone());
    let storage_2 = MobStorage::persistent(&paths.mob_db_path).expect("reopen mob storage");
    let handle_2 = MobBuilder::for_resume(storage_2)
        .with_session_service(service_2.clone())
        .with_default_llm_client(openai_test_client())
        .notify_orchestrator_on_resume(false)
        .resume()
        .await
        .unwrap_or_else(|error| {
            panic!("a mob whose member retirement cancelled a turn must restart: {error}")
        });
    let status = handle_2.status().await;
    assert!(
        matches!(status, Ok(meerkat_mob::MobState::Running)),
        "the restarted mob must admit commands, not fail-stop before admission: {status:?}"
    );
    let _ = handle_2.shutdown().await;
}

/// A member retired while its turn runs retires when the turn still reaches
/// its boundary. The retirement asks the running turn to cancel at its
/// boundary; the held provider call is then released inside the cooperative
/// grace, so the turn's durable rows land and the run ends cancelled at the
/// boundary, leaving the member's canonical head ahead of the runtime
/// boundary it last committed. The rest of the retirement reads the session
/// (its route observation first); those reads must settle the tail instead of
/// refusing it, or the retirement stops after it durably started.
#[cfg(feature = "test-support")]
#[tokio::test(flavor = "multi_thread")]
async fn a_member_retired_while_its_turn_reaches_the_boundary_retires() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(
            "warn,meerkat_mob::runtime::actor::retirement_io=info,meerkat_runtime::recovery=info,\
             meerkat_session::persistent=info,meerkat_mob::runtime::actor=info",
        ))
        .with_writer(std::io::stderr)
        .try_init();
    let temp = tempfile::tempdir().expect("temp dir");
    let paths = Paths::new(temp.path());
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
        meerkat_runtime::store::SqliteRuntimeStore::new_head_canonical(&paths.realm_db_path)
            .expect("open HeadCanonical runtime store"),
    );
    let (service, store) = persistent_head_canonical_service(&paths, runtime_store.clone());
    let client = Arc::new(HeldTurnClient::default());
    let storage = MobStorage::persistent(&paths.mob_db_path).expect("persistent mob storage");
    let handle = MobBuilder::new(mob_definition(MobRuntimeMode::TurnDriven), storage)
        .with_session_service(service.clone())
        .with_default_llm_client(client.clone())
        .create()
        .await
        .expect("create persistent mob");
    handle
        .spawn_spec(SpawnMemberSpec::new("lead", AgentIdentity::from("lead-1")))
        .await
        .expect("spawn lead");
    handle
        .spawn_spec(SpawnMemberSpec::new("worker", AgentIdentity::from("w-1")))
        .await
        .expect("spawn worker");
    let identity = AgentIdentity::from("w-1");
    let w1_sid = handle
        .resolve_bridge_session_id(&identity)
        .await
        .expect("w1 session id");
    send_and_wait(
        &handle,
        service.as_ref(),
        "w-1",
        "PRE_HELD_TURN token-alpha",
        "held-turn-first",
    )
    .await;
    wait_for_head_canonical_authority_convergence(
        store.as_ref(),
        runtime_store.as_ref(),
        &w1_sid,
        "PRE_HELD_TURN",
        "held-turn-first-boundary",
    )
    .await;

    handle
        .member(&identity)
        .await
        .expect("member handle")
        .send(HELD_TURN_PROMPT.to_string(), HandlingMode::Queue)
        .await
        .expect("queue the held turn");
    tokio::time::timeout(Duration::from_secs(30), client.entered.notified())
        .await
        .expect("the held turn's provider call is in flight");
    let retire = tokio::spawn({
        let handle = handle.clone();
        let identity = identity.clone();
        async move { handle.retire(identity).await }
    });
    // The retirement reaches its runtime quiesce and asks the turn to cancel
    // at its boundary; the turn then finishes well inside the cooperative
    // grace and reaches that boundary.
    sleep(Duration::from_millis(300)).await;
    client.release.notify_one();
    let retired = tokio::time::timeout(Duration::from_secs(60), retire)
        .await
        .expect("the retirement ends within its bound")
        .expect("the retirement task completes");
    assert!(
        retired.is_ok(),
        "a member retired while its turn reaches the boundary must retire: {retired:?}"
    );
    assert!(
        handle
            .get_member(&identity)
            .await
            .expect("read the roster")
            .is_none(),
        "the retired member leaves the roster"
    );
    let _ = handle.shutdown().await;
}

/// One destructive reset as Meta's identity reset sends it: an atomic
/// successor respawn onto a fresh session, with no parent auto-wiring.
async fn destructive_reset(handle: &MobHandle, identity: &AgentIdentity) -> Result<(), String> {
    let mut successor = SpawnMemberSpec::new("worker", identity.clone());
    successor.launch_mode = meerkat_mob::MemberLaunchMode::Fresh;
    successor.auto_wire_parent = false;
    handle
        .respawn_with_successor_spec(successor)
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Send `prompt` to the member and wait (bounded) until its current session's
/// history carries it.
async fn turn_lands(
    handle: &MobHandle,
    service: &dyn MobSessionService,
    identity: &AgentIdentity,
    prompt: &str,
) -> Result<(), String> {
    let session_id = handle
        .resolve_bridge_session_id(identity)
        .await
        .ok_or_else(|| "the member has no session".to_string())?;
    tokio::time::timeout(Duration::from_secs(30), async {
        handle
            .member(identity)
            .await
            .map_err(|error| error.to_string())?
            .send(prompt.to_string(), HandlingMode::Queue)
            .await
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|_| format!("sending {prompt} timed out"))??;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if history_blob(service, &session_id).await.contains(prompt) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "{prompt} never reached the history of {session_id}"
            ));
        }
        sleep(Duration::from_millis(100)).await;
    }
}

/// What one reset-after-reply scenario observed.
#[derive(Debug, PartialEq, Eq)]
struct ResetRaceObserved {
    reset: Result<(), String>,
    successor_on_a_new_session: bool,
    retried_reset: Option<Result<(), String>>,
    next_turn: Result<(), String>,
}

/// A host resets a member to a clean slate (Meta's identity reset: an atomic
/// successor respawn onto a fresh session) right after the member's reply,
/// while that turn's runtime boundary commit is still in flight. The turn's
/// final checkpoint has already moved the session's canonical head past the
/// boundary the runtime last committed. The reset must not race that commit:
/// it succeeds, the successor runs on a new session, and the member answers.
#[tokio::test(flavor = "multi_thread")]
async fn a_destructive_reset_right_after_a_reply_does_not_race_its_boundary_commit() {
    let temp = tempfile::tempdir().expect("temp dir");
    let paths = Paths::new(temp.path());
    let gated_store = Arc::new(PowerCutRuntimeStore::new(&paths.realm_db_path));
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = gated_store.clone();
    let (service, _store) = persistent_head_canonical_service(&paths, runtime_store);
    let storage = MobStorage::persistent(&paths.mob_db_path).expect("persistent mob storage");
    let handle = MobBuilder::new(mob_definition(MobRuntimeMode::TurnDriven), storage)
        .with_session_service(service.clone())
        .with_default_llm_client(openai_test_client())
        .create()
        .await
        .expect("create persistent mob");
    let identity = AgentIdentity::from("w-1");
    handle
        .spawn_spec(SpawnMemberSpec::new("worker", identity.clone()))
        .await
        .expect("spawn worker");
    let old_session = handle
        .resolve_bridge_session_id(&identity)
        .await
        .expect("worker session");

    // The member answers; its turn's runtime boundary commit waits at the gate.
    gated_store.hold_boundary_commits(meerkat_runtime::LogicalRuntimeId::for_session(&old_session));
    let _release_on_every_path = ReleaseBoundaryCommitsOnDrop(Arc::clone(&gated_store));
    tokio::time::timeout(Duration::from_secs(30), async {
        handle
            .member(&identity)
            .await
            .expect("member handle")
            .send(
                "RESET_RACE_TURN remember 42".to_string(),
                HandlingMode::Queue,
            )
            .await
            .expect("send the turn")
    })
    .await
    .expect("the turn is admitted");
    tokio::time::timeout(
        Duration::from_secs(30),
        gated_store.wait_boundary_commit_held(),
    )
    .await
    .expect("the turn's boundary commit reaches the gate");

    // The host resets the member while that commit is in flight.
    let mut reset = tokio::spawn({
        let handle = handle.clone();
        let identity = identity.clone();
        async move { destructive_reset(&handle, &identity).await }
    });
    let while_held = tokio::time::timeout(Duration::from_secs(5), &mut reset)
        .await
        .ok()
        .map(|joined| joined.expect("reset task"));
    gated_store.release_boundary_commits();
    let reset_result = match while_held.clone() {
        Some(result) => result,
        None => tokio::time::timeout(Duration::from_secs(30), reset)
            .await
            .expect("the reset returns once the commit lands")
            .expect("reset task"),
    };
    eprintln!("[reset-race] while the commit was held: {while_held:?}; reset: {reset_result:?}");

    // Impact when the reset was refused: a retried reset, then a turn.
    let retried_reset = if reset_result.is_err() {
        Some(
            tokio::time::timeout(
                Duration::from_secs(30),
                destructive_reset(&handle, &identity),
            )
            .await
            .unwrap_or_else(|_| Err("the retried reset timed out".to_string())),
        )
    } else {
        None
    };
    let successor_on_a_new_session = handle
        .resolve_bridge_session_id(&identity)
        .await
        .is_some_and(|session| session != old_session);
    let next_turn = turn_lands(
        &handle,
        service.as_ref(),
        &identity,
        "AFTER_RESET_TURN token-omega",
    )
    .await;

    assert_eq!(
        ResetRaceObserved {
            reset: reset_result,
            successor_on_a_new_session,
            retried_reset,
            next_turn,
        },
        ResetRaceObserved {
            reset: Ok(()),
            successor_on_a_new_session: true,
            retried_reset: None,
            next_turn: Ok(()),
        },
        "a reset right after a reply waits for, or fences, the turn's boundary commit"
    );
}

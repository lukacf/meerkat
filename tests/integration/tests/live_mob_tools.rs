#![cfg(all(feature = "integration-real-tests", not(target_arch = "wasm32")))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//!
//! Live integration tests for the agent mob tool surface (#191).
//!
//! These tests exercise the actual tool dispatch path through RPC with real
//! LLM calls. Every assertion uses authoritative state (MobMcpState, session
//! service) — never model narration.
//!
//! Run with:
//!   ANTHROPIC_API_KEY=... cargo nextest run -p meerkat-integration-tests \
//!     --test live_mob_tools --run-ignored ignored-only --test-threads=1

use meerkat::{AgentFactory, Config};
use meerkat_core::service::MobToolsFactory;
use meerkat_core::types::SessionId;
use meerkat_core::{ConfigRuntime, MemoryConfigStore};
use meerkat_mob::MobId;
use meerkat_mob_mcp::MobMcpState;
use meerkat_rpc::protocol::{RpcId, RpcRequest};
use meerkat_rpc::router::{MethodRouter, NotificationSink};
use meerkat_rpc::session_runtime::SessionRuntime;
use serde::Serialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant, sleep};

// ---------------------------------------------------------------------------
// API key helpers
// ---------------------------------------------------------------------------

fn first_env(vars: &[&str]) -> Option<String> {
    for name in vars {
        if let Ok(value) = std::env::var(name)
            && !value.is_empty()
        {
            return Some(value);
        }
    }
    None
}

fn anthropic_api_key() -> Option<String> {
    first_env(&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"])
}

fn smoke_model() -> String {
    std::env::var("SMOKE_MODEL").unwrap_or_else(|_| "claude-sonnet-4-5".to_string())
}

fn has_key_for_smoke_model(model: &str) -> bool {
    let lower = model.to_ascii_lowercase();
    if lower.starts_with("claude-") {
        anthropic_api_key().is_some()
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// Temp directory structure
// ---------------------------------------------------------------------------

struct SmokePaths {
    user_config_root: PathBuf,
    runtime_root: PathBuf,
    project_root: PathBuf,
    context_root: PathBuf,
}

impl SmokePaths {
    fn new(root: &Path) -> Self {
        Self {
            user_config_root: root.join("user-config"),
            runtime_root: root.join("runtime-root"),
            project_root: root.join("project-root"),
            context_root: root.join("context-root"),
        }
    }
}

// ---------------------------------------------------------------------------
// Factory + service
// ---------------------------------------------------------------------------

fn smoke_factory(paths: &SmokePaths) -> AgentFactory {
    AgentFactory::new(paths.runtime_root.join("factory-store"))
        .user_config_root(paths.user_config_root.clone())
        .runtime_root(paths.runtime_root.clone())
        .project_root(paths.project_root.clone())
        .context_root(paths.context_root.clone())
        .builtins(true)
        .shell(false)
        .comms(true)
        .mob(true)
}

// ---------------------------------------------------------------------------
// RPC stack builder
// ---------------------------------------------------------------------------

/// Build a full RPC stack with mob tools wired.
/// Returns (router, mob_state, notification_rx).
/// The mob_state is the SAME instance used by both the router (archive cleanup)
/// and the builder (tool factory), ensuring single-state truth.
async fn make_smoke_rpc_stack(
    paths: &SmokePaths,
) -> (
    MethodRouter,
    Arc<MobMcpState>,
    mpsc::Receiver<meerkat_rpc::protocol::RpcNotification>,
) {
    let factory = smoke_factory(paths);
    let config = Config::default();
    let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::new());
    let persistence = meerkat::PersistenceBundle::new(store, None, blob_store);

    let mut runtime = SessionRuntime::new(
        factory,
        config.clone(),
        64,
        persistence,
        NotificationSink::noop(),
    );
    let config_store: Arc<dyn meerkat_core::ConfigStore> = Arc::new(MemoryConfigStore::new(config));
    runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
        Arc::clone(&config_store),
        paths.runtime_root.join("config_state.json"),
    )));

    // Create mob state from the runtime's session service, preserving the
    // runtime adapter so delegate-spawned AutonomousHost helpers can boot.
    let mob_state = Arc::new(MobMcpState::new_with_runtime_adapter(
        runtime.session_service(),
        Some(runtime.runtime_adapter()),
    ));

    // Wire mob tools factory into the builder inside the session service.
    // This uses the builder_mob_tools_slot (Arc<RwLock>) to write through
    // to the consumed builder.
    *runtime.builder_mob_tools_slot.write().unwrap() = Some(Arc::new(
        meerkat_mob_mcp::AgentMobToolSurfaceFactory::new(Arc::clone(&mob_state)),
    ));

    let runtime = Arc::new(runtime);
    let (notif_tx, notif_rx) = mpsc::channel(256);
    let sink = NotificationSink::new(notif_tx);
    let router =
        MethodRouter::new_with_mob_state(runtime, config_store, sink, Arc::clone(&mob_state));
    (router, mob_state, notif_rx)
}

// ---------------------------------------------------------------------------
// RPC helpers
// ---------------------------------------------------------------------------

fn rpc_request(method: &str, params: impl Serialize) -> RpcRequest {
    let params_raw =
        serde_json::value::RawValue::from_string(serde_json::to_string(&params).unwrap()).unwrap();
    RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(RpcId::Num(1)),
        method: method.to_string(),
        params: Some(params_raw),
    }
}

fn rpc_result(resp: &meerkat_rpc::protocol::RpcResponse) -> Value {
    assert!(resp.error.is_none(), "RPC error: {:?}", resp.error);
    let raw = resp.result.as_ref().expect("missing result");
    serde_json::from_str(raw.get()).unwrap()
}

async fn rpc_create_session(router: &MethodRouter, model: &str, prompt: &str) -> SessionId {
    let resp = router
        .dispatch(rpc_request(
            "session/create",
            json!({
                "model": model,
                "prompt": prompt,
                "enable_mob": true,
                "enable_builtins": true,
                "max_tokens": 4096,
            }),
        ))
        .await
        .unwrap();
    let result = rpc_result(&resp);
    let sid = result["session_id"].as_str().expect("session_id");
    SessionId::parse(sid).expect("parse session_id")
}

async fn rpc_turn(router: &MethodRouter, session_id: &SessionId, prompt: &str) -> Value {
    let resp = router
        .dispatch(rpc_request(
            "turn/start",
            json!({
                "session_id": session_id.to_string(),
                "prompt": prompt,
            }),
        ))
        .await
        .unwrap();
    rpc_result(&resp)
}

async fn rpc_archive(router: &MethodRouter, session_id: &SessionId) {
    let resp = router
        .dispatch(rpc_request(
            "session/archive",
            json!({ "session_id": session_id.to_string() }),
        ))
        .await
        .unwrap();
    let result = rpc_result(&resp);
    assert_eq!(result["archived"], true);
}

async fn session_history_json(
    router: &MethodRouter,
    session_id: &SessionId,
    limit: usize,
) -> Value {
    let read_resp = router
        .dispatch(rpc_request(
            "session/history",
            json!({
                "session_id": session_id.to_string(),
                "limit": limit,
            }),
        ))
        .await
        .unwrap();
    rpc_result(&read_resp)
}

async fn session_history_text(
    router: &MethodRouter,
    session_id: &SessionId,
    limit: usize,
) -> String {
    serde_json::to_string(&session_history_json(router, session_id, limit).await)
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Guard macro for skipping tests without API keys
// ---------------------------------------------------------------------------

macro_rules! skip_if_no_key {
    ($model:expr) => {
        if !has_key_for_smoke_model(&$model) {
            eprintln!("Skipping: no API key for model {}", $model);
            return;
        }
    };
}

// ===========================================================================
// Tests
// ===========================================================================

/// 1. Force real tool calls to prove delegate and mob_list are dispatchable.
#[tokio::test]
#[ignore = "lane:e2e-live"]
async fn e2e_agent_tools_surface_present() {
    let model = smoke_model();
    skip_if_no_key!(model);

    let temp = TempDir::new().unwrap();
    let paths = SmokePaths::new(temp.path());
    let (router, mob_state, _notif_rx) = make_smoke_rpc_stack(&paths).await;
    let router = Arc::new(router);

    let session_id = rpc_create_session(
        &router,
        &model,
        "Call the mob_list tool exactly once. Then call the delegate tool exactly once \
         with task 'Reply with the single word PONG' and member_id 'probe-helper'.",
    )
    .await;

    // Assert via authoritative state: delegate was called → implicit mob exists.
    let implicit = mob_state.find_implicit_mob(&session_id.to_string()).await;
    assert!(
        implicit.is_some(),
        "delegate should have created an implicit mob for session {session_id}"
    );

    // mob_list was called (we can't verify the LLM saw the result, but the
    // implicit mob's existence proves delegate ran, and mob_list is dispatched
    // in the same turn).
    let all_mobs = mob_state.mob_list().await;
    assert!(
        !all_mobs.is_empty(),
        "mob_list should show at least the implicit mob"
    );
}

/// 2. Delegate creates implicit mob, archive cleans it up.
#[tokio::test]
#[ignore = "lane:e2e-live"]
async fn e2e_delegate_creates_implicit_mob() {
    let model = smoke_model();
    skip_if_no_key!(model);

    let temp = TempDir::new().unwrap();
    let paths = SmokePaths::new(temp.path());
    let (router, mob_state, _notif_rx) = make_smoke_rpc_stack(&paths).await;

    let session_id = rpc_create_session(
        &router,
        &model,
        "Call the delegate tool exactly once with task 'Reply with HELLO' \
         and member_id 'helper-alpha'.",
    )
    .await;

    // Assert: implicit mob exists
    let mob_id = mob_state
        .find_implicit_mob(&session_id.to_string())
        .await
        .expect("implicit mob should exist after delegate");

    // Assert: mob is classified as implicit
    assert!(mob_state.is_implicit_mob(&mob_id).await);

    // Assert: roster contains the helper
    let handle = mob_state.handle_for(&mob_id).await.expect("mob handle");
    let roster = handle.roster().await;
    assert!(
        roster
            .get(&meerkat_mob::MeerkatId::from("helper-alpha"))
            .is_some(),
        "roster should contain helper-alpha"
    );

    // Archive the session
    rpc_archive(&router, &session_id).await;

    // Assert: implicit mob cleaned up
    assert!(
        mob_state
            .find_implicit_mob(&session_id.to_string())
            .await
            .is_none(),
        "implicit mob should be cleaned up after archive"
    );
}

/// 3. Second delegate reuses the same implicit mob.
#[tokio::test]
#[ignore = "lane:e2e-live"]
async fn e2e_delegate_reuses_implicit_mob_across_turns() {
    let model = smoke_model();
    skip_if_no_key!(model);

    let temp = TempDir::new().unwrap();
    let paths = SmokePaths::new(temp.path());
    let (router, mob_state, _notif_rx) = make_smoke_rpc_stack(&paths).await;

    let session_id = rpc_create_session(
        &router,
        &model,
        "Call the delegate tool exactly once with task 'Reply with FIRST' \
         and member_id 'h-1'.",
    )
    .await;

    let mob_id_1 = mob_state
        .find_implicit_mob(&session_id.to_string())
        .await
        .expect("implicit mob after first delegate");

    // Second turn: another delegate
    rpc_turn(
        &router,
        &session_id,
        "Call the delegate tool exactly once with task 'Reply with SECOND' \
         and member_id 'h-2'.",
    )
    .await;

    let mob_id_2 = mob_state
        .find_implicit_mob(&session_id.to_string())
        .await
        .expect("implicit mob after second delegate");

    // Same mob reused
    assert_eq!(
        mob_id_1, mob_id_2,
        "implicit mob should be reused across turns"
    );

    // Roster should have 2 members
    let handle = mob_state.handle_for(&mob_id_1).await.expect("handle");
    let roster = handle.roster().await;
    assert!(
        roster.len() >= 2,
        "roster should have at least 2 members after two delegates, got {}",
        roster.len()
    );

    // Only one mob owned by this session
    let owned = mob_state
        .find_mobs_for_session(&session_id.to_string())
        .await;
    assert_eq!(
        owned.len(),
        1,
        "should own exactly one mob (the implicit one)"
    );
}

/// 5. Full explicit mob lifecycle: create → spawn → check → retire → destroy.
#[tokio::test]
#[ignore = "lane:e2e-live"]
async fn e2e_mob_create_spawn_check_retire_destroy_full_roundtrip() {
    let model = smoke_model();
    skip_if_no_key!(model);

    let temp = TempDir::new().unwrap();
    let paths = SmokePaths::new(temp.path());
    let (router, mob_state, _notif_rx) = make_smoke_rpc_stack(&paths).await;

    let definition_json = json!({
        "id": "smoke-explicit",
        "profiles": {
            "worker": {
                "model": model,
                "tools": { "comms": true },
                "peer_description": "Smoke test worker",
                "runtime_mode": "turn_driven"
            }
        },
        "wiring": { "auto_wire_orchestrator": false, "role_wiring": [] }
    });

    // Turn 1: mob_create
    let session_id = rpc_create_session(
        &router,
        &model,
        &format!(
            "Call mob_create with this exact definition JSON: {}",
            serde_json::to_string(&definition_json).unwrap()
        ),
    )
    .await;

    // Assert: explicit mob exists and is NOT implicit
    let owned = mob_state
        .find_mobs_for_session(&session_id.to_string())
        .await;
    assert!(
        !owned.is_empty(),
        "session should own a mob after mob_create"
    );
    let mob_id = owned[0].clone();
    assert!(
        !mob_state.is_implicit_mob(&mob_id).await,
        "explicitly created mob should not be classified as implicit"
    );

    // Turn 2: mob_spawn_member
    rpc_turn(
        &router,
        &session_id,
        &format!(
            "Call mob_spawn_member with mob_id='{}', profile='worker', \
             member_id='w-1', initial_message='Say WORKER_READY'.",
            mob_id
        ),
    )
    .await;

    // Assert: member exists in roster
    let handle = mob_state.handle_for(&mob_id).await.expect("handle");
    let roster = handle.roster().await;
    assert!(
        roster.get(&meerkat_mob::MeerkatId::from("w-1")).is_some(),
        "worker w-1 should be in roster after spawn"
    );

    // The member runs its initial turn (turn_driven mode = one turn per start_turn).
    // We don't wait for is_final here since turn_driven members stay alive between turns.
    // Instead we verify the member exists in the roster and proceed.

    // Turn 3: mob_check_member (agent calls it, we verify via mob_state)
    rpc_turn(
        &router,
        &session_id,
        &format!(
            "Call mob_check_member with mob_id='{}' and member_id='w-1'.",
            mob_id
        ),
    )
    .await;

    // Turn 4: mob_list_members
    rpc_turn(
        &router,
        &session_id,
        &format!("Call mob_list_members with mob_id='{}'.", mob_id),
    )
    .await;

    // Turn 5: mob_retire_member
    rpc_turn(
        &router,
        &session_id,
        &format!(
            "Call mob_retire_member with mob_id='{}' and member_id='w-1'.",
            mob_id
        ),
    )
    .await;

    // Assert: member is_final after retire (poll)
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let status = mob_state
            .mob_member_status(&mob_id, &meerkat_mob::MeerkatId::from("w-1"))
            .await;
        match status {
            Ok(snap) if snap.is_final => break,
            Err(_) => break, // member not found = retired
            _ => {}
        }
        assert!(Instant::now() < deadline, "timed out waiting for retire");
        sleep(Duration::from_millis(200)).await;
    }

    // Turn 6: mob_destroy
    rpc_turn(
        &router,
        &session_id,
        &format!("Call mob_destroy with mob_id='{}'.", mob_id),
    )
    .await;

    // Assert: mob gone
    let all = mob_state.mob_list().await;
    assert!(
        !all.iter().any(|(id, _)| id == &mob_id),
        "destroyed mob should not appear in mob_list"
    );
}

/// 6. Session B cannot see or mutate session A's mobs.
#[tokio::test]
#[ignore = "lane:e2e-live"]
async fn e2e_agent_cannot_see_other_sessions_mobs() {
    let model = smoke_model();
    skip_if_no_key!(model);

    let temp = TempDir::new().unwrap();
    let paths = SmokePaths::new(temp.path());
    let (router, mob_state, _notif_rx) = make_smoke_rpc_stack(&paths).await;

    let definition_json = json!({
        "id": "session-a-mob",
        "profiles": {
            "worker": {
                "model": model,
                "tools": { "comms": true },
                "peer_description": "A's worker",
                "runtime_mode": "turn_driven"
            }
        },
        "wiring": { "auto_wire_orchestrator": false, "role_wiring": [] }
    });

    // Session A: create mob
    let session_a = rpc_create_session(
        &router,
        &model,
        &format!(
            "Call mob_create with definition: {}",
            serde_json::to_string(&definition_json).unwrap()
        ),
    )
    .await;

    let a_mobs = mob_state
        .find_mobs_for_session(&session_a.to_string())
        .await;
    assert_eq!(a_mobs.len(), 1);
    let a_mob_id = a_mobs[0].clone();

    // Session B: try to list (should not see A's mob)
    let session_b = rpc_create_session(
        &router,
        &model,
        "Call mob_list. Then reply with the result.",
    )
    .await;

    // B should own zero mobs
    let b_mobs = mob_state
        .find_mobs_for_session(&session_b.to_string())
        .await;
    assert!(b_mobs.is_empty(), "session B should not own any mobs");

    // A's mob should still exist
    assert!(
        mob_state
            .mob_list()
            .await
            .iter()
            .any(|(id, _)| id == &a_mob_id),
        "A's mob should still exist in global state"
    );

    // Session B tries to destroy A's mob
    rpc_turn(
        &router,
        &session_b,
        &format!("Call mob_destroy with mob_id='{}'.", a_mob_id),
    )
    .await;

    // A's mob should STILL exist (destroy rejected)
    assert!(
        mob_state
            .mob_list()
            .await
            .iter()
            .any(|(id, _)| id == &a_mob_id),
        "A's mob should survive B's destroy attempt"
    );
}

/// 7. Archive cleans up both implicit and explicit session-owned mobs.
#[tokio::test]
#[ignore = "lane:e2e-live"]
async fn e2e_archive_cleans_both_implicit_and_explicit_mobs() {
    let model = smoke_model();
    skip_if_no_key!(model);

    let temp = TempDir::new().unwrap();
    let paths = SmokePaths::new(temp.path());
    let (router, mob_state, _notif_rx) = make_smoke_rpc_stack(&paths).await;

    let definition_json = json!({
        "id": "cleanup-explicit",
        "profiles": {
            "worker": {
                "model": model,
                "tools": { "comms": true },
                "peer_description": "cleanup worker",
                "runtime_mode": "turn_driven"
            }
        },
        "wiring": { "auto_wire_orchestrator": false, "role_wiring": [] }
    });

    // Create session, force delegate (implicit) and mob_create (explicit)
    let session_id = rpc_create_session(
        &router,
        &model,
        &format!(
            "Do two things in order:\n\
             1. Call delegate with task 'Reply DONE' and member_id 'cleanup-h'\n\
             2. Call mob_create with definition: {}",
            serde_json::to_string(&definition_json).unwrap()
        ),
    )
    .await;

    // Assert: 2 mobs owned by this session
    let owned = mob_state
        .find_mobs_for_session(&session_id.to_string())
        .await;
    assert_eq!(
        owned.len(),
        2,
        "session should own 2 mobs (1 implicit + 1 explicit), got {}",
        owned.len()
    );

    // Archive
    rpc_archive(&router, &session_id).await;

    // Assert: 0 mobs owned
    let owned_after = mob_state
        .find_mobs_for_session(&session_id.to_string())
        .await;
    assert_eq!(
        owned_after.len(),
        0,
        "all session-owned mobs should be cleaned up after archive"
    );

    // Neither mob in global list
    let all = mob_state.mob_list().await;
    for mob_id in &owned {
        assert!(
            !all.iter().any(|(id, _)| id == mob_id),
            "mob {mob_id} should not appear in global list after archive"
        );
    }
}

/// 10. Error contracts: implicit mob destroy rejected, bad member, spoofed owner.
#[tokio::test]
#[ignore = "lane:e2e-live"]
async fn e2e_tool_error_contracts() {
    let model = smoke_model();
    skip_if_no_key!(model);

    let temp = TempDir::new().unwrap();
    let paths = SmokePaths::new(temp.path());
    let (router, mob_state, _notif_rx) = make_smoke_rpc_stack(&paths).await;

    // Create session with delegate (creates implicit mob)
    let session_id = rpc_create_session(
        &router,
        &model,
        "Call delegate with task 'Say HI' and member_id 'err-helper'.",
    )
    .await;

    let implicit_mob_id = mob_state
        .find_implicit_mob(&session_id.to_string())
        .await
        .expect("implicit mob");

    // Try to destroy implicit mob (should fail)
    rpc_turn(
        &router,
        &session_id,
        &format!(
            "Call mob_destroy with mob_id='{}'. Report what happened.",
            implicit_mob_id
        ),
    )
    .await;

    // Assert: implicit mob still exists
    assert!(
        mob_state.is_implicit_mob(&implicit_mob_id).await,
        "implicit mob should survive destroy attempt"
    );

    // Create explicit mob with spoofed owner_session_id
    let spoofed_def = json!({
        "id": "spoofed-mob",
        "owner_session_id": "spoofed-session-999",
        "is_implicit": true,
        "profiles": {
            "worker": {
                "model": model,
                "tools": { "comms": true },
                "peer_description": "spoofed",
                "runtime_mode": "turn_driven"
            }
        },
        "wiring": { "auto_wire_orchestrator": false, "role_wiring": [] }
    });
    rpc_turn(
        &router,
        &session_id,
        &format!(
            "Call mob_create with definition: {}",
            serde_json::to_string(&spoofed_def).unwrap()
        ),
    )
    .await;

    // Assert: spoofed mob is owned by the CURRENT session, not the spoofed one
    let owned = mob_state
        .find_mobs_for_session(&session_id.to_string())
        .await;
    // Should own 2: implicit + the "spoofed" one (which was re-tagged)
    assert!(
        owned.len() >= 2,
        "session should own the spoofed mob (re-tagged to current session)"
    );
    let spoofed_mobs = mob_state.find_mobs_for_session("spoofed-session-999").await;
    assert!(
        spoofed_mobs.is_empty(),
        "no mobs should be owned by the spoofed session"
    );
    assert!(
        !mob_state.is_implicit_mob(&MobId::from("spoofed-mob")).await,
        "mob_create must not allow callers to mint faux implicit mobs"
    );
}

/// 4. Model override on rebuild recreates implicit mob with new model.
///
/// This tests the stale-model refresh path in AgentMobToolSurfaceFactory.
/// We exercise it directly since RPC turn/start hot-swaps the LLM client
/// without rebuilding the agent (build_mob_tools only runs at create time).
#[tokio::test]
#[ignore = "lane:e2e-live"]
async fn e2e_resume_model_override_recreates_implicit_mob() {
    let model_a = smoke_model();
    skip_if_no_key!(model_a);

    // Use a second model from the same provider. claude-haiku-4-5 if
    // the smoke model is a Claude model; otherwise skip.
    let model_b = if model_a.to_ascii_lowercase().starts_with("claude-") {
        "claude-haiku-4-5".to_string()
    } else {
        eprintln!("Skipping: model override test requires two Claude models");
        return;
    };

    let temp = TempDir::new().unwrap();
    let paths = SmokePaths::new(temp.path());
    let (router, mob_state, _notif_rx) = make_smoke_rpc_stack(&paths).await;

    // Session 1: create with model A, force delegate
    let session_id = rpc_create_session(
        &router,
        &model_a,
        "Call delegate with task 'Reply PING' and member_id 'ha-1'.",
    )
    .await;

    let mob_id_a = mob_state
        .find_implicit_mob(&session_id.to_string())
        .await
        .expect("implicit mob after first delegate");

    // Verify profile model is model A
    let handle_a = mob_state.handle_for(&mob_id_a).await.expect("handle");
    let profile_model_a = handle_a
        .definition()
        .profiles
        .get(&meerkat_mob::ProfileName::from("delegate"))
        .map(|p| p.model.clone())
        .expect("delegate profile");
    assert_eq!(
        profile_model_a, model_a,
        "implicit mob delegate profile should use model A"
    );

    // Now simulate what happens on resume with a different model:
    // Call build_mob_tools directly with model B and the same session_id.
    // This is the path that AgentFactory::build_agent() takes on rebuild.
    let factory = meerkat_mob_mcp::AgentMobToolSurfaceFactory::new(Arc::clone(&mob_state));
    let _dispatcher = factory
        .build_mob_tools(meerkat_core::service::MobToolsBuildArgs {
            session_id: session_id.clone(),
            model: model_b.clone(),
            authority_context: Some(meerkat_core::service::MobToolAuthorityContext::new(
                meerkat_core::service::OpaquePrincipalToken::new("e2e-resume-override"),
                true,
            )),
            effective_authority: None,
            comms_name: None,
            comms_runtime: None,
        })
        .await
        .expect("build_mob_tools with model B");

    // After build_mob_tools with model B:
    // - The existing implicit mob is untouched
    // - Reconciliation stays with the surface/runtime on the next delegate
    //   or explicit ensure_implicit_mob_for_model call
    assert!(
        mob_state
            .find_implicit_mob(&session_id.to_string())
            .await
            .as_ref()
            == Some(&mob_id_a),
        "build_mob_tools must not destroy or replace the implicit mob during rebuild"
    );
    let unchanged_handle = mob_state
        .handle_for(&mob_id_a)
        .await
        .expect("old mob handle");
    let unchanged_model = unchanged_handle
        .definition()
        .profiles
        .get(&meerkat_mob::ProfileName::from("delegate"))
        .map(|p| p.model.clone())
        .expect("delegate profile after build");
    assert_eq!(
        unchanged_model, model_a,
        "factory rebuild must not mutate the existing implicit mob in place"
    );

    let (reconciled_mob_id, created) = mob_state
        .ensure_implicit_mob_for_model(&session_id.to_string(), &model_b, Some(&mob_id_a))
        .await
        .expect("reconcile implicit mob after model override");
    assert!(
        created,
        "model mismatch should force on-demand implicit mob reconciliation"
    );
    assert_eq!(
        reconciled_mob_id, mob_id_a,
        "implicit mob IDs stay canonical per session across model reconciliation"
    );
    let reconciled_handle = mob_state
        .handle_for(&reconciled_mob_id)
        .await
        .expect("reconciled mob handle");
    let reconciled_model = reconciled_handle
        .definition()
        .profiles
        .get(&meerkat_mob::ProfileName::from("delegate"))
        .map(|p| p.model.clone())
        .expect("delegate profile after reconciliation");
    assert_eq!(
        reconciled_model, model_b,
        "on-demand reconciliation should refresh the implicit mob model"
    );
}

/// 8. Bidirectional comms: helper sends message, parent receives it.
///
/// This is the litmus test for whether the wiring actually works. The parent
/// must receive the helper's message in its session history — roster wired_to
/// is not proof of delivery.
#[tokio::test]
#[ignore = "lane:e2e-live"]
async fn e2e_delegate_bidirectional_comms() {
    let model = smoke_model();
    skip_if_no_key!(model);

    let temp = TempDir::new().unwrap();
    let paths = SmokePaths::new(temp.path());
    let (router, mob_state, _notif_rx) = make_smoke_rpc_stack(&paths).await;
    let router = Arc::new(router);

    let nonce = format!("COMMS_PING_{}", uuid::Uuid::new_v4().simple());

    let resp = router
        .dispatch(rpc_request(
            "session/create",
            json!({
                "model": model,
                "prompt": format!(
                    "Call the delegate tool exactly once with task \
                     'Call the send tool exactly once with kind peer_message, \
                     to e2e-comms-parent, and body exactly {nonce}. \
                     After the send succeeds, reply with SENT.' \
                     and member_id 'comms-helper'. \
                     After delegate returns, reply with DELEGATED."
                ),
                "initial_turn": "deferred",
                "enable_mob": true,
                "enable_builtins": true,
                "keep_alive": true,
                "comms_name": "e2e-comms-parent",
                "max_tokens": 4096,
            }),
        ))
        .await
        .unwrap();
    let result = rpc_result(&resp);
    let session_id = SessionId::parse(result["session_id"].as_str().expect("session_id"))
        .expect("parse session_id");

    let router_for_turn = Arc::clone(&router);
    let session_id_for_turn = session_id.clone();
    let mut turn_task = tokio::spawn(async move {
        router_for_turn
            .dispatch(rpc_request(
                "turn/start",
                json!({
                    "session_id": session_id_for_turn.to_string(),
                    "prompt": "Execute the pending instructions now. Do not add any extra work.",
                }),
            ))
            .await
    });

    // Wait for the implicit mob to be created (delegate was called).
    let deadline = Instant::now() + Duration::from_secs(120);
    let mob_id = loop {
        if let Some(id) = mob_state.find_implicit_mob(&session_id.to_string()).await {
            break id;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for implicit mob creation via delegate; turn_task={}; history={}",
            if turn_task.is_finished() {
                match tokio::time::timeout(Duration::from_secs(1), &mut turn_task).await {
                    Ok(Ok(Some(resp))) => format!("finished:{resp:?}"),
                    Ok(Ok(None)) => "finished:none".to_string(),
                    Ok(Err(join_err)) => format!("join_error:{join_err}"),
                    Err(_) => "finished:timed_out_join".to_string(),
                }
            } else {
                "running".to_string()
            },
            session_history_text(router.as_ref(), &session_id, 50).await
        );
        sleep(Duration::from_millis(500)).await;
    };

    // Wait for the helper member to appear in the roster.
    let helper_id = meerkat_mob::MeerkatId::from("comms-helper");
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let handle = mob_state.handle_for(&mob_id).await.expect("handle");
        if handle.roster().await.get(&helper_id).is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for comms-helper to appear in roster; turn_task={}; roster={:?}; history={}",
            if turn_task.is_finished() {
                match tokio::time::timeout(Duration::from_secs(1), &mut turn_task).await {
                    Ok(Ok(Some(resp))) => format!("finished:{resp:?}"),
                    Ok(Ok(None)) => "finished:none".to_string(),
                    Ok(Err(join_err)) => format!("join_error:{join_err}"),
                    Err(_) => "finished:timed_out_join".to_string(),
                }
            } else {
                "running".to_string()
            },
            handle
                .roster()
                .await
                .list()
                .map(|entry| entry.meerkat_id.to_string())
                .collect::<Vec<_>>(),
            session_history_text(router.as_ref(), &session_id, 100).await
        );
        sleep(Duration::from_millis(500)).await;
    }

    // Wait for the helper's initial turn to produce output.
    // The helper runs in AutonomousHost mode (set by delegate), so it won't
    // reach is_final — it stays alive waiting for comms. We check for output
    // instead, which indicates the initial turn completed.
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let status = mob_state.mob_member_status(&mob_id, &helper_id).await;
        if let Ok(snap) = &status
            && (snap.output_preview.is_some() || snap.is_final)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for comms-helper to produce output"
        );
        sleep(Duration::from_millis(500)).await;
    }

    // THE CRITICAL ASSERTION: verify the nonce landed in the parent's session history.
    // Comms messages are injected into the session context by the comms drain.
    // With keep_alive=true, the drain is active and should have processed the
    // helper's message.
    let deadline = Instant::now() + Duration::from_secs(30);
    let found = loop {
        let history_text = session_history_text(router.as_ref(), &session_id, 100).await;
        if history_text.contains(&nonce) {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        sleep(Duration::from_millis(500)).await;
    };

    assert!(
        found,
        "parent session history should contain the nonce '{nonce}' sent by the helper. \
         This proves bidirectional comms: helper→parent message was delivered and \
         injected into the parent's conversation context. history={}",
        session_history_text(router.as_ref(), &session_id, 100).await
    );

    // Clean up: interrupt the keep-alive session
    let _ = router
        .dispatch(rpc_request(
            "turn/interrupt",
            json!({ "session_id": session_id.to_string() }),
        ))
        .await;

    turn_task.abort();
}

#![cfg(all(feature = "integration-real-tests", not(target_arch = "wasm32")))]
#![allow(clippy::all)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Operator-scenario E2E smoke test with REAL LLM API calls.
//!
//! Proves the identity-first pipeline works end-to-end through the shipped
//! Meta UnifiedRuntimeBuilder → SessionBridge → real agent sessions.
//!
//! Run:
//! ```bash
//! ANTHROPIC_API_KEY=... ./scripts/repo-cargo test -p meerkat-meta \
//!     --test identity_first_operator_smoke --features integration-real-tests \
//!     -- --ignored --test-threads=1 --nocapture
//! ```

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::time::sleep;

use meerkat_mob::definition::WiringRules;
use meerkat_mob::{
    MobDefinition, MobId, MobRuntimeMode, Profile, ProfileBinding, ProfileName, ToolConfig,
};

use meerkat_meta::UnifiedRuntimeBuilder;
use meerkat_meta::identity_first::contracts::{AgentCustomizer, TopologyProvider};
use meerkat_meta::identity_first::orchestrator::{RestoreOutcome, restore_flow};
use meerkat_meta::identity_first::{
    AgentAddressability, AgentBuildContext, AgentBuildDraft, AgentIdentity, ContinuityStore,
    CorrelationId, CustomizerError, DispatchIdempotencyKey, DispatchInput, DispatchOrigin,
    DurabilityPolicy, DurableAgentSpec, IdentityLifecycleState, IdentityRuntime,
    IdentityRuntimeConfig, IdentityRuntimeError, LocalContinuityStore, LocalLeaseProvider,
    ManagedPeerEdge, SessionBridge, TopologyContext, TopologyError,
};
use meerkat_meta::member_comms_id::roster_member_id_for_identity;
use meerkat_meta::mob_handle_runtime::SessionCreatedContext;

// ---------------------------------------------------------------------------
// Env helpers
// ---------------------------------------------------------------------------

fn first_env(vars: &[&str]) -> Option<String> {
    for name in vars {
        if let Ok(value) = std::env::var(name) {
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

fn has_api_key() -> bool {
    first_env(&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]).is_some()
}

fn smoke_model() -> String {
    std::env::var("SMOKE_MODEL").unwrap_or_else(|_| "claude-sonnet-4-5".to_string())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn id(name: &str) -> AgentIdentity {
    AgentIdentity::parse(name).unwrap()
}

fn spec(name: &str, addr: AgentAddressability, profile: &str) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: id(name),
        profile: ProfileName::from(profile),
        addressability: addr,
        display_name: None,
        labels: BTreeMap::new(),
        context: None,
        additional_instructions: Vec::new(),
        initial_message: None,
        runtime_mode_override: None,
        backend: None,
        binding: None,
        placement: None,
    }
}

/// Build a MobDefinition for operator smoke tests. `test` makes the mob id
/// unique per test: member inproc participant names are
/// `{mob_id}/{role}/{identity}` and process-global, so the members of a
/// runtime a test leaves registered (one dropped without shutdown) never
/// block another test's members when the rows share one test process.
fn operator_definition(model: &str, test: &str) -> MobDefinition {
    let mut profiles = BTreeMap::new();

    profiles.insert(
        ProfileName::from("personal"),
        ProfileBinding::Inline(Box::new(Profile {
            model: model.to_string(),
            model_fallback: None,
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
            peer_description: "Personal assistant agent".to_string(),
            external_addressable: true,
            backend: None,
            runtime_mode: MobRuntimeMode::TurnDriven,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
            durable: false,
        })),
    );

    profiles.insert(
        ProfileName::from("review"),
        ProfileBinding::Inline(Box::new(Profile {
            model: model.to_string(),
            model_fallback: None,
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
            peer_description: "Review coordinator".to_string(),
            external_addressable: false,
            backend: None,
            runtime_mode: MobRuntimeMode::TurnDriven,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
            durable: false,
        })),
    );

    let mut definition = MobDefinition::explicit(MobId::from(format!("ops-smoke-{test}")));
    definition.profiles = profiles;
    definition.wiring = WiringRules {
        auto_wire_orchestrator: false,
        role_wiring: vec![],
    };
    definition
}

// History reading functions removed — we verify LLM response via
// MobMemberSnapshot.output_preview which is populated by the mob runtime
// after the LLM turn completes.

// ---------------------------------------------------------------------------
// Stub providers
// ---------------------------------------------------------------------------

struct EmptyTopology;

#[async_trait]
impl TopologyProvider for EmptyTopology {
    async fn compute_edges(
        &self,
        _target_identities: &[AgentIdentity],
        _context: &TopologyContext,
    ) -> Result<Vec<ManagedPeerEdge>, TopologyError> {
        Ok(vec![])
    }
}

struct NoopCustomizer;

#[async_trait]
impl AgentCustomizer for NoopCustomizer {
    async fn customize_build(
        &self,
        _context: &AgentBuildContext,
        _spec: &DurableAgentSpec,
        _draft: &mut AgentBuildDraft,
    ) -> Result<(), CustomizerError> {
        Ok(())
    }

    async fn after_create(
        &self,
        _identity: &AgentIdentity,
        _session_id: &meerkat_core::types::SessionId,
        _context: &SessionCreatedContext,
    ) -> Result<(), CustomizerError> {
        Ok(())
    }
}

// ===========================================================================
// SMOKE-01: Identity-first send → real LLM response
//
// Full chain: UnifiedRuntimeBuilder.build() → SessionBridge
// → IdentityRuntime.send() → bridge.deliver() → MobHandle.member().send()
// → LLM processes turn → assistant response in session history
// ===========================================================================

#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_smoke_01_identity_first_send_real_llm() {
    if !has_api_key() {
        eprintln!("Skipping SMOKE-01: no ANTHROPIC_API_KEY");
        return;
    }

    let model = smoke_model();
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state_path = temp.path().join("state");
    let start = Instant::now();

    eprintln!("[SMOKE-01] Building UnifiedRuntime with model={model}...");

    // --- Build the shipped runtime ---
    let unified = UnifiedRuntimeBuilder::default()
        .definition(operator_definition(&model, "01"))
        .persistent_state(&state_path)
        .comms(true)
        .build()
        .await
        .expect("build UnifiedRuntime");

    // --- Get the bridge ---
    let bridge: Arc<dyn SessionBridge> = unified
        .session_bridge()
        .map(|bridge| Arc::new(bridge) as Arc<dyn SessionBridge>)
        .expect("session_bridge should exist");

    // --- Build identity-first runtime ---
    let store = Arc::new(
        LocalContinuityStore::open(state_path.join("continuity.db"))
            .expect("open continuity store"),
    );
    let leases = Arc::new(LocalLeaseProvider::new());
    let identity_rt = IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: store.clone() as Arc<dyn ContinuityStore>,
        lease_provider: leases.clone(),
        runtime_instance_id: "ops-01".to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(bridge),
        default_timeout: None,
    });

    // --- Roster: 1 addressable personal agent ---
    let roster = vec![spec(
        "personal:alice",
        AgentAddressability::Addressable,
        "personal",
    )];

    // --- restore_flow: spawns a REAL mob member ---
    eprintln!("[SMOKE-01] Running restore_flow (spawns real agent)...");
    let result = restore_flow(
        &identity_rt,
        &roster,
        Some(&EmptyTopology as &dyn TopologyProvider),
        Some(&NoopCustomizer as &dyn AgentCustomizer),
    )
    .await
    .expect("restore_flow");

    let alice_binding = match result.outcomes.get(&id("personal:alice")).unwrap() {
        RestoreOutcome::Created { binding, .. } => binding.clone(),
        other => panic!("expected Created, got {other:?}"),
    };
    eprintln!(
        "[SMOKE-01] alice created: session={:?}, runtime_id={}",
        alice_binding.session_id, alice_binding.agent_runtime_id
    );

    // --- Send via identity-first API ---
    // Prompt asks the LLM to compute 7*6 and say SMOKE_OK.
    // We verify "42" appears in the ASSISTANT response (not in the prompt).
    eprintln!("[SMOKE-01] Sending message via identity_runtime.send()...");
    let content = meerkat_core::ContentInput::Text(
        "What is 7 multiplied by 6? State the number and say SMOKE_OK.".to_string(),
    );
    identity_rt
        .send(&id("personal:alice"), &content)
        .await
        .expect("identity send");

    // --- Wait for ASSISTANT response by polling member status ---
    // MobMemberSnapshot.output_preview is Some when the LLM has produced output.
    let mob_handle = unified.mob_handle();
    let alice_mid = roster_member_id_for_identity("personal:alice");
    let deadline = Instant::now() + Duration::from_secs(90);
    let output;
    loop {
        let member = mob_handle.member(&alice_mid).await.expect("member lookup");
        let snap = member.status().await.expect("member status");
        if let Some(ref preview) = snap.output_preview {
            output = preview.clone();
            break;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for LLM response. Member status: {snap:?}"
        );
        sleep(Duration::from_millis(500)).await;
    }
    eprintln!("[SMOKE-01] LLM output_preview: {output}");

    // Verify the LLM actually computed the answer — "42" can only come from the model
    assert!(
        output.contains("42"),
        "LLM response should contain 42, got: {output}"
    );

    let elapsed = start.elapsed();
    eprintln!(
        "[SMOKE-01] Real LLM responded in {:.1}s",
        elapsed.as_secs_f64()
    );

    // A real LLM call must take >1s
    assert!(
        elapsed.as_secs() >= 1,
        "Completed in {elapsed:?} — too fast for a real LLM call"
    );

    // --- Verify NotAddressable for InternalOnly ---
    // Register a review agent as InternalOnly and verify send() is rejected
    identity_rt
        .register(spec(
            "review:main",
            AgentAddressability::InternalOnly,
            "review",
        ))
        .await;

    let reject = identity_rt
        .send(
            &id("review:main"),
            &meerkat_core::ContentInput::Text("hi".into()),
        )
        .await;
    assert!(
        matches!(reject, Err(IdentityRuntimeError::NotAddressable(_))),
        "send to InternalOnly should be NotAddressable: {reject:?}"
    );

    eprintln!("[SMOKE-01] PASSED - identity-first → bridge → mob → real LLM ✓");
}

// ===========================================================================
// SMOKE-02: Dynamic roster change - add subscriber mid-lifecycle
//
// Boot with 1 initiative + 1 personal, verify LLM works, then reconcile
// to add a second personal agent ("personal:carol"). Verify carol fresh-creates
// while alice keeps her existing session.
// ===========================================================================

#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_smoke_02_dynamic_roster_add_subscriber() {
    if !has_api_key() {
        eprintln!("Skipping SMOKE-02: no ANTHROPIC_API_KEY");
        return;
    }

    let model = smoke_model();
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state_path = temp.path().join("state");
    let start = Instant::now();

    eprintln!("[SMOKE-02] Building UnifiedRuntime with model={model}...");

    let unified = UnifiedRuntimeBuilder::default()
        .definition(operator_definition(&model, "02"))
        .persistent_state(&state_path)
        .comms(true)
        .build()
        .await
        .expect("build UnifiedRuntime");

    let bridge: Arc<dyn SessionBridge> = unified
        .session_bridge()
        .map(|bridge| Arc::new(bridge) as Arc<dyn SessionBridge>)
        .expect("session_bridge should exist");

    let store = Arc::new(
        LocalContinuityStore::open(state_path.join("continuity.db"))
            .expect("open continuity store"),
    );
    let leases = Arc::new(LocalLeaseProvider::new());
    let identity_rt = IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: store.clone() as Arc<dyn ContinuityStore>,
        lease_provider: leases.clone(),
        runtime_instance_id: "ops-02".to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(bridge),
        default_timeout: None,
    });

    // --- Phase 1: Boot with alice only ---
    let roster_v1 = vec![spec(
        "personal:alice",
        AgentAddressability::Addressable,
        "personal",
    )];

    eprintln!("[SMOKE-02] Phase 1: restore_flow with alice only...");
    let result = restore_flow(
        &identity_rt,
        &roster_v1,
        Some(&EmptyTopology as &dyn TopologyProvider),
        Some(&NoopCustomizer as &dyn AgentCustomizer),
    )
    .await
    .expect("restore_flow v1");

    let alice_binding = match result.outcomes.get(&id("personal:alice")).unwrap() {
        RestoreOutcome::Created { binding, .. } => binding.clone(),
        other => panic!("expected Created for alice, got {other:?}"),
    };
    let alice_session_v1 = alice_binding.session_id.clone();
    eprintln!(
        "[SMOKE-02] alice created: session={:?}",
        alice_binding.session_id
    );

    // Send to alice to prove she works
    eprintln!("[SMOKE-02] Sending to alice...");
    let content =
        meerkat_core::ContentInput::Text("Say only the word ALPHA. Nothing else.".to_string());
    identity_rt
        .send(&id("personal:alice"), &content)
        .await
        .expect("send to alice");

    // Wait for alice's response
    let mob_handle = unified.mob_handle();
    let alice_mid = roster_member_id_for_identity("personal:alice");
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let member = mob_handle.member(&alice_mid).await.expect("member");
        let snap = member.status().await.expect("status");
        if snap.output_preview.is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for alice response"
        );
        sleep(Duration::from_millis(500)).await;
    }
    eprintln!("[SMOKE-02] alice responded to first message");

    // --- Phase 2: Add carol via full-roster restore_flow ---
    let roster_v2 = vec![
        spec(
            "personal:alice",
            AgentAddressability::Addressable,
            "personal",
        ),
        spec(
            "personal:carol",
            AgentAddressability::Addressable,
            "personal",
        ),
    ];

    eprintln!("[SMOKE-02] Phase 2: restore_flow with full roster (alice + carol)...");
    let result2 = restore_flow(
        &identity_rt,
        &roster_v2,
        Some(&EmptyTopology as &dyn TopologyProvider),
        Some(&NoopCustomizer as &dyn AgentCustomizer),
    )
    .await
    .expect("restore_flow v2");

    // carol should be Created (Uninitialized → fresh)
    let carol_binding = match result2.outcomes.get(&id("personal:carol")).unwrap() {
        RestoreOutcome::Created { binding, .. } => binding.clone(),
        other => panic!("expected Created for carol, got {other:?}"),
    };
    eprintln!(
        "[SMOKE-02] carol created: session={:?}",
        carol_binding.session_id
    );

    // alice should be Resumed (or Created) onto her existing session
    let alice_v2 = result2.outcomes.get(&id("personal:alice")).unwrap();
    let alice_session_v2 = match alice_v2 {
        RestoreOutcome::Created { binding, .. } | RestoreOutcome::Resumed { binding, .. } => {
            binding.session_id.clone()
        }
        other => panic!("expected Created/Resumed for alice v2, got {other:?}"),
    };

    // alice's session ID should be stable across reconciliation
    assert_eq!(
        alice_session_v1, alice_session_v2,
        "alice session should be stable across roster reconciliation"
    );

    // carol's Core binding should name a minted AgentRuntimeId
    assert!(
        !carol_binding.agent_runtime_id.as_str().is_empty(),
        "carol should have a minted AgentRuntimeId"
    );

    // Verify carol can receive messages
    eprintln!("[SMOKE-02] Sending to carol...");
    let carol_content =
        meerkat_core::ContentInput::Text("Say only the word BRAVO. Nothing else.".to_string());
    identity_rt
        .send(&id("personal:carol"), &carol_content)
        .await
        .expect("send to carol");

    let carol_mid = roster_member_id_for_identity("personal:carol");
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let member = mob_handle.member(&carol_mid).await.expect("member");
        let snap = member.status().await.expect("status");
        if snap.output_preview.is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for carol response"
        );
        sleep(Duration::from_millis(500)).await;
    }

    let elapsed = start.elapsed();
    eprintln!(
        "[SMOKE-02] PASSED in {:.1}s - roster reconciliation + carol fresh-create ✓",
        elapsed.as_secs_f64()
    );
    assert!(elapsed.as_secs() >= 1, "too fast for real LLM calls");
}

// ===========================================================================
// SMOKE-03: Respawn stuck agent without losing history
//
// Send a message, then respawn the agent. Since 0.9.4 Core wakes it onto the
// same session at a new generation: verify the session is kept, the
// generation advances, and the agent remembers the earlier turn.
// ===========================================================================

#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_smoke_03_respawn_preserves_history() {
    if !has_api_key() {
        eprintln!("Skipping SMOKE-03: no ANTHROPIC_API_KEY");
        return;
    }

    let model = smoke_model();
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state_path = temp.path().join("state");
    let start = Instant::now();

    eprintln!("[SMOKE-03] Building UnifiedRuntime with model={model}...");

    let unified = UnifiedRuntimeBuilder::default()
        .definition(operator_definition(&model, "03"))
        .persistent_state(&state_path)
        .comms(true)
        .build()
        .await
        .expect("build UnifiedRuntime");

    let bridge: Arc<dyn SessionBridge> = unified
        .session_bridge()
        .map(|bridge| Arc::new(bridge) as Arc<dyn SessionBridge>)
        .expect("session_bridge should exist");

    let store = Arc::new(
        LocalContinuityStore::open(state_path.join("continuity.db"))
            .expect("open continuity store"),
    );
    let leases = Arc::new(LocalLeaseProvider::new());
    let identity_rt = IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: store.clone() as Arc<dyn ContinuityStore>,
        lease_provider: leases.clone(),
        runtime_instance_id: "ops-03".to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(bridge),
        default_timeout: None,
    });

    let roster = vec![spec(
        "personal:alice",
        AgentAddressability::Addressable,
        "personal",
    )];

    eprintln!("[SMOKE-03] Bootstrapping alice...");
    let result = restore_flow(
        &identity_rt,
        &roster,
        Some(&EmptyTopology as &dyn TopologyProvider),
        Some(&NoopCustomizer as &dyn AgentCustomizer),
    )
    .await
    .expect("restore_flow");

    let alice_binding = match result.outcomes.get(&id("personal:alice")).unwrap() {
        RestoreOutcome::Created { binding, .. } => binding.clone(),
        other => panic!("expected Created, got {other:?}"),
    };
    let original_runtime_id = alice_binding.agent_runtime_id.clone();
    let original_session = alice_binding.session_id.clone();
    let original_generation = identity_rt
        .status(&id("personal:alice"))
        .await
        .expect("status before respawn")
        .generation
        .expect("alice has a Core binding");
    eprintln!(
        "[SMOKE-03] alice created: runtime_id={original_runtime_id}, gen={original_generation}"
    );

    // Send first message
    eprintln!("[SMOKE-03] Sending first message...");
    let content =
        meerkat_core::ContentInput::Text("What is 3+4? Reply with just the number.".to_string());
    identity_rt
        .send(&id("personal:alice"), &content)
        .await
        .expect("send");

    // Wait for response
    let mob_handle = unified.mob_handle();
    let alice_mid = roster_member_id_for_identity("personal:alice");
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let member = mob_handle.member(&alice_mid).await.expect("member");
        let snap = member.status().await.expect("status");
        if snap.output_preview.is_some() {
            break;
        }
        assert!(Instant::now() < deadline, "timed out waiting for response");
        sleep(Duration::from_millis(500)).await;
    }
    eprintln!("[SMOKE-03] Got first response, now respawning...");

    // Respawn: Core retires the member and wakes it onto the same session at
    // a new generation, so the transcript is kept and the incarnation moves.
    let respawned = identity_rt
        .respawn(&id("personal:alice"))
        .await
        .expect("respawn")
        .expect("respawn should hand back the new incarnation's scope");
    assert_eq!(
        Some(respawned.session_id()),
        original_session.as_ref(),
        "respawn should keep the session"
    );
    assert!(
        respawned.generation() > original_generation,
        "respawn should advance Core's generation: old={original_generation}, new={}",
        respawned.generation()
    );
    eprintln!(
        "[SMOKE-03] Respawn kept session={}, gen={}",
        respawned.session_id(),
        respawned.generation()
    );

    // Verify agent status is Active after respawn, at a new incarnation
    let status = identity_rt
        .status(&id("personal:alice"))
        .await
        .expect("status after respawn");
    assert_eq!(status.state, IdentityLifecycleState::Active);
    assert_ne!(
        status.agent_runtime_id.as_ref(),
        Some(&original_runtime_id),
        "respawn should run alice at a new AgentRuntimeId"
    );

    // Send post-respawn message that tests whether the agent remembers
    // the prior conversation. If session resume works, the agent should
    // know we asked about 3+4 = 7 earlier.
    eprintln!("[SMOKE-03] Sending post-respawn memory test...");
    let post_content = meerkat_core::ContentInput::Text(
        "What number did I ask you to compute in my previous message? \
         Reply with that number and the word MEMORY_OK."
            .to_string(),
    );
    identity_rt
        .send(&id("personal:alice"), &post_content)
        .await
        .expect("send after respawn");

    // Wait for a NEW response — output_preview should change to reflect
    // the second turn's answer (containing "7" and "MEMORY_OK")
    let deadline = Instant::now() + Duration::from_secs(90);
    let post_respawn_output;
    loop {
        let member = mob_handle.member(&alice_mid).await.expect("member");
        let snap = member.status().await.expect("status");
        if let Some(ref preview) = snap.output_preview {
            // Check for the memory test response — "MEMORY_OK" can only come
            // from the second turn's assistant response, not from the first
            if preview.contains("MEMORY_OK") {
                post_respawn_output = preview.clone();
                break;
            }
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for post-respawn memory test response"
        );
        sleep(Duration::from_millis(500)).await;
    }
    eprintln!("[SMOKE-03] Post-respawn response: {post_respawn_output}");

    // The LLM should reference "7" (from 3+4) — proving it has history
    assert!(
        post_respawn_output.contains('7'),
        "post-respawn response should reference 7 (from 3+4), got: {post_respawn_output}"
    );

    let elapsed = start.elapsed();
    eprintln!(
        "[SMOKE-03] PASSED in {:.1}s - respawn keeps the session + agent works",
        elapsed.as_secs_f64()
    );
    assert!(elapsed.as_secs() >= 1, "too fast for real LLM calls");
}

// ===========================================================================
// SMOKE-04: Reset agent - intentional clean slate
//
// Send a message, then reset the agent. Verify Core's generation advances and
// a new SessionId is minted.
// ===========================================================================

#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_smoke_04_reset_clean_slate() {
    if !has_api_key() {
        eprintln!("Skipping SMOKE-04: no ANTHROPIC_API_KEY");
        return;
    }

    let model = smoke_model();
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state_path = temp.path().join("state");
    let start = Instant::now();

    eprintln!("[SMOKE-04] Building UnifiedRuntime with model={model}...");

    let unified = UnifiedRuntimeBuilder::default()
        .definition(operator_definition(&model, "04"))
        .persistent_state(&state_path)
        .comms(true)
        .build()
        .await
        .expect("build UnifiedRuntime");

    let bridge: Arc<dyn SessionBridge> = unified
        .session_bridge()
        .map(|bridge| Arc::new(bridge) as Arc<dyn SessionBridge>)
        .expect("session_bridge should exist");

    let store = Arc::new(
        LocalContinuityStore::open(state_path.join("continuity.db"))
            .expect("open continuity store"),
    );
    let leases = Arc::new(LocalLeaseProvider::new());
    let identity_rt = IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: store.clone() as Arc<dyn ContinuityStore>,
        lease_provider: leases.clone(),
        runtime_instance_id: "ops-04".to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(bridge),
        default_timeout: None,
    });

    let roster = vec![spec(
        "personal:alice",
        AgentAddressability::Addressable,
        "personal",
    )];

    eprintln!("[SMOKE-04] Bootstrapping alice...");
    let result = restore_flow(
        &identity_rt,
        &roster,
        Some(&EmptyTopology as &dyn TopologyProvider),
        Some(&NoopCustomizer as &dyn AgentCustomizer),
    )
    .await
    .expect("restore_flow");

    let alice_binding = match result.outcomes.get(&id("personal:alice")).unwrap() {
        RestoreOutcome::Created { binding, .. } => binding.clone(),
        other => panic!("expected Created, got {other:?}"),
    };
    let old_session = alice_binding
        .session_id
        .clone()
        .expect("alice is bound to a session");
    let old_generation = identity_rt
        .status(&id("personal:alice"))
        .await
        .expect("status before reset")
        .generation
        .expect("alice has a Core binding");
    eprintln!("[SMOKE-04] alice created: session={old_session}, gen={old_generation}");

    // Send initial message to build history
    eprintln!("[SMOKE-04] Sending initial message...");
    let content = meerkat_core::ContentInput::Text("Remember this number: 42. Say OK.".to_string());
    identity_rt
        .send(&id("personal:alice"), &content)
        .await
        .expect("send");

    // Wait for response
    let mob_handle = unified.mob_handle();
    let alice_mid = roster_member_id_for_identity("personal:alice");
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let member = mob_handle.member(&alice_mid).await.expect("member");
        let snap = member.status().await.expect("status");
        if snap.output_preview.is_some() {
            break;
        }
        assert!(Instant::now() < deadline, "timed out waiting for response");
        sleep(Duration::from_millis(500)).await;
    }
    eprintln!("[SMOKE-04] Got response, now resetting...");

    // Reset the agent: Core mints a fresh session at a higher generation
    let reset_binding = identity_rt
        .reset(&id("personal:alice"))
        .await
        .expect("reset");

    // Verify generation advanced
    assert!(
        reset_binding.generation() > old_generation,
        "reset should advance Core's generation: old={old_generation}, new={}",
        reset_binding.generation()
    );

    // Verify new session ID
    assert_ne!(
        reset_binding.session_id(),
        Some(&old_session),
        "reset should mint new SessionId"
    );

    // Verify status reflects the reset
    let status = identity_rt
        .status(&id("personal:alice"))
        .await
        .expect("status after reset");
    assert_eq!(status.state, IdentityLifecycleState::Active);
    assert_eq!(status.generation, Some(reset_binding.generation()));
    assert_eq!(status.session_id.as_ref(), reset_binding.session_id());

    let elapsed = start.elapsed();
    eprintln!(
        "[SMOKE-04] PASSED in {:.1}s - reset: new gen={}, new session={:?}",
        elapsed.as_secs_f64(),
        reset_binding.generation(),
        reset_binding.session_id(),
    );
    assert!(elapsed.as_secs() >= 1, "too fast for real LLM calls");
}

// ===========================================================================
// SMOKE-05: Scheduled review dispatch
//
// Dispatch with a correlation id to an InternalOnly review agent and verify
// the admission is durable (the composition has a runtime store), send()
// refuses the InternalOnly agent, and both agents run with a Core runtime id.
// ===========================================================================

#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_smoke_05_async_checkpoint_durability() {
    if !has_api_key() {
        eprintln!("Skipping SMOKE-05: no ANTHROPIC_API_KEY");
        return;
    }

    let model = smoke_model();
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state_path = temp.path().join("state");
    let start = Instant::now();

    eprintln!("[SMOKE-05] Building UnifiedRuntime with model={model}...");

    let unified = UnifiedRuntimeBuilder::default()
        .definition(operator_definition(&model, "05"))
        .persistent_state(&state_path)
        .comms(true)
        .build()
        .await
        .expect("build UnifiedRuntime");

    let bridge: Arc<dyn SessionBridge> = unified
        .session_bridge()
        .map(|bridge| Arc::new(bridge) as Arc<dyn SessionBridge>)
        .expect("session_bridge should exist");

    let store = Arc::new(
        LocalContinuityStore::open(state_path.join("continuity.db"))
            .expect("open continuity store"),
    );
    let leases = Arc::new(LocalLeaseProvider::new());
    let identity_rt = IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: store.clone() as Arc<dyn ContinuityStore>,
        lease_provider: leases.clone(),
        runtime_instance_id: "ops-05".to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(bridge),
        default_timeout: None,
    });

    // Roster: 1 review + 1 personal
    let roster = vec![
        spec("review:main", AgentAddressability::InternalOnly, "review"),
        spec(
            "personal:alice",
            AgentAddressability::Addressable,
            "personal",
        ),
    ];

    eprintln!("[SMOKE-05] Bootstrapping roster...");
    let result = restore_flow(
        &identity_rt,
        &roster,
        Some(&EmptyTopology as &dyn TopologyProvider),
        Some(&NoopCustomizer as &dyn AgentCustomizer),
    )
    .await
    .expect("restore_flow");

    for identity in ["review:main", "personal:alice"] {
        match result.outcomes.get(&id(identity)).unwrap() {
            RestoreOutcome::Created { .. } => {}
            other => panic!("expected Created for {identity}, got {other:?}"),
        }
    }

    // Dispatch to review agent (InternalOnly) as one scheduled occurrence: a
    // dedup pair of idempotency key and correlation id (a half-pair is
    // refused, and a Scheduler-lane correlation is a canonical UUID).
    eprintln!("[SMOKE-05] Dispatching to review:main with correlation_id...");
    let dispatch_input = DispatchInput {
        content: meerkat_core::ContentInput::Text(
            "Run weekly review. Say REVIEW_DONE.".to_string(),
        ),
        origin: DispatchOrigin::Scheduler,
        correlation_id: Some(CorrelationId::new(
            uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, b"review-2026-W13").to_string(),
        )),
        idempotency_key: Some(DispatchIdempotencyKey::new("review-2026-W13")),
    };
    let (token, is_durable) = identity_rt
        .dispatch(&id("review:main"), &dispatch_input)
        .await
        .expect("dispatch to review");

    assert!(
        is_durable,
        "dispatch should be durable with has_runtime_store=true"
    );
    eprintln!("[SMOKE-05] Dispatch succeeded: fencing_token={token}, durable={is_durable}");

    // Wait for review to process
    let mob_handle = unified.mob_handle();
    let review_mid = roster_member_id_for_identity("review:main");
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let member = mob_handle.member(&review_mid).await.expect("member");
        let snap = member.status().await.expect("status");
        if snap.output_preview.is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for review response"
        );
        sleep(Duration::from_millis(500)).await;
    }
    eprintln!("[SMOKE-05] Review agent responded");

    // Verify send() to InternalOnly is rejected
    let reject = identity_rt
        .send(
            &id("review:main"),
            &meerkat_core::ContentInput::Text("hi".into()),
        )
        .await;
    assert!(
        matches!(reject, Err(IdentityRuntimeError::NotAddressable(_))),
        "send to InternalOnly should be NotAddressable: {reject:?}"
    );

    // Send to alice (Addressable)
    eprintln!("[SMOKE-05] Sending to alice...");
    identity_rt
        .send(
            &id("personal:alice"),
            &meerkat_core::ContentInput::Text("Say CHECKPOINT_OK.".to_string()),
        )
        .await
        .expect("send to alice");

    let alice_mid = roster_member_id_for_identity("personal:alice");
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let member = mob_handle.member(&alice_mid).await.expect("member");
        let snap = member.status().await.expect("status");
        if snap.output_preview.is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for alice response"
        );
        sleep(Duration::from_millis(500)).await;
    }

    // Verify status for both agents shows Active with runtime IDs
    let review_status = identity_rt
        .status(&id("review:main"))
        .await
        .expect("review status");
    assert_eq!(review_status.state, IdentityLifecycleState::Active);
    assert!(review_status.agent_runtime_id.is_some());

    let alice_status = identity_rt
        .status(&id("personal:alice"))
        .await
        .expect("alice status");
    assert_eq!(alice_status.state, IdentityLifecycleState::Active);
    assert!(alice_status.agent_runtime_id.is_some());

    let elapsed = start.elapsed();
    eprintln!(
        "[SMOKE-05] PASSED in {:.1}s - dispatch with correlation_id + durable admission",
        elapsed.as_secs_f64(),
    );
    assert!(elapsed.as_secs() >= 1, "too fast for real LLM calls");
}

// ===========================================================================
// SMOKE-07: Delete a subscriber's continuity
//
// Create 2 personal agents and delete one. Since 0.9.4 a delete starts the
// identity's next life at once on a fresh session (Core's last binding) and
// keeps it registered and live; the old session is never resumed. Re-run
// restore_flow: bob runs on the fresh session, alice keeps hers.
// ===========================================================================

#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_smoke_07_delete_and_recreate() {
    if !has_api_key() {
        eprintln!("Skipping SMOKE-07: no ANTHROPIC_API_KEY");
        return;
    }

    let model = smoke_model();
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state_path = temp.path().join("state");
    let start = Instant::now();

    eprintln!("[SMOKE-07] Building UnifiedRuntime with model={model}...");

    let unified = UnifiedRuntimeBuilder::default()
        .definition(operator_definition(&model, "07"))
        .persistent_state(&state_path)
        .comms(true)
        .build()
        .await
        .expect("build UnifiedRuntime");

    let bridge: Arc<dyn SessionBridge> = unified
        .session_bridge()
        .map(|bridge| Arc::new(bridge) as Arc<dyn SessionBridge>)
        .expect("session_bridge should exist");

    let store = Arc::new(
        LocalContinuityStore::open(state_path.join("continuity.db"))
            .expect("open continuity store"),
    );
    let leases = Arc::new(LocalLeaseProvider::new());
    let identity_rt = IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: store.clone() as Arc<dyn ContinuityStore>,
        lease_provider: leases.clone(),
        runtime_instance_id: "ops-07".to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(bridge),
        default_timeout: None,
    });

    let roster = vec![
        spec(
            "personal:alice",
            AgentAddressability::Addressable,
            "personal",
        ),
        spec("personal:bob", AgentAddressability::Addressable, "personal"),
    ];

    eprintln!("[SMOKE-07] Bootstrapping alice + bob...");
    let result = restore_flow(
        &identity_rt,
        &roster,
        Some(&EmptyTopology as &dyn TopologyProvider),
        Some(&NoopCustomizer as &dyn AgentCustomizer),
    )
    .await
    .expect("restore_flow");

    let alice_binding = match result.outcomes.get(&id("personal:alice")).unwrap() {
        RestoreOutcome::Created { binding, .. } => binding.clone(),
        other => panic!("expected Created for alice, got {other:?}"),
    };
    let bob_binding = match result.outcomes.get(&id("personal:bob")).unwrap() {
        RestoreOutcome::Created { binding, .. } => binding.clone(),
        other => panic!("expected Created for bob, got {other:?}"),
    };
    eprintln!(
        "[SMOKE-07] alice={}, bob={}",
        alice_binding.agent_runtime_id, bob_binding.agent_runtime_id
    );

    // Send to both to build some history
    eprintln!("[SMOKE-07] Sending to both agents...");
    identity_rt
        .send(
            &id("personal:alice"),
            &meerkat_core::ContentInput::Text("Say ALICE_OK.".into()),
        )
        .await
        .expect("send alice");
    identity_rt
        .send(
            &id("personal:bob"),
            &meerkat_core::ContentInput::Text("Say BOB_OK.".into()),
        )
        .await
        .expect("send bob");

    // Wait for alice to respond
    let mob_handle = unified.mob_handle();
    let alice_mid = roster_member_id_for_identity("personal:alice");
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let member = mob_handle.member(&alice_mid).await.expect("member");
        let snap = member.status().await.expect("status");
        if snap.output_preview.is_some() {
            break;
        }
        assert!(Instant::now() < deadline, "timed out waiting for alice");
        sleep(Duration::from_millis(500)).await;
    }
    eprintln!("[SMOKE-07] alice responded");

    // Delete bob: his next life starts at once on a fresh session
    eprintln!("[SMOKE-07] Deleting bob...");
    let fresh = identity_rt
        .delete_identity(&id("personal:bob"))
        .await
        .expect("delete bob")
        .expect("Core spawned bob, so his continuity is deleted");
    assert_ne!(
        fresh.session_id(),
        bob_binding.session_id.as_ref(),
        "delete should start bob on a fresh session"
    );

    // A delete keeps liveness, as a reset does: bob stays registered
    assert!(
        identity_rt.contains(&id("personal:bob")).await,
        "bob should stay registered after delete"
    );

    // alice should be unaffected
    let alice_status = identity_rt
        .status(&id("personal:alice"))
        .await
        .expect("alice status");
    assert_eq!(alice_status.state, IdentityLifecycleState::Active);

    // Re-bootstrap with the full roster: bob runs on the session his delete
    // started, and alice keeps hers.
    eprintln!("[SMOKE-07] Re-bootstrapping with full roster...");
    let result2 = restore_flow(
        &identity_rt,
        &roster,
        Some(&EmptyTopology as &dyn TopologyProvider),
        Some(&NoopCustomizer as &dyn AgentCustomizer),
    )
    .await
    .expect("restore_flow 2");

    let bob_new = match result2.outcomes.get(&id("personal:bob")).unwrap() {
        RestoreOutcome::Created { binding, .. } | RestoreOutcome::Resumed { binding, .. } => {
            binding.clone()
        }
        other => panic!("expected bob to run after his delete, got {other:?}"),
    };
    assert_eq!(
        bob_new.session_id.as_ref(),
        fresh.session_id(),
        "bob should run on the session his delete started"
    );
    assert_ne!(
        bob_new.session_id, bob_binding.session_id,
        "bob should never resume his deleted session"
    );
    eprintln!(
        "[SMOKE-07] bob after delete: old_session={:?}, new_session={:?}, rt={}",
        bob_binding.session_id, bob_new.session_id, bob_new.agent_runtime_id,
    );

    // alice should still be stable
    let alice_v2 = result2.outcomes.get(&id("personal:alice")).unwrap();
    let alice_v2_session = match alice_v2 {
        RestoreOutcome::Created { binding, .. } | RestoreOutcome::Resumed { binding, .. } => {
            binding.session_id.clone()
        }
        other => panic!("unexpected alice outcome: {other:?}"),
    };
    assert_eq!(
        alice_v2_session, alice_binding.session_id,
        "alice session should survive bob deletion"
    );

    let elapsed = start.elapsed();
    eprintln!(
        "[SMOKE-07] PASSED in {:.1}s - delete starts a fresh session, alice unaffected",
        elapsed.as_secs_f64(),
    );
    assert!(elapsed.as_secs() >= 1, "too fast for real LLM calls");
}

// ===========================================================================
// SMOKE-08: Customizer builds prompts from topology; roster changes reach live members as edges
//
// TopologyProvider wires alpha↔alice and alpha↔bob. Customizer appends
// subscriber names to alpha's system prompt. Verify the prompt content
// matches the topology edges.
// ===========================================================================

/// Topology provider that wires specific edges for SMOKE-08.
struct OperatorTopology {
    edges: Vec<(String, String)>,
}

#[async_trait]
impl TopologyProvider for OperatorTopology {
    async fn compute_edges(
        &self,
        _target_identities: &[AgentIdentity],
        _context: &TopologyContext,
    ) -> Result<Vec<ManagedPeerEdge>, TopologyError> {
        let mut edges = Vec::new();
        for (a, b) in &self.edges {
            edges.push(
                ManagedPeerEdge::new(id(a), id(b))
                    .map_err(|e| TopologyError::InvalidEdge(format!("{e}")))?,
            );
        }
        Ok(edges)
    }
}

/// Customizer that injects subscriber names into the system prompt based
/// on topology edges.
struct TopologyAwareCustomizer;

#[async_trait]
impl AgentCustomizer for TopologyAwareCustomizer {
    async fn customize_build(
        &self,
        context: &AgentBuildContext,
        _spec: &DurableAgentSpec,
        draft: &mut AgentBuildDraft,
    ) -> Result<(), CustomizerError> {
        // Find peer names from managed edges
        let my_id = &context.identity;
        let mut peers = Vec::new();
        for edge in &context.managed_edges {
            if edge.a() == my_id {
                peers.push(edge.b().to_string());
            } else if edge.b() == my_id {
                peers.push(edge.a().to_string());
            }
        }

        if !peers.is_empty() {
            peers.sort();
            let prompt = format!("Your subscribers are: {}", peers.join(", "));
            draft.system_prompt = Some(prompt);
        }

        Ok(())
    }

    async fn after_create(
        &self,
        _identity: &AgentIdentity,
        _session_id: &meerkat_core::types::SessionId,
        _context: &SessionCreatedContext,
    ) -> Result<(), CustomizerError> {
        Ok(())
    }
}

#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_smoke_08_customizer_topology_prompts() {
    if !has_api_key() {
        eprintln!("Skipping SMOKE-08: no ANTHROPIC_API_KEY");
        return;
    }

    let model = smoke_model();
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state_path = temp.path().join("state");
    let start = Instant::now();

    eprintln!("[SMOKE-08] Building UnifiedRuntime with model={model}...");

    let unified = UnifiedRuntimeBuilder::default()
        .definition(operator_definition(&model, "08"))
        .persistent_state(&state_path)
        .comms(true)
        .build()
        .await
        .expect("build UnifiedRuntime");

    let bridge: Arc<dyn SessionBridge> = unified
        .session_bridge()
        .map(|bridge| Arc::new(bridge) as Arc<dyn SessionBridge>)
        .expect("session_bridge should exist");

    let store = Arc::new(
        LocalContinuityStore::open(state_path.join("continuity.db"))
            .expect("open continuity store"),
    );
    let leases = Arc::new(LocalLeaseProvider::new());
    let identity_rt = IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: store.clone() as Arc<dyn ContinuityStore>,
        lease_provider: leases.clone(),
        runtime_instance_id: "ops-08".to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(bridge),
        default_timeout: None,
    });

    // Topology: alpha↔alice, alpha↔bob, beta↔carol
    let topology = OperatorTopology {
        edges: vec![
            ("review:alpha".into(), "personal:alice".into()),
            ("review:alpha".into(), "personal:bob".into()),
            ("review:beta".into(), "personal:carol".into()),
        ],
    };

    let roster = vec![
        spec("review:alpha", AgentAddressability::InternalOnly, "review"),
        spec("review:beta", AgentAddressability::InternalOnly, "review"),
        spec(
            "personal:alice",
            AgentAddressability::Addressable,
            "personal",
        ),
        spec("personal:bob", AgentAddressability::Addressable, "personal"),
        spec(
            "personal:carol",
            AgentAddressability::Addressable,
            "personal",
        ),
    ];

    eprintln!("[SMOKE-08] Phase 1: Bootstrap with topology...");
    let result = restore_flow(
        &identity_rt,
        &roster,
        Some(&topology as &dyn TopologyProvider),
        Some(&TopologyAwareCustomizer as &dyn AgentCustomizer),
    )
    .await
    .expect("restore_flow");

    // Verify alpha's draft has alice and bob in system_prompt
    let alpha_draft = match result.outcomes.get(&id("review:alpha")).unwrap() {
        RestoreOutcome::Created { draft, .. } => draft.clone(),
        other => panic!("expected Created for alpha, got {other:?}"),
    };
    let alpha_prompt = alpha_draft
        .system_prompt
        .expect("alpha should have system_prompt");
    assert!(
        alpha_prompt.contains("personal:alice"),
        "alpha prompt should mention alice: {alpha_prompt}"
    );
    assert!(
        alpha_prompt.contains("personal:bob"),
        "alpha prompt should mention bob: {alpha_prompt}"
    );
    assert!(
        !alpha_prompt.contains("personal:carol"),
        "alpha prompt should NOT mention carol: {alpha_prompt}"
    );
    eprintln!("[SMOKE-08] alpha prompt: {alpha_prompt}");

    // Verify beta's draft has carol but not alice/bob
    let beta_draft = match result.outcomes.get(&id("review:beta")).unwrap() {
        RestoreOutcome::Created { draft, .. } => draft.clone(),
        other => panic!("expected Created for beta, got {other:?}"),
    };
    let beta_prompt = beta_draft
        .system_prompt
        .expect("beta should have system_prompt");
    assert!(
        beta_prompt.contains("personal:carol"),
        "beta prompt should mention carol: {beta_prompt}"
    );
    assert!(
        !beta_prompt.contains("personal:alice"),
        "beta prompt should NOT mention alice: {beta_prompt}"
    );
    assert!(
        !beta_prompt.contains("personal:bob"),
        "beta prompt should NOT mention bob: {beta_prompt}"
    );
    eprintln!("[SMOKE-08] beta prompt: {beta_prompt}");

    // Verify managed_edges in result
    assert_eq!(
        result.managed_edges.len(),
        3,
        "should have 3 topology edges"
    );

    // Phase 2: Add dave wired to alpha, re-run restore_flow with full roster
    eprintln!("[SMOKE-08] Phase 2: Adding dave wired to alpha...");
    let topology_v2 = OperatorTopology {
        edges: vec![
            ("review:alpha".into(), "personal:alice".into()),
            ("review:alpha".into(), "personal:bob".into()),
            ("review:alpha".into(), "personal:dave".into()),
            ("review:beta".into(), "personal:carol".into()),
        ],
    };

    let roster_v2 = vec![
        spec("review:alpha", AgentAddressability::InternalOnly, "review"),
        spec("review:beta", AgentAddressability::InternalOnly, "review"),
        spec(
            "personal:alice",
            AgentAddressability::Addressable,
            "personal",
        ),
        spec("personal:bob", AgentAddressability::Addressable, "personal"),
        spec(
            "personal:carol",
            AgentAddressability::Addressable,
            "personal",
        ),
        spec(
            "personal:dave",
            AgentAddressability::Addressable,
            "personal",
        ),
    ];

    let result2 = restore_flow(
        &identity_rt,
        &roster_v2,
        Some(&topology_v2 as &dyn TopologyProvider),
        Some(&TopologyAwareCustomizer as &dyn AgentCustomizer),
    )
    .await
    .expect("restore_flow v2");

    // dave should be Created (fresh)
    let dave_outcome = result2.outcomes.get(&id("personal:dave")).unwrap();
    match dave_outcome {
        RestoreOutcome::Created { .. } => {
            eprintln!("[SMOKE-08] dave fresh-created");
        }
        other => panic!("expected Created for dave, got {other:?}"),
    }

    // alpha and beta are live, so restore_flow resumes them on the sessions
    // (and prompts) they were built with: the customizer runs at build time
    // only. The new topology shows in the restore's managed edges.
    for name in ["review:alpha", "review:beta"] {
        assert!(
            matches!(
                result2.outcomes.get(&id(name)),
                Some(RestoreOutcome::Resumed { .. })
            ),
            "{name} should resume: {:?}",
            result2.outcomes.get(&id(name))
        );
    }
    let linked = |a: &str, b: &str| {
        result2.managed_edges.iter().any(|edge| {
            let (x, y) = (edge.a().as_str(), edge.b().as_str());
            (x == a && y == b) || (x == b && y == a)
        })
    };
    assert!(
        linked("review:alpha", "personal:dave"),
        "the topology should link alpha and dave: {:?}",
        result2.managed_edges
    );
    assert!(
        linked("review:beta", "personal:carol") && !linked("review:beta", "personal:dave"),
        "the topology should still link beta to carol only: {:?}",
        result2.managed_edges
    );
    eprintln!("[SMOKE-08] topology v2: {:?}", result2.managed_edges);

    // Verify total managed edges is now 4
    assert_eq!(
        result2.managed_edges.len(),
        4,
        "should have 4 topology edges"
    );

    let elapsed = start.elapsed();
    eprintln!(
        "[SMOKE-08] PASSED in {:.1}s - customizer prompts at build + topology edges ✓",
        elapsed.as_secs_f64(),
    );
}

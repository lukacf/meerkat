//! Deterministic guard for identity-first respawn transcript continuity.
//!
//! `IdentityRuntime::respawn` is a *recovery boundary*: it re-fences the live
//! member and keeps its SessionId (`refresh_existing_session_runtime_state`),
//! so the conversation must survive across a respawn. The live operator smoke test
//! proves this via a model's verbal recall but is feature-gated and never runs
//! in CI. This scripted version proves the same invariant every build: a
//! `CaptureClient` records the exact messages sent to the LLM. We deliver turn 1
//! carrying a unique token, respawn the identity, deliver turn 2, and assert
//! turn 2's request REPLAYS turn 1's transcript. If respawn had torn the member
//! down and started fresh, the token would be gone — exactly the "respawn
//! forgets the conversation" regression (the identity-first analogue of the
//! reported downstream-app behavior).
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use meerkat_client::{LlmError, LlmEvent, LlmRequest};
use meerkat_core::types::StopReason;
use meerkat_meta::UnifiedRuntimeBuilder;
use meerkat_meta::identity_first::contracts::{AgentCustomizer, TopologyProvider};
use meerkat_meta::identity_first::orchestrator::{RestoreOutcome, restore_flow};
use meerkat_meta::identity_first::{
    AgentAddressability, AgentBuildContext, AgentBuildDraft, AgentIdentity, ContinuityStore,
    CustomizerError, DurabilityPolicy, DurableAgentSpec, IdentityRuntime, IdentityRuntimeConfig,
    LocalContinuityStore, LocalLeaseProvider, ManagedPeerEdge, SessionBridge, TopologyContext,
    TopologyError,
};
use meerkat_meta::mob_handle_runtime::SessionCreatedContext;
use meerkat_mob::{MobDefinition, ProfileName};
use tokio::time::sleep;

/// The one definition of the normalized-provider-accounting contract every
/// Meta LLM double must satisfy under meerkat 0.8.22. See the module docs.
#[path = "support/llm_usage.rs"]
mod llm_usage;

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

const MOB_TOML: &str = r#"
[mob]
id = "respawn-continuity"

[profiles.personal]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.personal.tools]
comms = true
"#;

fn one_member_definition() -> MobDefinition {
    MobDefinition::from_toml(MOB_TOML).expect("parse respawn-continuity mob definition")
}

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

/// Records the serialized LLM request each turn and answers "ok" so turns
/// complete without a real provider.
#[derive(Clone, Default)]
struct CaptureClient {
    requests: Arc<std::sync::Mutex<Vec<String>>>,
}
impl CaptureClient {
    fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
    fn last(&self) -> Option<String> {
        self.requests.lock().unwrap().last().cloned()
    }
}
impl meerkat_client::LlmClient for CaptureClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }
    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        self.requests
            .lock()
            .unwrap()
            .push(serde_json::to_string(request).unwrap_or_default());
        // meerkat 0.8.22 rejects a turn whose stream carried no normalized
        // provider accounting, so the terminal `Done` never travels alone.
        let [usage, done] =
            llm_usage::usage_then_done(request, meerkat::Provider::OpenAI, StopReason::EndTurn);
        Box::pin(async_stream::stream! {
            yield Ok(LlmEvent::TextDelta { delta: "ok".to_string(), meta: None });
            yield Ok(usage);
            yield Ok(done);
        })
    }
    fn provider(&self) -> meerkat::Provider {
        meerkat::Provider::OpenAI
    }
    fn health_check<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn identity_first_respawn_preserves_transcript() {
    let capture = CaptureClient::default();
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state_path = temp.path().join("state");

    let unified = UnifiedRuntimeBuilder::default()
        .definition(one_member_definition())
        .persistent_state(&state_path)
        .comms(true)
        .default_llm_client(Arc::new(capture.clone()))
        .build()
        .await
        .expect("build UnifiedRuntime");

    let bridge: Arc<dyn SessionBridge> = unified
        .session_bridge()
        .map(|bridge| Arc::new(bridge) as Arc<dyn SessionBridge>)
        .expect("session_bridge should exist");
    let store = Arc::new(
        LocalContinuityStore::open(state_path.join("continuity.db")).expect("continuity store"),
    );
    let identity_rt = IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: store.clone() as Arc<dyn ContinuityStore>,
        lease_provider: Arc::new(LocalLeaseProvider::new()),
        runtime_instance_id: "respawn-continuity".to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(bridge),
        default_timeout: None,
    });

    let alice = id("personal:alice");
    let roster = vec![spec(
        "personal:alice",
        AgentAddressability::Addressable,
        "personal",
    )];
    let result = restore_flow(
        &identity_rt,
        &roster,
        Some(&EmptyTopology as &dyn TopologyProvider),
        Some(&NoopCustomizer as &dyn AgentCustomizer),
    )
    .await
    .expect("restore_flow");
    match result.outcomes.get(&alice).expect("alice outcome") {
        RestoreOutcome::Created { .. } => {}
        other => panic!("expected Created, got {other:?}"),
    }

    // --- Turn 1: deliver a unique token; wait for the turn to complete ---
    const TOKEN: &str = "MARKER-ALPHA-7-ZEBRA";
    identity_rt
        .send(
            &alice,
            &meerkat_core::ContentInput::Text(format!("Please note this token: {TOKEN}")),
        )
        .await
        .expect("send turn 1");

    let deadline = Instant::now() + Duration::from_secs(20);
    while capture.count() < 1 {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for turn 1 to reach the LLM (capture.count={})",
            capture.count()
        );
        sleep(Duration::from_millis(100)).await;
    }
    // Let turn 1's assistant response commit to the transcript before respawn.
    sleep(Duration::from_millis(500)).await;
    let captures_after_turn1 = capture.count();

    // --- Respawn (recovery boundary: SessionId does not rotate) ---
    identity_rt.respawn(&alice).await.expect("respawn");

    // --- Turn 2: after respawn, refer back to the earlier token ---
    identity_rt
        .send(
            &alice,
            &meerkat_core::ContentInput::Text("What token did I give you earlier?".to_string()),
        )
        .await
        .expect("send turn 2");

    let deadline = Instant::now() + Duration::from_secs(30);
    while capture.count() <= captures_after_turn1 {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the post-respawn turn to reach the LLM"
        );
        sleep(Duration::from_millis(100)).await;
    }

    // The post-respawn request must REPLAY turn 1's transcript — the token can
    // only be present if the respawned member kept/resumed the conversation.
    let last_request = capture.last().expect("a post-respawn request was captured");
    assert!(
        last_request.contains(TOKEN),
        "post-respawn LLM request must replay the prior transcript (token {TOKEN}); \
         respawn dropped the conversation history"
    );

    unified.shutdown().await;
}

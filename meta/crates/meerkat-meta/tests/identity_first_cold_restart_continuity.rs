//! Upgrade-carry regression (Bug 1, a 0.7.20 field report): a full process
//! restart against the same on-disk store must RESUME each agent's transcript,
//! not fresh-spawn it empty.
//!
//! On cold restart the identity runtime re-creates the member and calls
//! `bridge.resume_session` (`MemberLaunchMode::Resume`). Before meerkat 0.7.14,
//! the re-created runtime authority re-projected a transcript that only differed
//! from the persisted row in bookkeeping (re-stamped run identity + timestamps),
//! and the session-store append-only continuity guard rejected it as "not a
//! continuation of persisted revision" — so `resume_session` fell into its
//! `Err` arm and spawned a fresh, EMPTY member, silently dropping history on
//! every restart. Every real deployment hit this; fresh-state tests never did.
//!
//! meerkat 0.7.14 (Ask B) makes the guard compare the shared prefix by content
//! address, tolerating bookkeeping-only divergence, so resume succeeds. This
//! test proves it end to end: boot, deliver a unique token, FULLY shut down,
//! rebuild a fresh runtime against the SAME state dir, and assert the
//! post-restart turn replays the token — impossible unless the transcript
//! carried across the restart. The sibling respawn-continuity test covers the
//! in-process recovery boundary; this one covers the cold restart.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
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

/// Per-test mob id: unique ACROSS tests (0.8.23's fail-closed in-proc
/// registration - concurrent tests must not share a supervisor route) but
/// stable WITHIN a test, whose restarts re-bootstrap the same mob.
fn unique_cold_restart_mob_id() -> String {
    static NEXT_TEST_MOB_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "cold-restart-continuity-{}",
        NEXT_TEST_MOB_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

const MOB_TOML: &str = r#"
[mob]
id = "MOB_ID_PLACEHOLDER"

[profiles.personal]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.personal.tools]
comms = true
"#;

fn one_member_definition(mob_id: &str) -> MobDefinition {
    MobDefinition::from_toml(&MOB_TOML.replace("MOB_ID_PLACEHOLDER", mob_id))
        .expect("parse cold-restart mob definition")
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

/// Injects a per-boot "host context" instruction — drifting system-prompt
/// parts are what made field deployments commit resume refresh rewrites on
/// every idle boot (the chain meerkat #837 fixed). Kept here so the idle
/// cycles below at least present drift; see the test's doc for what this
/// harness can and cannot rebuild.
struct DriftingContextCustomizer {
    tag: std::sync::Mutex<String>,
}
#[async_trait]
impl AgentCustomizer for DriftingContextCustomizer {
    async fn customize_build(
        &self,
        _context: &AgentBuildContext,
        _spec: &DurableAgentSpec,
        draft: &mut AgentBuildDraft,
    ) -> Result<(), CustomizerError> {
        let tag = self.tag.lock().unwrap().clone();
        draft.additional_instructions = vec![format!("Host context: {tag}")];
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
/// complete without a real provider. A [`CaptureClient::gated`] client holds
/// every call until its gate opens.
#[derive(Clone, Default)]
struct CaptureClient {
    requests: Arc<std::sync::Mutex<Vec<String>>>,
    gate: Option<tokio::sync::watch::Receiver<bool>>,
    /// Calls holding at the gate right now.
    held: Arc<AtomicUsize>,
    /// Calls that answered.
    answered: Arc<AtomicUsize>,
    /// Tool rounds still to answer: while above zero, a call that offers
    /// tools answers one call of its first tool, without holding.
    tool_rounds: Arc<AtomicUsize>,
}
impl CaptureClient {
    /// A client whose every call holds until `gate` reads true.
    fn gated(gate: tokio::sync::watch::Receiver<bool>) -> Self {
        Self {
            gate: Some(gate),
            ..Self::default()
        }
    }
    /// This client, answering its first `rounds` calls that offer tools with
    /// a tool call instead of text.
    fn with_tool_rounds(mut self, rounds: usize) -> Self {
        self.tool_rounds = Arc::new(AtomicUsize::new(rounds));
        self
    }
    fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
    fn last(&self) -> Option<String> {
        self.requests.lock().unwrap().last().cloned()
    }
    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
    fn held(&self) -> usize {
        self.held.load(Ordering::SeqCst)
    }
    fn answered(&self) -> usize {
        self.answered.load(Ordering::SeqCst)
    }
}
/// Takes a call out of [`CaptureClient::held`] when it leaves the gate,
/// answered or dropped.
struct HeldCall(Arc<AtomicUsize>);
impl Drop for HeldCall {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
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
        if let Some(tool) = request.tools.first()
            && self
                .tool_rounds
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                    left.checked_sub(1)
                })
                .is_ok()
        {
            let [usage, done] =
                llm_usage::usage_then_done(request, meerkat::Provider::OpenAI, StopReason::ToolUse);
            let call = LlmEvent::ToolCallComplete {
                id: KEPT_ROUND_CALL_ID.to_string(),
                name: tool.name.to_string(),
                args: serde_json::json!({}),
                meta: None,
            };
            return Box::pin(futures::stream::iter([Ok(call), Ok(usage), Ok(done)]));
        }
        let mut gate = self.gate.clone();
        let held = Arc::clone(&self.held);
        let answered = Arc::clone(&self.answered);
        Box::pin(async_stream::stream! {
            if let Some(gate) = gate.as_mut() {
                held.fetch_add(1, Ordering::SeqCst);
                let holding = HeldCall(Arc::clone(&held));
                let _ = gate.wait_for(|open| *open).await;
                drop(holding);
            }
            answered.fetch_add(1, Ordering::SeqCst);
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

const RUNTIME_INSTANCE: &str = "cold-restart-continuity";

/// The id of the tool call a [`CaptureClient::with_tool_rounds`] client
/// answers.
const KEPT_ROUND_CALL_ID: &str = "kept-round-call";

/// Build a fresh runtime + identity runtime against `state_path`, capturing LLM
/// requests into `capture`. Mirrors a process (re)start against a durable store.
async fn boot(
    state_path: &std::path::Path,
    mob_id: &str,
    capture: CaptureClient,
) -> (meerkat_meta::UnifiedRuntime, IdentityRuntime) {
    let unified = UnifiedRuntimeBuilder::default()
        .definition(one_member_definition(mob_id))
        .persistent_state(state_path)
        .comms(true)
        .default_llm_client(Arc::new(capture))
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
        continuity_store: store as Arc<dyn ContinuityStore>,
        lease_provider: Arc::new(LocalLeaseProvider::new()),
        runtime_instance_id: RUNTIME_INSTANCE.to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(bridge),
        default_timeout: None,
    });
    (unified, identity_rt)
}

// History: this test was first landed `#[ignore]`d as "harness can't reach
// resume" — boot 2 appeared to re-Create. That diagnosis was wrong: the bridge
// resumed fine, but the orchestrator reported the outcome by checkpoint-
// snapshot presence and labeled a genuine resume `Created` — the exact
// outcome-reporting lie from the field report. With outcomes keyed on the
// bridge verdict (and resume inheriting the persisted System message), the
// full cold-restart path passes deterministically and this is now a live
// regression guard for both bugs.
#[tokio::test(flavor = "multi_thread")]
async fn identity_first_cold_restart_preserves_transcript() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state_path = temp.path().join("state");
    let mob_id = unique_cold_restart_mob_id();
    let alice = id("personal:alice");
    let roster = vec![spec(
        "personal:alice",
        AgentAddressability::Addressable,
        "personal",
    )];
    const TOKEN: &str = "MARKER-BRAVO-9-YANKEE";

    // --- Boot 1: create the member, deliver turn 1 with the token, shut down ---
    let original_session_id;
    {
        let capture = CaptureClient::default();
        let (unified, identity_rt) = boot(&state_path, &mob_id, capture.clone()).await;

        let result = restore_flow(
            &identity_rt,
            &roster,
            Some(&EmptyTopology as &dyn TopologyProvider),
            Some(&NoopCustomizer as &dyn AgentCustomizer),
        )
        .await
        .expect("restore_flow (boot 1)");
        match result.outcomes.get(&alice).expect("alice outcome") {
            RestoreOutcome::Created { binding, .. } => {
                original_session_id = binding
                    .session_id
                    .clone()
                    .expect("Core binds the member to a session");
            }
            other => panic!("expected Created on first boot, got {other:?}"),
        }

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
                "timed out waiting for turn 1 to reach the LLM"
            );
            sleep(Duration::from_millis(100)).await;
        }
        // Let turn 1's assistant response commit to the persisted transcript
        // before the shutdown flush.
        sleep(Duration::from_millis(500)).await;
        unified.shutdown().await;
    }

    // --- Boot 2: fresh runtime, SAME store. Resume must carry the transcript ---
    {
        let capture = CaptureClient::default(); // fresh: only boot-2 requests
        let (unified, identity_rt) = boot(&state_path, &mob_id, capture.clone()).await;

        let result = restore_flow(
            &identity_rt,
            &roster,
            Some(&EmptyTopology as &dyn TopologyProvider),
            Some(&NoopCustomizer as &dyn AgentCustomizer),
        )
        .await
        .expect("restore_flow (boot 2)");
        // A cold restart with persisted history must RESUME onto the same
        // durable session — never re-Create (the empty fresh-spawn
        // regression) and never report a resume as anything else.
        match result.outcomes.get(&alice).expect("alice outcome") {
            RestoreOutcome::Resumed { binding, .. } => {
                assert_eq!(
                    binding.session_id.as_ref(),
                    Some(&original_session_id),
                    "cold restart must resume the SAME durable session"
                );
            }
            other => panic!("cold restart must report Resumed, got: {other:?}"),
        }

        identity_rt
            .send(
                &alice,
                &meerkat_core::ContentInput::Text("What token did I give you earlier?".to_string()),
            )
            .await
            .expect("send turn 2");

        let deadline = Instant::now() + Duration::from_secs(30);
        while capture.count() < 1 {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for the post-restart turn to reach the LLM"
            );
            sleep(Duration::from_millis(100)).await;
        }

        // The post-restart request must REPLAY turn 1's transcript. The token
        // can only be present if resume loaded the persisted conversation
        // instead of falling back to an empty fresh spawn (Bug 1 / Ask B).
        let last_request = capture.last().expect("a post-restart request was captured");
        assert!(
            last_request.contains(TOKEN),
            "post-restart LLM request must replay the persisted transcript (token {TOKEN}); \
             cold-restart resume dropped the conversation history"
        );

        // Let turn 2 commit before the second restart.
        sleep(Duration::from_millis(500)).await;
        unified.shutdown().await;
    }

    // --- Boot 3 (second-restart variant): the resumed-then-extended session
    // must survive ANOTHER restart. The field report hit the loss on every
    // restart; the second one exercises resume over a transcript that itself
    // grew after a resume. ---
    {
        let capture = CaptureClient::default(); // fresh: only boot-3 requests
        let (unified, identity_rt) = boot(&state_path, &mob_id, capture.clone()).await;

        let result = restore_flow(
            &identity_rt,
            &roster,
            Some(&EmptyTopology as &dyn TopologyProvider),
            Some(&NoopCustomizer as &dyn AgentCustomizer),
        )
        .await
        .expect("restore_flow (boot 3)");
        match result.outcomes.get(&alice).expect("alice outcome") {
            RestoreOutcome::Resumed { binding, .. } => {
                assert_eq!(
                    binding.session_id.as_ref(),
                    Some(&original_session_id),
                    "second restart must still resume the SAME durable session"
                );
            }
            other => panic!("second restart must report Resumed, got: {other:?}"),
        }

        identity_rt
            .send(
                &alice,
                &meerkat_core::ContentInput::Text("And once more: which token?".to_string()),
            )
            .await
            .expect("send turn 3");

        let deadline = Instant::now() + Duration::from_secs(30);
        while capture.count() < 1 {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for the second post-restart turn to reach the LLM"
            );
            sleep(Duration::from_millis(100)).await;
        }
        let last_request = capture
            .last()
            .expect("a second post-restart request was captured");
        assert!(
            last_request.contains(TOKEN),
            "the transcript must survive a SECOND restart (token {TOKEN} missing)"
        );

        unified.shutdown().await;
    }
}

/// Idle-member coverage for a 0.7.23 regression (meerkat #837): a
/// member restarted repeatedly with NO turns in between must keep resuming
/// onto the same durable session, and the eventual turn must replay history.
///
/// Field shape: turn-less boots committed chained resume-system-prompt-refresh
/// rewrites; meerkat 0.7.16/0.7.17's rewrite-chain walk miswalked the chain
/// and failed closed as a cycle, refusing resume on every boot (for nearly
/// every identity). The survivor had run a turn after its refresh - the
/// shape the sibling test above exercises, which is why it kept passing.
///
/// Honesty note: the failing chain was built by PRE-0.7.21 boots (before
/// resume inherited the persisted System message) and carried in the store;
/// current-version code does not rebuild that legacy shape, so this test does
/// NOT go red on meerkat 0.7.17 — the authoritative red/green repro lives
/// upstream (#837, seeded with real 0.7.13/0.7.14/0.7.15 binaries). What this
/// variant pins on the meta side: idle restart cycles (with prompt drift
/// presented) stay Resumed end to end — the gap in our coverage that let the
/// idle-member shape ship unexercised.
#[tokio::test(flavor = "multi_thread")]
async fn identity_first_cold_restart_turnless_resume_chain_preserves_transcript() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state_path = temp.path().join("state");
    let mob_id = unique_cold_restart_mob_id();
    let alice = id("personal:alice");
    let roster = vec![spec(
        "personal:alice",
        AgentAddressability::Addressable,
        "personal",
    )];
    const TOKEN: &str = "MARKER-IDLE-CHAIN-7-ZULU";
    let customizer = DriftingContextCustomizer {
        tag: std::sync::Mutex::new("boot-1".to_string()),
    };

    // --- Boot 1: create, one turn carrying the token, shut down ---
    let original_session_id;
    {
        let capture = CaptureClient::default();
        let (unified, identity_rt) = boot(&state_path, &mob_id, capture.clone()).await;
        let result = restore_flow(
            &identity_rt,
            &roster,
            Some(&EmptyTopology as &dyn TopologyProvider),
            Some(&customizer as &dyn AgentCustomizer),
        )
        .await
        .expect("restore_flow (boot 1)");
        match result.outcomes.get(&alice).expect("alice outcome") {
            RestoreOutcome::Created { binding, .. } => {
                original_session_id = binding
                    .session_id
                    .clone()
                    .expect("Core binds the member to a session");
            }
            other => panic!("expected Created on first boot, got {other:?}"),
        }
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
                "timed out waiting for turn 1 to reach the LLM"
            );
            sleep(Duration::from_millis(100)).await;
        }
        sleep(Duration::from_millis(500)).await;
        unified.shutdown().await;
    }

    // --- Boots 2-4: resume, NO turn, shut down. Each boot may stack another
    // turn-less refresh commit onto the history graph — the chain that
    // 0.7.16/0.7.17 miswalked. ---
    for boot_n in 2..=4 {
        *customizer.tag.lock().unwrap() = format!("boot-{boot_n}");
        let capture = CaptureClient::default();
        let (unified, identity_rt) = boot(&state_path, &mob_id, capture.clone()).await;
        let result = restore_flow(
            &identity_rt,
            &roster,
            Some(&EmptyTopology as &dyn TopologyProvider),
            Some(&customizer as &dyn AgentCustomizer),
        )
        .await
        .unwrap_or_else(|e| panic!("restore_flow (turn-less boot {boot_n}): {e}"));
        match result.outcomes.get(&alice).expect("alice outcome") {
            RestoreOutcome::Resumed { binding, .. } => {
                assert_eq!(
                    binding.session_id.as_ref(),
                    Some(&original_session_id),
                    "turn-less boot {boot_n} must resume the SAME durable session"
                );
            }
            other => panic!(
                "turn-less boot {boot_n} must report Resumed (idle members must not \
                 degrade), got: {other:?}"
            ),
        }
        // Give any resume-time refresh rewrite a moment to commit, then stop.
        sleep(Duration::from_millis(500)).await;
        unified.shutdown().await;
    }

    // --- Boot 5: resume once more and run a turn. The turn's run-boundary
    // commit is where the miswalked chain used to fail closed. ---
    {
        *customizer.tag.lock().unwrap() = "boot-5".to_string();
        let capture = CaptureClient::default();
        let (unified, identity_rt) = boot(&state_path, &mob_id, capture.clone()).await;
        let result = restore_flow(
            &identity_rt,
            &roster,
            Some(&EmptyTopology as &dyn TopologyProvider),
            Some(&customizer as &dyn AgentCustomizer),
        )
        .await
        .expect("restore_flow (boot 5)");
        match result.outcomes.get(&alice).expect("alice outcome") {
            RestoreOutcome::Resumed { binding, .. } => {
                assert_eq!(
                    binding.session_id.as_ref(),
                    Some(&original_session_id),
                    "boot 5 must still resume the SAME durable session"
                );
            }
            other => panic!("boot 5 must report Resumed, got: {other:?}"),
        }
        identity_rt
            .send(
                &alice,
                &meerkat_core::ContentInput::Text("What token did I give you earlier?".to_string()),
            )
            .await
            .expect("send the post-idle-chain turn");
        let deadline = Instant::now() + Duration::from_secs(30);
        while capture.count() < 1 {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for the post-idle-chain turn to reach the LLM"
            );
            sleep(Duration::from_millis(100)).await;
        }
        let last_request = capture
            .last()
            .expect("a post-idle-chain request was captured");
        assert!(
            last_request.contains(TOKEN),
            "after three turn-less restarts the transcript must still replay (token {TOKEN})"
        );
        unified.shutdown().await;
    }
}

/// Bug I terminal state, revived (meerkat 0.7.29, ask 31).
///
/// Forge the exact damage Bug I left behind through public APIs: a MOB-PLANE
/// retire archives the durable session and durably writes
/// `runtime_state=retired` while the continuity record still binds the
/// identity to that session — precisely what the ≤0.7.28 resume-failure
/// rollback did to affected members. On meerkat ≤0.7.28 the next resume
/// answered `missing durable session snapshot` forever (retire is terminal);
/// on ≥0.7.29 the ordinary resume path must AUTO-REVIVE the
/// retired-with-intact-snapshot session (`authorize_revivable_retired_session`)
/// with the transcript intact — no operator row surgery, no meta revive
/// call.
///
/// History: landed `#[ignore]`d against meerkat 0.7.29, whose revival flow
/// died at "generated MeerkatMachine did not grant active executor
/// registration" (executor registration staged against the still-Retired
/// machine — upstream ask 34, fixed in 0.7.30). Un-ignored as the ask 34
/// acceptance criterion.
#[tokio::test(flavor = "multi_thread")]
async fn identity_first_resume_revives_terminally_retired_runtime() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state_path = temp.path().join("state");
    let mob_id = unique_cold_restart_mob_id();
    let alice = id("personal:alice");
    let roster = vec![spec(
        "personal:alice",
        AgentAddressability::Addressable,
        "personal",
    )];
    const TOKEN: &str = "MARKER-REVIVAL-7-ZULU";

    // --- Boot 1: create, deliver turn 1, then retire on the MOB PLANE
    // (bypassing the continuity layer, exactly like Bug I's rollback). ---
    let original_session_id;
    {
        let capture = CaptureClient::default();
        let (unified, identity_rt) = boot(&state_path, &mob_id, capture.clone()).await;

        let result = restore_flow(
            &identity_rt,
            &roster,
            Some(&EmptyTopology as &dyn TopologyProvider),
            Some(&NoopCustomizer as &dyn AgentCustomizer),
        )
        .await
        .expect("restore_flow (boot 1)");
        match result.outcomes.get(&alice).expect("alice outcome") {
            RestoreOutcome::Created { binding, .. } => {
                original_session_id = binding
                    .session_id
                    .clone()
                    .expect("Core binds the member to a session");
            }
            other => panic!("expected Created on first boot, got {other:?}"),
        }

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
                "timed out waiting for turn 1 to reach the LLM"
            );
            sleep(Duration::from_millis(100)).await;
        }
        sleep(Duration::from_millis(500)).await;

        let members = unified.mob_handle().list_members().await;
        assert_eq!(members.len(), 1, "one durable member expected");
        let member_id = members[0].agent_identity.clone();
        unified
            .mob_handle()
            .retire(member_id.clone())
            .await
            .expect("mob-plane retire (forging the Bug I terminal state)");
        // Wait until the retire fully lands (member gone or final) so the
        // archived+retired pair is durable before the restart.
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let members = unified.mob_handle().list_members().await;
            let live = members
                .iter()
                .any(|entry| entry.agent_identity == member_id && !entry.is_final);
            if !live {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for the mob-plane retire to finalize"
            );
            sleep(Duration::from_millis(100)).await;
        }
        sleep(Duration::from_millis(300)).await;
        unified.shutdown().await;
    }

    // --- Boot 2: the continuity record still binds alice to the retired
    // session. The resume must revive it with the transcript intact. ---
    {
        let capture = CaptureClient::default();
        let (unified, identity_rt) = boot(&state_path, &mob_id, capture.clone()).await;

        let result = restore_flow(
            &identity_rt,
            &roster,
            Some(&EmptyTopology as &dyn TopologyProvider),
            Some(&NoopCustomizer as &dyn AgentCustomizer),
        )
        .await
        .expect("restore_flow (boot 2) — a retired-with-intact-snapshot session must revive");
        match result.outcomes.get(&alice).expect("alice outcome") {
            RestoreOutcome::Resumed { binding, .. } => {
                assert_eq!(
                    binding.session_id.as_ref(),
                    Some(&original_session_id),
                    "revival must rebind the SAME durable session, not a fresh one"
                );
            }
            other => panic!(
                "resume of a terminally-retired runtime must revive (ask 31), got: {other:?}"
            ),
        }

        identity_rt
            .send(
                &alice,
                &meerkat_core::ContentInput::Text("What token did I give you earlier?".to_string()),
            )
            .await
            .expect("send post-revival turn");
        let deadline = Instant::now() + Duration::from_secs(30);
        while capture.count() < 1 {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for the post-revival turn to reach the LLM"
            );
            sleep(Duration::from_millis(100)).await;
        }
        let last_request = capture.last().expect("a post-revival request was captured");
        assert!(
            last_request.contains(TOKEN),
            "post-revival LLM request must replay the pre-retire transcript (token {TOKEN})"
        );
        unified.shutdown().await;
    }
}

// ---------------------------------------------------------------------------
// Whole-member materialisation diff: RESUMED must equal FRESH.
//
// The class this guards has five measured instances across three trains:
// tool categories (0.8.20), tooling override (0.8.21-0.8.25), system prompt
// (a resumed member kept a stale persona; a child was shown her brother's
// records), brain_swap staging on resume, and #1079's persistence tools on CLI
// resume. Every one was "resume reads the creation-time stamp for X while
// fresh reads the current definition for X", and respawn could not cure any of
// them because the durable identity carried the stale value.
//
// A catalog-only check catches four of five and CANNOT catch the prompt one,
// so this compares the whole materialised member as the model actually
// received it: the captured `LlmRequest` is the strongest oracle available,
// because it sits downstream of every projection seam. Fields are compared by
// NAME and the difference is printed by name - a count hid the 0.8.20 defect
// and two matchers each missed a real tool within twenty minutes.
// ---------------------------------------------------------------------------

/// What the model was actually handed for one member, on one boot.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MaterialisedMember {
    model: String,
    system_prompt: Option<String>,
    tool_names: BTreeSet<String>,
}

impl MaterialisedMember {
    /// Typed extraction from the captured request JSON. Deserializing back into
    /// `LlmRequest` and walking fields means no JSON-path guessing, and a wire
    /// change upstream fails here loudly instead of silently extracting nothing.
    fn from_captured_request(json: &str) -> Self {
        let request: LlmRequest =
            serde_json::from_str(json).expect("captured request must deserialize as LlmRequest");
        let system_prompt = request.messages.iter().find_map(|message| match message {
            meerkat_core::Message::System(system) => Some(system.content.clone()),
            _ => None,
        });
        Self {
            model: request.model,
            system_prompt,
            tool_names: request
                .tools
                .iter()
                .map(|tool| tool.name.as_str().to_string())
                .collect(),
        }
    }
}

/// Field-by-field difference, one human-readable line per differing field.
/// Empty means identical. Tool names report the SET DIFFERENCE in both
/// directions, never a count.
fn materialisation_diff(fresh: &MaterialisedMember, resumed: &MaterialisedMember) -> Vec<String> {
    let mut differences = Vec::new();
    if fresh.model != resumed.model {
        differences.push(format!(
            "model: fresh={:?} resumed={:?}",
            fresh.model, resumed.model
        ));
    }
    if fresh.system_prompt != resumed.system_prompt {
        differences.push(format!(
            "system_prompt: fresh={} chars, resumed={} chars (contents differ)",
            fresh.system_prompt.as_deref().map_or(0, str::len),
            resumed.system_prompt.as_deref().map_or(0, str::len),
        ));
    }
    if fresh.tool_names != resumed.tool_names {
        let only_fresh: Vec<&String> = fresh.tool_names.difference(&resumed.tool_names).collect();
        let only_resumed: Vec<&String> = resumed.tool_names.difference(&fresh.tool_names).collect();
        differences.push(format!(
            "tool_names: only_fresh={only_fresh:?} only_resumed={only_resumed:?}"
        ));
    }
    differences
}

/// Falsifiability control for the diff itself: a check that cannot fire proves
/// nothing, so this hand-builds two members that differ in exactly two fields
/// and asserts the diff names BOTH fields and nothing else, then asserts
/// identical members produce no lines.
#[test]
fn materialisation_diff_names_exactly_the_fields_that_changed() {
    let fresh = MaterialisedMember {
        model: "gpt-5.5".to_string(),
        system_prompt: Some("persona v2".to_string()),
        tool_names: ["send", "peers", "memory_recall"]
            .into_iter()
            .map(str::to_string)
            .collect(),
    };
    let mut resumed = fresh.clone();
    resumed.tool_names.remove("memory_recall");
    resumed.system_prompt = Some("persona v1".to_string());

    let lines = materialisation_diff(&fresh, &resumed);
    assert_eq!(lines.len(), 2, "exactly two fields changed: {lines:?}");
    assert!(
        lines.iter().any(|line| line.starts_with("system_prompt:")),
        "the prompt change must be named: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("tool_names:") && line.contains("memory_recall")),
        "the missing tool must be named, not counted: {lines:?}"
    );
    assert!(
        !lines.iter().any(|line| line.starts_with("model:")),
        "an unchanged field must not be reported: {lines:?}"
    );
    assert!(
        materialisation_diff(&fresh, &fresh).is_empty(),
        "identical members must produce no lines"
    );
}

/// A cold-restarted (RESUMED) member must be materialised identically to the
/// FRESH member the same definition produced: same model, same projected
/// system prompt, same tool catalog - as the model actually received them.
#[tokio::test]
async fn identity_first_cold_restart_resumed_member_matches_fresh_materialisation() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state_path = temp.path().join("state");
    let mob_id = unique_cold_restart_mob_id();
    let alice = id("personal:alice");
    let roster = vec![spec(
        "personal:alice",
        AgentAddressability::Addressable,
        "personal",
    )];

    // --- Boot 1: FRESH materialisation ---
    let fresh = {
        let capture = CaptureClient::default();
        let (unified, identity_rt) = boot(&state_path, &mob_id, capture.clone()).await;
        let result = restore_flow(
            &identity_rt,
            &roster,
            Some(&EmptyTopology as &dyn TopologyProvider),
            Some(&NoopCustomizer as &dyn AgentCustomizer),
        )
        .await
        .expect("restore_flow (boot 1)");
        assert!(
            matches!(
                result.outcomes.get(&alice).expect("alice outcome"),
                RestoreOutcome::Created { .. }
            ),
            "first boot must CREATE, or this test is not comparing fresh against resumed"
        );
        identity_rt
            .send(
                &alice,
                &meerkat_core::ContentInput::Text("materialisation probe, boot 1".to_string()),
            )
            .await
            .expect("send boot-1 turn");
        let deadline = Instant::now() + Duration::from_secs(20);
        while capture.count() < 1 {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for the boot-1 turn to reach the LLM"
            );
            sleep(Duration::from_millis(100)).await;
        }
        let request = capture.last().expect("a boot-1 request was captured");
        sleep(Duration::from_millis(500)).await;
        unified.shutdown().await;
        MaterialisedMember::from_captured_request(&request)
    };

    // --- Boot 2: RESUMED materialisation, same store, same definition ---
    let resumed = {
        let capture = CaptureClient::default();
        let (unified, identity_rt) = boot(&state_path, &mob_id, capture.clone()).await;
        let result = restore_flow(
            &identity_rt,
            &roster,
            Some(&EmptyTopology as &dyn TopologyProvider),
            Some(&NoopCustomizer as &dyn AgentCustomizer),
        )
        .await
        .expect("restore_flow (boot 2)");
        assert!(
            matches!(
                result.outcomes.get(&alice).expect("alice outcome"),
                RestoreOutcome::Resumed { .. }
            ),
            "second boot must RESUME, or this test is not comparing fresh against resumed"
        );
        identity_rt
            .send(
                &alice,
                &meerkat_core::ContentInput::Text("materialisation probe, boot 2".to_string()),
            )
            .await
            .expect("send boot-2 turn");
        let deadline = Instant::now() + Duration::from_secs(30);
        while capture.count() < 1 {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for the boot-2 turn to reach the LLM"
            );
            sleep(Duration::from_millis(100)).await;
        }
        let request = capture.last().expect("a boot-2 request was captured");
        sleep(Duration::from_millis(500)).await;
        unified.shutdown().await;
        MaterialisedMember::from_captured_request(&request)
    };

    // The positive half: the check must be looking at something. A member with
    // no tools and no prompt would compare equal trivially.
    assert!(
        !fresh.tool_names.is_empty(),
        "the fresh member carried no tools; the comparison would be vacuous"
    );
    assert!(
        fresh.system_prompt.is_some(),
        "the fresh member carried no system prompt; the comparison would be vacuous"
    );

    let differences = materialisation_diff(&fresh, &resumed);
    assert!(
        differences.is_empty(),
        "resumed member diverged from fresh materialisation of the same definition:\n  {}",
        differences.join("\n  ")
    );
}

/// A graceful shutdown keeps a durable member's in-flight turn, and the
/// input queued behind it, for the member's next start (meerkat 0.9.5): at
/// the next boot the turn asks its model again without a new send, and the
/// queued input runs after it. Every identity-first member here is durable:
/// its session lives on the persistent session service, which commits each
/// round. `drain` is the gateway's in-flight drain window; `None` is the
/// default, no window. The turn is still held at the window's bound, so both
/// paths leave it to the shutdown, whose own stop leaves it running for the
/// terminal Shutdown to keep.
///
/// Waits on Meta's pin of a meerkat whose graceful Shutdown keeps durable
/// runs, with the resumed-run machinery of the same release. On an older
/// meerkat the Shutdown cancels the turn and stops the queued input, and the
/// boot-2 half of this row fails.
async fn a_graceful_shutdown_keeps_durable_work(drain: Option<Duration>) {
    const KEPT: &str = "KEPT-TURN-KILO-7";
    const QUEUED: &str = "QUEUED-INPUT-LIMA-3";
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state_path = temp.path().join("state");
    let mob_id = unique_cold_restart_mob_id();
    let alice = id("personal:alice");
    let roster = vec![spec(
        "personal:alice",
        AgentAddressability::Addressable,
        "personal",
    )];

    // --- Boot 1: the turn holds at its model, a second input queues behind
    // it, and the runtime shuts down gracefully ---
    {
        let (_open, gate) = tokio::sync::watch::channel(false);
        let capture = CaptureClient::gated(gate);
        let (unified, identity_rt) = boot(&state_path, &mob_id, capture.clone()).await;
        let result = restore_flow(
            &identity_rt,
            &roster,
            Some(&EmptyTopology as &dyn TopologyProvider),
            Some(&NoopCustomizer as &dyn AgentCustomizer),
        )
        .await
        .expect("restore_flow (boot 1)");
        assert!(
            matches!(
                result.outcomes.get(&alice),
                Some(RestoreOutcome::Created { .. })
            ),
            "{:?}",
            result.outcomes
        );

        identity_rt
            .send(
                &alice,
                &meerkat_core::ContentInput::Text(format!("Start the work: {KEPT}")),
            )
            .await
            .expect("send the turn");
        let deadline = Instant::now() + Duration::from_secs(20);
        while capture.held() < 1 {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for the turn to reach its model"
            );
            sleep(Duration::from_millis(50)).await;
        }
        identity_rt
            .send(
                &alice,
                &meerkat_core::ContentInput::Text(format!("Then this: {QUEUED}")),
            )
            .await
            .expect("queue the second input");

        if let Some(window) = drain {
            let report = unified.drain_in_flight_turns(Instant::now() + window).await;
            assert!(
                matches!(
                    report.outcome,
                    meerkat_meta::InFlightDrainOutcome::Drained(_)
                ),
                "the drain stopped the mob and waited: {report:?}"
            );
            assert_eq!(capture.held(), 1, "the turn is still held at the bound");
        }
        unified.shutdown().await;
        assert_eq!(
            (capture.held(), capture.answered(), capture.count()),
            (0, 0, 1),
            "boot 1 ended with the turn unanswered and the queued input not started"
        );
    }

    // --- Boot 2: the same store, a model that answers, and nothing sent ---
    {
        let capture = CaptureClient::default();
        let (unified, identity_rt) = boot(&state_path, &mob_id, capture.clone()).await;
        let result = restore_flow(
            &identity_rt,
            &roster,
            Some(&EmptyTopology as &dyn TopologyProvider),
            Some(&NoopCustomizer as &dyn AgentCustomizer),
        )
        .await
        .expect("restore_flow (boot 2)");
        assert!(
            matches!(
                result.outcomes.get(&alice),
                Some(RestoreOutcome::Resumed { .. })
            ),
            "{:?}",
            result.outcomes
        );

        let deadline = Instant::now() + Duration::from_secs(30);
        while !capture
            .requests()
            .iter()
            .any(|request| request.contains(QUEUED))
        {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for the kept turn and the queued input to run: {:?}",
                capture.requests()
            );
            sleep(Duration::from_millis(100)).await;
        }
        let requests = capture.requests();
        let kept = requests
            .iter()
            .position(|request| request.contains(KEPT) && !request.contains(QUEUED))
            .expect("the kept turn asked its model again");
        let queued = requests
            .iter()
            .position(|request| request.contains(QUEUED))
            .expect("the queued input ran");
        assert!(
            kept < queued,
            "the kept turn resumes before the queued input runs: {requests:?}"
        );
        sleep(Duration::from_millis(500)).await;
        unified.shutdown().await;
    }
}

/// [`a_graceful_shutdown_keeps_durable_work`] without a drain window: the
/// shutdown's own stop is the first, on a running mob, and must leave the
/// turn running for the terminal Shutdown. Waits on that meerkat pin.
#[tokio::test(flavor = "multi_thread")]
async fn a_graceful_shutdown_keeps_a_durable_members_turn_and_queued_input() {
    a_graceful_shutdown_keeps_durable_work(None).await;
}

/// [`a_graceful_shutdown_keeps_durable_work`] after a drain whose window ends
/// with the turn still running: the drain's stop leaves it running and the
/// terminal Shutdown keeps it. Waits on that meerkat pin.
#[tokio::test(flavor = "multi_thread")]
async fn a_drained_graceful_shutdown_keeps_a_durable_members_turn_and_queued_input() {
    a_graceful_shutdown_keeps_durable_work(Some(Duration::from_millis(500))).await;
}

/// A graceful shutdown keeps a durable member's turn that is in flight past
/// a round commit, and the next start restores the member over that open run
/// (meerkat 0.9.5). At boot 1 the turn's first model call answers a tool
/// call, whose round commits and stands as the session's provisional tail,
/// and its second call holds. The shutdown suspends the run. At boot 2 the
/// mob start settles the tail: the store-owned recovery continues the run,
/// and the restore reports that through
/// `settle_durable_tail_before_cold_start_reporting`, which both of Meta's
/// session-service wrappers must forward. The member is resumed and the run
/// asks its model again once, from its committed round. Without the forward
/// the wrapper reports a plain settle, the start's committed read meets the
/// kept tail, and the member is refused (`MemberDurableTailUnprovable`).
#[tokio::test(flavor = "multi_thread")]
async fn a_graceful_shutdown_after_a_round_commit_restores_the_member_over_its_open_run() {
    const KEPT: &str = "KEPT-ROUND-NOVEMBER-4";
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state_path = temp.path().join("state");
    let mob_id = unique_cold_restart_mob_id();
    let alice = id("personal:alice");
    let roster = vec![spec(
        "personal:alice",
        AgentAddressability::Addressable,
        "personal",
    )];

    // --- Boot 1: one tool round commits, the next model call holds, and the
    // runtime shuts down gracefully ---
    {
        let (_open, gate) = tokio::sync::watch::channel(false);
        let capture = CaptureClient::gated(gate).with_tool_rounds(1);
        let (unified, identity_rt) = boot(&state_path, &mob_id, capture.clone()).await;
        let result = restore_flow(
            &identity_rt,
            &roster,
            Some(&EmptyTopology as &dyn TopologyProvider),
            Some(&NoopCustomizer as &dyn AgentCustomizer),
        )
        .await
        .expect("restore_flow (boot 1)");
        assert!(
            matches!(
                result.outcomes.get(&alice),
                Some(RestoreOutcome::Created { .. })
            ),
            "{:?}",
            result.outcomes
        );

        identity_rt
            .send(
                &alice,
                &meerkat_core::ContentInput::Text(format!("Start the work: {KEPT}")),
            )
            .await
            .expect("send the turn");
        let deadline = Instant::now() + Duration::from_secs(20);
        while capture.held() < 1 {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for the turn's second model call: {:?}",
                capture.requests()
            );
            sleep(Duration::from_millis(50)).await;
        }
        assert!(
            capture
                .last()
                .is_some_and(|request| request.contains(KEPT_ROUND_CALL_ID)),
            "the held call follows the committed tool round: {:?}",
            capture.requests()
        );
        unified.shutdown().await;
        assert_eq!(
            (capture.held(), capture.answered()),
            (0, 0),
            "boot 1 ended with the turn's second call unanswered"
        );
    }

    // --- Boot 2: the same store and a model that answers ---
    {
        let capture = CaptureClient::default();
        let (unified, identity_rt) = boot(&state_path, &mob_id, capture.clone()).await;
        let result = restore_flow(
            &identity_rt,
            &roster,
            Some(&EmptyTopology as &dyn TopologyProvider),
            Some(&NoopCustomizer as &dyn AgentCustomizer),
        )
        .await
        .expect("restore_flow (boot 2)");
        assert!(
            matches!(
                result.outcomes.get(&alice),
                Some(RestoreOutcome::Resumed { .. })
            ),
            "the member is restored over its open run: {:?}",
            result.outcomes
        );

        let deadline = Instant::now() + Duration::from_secs(30);
        while capture.answered() < 1 {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for the open run to ask its model again: {:?}",
                capture.requests()
            );
            sleep(Duration::from_millis(100)).await;
        }
        sleep(Duration::from_millis(500)).await;
        let requests = capture.requests();
        let resumed = requests
            .iter()
            .filter(|request| request.contains(KEPT))
            .collect::<Vec<_>>();
        assert_eq!(
            resumed.len(),
            1,
            "the open run asks its model again once: {requests:?}"
        );
        assert!(
            resumed[0].contains(KEPT_ROUND_CALL_ID),
            "the run continues from its committed tool round: {requests:?}"
        );
        unified.shutdown().await;
    }
}

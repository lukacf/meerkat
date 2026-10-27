#![allow(
    clippy::all,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    unused_imports,
    clippy::redundant_clone
)]
//! Turn-completion contract: a caller must be able to tell "my turn finished"
//! from "nothing happened yet" WITHOUT comparing output text.
//!
//! The defect these tests pin: a consumer captured the previous turn's output
//! text as a baseline, dispatched again, and waited for the text to change.
//! Both turns legitimately answered exactly `ACK`, so the comparison never
//! fired and the probe slept out its full configured wait — a 900s wait was
//! reported as a "962-second turn" that never happened.
//!
//! Test naming convention: `identity_first_completion_<scenario>`.

#[path = "support/core_members.rs"]
mod core_members;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use meerkat_core::types::HandlingMode;
use meerkat_meta::identity_first::contracts::{ContinuityStore, LeaseProvider};
use meerkat_meta::identity_first::{
    AgentAddressability, AgentBuildDraft, AgentIdentity, AgentRuntimeId, BridgeDelivery,
    BridgeError, CompletionCursor, CompletionProgress, DispatchInput, DispatchOrigin,
    DurabilityPolicy, DurableAgentSpec, FencingToken, IdentityRuntime, IdentityRuntimeConfig,
    IdentityRuntimeError, MemberInspection, ResumeSessionOutcome, SessionBridge, SessionSnapshot,
};
use meerkat_meta::identity_first::{LocalContinuityStore, LocalLeaseProvider};

// ===========================================================================
// Helpers
// ===========================================================================

fn make_identity(name: &str) -> AgentIdentity {
    AgentIdentity::parse(name).unwrap()
}

fn make_spec(name: &str) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: make_identity(name),
        profile: meerkat_mob::ProfileName::from("default"),
        addressability: AgentAddressability::Addressable,
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

fn make_content() -> meerkat_core::ContentInput {
    meerkat_core::ContentInput::Text("ping".to_string())
}

fn make_dispatch_input() -> DispatchInput {
    DispatchInput {
        content: make_content(),
        origin: DispatchOrigin::System,
        correlation_id: None,
        idempotency_key: None,
    }
}

/// Bridge whose `inspect_member` reports a scripted `output_preview`. The
/// production defect lives entirely in what a caller can observe, so the tests
/// script the observable: the agent that answers `ACK` twice.
#[derive(Default)]
struct ScriptedPreviewBridge {
    preview: tokio::sync::Mutex<Option<String>>,
    /// The members Core holds, as the tests bind them and this double's
    /// creates, resumes, successors and retires leave them.
    core: core_members::CoreMembers,
}

impl ScriptedPreviewBridge {
    async fn set_preview(&self, preview: &str) {
        *self.preview.lock().await = Some(preview.to_string());
    }
}

#[async_trait]
impl SessionBridge for ScriptedPreviewBridge {
    async fn create_session(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        _draft: &AgentBuildDraft,
    ) -> Result<meerkat_meta::identity_first::MemberRuntimeBinding, BridgeError> {
        let agent_runtime_id = AgentRuntimeId::parse(&format!("rt:{identity}:0"))
            .map_err(|error| BridgeError::Mob(error.to_string()))?;
        let session_id = &meerkat_core::types::SessionId::new();
        let binding = meerkat_meta::identity_first::MemberRuntimeBinding {
            agent_runtime_id,
            session_id: Some(session_id.clone()),
        };
        self.core.bind(identity, &binding, &spec.profile);
        Ok(binding)
    }

    async fn resume_session(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        _draft: &AgentBuildDraft,
        session_id: &meerkat_core::types::SessionId,
        _resume_from_role: Option<&meerkat_mob::ProfileName>,
    ) -> Result<ResumeSessionOutcome, BridgeError> {
        let agent_runtime_id = AgentRuntimeId::parse(&format!("rt:{identity}:0"))
            .map_err(|error| BridgeError::Mob(error.to_string()))?;
        self.core.bind(
            identity,
            &meerkat_meta::identity_first::MemberRuntimeBinding {
                agent_runtime_id: agent_runtime_id.clone(),
                session_id: Some(session_id.clone()),
            },
            &spec.profile,
        );
        Ok(ResumeSessionOutcome::Resumed {
            session_id: session_id.clone(),
            agent_runtime_id,
        })
    }

    /// One Core transition: the successor is a fresh session one generation
    /// up, and Core's fence for the member advances with it.
    async fn reset_member_to_successor(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        _draft: &AgentBuildDraft,
    ) -> Result<meerkat_meta::identity_first::MemberRuntimeBinding, BridgeError> {
        let generation = self
            .core
            .last_member_binding(identity)?
            .map_or(0, |binding| binding.generation().get() + 1);
        let agent_runtime_id = AgentRuntimeId::parse(&format!("rt:{identity}:{generation}"))
            .map_err(|error| BridgeError::Mob(error.to_string()))?;
        let binding = meerkat_meta::identity_first::MemberRuntimeBinding {
            agent_runtime_id,
            session_id: Some(meerkat_core::types::SessionId::new()),
        };
        self.core.bind(identity, &binding, &spec.profile);
        Ok(binding)
    }

    async fn live_member(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_meta::identity_first::MemberRuntimeBinding>, BridgeError> {
        self.core.live_member(identity)
    }

    async fn live_member_scope(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, BridgeError> {
        self.core.live_member_scope(identity)
    }

    async fn ensure_member_live(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_meta::identity_first::MemberRuntimeBinding>, BridgeError> {
        self.core.live_member(identity)
    }

    async fn member_binding_history(
        &self,
    ) -> Result<meerkat_mob::MemberBindingHistory, BridgeError> {
        self.core.history()
    }

    async fn last_member_binding(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberBinding>, BridgeError> {
        self.core.last_member_binding(identity)
    }

    async fn identity_management(
        &self,
        _identity: &AgentIdentity,
    ) -> Result<meerkat_mob::MemberIdentityManagement, BridgeError> {
        Ok(meerkat_mob::MemberIdentityManagement::CommandDriven)
    }

    async fn deliver_admitted(
        &self,
        runtime_id: &AgentRuntimeId,
        _delivery: BridgeDelivery,
    ) -> Result<meerkat_core::types::SessionId, BridgeError> {
        let _ = runtime_id;
        Ok(meerkat_core::types::SessionId::new())
    }

    async fn checkpoint_session(
        &self,
        _runtime_id: &AgentRuntimeId,
        _session_id: &meerkat_core::types::SessionId,
    ) -> Result<SessionSnapshot, BridgeError> {
        Ok(SessionSnapshot { data: Vec::new() })
    }

    async fn retire_member(&self, runtime_id: &AgentRuntimeId) -> Result<(), BridgeError> {
        self.core.retire(runtime_id);
        Ok(())
    }

    async fn inspect_member(
        &self,
        _runtime_id: &AgentRuntimeId,
    ) -> Result<MemberInspection, BridgeError> {
        Ok(MemberInspection {
            output_preview: self.preview.lock().await.clone(),
            preview_unavailable: None,
            is_final: false,
            peer_reachable_count: 0,
        })
    }
}

fn make_runtime(bridge: Arc<dyn SessionBridge>) -> Arc<IdentityRuntime> {
    let store: Arc<dyn ContinuityStore> = Arc::new(LocalContinuityStore::in_memory().unwrap());
    let lease: Arc<dyn LeaseProvider> = Arc::new(LocalLeaseProvider::new());
    Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: store,
        lease_provider: lease,
        runtime_instance_id: "completion-cursor-test".to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(bridge),
        default_timeout: None,
    }))
}

/// Register `name` with Core holding its member live at generation 0 under
/// fence `token`: the completion cursor's epoch is Core's fence.
async fn register_active(
    runtime: &IdentityRuntime,
    bridge: &ScriptedPreviewBridge,
    name: &str,
    token: u64,
) -> AgentIdentity {
    let identity = make_identity(name);
    bridge.core.bind_live_at_fence(
        &identity,
        &meerkat_mob::ProfileName::from("default"),
        0,
        Some(meerkat_core::types::SessionId::new()),
        token,
    );
    runtime.register(make_spec(name)).await;
    identity
}

/// The OLD completion detector, reproduced exactly: capture the previous
/// output text, then wait for `output_preview != baseline`.
///
/// Kept executable so the regression tests below cannot pass vacuously. Any
/// change that makes this function start detecting identical consecutive
/// turns would mean the scenario under test is no longer the production one.
async fn old_text_baseline_detector(
    runtime: &IdentityRuntime,
    identity: &AgentIdentity,
    baseline: Option<String>,
    budget: Duration,
) -> Option<String> {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if let Ok(inspection) = runtime.inspect(identity).await
            && let Some(preview) = inspection.output_preview
            && Some(&preview) != baseline.as_ref()
        {
            return Some(preview);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    None
}

// ===========================================================================
// The production regression
// ===========================================================================

/// The falsifier for the test below: on the EXACT same sequence of events, the
/// old text-baseline detector never fires and burns its whole budget — the
/// mechanism that turned a 900s wait into a reported "962-second turn".
///
/// If this test ever starts returning `Some`, the scenario has drifted and the
/// cursor test that follows is no longer proving anything.
#[tokio::test]
async fn identity_first_completion_old_text_baseline_misses_identical_turn() {
    let bridge = Arc::new(ScriptedPreviewBridge::default());
    let runtime = make_runtime(bridge.clone());
    let identity = register_active(&runtime, &bridge, "triage:main", 7).await;

    // Turn 1 answers `ACK`; the consumer captures that as its baseline.
    bridge.set_preview("ACK").await;
    runtime.record_turn_completed(&identity).await;
    let baseline_text = runtime.inspect(&identity).await.unwrap().output_preview;
    assert_eq!(baseline_text.as_deref(), Some("ACK"));

    // Turn 2 runs and completes, answering `ACK` again.
    runtime
        .send_admission_tracked(&identity, None, &make_content(), HandlingMode::Queue, None)
        .await
        .unwrap();
    runtime.record_turn_completed(&identity).await;
    assert_eq!(
        runtime.completion_cursor(&identity).await.turns,
        2,
        "precondition: two turns really did complete"
    );

    let observed = old_text_baseline_detector(
        &runtime,
        &identity,
        baseline_text,
        Duration::from_millis(200),
    )
    .await;

    assert_eq!(
        observed, None,
        "the old detector MUST fail here — it compares output text, and both \
         turns said `ACK`, so it sleeps out its entire budget on a turn that \
         completed. This is the defect; the next test is its fix."
    );
}

/// THE defect. Two consecutive turns both answering exactly `ACK`: the second
/// completion must be detected. The assertion on identical previews is
/// load-bearing — it proves the scenario is the one that defeats a text
/// comparison, so the cursor assertions cannot pass for the wrong reason.
#[tokio::test]
async fn identity_first_completion_two_identical_ack_turns_are_two_completions() {
    let bridge = Arc::new(ScriptedPreviewBridge::default());
    let runtime = make_runtime(bridge.clone());
    let identity = register_active(&runtime, &bridge, "triage:main", 7).await;

    // Turn 1.
    let first = runtime
        .send_admission_tracked(&identity, None, &make_content(), HandlingMode::Queue, None)
        .await
        .unwrap();
    assert_eq!(
        runtime
            .completion_cursor(&identity)
            .await
            .progress_since(first.completion_baseline),
        CompletionProgress::Pending,
        "no turn has completed yet"
    );

    bridge.set_preview("ACK").await;
    runtime.record_turn_completed(&identity).await;
    let after_first = runtime.completion_cursor(&identity).await;
    assert_eq!(
        after_first.progress_since(first.completion_baseline),
        CompletionProgress::Completed,
        "the first turn's completion must be visible"
    );
    let first_preview = runtime.inspect(&identity).await.unwrap().output_preview;

    // Turn 2 — same question, same answer, byte for byte.
    let second = runtime
        .send_admission_tracked(&identity, None, &make_content(), HandlingMode::Queue, None)
        .await
        .unwrap();
    assert_eq!(
        second.completion_baseline, after_first,
        "the second send's baseline is the cursor the first turn left"
    );
    assert_eq!(
        runtime
            .completion_cursor(&identity)
            .await
            .progress_since(second.completion_baseline),
        CompletionProgress::Pending,
        "the second turn has not completed yet"
    );

    bridge.set_preview("ACK").await;
    runtime.record_turn_completed(&identity).await;

    let second_preview = runtime.inspect(&identity).await.unwrap().output_preview;
    assert_eq!(
        first_preview, second_preview,
        "precondition: the two turns produced byte-identical output, so any \
         text comparison reports 'nothing happened' — this is the exact shape \
         that manufactured the phantom 962s turn"
    );
    assert_eq!(
        runtime
            .completion_cursor(&identity)
            .await
            .progress_since(second.completion_baseline),
        CompletionProgress::Completed,
        "the SECOND completion must be detected despite identical output"
    );
}

/// The same scenario driven through the real wait primitive rather than by
/// hand: `wait_for_completion` must return for the second identical turn.
#[tokio::test]
async fn identity_first_completion_wait_returns_for_identical_second_turn() {
    let bridge = Arc::new(ScriptedPreviewBridge::default());
    let runtime = make_runtime(bridge.clone());
    let identity = register_active(&runtime, &bridge, "triage:main", 3).await;

    bridge.set_preview("ACK").await;
    let first = runtime
        .send_admission_tracked(&identity, None, &make_content(), HandlingMode::Queue, None)
        .await
        .unwrap();
    runtime.record_turn_completed(&identity).await;
    runtime
        .wait_for_completion(&identity, first.completion_baseline, Duration::from_secs(5))
        .await
        .unwrap();

    let second = runtime
        .send_admission_tracked(&identity, None, &make_content(), HandlingMode::Queue, None)
        .await
        .unwrap();
    let completer = {
        let runtime = Arc::clone(&runtime);
        let identity = identity.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            runtime.record_turn_completed(&identity).await;
        })
    };
    let observed = runtime
        .wait_for_completion(
            &identity,
            second.completion_baseline,
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    completer.await.unwrap();
    assert_eq!(observed.turns, second.completion_baseline.turns + 1);
}

// ===========================================================================
// Cursor mechanics
// ===========================================================================

/// A poller sees a strict monotonic increase across several turns.
#[tokio::test]
async fn identity_first_completion_cursor_increases_strictly_across_turns() {
    let bridge = Arc::new(ScriptedPreviewBridge::default());
    let runtime = make_runtime(bridge.clone());
    let identity = register_active(&runtime, &bridge, "triage:main", 11).await;

    // Every turn answers the same thing, so nothing here is distinguishable
    // by content.
    bridge.set_preview("ACK").await;
    let mut previous = runtime.completion_cursor(&identity).await;
    assert_eq!(previous, CompletionCursor::start(FencingToken::new(11)));
    for expected in 1..=5_u64 {
        runtime.record_turn_completed(&identity).await;
        let current = runtime.completion_cursor(&identity).await;
        assert!(
            current > previous,
            "cursor must strictly increase: {current} is not above {previous}"
        );
        assert_eq!(current.turns, expected);
        assert_eq!(current.epoch, FencingToken::new(11));
        previous = current;
    }
}

/// Per-identity isolation: one identity's completion never satisfies a wait
/// established against another.
#[tokio::test]
async fn identity_first_completion_other_identity_completion_does_not_satisfy_wait() {
    let bridge = Arc::new(ScriptedPreviewBridge::default());
    let runtime = make_runtime(bridge.clone());
    let waiting = register_active(&runtime, &bridge, "triage:main", 4).await;
    let other = register_active(&runtime, &bridge, "worker:alpha", 5).await;

    let admission = runtime
        .send_admission_tracked(&waiting, None, &make_content(), HandlingMode::Queue, None)
        .await
        .unwrap();

    // The other identity completes several turns.
    for _ in 0..3 {
        runtime.record_turn_completed(&other).await;
    }
    assert_eq!(runtime.completion_cursor(&other).await.turns, 3);

    assert_eq!(
        runtime
            .completion_cursor(&waiting)
            .await
            .progress_since(admission.completion_baseline),
        CompletionProgress::Pending,
        "a different identity's turns must not satisfy this identity's wait"
    );

    // And the real waiter agrees: it times out rather than returning early.
    let error = runtime
        .wait_for_completion(
            &waiting,
            admission.completion_baseline,
            Duration::from_millis(250),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(error, IdentityRuntimeError::Internal(ref msg) if msg.contains("timed out")),
        "expected a timeout, got {error}"
    );
}

/// A genuinely stalled turn still times out rather than hanging forever.
#[tokio::test]
async fn identity_first_completion_stalled_turn_times_out() {
    let bridge = Arc::new(ScriptedPreviewBridge::default());
    let runtime = make_runtime(bridge.clone());
    let identity = register_active(&runtime, &bridge, "triage:main", 2).await;

    // Stale output from a previous turn is present the whole time — exactly
    // the state in which a "wait for any output" probe returns instantly.
    bridge.set_preview("ACK").await;
    let admission = runtime
        .send_admission_tracked(&identity, None, &make_content(), HandlingMode::Queue, None)
        .await
        .unwrap();

    let error = runtime
        .wait_for_completion(
            &identity,
            admission.completion_baseline,
            Duration::from_millis(250),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(error, IdentityRuntimeError::Internal(ref msg) if msg.contains("timed out")),
        "expected a timeout, got {error}"
    );
}

/// Dispatch carries the same baseline contract as send.
#[tokio::test]
async fn identity_first_completion_dispatch_admission_carries_baseline() {
    let bridge = Arc::new(ScriptedPreviewBridge::default());
    let runtime = make_runtime(bridge.clone());
    let identity = register_active(&runtime, &bridge, "internal:main", 6).await;

    let first = runtime
        .dispatch_admission_tracked(&identity, None, &make_dispatch_input())
        .await
        .unwrap();
    assert_eq!(first.completion_baseline.epoch, FencingToken::new(6));
    assert_eq!(first.completion_baseline.turns, 0);
    assert!(first.durable);

    runtime.record_turn_completed(&identity).await;

    let second = runtime
        .dispatch_admission_tracked(&identity, None, &make_dispatch_input())
        .await
        .unwrap();
    assert_eq!(second.completion_baseline.turns, 1);
    assert_eq!(
        runtime
            .completion_cursor(&identity)
            .await
            .progress_since(first.completion_baseline),
        CompletionProgress::Completed
    );
    assert_eq!(
        runtime
            .completion_cursor(&identity)
            .await
            .progress_since(second.completion_baseline),
        CompletionProgress::Pending
    );
}

// ===========================================================================
// Incarnation (epoch) semantics
// ===========================================================================

/// Across a lease incarnation change the cursor never regresses, and a
/// baseline from the old incarnation is reported as such rather than being
/// read as either completion or continued waiting.
#[tokio::test]
async fn identity_first_completion_cursor_never_regresses_across_incarnations() {
    let bridge = Arc::new(ScriptedPreviewBridge::default());
    let runtime = make_runtime(bridge.clone());
    let identity = register_active(&runtime, &bridge, "triage:main", 4).await;

    for _ in 0..3 {
        runtime.record_turn_completed(&identity).await;
    }
    let before = runtime.completion_cursor(&identity).await;
    assert_eq!(before, CompletionCursor::new(FencingToken::new(4), 3));

    // A new incarnation: Core binds a successor under a strictly higher
    // fence.
    bridge.core.bind_live_at_fence(
        &identity,
        &meerkat_mob::ProfileName::from("default"),
        1,
        Some(meerkat_core::types::SessionId::new()),
        9,
    );

    let after = runtime.completion_cursor(&identity).await;
    assert_eq!(after, CompletionCursor::new(FencingToken::new(9), 0));
    assert!(
        after > before,
        "a fresh incarnation must sort above every cursor the old one published"
    );
    assert_eq!(
        after.progress_since(before),
        CompletionProgress::IncarnationChanged,
        "turn counts are not comparable across incarnations"
    );

    let error = runtime
        .wait_for_completion(&identity, before, Duration::from_millis(250))
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            IdentityRuntimeError::CompletionIncarnationChanged { .. }
        ),
        "expected an incarnation-change error, got {error}"
    );
}

/// The retention guard: a cursor value is never re-published.
///
/// Delete starts the identity's fresh life on Core: a successor one
/// generation up under a higher fence. The ledger is deliberately NOT pruned
/// across it, so the identity cannot hand a second caller a cursor a first
/// caller already saw. This is the runtime-level counterpart to
/// `..._stale_epoch_cannot_rewind_cursor`, which covers the pure value.
#[tokio::test]
async fn identity_first_completion_cursor_is_never_republished_after_delete() {
    let bridge = Arc::new(ScriptedPreviewBridge::default());
    let runtime = make_runtime(bridge.clone());
    let identity = register_active(&runtime, &bridge, "triage:main", 5).await;

    let mut published = Vec::new();
    for _ in 0..3 {
        runtime.record_turn_completed(&identity).await;
        published.push(runtime.completion_cursor(&identity).await);
    }

    // The identity's life ends: delete starts a fresh one on Core.
    runtime.delete_identity(&identity).await.unwrap();
    let after_delete = runtime.completion_cursor(&identity).await;
    assert!(
        after_delete > *published.last().unwrap(),
        "the fresh life's cursor must sort above everything the old life \
         published: {after_delete} is not above {}",
        published.last().unwrap()
    );
    assert_eq!(after_delete.turns, 0, "the turn count is per-incarnation");
    published.push(after_delete);

    for _ in 0..3 {
        runtime.record_turn_completed(&identity).await;
        published.push(runtime.completion_cursor(&identity).await);
    }

    let mut seen = std::collections::BTreeSet::new();
    for cursor in &published {
        assert!(
            seen.insert(*cursor),
            "cursor {cursor} was published twice across the identity's lifetime"
        );
    }
    for pair in published.windows(2) {
        assert!(
            pair[1] > pair[0],
            "published cursors must be strictly increasing: {} then {}",
            pair[0],
            pair[1]
        );
    }
}

/// A stale (lower) epoch presented after the cursor has advanced must not
/// rewind what has already been published.
#[tokio::test]
async fn identity_first_completion_stale_epoch_cannot_rewind_cursor() {
    let advanced = CompletionCursor::new(FencingToken::new(9), 4);
    assert_eq!(
        advanced.rebased(FencingToken::new(3)),
        advanced,
        "an older token must leave the cursor untouched"
    );
    assert_eq!(
        advanced.rebased(FencingToken::new(9)),
        advanced,
        "the same token must leave the count intact"
    );
    assert_eq!(
        advanced.rebased(FencingToken::new(10)),
        CompletionCursor::new(FencingToken::new(10), 0),
        "a newer token starts a fresh count"
    );
}

/// Lexicographic ordering: epoch dominates, turns break ties.
#[test]
fn identity_first_completion_cursor_orders_epoch_then_turns() {
    let a = CompletionCursor::new(FencingToken::new(1), 100);
    let b = CompletionCursor::new(FencingToken::new(2), 0);
    assert!(b > a, "a higher epoch outranks any turn count below it");
    assert!(
        CompletionCursor::new(FencingToken::new(2), 1) > b,
        "within an epoch, turns order the cursor"
    );
    assert_eq!(
        CompletionCursor::default(),
        CompletionCursor::start(FencingToken::new(0))
    );
}

/// The cursor is a count, not a content digest: identical text at different
/// counts is distinguishable, and different text at the same count is not
/// mistaken for progress.
#[test]
fn identity_first_completion_cursor_is_independent_of_output_content() {
    let baseline = CompletionCursor::new(FencingToken::new(1), 2);
    assert_eq!(
        baseline.progress_since(baseline),
        CompletionProgress::Pending,
        "an unchanged cursor is Pending regardless of what the agent said"
    );
    assert_eq!(
        baseline.advanced().progress_since(baseline),
        CompletionProgress::Completed,
        "one more completed turn is Completed regardless of what it said"
    );
}

// ===========================================================================
// Wire compatibility
// ===========================================================================

/// The cursor is a plain `{epoch, turns}` object on the wire, and round-trips.
#[test]
fn identity_first_completion_cursor_wire_round_trip() {
    let cursor = CompletionCursor::new(FencingToken::new(12), 34);
    let value = serde_json::to_value(cursor).unwrap();
    assert_eq!(value, serde_json::json!({ "epoch": 12, "turns": 34 }));
    let parsed: CompletionCursor = serde_json::from_value(value).unwrap();
    assert_eq!(parsed, cursor);
}

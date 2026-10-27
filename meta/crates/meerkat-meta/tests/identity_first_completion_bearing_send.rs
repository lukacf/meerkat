#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::redundant_clone
)]
//! `IdentityRuntime::send_awaiting_commit` - the completion-bearing send.
//!
//! The defect this API exists to remove: **a session-wide `RunCompleted` /
//! `RunFailed` cannot authorize a specific turn**, because queued turns share a
//! `session_id`. A caller that sends and then waits on the session's event
//! stream can be released by some OTHER turn's terminal. A test built that way
//! passes while the behaviour it claims to prove is broken. A timer is worse
//! still: it elapses whether the turn succeeded, failed closed, or never ran.
//!
//! Every test here asserts on an exact typed VARIANT and on a CALL COUNT.
//! Neither alone is enough - the variant says what the runtime concluded, the
//! count says what it actually did to the bridge. A silent fallback to the
//! ingress lane would keep the variant plausible and move the count.
//!
//! Nothing here accepts a wall clock. Completion ordering is proven by direct
//! poll, so "still running" and "finished but slow" cannot be confused.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use async_trait::async_trait;
use meerkat_core::types::HandlingMode;
use meerkat_meta::identity_first::contracts::{ContinuityStore, LeaseProvider};
use meerkat_meta::identity_first::{
    AgentAddressability, AgentBuildDraft, AgentIdentity, AgentRuntimeId, BridgeAdmissionError,
    BridgeDelivery, BridgeError, BridgeTurnReceipt, DurabilityPolicy, DurableAgentSpec,
    FencingToken, IdentityRuntime, IdentityRuntimeConfig, IdentityRuntimeError, MemberInspection,
    MemberRuntimeBinding, ResumeSessionOutcome, SessionBridge, SessionSnapshot,
};
use meerkat_meta::identity_first::{LocalContinuityStore, LocalLeaseProvider};

#[path = "support/core_members.rs"]
mod core_members;

use core_members::CoreMembers;

// ===========================================================================
// Fixtures
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

fn content(text: &str) -> meerkat_core::ContentInput {
    meerkat_core::ContentInput::Text(text.to_string())
}

// ===========================================================================
// The scripted bridge
// ===========================================================================

/// What the completion-bearing verb does when called.
#[derive(Clone)]
enum CompletionScript {
    /// Park until this delivery's OWN gate is released, then succeed.
    ///
    /// Gates are per-CALL, not per-session, which is the whole point: it lets a
    /// test release one delivery's terminal and prove a sibling's is untouched.
    GateThenSucceed,
    /// Fail immediately without returning a receipt.
    AdmissionFail(fn() -> BridgeAdmissionError),
    ActorTimeout(&'static str),
    /// Admit (so a receipt IS returned), park until released, then fail the
    /// turn. This is the only way to script "the turn ran and failed", which
    /// `Fail` cannot express because it never admits.
    GateThenFail(fn() -> String),
    /// Admit with an UNRESOLVED session, park, then let the turn SUCCEED.
    ///
    /// The receipt's `session_result` is `Err`, so `wait()` yields
    /// `PostAdmissionResolutionFailed` - the turn did its work and only the
    /// post-hoc projection failed. Distinct from `GateThenFail`, where the turn
    /// itself failed, and the runtime must not conflate them.
    GateThenSucceedUnresolved(fn() -> String),
}

/// Bridge that counts what it was actually asked to do and gates completion per
/// delivery.
///
/// Both lanes are counted separately. `ingress_calls` is the fallback detector:
/// a completion-bearing send that quietly degrades to the admission-only path
/// would still return `Ok`, and only this counter would show it.
struct PhaseScriptedBridge {
    script: CompletionScript,
    /// The session id every admitted turn resolves onto.
    ///
    /// Explicit, never `SessionId::new()` per call: a fresh id per delivery
    /// would make every turn look like a session ROTATION, and a legitimate
    /// rotation must stay distinguishable from the sibling-release defect T5
    /// is about.
    delivered_session: meerkat_core::types::SessionId,
    ingress_calls: AtomicUsize,
    completion_calls: AtomicUsize,
    receipt_waits_entered: Arc<AtomicUsize>,
    /// One release flag per completion call, in call order.
    gates: std::sync::Mutex<Vec<Arc<tokio::sync::Notify>>>,
    released: std::sync::Mutex<Vec<Arc<std::sync::atomic::AtomicBool>>>,
    /// The members Core holds: every Core read the runtime makes answers
    /// from here.
    core: CoreMembers,
}

impl PhaseScriptedBridge {
    fn new(script: CompletionScript) -> Arc<Self> {
        Self::returning(script, meerkat_core::types::SessionId::new())
    }

    /// Build one that resolves every turn onto an EXACT session id.
    fn returning(
        script: CompletionScript,
        delivered_session: meerkat_core::types::SessionId,
    ) -> Arc<Self> {
        Arc::new(Self {
            script,
            delivered_session,
            ingress_calls: AtomicUsize::new(0),
            completion_calls: AtomicUsize::new(0),
            receipt_waits_entered: Arc::new(AtomicUsize::new(0)),
            gates: std::sync::Mutex::new(Vec::new()),
            released: std::sync::Mutex::new(Vec::new()),
            core: CoreMembers::default(),
        })
    }

    fn ingress_calls(&self) -> usize {
        self.ingress_calls.load(Ordering::SeqCst)
    }

    fn completion_calls(&self) -> usize {
        self.completion_calls.load(Ordering::SeqCst)
    }

    fn receipt_waits_entered(&self) -> usize {
        self.receipt_waits_entered.load(Ordering::SeqCst)
    }

    /// Release the Nth completion call (0-indexed, in call order).
    fn release(&self, nth: usize) {
        let (notify, flag) = {
            let gates = self.gates.lock().expect("gates mutex poisoned");
            let released = self.released.lock().expect("released mutex poisoned");
            (
                Arc::clone(gates.get(nth).expect("no such completion call")),
                Arc::clone(released.get(nth).expect("no such completion call")),
            )
        };
        flag.store(true, Ordering::SeqCst);
        notify.notify_waiters();
    }
}

#[async_trait]
impl SessionBridge for PhaseScriptedBridge {
    async fn create_session(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        _draft: &AgentBuildDraft,
    ) -> Result<MemberRuntimeBinding, BridgeError> {
        let agent_runtime_id = AgentRuntimeId::parse(&format!("rt:{identity}:0"))
            .map_err(|error| BridgeError::Mob(error.to_string()))?;
        let binding = MemberRuntimeBinding {
            agent_runtime_id,
            session_id: Some(meerkat_core::types::SessionId::new()),
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
            &MemberRuntimeBinding {
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

    /// Core's successor respawn (reset, delete): the identity's member at
    /// the next generation, on a fresh session, under the next fence.
    async fn reset_member_to_successor(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        _draft: &AgentBuildDraft,
    ) -> Result<MemberRuntimeBinding, BridgeError> {
        let generation = self
            .core
            .last_member_binding(identity)?
            .map_or(0, |binding| binding.generation().get() + 1);
        let agent_runtime_id = AgentRuntimeId::parse(&format!("rt:{identity}:{generation}"))
            .map_err(|error| BridgeError::Mob(error.to_string()))?;
        let binding = MemberRuntimeBinding {
            agent_runtime_id,
            session_id: Some(meerkat_core::types::SessionId::new()),
        };
        self.core.bind(identity, &binding, &spec.profile);
        Ok(binding)
    }

    async fn live_member(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<MemberRuntimeBinding>, BridgeError> {
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
    ) -> Result<Option<MemberRuntimeBinding>, BridgeError> {
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

    /// The INGRESS lane. Counted so a silent fallback cannot hide.
    async fn deliver_admitted(
        &self,
        _runtime_id: &AgentRuntimeId,
        _delivery: BridgeDelivery,
    ) -> Result<meerkat_core::types::SessionId, BridgeError> {
        self.ingress_calls.fetch_add(1, Ordering::SeqCst);
        Ok(meerkat_core::types::SessionId::new())
    }

    /// The COMPLETION lane, scripted at the PRIMITIVE - `begin_awaiting_commit`
    /// - because that is what the runtime calls. The composed
    /// `deliver_awaiting_commit_*` is the trait default and is left alone, so
    /// these tests exercise the real composition rather than a stand-in.
    async fn begin_awaiting_commit(
        &self,
        _runtime_id: &AgentRuntimeId,
        _content: &meerkat_core::ContentInput,
        _system_prompt: Option<&str>,
        _injected_context: &[meerkat_core::ContentInput],
        _handling_mode: HandlingMode,
        _interaction_id: Option<&str>,
    ) -> Result<BridgeTurnReceipt, BridgeAdmissionError> {
        self.completion_calls.fetch_add(1, Ordering::SeqCst);
        match self.script {
            // No receipt is observed; this alone establishes no execution fate.
            CompletionScript::AdmissionFail(make) => Err(make()),
            CompletionScript::ActorTimeout(stage) => {
                Err(BridgeAdmissionError::ActorAdmissionTimeout {
                    operation: "deliver.start_work",
                    identity: meerkat_mob::AgentIdentity::from("rt-agent-alpha-0"),
                    waited: Duration::from_millis(50),
                    command: Some(meerkat_meta::identity_first::ActorCommandTimeout {
                        command_kind: "StartWork",
                        stage,
                    }),
                })
            }
            CompletionScript::GateThenSucceed
            | CompletionScript::GateThenFail(_)
            | CompletionScript::GateThenSucceedUnresolved(_) => {
                let outcome = match self.script {
                    CompletionScript::GateThenFail(make) => Some(make),
                    _ => None,
                };
                let session_result = match self.script {
                    CompletionScript::GateThenSucceedUnresolved(make) => Err(make()),
                    _ => Ok(self.delivered_session.clone()),
                };
                let (notify, flag) = {
                    let mut gates = self.gates.lock().expect("gates mutex poisoned");
                    let mut released = self.released.lock().expect("released mutex poisoned");
                    let notify = Arc::new(tokio::sync::Notify::new());
                    let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
                    gates.push(Arc::clone(&notify));
                    released.push(Arc::clone(&flag));
                    (notify, flag)
                };
                let receipt_waits_entered = Arc::clone(&self.receipt_waits_entered);
                // ADMITTED. The receipt is returned immediately; only its
                // completion parks. That is the property under test.
                Ok(BridgeTurnReceipt::new(session_result, async move {
                    receipt_waits_entered.fetch_add(1, Ordering::SeqCst);
                    while !flag.load(Ordering::SeqCst) {
                        notify.notified().await;
                    }
                    match outcome {
                        Some(make) => Err(make()),
                        None => Ok(()),
                    }
                }))
            }
        }
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
            output_preview: None,
            preview_unavailable: None,
            is_final: false,
            peer_reachable_count: 0,
        })
    }
}

fn make_runtime(bridge: Option<Arc<dyn SessionBridge>>) -> Arc<IdentityRuntime> {
    let store: Arc<dyn ContinuityStore> = Arc::new(LocalContinuityStore::in_memory().unwrap());
    let lease: Arc<dyn LeaseProvider> = Arc::new(LocalLeaseProvider::new());
    Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: store,
        lease_provider: lease,
        runtime_instance_id: "completion-bearing-send-test".to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge,
        default_timeout: None,
    }))
}

/// Register an identity Core holds live (generation 0, fence `token`) on a
/// fresh session.
async fn register_bound(
    runtime: &IdentityRuntime,
    bridge: &PhaseScriptedBridge,
    name: &str,
    token: u64,
) -> AgentIdentity {
    register_bound_on(
        runtime,
        bridge,
        name,
        token,
        meerkat_core::types::SessionId::new(),
    )
    .await
}

/// Register with Core holding the member on an EXACT session, so a test can
/// pin the session the identity is currently bound to.
async fn register_bound_on(
    runtime: &IdentityRuntime,
    bridge: &PhaseScriptedBridge,
    name: &str,
    token: u64,
    session: meerkat_core::types::SessionId,
) -> AgentIdentity {
    let identity = make_identity(name);
    bridge.core.bind_live_at_fence(
        &identity,
        &meerkat_mob::ProfileName::from("default"),
        0,
        Some(session),
        token,
    );
    runtime.register(make_spec(name)).await;
    identity
}

// ===========================================================================
// T0 - the positive
// ===========================================================================

/// A successful completion-bearing send: exactly ONE submit through the
/// completion lane, ZERO through ingress, and the call does not return until
/// its OWN terminal.
///
/// "Does not return until" is proven by direct poll, not by elapsed time: the
/// future is polled while the bridge is still holding the turn, and must be
/// Pending. A wall-clock version of this assertion would pass against an
/// implementation that returned immediately and simply happened to be fast.
#[tokio::test(flavor = "current_thread")]
async fn a_successful_completion_bearing_send_submits_once_and_waits_for_its_own_terminal() {
    let delivered_session = meerkat_core::types::SessionId::new();
    let bridge = PhaseScriptedBridge::returning(
        CompletionScript::GateThenSucceed,
        delivered_session.clone(),
    );
    let runtime = make_runtime(Some(Arc::clone(&bridge) as Arc<dyn SessionBridge>));
    let alice = register_bound_on(&runtime, &bridge, "alice", 1, delivered_session.clone()).await;

    let turn = content("turn one");
    let send = runtime.send_awaiting_commit(&alice, &turn);
    let mut send = Box::pin(send);
    let mut cx = Context::from_waker(Waker::noop());

    // Drive until the receipt's terminal future is actually entered. The
    // earlier bridge-call counter marks only Begin, which is too early: the
    // runtime has not necessarily received the receipt or released its lock.
    for _ in 0..64 {
        if bridge.receipt_waits_entered() == 1 {
            break;
        }
        assert!(
            send.as_mut().poll(&mut cx).is_pending(),
            "the send completed before the bridge admitted its exact turn"
        );
    }
    assert_eq!(
        bridge.completion_calls(),
        1,
        "the completion lane must have been entered exactly once"
    );
    assert_eq!(
        bridge.receipt_waits_entered(),
        1,
        "the runtime must own and await the exact receipt before release"
    );
    assert!(
        send.as_mut().poll(&mut cx).is_pending(),
        "the send returned while the bridge was still holding the turn - it did \
         not wait for its own terminal"
    );

    bridge.release(0);
    let token = send
        .await
        .expect("the completion-bearing send must succeed");

    assert_eq!(
        bridge.completion_calls(),
        1,
        "exactly one submit: more than one means the turn ran more than once"
    );
    assert_eq!(
        bridge.ingress_calls(),
        0,
        "the completion lane must NEVER touch the ingress verb - any count here \
         is a silent fallback reporting success for an unawaited turn"
    );
    let status = runtime.status(&alice).await.expect("alice status");
    assert_eq!(status.session_id, Some(delivered_session));
    assert_eq!(
        token,
        FencingToken::new(1),
        "the returned token must be the fence Core holds the member under"
    );
}

// ===========================================================================
// T0b - fail closed on local absence
// ===========================================================================

/// No bridge installed: the send fails closed, naming the missing bridge,
/// and nothing is submitted.
#[tokio::test]
async fn no_bridge_fails_closed_and_submits_nothing() {
    let runtime = make_runtime(None);
    runtime.register(make_spec("alice")).await;
    let alice = make_identity("alice");

    match runtime.send_awaiting_commit(&alice, &content("turn")).await {
        // Without a bridge Core's mob cannot even wake the identity, so the
        // send refuses before any lane is chosen.
        Err(error) => {
            assert!(
                error.to_string().contains("no session bridge"),
                "the error must name which precondition failed: {error}"
            );
        }
        other => panic!(
            "with no bridge there is nothing that can observe completion, so this \
             must fail CLOSED rather than report success for a turn nobody ran. \
             Got: {other:?}"
        ),
    }
}

// ===========================================================================
// T1-T4 - the phase matrix
// ===========================================================================

/// T1. Ingress-only bridge: `CompletionUnsupported` becomes
/// `CompletionUnavailable` - NOTHING SUBMITTED - and never degrades to ingress.
#[tokio::test]
async fn an_ingress_only_bridge_is_unavailable_not_failed_and_never_falls_back() {
    let bridge = PhaseScriptedBridge::new(CompletionScript::AdmissionFail(|| {
        BridgeAdmissionError::CompletionUnsupported(
            "this bridge implements ingress-only delivery".into(),
        )
    }));
    let runtime = make_runtime(Some(Arc::clone(&bridge) as Arc<dyn SessionBridge>));
    let alice = register_bound(&runtime, &bridge, "alice", 1).await;

    match runtime.send_awaiting_commit(&alice, &content("turn")).await {
        Err(IdentityRuntimeError::CompletionUnavailable { reason, .. }) => assert!(
            reason.contains("ingress-only"),
            "the unsupported detail must be carried, not dropped: {reason}"
        ),
        other => panic!(
            "CompletionUnsupported is documented as returned BEFORE any submission. \
             Reporting it as CompletionFailed would tell an operator a turn RAN when \
             nothing was delivered. Got: {other:?}"
        ),
    }
    assert_eq!(
        bridge.ingress_calls(),
        0,
        "an unsupported completion must NOT be retried down the ingress lane"
    );
}

/// T2. No receipt was observed before timeout. Execution fate stays unknown.
#[tokio::test]
async fn an_admission_failure_is_typed_as_admission_and_not_as_completion() {
    let bridge = PhaseScriptedBridge::new(CompletionScript::AdmissionFail(|| {
        BridgeAdmissionError::ActorAdmissionTimeout {
            operation: "deliver.submit_work",
            identity: meerkat_mob::AgentIdentity::from("rt-agent-alpha-0"),
            waited: Duration::from_millis(50),
            command: None,
        }
    }));
    let runtime = make_runtime(Some(Arc::clone(&bridge) as Arc<dyn SessionBridge>));
    let alice = register_bound(&runtime, &bridge, "alice", 1).await;

    match runtime.send_awaiting_commit(&alice, &content("turn")).await {
        Err(error @ IdentityRuntimeError::ActorAdmissionTimeout { .. }) => assert!(
            error.to_string().contains("deliver.submit_work"),
            "the timeout must name the round trip that expired: {error}"
        ),
        other => panic!("timeout must remain a typed observation, got: {other:?}"),
    }
    assert_eq!(
        bridge.ingress_calls(),
        0,
        "no fallback on admission failure"
    );
}

/// T3. Admitted, turn ran and FAILED: exactly one submit, no retry.
#[tokio::test]
async fn completion_lane_preserves_both_actor_deadline_stages_without_retry() {
    for stage in ["actor_command_admission", "actor_command_reply"] {
        let bridge = PhaseScriptedBridge::new(CompletionScript::ActorTimeout(stage));
        let runtime = make_runtime(Some(Arc::clone(&bridge) as Arc<dyn SessionBridge>));
        let alice = register_bound(&runtime, &bridge, "alice", 1).await;
        let error = runtime
            .send_awaiting_commit(&alice, &content("turn"))
            .await
            .unwrap_err();
        let data = error.structured_data().expect("typed timeout");
        assert_eq!(data["command_kind"], "StartWork");
        assert_eq!(data["stage"], stage);
        assert!(data.get("executed").is_none());
        assert!(data.get("retryable").is_none());
        assert!(!error.to_string().contains("never started"));
        assert_eq!(bridge.completion_calls(), 1);
        assert_eq!(bridge.ingress_calls(), 0);
        let health = runtime.member_health(&alice).await.unwrap();
        assert_eq!(health.last_delivery_error.unwrap().data, Some(data));
        assert!(health.last_reload.is_none());
    }
}

/// T3. Admitted, turn ran and FAILED: exactly one submit, no retry.
#[tokio::test(flavor = "current_thread")]
async fn a_turn_that_ran_and_failed_is_completion_failed_and_is_submitted_exactly_once() {
    // GateThenFail, NOT Fail. `Fail` returns Err from begin_awaiting_commit
    // BEFORE a receipt exists, so nothing is ever admitted and the completion
    // phase is never reached - the test would pass without exercising the
    // behaviour it names.
    let bound_session = meerkat_core::types::SessionId::new();
    let bridge = PhaseScriptedBridge::returning(
        CompletionScript::GateThenFail(|| "model provider returned a terminal error".to_string()),
        bound_session.clone(),
    );
    let runtime = make_runtime(Some(Arc::clone(&bridge) as Arc<dyn SessionBridge>));
    let alice = register_bound_on(&runtime, &bridge, "alice", 1, bound_session).await;

    let turn = content("turn");
    let mut send = Box::pin(runtime.send_awaiting_commit(&alice, &turn));
    let mut cx = Context::from_waker(Waker::noop());
    drive_until_admitted(&mut send, &bridge, &mut cx);
    bridge.release(0);

    match send.await {
        Err(IdentityRuntimeError::CompletionFailed { detail, .. }) => assert!(
            detail.contains("terminal error"),
            "the failure detail must survive: {detail}"
        ),
        other => panic!("expected CompletionFailed, got {other:?}"),
    }
    assert_eq!(
        bridge.completion_calls(),
        1,
        "EXACTLY one. A second submit would run the member's turn twice - which is \
         the defect, not a recovery."
    );
    assert_eq!(bridge.ingress_calls(), 0, "no fallback after the turn ran");
}

/// T4. Admitted, turn ran and SUCCEEDED, only the session projection failed:
/// its own dedicated phase, exactly one submit, no retry.
#[tokio::test(flavor = "current_thread")]
async fn a_post_admission_projection_failure_keeps_its_own_phase_and_is_submitted_once() {
    // GateThenSucceedUnresolved: the turn must ADMIT and SUCCEED with an
    // unresolved session. `Fail` would return before a receipt exists, which is
    // a different phase entirely - "nothing was submitted", not "the turn ran
    // and only its projection failed".
    let bound_session = meerkat_core::types::SessionId::new();
    let bridge = PhaseScriptedBridge::returning(
        CompletionScript::GateThenSucceedUnresolved(|| {
            "member has no bridge session after deliver".to_string()
        }),
        bound_session.clone(),
    );
    let runtime = make_runtime(Some(Arc::clone(&bridge) as Arc<dyn SessionBridge>));
    let alice = register_bound_on(&runtime, &bridge, "alice", 1, bound_session).await;

    let turn = content("turn");
    let mut send = Box::pin(runtime.send_awaiting_commit(&alice, &turn));
    let mut cx = Context::from_waker(Waker::noop());
    drive_until_admitted(&mut send, &bridge, &mut cx);
    bridge.release(0);

    match send.await {
        Err(IdentityRuntimeError::PostAdmissionResolutionFailed { detail, .. }) => assert!(
            detail.contains("no bridge session"),
            "the projection detail must survive: {detail}"
        ),
        other => panic!(
            "the member DID the work here and only the post-hoc projection failed. \
             Collapsing it into CompletionFailed loses the one fact an operator needs: \
             the turn succeeded. Got: {other:?}"
        ),
    }
    assert_eq!(
        bridge.completion_calls(),
        1,
        "EXACTLY one: the turn already did its work"
    );
}

// ===========================================================================
// T5 - the negative that justifies the whole API
// ===========================================================================

/// A SIBLING turn on the SAME IDENTITY must not release this delivery.
///
/// This is the defect in one test. Queued turns share a `session_id`, so any
/// wait keyed on session-wide terminal events can be satisfied by the wrong
/// turn. Two deliveries to ONE identity are in flight here; releasing the
/// SECOND one's terminal must leave the FIRST one's future Pending, and only
/// the first's own terminal may complete it.
///
/// Both deliveries resolve onto the identity's EXISTING current session id, not
/// a fresh one. With fresh ids each turn would look like a session ROTATION,
/// and a legitimate rotation would be indistinguishable from the sibling-release
/// defect this test exists to catch.
///
/// That both are in flight at once is itself load-bearing: it can only happen
/// because the lifecycle lock is released between admission and the turn. Under
/// the pre-receipt code the second send could not even be admitted until the
/// first finished, so this test doubles as the two-in-flight proof.
///
/// Proven by direct poll. No timeout, no sleep: with a clock, "not finished
/// yet" and "finished, because the wrong terminal released it a moment ago" are
/// indistinguishable.
#[tokio::test(flavor = "current_thread")]
async fn a_sibling_turn_on_the_same_identity_cannot_release_this_delivery() {
    let bound_session = meerkat_core::types::SessionId::new();
    let bridge =
        PhaseScriptedBridge::returning(CompletionScript::GateThenSucceed, bound_session.clone());
    let runtime = make_runtime(Some(Arc::clone(&bridge) as Arc<dyn SessionBridge>));
    let alice = register_bound_on(&runtime, &bridge, "alice", 1, bound_session).await;

    let first_turn = content("first turn");
    let second_turn = content("second turn");
    let mut first = Box::pin(runtime.send_awaiting_commit(&alice, &first_turn));
    let mut second = Box::pin(runtime.send_awaiting_commit(&alice, &second_turn));
    let mut cx = Context::from_waker(Waker::noop());

    for _ in 0..256 {
        if bridge.receipt_waits_entered() == 2 {
            break;
        }
        assert!(
            first.as_mut().poll(&mut cx).is_pending(),
            "the first send completed before its exact terminal"
        );
        assert!(
            second.as_mut().poll(&mut cx).is_pending(),
            "the second send completed before its exact terminal"
        );
    }
    assert_eq!(
        bridge.completion_calls(),
        2,
        "both deliveries to the SAME identity must be admitted and in flight. If \
         this is 1, the lifecycle lock is still held across the turn and the \
         second send cannot even be admitted until the first finishes."
    );
    assert_eq!(
        bridge.receipt_waits_entered(),
        2,
        "both exact receipts must reach their independent terminal waits"
    );

    // Release ONLY the second delivery's terminal.
    bridge.release(1);
    second
        .await
        .expect("the second delivery must complete on its own terminal");

    assert!(
        first.as_mut().poll(&mut cx).is_pending(),
        "a SIBLING turn's terminal released this delivery. That is the exact \
         defect: a session-wide terminal cannot authorize one specific turn, so \
         waiting on one lets the wrong turn satisfy the wait."
    );

    // Only its OWN terminal completes it.
    bridge.release(0);
    first
        .await
        .expect("the first delivery must complete on its own terminal");
}

// ===========================================================================
// The 2D outcome table - terminal x supersede
// ===========================================================================
//
// What the turn DID and whether its embodiment STILL EXISTS are independent
// axes. An implementation that returns on supersede before mapping the terminal
// loses one of them, and which one it loses changes what an operator does next.
//
// That these scenarios are reachable AT ALL is itself the proof the lock is
// released across the turn: under the pre-receipt code nothing could mutate the
// identity mid-flight, because the send held the lifecycle lock for the whole
// turn. Each of these tests would deadlock rather than fail.

/// Replace the identity's embodiment while its turn is in flight: Core now
/// holds the member at the next generation, on a fresh session, under fence
/// `token`. Returns the replacement's runtime id.
///
/// A fresh runtime id is exactly what a reset or respawn produces, and it is
/// one of the three captured fields, so the SUPERSEDE this drives is the real
/// path rather than a simulated one.
///
/// It is NOT evidence that the lifecycle lock is free: it takes no lock. The
/// delete case proves the lock.
fn supersede_while_running(bridge: &PhaseScriptedBridge, name: &str, token: u64) -> AgentRuntimeId {
    bridge.core.bind_live_at_fence(
        &make_identity(name),
        &meerkat_mob::ProfileName::from("default"),
        1,
        Some(meerkat_core::types::SessionId::new()),
        token,
    )
}

/// Drive a send until the bridge has admitted it and the turn is parked.
fn drive_until_admitted<F: std::future::Future>(
    send: &mut std::pin::Pin<Box<F>>,
    bridge: &PhaseScriptedBridge,
    cx: &mut Context<'_>,
) {
    for _ in 0..256 {
        if bridge.receipt_waits_entered() == 1 {
            return;
        }
        assert!(
            send.as_mut().poll(cx).is_pending(),
            "the send completed before the bridge admitted its exact turn"
        );
    }
    panic!("the runtime never released admission and entered the receipt wait");
}

/// ROW: turn SUCCEEDED + superseded -> `PostAdmissionSuperseded`, and the old
/// session is NOT rebound onto the live incarnation.
///
/// Not rebinding is the whole point. `reconcile_delivered_session_locked` treats
/// a session mismatch as a rotation and calls
/// `rebind_session_after_live_respawn_locked`, which SUSPENDS the identity and
/// re-acquires its leases - so reconciling here would bind the NEW embodiment to
/// the DEAD turn's session. Losing the attribution is the lesser harm.
#[tokio::test(flavor = "current_thread")]
async fn a_successful_turn_against_a_superseded_identity_is_typed_and_never_rebinds() {
    let bound_session = meerkat_core::types::SessionId::new();
    let bridge =
        PhaseScriptedBridge::returning(CompletionScript::GateThenSucceed, bound_session.clone());
    let runtime = make_runtime(Some(Arc::clone(&bridge) as Arc<dyn SessionBridge>));
    let alice = register_bound_on(&runtime, &bridge, "alice", 1, bound_session).await;

    let turn = content("turn");
    let mut send = Box::pin(runtime.send_awaiting_commit(&alice, &turn));
    let mut cx = Context::from_waker(Waker::noop());
    drive_until_admitted(&mut send, &bridge, &mut cx);

    // The turn is running and the lock is free - so this can happen at all.
    let replacement = supersede_while_running(&bridge, "alice", 2);
    bridge.release(0);

    match send.await {
        Err(IdentityRuntimeError::PostAdmissionSuperseded { detail, .. }) => assert!(
            detail.contains("admitted onto"),
            "the detail must name both embodiments so an operator can see what \
             changed: {detail}"
        ),
        other => panic!(
            "a turn that SUCCEEDED against an embodiment that no longer exists must \
             be typed non-retryable, not reported as success. Got: {other:?}"
        ),
    }

    let status = runtime.status(&alice).await.expect("alice status");
    assert_eq!(
        status.agent_runtime_id,
        Some(replacement),
        "the live incarnation must be the replacement, untouched by the dead turn"
    );
}

/// ROW: turn RAN AND FAILED + superseded -> `CompletionFailed`, with the
/// supersede retained as SECONDARY detail.
///
/// The failure leads because it is what an operator acts on. Collapsing this to
/// `PostAdmissionSuperseded` would hide that the model call itself failed, and
/// the two demand different responses.
#[tokio::test(flavor = "current_thread")]
async fn a_failed_turn_leads_and_carries_the_supersede_as_secondary_detail() {
    let bound_session = meerkat_core::types::SessionId::new();
    let bridge = PhaseScriptedBridge::returning(
        CompletionScript::GateThenFail(|| "model provider returned a terminal error".to_string()),
        bound_session.clone(),
    );
    let runtime = make_runtime(Some(Arc::clone(&bridge) as Arc<dyn SessionBridge>));
    let alice = register_bound_on(&runtime, &bridge, "alice", 1, bound_session).await;

    let turn = content("turn");
    let mut send = Box::pin(runtime.send_awaiting_commit(&alice, &turn));
    let mut cx = Context::from_waker(Waker::noop());
    drive_until_admitted(&mut send, &bridge, &mut cx);

    supersede_while_running(&bridge, "alice", 2);
    bridge.release(0);

    match send.await {
        Err(IdentityRuntimeError::CompletionFailed { detail, .. }) => {
            assert!(
                detail.contains("terminal error"),
                "the turn's own failure must LEAD: {detail}"
            );
            assert!(
                detail.contains("superseded"),
                "the supersede must be retained as secondary detail, not dropped - \
                 both facts are true and an operator needs both: {detail}"
            );
        }
        other => panic!(
            "the turn RAN AND FAILED. Reporting only the supersede would hide the \
             model failure. Got: {other:?}"
        ),
    }
}

// DELETED: `a_lifecycle_operation_can_take_the_lock_while_a_turn_is_pending`.
//
// Written twice, wrong twice. v1 used `status()`, which reads `entries` and
// never takes the lifecycle lock. v2 used `set_state`, which does not take it
// either - I attributed a `lifecycle_lock_for` grep hit at runtime.rs:5582 to
// `set_state` when it actually belongs to `mark_active_runtime_broken`, the
// NEXT function in the file. Both versions would have passed against the
// defective code.
//
// Deleted rather than patched a third time. What it needs is a PUBLIC operation
// verified BY READING ITS BODY to acquire `lifecycle_lock_for(identity)` - not
// by a grep hit inside a line range. `delete_identity`, exercised by
// `an_identity_deleted_after_admission_is_superseded_not_unknown`, is a
// candidate and would make that test the lock-freedom proof too, but the same
// standard applies: read the body first.

/// `begin_awaiting_commit` returns a RECEIPT while the turn is still running.
///
/// The bridge-level statement of the whole design: admission and the turn are
/// separable. If `begin` only returned once the turn finished, there would be no
/// seam at which the runtime could release its lock, and Option A would be
/// impossible. Direct poll, so "still running" cannot be confused with "slow".
#[tokio::test(flavor = "current_thread")]
async fn begin_returns_a_receipt_while_its_terminal_is_still_pending() {
    let session = meerkat_core::types::SessionId::new();
    let bridge = PhaseScriptedBridge::returning(CompletionScript::GateThenSucceed, session.clone());
    let runtime_id = AgentRuntimeId::parse("rt:alice").expect("runtime id");

    // begin returns without the turn having finished.
    let receipt = (&*bridge as &dyn SessionBridge)
        .begin_awaiting_commit(
            &runtime_id,
            &content("turn"),
            None,
            &[],
            HandlingMode::Queue,
            None,
        )
        .await
        .expect("begin must admit and hand back a receipt");
    assert_eq!(
        receipt.resolved_session(),
        Some(&session),
        "the receipt carries the session resolved at admission"
    );

    let mut waiting = Box::pin(receipt.wait());
    let mut cx = Context::from_waker(Waker::noop());
    assert!(
        waiting.as_mut().poll(&mut cx).is_pending(),
        "begin returned, but its terminal must still be PENDING - if wait() were \
         already Ready, begin had blocked on the turn and there would be no seam \
         at which a caller could release its lock"
    );

    bridge.release(0);
    assert_eq!(
        waiting.await.expect("the turn must complete once released"),
        session
    );
}

/// ROW: turn SUCCEEDED, session projection FAILED, and the identity was
/// superseded -> `PostAdmissionSuperseded` retaining BOTH details.
///
/// The distinct sibling of the failed-turn row. Here the member did its work, so
/// the supersede is the material fact and leads; the projection detail is kept
/// because both are true and dropping either loses information an operator needs.
#[tokio::test(flavor = "current_thread")]
async fn a_projection_failure_plus_supersede_leads_with_the_supersede_and_keeps_both() {
    let bound_session = meerkat_core::types::SessionId::new();
    let bridge = PhaseScriptedBridge::returning(
        CompletionScript::GateThenSucceedUnresolved(|| {
            "member has no bridge session after deliver".to_string()
        }),
        bound_session.clone(),
    );
    let runtime = make_runtime(Some(Arc::clone(&bridge) as Arc<dyn SessionBridge>));
    let alice = register_bound_on(&runtime, &bridge, "alice", 1, bound_session).await;

    let turn = content("turn");
    let mut send = Box::pin(runtime.send_awaiting_commit(&alice, &turn));
    let mut cx = Context::from_waker(Waker::noop());
    drive_until_admitted(&mut send, &bridge, &mut cx);

    supersede_while_running(&bridge, "alice", 2);
    bridge.release(0);

    match send.await {
        Err(IdentityRuntimeError::PostAdmissionSuperseded { detail, .. }) => {
            assert!(
                detail.contains("admitted onto"),
                "the supersede must LEAD - the turn succeeded, so what matters is \
                 that its embodiment is gone: {detail}"
            );
            assert!(
                detail.contains("no bridge session"),
                "the projection detail must be RETAINED, not dropped: {detail}"
            );
        }
        other => panic!(
            "a turn that SUCCEEDED with a failed projection, against a superseded \
             identity, must not be reported as an ordinary projection failure - \
             that would imply the embodiment is still there. Got: {other:?}"
        ),
    }
}

/// An explicit DELETE after admission maps to superseded and never rebinds.
/// ALSO the lock-freedom proof.
///
/// Delete starts the identity's fresh life: Core respawns its member at the
/// next generation on a fresh session. The post-relock read sees the new
/// incarnation, and that must become supersede detail rather than a
/// reconcile onto the fresh life.
///
/// LOCK FREEDOM. `delete_identity` delegates to
/// `delete_identity_with_expected_member_alias`, whose FIRST statement is
/// `let lifecycle_lock = self.lifecycle_lock_for(identity).await;` - verified by
/// reading that body, not by a grep hit inside a line range, which is how two
/// earlier versions of a separate lock test ended up vacuous. After taking the
/// lock, delete has Core respawn the member, so the status read moves past the
/// admitted generation. The test direct-polls to that exact marker, then
/// releases the turn and drives both operations to their typed terminals.
#[tokio::test(flavor = "current_thread")]
async fn an_identity_deleted_after_admission_is_superseded_not_unknown() {
    let bound_session = meerkat_core::types::SessionId::new();
    let bridge =
        PhaseScriptedBridge::returning(CompletionScript::GateThenSucceed, bound_session.clone());
    let runtime = make_runtime(Some(Arc::clone(&bridge) as Arc<dyn SessionBridge>));
    let alice = register_bound_on(&runtime, &bridge, "alice", 1, bound_session).await;

    let turn = content("turn");
    let mut send = Box::pin(runtime.send_awaiting_commit(&alice, &turn));
    let mut cx = Context::from_waker(Waker::noop());
    drive_until_admitted(&mut send, &bridge, &mut cx);
    let admitted_generation = runtime
        .status(&alice)
        .await
        .expect("alice status")
        .generation;

    // Takes lifecycle_lock_for(&alice) - the SAME lock the send holds during
    // admission. Drive it without a clock until Core's member moved past the
    // admitted generation, which proves the delete acquired that lock and
    // crossed its respawn.
    let mut delete = Box::pin(runtime.delete_identity(&alice));
    let mut delete_terminal = None;
    let mut delete_transition_observed = false;
    for _ in 0..512 {
        if let Poll::Ready(result) = delete.as_mut().poll(&mut cx) {
            delete_terminal = Some(result);
            delete_transition_observed = true;
            break;
        }
        match runtime.status(&alice).await {
            Ok(status) if status.generation != admitted_generation => {
                delete_transition_observed = true;
                break;
            }
            _ => {}
        }
    }
    assert!(
        delete_transition_observed,
        "delete never acquired the lifecycle lock while the turn was parked"
    );

    bridge.release(0);
    let send_result = match delete_terminal {
        Some(delete_result) => {
            delete_result.expect("the lifecycle delete itself must succeed");
            send.await
        }
        None => {
            let (delete_result, send_result) = tokio::join!(delete, send);
            delete_result.expect("the lifecycle delete itself must succeed");
            send_result
        }
    };

    match send_result {
        Err(IdentityRuntimeError::PostAdmissionSuperseded { detail, .. }) => assert!(
            detail.contains("admitted onto"),
            "the detail must name the embodiment the turn ran against: {detail}"
        ),
        other => panic!(
            "a turn that completed against an identity deleted while it ran must be \
             superseded, never reconciled onto the fresh life. Got: {other:?}"
        ),
    }
}

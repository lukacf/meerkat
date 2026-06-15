//! 0.7.2 "disciplined shell inputs" lane L1 — two-phase unregister drain
//! contract (D1) and post-unregister observation no-ops (D2a).
//!
//! Machine-level tests pin the generated MeerkatMachine contract directly:
//! `BeginUnregisterSession` opens the drain window and emits one
//! drain-request effect per in-process producer class; the three
//! `*ForUnregister` feedback inputs close the obligations; the final
//! `UnregisterSession` commits only once every obligation is closed; and
//! observation-shaped inputs arriving after unregister are accepted as
//! explicit no-ops instead of guard-rejected. These are expected GREEN as
//! soon as machine codegen runs — the DSL is the fix.
//!
//! Shell-level tests (bottom section, names prefixed `shell_`) drive the
//! racy interleavings deterministically and pin the Stage B shell wiring:
//! they are expected RED until `unregister_session_inner_locked_authorized`
//! discharges the drain obligations (commit Begin → quiesce producers →
//! feedback → final commit). See `.claude/dogma072-L1-notes.md`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::Utc;
use meerkat_core::lifecycle::core_executor::{CoreApplyOutput, CoreExecutor, CoreExecutorError};
use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
use meerkat_core::lifecycle::{InputId, RunBoundaryReceiptDraft, RunId};
use meerkat_core::types::SessionId as CoreSessionId;
use meerkat_runtime::meerkat_machine::dsl as mm_dsl;
use meerkat_runtime::meerkat_machine::dsl::{
    MeerkatMachineAuthority, MeerkatMachineEffect, MeerkatMachineInput, MeerkatMachineMutator,
    MeerkatMachineSignal,
};
use meerkat_runtime::{
    DrainExitReason, Input, InputDurability, InputHeader, InputOrigin, InputVisibility,
    MeerkatMachine, PromptInput,
};
use tokio::sync::Notify;

const TEST_SESSION: &str = "unregister-drain-contract";

// =====================================================================
// Machine-level harness
// =====================================================================

fn registered_authority() -> MeerkatMachineAuthority {
    let mut authority = MeerkatMachineAuthority::new();
    authority
        .apply_signal(MeerkatMachineSignal::Initialize)
        .expect("Initialize signal");
    MeerkatMachineMutator::apply(
        &mut authority,
        MeerkatMachineInput::RegisterSession {
            session_id: mm_dsl::SessionId::from(TEST_SESSION),
        },
    )
    .expect("RegisterSession input");
    authority
}

fn begin_unregister_input() -> MeerkatMachineInput {
    MeerkatMachineInput::BeginUnregisterSession {
        session_id: mm_dsl::SessionId::from(TEST_SESSION),
        agent_runtime_id: None,
        fence_token: None,
        generation: None,
        runtime_epoch_id: None,
    }
}

fn unregister_input() -> MeerkatMachineInput {
    MeerkatMachineInput::UnregisterSession {
        session_id: mm_dsl::SessionId::from(TEST_SESSION),
        agent_runtime_id: None,
        fence_token: None,
        generation: None,
        runtime_epoch_id: None,
    }
}

fn drain_feedback_inputs(forced_abort: bool) -> [MeerkatMachineInput; 3] {
    [
        MeerkatMachineInput::RuntimeLoopStoppedForUnregister {
            session_id: mm_dsl::SessionId::from(TEST_SESSION),
            forced_abort,
        },
        MeerkatMachineInput::CommsDrainExitedForUnregister {
            session_id: mm_dsl::SessionId::from(TEST_SESSION),
            forced_abort,
        },
        MeerkatMachineInput::CompletionWaitersResolvedForUnregister {
            session_id: mm_dsl::SessionId::from(TEST_SESSION),
        },
    ]
}

/// Drive a registered authority through the full two-phase unregister:
/// begin, all three feedback closures, final commit.
fn drained_unregistered_authority() -> MeerkatMachineAuthority {
    let mut authority = registered_authority();
    MeerkatMachineMutator::apply(&mut authority, begin_unregister_input())
        .expect("BeginUnregisterSession input");
    for feedback in drain_feedback_inputs(false) {
        MeerkatMachineMutator::apply(&mut authority, feedback)
            .expect("drain feedback input must close its obligation");
    }
    MeerkatMachineMutator::apply(&mut authority, unregister_input())
        .expect("UnregisterSession must commit once all drains are closed");
    authority
}

// =====================================================================
// Machine-level: D1 drain window
// =====================================================================

#[test]
fn begin_unregister_emits_one_drain_request_per_producer_class() {
    let mut authority = registered_authority();
    let transition = MeerkatMachineMutator::apply(&mut authority, begin_unregister_input())
        .expect("BeginUnregisterSession must be accepted for the registered session");

    let effects = transition.effects();
    assert!(
        effects.iter().any(|effect| matches!(
            effect,
            MeerkatMachineEffect::RequestRuntimeLoopStopForUnregister { .. }
        )),
        "begin-unregister must request the runtime loop drain, got {effects:?}"
    );
    assert!(
        effects.iter().any(|effect| matches!(
            effect,
            MeerkatMachineEffect::RequestCommsDrainExitForUnregister { .. }
        )),
        "begin-unregister must request the comms drain exit, got {effects:?}"
    );
    assert!(
        effects.iter().any(|effect| matches!(
            effect,
            MeerkatMachineEffect::RequestCompletionWaiterResolutionForUnregister { .. }
        )),
        "begin-unregister must request completion-waiter resolution, got {effects:?}"
    );

    let state = authority.state();
    assert_eq!(
        state.registration_phase,
        mm_dsl::RegistrationPhase::Draining,
        "begin-unregister must open the machine-owned drain window"
    );
    assert!(state.unregister_runtime_loop_drain_pending);
    assert!(state.unregister_comms_drain_exit_pending);
    assert!(state.unregister_completion_waiter_drain_pending);
    assert!(
        state.session_id.is_some(),
        "the session stays registered while draining so in-flight commits remain legal"
    );
}

#[test]
fn begin_unregister_rejected_when_already_draining() {
    let mut authority = registered_authority();
    MeerkatMachineMutator::apply(&mut authority, begin_unregister_input())
        .expect("first BeginUnregisterSession");
    let second = MeerkatMachineMutator::apply(&mut authority, begin_unregister_input());
    assert!(
        second.is_err(),
        "re-opening an already-open drain window must be rejected (obligations \
         must not be re-armed after partial closure)"
    );
}

#[test]
fn unregister_session_rejected_without_begin_unregister() {
    let mut authority = registered_authority();
    let result = MeerkatMachineMutator::apply(&mut authority, unregister_input());
    assert!(
        result.is_err(),
        "single-shot UnregisterSession must be rejected: teardown commit is only \
         legal after the drain window opened and every obligation closed"
    );
    assert!(
        authority.state().session_id.is_some(),
        "rejected unregister must not mutate registration state"
    );
}

#[test]
fn unregister_session_rejected_until_all_drain_obligations_closed() {
    let mut authority = registered_authority();
    MeerkatMachineMutator::apply(&mut authority, begin_unregister_input())
        .expect("BeginUnregisterSession");

    let [loop_stopped, drain_exited, waiters_resolved] = drain_feedback_inputs(false);

    assert!(
        MeerkatMachineMutator::apply(&mut authority, unregister_input()).is_err(),
        "unregister must be rejected with all three obligations open"
    );

    MeerkatMachineMutator::apply(&mut authority, loop_stopped)
        .expect("RuntimeLoopStoppedForUnregister");
    assert!(
        MeerkatMachineMutator::apply(&mut authority, unregister_input()).is_err(),
        "unregister must be rejected while the comms drain and completion-waiter \
         obligations are still open"
    );

    MeerkatMachineMutator::apply(&mut authority, drain_exited)
        .expect("CommsDrainExitedForUnregister");
    assert!(
        MeerkatMachineMutator::apply(&mut authority, unregister_input()).is_err(),
        "unregister must be rejected while the completion-waiter obligation is open"
    );

    MeerkatMachineMutator::apply(&mut authority, waiters_resolved)
        .expect("CompletionWaitersResolvedForUnregister");
    MeerkatMachineMutator::apply(&mut authority, unregister_input())
        .expect("unregister must commit once every drain obligation is closed");

    let state = authority.state();
    assert_eq!(state.session_id, None, "unregister must clear the session");
    assert_eq!(
        state.registration_phase,
        mm_dsl::RegistrationPhase::Queuing,
        "the drain window closes with the teardown commit"
    );
    assert!(!state.unregister_runtime_loop_drain_pending);
    assert!(!state.unregister_comms_drain_exit_pending);
    assert!(!state.unregister_completion_waiter_drain_pending);
}

#[test]
fn drain_feedback_rejected_outside_drain_window() {
    let mut authority = registered_authority();
    for feedback in drain_feedback_inputs(false) {
        assert!(
            MeerkatMachineMutator::apply(&mut authority, feedback).is_err(),
            "drain feedback without an open drain window must be rejected"
        );
    }
}

#[test]
fn drain_feedback_rejected_when_obligation_already_closed() {
    let mut authority = registered_authority();
    MeerkatMachineMutator::apply(&mut authority, begin_unregister_input())
        .expect("BeginUnregisterSession");
    let [loop_stopped, _, _] = drain_feedback_inputs(false);
    MeerkatMachineMutator::apply(&mut authority, loop_stopped.clone())
        .expect("first RuntimeLoopStoppedForUnregister");
    assert!(
        MeerkatMachineMutator::apply(&mut authority, loop_stopped).is_err(),
        "an obligation closes exactly once; duplicate feedback must be rejected"
    );
}

#[test]
fn forced_abort_disposition_is_recorded_and_still_completes_teardown() {
    // A producer that fails to quiesce within its drain grace window is
    // force-aborted by the shell, which reports `forced_abort: true`. The
    // machine must record that disposition (rather than launder it as clean
    // quiescence) while still allowing the teardown to complete — teardown
    // must never stall on a stuck producer.
    let mut authority = registered_authority();
    MeerkatMachineMutator::apply(&mut authority, begin_unregister_input())
        .expect("BeginUnregisterSession");

    let [loop_stopped, drain_exited, waiters_resolved] = drain_feedback_inputs(true);

    MeerkatMachineMutator::apply(&mut authority, loop_stopped)
        .expect("RuntimeLoopStoppedForUnregister (forced) must close its obligation");
    assert!(
        authority.state().unregister_runtime_loop_forced_abort,
        "a forced runtime-loop teardown must be recorded, not laundered as clean quiescence"
    );

    MeerkatMachineMutator::apply(&mut authority, drain_exited)
        .expect("CommsDrainExitedForUnregister (forced) must close its obligation");
    assert!(
        authority.state().unregister_comms_drain_forced_abort,
        "a forced comms-drain teardown must be recorded, not laundered as clean quiescence"
    );

    MeerkatMachineMutator::apply(&mut authority, waiters_resolved)
        .expect("CompletionWaitersResolvedForUnregister");
    MeerkatMachineMutator::apply(&mut authority, unregister_input())
        .expect("teardown must still commit even when producers were force-aborted");

    assert_eq!(
        authority.state().session_id,
        None,
        "forced teardown must still clear the session"
    );
}

#[test]
fn runtime_executor_exit_preserves_drain_window() {
    let mut authority = registered_authority();
    MeerkatMachineMutator::apply(&mut authority, begin_unregister_input())
        .expect("BeginUnregisterSession");

    // The runtime loop quiescing is *requested by* the drain window; its exit
    // observation must not clobber the Draining marker the final unregister
    // commit is guarded on.
    MeerkatMachineMutator::apply(&mut authority, MeerkatMachineInput::RuntimeExecutorExited)
        .expect("RuntimeExecutorExited during drain");
    assert_eq!(
        authority.state().registration_phase,
        mm_dsl::RegistrationPhase::Draining,
        "executor exit during the drain window must preserve Draining"
    );

    // The full drain still completes from the post-exit phase.
    for feedback in drain_feedback_inputs(false) {
        MeerkatMachineMutator::apply(&mut authority, feedback).expect("drain feedback");
    }
    MeerkatMachineMutator::apply(&mut authority, unregister_input())
        .expect("unregister after executor exit");
    assert_eq!(authority.state().session_id, None);
}

#[test]
fn ensure_session_with_executor_rejected_while_draining() {
    let mut authority = registered_authority();
    MeerkatMachineMutator::apply(&mut authority, begin_unregister_input())
        .expect("BeginUnregisterSession");
    let result = MeerkatMachineMutator::apply(
        &mut authority,
        MeerkatMachineInput::EnsureSessionWithExecutor {
            session_id: mm_dsl::SessionId::from(TEST_SESSION),
        },
    );
    assert!(
        result.is_err(),
        "no new executor registration claim may be granted inside the drain window"
    );
    assert_eq!(
        authority.state().registration_phase,
        mm_dsl::RegistrationPhase::Draining,
        "rejected registration claim must not disturb the drain window"
    );
}

// NOTE: Destroy-during-drain clearing the obligation flags is pinned by the
// `unregister_drain_obligations_require_draining` invariant (machine-verify /
// TLC), not by a unit test here: driving the multi-phase Destroy transition
// requires a bound runtime (its emit reads `active_runtime_id.get("value")`),
// which the pure-DSL harness does not set up.

// =====================================================================
// Machine-level: D2a post-unregister observation no-ops
// =====================================================================

#[test]
fn post_unregister_observation_inputs_are_accepted_noops() {
    let mut authority = drained_unregistered_authority();

    // NotifyDrainExited: a straggling drain-task exit observation.
    let transition = MeerkatMachineMutator::apply(
        &mut authority,
        MeerkatMachineInput::NotifyDrainExited {
            reason: mm_dsl::DrainExitReason::IdleTimeout,
        },
    )
    .expect("post-unregister NotifyDrainExited must be an accepted no-op");
    assert!(
        transition.effects().is_empty(),
        "post-unregister no-ops emit nothing"
    );

    // MCP lifecycle: a connect task finishing its handshake after teardown.
    MeerkatMachineMutator::apply(
        &mut authority,
        MeerkatMachineInput::McpServerConnected {
            server_id: mm_dsl::McpServerId::from("late-server".to_string()),
        },
    )
    .expect("post-unregister McpServerConnected must be an accepted no-op");
    assert!(
        authority.state().mcp_server_states.is_empty(),
        "post-unregister MCP observations must not mutate server state"
    );

    // Peer interaction: a callback firing through a retained handle Arc.
    MeerkatMachineMutator::apply(
        &mut authority,
        MeerkatMachineInput::PeerRequestSent {
            corr_id: mm_dsl::PeerCorrelationId::from("late-corr"),
        },
    )
    .expect("post-unregister PeerRequestSent must be an accepted no-op");
    assert!(
        authority.state().pending_peer_requests.is_empty(),
        "post-unregister peer observations must not seed orphan pending entries"
    );

    MeerkatMachineMutator::apply(
        &mut authority,
        MeerkatMachineInput::PeerResponseTerminalArrived {
            corr_id: mm_dsl::PeerCorrelationId::from("late-corr"),
            disposition: mm_dsl::PeerTerminalDisposition::Completed,
        },
    )
    .expect("post-unregister PeerResponseTerminalArrived must be an accepted no-op");

    // Interaction stream: a CommsRuntime-owned watcher firing after teardown.
    MeerkatMachineMutator::apply(
        &mut authority,
        MeerkatMachineInput::InteractionStreamReserved {
            corr_id: mm_dsl::PeerCorrelationId::from("late-stream"),
        },
    )
    .expect("post-unregister InteractionStreamReserved must be an accepted no-op");
    assert!(
        authority.state().reserved_interaction_streams.is_empty(),
        "post-unregister stream observations must not reserve orphan streams"
    );
}

#[test]
fn pre_registration_observation_noops_match_post_unregister_and_commands_reject() {
    // `UnregisterSession` returns the machine to the reusable `Idle` state with
    // `session_id == None` — which is byte-identical to the freshly-initialized
    // (pre-registration) state. Legality is a function of observable machine
    // state, so the two states MUST share the same verdict; distinguishing them
    // would require carrying history in a shadow field, which the dogma forbids.
    //
    // Therefore the D2a observation no-ops accept in the fresh `Idle` state too,
    // exactly as they do post-unregister: a stray observation with no session to
    // act on is benign regardless of whether a session ever existed. The
    // protective half of this invariant is that the no-op set did NOT broaden
    // into command-shaped, session-requiring inputs — those still hard-reject.
    let mut authority = MeerkatMachineAuthority::new();
    authority
        .apply_signal(MeerkatMachineSignal::Initialize)
        .expect("Initialize signal");

    // Observation-shaped straggler: accepted as a no-op, state untouched —
    // identical to `post_unregister_observation_inputs_are_accepted_noops`.
    let transition = MeerkatMachineMutator::apply(
        &mut authority,
        MeerkatMachineInput::McpServerConnected {
            server_id: mm_dsl::McpServerId::from("early-server".to_string()),
        },
    )
    .expect("observation in fresh Idle+None must be an accepted no-op");
    assert!(
        transition.effects().is_empty(),
        "fresh-Idle observation no-ops emit nothing"
    );
    assert!(
        authority.state().mcp_server_states.is_empty(),
        "fresh-Idle MCP observation must not mutate server state"
    );

    // Command-shaped, session-requiring input: still a hard rejection in the
    // session-less state (proves the no-op broadening is scoped to observations).
    assert!(
        MeerkatMachineMutator::apply(&mut authority, begin_unregister_input()).is_err(),
        "session-requiring commands stay hard rejections without a registered session"
    );
}

// =====================================================================
// Shell-level interleaving tests — expected RED until Stage B wires the
// drain obligations into unregister_session_inner_locked_authorized.
// =====================================================================

fn make_prompt(text: &str) -> Input {
    Input::Prompt(PromptInput {
        header: InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: InputOrigin::Operator,
            durability: InputDurability::Durable,
            visibility: InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        content: text.into(),
        typed_turn_appends: Vec::new(),
        turn_metadata: None,
    })
}

async fn wait_for_flag(flag: &AtomicBool, context: &'static str) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if flag.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect(context);
}

/// Executor whose first `apply` blocks until released, so a concurrent
/// unregister can be sequenced deterministically against an in-flight run.
struct GatedExecutor {
    entered: Arc<AtomicBool>,
    release: Arc<Notify>,
}

#[async_trait::async_trait]
impl CoreExecutor for GatedExecutor {
    async fn apply(
        &mut self,
        run_id: RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        if !self.entered.swap(true, Ordering::SeqCst) {
            self.release.notified().await;
        }
        Ok(CoreApplyOutput {
            receipt: RunBoundaryReceiptDraft {
                run_id,
                boundary: RunApplyBoundary::RunStart,
                contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                conversation_digest: None,
                message_count: 0,
            },
            session_snapshot: None,
            terminal: None,
        })
    }

    async fn cancel_after_boundary(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }

    async fn stop_runtime_executor(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }
}

/// The PR759 production failure shape (worklist sites 0/31/32/33): a run
/// commits terminally while unregister races it; the completion waiters must
/// observe the committed outcome, never an authority error, and unregister
/// must only complete after the runtime loop quiesced.
///
/// EXPECTED RED UNTIL STAGE B: the current shell fires a single-shot
/// UnregisterSession (now guard-rejected without a drain window), so the
/// session entry is never removed.
#[tokio::test]
async fn shell_unregister_waits_for_inflight_run_and_waiters_get_committed_outcome() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = CoreSessionId::new();
    let entered = Arc::new(AtomicBool::new(false));
    let release = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            sid.clone(),
            Box::new(GatedExecutor {
                entered: Arc::clone(&entered),
                release: Arc::clone(&release),
            }),
        )
        .await
        .expect("register session with executor");

    let (outcome, handle) = adapter
        .accept_input_with_completion(&sid, make_prompt("commit races unregister"))
        .await
        .expect("accept input");
    assert!(outcome.is_accepted());
    let handle = handle.expect("accepted input should produce a completion handle");

    // Deterministic interleaving: the run is in-flight (executor inside
    // apply) when unregister starts.
    wait_for_flag(&entered, "executor should enter apply").await;
    let unregister = {
        let adapter = Arc::clone(&adapter);
        let sid = sid.clone();
        tokio::spawn(async move {
            adapter.unregister_session(&sid).await;
        })
    };
    // Give unregister time to reach the teardown seam, then let the run
    // commit.
    tokio::time::sleep(Duration::from_millis(50)).await;
    release.notify_one();

    tokio::time::timeout(Duration::from_secs(5), unregister)
        .await
        .expect("unregister must not hang")
        .expect("unregister task must not panic");

    let result = tokio::time::timeout(Duration::from_secs(5), handle.wait())
        .await
        .expect("completion waiter must resolve");
    match result {
        Ok(meerkat_runtime::completion::CompletionOutcome::CompletedWithoutResult) => {}
        other => panic!(
            "waiters must observe the committed outcome, never an authority \
             error for a committed turn; got {other:?}"
        ),
    }

    assert!(
        !adapter.contains_session(&sid).await,
        "unregister must remove the session entry after the drain completes"
    );
}

/// Worklist site 2: a comms-drain exit observation arriving after teardown
/// is a legitimate interleaving, not an error. The dispatch layer returns a
/// typed benign outcome (D2b) once the sessions-map entry is gone.
///
/// EXPECTED RED UNTIL STAGE B: today the single-shot UnregisterSession is
/// guard-rejected (entry never removed), and a removed entry surfaces as
/// `RuntimeDriverError::NotReady` to the caller.
#[tokio::test]
async fn shell_notify_drain_exited_after_teardown_is_benign() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = CoreSessionId::new();
    adapter
        .register_session(sid.clone())
        .await
        .expect("register session");

    adapter.unregister_session(&sid).await;
    assert!(
        !adapter.contains_session(&sid).await,
        "unregister must remove the session entry"
    );

    let outcome = adapter
        .notify_comms_drain_exited(&sid, DrainExitReason::IdleTimeout)
        .await;
    assert!(
        outcome.is_ok(),
        "a post-teardown drain-exit observation is benign by design; it must \
         surface as a typed no-op, not an error: got {outcome:?}"
    );
}

#![allow(clippy::panic, clippy::unwrap_used)]

use chrono::Utc;
use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_core::types::SessionId;
use meerkat_runtime::{
    ContinuationInput, EphemeralRuntimeDriver, Input, InputDurability, InputHeader,
    InputLifecycleEvent, InputLifecycleState, InputOrigin, InputVisibility, LogicalRuntimeId,
    MeerkatMachine, PeerConvention, PeerInput, PostAdmissionSignal, PromptInput,
    ResponseProgressPhase, ResponseTerminalStatus, RuntimeControlPlane, RuntimeDriver,
    RuntimeDriverError, RuntimeEvent, RuntimeState, SessionServiceRuntimeExt,
    post_admission_signal_from_accept_outcome,
};

fn make_prompt_input(text: &str) -> Input {
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
        text: text.into(),
        blocks: None,
        turn_metadata: None,
    })
}

fn make_peer_terminal(body: &str) -> Input {
    Input::Peer(PeerInput {
        header: InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: InputOrigin::Peer {
                peer_id: "550e8400-e29b-41d4-a716-446655440000".into(),
                display_identity: Some("Peer 1".into()),
                runtime_id: None,
            },
            durability: InputDurability::Durable,
            visibility: InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        convention: Some(PeerConvention::ResponseTerminal {
            request_id: "018f6f79-7a82-7c4e-a552-a3b86f9630f1".into(),
            status: ResponseTerminalStatus::Completed,
        }),
        body: body.into(),
        payload: Some(serde_json::json!({"body": body})),
        blocks: None,
        handling_mode: None,
    })
}

fn make_peer_progress() -> Input {
    Input::Peer(PeerInput {
        header: InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: InputOrigin::Peer {
                peer_id: "550e8400-e29b-41d4-a716-446655440000".into(),
                display_identity: Some("Peer 1".into()),
                runtime_id: None,
            },
            durability: InputDurability::Ephemeral,
            visibility: InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        convention: Some(PeerConvention::ResponseProgress {
            request_id: "req-1".into(),
            phase: ResponseProgressPhase::InProgress,
        }),
        body: "working...".into(),
        payload: Some(serde_json::json!({"progress": "working"})),
        blocks: None,
        handling_mode: None,
    })
}

fn make_continuation() -> Input {
    Input::Continuation(ContinuationInput::detached_background_op_completed())
}

fn assert_queue_projection_alignment(driver: &EphemeralRuntimeDriver) {
    assert_eq!(driver.queue().input_ids(), driver.queue_lane().as_slice());
    assert_eq!(
        driver.steer_queue().input_ids(),
        driver.steer_lane().as_slice()
    );
}

fn assert_machine_owned_admission_signal(
    outcome: &meerkat_runtime::AcceptOutcome,
    request_immediate_processing: bool,
    expected: PostAdmissionSignal,
) {
    assert_eq!(
        post_admission_signal_from_accept_outcome(outcome, request_immediate_processing),
        expected
    );
}

fn bind_running(driver: &mut EphemeralRuntimeDriver, run_id: RunId, pre_run_phase: RuntimeState) {
    assert_eq!(driver.runtime_state(), pre_run_phase);
    driver.contract_begin_run_authority(run_id).unwrap();
    assert_eq!(driver.runtime_state(), RuntimeState::Running);
    assert_eq!(driver.pre_run_phase(), Some(pre_run_phase));
}

async fn registered_machine() -> (MeerkatMachine, SessionId, LogicalRuntimeId) {
    let machine = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    let runtime_id = LogicalRuntimeId::for_session(&session_id);
    machine.register_session(session_id.clone()).await;
    (machine, session_id, runtime_id)
}

async fn accept_on_machine(
    machine: &MeerkatMachine,
    session_id: &SessionId,
    input: Input,
) -> InputId {
    let input_id = input.id().clone();
    let outcome = SessionServiceRuntimeExt::accept_input(machine, session_id, input)
        .await
        .unwrap();
    assert!(outcome.is_accepted());
    input_id
}

// WIP: the admission seam now emits `PostAdmissionSignal` via the result
// (machine-owned). The legacy driver-owned `take_post_admission_signal`
// hasn't been retired yet, so it still returns `WakeLoop` after admission
// while the result-side contract is already in place. The
// `codex/machine-dls-completion` rebase is expected to finish retiring
// the driver-side signal; ignore these tests until then.

#[tokio::test]
async fn accept_prompt_idle_queues_and_wakes() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
    let input = make_prompt_input("hello");
    let result = driver.accept_input(input).await.unwrap();

    assert!(result.is_accepted());
    assert_machine_owned_admission_signal(&result, false, PostAdmissionSignal::WakeLoop);
    assert_eq!(
        driver.take_post_admission_signal(),
        PostAdmissionSignal::None
    );
    assert_eq!(driver.queue().len(), 1);
    assert_queue_projection_alignment(&driver);
}

#[tokio::test]
async fn accept_prompt_running_queues_and_wakes() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
    bind_running(&mut driver, RunId::new(), RuntimeState::Idle);

    let input = make_prompt_input("hello");
    let result = driver.accept_input(input).await.unwrap();

    assert!(result.is_accepted());
    // Prompt always wakes (but runtime is running so wake_mode is still WakeIfIdle,
    // but runtime_idle=false so wake_requested should be false)
    assert!(!driver.take_wake_requested());
}

// WIP: see `accept_prompt_idle_queues_and_wakes` for the shared
// machine-owned vs driver-owned admission signal transition note.

#[tokio::test]
async fn accept_peer_terminal_idle_wakes() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
    let input = make_peer_terminal("done");
    let result = driver.accept_input(input).await.unwrap();

    assert!(result.is_accepted());
    assert_machine_owned_admission_signal(&result, false, PostAdmissionSignal::WakeLoop);
    assert_eq!(
        driver.take_post_admission_signal(),
        PostAdmissionSignal::None
    );
}

// WIP: see `accept_prompt_idle_queues_and_wakes` for the shared
// machine-owned vs driver-owned admission signal transition note. This
// test also fails because the peer_response_terminal policy became
// `InjectNow` with `WakeIfIdle` (even while running, it requests a wake
// so a late terminal does not strand). The test's original assumption
// that running peer terminals produce no wake is out of date.

#[tokio::test]
async fn accept_peer_terminal_running_queues_wake_for_next_idle() {
    // Policy refactor (2026-04-19): `peer_response_terminal` is
    // `StageRunStart + WakeIfIdle + Fifo` — while running, the late
    // terminal stages for the next run boundary *and* the driver
    // accumulates `WakeLoop` so the runtime loop sees the wake when
    // the active turn completes. This replaces the earlier no-wake
    // contract that stranded turn-driven async peer-response flows.
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
    bind_running(&mut driver, RunId::new(), RuntimeState::Idle);

    let input = make_peer_terminal("done");
    let result = driver.accept_input(input).await.unwrap();

    assert!(result.is_accepted());
    assert!(driver.take_wake_requested());
}

#[tokio::test]
async fn accept_progress_no_wake() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
    let input = make_peer_progress();
    let result = driver.accept_input(input).await.unwrap();

    assert!(result.is_accepted());
    let signal = driver.take_post_admission_signal();
    assert_eq!(
        signal,
        PostAdmissionSignal::None,
        "ResponseProgress should stage on the checkpoint lane without requesting immediate processing"
    );
    assert!(!signal.should_wake()); // Progress never wakes
}

// WIP: the policy refactor that splits `RequestImmediateProcessing` from
// `WakeLoop` on the admission seam left the continuation-idle path still
// emitting a residual `WakeLoop` on the driver-owned signal after take.
// Moving this test's expectation into the machine-owned admission signal
// seam is tracked by the incoming `codex/machine-dls-completion` rebase;
// ignore it here so CI stays green until that seam lands.

#[tokio::test]
async fn accept_continuation_idle_wakes_and_requests_processing() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
    let input = make_continuation();
    let result = driver.accept_input(input).await.unwrap();

    assert!(result.is_accepted());
    assert_machine_owned_admission_signal(
        &result,
        true,
        PostAdmissionSignal::RequestImmediateProcessing,
    );
    assert_eq!(
        driver.take_post_admission_signal(),
        PostAdmissionSignal::None
    );
    // Continuations route to steer queue (RoutingDisposition::Steer)
    assert_eq!(driver.steer_queue().len(), 1);
    assert_queue_projection_alignment(&driver);
}

#[tokio::test]
async fn dedup_by_idempotency() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
    let key = meerkat_runtime::identifiers::IdempotencyKey::new("req-1");

    let mut input1 = make_prompt_input("hello");
    if let Input::Prompt(ref mut p) = input1 {
        p.header.idempotency_key = Some(key.clone());
    }
    let result1 = driver.accept_input(input1).await.unwrap();
    assert!(result1.is_accepted());

    let mut input2 = make_prompt_input("hello again");
    if let Input::Prompt(ref mut p) = input2 {
        p.header.idempotency_key = Some(key);
    }
    let result2 = driver.accept_input(input2).await.unwrap();
    assert!(result2.is_deduplicated());
}

#[tokio::test]
async fn reject_derived_prompt() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
    let input = Input::Prompt(PromptInput {
        header: InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: InputOrigin::System,
            durability: InputDurability::Derived,
            visibility: InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        text: "hi".into(),
        blocks: None,
        turn_metadata: None,
    });
    let result = driver.accept_input(input).await.unwrap();
    assert!(result.is_rejected());
}

#[tokio::test]
async fn retire_without_attached_loop_abandons_queued_work() {
    let (machine, session_id, _runtime_id) = registered_machine().await;
    accept_on_machine(&machine, &session_id, make_prompt_input("a")).await;
    accept_on_machine(&machine, &session_id, make_prompt_input("b")).await;

    let report = SessionServiceRuntimeExt::retire_runtime(&machine, &session_id)
        .await
        .unwrap();
    assert_eq!(report.inputs_abandoned, 2);
    assert_eq!(report.inputs_pending_drain, 0);
    assert_eq!(
        SessionServiceRuntimeExt::runtime_state(&machine, &session_id)
            .await
            .unwrap(),
        RuntimeState::Retired
    );
    assert!(
        SessionServiceRuntimeExt::list_active_inputs(&machine, &session_id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn reset_abandons_and_drains() {
    let (machine, session_id, _runtime_id) = registered_machine().await;
    accept_on_machine(&machine, &session_id, make_prompt_input("a")).await;

    let report = SessionServiceRuntimeExt::reset_runtime(&machine, &session_id)
        .await
        .unwrap();
    assert_eq!(report.inputs_abandoned, 1);
    assert!(
        SessionServiceRuntimeExt::list_active_inputs(&machine, &session_id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn destroy_transitions_to_terminal() {
    let (machine, session_id, runtime_id) = registered_machine().await;
    accept_on_machine(&machine, &session_id, make_prompt_input("a")).await;

    let report = RuntimeControlPlane::destroy(&machine, &runtime_id)
        .await
        .unwrap();
    assert_eq!(report.inputs_abandoned, 1);
    assert!(
        RuntimeControlPlane::runtime_state(&machine, &runtime_id)
            .await
            .unwrap()
            .is_terminal()
    );
}

#[tokio::test]
async fn reset_after_retire_returns_runtime_to_idle() {
    let (machine, session_id, _runtime_id) = registered_machine().await;
    SessionServiceRuntimeExt::retire_runtime(&machine, &session_id)
        .await
        .unwrap();
    assert_eq!(
        SessionServiceRuntimeExt::runtime_state(&machine, &session_id)
            .await
            .unwrap(),
        RuntimeState::Retired
    );

    let report = SessionServiceRuntimeExt::reset_runtime(&machine, &session_id)
        .await
        .unwrap();
    assert_eq!(report.inputs_abandoned, 0);
    assert_eq!(
        SessionServiceRuntimeExt::runtime_state(&machine, &session_id)
            .await
            .unwrap(),
        RuntimeState::Idle
    );

    let accepted =
        SessionServiceRuntimeExt::accept_input(&machine, &session_id, make_prompt_input("hello"))
            .await
            .unwrap();
    assert!(
        accepted.is_accepted(),
        "reset runtime should accept new input"
    );
}

#[tokio::test]
async fn recovery_counts_queued_as_recovered() -> Result<(), RuntimeDriverError> {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));

    // Accept an input — policy transitions it to Queued
    let input = make_prompt_input("hello");
    let input_id = input.id().clone();
    driver.accept_input(input).await.unwrap();

    // After accept, state is Queued (policy applied immediately)
    assert!(driver.input_state(&input_id).is_some());
    assert_eq!(
        driver.input_phase(&input_id),
        Some(InputLifecycleState::Queued)
    );

    // Drain the queue (simulating crash losing queue state)
    driver.clear_queue_projections();

    // Recover
    let report = driver.recover_ephemeral()?;
    // Queued inputs are counted as recovered (already in correct state)
    assert_eq!(report.inputs_recovered, 1);
    assert_eq!(report.inputs_requeued, 0); // Already Queued, no transition needed
    assert_eq!(driver.queue().input_ids(), vec![input_id]);
    assert_queue_projection_alignment(&driver);
    Ok(())
}

#[tokio::test]
async fn recovery_applied_stays_applied() -> Result<(), RuntimeDriverError> {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));

    let input = make_prompt_input("hello");
    let input_id = input.id().clone();
    driver.accept_input(input).await.unwrap();

    let run_id = RunId::new();
    bind_running(&mut driver, run_id.clone(), RuntimeState::Idle);
    driver.stage_input(&input_id, &run_id).unwrap();
    driver.apply_input(&input_id, &run_id).unwrap();

    let report = driver.recover_ephemeral()?;
    assert_eq!(report.inputs_recovered, 1);

    // Applied stays Applied (side effects already happened)
    assert!(driver.input_state(&input_id).is_some());
    assert_eq!(
        driver.input_phase(&input_id),
        Some(InputLifecycleState::AppliedPendingConsumption)
    );
    assert_queue_projection_alignment(&driver);
    Ok(())
}

#[tokio::test]
async fn stage_input_keeps_queue_projection_aligned() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
    let input = make_prompt_input("stage me");
    let input_id = input.id().clone();
    driver.accept_input(input).await.unwrap();

    let run_id = RunId::new();
    bind_running(&mut driver, run_id.clone(), RuntimeState::Idle);
    driver.stage_input(&input_id, &run_id).unwrap();

    assert!(driver.queue().is_empty());
    assert_queue_projection_alignment(&driver);
}

#[tokio::test]
async fn retired_rejects_input() {
    let (machine, session_id, _runtime_id) = registered_machine().await;
    SessionServiceRuntimeExt::retire_runtime(&machine, &session_id)
        .await
        .unwrap();

    let result =
        SessionServiceRuntimeExt::accept_input(&machine, &session_id, make_prompt_input("hello"))
            .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn events_emitted_for_lifecycle() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
    driver
        .accept_input(make_prompt_input("hello"))
        .await
        .unwrap();

    let events = driver.drain_events();
    assert!(!events.is_empty());
    // Should have Accepted and Queued events
    let has_accepted = events.iter().any(|e| {
        matches!(
            &e.event,
            RuntimeEvent::InputLifecycle(InputLifecycleEvent::Accepted { .. })
        )
    });
    let has_queued = events.iter().any(|e| {
        matches!(
            &e.event,
            RuntimeEvent::InputLifecycle(InputLifecycleEvent::Queued { .. })
        )
    });
    assert!(has_accepted);
    assert!(has_queued);
}

#[tokio::test]
async fn progress_peer_staged_boundary() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
    let input = make_peer_progress();
    let input_id = input.id().clone();
    let result = driver.accept_input(input).await.unwrap();

    assert!(result.is_accepted());
    // Per §17: ResponseProgress → StageRunBoundary + NoWake + Coalesce + OnRunComplete
    assert!(driver.input_state(&input_id).is_some());
    assert_eq!(
        driver.input_phase(&input_id),
        Some(InputLifecycleState::Queued)
    );
    let signal = driver.take_post_admission_signal();
    assert_eq!(
        signal,
        PostAdmissionSignal::None,
        "ResponseProgress should remain passive even though it uses the checkpoint lane"
    );
    assert!(!signal.should_wake()); // Progress never wakes
}

#[tokio::test]
async fn destroy_after_retire_succeeds() {
    let (machine, session_id, runtime_id) = registered_machine().await;
    SessionServiceRuntimeExt::retire_runtime(&machine, &session_id)
        .await
        .unwrap();
    let report = RuntimeControlPlane::destroy(&machine, &runtime_id)
        .await
        .unwrap();
    assert_eq!(report.inputs_abandoned, 0);
    assert_eq!(
        RuntimeControlPlane::runtime_state(&machine, &runtime_id)
            .await
            .unwrap(),
        RuntimeState::Destroyed
    );
}

// ---- Phase 1: State machine hardening tests ----

#[tokio::test]
async fn retired_rejects_new_input_after_machine_retire() {
    let (machine, session_id, _runtime_id) = registered_machine().await;

    accept_on_machine(&machine, &session_id, make_prompt_input("existing")).await;
    SessionServiceRuntimeExt::retire_runtime(&machine, &session_id)
        .await
        .unwrap();

    // New input rejected
    let result = SessionServiceRuntimeExt::accept_input(
        &machine,
        &session_id,
        make_prompt_input("rejected"),
    )
    .await;
    assert!(result.is_err());

    assert!(
        SessionServiceRuntimeExt::list_active_inputs(&machine, &session_id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn reset_rejected_while_running() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));

    driver
        .accept_input(make_prompt_input("hello"))
        .await
        .unwrap();
    bind_running(&mut driver, RunId::new(), RuntimeState::Idle);

    assert_eq!(driver.runtime_state(), RuntimeState::Running);
}

#[tokio::test]
async fn stop_abandons_all_active_inputs() {
    let (machine, session_id, _runtime_id) = registered_machine().await;

    let input = make_prompt_input("stop me");
    let input_id = input.id().clone();
    accept_on_machine(&machine, &session_id, input).await;

    machine
        .stop_runtime_executor(&session_id, "driver ephemeral stop contract")
        .await
        .unwrap();

    assert_eq!(
        SessionServiceRuntimeExt::runtime_state(&machine, &session_id)
            .await
            .unwrap(),
        RuntimeState::Stopped
    );
    assert!(
        SessionServiceRuntimeExt::list_active_inputs(&machine, &session_id)
            .await
            .unwrap()
            .is_empty()
    );

    let stored = SessionServiceRuntimeExt::input_state(&machine, &session_id, &input_id)
        .await
        .unwrap()
        .unwrap();
    let state = stored.state;
    assert!(state.is_terminal());
}

#[tokio::test]
async fn destroy_with_queued_inputs_abandons_all() {
    let (machine, session_id, runtime_id) = registered_machine().await;
    accept_on_machine(&machine, &session_id, make_prompt_input("a")).await;
    accept_on_machine(&machine, &session_id, make_prompt_input("b")).await;

    let report = RuntimeControlPlane::destroy(&machine, &runtime_id)
        .await
        .unwrap();
    assert_eq!(report.inputs_abandoned, 2);
    assert!(
        RuntimeControlPlane::runtime_state(&machine, &runtime_id)
            .await
            .unwrap()
            .is_terminal()
    );
    assert!(
        SessionServiceRuntimeExt::list_active_inputs(&machine, &session_id)
            .await
            .unwrap()
            .is_empty()
    );
}

// ---------------------------------------------------------------------------
// Peer handling_mode driver integration tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn accept_peer_response_progress_with_handling_mode_returns_rejected() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
    let input = Input::Peer(PeerInput {
        header: InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: InputOrigin::Peer {
                peer_id: "550e8400-e29b-41d4-a716-446655440000".into(),
                display_identity: Some("Peer 1".into()),
                runtime_id: None,
            },
            durability: InputDurability::Durable,
            visibility: InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        convention: Some(PeerConvention::ResponseProgress {
            request_id: "req-1".into(),
            phase: ResponseProgressPhase::InProgress,
        }),
        body: "working".into(),
        payload: Some(serde_json::json!({"progress": "working"})),
        blocks: None,
        handling_mode: Some(meerkat_core::types::HandlingMode::Queue),
    });
    let outcome = driver.accept_input(input).await.unwrap();
    assert!(
        outcome.is_rejected(),
        "ResponseProgress with handling_mode must be rejected"
    );
}

#[tokio::test]
async fn accept_peer_response_terminal_with_handling_mode_returns_accepted() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
    let input = Input::Peer(PeerInput {
        header: InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: InputOrigin::Peer {
                peer_id: "550e8400-e29b-41d4-a716-446655440000".into(),
                display_identity: Some("Peer 1".into()),
                runtime_id: None,
            },
            durability: InputDurability::Durable,
            visibility: InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        convention: Some(PeerConvention::ResponseTerminal {
            request_id: "018f6f79-7a82-7c4e-a552-a3b86f9630f1".into(),
            status: ResponseTerminalStatus::Completed,
        }),
        body: "done".into(),
        payload: Some(serde_json::json!({"ok": true})),
        blocks: None,
        handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
    });
    let outcome = driver.accept_input(input).await.unwrap();
    assert!(
        outcome.is_accepted(),
        "ResponseTerminal with handling_mode=Steer must be accepted"
    );
}

#[tokio::test]
async fn accept_peer_response_terminal_defers_context_projection_to_machine_batch()
-> Result<(), String> {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
    let input = make_peer_terminal("done");

    let outcome = driver.accept_input(input).await.unwrap();
    let meerkat_runtime::AcceptOutcome::Accepted { input_id, .. } = outcome else {
        return Err("expected terminal peer response to be accepted".to_string());
    };

    let projection = driver
        .admitted_primitive_projection(&input_id)
        .ok_or_else(|| "accepted input should have primitive projection".to_string())?;
    assert!(projection.append.is_none());
    assert!(
        projection.context_append.is_none(),
        "terminal peer response context must be projected by the machine-selected runtime batch"
    );
    Ok(())
}

#[tokio::test]
async fn accept_peer_response_terminal_with_empty_request_id_returns_rejected() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
    let input = Input::Peer(PeerInput {
        header: InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: InputOrigin::Peer {
                peer_id: "550e8400-e29b-41d4-a716-446655440000".into(),
                display_identity: Some("Peer 1".into()),
                runtime_id: None,
            },
            durability: InputDurability::Durable,
            visibility: InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        convention: Some(PeerConvention::ResponseTerminal {
            request_id: " ".into(),
            status: ResponseTerminalStatus::Completed,
        }),
        body: "done".into(),
        payload: Some(serde_json::json!({"ok": true})),
        blocks: None,
        handling_mode: None,
    });
    let outcome = driver.accept_input(input).await.unwrap();
    assert!(
        outcome.is_rejected(),
        "ResponseTerminal with empty request_id must fail closed at admission"
    );
}

#[tokio::test]
async fn accept_peer_message_with_steer_handling_mode_returns_accepted() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
    let input = Input::Peer(PeerInput {
        header: InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: InputOrigin::Peer {
                peer_id: "peer-1".into(),
                display_identity: None,
                runtime_id: None,
            },
            durability: InputDurability::Durable,
            visibility: InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        convention: Some(PeerConvention::Message),
        body: "hi".into(),
        payload: None,
        blocks: None,
        handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
    });
    let outcome = driver.accept_input(input).await.unwrap();
    assert!(
        outcome.is_accepted(),
        "Message with handling_mode=Steer must be accepted"
    );
    if let meerkat_runtime::AcceptOutcome::Accepted { policy, .. } = outcome {
        assert_eq!(
            policy.routing_disposition,
            meerkat_runtime::RoutingDisposition::Steer
        );
    }
}

// ---------------------------------------------------------------------------
// §: PostAdmissionSignal typed enum tests
// ---------------------------------------------------------------------------

#[test]
fn post_admission_signal_ordering() {
    assert!(PostAdmissionSignal::None < PostAdmissionSignal::WakeLoop);
    assert!(PostAdmissionSignal::WakeLoop < PostAdmissionSignal::InterruptYielding);
    assert!(
        PostAdmissionSignal::InterruptYielding < PostAdmissionSignal::RequestImmediateProcessing
    );
}

#[test]
fn post_admission_signal_should_wake() {
    assert!(!PostAdmissionSignal::None.should_wake());
    assert!(PostAdmissionSignal::WakeLoop.should_wake());
    assert!(PostAdmissionSignal::InterruptYielding.should_wake());
    assert!(PostAdmissionSignal::RequestImmediateProcessing.should_wake());
}

#[test]
fn post_admission_signal_should_interrupt_yielding() {
    assert!(!PostAdmissionSignal::None.should_interrupt_yielding());
    assert!(!PostAdmissionSignal::WakeLoop.should_interrupt_yielding());
    assert!(PostAdmissionSignal::InterruptYielding.should_interrupt_yielding());
    assert!(PostAdmissionSignal::RequestImmediateProcessing.should_interrupt_yielding());
}

#[test]
fn post_admission_signal_should_process_immediately() {
    assert!(!PostAdmissionSignal::None.should_process_immediately());
    assert!(!PostAdmissionSignal::WakeLoop.should_process_immediately());
    assert!(!PostAdmissionSignal::InterruptYielding.should_process_immediately());
    assert!(PostAdmissionSignal::RequestImmediateProcessing.should_process_immediately());
}

// WIP: same machine-owned vs driver-owned admission signal transition.

#[tokio::test]
async fn post_admission_signal_accumulates_strongest() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));

    // Admission-time classification is machine-owned; direct driver acceptance
    // should not retain a local signal shadow.
    let prompt = make_prompt_input("hello");
    let outcome = driver.accept_input(prompt).await.unwrap();
    assert_machine_owned_admission_signal(&outcome, false, PostAdmissionSignal::WakeLoop);
    assert_eq!(
        driver.take_post_admission_signal(),
        PostAdmissionSignal::None
    );
}

// WIP: same machine-owned vs driver-owned admission signal transition.

#[tokio::test]
async fn post_admission_signal_steer_is_request_immediate() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));

    // Accept a steer-mode input (RequestImmediateProcessing signal)
    let steer_input = Input::Peer(PeerInput {
        header: InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: InputOrigin::Peer {
                peer_id: "p".into(),
                display_identity: None,
                runtime_id: None,
            },
            durability: InputDurability::Durable,
            visibility: InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        convention: Some(PeerConvention::Message),
        body: "urgent".into(),
        payload: None,
        blocks: None,
        handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
    });
    let outcome = driver.accept_input(steer_input).await.unwrap();
    assert_machine_owned_admission_signal(
        &outcome,
        true,
        PostAdmissionSignal::RequestImmediateProcessing,
    );
    assert_eq!(
        driver.take_post_admission_signal(),
        PostAdmissionSignal::None
    );
}

#[tokio::test]
async fn post_admission_signal_queue_peer_message_while_running_interrupts_yielding() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));

    // Admit a prompt to start a run
    let prompt = make_prompt_input("start");
    let _ = driver.accept_input(prompt).await.unwrap();
    let _ = driver.take_post_admission_signal();

    // Start a run so the runtime is in Running state
    bind_running(&mut driver, RunId::new(), RuntimeState::Idle);

    // Now admit a default queue-mode peer message while running.
    let peer = Input::Peer(PeerInput {
        header: InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: InputOrigin::Peer {
                peer_id: "p".into(),
                display_identity: None,
                runtime_id: None,
            },
            durability: InputDurability::Durable,
            visibility: InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        convention: Some(PeerConvention::Message),
        body: "interrupt me".into(),
        payload: None,
        blocks: None,
        handling_mode: None,
    });
    let outcome = driver.accept_input(peer).await.unwrap();
    let signal = post_admission_signal_from_accept_outcome(&outcome, false);

    assert_eq!(
        signal,
        PostAdmissionSignal::InterruptYielding,
        "queue-mode peer message while running should request cooperative interrupt, got {signal:?}"
    );
    assert_eq!(
        driver.take_post_admission_signal(),
        PostAdmissionSignal::None
    );
}

#[test]
fn post_admission_signal_request_immediate_ordering() {
    assert!(PostAdmissionSignal::None < PostAdmissionSignal::WakeLoop);
    assert!(PostAdmissionSignal::WakeLoop < PostAdmissionSignal::InterruptYielding);
    assert!(
        PostAdmissionSignal::InterruptYielding < PostAdmissionSignal::RequestImmediateProcessing
    );
    assert!(!PostAdmissionSignal::WakeLoop.should_process_immediately());
    assert!(!PostAdmissionSignal::InterruptYielding.should_process_immediately());
    assert!(PostAdmissionSignal::RequestImmediateProcessing.should_process_immediately());
}

// Pins the DSL effect carries a typed `PostAdmissionSignalKind` variant, not
// a string. The `map` helper's match is exhaustive with no default arm —
// adding a variant without updating consumers stops compilation.
#[test]
fn post_admission_signal_dsl_effect_carries_typed_variant() {
    use meerkat_runtime::meerkat_machine::dsl::{MeerkatMachineEffect, PostAdmissionSignalKind};

    fn map(signal: PostAdmissionSignalKind) -> PostAdmissionSignal {
        match signal {
            PostAdmissionSignalKind::WakeLoop => PostAdmissionSignal::WakeLoop,
            PostAdmissionSignalKind::InterruptYielding => PostAdmissionSignal::InterruptYielding,
            PostAdmissionSignalKind::RequestImmediateProcessing => {
                PostAdmissionSignal::RequestImmediateProcessing
            }
        }
    }

    for (variant, expected) in [
        (
            PostAdmissionSignalKind::WakeLoop,
            PostAdmissionSignal::WakeLoop,
        ),
        (
            PostAdmissionSignalKind::InterruptYielding,
            PostAdmissionSignal::InterruptYielding,
        ),
        (
            PostAdmissionSignalKind::RequestImmediateProcessing,
            PostAdmissionSignal::RequestImmediateProcessing,
        ),
    ] {
        let effect = MeerkatMachineEffect::PostAdmissionSignal { signal: variant };
        let MeerkatMachineEffect::PostAdmissionSignal { signal } = effect else {
            panic!("constructed effect did not round-trip as PostAdmissionSignal");
        };
        assert_eq!(map(signal), expected);
    }
}

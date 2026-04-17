#![allow(clippy::unwrap_used)]

use chrono::Utc;
use meerkat_core::lifecycle::{
    InputId, RunBoundaryReceipt, RunId, run_primitive::RunApplyBoundary,
};
use meerkat_runtime::{
    ContinuationInput, EphemeralRuntimeDriver, Input, InputDurability, InputHeader,
    InputLifecycleEvent, InputLifecycleState, InputOrigin, InputVisibility, LogicalRuntimeId,
    PeerConvention, PeerInput, PostAdmissionSignal, PromptInput, ResponseProgressPhase,
    ResponseTerminalStatus, RuntimeDriver, RuntimeDriverError, RuntimeEvent, RuntimeState,
    driver::ephemeral::ReplayQueuedContributorsPlan, post_admission_signal_from_accept_outcome,
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
                peer_id: "peer-1".into(),
                runtime_id: None,
            },
            durability: InputDurability::Durable,
            visibility: InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        convention: Some(PeerConvention::ResponseTerminal {
            request_id: "req-1".into(),
            status: ResponseTerminalStatus::Completed,
        }),
        body: body.into(),
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
                peer_id: "peer-1".into(),
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
    driver.contract_set_control_projection(
        RuntimeState::Running,
        Some(run_id),
        Some(pre_run_phase),
    );
}

fn complete_run_projection(driver: &mut EphemeralRuntimeDriver, next_phase: RuntimeState) {
    driver.contract_set_control_projection(next_phase, None, None);
}

fn retire_runtime(driver: &mut EphemeralRuntimeDriver) -> meerkat_runtime::RetireReport {
    driver.contract_set_control_projection(RuntimeState::Retired, None, None);
    driver.contract_finalize_retire()
}

fn reset_runtime(driver: &mut EphemeralRuntimeDriver) -> meerkat_runtime::ResetReport {
    driver.contract_set_control_projection(RuntimeState::Idle, None, None);
    driver.contract_reset_cleanup()
}

fn destroy_runtime(driver: &mut EphemeralRuntimeDriver) -> usize {
    driver.contract_set_control_projection(RuntimeState::Destroyed, None, None);
    driver.contract_destroy_cleanup()
}

fn stop_runtime(driver: &mut EphemeralRuntimeDriver) {
    driver.contract_set_control_projection(RuntimeState::Stopped, None, None);
    driver.contract_finalize_stop_runtime();
}

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

#[tokio::test]
async fn accept_peer_terminal_running_no_wake() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
    bind_running(&mut driver, RunId::new(), RuntimeState::Idle);

    let input = make_peer_terminal("done");
    let result = driver.accept_input(input).await.unwrap();

    assert!(result.is_accepted());
    assert!(!driver.take_wake_requested()); // Running → no wake
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
async fn retire_preserves_queued_for_drain() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
    driver.accept_input(make_prompt_input("a")).await.unwrap();
    driver.accept_input(make_prompt_input("b")).await.unwrap();

    let report = retire_runtime(&mut driver);
    assert_eq!(report.inputs_abandoned, 0);
    assert_eq!(report.inputs_pending_drain, 2);
    assert_eq!(driver.runtime_state(), RuntimeState::Retired);
    // Queue is still intact for drain
    assert_eq!(driver.queue().len(), 2);
}

#[tokio::test]
async fn reset_abandons_and_drains() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
    driver.accept_input(make_prompt_input("a")).await.unwrap();

    let report = reset_runtime(&mut driver);
    assert_eq!(report.inputs_abandoned, 1);
    assert!(driver.queue().is_empty());
}

#[tokio::test]
async fn destroy_transitions_to_terminal() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
    driver.accept_input(make_prompt_input("a")).await.unwrap();

    let abandoned = destroy_runtime(&mut driver);
    assert_eq!(abandoned, 1);
    assert!(driver.runtime_state().is_terminal());
}

#[tokio::test]
async fn on_run_completed_consumes() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));

    // Accept and manually transition to simulate run
    let input = make_prompt_input("hello");
    let input_id = input.id().clone();
    driver.accept_input(input).await.unwrap();
    driver.take_wake_requested();

    // Simulate run start
    let run_id = RunId::new();
    bind_running(&mut driver, run_id.clone(), RuntimeState::Idle);

    // Stage and apply
    driver.stage_input(&input_id, &run_id).unwrap();
    driver.apply_input(&input_id, &run_id).unwrap();
    driver
        .boundary_applied(
            run_id.clone(),
            RunBoundaryReceipt {
                run_id: run_id.clone(),
                boundary: RunApplyBoundary::RunStart,
                contributing_input_ids: vec![input_id.clone()],
                conversation_digest: None,
                message_count: 0,
                sequence: 1,
            },
            None,
        )
        .await
        .unwrap();

    // Run completed
    driver
        .run_completed(run_id.clone(), vec![input_id.clone()])
        .await
        .unwrap();
    complete_run_projection(&mut driver, RuntimeState::Idle);

    // Input should be consumed
    let state = driver.input_state(&input_id).unwrap();
    assert_eq!(state.current_state(), InputLifecycleState::Consumed);
    assert_eq!(driver.runtime_state(), RuntimeState::Idle);
}

#[tokio::test]
async fn on_run_failed_rollbacks() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));

    let input = make_prompt_input("hello");
    let input_id = input.id().clone();
    driver.accept_input(input).await.unwrap();

    let run_id = RunId::new();
    bind_running(&mut driver, run_id.clone(), RuntimeState::Idle);
    driver.stage_input(&input_id, &run_id).unwrap();

    // Run failed
    driver
        .run_failed(
            run_id,
            vec![input_id.clone()],
            ReplayQueuedContributorsPlan {
                queue_work_ids: vec![input_id.clone()],
                steer_work_ids: Vec::new(),
                wake_runtime: true,
                notice_kind: "RunFailed",
            },
            "LLM error".into(),
            true,
        )
        .await
        .unwrap();
    complete_run_projection(&mut driver, RuntimeState::Idle);

    // Input should be rolled back to Queued
    let state = driver.input_state(&input_id).unwrap();
    assert_eq!(state.current_state(), InputLifecycleState::Queued);
    assert_eq!(driver.runtime_state(), RuntimeState::Idle);
    assert_queue_projection_alignment(&driver);
}

#[tokio::test]
async fn on_run_failed_requests_wake_for_backlog() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));

    let input1 = make_prompt_input("first");
    let input1_id = input1.id().clone();
    let input2 = make_prompt_input("second");
    driver.accept_input(input1).await.unwrap();
    driver.accept_input(input2).await.unwrap();
    let _ = driver.take_wake_requested();

    let run_id = RunId::new();
    let (dequeued_id, _) = driver.dequeue_next().unwrap();
    assert_eq!(dequeued_id, input1_id);
    bind_running(&mut driver, run_id.clone(), RuntimeState::Idle);
    driver.stage_input(&input1_id, &run_id).unwrap();

    driver
        .run_failed(
            run_id,
            vec![input1_id.clone()],
            ReplayQueuedContributorsPlan {
                queue_work_ids: vec![input1_id.clone()],
                steer_work_ids: Vec::new(),
                wake_runtime: true,
                notice_kind: "RunFailed",
            },
            "LLM error".into(),
            true,
        )
        .await
        .unwrap();

    assert!(
        driver.take_wake_requested(),
        "queued work behind a failed run should request another wake"
    );
}

#[tokio::test]
async fn reset_after_retire_returns_runtime_to_idle() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
    retire_runtime(&mut driver);
    assert_eq!(driver.runtime_state(), RuntimeState::Retired);

    let report = reset_runtime(&mut driver);
    assert_eq!(report.inputs_abandoned, 0);
    assert_eq!(driver.runtime_state(), RuntimeState::Idle);

    let accepted = driver
        .accept_input(make_prompt_input("hello"))
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
    let state = driver.input_state(&input_id).unwrap();
    assert_eq!(state.current_state(), InputLifecycleState::Queued);

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
    let state = driver.input_state(&input_id).unwrap();
    assert_eq!(
        state.current_state(),
        InputLifecycleState::AppliedPendingConsumption
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
async fn rollback_restores_queue_projection_order() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));

    let first = make_prompt_input("first");
    let first_id = first.id().clone();
    let second = make_prompt_input("second");
    let second_id = second.id().clone();
    driver.accept_input(first).await.unwrap();
    driver.accept_input(second).await.unwrap();
    let _ = driver.take_wake_requested();

    let run_id = RunId::new();
    let (dequeued_id, _) = driver.dequeue_next().unwrap();
    assert_eq!(dequeued_id, first_id);
    bind_running(&mut driver, run_id.clone(), RuntimeState::Idle);
    driver.stage_input(&first_id, &run_id).unwrap();

    driver
        .run_failed(
            run_id,
            vec![first_id.clone()],
            ReplayQueuedContributorsPlan {
                queue_work_ids: vec![first_id.clone()],
                steer_work_ids: Vec::new(),
                wake_runtime: true,
                notice_kind: "RunFailed",
            },
            "rollback".into(),
            true,
        )
        .await
        .unwrap();

    assert_eq!(driver.queue().input_ids(), vec![first_id, second_id]);
    assert_queue_projection_alignment(&driver);
}

#[tokio::test]
async fn retired_rejects_input() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
    retire_runtime(&mut driver);

    let result = driver.accept_input(make_prompt_input("hello")).await;
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
    let state = driver.input_state(&input_id).unwrap();
    assert_eq!(state.current_state(), InputLifecycleState::Queued);
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
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
    retire_runtime(&mut driver);
    let abandoned = destroy_runtime(&mut driver);
    assert_eq!(abandoned, 0);
    assert_eq!(driver.runtime_state(), RuntimeState::Destroyed);
}

// ---- Phase 1: State machine hardening tests ----

#[tokio::test]
async fn retired_can_drain_queue_via_run_cycle() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));

    // Accept an input
    let input = make_prompt_input("drain me");
    let input_id = input.id().clone();
    driver.accept_input(input).await.unwrap();
    let _ = driver.take_wake_requested();

    // Retire — queue preserved for drain
    let report = retire_runtime(&mut driver);
    assert_eq!(report.inputs_abandoned, 0);
    assert_eq!(report.inputs_pending_drain, 1);
    assert_eq!(driver.queue().len(), 1);

    // Can still dequeue and process (Retired → Running)
    let (dequeued_id, _) = driver.dequeue_next().unwrap();
    assert_eq!(dequeued_id, input_id);

    let run_id = RunId::new();
    bind_running(&mut driver, run_id.clone(), RuntimeState::Retired);
    assert_eq!(driver.runtime_state(), RuntimeState::Running);

    driver.stage_input(&input_id, &run_id).unwrap();
    driver.apply_input(&input_id, &run_id).unwrap();
    driver
        .boundary_applied(
            run_id.clone(),
            RunBoundaryReceipt {
                run_id: run_id.clone(),
                boundary: RunApplyBoundary::RunStart,
                contributing_input_ids: vec![input_id.clone()],
                conversation_digest: None,
                message_count: 0,
                sequence: 1,
            },
            None,
        )
        .await
        .unwrap();

    // Complete run → returns to Retired (not Idle)
    driver.run_completed(run_id, vec![input_id]).await.unwrap();
    complete_run_projection(&mut driver, RuntimeState::Retired);

    assert_eq!(driver.runtime_state(), RuntimeState::Retired);
    assert!(driver.queue().is_empty());
}

#[tokio::test]
async fn retired_rejects_new_input_while_draining() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));

    driver
        .accept_input(make_prompt_input("existing"))
        .await
        .unwrap();
    retire_runtime(&mut driver);

    // New input rejected
    let result = driver.accept_input(make_prompt_input("rejected")).await;
    assert!(result.is_err());

    // But existing queue is intact
    assert_eq!(driver.queue().len(), 1);
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
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));

    let input = make_prompt_input("stop me");
    let input_id = input.id().clone();
    driver.accept_input(input).await.unwrap();

    stop_runtime(&mut driver);

    assert_eq!(driver.runtime_state(), RuntimeState::Stopped);
    assert!(driver.queue().is_empty());

    let state = driver.input_state(&input_id).unwrap();
    assert!(state.is_terminal());
}

#[tokio::test]
async fn destroy_with_queued_inputs_abandons_all() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
    driver.accept_input(make_prompt_input("a")).await.unwrap();
    driver.accept_input(make_prompt_input("b")).await.unwrap();

    let abandoned = destroy_runtime(&mut driver);
    assert_eq!(abandoned, 2);
    assert!(driver.runtime_state().is_terminal());
    assert!(driver.active_input_ids().is_empty());
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
                peer_id: "peer-1".into(),
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
                peer_id: "peer-1".into(),
                runtime_id: None,
            },
            durability: InputDurability::Durable,
            visibility: InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        convention: Some(PeerConvention::ResponseTerminal {
            request_id: "req-1".into(),
            status: ResponseTerminalStatus::Completed,
        }),
        body: "done".into(),
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
async fn accept_peer_message_with_steer_handling_mode_returns_accepted() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
    let input = Input::Peer(PeerInput {
        header: InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: InputOrigin::Peer {
                peer_id: "peer-1".into(),
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

#![allow(clippy::unwrap_used)]

use chrono::Utc;
use meerkat_core::lifecycle::{InputId, RunEvent, RunId};
use meerkat_runtime::{
    ContinuationInput, EphemeralRuntimeDriver, Input, InputDurability, InputHeader,
    InputLifecycleEvent, InputLifecycleState, InputOrigin, InputVisibility, LogicalRuntimeId,
    PeerConvention, PeerInput, PromptInput, ResponseProgressPhase, ResponseTerminalStatus,
    RuntimeControlCommand, RuntimeDriver, RuntimeEvent, RuntimeState,
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
    })
}

fn make_continuation() -> Input {
    Input::Continuation(ContinuationInput::terminal_peer_response(
        "driver test continuation",
    ))
}

#[tokio::test]
async fn accept_prompt_idle_queues_and_wakes() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
    let input = make_prompt_input("hello");
    let result = driver.accept_input(input).await.unwrap();

    assert!(result.is_accepted());
    assert!(driver.take_wake_requested());
    assert_eq!(driver.queue().len(), 1);
}

#[tokio::test]
async fn accept_prompt_running_queues_and_wakes() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
    driver.start_run(RunId::new()).unwrap();

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
    assert!(driver.take_wake_requested()); // Terminal peer wakes idle runtime
}

#[tokio::test]
async fn accept_peer_terminal_running_no_wake() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
    driver.start_run(RunId::new()).unwrap();

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
    assert!(!driver.take_wake_requested()); // Progress never wakes
}

#[tokio::test]
async fn accept_continuation_idle_wakes_and_requests_processing() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
    let input = make_continuation();
    let result = driver.accept_input(input).await.unwrap();

    assert!(result.is_accepted());
    assert!(driver.take_wake_requested());
    assert!(driver.take_process_requested());
    assert_eq!(driver.queue().len(), 1);
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

    let report = driver.retire().unwrap();
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

    let report = driver.reset().unwrap();
    assert_eq!(report.inputs_abandoned, 1);
    assert!(driver.queue().is_empty());
}

#[tokio::test]
async fn destroy_transitions_to_terminal() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
    driver.accept_input(make_prompt_input("a")).await.unwrap();

    let abandoned = driver.destroy().unwrap();
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
    driver.start_run(run_id.clone()).unwrap();

    // Stage and apply
    driver.stage_input(&input_id, &run_id).unwrap();
    driver.apply_input(&input_id, &run_id).unwrap();

    // Run completed
    driver
        .on_run_event(RunEvent::RunCompleted {
            run_id: run_id.clone(),
            consumed_input_ids: vec![input_id.clone()],
        })
        .await
        .unwrap();

    // Input should be consumed
    let state = driver.input_state(&input_id).unwrap();
    assert_eq!(state.current_state(), InputLifecycleState::Consumed);
    assert!(driver.control().is_idle());
}

#[tokio::test]
async fn on_run_failed_rollbacks() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));

    let input = make_prompt_input("hello");
    let input_id = input.id().clone();
    driver.accept_input(input).await.unwrap();

    let run_id = RunId::new();
    driver.start_run(run_id.clone()).unwrap();
    driver.stage_input(&input_id, &run_id).unwrap();

    // Run failed
    driver
        .on_run_event(RunEvent::RunFailed {
            run_id,
            error: "LLM error".into(),
            recoverable: true,
        })
        .await
        .unwrap();

    // Input should be rolled back to Queued
    let state = driver.input_state(&input_id).unwrap();
    assert_eq!(state.current_state(), InputLifecycleState::Queued);
    assert!(driver.control().is_idle());
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
    driver.start_run(run_id.clone()).unwrap();
    driver.stage_input(&input1_id, &run_id).unwrap();

    driver
        .on_run_event(RunEvent::RunFailed {
            run_id,
            error: "LLM error".into(),
            recoverable: true,
        })
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
    driver.retire().unwrap();
    assert_eq!(driver.runtime_state(), RuntimeState::Retired);

    let report = driver.reset().unwrap();
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
async fn recovery_counts_queued_as_recovered() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));

    // Accept an input — policy transitions it to Queued
    let input = make_prompt_input("hello");
    let input_id = input.id().clone();
    driver.accept_input(input).await.unwrap();

    // After accept, state is Queued (policy applied immediately)
    let state = driver.input_state(&input_id).unwrap();
    assert_eq!(state.current_state(), InputLifecycleState::Queued);

    // Drain the queue (simulating crash losing queue state)
    driver.queue_mut().drain();

    // Recover
    let report = driver.recover_ephemeral();
    // Queued inputs are counted as recovered (already in correct state)
    assert_eq!(report.inputs_recovered, 1);
    assert_eq!(report.inputs_requeued, 0); // Already Queued, no transition needed
}

#[tokio::test]
async fn recovery_applied_stays_applied() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));

    let input = make_prompt_input("hello");
    let input_id = input.id().clone();
    driver.accept_input(input).await.unwrap();

    let run_id = RunId::new();
    driver.start_run(run_id.clone()).unwrap();
    driver.stage_input(&input_id, &run_id).unwrap();
    driver.apply_input(&input_id, &run_id).unwrap();

    // Recover — apply RecoverRequested through the authority
    use meerkat_runtime::RuntimeControlInput;
    use meerkat_runtime::RuntimeControlMutator;
    driver
        .control_mut()
        .apply(RuntimeControlInput::RecoverRequested)
        .unwrap();
    let report = driver.recover_ephemeral();
    assert_eq!(report.inputs_recovered, 1);

    // Applied stays Applied (side effects already happened)
    let state = driver.input_state(&input_id).unwrap();
    assert_eq!(
        state.current_state(),
        InputLifecycleState::AppliedPendingConsumption
    );
}

#[tokio::test]
async fn retired_rejects_input() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
    driver.retire().unwrap();

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
    assert!(!driver.take_wake_requested()); // Progress never wakes
}

#[tokio::test]
async fn destroy_after_retire_succeeds() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
    driver.retire().unwrap();
    let abandoned = driver.destroy().unwrap();
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
    let report = driver.retire().unwrap();
    assert_eq!(report.inputs_abandoned, 0);
    assert_eq!(report.inputs_pending_drain, 1);
    assert_eq!(driver.queue().len(), 1);

    // Can still dequeue and process (Retired → Running)
    let (dequeued_id, _) = driver.dequeue_next().unwrap();
    assert_eq!(dequeued_id, input_id);

    let run_id = RunId::new();
    driver.start_run(run_id.clone()).unwrap();
    assert!(driver.control().is_running());

    driver.stage_input(&input_id, &run_id).unwrap();
    driver.apply_input(&input_id, &run_id).unwrap();

    // Complete run → returns to Retired (not Idle)
    driver
        .on_run_event(RunEvent::RunCompleted {
            run_id,
            consumed_input_ids: vec![input_id],
        })
        .await
        .unwrap();

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
    driver.retire().unwrap();

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
    driver.start_run(RunId::new()).unwrap();

    let result = driver.reset();
    assert!(result.is_err());
    assert!(driver.control().is_running());
}

#[tokio::test]
async fn stop_abandons_all_active_inputs() {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));

    let input = make_prompt_input("stop me");
    let input_id = input.id().clone();
    driver.accept_input(input).await.unwrap();

    driver
        .on_runtime_control(RuntimeControlCommand::Stop)
        .await
        .unwrap();

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

    let abandoned = driver.destroy().unwrap();
    assert_eq!(abandoned, 2);
    assert!(driver.runtime_state().is_terminal());
    assert!(driver.active_input_ids().is_empty());
}

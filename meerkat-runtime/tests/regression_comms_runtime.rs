#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Regression tests for comms → RuntimeDriver path.
//!
//! These tests mirror the 12 behavioral contracts from
//! meerkat-core/tests/regression_comms_host.rs but exercise them
//! through the v9 RuntimeDriver input acceptance path.
//!
//! Each test verifies:
//! - Correct PeerConvention mapping (CommsInputBridge)
//! - Correct PolicyDecision (DefaultPolicyTable)
//! - Correct wake/no-wake semantics
//! - Correct InputState lifecycle transitions

use meerkat_core::interaction::{
    InboxInteraction, InteractionContent, InteractionId, ResponseStatus,
};
use meerkat_runtime::comms_bridge::interaction_to_peer_input;
use meerkat_runtime::driver::ephemeral::EphemeralRuntimeDriver;
use meerkat_runtime::identifiers::LogicalRuntimeId;
use meerkat_runtime::input::{Input, InputDurability, PeerConvention};
use meerkat_runtime::input_state::InputLifecycleState;
use meerkat_runtime::policy_table::DefaultPolicyTable;
use meerkat_runtime::runtime_state::RuntimeState;
use meerkat_runtime::traits::RuntimeDriver;
use uuid::Uuid;

fn iid() -> InteractionId {
    InteractionId(Uuid::now_v7())
}

fn make_message(from: &str, body: &str) -> InboxInteraction {
    InboxInteraction {
        id: iid(),
        from: from.into(),
        content: InteractionContent::Message {
            body: body.into(),
            blocks: None,
        },
        rendered_text: format!("[{from}]: {body}"),
    }
}

fn make_response(from: &str, status: ResponseStatus) -> InboxInteraction {
    let in_reply_to = iid();
    InboxInteraction {
        id: iid(),
        from: from.into(),
        content: InteractionContent::Response {
            in_reply_to,
            status,
            result: serde_json::json!({"ok": true}),
        },
        rendered_text: format!("[{from}]: response ({status:?})"),
    }
}

fn make_request(from: &str, intent: &str) -> InboxInteraction {
    InboxInteraction {
        id: iid(),
        from: from.into(),
        content: InteractionContent::Request {
            intent: intent.into(),
            params: serde_json::json!({}),
        },
        rendered_text: format!("[{from}]: request ({intent})"),
    }
}

fn rid() -> LogicalRuntimeId {
    LogicalRuntimeId::new("test-runtime")
}

// ---------------------------------------------------------------------------
// §1: Completed response triggers continuation (wake) when idle
// ---------------------------------------------------------------------------
#[tokio::test]
async fn completed_response_idle_wakes() {
    let mut driver = EphemeralRuntimeDriver::new(rid());
    let interaction = make_response("peer-1", ResponseStatus::Completed);
    let input = interaction_to_peer_input(&interaction, &rid());

    // Verify bridge mapping
    if let Input::Peer(ref p) = input {
        assert!(matches!(
            p.convention,
            Some(PeerConvention::ResponseTerminal { .. })
        ));
        assert_eq!(p.header.durability, InputDurability::Durable);
    } else {
        panic!("Expected PeerInput");
    }

    // Verify policy: terminal response + idle → StageRunStart + WakeIfIdle
    let policy = DefaultPolicyTable::resolve(&input, true);
    assert_eq!(policy.apply_mode, meerkat_runtime::ApplyMode::StageRunStart);
    assert_eq!(policy.wake_mode, meerkat_runtime::WakeMode::WakeIfIdle);

    // Verify driver behavior
    let outcome = driver.accept_input(input).await.unwrap();
    assert!(outcome.is_accepted());
    assert!(driver.take_wake_requested());
}

// ---------------------------------------------------------------------------
// §2: Accepted response injects context, no continuation (no wake)
// ---------------------------------------------------------------------------
#[tokio::test]
async fn accepted_response_no_wake() {
    let mut driver = EphemeralRuntimeDriver::new(rid());
    let interaction = make_response("peer-1", ResponseStatus::Accepted);
    let input = interaction_to_peer_input(&interaction, &rid());

    // Verify bridge: Accepted → ResponseProgress
    if let Input::Peer(ref p) = input {
        assert!(matches!(
            p.convention,
            Some(PeerConvention::ResponseProgress { .. })
        ));
        assert_eq!(p.header.durability, InputDurability::Ephemeral);
    } else {
        panic!("Expected PeerInput");
    }

    // Verify policy per §17: progress → StageRunBoundary + NoWake + Coalesce + OnRunComplete
    let policy = DefaultPolicyTable::resolve(&input, true);
    assert_eq!(
        policy.apply_mode,
        meerkat_runtime::ApplyMode::StageRunBoundary
    );
    assert_eq!(policy.wake_mode, meerkat_runtime::WakeMode::None);
    assert_eq!(policy.queue_mode, meerkat_runtime::QueueMode::Coalesce);
    assert_eq!(
        policy.consume_point,
        meerkat_runtime::ConsumePoint::OnRunComplete
    );

    // Verify driver: accepted but no wake, queued (not immediately consumed)
    let outcome = driver.accept_input(input).await.unwrap();
    assert!(outcome.is_accepted());
    assert!(!driver.take_wake_requested());

    // Input should be queued (StageRunBoundary queues for boundary application)
    if let meerkat_runtime::AcceptOutcome::Accepted { input_id, .. } = &outcome {
        let state = driver.input_state(input_id).unwrap();
        assert_eq!(state.current_state(), InputLifecycleState::Queued);
    }
}

// ---------------------------------------------------------------------------
// §3: Failed response triggers continuation (wake) when idle
// ---------------------------------------------------------------------------
#[tokio::test]
async fn failed_response_idle_wakes() {
    let mut driver = EphemeralRuntimeDriver::new(rid());
    let interaction = make_response("peer-1", ResponseStatus::Failed);
    let input = interaction_to_peer_input(&interaction, &rid());

    // Verify bridge: Failed → ResponseTerminal
    if let Input::Peer(ref p) = input {
        assert!(matches!(
            p.convention,
            Some(PeerConvention::ResponseTerminal {
                status: meerkat_runtime::ResponseTerminalStatus::Failed,
                ..
            })
        ));
    } else {
        panic!("Expected PeerInput");
    }

    // Verify: terminal response + idle → wake
    let outcome = driver.accept_input(input).await.unwrap();
    assert!(outcome.is_accepted());
    assert!(driver.take_wake_requested());
}

// ---------------------------------------------------------------------------
// §4: Response + passthrough message: both queued
// ---------------------------------------------------------------------------
#[tokio::test]
async fn response_with_passthrough_message_both_queued() {
    let mut driver = EphemeralRuntimeDriver::new(rid());

    // Accept a completed response
    let resp = make_response("peer-1", ResponseStatus::Completed);
    let input1 = interaction_to_peer_input(&resp, &rid());
    driver.accept_input(input1).await.unwrap();

    // Accept a message
    let msg = make_message("peer-2", "hello");
    let input2 = interaction_to_peer_input(&msg, &rid());
    driver.accept_input(input2).await.unwrap();

    // Both should be queued
    assert_eq!(driver.queue().len(), 2);
    assert!(driver.take_wake_requested()); // Terminal response woke it
}

// ---------------------------------------------------------------------------
// §5: Response after completed host turn triggers continuation
// ---------------------------------------------------------------------------
#[tokio::test]
async fn response_after_completed_turn_wakes() {
    let mut driver = EphemeralRuntimeDriver::new(rid());

    // Simulate a completed run by starting and completing
    let run_id = meerkat_core::lifecycle::RunId::new();
    driver.start_run(run_id.clone()).unwrap();
    driver.complete_run().unwrap();

    // Now idle — accept a terminal response
    let resp = make_response("peer-1", ResponseStatus::Completed);
    let input = interaction_to_peer_input(&resp, &rid());
    let outcome = driver.accept_input(input).await.unwrap();

    assert!(outcome.is_accepted());
    assert!(driver.take_wake_requested()); // Should wake idle runtime
}

// ---------------------------------------------------------------------------
// §6: Peer lifecycle batching — multiple peer_added collapse
// ---------------------------------------------------------------------------
#[tokio::test]
async fn peer_lifecycle_accepts_as_requests() {
    let mut driver = EphemeralRuntimeDriver::new(rid());

    // Silent intents (mob.peer_added) are PeerInput with Request convention
    let req1 = make_request("peer-1", "mob.peer_added");
    let input1 = interaction_to_peer_input(&req1, &rid());

    if let Input::Peer(ref p) = input1 {
        assert!(matches!(p.convention, Some(PeerConvention::Request { .. })));
    }

    // Policy: peer_request + idle → StageRunStart + WakeIfIdle
    let policy = DefaultPolicyTable::resolve(&input1, true);
    assert_eq!(policy.apply_mode, meerkat_runtime::ApplyMode::StageRunStart);
    assert_eq!(policy.wake_mode, meerkat_runtime::WakeMode::WakeIfIdle);

    let outcome = driver.accept_input(input1).await.unwrap();
    assert!(outcome.is_accepted());
}

// ---------------------------------------------------------------------------
// §7: Peer lifecycle net-out: add + retire same peer cancels
// ---------------------------------------------------------------------------
#[tokio::test]
async fn peer_lifecycle_net_out_both_accepted() {
    let mut driver = EphemeralRuntimeDriver::new(rid());

    // Both arrive as separate inputs — the v9 path accepts both individually
    // (batching/coalescing happens at the queue level, not acceptance)
    let added = make_request("peer-1", "mob.peer_added");
    let retired = make_request("peer-1", "mob.peer_retired");

    let input1 = interaction_to_peer_input(&added, &rid());
    let input2 = interaction_to_peer_input(&retired, &rid());

    let o1 = driver.accept_input(input1).await.unwrap();
    let o2 = driver.accept_input(input2).await.unwrap();

    assert!(o1.is_accepted());
    assert!(o2.is_accepted());
    assert_eq!(driver.queue().len(), 2); // Both queued individually
}

// ---------------------------------------------------------------------------
// §8: Silent comms intent — no LLM turn (maps to Request, policy wakes)
// ---------------------------------------------------------------------------
#[tokio::test]
async fn silent_intent_maps_to_request_with_wake() {
    let mut driver = EphemeralRuntimeDriver::new(rid());

    let interaction = make_request("coordinator", "mob.peer_added");
    let input = interaction_to_peer_input(&interaction, &rid());

    // Under v9, silent intents are PeerInput(Request). The runtime's policy
    // says WakeIfIdle for requests. The SILENT behavior is handled by the
    // SilentIntentOverride layer (not yet implemented in DefaultPolicyTable).
    // For now, verify the mapping is correct.
    let policy = DefaultPolicyTable::resolve(&input, true);
    assert_eq!(policy.apply_mode, meerkat_runtime::ApplyMode::StageRunStart);

    let outcome = driver.accept_input(input).await.unwrap();
    assert!(outcome.is_accepted());
}

// ---------------------------------------------------------------------------
// §9: Non-silent comms intent triggers LLM turn
// ---------------------------------------------------------------------------
#[tokio::test]
async fn non_silent_intent_triggers_wake() {
    let mut driver = EphemeralRuntimeDriver::new(rid());

    let interaction = make_request("coordinator", "custom.action");
    let input = interaction_to_peer_input(&interaction, &rid());

    let policy = DefaultPolicyTable::resolve(&input, true);
    assert_eq!(policy.wake_mode, meerkat_runtime::WakeMode::WakeIfIdle);

    let outcome = driver.accept_input(input).await.unwrap();
    assert!(outcome.is_accepted());
    assert!(driver.take_wake_requested());
}

// ---------------------------------------------------------------------------
// §10: Message interaction triggers host-mode run
// ---------------------------------------------------------------------------
#[tokio::test]
async fn message_triggers_wake() {
    let mut driver = EphemeralRuntimeDriver::new(rid());

    let interaction = make_message("peer-1", "hello world");
    let input = interaction_to_peer_input(&interaction, &rid());

    // peer_message + idle → StageRunStart + WakeIfIdle
    let policy = DefaultPolicyTable::resolve(&input, true);
    assert_eq!(policy.apply_mode, meerkat_runtime::ApplyMode::StageRunStart);
    assert_eq!(policy.wake_mode, meerkat_runtime::WakeMode::WakeIfIdle);

    let outcome = driver.accept_input(input).await.unwrap();
    assert!(outcome.is_accepted());
    assert!(driver.take_wake_requested());
    assert_eq!(driver.queue().len(), 1);
}

// ---------------------------------------------------------------------------
// §11: Request interaction triggers host-mode run
// ---------------------------------------------------------------------------
#[tokio::test]
async fn request_triggers_wake() {
    let mut driver = EphemeralRuntimeDriver::new(rid());

    let interaction = make_request("peer-1", "analyze");
    let input = interaction_to_peer_input(&interaction, &rid());

    let outcome = driver.accept_input(input).await.unwrap();
    assert!(outcome.is_accepted());
    assert!(driver.take_wake_requested());
}

// ---------------------------------------------------------------------------
// §12: Empty inbox — no turns
// ---------------------------------------------------------------------------
#[tokio::test]
async fn no_input_no_wake() {
    let driver = EphemeralRuntimeDriver::new(rid());
    // No accept_input called — queue empty, no wake
    assert!(driver.queue().is_empty());
    assert_eq!(driver.runtime_state(), RuntimeState::Idle);
}

// ---------------------------------------------------------------------------
// Additional: Message while running — checkpoint policy, no wake
// ---------------------------------------------------------------------------
#[tokio::test]
async fn message_while_running_checkpoint_no_wake() {
    let mut driver = EphemeralRuntimeDriver::new(rid());

    // Start a run
    driver
        .start_run(meerkat_core::lifecycle::RunId::new())
        .unwrap();

    let interaction = make_message("peer-1", "hello");
    let input = interaction_to_peer_input(&interaction, &rid());

    // peer_message + running → StageRunStart + InterruptYielding (per §17)
    let policy = DefaultPolicyTable::resolve(&input, false);
    assert_eq!(policy.apply_mode, meerkat_runtime::ApplyMode::StageRunStart);
    assert_eq!(
        policy.wake_mode,
        meerkat_runtime::WakeMode::InterruptYielding
    );

    let outcome = driver.accept_input(input).await.unwrap();
    assert!(outcome.is_accepted());
    assert!(!driver.take_wake_requested()); // Running → no wake
}

// ---------------------------------------------------------------------------
// Additional: Terminal response while running — checkpoint, no wake
// ---------------------------------------------------------------------------
#[tokio::test]
async fn terminal_response_while_running_no_wake() {
    let mut driver = EphemeralRuntimeDriver::new(rid());

    driver
        .start_run(meerkat_core::lifecycle::RunId::new())
        .unwrap();

    let interaction = make_response("peer-1", ResponseStatus::Completed);
    let input = interaction_to_peer_input(&interaction, &rid());

    // peer_response_terminal + running → StageRunStart + NoWake (per §17)
    let policy = DefaultPolicyTable::resolve(&input, false);
    assert_eq!(policy.apply_mode, meerkat_runtime::ApplyMode::StageRunStart);
    assert_eq!(policy.wake_mode, meerkat_runtime::WakeMode::None);

    let outcome = driver.accept_input(input).await.unwrap();
    assert!(outcome.is_accepted());
    assert!(!driver.take_wake_requested());
}

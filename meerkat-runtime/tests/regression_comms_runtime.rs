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

use meerkat_core::comms::PeerId;
use meerkat_core::interaction::{
    InboxInteraction, InteractionContent, InteractionId, PeerIngressConvention, PeerIngressFact,
    PeerIngressIdentity, PeerInputCandidate, PeerInputClass, ResponseStatus,
};
use meerkat_core::lifecycle::RunId;
use meerkat_runtime::comms_bridge::peer_input_candidate_to_runtime_input;
use meerkat_runtime::driver::ephemeral::{EphemeralRuntimeDriver, PostAdmissionSignal};
use meerkat_runtime::identifiers::LogicalRuntimeId;
use meerkat_runtime::input::{Input, InputDurability, PeerConvention};
use meerkat_runtime::input_state::InputLifecycleState;
use meerkat_runtime::policy_table::DefaultPolicyTable;
use meerkat_runtime::post_admission_signal_from_accept_outcome;
use meerkat_runtime::runtime_state::RuntimeState;
use meerkat_runtime::traits::RuntimeDriver;
use uuid::Uuid;

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

fn bind_running(driver: &mut EphemeralRuntimeDriver) -> RunId {
    let run_id = RunId::new();
    assert_eq!(driver.runtime_state(), RuntimeState::Idle);
    driver.contract_begin_run_authority(run_id.clone()).unwrap();
    assert_eq!(driver.runtime_state(), RuntimeState::Running);
    run_id
}

fn iid() -> InteractionId {
    InteractionId(Uuid::now_v7())
}

fn response_route_id() -> PeerId {
    PeerId::from_uuid(Uuid::parse_str("018f6f79-7a82-7c4e-a552-a3b86f9630f4").unwrap())
}

fn make_message(from: &str, body: &str) -> InboxInteraction {
    InboxInteraction {
        from_route: None,
        id: iid(),
        from: from.into(),
        content: InteractionContent::Message {
            body: body.into(),
            blocks: None,
        },
        rendered_text: format!("[{from}]: {body}"),
        handling_mode: meerkat_core::types::HandlingMode::Queue,
        render_metadata: None,
    }
}

fn make_message_with_blocks(from: &str, body: &str) -> InboxInteraction {
    InboxInteraction {
        from_route: None,
        id: iid(),
        from: from.into(),
        content: InteractionContent::Message {
            body: body.into(),
            blocks: Some(vec![
                meerkat_core::types::ContentBlock::Text { text: body.into() },
                meerkat_core::types::ContentBlock::Image {
                    media_type: "image/png".into(),
                    data: "abc123".into(),
                },
            ]),
        },
        rendered_text: format!("[{from}]: {body}"),
        handling_mode: meerkat_core::types::HandlingMode::Queue,
        render_metadata: None,
    }
}

fn make_response(from: &str, status: ResponseStatus) -> InboxInteraction {
    let in_reply_to = iid();
    InboxInteraction {
        from_route: Some(response_route_id()),
        id: iid(),
        from: from.into(),
        content: InteractionContent::Response {
            in_reply_to,
            status,
            result: serde_json::json!({"ok": true}),
            blocks: None,
        },
        rendered_text: format!("[{from}]: response ({status:?})"),
        handling_mode: meerkat_core::types::HandlingMode::Queue,
        render_metadata: None,
    }
}

fn make_request(from: &str, intent: &str) -> InboxInteraction {
    InboxInteraction {
        from_route: None,
        id: iid(),
        from: from.into(),
        content: InteractionContent::Request {
            intent: intent.into(),
            params: serde_json::json!({}),
            blocks: None,
        },
        rendered_text: format!("[{from}]: request ({intent})"),
        handling_mode: meerkat_core::types::HandlingMode::Queue,
        render_metadata: None,
    }
}

fn rid() -> LogicalRuntimeId {
    LogicalRuntimeId::new("test-runtime")
}

fn test_peer_id() -> PeerId {
    PeerId::parse("33333333-3333-4333-8333-333333333333").expect("canonical test peer id")
}

fn peer_kind_for_convention(convention: &PeerIngressConvention) -> meerkat_core::PeerIngressKind {
    match convention {
        PeerIngressConvention::Message => meerkat_core::PeerIngressKind::Message,
        PeerIngressConvention::Request { .. } | PeerIngressConvention::Lifecycle { .. } => {
            meerkat_core::PeerIngressKind::Request
        }
        PeerIngressConvention::Response { .. } => meerkat_core::PeerIngressKind::Response,
        PeerIngressConvention::Ack { .. } => meerkat_core::PeerIngressKind::Ack,
        PeerIngressConvention::PlainEvent { .. } => meerkat_core::PeerIngressKind::PlainEvent,
    }
}

fn runtime_input_for_interaction(
    interaction: &InboxInteraction,
    runtime_id: &LogicalRuntimeId,
) -> Input {
    let id = interaction.id;
    let (class, convention, response_terminality) = match &interaction.content {
        InteractionContent::Message { .. } => (
            PeerInputClass::ActionableMessage,
            PeerIngressConvention::Message,
            None,
        ),
        InteractionContent::Request { intent, .. } => (
            PeerInputClass::ActionableRequest,
            PeerIngressConvention::Request {
                request_id: id.to_string(),
                intent: intent.clone(),
            },
            None,
        ),
        InteractionContent::Response {
            in_reply_to,
            status,
            ..
        } => {
            let classification =
                meerkat_core::PeerIngressMachinePolicy::default().classify_response(*status);
            (
                classification.class,
                PeerIngressConvention::Response {
                    in_reply_to: *in_reply_to,
                    status: *status,
                },
                classification.response_terminality,
            )
        }
    };
    let kind = peer_kind_for_convention(&convention);
    let ingress = PeerIngressFact::peer(
        id,
        class,
        kind,
        Some(meerkat_core::PeerIngressAuthDecision::Required),
        PeerIngressIdentity::new(test_peer_id(), interaction.from.clone(), convention),
    );
    let mut candidate = PeerInputCandidate::new(interaction.clone(), ingress, None);
    candidate.response_terminality = response_terminality;
    peer_input_candidate_to_runtime_input(&candidate, runtime_id)
        .expect("test interaction should project to runtime input")
}

// ---------------------------------------------------------------------------
// §1: Completed response triggers continuation (wake) when idle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn completed_response_idle_wakes() {
    let mut driver = EphemeralRuntimeDriver::new(rid());
    let interaction = make_response("peer-1", ResponseStatus::Completed);
    let input = runtime_input_for_interaction(&interaction, &rid());

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
    assert_machine_owned_admission_signal(&outcome, false, PostAdmissionSignal::WakeLoop);
    assert_eq!(
        driver.take_post_admission_signal(),
        PostAdmissionSignal::None
    );
}

#[tokio::test]
async fn completed_response_admission_stamps_apply_intent_without_context_projection() {
    let mut driver = EphemeralRuntimeDriver::new(rid());
    let interaction = make_response("peer-1", ResponseStatus::Completed);
    let input = runtime_input_for_interaction(&interaction, &rid());

    let outcome = driver.accept_input(input).await.unwrap();
    let meerkat_runtime::AcceptOutcome::Accepted { input_id, .. } = outcome else {
        panic!("expected terminal peer response to be accepted");
    };

    let semantics = driver
        .admitted_runtime_semantics(&input_id)
        .expect("accepted input should have runtime semantics");
    assert_eq!(
        semantics.execution_kind,
        meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn
    );
    assert_eq!(
        semantics.peer_response_terminal_apply_intent,
        Some(
            meerkat_core::lifecycle::run_primitive::PeerResponseTerminalApplyIntent::AppendContextAndRun
        )
    );
    assert_eq!(
        driver.input_phase(&input_id),
        Some(InputLifecycleState::Queued)
    );
    let projection = driver
        .admitted_primitive_projection(&input_id)
        .expect("accepted input should have primitive projection");
    assert!(projection.append.is_none());
    assert!(
        projection.context_append.is_none(),
        "terminal response context projection is supplied by the machine-selected runtime batch"
    );
}

// ---------------------------------------------------------------------------
// §2: Accepted response injects context, no continuation (no wake)
// ---------------------------------------------------------------------------
#[tokio::test]
async fn accepted_response_no_wake() {
    let mut driver = EphemeralRuntimeDriver::new(rid());
    let interaction = make_response("peer-1", ResponseStatus::Accepted);
    let input = runtime_input_for_interaction(&interaction, &rid());

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
    assert!(outcome.is_accepted(), "unexpected outcome: {outcome:?}");
    assert_machine_owned_admission_signal(&outcome, false, PostAdmissionSignal::None);
    assert_eq!(
        driver.take_post_admission_signal(),
        PostAdmissionSignal::None
    );

    // Input should be queued (StageRunBoundary queues for boundary application)
    if let meerkat_runtime::AcceptOutcome::Accepted { input_id, .. } = &outcome {
        assert_eq!(
            driver.input_phase(input_id),
            Some(InputLifecycleState::Queued)
        );
    }
}

// ---------------------------------------------------------------------------
// §3: Failed response triggers continuation (wake) when idle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn failed_response_idle_wakes() {
    let mut driver = EphemeralRuntimeDriver::new(rid());
    let interaction = make_response("peer-1", ResponseStatus::Failed);
    let input = runtime_input_for_interaction(&interaction, &rid());

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
    assert_machine_owned_admission_signal(&outcome, false, PostAdmissionSignal::WakeLoop);
    assert_eq!(
        driver.take_post_admission_signal(),
        PostAdmissionSignal::None
    );
}

// ---------------------------------------------------------------------------
// §4: Response + passthrough message: both queued
// ---------------------------------------------------------------------------

#[tokio::test]
async fn response_with_passthrough_message_both_queued() {
    let mut driver = EphemeralRuntimeDriver::new(rid());

    // Accept a completed response
    let resp = make_response("peer-1", ResponseStatus::Completed);
    let input1 = runtime_input_for_interaction(&resp, &rid());
    let outcome1 = driver.accept_input(input1).await.unwrap();

    // Accept a message
    let msg = make_message("peer-2", "hello");
    let input2 = runtime_input_for_interaction(&msg, &rid());
    let outcome2 = driver.accept_input(input2).await.unwrap();

    // Both should be queued
    assert_eq!(driver.queue().len(), 2);
    assert_machine_owned_admission_signal(&outcome1, false, PostAdmissionSignal::WakeLoop);
    assert_machine_owned_admission_signal(&outcome2, false, PostAdmissionSignal::WakeLoop);
    assert_eq!(
        driver.take_post_admission_signal(),
        PostAdmissionSignal::None
    );
}

// ---------------------------------------------------------------------------
// §5: Response after completed host turn triggers continuation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn response_after_completed_turn_wakes() {
    let mut driver = EphemeralRuntimeDriver::new(rid());

    // The completed host turn leaves the driver idle before the late terminal
    // response is admitted.
    assert_eq!(driver.runtime_state(), RuntimeState::Idle);

    // Now idle — accept a terminal response
    let resp = make_response("peer-1", ResponseStatus::Completed);
    let input = runtime_input_for_interaction(&resp, &rid());
    let outcome = driver.accept_input(input).await.unwrap();

    assert!(outcome.is_accepted());
    assert_machine_owned_admission_signal(&outcome, false, PostAdmissionSignal::WakeLoop);
    assert_eq!(
        driver.take_post_admission_signal(),
        PostAdmissionSignal::None
    );
}

// ---------------------------------------------------------------------------
// §6: Peer lifecycle batching — multiple peer_added collapse
// ---------------------------------------------------------------------------
#[tokio::test]
async fn peer_lifecycle_accepts_as_requests() {
    let mut driver = EphemeralRuntimeDriver::new(rid());

    // Silent intents (mob.peer_added) are PeerInput with Request convention
    let req1 = make_request("peer-1", "mob.peer_added");
    let input1 = runtime_input_for_interaction(&req1, &rid());

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

    let input1 = runtime_input_for_interaction(&added, &rid());
    let input2 = runtime_input_for_interaction(&retired, &rid());

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
    let input = runtime_input_for_interaction(&interaction, &rid());

    // Under v9, silent intents are PeerInput(Request). The runtime's policy
    // resolves request inputs through the same staged-run wake path; caller
    // intent metadata remains on the typed input rather than a stringly side
    // channel.
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
    let input = runtime_input_for_interaction(&interaction, &rid());

    let policy = DefaultPolicyTable::resolve(&input, true);
    assert_eq!(policy.wake_mode, meerkat_runtime::WakeMode::WakeIfIdle);

    let outcome = driver.accept_input(input).await.unwrap();
    assert!(outcome.is_accepted());
    assert_machine_owned_admission_signal(&outcome, false, PostAdmissionSignal::WakeLoop);
    assert_eq!(
        driver.take_post_admission_signal(),
        PostAdmissionSignal::None
    );
}

// ---------------------------------------------------------------------------
// §10: Message interaction triggers host-mode run
// ---------------------------------------------------------------------------

#[tokio::test]
async fn message_triggers_wake() {
    let mut driver = EphemeralRuntimeDriver::new(rid());

    let interaction = make_message("peer-1", "hello world");
    let input = runtime_input_for_interaction(&interaction, &rid());

    // peer_message + idle → StageRunStart + WakeIfIdle
    let policy = DefaultPolicyTable::resolve(&input, true);
    assert_eq!(policy.apply_mode, meerkat_runtime::ApplyMode::StageRunStart);
    assert_eq!(policy.wake_mode, meerkat_runtime::WakeMode::WakeIfIdle);

    let outcome = driver.accept_input(input).await.unwrap();
    assert!(outcome.is_accepted());
    assert_machine_owned_admission_signal(&outcome, false, PostAdmissionSignal::WakeLoop);
    assert_eq!(
        driver.take_post_admission_signal(),
        PostAdmissionSignal::None
    );
    assert_eq!(driver.queue().len(), 1);
}

// ---------------------------------------------------------------------------
// §11: Request interaction triggers host-mode run
// ---------------------------------------------------------------------------

#[tokio::test]
async fn request_triggers_wake() {
    let mut driver = EphemeralRuntimeDriver::new(rid());

    let interaction = make_request("peer-1", "analyze");
    let input = runtime_input_for_interaction(&interaction, &rid());

    let outcome = driver.accept_input(input).await.unwrap();
    assert!(outcome.is_accepted());
    assert_machine_owned_admission_signal(&outcome, false, PostAdmissionSignal::WakeLoop);
    assert_eq!(
        driver.take_post_admission_signal(),
        PostAdmissionSignal::None
    );
}

#[tokio::test]
async fn request_prompt_uses_rendered_text_projection() {
    let interaction = make_request("peer-1", "custom.action");
    let input = runtime_input_for_interaction(&interaction, &rid());

    if let Input::Peer(peer) = input {
        assert_eq!(peer.body, interaction.rendered_text);
    } else {
        panic!("Expected PeerInput");
    }
}

#[tokio::test]
async fn response_prompt_uses_rendered_text_projection() {
    let interaction = make_response("peer-1", ResponseStatus::Completed);
    let input = runtime_input_for_interaction(&interaction, &rid());

    if let Input::Peer(peer) = input {
        assert_eq!(peer.body, interaction.rendered_text);
    } else {
        panic!("Expected PeerInput");
    }
}

#[tokio::test]
async fn message_blocks_survive_bridge() {
    let interaction = make_message_with_blocks("peer-1", "look");
    let input = runtime_input_for_interaction(&interaction, &rid());

    if let Input::Peer(peer) = input {
        assert!(peer.blocks.is_some());
        // peer.body is the canonical rendered projection, while blocks preserve
        // the original multimodal content.
        assert_eq!(peer.body, interaction.rendered_text);
    } else {
        panic!("Expected PeerInput");
    }
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
// Additional: Message while running with explicit Queue — queue policy, no interrupt
// ---------------------------------------------------------------------------

#[tokio::test]
async fn message_while_running_with_explicit_queue_stays_queued() {
    let mut driver = EphemeralRuntimeDriver::new(rid());

    // Start a run
    bind_running(&mut driver);

    let interaction = make_message("peer-1", "hello");
    let input = runtime_input_for_interaction(&interaction, &rid());

    // peer_message + running + explicit Queue → StageRunStart + no interrupt
    let policy = DefaultPolicyTable::resolve(&input, false);
    assert_eq!(policy.apply_mode, meerkat_runtime::ApplyMode::StageRunStart);
    assert_eq!(policy.wake_mode, meerkat_runtime::WakeMode::None);

    let outcome = driver.accept_input(input).await.unwrap();
    assert!(outcome.is_accepted());
    assert_eq!(
        post_admission_signal_from_accept_outcome(&outcome, false),
        PostAdmissionSignal::None
    );
    assert_eq!(
        driver.take_post_admission_signal(),
        PostAdmissionSignal::None
    );
}

// ---------------------------------------------------------------------------
// Additional: Steered message while running — cooperative interrupt
// ---------------------------------------------------------------------------

#[tokio::test]
async fn message_with_steer_while_running_requests_cooperative_interrupt() {
    let mut driver = EphemeralRuntimeDriver::new(rid());

    // Start a run
    bind_running(&mut driver);

    let mut interaction = make_message("peer-1", "hello");
    interaction.handling_mode = meerkat_core::types::HandlingMode::Steer;
    let input = runtime_input_for_interaction(&interaction, &rid());

    // peer_message + explicit steer + running → StageRunBoundary + cooperative interrupt
    let policy = DefaultPolicyTable::resolve(&input, false);
    assert_eq!(
        policy.apply_mode,
        meerkat_runtime::ApplyMode::StageRunBoundary
    );
    assert_eq!(
        policy.wake_mode,
        meerkat_runtime::WakeMode::InterruptYielding
    );

    let outcome = driver.accept_input(input).await.unwrap();
    assert!(outcome.is_accepted());
    assert_eq!(
        post_admission_signal_from_accept_outcome(&outcome, true),
        PostAdmissionSignal::RequestImmediateProcessing
    );
    assert_eq!(
        driver.take_post_admission_signal(),
        PostAdmissionSignal::None
    );
}

#[tokio::test]
async fn message_without_steer_while_running_requests_idle_wake() {
    let mut driver = EphemeralRuntimeDriver::new(rid());

    bind_running(&mut driver);

    let interaction = make_message("peer-1", "hello");
    let mut input = runtime_input_for_interaction(&interaction, &rid());
    if let Input::Peer(peer) = &mut input {
        peer.handling_mode = None;
    }

    // A default peer message is ordinary queued work. It should wake the
    // runtime after the active turn, not cancel that turn at its next boundary.
    let policy = DefaultPolicyTable::resolve(&input, false);
    assert_eq!(policy.apply_mode, meerkat_runtime::ApplyMode::StageRunStart);
    assert_eq!(policy.wake_mode, meerkat_runtime::WakeMode::WakeIfIdle);

    let outcome = driver.accept_input(input).await.unwrap();
    assert!(outcome.is_accepted());
    assert_machine_owned_admission_signal(&outcome, false, PostAdmissionSignal::WakeLoop);
    assert_eq!(
        driver.take_post_admission_signal(),
        PostAdmissionSignal::WakeLoop
    );
}

// ---------------------------------------------------------------------------
// Additional: Terminal response while running — staged for the next run and
// guaranteed to wake the loop once the active turn reaches idle.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn terminal_response_while_running_requests_idle_wake() {
    let mut driver = EphemeralRuntimeDriver::new(rid());

    bind_running(&mut driver);

    let interaction = make_response("peer-1", ResponseStatus::Completed);
    let input = runtime_input_for_interaction(&interaction, &rid());

    // peer_response_terminal + running → StageRunStart + WakeIfIdle.
    // This is intentionally not an interrupt. The wake only guarantees the
    // loop re-checks the queue when the current run settles back to idle.
    let policy = DefaultPolicyTable::resolve(&input, false);
    assert_eq!(policy.apply_mode, meerkat_runtime::ApplyMode::StageRunStart);
    assert_eq!(policy.wake_mode, meerkat_runtime::WakeMode::WakeIfIdle);

    let outcome = driver.accept_input(input).await.unwrap();
    assert!(outcome.is_accepted());
    assert_machine_owned_admission_signal(&outcome, false, PostAdmissionSignal::WakeLoop);
    assert_eq!(
        driver.take_post_admission_signal(),
        PostAdmissionSignal::WakeLoop
    );
}

// ---------------------------------------------------------------------------
// §13: Terminal response produces exactly one Peer input — no synthetic Continuation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn drain_terminal_response_produces_exactly_one_peer_input() {
    let mut driver = EphemeralRuntimeDriver::new(rid());

    // Build a terminal response interaction and convert to runtime input.
    let interaction = make_response("peer-1", ResponseStatus::Completed);
    let input = runtime_input_for_interaction(&interaction, &rid());

    // The bridge must produce a Peer input, not a Continuation.
    assert!(
        matches!(&input, Input::Peer(_)),
        "terminal response must map to Peer, got {:?}",
        input.kind_id()
    );

    // Accept through the driver.
    let outcome = driver.accept_input(input).await.unwrap();
    assert!(outcome.is_accepted());

    // Exactly 1 input in the queue — zero Continuations.
    assert_eq!(
        driver.queue().len(),
        1,
        "terminal response must produce exactly 1 queued input"
    );

    // Verify the queued input is a Peer with ResponseTerminal convention.
    let queued_ids = driver.queue().input_ids();
    let queued_state = driver.input_state(&queued_ids[0]).unwrap();
    if let Some(Input::Peer(peer)) = &queued_state.persisted_input {
        assert!(
            matches!(
                peer.convention,
                Some(PeerConvention::ResponseTerminal { .. })
            ),
            "queued input must be ResponseTerminal"
        );
    }
}

// ---------------------------------------------------------------------------
// §14: Terminal response + Steer handling_mode while running
// ---------------------------------------------------------------------------
#[tokio::test]
async fn terminal_response_with_steer_policy_while_running() {
    // Build a terminal response with Steer handling_mode.
    let in_reply_to = iid();
    let interaction = InboxInteraction {
        from_route: Some(response_route_id()),
        id: iid(),
        from: "peer-1".into(),
        content: InteractionContent::Response {
            in_reply_to,
            status: ResponseStatus::Completed,
            result: serde_json::json!({"ok": true}),
            blocks: None,
        },
        rendered_text: "[peer-1]: response (Completed)".into(),
        handling_mode: meerkat_core::types::HandlingMode::Steer,
        render_metadata: None,
    };
    let input = runtime_input_for_interaction(&interaction, &rid());

    // While running: explicit steer uses cooperative interrupt semantics at the
    // policy layer, and ingress still requests immediate processing via the
    // typed steer signal. The terminal apply intent remains StageRunStart.
    let policy = DefaultPolicyTable::resolve(&input, false);
    assert_eq!(
        policy.wake_mode,
        meerkat_runtime::WakeMode::InterruptYielding
    );
    assert_eq!(
        policy.routing_disposition,
        meerkat_runtime::RoutingDisposition::Steer
    );
    assert_eq!(policy.apply_mode, meerkat_runtime::ApplyMode::StageRunStart);

    // Verify driver behavior while running.
    let mut driver = EphemeralRuntimeDriver::new(rid());
    bind_running(&mut driver);
    let outcome = driver.accept_input(input).await.unwrap();
    assert!(outcome.is_accepted());
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

// ---------------------------------------------------------------------------
// §15: Terminal response + Steer handling_mode while idle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn terminal_response_with_steer_policy_while_idle() {
    // Build a terminal response with Steer handling_mode.
    let in_reply_to = iid();
    let interaction = InboxInteraction {
        from_route: Some(response_route_id()),
        id: iid(),
        from: "peer-1".into(),
        content: InteractionContent::Response {
            in_reply_to,
            status: ResponseStatus::Completed,
            result: serde_json::json!({"ok": true}),
            blocks: None,
        },
        rendered_text: "[peer-1]: response (Completed)".into(),
        handling_mode: meerkat_core::types::HandlingMode::Steer,
        render_metadata: None,
    };
    let input = runtime_input_for_interaction(&interaction, &rid());

    // While idle: should get WakeIfIdle + Steer with the machine-owned
    // terminal apply intent preserved as StageRunStart.
    let policy = DefaultPolicyTable::resolve(&input, true);
    assert_eq!(policy.wake_mode, meerkat_runtime::WakeMode::WakeIfIdle);
    assert_eq!(
        policy.routing_disposition,
        meerkat_runtime::RoutingDisposition::Steer
    );
    assert_eq!(policy.apply_mode, meerkat_runtime::ApplyMode::StageRunStart);

    // Verify driver behavior while idle.
    let mut driver = EphemeralRuntimeDriver::new(rid());
    let outcome = driver.accept_input(input).await.unwrap();
    assert!(outcome.is_accepted());
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

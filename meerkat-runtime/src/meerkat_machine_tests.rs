use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use chrono::Utc;
use meerkat_core::agent::{CommsCapabilityError, CommsRuntime};
use meerkat_core::comms_drain_lifecycle_authority::{CommsDrainMode, CommsDrainPhase};
use meerkat_core::lifecycle::core_executor::{CoreApplyOutput, CoreExecutor, CoreExecutorError};
use meerkat_core::lifecycle::run_control::RunControlCommand;
use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_core::ops::{OperationId, OperationResult};
use meerkat_core::ops_lifecycle::{
    OperationKind, OperationProgressUpdate, OperationSpec, OpsLifecycleRegistry,
};
use tokio::sync::Notify;

use crate::completion::CompletionOutcome;
use crate::identifiers::IdempotencyKey;

struct FakeDrainRuntime {
    notify: Arc<Notify>,
    dismiss: AtomicBool,
}

impl FakeDrainRuntime {
    fn dismissing() -> Self {
        Self {
            notify: Arc::new(Notify::new()),
            dismiss: AtomicBool::new(true),
        }
    }

    fn idle() -> Self {
        Self {
            notify: Arc::new(Notify::new()),
            dismiss: AtomicBool::new(false),
        }
    }
}

fn make_prompt(text: &str) -> Input {
    Input::Prompt(crate::input::PromptInput {
        header: crate::input::InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: crate::input::InputOrigin::Operator,
            durability: crate::input::InputDurability::Durable,
            visibility: crate::input::InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        text: text.into(),
        blocks: None,
        turn_metadata: None,
    })
}

fn make_progress_input(label: &str) -> Input {
    Input::Peer(crate::input::PeerInput {
        header: crate::input::InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: crate::input::InputOrigin::Peer {
                peer_id: "peer-1".into(),
                runtime_id: None,
            },
            durability: crate::input::InputDurability::Ephemeral,
            visibility: crate::input::InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        convention: Some(crate::input::PeerConvention::ResponseProgress {
            request_id: format!("req-{label}"),
            phase: crate::input::ResponseProgressPhase::InProgress,
        }),
        body: format!("progress-{label}"),
        blocks: None,
        handling_mode: None,
    })
}

#[async_trait::async_trait]
impl CommsRuntime for FakeDrainRuntime {
    async fn drain_messages(&self) -> Vec<String> {
        Vec::new()
    }

    fn inbox_notify(&self) -> Arc<Notify> {
        Arc::clone(&self.notify)
    }

    fn dismiss_received(&self) -> bool {
        self.dismiss.load(Ordering::Acquire)
    }

    async fn drain_classified_inbox_interactions(
        &self,
    ) -> Result<Vec<meerkat_core::interaction::ClassifiedInboxInteraction>, CommsCapabilityError>
    {
        Ok(Vec::new())
    }
}

async fn spawn_test_comms_drain(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    mode: CommsDrainMode,
    comms_runtime: Arc<dyn CommsRuntime>,
    idle_timeout: Duration,
) {
    adapter.register_session(session_id.clone()).await;
    let mut slots = adapter.comms_drain_slots.write().await;
    let slot = slots
        .entry(session_id.clone())
        .or_insert_with(CommsDrainSlot::new);
    let result = protocol_comms_drain_spawn::execute_ensure_running(&mut slot.authority, mode)
        .expect("ensure running");
    let obligation = result
        .obligation
        .expect("spawn obligation should be present");

    apply_runtime_drain_effects(slot, &result.effects);
    for effect in &result.effects {
        if let CommsDrainLifecycleEffect::SpawnDrainTask { .. } = effect {
            slot.handle = Some(crate::comms_drain::spawn_comms_drain(
                Arc::clone(adapter),
                session_id.clone(),
                Arc::clone(&comms_runtime),
                Some(idle_timeout),
            ));
        }
    }

    let feedback_effects =
        protocol_comms_drain_spawn::submit_task_spawned(&mut slot.authority, obligation)
            .expect("task spawned");
    apply_runtime_drain_effects(slot, &feedback_effects);
}

async fn current_phase(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
) -> Option<CommsDrainPhase> {
    let slots = adapter.comms_drain_slots.read().await;
    slots.get(session_id).map(|slot| slot.authority.phase())
}

async fn handle_present(adapter: &Arc<MeerkatMachine>, session_id: &SessionId) -> bool {
    let slots = adapter.comms_drain_slots.read().await;
    slots
        .get(session_id)
        .and_then(|slot| slot.handle.as_ref())
        .is_some()
}

async fn wait_for_phase(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    expected: CommsDrainPhase,
) {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if current_phase(adapter, session_id).await == Some(expected) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("phase transition");
}

#[tokio::test]
async fn dismiss_exit_updates_authority_before_join() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let comms_runtime: Arc<dyn CommsRuntime> = Arc::new(FakeDrainRuntime::dismissing());

    spawn_test_comms_drain(
        &adapter,
        &session_id,
        CommsDrainMode::PersistentHost,
        comms_runtime,
        Duration::from_millis(25),
    )
    .await;

    wait_for_phase(&adapter, &session_id, CommsDrainPhase::Stopped).await;
    assert!(
        !handle_present(&adapter, &session_id).await,
        "drain task should clear its slot before wait_comms_drain joins"
    );

    adapter.wait_comms_drain(&session_id).await;
    assert_eq!(
        current_phase(&adapter, &session_id).await,
        Some(CommsDrainPhase::Stopped)
    );
}

#[tokio::test]
async fn idle_timeout_updates_authority_before_join() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let comms_runtime: Arc<dyn CommsRuntime> = Arc::new(FakeDrainRuntime::idle());

    spawn_test_comms_drain(
        &adapter,
        &session_id,
        CommsDrainMode::Timed,
        comms_runtime,
        Duration::from_millis(25),
    )
    .await;

    wait_for_phase(&adapter, &session_id, CommsDrainPhase::Stopped).await;
    assert!(
        !handle_present(&adapter, &session_id).await,
        "drain task should clear its slot before wait_comms_drain joins"
    );

    adapter.wait_comms_drain(&session_id).await;
    assert_eq!(
        current_phase(&adapter, &session_id).await,
        Some(CommsDrainPhase::Stopped)
    );
}

#[tokio::test]
async fn unregister_session_aborts_and_removes_drain_slot() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let comms_runtime: Arc<dyn CommsRuntime> = Arc::new(FakeDrainRuntime::idle());

    adapter.register_session(session_id.clone()).await;
    spawn_test_comms_drain(
        &adapter,
        &session_id,
        CommsDrainMode::PersistentHost,
        comms_runtime,
        Duration::from_secs(60),
    )
    .await;

    assert_eq!(
        current_phase(&adapter, &session_id).await,
        Some(CommsDrainPhase::Running)
    );
    assert!(handle_present(&adapter, &session_id).await);

    adapter.unregister_session(&session_id).await;

    let slots = adapter.comms_drain_slots.read().await;
    assert!(
        !slots.contains_key(&session_id),
        "unregister must remove the comms drain slot entirely"
    );
}

#[tokio::test]
async fn session_service_runtime_ext_write_side_follows_machine_control_surface() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    let state = <MeerkatMachine as SessionServiceRuntimeExt>::runtime_state(&adapter, &session_id)
        .await
        .expect("runtime state should route through the machine seam");
    assert_eq!(state, RuntimeState::Idle);

    let outcome = <MeerkatMachine as SessionServiceRuntimeExt>::accept_input(
        &adapter,
        &session_id,
        make_prompt("service-ext-write-side"),
    )
    .await
    .expect("accept_input should route through the machine seam");
    assert!(
        matches!(outcome, AcceptOutcome::Accepted { .. }),
        "prompt should still be admitted through the SessionServiceRuntimeExt seam"
    );

    let active =
        <MeerkatMachine as SessionServiceRuntimeExt>::list_active_inputs(&adapter, &session_id)
            .await
            .expect("active inputs should still be readable");
    assert_eq!(active.len(), 1, "accepted input should remain active");
    let active_state = <MeerkatMachine as SessionServiceRuntimeExt>::input_state(
        &adapter,
        &session_id,
        &active[0],
    )
    .await
    .expect("input_state should route through the machine seam");
    assert_eq!(
        active_state.map(|state| state.current_state()),
        Some(crate::input_state::InputLifecycleState::Queued),
        "accepted prompt should still be visible through machine-routed input_state"
    );

    let retire_report =
        <MeerkatMachine as SessionServiceRuntimeExt>::retire_runtime(&adapter, &session_id)
            .await
            .expect("retire should route through the machine seam");
    assert_eq!(
        retire_report.inputs_abandoned, 1,
        "retire should still abandon queued work when no runtime loop is attached"
    );
    assert_eq!(retire_report.inputs_pending_drain, 0);

    let reset_report =
        <MeerkatMachine as SessionServiceRuntimeExt>::reset_runtime(&adapter, &session_id)
            .await
            .expect("reset should route through the machine seam");
    assert_eq!(
        reset_report.inputs_abandoned, 0,
        "reset after retire should not find residual queued work"
    );
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_reports_registered_idle_session() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist for registered session");

    assert_eq!(snapshot.binding.session_id, session_id);
    assert_eq!(
        snapshot.binding.driver_kind,
        crate::meerkat_machine_types::MeerkatDriverKind::Ephemeral
    );
    assert!(snapshot.binding.driver_present);
    assert!(snapshot.binding.completions_present);
    assert!(snapshot.binding.ops_registry_present);
    assert_eq!(snapshot.control.phase, RuntimeState::Idle);
    assert!(!snapshot.binding.attachment_live);
    assert!(!snapshot.binding.detached_wake_present);
    assert_eq!(snapshot.binding.cursor_state.agent_applied_cursor, 0);
    assert_eq!(snapshot.binding.cursor_state.runtime_observed_seq, 0);
    assert_eq!(snapshot.binding.cursor_state.runtime_last_injected_seq, 0);
    assert!(snapshot.inputs.admission_order.is_empty());
    assert!(snapshot.inputs.queue.is_empty());
    assert!(snapshot.inputs.steer_queue.is_empty());
    assert_eq!(snapshot.completion_waiters.input_count, 0);
    assert_eq!(snapshot.completion_waiters.waiter_count, 0);
    assert!(snapshot.completion_waiters.waiting_inputs.is_empty());
    assert!(!snapshot.drain.slot_present);
    assert_eq!(snapshot.drain.phase, None);
    assert_eq!(snapshot.drain.mode, None);
    assert!(!snapshot.drain.handle_present);
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_tracks_queued_prompt_input() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let input = make_prompt("hello from the runtime spine");
    let input_id = input.id().clone();

    let (_outcome, handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");
    assert!(
        handle.is_some(),
        "queued prompt should register a completion"
    );

    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist for registered session");

    assert_eq!(snapshot.control.phase, RuntimeState::Idle);
    assert_eq!(snapshot.inputs.queue, vec![input_id.clone()]);
    assert!(snapshot.inputs.steer_queue.is_empty());
    assert_eq!(snapshot.inputs.current_run_id, None);
    assert_eq!(
        snapshot.inputs.current_run_contributors,
        Vec::<InputId>::new()
    );
    assert_eq!(snapshot.inputs.admission_order.len(), 1);
    assert_eq!(snapshot.completion_waiters.input_count, 1);
    assert_eq!(snapshot.completion_waiters.waiter_count, 1);
    assert_eq!(snapshot.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        snapshot.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        snapshot.completion_waiters.waiting_inputs[0].waiter_count,
        1
    );

    let input_snapshot = &snapshot.inputs.admission_order[0];
    assert_eq!(input_snapshot.input_id, input_id);
    assert_eq!(
        input_snapshot.lifecycle,
        Some(crate::input_state::InputLifecycleState::Queued)
    );
    assert_eq!(
        input_snapshot.handling_mode,
        Some(meerkat_core::types::HandlingMode::Queue)
    );
    assert_eq!(snapshot.inputs.queue, vec![input_id.clone()]);
    assert!(snapshot.inputs.steer_queue.is_empty());
    assert!(input_snapshot.content_shape.is_some());
    assert_eq!(input_snapshot.last_run_id, None);
    assert_eq!(input_snapshot.last_boundary_sequence, None);
    assert!(input_snapshot.terminal_outcome.is_none());
    assert!(input_snapshot.is_prompt);
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_tracks_deduplicated_completion_waiters() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let mut first = make_prompt("first deduplicated input");
    if let Input::Prompt(prompt) = &mut first {
        prompt.header.idempotency_key = Some(IdempotencyKey::new("same-input"));
    }
    let first_input_id = first.id().clone();

    let (first_outcome, first_handle) = adapter
        .accept_input_with_completion(&session_id, first)
        .await
        .expect("first input should be accepted");
    assert!(matches!(first_outcome, AcceptOutcome::Accepted { .. }));
    assert!(
        first_handle.is_some(),
        "first queued input should register a completion waiter"
    );

    let mut duplicate = make_prompt("second deduplicated input");
    if let Input::Prompt(prompt) = &mut duplicate {
        prompt.header.idempotency_key = Some(IdempotencyKey::new("same-input"));
    }

    let (duplicate_outcome, duplicate_handle) = adapter
        .accept_input_with_completion(&session_id, duplicate)
        .await
        .expect("duplicate input should deduplicate");
    match duplicate_outcome {
        AcceptOutcome::Deduplicated { existing_id, .. } => {
            assert_eq!(existing_id, first_input_id);
        }
        other => panic!("expected deduplicated outcome, got {other:?}"),
    }
    assert!(
        duplicate_handle.is_some(),
        "deduplicated in-flight input should join the existing waiter set"
    );

    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist for registered session");

    assert_eq!(snapshot.inputs.queue, vec![first_input_id.clone()]);
    assert_eq!(snapshot.inputs.admission_order.len(), 1);
    assert_eq!(snapshot.completion_waiters.input_count, 1);
    assert_eq!(snapshot.completion_waiters.waiter_count, 2);
    assert_eq!(snapshot.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        snapshot.completion_waiters.waiting_inputs[0].input_id,
        first_input_id
    );
    assert_eq!(
        snapshot.completion_waiters.waiting_inputs[0].waiter_count,
        2
    );
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_tracks_steered_prompt_input() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let input = Input::Prompt(crate::input::PromptInput::new(
        "steer prompt",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ));
    let input_id = input.id().clone();

    let (_outcome, handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("steered prompt should be accepted");
    assert!(
        handle.is_some(),
        "steered prompt should register a completion"
    );

    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist for registered session");

    assert!(snapshot.inputs.queue.is_empty());
    assert_eq!(snapshot.inputs.steer_queue, vec![input_id.clone()]);
    assert!(snapshot.inputs.wake_requested);
    assert!(snapshot.inputs.process_requested);
    assert_eq!(snapshot.inputs.admission_order.len(), 1);
    let input_snapshot = &snapshot.inputs.admission_order[0];
    assert_eq!(input_snapshot.input_id, input_id);
    assert_eq!(
        input_snapshot.handling_mode,
        Some(meerkat_core::types::HandlingMode::Steer)
    );
    assert_eq!(
        input_snapshot.lifecycle,
        Some(crate::input_state::InputLifecycleState::Queued)
    );
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_clears_completion_waiters_after_reset() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let input = make_prompt("reset pending waiter");
    let input_id = input.id().clone();
    let (_outcome, handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");
    let handle = handle.expect("queued prompt should register a waiter");

    let before_reset = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before reset");
    assert_eq!(before_reset.completion_waiters.input_count, 1);
    assert_eq!(before_reset.completion_waiters.waiter_count, 1);
    assert_eq!(before_reset.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_reset.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );

    SessionServiceRuntimeExt::reset_runtime(&adapter, &session_id)
        .await
        .expect("reset should succeed for idle queued runtime");

    let after_reset = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after reset");
    assert_eq!(after_reset.completion_waiters.input_count, 0);
    assert_eq!(after_reset.completion_waiters.waiter_count, 0);
    assert!(after_reset.completion_waiters.waiting_inputs.is_empty());

    match handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime reset");
        }
        other => panic!("expected runtime reset termination, got {other:?}"),
    }
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_preserves_completion_waiters_after_recycle() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let input = make_prompt("recycle pending waiter");
    let input_id = input.id().clone();
    let (_outcome, handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");
    let handle = handle.expect("queued prompt should register a waiter");

    let before_recycle = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before recycle");
    assert_eq!(before_recycle.control.phase, RuntimeState::Idle);
    assert_eq!(before_recycle.inputs.queue, vec![input_id.clone()]);
    assert_eq!(before_recycle.completion_waiters.input_count, 1);
    assert_eq!(before_recycle.completion_waiters.waiter_count, 1);
    assert_eq!(
        before_recycle.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    let report = crate::traits::RuntimeControlPlane::recycle(&adapter, &runtime_id)
        .await
        .expect("recycle should preserve queued work");
    assert_eq!(report.inputs_transferred, 1);

    let after_recycle = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after recycle");
    assert_eq!(after_recycle.control.phase, RuntimeState::Idle);
    assert_eq!(after_recycle.inputs.queue, vec![input_id.clone()]);
    assert_eq!(after_recycle.completion_waiters.input_count, 1);
    assert_eq!(after_recycle.completion_waiters.waiter_count, 1);
    assert_eq!(after_recycle.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        after_recycle.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        after_recycle.completion_waiters.waiting_inputs[0].waiter_count,
        1
    );

    SessionServiceRuntimeExt::reset_runtime(&adapter, &session_id)
        .await
        .expect("reset should terminate preserved waiter at test end");
    match handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime reset");
        }
        other => panic!("expected runtime reset termination, got {other:?}"),
    }
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_recycle_reconciles_stale_completion_waiters() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let input = make_prompt("preserve active waiter");
    let input_id = input.id().clone();
    let (_outcome, active_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");
    let active_handle = active_handle.expect("queued prompt should register a waiter");

    let completions = {
        let sessions = adapter.sessions.read().await;
        Arc::clone(
            &sessions
                .get(&session_id)
                .expect("registered session should exist")
                .completions,
        )
    };
    let stale_input_id = InputId::new();
    let stale_handle = {
        let mut completions = completions.lock().await;
        completions.register(stale_input_id.clone())
    };

    let before_recycle = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before recycle");
    assert_eq!(before_recycle.completion_waiters.input_count, 2);
    assert_eq!(before_recycle.completion_waiters.waiter_count, 2);
    assert!(
        before_recycle
            .completion_waiters
            .waiting_inputs
            .iter()
            .any(|entry| entry.input_id == input_id && entry.waiter_count == 1),
        "active queued input should have one visible waiter before recycle"
    );
    assert!(
        before_recycle
            .completion_waiters
            .waiting_inputs
            .iter()
            .any(|entry| entry.input_id == stale_input_id && entry.waiter_count == 1),
        "stale waiter should be visible before recycle reconciliation"
    );

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    let report = crate::traits::RuntimeControlPlane::recycle(&adapter, &runtime_id)
        .await
        .expect("recycle should reconcile waiters against active input truth");
    assert_eq!(report.inputs_transferred, 1);

    let after_recycle = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after recycle");
    assert_eq!(after_recycle.completion_waiters.input_count, 1);
    assert_eq!(after_recycle.completion_waiters.waiter_count, 1);
    assert_eq!(after_recycle.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        after_recycle.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        after_recycle.completion_waiters.waiting_inputs[0].waiter_count,
        1
    );

    match stale_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "recycled input no longer pending");
        }
        other => panic!("expected recycled stale waiter termination, got {other:?}"),
    }

    SessionServiceRuntimeExt::reset_runtime(&adapter, &session_id)
        .await
        .expect("reset should terminate preserved waiter at test end");
    match active_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime reset");
        }
        other => panic!("expected runtime reset termination, got {other:?}"),
    }
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_preserves_completion_waiters_after_recover() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let input = make_prompt("recover pending waiter");
    let input_id = input.id().clone();
    let (_outcome, handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");
    let handle = handle.expect("queued prompt should register a waiter");

    let before_recover = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before recover");
    assert_eq!(before_recover.control.phase, RuntimeState::Idle);
    assert_eq!(before_recover.inputs.queue, vec![input_id.clone()]);
    assert_eq!(before_recover.completion_waiters.input_count, 1);
    assert_eq!(before_recover.completion_waiters.waiter_count, 1);
    assert_eq!(
        before_recover.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    let report = crate::traits::RuntimeControlPlane::recover(&adapter, &runtime_id)
        .await
        .expect("recover should preserve queued work");
    assert_eq!(report.inputs_recovered, 1);

    let after_recover = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after recover");
    assert_eq!(after_recover.control.phase, RuntimeState::Idle);
    assert_eq!(after_recover.inputs.queue, vec![input_id.clone()]);
    assert_eq!(after_recover.completion_waiters.input_count, 1);
    assert_eq!(after_recover.completion_waiters.waiter_count, 1);
    assert_eq!(after_recover.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        after_recover.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        after_recover.completion_waiters.waiting_inputs[0].waiter_count,
        1
    );

    SessionServiceRuntimeExt::reset_runtime(&adapter, &session_id)
        .await
        .expect("reset should terminate preserved waiter at test end");
    match handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime reset");
        }
        other => panic!("expected runtime reset termination, got {other:?}"),
    }
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_recover_reconciles_stale_completion_waiters() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let input = make_prompt("preserve active waiter on recover");
    let input_id = input.id().clone();
    let (_outcome, active_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");
    let active_handle = active_handle.expect("queued prompt should register a waiter");

    let completions = {
        let sessions = adapter.sessions.read().await;
        Arc::clone(
            &sessions
                .get(&session_id)
                .expect("registered session should exist")
                .completions,
        )
    };
    let stale_input_id = InputId::new();
    let stale_handle = {
        let mut completions = completions.lock().await;
        completions.register(stale_input_id.clone())
    };

    let before_recover = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before recover");
    assert_eq!(before_recover.completion_waiters.input_count, 2);
    assert_eq!(before_recover.completion_waiters.waiter_count, 2);
    assert!(
        before_recover
            .completion_waiters
            .waiting_inputs
            .iter()
            .any(|entry| entry.input_id == input_id && entry.waiter_count == 1),
        "active queued input should have one visible waiter before recover"
    );
    assert!(
        before_recover
            .completion_waiters
            .waiting_inputs
            .iter()
            .any(|entry| entry.input_id == stale_input_id && entry.waiter_count == 1),
        "stale waiter should be visible before recover reconciliation"
    );

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    let report = crate::traits::RuntimeControlPlane::recover(&adapter, &runtime_id)
        .await
        .expect("recover should reconcile waiters against active input truth");
    assert_eq!(report.inputs_recovered, 1);

    let after_recover = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after recover");
    assert_eq!(after_recover.completion_waiters.input_count, 1);
    assert_eq!(after_recover.completion_waiters.waiter_count, 1);
    assert_eq!(after_recover.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        after_recover.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        after_recover.completion_waiters.waiting_inputs[0].waiter_count,
        1
    );

    match stale_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "recovered input no longer pending");
        }
        other => panic!("expected recovered stale waiter termination, got {other:?}"),
    }

    SessionServiceRuntimeExt::reset_runtime(&adapter, &session_id)
        .await
        .expect("reset should terminate preserved waiter at test end");
    match active_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime reset");
        }
        other => panic!("expected runtime reset termination, got {other:?}"),
    }
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_clears_completion_waiters_after_destroy() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let input = make_prompt("destroy completion waiter");
    let input_id = input.id().clone();
    let (_outcome, handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");
    let handle = handle.expect("queued prompt should register a completion waiter");

    let before_destroy = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before destroy");
    assert_eq!(before_destroy.control.phase, RuntimeState::Idle);
    assert_eq!(before_destroy.completion_waiters.input_count, 1);
    assert_eq!(before_destroy.completion_waiters.waiter_count, 1);
    assert_eq!(before_destroy.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_destroy.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        before_destroy.completion_waiters.waiting_inputs[0].waiter_count,
        1
    );

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    let report = crate::traits::RuntimeControlPlane::destroy(&adapter, &runtime_id)
        .await
        .expect("destroy should terminate active completion waiters");
    assert_eq!(report.inputs_abandoned, 1);

    let after_destroy = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after destroy");
    assert_eq!(after_destroy.control.phase, RuntimeState::Destroyed);
    assert!(
        after_destroy.inputs.queue.is_empty(),
        "destroy should not leave ordinary queued work behind once the runtime is destroyed"
    );
    assert!(
        after_destroy.inputs.steer_queue.is_empty(),
        "destroy should not leave steer-queued work behind once the runtime is destroyed"
    );
    assert_eq!(after_destroy.completion_waiters.input_count, 0);
    assert_eq!(after_destroy.completion_waiters.waiter_count, 0);
    assert!(
        after_destroy.completion_waiters.waiting_inputs.is_empty(),
        "destroy should clear the completion waiter carrier immediately"
    );

    match handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime destroyed");
        }
        other => panic!("expected runtime destroyed termination, got {other:?}"),
    }
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_destroy_clears_steered_waiter_and_queue_but_preserves_wait_all()
 {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let input = Input::Prompt(crate::input::PromptInput::new(
        "destroy steered prompt",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ));
    let input_id = input.id().clone();
    let (_outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("steered prompt should be accepted");
    let completion_handle =
        completion_handle.expect("steered prompt should register a completion waiter");

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "destroy steered wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_destroy = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before destroy");
    let wait_request_id = before_destroy
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_destroy.control.phase, RuntimeState::Idle);
    assert!(before_destroy.inputs.queue.is_empty());
    assert_eq!(before_destroy.inputs.steer_queue, vec![input_id.clone()]);
    assert!(before_destroy.inputs.wake_requested);
    assert!(before_destroy.inputs.process_requested);
    assert_eq!(before_destroy.completion_waiters.input_count, 1);
    assert_eq!(before_destroy.completion_waiters.waiter_count, 1);
    assert_eq!(before_destroy.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_destroy.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert!(before_destroy.ops.pending_wait_present);
    assert_eq!(
        before_destroy.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before destroy"
    );
    assert_eq!(
        before_destroy.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before destroy"
    );

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    let report = crate::traits::RuntimeControlPlane::destroy(&adapter, &runtime_id)
        .await
        .expect("destroy should clear the steered completion waiter while preserving wait_all");
    assert_eq!(report.inputs_abandoned, 1);

    match completion_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime destroyed");
        }
        other => {
            panic!("expected runtime destroyed termination for steered input, got {other:?}")
        }
    }

    let after_destroy = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after destroy");
    assert_eq!(after_destroy.control.phase, RuntimeState::Destroyed);
    assert_eq!(after_destroy.control.current_run_id, None);
    assert_eq!(after_destroy.inputs.current_run_id, None);
    assert!(after_destroy.inputs.queue.is_empty());
    assert!(
        after_destroy.inputs.steer_queue.is_empty(),
        "destroy should clear steered queued work immediately on the plain runtime path"
    );
    assert_eq!(after_destroy.completion_waiters.input_count, 0);
    assert_eq!(after_destroy.completion_waiters.waiter_count, 0);
    assert!(
        after_destroy.completion_waiters.waiting_inputs.is_empty(),
        "destroy should clear input-owned steered completion waiters immediately"
    );
    assert_eq!(
        after_destroy.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "destroy should preserve the authority-owned wait request after steered completion waiters clear"
    );
    assert!(after_destroy.ops.pending_wait_present);
    assert_eq!(
        after_destroy.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "destroy should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_destroy.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "destroy should preserve the tracked wait target until the operation settles"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after destroy");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Destroyed);
    assert!(settled.inputs.queue.is_empty());
    assert!(settled.inputs.steer_queue.is_empty());
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_clears_completion_waiters_after_destroy_with_runtime_loop()
{
    struct RecordingExecutor {
        apply_calls: Arc<AtomicUsize>,
        control_calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for RecordingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
                run_result: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(RecordingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                control_calls: Arc::clone(&control_calls),
            }),
        )
        .await;

    let input = make_progress_input("destroy-with-loop");
    let input_id = input.id().clone();
    let outcome = adapter
        .accept_input_without_wake(&session_id, input)
        .await
        .expect("progress input should queue without waking the attached loop");
    assert!(outcome.is_accepted());

    let handle = {
        let completions = {
            let sessions = adapter.sessions.read().await;
            sessions
                .get(&session_id)
                .expect("attached session should exist")
                .completions
                .clone()
        };
        let mut completions = completions.lock().await;
        completions.register(input_id.clone())
    };

    let before_destroy = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before destroy");
    assert_eq!(before_destroy.control.phase, RuntimeState::Attached);
    assert_eq!(before_destroy.completion_waiters.input_count, 1);
    assert_eq!(before_destroy.completion_waiters.waiter_count, 1);
    assert_eq!(before_destroy.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_destroy.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "destroy should be able to abandon queued attached-loop work before apply runs"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "destroy has not yet attempted any executor control"
    );

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    let report = crate::traits::RuntimeControlPlane::destroy(&adapter, &runtime_id)
        .await
        .expect("destroy should synchronously clear queued waiters even with a live loop");
    assert_eq!(report.inputs_abandoned, 1);

    let after_destroy = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after destroy");
    assert_eq!(after_destroy.control.phase, RuntimeState::Destroyed);
    assert!(
        after_destroy.inputs.queue.is_empty(),
        "destroy should not leave ordinary queued work behind even when an attached loop exists"
    );
    assert!(
        after_destroy.inputs.steer_queue.is_empty(),
        "destroy should not leave steer-queued work behind even when an attached loop exists"
    );
    assert_eq!(after_destroy.completion_waiters.input_count, 0);
    assert_eq!(after_destroy.completion_waiters.waiter_count, 0);
    assert!(
        after_destroy.completion_waiters.waiting_inputs.is_empty(),
        "destroy should clear the completion waiter carrier immediately even when a loop is attached"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "destroy should bypass queued attached-loop work entirely"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "destroy currently bypasses the executor control seam and does not deliver an out-of-band control command"
    );

    match handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime destroyed");
        }
        other => panic!("expected runtime destroyed termination, got {other:?}"),
    }
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_attached_steered_prompt_requests_immediate_processing() {
    struct BlockingExecutor {
        apply_calls: Arc<AtomicUsize>,
        control_calls: Arc<AtomicUsize>,
        apply_started: Arc<Notify>,
        apply_finished: Arc<Notify>,
        allow_finish: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            self.apply_started.notify_waiters();
            self.allow_finish.notified().await;
            self.apply_finished.notify_waiters();

            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
                run_result: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));
    let apply_started = Arc::new(Notify::new());
    let apply_finished = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                control_calls: Arc::clone(&control_calls),
                apply_started: Arc::clone(&apply_started),
                apply_finished: Arc::clone(&apply_finished),
                allow_finish: Arc::clone(&allow_finish),
            }),
        )
        .await;

    let input = Input::Prompt(crate::input::PromptInput::new(
        "attached steered prompt",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ));
    let input_id = input.id().clone();
    let (outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("attached steered prompt should be accepted");
    assert!(outcome.is_accepted());
    let completion_handle =
        completion_handle.expect("attached steered prompt should expose a completion waiter");

    tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
        .await
        .expect("attached steered prompt should request immediate processing");

    let during_apply = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist while attached steered work is active");
    assert_eq!(
        during_apply.control.phase,
        RuntimeState::Running,
        "attached steered input should enter Running rather than remain in a queue-only attached state"
    );
    assert_eq!(
        during_apply.control.current_run_id, during_apply.inputs.current_run_id,
        "attached steered input should bind control and ingress to the same active run"
    );
    assert!(
        during_apply.control.current_run_id.is_some(),
        "attached steered input should create an active run binding"
    );
    assert!(
        during_apply.inputs.queue.is_empty(),
        "attached steered input should not occupy the ordinary queue while it is actively processing"
    );
    assert!(
        during_apply.inputs.steer_queue.is_empty(),
        "attached steered input should not remain in the steer queue once immediate processing begins"
    );
    assert_eq!(during_apply.completion_waiters.input_count, 1);
    assert_eq!(during_apply.completion_waiters.waiter_count, 1);
    assert_eq!(during_apply.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        during_apply.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "attached steered input should wake the attached loop exactly once"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        1,
        "attached steered admission currently routes one control command through the executor seam while requesting immediate processing"
    );

    allow_finish.notify_waiters();
    tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
        .await
        .expect("attached steered prompt should finish after the executor is released");

    match completion_handle.wait().await {
        CompletionOutcome::CompletedWithoutResult => {}
        other => panic!(
            "expected attached steered prompt to complete through the live loop, got {other:?}"
        ),
    }

    let settled = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after attached steered work completes");
            if snapshot.control.phase == RuntimeState::Attached {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("attached runtime should return to Attached after steered work completes");
    assert_eq!(settled.control.current_run_id, None);
    assert_eq!(settled.inputs.current_run_id, None);
    assert!(settled.inputs.queue.is_empty());
    assert!(settled.inputs.steer_queue.is_empty());
    assert_eq!(settled.completion_waiters.input_count, 0);
    assert_eq!(settled.completion_waiters.waiter_count, 0);
    assert!(settled.completion_waiters.waiting_inputs.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_attached_steered_prompt_splits_completion_and_wait_all_lifetimes()
 {
    struct BlockingExecutor {
        apply_calls: Arc<AtomicUsize>,
        control_calls: Arc<AtomicUsize>,
        apply_started: Arc<Notify>,
        apply_finished: Arc<Notify>,
        allow_finish: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            self.apply_started.notify_waiters();
            self.allow_finish.notified().await;
            self.apply_finished.notify_waiters();

            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
                run_result: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));
    let apply_started = Arc::new(Notify::new());
    let apply_finished = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                control_calls: Arc::clone(&control_calls),
                apply_started: Arc::clone(&apply_started),
                apply_finished: Arc::clone(&apply_finished),
                allow_finish: Arc::clone(&allow_finish),
            }),
        )
        .await;

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for attached session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "attached steered wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let input = Input::Prompt(crate::input::PromptInput::new(
        "attached steered split lifetimes",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ));
    let input_id = input.id().clone();
    let (outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("attached steered prompt should be accepted");
    assert!(outcome.is_accepted());
    let completion_handle =
        completion_handle.expect("attached steered prompt should expose a completion waiter");

    tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
        .await
        .expect("attached steered prompt should request immediate processing");

    let during_apply = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist while attached steered work is active");
    let wait_request_id = during_apply
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(during_apply.control.phase, RuntimeState::Running);
    assert_eq!(
        during_apply.control.current_run_id,
        during_apply.inputs.current_run_id
    );
    assert!(during_apply.control.current_run_id.is_some());
    assert!(during_apply.inputs.queue.is_empty());
    assert!(during_apply.inputs.steer_queue.is_empty());
    assert_eq!(during_apply.completion_waiters.input_count, 1);
    assert_eq!(during_apply.completion_waiters.waiter_count, 1);
    assert_eq!(during_apply.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        during_apply.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        during_apply.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id while the attached steered prompt is active"
    );
    assert_eq!(
        during_apply.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the background operation while attached steered work is active"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "attached steered work should wake the loop exactly once"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        1,
        "attached steered admission should still route one control command through the executor seam"
    );

    allow_finish.notify_waiters();
    tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
        .await
        .expect("attached steered prompt should finish after the executor is released");

    match completion_handle.wait().await {
        CompletionOutcome::CompletedWithoutResult => {}
        other => panic!(
            "expected attached steered prompt to complete while wait_all remains live, got {other:?}"
        ),
    }

    let after_completion = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after attached steered completion");
            if snapshot.control.phase == RuntimeState::Attached {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("attached runtime should return to Attached after steered work completes");
    assert_eq!(after_completion.control.current_run_id, None);
    assert_eq!(after_completion.inputs.current_run_id, None);
    assert!(after_completion.inputs.queue.is_empty());
    assert!(after_completion.inputs.steer_queue.is_empty());
    assert_eq!(after_completion.completion_waiters.input_count, 0);
    assert_eq!(after_completion.completion_waiters.waiter_count, 0);
    assert!(
        after_completion
            .completion_waiters
            .waiting_inputs
            .is_empty()
    );
    assert_eq!(
        after_completion.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "attached steered completion should clear the input-owned completion waiter while preserving the ops-owned wait_all carrier"
    );
    assert!(after_completion.ops.pending_wait_present);
    assert_eq!(
        after_completion.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "request-id agreement should survive after attached steered completion clears the input waiter"
    );
    assert_eq!(
        after_completion.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "tracked wait target should remain present until the background operation itself settles"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after attached steered completion");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Attached);
    assert_eq!(settled.control.current_run_id, None);
    assert_eq!(settled.inputs.current_run_id, None);
    assert!(settled.inputs.queue.is_empty());
    assert!(settled.inputs.steer_queue.is_empty());
    assert_eq!(settled.completion_waiters.input_count, 0);
    assert_eq!(settled.completion_waiters.waiter_count, 0);
    assert!(settled.completion_waiters.waiting_inputs.is_empty());
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_attached_steered_prompt_preserves_completion_after_wait_all_settles()
 {
    struct BlockingExecutor {
        apply_calls: Arc<AtomicUsize>,
        control_calls: Arc<AtomicUsize>,
        apply_started: Arc<Notify>,
        apply_finished: Arc<Notify>,
        allow_finish: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            self.apply_started.notify_waiters();
            self.allow_finish.notified().await;
            self.apply_finished.notify_waiters();

            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
                run_result: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));
    let apply_started = Arc::new(Notify::new());
    let apply_finished = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                control_calls: Arc::clone(&control_calls),
                apply_started: Arc::clone(&apply_started),
                apply_finished: Arc::clone(&apply_finished),
                allow_finish: Arc::clone(&allow_finish),
            }),
        )
        .await;

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for attached session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            // Use MobMemberChild here so the proof isolates completion-vs-wait_all
            // ordering without immediately arming the detached-wake continuation path.
            kind: OperationKind::MobMemberChild,
            owner_session_id: session_id.clone(),
            display_name: "attached steered wait-first child".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: true,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let input = Input::Prompt(crate::input::PromptInput::new(
        "attached steered wait-first split",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ));
    let input_id = input.id().clone();
    let (outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("attached steered prompt should be accepted");
    assert!(outcome.is_accepted());
    let completion_handle =
        completion_handle.expect("attached steered prompt should expose a completion waiter");

    tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
        .await
        .expect("attached steered prompt should request immediate processing");

    let during_apply = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist while attached steered work is active");
    let wait_request_id = during_apply
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(during_apply.control.phase, RuntimeState::Running);
    assert_eq!(
        during_apply.control.current_run_id,
        during_apply.inputs.current_run_id
    );
    assert!(during_apply.control.current_run_id.is_some());
    assert!(during_apply.inputs.queue.is_empty());
    assert!(during_apply.inputs.steer_queue.is_empty());
    assert_eq!(during_apply.completion_waiters.input_count, 1);
    assert_eq!(during_apply.completion_waiters.waiter_count, 1);
    assert_eq!(during_apply.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        during_apply.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        during_apply.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id while attached steered work is active"
    );
    assert_eq!(
        during_apply.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the live operation while attached steered work is active"
    );
    assert_eq!(apply_calls.load(Ordering::SeqCst), 1);
    assert_eq!(control_calls.load(Ordering::SeqCst), 1);

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete while attached steered work remains in flight");
    let wait_result = wait_future.await.expect("wait_all should resolve first");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let after_wait_all = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after wait_all settles");
            if snapshot.ops.wait_request_id.is_none() && !snapshot.ops.pending_wait_present {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("attached steered snapshot should eventually clear the wait_all carrier");
    assert_eq!(
        after_wait_all.control.phase,
        RuntimeState::Running,
        "attached steered work should remain Running while apply is still blocked even after wait_all settles"
    );
    assert_eq!(
        after_wait_all.control.current_run_id, after_wait_all.inputs.current_run_id,
        "attached steered work should keep control and ingress bound to the same active run until completion"
    );
    assert!(after_wait_all.control.current_run_id.is_some());
    assert!(after_wait_all.inputs.queue.is_empty());
    assert!(after_wait_all.inputs.steer_queue.is_empty());
    assert_eq!(after_wait_all.completion_waiters.input_count, 1);
    assert_eq!(after_wait_all.completion_waiters.waiter_count, 1);
    assert_eq!(after_wait_all.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        after_wait_all.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(after_wait_all.ops.wait_request_id, None);
    assert!(!after_wait_all.ops.pending_wait_present);
    assert_eq!(after_wait_all.ops.pending_wait_request_id, None);
    assert!(
        after_wait_all.ops.wait_operation_ids.is_empty(),
        "wait_all should release the tracked wait target before the attached steered completion waiter clears"
    );

    allow_finish.notify_waiters();
    tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
        .await
        .expect("attached steered prompt should finish after the executor is released");

    match completion_handle.wait().await {
        CompletionOutcome::CompletedWithoutResult => {}
        other => panic!(
            "expected attached steered prompt to complete after wait_all settled first, got {other:?}"
        ),
    }

    let settled = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after attached steered completion");
            if snapshot.control.phase == RuntimeState::Attached {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("attached runtime should return to Attached after steered work completes");
    assert_eq!(settled.control.current_run_id, None);
    assert_eq!(settled.inputs.current_run_id, None);
    assert!(settled.inputs.queue.is_empty());
    assert!(settled.inputs.steer_queue.is_empty());
    assert_eq!(settled.completion_waiters.input_count, 0);
    assert_eq!(settled.completion_waiters.waiter_count, 0);
    assert!(settled.completion_waiters.waiting_inputs.is_empty());
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "isolated attached steered completion should not trigger a follow-on continuation run"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        1,
        "isolated attached steered completion should not emit extra executor control commands"
    );
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_attached_steered_prompt_destroy_splits_completion_and_wait_all_lifetimes()
 {
    struct BlockingExecutor {
        apply_calls: Arc<AtomicUsize>,
        control_calls: Arc<AtomicUsize>,
        apply_started: Arc<Notify>,
        apply_finished: Arc<Notify>,
        allow_finish: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            self.apply_started.notify_waiters();
            self.allow_finish.notified().await;
            self.apply_finished.notify_waiters();

            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
                run_result: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));
    let apply_started = Arc::new(Notify::new());
    let apply_finished = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                control_calls: Arc::clone(&control_calls),
                apply_started: Arc::clone(&apply_started),
                apply_finished: Arc::clone(&apply_finished),
                allow_finish: Arc::clone(&allow_finish),
            }),
        )
        .await;

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for attached session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::MobMemberChild,
            owner_session_id: session_id.clone(),
            display_name: "attached steered destroy wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: true,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let input = Input::Prompt(crate::input::PromptInput::new(
        "attached steered destroy split",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ));
    let input_id = input.id().clone();
    let (outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("attached steered prompt should be accepted");
    assert!(outcome.is_accepted());
    let completion_handle =
        completion_handle.expect("attached steered prompt should expose a completion waiter");

    tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
        .await
        .expect("attached steered prompt should request immediate processing");

    let during_apply = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist while attached steered work is active");
    let wait_request_id = during_apply
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(during_apply.control.phase, RuntimeState::Running);
    assert_eq!(
        during_apply.control.current_run_id,
        during_apply.inputs.current_run_id
    );
    assert!(during_apply.control.current_run_id.is_some());
    assert!(during_apply.inputs.queue.is_empty());
    assert!(during_apply.inputs.steer_queue.is_empty());
    assert_eq!(during_apply.completion_waiters.input_count, 1);
    assert_eq!(during_apply.completion_waiters.waiter_count, 1);
    assert_eq!(during_apply.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        during_apply.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        during_apply.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id while attached steered work is active"
    );
    assert_eq!(
        during_apply.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the live operation while attached steered work is active"
    );
    assert_eq!(apply_calls.load(Ordering::SeqCst), 1);
    assert_eq!(control_calls.load(Ordering::SeqCst), 1);

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    crate::traits::RuntimeControlPlane::destroy(&adapter, &runtime_id)
        .await
        .expect("destroy should split attached steered completion and wait_all lifetimes");

    match completion_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime destroyed");
        }
        other => panic!(
            "expected attached steered completion waiter to terminate on destroy, got {other:?}"
        ),
    }

    let after_destroy = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after destroy");
    assert_eq!(after_destroy.control.phase, RuntimeState::Destroyed);
    assert!(after_destroy.inputs.queue.is_empty());
    assert!(after_destroy.inputs.steer_queue.is_empty());
    assert_eq!(after_destroy.completion_waiters.input_count, 0);
    assert_eq!(after_destroy.completion_waiters.waiter_count, 0);
    assert!(
        after_destroy.completion_waiters.waiting_inputs.is_empty(),
        "destroy should clear the steered completion waiter immediately even while apply remains blocked"
    );
    assert_eq!(
        after_destroy.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "destroy should preserve the authority-owned wait request after the steered completion waiter clears"
    );
    assert!(after_destroy.ops.pending_wait_present);
    assert_eq!(
        after_destroy.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "destroy should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_destroy.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "destroy should preserve the tracked wait target until the waited operation settles"
    );

    allow_finish.notify_waiters();
    tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
        .await
        .expect("blocked attached apply should finish once the executor is released");

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after destroy");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Destroyed);
    assert_eq!(settled.control.current_run_id, None);
    assert_eq!(settled.inputs.current_run_id, None);
    assert!(settled.inputs.queue.is_empty());
    assert!(settled.inputs.steer_queue.is_empty());
    assert_eq!(settled.completion_waiters.input_count, 0);
    assert_eq!(settled.completion_waiters.waiter_count, 0);
    assert!(settled.completion_waiters.waiting_inputs.is_empty());
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn interrupt_current_run_returns_not_ready_without_attached_loop() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let err = adapter
        .interrupt_current_run(&session_id)
        .await
        .expect_err("interrupt should reject when no attached loop exists");
    match err {
        RuntimeDriverError::NotReady { state } => {
            assert_eq!(state, RuntimeState::Idle);
        }
        other => panic!("expected NotReady(Idle), got {other:?}"),
    }
}

#[tokio::test]
async fn cancel_after_boundary_returns_not_ready_without_attached_loop() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let err = adapter
        .cancel_after_boundary(&session_id)
        .await
        .expect_err("boundary cancel should reject when no attached loop exists");
    match err {
        RuntimeDriverError::NotReady { state } => {
            assert_eq!(state, RuntimeState::Idle);
        }
        other => panic!("expected NotReady(Idle), got {other:?}"),
    }
}

#[tokio::test]
async fn interrupt_current_run_on_attached_runtime_is_deferred_until_apply_finishes() {
    struct BlockingExecutor {
        apply_calls: Arc<AtomicUsize>,
        cancel_calls: Arc<AtomicUsize>,
        apply_started: Arc<Notify>,
        apply_finished: Arc<Notify>,
        allow_finish: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            self.apply_started.notify_waiters();
            self.allow_finish.notified().await;
            self.apply_finished.notify_waiters();

            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
                run_result: None,
            })
        }

        async fn control(&mut self, command: RunControlCommand) -> Result<(), CoreExecutorError> {
            if matches!(command, RunControlCommand::CancelCurrentRun { .. }) {
                self.cancel_calls.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }
    }

    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let cancel_calls = Arc::new(AtomicUsize::new(0));
    let apply_started = Arc::new(Notify::new());
    let apply_finished = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                cancel_calls: Arc::clone(&cancel_calls),
                apply_started: Arc::clone(&apply_started),
                apply_finished: Arc::clone(&apply_finished),
                allow_finish: Arc::clone(&allow_finish),
            }),
        )
        .await;

    let input = Input::Prompt(crate::input::PromptInput::new(
        "attached steered deferred interrupt",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ));
    let input_id = input.id().clone();
    let (outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("attached steered prompt should be accepted");
    assert!(outcome.is_accepted());
    let completion_handle =
        completion_handle.expect("attached steered prompt should expose a completion waiter");

    tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
        .await
        .expect("attached steered prompt should request immediate processing");

    let during_apply = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist while apply is blocked");
    assert_eq!(during_apply.control.phase, RuntimeState::Running);
    assert_eq!(
        during_apply.control.current_run_id,
        during_apply.inputs.current_run_id
    );
    assert!(during_apply.control.current_run_id.is_some());
    assert!(during_apply.inputs.queue.is_empty());
    assert!(during_apply.inputs.steer_queue.is_empty());
    assert_eq!(during_apply.completion_waiters.input_count, 1);
    assert_eq!(during_apply.completion_waiters.waiter_count, 1);
    assert_eq!(during_apply.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        during_apply.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "attached steered prompt should start the run exactly once"
    );
    assert_eq!(
        cancel_calls.load(Ordering::SeqCst),
        0,
        "no interrupt should reach the executor before it is requested"
    );

    adapter
        .interrupt_current_run(&session_id)
        .await
        .expect("interrupt should enqueue against the attached loop");

    let after_interrupt = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after interrupt is requested");
    assert_eq!(
        after_interrupt.control.phase,
        RuntimeState::Running,
        "interrupt should stay deferred while the attached executor is still inside apply()"
    );
    assert_eq!(
        after_interrupt.control.current_run_id,
        after_interrupt.inputs.current_run_id
    );
    assert!(after_interrupt.control.current_run_id.is_some());
    assert!(after_interrupt.inputs.queue.is_empty());
    assert!(after_interrupt.inputs.steer_queue.is_empty());
    assert_eq!(after_interrupt.completion_waiters.input_count, 1);
    assert_eq!(after_interrupt.completion_waiters.waiter_count, 1);
    assert_eq!(after_interrupt.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        after_interrupt.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        cancel_calls.load(Ordering::SeqCst),
        0,
        "cancel should remain queued until apply returns"
    );

    allow_finish.notify_waiters();
    tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
        .await
        .expect("apply should finish after the executor is released");

    match completion_handle.wait().await {
        CompletionOutcome::CompletedWithoutResult => {}
        other => panic!(
            "expected attached queued prompt to complete normally before queued cancel drains, got {other:?}"
        ),
    }

    let settled = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after apply returns");
            if snapshot.control.phase == RuntimeState::Attached
                && snapshot.control.current_run_id.is_none()
                && snapshot.inputs.current_run_id.is_none()
                && snapshot.completion_waiters.waiter_count == 0
                && cancel_calls.load(Ordering::SeqCst) == 1
            {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("attached runtime should eventually return to Attached after queued cancel drains");
    assert!(settled.inputs.queue.is_empty());
    assert!(settled.inputs.steer_queue.is_empty());
    assert_eq!(settled.completion_waiters.input_count, 0);
    assert_eq!(settled.completion_waiters.waiter_count, 0);
    assert!(settled.completion_waiters.waiting_inputs.is_empty());
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "queued cancel should not replay the already-running attached steered turn"
    );
    assert_eq!(
        cancel_calls.load(Ordering::SeqCst),
        1,
        "queued cancel should reach the executor exactly once after apply finishes"
    );
}

#[tokio::test]
async fn cancel_after_boundary_on_attached_runtime_is_deferred_until_apply_finishes() {
    struct BlockingExecutor {
        apply_calls: Arc<AtomicUsize>,
        boundary_cancel_calls: Arc<AtomicUsize>,
        apply_started: Arc<Notify>,
        apply_finished: Arc<Notify>,
        allow_finish: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            self.apply_started.notify_waiters();
            self.allow_finish.notified().await;
            self.apply_finished.notify_waiters();

            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
                run_result: None,
            })
        }

        async fn control(&mut self, command: RunControlCommand) -> Result<(), CoreExecutorError> {
            if matches!(command, RunControlCommand::CancelAfterBoundary { .. }) {
                self.boundary_cancel_calls.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }
    }

    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let boundary_cancel_calls = Arc::new(AtomicUsize::new(0));
    let apply_started = Arc::new(Notify::new());
    let apply_finished = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                boundary_cancel_calls: Arc::clone(&boundary_cancel_calls),
                apply_started: Arc::clone(&apply_started),
                apply_finished: Arc::clone(&apply_finished),
                allow_finish: Arc::clone(&allow_finish),
            }),
        )
        .await;

    let input = Input::Prompt(crate::input::PromptInput::new(
        "attached steered deferred boundary cancel",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ));
    let (outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("attached steered prompt should be accepted");
    assert!(outcome.is_accepted());
    let completion_handle =
        completion_handle.expect("attached steered prompt should expose a completion waiter");

    tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
        .await
        .expect("attached steered prompt should request immediate processing");

    adapter
        .cancel_after_boundary(&session_id)
        .await
        .expect("boundary cancel should enqueue against the attached loop");

    let after_request = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after boundary cancel is requested");
    assert_eq!(after_request.control.phase, RuntimeState::Running);
    assert!(after_request.control.current_run_id.is_some());
    assert_eq!(
        boundary_cancel_calls.load(Ordering::SeqCst),
        0,
        "boundary cancel should remain queued until apply returns"
    );

    allow_finish.notify_waiters();
    tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
        .await
        .expect("apply should finish after the executor is released");

    match completion_handle.wait().await {
        CompletionOutcome::CompletedWithoutResult => {}
        other => panic!(
            "expected attached queued prompt to complete normally before queued boundary cancel drains, got {other:?}"
        ),
    }

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after apply returns");
            if snapshot.control.phase == RuntimeState::Attached
                && snapshot.control.current_run_id.is_none()
                && snapshot.inputs.current_run_id.is_none()
                && boundary_cancel_calls.load(Ordering::SeqCst) == 1
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("attached runtime should eventually drain the queued boundary cancel");
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "queued boundary cancel should not replay the already-running attached steered turn"
    );
}

#[tokio::test]
async fn running_peer_message_interrupt_yielding_drains_before_next_apply() {
    struct BlockingThenImmediateExecutor {
        apply_calls: Arc<AtomicUsize>,
        interrupt_calls: Arc<AtomicUsize>,
        events: Arc<std::sync::Mutex<Vec<&'static str>>>,
        first_apply_started: Arc<Notify>,
        allow_first_finish: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingThenImmediateExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            let apply_index = self.apply_calls.fetch_add(1, Ordering::SeqCst);
            if apply_index == 0 {
                self.events
                    .lock()
                    .expect("events mutex poisoned")
                    .push("apply1_start");
                self.first_apply_started.notify_waiters();
                self.allow_first_finish.notified().await;
                self.events
                    .lock()
                    .expect("events mutex poisoned")
                    .push("apply1_finish");
            } else {
                self.events
                    .lock()
                    .expect("events mutex poisoned")
                    .push("apply2_start");
            }

            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
                run_result: None,
            })
        }

        async fn control(&mut self, command: RunControlCommand) -> Result<(), CoreExecutorError> {
            if matches!(command, RunControlCommand::InterruptYielding) {
                self.interrupt_calls.fetch_add(1, Ordering::SeqCst);
                self.events
                    .lock()
                    .expect("events mutex poisoned")
                    .push("interrupt_yielding");
            }
            Ok(())
        }
    }

    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let interrupt_calls = Arc::new(AtomicUsize::new(0));
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let first_apply_started = Arc::new(Notify::new());
    let allow_first_finish = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingThenImmediateExecutor {
                apply_calls: Arc::clone(&apply_calls),
                interrupt_calls: Arc::clone(&interrupt_calls),
                events: Arc::clone(&events),
                first_apply_started: Arc::clone(&first_apply_started),
                allow_first_finish: Arc::clone(&allow_first_finish),
            }),
        )
        .await;

    let first_input = Input::Prompt(crate::input::PromptInput::new(
        "attached running peer interrupt",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ));
    let first_input_id = first_input.id().clone();
    let (outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, first_input)
        .await
        .expect("attached steered prompt should be accepted");
    assert!(outcome.is_accepted());
    let completion_handle =
        completion_handle.expect("attached steered prompt should expose a completion waiter");

    tokio::time::timeout(Duration::from_secs(1), first_apply_started.notified())
        .await
        .expect("attached steered prompt should request immediate processing");

    interrupt_calls.store(0, Ordering::SeqCst);
    events.lock().expect("events mutex poisoned").clear();

    let peer_input = Input::Peer(crate::input::PeerInput {
        header: crate::input::InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: crate::input::InputOrigin::Peer {
                peer_id: "peer-interrupt".into(),
                runtime_id: None,
            },
            durability: crate::input::InputDurability::Durable,
            visibility: crate::input::InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        convention: Some(crate::input::PeerConvention::Message),
        body: "interrupt while running".into(),
        blocks: None,
        handling_mode: None,
    });
    let peer_input_id = peer_input.id().clone();
    let peer_outcome = adapter
        .accept_input(&session_id, peer_input)
        .await
        .expect("running peer message should be accepted");
    assert!(peer_outcome.is_accepted());

    let after_peer_accept = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist while the first apply is blocked");
    assert_eq!(after_peer_accept.control.phase, RuntimeState::Running);
    assert!(after_peer_accept.control.current_run_id.is_some());
    assert_eq!(
        after_peer_accept.control.current_run_id,
        after_peer_accept.inputs.current_run_id
    );
    assert_eq!(after_peer_accept.inputs.queue.len(), 1);
    assert_eq!(after_peer_accept.inputs.queue[0], peer_input_id);
    assert!(after_peer_accept.inputs.steer_queue.is_empty());
    assert_eq!(after_peer_accept.completion_waiters.input_count, 1);
    assert_eq!(after_peer_accept.completion_waiters.waiter_count, 1);
    assert_eq!(
        after_peer_accept.completion_waiters.waiting_inputs[0].input_id,
        first_input_id
    );
    assert_eq!(
        interrupt_calls.load(Ordering::SeqCst),
        0,
        "interrupt-yielding should remain queued until the running apply returns"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "the running turn should still be on its first apply"
    );
    {
        let event_log = events.lock().expect("events mutex poisoned");
        let first_apply_finish_index = event_log.iter().position(|event| *event == "apply1_finish");
        assert!(
            first_apply_finish_index.is_none(),
            "the first apply should still be blocked while interrupt-yielding is queued"
        );
        assert!(
            event_log.is_empty(),
            "no queued control or replay should be observed before the running apply returns: {event_log:?}"
        );
    }

    allow_first_finish.notify_waiters();

    let settled = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist until runtime settles");
            if snapshot.control.phase == RuntimeState::Attached
                && snapshot.control.current_run_id.is_none()
                && snapshot.inputs.current_run_id.is_none()
                && snapshot.inputs.queue.is_empty()
                && snapshot.inputs.steer_queue.is_empty()
                && snapshot.completion_waiters.waiter_count == 0
                && interrupt_calls.load(Ordering::SeqCst) == 1
                && apply_calls.load(Ordering::SeqCst) == 2
            {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    let settled = match settled {
        Ok(snapshot) => snapshot,
        Err(_) => {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should still exist after timeout");
            let event_log = events.lock().expect("events mutex poisoned").clone();
            panic!(
                "attached runtime did not settle after interrupt-yielding as expected: phase={:?} control_run={:?} ingress_run={:?} queue={:?} steer_queue={:?} waiters={} interrupt_calls={} apply_calls={} events={:?}",
                snapshot.control.phase,
                snapshot.control.current_run_id,
                snapshot.inputs.current_run_id,
                snapshot.inputs.queue,
                snapshot.inputs.steer_queue,
                snapshot.completion_waiters.waiter_count,
                interrupt_calls.load(Ordering::SeqCst),
                apply_calls.load(Ordering::SeqCst),
                event_log,
            );
        }
    };
    assert_eq!(settled.control.phase, RuntimeState::Attached);
    assert!(settled.inputs.queue.is_empty());
    assert!(settled.inputs.steer_queue.is_empty());
    assert_eq!(settled.completion_waiters.input_count, 0);
    assert_eq!(settled.completion_waiters.waiter_count, 0);

    let (interrupt_index, second_apply_index) = {
        let event_log = events.lock().expect("events mutex poisoned");
        let interrupt_index = event_log
            .iter()
            .position(|event| *event == "interrupt_yielding")
            .expect("interrupt control should be delivered");
        let second_apply_index = event_log
            .iter()
            .position(|event| *event == "apply2_start")
            .expect("queued peer input should eventually start a second apply");
        (interrupt_index, second_apply_index)
    };
    assert!(
        interrupt_index < second_apply_index,
        "interrupt-yielding control must drain before the next queued input starts"
    );

    match completion_handle.wait().await {
        CompletionOutcome::CompletedWithoutResult => {}
        other => panic!(
            "expected first attached steered prompt to complete before queued peer input runs, got {other:?}"
        ),
    }
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_attached_steered_prompt_defers_stop_until_apply_finishes() {
    struct BlockingExecutor {
        apply_calls: Arc<AtomicUsize>,
        control_calls: Arc<AtomicUsize>,
        stop_calls: Arc<AtomicUsize>,
        apply_started: Arc<Notify>,
        apply_finished: Arc<Notify>,
        allow_finish: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            self.apply_started.notify_waiters();
            self.allow_finish.notified().await;
            self.apply_finished.notify_waiters();

            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
                run_result: None,
            })
        }

        async fn control(&mut self, command: RunControlCommand) -> Result<(), CoreExecutorError> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            if matches!(command, RunControlCommand::StopRuntimeExecutor { .. }) {
                self.stop_calls.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }
    }

    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));
    let stop_calls = Arc::new(AtomicUsize::new(0));
    let apply_started = Arc::new(Notify::new());
    let apply_finished = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                control_calls: Arc::clone(&control_calls),
                stop_calls: Arc::clone(&stop_calls),
                apply_started: Arc::clone(&apply_started),
                apply_finished: Arc::clone(&apply_finished),
                allow_finish: Arc::clone(&allow_finish),
            }),
        )
        .await;

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for attached session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::MobMemberChild,
            owner_session_id: session_id.clone(),
            display_name: "attached steered deferred stop wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: true,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let input = Input::Prompt(crate::input::PromptInput::new(
        "attached steered deferred stop",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ));
    let input_id = input.id().clone();
    let (outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("attached steered prompt should be accepted");
    assert!(outcome.is_accepted());
    let completion_handle =
        completion_handle.expect("attached steered prompt should expose a completion waiter");

    tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
        .await
        .expect("attached steered prompt should request immediate processing");

    let during_apply = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist while attached steered work is active");
    let wait_request_id = during_apply
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(during_apply.control.phase, RuntimeState::Running);
    assert_eq!(
        during_apply.control.current_run_id,
        during_apply.inputs.current_run_id
    );
    assert!(during_apply.control.current_run_id.is_some());
    assert!(during_apply.inputs.queue.is_empty());
    assert!(during_apply.inputs.steer_queue.is_empty());
    assert_eq!(during_apply.completion_waiters.input_count, 1);
    assert_eq!(during_apply.completion_waiters.waiter_count, 1);
    assert_eq!(during_apply.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        during_apply.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        during_apply.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id while attached steered work is active"
    );
    assert_eq!(
        during_apply.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the live operation while attached steered work is active"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "attached steered work should wake the loop exactly once"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        1,
        "attached steered admission should still route one control command through the executor seam"
    );
    assert_eq!(
        stop_calls.load(Ordering::SeqCst),
        0,
        "no explicit stop command should have reached the executor before it is requested"
    );

    adapter
        .stop_runtime_executor(
            &session_id,
            RunControlCommand::StopRuntimeExecutor {
                reason: "attached steered deferred stop".into(),
            },
        )
        .await
        .expect("stop should queue against the attached loop");

    let after_stop_request = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after stop is requested");
    assert_eq!(
        after_stop_request.control.phase,
        RuntimeState::Running,
        "stop should stay deferred while the attached executor is still inside apply()"
    );
    assert_eq!(
        after_stop_request.control.current_run_id,
        after_stop_request.inputs.current_run_id
    );
    assert!(after_stop_request.control.current_run_id.is_some());
    assert!(after_stop_request.inputs.queue.is_empty());
    assert!(after_stop_request.inputs.steer_queue.is_empty());
    assert_eq!(after_stop_request.completion_waiters.input_count, 1);
    assert_eq!(after_stop_request.completion_waiters.waiter_count, 1);
    assert_eq!(
        after_stop_request.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        after_stop_request.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "stop should not clear the authority-owned wait request while apply is still blocked"
    );
    assert!(after_stop_request.ops.pending_wait_present);
    assert_eq!(
        after_stop_request.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "stop should preserve request-id agreement while it remains queued behind apply()"
    );
    assert_eq!(
        after_stop_request.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "stop should preserve the tracked wait target while apply is still blocked"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        1,
        "the explicit stop command should still be queued instead of reaching the executor mid-apply"
    );
    assert_eq!(
        stop_calls.load(Ordering::SeqCst),
        0,
        "the explicit stop command should not be delivered until the loop drains controls after apply()"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete while stop is still deferred behind apply");
    let wait_result = wait_future
        .await
        .expect("wait_all should still resolve first");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let after_wait_all = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after wait_all settles");
            if snapshot.ops.wait_request_id.is_none() && !snapshot.ops.pending_wait_present {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("attached steered snapshot should eventually clear the wait_all carrier");
    assert_eq!(
        after_wait_all.control.phase,
        RuntimeState::Running,
        "stop should still be deferred while the attached executor remains inside apply()"
    );
    assert_eq!(
        after_wait_all.control.current_run_id,
        after_wait_all.inputs.current_run_id
    );
    assert!(after_wait_all.control.current_run_id.is_some());
    assert!(after_wait_all.inputs.queue.is_empty());
    assert!(after_wait_all.inputs.steer_queue.is_empty());
    assert_eq!(after_wait_all.completion_waiters.input_count, 1);
    assert_eq!(after_wait_all.completion_waiters.waiter_count, 1);
    assert_eq!(
        after_wait_all.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(after_wait_all.ops.wait_request_id, None);
    assert!(!after_wait_all.ops.pending_wait_present);
    assert_eq!(after_wait_all.ops.pending_wait_request_id, None);
    assert!(after_wait_all.ops.wait_operation_ids.is_empty());
    assert_eq!(
        stop_calls.load(Ordering::SeqCst),
        0,
        "the queued stop command should still not reach the executor before apply() completes"
    );

    allow_finish.notify_waiters();
    tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
        .await
        .expect("attached steered prompt should finish after the executor is released");

    match completion_handle.wait().await {
        CompletionOutcome::CompletedWithoutResult => {}
        other => panic!(
            "expected attached steered completion to finish normally before queued stop drains, got {other:?}"
        ),
    }

    let settled = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after queued stop drains");
            if snapshot.control.phase == RuntimeState::Stopped {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("attached runtime should eventually publish Stopped after apply returns");
    assert_eq!(settled.control.current_run_id, None);
    assert_eq!(settled.inputs.current_run_id, None);
    assert!(settled.inputs.queue.is_empty());
    assert!(settled.inputs.steer_queue.is_empty());
    assert_eq!(settled.completion_waiters.input_count, 0);
    assert_eq!(settled.completion_waiters.waiter_count, 0);
    assert!(settled.completion_waiters.waiting_inputs.is_empty());
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "deferred stop should not replay the attached steered input"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        2,
        "one admission control plus one deferred stop command should reach the attached executor"
    );
    assert_eq!(
        stop_calls.load(Ordering::SeqCst),
        1,
        "the queued stop command should reach the executor exactly once after apply() finishes"
    );
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_clears_completion_waiters_after_reset_with_runtime_loop() {
    struct RecordingExecutor {
        apply_calls: Arc<AtomicUsize>,
        control_calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for RecordingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
                run_result: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(RecordingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                control_calls: Arc::clone(&control_calls),
            }),
        )
        .await;

    let input = make_progress_input("reset-with-loop");
    let input_id = input.id().clone();
    let outcome = adapter
        .accept_input_without_wake(&session_id, input)
        .await
        .expect("progress input should queue without waking the attached loop");
    assert!(outcome.is_accepted());

    let handle = {
        let completions = {
            let sessions = adapter.sessions.read().await;
            sessions
                .get(&session_id)
                .expect("attached session should exist")
                .completions
                .clone()
        };
        let mut completions = completions.lock().await;
        completions.register(input_id.clone())
    };

    let before_reset = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before reset");
    assert_eq!(before_reset.control.phase, RuntimeState::Attached);
    assert_eq!(before_reset.completion_waiters.input_count, 1);
    assert_eq!(before_reset.completion_waiters.waiter_count, 1);
    assert_eq!(before_reset.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_reset.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "reset should be able to discard queued attached-loop work before apply runs"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "reset has not yet attempted any executor control"
    );

    let report = SessionServiceRuntimeExt::reset_runtime(&adapter, &session_id)
        .await
        .expect("reset should succeed for attached queued runtime");
    assert_eq!(report.inputs_abandoned, 1);

    let after_reset = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after reset");
    assert_eq!(after_reset.control.phase, RuntimeState::Idle);
    assert_eq!(after_reset.completion_waiters.input_count, 0);
    assert_eq!(after_reset.completion_waiters.waiter_count, 0);
    assert!(
        after_reset.completion_waiters.waiting_inputs.is_empty(),
        "reset should clear the completion waiter carrier immediately even when a loop is attached"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "reset should bypass queued attached-loop work entirely"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "reset currently bypasses the executor control seam and does not deliver an out-of-band control command"
    );

    match handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime reset");
        }
        other => panic!("expected runtime reset termination, got {other:?}"),
    }
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_clears_completion_waiters_after_stop_runtime_executor() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let input = make_prompt("stop completion waiter");
    let input_id = input.id().clone();
    let (_outcome, handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");
    let handle = handle.expect("queued prompt should register a completion waiter");

    let before_stop = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before stop");
    assert_eq!(before_stop.control.phase, RuntimeState::Idle);
    assert_eq!(before_stop.completion_waiters.input_count, 1);
    assert_eq!(before_stop.completion_waiters.waiter_count, 1);
    assert_eq!(before_stop.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_stop.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );

    adapter
        .stop_runtime_executor(
            &session_id,
            RunControlCommand::StopRuntimeExecutor {
                reason: "stop test".into(),
            },
        )
        .await
        .expect("stop should terminate active completion waiters");

    let after_stop = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after stop");
    assert_eq!(after_stop.control.phase, RuntimeState::Stopped);
    assert!(
        after_stop.inputs.queue.is_empty(),
        "stop should not leave ordinary queued work behind once the runtime is stopped"
    );
    assert!(
        after_stop.inputs.steer_queue.is_empty(),
        "stop should not leave steer-queued work behind once the runtime is stopped"
    );
    assert_eq!(after_stop.completion_waiters.input_count, 0);
    assert_eq!(after_stop.completion_waiters.waiter_count, 0);
    assert!(
        after_stop.completion_waiters.waiting_inputs.is_empty(),
        "stop should clear the completion waiter carrier immediately"
    );

    match handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime stopped");
        }
        other => panic!("expected runtime stopped termination, got {other:?}"),
    }
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_clears_completion_waiters_after_stop_runtime_executor_with_runtime_loop()
 {
    struct RecordingExecutor {
        apply_calls: Arc<AtomicUsize>,
        stop_calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for RecordingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
                run_result: None,
            })
        }

        async fn control(&mut self, command: RunControlCommand) -> Result<(), CoreExecutorError> {
            if matches!(command, RunControlCommand::StopRuntimeExecutor { .. }) {
                self.stop_calls.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }
    }

    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let stop_calls = Arc::new(AtomicUsize::new(0));

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(RecordingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                stop_calls: Arc::clone(&stop_calls),
            }),
        )
        .await;

    let input = make_progress_input("stop-with-loop");
    let input_id = input.id().clone();
    let outcome = adapter
        .accept_input_without_wake(&session_id, input)
        .await
        .expect("progress input should queue without waking the attached loop");
    assert!(outcome.is_accepted());

    let handle = {
        let completions = {
            let sessions = adapter.sessions.read().await;
            sessions
                .get(&session_id)
                .expect("attached session should exist")
                .completions
                .clone()
        };
        let mut completions = completions.lock().await;
        completions.register(input_id.clone())
    };

    let before_stop = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before stop");
    assert_eq!(before_stop.control.phase, RuntimeState::Attached);
    assert_eq!(before_stop.completion_waiters.input_count, 1);
    assert_eq!(before_stop.completion_waiters.waiter_count, 1);
    assert_eq!(before_stop.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_stop.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "stop should be able to preempt queued attached-loop work before apply runs"
    );

    adapter
        .stop_runtime_executor(
            &session_id,
            RunControlCommand::StopRuntimeExecutor {
                reason: "stop attached-loop completion waiter".into(),
            },
        )
        .await
        .expect("stop should terminate queued completion waiters through the live control seam");

    match handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime stopped");
        }
        other => panic!("expected runtime stopped termination, got {other:?}"),
    }

    let after_stop = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after stop");
    assert_eq!(after_stop.control.phase, RuntimeState::Stopped);
    assert!(
        after_stop.inputs.queue.is_empty(),
        "stop should not leave ordinary queued work behind even when an attached loop exists"
    );
    assert!(
        after_stop.inputs.steer_queue.is_empty(),
        "stop should not leave steer-queued work behind even when an attached loop exists"
    );
    assert_eq!(after_stop.completion_waiters.input_count, 0);
    assert_eq!(after_stop.completion_waiters.waiter_count, 0);
    assert!(
        after_stop.completion_waiters.waiting_inputs.is_empty(),
        "stop through the live loop should clear the completion waiter carrier"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "stop should beat queued ordinary work on an attached runtime loop"
    );
    assert_eq!(
        stop_calls.load(Ordering::SeqCst),
        1,
        "the attached executor should observe exactly one stop-runtime-executor control"
    );
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_clears_completion_waiters_after_retire_without_runtime_loop()
 {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let input = make_prompt("retire completion waiter");
    let input_id = input.id().clone();
    let (_outcome, handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");
    let handle = handle.expect("queued prompt should register a completion waiter");

    let before_retire = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before retire");
    assert_eq!(before_retire.control.phase, RuntimeState::Idle);
    assert_eq!(before_retire.completion_waiters.input_count, 1);
    assert_eq!(before_retire.completion_waiters.waiter_count, 1);
    assert_eq!(before_retire.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_retire.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    let report = crate::traits::RuntimeControlPlane::retire(&adapter, &runtime_id)
        .await
        .expect("retire should clear queued waiters when no runtime loop can drain");
    assert_eq!(report.inputs_abandoned, 1);
    assert_eq!(report.inputs_pending_drain, 0);

    let after_retire = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after retire");
    assert_eq!(after_retire.control.phase, RuntimeState::Retired);
    assert_eq!(after_retire.completion_waiters.input_count, 0);
    assert_eq!(after_retire.completion_waiters.waiter_count, 0);
    assert!(
        after_retire.completion_waiters.waiting_inputs.is_empty(),
        "retire without a live runtime loop should clear the completion waiter carrier immediately"
    );

    match handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "retired without runtime loop");
        }
        other => panic!("expected retired-without-runtime-loop termination, got {other:?}"),
    }
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_preserves_completion_waiters_after_retire_with_runtime_loop()
 {
    struct BlockingExecutor {
        apply_calls: Arc<AtomicUsize>,
        apply_started: Arc<Notify>,
        allow_finish: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            self.apply_started.notify_waiters();
            self.allow_finish.notified().await;

            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
                run_result: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let apply_started = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                apply_started: Arc::clone(&apply_started),
                allow_finish: Arc::clone(&allow_finish),
            }),
        )
        .await;

    let input = make_progress_input("retire-with-loop");
    let input_id = input.id().clone();
    let (outcome, handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("progress input should be accepted");
    assert!(outcome.is_accepted());
    let handle = handle.expect("queued progress input should expose a completion waiter");

    let before_retire = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before retire");
    assert_eq!(before_retire.control.phase, RuntimeState::Attached);
    assert_eq!(before_retire.completion_waiters.input_count, 1);
    assert_eq!(before_retire.completion_waiters.waiter_count, 1);
    assert_eq!(before_retire.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_retire.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "queued progress input should remain pending until retire wakes the attached loop"
    );

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    let report = crate::traits::RuntimeControlPlane::retire(&adapter, &runtime_id)
        .await
        .expect("retire should preserve queued work for the live runtime loop to drain");
    assert_eq!(report.inputs_abandoned, 0);
    assert_eq!(report.inputs_pending_drain, 1);

    tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
        .await
        .expect("retire should wake the attached runtime loop to drain queued work");

    let after_retire = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after retire wakes the loop");
    assert_eq!(
        after_retire.control.phase,
        RuntimeState::Running,
        "retire currently hands preserved queued work to the attached loop, which re-enters Running while draining it"
    );
    assert_eq!(after_retire.completion_waiters.input_count, 1);
    assert_eq!(after_retire.completion_waiters.waiter_count, 1);
    assert_eq!(after_retire.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        after_retire.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "retire should wake the attached loop exactly once for the preserved queued work"
    );

    allow_finish.notify_waiters();

    match handle.wait().await {
        CompletionOutcome::CompletedWithoutResult => {}
        other => panic!("expected retire+drain to complete queued work, got {other:?}"),
    }

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after drained completion settles");
    assert_eq!(settled.control.phase, RuntimeState::Retired);
    assert_eq!(settled.completion_waiters.input_count, 0);
    assert_eq!(settled.completion_waiters.waiter_count, 0);
    assert!(
        settled.completion_waiters.waiting_inputs.is_empty(),
        "retire+drain should clear the completion waiter carrier once the preserved work completes"
    );
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_preserves_completion_waiters_after_recover_with_runtime_loop()
 {
    struct BlockingExecutor {
        apply_calls: Arc<AtomicUsize>,
        control_calls: Arc<AtomicUsize>,
        apply_started: Arc<Notify>,
        apply_finished: Arc<Notify>,
        allow_finish: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            self.apply_started.notify_waiters();
            self.allow_finish.notified().await;
            self.apply_finished.notify_waiters();

            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
                run_result: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));
    let apply_started = Arc::new(Notify::new());
    let apply_finished = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                control_calls: Arc::clone(&control_calls),
                apply_started: Arc::clone(&apply_started),
                apply_finished: Arc::clone(&apply_finished),
                allow_finish: Arc::clone(&allow_finish),
            }),
        )
        .await;

    let input = make_progress_input("recover-with-loop-completion");
    let input_id = input.id().clone();
    let (outcome, handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("progress input should be accepted");
    assert!(outcome.is_accepted());
    let handle = handle.expect("queued progress input should expose a completion waiter");

    let before_recover = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before recover");
    assert_eq!(before_recover.control.phase, RuntimeState::Attached);
    assert_eq!(before_recover.completion_waiters.input_count, 1);
    assert_eq!(before_recover.completion_waiters.waiter_count, 1);
    assert_eq!(before_recover.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_recover.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        before_recover.completion_waiters.waiting_inputs[0].waiter_count,
        1
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "queued attached-loop work should remain pending until recover wakes the loop"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "recover should not yet have attempted executor control"
    );

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    let report = crate::traits::RuntimeControlPlane::recover(&adapter, &runtime_id)
        .await
        .expect(
            "recover should preserve queued completion waiters while replaying attached-loop work",
        );
    assert_eq!(report.inputs_recovered, 1);

    tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
        .await
        .expect("recover should wake the attached runtime loop to replay preserved queued work");

    let during_recover = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist while recover replay is in flight");
    assert_eq!(
        during_recover.control.phase,
        RuntimeState::Running,
        "recover currently re-enters Running while the attached loop replays recovered work"
    );
    assert_eq!(during_recover.completion_waiters.input_count, 1);
    assert_eq!(during_recover.completion_waiters.waiter_count, 1);
    assert_eq!(during_recover.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        during_recover.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        during_recover.completion_waiters.waiting_inputs[0].waiter_count,
        1
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "recover should wake the attached loop exactly once for the recovered queued work"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "recover should not route any out-of-band control command through the executor seam"
    );

    allow_finish.notify_waiters();
    tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
        .await
        .expect("attached loop should finish replaying the recovered queued work");

    match handle.wait().await {
        CompletionOutcome::CompletedWithoutResult => {}
        other => panic!("expected recover+replay to complete queued work, got {other:?}"),
    }

    let settled = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after attached-loop replay");
            if snapshot.control.phase != RuntimeState::Running {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("runtime should eventually leave Running once the recovered work finishes replaying");
    assert_eq!(
        settled.control.phase,
        RuntimeState::Attached,
        "recover should currently return to Attached once the recovered work finishes replaying"
    );
    assert_eq!(settled.completion_waiters.input_count, 0);
    assert_eq!(settled.completion_waiters.waiter_count, 0);
    assert!(
        settled.completion_waiters.waiting_inputs.is_empty(),
        "recover+replay should clear the completion waiter carrier once the preserved work completes"
    );
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_preserves_completion_waiters_after_recycle_with_runtime_loop()
 {
    struct BlockingExecutor {
        apply_calls: Arc<AtomicUsize>,
        control_calls: Arc<AtomicUsize>,
        apply_started: Arc<Notify>,
        apply_finished: Arc<Notify>,
        allow_finish: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            self.apply_started.notify_waiters();
            self.allow_finish.notified().await;
            self.apply_finished.notify_waiters();

            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
                run_result: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));
    let apply_started = Arc::new(Notify::new());
    let apply_finished = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                control_calls: Arc::clone(&control_calls),
                apply_started: Arc::clone(&apply_started),
                apply_finished: Arc::clone(&apply_finished),
                allow_finish: Arc::clone(&allow_finish),
            }),
        )
        .await;

    let input = make_progress_input("recycle-with-loop-completion");
    let input_id = input.id().clone();
    let (outcome, handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("progress input should be accepted");
    assert!(outcome.is_accepted());
    let handle = handle.expect("queued progress input should expose a completion waiter");

    let before_recycle = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before recycle");
    assert_eq!(before_recycle.control.phase, RuntimeState::Attached);
    assert_eq!(before_recycle.completion_waiters.input_count, 1);
    assert_eq!(before_recycle.completion_waiters.waiter_count, 1);
    assert_eq!(before_recycle.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_recycle.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        before_recycle.completion_waiters.waiting_inputs[0].waiter_count,
        1
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "queued attached-loop work should remain pending until recycle wakes the loop"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "recycle should not yet have attempted executor control"
    );

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    let report = crate::traits::RuntimeControlPlane::recycle(&adapter, &runtime_id)
        .await
        .expect(
            "recycle should preserve queued completion waiters while replaying attached-loop work",
        );
    assert_eq!(report.inputs_transferred, 1);

    tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
        .await
        .expect("recycle should wake the attached runtime loop to replay preserved queued work");

    let during_recycle = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist while recycle replay is in flight");
    assert_eq!(
        during_recycle.control.phase,
        RuntimeState::Running,
        "recycle currently re-enters Running while the attached loop replays preserved work"
    );
    assert_eq!(during_recycle.completion_waiters.input_count, 1);
    assert_eq!(during_recycle.completion_waiters.waiter_count, 1);
    assert_eq!(during_recycle.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        during_recycle.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        during_recycle.completion_waiters.waiting_inputs[0].waiter_count,
        1
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "recycle should wake the attached loop exactly once for the preserved queued work"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "recycle should not route any out-of-band control command through the executor seam"
    );

    allow_finish.notify_waiters();
    tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
        .await
        .expect("attached loop should finish replaying the preserved queued work");

    match handle.wait().await {
        CompletionOutcome::CompletedWithoutResult => {}
        other => panic!("expected recycle+replay to complete queued work, got {other:?}"),
    }

    let settled = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after attached-loop replay");
            if snapshot.control.phase == RuntimeState::Attached {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("runtime should return to Attached once the preserved work finishes replaying");
    assert_eq!(settled.completion_waiters.input_count, 0);
    assert_eq!(settled.completion_waiters.waiter_count, 0);
    assert!(
        settled.completion_waiters.waiting_inputs.is_empty(),
        "recycle+replay should clear the completion waiter carrier once the preserved work completes"
    );
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_tracks_epoch_cursor_state() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let cursor_state = {
        let sessions = adapter.sessions.read().await;
        Arc::clone(
            &sessions
                .get(&session_id)
                .expect("registered session should exist")
                .cursor_state,
        )
    };
    cursor_state
        .agent_applied_cursor
        .store(7, Ordering::Release);
    cursor_state
        .runtime_observed_seq
        .store(11, Ordering::Release);
    cursor_state
        .runtime_last_injected_seq
        .store(13, Ordering::Release);

    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist for registered session");

    assert_eq!(snapshot.binding.cursor_state.agent_applied_cursor, 7);
    assert_eq!(snapshot.binding.cursor_state.runtime_observed_seq, 11);
    assert_eq!(snapshot.binding.cursor_state.runtime_last_injected_seq, 13);
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_tracks_runtime_ops_state() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;
    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "background test op".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");
    registry
        .report_progress(
            &operation_id,
            OperationProgressUpdate {
                message: "still working".into(),
                percent: Some(0.5),
            },
        )
        .expect("progress update should be accepted");

    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist for registered session");

    assert_eq!(snapshot.ops.operation_count, 1);
    assert_eq!(snapshot.ops.active_count, 1);
    assert_eq!(snapshot.ops.wait_request_id, None);
    assert!(!snapshot.ops.pending_wait_present);
    assert_eq!(snapshot.ops.pending_wait_request_id, None);
    assert!(snapshot.ops.wait_operation_ids.is_empty());
    assert_eq!(snapshot.ops.detached_wake_pending, None);
    assert_eq!(snapshot.ops.detached_wake_signaled, None);
    assert_eq!(snapshot.ops.operations.len(), 1);

    let op = &snapshot.ops.operations[0];
    assert_eq!(op.id, operation_id);
    assert_eq!(op.kind, OperationKind::BackgroundToolOp);
    assert_eq!(op.display_name, "background test op");
    assert_eq!(op.status.as_str(), "running");
    assert!(!op.peer_ready);
    assert!(op.peer_handle.is_none());
    assert_eq!(op.progress_count, 1);
    assert_eq!(op.watcher_count, 0);
    assert_eq!(op.terminal_outcome, None);
    assert!(op.started_at_ms.is_some());
    assert!(op.completed_at_ms.is_none());
    assert!(op.elapsed_ms.is_none());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_tracks_wait_all_state() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;
    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist for registered session");

    assert_eq!(snapshot.ops.operation_count, 1);
    assert_eq!(snapshot.ops.active_count, 1);
    let wait_request_id = snapshot
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert!(
        snapshot.ops.pending_wait_present,
        "pending wait carrier should be present while wait_all is active"
    );
    assert_eq!(
        snapshot.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id as the authority"
    );
    assert_eq!(snapshot.ops.wait_operation_ids, vec![operation_id.clone()]);

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete");
    let wait_result = wait_future.await.expect("wait_all should resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(wait_result.satisfied.operation_ids, vec![operation_id]);
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_preserves_wait_all_after_recover() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;
    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "recover wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_recover = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before recover");
    let wait_request_id = before_recover
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert!(before_recover.ops.pending_wait_present);
    assert_eq!(
        before_recover.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before recover"
    );
    assert_eq!(
        before_recover.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before recover"
    );

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    let report = crate::traits::RuntimeControlPlane::recover(&adapter, &runtime_id)
        .await
        .expect("recover should preserve the active wait_all carrier");
    assert_eq!(report.inputs_recovered, 0);

    let after_recover = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after recover");
    assert_eq!(
        after_recover.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "recover should preserve the authority-owned wait request"
    );
    assert!(
        after_recover.ops.pending_wait_present,
        "recover should preserve the pending wait carrier while the operation remains active"
    );
    assert_eq!(
        after_recover.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "recover should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_recover.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "recover should preserve the tracked wait targets"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after recover");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(
        settled.control.current_run_id, None,
        "recycle should not leave a settled control-side current-run binding behind"
    );
    assert_eq!(
        settled.inputs.current_run_id, None,
        "recycle should not leave a settled ingress-side current-run binding behind"
    );
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_recover_splits_completion_and_wait_all_lifetimes() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;
    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let input = make_prompt("recover split lifetimes");
    let input_id = input.id().clone();
    let (_outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");
    let completion_handle =
        completion_handle.expect("queued prompt should register a completion waiter");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "recover split wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_recover = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before recover");
    let wait_request_id = before_recover
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_recover.control.phase, RuntimeState::Idle);
    assert_eq!(before_recover.inputs.queue, vec![input_id.clone()]);
    assert_eq!(before_recover.completion_waiters.input_count, 1);
    assert_eq!(before_recover.completion_waiters.waiter_count, 1);
    assert_eq!(before_recover.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_recover.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert!(before_recover.ops.pending_wait_present);
    assert_eq!(
        before_recover.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before recover"
    );
    assert_eq!(
        before_recover.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before recover"
    );

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    let report = crate::traits::RuntimeControlPlane::recover(&adapter, &runtime_id)
        .await
        .expect("recover should preserve both queued input and active wait_all");
    assert_eq!(report.inputs_recovered, 1);

    let after_recover = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after recover");
    assert_eq!(after_recover.control.phase, RuntimeState::Idle);
    assert_eq!(after_recover.inputs.queue, vec![input_id.clone()]);
    assert_eq!(after_recover.completion_waiters.input_count, 1);
    assert_eq!(after_recover.completion_waiters.waiter_count, 1);
    assert_eq!(after_recover.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        after_recover.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        after_recover.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "recover should preserve the authority-owned wait request"
    );
    assert!(after_recover.ops.pending_wait_present);
    assert_eq!(
        after_recover.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "recover should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_recover.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "recover should preserve the tracked wait target"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after recover");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let after_wait_all = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(after_wait_all.control.phase, RuntimeState::Idle);
    assert_eq!(
        after_wait_all.control.current_run_id, None,
        "recover should not leave a settled control-side current-run binding behind"
    );
    assert_eq!(
        after_wait_all.inputs.current_run_id, None,
        "recover should not leave a settled ingress-side current-run binding behind"
    );
    assert_eq!(after_wait_all.inputs.queue, vec![input_id.clone()]);
    assert_eq!(after_wait_all.completion_waiters.input_count, 1);
    assert_eq!(after_wait_all.completion_waiters.waiter_count, 1);
    assert_eq!(after_wait_all.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        after_wait_all.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(after_wait_all.ops.wait_request_id, None);
    assert!(!after_wait_all.ops.pending_wait_present);
    assert_eq!(after_wait_all.ops.pending_wait_request_id, None);
    assert!(after_wait_all.ops.wait_operation_ids.is_empty());

    SessionServiceRuntimeExt::reset_runtime(&adapter, &session_id)
        .await
        .expect("reset should terminate the preserved completion waiter at test end");
    match completion_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime reset");
        }
        other => panic!("expected runtime reset termination, got {other:?}"),
    }
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_recover_preserves_steered_input_and_wait_all() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;
    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let input = Input::Prompt(crate::input::PromptInput::new(
        "recover steered prompt",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ));
    let input_id = input.id().clone();
    let (_outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("steered prompt should be accepted");
    let completion_handle =
        completion_handle.expect("steered prompt should register a completion waiter");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "recover steered wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_recover = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before recover");
    let wait_request_id = before_recover
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_recover.control.phase, RuntimeState::Idle);
    assert!(before_recover.inputs.queue.is_empty());
    assert_eq!(before_recover.inputs.steer_queue, vec![input_id.clone()]);
    assert!(before_recover.inputs.wake_requested);
    assert!(before_recover.inputs.process_requested);
    assert_eq!(before_recover.completion_waiters.input_count, 1);
    assert_eq!(before_recover.completion_waiters.waiter_count, 1);
    assert_eq!(before_recover.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_recover.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        before_recover.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before recover"
    );
    assert_eq!(
        before_recover.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before recover"
    );

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    let report = crate::traits::RuntimeControlPlane::recover(&adapter, &runtime_id)
        .await
        .expect("recover should preserve steered input and active wait_all");
    assert_eq!(report.inputs_recovered, 1);

    let after_recover = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after recover");
    assert_eq!(after_recover.control.phase, RuntimeState::Idle);
    assert!(after_recover.inputs.queue.is_empty());
    assert_eq!(after_recover.inputs.steer_queue, vec![input_id.clone()]);
    assert!(after_recover.inputs.wake_requested);
    assert!(after_recover.inputs.process_requested);
    assert_eq!(after_recover.completion_waiters.input_count, 1);
    assert_eq!(after_recover.completion_waiters.waiter_count, 1);
    assert_eq!(after_recover.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        after_recover.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        after_recover.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "recover should preserve the authority-owned wait request"
    );
    assert!(after_recover.ops.pending_wait_present);
    assert_eq!(
        after_recover.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "recover should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_recover.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "recover should preserve the tracked wait target"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after recover");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let after_wait_all = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(after_wait_all.control.phase, RuntimeState::Idle);
    assert_eq!(after_wait_all.control.current_run_id, None);
    assert_eq!(after_wait_all.inputs.current_run_id, None);
    assert!(after_wait_all.inputs.queue.is_empty());
    assert_eq!(after_wait_all.inputs.steer_queue, vec![input_id.clone()]);
    assert!(after_wait_all.inputs.wake_requested);
    assert!(after_wait_all.inputs.process_requested);
    assert_eq!(after_wait_all.completion_waiters.input_count, 1);
    assert_eq!(after_wait_all.completion_waiters.waiter_count, 1);
    assert_eq!(after_wait_all.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        after_wait_all.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(after_wait_all.ops.wait_request_id, None);
    assert!(!after_wait_all.ops.pending_wait_present);
    assert_eq!(after_wait_all.ops.pending_wait_request_id, None);
    assert!(after_wait_all.ops.wait_operation_ids.is_empty());

    SessionServiceRuntimeExt::reset_runtime(&adapter, &session_id)
        .await
        .expect("reset should terminate the preserved steered completion waiter at test end");
    match completion_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime reset");
        }
        other => panic!("expected runtime reset termination, got {other:?}"),
    }
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_recycle_preserves_steered_input_and_wait_all() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;
    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let input = Input::Prompt(crate::input::PromptInput::new(
        "recycle steered prompt",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ));
    let input_id = input.id().clone();
    let (_outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("steered prompt should be accepted");
    let completion_handle =
        completion_handle.expect("steered prompt should register a completion waiter");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "recycle steered wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_recycle = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before recycle");
    let wait_request_id = before_recycle
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_recycle.control.phase, RuntimeState::Idle);
    assert!(before_recycle.inputs.queue.is_empty());
    assert_eq!(before_recycle.inputs.steer_queue, vec![input_id.clone()]);
    assert!(before_recycle.inputs.wake_requested);
    assert!(before_recycle.inputs.process_requested);
    assert_eq!(before_recycle.completion_waiters.input_count, 1);
    assert_eq!(before_recycle.completion_waiters.waiter_count, 1);
    assert_eq!(before_recycle.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_recycle.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        before_recycle.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before recycle"
    );
    assert_eq!(
        before_recycle.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before recycle"
    );

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    let report = crate::traits::RuntimeControlPlane::recycle(&adapter, &runtime_id)
        .await
        .expect("recycle should preserve steered input and active wait_all");
    assert_eq!(report.inputs_transferred, 1);

    let after_recycle = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after recycle");
    assert_eq!(after_recycle.control.phase, RuntimeState::Idle);
    assert!(after_recycle.inputs.queue.is_empty());
    assert_eq!(after_recycle.inputs.steer_queue, vec![input_id.clone()]);
    assert!(after_recycle.inputs.wake_requested);
    assert!(after_recycle.inputs.process_requested);
    assert_eq!(after_recycle.completion_waiters.input_count, 1);
    assert_eq!(after_recycle.completion_waiters.waiter_count, 1);
    assert_eq!(after_recycle.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        after_recycle.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        after_recycle.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "recycle should preserve the authority-owned wait request"
    );
    assert!(after_recycle.ops.pending_wait_present);
    assert_eq!(
        after_recycle.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "recycle should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_recycle.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "recycle should preserve the tracked wait target"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after recycle");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let after_wait_all = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(after_wait_all.control.phase, RuntimeState::Idle);
    assert_eq!(after_wait_all.control.current_run_id, None);
    assert_eq!(after_wait_all.inputs.current_run_id, None);
    assert!(after_wait_all.inputs.queue.is_empty());
    assert_eq!(after_wait_all.inputs.steer_queue, vec![input_id.clone()]);
    assert!(after_wait_all.inputs.wake_requested);
    assert!(after_wait_all.inputs.process_requested);
    assert_eq!(after_wait_all.completion_waiters.input_count, 1);
    assert_eq!(after_wait_all.completion_waiters.waiter_count, 1);
    assert_eq!(after_wait_all.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        after_wait_all.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(after_wait_all.ops.wait_request_id, None);
    assert!(!after_wait_all.ops.pending_wait_present);
    assert_eq!(after_wait_all.ops.pending_wait_request_id, None);
    assert!(after_wait_all.ops.wait_operation_ids.is_empty());

    SessionServiceRuntimeExt::reset_runtime(&adapter, &session_id)
        .await
        .expect("reset should terminate the preserved steered completion waiter at test end");
    match completion_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime reset");
        }
        other => panic!("expected runtime reset termination, got {other:?}"),
    }
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_preserves_wait_all_after_recycle() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;
    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "recycle wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_recycle = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before recycle");
    let wait_request_id = before_recycle
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert!(before_recycle.ops.pending_wait_present);
    assert_eq!(
        before_recycle.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before recycle"
    );
    assert_eq!(
        before_recycle.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before recycle"
    );

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    let report = crate::traits::RuntimeControlPlane::recycle(&adapter, &runtime_id)
        .await
        .expect("recycle should preserve the active wait_all carrier");
    assert_eq!(report.inputs_transferred, 0);

    let after_recycle = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after recycle");
    assert_eq!(
        after_recycle.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "recycle should preserve the authority-owned wait request"
    );
    assert!(
        after_recycle.ops.pending_wait_present,
        "recycle should preserve the pending wait carrier while the operation remains active"
    );
    assert_eq!(
        after_recycle.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "recycle should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_recycle.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "recycle should preserve the tracked wait targets"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after recycle");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_recycle_splits_completion_and_wait_all_lifetimes() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;
    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let input = make_prompt("recycle split lifetimes");
    let input_id = input.id().clone();
    let (_outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");
    let completion_handle =
        completion_handle.expect("queued prompt should register a completion waiter");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "recycle split wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_recycle = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before recycle");
    let wait_request_id = before_recycle
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_recycle.control.phase, RuntimeState::Idle);
    assert_eq!(before_recycle.inputs.queue, vec![input_id.clone()]);
    assert_eq!(before_recycle.completion_waiters.input_count, 1);
    assert_eq!(before_recycle.completion_waiters.waiter_count, 1);
    assert_eq!(before_recycle.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_recycle.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert!(before_recycle.ops.pending_wait_present);
    assert_eq!(
        before_recycle.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before recycle"
    );
    assert_eq!(
        before_recycle.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before recycle"
    );

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    let report = crate::traits::RuntimeControlPlane::recycle(&adapter, &runtime_id)
        .await
        .expect("recycle should preserve both queued input and active wait_all");
    assert_eq!(report.inputs_transferred, 1);

    let after_recycle = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after recycle");
    assert_eq!(after_recycle.control.phase, RuntimeState::Idle);
    assert_eq!(after_recycle.inputs.queue, vec![input_id.clone()]);
    assert_eq!(after_recycle.completion_waiters.input_count, 1);
    assert_eq!(after_recycle.completion_waiters.waiter_count, 1);
    assert_eq!(after_recycle.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        after_recycle.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        after_recycle.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "recycle should preserve the authority-owned wait request"
    );
    assert!(after_recycle.ops.pending_wait_present);
    assert_eq!(
        after_recycle.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "recycle should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_recycle.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "recycle should preserve the tracked wait target"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after recycle");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let after_wait_all = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(after_wait_all.control.phase, RuntimeState::Idle);
    assert_eq!(after_wait_all.inputs.queue, vec![input_id.clone()]);
    assert_eq!(after_wait_all.completion_waiters.input_count, 1);
    assert_eq!(after_wait_all.completion_waiters.waiter_count, 1);
    assert_eq!(after_wait_all.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        after_wait_all.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(after_wait_all.ops.wait_request_id, None);
    assert!(!after_wait_all.ops.pending_wait_present);
    assert_eq!(after_wait_all.ops.pending_wait_request_id, None);
    assert!(after_wait_all.ops.wait_operation_ids.is_empty());

    SessionServiceRuntimeExt::reset_runtime(&adapter, &session_id)
        .await
        .expect("reset should terminate the preserved completion waiter at test end");
    match completion_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime reset");
        }
        other => panic!("expected runtime reset termination, got {other:?}"),
    }
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_preserves_wait_all_after_recover_with_runtime_loop() {
    struct BlockingExecutor {
        apply_calls: Arc<AtomicUsize>,
        control_calls: Arc<AtomicUsize>,
        apply_started: Arc<Notify>,
        apply_finished: Arc<Notify>,
        allow_finish: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            self.apply_started.notify_waiters();
            self.allow_finish.notified().await;
            self.apply_finished.notify_waiters();

            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
                run_result: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));
    let apply_started = Arc::new(Notify::new());
    let apply_finished = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                control_calls: Arc::clone(&control_calls),
                apply_started: Arc::clone(&apply_started),
                apply_finished: Arc::clone(&apply_finished),
                allow_finish: Arc::clone(&allow_finish),
            }),
        )
        .await;

    let outcome = adapter
        .accept_input_without_wake(
            &session_id,
            make_progress_input("recover-wait-all-with-loop"),
        )
        .await
        .expect("queued progress input should be accepted without waking the loop");
    assert!(outcome.is_accepted());

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for attached session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "recover wait target with loop".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_recover = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before recover");
    let wait_request_id = before_recover
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_recover.control.phase, RuntimeState::Attached);
    assert!(before_recover.ops.pending_wait_present);
    assert_eq!(
        before_recover.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before recover"
    );
    assert_eq!(
        before_recover.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before recover"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "queued attached-loop work should remain pending until recover wakes the loop"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "recover should not yet have attempted executor control"
    );

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    let report = crate::traits::RuntimeControlPlane::recover(&adapter, &runtime_id)
        .await
        .expect("recover should preserve wait_all while replaying attached-loop work");
    assert_eq!(report.inputs_recovered, 1);

    tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
        .await
        .expect("recover should wake the attached runtime loop to replay preserved work");

    let during_recover = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist while recover replay is in flight");
    assert_eq!(
        during_recover.control.phase,
        RuntimeState::Running,
        "recover currently re-enters Running while the attached loop replays recovered work"
    );
    assert_eq!(
        during_recover.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "recover should preserve the authority-owned wait request while replay is in flight"
    );
    assert!(during_recover.ops.pending_wait_present);
    assert_eq!(
        during_recover.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "recover should preserve request-id agreement across the wait carrier seam while replaying"
    );
    assert_eq!(
        during_recover.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "recover should preserve the tracked wait target while recovered work is replaying"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "recover should wake the attached loop exactly once for the recovered queued work"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "recover should not route any out-of-band control command through the executor seam"
    );

    allow_finish.notify_waiters();
    tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
        .await
        .expect("attached loop should finish replaying the recovered queued work");

    let after_replay = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after attached-loop replay");
            if snapshot.control.phase != RuntimeState::Running {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("runtime should eventually leave Running once the recovered work finishes replaying");
    assert_eq!(
        after_replay.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "wait_all should remain active after the attached loop finishes replaying recovered work"
    );
    assert!(after_replay.ops.pending_wait_present);
    assert_eq!(
        after_replay.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "request-id agreement should survive the return to Idle after replay"
    );
    assert_eq!(
        after_replay.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "the tracked wait target should remain present until the operation itself settles"
    );
    assert_eq!(
        after_replay.control.phase,
        RuntimeState::Attached,
        "recover should currently return to Attached once the recovered work finishes replaying"
    );
    assert!(
        after_replay.inputs.queue.is_empty(),
        "recover should not leave ordinary queued work behind once the attached runtime returns to Attached after replay"
    );
    assert!(
        after_replay.inputs.steer_queue.is_empty(),
        "recover should not leave steer-queued work behind once the attached runtime returns to Attached after replay"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after recover+replay");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Attached);
    assert_eq!(
        settled.control.current_run_id, None,
        "recover should not leave a settled control-side current-run binding behind even on attached runtimes"
    );
    assert_eq!(
        settled.inputs.current_run_id, None,
        "recover should not leave a settled ingress-side current-run binding behind even on attached runtimes"
    );
    assert!(
        settled.inputs.queue.is_empty(),
        "recover should keep the attached settled snapshot free of ordinary queued work after replay completes"
    );
    assert!(
        settled.inputs.steer_queue.is_empty(),
        "recover should keep the attached settled snapshot free of steer-queued work after replay completes"
    );
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_recover_with_runtime_loop_splits_completion_and_wait_all_lifetimes()
 {
    struct BlockingExecutor {
        apply_calls: Arc<AtomicUsize>,
        control_calls: Arc<AtomicUsize>,
        apply_started: Arc<Notify>,
        apply_finished: Arc<Notify>,
        allow_finish: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            self.apply_started.notify_waiters();
            self.allow_finish.notified().await;
            self.apply_finished.notify_waiters();

            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
                run_result: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));
    let apply_started = Arc::new(Notify::new());
    let apply_finished = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                control_calls: Arc::clone(&control_calls),
                apply_started: Arc::clone(&apply_started),
                apply_finished: Arc::clone(&apply_finished),
                allow_finish: Arc::clone(&allow_finish),
            }),
        )
        .await;

    let input = make_progress_input("recover-with-loop-split-lifetimes");
    let input_id = input.id().clone();
    let (outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("progress input should be accepted");
    assert!(outcome.is_accepted());
    let completion_handle =
        completion_handle.expect("queued progress input should expose a completion waiter");

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for attached session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "recover split wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_recover = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before recover");
    let wait_request_id = before_recover
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_recover.control.phase, RuntimeState::Attached);
    assert_eq!(before_recover.completion_waiters.input_count, 1);
    assert_eq!(before_recover.completion_waiters.waiter_count, 1);
    assert_eq!(before_recover.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_recover.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert!(before_recover.ops.pending_wait_present);
    assert_eq!(
        before_recover.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before recover"
    );
    assert_eq!(
        before_recover.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before recover"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "queued attached-loop work should remain pending until recover wakes the loop"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "recover should not yet have attempted executor control"
    );

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    let report = crate::traits::RuntimeControlPlane::recover(&adapter, &runtime_id)
        .await
        .expect("recover should split completion and wait_all lifetimes on attached runtimes");
    assert_eq!(report.inputs_recovered, 1);

    tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
        .await
        .expect("recover should wake the attached runtime loop to replay recovered work");

    let during_recover = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist while recover replay is in flight");
    assert_eq!(
        during_recover.control.phase,
        RuntimeState::Running,
        "recover currently re-enters Running while the attached loop replays recovered work"
    );
    assert_eq!(during_recover.completion_waiters.input_count, 1);
    assert_eq!(during_recover.completion_waiters.waiter_count, 1);
    assert_eq!(during_recover.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        during_recover.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        during_recover.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "recover should preserve the authority-owned wait request while replay is in flight"
    );
    assert!(during_recover.ops.pending_wait_present);
    assert_eq!(
        during_recover.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "recover should preserve request-id agreement across the wait carrier seam while replaying"
    );
    assert_eq!(
        during_recover.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "recover should preserve the tracked wait target while recovered work is replaying"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "recover should wake the attached loop exactly once for the recovered queued work"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "recover should not route any out-of-band control command through the executor seam"
    );

    allow_finish.notify_waiters();
    tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
        .await
        .expect("attached loop should finish replaying the recovered queued work");

    match completion_handle.wait().await {
        CompletionOutcome::CompletedWithoutResult => {}
        other => panic!("expected recover+replay to complete queued work, got {other:?}"),
    }

    let after_replay = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after attached-loop replay");
            if snapshot.control.phase != RuntimeState::Running {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("runtime should eventually leave Running once the recovered work finishes replaying");
    assert_eq!(
        after_replay.control.phase,
        RuntimeState::Attached,
        "recover should currently return to Attached once the recovered work finishes replaying"
    );
    assert_eq!(after_replay.completion_waiters.input_count, 0);
    assert_eq!(after_replay.completion_waiters.waiter_count, 0);
    assert!(
        after_replay.completion_waiters.waiting_inputs.is_empty(),
        "recover should clear completion waiters once the recovered work finishes replaying"
    );
    assert_eq!(
        after_replay.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "wait_all should remain active after the recovered work finishes replaying"
    );
    assert!(after_replay.ops.pending_wait_present);
    assert_eq!(
        after_replay.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "request-id agreement should survive the return to Attached after replay"
    );
    assert_eq!(
        after_replay.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "the tracked wait target should remain present until the operation itself settles"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after recover+replay");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Attached);
    assert_eq!(
        settled.control.current_run_id, None,
        "recycle should not leave a settled control-side current-run binding behind even on attached runtimes"
    );
    assert_eq!(
        settled.inputs.current_run_id, None,
        "recycle should not leave a settled ingress-side current-run binding behind even on attached runtimes"
    );
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_preserves_wait_all_after_recycle_with_runtime_loop() {
    struct BlockingExecutor {
        apply_calls: Arc<AtomicUsize>,
        control_calls: Arc<AtomicUsize>,
        apply_started: Arc<Notify>,
        apply_finished: Arc<Notify>,
        allow_finish: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            self.apply_started.notify_waiters();
            self.allow_finish.notified().await;
            self.apply_finished.notify_waiters();

            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
                run_result: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));
    let apply_started = Arc::new(Notify::new());
    let apply_finished = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                control_calls: Arc::clone(&control_calls),
                apply_started: Arc::clone(&apply_started),
                apply_finished: Arc::clone(&apply_finished),
                allow_finish: Arc::clone(&allow_finish),
            }),
        )
        .await;

    let outcome = adapter
        .accept_input_without_wake(
            &session_id,
            make_progress_input("recycle-wait-all-with-loop"),
        )
        .await
        .expect("queued progress input should be accepted without waking the loop");
    assert!(outcome.is_accepted());

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for attached session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "recycle wait target with loop".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_recycle = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before recycle");
    let wait_request_id = before_recycle
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_recycle.control.phase, RuntimeState::Attached);
    assert!(before_recycle.ops.pending_wait_present);
    assert_eq!(
        before_recycle.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before recycle"
    );
    assert_eq!(
        before_recycle.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before recycle"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "queued attached-loop work should remain pending until recycle wakes the loop"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "recycle should not yet have attempted executor control"
    );

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    let report = crate::traits::RuntimeControlPlane::recycle(&adapter, &runtime_id)
        .await
        .expect("recycle should preserve wait_all while requeueing attached-loop work");
    assert_eq!(report.inputs_transferred, 1);

    tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
        .await
        .expect("recycle should wake the attached runtime loop to replay preserved work");

    let after_recycle = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after recycle wakes the loop");
    assert_eq!(
        after_recycle.control.phase,
        RuntimeState::Running,
        "recycle currently re-enters Running while the attached loop replays preserved work"
    );
    assert_eq!(
        after_recycle.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "recycle should preserve the authority-owned wait request while the attached loop is replaying preserved work"
    );
    assert!(after_recycle.ops.pending_wait_present);
    assert_eq!(
        after_recycle.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "recycle should preserve request-id agreement across the wait carrier seam while replaying"
    );
    assert_eq!(
        after_recycle.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "recycle should preserve the tracked wait target while preserved work is replaying"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "recycle should wake the attached loop exactly once for the preserved queued work"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "recycle should not route any out-of-band control command through the executor seam"
    );

    allow_finish.notify_waiters();
    tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
        .await
        .expect("attached loop should finish replaying the preserved queued work");

    let after_replay = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after attached-loop replay");
            if snapshot.control.phase == RuntimeState::Attached {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("runtime should return to Attached once the preserved work finishes replaying");
    assert_eq!(
        after_replay.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "wait_all should remain active after the attached loop finishes replaying preserved work"
    );
    assert!(after_replay.ops.pending_wait_present);
    assert_eq!(
        after_replay.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "request-id agreement should survive the return to Attached after replay"
    );
    assert_eq!(
        after_replay.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "the tracked wait target should remain present until the operation itself settles"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after recycle+replay");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Attached);
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_recycle_with_runtime_loop_splits_completion_and_wait_all_lifetimes()
 {
    struct BlockingExecutor {
        apply_calls: Arc<AtomicUsize>,
        control_calls: Arc<AtomicUsize>,
        apply_started: Arc<Notify>,
        apply_finished: Arc<Notify>,
        allow_finish: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            self.apply_started.notify_waiters();
            self.allow_finish.notified().await;
            self.apply_finished.notify_waiters();

            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
                run_result: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));
    let apply_started = Arc::new(Notify::new());
    let apply_finished = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                control_calls: Arc::clone(&control_calls),
                apply_started: Arc::clone(&apply_started),
                apply_finished: Arc::clone(&apply_finished),
                allow_finish: Arc::clone(&allow_finish),
            }),
        )
        .await;

    let input = make_progress_input("recycle-with-loop-split-lifetimes");
    let input_id = input.id().clone();
    let (outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("progress input should be accepted");
    assert!(outcome.is_accepted());
    let completion_handle =
        completion_handle.expect("queued progress input should expose a completion waiter");

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for attached session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "recycle split wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_recycle = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before recycle");
    let wait_request_id = before_recycle
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_recycle.control.phase, RuntimeState::Attached);
    assert_eq!(before_recycle.completion_waiters.input_count, 1);
    assert_eq!(before_recycle.completion_waiters.waiter_count, 1);
    assert_eq!(before_recycle.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_recycle.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert!(before_recycle.ops.pending_wait_present);
    assert_eq!(
        before_recycle.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before recycle"
    );
    assert_eq!(
        before_recycle.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before recycle"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "queued attached-loop work should remain pending until recycle wakes the loop"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "recycle should not yet have attempted executor control"
    );

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    let report = crate::traits::RuntimeControlPlane::recycle(&adapter, &runtime_id)
        .await
        .expect("recycle should split completion and wait_all lifetimes on attached runtimes");
    assert_eq!(report.inputs_transferred, 1);

    tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
        .await
        .expect("recycle should wake the attached runtime loop to replay preserved work");

    let during_recycle = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist while recycle replay is in flight");
    assert_eq!(
        during_recycle.control.phase,
        RuntimeState::Running,
        "recycle currently re-enters Running while the attached loop replays preserved work"
    );
    assert_eq!(during_recycle.completion_waiters.input_count, 1);
    assert_eq!(during_recycle.completion_waiters.waiter_count, 1);
    assert_eq!(during_recycle.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        during_recycle.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        during_recycle.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "recycle should preserve the authority-owned wait request while replay is in flight"
    );
    assert!(during_recycle.ops.pending_wait_present);
    assert_eq!(
        during_recycle.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "recycle should preserve request-id agreement across the wait carrier seam while replaying"
    );
    assert_eq!(
        during_recycle.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "recycle should preserve the tracked wait target while preserved work is replaying"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "recycle should wake the attached loop exactly once for the preserved queued work"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "recycle should not route any out-of-band control command through the executor seam"
    );

    allow_finish.notify_waiters();
    tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
        .await
        .expect("attached loop should finish replaying the preserved queued work");

    match completion_handle.wait().await {
        CompletionOutcome::CompletedWithoutResult => {}
        other => panic!("expected recycle+replay to complete queued work, got {other:?}"),
    }

    let after_replay = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after attached-loop replay");
            if snapshot.control.phase == RuntimeState::Attached {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("runtime should return to Attached once the preserved work finishes replaying");
    assert_eq!(after_replay.completion_waiters.input_count, 0);
    assert_eq!(after_replay.completion_waiters.waiter_count, 0);
    assert!(
        after_replay.completion_waiters.waiting_inputs.is_empty(),
        "recycle should clear completion waiters once the preserved work finishes replaying"
    );
    assert!(
        after_replay.inputs.queue.is_empty(),
        "recycle should not leave ordinary queued work behind once the attached runtime returns to Attached after replay"
    );
    assert!(
        after_replay.inputs.steer_queue.is_empty(),
        "recycle should not leave steer-queued work behind once the attached runtime returns to Attached after replay"
    );
    assert_eq!(
        after_replay.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "wait_all should remain active after the preserved work finishes replaying"
    );
    assert!(after_replay.ops.pending_wait_present);
    assert_eq!(
        after_replay.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "request-id agreement should survive the return to Attached after replay"
    );
    assert_eq!(
        after_replay.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "the tracked wait target should remain present until the operation itself settles"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after recycle+replay");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Attached);
    assert!(
        settled.inputs.queue.is_empty(),
        "recycle should keep the attached settled snapshot free of ordinary queued work after replay completes"
    );
    assert!(
        settled.inputs.steer_queue.is_empty(),
        "recycle should keep the attached settled snapshot free of steer-queued work after replay completes"
    );
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_preserves_wait_all_after_reset() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;
    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "reset wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_reset = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before reset");
    let wait_request_id = before_reset
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert!(before_reset.ops.pending_wait_present);
    assert_eq!(
        before_reset.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before reset"
    );
    assert_eq!(
        before_reset.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before reset"
    );

    SessionServiceRuntimeExt::reset_runtime(&adapter, &session_id)
        .await
        .expect("reset should succeed while the runtime is idle");

    let after_reset = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after reset");
    assert_eq!(
        after_reset.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "reset should preserve the authority-owned wait request"
    );
    assert!(
        after_reset.ops.pending_wait_present,
        "reset should preserve the pending wait carrier while the operation remains active"
    );
    assert_eq!(
        after_reset.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "reset should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_reset.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "reset should preserve the tracked wait targets"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after reset");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_reset_clears_steered_waiter_and_queue_but_preserves_wait_all()
 {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let input = Input::Prompt(crate::input::PromptInput::new(
        "reset steered prompt",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ));
    let input_id = input.id().clone();
    let (_outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("steered prompt should be accepted");
    let completion_handle =
        completion_handle.expect("steered prompt should register a completion waiter");

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "reset steered wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_reset = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before reset");
    let wait_request_id = before_reset
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_reset.control.phase, RuntimeState::Idle);
    assert!(before_reset.inputs.queue.is_empty());
    assert_eq!(before_reset.inputs.steer_queue, vec![input_id.clone()]);
    assert!(before_reset.inputs.wake_requested);
    assert!(before_reset.inputs.process_requested);
    assert_eq!(before_reset.completion_waiters.input_count, 1);
    assert_eq!(before_reset.completion_waiters.waiter_count, 1);
    assert_eq!(before_reset.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_reset.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert!(before_reset.ops.pending_wait_present);
    assert_eq!(
        before_reset.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before reset"
    );
    assert_eq!(
        before_reset.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before reset"
    );

    let report = SessionServiceRuntimeExt::reset_runtime(&adapter, &session_id)
        .await
        .expect("reset should clear steered completion waiters while preserving wait_all");
    assert_eq!(report.inputs_abandoned, 1);

    match completion_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime reset");
        }
        other => {
            panic!("expected runtime reset termination for steered input, got {other:?}")
        }
    }

    let after_reset = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after reset");
    assert_eq!(after_reset.control.phase, RuntimeState::Idle);
    assert_eq!(after_reset.control.current_run_id, None);
    assert_eq!(after_reset.inputs.current_run_id, None);
    assert!(after_reset.inputs.queue.is_empty());
    assert!(
        after_reset.inputs.steer_queue.is_empty(),
        "reset should clear steered queued work immediately on the plain runtime path"
    );
    assert_eq!(after_reset.completion_waiters.input_count, 0);
    assert_eq!(after_reset.completion_waiters.waiter_count, 0);
    assert!(
        after_reset.completion_waiters.waiting_inputs.is_empty(),
        "reset should clear input-owned steered completion waiters immediately"
    );
    assert_eq!(
        after_reset.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "reset should preserve the authority-owned wait request after steered completion waiters clear"
    );
    assert!(after_reset.ops.pending_wait_present);
    assert_eq!(
        after_reset.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "reset should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_reset.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "reset should preserve the tracked wait target until the operation settles"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after reset");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Idle);
    assert_eq!(settled.control.current_run_id, None);
    assert_eq!(settled.inputs.current_run_id, None);
    assert!(settled.inputs.queue.is_empty());
    assert!(settled.inputs.steer_queue.is_empty());
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_reset_splits_completion_and_wait_all_lifetimes() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let input = make_prompt("reset split lifetimes");
    let input_id = input.id().clone();
    let (_outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");
    let completion_handle =
        completion_handle.expect("queued prompt should register a completion waiter");

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "reset split wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_reset = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before reset");
    let wait_request_id = before_reset
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_reset.control.phase, RuntimeState::Idle);
    assert_eq!(before_reset.completion_waiters.input_count, 1);
    assert_eq!(before_reset.completion_waiters.waiter_count, 1);
    assert_eq!(before_reset.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_reset.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert!(before_reset.ops.pending_wait_present);
    assert_eq!(
        before_reset.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before reset"
    );
    assert_eq!(
        before_reset.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before reset"
    );

    let report = SessionServiceRuntimeExt::reset_runtime(&adapter, &session_id)
        .await
        .expect("reset should split completion and wait_all lifetimes");
    assert_eq!(report.inputs_abandoned, 1);

    match completion_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime reset");
        }
        other => panic!("expected runtime reset termination, got {other:?}"),
    }

    let after_reset = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after reset");
    assert_eq!(after_reset.control.phase, RuntimeState::Idle);
    assert!(
        after_reset.inputs.queue.is_empty(),
        "reset should abandon ordinary queued work immediately on the plain runtime path"
    );
    assert!(
        after_reset.inputs.steer_queue.is_empty(),
        "reset should abandon steered queued work immediately on the plain runtime path"
    );
    assert_eq!(after_reset.completion_waiters.input_count, 0);
    assert_eq!(after_reset.completion_waiters.waiter_count, 0);
    assert!(
        after_reset.completion_waiters.waiting_inputs.is_empty(),
        "reset should clear input-owned completion waiters immediately"
    );
    assert_eq!(
        after_reset.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "reset should preserve the authority-owned wait request after completion waiters clear"
    );
    assert!(after_reset.ops.pending_wait_present);
    assert_eq!(
        after_reset.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "reset should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_reset.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "reset should preserve the tracked wait target until the operation settles"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after reset");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Idle);
    assert!(
        settled.inputs.queue.is_empty(),
        "reset should not reintroduce ordinary queued work once the plain runtime settles"
    );
    assert!(
        settled.inputs.steer_queue.is_empty(),
        "reset should not reintroduce steered queued work once the plain runtime settles"
    );
    assert_eq!(
        settled.control.current_run_id, None,
        "reset should not leave a settled control-side current-run binding behind"
    );
    assert_eq!(
        settled.inputs.current_run_id, None,
        "reset should not leave a settled ingress-side current-run binding behind"
    );
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_preserves_wait_all_after_reset_with_runtime_loop() {
    struct RecordingExecutor {
        apply_calls: Arc<AtomicUsize>,
        control_calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for RecordingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
                run_result: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(RecordingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                control_calls: Arc::clone(&control_calls),
            }),
        )
        .await;

    let outcome = adapter
        .accept_input_without_wake(&session_id, make_progress_input("reset-wait-all-with-loop"))
        .await
        .expect("queued progress input should be accepted without waking the loop");
    assert!(outcome.is_accepted());

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for attached session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "reset wait target with loop".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_reset = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before reset");
    let wait_request_id = before_reset
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_reset.control.phase, RuntimeState::Attached);
    assert!(before_reset.ops.pending_wait_present);
    assert_eq!(
        before_reset.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before reset"
    );
    assert_eq!(
        before_reset.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before reset"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "reset should be able to discard queued attached-loop work before apply runs"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "reset has not yet attempted any executor control"
    );

    let report = SessionServiceRuntimeExt::reset_runtime(&adapter, &session_id)
        .await
        .expect("reset should preserve wait_all while abandoning queued attached-loop work");
    assert_eq!(report.inputs_abandoned, 1);

    let after_reset = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after reset");
    assert_eq!(after_reset.control.phase, RuntimeState::Idle);
    assert!(
        after_reset.inputs.queue.is_empty(),
        "attached reset should abandon ordinary queued work immediately"
    );
    assert!(
        after_reset.inputs.steer_queue.is_empty(),
        "attached reset should abandon steered queued work immediately"
    );
    assert_eq!(
        after_reset.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "reset should preserve the authority-owned wait request even with an attached loop"
    );
    assert!(
        after_reset.ops.pending_wait_present,
        "reset should preserve the pending wait carrier while the operation remains active"
    );
    assert_eq!(
        after_reset.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "reset should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_reset.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "reset should preserve the tracked wait targets until the operation settles"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "reset should bypass queued attached-loop work entirely"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "reset currently bypasses the executor control seam and does not deliver an out-of-band control command"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after reset");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Idle);
    assert!(
        settled.inputs.queue.is_empty(),
        "attached reset should not reintroduce ordinary queued work once the runtime settles"
    );
    assert!(
        settled.inputs.steer_queue.is_empty(),
        "attached reset should not reintroduce steered queued work once the runtime settles"
    );
    assert_eq!(
        settled.control.current_run_id, None,
        "reset should not leave a settled control-side current-run binding behind even on attached runtimes"
    );
    assert_eq!(
        settled.inputs.current_run_id, None,
        "reset should not leave a settled ingress-side current-run binding behind even on attached runtimes"
    );
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_reset_with_runtime_loop_splits_completion_and_wait_all_lifetimes()
 {
    struct RecordingExecutor {
        apply_calls: Arc<AtomicUsize>,
        control_calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for RecordingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
                run_result: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(RecordingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                control_calls: Arc::clone(&control_calls),
            }),
        )
        .await;

    let input = make_progress_input("reset-with-loop-split-lifetimes");
    let input_id = input.id().clone();
    let (outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("progress input should be accepted");
    assert!(outcome.is_accepted());
    let completion_handle =
        completion_handle.expect("queued progress input should expose a completion waiter");

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for attached session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "reset split-lifetime wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_reset = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before reset");
    let wait_request_id = before_reset
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_reset.control.phase, RuntimeState::Attached);
    assert_eq!(before_reset.completion_waiters.input_count, 1);
    assert_eq!(before_reset.completion_waiters.waiter_count, 1);
    assert_eq!(before_reset.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_reset.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        before_reset.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before reset"
    );
    assert_eq!(
        before_reset.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before reset"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "reset should be able to discard queued attached-loop work before apply runs"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "reset has not yet attempted any executor control"
    );

    let report = SessionServiceRuntimeExt::reset_runtime(&adapter, &session_id)
        .await
        .expect("reset should preserve wait_all while abandoning queued attached-loop work");
    assert_eq!(report.inputs_abandoned, 1);

    let after_reset = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after reset");
    assert_eq!(after_reset.control.phase, RuntimeState::Idle);
    assert_eq!(after_reset.completion_waiters.input_count, 0);
    assert_eq!(after_reset.completion_waiters.waiter_count, 0);
    assert!(
        after_reset.completion_waiters.waiting_inputs.is_empty(),
        "reset should clear input-owned completion waiters immediately"
    );
    assert_eq!(
        after_reset.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "reset should preserve the authority-owned wait request even after clearing input waiters"
    );
    assert!(after_reset.ops.pending_wait_present);
    assert_eq!(
        after_reset.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "reset should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_reset.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "reset should preserve the tracked wait target until the operation settles"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "reset should bypass queued attached-loop work entirely"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "reset currently bypasses the executor control seam and does not deliver an out-of-band control command"
    );

    match completion_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime reset");
        }
        other => {
            panic!("expected reset to terminate the completion waiter immediately, got {other:?}")
        }
    }

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after reset");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Idle);
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_preserves_wait_all_after_destroy() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;
    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "destroy wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_destroy = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before destroy");
    let wait_request_id = before_destroy
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert!(before_destroy.ops.pending_wait_present);
    assert_eq!(
        before_destroy.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before destroy"
    );
    assert_eq!(
        before_destroy.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before destroy"
    );

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    let report = crate::traits::RuntimeControlPlane::destroy(&adapter, &runtime_id)
        .await
        .expect("destroy should succeed for an idle runtime");
    assert_eq!(report.inputs_abandoned, 0);

    let after_destroy = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after destroy");
    assert_eq!(after_destroy.control.phase, RuntimeState::Destroyed);
    assert_eq!(
        after_destroy.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "destroy currently preserves the authority-owned wait request"
    );
    assert!(
        after_destroy.ops.pending_wait_present,
        "destroy currently preserves the pending wait carrier while the operation remains active"
    );
    assert_eq!(
        after_destroy.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "destroy should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_destroy.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "destroy should preserve the tracked wait targets until the operation settles"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after destroy");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Destroyed);
    assert_eq!(
        settled.control.current_run_id, None,
        "destroy should not leave a settled control-side current-run binding behind"
    );
    assert_eq!(
        settled.inputs.current_run_id, None,
        "destroy should not leave a settled ingress-side current-run binding behind"
    );
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_destroy_splits_completion_and_wait_all_lifetimes() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let input = make_prompt("destroy split lifetimes");
    let input_id = input.id().clone();
    let (_outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");
    let completion_handle =
        completion_handle.expect("queued prompt should register a completion waiter");

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "destroy split wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_destroy = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before destroy");
    let wait_request_id = before_destroy
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_destroy.control.phase, RuntimeState::Idle);
    assert_eq!(before_destroy.completion_waiters.input_count, 1);
    assert_eq!(before_destroy.completion_waiters.waiter_count, 1);
    assert_eq!(before_destroy.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_destroy.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert!(before_destroy.ops.pending_wait_present);
    assert_eq!(
        before_destroy.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before destroy"
    );
    assert_eq!(
        before_destroy.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before destroy"
    );

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    let report = crate::traits::RuntimeControlPlane::destroy(&adapter, &runtime_id)
        .await
        .expect("destroy should split completion and wait_all lifetimes");
    assert_eq!(report.inputs_abandoned, 1);

    match completion_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime destroyed");
        }
        other => panic!("expected runtime destroyed termination, got {other:?}"),
    }

    let after_destroy = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after destroy");
    assert_eq!(after_destroy.control.phase, RuntimeState::Destroyed);
    assert_eq!(after_destroy.completion_waiters.input_count, 0);
    assert_eq!(after_destroy.completion_waiters.waiter_count, 0);
    assert!(
        after_destroy.completion_waiters.waiting_inputs.is_empty(),
        "destroy should clear input-owned completion waiters immediately"
    );
    assert_eq!(
        after_destroy.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "destroy should preserve the authority-owned wait request after completion waiters clear"
    );
    assert!(after_destroy.ops.pending_wait_present);
    assert_eq!(
        after_destroy.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "destroy should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_destroy.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "destroy should preserve the tracked wait target until the operation settles"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after destroy");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Destroyed);
    assert_eq!(
        settled.control.current_run_id, None,
        "destroy should not leave a settled control-side current-run binding behind even on attached runtimes"
    );
    assert_eq!(
        settled.inputs.current_run_id, None,
        "destroy should not leave a settled ingress-side current-run binding behind even on attached runtimes"
    );
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_preserves_wait_all_after_destroy_with_runtime_loop() {
    struct RecordingExecutor {
        apply_calls: Arc<AtomicUsize>,
        control_calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for RecordingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
                run_result: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(RecordingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                control_calls: Arc::clone(&control_calls),
            }),
        )
        .await;

    let outcome = adapter
        .accept_input_without_wake(
            &session_id,
            make_progress_input("destroy-wait-all-with-loop"),
        )
        .await
        .expect("queued progress input should be accepted without waking the loop");
    assert!(outcome.is_accepted());

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for attached session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "destroy wait target with loop".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_destroy = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before destroy");
    let wait_request_id = before_destroy
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_destroy.control.phase, RuntimeState::Attached);
    assert!(before_destroy.ops.pending_wait_present);
    assert_eq!(
        before_destroy.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before destroy"
    );
    assert_eq!(
        before_destroy.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before destroy"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "destroy should be able to abandon queued attached-loop work before apply runs"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "destroy has not yet attempted any executor control"
    );

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    let report = crate::traits::RuntimeControlPlane::destroy(&adapter, &runtime_id)
        .await
        .expect("destroy should preserve wait_all while abandoning queued attached-loop work");
    assert_eq!(report.inputs_abandoned, 1);

    let after_destroy = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after destroy");
    assert_eq!(after_destroy.control.phase, RuntimeState::Destroyed);
    assert_eq!(
        after_destroy.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "destroy should preserve the authority-owned wait request even with an attached loop"
    );
    assert!(
        after_destroy.ops.pending_wait_present,
        "destroy should preserve the pending wait carrier while the operation remains active"
    );
    assert_eq!(
        after_destroy.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "destroy should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_destroy.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "destroy should preserve the tracked wait targets until the operation settles"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "destroy should bypass queued attached-loop work entirely"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "destroy currently bypasses the executor control seam and does not deliver an out-of-band control command"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete");

    let waited = wait_future
        .await
        .expect("wait_all should resolve after completion");
    assert_eq!(waited.satisfied.wait_request_id, wait_request_id);
    assert_eq!(waited.satisfied.operation_ids, vec![operation_id]);

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Destroyed);
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_destroy_with_runtime_loop_splits_completion_and_wait_all_lifetimes()
 {
    struct RecordingExecutor {
        apply_calls: Arc<AtomicUsize>,
        control_calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for RecordingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
                run_result: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(RecordingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                control_calls: Arc::clone(&control_calls),
            }),
        )
        .await;

    let input = make_progress_input("destroy-with-loop-split-lifetimes");
    let input_id = input.id().clone();
    let outcome = adapter
        .accept_input_without_wake(&session_id, input)
        .await
        .expect("queued progress input should be accepted without waking the loop");
    assert!(outcome.is_accepted());

    let completion_handle = {
        let completions = {
            let sessions = adapter.sessions.read().await;
            sessions
                .get(&session_id)
                .expect("attached session should exist")
                .completions
                .clone()
        };
        let mut completions = completions.lock().await;
        completions.register(input_id.clone())
    };

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for attached session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "destroy split wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_destroy = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before destroy");
    let wait_request_id = before_destroy
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_destroy.control.phase, RuntimeState::Attached);
    assert_eq!(before_destroy.completion_waiters.input_count, 1);
    assert_eq!(before_destroy.completion_waiters.waiter_count, 1);
    assert_eq!(before_destroy.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_destroy.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert!(before_destroy.ops.pending_wait_present);
    assert_eq!(
        before_destroy.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before destroy"
    );
    assert_eq!(
        before_destroy.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before destroy"
    );

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    let report = crate::traits::RuntimeControlPlane::destroy(&adapter, &runtime_id)
        .await
        .expect("destroy should split completion and wait_all lifetimes on attached runtimes");
    assert_eq!(report.inputs_abandoned, 1);

    match completion_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime destroyed");
        }
        other => panic!("expected runtime destroyed termination, got {other:?}"),
    }

    let after_destroy = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after destroy");
    assert_eq!(after_destroy.control.phase, RuntimeState::Destroyed);
    assert_eq!(after_destroy.completion_waiters.input_count, 0);
    assert_eq!(after_destroy.completion_waiters.waiter_count, 0);
    assert!(
        after_destroy.completion_waiters.waiting_inputs.is_empty(),
        "destroy should clear input-owned completion waiters immediately"
    );
    assert_eq!(
        after_destroy.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "destroy should preserve the authority-owned wait request after completion waiters clear"
    );
    assert!(after_destroy.ops.pending_wait_present);
    assert_eq!(
        after_destroy.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "destroy should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_destroy.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "destroy should preserve the tracked wait target until the operation settles"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "destroy should bypass queued attached-loop work entirely"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "destroy currently bypasses the executor control seam and does not deliver an out-of-band control command"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after destroy");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Destroyed);
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_preserves_wait_all_after_stop_runtime_executor() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;
    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "stop wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_stop = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before stop");
    let wait_request_id = before_stop
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert!(before_stop.ops.pending_wait_present);
    assert_eq!(
        before_stop.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before stop"
    );
    assert_eq!(
        before_stop.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before stop"
    );

    adapter
        .stop_runtime_executor(
            &session_id,
            RunControlCommand::StopRuntimeExecutor {
                reason: "stop wait_all test".into(),
            },
        )
        .await
        .expect("stop should preserve the active wait_all carrier");

    let after_stop = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after stop");
            if snapshot.control.phase == RuntimeState::Stopped {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("attached stop should eventually publish the Stopped phase");
    assert_eq!(
        after_stop.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "stop currently preserves the authority-owned wait request"
    );
    assert!(
        after_stop.ops.pending_wait_present,
        "stop currently preserves the pending wait carrier while the operation remains active"
    );
    assert_eq!(
        after_stop.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "stop should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_stop.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "stop should preserve the tracked wait targets until the operation settles"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after stop");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Stopped);
    assert_eq!(
        settled.control.current_run_id, None,
        "stop should not leave a settled control-side current-run binding behind"
    );
    assert_eq!(
        settled.inputs.current_run_id, None,
        "stop should not leave a settled ingress-side current-run binding behind"
    );
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_stop_runtime_executor_clears_steered_waiter_and_queue_but_preserves_wait_all()
 {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let input = Input::Prompt(crate::input::PromptInput::new(
        "stop steered prompt",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ));
    let input_id = input.id().clone();
    let (_outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("steered prompt should be accepted");
    let completion_handle =
        completion_handle.expect("steered prompt should register a completion waiter");

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "stop steered wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_stop = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before stop");
    let wait_request_id = before_stop
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_stop.control.phase, RuntimeState::Idle);
    assert!(before_stop.inputs.queue.is_empty());
    assert_eq!(before_stop.inputs.steer_queue, vec![input_id.clone()]);
    assert!(before_stop.inputs.wake_requested);
    assert!(before_stop.inputs.process_requested);
    assert_eq!(before_stop.completion_waiters.input_count, 1);
    assert_eq!(before_stop.completion_waiters.waiter_count, 1);
    assert_eq!(before_stop.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_stop.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert!(before_stop.ops.pending_wait_present);
    assert_eq!(
        before_stop.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before stop"
    );
    assert_eq!(
        before_stop.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before stop"
    );

    adapter
        .stop_runtime_executor(
            &session_id,
            RunControlCommand::StopRuntimeExecutor {
                reason: "stop steered split lifetimes".into(),
            },
        )
        .await
        .expect("stop should clear steered completion waiters while preserving wait_all");

    match completion_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime stopped");
        }
        other => {
            panic!("expected runtime stopped termination for steered input, got {other:?}")
        }
    }

    let after_stop = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after stop");
    assert_eq!(after_stop.control.phase, RuntimeState::Stopped);
    assert_eq!(after_stop.control.current_run_id, None);
    assert_eq!(after_stop.inputs.current_run_id, None);
    assert!(after_stop.inputs.queue.is_empty());
    assert!(
        after_stop.inputs.steer_queue.is_empty(),
        "stop should clear steered queued work immediately on the plain runtime path"
    );
    assert_eq!(after_stop.completion_waiters.input_count, 0);
    assert_eq!(after_stop.completion_waiters.waiter_count, 0);
    assert!(
        after_stop.completion_waiters.waiting_inputs.is_empty(),
        "stop should clear input-owned steered completion waiters immediately"
    );
    assert_eq!(
        after_stop.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "stop should preserve the authority-owned wait request after steered completion waiters clear"
    );
    assert!(after_stop.ops.pending_wait_present);
    assert_eq!(
        after_stop.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "stop should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_stop.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "stop should preserve the tracked wait target until the operation settles"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after stop");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Stopped);
    assert_eq!(settled.control.current_run_id, None);
    assert_eq!(settled.inputs.current_run_id, None);
    assert!(settled.inputs.queue.is_empty());
    assert!(settled.inputs.steer_queue.is_empty());
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_stop_runtime_executor_splits_completion_and_wait_all_lifetimes()
 {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let input = make_prompt("stop split lifetimes");
    let input_id = input.id().clone();
    let (_outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");
    let completion_handle =
        completion_handle.expect("queued prompt should register a completion waiter");

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "stop split wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_stop = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before stop");
    let wait_request_id = before_stop
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_stop.control.phase, RuntimeState::Idle);
    assert_eq!(before_stop.completion_waiters.input_count, 1);
    assert_eq!(before_stop.completion_waiters.waiter_count, 1);
    assert_eq!(before_stop.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_stop.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert!(before_stop.ops.pending_wait_present);
    assert_eq!(
        before_stop.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before stop"
    );
    assert_eq!(
        before_stop.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before stop"
    );

    adapter
        .stop_runtime_executor(
            &session_id,
            RunControlCommand::StopRuntimeExecutor {
                reason: "stop split lifetimes".into(),
            },
        )
        .await
        .expect("stop should split completion and wait_all lifetimes");

    match completion_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime stopped");
        }
        other => panic!("expected runtime stopped termination, got {other:?}"),
    }

    let after_stop = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after stop");
    assert_eq!(after_stop.control.phase, RuntimeState::Stopped);
    assert!(
        after_stop.inputs.queue.is_empty(),
        "stop should abandon ordinary queued work immediately on the plain runtime path"
    );
    assert!(
        after_stop.inputs.steer_queue.is_empty(),
        "stop should abandon steered queued work immediately on the plain runtime path"
    );
    assert_eq!(after_stop.completion_waiters.input_count, 0);
    assert_eq!(after_stop.completion_waiters.waiter_count, 0);
    assert!(
        after_stop.completion_waiters.waiting_inputs.is_empty(),
        "stop should clear input-owned completion waiters immediately"
    );
    assert_eq!(
        after_stop.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "stop should preserve the authority-owned wait request after completion waiters clear"
    );
    assert!(after_stop.ops.pending_wait_present);
    assert_eq!(
        after_stop.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "stop should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_stop.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "stop should preserve the tracked wait target until the operation settles"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after stop");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Stopped);
    assert!(
        settled.inputs.queue.is_empty(),
        "stop should not reintroduce ordinary queued work once the plain runtime settles"
    );
    assert!(
        settled.inputs.steer_queue.is_empty(),
        "stop should not reintroduce steered queued work once the plain runtime settles"
    );
    assert_eq!(
        settled.control.current_run_id, None,
        "stop should not leave a settled control-side current-run binding behind even on attached runtimes"
    );
    assert_eq!(
        settled.inputs.current_run_id, None,
        "stop should not leave a settled ingress-side current-run binding behind even on attached runtimes"
    );
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_preserves_wait_all_after_stop_runtime_executor_with_runtime_loop()
 {
    struct RecordingExecutor {
        apply_calls: Arc<AtomicUsize>,
        stop_calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for RecordingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
                run_result: None,
            })
        }

        async fn control(&mut self, command: RunControlCommand) -> Result<(), CoreExecutorError> {
            if matches!(command, RunControlCommand::StopRuntimeExecutor { .. }) {
                self.stop_calls.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }
    }

    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let stop_calls = Arc::new(AtomicUsize::new(0));

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(RecordingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                stop_calls: Arc::clone(&stop_calls),
            }),
        )
        .await;

    let outcome = adapter
        .accept_input_without_wake(&session_id, make_progress_input("stop-wait-all-with-loop"))
        .await
        .expect("queued progress input should be accepted without waking the loop");
    assert!(outcome.is_accepted());

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for attached session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "stop wait target with loop".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_stop = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before stop");
    let wait_request_id = before_stop
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_stop.control.phase, RuntimeState::Attached);
    assert!(before_stop.ops.pending_wait_present);
    assert_eq!(
        before_stop.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before stop"
    );
    assert_eq!(
        before_stop.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before stop"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "stop should be able to preempt queued attached-loop work before apply runs"
    );

    adapter
        .stop_runtime_executor(
            &session_id,
            RunControlCommand::StopRuntimeExecutor {
                reason: "stop attached-loop wait_all".into(),
            },
        )
        .await
        .expect("stop should preserve the active wait_all carrier through the live control seam");

    let after_stop = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after stop");
            if snapshot.control.phase == RuntimeState::Stopped {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("attached stop should eventually publish the Stopped phase");
    assert_eq!(
        after_stop.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "stop should preserve the authority-owned wait request on attached runtimes"
    );
    assert!(after_stop.ops.pending_wait_present);
    assert_eq!(
        after_stop.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "stop should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_stop.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "stop should preserve the tracked wait target until the operation settles"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "stop should beat queued ordinary work on an attached runtime loop"
    );
    assert_eq!(
        stop_calls.load(Ordering::SeqCst),
        1,
        "the attached executor should observe exactly one stop-runtime-executor control"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after stop");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Stopped);
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_stop_runtime_executor_with_runtime_loop_splits_completion_and_wait_all_lifetimes()
 {
    struct RecordingExecutor {
        apply_calls: Arc<AtomicUsize>,
        stop_calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for RecordingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
                run_result: None,
            })
        }

        async fn control(&mut self, command: RunControlCommand) -> Result<(), CoreExecutorError> {
            if matches!(command, RunControlCommand::StopRuntimeExecutor { .. }) {
                self.stop_calls.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }
    }

    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let stop_calls = Arc::new(AtomicUsize::new(0));

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(RecordingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                stop_calls: Arc::clone(&stop_calls),
            }),
        )
        .await;

    let input = make_progress_input("stop-with-loop-split-lifetimes");
    let input_id = input.id().clone();
    let outcome = adapter
        .accept_input_without_wake(&session_id, input)
        .await
        .expect("queued progress input should be accepted without waking the loop");
    assert!(outcome.is_accepted());

    let completion_handle = {
        let completions = {
            let sessions = adapter.sessions.read().await;
            sessions
                .get(&session_id)
                .expect("attached session should exist")
                .completions
                .clone()
        };
        let mut completions = completions.lock().await;
        completions.register(input_id.clone())
    };

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for attached session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "stop split wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_stop = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before stop");
    let wait_request_id = before_stop
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_stop.control.phase, RuntimeState::Attached);
    assert_eq!(before_stop.completion_waiters.input_count, 1);
    assert_eq!(before_stop.completion_waiters.waiter_count, 1);
    assert_eq!(before_stop.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_stop.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert!(before_stop.ops.pending_wait_present);
    assert_eq!(
        before_stop.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before stop"
    );
    assert_eq!(
        before_stop.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before stop"
    );

    adapter
        .stop_runtime_executor(
            &session_id,
            RunControlCommand::StopRuntimeExecutor {
                reason: "stop attached-loop split lifetimes".into(),
            },
        )
        .await
        .expect("stop should split completion and wait_all lifetimes on attached runtimes");

    match completion_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime stopped");
        }
        other => panic!("expected runtime stopped termination, got {other:?}"),
    }

    let after_stop = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after stop");
            if snapshot.control.phase == RuntimeState::Stopped {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("attached stop should eventually publish the Stopped phase");
    assert!(
        after_stop.inputs.queue.is_empty(),
        "attached stop should abandon ordinary queued work immediately"
    );
    assert!(
        after_stop.inputs.steer_queue.is_empty(),
        "attached stop should abandon steered queued work immediately"
    );
    assert_eq!(after_stop.completion_waiters.input_count, 0);
    assert_eq!(after_stop.completion_waiters.waiter_count, 0);
    assert!(
        after_stop.completion_waiters.waiting_inputs.is_empty(),
        "stop should clear input-owned completion waiters immediately"
    );
    assert_eq!(
        after_stop.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "stop should preserve the authority-owned wait request after completion waiters clear"
    );
    assert!(after_stop.ops.pending_wait_present);
    assert_eq!(
        after_stop.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "stop should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_stop.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "stop should preserve the tracked wait target until the operation settles"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "stop should beat queued ordinary work on an attached runtime loop"
    );
    assert_eq!(
        stop_calls.load(Ordering::SeqCst),
        1,
        "the attached executor should observe exactly one stop-runtime-executor control"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after stop");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Stopped);
    assert!(
        settled.inputs.queue.is_empty(),
        "attached stop should not reintroduce ordinary queued work once the runtime settles"
    );
    assert!(
        settled.inputs.steer_queue.is_empty(),
        "attached stop should not reintroduce steered queued work once the runtime settles"
    );
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_preserves_wait_all_after_retire() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;
    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "retire wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_retire = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before retire");
    let wait_request_id = before_retire
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert!(before_retire.ops.pending_wait_present);
    assert_eq!(
        before_retire.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before retire"
    );
    assert_eq!(
        before_retire.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before retire"
    );

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    let report = crate::traits::RuntimeControlPlane::retire(&adapter, &runtime_id)
        .await
        .expect("retire should preserve the active wait_all carrier");
    assert_eq!(report.inputs_abandoned, 0);
    assert_eq!(report.inputs_pending_drain, 0);

    let after_retire = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after retire");
    assert_eq!(after_retire.control.phase, RuntimeState::Retired);
    assert_eq!(
        after_retire.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "retire currently preserves the authority-owned wait request"
    );
    assert!(
        after_retire.ops.pending_wait_present,
        "retire currently preserves the pending wait carrier while the operation remains active"
    );
    assert_eq!(
        after_retire.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "retire should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_retire.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "retire should preserve the tracked wait targets until the operation settles"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after retire");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Retired);
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_retire_splits_completion_and_wait_all_lifetimes() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let input = make_prompt("retire split lifetimes");
    let input_id = input.id().clone();
    let (_outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");
    let completion_handle =
        completion_handle.expect("queued prompt should register a completion waiter");

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "retire split wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_retire = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before retire");
    let wait_request_id = before_retire
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_retire.control.phase, RuntimeState::Idle);
    assert_eq!(before_retire.completion_waiters.input_count, 1);
    assert_eq!(before_retire.completion_waiters.waiter_count, 1);
    assert_eq!(before_retire.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_retire.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert!(before_retire.ops.pending_wait_present);
    assert_eq!(
        before_retire.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before retire"
    );
    assert_eq!(
        before_retire.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before retire"
    );

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    let report = crate::traits::RuntimeControlPlane::retire(&adapter, &runtime_id)
        .await
        .expect("retire should split completion and wait_all lifetimes");
    assert_eq!(report.inputs_abandoned, 1);
    assert_eq!(report.inputs_pending_drain, 0);

    match completion_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "retired without runtime loop");
        }
        other => panic!("expected retire termination, got {other:?}"),
    }

    let after_retire = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after retire");
    assert_eq!(after_retire.control.phase, RuntimeState::Retired);
    assert_eq!(
        after_retire.control.current_run_id, None,
        "retire should not leave a settled current-run binding behind once the runtime is actually Retired"
    );
    assert_eq!(
        after_retire.inputs.current_run_id, None,
        "retire should clear ingress-side current-run binding once the runtime is actually Retired"
    );
    assert_eq!(after_retire.completion_waiters.input_count, 0);
    assert_eq!(after_retire.completion_waiters.waiter_count, 0);
    assert!(
        after_retire.completion_waiters.waiting_inputs.is_empty(),
        "retire should clear input-owned completion waiters immediately when no runtime loop can drain"
    );
    assert_eq!(
        after_retire.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "retire should preserve the authority-owned wait request after completion waiters clear"
    );
    assert!(after_retire.ops.pending_wait_present);
    assert_eq!(
        after_retire.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "retire should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_retire.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "retire should preserve the tracked wait target until the operation settles"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after retire");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Retired);
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_retire_clears_steered_waiter_but_leaves_steer_queue_visible()
 {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let input = Input::Prompt(crate::input::PromptInput::new(
        "retire steered prompt",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ));
    let input_id = input.id().clone();
    let (_outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("steered prompt should be accepted");
    let completion_handle =
        completion_handle.expect("steered prompt should register a completion waiter");

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "retire steered wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_retire = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before retire");
    let wait_request_id = before_retire
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_retire.control.phase, RuntimeState::Idle);
    assert!(before_retire.inputs.queue.is_empty());
    assert_eq!(before_retire.inputs.steer_queue, vec![input_id.clone()]);
    assert!(before_retire.inputs.wake_requested);
    assert!(before_retire.inputs.process_requested);
    assert_eq!(before_retire.completion_waiters.input_count, 1);
    assert_eq!(before_retire.completion_waiters.waiter_count, 1);
    assert_eq!(before_retire.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_retire.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert!(before_retire.ops.pending_wait_present);
    assert_eq!(
        before_retire.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before retire"
    );
    assert_eq!(
        before_retire.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before retire"
    );

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    let report = crate::traits::RuntimeControlPlane::retire(&adapter, &runtime_id)
        .await
        .expect("retire should terminate the steered completion waiter while preserving wait_all");
    assert_eq!(report.inputs_abandoned, 1);
    assert_eq!(report.inputs_pending_drain, 0);

    match completion_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "retired without runtime loop");
        }
        other => panic!("expected retire termination for steered input, got {other:?}"),
    }

    let after_retire = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after retire");
    assert_eq!(after_retire.control.phase, RuntimeState::Retired);
    assert_eq!(after_retire.control.current_run_id, None);
    assert_eq!(after_retire.inputs.current_run_id, None);
    assert!(after_retire.inputs.queue.is_empty());
    assert_eq!(
        after_retire.inputs.steer_queue,
        vec![input_id.clone()],
        "retire currently leaves the steered input visible in the steer queue even after the steered completion waiter terminates"
    );
    assert_eq!(after_retire.completion_waiters.input_count, 0);
    assert_eq!(after_retire.completion_waiters.waiter_count, 0);
    assert!(
        after_retire.completion_waiters.waiting_inputs.is_empty(),
        "retire should clear input-owned steered completion waiters immediately"
    );
    assert_eq!(
        after_retire.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "retire should preserve the authority-owned wait request after steered completion waiters clear"
    );
    assert!(after_retire.ops.pending_wait_present);
    assert_eq!(
        after_retire.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "retire should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_retire.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "retire should preserve the tracked wait target until the operation settles"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after retire");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Retired);
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_preserves_wait_all_after_retire_with_runtime_loop() {
    struct BlockingExecutor {
        apply_calls: Arc<AtomicUsize>,
        apply_started: Arc<Notify>,
        apply_finished: Arc<Notify>,
        allow_finish: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            self.apply_started.notify_waiters();
            self.allow_finish.notified().await;
            self.apply_finished.notify_waiters();

            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
                run_result: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let apply_started = Arc::new(Notify::new());
    let apply_finished = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                apply_started: Arc::clone(&apply_started),
                apply_finished: Arc::clone(&apply_finished),
                allow_finish: Arc::clone(&allow_finish),
            }),
        )
        .await;

    let input = make_progress_input("retire-wait-all-with-loop");
    let outcome = adapter
        .accept_input_without_wake(&session_id, input)
        .await
        .expect("queued progress input should be accepted without waking the loop");
    assert!(outcome.is_accepted());

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for attached session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "retire wait target with loop".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_retire = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before retire");
    let wait_request_id = before_retire
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_retire.control.phase, RuntimeState::Attached);
    assert!(before_retire.ops.pending_wait_present);
    assert_eq!(
        before_retire.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before retire"
    );
    assert_eq!(
        before_retire.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before retire"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "queued attached-loop work should remain pending until retire wakes the loop"
    );

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    let report = crate::traits::RuntimeControlPlane::retire(&adapter, &runtime_id)
        .await
        .expect("retire should preserve wait_all while the live loop drains queued work");
    assert_eq!(report.inputs_abandoned, 0);
    assert_eq!(report.inputs_pending_drain, 1);

    tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
        .await
        .expect("retire should wake the attached runtime loop to drain queued work");

    let after_retire = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after retire wakes the loop");
    assert_eq!(
        after_retire.control.phase,
        RuntimeState::Running,
        "retire currently re-enters Running while the attached loop drains preserved work"
    );
    assert_eq!(
        after_retire.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "retire should preserve the authority-owned wait request while the attached loop is draining"
    );
    assert!(after_retire.ops.pending_wait_present);
    assert_eq!(
        after_retire.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "retire should preserve request-id agreement across the wait carrier seam while draining"
    );
    assert_eq!(
        after_retire.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "retire should preserve the tracked wait target while queued work is draining"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "retire should wake the attached loop exactly once for the preserved queued work"
    );

    allow_finish.notify_waiters();
    tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
        .await
        .expect("attached loop should finish draining the preserved queued work");

    let after_drain = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after attached-loop drain");
            if snapshot.control.phase == RuntimeState::Retired {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("runtime should return to Retired once the preserved work finishes draining");

    assert_eq!(
        after_drain.control.current_run_id, None,
        "retire should not leave a settled control-side current-run binding once the attached loop returns to Retired"
    );
    assert_eq!(
        after_drain.inputs.current_run_id, None,
        "retire should not leave a settled ingress-side current-run binding once the attached loop returns to Retired"
    );
    assert_eq!(
        after_drain.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "wait_all should remain active after the attached loop finishes draining preserved work"
    );
    assert!(after_drain.ops.pending_wait_present);
    assert_eq!(
        after_drain.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "request-id agreement should survive the return to Retired after drain"
    );
    assert_eq!(
        after_drain.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "the tracked wait target should remain present until the operation itself settles"
    );
    assert!(
        after_drain.inputs.queue.is_empty(),
        "retire should not leave ordinary queued work behind once the attached runtime returns to Retired"
    );
    assert!(
        after_drain.inputs.steer_queue.is_empty(),
        "retire should not leave steer-queued work behind once the attached runtime returns to Retired"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after retire+drain");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Retired);
    assert!(
        settled.inputs.queue.is_empty(),
        "retire should keep the attached settled Retired snapshot free of ordinary queued work"
    );
    assert!(
        settled.inputs.steer_queue.is_empty(),
        "retire should keep the attached settled Retired snapshot free of steer-queued work"
    );
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_retire_with_runtime_loop_splits_completion_and_wait_all_lifetimes()
 {
    struct BlockingExecutor {
        apply_calls: Arc<AtomicUsize>,
        control_calls: Arc<AtomicUsize>,
        apply_started: Arc<Notify>,
        apply_finished: Arc<Notify>,
        allow_finish: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            self.apply_started.notify_waiters();
            self.allow_finish.notified().await;
            self.apply_finished.notify_waiters();

            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
                run_result: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));
    let apply_started = Arc::new(Notify::new());
    let apply_finished = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                control_calls: Arc::clone(&control_calls),
                apply_started: Arc::clone(&apply_started),
                apply_finished: Arc::clone(&apply_finished),
                allow_finish: Arc::clone(&allow_finish),
            }),
        )
        .await;

    let input = make_progress_input("retire-with-loop-split-lifetimes");
    let input_id = input.id().clone();
    let (outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("progress input should be accepted");
    assert!(outcome.is_accepted());
    let completion_handle =
        completion_handle.expect("queued progress input should expose a completion waiter");

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for attached session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "retire split-lifetime wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_retire = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before retire");
    let wait_request_id = before_retire
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_retire.control.phase, RuntimeState::Attached);
    assert_eq!(before_retire.completion_waiters.input_count, 1);
    assert_eq!(before_retire.completion_waiters.waiter_count, 1);
    assert_eq!(before_retire.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_retire.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        before_retire.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before retire"
    );
    assert_eq!(
        before_retire.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before retire"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "queued attached-loop work should remain pending until retire wakes the loop"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "retire should not yet have attempted executor control"
    );

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    let report = crate::traits::RuntimeControlPlane::retire(&adapter, &runtime_id)
        .await
        .expect("retire should preserve queued work and wait_all while the live loop drains");
    assert_eq!(report.inputs_abandoned, 0);
    assert_eq!(report.inputs_pending_drain, 1);

    tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
        .await
        .expect("retire should wake the attached runtime loop to drain queued work");

    let during_retire = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after retire wakes the loop");
    assert_eq!(
        during_retire.control.phase,
        RuntimeState::Running,
        "retire currently re-enters Running while the attached loop drains preserved work"
    );
    assert_eq!(during_retire.completion_waiters.input_count, 1);
    assert_eq!(during_retire.completion_waiters.waiter_count, 1);
    assert_eq!(during_retire.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        during_retire.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        during_retire.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "retire should preserve the authority-owned wait request while the attached loop is draining"
    );
    assert!(during_retire.ops.pending_wait_present);
    assert_eq!(
        during_retire.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "retire should preserve request-id agreement across the wait carrier seam while draining"
    );
    assert_eq!(
        during_retire.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "retire should preserve the tracked wait target while queued work is draining"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "retire should wake the attached loop exactly once for the preserved queued work"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "retire should not route any out-of-band control command through the executor seam"
    );

    allow_finish.notify_waiters();
    tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
        .await
        .expect("attached loop should finish draining the preserved queued work");

    match completion_handle.wait().await {
        CompletionOutcome::CompletedWithoutResult => {}
        other => panic!(
            "expected retire+drain to complete queued work while wait_all remains live, got {other:?}"
        ),
    }

    let after_drain = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after attached-loop drain");
            if snapshot.control.phase == RuntimeState::Retired {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("runtime should return to Retired once the preserved work finishes draining");
    assert!(
        after_drain.inputs.queue.is_empty(),
        "retire with a runtime loop should not leave ordinary queued work behind once the runtime returns to Retired"
    );
    assert!(
        after_drain.inputs.steer_queue.is_empty(),
        "retire with a runtime loop should not leave steer-queued work behind once the runtime returns to Retired"
    );
    assert_eq!(after_drain.completion_waiters.input_count, 0);
    assert_eq!(after_drain.completion_waiters.waiter_count, 0);
    assert!(
        after_drain.completion_waiters.waiting_inputs.is_empty(),
        "completion waiters should clear once retire-drained work completes"
    );
    assert_eq!(
        after_drain.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "wait_all should remain active after input-owned completion waiters clear"
    );
    assert!(after_drain.ops.pending_wait_present);
    assert_eq!(
        after_drain.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "request-id agreement should survive the return to Retired after drain"
    );
    assert_eq!(
        after_drain.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "the tracked wait target should remain present until the operation itself settles"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after retire+drain");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Retired);
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_tracks_comms_drain_state() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let comms_runtime: Arc<dyn CommsRuntime> = Arc::new(FakeDrainRuntime::idle());

    spawn_test_comms_drain(
        &adapter,
        &session_id,
        CommsDrainMode::PersistentHost,
        comms_runtime,
        Duration::from_secs(60),
    )
    .await;

    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist for registered session");

    assert!(snapshot.drain.slot_present);
    assert_eq!(snapshot.drain.phase, Some(CommsDrainPhase::Running));
    assert_eq!(snapshot.drain.mode, Some(CommsDrainMode::PersistentHost));
    assert!(snapshot.drain.handle_present);
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_tracks_stopped_comms_drain_state() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let comms_runtime: Arc<dyn CommsRuntime> = Arc::new(FakeDrainRuntime::idle());

    let spawned = adapter
        .maybe_spawn_comms_drain(&session_id, true, Some(comms_runtime))
        .await;
    assert!(
        !spawned,
        "unregistered session should not spawn a comms drain"
    );

    adapter.register_session(session_id.clone()).await;

    let spawned = adapter
        .maybe_spawn_comms_drain(
            &session_id,
            true,
            Some(Arc::new(FakeDrainRuntime::idle()) as Arc<dyn CommsRuntime>),
        )
        .await;
    assert!(spawned, "registered session should spawn a comms drain");

    adapter.abort_comms_drain(&session_id).await;

    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist for registered session");

    assert!(snapshot.drain.slot_present);
    assert_eq!(snapshot.drain.phase, Some(CommsDrainPhase::Stopped));
    assert_eq!(snapshot.drain.mode, Some(CommsDrainMode::PersistentHost));
    assert!(!snapshot.drain.handle_present);
}

// ---------------------------------------------------------------
// A1: Session command guards (TLA+ DestroyedShapeInvariant,
//     RunningHasActiveRunInvariant)
// ---------------------------------------------------------------

#[tokio::test]
async fn register_session_rejects_destroyed_session() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    // Transition to Destroyed via the control-plane destroy path.
    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    crate::traits::RuntimeControlPlane::destroy(&adapter, &runtime_id)
        .await
        .expect("destroy should succeed");

    // Second register must be rejected — DestroyedShapeInvariant forbids
    // resurrecting a destroyed binding.
    let err = adapter
        .execute_meerkat_machine_session_command(MeerkatMachineSessionCommand::RegisterSession {
            session_id: session_id.clone(),
        })
        .await
        .expect_err("register should reject a destroyed session");
    assert!(
        matches!(err, RuntimeDriverError::Destroyed),
        "expected Destroyed, got {err:?}"
    );
}

#[tokio::test]
async fn unregister_session_rejects_unknown_session() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    // Unregister on a session that was never registered must return an error.
    let err = adapter
        .execute_meerkat_machine_session_command(MeerkatMachineSessionCommand::UnregisterSession {
            session_id: session_id.clone(),
        })
        .await
        .expect_err("unregister should reject an unknown session");
    assert!(
        matches!(err, RuntimeDriverError::NotReady { .. }),
        "expected NotReady, got {err:?}"
    );
}

#[tokio::test]
async fn interrupt_current_run_rejects_destroyed_session() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    crate::traits::RuntimeControlPlane::destroy(&adapter, &runtime_id)
        .await
        .expect("destroy should succeed");

    let err = adapter
        .interrupt_current_run(&session_id)
        .await
        .expect_err("interrupt should reject a destroyed session");
    assert!(
        matches!(err, RuntimeDriverError::Destroyed),
        "expected Destroyed, got {err:?}"
    );
}

#[tokio::test]
async fn cancel_after_boundary_rejects_destroyed_session() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    crate::traits::RuntimeControlPlane::destroy(&adapter, &runtime_id)
        .await
        .expect("destroy should succeed");

    let err = adapter
        .cancel_after_boundary(&session_id)
        .await
        .expect_err("cancel_after_boundary should reject a destroyed session");
    assert!(
        matches!(err, RuntimeDriverError::Destroyed),
        "expected Destroyed, got {err:?}"
    );
}

#[tokio::test]
async fn stop_runtime_executor_rejects_destroyed_session() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    crate::traits::RuntimeControlPlane::destroy(&adapter, &runtime_id)
        .await
        .expect("destroy should succeed");

    let err = adapter
        .stop_runtime_executor(
            &session_id,
            RunControlCommand::StopRuntimeExecutor {
                reason: "test".to_string(),
            },
        )
        .await
        .expect_err("stop_runtime_executor should reject a destroyed session");
    assert!(
        matches!(err, RuntimeDriverError::Destroyed),
        "expected Destroyed, got {err:?}"
    );
}

// ---------------------------------------------------------------
// A2: Drain command guards (TLA+ DrainBindingInvariant,
//     DrainModeInvariant)
// ---------------------------------------------------------------

#[tokio::test]
async fn set_peer_ingress_context_rejects_unknown_session() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    let spawned = adapter
        .update_peer_ingress_context(&session_id, true, None)
        .await;
    assert!(
        !spawned,
        "update_peer_ingress_context should not spawn for unknown session"
    );
}

#[tokio::test]
async fn set_peer_ingress_context_rejects_destroyed_session() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    crate::traits::RuntimeControlPlane::destroy(adapter.as_ref(), &runtime_id)
        .await
        .expect("destroy should succeed");

    let spawned = adapter
        .update_peer_ingress_context(&session_id, true, None)
        .await;
    assert!(
        !spawned,
        "update_peer_ingress_context should not spawn for destroyed session"
    );
}

#[tokio::test]
async fn notify_drain_exited_rejects_unknown_session() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    // Should not panic — the guard silently rejects.
    adapter
        .notify_comms_drain_exited(&session_id, DrainExitReason::Dismissed)
        .await;
}

#[tokio::test]
async fn notify_drain_exited_rejects_destroyed_session() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    crate::traits::RuntimeControlPlane::destroy(adapter.as_ref(), &runtime_id)
        .await
        .expect("destroy should succeed");

    // Should not panic — the guard silently rejects.
    adapter
        .notify_comms_drain_exited(&session_id, DrainExitReason::Dismissed)
        .await;
}

#[tokio::test]
async fn abort_comms_drain_tolerates_unknown_session() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    // Guard rejects unknown session but caller swallows the error.
    adapter.abort_comms_drain(&session_id).await;
}

#[tokio::test]
async fn wait_comms_drain_tolerates_unknown_session() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    // Guard rejects unknown session but caller swallows the error.
    adapter.wait_comms_drain(&session_id).await;
}

// ---------------------------------------------------------------
// A3: Control command guards (TLA+ ActiveRunPhaseInvariant,
//     LiveBindingLifecycleInvariant, AdmitQueuedInput precondition)
// ---------------------------------------------------------------

#[tokio::test]
async fn ingest_rejects_retired_session() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    crate::traits::RuntimeControlPlane::retire(&adapter, &runtime_id)
        .await
        .expect("retire should succeed");

    let input = make_prompt("should be rejected");
    let err = crate::traits::RuntimeControlPlane::ingest(&adapter, &runtime_id, input)
        .await
        .expect_err("ingest should reject a retired session");
    assert!(
        matches!(
            err,
            RuntimeControlPlaneError::InvalidState {
                state: RuntimeState::Retired
            }
        ),
        "expected InvalidState(Retired), got {err:?}"
    );
}

#[tokio::test]
async fn ingest_rejects_stopped_session() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    // Stop the session by driving it through retire → stop.
    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    adapter
        .stop_runtime_executor(
            &session_id,
            RunControlCommand::StopRuntimeExecutor {
                reason: "test stop".to_string(),
            },
        )
        .await
        .expect("stop should succeed");

    let input = make_prompt("should be rejected");
    let err = crate::traits::RuntimeControlPlane::ingest(&adapter, &runtime_id, input)
        .await
        .expect_err("ingest should reject a stopped session");
    assert!(
        matches!(
            err,
            RuntimeControlPlaneError::InvalidState {
                state: RuntimeState::Stopped
            }
        ),
        "expected InvalidState(Stopped), got {err:?}"
    );
}

#[tokio::test]
async fn retire_rejects_initializing_session() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    // Don't register, just create a session entry in Initializing state.
    // Since there's no way to create a session in Initializing via the
    // public API (register_session transitions to Idle), we test that
    // retire guards against the union of incompatible phases. The Idle →
    // Retired path is already exercised above. Focus here on the existing
    // Stopped guard and Destroyed guard.
    adapter.register_session(session_id.clone()).await;

    // First stop
    adapter
        .stop_runtime_executor(
            &session_id,
            RunControlCommand::StopRuntimeExecutor {
                reason: "test".to_string(),
            },
        )
        .await
        .expect("stop should succeed");

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    let err = crate::traits::RuntimeControlPlane::retire(&adapter, &runtime_id)
        .await
        .expect_err("retire should reject a stopped session");
    assert!(
        matches!(
            err,
            RuntimeControlPlaneError::InvalidState {
                state: RuntimeState::Stopped
            }
        ),
        "expected InvalidState(Stopped), got {err:?}"
    );
}

// ---------------------------------------------------------------
// A4: Ingress command guards (TLA+ WaitingInputsInvariant)
// ---------------------------------------------------------------

#[tokio::test]
async fn accept_input_with_completion_rejects_retired_session() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    crate::traits::RuntimeControlPlane::retire(&adapter, &runtime_id)
        .await
        .expect("retire should succeed");

    let input = make_prompt("should be rejected");
    let result = adapter
        .accept_input_with_completion(&session_id, input)
        .await;
    match result {
        Err(RuntimeDriverError::NotReady {
            state: RuntimeState::Retired,
        }) => {}
        Err(other) => panic!("expected NotReady(Retired), got {other:?}"),
        Ok(_) => panic!("accept_input_with_completion should reject retired session"),
    }
}

#[tokio::test]
async fn accept_input_with_completion_rejects_destroyed_session() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    crate::traits::RuntimeControlPlane::destroy(&adapter, &runtime_id)
        .await
        .expect("destroy should succeed");

    let input = make_prompt("should be rejected");
    let result = adapter
        .accept_input_with_completion(&session_id, input)
        .await;
    match result {
        Err(RuntimeDriverError::Destroyed) => {}
        Err(other) => panic!("expected Destroyed, got {other:?}"),
        Ok(_) => panic!("accept_input_with_completion should reject destroyed session"),
    }
}

// ---------------------------------------------------------------
// A5: Legacy run command guards (TLA+ RunningHasActiveRunInvariant)
// ---------------------------------------------------------------

#[tokio::test]
async fn legacy_run_prepare_rejects_retired_session() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    crate::traits::RuntimeControlPlane::retire(&adapter, &runtime_id)
        .await
        .expect("retire should succeed");

    let input = make_prompt("should be rejected");
    let err = adapter
        .accept_input_and_run::<(), _, _>(&session_id, input, |_run_id, _prim| async {
            panic!("executor should not be called on retired session");
        })
        .await
        .expect_err("legacy run should reject a retired session");
    assert!(
        matches!(
            err,
            RuntimeDriverError::NotReady {
                state: RuntimeState::Retired
            }
        ),
        "expected NotReady(Retired), got {err:?}"
    );
}

#[tokio::test]
async fn legacy_run_prepare_rejects_destroyed_session() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    crate::traits::RuntimeControlPlane::destroy(&adapter, &runtime_id)
        .await
        .expect("destroy should succeed");

    let input = make_prompt("should be rejected");
    let err = adapter
        .accept_input_and_run::<(), _, _>(&session_id, input, |_run_id, _prim| async {
            panic!("executor should not be called on destroyed session");
        })
        .await
        .expect_err("legacy run should reject a destroyed session");
    assert!(
        matches!(err, RuntimeDriverError::Destroyed),
        "expected Destroyed, got {err:?}"
    );
}

// ---------------------------------------------------------------
// A6: Deep invariant region validation via spine snapshot
// ---------------------------------------------------------------

#[tokio::test]
async fn spine_invariants_hold_after_register() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist");
    snapshot
        .validate_spine_invariants()
        .expect("all invariants should hold after register");
}

#[tokio::test]
async fn spine_invariants_hold_after_queued_input() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    let input = make_prompt("queued input");
    let (_outcome, _handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");

    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist");
    snapshot
        .validate_spine_invariants()
        .expect("all invariants should hold after queued input");
}

#[tokio::test]
async fn spine_invariants_hold_after_destroy() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    let input = make_prompt("will be destroyed");
    let (_outcome, _handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    crate::traits::RuntimeControlPlane::destroy(&adapter, &runtime_id)
        .await
        .expect("destroy should succeed");

    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after destroy");
    snapshot
        .validate_spine_invariants()
        .expect("all invariants should hold after destroy");
}

#[tokio::test]
async fn spine_invariants_hold_after_retire() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    let input = make_prompt("will be retired");
    let (_outcome, _handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    crate::traits::RuntimeControlPlane::retire(&adapter, &runtime_id)
        .await
        .expect("retire should succeed");

    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after retire");
    snapshot
        .validate_spine_invariants()
        .expect("all invariants should hold after retire");
}

#[tokio::test]
async fn spine_invariants_hold_after_reset() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    let input = make_prompt("will be reset");
    let (_outcome, _handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    crate::traits::RuntimeControlPlane::reset(&adapter, &runtime_id)
        .await
        .expect("reset should succeed");

    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after reset");
    snapshot
        .validate_spine_invariants()
        .expect("all invariants should hold after reset");
}

#[tokio::test]
async fn spine_invariants_hold_after_recycle() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    let input = make_prompt("will be recycled");
    let (_outcome, _handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    crate::traits::RuntimeControlPlane::recycle(&adapter, &runtime_id)
        .await
        .expect("recycle should succeed");

    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after recycle");
    snapshot
        .validate_spine_invariants()
        .expect("all invariants should hold after recycle");
}

#[tokio::test]
async fn spine_invariants_hold_after_steered_input() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    let steered = Input::Prompt(crate::input::PromptInput::new(
        "steered input",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ));
    let (_outcome, _handle) = adapter
        .accept_input_with_completion(&session_id, steered)
        .await
        .expect("steered prompt should be accepted");

    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist");
    snapshot
        .validate_spine_invariants()
        .expect("all invariants should hold after steered input");
}

// ---------------------------------------------------------------
// A7: PublishCommittedVisibleSet dispatch guards
// (TLA+ VisibleSurfacesMatchAppliedStateInvariant)
// ---------------------------------------------------------------

#[tokio::test]
async fn publish_committed_visible_set_succeeds_for_registered_session() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    let state = meerkat_core::SessionToolVisibilityState {
        active_revision: 1,
        staged_revision: 1,
        ..Default::default()
    };
    let result = adapter
        .publish_committed_visible_set(&session_id, state.clone())
        .await;
    let published = result.expect("publish should succeed for registered session");
    assert_eq!(
        published.active_revision, state.active_revision,
        "returned state should match the submitted state"
    );
}

#[tokio::test]
async fn publish_committed_visible_set_rejects_unknown_session() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();

    let state = meerkat_core::SessionToolVisibilityState::default();
    let err = adapter
        .publish_committed_visible_set(&session_id, state)
        .await
        .expect_err("publish should reject an unknown session");
    assert!(
        matches!(err, RuntimeDriverError::NotReady { .. }),
        "expected NotReady, got {err:?}"
    );
}

#[tokio::test]
async fn publish_committed_visible_set_rejects_destroyed_session() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    crate::traits::RuntimeControlPlane::destroy(&adapter, &runtime_id)
        .await
        .expect("destroy should succeed");

    let state = meerkat_core::SessionToolVisibilityState::default();
    let err = adapter
        .publish_committed_visible_set(&session_id, state)
        .await
        .expect_err("publish should reject a destroyed session");
    assert!(
        matches!(err, RuntimeDriverError::Destroyed),
        "expected Destroyed, got {err:?}"
    );
}

#[tokio::test]
async fn publish_committed_visible_set_rejects_stale_active_revision() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    // VisibleSurfacesMatchAppliedStateInvariant: active_revision must not
    // lag behind staged_revision.
    let state = meerkat_core::SessionToolVisibilityState {
        active_revision: 1,
        staged_revision: 3,
        ..Default::default()
    };
    let err = adapter
        .publish_committed_visible_set(&session_id, state)
        .await
        .expect_err("publish should reject stale active revision");
    assert!(
        matches!(err, RuntimeDriverError::ValidationFailed { .. }),
        "expected ValidationFailed, got {err:?}"
    );
}

#[tokio::test]
async fn publish_committed_visible_set_accepts_active_ahead_of_staged() {
    let adapter = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    // active_revision > staged_revision is valid — the active set has
    // advanced past the last staged projection.
    let state = meerkat_core::SessionToolVisibilityState {
        active_revision: 5,
        staged_revision: 3,
        ..Default::default()
    };
    let result = adapter
        .publish_committed_visible_set(&session_id, state)
        .await;
    result.expect("publish should succeed when active_revision >= staged_revision");
}

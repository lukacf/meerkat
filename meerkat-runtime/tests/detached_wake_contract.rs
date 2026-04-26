#![allow(
    clippy::expect_used,
    clippy::large_futures,
    clippy::panic,
    clippy::unwrap_used
)]
//! Detached-wake contract tests for background shell job notification.
//!
//! These tests verify the current runtime-loop-owned detached-wake paths that
//! inject `Input::Continuation` into a quiescent session when background
//! operations reach terminal state.

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use meerkat_core::RunBoundaryReceipt;
use meerkat_core::lifecycle::core_executor::{CoreApplyOutput, CoreExecutorError};
use meerkat_core::lifecycle::run_control::RunControlCommand;
use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
use meerkat_core::lifecycle::{CoreExecutor, RunId};
use meerkat_core::ops_lifecycle::{
    OperationKind, OperationResult, OperationSpec, OpsLifecycleRegistry,
};
use meerkat_core::types::{RunResult, SessionId, Usage};
use meerkat_runtime::{
    ContinuationInput, InputDurability, InputOrigin, InputVisibility, MeerkatMachine,
    RuntimeOpsLifecycleRegistry,
};
use tokio::sync::Notify;

async fn wait_for_apply_count(apply_count: &AtomicUsize, expected: usize, context: &'static str) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if apply_count.load(Ordering::SeqCst) >= expected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect(context);
}

fn background_spec(name: &str) -> OperationSpec {
    OperationSpec {
        id: meerkat_core::ops_lifecycle::OperationId::new(),
        kind: OperationKind::BackgroundToolOp,
        owner_session_id: SessionId::new(),
        display_name: name.into(),
        source_label: "test-detached-wake".into(),
        child_session_id: None,
        expect_peer_channel: false,
    }
}

fn mob_member_spec(name: &str) -> OperationSpec {
    OperationSpec {
        id: meerkat_core::ops_lifecycle::OperationId::new(),
        kind: OperationKind::MobMemberChild,
        owner_session_id: SessionId::new(),
        display_name: name.into(),
        source_label: "test-detached-wake".into(),
        child_session_id: Some(SessionId::new()),
        expect_peer_channel: true,
    }
}

fn op_result(id: &meerkat_core::ops_lifecycle::OperationId, content: &str) -> OperationResult {
    OperationResult {
        id: id.clone(),
        content: content.into(),
        is_error: false,
        duration_ms: 42,
        tokens_used: 7,
    }
}

/// Simple executor that succeeds immediately and returns a RunResult.
struct ResultExecutor;

#[async_trait::async_trait]
impl CoreExecutor for ResultExecutor {
    async fn apply(
        &mut self,
        run_id: RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
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
            run_result: Some(RunResult {
                text: "done".into(),
                session_id: SessionId::new(),
                usage: Usage::default(),
                turns: 1,
                tool_calls: 0,
                structured_output: None,
                schema_warnings: None,
                skill_diagnostics: None,
            }),
        })
    }
    async fn control(&mut self, _cmd: RunControlCommand) -> Result<(), CoreExecutorError> {
        Ok(())
    }
}

// ─── CHOKE-004-IT: Idle runtime wakes after background op terminal ───

#[tokio::test]
async fn choke_004_feed_backed_idle_runtime_injects_continuation_without_manual_trigger() {
    struct CountingExecutor {
        apply_count: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for CountingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_count.fetch_add(1, Ordering::SeqCst);
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
                run_result: Some(RunResult {
                    text: "done".into(),
                    session_id: SessionId::new(),
                    usage: Usage::default(),
                    turns: 1,
                    tool_calls: 0,
                    structured_output: None,
                    schema_warnings: None,
                    skill_diagnostics: None,
                }),
            })
        }

        async fn control(&mut self, _cmd: RunControlCommand) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let apply_count = Arc::new(AtomicUsize::new(0));
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(CountingExecutor {
                apply_count: Arc::clone(&apply_count),
            }),
        )
        .await;

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("session registry should exist");

    let spec = background_spec("idle-feed");
    let op_id = spec.id.clone();
    registry.register_operation(spec).unwrap();
    registry.provisioning_succeeded(&op_id).unwrap();
    registry
        .complete_operation(&op_id, op_result(&op_id, "done"))
        .unwrap();

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if apply_count.load(Ordering::SeqCst) >= 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("feed-backed idle wake should inject a continuation without manual trigger");

    assert_eq!(
        apply_count.load(Ordering::SeqCst),
        1,
        "background completion should produce exactly one continuation apply on the idle feed path"
    );
}

#[tokio::test]
async fn choke_004_idle_runtime_wakes_on_detached_op_completion() {
    struct CountingExecutor {
        apply_count: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for CountingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_count.fetch_add(1, Ordering::SeqCst);
            let mut executor = ResultExecutor;
            executor.apply(run_id, primitive).await
        }
        async fn control(&mut self, _cmd: RunControlCommand) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let apply_count = Arc::new(AtomicUsize::new(0));
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    // Register session with executor so runtime loop is running
    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(CountingExecutor {
                apply_count: Arc::clone(&apply_count),
            }),
        )
        .await;

    // The runtime loop owns detached wake for registered sessions.

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("session registry should exist");

    let spec = background_spec("wake-on-complete");
    let op_id = spec.id.clone();
    registry.register_operation(spec).unwrap();
    registry.provisioning_succeeded(&op_id).unwrap();
    registry
        .complete_operation(&op_id, op_result(&op_id, "done"))
        .unwrap();

    // Trigger the runtime loop so it reaches the post-drain wake check and can
    // inject the continuation on the feed-backed path.
    use meerkat_runtime::{Input, InputDurability, InputHeader, PromptInput};
    let trigger_input = Input::Prompt(PromptInput {
        header: InputHeader {
            id: meerkat_core::lifecycle::InputId::new(),
            timestamp: chrono::Utc::now(),
            source: InputOrigin::Operator,
            durability: InputDurability::Durable,
            visibility: InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        text: "trigger wake".into(),
        blocks: None,
        turn_metadata: None,
    });

    let (_, handle) = adapter
        .accept_input_with_completion(&session_id, trigger_input)
        .await
        .unwrap();

    // Wait for the turn to complete
    if let Some(handle) = handle {
        let _ = tokio::time::timeout(Duration::from_secs(2), handle.wait()).await;
    }

    wait_for_apply_count(
        &apply_count,
        2,
        "runtime loop should inject a detached-op continuation after the trigger turn",
    )
    .await;
}

// ─── CHOKE-004-IT-B: Five completions produce one coalesced wake ───

#[tokio::test]
async fn choke_004_five_completions_produce_one_coalesced_wake() {
    struct CountingExecutor {
        apply_count: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for CountingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_count.fetch_add(1, Ordering::SeqCst);
            let mut executor = ResultExecutor;
            executor.apply(run_id, primitive).await
        }
        async fn control(&mut self, _cmd: RunControlCommand) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let apply_count = Arc::new(AtomicUsize::new(0));
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(CountingExecutor {
                apply_count: Arc::clone(&apply_count),
            }),
        )
        .await;

    // The runtime loop owns detached wake for registered sessions.

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("session registry should exist");

    let mut op_ids = Vec::new();
    for i in 0..5 {
        let spec = background_spec(&format!("coalesce-{i}"));
        let op_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&op_id).unwrap();
        op_ids.push(op_id);
    }

    // Complete all 5 operations — each sets pending=true, but pending is
    // already true after the first, so only one wake is needed.
    for op_id in &op_ids {
        registry
            .complete_operation(op_id, op_result(op_id, "done"))
            .unwrap();
    }

    // Trigger the runtime loop with a prompt so the idle-wake path fires
    use meerkat_runtime::{Input, InputHeader, PromptInput};
    let trigger_input = Input::Prompt(PromptInput {
        header: InputHeader {
            id: meerkat_core::lifecycle::InputId::new(),
            timestamp: chrono::Utc::now(),
            source: InputOrigin::Operator,
            durability: InputDurability::Durable,
            visibility: InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        text: "trigger".into(),
        blocks: None,
        turn_metadata: None,
    });

    let (_, handle) = adapter
        .accept_input_with_completion(&session_id, trigger_input)
        .await
        .unwrap();

    if let Some(handle) = handle {
        let _ = tokio::time::timeout(Duration::from_secs(2), handle.wait()).await;
    }

    wait_for_apply_count(
        &apply_count,
        2,
        "runtime loop should coalesce five completions into one continuation",
    )
    .await;

    tokio::time::sleep(Duration::from_millis(75)).await;

    // 5 completions -> pending=true (set once, stays true) -> 1 notify -> 1 continuation.
    assert_eq!(
        apply_count.load(Ordering::SeqCst),
        2,
        "five completions should coalesce into one continuation after the trigger prompt"
    );
}

// ─── CHOKE-004-IT-C: Completion during Running defers wake ───

#[tokio::test]
async fn choke_004_completion_during_running_defers_wake() {
    use std::sync::atomic::AtomicUsize;

    struct SlowExecutor {
        apply_count: Arc<AtomicUsize>,
        first_apply_started: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for SlowExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            let call_index = self.apply_count.fetch_add(1, Ordering::SeqCst) + 1;
            // First call: sleep long enough for the op to complete during running
            if call_index == 1 {
                self.first_apply_started.notify_one();
                tokio::time::sleep(Duration::from_millis(300)).await;
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
                run_result: Some(RunResult {
                    text: "done".into(),
                    session_id: SessionId::new(),
                    usage: Usage::default(),
                    turns: 1,
                    tool_calls: 0,
                    structured_output: None,
                    schema_warnings: None,
                    skill_diagnostics: None,
                }),
            })
        }
        async fn control(&mut self, _cmd: RunControlCommand) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let apply_count = Arc::new(AtomicUsize::new(0));
    let first_apply_started = Arc::new(Notify::new());
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(SlowExecutor {
                apply_count: apply_count.clone(),
                first_apply_started: Arc::clone(&first_apply_started),
            }),
        )
        .await;

    // Waker task is spawned automatically during register_session_with_executor.

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("session registry should exist");

    let spec = background_spec("deferred-wake");
    let op_id = spec.id.clone();
    registry.register_operation(spec).unwrap();
    registry.provisioning_succeeded(&op_id).unwrap();

    // Start a turn (session becomes Running)
    use meerkat_runtime::{Input, InputHeader, PromptInput};
    let trigger = Input::Prompt(PromptInput {
        header: InputHeader {
            id: meerkat_core::lifecycle::InputId::new(),
            timestamp: chrono::Utc::now(),
            source: InputOrigin::Operator,
            durability: InputDurability::Durable,
            visibility: InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        text: "start turn".into(),
        blocks: None,
        turn_metadata: None,
    });

    let (_, handle) = adapter
        .accept_input_with_completion(&session_id, trigger)
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(1), first_apply_started.notified())
        .await
        .expect("initial prompt apply should start");

    // Complete the op while the session is running (during the 300ms sleep)
    registry
        .complete_operation(&op_id, op_result(&op_id, "done-while-running"))
        .unwrap();

    // The completion-feed entry is visible at this point, but the runtime loop
    // hasn't reached the idle-wake path yet because the executor is still
    // sleeping and the session is not quiescent.

    // Wait for the first turn to complete
    if let Some(handle) = handle {
        let _ = tokio::time::timeout(Duration::from_secs(3), handle.wait()).await;
    }

    wait_for_apply_count(
        &apply_count,
        2,
        "runtime loop should inject deferred continuation after the prompt quiesces",
    )
    .await;

    // The executor should have been called at least twice:
    // 1. The initial prompt
    // 2. The deferred continuation (injected after quiescence)
    let calls = apply_count.load(Ordering::SeqCst);
    assert!(
        calls >= 2,
        "expected at least 2 executor calls (prompt + deferred continuation), got {calls}"
    );
}

// ─── CHOKE-004-IT-D: MobMemberChild completion does NOT trigger idle wake ───
//
// MobMemberChild completions already wake the session through comms-based
// terminal response injection. The CompletionFeed-based idle wake must filter
// them out to avoid duplicate continuation injections.

#[tokio::test]
async fn choke_004_mob_member_child_completion_does_not_trigger_idle_wake() {
    use std::sync::atomic::AtomicUsize;

    struct CountingExecutor {
        apply_count: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for CountingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_count.fetch_add(1, Ordering::SeqCst);
            let run_result = RunResult {
                text: "done".into(),
                session_id: SessionId::new(),
                usage: Usage::default(),
                turns: 1,
                tool_calls: 0,
                structured_output: None,
                schema_warnings: None,
                skill_diagnostics: None,
            };
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
                terminal: Some(
                    meerkat_core::lifecycle::core_executor::CoreApplyTerminal::RunResult(
                        run_result.clone(),
                    ),
                ),
                run_result: Some(run_result),
            })
        }
        async fn control(&mut self, _cmd: RunControlCommand) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let apply_count = Arc::new(AtomicUsize::new(0));
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(CountingExecutor {
                apply_count: apply_count.clone(),
            }),
        )
        .await;

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("session registry should exist");

    // Register and complete a MobMemberChild operation
    let spec = mob_member_spec("delegate-no-idle-wake");
    let op_id = spec.id.clone();
    registry.register_operation(spec).unwrap();
    registry.provisioning_succeeded(&op_id).unwrap();
    registry
        .complete_operation(&op_id, op_result(&op_id, "delegate done"))
        .unwrap();

    // Trigger a prompt to flush any queued continuations
    use meerkat_runtime::{Input, InputHeader, PromptInput};
    let trigger = Input::Prompt(PromptInput {
        header: InputHeader {
            id: meerkat_core::lifecycle::InputId::new(),
            timestamp: chrono::Utc::now(),
            source: InputOrigin::Operator,
            durability: InputDurability::Durable,
            visibility: InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        text: "flush".into(),
        blocks: None,
        turn_metadata: None,
    });

    let (_, handle) = adapter
        .accept_input_with_completion(&session_id, trigger)
        .await
        .unwrap();
    if let Some(handle) = handle {
        let _ = tokio::time::timeout(Duration::from_secs(2), handle.wait()).await;
    }
    tokio::time::sleep(Duration::from_millis(75)).await;

    // The executor should have been called exactly once (the flush prompt).
    // If MobMemberChild completion triggered an idle wake, we'd see 2+ calls
    // (the continuation + the flush).
    let calls = apply_count.load(Ordering::SeqCst);
    assert_eq!(
        calls, 1,
        "MobMemberChild completion should NOT trigger idle wake continuation; \
         expected 1 executor call (flush only), got {calls}"
    );
}

// ─── UNIT-003: ContinuationInput helper builds correct shape ───

#[test]
fn unit_003_continuation_helper_builds_derived_invisible_steer() {
    let continuation = ContinuationInput::detached_background_op_completed();

    assert_eq!(continuation.header.durability, InputDurability::Derived);
    assert!(!continuation.header.visibility.transcript_eligible);
    assert!(!continuation.header.visibility.operator_eligible);
    assert!(matches!(continuation.header.source, InputOrigin::System));
    assert_eq!(
        continuation.handling_mode,
        meerkat_core::types::HandlingMode::Steer
    );
    assert_eq!(continuation.reason, "detached_background_op_completed");
    assert!(continuation.request_id.is_none());
}

// ─── UNIT-004: Completion feed carries kind through so idle-wake can
//              filter BackgroundToolOp vs MobMemberChild ───

#[test]
fn unit_004_completion_feed_carries_operation_kind() {
    let registry = RuntimeOpsLifecycleRegistry::new();
    let feed = registry.completion_feed_handle();
    let baseline = feed.watermark();

    // Register and complete a BackgroundToolOp.
    let bg_spec = background_spec("bg-arms");
    let bg_id = bg_spec.id.clone();
    registry.register_operation(bg_spec).unwrap();
    registry.provisioning_succeeded(&bg_id).unwrap();
    registry
        .complete_operation(&bg_id, op_result(&bg_id, "bg done"))
        .unwrap();

    // Register and complete a MobMemberChild.
    let mob_spec = mob_member_spec("mob-no-arm");
    let mob_id = mob_spec.id.clone();
    registry.register_operation(mob_spec).unwrap();
    registry.provisioning_succeeded(&mob_id).unwrap();
    registry
        .complete_operation(&mob_id, op_result(&mob_id, "mob done"))
        .unwrap();

    let batch = feed.list_since(baseline);
    let kinds: Vec<_> = batch.entries.iter().map(|e| e.kind).collect();
    assert!(
        kinds.contains(&OperationKind::BackgroundToolOp),
        "idle-wake filter depends on BackgroundToolOp entries appearing in the feed"
    );
    assert!(
        kinds.contains(&OperationKind::MobMemberChild),
        "idle-wake filter depends on MobMemberChild entries appearing so it can skip them"
    );
}

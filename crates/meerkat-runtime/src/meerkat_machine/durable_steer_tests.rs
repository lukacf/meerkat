//! Durable in-turn Steer delivery through the runtime ingress, the generated
//! MeerkatMachine join/resolve transitions, and the runtime-loop terminal.
//!
//! The executor below drives the REAL core boundary coordinator through its
//! test-support runner seam, so the witness facts the runtime resolves at the
//! run terminal are produced exactly as the agent loop produces them.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use meerkat_core::handles::TurnStateHandle;
use meerkat_core::lifecycle::core_executor::{
    CoreApplyOutput, CoreExecutor, CoreExecutorBoundaryHandle, CoreExecutorError,
};
use meerkat_core::lifecycle::run_primitive::RunPrimitive;
use meerkat_core::lifecycle::run_receipt::RunBoundaryReceiptDraft;
use meerkat_core::lifecycle::{
    ConversationAppend, ConversationAppendRole, CoreRenderable, InputId, RunId,
};
use tokio::sync::{Mutex as AsyncMutex, Notify, mpsc};

use crate::input_state::InputLifecycleState;

/// What the simulated runner does next inside `apply`.
#[derive(Debug, Clone, Copy)]
enum RunnerStep {
    /// Consume the exact boundary (parking for every registered preparation)
    /// and reopen the next one, as when the model returns tool calls.
    BoundaryThenToolCalls,
    /// Consume the exact boundary and leave the window closed, as while the
    /// model streams its next response.
    BoundaryThenStream,
    /// The model returned tool calls again: open the next boundary.
    OpenNextBoundary,
    /// An extraction or noncommitting boundary: consume request-only context
    /// and refuse durable deliveries.
    ExtractionBoundary,
    /// Finish the run with a committed boundary.
    Finish,
    /// Fail the run; the (ephemeral) session keeps the failed run's image.
    FailKeepingImage,
    /// Fail the run after the owning service reports the image discarded, as
    /// the persistent service does before returning a non-committing error.
    FailDiscardingImage,
    /// Cancel the run.
    Cancel,
}

struct RunnerScript {
    steps: AsyncMutex<mpsc::UnboundedReceiver<RunnerStep>>,
    sender: mpsc::UnboundedSender<RunnerStep>,
    apply_started: Notify,
    apply_calls: AtomicUsize,
    steps_done: AtomicUsize,
    primitives: std::sync::Mutex<Vec<Vec<InputId>>>,
    applied_durable: std::sync::Mutex<Vec<InputId>>,
    /// While armed, the ingress side of a boundary preparation is held after
    /// the core coordinator resolved it, so the runtime loop can win the
    /// mutation gate first (the "run advanced during preparation" ordering).
    prepare_hold_armed: AtomicBool,
    prepare_hold_released: AtomicBool,
    prepare_hold_reached: AtomicBool,
}

impl RunnerScript {
    fn new() -> Arc<Self> {
        let (sender, steps) = mpsc::unbounded_channel();
        Arc::new(Self {
            steps: AsyncMutex::new(steps),
            sender,
            apply_started: Notify::new(),
            apply_calls: AtomicUsize::new(0),
            steps_done: AtomicUsize::new(0),
            primitives: std::sync::Mutex::new(Vec::new()),
            applied_durable: std::sync::Mutex::new(Vec::new()),
            prepare_hold_armed: AtomicBool::new(false),
            prepare_hold_released: AtomicBool::new(false),
            prepare_hold_reached: AtomicBool::new(false),
        })
    }

    fn step(&self, step: RunnerStep) {
        self.sender
            .send(step)
            .expect("runner script receiver alive");
    }

    /// Send a step that must be fully processed before the test continues
    /// (a boundary consumed with nothing registered, or a window opened).
    async fn step_and_wait(&self, step: RunnerStep) {
        let before = self.steps_done.load(Ordering::SeqCst);
        self.step(step);
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.steps_done.load(Ordering::SeqCst) <= before {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("runner step processed");
    }

    fn primitives(&self) -> Vec<Vec<InputId>> {
        self.primitives.lock().unwrap().clone()
    }

    fn applied_durable(&self) -> Vec<InputId> {
        self.applied_durable.lock().unwrap().clone()
    }
}

struct DurableSteerBoundaryHandle {
    state: meerkat_core::TransientTurnContextStateHandle,
    script: Arc<RunnerScript>,
}

#[async_trait::async_trait]
impl CoreExecutorBoundaryHandle for DurableSteerBoundaryHandle {
    async fn cancel_after_boundary(
        &self,
        _expected_run_id: &RunId,
        _reason: String,
    ) -> Result<(), CoreExecutorError> {
        Ok(())
    }

    async fn prepare_turn_boundary_delivery(
        &self,
        expected_run_id: &RunId,
        delivery: meerkat_core::TurnBoundaryDelivery,
    ) -> Result<
        meerkat_core::lifecycle::CoreBoundaryStageOutput,
        meerkat_core::lifecycle::CoreBoundaryStageError,
    > {
        let prepared = self
            .state
            .prepare_active_turn_boundary(expected_run_id, delivery)
            .await
            .map(|prepared| prepared.into_stage_output(None));
        if self.script.prepare_hold_armed.load(Ordering::SeqCst) {
            self.script
                .prepare_hold_reached
                .store(true, Ordering::SeqCst);
            while !self.script.prepare_hold_released.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        }
        prepared
    }
}

struct DurableSteerExecutor {
    turn_state: Arc<dyn TurnStateHandle>,
    state: meerkat_core::TransientTurnContextStateHandle,
    script: Arc<RunnerScript>,
}

#[async_trait::async_trait]
impl CoreExecutor for DurableSteerExecutor {
    fn boundary_handle(&self) -> Option<Arc<dyn CoreExecutorBoundaryHandle>> {
        Some(Arc::new(DurableSteerBoundaryHandle {
            state: self.state.clone(),
            script: Arc::clone(&self.script),
        }))
    }

    async fn apply(
        &mut self,
        run_id: RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        self.script.apply_calls.fetch_add(1, Ordering::SeqCst);
        let contributors = primitive.contributing_input_ids().to_vec();
        self.script
            .primitives
            .lock()
            .unwrap()
            .push(contributors.clone());
        let run_guard = self
            .state
            .begin_boundary_run_for_test(run_id.clone())
            .map_err(|error| CoreExecutorError::Internal(error.to_string()))?;
        self.turn_state
            .primitive_applied(run_id.clone())
            .map_err(|error| CoreExecutorError::Internal(error.to_string()))?;
        self.script.apply_started.notify_one();
        let mut steps = self.script.steps.lock().await;
        let outcome = loop {
            let Some(step) = steps.recv().await else {
                break Err(CoreExecutorError::Internal("runner script closed".into()));
            };
            match step {
                RunnerStep::BoundaryThenToolCalls
                | RunnerStep::BoundaryThenStream
                | RunnerStep::ExtractionBoundary => {
                    let accept_durable = !matches!(step, RunnerStep::ExtractionBoundary);
                    let taken = self
                        .state
                        .take_boundary_for_test(&run_id, accept_durable)
                        .await
                        .map_err(|error| CoreExecutorError::Internal(error.to_string()))?;
                    if let Some(appends) = taken.applied_durable {
                        self.script
                            .applied_durable
                            .lock()
                            .unwrap()
                            .push(appends.input_id().clone());
                    }
                    if matches!(step, RunnerStep::BoundaryThenToolCalls) {
                        self.state
                            .open_next_boundary_for_test(&run_id)
                            .map_err(|error| CoreExecutorError::Internal(error.to_string()))?;
                    }
                }
                RunnerStep::OpenNextBoundary => {
                    self.state
                        .open_next_boundary_for_test(&run_id)
                        .map_err(|error| CoreExecutorError::Internal(error.to_string()))?;
                }
                RunnerStep::Finish
                | RunnerStep::FailKeepingImage
                | RunnerStep::FailDiscardingImage
                | RunnerStep::Cancel => {}
            }
            if !matches!(
                step,
                RunnerStep::Finish
                    | RunnerStep::FailKeepingImage
                    | RunnerStep::FailDiscardingImage
                    | RunnerStep::Cancel
            ) {
                self.script.steps_done.fetch_add(1, Ordering::SeqCst);
                continue;
            }
            match step {
                RunnerStep::Finish => {
                    break Ok(CoreApplyOutput::with_untyped_snapshot(
                        RunBoundaryReceiptDraft {
                            run_id: run_id.clone(),
                            boundary: primitive.apply_boundary(),
                            contributing_input_ids: contributors.clone(),
                            conversation_digest: None,
                            message_count: 0,
                        },
                        None,
                        None,
                    ));
                }
                RunnerStep::FailKeepingImage => {
                    break Err(CoreExecutorError::apply_failed_runtime_turn(
                        "scripted turn failure",
                    ));
                }
                RunnerStep::FailDiscardingImage => {
                    self.state.discard_uncommitted_durable_deliveries(&run_id);
                    break Err(CoreExecutorError::apply_failed_runtime_turn(
                        "scripted turn failure after an uncommitted image",
                    ));
                }
                RunnerStep::Cancel => break Err(CoreExecutorError::Cancelled),
                RunnerStep::BoundaryThenToolCalls
                | RunnerStep::BoundaryThenStream
                | RunnerStep::OpenNextBoundary
                | RunnerStep::ExtractionBoundary => {
                    unreachable!("non-terminal runner steps continue above")
                }
            }
        };
        drop(steps);
        // The agent loop's run guard closes the run before `apply` returns.
        drop(run_guard);
        outcome
    }

    async fn cancel_after_boundary(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }

    async fn stop_runtime_executor(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }
}

struct DurableSteerRig {
    adapter: Arc<MeerkatMachine>,
    session_id: SessionId,
    state: meerkat_core::TransientTurnContextStateHandle,
    script: Arc<RunnerScript>,
}

impl DurableSteerRig {
    async fn ephemeral() -> Self {
        Self::with_adapter(Arc::new(MeerkatMachine::ephemeral())).await
    }

    async fn persistent(store: Arc<dyn crate::store::RuntimeStore>) -> Self {
        Self::with_adapter(Arc::new(MeerkatMachine::persistent_without_blobs(store))).await
    }

    async fn with_adapter(adapter: Arc<MeerkatMachine>) -> Self {
        Self::with_adapter_for_session(adapter, SessionId::new()).await
    }

    async fn with_adapter_for_session(adapter: Arc<MeerkatMachine>, session_id: SessionId) -> Self {
        let bindings = adapter
            .prepare_bindings(session_id.clone())
            .await
            .expect("prepare generated runtime bindings");
        let state = meerkat_core::TransientTurnContextStateHandle::new();
        let script = RunnerScript::new();
        adapter
            .ensure_session_with_executor(
                session_id.clone(),
                Box::new(DurableSteerExecutor {
                    turn_state: Arc::clone(bindings.turn_state()),
                    state: state.clone(),
                    script: Arc::clone(&script),
                }),
            )
            .await
            .expect("install durable steer executor");
        Self {
            adapter,
            session_id,
            state,
            script,
        }
    }

    /// Start a batch turn and wait until the simulated runner is inside it.
    async fn start_busy_turn(&self) -> InputId {
        let prompt = Input::Prompt(crate::input::PromptInput::new(
            "batch prompt keeps the runtime busy",
            None,
        ));
        let input_id = prompt.id().clone();
        let (outcome, _completion) = self
            .adapter
            .accept_input_with_completion(&self.session_id, prompt)
            .await
            .expect("batch prompt accepted");
        assert!(outcome.is_accepted());
        tokio::time::timeout(Duration::from_secs(5), self.script.apply_started.notified())
            .await
            .expect("batch apply starts");
        input_id
    }

    async fn admit(&self, input: Input) -> Option<crate::completion::CompletionHandle> {
        let (outcome, completion) = self
            .adapter
            .accept_input_with_completion(&self.session_id, input)
            .await
            .expect("steer input accepted");
        assert!(outcome.is_accepted());
        completion
    }

    async fn wait_for_waiting_delivery(&self) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !self.state.has_waiting_delivery_for_test() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("a live-boundary preparation registers");
    }

    async fn phase(&self, input_id: &InputId) -> Option<InputLifecycleState> {
        self.stored(input_id).await.map(|stored| stored.seed.phase)
    }

    async fn stored(&self, input_id: &InputId) -> Option<crate::input_state::StoredInputState> {
        crate::service_ext::SessionServiceRuntimeExt::input_state(
            self.adapter.as_ref(),
            &self.session_id,
            input_id,
        )
        .await
        .expect("input state query")
    }

    async fn wait_for_phase(&self, input_id: &InputId, expected: InputLifecycleState) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if self.phase(input_id).await == Some(expected) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!("input {input_id} did not reach {expected:?}");
        });
    }

    async fn wait_for_apply_calls(&self, expected: usize) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.script.apply_calls.load(Ordering::SeqCst) < expected {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("expected {expected} apply calls"));
    }

    async fn steer_queue(&self) -> Vec<InputId> {
        self.adapter
            .meerkat_machine_spine_snapshot(&self.session_id)
            .await
            .expect("spine snapshot")
            .inputs
            .steer_queue
    }

    async fn queue(&self) -> Vec<InputId> {
        self.adapter
            .meerkat_machine_spine_snapshot(&self.session_id)
            .await
            .expect("spine snapshot")
            .inputs
            .queue
    }
}

fn notice_append(detail: &str, role: ConversationAppendRole) -> ConversationAppend {
    ConversationAppend {
        role,
        content: match role {
            ConversationAppendRole::SystemNotice => CoreRenderable::SystemNotice {
                kind: meerkat_core::types::SystemNoticeKind::Generic,
                body: Some(detail.to_string()),
                blocks: Vec::new(),
            },
            _ => CoreRenderable::Text {
                text: detail.to_string(),
            },
        },
        identity: None,
    }
}

/// A Steer prompt with no text and one typed conversation append, the shape a
/// detached background-job completion takes.
fn typed_steer(detail: &str, role: ConversationAppendRole) -> Input {
    let mut prompt = crate::input::PromptInput::new(
        "",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    );
    prompt.typed_turn_appends = vec![notice_append(detail, role)];
    Input::Prompt(prompt)
}

fn peer_steer(body: &str) -> Input {
    Input::Peer(crate::input::PeerInput {
        directed_interaction_id: None,
        objective_id: None,
        system_prompts: Vec::new(),
        injected_context: Vec::new(),
        sender_taint: None,
        header: crate::input::InputHeader {
            id: InputId::new(),
            timestamp: chrono::Utc::now(),
            source: crate::input::InputOrigin::Peer {
                peer_id: "durable-steer-peer".into(),
                display_identity: None,
                runtime_id: None,
            },
            durability: crate::input::InputDurability::Durable,
            visibility: crate::input::InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        convention: Some(crate::input::PeerConvention::Message),
        content: body.into(),
        payload: None,
        handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
    })
}

fn contributions(primitives: &[Vec<InputId>], input_id: &InputId) -> usize {
    primitives
        .iter()
        .filter(|contributors| contributors.contains(input_id))
        .count()
}

#[tokio::test]
async fn admission_classifies_live_boundary_delivery_from_typed_structure() {
    use crate::ingress_types::{LiveBoundaryDeliveryClass, RuntimeInputSemantics};
    let class = |input: &Input| {
        RuntimeInputSemantics::try_from_generated_admission(input, false)
            .expect("generated admission")
            .live_boundary_delivery()
    };
    assert_eq!(
        class(&typed_steer("notice", ConversationAppendRole::SystemNotice)),
        Some(LiveBoundaryDeliveryClass::DurableAppend)
    );
    assert_eq!(
        class(&typed_steer("user row", ConversationAppendRole::User)),
        Some(LiveBoundaryDeliveryClass::DurableAppend)
    );
    assert_eq!(
        class(&typed_steer("system row", ConversationAppendRole::System)),
        Some(LiveBoundaryDeliveryClass::FollowUpOnly),
        "a System-role append is never delivered in-turn"
    );
    let text_steer = Input::Prompt(crate::input::PromptInput::new(
        "text only",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ));
    assert_eq!(
        class(&text_steer),
        Some(LiveBoundaryDeliveryClass::RequestOnly)
    );
    assert_eq!(
        class(&peer_steer("peer")),
        Some(LiveBoundaryDeliveryClass::RequestOnly)
    );
    let queued = Input::Prompt(crate::input::PromptInput::new("queued", None));
    assert_eq!(class(&queued), None, "queue-lane admissions carry no class");
}

#[tokio::test]
async fn durable_steer_joins_the_running_run_and_is_consumed_with_its_commit() {
    let rig = DurableSteerRig::ephemeral().await;
    let batch = rig.start_busy_turn().await;
    let steer = typed_steer("BG-DONE", ConversationAppendRole::SystemNotice);
    let steer_id = steer.id().clone();
    let completion = rig.admit(steer).await.expect("completion handle");
    rig.wait_for_waiting_delivery().await;

    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    rig.wait_for_phase(&steer_id, InputLifecycleState::Staged)
        .await;
    let joined = rig.stored(&steer_id).await.expect("joined input");
    assert!(
        joined.seed.last_run_id.is_some(),
        "joined to the running run"
    );
    assert!(!rig.steer_queue().await.contains(&steer_id));
    assert!(!rig.queue().await.contains(&steer_id));
    assert_eq!(rig.script.applied_durable(), vec![steer_id.clone()]);

    // The input is consumed only with the run terminal.
    let mut waiter = Box::pin(completion.wait());
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut waiter)
            .await
            .is_err(),
        "a joined durable steer resolves with the run, not at injection"
    );

    rig.script.step(RunnerStep::Finish);
    let outcome = tokio::time::timeout(Duration::from_secs(5), waiter)
        .await
        .expect("durable steer completion resolves with the run")
        .expect("completion outcome");
    assert!(
        !matches!(
            outcome,
            crate::completion::CompletionOutcome::RuntimeTerminated { .. }
        ),
        "{outcome:?}"
    );
    rig.wait_for_phase(&steer_id, InputLifecycleState::Consumed)
        .await;
    rig.wait_for_phase(&batch, InputLifecycleState::Consumed)
        .await;
    let consumed = rig.stored(&steer_id).await.expect("consumed input");
    assert_eq!(consumed.seed.last_run_id, joined.seed.last_run_id);
    // One run: no follow-up turn carries the steer.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(rig.script.apply_calls.load(Ordering::SeqCst), 1);
    assert_eq!(contributions(&rig.script.primitives(), &steer_id), 0);
}

#[tokio::test]
async fn durable_steer_with_no_later_boundary_runs_as_exactly_one_follow_up() {
    let rig = DurableSteerRig::ephemeral().await;
    rig.start_busy_turn().await;
    // The model is streaming: the boundary was consumed and stays closed.
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenStream)
        .await;
    let steer = typed_steer("late", ConversationAppendRole::SystemNotice);
    let steer_id = steer.id().clone();
    rig.admit(steer).await;
    rig.wait_for_waiting_delivery().await;
    assert_eq!(
        rig.phase(&steer_id).await,
        Some(InputLifecycleState::Queued)
    );

    // The model returns final text: the run ends without another boundary.
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_apply_calls(2).await;
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_phase(&steer_id, InputLifecycleState::Consumed)
        .await;
    assert!(rig.script.applied_durable().is_empty());
    assert_eq!(
        contributions(&rig.script.primitives(), &steer_id),
        1,
        "exactly one follow-up turn carries the durable append"
    );
}

#[tokio::test]
async fn durable_steer_waits_across_a_closed_window_and_lands_at_the_next_boundary() {
    let rig = DurableSteerRig::ephemeral().await;
    rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenStream)
        .await;
    let steer = typed_steer("waits", ConversationAppendRole::SystemNotice);
    let steer_id = steer.id().clone();
    rig.admit(steer).await;
    rig.wait_for_waiting_delivery().await;
    // The model returned tool calls: the next boundary opens and the waiting
    // durable delivery attaches to it.
    rig.script.step(RunnerStep::OpenNextBoundary);
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    rig.wait_for_phase(&steer_id, InputLifecycleState::Staged)
        .await;
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_phase(&steer_id, InputLifecycleState::Consumed)
        .await;
    assert_eq!(rig.script.applied_durable(), vec![steer_id.clone()]);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(rig.script.apply_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn durable_steer_applied_then_run_cancelled_is_consumed_when_the_image_is_kept() {
    applied_then_run_cancelled_is_consumed_when_the_image_is_kept(
        DurableSteerRig::ephemeral().await,
    )
    .await;
}

#[tokio::test]
async fn persistent_durable_steer_applied_then_run_cancelled_is_consumed_when_the_image_is_kept() {
    let store: Arc<dyn crate::store::RuntimeStore> =
        Arc::new(crate::store::InMemoryRuntimeStore::new());
    let rig = DurableSteerRig::persistent(Arc::clone(&store)).await;
    let runtime_id = MeerkatMachine::logical_runtime_id(&rig.session_id);
    let steer_id = applied_then_run_cancelled_is_consumed_when_the_image_is_kept(rig).await;
    let row = store
        .load_input_state(&runtime_id, &steer_id)
        .await
        .expect("load consumed row")
        .expect("consumed row persisted");
    assert_eq!(row.seed.phase, InputLifecycleState::Consumed);
}

async fn applied_then_run_cancelled_is_consumed_when_the_image_is_kept(
    rig: DurableSteerRig,
) -> InputId {
    let batch = rig.start_busy_turn().await;
    let steer = typed_steer("kept", ConversationAppendRole::SystemNotice);
    let steer_id = steer.id().clone();
    rig.admit(steer).await;
    rig.wait_for_waiting_delivery().await;
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    rig.wait_for_phase(&steer_id, InputLifecycleState::Staged)
        .await;

    rig.script.step(RunnerStep::Cancel);
    rig.wait_for_phase(&steer_id, InputLifecycleState::Consumed)
        .await;
    rig.wait_for_phase(&batch, InputLifecycleState::Abandoned)
        .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        contributions(&rig.script.primitives(), &steer_id),
        0,
        "a retained append is never delivered again"
    );
    steer_id
}

#[tokio::test]
async fn durable_steer_applied_then_run_failed_with_a_kept_image_is_consumed_not_replayed() {
    applied_then_run_failed_with_a_kept_image_is_consumed_not_replayed(
        DurableSteerRig::ephemeral().await,
    )
    .await;
}

#[tokio::test]
async fn persistent_durable_steer_applied_then_run_failed_with_a_kept_image_is_consumed_not_replayed()
 {
    let store: Arc<dyn crate::store::RuntimeStore> =
        Arc::new(crate::store::InMemoryRuntimeStore::new());
    let rig = DurableSteerRig::persistent(Arc::clone(&store)).await;
    let runtime_id = MeerkatMachine::logical_runtime_id(&rig.session_id);
    let steer_id = applied_then_run_failed_with_a_kept_image_is_consumed_not_replayed(rig).await;
    let row = store
        .load_input_state(&runtime_id, &steer_id)
        .await
        .expect("load consumed row")
        .expect("consumed row persisted");
    assert_eq!(row.seed.phase, InputLifecycleState::Consumed);
}

async fn applied_then_run_failed_with_a_kept_image_is_consumed_not_replayed(
    rig: DurableSteerRig,
) -> InputId {
    let batch = rig.start_busy_turn().await;
    let steer = typed_steer("kept-on-failure", ConversationAppendRole::SystemNotice);
    let steer_id = steer.id().clone();
    rig.admit(steer).await;
    rig.wait_for_waiting_delivery().await;
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    rig.wait_for_phase(&steer_id, InputLifecycleState::Staged)
        .await;

    rig.script.step(RunnerStep::FailKeepingImage);
    rig.wait_for_phase(&steer_id, InputLifecycleState::Consumed)
        .await;
    // The batch is replayed; the late durable input is not.
    rig.wait_for_apply_calls(2).await;
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_phase(&batch, InputLifecycleState::Consumed)
        .await;
    assert_eq!(contributions(&rig.script.primitives(), &batch), 2);
    assert_eq!(contributions(&rig.script.primitives(), &steer_id), 0);
    steer_id
}

#[tokio::test]
async fn durable_steer_applied_then_image_discarded_is_redelivered_exactly_once() {
    applied_then_image_discarded_is_redelivered_exactly_once(DurableSteerRig::ephemeral().await)
        .await;
}

#[tokio::test]
async fn persistent_durable_steer_applied_then_image_discarded_is_redelivered_exactly_once() {
    let store: Arc<dyn crate::store::RuntimeStore> =
        Arc::new(crate::store::InMemoryRuntimeStore::new());
    let rig = DurableSteerRig::persistent(Arc::clone(&store)).await;
    let runtime_id = MeerkatMachine::logical_runtime_id(&rig.session_id);
    let steer_id = applied_then_image_discarded_is_redelivered_exactly_once(rig).await;
    let row = store
        .load_input_state(&runtime_id, &steer_id)
        .await
        .expect("load consumed row")
        .expect("consumed row persisted");
    assert_eq!(row.seed.phase, InputLifecycleState::Consumed);
}

async fn applied_then_image_discarded_is_redelivered_exactly_once(rig: DurableSteerRig) -> InputId {
    let batch = rig.start_busy_turn().await;
    let steer = typed_steer("discarded", ConversationAppendRole::SystemNotice);
    let steer_id = steer.id().clone();
    rig.admit(steer).await;
    rig.wait_for_waiting_delivery().await;
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    rig.wait_for_phase(&steer_id, InputLifecycleState::Staged)
        .await;

    rig.script.step(RunnerStep::FailDiscardingImage);
    rig.wait_for_apply_calls(2).await;
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_apply_calls(3).await;
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_phase(&steer_id, InputLifecycleState::Consumed)
        .await;
    rig.wait_for_phase(&batch, InputLifecycleState::Consumed)
        .await;
    assert_eq!(
        contributions(&rig.script.primitives(), &steer_id),
        1,
        "a discarded append is redelivered by exactly one follow-up turn"
    );
    steer_id
}

#[tokio::test]
async fn second_durable_steer_while_the_first_waits_takes_the_fifo_follow_up() {
    let rig = DurableSteerRig::ephemeral().await;
    rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenStream)
        .await;
    let first = typed_steer("first", ConversationAppendRole::SystemNotice);
    let first_id = first.id().clone();
    rig.admit(first).await;
    rig.wait_for_waiting_delivery().await;
    let second = typed_steer("second", ConversationAppendRole::SystemNotice);
    let second_id = second.id().clone();
    rig.admit(second).await;
    // FIFO within the durable class: the second one is normalized to the
    // queued follow-up while the first still waits.
    tokio::time::timeout(Duration::from_secs(5), async {
        while !rig.queue().await.contains(&second_id) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the second durable steer falls back to the queue");

    rig.script.step(RunnerStep::OpenNextBoundary);
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    rig.wait_for_phase(&first_id, InputLifecycleState::Staged)
        .await;
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_apply_calls(2).await;
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_phase(&first_id, InputLifecycleState::Consumed)
        .await;
    rig.wait_for_phase(&second_id, InputLifecycleState::Consumed)
        .await;
    assert_eq!(rig.script.applied_durable(), vec![first_id.clone()]);
    assert_eq!(contributions(&rig.script.primitives(), &first_id), 0);
    assert_eq!(contributions(&rig.script.primitives(), &second_id), 1);
}

#[tokio::test]
async fn request_only_steer_keeps_its_boundary_while_a_durable_steer_waits() {
    let rig = DurableSteerRig::ephemeral().await;
    rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenStream)
        .await;
    let durable = typed_steer("durable", ConversationAppendRole::SystemNotice);
    let durable_id = durable.id().clone();
    rig.admit(durable).await;
    rig.wait_for_waiting_delivery().await;
    rig.script.step_and_wait(RunnerStep::OpenNextBoundary).await;
    // A request-only peer steer admitted now is still the head of its own
    // class and prepares the open boundary beside the durable delivery.
    let peer = peer_steer("peer steer");
    let peer_id = peer.id().clone();
    rig.admit(peer).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !rig.state.has_registered_request_only_delivery_for_test() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the request-only peer steer prepares beside the waiting durable delivery");
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    rig.wait_for_phase(&peer_id, InputLifecycleState::Consumed)
        .await;
    rig.wait_for_phase(&durable_id, InputLifecycleState::Staged)
        .await;
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_phase(&durable_id, InputLifecycleState::Consumed)
        .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        rig.script.apply_calls.load(Ordering::SeqCst),
        1,
        "both steers landed in the running turn; no follow-up turn ran"
    );
}

#[tokio::test]
async fn durable_steer_refused_at_an_extraction_boundary_takes_one_follow_up() {
    let rig = DurableSteerRig::ephemeral().await;
    rig.start_busy_turn().await;
    let steer = typed_steer("extraction", ConversationAppendRole::SystemNotice);
    let steer_id = steer.id().clone();
    rig.admit(steer).await;
    rig.wait_for_waiting_delivery().await;
    rig.script.step(RunnerStep::ExtractionBoundary);
    tokio::time::timeout(Duration::from_secs(5), async {
        while rig.state.has_waiting_delivery_for_test() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the extraction boundary refuses the durable delivery");
    // The refused input keeps its admitted steer-lane state for its
    // follow-up; no normalization moves it to the queue lane.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(rig.steer_queue().await.contains(&steer_id));
    assert!(!rig.queue().await.contains(&steer_id));
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_apply_calls(2).await;
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_phase(&steer_id, InputLifecycleState::Consumed)
        .await;
    assert!(rig.script.applied_durable().is_empty());
    assert_eq!(contributions(&rig.script.primitives(), &steer_id), 1);
}

#[tokio::test]
async fn follow_up_only_typed_steer_never_prepares_a_live_boundary() {
    let rig = DurableSteerRig::ephemeral().await;
    rig.start_busy_turn().await;
    let steer = typed_steer("system row", ConversationAppendRole::System);
    let steer_id = steer.id().clone();
    rig.admit(steer).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !rig.state.has_waiting_delivery_for_test(),
        "a follow-up-only input never registers at a boundary"
    );
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_apply_calls(2).await;
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_phase(&steer_id, InputLifecycleState::Consumed)
        .await;
    assert!(rig.script.applied_durable().is_empty());
    assert_eq!(contributions(&rig.script.primitives(), &steer_id), 1);
}

#[tokio::test]
async fn persistent_join_is_durable_before_publication_and_the_receipt_names_the_late_contributor()
{
    let store: Arc<dyn crate::store::RuntimeStore> =
        Arc::new(crate::store::InMemoryRuntimeStore::new());
    let rig = DurableSteerRig::persistent(Arc::clone(&store)).await;
    let runtime_id = MeerkatMachine::logical_runtime_id(&rig.session_id);
    let batch = rig.start_busy_turn().await;
    let steer = typed_steer("persisted", ConversationAppendRole::SystemNotice);
    let steer_id = steer.id().clone();
    rig.admit(steer).await;
    rig.wait_for_waiting_delivery().await;
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    rig.wait_for_phase(&steer_id, InputLifecycleState::Staged)
        .await;

    let row = store
        .load_input_state(&runtime_id, &steer_id)
        .await
        .expect("load joined row")
        .expect("joined row persisted");
    assert_eq!(row.seed.phase, InputLifecycleState::Staged);
    let run_id = row.seed.last_run_id.clone().expect("durable run binding");

    rig.script.step(RunnerStep::Finish);
    rig.wait_for_phase(&steer_id, InputLifecycleState::Consumed)
        .await;
    let receipts = store
        .load_committed_boundary_receipts(&runtime_id, &run_id)
        .await
        .expect("load run receipts");
    let terminal = receipts.last().expect("terminal receipt committed");
    assert_eq!(terminal.contributing_input_ids, vec![batch, steer_id]);
}

#[tokio::test]
async fn durable_steer_that_misses_its_boundary_keeps_steer_priority_over_queued_work() {
    let rig = DurableSteerRig::ephemeral().await;
    rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenStream)
        .await;
    // Ordinary queue-lane work admitted BEFORE the durable steer.
    let queued = Input::Prompt(crate::input::PromptInput::new("queued work", None));
    let queued_id = queued.id().clone();
    rig.admit(queued).await;
    let steer = typed_steer("misses", ConversationAppendRole::SystemNotice);
    let steer_id = steer.id().clone();
    rig.admit(steer).await;
    rig.wait_for_waiting_delivery().await;

    // The model answers: the run ends without another boundary.
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_apply_calls(2).await;
    assert!(rig.script.applied_durable().is_empty());
    let follow_up = rig.script.primitives()[1].clone();
    assert!(
        follow_up.contains(&steer_id) && !follow_up.contains(&queued_id),
        "the steer lane is served first, as before durable delivery: {follow_up:?}"
    );
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_phase(&steer_id, InputLifecycleState::Consumed)
        .await;
    rig.wait_for_apply_calls(3).await;
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_phase(&queued_id, InputLifecycleState::Consumed)
        .await;
    assert_eq!(contributions(&rig.script.primitives(), &steer_id), 1);
    assert_eq!(contributions(&rig.script.primitives(), &queued_id), 1);
}

#[tokio::test]
async fn persistent_durable_fallback_after_the_run_retired_keeps_the_runtime_healthy() {
    let store: Arc<dyn crate::store::RuntimeStore> =
        Arc::new(crate::store::InMemoryRuntimeStore::new());
    let rig = DurableSteerRig::persistent(Arc::clone(&store)).await;
    let runtime_id = MeerkatMachine::logical_runtime_id(&rig.session_id);
    let batch = rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenStream)
        .await;
    // Hold the ingress after the coordinator resolves its preparation, so the
    // runtime loop commits the run (and the lifecycle leaves Running) first.
    rig.script.prepare_hold_armed.store(true, Ordering::SeqCst);
    let steer = typed_steer("retiring", ConversationAppendRole::SystemNotice);
    let steer_id = steer.id().clone();
    rig.admit(steer).await;
    rig.wait_for_waiting_delivery().await;
    crate::traits::RuntimeControlPlane::retire(rig.adapter.as_ref(), &runtime_id)
        .await
        .expect("retire while the run is bound");
    // Pause the loop's next lap before it claims queue authority, so the
    // held ingress observes "run advanced, input still queued" in Retired.
    let (loop_paused, loop_release) = rig
        .adapter
        .arm_runtime_loop_before_queue_authority_test_hook(rig.session_id.clone());

    // The model answers without another boundary: the delivery is withdrawn
    // and the run commits into Retired while the ingress is still held.
    rig.script.step(RunnerStep::Finish);
    tokio::time::timeout(Duration::from_secs(5), loop_paused)
        .await
        .expect("the retired runtime loop reaches its drain lap")
        .expect("queue-authority hook armed");
    tokio::time::timeout(Duration::from_secs(5), async {
        while !rig.script.prepare_hold_reached.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("the durable preparation resolved");
    rig.wait_for_phase(&batch, InputLifecycleState::Consumed)
        .await;
    assert_eq!(
        crate::store::load_runtime_state(store.as_ref(), &runtime_id)
            .await
            .expect("load runtime state"),
        Some(crate::runtime_state::RuntimeState::Retired)
    );
    assert_eq!(
        rig.phase(&steer_id).await,
        Some(InputLifecycleState::Queued)
    );

    // The ingress now observes the advanced run with the input still queued.
    // Its fallback runs no generated transition (`LiveBoundaryUnavailable`
    // does not exist in Retired), so the persistent runtime stays
    // durability-ready and the retiring runtime drains the notice as exactly
    // one follow-up turn.
    rig.script
        .prepare_hold_released
        .store(true, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(rig.steer_queue().await.contains(&steer_id));
    loop_release
        .send(())
        .expect("the runtime loop still waits at the drain lap");
    rig.wait_for_apply_calls(2).await;
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_phase(&steer_id, InputLifecycleState::Consumed)
        .await;
    assert!(rig.script.applied_durable().is_empty());
    assert_eq!(contributions(&rig.script.primitives(), &steer_id), 1);
}

#[tokio::test]
async fn persistent_crash_after_the_join_recovers_the_input_for_exactly_one_follow_up() {
    let store: Arc<dyn crate::store::RuntimeStore> =
        Arc::new(crate::store::InMemoryRuntimeStore::new());
    let crashed = DurableSteerRig::persistent(Arc::clone(&store)).await;
    let session_id = crashed.session_id.clone();
    let runtime_id = MeerkatMachine::logical_runtime_id(&session_id);
    let batch = crashed.start_busy_turn().await;
    let steer = typed_steer("crash", ConversationAppendRole::SystemNotice);
    let steer_id = steer.id().clone();
    crashed.admit(steer).await;
    crashed.wait_for_waiting_delivery().await;
    crashed.script.step(RunnerStep::BoundaryThenToolCalls);
    crashed
        .wait_for_phase(&steer_id, InputLifecycleState::Staged)
        .await;
    // The runner applied the append into the live image of the in-flight run.
    assert_eq!(crashed.script.applied_durable(), vec![steer_id.clone()]);
    let joined = store
        .load_input_state(&runtime_id, &steer_id)
        .await
        .expect("load joined row")
        .expect("joined row persisted");
    assert_eq!(joined.seed.phase, InputLifecycleState::Staged);
    let crashed_run = joined
        .seed
        .last_run_id
        .clone()
        .expect("durable run binding");

    // The process dies with the run in flight: nothing of the run commits and
    // its live image (which held the append) is lost. The abandoned machine
    // is never driven again.
    std::mem::forget(crashed);

    let recovered = DurableSteerRig::with_adapter_for_session(
        Arc::new(MeerkatMachine::persistent_without_blobs(Arc::clone(&store))),
        session_id,
    )
    .await;
    // Cold recovery returns both contributors of the interrupted run to their
    // lanes; the durable steer is delivered by exactly one follow-up turn.
    // Wait for the fact the rest of the test needs: the joined input is no
    // longer bound to the crashed run. Recovery requeues it, and the
    // recovered runtime may restage it for its follow-up run at once, so
    // "not Staged" is not a phase every legal path passes through where a
    // poll can see it; a restaged input waits for the Finish step below and
    // would never leave Staged here.
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(stored) = recovered.stored(&steer_id).await
                && (stored.seed.phase != InputLifecycleState::Staged
                    || stored.seed.last_run_id.as_ref() != Some(&crashed_run))
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("recovery releases the joined input from the crashed run");
    for expected_calls in 1..=2 {
        if recovered.phase(&steer_id).await == Some(InputLifecycleState::Consumed)
            && recovered.phase(&batch).await == Some(InputLifecycleState::Consumed)
        {
            break;
        }
        recovered.wait_for_apply_calls(expected_calls).await;
        recovered.script.step(RunnerStep::Finish);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    recovered
        .wait_for_phase(&steer_id, InputLifecycleState::Consumed)
        .await;
    recovered
        .wait_for_phase(&batch, InputLifecycleState::Consumed)
        .await;
    assert_eq!(
        contributions(&recovered.script.primitives(), &steer_id),
        1,
        "the recovered durable steer is delivered exactly once"
    );
    let consumed = store
        .load_input_state(&runtime_id, &steer_id)
        .await
        .expect("load consumed row")
        .expect("consumed row persisted");
    assert_eq!(consumed.seed.phase, InputLifecycleState::Consumed);
    assert_ne!(
        consumed.seed.last_run_id,
        Some(crashed_run),
        "a follow-up run of the recovered runtime consumed it"
    );
}

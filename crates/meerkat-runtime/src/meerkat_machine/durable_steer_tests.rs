//! Durable in-turn Steer delivery through the runtime ingress, the generated
//! MeerkatMachine join/resolve transitions, and the runtime-loop terminal.
//!
//! The executor below drives the REAL core boundary coordinator through its
//! test-support runner seam, so the witness facts the runtime resolves at the
//! run terminal are produced exactly as the agent loop produces them.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
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

/// How long a test makes the runner's append record lag the boundary take.
const APPEND_RECORD_LAG: Duration = Duration::from_millis(150);

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
    /// Cancel the run after the owning service reports the image discarded,
    /// as the persistent service does for a cancelled, uncommitted run.
    CancelDiscardingImage,
}

struct RunnerScript {
    steps: AsyncMutex<mpsc::UnboundedReceiver<RunnerStep>>,
    sender: mpsc::UnboundedSender<RunnerStep>,
    apply_started: Notify,
    apply_calls: AtomicUsize,
    /// Applies that are fully observable: their primitive recorded and their
    /// run set inside `apply`. A waiter keyed on `apply_calls` alone could
    /// read `active_run` or `primitives` before the newest apply recorded
    /// them (the counter moves at entry), which lost the race on a loaded
    /// CI runner (`current_run` saw no run right after the stop).
    applies_observable: tokio::sync::watch::Sender<usize>,
    steps_done: AtomicUsize,
    primitives: std::sync::Mutex<Vec<Vec<InputId>>>,
    applied_durable: std::sync::Mutex<Vec<InputId>>,
    /// Request-only contexts each boundary take handed the model.
    request_only_taken: std::sync::Mutex<Vec<usize>>,
    discarded: std::sync::Mutex<Vec<meerkat_core::event::BoundaryAppendsDiscarded>>,
    applied_runs: std::sync::Mutex<Vec<RunId>>,
    publication_store: std::sync::Mutex<
        Option<(
            Arc<dyn crate::store::RuntimeStore>,
            crate::identifiers::LogicalRuntimeId,
        )>,
    >,
    runtime_stopped: Notify,
    /// While armed, the ingress side of a boundary preparation is held after
    /// the core coordinator resolved it, so the runtime loop can win the
    /// mutation gate first (the "run advanced during preparation" ordering).
    prepare_hold_armed: AtomicBool,
    prepare_hold_released: AtomicBool,
    prepare_hold_reached: AtomicBool,
    /// Delay between a boundary take returning a durable append and the
    /// runner recording it (milliseconds). Zero by default; a test sets it to
    /// force the record to lag the input's Staged phase.
    append_record_lag_ms: AtomicU64,
    /// The run currently inside `apply`, for the exact-run interrupt handle.
    active_run: std::sync::Mutex<Option<RunId>>,
    /// The runner step an accepted exact-run hard interrupt injects, or
    /// `None` to acknowledge the interrupt without ending the run yet.
    interrupt_step: std::sync::Mutex<Option<RunnerStep>>,
    interrupts: AtomicUsize,
    /// When set, the exact-run interrupt callback fails after counting.
    interrupt_fails: AtomicBool,
    /// The reply capabilities each prepared durable boundary delivery
    /// carries, in preparation order.
    prepared_reply_capabilities:
        std::sync::Mutex<Vec<Vec<meerkat_core::comms::PeerReplyCapability>>>,
}

impl RunnerScript {
    fn new() -> Arc<Self> {
        let (sender, steps) = mpsc::unbounded_channel();
        Arc::new(Self {
            steps: AsyncMutex::new(steps),
            sender,
            apply_started: Notify::new(),
            apply_calls: AtomicUsize::new(0),
            applies_observable: tokio::sync::watch::Sender::new(0),
            steps_done: AtomicUsize::new(0),
            primitives: std::sync::Mutex::new(Vec::new()),
            applied_durable: std::sync::Mutex::new(Vec::new()),
            request_only_taken: std::sync::Mutex::new(Vec::new()),
            discarded: std::sync::Mutex::new(Vec::new()),
            applied_runs: std::sync::Mutex::new(Vec::new()),
            publication_store: std::sync::Mutex::new(None),
            runtime_stopped: Notify::new(),
            prepare_hold_armed: AtomicBool::new(false),
            prepare_hold_released: AtomicBool::new(false),
            prepare_hold_reached: AtomicBool::new(false),
            append_record_lag_ms: AtomicU64::new(0),
            active_run: std::sync::Mutex::new(None),
            interrupt_step: std::sync::Mutex::new(Some(RunnerStep::CancelDiscardingImage)),
            interrupts: AtomicUsize::new(0),
            interrupt_fails: AtomicBool::new(false),
            prepared_reply_capabilities: std::sync::Mutex::new(Vec::new()),
        })
    }

    /// Record each applied durable append only `lag` after its boundary take
    /// returned, so the input shows Staged before the record exists.
    fn lag_append_records(&self, lag: Duration) {
        self.append_record_lag_ms.store(
            u64::try_from(lag.as_millis()).unwrap_or(u64::MAX),
            Ordering::SeqCst,
        );
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

    fn prepared_reply_capabilities(&self) -> Vec<Vec<meerkat_core::comms::PeerReplyCapability>> {
        self.prepared_reply_capabilities.lock().unwrap().clone()
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
        if let meerkat_core::TurnBoundaryDelivery::DurableAppends(appends) = &delivery {
            self.script
                .prepared_reply_capabilities
                .lock()
                .unwrap()
                .push(appends.peer_reply_capabilities().to_vec());
        }
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

/// Exact-run hard interrupt: compares the run inside `apply` and, when it
/// matches, ends it with the configured runner step.
struct DurableSteerInterruptHandle {
    script: Arc<RunnerScript>,
}

#[async_trait::async_trait]
impl meerkat_core::lifecycle::CoreExecutorInterruptHandle for DurableSteerInterruptHandle {
    async fn hard_cancel_run_if_current(
        &self,
        expected_run_id: &RunId,
        _reason: String,
    ) -> Result<bool, CoreExecutorError> {
        if self.script.active_run.lock().unwrap().as_ref() != Some(expected_run_id) {
            return Ok(false);
        }
        self.script.interrupts.fetch_add(1, Ordering::SeqCst);
        if self.script.interrupt_fails.load(Ordering::SeqCst) {
            return Err(CoreExecutorError::Internal(
                "scripted interrupt callback failure".into(),
            ));
        }
        if let Some(step) = *self.script.interrupt_step.lock().unwrap() {
            self.script.step(step);
        }
        Ok(true)
    }
}

/// Records the interaction terminals the runtime publishes. A directed input
/// is admitted only by an executor that offers this capability.
#[derive(Default)]
struct DurableSteerTerminalPublisher {
    events: std::sync::Mutex<Vec<meerkat_core::event::AgentEvent>>,
}

#[async_trait::async_trait]
impl meerkat_core::lifecycle::CoreExecutorPublicationHandle for DurableSteerTerminalPublisher {
    async fn publish_interaction_terminals(
        &self,
        events: &[meerkat_core::event::AgentEvent],
    ) -> Result<
        Vec<meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt>,
        CoreExecutorError,
    > {
        self.events.lock().unwrap().extend_from_slice(events);
        events
            .iter()
            .enumerate()
            .map(|(index, event)| {
                meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt::try_new(
                    event,
                    index as u64 + 1,
                )
            })
            .collect()
    }
}

struct DurableSteerExecutor {
    turn_state: Arc<dyn TurnStateHandle>,
    state: meerkat_core::TransientTurnContextStateHandle,
    script: Arc<RunnerScript>,
    terminal_publisher: Option<Arc<DurableSteerTerminalPublisher>>,
}

#[async_trait::async_trait]
impl CoreExecutor for DurableSteerExecutor {
    async fn publish_boundary_appends_discarded(
        &mut self,
        discarded: &meerkat_core::event::BoundaryAppendsDiscarded,
    ) -> Result<(), CoreExecutorError> {
        assert_eq!(
            self.script.apply_calls.load(Ordering::SeqCst),
            1,
            "discard publication must precede the requeued apply"
        );
        let stored = self.script.publication_store.lock().unwrap().clone();
        if let Some((store, runtime_id)) = stored {
            for input in &discarded.input_ids {
                let row = store
                    .load_input_state(&runtime_id, input)
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    row.seed.phase,
                    InputLifecycleState::Queued,
                    "discard publication must follow the durable requeue write"
                );
            }
        }
        self.script
            .discarded
            .lock()
            .unwrap()
            .push(discarded.clone());
        Ok(())
    }

    fn boundary_handle(&self) -> Option<Arc<dyn CoreExecutorBoundaryHandle>> {
        Some(Arc::new(DurableSteerBoundaryHandle {
            state: self.state.clone(),
            script: Arc::clone(&self.script),
        }))
    }

    fn interrupt_handle(
        &self,
    ) -> Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorInterruptHandle>> {
        Some(Arc::new(DurableSteerInterruptHandle {
            script: Arc::clone(&self.script),
        }))
    }

    fn publication_handle(
        &self,
    ) -> Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>> {
        self.terminal_publisher
            .clone()
            .map(|publisher| publisher as Arc<_>)
    }

    async fn apply(
        &mut self,
        run_id: RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        self.script.apply_calls.fetch_add(1, Ordering::SeqCst);
        self.script
            .applied_runs
            .lock()
            .unwrap()
            .push(run_id.clone());
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
        *self.script.active_run.lock().unwrap() = Some(run_id.clone());
        self.script
            .applies_observable
            .send_modify(|observable| *observable += 1);
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
                    if !taken.request_only.is_empty() {
                        self.script
                            .request_only_taken
                            .lock()
                            .unwrap()
                            .push(taken.request_only.len());
                    }
                    if let Some(appends) = taken.applied_durable {
                        let lag = self.script.append_record_lag_ms.load(Ordering::SeqCst);
                        if lag > 0 {
                            tokio::time::sleep(Duration::from_millis(lag)).await;
                        }
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
                | RunnerStep::Cancel
                | RunnerStep::CancelDiscardingImage => {}
            }
            if !matches!(
                step,
                RunnerStep::Finish
                    | RunnerStep::FailKeepingImage
                    | RunnerStep::FailDiscardingImage
                    | RunnerStep::Cancel
                    | RunnerStep::CancelDiscardingImage
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
                RunnerStep::CancelDiscardingImage => {
                    self.state.discard_uncommitted_durable_deliveries(&run_id);
                    break Err(CoreExecutorError::Cancelled);
                }
                RunnerStep::BoundaryThenToolCalls
                | RunnerStep::BoundaryThenStream
                | RunnerStep::OpenNextBoundary
                | RunnerStep::ExtractionBoundary => {
                    unreachable!("non-terminal runner steps continue above")
                }
            }
        };
        drop(steps);
        *self.script.active_run.lock().unwrap() = None;
        // The agent loop's run guard closes the run before `apply` returns.
        drop(run_guard);
        outcome
    }

    async fn cancel_after_boundary(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }

    async fn stop_runtime_executor(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        self.script.runtime_stopped.notify_one();
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
        let rig = Self::with_adapter(Arc::new(
            MeerkatMachine::persistent_without_blobs(Arc::clone(&store))
                .expect("persistent machine"),
        ))
        .await;
        *rig.script.publication_store.lock().unwrap() =
            Some((store, MeerkatMachine::logical_runtime_id(&rig.session_id)));
        rig
    }

    async fn with_adapter(adapter: Arc<MeerkatMachine>) -> Self {
        Self::with_adapter_for_session(adapter, SessionId::new()).await
    }

    /// An ephemeral rig whose executor offers a terminal publication
    /// capability, as directed inputs require.
    async fn ephemeral_with_terminal_publication() -> Self {
        Self::with_executor_options(
            Arc::new(MeerkatMachine::ephemeral()),
            SessionId::new(),
            Some(Arc::new(DurableSteerTerminalPublisher::default())),
        )
        .await
    }

    async fn with_adapter_for_session(adapter: Arc<MeerkatMachine>, session_id: SessionId) -> Self {
        Self::with_executor_options(adapter, session_id, None).await
    }

    async fn with_executor_options(
        adapter: Arc<MeerkatMachine>,
        session_id: SessionId,
        terminal_publisher: Option<Arc<DurableSteerTerminalPublisher>>,
    ) -> Self {
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
                    terminal_publisher,
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
        let (outcome, completion) = tokio::time::timeout(
            Duration::from_secs(5),
            self.adapter
                .accept_input_with_completion(&self.session_id, input),
        )
        .await
        .expect("a waiting durable delivery must not hold up later admissions")
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

    /// Wait until the runner has recorded `expected` applied durable
    /// appends. The runner records an append after its boundary take returns,
    /// which can be after the input already shows Staged, so a test asserting
    /// what was applied waits for the record, not for the phase.
    async fn wait_for_applied_durable(&self, expected: usize) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.script.applied_durable().len() < expected {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("expected {expected} applied durable appends"));
    }

    /// Wait until `expected` applies are fully observable (their primitive
    /// recorded and their run inside `apply`), on the script's event, not a
    /// poll of the entry counter.
    async fn wait_for_apply_calls(&self, expected: usize) {
        let mut observable = self.script.applies_observable.subscribe();
        tokio::time::timeout(
            Duration::from_secs(5),
            observable.wait_for(|applies| *applies >= expected),
        )
        .await
        .unwrap_or_else(|_| panic!("expected {expected} apply calls"))
        .expect("the runner script outlives the rig");
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
        runtime_source: None,
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

fn steer_header(source: crate::input::InputOrigin) -> crate::input::InputHeader {
    crate::input::InputHeader {
        ingress_context: None,
        retained_resume: None,
        authority_association: None,
        id: InputId::new(),
        timestamp: chrono::Utc::now(),
        source,
        durability: crate::input::InputDurability::Durable,
        visibility: crate::input::InputVisibility::default(),
        idempotency_key: None,
        supersession_key: None,
        correlation_id: None,
    }
}

fn steer_metadata() -> meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
    meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
        handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
        ..Default::default()
    }
}

/// A peer steer of one convention, as the comms bridge admits it.
fn peer_steer_with(convention: crate::input::PeerConvention, body: &str) -> Input {
    Input::Peer(crate::input::PeerInput {
        directed_interaction_id: None,
        objective_id: None,
        system_prompts: Vec::new(),
        injected_context: Vec::new(),
        sender_taint: None,
        header: steer_header(crate::input::InputOrigin::Peer {
            peer_id: "durable-steer-peer".into(),
            display_identity: None,
            runtime_id: None,
        }),
        convention: Some(convention),
        content: body.into(),
        payload: None,
        handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
        in_reply_to: None,
    })
}

fn peer_steer(body: &str) -> Input {
    peer_steer_with(crate::input::PeerConvention::Message, body)
}

fn peer_request_steer(body: &str) -> Input {
    peer_steer_with(
        crate::input::PeerConvention::Request {
            request_id: format!("req-{}", uuid::Uuid::new_v4()),
            intent: "checksum_token".into(),
        },
        body,
    )
}

fn peer_lifecycle_steer(body: &str) -> Input {
    peer_steer_with(
        crate::input::PeerConvention::Lifecycle {
            kind: meerkat_core::comms::PeerLifecycleKind::PeerAdded,
        },
        body,
    )
}

/// A directed peer steer: its interaction, input, correlation and idempotency
/// ids agree and it names the sending runtime, as a host-residency-fenced
/// bridge delivery carries them.
fn directed_peer_steer(body: &str) -> Input {
    let mut input = peer_steer(body);
    if let Input::Peer(peer) = &mut input {
        let interaction = meerkat_core::interaction::InteractionId(peer.header.id.0);
        peer.directed_interaction_id = Some(interaction);
        peer.header.correlation_id =
            Some(crate::identifiers::CorrelationId::from_uuid(interaction.0));
        peer.header.idempotency_key = Some(crate::identifiers::IdempotencyKey::new(
            interaction.to_string(),
        ));
        peer.header.source = crate::input::InputOrigin::Peer {
            peer_id: "durable-steer-peer".into(),
            display_identity: None,
            runtime_id: Some(crate::identifiers::LogicalRuntimeId(
                "durable-steer-peer-runtime".into(),
            )),
        };
    }
    input
}

/// A directed flow step, as a remote flow delivers it to the member that
/// runs the loop: its own turn must start, since it owns a directed terminal.
fn directed_flow_step_steer(text: &str) -> Input {
    crate::mob_adapter::create_tracked_flow_step_input(
        "remote-step",
        text.into(),
        "durable-steer-flow",
        Some(steer_metadata()),
        &uuid::Uuid::new_v4().to_string(),
    )
    .expect("tracked flow step")
}

fn text_prompt_steer(text: &str) -> Input {
    Input::Prompt(crate::input::PromptInput::new(text, Some(steer_metadata())))
}

fn external_event_steer(body: &str) -> Input {
    Input::ExternalEvent(crate::input::ExternalEventInput {
        header: steer_header(crate::input::InputOrigin::External {
            source_name: "webhook".into(),
        }),
        event_type: "webhook".into(),
        payload: serde_json::json!({ "body": body }),
        blocks: None,
        handling_mode: meerkat_core::types::HandlingMode::Steer,
        render_metadata: None,
        objective_id: None,
    })
}

fn flow_step_steer(text: &str) -> Input {
    Input::FlowStep(crate::input::FlowStepInput {
        header: steer_header(crate::input::InputOrigin::Operator),
        step_id: "local-step".into(),
        content: text.into(),
        directed_interaction_id: None,
        turn_metadata: Some(steer_metadata()),
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
        Some(LiveBoundaryDeliveryClass::DurableAppend)
    );
    for (label, input) in [
        ("peer message", peer_steer("peer")),
        ("peer request", peer_request_steer("peer request")),
        ("peer lifecycle", peer_lifecycle_steer("peer lifecycle")),
        ("external event", external_event_steer("event")),
        ("local flow step", flow_step_steer("step")),
    ] {
        assert_eq!(
            class(&input),
            Some(LiveBoundaryDeliveryClass::DurableAppend),
            "{label} steer"
        );
    }
    for (label, input) in [
        ("peer", directed_peer_steer("directed")),
        ("flow step", directed_flow_step_steer("directed step")),
    ] {
        assert_eq!(
            class(&input),
            Some(LiveBoundaryDeliveryClass::FollowUpOnly),
            "a directed {label} delivery owns a terminal its own turn must start"
        );
    }
    let routed = Input::Prompt(crate::input::PromptInput::new(
        "routed",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                model: Some(meerkat_core::lifecycle::run_primitive::ModelId::new(
                    "claude-sonnet-4-5",
                )),
                ..steer_metadata()
            },
        ),
    ));
    assert_eq!(
        class(&routed),
        Some(LiveBoundaryDeliveryClass::FollowUpOnly),
        "model routing applies only to a fresh turn"
    );
    let queued = Input::Prompt(crate::input::PromptInput::new("queued", None));
    assert_eq!(class(&queued), None, "queue-lane admissions carry no class");
}

#[tokio::test]
async fn durable_steer_joins_the_running_run_and_is_consumed_with_its_commit() {
    durable_steer_joins_the_running_run(Duration::ZERO).await;
}

/// The same join, with the runner recording the applied append only after
/// the input shows Staged (the record can lag the phase).
#[tokio::test]
async fn durable_steer_join_is_asserted_after_a_lagging_append_record() {
    durable_steer_joins_the_running_run(APPEND_RECORD_LAG).await;
}

async fn durable_steer_joins_the_running_run(append_record_lag: Duration) {
    let rig = DurableSteerRig::ephemeral().await;
    rig.script.lag_append_records(append_record_lag);
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
    rig.wait_for_applied_durable(1).await;
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
    assert!(
        rig.script.discarded.lock().unwrap().is_empty(),
        "unapplied input is not discarded"
    );
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
    assert!(
        rig.script.discarded.lock().unwrap().is_empty(),
        "cancellation retains the applied input and publishes no discard"
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
    assert!(
        rig.script.discarded.lock().unwrap().is_empty(),
        "retained input is not discarded"
    );
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
    applied_steer_then_image_discarded_is_redelivered_exactly_once(
        rig,
        typed_steer("discarded", ConversationAppendRole::SystemNotice),
    )
    .await
}

async fn applied_steer_then_image_discarded_is_redelivered_exactly_once(
    rig: DurableSteerRig,
    steer: Input,
) -> InputId {
    let batch = rig.start_busy_turn().await;
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
    let applied_runs = rig.script.applied_runs.lock().unwrap();
    assert_ne!(applied_runs[0], applied_runs[1]);
    assert_eq!(
        *rig.script.discarded.lock().unwrap(),
        vec![meerkat_core::event::BoundaryAppendsDiscarded {
            session_id: rig.session_id.clone(),
            run_id: applied_runs[0].clone(),
            input_ids: vec![steer_id.clone()],
        }],
        "discard must publish the original application, including without a completion observer"
    );
    steer_id
}

/// Admit each input in order; only the oldest durable one prepares, so
/// exactly one delivery is registered once they are all admitted.
async fn admit_in_order(rig: &DurableSteerRig, inputs: Vec<Input>) -> Vec<InputId> {
    let mut ids = Vec::new();
    for input in inputs {
        ids.push(input.id().clone());
        rig.admit(input).await;
    }
    rig.wait_for_waiting_delivery().await;
    // The later ones wait in the steer lane: they neither register beside
    // the oldest nor fall back to the queue lane.
    tokio::time::sleep(Duration::from_millis(50)).await;
    for later in &ids[1..] {
        assert!(rig.steer_queue().await.contains(later));
        assert!(!rig.queue().await.contains(later));
    }
    ids
}

#[tokio::test]
async fn second_durable_steer_while_the_first_waits_joins_the_following_boundary() {
    let rig = DurableSteerRig::ephemeral().await;
    rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenStream)
        .await;
    let ids = admit_in_order(
        &rig,
        vec![
            typed_steer("first", ConversationAppendRole::SystemNotice),
            typed_steer("second", ConversationAppendRole::SystemNotice),
        ],
    )
    .await;
    let [first_id, second_id] = ids.as_slice() else {
        unreachable!("two steers")
    };
    let (first_id, second_id) = (first_id.clone(), second_id.clone());

    // Each boundary takes one durable delivery, in arrival order: the second
    // prepares once the first has joined, for the following boundary.
    rig.script.step(RunnerStep::OpenNextBoundary);
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    rig.wait_for_phase(&first_id, InputLifecycleState::Staged)
        .await;
    rig.wait_for_applied_durable(1).await;
    assert_eq!(
        rig.phase(&second_id).await,
        Some(InputLifecycleState::Queued)
    );
    rig.wait_for_waiting_delivery().await;
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    rig.wait_for_phase(&second_id, InputLifecycleState::Staged)
        .await;
    rig.wait_for_applied_durable(2).await;
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_phase(&first_id, InputLifecycleState::Consumed)
        .await;
    rig.wait_for_phase(&second_id, InputLifecycleState::Consumed)
        .await;
    assert_eq!(
        rig.script.applied_durable(),
        vec![first_id.clone(), second_id.clone()]
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        rig.script.apply_calls.load(Ordering::SeqCst),
        1,
        "both joined the running run; no follow-up turn ran"
    );
}

#[tokio::test]
async fn durable_steer_still_waiting_when_the_run_ends_takes_the_fifo_follow_up() {
    let rig = DurableSteerRig::ephemeral().await;
    rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenStream)
        .await;
    let ids = admit_in_order(
        &rig,
        vec![
            typed_steer("first", ConversationAppendRole::SystemNotice),
            typed_steer("second", ConversationAppendRole::SystemNotice),
        ],
    )
    .await;
    let [first_id, second_id] = ids.as_slice() else {
        unreachable!("two steers")
    };
    let (first_id, second_id) = (first_id.clone(), second_id.clone());

    rig.script.step(RunnerStep::OpenNextBoundary);
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    rig.wait_for_phase(&first_id, InputLifecycleState::Staged)
        .await;
    // The second registers for the next boundary of the running run.
    rig.wait_for_waiting_delivery().await;
    // The model answers: the run ends before the boundary the second waits for.
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

/// A durable steer that waits behind an earlier one while the run ends, and
/// that the runtime loop takes into the follow-up run before its wait ends,
/// prepares nothing in that run: it registers no boundary delivery there,
/// its boundary work settles without an error, a durable steer admitted
/// during the follow-up still joins it, and the follow-up turn applies each
/// of the two waiting steers exactly once.
#[tokio::test]
async fn durable_steer_taken_into_the_follow_up_while_it_waited_prepares_nothing_there() {
    let rig = DurableSteerRig::ephemeral().await;
    let (outcomes_tx, mut outcomes) = mpsc::unbounded_channel();
    *rig.adapter
        .test_accepted_boundary_work_outcomes
        .lock()
        .unwrap() = Some(outcomes_tx);
    let (reached_tx, reached) = tokio::sync::oneshot::channel();
    let (release, release_rx) = tokio::sync::oneshot::channel();
    *rig.adapter.test_durable_steer_relock_hold.lock().unwrap() = Some((reached_tx, release_rx));

    rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenStream)
        .await;
    // The first waits for the next boundary of the running run; the second
    // waits behind it.
    let ids = admit_in_order(
        &rig,
        vec![
            typed_steer("first", ConversationAppendRole::SystemNotice),
            typed_steer("second", ConversationAppendRole::SystemNotice),
        ],
    )
    .await;
    let [first_id, second_id] = ids.as_slice() else {
        unreachable!("two steers")
    };
    let (first_id, second_id) = (first_id.clone(), second_id.clone());

    // The run ends before that boundary. The second's wait ends, and the
    // runtime loop takes both into the follow-up run before the second's
    // work retakes the session's mutation gate.
    rig.script.step(RunnerStep::Finish);
    tokio::time::timeout(Duration::from_secs(5), reached)
        .await
        .expect("the second's wait ends")
        .expect("the hold is armed");
    rig.wait_for_apply_calls(2).await;
    rig.wait_for_phase(&first_id, InputLifecycleState::Staged)
        .await;
    rig.wait_for_phase(&second_id, InputLifecycleState::Staged)
        .await;
    release
        .send(())
        .expect("the second's work waits at the hold");

    // Before the follow-up's first boundary: the second registers no delivery
    // in the run it is already staged into, and its work settles cleanly.
    let mut settled = Vec::new();
    let second_outcome = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            assert!(
                !rig.state.has_waiting_delivery_for_test(),
                "the second registered a boundary delivery in the follow-up run it is staged into"
            );
            while let Ok(outcome) = outcomes.try_recv() {
                settled.push(outcome);
            }
            if let Some((_, outcome)) = settled
                .iter()
                .find(|(input_id, _)| input_id.as_ref() == Some(&second_id))
            {
                return outcome.clone();
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("the second's boundary work settles");
    assert_eq!(second_outcome, Ok(()));
    assert!(!rig.state.has_waiting_delivery_for_test());

    // A durable steer admitted during the follow-up joins it.
    let third = typed_steer("third", ConversationAppendRole::SystemNotice);
    let third_id = third.id().clone();
    rig.admit(third).await;
    rig.wait_for_waiting_delivery().await;
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    rig.wait_for_applied_durable(1).await;
    assert_eq!(rig.script.applied_durable(), vec![third_id.clone()]);
    rig.script.step(RunnerStep::Finish);
    for input_id in [&first_id, &second_id, &third_id] {
        rig.wait_for_phase(input_id, InputLifecycleState::Consumed)
            .await;
    }
    let primitives = rig.script.primitives();
    assert_eq!(contributions(&primitives, &first_id), 1);
    assert_eq!(contributions(&primitives, &second_id), 1);
    assert_eq!(contributions(&primitives, &third_id), 0);
    assert!(primitives[1].contains(&first_id) && primitives[1].contains(&second_id));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        rig.script.apply_calls.load(Ordering::SeqCst),
        2,
        "the follow-up run applied all three; no further turn ran"
    );
    while let Ok(outcome) = outcomes.try_recv() {
        settled.push(outcome);
    }
    for input_id in [&first_id, &third_id] {
        assert_eq!(
            settled
                .iter()
                .find(|(settled_id, _)| settled_id.as_ref() == Some(input_id))
                .map(|(_, outcome)| outcome.clone()),
            Some(Ok(())),
            "boundary work of {input_id}"
        );
    }
}

/// FIFO is per delivery class: a request-only steer admitted after a durable
/// peer steer that waits for the next boundary still heads its own class and
/// lands in that boundary beside it. The request-only steer here is an
/// ordinary continuation carrying a turn append, one of the kinds that stay
/// request-only. Both land in the running turn; no follow-up turn runs.
#[tokio::test]
async fn request_only_steer_keeps_its_boundary_while_a_durable_steer_waits() {
    let rig = DurableSteerRig::ephemeral().await;
    rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenStream)
        .await;
    let durable = peer_steer("durable peer steer");
    let durable_id = durable.id().clone();
    rig.admit(durable).await;
    rig.wait_for_waiting_delivery().await;
    rig.script.step_and_wait(RunnerStep::OpenNextBoundary).await;

    let mut continuation = crate::input::ContinuationInput::detached_background_op_completed();
    continuation.turn_append = Some(notice_append(
        "review context",
        ConversationAppendRole::SystemNotice,
    ));
    let continuation = Input::Continuation(continuation);
    let continuation_id = continuation.id().clone();
    assert_eq!(
        crate::ingress_types::RuntimeInputSemantics::try_from_generated_admission(
            &continuation,
            false
        )
        .expect("generated admission")
        .live_boundary_delivery(),
        Some(crate::ingress_types::LiveBoundaryDeliveryClass::RequestOnly),
        "an ordinary continuation with a turn append steers request-only"
    );
    rig.admit(continuation).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !rig.state.has_registered_request_only_delivery_for_test() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the request-only continuation prepares beside the waiting durable delivery");
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    rig.wait_for_phase(&continuation_id, InputLifecycleState::Consumed)
        .await;
    rig.wait_for_phase(&durable_id, InputLifecycleState::Staged)
        .await;
    rig.wait_for_applied_durable(1).await;
    assert_eq!(rig.script.applied_durable(), vec![durable_id.clone()]);
    assert_eq!(*rig.script.request_only_taken.lock().unwrap(), vec![1]);
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

/// Three peer steers admitted inside one gated boundary are
/// each delivered exactly once, in arrival order: one per boundary of the
/// running run (each prepares once the one before it has joined), and the
/// one still waiting when the run ends in its follow-up turn.
#[tokio::test]
async fn three_peer_steers_in_one_gated_boundary_land_once_each_in_arrival_order() {
    let rig = DurableSteerRig::ephemeral().await;
    rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenStream)
        .await;
    let steers = admit_in_order(
        &rig,
        ["peer one", "peer two", "peer three"]
            .into_iter()
            .map(peer_steer)
            .collect(),
    )
    .await;
    let [one, two, three] = steers.as_slice() else {
        unreachable!("three steers")
    };

    rig.script.step(RunnerStep::OpenNextBoundary);
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    rig.wait_for_applied_durable(1).await;
    rig.wait_for_waiting_delivery().await;
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    rig.wait_for_applied_durable(2).await;
    rig.wait_for_waiting_delivery().await;
    assert_eq!(rig.phase(three).await, Some(InputLifecycleState::Queued));
    // The model answers before a third boundary: the third takes the
    // follow-up turn.
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_apply_calls(2).await;
    rig.script.step(RunnerStep::Finish);
    for id in [one, two, three] {
        rig.wait_for_phase(id, InputLifecycleState::Consumed).await;
    }
    assert_eq!(
        rig.script.applied_durable(),
        vec![one.clone(), two.clone()],
        "the first two land at the first and second boundary of the running run"
    );
    assert_eq!(contributions(&rig.script.primitives(), one), 0);
    assert_eq!(contributions(&rig.script.primitives(), two), 0);
    assert_eq!(
        contributions(&rig.script.primitives(), three),
        1,
        "the third is delivered by exactly one follow-up turn"
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
    persistent_crash_after_the_join(Duration::ZERO).await;
}

/// The same crash, with the crashed runner recording the applied append only
/// after the input shows Staged (the record can lag the phase).
#[tokio::test]
async fn persistent_crash_after_the_join_is_asserted_after_a_lagging_append_record() {
    persistent_crash_after_the_join(APPEND_RECORD_LAG).await;
}

async fn persistent_crash_after_the_join(append_record_lag: Duration) {
    persistent_crash_after_the_join_of(
        append_record_lag,
        typed_steer("crash", ConversationAppendRole::SystemNotice),
    )
    .await;
}

async fn persistent_crash_after_the_join_of(append_record_lag: Duration, steer: Input) {
    let store: Arc<dyn crate::store::RuntimeStore> =
        Arc::new(crate::store::InMemoryRuntimeStore::new());
    let crashed = DurableSteerRig::persistent(Arc::clone(&store)).await;
    crashed.script.lag_append_records(append_record_lag);
    let session_id = crashed.session_id.clone();
    let runtime_id = MeerkatMachine::logical_runtime_id(&session_id);
    let batch = crashed.start_busy_turn().await;
    let steer_id = steer.id().clone();
    crashed.admit(steer).await;
    crashed.wait_for_waiting_delivery().await;
    crashed.script.step(RunnerStep::BoundaryThenToolCalls);
    crashed
        .wait_for_phase(&steer_id, InputLifecycleState::Staged)
        .await;
    // The runner applied the append into the live image of the in-flight run.
    crashed.wait_for_applied_durable(1).await;
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
        Arc::new(
            MeerkatMachine::persistent_without_blobs(Arc::clone(&store))
                .expect("persistent machine"),
        ),
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

#[tokio::test]
async fn discarded_boundary_batch_preserves_order_and_excludes_unapplied_input() {
    let store: Arc<dyn crate::store::RuntimeStore> =
        Arc::new(crate::store::InMemoryRuntimeStore::new());
    let rig = DurableSteerRig::persistent(store).await;
    let batch = rig.start_busy_turn().await;
    let mut discarded_ids = Vec::new();
    for body in ["first application", "second application"] {
        let steer = typed_steer(body, ConversationAppendRole::SystemNotice);
        let input_id = steer.id().clone();
        rig.admit(steer).await;
        rig.wait_for_waiting_delivery().await;
        rig.script.step(RunnerStep::BoundaryThenToolCalls);
        rig.wait_for_phase(&input_id, InputLifecycleState::Staged)
            .await;
        discarded_ids.push(input_id);
    }
    let unapplied = typed_steer("not applied", ConversationAppendRole::SystemNotice);
    let unapplied_id = unapplied.id().clone();
    rig.admit(unapplied).await;
    rig.wait_for_waiting_delivery().await;
    rig.script.step(RunnerStep::FailDiscardingImage);
    rig.wait_for_apply_calls(2).await;
    assert_eq!(
        *rig.script.discarded.lock().unwrap(),
        vec![meerkat_core::event::BoundaryAppendsDiscarded {
            session_id: rig.session_id.clone(),
            run_id: rig.script.applied_runs.lock().unwrap()[0].clone(),
            input_ids: discarded_ids.clone(),
        }]
    );
    assert_eq!(rig.script.applied_durable(), discarded_ids);
    // Each queued input can take its own follow-up; extra script steps remain
    // inert if the existing batching policy combines any of them.
    for _ in 0..4 {
        rig.script.step(RunnerStep::Finish);
    }
    for input_id in discarded_ids.iter().chain(std::iter::once(&unapplied_id)) {
        rig.wait_for_phase(input_id, InputLifecycleState::Consumed)
            .await;
        assert_eq!(contributions(&rig.script.primitives(), input_id), 1);
    }
    rig.wait_for_phase(&batch, InputLifecycleState::Consumed)
        .await;
    assert_eq!(rig.script.discarded.lock().unwrap().len(), 1);
}

#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn boundary_discard_write_failure_emits_no_source_event() {
    use crate::store::RuntimeStore;
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(
        crate::store::SqliteRuntimeStore::new(directory.path().join("runtime.sqlite")).unwrap(),
    );
    let rig = DurableSteerRig::persistent(store.clone()).await;
    rig.start_busy_turn().await;
    let steer = typed_steer(
        "discard write failure",
        ConversationAppendRole::SystemNotice,
    );
    let input_id = steer.id().clone();
    rig.admit(steer).await;
    rig.wait_for_waiting_delivery().await;
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    rig.wait_for_phase(&input_id, InputLifecycleState::Staged)
        .await;
    let runtime_id = MeerkatMachine::logical_runtime_id(&rig.session_id);
    let connection = rusqlite::Connection::open(store.path()).unwrap();
    connection
        .execute_batch(&format!(
            "CREATE TRIGGER fail_discard_requeue BEFORE UPDATE ON runtime_input_states
         WHEN NEW.runtime_id = '{runtime_id}' AND NEW.input_id = '{input_id}'
         BEGIN SELECT RAISE(ABORT, 'boundary discard requeue failure'); END;"
        ))
        .unwrap();
    rig.script.step(RunnerStep::FailDiscardingImage);
    tokio::time::timeout(
        Duration::from_secs(5),
        rig.script.runtime_stopped.notified(),
    )
    .await
    .expect("persistence failure stops the exact executor");
    assert!(rig.script.discarded.lock().unwrap().is_empty());
    assert_eq!(rig.script.apply_calls.load(Ordering::SeqCst), 1);
    let row = store
        .load_input_state(&runtime_id, &input_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.seed.phase,
        InputLifecycleState::Staged,
        "failed requeue persistence must not project committed discard truth"
    );
    connection
        .execute_batch("DROP TRIGGER fail_discard_requeue;")
        .unwrap();
}

// ---------------------------------------------------------------------------
// Run-fenced Stop (`MeerkatMachine::stop_run`)
// ---------------------------------------------------------------------------

fn cancelled_terminal() -> Option<crate::input_state::InputTerminalOutcome> {
    Some(crate::input_state::InputTerminalOutcome::Abandoned {
        reason: crate::input_state::InputAbandonReason::Cancelled,
    })
}

fn queued_prompt(text: &str) -> Input {
    Input::Prompt(crate::input::PromptInput::new(text, None))
}

fn stopped_contributors(
    receipt: crate::run_stop::RunStopReceipt,
    expected_run: &RunId,
) -> Vec<crate::run_stop::RunStopContributor> {
    match receipt {
        crate::run_stop::RunStopReceipt::Stopped {
            run_id,
            contributors,
        } => {
            assert_eq!(&run_id, expected_run);
            contributors
        }
        other => panic!("expected a Stopped receipt, got {other:?}"),
    }
}

/// A stop that never returns is a failure, not a hang.
async fn stop_bounded(
    rig: &DurableSteerRig,
    run_id: &RunId,
    reason: &str,
) -> Result<crate::run_stop::RunStopReceipt, RuntimeDriverError> {
    tokio::time::timeout(
        Duration::from_secs(10),
        rig.adapter.stop_run(&rig.session_id, run_id, reason),
    )
    .await
    .expect("stop_run returns once every contributor is terminal")
}

/// Event-based "no successor" proof. A stopped contributor that re-entered a
/// lane would be staged no later than fresh queued work (a requeued steer has
/// Steer-lane priority and a replayed batch returns to the head of its lane),
/// so the next apply carrying exactly the sentinel proves none did.
async fn assert_next_run_carries_only_a_fresh_sentinel(rig: &DurableSteerRig) {
    let applied_before = rig.script.apply_calls.load(Ordering::SeqCst);
    let sentinel = queued_prompt("sentinel after the stop");
    let sentinel_id = sentinel.id().clone();
    rig.admit(sentinel).await;
    rig.wait_for_apply_calls(applied_before + 1).await;
    assert_eq!(
        rig.script.primitives()[applied_before],
        vec![sentinel_id.clone()],
        "the run after the stop carries no stopped contributor"
    );
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_phase(&sentinel_id, InputLifecycleState::Consumed)
        .await;
}

fn current_run(rig: &DurableSteerRig) -> RunId {
    rig.script
        .active_run
        .lock()
        .unwrap()
        .clone()
        .expect("a run is inside apply")
}

/// The reported sequence: A starts R, durable steer S joins R and is applied
/// into R's image, the host stops R, and the owning service discards the
/// cancelled image. S must be terminal with R, never requeued into a
/// successor provider request.
async fn stop_run_terminalizes_a_discarded_durable_join(
    rig: &DurableSteerRig,
) -> (InputId, InputId) {
    let batch = rig.start_busy_turn().await;
    let run_id = current_run(rig);
    let steer = typed_steer("steer joins R", ConversationAppendRole::User);
    let steer_id = steer.id().clone();
    rig.admit(steer).await;
    rig.wait_for_waiting_delivery().await;
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    rig.wait_for_phase(&steer_id, InputLifecycleState::Staged)
        .await;
    rig.wait_for_applied_durable(1).await;

    let receipt = stop_bounded(&rig, &run_id, "host stopped the selected run")
        .await
        .expect("stop the selected run");
    // The wire projection every surface serves: the batch contributor
    // receives the canonical cancellation completion, the joined steer the
    // runtime-termination carrier, and both commit a cancelled terminal.
    let wire = crate::run_stop_wire::wire_run_stop_receipt(&receipt).expect("wire receipt");
    match &wire {
        meerkat_contracts::WireRunStopReceipt::Stopped {
            run_id: wire_run,
            contributors,
        } => {
            assert_eq!(wire_run, &run_id.to_string());
            for contributor in contributors {
                assert_eq!(
                    contributor.terminal,
                    Some(meerkat_contracts::wire::runtime::WireInputTerminalOutcome::Cancelled),
                    "{contributor:?}"
                );
                let expected = if contributor.input_id == batch.to_string() {
                    meerkat_contracts::WireRunStopCompletion::Cancelled
                } else {
                    meerkat_contracts::WireRunStopCompletion::RuntimeTerminated
                };
                assert_eq!(contributor.completion, expected, "{contributor:?}");
            }
        }
        other => panic!("expected a Stopped wire receipt, got {other:?}"),
    }
    let contributors = stopped_contributors(receipt, &run_id);
    let mut ids = contributors
        .iter()
        .map(|contributor| contributor.input_id.clone())
        .collect::<Vec<_>>();
    ids.sort_by_key(ToString::to_string);
    let mut expected = vec![batch.clone(), steer_id.clone()];
    expected.sort_by_key(ToString::to_string);
    assert_eq!(ids, expected, "the receipt names exactly R's contributors");
    for contributor in &contributors {
        assert_eq!(
            contributor.terminal,
            cancelled_terminal(),
            "{contributor:?} terminalizes as cancelled with the stopped run"
        );
    }
    assert_eq!(
        rig.phase(&steer_id).await,
        Some(InputLifecycleState::Abandoned)
    );
    assert_eq!(
        rig.phase(&batch).await,
        Some(InputLifecycleState::Abandoned)
    );
    assert!(
        rig.script.discarded.lock().unwrap().is_empty(),
        "a stopped run's join is cancelled, not requeued as discarded"
    );
    assert_eq!(rig.script.interrupts.load(Ordering::SeqCst), 1);
    (batch, steer_id)
}

#[tokio::test]
async fn stop_run_terminalizes_a_discarded_durable_join_without_a_successor() {
    let rig = DurableSteerRig::ephemeral().await;
    let (_batch, steer_id) = stop_run_terminalizes_a_discarded_durable_join(&rig).await;
    assert!(rig.steer_queue().await.is_empty());
    assert!(rig.queue().await.is_empty());
    assert_eq!(
        rig.script.apply_calls.load(Ordering::SeqCst),
        1,
        "no successor provider request after the stop"
    );
    assert_next_run_carries_only_a_fresh_sentinel(&rig).await;
    assert_eq!(contributions(&rig.script.primitives(), &steer_id), 0);
}

#[tokio::test]
async fn persistent_stop_run_terminalizes_a_discarded_durable_join_without_a_successor() {
    let store: Arc<dyn crate::store::RuntimeStore> =
        Arc::new(crate::store::InMemoryRuntimeStore::new());
    let rig = DurableSteerRig::persistent(Arc::clone(&store)).await;
    let runtime_id = MeerkatMachine::logical_runtime_id(&rig.session_id);
    let (batch, steer_id) = stop_run_terminalizes_a_discarded_durable_join(&rig).await;
    for input_id in [&batch, &steer_id] {
        let row = store
            .load_input_state(&runtime_id, input_id)
            .await
            .expect("load stopped row")
            .expect("stopped row persisted");
        assert_eq!(row.seed.phase, InputLifecycleState::Abandoned);
        assert_eq!(row.seed.terminal_outcome, cancelled_terminal());
    }
    assert_eq!(rig.script.apply_calls.load(Ordering::SeqCst), 1);
    assert_next_run_carries_only_a_fresh_sentinel(&rig).await;
    assert_eq!(contributions(&rig.script.primitives(), &steer_id), 0);
}

/// Unrelated queued input B is not a contributor of the stopped run: it keeps
/// its place and runs next, alone. A late Stop(R) then touches neither B nor
/// the newer run.
#[tokio::test]
async fn stop_run_preserves_unrelated_queued_input_and_a_late_stop_is_harmless() {
    let rig = DurableSteerRig::ephemeral().await;
    let batch = rig.start_busy_turn().await;
    let stopped_run = current_run(&rig);
    let steer = typed_steer("joins R", ConversationAppendRole::SystemNotice);
    let steer_id = steer.id().clone();
    rig.admit(steer).await;
    rig.wait_for_waiting_delivery().await;
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    rig.wait_for_phase(&steer_id, InputLifecycleState::Staged)
        .await;
    rig.wait_for_applied_durable(1).await;
    let unrelated = queued_prompt("unrelated queued work B");
    let unrelated_id = unrelated.id().clone();
    let unrelated_completion = rig.admit(unrelated).await.expect("B completion handle");
    assert_eq!(
        rig.phase(&unrelated_id).await,
        Some(InputLifecycleState::Queued)
    );

    let contributors = stopped_contributors(
        stop_bounded(&rig, &stopped_run, "stop R")
            .await
            .expect("stop R"),
        &stopped_run,
    );
    assert!(
        contributors
            .iter()
            .all(|contributor| contributor.input_id != unrelated_id),
        "B is not a contributor of R"
    );
    assert!(
        contributors
            .iter()
            .any(|contributor| contributor.input_id == batch)
    );

    // B runs next, alone.
    rig.wait_for_apply_calls(2).await;
    let newer_run = current_run(&rig);
    assert_ne!(newer_run, stopped_run);
    assert_eq!(rig.script.primitives()[1], vec![unrelated_id.clone()]);

    // A late Stop(R) is a no-op for B and the newer run.
    let late = stop_bounded(&rig, &stopped_run, "late stop R")
        .await
        .expect("late stop");
    match late {
        crate::run_stop::RunStopReceipt::NotCurrent {
            run_id,
            current_run_id,
        } => {
            assert_eq!(run_id, stopped_run);
            assert_eq!(current_run_id, Some(newer_run.clone()));
        }
        other => panic!("late stop must be NotCurrent, got {other:?}"),
    }
    assert_eq!(
        rig.script.interrupts.load(Ordering::SeqCst),
        1,
        "the late stop never interrupts the newer run"
    );
    rig.script.step(RunnerStep::Finish);
    let outcome = tokio::time::timeout(Duration::from_secs(5), unrelated_completion.wait())
        .await
        .expect("B resolves")
        .expect("B completion");
    assert!(
        !matches!(
            outcome,
            crate::completion::CompletionOutcome::Cancelled
                | crate::completion::CompletionOutcome::RuntimeTerminated { .. }
        ),
        "{outcome:?}"
    );
    rig.wait_for_phase(&unrelated_id, InputLifecycleState::Consumed)
        .await;
    assert_next_run_carries_only_a_fresh_sentinel(&rig).await;
    assert_eq!(contributions(&rig.script.primitives(), &steer_id), 0);
}

/// The stop is linearized before a racing steer reaches its boundary: the
/// generated join refuses the stopped run, so S is never R's contributor. It
/// stays ordinary queued work and runs exactly once after R.
#[tokio::test]
async fn steer_that_reaches_its_boundary_after_the_stop_never_joins_the_stopped_run() {
    let rig = DurableSteerRig::ephemeral().await;
    let batch = rig.start_busy_turn().await;
    let stopped_run = current_run(&rig);
    // Hold the run open after the interrupt so the boundary can race it.
    *rig.script.interrupt_step.lock().unwrap() = None;
    let steer = typed_steer("races the stop", ConversationAppendRole::SystemNotice);
    let steer_id = steer.id().clone();
    rig.admit(steer).await;
    rig.wait_for_waiting_delivery().await;

    let adapter = Arc::clone(&rig.adapter);
    let session_id = rig.session_id.clone();
    let run_for_stop = stopped_run.clone();
    let stop = tokio::spawn(async move {
        adapter
            .stop_run(&session_id, &run_for_stop, "stop before the join")
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while rig.script.interrupts.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("stop dispatched its interrupt");

    // The runner reaches the boundary after the stop: the join is refused.
    // The runner parks at the boundary for every registered preparation, so
    // the join decision is made before the step completes.
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    assert_eq!(
        rig.phase(&steer_id).await,
        Some(InputLifecycleState::Queued),
        "a stopped run admits no durable join"
    );
    assert!(rig.script.applied_durable().is_empty());

    rig.script.step(RunnerStep::CancelDiscardingImage);
    let contributors = stopped_contributors(
        tokio::time::timeout(Duration::from_secs(5), stop)
            .await
            .expect("stop returns")
            .expect("stop task")
            .expect("stop"),
        &stopped_run,
    );
    assert_eq!(
        contributors
            .iter()
            .map(|contributor| contributor.input_id.clone())
            .collect::<Vec<_>>(),
        vec![batch]
    );

    rig.wait_for_apply_calls(2).await;
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_phase(&steer_id, InputLifecycleState::Consumed)
        .await;
    assert_eq!(
        contributions(&rig.script.primitives(), &steer_id),
        1,
        "the unjoined steer is ordinary follow-up work, delivered exactly once"
    );
}

/// Stop racing the join boundary itself: whichever the generated machine
/// linearizes first decides, and S is either a terminal contributor of R or
/// ordinary follow-up work delivered once, never both and never lost.
///
/// Which side wins is up to the scheduler (the launch order alternates, but
/// the machine gate decides), so the loop records the outcome of each
/// iteration but does not require both: each side has its own
/// deterministic test
/// (`stop_run_terminalizes_a_discarded_durable_join_without_a_successor` and
/// `steer_that_reaches_its_boundary_after_the_stop_never_joins_the_stopped_run`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_racing_the_join_boundary_never_both_cancels_and_redelivers_the_steer() {
    const ITERATIONS: usize = 8;
    let mut joined_then_cancelled = 0;
    let mut refused_then_delivered = 0;
    for iteration in 0..ITERATIONS {
        let rig = DurableSteerRig::ephemeral().await;
        rig.start_busy_turn().await;
        let stopped_run = current_run(&rig);
        *rig.script.interrupt_step.lock().unwrap() = None;
        let steer = typed_steer("race", ConversationAppendRole::SystemNotice);
        let steer_id = steer.id().clone();
        rig.admit(steer).await;
        rig.wait_for_waiting_delivery().await;

        // Alternate which side is launched first so both linearizations
        // are exercised on the multi-threaded runtime.
        let boundary_first = iteration % 2 == 1;
        if boundary_first {
            rig.script.step(RunnerStep::BoundaryThenToolCalls);
        }
        let adapter = Arc::clone(&rig.adapter);
        let session_id = rig.session_id.clone();
        let run_for_stop = stopped_run.clone();
        let stop = tokio::spawn(async move {
            adapter
                .stop_run(&session_id, &run_for_stop, "racing stop")
                .await
        });
        if !boundary_first {
            rig.script.step(RunnerStep::BoundaryThenToolCalls);
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            while rig.script.interrupts.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("stop dispatched its interrupt");
        rig.script.step(RunnerStep::CancelDiscardingImage);
        let contributors = stopped_contributors(
            tokio::time::timeout(Duration::from_secs(5), stop)
                .await
                .expect("stop returns")
                .expect("stop task")
                .expect("stop"),
            &stopped_run,
        );
        let joined = contributors
            .iter()
            .find(|contributor| contributor.input_id == steer_id);
        if let Some(joined) = joined {
            assert_eq!(joined.terminal, cancelled_terminal());
            assert_next_run_carries_only_a_fresh_sentinel(&rig).await;
            assert_eq!(contributions(&rig.script.primitives(), &steer_id), 0);
            joined_then_cancelled += 1;
        } else {
            rig.wait_for_apply_calls(2).await;
            rig.script.step(RunnerStep::Finish);
            rig.wait_for_phase(&steer_id, InputLifecycleState::Consumed)
                .await;
            assert_eq!(contributions(&rig.script.primitives(), &steer_id), 1);
            refused_then_delivered += 1;
        }
    }
    eprintln!(
        "stop/join race: {joined_then_cancelled} joined-then-cancelled, \
         {refused_then_delivered} refused-then-delivered"
    );
    assert_eq!(joined_then_cancelled + refused_then_delivered, ITERATIONS);
}

/// A retryable failure racing the stop never replays the stopped batch: the
/// generated staged-rollback resolution terminalizes it as cancelled.
#[tokio::test]
async fn stop_run_never_replays_the_batch_when_the_stopped_run_fails() {
    let rig = DurableSteerRig::ephemeral().await;
    let batch = rig.start_busy_turn().await;
    let stopped_run = current_run(&rig);
    *rig.script.interrupt_step.lock().unwrap() = Some(RunnerStep::FailKeepingImage);
    let contributors = stopped_contributors(
        stop_bounded(&rig, &stopped_run, "stop R")
            .await
            .expect("stop R"),
        &stopped_run,
    );
    assert_eq!(contributors.len(), 1);
    assert_eq!(contributors[0].input_id, batch);
    assert_eq!(contributors[0].terminal, cancelled_terminal());
    assert_eq!(
        rig.script.apply_calls.load(Ordering::SeqCst),
        1,
        "the failed stopped run is not retried"
    );
    assert_next_run_carries_only_a_fresh_sentinel(&rig).await;
    assert_eq!(contributions(&rig.script.primitives(), &batch), 1);
}

/// Once the stop is committed, a failed interrupt dispatch does not fail the
/// stop: the run still ends as a stopped run, and the contributors' terminals
/// are the outcome.
#[tokio::test]
async fn committed_stop_waits_for_contributor_terminals_when_the_interrupt_fails() {
    let rig = DurableSteerRig::ephemeral().await;
    let batch = rig.start_busy_turn().await;
    let stopped_run = current_run(&rig);
    let steer = typed_steer(
        "joined before a failing interrupt",
        ConversationAppendRole::User,
    );
    let steer_id = steer.id().clone();
    rig.admit(steer).await;
    rig.wait_for_waiting_delivery().await;
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    rig.wait_for_phase(&steer_id, InputLifecycleState::Staged)
        .await;
    rig.wait_for_applied_durable(1).await;
    rig.script.interrupt_fails.store(true, Ordering::SeqCst);

    let adapter = Arc::clone(&rig.adapter);
    let session_id = rig.session_id.clone();
    let run_for_stop = stopped_run.clone();
    let stop = tokio::spawn(async move {
        adapter
            .stop_run(&session_id, &run_for_stop, "stop with a failing interrupt")
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while rig.script.interrupts.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("stop dispatched its interrupt");
    // The run ends on its own, discarding its image, after the failed
    // interrupt; the committed stop still governs its terminal.
    rig.script.step(RunnerStep::CancelDiscardingImage);
    let contributors = stopped_contributors(
        tokio::time::timeout(Duration::from_secs(10), stop)
            .await
            .expect("stop returns")
            .expect("stop task")
            .expect("a committed stop does not fail on its interrupt dispatch"),
        &stopped_run,
    );
    let mut ids = contributors
        .iter()
        .map(|contributor| contributor.input_id.clone())
        .collect::<Vec<_>>();
    ids.sort_by_key(ToString::to_string);
    let mut expected = vec![batch, steer_id.clone()];
    expected.sort_by_key(ToString::to_string);
    assert_eq!(ids, expected);
    for contributor in &contributors {
        assert_eq!(
            contributor.terminal,
            cancelled_terminal(),
            "{contributor:?}"
        );
    }
    assert_next_run_carries_only_a_fresh_sentinel(&rig).await;
    assert_eq!(contributions(&rig.script.primitives(), &steer_id), 0);
}

/// Behaviour unchanged for the plain exact-run interrupt: without a Stop, a
/// discarded join still returns to its lane and is delivered by exactly one
/// follow-up turn.
#[tokio::test]
async fn plain_exact_run_interrupt_still_requeues_a_discarded_join() {
    let rig = DurableSteerRig::ephemeral().await;
    let batch = rig.start_busy_turn().await;
    let run_id = current_run(&rig);
    let steer = typed_steer("interrupted, not stopped", ConversationAppendRole::User);
    let steer_id = steer.id().clone();
    rig.admit(steer).await;
    rig.wait_for_waiting_delivery().await;
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    rig.wait_for_phase(&steer_id, InputLifecycleState::Staged)
        .await;
    rig.wait_for_applied_durable(1).await;

    assert!(
        rig.adapter
            .hard_cancel_run_if_current(&rig.session_id, &run_id, "plain interrupt")
            .await
            .expect("plain exact-run interrupt")
    );
    rig.wait_for_phase(&batch, InputLifecycleState::Abandoned)
        .await;
    rig.wait_for_apply_calls(2).await;
    assert_eq!(
        rig.script.primitives()[1],
        vec![steer_id.clone()],
        "the discarded join takes its one follow-up turn"
    );
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_phase(&steer_id, InputLifecycleState::Consumed)
        .await;
    assert_eq!(
        *rig.script.discarded.lock().unwrap(),
        vec![meerkat_core::event::BoundaryAppendsDiscarded {
            session_id: rig.session_id.clone(),
            run_id,
            input_ids: vec![steer_id],
        }]
    );
}

#[tokio::test]
async fn stop_run_of_an_unknown_run_is_not_current() {
    let rig = DurableSteerRig::ephemeral().await;
    rig.start_busy_turn().await;
    let current = current_run(&rig);
    let unknown = RunId::new();
    match stop_bounded(&rig, &unknown, "stale")
        .await
        .expect("stale stop")
    {
        crate::run_stop::RunStopReceipt::NotCurrent {
            run_id,
            current_run_id,
        } => {
            assert_eq!(run_id, unknown);
            assert_eq!(current_run_id, Some(current));
        }
        other => panic!("expected NotCurrent, got {other:?}"),
    }
    assert_eq!(rig.script.interrupts.load(Ordering::SeqCst), 0);
}

fn owner_context(text: &str) -> Vec<meerkat_core::lifecycle::TurnRequestContext> {
    vec![meerkat_core::lifecycle::TurnRequestContext::new(text.to_string()).expect("context")]
}

/// Owner request-only context (a live delegation steer) delivered straight
/// into the running turn: it waits across the closed window, lands at the
/// next boundary, is recorded on that boundary's durable receipt in the
/// run's dense sequence, and never becomes a turn of its own.
#[tokio::test]
async fn owner_context_lands_at_the_next_boundary_with_a_durable_receipt() {
    let store: Arc<dyn crate::store::RuntimeStore> =
        Arc::new(crate::store::InMemoryRuntimeStore::new());
    let rig = DurableSteerRig::persistent(Arc::clone(&store)).await;
    let runtime_id = MeerkatMachine::logical_runtime_id(&rig.session_id);
    let busy = rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenStream)
        .await;
    let adapter = Arc::clone(&rig.adapter);
    let session_id = rig.session_id.clone();
    let delivery = tokio::spawn(async move {
        adapter
            .deliver_live_owner_request_context(
                &session_id,
                "live-delegation-steer:continuation-1",
                owner_context("into notes dot md"),
            )
            .await
    });
    rig.wait_for_waiting_delivery().await;
    // The model returned tool calls: the next boundary opens and the waiting
    // owner context attaches to it.
    rig.script.step(RunnerStep::OpenNextBoundary);
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    let delivered = delivery.await.expect("delivery task").expect("delivery");
    let crate::live_execution::LiveOwnerContextDelivery::Delivered {
        run_id,
        boundary_sequence,
    } = delivered
    else {
        panic!("delivered at the next boundary, got {delivered:?}");
    };
    let receipt = store
        .load_boundary_receipt(&runtime_id, &run_id, boundary_sequence)
        .await
        .expect("load receipt")
        .expect("the owner contribution is on a durable runtime receipt");
    assert_eq!(
        receipt.owner_contributions,
        vec!["live-delegation-steer:continuation-1".to_string()]
    );
    assert!(receipt.contributing_input_ids.is_empty());
    rig.script.step(RunnerStep::Finish);
    // The terminal receipt is the dense successor of the owner receipt: the
    // run still commits.
    rig.wait_for_phase(&busy, InputLifecycleState::Consumed)
        .await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while rig.script.request_only_taken.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the runner took the owner context");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        rig.script.apply_calls.load(Ordering::SeqCst),
        1,
        "owner context never runs as a turn of its own"
    );
}

/// The late window: the worker is on its final model call when the
/// continuation arrives. The run ends without another boundary, so the
/// context is NotDelivered, nothing is recorded, and no follow-up turn (no
/// extra row, no orphan reply) ever runs.
#[tokio::test]
async fn owner_context_is_not_delivered_when_the_run_ends_first_and_never_runs() {
    let store: Arc<dyn crate::store::RuntimeStore> =
        Arc::new(crate::store::InMemoryRuntimeStore::new());
    let rig = DurableSteerRig::persistent(Arc::clone(&store)).await;
    let runtime_id = MeerkatMachine::logical_runtime_id(&rig.session_id);
    rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenStream)
        .await;
    let adapter = Arc::clone(&rig.adapter);
    let session_id = rig.session_id.clone();
    let delivery = tokio::spawn(async move {
        adapter
            .deliver_live_owner_request_context(
                &session_id,
                "live-delegation-steer:late",
                owner_context("too late"),
            )
            .await
    });
    rig.wait_for_waiting_delivery().await;
    let run_id = rig
        .script
        .active_run
        .lock()
        .unwrap()
        .clone()
        .expect("active run");
    // The model returns final text: the run ends with no further boundary.
    rig.script.step(RunnerStep::Finish);
    assert_eq!(
        delivery.await.expect("delivery task").expect("delivery"),
        crate::live_execution::LiveOwnerContextDelivery::NotDelivered
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        rig.script.apply_calls.load(Ordering::SeqCst),
        1,
        "a missed continuation never seeds a turn"
    );
    assert!(rig.script.request_only_taken.lock().unwrap().is_empty());
    let receipts = store
        .load_committed_boundary_receipts(&runtime_id, &run_id)
        .await
        .expect("load receipts");
    assert!(
        receipts
            .iter()
            .all(|receipt| receipt.owner_contributions.is_empty()),
        "nothing was recorded for an undelivered contribution"
    );
}

#[tokio::test]
async fn owner_context_without_an_active_run_is_not_delivered() {
    let rig = DurableSteerRig::ephemeral().await;
    assert_eq!(
        rig.adapter
            .deliver_live_owner_request_context(
                &rig.session_id,
                "live-delegation-steer:idle",
                owner_context("nobody listening"),
            )
            .await
            .expect("delivery"),
        crate::live_execution::LiveOwnerContextDelivery::NotDelivered
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(rig.script.apply_calls.load(Ordering::SeqCst), 0);
}

/// A context bound to one run (a live delegation steer authorized for the
/// delegation's run) never reaches a later run of the same session: an
/// existing member that outlives the delegation starts its next turn, and
/// the stale steer is NotDelivered instead of landing in that turn.
#[tokio::test]
async fn owner_context_bound_to_an_ended_run_never_reaches_the_next_run() {
    let store: Arc<dyn crate::store::RuntimeStore> =
        Arc::new(crate::store::InMemoryRuntimeStore::new());
    let rig = DurableSteerRig::persistent(Arc::clone(&store)).await;
    let first = rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenStream)
        .await;
    let first_run = rig
        .adapter
        .live_owner_current_run_id(&rig.session_id)
        .await
        .expect("the first run is active");
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_phase(&first, InputLifecycleState::Consumed)
        .await;

    let second = rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenStream)
        .await;
    let second_run = rig
        .adapter
        .live_owner_current_run_id(&rig.session_id)
        .await
        .expect("the second run is active");
    assert_ne!(second_run, first_run);
    let adapter = Arc::clone(&rig.adapter);
    let session_id = rig.session_id.clone();
    let delivery = tokio::spawn(async move {
        adapter
            .deliver_live_owner_request_context_into_run(
                &session_id,
                &first_run,
                "live-delegation-steer:stale",
                owner_context("meant for the first run"),
            )
            .await
    });
    // Let the delivery settle before the run moves: bound to the ended run
    // it returns at once; an unbound one would register for the second
    // run's next boundary.
    tokio::time::timeout(Duration::from_secs(5), async {
        while !delivery.is_finished() && !rig.state.has_waiting_delivery_for_test() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the delivery returned or registered");
    // The second run opens a real next boundary: an unbound context would
    // attach to it.
    rig.script.step(RunnerStep::OpenNextBoundary);
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_phase(&second, InputLifecycleState::Consumed)
        .await;
    assert_eq!(
        delivery.await.expect("delivery task").expect("delivery"),
        crate::live_execution::LiveOwnerContextDelivery::NotDelivered
    );
    assert!(
        rig.script.request_only_taken.lock().unwrap().is_empty(),
        "the stale steer never reached the second run"
    );
}

/// One content steer joins the running run at the next boundary and is
/// consumed with the run's commit: written once, never by a follow-up turn.
async fn content_steer_joins_the_running_run_and_is_consumed_with_it(steer: Input) {
    let rig = DurableSteerRig::ephemeral().await;
    let batch = rig.start_busy_turn().await;
    let steer_id = steer.id().clone();
    let completion = rig.admit(steer).await.expect("completion handle");
    rig.wait_for_waiting_delivery().await;
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    rig.wait_for_phase(&steer_id, InputLifecycleState::Staged)
        .await;
    rig.wait_for_applied_durable(1).await;
    assert_eq!(rig.script.applied_durable(), vec![steer_id.clone()]);
    let mut waiter = Box::pin(completion.wait());
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut waiter)
            .await
            .is_err(),
        "the joined steer is consumed with the run, not at the boundary"
    );
    rig.script.step(RunnerStep::Finish);
    tokio::time::timeout(Duration::from_secs(5), waiter)
        .await
        .expect("the steer completes with the run")
        .expect("completion outcome");
    rig.wait_for_phase(&steer_id, InputLifecycleState::Consumed)
        .await;
    rig.wait_for_phase(&batch, InputLifecycleState::Consumed)
        .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(rig.script.apply_calls.load(Ordering::SeqCst), 1);
    assert_eq!(contributions(&rig.script.primitives(), &steer_id), 0);
}

#[tokio::test]
async fn peer_message_steer_joins_the_running_run_and_is_consumed_with_it() {
    content_steer_joins_the_running_run_and_is_consumed_with_it(peer_steer("peer message")).await;
}

#[tokio::test]
async fn peer_request_steer_joins_the_running_run_and_is_consumed_with_it() {
    content_steer_joins_the_running_run_and_is_consumed_with_it(peer_request_steer("peer request"))
        .await;
}

#[tokio::test]
async fn peer_lifecycle_steer_joins_the_running_run_and_is_consumed_with_it() {
    content_steer_joins_the_running_run_and_is_consumed_with_it(peer_lifecycle_steer(
        "peer lifecycle",
    ))
    .await;
}

#[tokio::test]
async fn text_prompt_steer_joins_the_running_run_and_is_consumed_with_it() {
    content_steer_joins_the_running_run_and_is_consumed_with_it(text_prompt_steer(
        "also check the second file",
    ))
    .await;
}

#[tokio::test]
async fn external_event_steer_joins_the_running_run_and_is_consumed_with_it() {
    content_steer_joins_the_running_run_and_is_consumed_with_it(external_event_steer(
        "deploy finished",
    ))
    .await;
}

#[tokio::test]
async fn local_flow_step_steer_joins_the_running_run_and_is_consumed_with_it() {
    content_steer_joins_the_running_run_and_is_consumed_with_it(flow_step_steer("next step")).await;
}

/// A FollowUpOnly steer never registers at a boundary of the running run
/// and is delivered by exactly one follow-up turn.
async fn follow_up_only_steer_never_joins_and_takes_one_follow_up(
    rig: DurableSteerRig,
    steer: Input,
) {
    rig.start_busy_turn().await;
    let steer_id = steer.id().clone();
    rig.admit(steer).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !rig.state.has_waiting_delivery_for_test(),
        "a follow-up-only delivery never registers at a boundary"
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
async fn directed_peer_steer_never_joins_and_takes_one_follow_up() {
    follow_up_only_steer_never_joins_and_takes_one_follow_up(
        DurableSteerRig::ephemeral_with_terminal_publication().await,
        directed_peer_steer("directed"),
    )
    .await;
}

#[tokio::test]
async fn directed_flow_step_steer_never_joins_and_takes_one_follow_up() {
    follow_up_only_steer_never_joins_and_takes_one_follow_up(
        DurableSteerRig::ephemeral_with_terminal_publication().await,
        directed_flow_step_steer("directed step"),
    )
    .await;
}

#[tokio::test]
async fn model_routed_prompt_steer_never_joins_and_takes_one_follow_up() {
    let routed = Input::Prompt(crate::input::PromptInput::new(
        "routed",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                model: Some(meerkat_core::lifecycle::run_primitive::ModelId::new(
                    "claude-sonnet-4-5",
                )),
                ..steer_metadata()
            },
        ),
    ));
    follow_up_only_steer_never_joins_and_takes_one_follow_up(
        DurableSteerRig::ephemeral().await,
        routed,
    )
    .await;
}

#[tokio::test]
async fn persistent_peer_steer_applied_then_image_discarded_is_redelivered_exactly_once() {
    let store: Arc<dyn crate::store::RuntimeStore> =
        Arc::new(crate::store::InMemoryRuntimeStore::new());
    let rig = DurableSteerRig::persistent(Arc::clone(&store)).await;
    let runtime_id = MeerkatMachine::logical_runtime_id(&rig.session_id);
    let steer_id = applied_steer_then_image_discarded_is_redelivered_exactly_once(
        rig,
        peer_steer("discarded"),
    )
    .await;
    let row = store
        .load_input_state(&runtime_id, &steer_id)
        .await
        .expect("load consumed row")
        .expect("consumed row persisted");
    assert_eq!(row.seed.phase, InputLifecycleState::Consumed);
}

/// A peer steer that joined its run before the process crashed is recovered
/// from its stored row by the generated recovery and join-resolution
/// transitions: the crashed join resolves into exactly one follow-up turn,
/// with no refusal.
#[tokio::test]
async fn persistent_crash_after_a_peer_steer_join_recovers_for_exactly_one_follow_up() {
    persistent_crash_after_the_join_of(Duration::ZERO, peer_steer("crash")).await;
}

/// A peer message from a canonical sender with a delivery id carries its
/// reply capability into the durable boundary delivery the runtime
/// prepares; a joined prompt steer carries none.
#[tokio::test]
async fn joined_peer_message_carries_its_reply_capability_into_the_boundary() {
    let rig = DurableSteerRig::ephemeral().await;
    rig.start_busy_turn().await;
    let sender = meerkat_core::comms::PeerId::new();
    let delivery = uuid::Uuid::new_v4();
    let mut header = steer_header(crate::input::InputOrigin::Peer {
        peer_id: sender.to_string(),
        display_identity: Some("sender-member".into()),
        runtime_id: None,
    });
    header.correlation_id = Some(crate::identifiers::CorrelationId(delivery));
    let peer = Input::Peer(crate::input::PeerInput {
        directed_interaction_id: None,
        objective_id: None,
        system_prompts: Vec::new(),
        injected_context: Vec::new(),
        sender_taint: None,
        header,
        convention: Some(crate::input::PeerConvention::Message),
        content: "answer this".into(),
        payload: None,
        handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
        in_reply_to: None,
    });
    let expected = crate::input::peer_reply_capability(&peer).expect("a reply capability");
    assert_eq!(expected.in_reply_to, meerkat_core::InteractionId(delivery));
    assert_eq!(expected.peer_id, sender);
    assert_eq!(
        expected.kind,
        meerkat_core::comms::PeerReplyDeliveryKind::Message
    );
    let peer_id = peer.id().clone();
    rig.admit(peer).await;
    rig.wait_for_waiting_delivery().await;
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    rig.wait_for_phase(&peer_id, InputLifecycleState::Staged)
        .await;

    let prompt = text_prompt_steer("plain text steer");
    let prompt_id = prompt.id().clone();
    rig.admit(prompt).await;
    rig.wait_for_waiting_delivery().await;
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    rig.wait_for_phase(&prompt_id, InputLifecycleState::Staged)
        .await;
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_phase(&prompt_id, InputLifecycleState::Consumed)
        .await;

    assert_eq!(
        rig.script.prepared_reply_capabilities(),
        vec![vec![expected], Vec::new()],
        "the peer message's boundary carries its capability, the prompt's carries none"
    );
}

/// Many durable peer steers admitted while the running turn opens no
/// boundary do not wait on one another to be admitted: every admission
/// returns promptly, one delivery waits for a boundary, the steer lane holds
/// all of them in arrival order, and when the run ends each is consumed
/// exactly once, in arrival order, by the follow-up turn.
#[tokio::test]
async fn twenty_five_durable_peer_steers_admit_promptly_and_drain_in_order_after_the_run() {
    const FAN_IN: usize = 25;
    let rig = DurableSteerRig::ephemeral().await;
    let batch = rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenStream)
        .await;

    let adapter = Arc::clone(&rig.adapter);
    let session_id = rig.session_id.clone();
    let accepts = (0..FAN_IN).map(|index| {
        let adapter = Arc::clone(&adapter);
        let session_id = session_id.clone();
        tokio::spawn(async move {
            let steer = peer_steer(&format!("fan-in report {index}"));
            let id = steer.id().clone();
            let (outcome, _completion) = adapter
                .accept_input_with_completion(&session_id, steer)
                .await
                .expect("durable peer steer admitted");
            assert!(outcome.is_accepted());
            id
        })
    });
    let admitted = tokio::time::timeout(Duration::from_secs(5), futures::future::join_all(accepts))
        .await
        .expect("a waiting durable delivery must not hold up later admissions");
    let admitted: std::collections::HashSet<InputId> = admitted
        .into_iter()
        .map(|joined| joined.expect("admission task"))
        .collect();
    assert_eq!(admitted.len(), FAN_IN);

    rig.wait_for_waiting_delivery().await;
    let arrival: Vec<InputId> = rig
        .adapter
        .meerkat_machine_spine_snapshot(&rig.session_id)
        .await
        .expect("spine snapshot")
        .inputs
        .admission_order
        .iter()
        .map(|admission| admission.input_id.clone())
        .filter(|input_id| admitted.contains(input_id))
        .collect();
    assert_eq!(arrival.len(), FAN_IN);
    assert_eq!(
        rig.steer_queue().await,
        arrival,
        "the steer lane holds every waiting steer in arrival order"
    );

    rig.script.step(RunnerStep::Finish);
    rig.wait_for_apply_calls(2).await;
    rig.script.step(RunnerStep::Finish);
    for input_id in &arrival {
        rig.wait_for_phase(input_id, InputLifecycleState::Consumed)
            .await;
    }
    rig.wait_for_phase(&batch, InputLifecycleState::Consumed)
        .await;
    let primitives = rig.script.primitives();
    for input_id in &arrival {
        assert_eq!(
            contributions(&primitives, input_id),
            1,
            "each steer is consumed exactly once"
        );
    }
    let follow_up: Vec<InputId> = primitives
        .iter()
        .flatten()
        .filter(|input_id| admitted.contains(input_id))
        .cloned()
        .collect();
    assert_eq!(
        follow_up, arrival,
        "the follow-up takes them in arrival order"
    );
    assert!(
        rig.script.applied_durable().is_empty(),
        "no boundary opened"
    );
}

/// The most inputs one run's commit names: its batch and every durable Steer
/// it took in. It is the terminal-completion batch bound.
const RUN_CONTRIBUTOR_BOUND: usize = 256;

/// A queue-lane external event, one of a run's non-prompt batch.
fn queued_batch_event(index: usize) -> Input {
    Input::ExternalEvent(crate::input::ExternalEventInput {
        header: steer_header(crate::input::InputOrigin::External {
            source_name: "webhook".into(),
        }),
        event_type: "batch".into(),
        payload: serde_json::json!({ "index": index }),
        blocks: None,
        handling_mode: meerkat_core::types::HandlingMode::Queue,
        render_metadata: None,
        objective_id: None,
    })
}

/// A run's commit names its batch and every durable Steer that joined it, and
/// one terminal-completion batch names at most 256 inputs. So a run takes a
/// durable join only while its batch plus its joins stay within 256: the join
/// that would pass that is refused, takes the queued fallback, and is
/// delivered by the next run. Nothing leaves the registration reload-required.
async fn durable_steer_joins_up_to_the_contributor_bound(batch: usize) {
    assert!(batch <= RUN_CONTRIBUTOR_BOUND);
    let store: Arc<dyn crate::store::RuntimeStore> =
        Arc::new(crate::store::InMemoryRuntimeStore::new());
    let rig = DurableSteerRig::persistent(store).await;
    let assert_ready = |context: &'static str| {
        let adapter = Arc::clone(&rig.adapter);
        let session_id = rig.session_id.clone();
        async move {
            let required = adapter.durability_reload_required(&session_id).await;
            assert!(
                required.is_none(),
                "{context}: the registration became reload-required: {required:?}"
            );
        }
    };

    // One run takes the whole batch: run starts are held while it queues.
    rig.adapter
        .hold_run_starts(&rig.session_id, crate::RunStartHoldReason::MobStop)
        .await
        .expect("hold run starts");
    let mut parks = rig.adapter.run_start_held_parks();
    let mut batch_ids = Vec::with_capacity(batch);
    let mut batch_waiters = Vec::with_capacity(batch);
    for index in 0..batch {
        let event = queued_batch_event(index);
        batch_ids.push(event.id().clone());
        batch_waiters.push(rig.admit(event).await.expect("batch waiter"));
    }
    tokio::time::timeout(Duration::from_secs(30), parks.wait_for(|parks| *parks >= 1))
        .await
        .expect("the runtime loop parks on the hold")
        .expect("park signal");
    rig.adapter
        .release_run_starts(&rig.session_id, crate::RunStartHoldReason::MobStop)
        .await
        .expect("release run starts");
    rig.wait_for_apply_calls(1).await;
    assert_eq!(
        rig.script.primitives(),
        vec![batch_ids.clone()],
        "the first run stages the whole batch"
    );

    // Up to the bound, each durable Steer joins the running run.
    let mut joined = Vec::new();
    let mut joined_waiters = Vec::new();
    for index in batch..RUN_CONTRIBUTOR_BOUND {
        let steer = typed_steer(
            &format!("joins as contributor {}", index + 1),
            ConversationAppendRole::SystemNotice,
        );
        let steer_id = steer.id().clone();
        joined_waiters.push(rig.admit(steer).await.expect("joined waiter"));
        rig.wait_for_waiting_delivery().await;
        rig.script.step(RunnerStep::BoundaryThenToolCalls);
        rig.wait_for_phase(&steer_id, InputLifecycleState::Staged)
            .await;
        joined.push(steer_id);
    }
    rig.wait_for_applied_durable(joined.len()).await;
    assert_eq!(rig.script.applied_durable(), joined);

    // The next one would pass the bound: its join is refused and it keeps
    // its steer-lane follow-up.
    let refused = typed_steer(
        "would be contributor 257",
        ConversationAppendRole::SystemNotice,
    );
    let refused_id = refused.id().clone();
    let refused_waiter = rig.admit(refused).await.expect("refused waiter");
    rig.wait_for_waiting_delivery().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    assert_eq!(
        rig.phase(&refused_id).await,
        Some(InputLifecycleState::Queued),
        "a join past the bound is refused and the input stays queued"
    );
    assert!(
        rig.steer_queue().await.contains(&refused_id),
        "the refused input keeps its steer-lane follow-up"
    );
    assert_eq!(
        rig.script.applied_durable(),
        joined,
        "the refused input was not appended to the running turn"
    );
    assert_ready("after the refused join").await;

    // The run commits its batch and its joins: exactly the bound or fewer.
    rig.script.step(RunnerStep::Finish);
    for input_id in batch_ids.iter().chain(joined.iter()) {
        rig.wait_for_phase(input_id, InputLifecycleState::Consumed)
            .await;
    }
    for waiter in batch_waiters.into_iter().chain(joined_waiters) {
        let outcome = tokio::time::timeout(Duration::from_secs(30), waiter.wait())
            .await
            .expect("a contributor's waiter resolves with the run")
            .expect("completion outcome");
        assert!(
            matches!(
                outcome,
                crate::completion::CompletionOutcome::CompletedWithoutResult
            ),
            "{outcome:?}"
        );
    }
    assert_ready("after the full run committed").await;

    // The next run delivers the refused input.
    rig.wait_for_apply_calls(2).await;
    assert_eq!(
        rig.script.primitives().get(1),
        Some(&vec![refused_id.clone()]),
        "the next run carries the refused input"
    );
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_phase(&refused_id, InputLifecycleState::Consumed)
        .await;
    let outcome = tokio::time::timeout(Duration::from_secs(30), refused_waiter.wait())
        .await
        .expect("the refused input's waiter resolves with the next run")
        .expect("completion outcome");
    assert!(
        matches!(
            outcome,
            crate::completion::CompletionOutcome::CompletedWithoutResult
        ),
        "{outcome:?}"
    );
    assert_eq!(
        contributions(&rig.script.primitives(), &refused_id),
        1,
        "exactly one follow-up run carries the refused input"
    );
    assert_ready("after the follow-up run").await;
}

/// 255 batch inputs plus one join is 256 contributors: the join is taken. The
/// next one would make 257 and is refused.
#[tokio::test]
async fn durable_steer_joins_a_run_until_it_holds_256_contributors() {
    durable_steer_joins_up_to_the_contributor_bound(RUN_CONTRIBUTOR_BOUND - 1).await;
}

/// 256 batch inputs fill the run: the first durable join is refused.
#[tokio::test]
async fn durable_steer_join_is_refused_by_a_run_that_holds_256_contributors() {
    durable_steer_joins_up_to_the_contributor_bound(RUN_CONTRIBUTOR_BOUND).await;
}

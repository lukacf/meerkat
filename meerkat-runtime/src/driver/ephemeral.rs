//! EphemeralRuntimeDriver -- in-memory runtime driver for ephemeral sessions.
//!
//! Implements `RuntimeDriver` with:
//! - Input acceptance with validation, policy resolution, dedup, supersession
//! - InputState lifecycle management via InputLifecycleAuthority
//! - InputQueue FIFO management
//! - S24 ephemeral recovery
//! - S25 retire/reset/destroy lifecycle operations

use std::sync::{Arc, RwLock as StdRwLock};

use meerkat_core::lifecycle::{InputId, RunBoundaryReceipt, RunId};
use meerkat_core::types::HandlingMode;

use crate::accept::AcceptOutcome;
use crate::durability::{DurabilityError, validate_durability};
use crate::identifiers::LogicalRuntimeId;
use crate::input::Input;
use crate::input_ledger::InputLedger;
use crate::input_lifecycle_authority::{InputLifecycleError, InputLifecycleInput};
use crate::input_state::{InputAbandonReason, InputLifecycleState, InputState, PolicySnapshot};
use crate::policy::PolicyDecision;
use crate::queue::InputQueue;
use crate::runtime_event::{
    InputLifecycleEvent, RuntimeEvent, RuntimeEventEnvelope, RuntimeStateChangeEvent,
};
use crate::runtime_ingress_authority::{
    ContentShape, RequestId, ReservationKey, RuntimeIngressAuthority, RuntimeIngressEffect,
    RuntimeIngressInput, RuntimeIngressMutator,
};
use crate::runtime_state::RuntimeState;
use crate::traits::{RecoveryReport, ResetReport, RetireReport, RuntimeDriverError};

/// Typed post-admission signal that the runtime loop should act on.
///
/// Replaces the boolean `wake_requested` / `process_requested` flags with
/// an ordered enum where each variant is strictly stronger than the previous.
/// The driver accumulates the maximum signal across ingress effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PostAdmissionSignal {
    /// No action needed.
    None,
    /// Wake the runtime loop to process queued work (idle → running).
    WakeLoop,
    /// Interrupt cooperative yielding points within an active turn.
    ///
    /// This is weaker than immediate processing but still stronger than a
    /// plain wake. The current ingress authority no longer emits this
    /// independently, but the runtime/control seam still uses the noun and the
    /// stronger `RequestImmediateProcessing` implies it.
    InterruptYielding,
    /// Request immediate steer/checkpoint processing within the current turn.
    /// Implies WakeLoop — strictly strongest.
    RequestImmediateProcessing,
}

impl PostAdmissionSignal {
    /// Whether the runtime loop should be woken.
    pub fn should_wake(self) -> bool {
        self >= Self::WakeLoop
    }

    /// Whether cooperative yield points should be interrupted.
    pub fn should_interrupt_yielding(self) -> bool {
        self >= Self::InterruptYielding
    }

    /// Whether immediate in-turn processing was requested.
    pub fn should_process_immediately(self) -> bool {
        self == Self::RequestImmediateProcessing
    }
}

/// Shared coarse runtime control projection owned by the checked-in
/// `MeerkatMachine` and borrowed by concrete driver shells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeControlProjection {
    pub(crate) phase: RuntimeState,
    pub(crate) current_run_id: Option<RunId>,
    pub(crate) pre_run_phase: Option<RuntimeState>,
}

impl Default for RuntimeControlProjection {
    fn default() -> Self {
        Self {
            phase: RuntimeState::Idle,
            current_run_id: None,
            pre_run_phase: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ReplayQueuedContributorsPlan {
    pub queue_work_ids: Vec<InputId>,
    pub steer_work_ids: Vec<InputId>,
    pub wake_runtime: bool,
    pub notice_kind: &'static str,
}

/// Ephemeral runtime driver -- all state in-memory.
#[derive(Clone)]
pub struct EphemeralRuntimeDriver {
    runtime_id: LogicalRuntimeId,
    /// Shared coarse runtime projection owned by the machine/session entry.
    ///
    /// The concrete driver may read and realize this state, but it is not the
    /// semantic owner of the lifecycle tuple.
    control: Arc<StdRwLock<RuntimeControlProjection>>,
    ledger: InputLedger,
    queue: InputQueue,
    steer_queue: InputQueue,
    events: Vec<RuntimeEventEnvelope>,
    /// Typed post-admission signal replacing boolean wake/process flags.
    ///
    /// Accumulates the strongest signal across all ingress effects since last
    /// drain. `RequestImmediateProcessing` is strictly stronger than `WakeLoop`.
    post_admission_signal: PostAdmissionSignal,
    silent_comms_intents: Vec<String>,
    /// Canonical ingress authority -- coarse-grained ingress orchestration
    /// (queues, current run, wake/process flags, ingress phase).
    /// Runs alongside InputLifecycleAuthority (per-input) and InputLedger.
    ingress: RuntimeIngressAuthority,
}

impl EphemeralRuntimeDriver {
    fn read_control_projection(&self) -> std::sync::RwLockReadGuard<'_, RuntimeControlProjection> {
        match self.control.read() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::error!("runtime control projection lock poisoned");
                poisoned.into_inner()
            }
        }
    }

    fn write_control_projection(
        &self,
    ) -> std::sync::RwLockWriteGuard<'_, RuntimeControlProjection> {
        match self.control.write() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::error!("runtime control projection lock poisoned");
                poisoned.into_inner()
            }
        }
    }

    pub fn new(runtime_id: LogicalRuntimeId) -> Self {
        Self::new_with_control(
            runtime_id,
            Arc::new(StdRwLock::new(RuntimeControlProjection::default())),
        )
    }

    pub(crate) fn new_with_control(
        runtime_id: LogicalRuntimeId,
        control: Arc<StdRwLock<RuntimeControlProjection>>,
    ) -> Self {
        Self {
            runtime_id,
            control,
            ledger: InputLedger::new(),
            queue: InputQueue::new(),
            steer_queue: InputQueue::new(),
            events: Vec::new(),
            post_admission_signal: PostAdmissionSignal::None,
            silent_comms_intents: Vec::new(),
            ingress: RuntimeIngressAuthority::new(),
        }
    }

    pub(crate) fn control_handle(&self) -> Arc<StdRwLock<RuntimeControlProjection>> {
        self.control.clone()
    }

    fn control_snapshot(&self) -> RuntimeControlProjection {
        self.read_control_projection().clone()
    }

    pub fn set_silent_comms_intents(&mut self, intents: Vec<String>) {
        if self.control_snapshot().phase == RuntimeState::Stopped {
            return;
        }
        self.silent_comms_intents = intents;
    }

    pub fn silent_comms_intents(&self) -> Vec<String> {
        self.silent_comms_intents.clone()
    }

    /// Get a reference to the ingress authority.
    pub fn ingress(&self) -> &RuntimeIngressAuthority {
        &self.ingress
    }

    fn build_projection_queue(&self, ids: &[InputId], lane: &str) -> InputQueue {
        let mut queue = InputQueue::new();
        for input_id in ids {
            match self
                .ledger
                .get(input_id)
                .and_then(|state| state.persisted_input.clone())
            {
                Some(input) => queue.enqueue(input_id.clone(), input),
                None => {
                    tracing::error!(
                        input_id = ?input_id,
                        lane,
                        "ingress queue references input without persisted payload"
                    );
                    debug_assert!(
                        false,
                        "ingress queue projection missing persisted payload for {input_id:?} in {lane}"
                    );
                }
            }
        }
        queue
    }

    fn rebuild_queue_projections(&mut self) {
        self.queue = self.build_projection_queue(self.ingress.queue(), "queue");
        self.steer_queue = self.build_projection_queue(self.ingress.steer_queue(), "steer_queue");
    }

    pub(crate) fn replace_ingress(&mut self, ingress: RuntimeIngressAuthority) {
        self.ingress = ingress;
    }

    pub(crate) fn rebuild_queue_projections_after_recovery(&mut self) {
        self.rebuild_queue_projections();
        self.debug_assert_queue_projection_alignment();
    }

    fn debug_assert_queue_projection_alignment(&self) {
        debug_assert_eq!(
            self.queue.input_ids(),
            self.ingress.queue(),
            "physical queue must match canonical ingress queue lane"
        );
        debug_assert_eq!(
            self.steer_queue.input_ids(),
            self.ingress.steer_queue(),
            "physical steer queue must match canonical ingress steer lane"
        );
    }

    /// Admit a store-recovered input into the ingress authority's tracking.
    /// Called by the persistent driver during crash recovery to ensure the
    /// authority knows about inputs loaded from the store before `Recover` fires.
    ///
    /// Important: this only seeds canonical ingress truth. The caller restores
    /// the recovered `InputState` into the ledger before this call, so we must
    /// not rebuild the physical queue projections here. Rebuilding before the
    /// full recovery batch is loaded would make queue projection checks observe
    /// transient partial state.
    #[allow(clippy::too_many_arguments)]
    pub fn admit_recovered_to_ingress(
        &mut self,
        work_id: InputId,
        content_shape: ContentShape,
        handling_mode: HandlingMode,
        is_prompt: bool,
        lifecycle_state: InputLifecycleState,
        policy: PolicyDecision,
        request_id: Option<RequestId>,
        reservation_key: Option<ReservationKey>,
    ) -> Result<(), RuntimeDriverError> {
        match self.ingress.apply(RuntimeIngressInput::AdmitRecovered {
            work_id,
            content_shape,
            handling_mode,
            is_prompt,
            lifecycle_state,
            policy,
            request_id,
            reservation_key,
        }) {
            Ok(transition) => {
                self.process_ingress_effects(&transition.effects);
                Ok(())
            }
            Err(err) => Err(RuntimeDriverError::Internal(format!(
                "ingress AdmitRecovered failed: {err}"
            ))),
        }
    }

    /// Process effects returned by the ingress authority.
    ///
    /// Translates authority effects into driver state changes and events.
    /// Called after each successful `ingress.apply()`.
    fn process_ingress_effects(&mut self, effects: &[RuntimeIngressEffect]) {
        for effect in effects {
            match effect {
                RuntimeIngressEffect::WakeRuntime => {
                    if self.post_admission_signal < PostAdmissionSignal::WakeLoop {
                        self.post_admission_signal = PostAdmissionSignal::WakeLoop;
                    }
                }
                RuntimeIngressEffect::InterruptYielding => {
                    if self.post_admission_signal < PostAdmissionSignal::InterruptYielding {
                        self.post_admission_signal = PostAdmissionSignal::InterruptYielding;
                    }
                }
                RuntimeIngressEffect::RequestImmediateProcessing => {
                    self.post_admission_signal = PostAdmissionSignal::RequestImmediateProcessing;
                }
                RuntimeIngressEffect::IngressAccepted { work_id } => {
                    tracing::debug!(
                        work_id = ?work_id,
                        "ingress authority: input accepted"
                    );
                }
                RuntimeIngressEffect::Deduplicated {
                    work_id,
                    existing_id,
                } => {
                    tracing::debug!(
                        work_id = ?work_id,
                        existing_id = ?existing_id,
                        "ingress authority: input deduplicated"
                    );
                }
                RuntimeIngressEffect::CompletionResolved { work_id, outcome } => {
                    tracing::debug!(
                        work_id = ?work_id,
                        outcome = ?outcome,
                        "ingress authority: completion resolved"
                    );
                }
                RuntimeIngressEffect::InputLifecycleNotice { work_id, new_state } => {
                    tracing::trace!(
                        work_id = ?work_id,
                        new_state = ?new_state,
                        "ingress authority: lifecycle notice"
                    );
                }
                RuntimeIngressEffect::ReadyForRun {
                    run_id,
                    contributing_work_ids,
                } => {
                    tracing::debug!(
                        run_id = ?run_id,
                        contributors = contributing_work_ids.len(),
                        "ingress authority: ready for run"
                    );
                }
                RuntimeIngressEffect::IngressNotice { kind, detail } => {
                    tracing::debug!(
                        kind = %kind,
                        detail = %detail,
                        "ingress authority: notice"
                    );
                }
                RuntimeIngressEffect::StageInput { work_id, run_id } => {
                    // Derived from ingress authority's StageDrainSnapshot decision:
                    // drive the per-input lifecycle authority and emit the Staged event.
                    if let Some(state) = self.ledger.get_mut(work_id)
                        && let Err(e) = state.apply(InputLifecycleInput::StageForRun {
                            run_id: run_id.clone(),
                        })
                    {
                        tracing::warn!(
                            work_id = ?work_id,
                            run_id = ?run_id,
                            error = %e,
                            "per-input StageForRun failed (driven by ingress StageInput effect)"
                        );
                    }
                    self.emit_event(RuntimeEvent::InputLifecycle(InputLifecycleEvent::Staged {
                        input_id: work_id.clone(),
                        run_id: run_id.clone(),
                    }));
                }
                // Accept-phase shell directives — handled by process_accept_effects
                // when an Input payload is available. Log if they arrive here unexpectedly.
                RuntimeIngressEffect::PersistAndQueue { .. }
                | RuntimeIngressEffect::EnqueueTo { .. }
                | RuntimeIngressEffect::EnqueueFront { .. }
                | RuntimeIngressEffect::RemoveFromQueues { .. }
                | RuntimeIngressEffect::CoalesceExisting { .. }
                | RuntimeIngressEffect::SupersedeExisting { .. }
                | RuntimeIngressEffect::ConsumeOnAccept { .. }
                | RuntimeIngressEffect::EmitQueuedEvent { .. } => {
                    tracing::trace!(
                        effect = ?effect,
                        "ingress authority: accept-phase effect (handled in accept path)"
                    );
                }
            }
        }
    }
    /// Process ingress authority effects that require the Input payload.
    ///
    /// Called only from `accept_input` where the `Input` is available.
    /// Delegates non-accept effects to `process_ingress_effects`, then
    /// handles the accept-phase shell directives (enqueue, coalesce, etc.).
    fn process_accept_effects(&mut self, effects: &[RuntimeIngressEffect], input: &Input) {
        for effect in effects {
            match effect {
                RuntimeIngressEffect::PersistAndQueue { work_id } => {
                    if let Some(s) = self.ledger.get_mut(work_id) {
                        s.persisted_input = Some(input.clone());
                        let _ = s.apply(InputLifecycleInput::QueueAccepted);
                    }
                }
                RuntimeIngressEffect::EnqueueTo { work_id, target } => {
                    self.enqueue_to(*target, work_id.clone(), input.clone());
                }
                RuntimeIngressEffect::EnqueueFront { work_id, target } => {
                    self.enqueue_front_to(*target, work_id.clone(), input.clone());
                }
                RuntimeIngressEffect::RemoveFromQueues { work_id } => {
                    let _ = self.queue.remove(work_id);
                    let _ = self.steer_queue.remove(work_id);
                }
                RuntimeIngressEffect::CoalesceExisting {
                    new_id,
                    existing_id,
                } => {
                    if let Some(existing_state) = self.ledger.get_mut(existing_id) {
                        let _ = crate::coalescing::apply_coalescing(existing_state, new_id.clone());
                    }
                }
                RuntimeIngressEffect::SupersedeExisting {
                    new_id,
                    existing_id,
                } => {
                    if let Some(existing_state) = self.ledger.get_mut(existing_id) {
                        let _ =
                            crate::coalescing::apply_supersession(existing_state, new_id.clone());
                    }
                }
                RuntimeIngressEffect::ConsumeOnAccept { work_id } => {
                    if let Some(s) = self.ledger.get_mut(work_id) {
                        let _ = s.apply(InputLifecycleInput::ConsumeOnAccept);
                    }
                }
                RuntimeIngressEffect::EmitQueuedEvent { work_id } => {
                    self.emit_event(RuntimeEvent::InputLifecycle(InputLifecycleEvent::Queued {
                        input_id: work_id.clone(),
                    }));
                }
                // All other effects: delegate to the standard handler.
                other => {
                    self.process_ingress_effects(std::slice::from_ref(other));
                }
            }
        }
        self.rebuild_queue_projections();
        self.debug_assert_queue_projection_alignment();
    }

    pub fn is_idle(&self) -> bool {
        self.control_snapshot().phase == RuntimeState::Idle
    }
    pub fn is_idle_or_attached(&self) -> bool {
        self.control_snapshot().phase.is_idle_or_attached()
    }

    pub fn phase(&self) -> RuntimeState {
        self.control_snapshot().phase
    }

    pub fn current_run_id(&self) -> Option<RunId> {
        self.control_snapshot().current_run_id
    }

    pub fn pre_run_phase(&self) -> Option<RuntimeState> {
        self.control_snapshot().pre_run_phase
    }

    pub fn can_process_queue(&self) -> bool {
        self.control_snapshot().phase.can_process_queue()
    }

    /// Low-level control projection shim for external contract tests.
    ///
    /// This does not decide lifecycle legality; it only applies an already
    /// chosen MeerkatMachine control projection to the concrete driver shell.
    #[doc(hidden)]
    pub fn contract_set_control_projection(
        &mut self,
        next_phase: RuntimeState,
        current_run_id: Option<RunId>,
        pre_run_phase: Option<RuntimeState>,
    ) {
        self.set_control_projection(next_phase, current_run_id, pre_run_phase);
    }

    fn set_phase(&mut self, next_phase: RuntimeState) -> RuntimeState {
        let mut control = self.write_control_projection();
        let from_phase = control.phase;
        control.phase = next_phase;
        from_phase
    }

    fn transition_phase(&mut self, next_phase: RuntimeState) {
        let from_phase = self.set_phase(next_phase);
        self.emit_event(RuntimeEvent::RuntimeStateChange(RuntimeStateChangeEvent {
            from: from_phase,
            to: next_phase,
        }));
    }

    pub(crate) fn set_control_projection(
        &mut self,
        next_phase: RuntimeState,
        current_run_id: Option<RunId>,
        pre_run_phase: Option<RuntimeState>,
    ) {
        if self.control_snapshot().phase == next_phase {
            self.write_control_projection().phase = next_phase;
        } else {
            self.transition_phase(next_phase);
        }
        let mut control = self.write_control_projection();
        control.current_run_id = current_run_id;
        control.pre_run_phase = pre_run_phase;
    }
    /// Drain and return the accumulated post-admission signal.
    ///
    /// Returns the strongest signal seen since the last drain and resets to `None`.
    pub fn take_post_admission_signal(&mut self) -> PostAdmissionSignal {
        std::mem::replace(&mut self.post_admission_signal, PostAdmissionSignal::None)
    }

    /// Inspect the current typed post-admission signal without draining it.
    pub fn post_admission_signal(&self) -> PostAdmissionSignal {
        self.post_admission_signal
    }

    /// Drain the typed signal and return whether wake is needed (backward-compat).
    ///
    /// **Deprecated**: prefer `take_post_admission_signal()` for typed semantics.
    /// This drains the signal and returns `should_wake()`.
    pub fn take_wake_requested(&mut self) -> bool {
        // Note: we DON'T drain here — take_process_requested is always
        // called immediately after and expects to see the same signal.
        self.post_admission_signal.should_wake()
    }

    /// Return whether immediate processing was requested and drain (backward-compat).
    ///
    /// **Deprecated**: prefer `take_post_admission_signal()` for typed semantics.
    /// Must be called after `take_wake_requested()`. Drains the signal.
    pub fn take_process_requested(&mut self) -> bool {
        let signal = std::mem::replace(&mut self.post_admission_signal, PostAdmissionSignal::None);
        signal.should_process_immediately()
    }
    pub fn drain_events(&mut self) -> Vec<RuntimeEventEnvelope> {
        std::mem::take(&mut self.events)
    }
    pub fn queue(&self) -> &InputQueue {
        &self.queue
    }
    pub fn steer_queue(&self) -> &InputQueue {
        &self.steer_queue
    }

    #[cfg(test)]
    pub fn queue_mut(&mut self) -> &mut InputQueue {
        &mut self.queue
    }

    #[cfg(test)]
    pub fn steer_queue_mut(&mut self) -> &mut InputQueue {
        &mut self.steer_queue
    }

    /// Route an input to the correct queue based on handling mode.
    fn enqueue_to(&mut self, mode: HandlingMode, input_id: InputId, input: Input) {
        match mode {
            HandlingMode::Steer => self.steer_queue.enqueue(input_id, input),
            HandlingMode::Queue => self.queue.enqueue(input_id, input),
        }
    }
    /// Route an input to the front of the correct queue based on handling mode.
    fn enqueue_front_to(&mut self, mode: HandlingMode, input_id: InputId, input: Input) {
        match mode {
            HandlingMode::Steer => self.steer_queue.enqueue_front(input_id, input),
            HandlingMode::Queue => self.queue.enqueue_front(input_id, input),
        }
    }
    pub fn has_queued_input(&self, input_id: &InputId) -> bool {
        self.ingress
            .queue()
            .iter()
            .any(|queued_id| queued_id == input_id)
            || self
                .ingress
                .steer_queue()
                .iter()
                .any(|queued_id| queued_id == input_id)
    }
    pub fn has_queued_input_outside(&self, excluded: &[InputId]) -> bool {
        self.ingress
            .queue()
            .iter()
            .chain(self.ingress.steer_queue().iter())
            .any(|queued_id| !excluded.iter().any(|excluded_id| excluded_id == queued_id))
    }
    fn existing_superseded_input(
        &self,
        input: &Input,
    ) -> Option<(InputId, crate::coalescing::CoalescingResult)> {
        self.ingress
            .queue()
            .iter()
            .chain(self.ingress.steer_queue().iter())
            .find_map(|queued_id| {
                let existing = self.ledger.get(queued_id)?.persisted_input.as_ref()?;
                let result =
                    crate::coalescing::check_supersession(input, existing, &self.runtime_id);
                match result {
                    crate::coalescing::CoalescingResult::Supersedes { .. } => {
                        Some((queued_id.clone(), result))
                    }
                    crate::coalescing::CoalescingResult::Standalone => None,
                }
            })
    }
    pub fn ledger(&self) -> &InputLedger {
        &self.ledger
    }
    pub fn runtime_id(&self) -> &LogicalRuntimeId {
        &self.runtime_id
    }
    pub(crate) fn ledger_mut(&mut self) -> &mut InputLedger {
        &mut self.ledger
    }
    pub fn input_states_snapshot(&self) -> Vec<InputState> {
        self.ledger.iter().map(|(_, state)| state.clone()).collect()
    }
    /// Clear the physical queue projections without touching canonical ingress
    /// truth. Used by recovery contract tests to simulate projection loss.
    pub fn clear_queue_projections(&mut self) {
        self.queue = InputQueue::new();
        self.steer_queue = InputQueue::new();
    }
    pub fn dequeue_next(&mut self) -> Option<(InputId, Input)> {
        let queued = self
            .steer_queue
            .dequeue()
            .or_else(|| self.queue.dequeue())?;
        Some((queued.input_id, queued.input))
    }

    /// Dequeue a specific input by ID from whichever queue contains it.
    pub fn dequeue_by_id(&mut self, input_id: &InputId) -> Option<(InputId, Input)> {
        self.steer_queue
            .dequeue_by_id(input_id)
            .or_else(|| self.queue.dequeue_by_id(input_id))
    }

    /// Look up the persisted input for a given ID (from the ledger).
    #[allow(dead_code)] // Used by runtime_loop boundary classification via authority
    pub fn persisted_input(&self, input_id: &InputId) -> Option<&Input> {
        self.ledger
            .get(input_id)
            .and_then(|state| state.persisted_input.as_ref())
    }

    pub fn stage_input(
        &mut self,
        input_id: &InputId,
        run_id: &RunId,
    ) -> Result<(), InputLifecycleError> {
        self.stage_batch(std::slice::from_ref(input_id), run_id)
    }

    /// Stage a batch of inputs in a single `StageDrainSnapshot` call.
    ///
    /// All input IDs are passed as contributing work IDs to the ingress
    /// authority atomically, avoiding the guard rejection that occurs when
    /// `stage_input` is called one-at-a-time (since the first call sets
    /// `current_run` and subsequent calls fail the `no_current_run` guard).
    pub fn stage_batch(
        &mut self,
        input_ids: &[InputId],
        run_id: &RunId,
    ) -> Result<(), InputLifecycleError> {
        self.machine_realize_stage_batch(input_ids, run_id)
    }

    /// Machine-owned realization for a validated staged contributor batch.
    pub(crate) fn machine_realize_stage_batch(
        &mut self,
        input_ids: &[InputId],
        run_id: &RunId,
    ) -> Result<(), InputLifecycleError> {
        // The checked-in Meerkat machine already owns contributor-set legality
        // for starting a run batch. Production no longer routes this through
        // an ingress-helper transition surface; it applies the already-decided
        // queue removal and staged lifecycle updates directly.
        self.ingress.remove_from_queue_lanes(input_ids);
        for input_id in input_ids {
            self.ingress
                .set_lifecycle_state(input_id, InputLifecycleState::Staged);
            if let Some(state) = self.ledger.get_mut(input_id) {
                state.apply(InputLifecycleInput::StageForRun {
                    run_id: run_id.clone(),
                })?;
            }
            self.emit_event(RuntimeEvent::InputLifecycle(InputLifecycleEvent::Staged {
                input_id: input_id.clone(),
                run_id: run_id.clone(),
            }));
        }
        self.rebuild_queue_projections();
        self.debug_assert_queue_projection_alignment();

        Ok(())
    }

    pub fn apply_input(
        &mut self,
        input_id: &InputId,
        run_id: &RunId,
    ) -> Result<(), InputLifecycleError> {
        let state =
            self.ledger
                .get_mut(input_id)
                .ok_or(InputLifecycleError::InvalidTransition {
                    from: InputLifecycleState::Staged,
                    input: "MarkApplied (input not found)".into(),
                })?;
        state.apply(InputLifecycleInput::MarkApplied {
            run_id: run_id.clone(),
        })?;
        state.apply(InputLifecycleInput::MarkAppliedPendingConsumption {
            boundary_sequence: 0,
        })?;
        self.emit_event(RuntimeEvent::InputLifecycle(InputLifecycleEvent::Applied {
            input_id: input_id.clone(),
            run_id: run_id.clone(),
        }));
        Ok(())
    }

    pub fn consume_inputs(
        &mut self,
        input_ids: &[InputId],
        run_id: &RunId,
    ) -> Result<(), InputLifecycleError> {
        for input_id in input_ids {
            if let Some(state) = self.ledger.get_mut(input_id) {
                match state.apply(InputLifecycleInput::Consume) {
                    Ok(_) => {
                        self.events
                            .push(self.make_envelope(RuntimeEvent::InputLifecycle(
                                InputLifecycleEvent::Consumed {
                                    input_id: input_id.clone(),
                                    run_id: run_id.clone(),
                                },
                            )));
                    }
                    Err(
                        InputLifecycleError::InvalidTransition { .. }
                        | InputLifecycleError::TerminalState { .. },
                    ) => {}
                    Err(err) => return Err(err),
                }
            }
        }
        Ok(())
    }

    pub fn rollback_staged(&mut self, input_ids: &[InputId]) -> Result<(), InputLifecycleError> {
        let mut terminal_inputs = Vec::new();

        for input_id in input_ids {
            if let Some(state) = self.ledger.get_mut(input_id) {
                match state.apply(InputLifecycleInput::RollbackStaged) {
                    Ok(transition) => {
                        if transition.next_phase != crate::input_state::InputLifecycleState::Queued
                        {
                            tracing::warn!(
                                input_id = %input_id,
                                next_phase = ?transition.next_phase,
                                "input abandoned after max stage attempts"
                            );
                            if let Some(outcome) = state.terminal_outcome().cloned() {
                                terminal_inputs.push((input_id.clone(), outcome.clone()));
                                if let crate::input_state::InputTerminalOutcome::Abandoned {
                                    reason,
                                } = outcome
                                {
                                    self.events.push(self.make_envelope(
                                        RuntimeEvent::InputLifecycle(
                                            InputLifecycleEvent::Abandoned {
                                                input_id: input_id.clone(),
                                                reason: format!("{reason:?}"),
                                            },
                                        ),
                                    ));
                                }
                            }
                        }
                    }
                    Err(
                        InputLifecycleError::InvalidTransition { .. }
                        | InputLifecycleError::TerminalState { .. },
                    ) => {}
                    Err(err) => return Err(err),
                }
            }
        }

        if !terminal_inputs.is_empty() {
            match self
                .ingress
                .apply(RuntimeIngressInput::ReconcileTerminalInputs { terminal_inputs })
            {
                Ok(transition) => {
                    self.process_ingress_effects(&transition.effects);
                }
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        "ingress authority rejected ReconcileTerminalInputs"
                    );
                }
            }
        }

        self.rebuild_queue_projections();
        self.debug_assert_queue_projection_alignment();
        Ok(())
    }

    pub(crate) fn finalize_retire(&mut self) -> RetireReport {
        let inputs_pending_drain = self.ledger.iter().filter(|(_, s)| !s.is_terminal()).count();
        RetireReport {
            inputs_abandoned: 0,
            inputs_pending_drain,
        }
    }

    /// Low-level retire realization shim for external contract tests.
    ///
    /// Callers are responsible for setting the control projection first.
    #[doc(hidden)]
    pub fn contract_finalize_retire(&mut self) -> RetireReport {
        self.finalize_retire()
    }

    pub(crate) fn reset_cleanup(&mut self) -> ResetReport {
        let abandoned = self.abandon_all_non_terminal(InputAbandonReason::Reset);
        self.queue.drain();
        self.steer_queue.drain();
        self.post_admission_signal = PostAdmissionSignal::None;
        self.silent_comms_intents.clear();
        self.rebuild_queue_projections();
        self.debug_assert_queue_projection_alignment();
        ResetReport {
            inputs_abandoned: abandoned,
        }
    }

    /// Low-level reset realization shim for external contract tests.
    ///
    /// Callers are responsible for setting the post-reset control projection.
    #[doc(hidden)]
    pub fn contract_reset_cleanup(&mut self) -> ResetReport {
        self.reset_cleanup()
    }

    pub(crate) fn destroy_cleanup(&mut self) -> usize {
        let abandoned = self.abandon_all_non_terminal(InputAbandonReason::Destroyed);
        self.silent_comms_intents.clear();
        self.rebuild_queue_projections();
        self.debug_assert_queue_projection_alignment();
        abandoned
    }

    /// Low-level destroy realization shim for external contract tests.
    ///
    /// Callers are responsible for setting the destroyed control projection.
    #[doc(hidden)]
    pub fn contract_destroy_cleanup(&mut self) -> usize {
        self.destroy_cleanup()
    }

    pub(crate) fn stop_runtime_cleanup(&mut self) {
        self.abandon_all_non_terminal(InputAbandonReason::Stopped);
        self.silent_comms_intents.clear();
        self.queue.drain();
        self.steer_queue.drain();
    }

    pub(crate) fn finalize_stop_runtime(&mut self) {
        self.stop_runtime_cleanup();
    }

    /// Low-level stop realization shim for external contract tests.
    ///
    /// Callers are responsible for setting the stopped control projection.
    #[doc(hidden)]
    pub fn contract_finalize_stop_runtime(&mut self) {
        self.finalize_stop_runtime();
    }

    pub fn recover_ephemeral(&mut self) -> Result<RecoveryReport, RuntimeDriverError> {
        crate::meerkat_machine::machine_recover_ephemeral_driver(self)
    }

    pub fn recycle_preserving_work(&mut self) -> Result<usize, RuntimeDriverError> {
        let transferred = self.ledger.active_input_ids().len();
        let runtime_id = self.runtime_id.clone();
        let silent_comms_intents = self.silent_comms_intents.clone();
        let ledger = self.ledger.clone();
        let ingress = self.ingress.clone();
        let control = self.control.clone();

        *self = Self::new_with_control(runtime_id, control);
        self.silent_comms_intents = silent_comms_intents;
        self.ledger = ledger;
        self.ingress = ingress;

        self.recover_ephemeral()?;
        self.rebuild_queue_projections();
        self.debug_assert_queue_projection_alignment();

        Ok(transferred)
    }

    fn emit_event(&mut self, event: RuntimeEvent) {
        self.events.push(self.make_envelope(event));
    }
    fn make_envelope(&self, event: RuntimeEvent) -> RuntimeEventEnvelope {
        RuntimeEventEnvelope {
            id: crate::identifiers::RuntimeEventId::new(),
            timestamp: chrono::Utc::now(),
            runtime_id: self.runtime_id.clone(),
            event,
            causation_id: None,
            correlation_id: None,
        }
    }

    pub fn abandon_all_non_terminal(&mut self, reason: InputAbandonReason) -> usize {
        let non_terminal_ids: Vec<InputId> = self
            .ledger
            .iter()
            .filter(|(_, s)| !s.is_terminal())
            .map(|(id, _)| id.clone())
            .collect();
        let mut count = 0;
        let mut terminal_inputs = Vec::new();
        for id in &non_terminal_ids {
            let outcome = if let Some(state) = self.ledger.get_mut(id)
                && state
                    .apply(InputLifecycleInput::Abandon {
                        reason: reason.clone(),
                    })
                    .is_ok()
            {
                count += 1;
                state.terminal_outcome().cloned()
            } else {
                None
            };
            if outcome.is_some() {
                self.events
                    .push(self.make_envelope(RuntimeEvent::InputLifecycle(
                        InputLifecycleEvent::Abandoned {
                            input_id: id.clone(),
                            reason: format!("{reason:?}"),
                        },
                    )));
            }
            if let Some(outcome) = outcome {
                terminal_inputs.push((id.clone(), outcome));
            }
        }
        if !terminal_inputs.is_empty() {
            match self
                .ingress
                .apply(RuntimeIngressInput::ReconcileTerminalInputs { terminal_inputs })
            {
                Ok(transition) => self.process_ingress_effects(&transition.effects),
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        "ingress authority rejected terminal reconciliation after abandon"
                    );
                }
            }
        }
        count
    }

    pub fn abandon_pending_inputs(&mut self, reason: InputAbandonReason) -> usize {
        let abandoned = self.abandon_all_non_terminal(reason);
        self.queue.drain();
        self.steer_queue.drain();
        self.post_admission_signal = PostAdmissionSignal::None;
        self.rebuild_queue_projections();
        self.debug_assert_queue_projection_alignment();
        abandoned
    }

    pub async fn run_completed(
        &mut self,
        run_id: RunId,
        consumed_input_ids: Vec<InputId>,
    ) -> Result<(), RuntimeDriverError> {
        self.machine_realize_run_completed(&run_id, &consumed_input_ids)
    }

    /// Machine-owned realization for a validated run-completion transition.
    pub(crate) fn machine_realize_run_completed(
        &mut self,
        run_id: &RunId,
        consumed_input_ids: &[InputId],
    ) -> Result<(), RuntimeDriverError> {
        // The checked-in Meerkat machine already owns contributor-set legality
        // for completion. Production applies the already-decided consumed
        // lifecycle mirror directly instead of routing through an ingress
        // helper transition.
        for input_id in consumed_input_ids {
            self.ingress
                .set_lifecycle_state(input_id, InputLifecycleState::Consumed);
        }

        self.consume_inputs(consumed_input_ids, run_id)
            .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?;
        Ok(())
    }

    pub async fn run_failed(
        &mut self,
        run_id: RunId,
        contributing_input_ids: Vec<InputId>,
        replay_plan: ReplayQueuedContributorsPlan,
        _error: String,
        _recoverable: bool,
    ) -> Result<(), RuntimeDriverError> {
        self.machine_realize_run_failed(&run_id, &contributing_input_ids, &replay_plan)
    }

    /// Machine-owned realization for a validated failed-run replay plan.
    pub(crate) fn machine_realize_run_failed(
        &mut self,
        run_id: &RunId,
        contributing_input_ids: &[InputId],
        replay_plan: &ReplayQueuedContributorsPlan,
    ) -> Result<(), RuntimeDriverError> {
        self.ingress
            .replay_queue_lanes(&replay_plan.queue_work_ids, &replay_plan.steer_work_ids);
        if replay_plan.wake_runtime && self.post_admission_signal < PostAdmissionSignal::WakeLoop {
            self.post_admission_signal = PostAdmissionSignal::WakeLoop;
        }
        tracing::debug!(
            run_id = ?run_id,
            kind = replay_plan.notice_kind,
            queue = replay_plan.queue_work_ids.len(),
            steer = replay_plan.steer_work_ids.len(),
            "runtime replayed queued contributors"
        );

        self.rollback_staged(contributing_input_ids)
            .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?;
        Ok(())
    }

    pub async fn boundary_applied(
        &mut self,
        run_id: RunId,
        receipt: RunBoundaryReceipt,
        _session_snapshot: Option<Vec<u8>>,
    ) -> Result<(), RuntimeDriverError> {
        self.machine_realize_boundary_applied(&run_id, &receipt)
    }

    /// Machine-owned realization for a validated boundary-application step.
    pub(crate) fn machine_realize_boundary_applied(
        &mut self,
        run_id: &RunId,
        receipt: &RunBoundaryReceipt,
    ) -> Result<(), RuntimeDriverError> {
        // The checked-in Meerkat machine already owns contributor-set legality
        // for boundary application. Production applies the already-decided
        // pending-consumption lifecycle mirror directly instead of routing
        // through an ingress helper transition.
        for input_id in &receipt.contributing_input_ids {
            self.ingress
                .set_lifecycle_state(input_id, InputLifecycleState::AppliedPendingConsumption);
        }
        tracing::debug!(
            contributors = receipt.contributing_input_ids.len(),
            sequence = receipt.sequence,
            "runtime boundary applied"
        );

        for input_id in &receipt.contributing_input_ids {
            if let Some(state) = self.ledger.get_mut(input_id) {
                let applied = state
                    .apply(InputLifecycleInput::MarkApplied {
                        run_id: run_id.clone(),
                    })
                    .is_ok();
                let _ = state.apply(InputLifecycleInput::MarkAppliedPendingConsumption {
                    boundary_sequence: receipt.sequence,
                });
                if applied {
                    self.events
                        .push(self.make_envelope(RuntimeEvent::InputLifecycle(
                            InputLifecycleEvent::Applied {
                                input_id: input_id.clone(),
                                run_id: run_id.clone(),
                            },
                        )));
                }
            }
        }
        Ok(())
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl crate::traits::RuntimeDriver for EphemeralRuntimeDriver {
    async fn accept_input(&mut self, input: Input) -> Result<AcceptOutcome, RuntimeDriverError> {
        match self.runtime_state() {
            RuntimeState::Retired | RuntimeState::Stopped => {
                return Err(RuntimeDriverError::NotReady {
                    state: self.runtime_state(),
                });
            }
            RuntimeState::Destroyed => return Err(RuntimeDriverError::Destroyed),
            RuntimeState::Initializing
            | RuntimeState::Idle
            | RuntimeState::Attached
            | RuntimeState::Running => {}
        }

        if let Err(e) = validate_durability(&input) {
            match e {
                DurabilityError::DerivedForbidden { .. }
                | DurabilityError::ExternalDerivedForbidden => {
                    return Ok(AcceptOutcome::Rejected {
                        reason: crate::accept::RejectReason::DurabilityViolation {
                            detail: e.to_string(),
                        },
                    });
                }
            }
        }
        if let Err(e) = crate::peer_handling_mode::validate_peer_handling_mode(&input) {
            return Ok(AcceptOutcome::Rejected {
                reason: crate::accept::RejectReason::PeerHandlingModeInvalid {
                    detail: e.to_string(),
                },
            });
        }
        let input_id = input.id().clone();
        let mut state = InputState::new_accepted(input_id.clone());
        state.durability = Some(input.header().durability);
        state.idempotency_key = input.header().idempotency_key.clone();
        if let Some(ref key) = input.header().idempotency_key {
            if let Some(existing_id) = self
                .ledger
                .accept_with_idempotency(state.clone(), key.clone())
            {
                // Route dedup decision through ingress authority — the authority
                // owns the semantic decision to reject the duplicate.
                match self.ingress.apply(RuntimeIngressInput::AdmitDeduplicated {
                    work_id: input_id.clone(),
                    existing_id: existing_id.clone(),
                }) {
                    Ok(transition) => {
                        self.process_ingress_effects(&transition.effects);
                    }
                    Err(err) => {
                        return Err(RuntimeDriverError::Internal(format!(
                            "ingress authority rejected AdmitDeduplicated for '{input_id}' against '{existing_id}': {err}"
                        )));
                    }
                }
                self.emit_event(RuntimeEvent::InputLifecycle(
                    InputLifecycleEvent::Deduplicated {
                        input_id: input_id.clone(),
                        existing_id: existing_id.clone(),
                    },
                ));
                return Ok(AcceptOutcome::Deduplicated {
                    input_id,
                    existing_id,
                });
            }
        } else {
            self.ledger.accept(state.clone());
        }
        let runtime_idle = self.runtime_state().is_idle_or_attached();
        let existing_superseded_id = self.existing_superseded_input(&input).map(|(id, _)| id);
        let resolved = crate::accept::resolve_admission(
            &input,
            runtime_idle,
            &self.silent_comms_intents,
            existing_superseded_id,
        );
        if let Some(s) = self.ledger.get_mut(&input_id) {
            s.policy = Some(PolicySnapshot {
                version: resolved.policy.policy_version,
                decision: resolved.policy.clone(),
            });
        }
        self.emit_event(RuntimeEvent::InputLifecycle(
            InputLifecycleEvent::Accepted {
                input_id: input_id.clone(),
            },
        ));
        // --- Ingress authority: machine-owned admission plan ---
        // All mechanical lookups happen here, but the checked-in admission
        // plan now decides the semantic path (consume-on-accept, queue,
        // priority, coalesce, supersede). The helper only applies that plan.
        let handling_mode = resolved.handling_mode;
        let content_shape = ContentShape(input.kind_id().to_string());
        let is_prompt = matches!(input, Input::Prompt(_));
        let ingress_input = match resolved.admission_plan {
            crate::accept::AdmissionPlan::ConsumedOnAccept => {
                RuntimeIngressInput::AdmitConsumedOnAccept {
                    work_id: input_id.clone(),
                    content_shape,
                    request_id: None,
                    reservation_key: None,
                    policy: resolved.policy.clone(),
                }
            }
            crate::accept::AdmissionPlan::Queued {
                persist_and_queue,
                queue_action,
                existing_action,
            } => RuntimeIngressInput::AdmitQueued {
                work_id: input_id.clone(),
                content_shape,
                handling_mode,
                is_prompt,
                request_id: None,
                reservation_key: None,
                policy: resolved.policy.clone(),
                persist_and_queue,
                queue_action,
                existing_action,
            },
        };
        match self.ingress.apply(ingress_input) {
            Ok(transition) => {
                self.process_accept_effects(&transition.effects, &input);
            }
            Err(err) => {
                return Err(RuntimeDriverError::Internal(format!(
                    "ingress authority rejected accept_input for '{input_id}': {err}"
                )));
            }
        }
        let final_state = self.ledger.get(&input_id).cloned().unwrap_or_else(|| state);
        Ok(AcceptOutcome::Accepted {
            input_id,
            policy: resolved.policy,
            state: final_state,
        })
    }

    async fn on_runtime_event(
        &mut self,
        _event: RuntimeEventEnvelope,
    ) -> Result<(), RuntimeDriverError> {
        Ok(())
    }

    async fn recover(&mut self) -> Result<RecoveryReport, RuntimeDriverError> {
        self.recover_ephemeral()
    }
    fn runtime_state(&self) -> RuntimeState {
        self.control_snapshot().phase
    }
    fn input_state(&self, input_id: &InputId) -> Option<&InputState> {
        self.ledger.get(input_id)
    }
    fn active_input_ids(&self) -> Vec<InputId> {
        self.ledger.active_input_ids()
    }
}

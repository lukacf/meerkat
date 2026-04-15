//! EphemeralRuntimeDriver -- in-memory runtime driver for ephemeral sessions.
//!
//! Implements `RuntimeDriver` with:
//! - Input acceptance with validation, policy resolution, dedup, supersession
//! - InputState lifecycle management via InputLifecycleAuthority
//! - InputQueue FIFO management
//! - S24 ephemeral recovery
//! - S25 retire/reset/destroy lifecycle operations

use std::collections::BTreeSet;

use meerkat_core::lifecycle::{InputId, RunEvent, RunId};
use meerkat_core::types::HandlingMode;

use crate::accept::AcceptOutcome;
use crate::durability::{DurabilityError, validate_durability};
use crate::identifiers::LogicalRuntimeId;
use crate::input::Input;
use crate::input_ledger::InputLedger;
use crate::input_lifecycle_authority::{InputLifecycleError, InputLifecycleInput};
use crate::input_state::{InputAbandonReason, InputLifecycleState, InputState, PolicySnapshot};
use crate::policy::{PolicyDecision, RoutingDisposition};
use crate::policy_table::DefaultPolicyTable;
use crate::queue::InputQueue;
use crate::runtime_event::{
    InputLifecycleEvent, RuntimeEvent, RuntimeEventEnvelope, RuntimeStateChangeEvent,
};
use crate::runtime_ingress_authority::{
    ContentShape, RequestId, ReservationKey, RuntimeIngressAuthority, RuntimeIngressEffect,
    RuntimeIngressInput, RuntimeIngressMutator,
};
use crate::runtime_state::RuntimeState;
use crate::traits::{
    DestroyReport, RecoveryReport, ResetReport, RetireReport, RuntimeControlCommand,
    RuntimeDriverError,
};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunReturnPhase {
    Idle,
    Attached,
    Retired,
}

impl RunReturnPhase {
    fn as_runtime_state(self) -> RuntimeState {
        match self {
            Self::Idle => RuntimeState::Idle,
            Self::Attached => RuntimeState::Attached,
            Self::Retired => RuntimeState::Retired,
        }
    }
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

/// Derive the handling mode from a policy decision's routing disposition.
pub(crate) fn handling_mode_from_policy(policy: &crate::policy::PolicyDecision) -> HandlingMode {
    match policy.routing_disposition {
        RoutingDisposition::Steer => HandlingMode::Steer,
        _ => HandlingMode::Queue,
    }
}

/// Whether this input should request immediate processing after admission.
///
/// This is intentionally narrower than "routes through the steer lane".
/// `ResponseProgress` uses checkpoint routing for boundary batching, but it
/// must remain passive unless the input kind explicitly carries steer intent.
pub(crate) fn requests_immediate_processing(input: &Input) -> bool {
    matches!(input.handling_mode(), Some(HandlingMode::Steer))
}

/// Ephemeral runtime driver -- all state in-memory.
#[derive(Clone)]
pub struct EphemeralRuntimeDriver {
    runtime_id: LogicalRuntimeId,
    /// Canonical coarse runtime phase owned by the checked-in MeerkatMachine.
    phase: RuntimeState,
    /// Active run identity, if a run is currently bound.
    current_run_id: Option<RunId>,
    /// The coarse phase a run returns to when it terminates.
    pre_run_phase: Option<RunReturnPhase>,
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
    pub fn new(runtime_id: LogicalRuntimeId) -> Self {
        Self {
            runtime_id,
            phase: RuntimeState::Idle,
            current_run_id: None,
            pre_run_phase: None,
            ledger: InputLedger::new(),
            queue: InputQueue::new(),
            steer_queue: InputQueue::new(),
            events: Vec::new(),
            post_admission_signal: PostAdmissionSignal::None,
            silent_comms_intents: Vec::new(),
            ingress: RuntimeIngressAuthority::new(),
        }
    }

    pub fn set_silent_comms_intents(&mut self, intents: Vec<String>) {
        let overrides = intents.into_iter().collect::<BTreeSet<_>>();
        match self
            .ingress
            .apply(RuntimeIngressInput::SetSilentIntentOverrides { intents: overrides })
        {
            Ok(transition) => {
                self.process_ingress_effects(&transition.effects);
                self.silent_comms_intents = self
                    .ingress
                    .silent_intent_overrides()
                    .iter()
                    .cloned()
                    .collect();
            }
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "ingress authority rejected SetSilentIntentOverrides"
                );
            }
        }
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
        lifecycle_state: InputLifecycleState,
        policy: PolicyDecision,
        request_id: Option<RequestId>,
        reservation_key: Option<ReservationKey>,
    ) -> Result<(), RuntimeDriverError> {
        match self.ingress.apply(RuntimeIngressInput::AdmitRecovered {
            work_id,
            content_shape,
            handling_mode,
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
                RuntimeIngressEffect::SilentIntentApplied { work_id, intent } => {
                    tracing::debug!(
                        work_id = ?work_id,
                        intent = %intent,
                        "ingress authority: silent intent applied"
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
                RuntimeIngressEffect::RecoverConsumeOnAccept { work_id }
                | RuntimeIngressEffect::RecoverRollback { work_id }
                | RuntimeIngressEffect::RecoverKeep { work_id } => {
                    tracing::trace!(
                        work_id = ?work_id,
                        effect = ?effect,
                        "ingress authority: recovery effect"
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
        self.phase == RuntimeState::Idle
    }
    pub fn is_idle_or_attached(&self) -> bool {
        self.phase.is_idle_or_attached()
    }

    pub fn phase(&self) -> RuntimeState {
        self.phase
    }

    pub fn current_run_id(&self) -> Option<&RunId> {
        self.current_run_id.as_ref()
    }

    pub fn pre_run_phase(&self) -> Option<RuntimeState> {
        self.pre_run_phase.map(RunReturnPhase::as_runtime_state)
    }

    pub fn can_process_queue(&self) -> bool {
        self.phase.can_process_queue()
    }

    fn resolve_run_return_phase(&self) -> RuntimeState {
        self.pre_run_phase
            .map(RunReturnPhase::as_runtime_state)
            .unwrap_or(RuntimeState::Idle)
    }

    fn set_phase(&mut self, next_phase: RuntimeState) -> RuntimeState {
        let from_phase = self.phase;
        self.phase = next_phase;
        from_phase
    }

    fn transition_phase(&mut self, next_phase: RuntimeState) {
        let from_phase = self.set_phase(next_phase);
        self.emit_event(RuntimeEvent::RuntimeStateChange(RuntimeStateChangeEvent {
            from: from_phase,
            to: next_phase,
        }));
    }

    fn begin_run_from_current_phase(
        &mut self,
        run_id: RunId,
    ) -> Result<(), crate::runtime_state::RuntimeStateTransitionError> {
        let pre_run_phase = match self.phase {
            RuntimeState::Idle => RunReturnPhase::Idle,
            RuntimeState::Attached => RunReturnPhase::Attached,
            RuntimeState::Retired => RunReturnPhase::Retired,
            from => {
                return Err(crate::runtime_state::RuntimeStateTransitionError {
                    from,
                    to: RuntimeState::Running,
                });
            }
        };
        if self.current_run_id.is_some() {
            return Err(crate::runtime_state::RuntimeStateTransitionError {
                from: self.phase,
                to: RuntimeState::Running,
            });
        }
        self.current_run_id = Some(run_id);
        self.pre_run_phase = Some(pre_run_phase);
        self.phase = RuntimeState::Running;
        Ok(())
    }

    fn finish_run_from_current_phase(
        &mut self,
        run_id: &RunId,
    ) -> Result<RuntimeState, crate::runtime_state::RuntimeStateTransitionError> {
        let next_phase = match self.phase {
            RuntimeState::Running => self.resolve_run_return_phase(),
            RuntimeState::Retired => RuntimeState::Retired,
            from => {
                return Err(crate::runtime_state::RuntimeStateTransitionError {
                    from,
                    to: self.resolve_run_return_phase(),
                });
            }
        };
        match self.current_run_id.as_ref() {
            Some(active_id) if active_id == run_id => {}
            _ => {
                return Err(crate::runtime_state::RuntimeStateTransitionError {
                    from: self.phase,
                    to: next_phase,
                });
            }
        }
        self.current_run_id = None;
        self.pre_run_phase = None;
        self.phase = next_phase;
        Ok(next_phase)
    }

    pub fn attach(&mut self) -> Result<(), crate::runtime_state::RuntimeStateTransitionError> {
        if self.phase != RuntimeState::Idle {
            return Err(crate::runtime_state::RuntimeStateTransitionError {
                from: self.phase,
                to: RuntimeState::Attached,
            });
        }
        self.transition_phase(RuntimeState::Attached);
        Ok(())
    }
    pub fn detach(
        &mut self,
    ) -> Result<Option<RuntimeState>, crate::runtime_state::RuntimeStateTransitionError> {
        if self.phase == RuntimeState::Attached {
            self.transition_phase(RuntimeState::Idle);
            Ok(Some(RuntimeState::Attached))
        } else {
            Ok(None)
        }
    }
    pub fn start_run(
        &mut self,
        run_id: RunId,
    ) -> Result<(), crate::runtime_state::RuntimeStateTransitionError> {
        self.begin_run_from_current_phase(run_id)
    }
    pub fn complete_run(
        &mut self,
    ) -> Result<RunId, crate::runtime_state::RuntimeStateTransitionError> {
        let run_id = self.current_run_id.clone().ok_or(
            crate::runtime_state::RuntimeStateTransitionError {
                from: self.phase,
                to: RuntimeState::Idle,
            },
        )?;
        let _ = self.finish_run_from_current_phase(&run_id)?;
        Ok(run_id)
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
        // Ingress authority: StageDrainSnapshot — the authority owns the Staged
        // transition. It emits StageInput effects that drive the per-input lifecycle
        // authority and Staged events (handled in process_ingress_effects).
        // No independent shell StageForRun call — single source of truth.
        match self.ingress.apply(RuntimeIngressInput::StageDrainSnapshot {
            run_id: run_id.clone(),
            contributing_work_ids: input_ids.to_vec(),
        }) {
            Ok(transition) => {
                self.process_ingress_effects(&transition.effects);
                self.rebuild_queue_projections();
                self.debug_assert_queue_projection_alignment();
            }
            Err(err) => {
                tracing::warn!(
                    input_ids = ?input_ids,
                    run_id = ?run_id,
                    error = %err,
                    "ingress authority rejected StageDrainSnapshot"
                );
                return Err(InputLifecycleError::InvalidTransition {
                    from: InputLifecycleState::Queued,
                    input: format!("StageDrainSnapshot rejected: {err}"),
                });
            }
        }

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

    pub fn retire(&mut self) -> Result<RetireReport, RuntimeDriverError> {
        // Ingress authority: Retire
        match self.ingress.apply(RuntimeIngressInput::Retire) {
            Ok(transition) => {
                self.process_ingress_effects(&transition.effects);
            }
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "ingress authority rejected Retire"
                );
            }
        }

        let next_phase = match self.phase {
            RuntimeState::Idle | RuntimeState::Attached | RuntimeState::Running => {
                RuntimeState::Retired
            }
            from => {
                return Err(RuntimeDriverError::Internal(
                    crate::runtime_state::RuntimeStateTransitionError {
                        from,
                        to: RuntimeState::Retired,
                    }
                    .to_string(),
                ));
            }
        };
        self.transition_phase(next_phase);
        let inputs_pending_drain = self.ledger.iter().filter(|(_, s)| !s.is_terminal()).count();
        Ok(RetireReport {
            inputs_abandoned: 0,
            inputs_pending_drain,
        })
    }

    pub fn reset(&mut self) -> Result<ResetReport, RuntimeDriverError> {
        // Ingress authority: Reset (authority rejects if a run is in progress)
        match self.ingress.apply(RuntimeIngressInput::Reset) {
            Ok(transition) => {
                self.process_ingress_effects(&transition.effects);
            }
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "ingress authority rejected Reset"
                );
            }
        }

        let abandoned = self.abandon_all_non_terminal(InputAbandonReason::Reset);
        self.queue.drain();
        self.steer_queue.drain();
        self.post_admission_signal = PostAdmissionSignal::None;
        match self.phase {
            RuntimeState::Initializing
            | RuntimeState::Idle
            | RuntimeState::Attached
            | RuntimeState::Retired => {
                self.current_run_id = None;
                self.pre_run_phase = None;
                self.transition_phase(RuntimeState::Idle);
            }
            from => {
                return Err(RuntimeDriverError::Internal(
                    crate::runtime_state::RuntimeStateTransitionError {
                        from,
                        to: RuntimeState::Idle,
                    }
                    .to_string(),
                ));
            }
        }
        self.rebuild_queue_projections();
        self.debug_assert_queue_projection_alignment();
        Ok(ResetReport {
            inputs_abandoned: abandoned,
        })
    }

    pub fn destroy(&mut self) -> Result<usize, RuntimeDriverError> {
        // Ingress authority: Destroy
        match self.ingress.apply(RuntimeIngressInput::Destroy) {
            Ok(transition) => {
                self.process_ingress_effects(&transition.effects);
            }
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "ingress authority rejected Destroy"
                );
            }
        }

        match self.phase {
            RuntimeState::Initializing
            | RuntimeState::Idle
            | RuntimeState::Attached
            | RuntimeState::Running
            | RuntimeState::Retired
            | RuntimeState::Stopped => {
                self.current_run_id = None;
                self.pre_run_phase = None;
                self.transition_phase(RuntimeState::Destroyed);
            }
            from => {
                return Err(RuntimeDriverError::Internal(
                    crate::runtime_state::RuntimeStateTransitionError {
                        from,
                        to: RuntimeState::Destroyed,
                    }
                    .to_string(),
                ));
            }
        }
        let abandoned = self.abandon_all_non_terminal(InputAbandonReason::Destroyed);
        self.rebuild_queue_projections();
        self.debug_assert_queue_projection_alignment();
        Ok(abandoned)
    }

    pub fn recover_ephemeral(&mut self) -> RecoveryReport {
        // Ingress authority: Recover — returns typed per-input recovery effects.
        let recovery_effects = match self.ingress.apply(RuntimeIngressInput::Recover) {
            Ok(transition) => {
                self.process_ingress_effects(&transition.effects);
                transition.effects
            }
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "ingress authority rejected Recover"
                );
                Vec::new()
            }
        };

        // Execute the authority's per-input recovery effects.
        let mut recovered = 0;
        let mut abandoned = 0;
        let mut requeued = 0;

        for effect in &recovery_effects {
            match effect {
                RuntimeIngressEffect::RecoverConsumeOnAccept { work_id } => {
                    if let Some(state) = self.ledger.get_mut(work_id) {
                        let _ = state.apply(InputLifecycleInput::ConsumeOnAccept);
                        abandoned += 1;
                        recovered += 1;
                    }
                }
                RuntimeIngressEffect::RecoverRollback { work_id } => {
                    if let Some(state) = self.ledger.get_mut(work_id) {
                        match state.current_state() {
                            InputLifecycleState::Accepted => {
                                let _ = state.apply(InputLifecycleInput::QueueAccepted);
                            }
                            InputLifecycleState::Staged => {
                                let _ = state.apply(InputLifecycleInput::RollbackStaged);
                            }
                            _ => {}
                        }
                        requeued += 1;
                        recovered += 1;
                    }
                }
                RuntimeIngressEffect::RecoverKeep { work_id } => {
                    if self.ledger.get(work_id).is_some() {
                        recovered += 1;
                    }
                }
                _ => {}
            }
        }

        self.rebuild_queue_projections();
        self.debug_assert_queue_projection_alignment();

        RecoveryReport {
            inputs_recovered: recovered,
            inputs_abandoned: abandoned,
            inputs_requeued: requeued,
            details: Vec::new(),
        }
    }

    pub fn recycle_preserving_work(&mut self) -> Result<usize, RuntimeDriverError> {
        let next_phase = match self.phase {
            RuntimeState::Idle | RuntimeState::Retired => {
                if self.current_run_id.is_some() {
                    return Err(RuntimeDriverError::Internal(
                        crate::runtime_state::RuntimeStateTransitionError {
                            from: self.phase,
                            to: RuntimeState::Idle,
                        }
                        .to_string(),
                    ));
                }
                RuntimeState::Idle
            }
            RuntimeState::Attached => {
                if self.current_run_id.is_some() {
                    return Err(RuntimeDriverError::Internal(
                        crate::runtime_state::RuntimeStateTransitionError {
                            from: self.phase,
                            to: RuntimeState::Attached,
                        }
                        .to_string(),
                    ));
                }
                RuntimeState::Attached
            }
            from => {
                return Err(RuntimeDriverError::Internal(
                    crate::runtime_state::RuntimeStateTransitionError {
                        from,
                        to: RuntimeState::Idle,
                    }
                    .to_string(),
                ));
            }
        };
        if self.phase == next_phase {
            self.phase = next_phase;
        } else {
            self.transition_phase(next_phase);
        }
        self.current_run_id = None;
        self.pre_run_phase = None;

        let transferred = self.ledger.active_input_ids().len();
        let runtime_id = self.runtime_id.clone();
        let silent_comms_intents = self.silent_comms_intents.clone();
        let ledger = self.ledger.clone();
        let ingress = self.ingress.clone();
        let phase = self.phase;
        let current_run_id = self.current_run_id.clone();
        let pre_run_phase = self.pre_run_phase;

        *self = Self::new(runtime_id);
        self.silent_comms_intents = silent_comms_intents;
        self.ledger = ledger;
        self.ingress = ingress;
        self.phase = phase;
        self.current_run_id = current_run_id;
        self.pre_run_phase = pre_run_phase;

        let _ = self.recover_ephemeral();
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
        for id in &non_terminal_ids {
            if let Some(state) = self.ledger.get_mut(id)
                && state
                    .apply(InputLifecycleInput::Abandon {
                        reason: reason.clone(),
                    })
                    .is_ok()
            {
                count += 1;
                self.events
                    .push(self.make_envelope(RuntimeEvent::InputLifecycle(
                        InputLifecycleEvent::Abandoned {
                            input_id: id.clone(),
                            reason: format!("{reason:?}"),
                        },
                    )));
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
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl crate::traits::RuntimeDriver for EphemeralRuntimeDriver {
    async fn accept_input(&mut self, input: Input) -> Result<AcceptOutcome, RuntimeDriverError> {
        let control_phase = self.phase;
        match control_phase.can_accept_input() {
            true => {}
            false => {
                return Err(RuntimeDriverError::NotReady {
                    state: control_phase,
                });
            }
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
                        tracing::warn!(
                            input_id = ?input_id,
                            existing_id = ?existing_id,
                            error = %err,
                            "ingress authority rejected AdmitDeduplicated"
                        );
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
        let runtime_idle = self.phase.is_idle_or_attached();
        let mut policy = DefaultPolicyTable::resolve(&input, runtime_idle);
        crate::silent_intent::apply_silent_intent_override(
            &input,
            &self.silent_comms_intents,
            &mut policy,
        );
        if let Some(s) = self.ledger.get_mut(&input_id) {
            s.policy = Some(PolicySnapshot {
                version: policy.policy_version,
                decision: policy.clone(),
            });
        }
        self.emit_event(RuntimeEvent::InputLifecycle(
            InputLifecycleEvent::Accepted {
                input_id: input_id.clone(),
            },
        ));
        // --- Ingress authority: authority-owned classification ---
        // All mechanical lookups done unconditionally; authority.admit() decides
        // the admission path (queued vs consumed-on-accept) based on the policy.
        // Zero policy branching in shell code.
        let handling_mode = handling_mode_from_policy(&policy);
        let request_immediate_processing = requests_immediate_processing(&input);
        let content_shape = ContentShape(input.kind_id().to_string());
        let is_prompt = matches!(input, Input::Prompt(_));
        let existing_superseded_id = self.existing_superseded_input(&input).map(|(id, _)| id);
        match self.ingress.admit(
            input_id.clone(),
            content_shape,
            handling_mode,
            request_immediate_processing,
            is_prompt,
            None, // request_id
            None, // reservation_key
            policy.clone(),
            existing_superseded_id,
        ) {
            Ok(transition) => {
                self.process_accept_effects(&transition.effects, &input);
            }
            Err(err) => {
                tracing::warn!(
                    input_id = ?input_id,
                    error = %err,
                    "ingress authority rejected accept_input"
                );
            }
        }
        // Wake/process decisions are driven by the ingress authority effects
        // (WakeRuntime / RequestImmediateProcessing). The authority respects
        // WakeMode::None and only emits these effects when the policy allows.
        // No shell-side override here — that would bypass the authority.
        let final_state = self.ledger.get(&input_id).cloned().unwrap_or_else(|| state);
        Ok(AcceptOutcome::Accepted {
            input_id,
            policy,
            state: final_state,
        })
    }

    async fn on_runtime_event(
        &mut self,
        _event: RuntimeEventEnvelope,
    ) -> Result<(), RuntimeDriverError> {
        Ok(())
    }

    async fn on_run_event(&mut self, event: RunEvent) -> Result<(), RuntimeDriverError> {
        match event {
            RunEvent::RunCompleted {
                run_id,
                consumed_input_ids,
            } => {
                // Ingress authority: RunCompleted — always call; authority rejects if run doesn't match.
                match self.ingress.apply(RuntimeIngressInput::RunCompleted {
                    run_id: run_id.clone(),
                }) {
                    Ok(transition) => {
                        self.process_ingress_effects(&transition.effects);
                    }
                    Err(err) => {
                        tracing::warn!(
                            run_id = ?run_id,
                            error = %err,
                            "ingress authority rejected RunCompleted"
                        );
                    }
                }

                self.consume_inputs(&consumed_input_ids, &run_id)
                    .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?;
                self.finish_run_from_current_phase(&run_id)
                    .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?;
            }
            RunEvent::RunFailed { ref run_id, .. } => {
                // Ingress authority: RunFailed — always call; authority rejects if run doesn't match.
                match self.ingress.apply(RuntimeIngressInput::RunFailed {
                    run_id: run_id.clone(),
                }) {
                    Ok(transition) => {
                        self.process_ingress_effects(&transition.effects);
                    }
                    Err(err) => {
                        tracing::warn!(
                            run_id = ?run_id,
                            error = %err,
                            "ingress authority rejected RunFailed"
                        );
                    }
                }

                let staged_ids: Vec<InputId> = self
                    .ledger
                    .iter()
                    .filter(|(_, s)| s.current_state() == InputLifecycleState::Staged)
                    .map(|(id, _)| id.clone())
                    .collect();
                self.rollback_staged(&staged_ids)
                    .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?;
                // Wake decision is authority-driven: the ingress authority emits
                // WakeRuntime when rolled-back inputs are re-enqueued (above).
                self.finish_run_from_current_phase(run_id)
                    .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?;
            }
            RunEvent::RunCancelled { ref run_id, .. } => {
                // Ingress authority: RunCancelled — always call; authority rejects if run doesn't match.
                match self.ingress.apply(RuntimeIngressInput::RunCancelled {
                    run_id: run_id.clone(),
                }) {
                    Ok(transition) => {
                        self.process_ingress_effects(&transition.effects);
                    }
                    Err(err) => {
                        tracing::warn!(
                            run_id = ?run_id,
                            error = %err,
                            "ingress authority rejected RunCancelled"
                        );
                    }
                }

                let staged_ids: Vec<InputId> = self
                    .ledger
                    .iter()
                    .filter(|(_, s)| s.current_state() == InputLifecycleState::Staged)
                    .map(|(id, _)| id.clone())
                    .collect();
                self.rollback_staged(&staged_ids)
                    .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?;
                // Wake decision is authority-driven: the ingress authority emits
                // WakeRuntime when rolled-back inputs are re-enqueued (above).
                self.finish_run_from_current_phase(run_id)
                    .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?;
            }
            RunEvent::RunStarted { .. } => {}
            RunEvent::BoundaryApplied {
                run_id, receipt, ..
            } => {
                // Ingress authority: BoundaryApplied
                if self.ingress.current_run() == Some(&run_id) {
                    match self.ingress.apply(RuntimeIngressInput::BoundaryApplied {
                        run_id: run_id.clone(),
                        boundary_sequence: receipt.sequence,
                    }) {
                        Ok(transition) => {
                            self.process_ingress_effects(&transition.effects);
                        }
                        Err(err) => {
                            tracing::warn!(
                                run_id = ?run_id,
                                boundary_sequence = receipt.sequence,
                                error = %err,
                                "ingress authority rejected BoundaryApplied"
                            );
                        }
                    }
                }

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
            }
            _ => {}
        }
        Ok(())
    }

    async fn on_runtime_control(
        &mut self,
        command: RuntimeControlCommand,
    ) -> Result<(), RuntimeDriverError> {
        match command {
            RuntimeControlCommand::Stop => {
                // Ingress authority: Stop (abandon everything while preserving
                // the distinction between a stopped runtime and a destroyed one).
                match self.ingress.apply(RuntimeIngressInput::Stop) {
                    Ok(transition) => {
                        self.process_ingress_effects(&transition.effects);
                    }
                    Err(err) => {
                        tracing::warn!(
                            error = %err,
                            "ingress authority rejected Stop on Stop"
                        );
                    }
                }

                match self.phase {
                    RuntimeState::Initializing
                    | RuntimeState::Idle
                    | RuntimeState::Attached
                    | RuntimeState::Running
                    | RuntimeState::Retired => {
                        self.current_run_id = None;
                        self.pre_run_phase = None;
                        self.transition_phase(RuntimeState::Stopped);
                    }
                    from => {
                        return Err(RuntimeDriverError::Internal(
                            crate::runtime_state::RuntimeStateTransitionError {
                                from,
                                to: RuntimeState::Stopped,
                            }
                            .to_string(),
                        ));
                    }
                }
                self.abandon_all_non_terminal(InputAbandonReason::Stopped);
                self.queue.drain();
                self.steer_queue.drain();
            }
        }
        Ok(())
    }

    async fn recover(&mut self) -> Result<RecoveryReport, RuntimeDriverError> {
        Ok(self.recover_ephemeral())
    }
    fn runtime_state(&self) -> RuntimeState {
        self.phase
    }
    async fn retire(&mut self) -> Result<RetireReport, RuntimeDriverError> {
        EphemeralRuntimeDriver::retire(self)
    }
    async fn reset(&mut self) -> Result<ResetReport, RuntimeDriverError> {
        EphemeralRuntimeDriver::reset(self)
    }
    async fn destroy(&mut self) -> Result<DestroyReport, RuntimeDriverError> {
        let abandoned = EphemeralRuntimeDriver::destroy(self)?;
        Ok(DestroyReport {
            inputs_abandoned: abandoned,
        })
    }
    fn input_state(&self, input_id: &InputId) -> Option<&InputState> {
        self.ledger.get(input_id)
    }
    fn active_input_ids(&self) -> Vec<InputId> {
        self.ledger.active_input_ids()
    }
}

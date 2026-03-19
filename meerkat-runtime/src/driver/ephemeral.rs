//! EphemeralRuntimeDriver -- in-memory runtime driver for ephemeral sessions.
//!
//! Implements `RuntimeDriver` with:
//! - Input acceptance with validation, policy resolution, dedup, supersession
//! - InputState lifecycle management via InputLifecycleAuthority
//! - InputQueue FIFO management
//! - S24 ephemeral recovery
//! - S25 retire/reset/destroy lifecycle operations

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
use crate::runtime_control_authority::{
    RuntimeControlAuthority, RuntimeControlInput, RuntimeControlMutator,
};
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

/// Derive the handling mode from a policy decision's routing disposition.
pub(crate) fn handling_mode_from_policy(policy: &crate::policy::PolicyDecision) -> HandlingMode {
    match policy.routing_disposition {
        RoutingDisposition::Steer => HandlingMode::Steer,
        _ => HandlingMode::Queue,
    }
}

/// Ephemeral runtime driver -- all state in-memory.
#[derive(Clone)]
pub struct EphemeralRuntimeDriver {
    runtime_id: LogicalRuntimeId,
    /// Canonical RMAT/MTAS authority for runtime control state.
    /// Single source of truth for phase transitions (Idle/Attached/Running/Retired/etc.).
    control: RuntimeControlAuthority,
    ledger: InputLedger,
    queue: InputQueue,
    steer_queue: InputQueue,
    events: Vec<RuntimeEventEnvelope>,
    wake_requested: bool,
    process_requested: bool,
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
            control: RuntimeControlAuthority::from_state(RuntimeState::Idle),
            ledger: InputLedger::new(),
            queue: InputQueue::new(),
            steer_queue: InputQueue::new(),
            events: Vec::new(),
            wake_requested: false,
            process_requested: false,
            silent_comms_intents: Vec::new(),
            ingress: RuntimeIngressAuthority::new(),
        }
    }

    pub fn set_silent_comms_intents(&mut self, intents: Vec<String>) {
        self.silent_comms_intents = intents;
    }

    /// Get a reference to the ingress authority.
    pub fn ingress(&self) -> &RuntimeIngressAuthority {
        &self.ingress
    }

    /// Admit a store-recovered input into the ingress authority's tracking.
    /// Called by the persistent driver during crash recovery to ensure the
    /// authority knows about inputs loaded from the store before `Recover` fires.
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
                    self.wake_requested = true;
                }
                RuntimeIngressEffect::RequestImmediateProcessing => {
                    self.process_requested = true;
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
    }

    pub fn is_idle(&self) -> bool {
        self.control.is_idle()
    }
    pub fn is_idle_or_attached(&self) -> bool {
        self.control.phase().is_idle_or_attached()
    }

    pub fn attach(&mut self) -> Result<(), crate::runtime_state::RuntimeStateTransitionError> {
        let transition = self.control.apply(RuntimeControlInput::AttachExecutor)?;
        self.emit_event(RuntimeEvent::RuntimeStateChange(RuntimeStateChangeEvent {
            from: transition.from_phase,
            to: transition.next_phase,
        }));
        Ok(())
    }
    pub fn detach(
        &mut self,
    ) -> Result<Option<RuntimeState>, crate::runtime_state::RuntimeStateTransitionError> {
        if self.control.is_attached() {
            let transition = self.control.apply(RuntimeControlInput::DetachExecutor)?;
            self.emit_event(RuntimeEvent::RuntimeStateChange(RuntimeStateChangeEvent {
                from: transition.from_phase,
                to: transition.next_phase,
            }));
            Ok(Some(RuntimeState::Attached))
        } else {
            Ok(None)
        }
    }
    pub fn start_run(
        &mut self,
        run_id: RunId,
    ) -> Result<(), crate::runtime_state::RuntimeStateTransitionError> {
        let _transition = self
            .control
            .apply(RuntimeControlInput::BeginRun { run_id })?;
        Ok(())
    }
    pub fn complete_run(
        &mut self,
    ) -> Result<RunId, crate::runtime_state::RuntimeStateTransitionError> {
        let run_id = self.control.current_run_id().cloned().ok_or(
            crate::runtime_state::RuntimeStateTransitionError {
                from: self.control.phase(),
                to: RuntimeState::Idle,
            },
        )?;
        let _transition = self.control.apply(RuntimeControlInput::RunCompleted {
            run_id: run_id.clone(),
        })?;
        Ok(run_id)
    }
    pub fn take_wake_requested(&mut self) -> bool {
        std::mem::take(&mut self.wake_requested)
    }
    pub fn take_process_requested(&mut self) -> bool {
        std::mem::take(&mut self.process_requested)
    }
    pub fn drain_events(&mut self) -> Vec<RuntimeEventEnvelope> {
        std::mem::take(&mut self.events)
    }
    pub fn queue(&self) -> &InputQueue {
        &self.queue
    }
    pub fn queue_mut(&mut self) -> &mut InputQueue {
        &mut self.queue
    }
    pub fn steer_queue(&self) -> &InputQueue {
        &self.steer_queue
    }
    pub fn steer_queue_mut(&mut self) -> &mut InputQueue {
        &mut self.steer_queue
    }
    pub fn enqueue_recovered_input(&mut self, input_id: InputId, input: Input) {
        self.queue.enqueue(input_id, input);
    }
    pub fn enqueue_front_input(&mut self, input_id: InputId, input: Input) {
        self.queue.enqueue_front(input_id, input);
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
        self.queue
            .input_ids()
            .iter()
            .any(|queued_id| queued_id == input_id)
            || self
                .steer_queue
                .input_ids()
                .iter()
                .any(|queued_id| queued_id == input_id)
    }
    pub fn has_queued_input_outside(&self, excluded: &[InputId]) -> bool {
        self.queue
            .input_ids()
            .iter()
            .chain(self.steer_queue.input_ids().iter())
            .any(|queued_id| !excluded.iter().any(|excluded_id| excluded_id == queued_id))
    }
    fn existing_superseded_input(
        &self,
        input: &Input,
    ) -> Option<(InputId, crate::coalescing::CoalescingResult)> {
        self.queue
            .input_ids()
            .into_iter()
            .chain(self.steer_queue.input_ids())
            .find_map(|queued_id| {
                let existing = self.ledger.get(&queued_id)?.persisted_input.as_ref()?;
                let result =
                    crate::coalescing::check_supersession(input, existing, &self.runtime_id);
                match result {
                    crate::coalescing::CoalescingResult::Supersedes { .. } => {
                        Some((queued_id, result))
                    }
                    crate::coalescing::CoalescingResult::Standalone => None,
                }
            })
    }
    pub fn remove_input(&mut self, input_id: &InputId) {
        let _ = self.queue.remove(input_id);
        let _ = self.steer_queue.remove(input_id);
        let _ = self.ledger.remove(input_id);
    }
    pub fn ledger(&self) -> &InputLedger {
        &self.ledger
    }
    pub fn ledger_mut(&mut self) -> &mut InputLedger {
        &mut self.ledger
    }
    pub fn input_states_snapshot(&self) -> Vec<InputState> {
        self.ledger.iter().map(|(_, state)| state.clone()).collect()
    }
    pub fn forget_input(&mut self, input_id: &InputId) {
        let _ = self.queue.remove(input_id);
        let _ = self.steer_queue.remove(input_id);
        let _ = self.ledger.remove(input_id);
        self.wake_requested = false;
        self.process_requested = false;
    }
    /// Get a reference to the runtime control authority.
    pub fn control(&self) -> &RuntimeControlAuthority {
        &self.control
    }
    /// Get a mutable reference to the runtime control authority.
    pub fn control_mut(&mut self) -> &mut RuntimeControlAuthority {
        &mut self.control
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
        // Ingress authority: StageDrainSnapshot — the authority owns the Staged
        // transition. It emits StageInput effects that drive the per-input lifecycle
        // authority and Staged events (handled in process_ingress_effects).
        // No independent shell StageForRun call — single source of truth.
        match self.ingress.apply(RuntimeIngressInput::StageDrainSnapshot {
            run_id: run_id.clone(),
            contributing_work_ids: vec![input_id.clone()],
        }) {
            Ok(transition) => {
                self.process_ingress_effects(&transition.effects);
            }
            Err(err) => {
                tracing::warn!(
                    input_id = ?input_id,
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
            if let Some(state) = self.ledger.get_mut(input_id)
                && state.current_state() == InputLifecycleState::AppliedPendingConsumption
            {
                state.apply(InputLifecycleInput::Consume)?;
                self.events
                    .push(self.make_envelope(RuntimeEvent::InputLifecycle(
                        InputLifecycleEvent::Consumed {
                            input_id: input_id.clone(),
                            run_id: run_id.clone(),
                        },
                    )));
            }
        }
        Ok(())
    }

    pub fn rollback_staged(&mut self, input_ids: &[InputId]) -> Result<(), InputLifecycleError> {
        for input_id in input_ids {
            let mut requeue_input = None;
            let mut is_steer = false;
            if let Some(state) = self.ledger.get_mut(input_id)
                && state.current_state() == InputLifecycleState::Staged
            {
                state.apply(InputLifecycleInput::RollbackStaged)?;
                requeue_input = state.persisted_input.clone();
                is_steer = requeue_input
                    .as_ref()
                    .and_then(super::super::input::Input::handling_mode)
                    == Some(HandlingMode::Steer);
            }
            if !self.has_queued_input(input_id)
                && let Some(input) = requeue_input
            {
                if is_steer {
                    self.steer_queue.enqueue(input_id.clone(), input);
                } else {
                    self.queue.enqueue(input_id.clone(), input);
                }
            }
        }
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

        let transition = self
            .control
            .apply(RuntimeControlInput::RetireRequested)
            .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?;
        self.emit_event(RuntimeEvent::RuntimeStateChange(RuntimeStateChangeEvent {
            from: transition.from_phase,
            to: transition.next_phase,
        }));
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
        self.wake_requested = false;
        self.process_requested = false;
        let from_phase = self.control.phase();
        if from_phase != RuntimeState::Idle {
            let transition = self
                .control
                .apply(RuntimeControlInput::ResetRequested)
                .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?;
            self.emit_event(RuntimeEvent::RuntimeStateChange(RuntimeStateChangeEvent {
                from: transition.from_phase,
                to: transition.next_phase,
            }));
        }
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

        let transition = self
            .control
            .apply(RuntimeControlInput::DestroyRequested)
            .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?;
        self.emit_event(RuntimeEvent::RuntimeStateChange(RuntimeStateChangeEvent {
            from: transition.from_phase,
            to: transition.next_phase,
        }));
        let abandoned = self.abandon_all_non_terminal(InputAbandonReason::Destroyed);
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

        RecoveryReport {
            inputs_recovered: recovered,
            inputs_abandoned: abandoned,
            inputs_requeued: requeued,
            details: Vec::new(),
        }
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
        self.wake_requested = false;
        self.process_requested = false;
        abandoned
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl crate::traits::RuntimeDriver for EphemeralRuntimeDriver {
    async fn accept_input(&mut self, input: Input) -> Result<AcceptOutcome, RuntimeDriverError> {
        if !self.control.phase().can_accept_input() {
            return Err(RuntimeDriverError::NotReady {
                state: self.control.phase(),
            });
        }
        if let Err(e) = validate_durability(&input) {
            match e {
                DurabilityError::DerivedForbidden { .. }
                | DurabilityError::ExternalDerivedForbidden => {
                    return Ok(AcceptOutcome::Rejected {
                        reason: e.to_string(),
                    });
                }
            }
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
        let runtime_idle = self.control.phase().is_idle_or_attached();
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
        let content_shape = ContentShape(input.kind_id().to_string());
        let is_prompt = matches!(input, Input::Prompt(_));
        let existing_superseded_id = self.existing_superseded_input(&input).map(|(id, _)| id);
        match self.ingress.admit(
            input_id.clone(),
            content_shape,
            handling_mode,
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
                self.control
                    .apply(RuntimeControlInput::RunCompleted {
                        run_id: run_id.clone(),
                    })
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
                self.control
                    .apply(RuntimeControlInput::RunFailed {
                        run_id: run_id.clone(),
                    })
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
                self.control
                    .apply(RuntimeControlInput::RunCancelled {
                        run_id: run_id.clone(),
                    })
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
                    if let Some(state) = self.ledger.get_mut(input_id)
                        && state.current_state() == InputLifecycleState::Staged
                    {
                        let _ = state.apply(InputLifecycleInput::MarkApplied {
                            run_id: run_id.clone(),
                        });
                        let _ = state.apply(InputLifecycleInput::MarkAppliedPendingConsumption {
                            boundary_sequence: receipt.sequence,
                        });
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
                // Ingress authority: Destroy (Stop abandons everything)
                if !self.ingress.is_terminal() {
                    match self.ingress.apply(RuntimeIngressInput::Destroy) {
                        Ok(transition) => {
                            self.process_ingress_effects(&transition.effects);
                        }
                        Err(err) => {
                            tracing::warn!(
                                error = %err,
                                "ingress authority rejected Destroy on Stop"
                            );
                        }
                    }
                }

                let transition = self
                    .control
                    .apply(RuntimeControlInput::StopRequested)
                    .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?;
                self.emit_event(RuntimeEvent::RuntimeStateChange(RuntimeStateChangeEvent {
                    from: transition.from_phase,
                    to: transition.next_phase,
                }));
                self.abandon_all_non_terminal(InputAbandonReason::Destroyed);
                self.queue.drain();
                self.steer_queue.drain();
            }
            RuntimeControlCommand::Resume => {
                if self.control.phase() == RuntimeState::Recovering {
                    self.control
                        .apply(RuntimeControlInput::ResumeRequested)
                        .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?;
                }
            }
        }
        Ok(())
    }

    async fn recover(&mut self) -> Result<RecoveryReport, RuntimeDriverError> {
        Ok(self.recover_ephemeral())
    }
    fn runtime_state(&self) -> RuntimeState {
        self.control.phase()
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

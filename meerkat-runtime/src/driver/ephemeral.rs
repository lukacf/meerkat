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
use crate::policy::{
    ApplyMode, ConsumePoint, DrainPolicy, QueueMode, RoutingDisposition, WakeMode,
};
use crate::policy_table::DefaultPolicyTable;
use crate::queue::InputQueue;
use crate::runtime_event::{
    InputLifecycleEvent, RuntimeEvent, RuntimeEventEnvelope, RuntimeStateChangeEvent,
};
use crate::runtime_ingress_authority::{
    ContentShape, RuntimeIngressAuthority, RuntimeIngressEffect, RuntimeIngressInput,
    RuntimeIngressMutator,
};
use crate::runtime_state::RuntimeState;
use crate::state_machine::RuntimeStateMachine;
use crate::traits::{
    DestroyReport, RecoveryReport, ResetReport, RetireReport, RuntimeControlCommand,
    RuntimeDriverError,
};

/// Derive the handling mode from a policy decision's routing disposition.
fn handling_mode_from_policy(policy: &crate::policy::PolicyDecision) -> HandlingMode {
    match policy.routing_disposition {
        RoutingDisposition::Steer => HandlingMode::Steer,
        _ => HandlingMode::Queue,
    }
}

/// Ephemeral runtime driver -- all state in-memory.
#[derive(Clone)]
pub struct EphemeralRuntimeDriver {
    runtime_id: LogicalRuntimeId,
    state_machine: RuntimeStateMachine,
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
        let sm = RuntimeStateMachine::from_state(RuntimeState::Idle);
        Self {
            runtime_id,
            state_machine: sm,
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
            }
        }
    }
    pub fn is_idle(&self) -> bool {
        self.state_machine.is_idle()
    }
    pub fn is_idle_or_attached(&self) -> bool {
        self.state_machine.state().is_idle_or_attached()
    }

    pub fn attach(&mut self) -> Result<(), crate::runtime_state::RuntimeStateTransitionError> {
        self.state_machine.attach()?;
        self.emit_event(RuntimeEvent::RuntimeStateChange(RuntimeStateChangeEvent {
            from: RuntimeState::Idle,
            to: RuntimeState::Attached,
        }));
        Ok(())
    }
    pub fn detach(
        &mut self,
    ) -> Result<Option<RuntimeState>, crate::runtime_state::RuntimeStateTransitionError> {
        let result = self.state_machine.detach()?;
        if result.is_some() {
            self.emit_event(RuntimeEvent::RuntimeStateChange(RuntimeStateChangeEvent {
                from: RuntimeState::Attached,
                to: RuntimeState::Idle,
            }));
        }
        Ok(result)
    }
    pub fn start_run(
        &mut self,
        run_id: RunId,
    ) -> Result<(), crate::runtime_state::RuntimeStateTransitionError> {
        self.state_machine.start_run(run_id)
    }
    pub fn complete_run(
        &mut self,
    ) -> Result<RunId, crate::runtime_state::RuntimeStateTransitionError> {
        self.state_machine.complete_run()
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
    pub fn state_machine_ref(&self) -> &RuntimeStateMachine {
        &self.state_machine
    }
    pub fn state_machine_mut(&mut self) -> &mut RuntimeStateMachine {
        &mut self.state_machine
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
        // Ingress authority: StageDrainSnapshot (single contributor)
        // Note: callers may stage inputs one-at-a-time, so we apply
        // StageDrainSnapshot only if the authority has no current run yet.
        // If a run is already in progress, the authority was already notified.
        if self.ingress.current_run().is_none() {
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
                }
            }
        }

        let state =
            self.ledger
                .get_mut(input_id)
                .ok_or(InputLifecycleError::InvalidTransition {
                    from: InputLifecycleState::Accepted,
                    input: "StageForRun (input not found)".into(),
                })?;
        state.apply(InputLifecycleInput::StageForRun {
            run_id: run_id.clone(),
        })?;
        self.emit_event(RuntimeEvent::InputLifecycle(InputLifecycleEvent::Staged {
            input_id: input_id.clone(),
            run_id: run_id.clone(),
        }));
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

        let from = self.state_machine.state();
        self.state_machine
            .transition(RuntimeState::Retired)
            .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?;
        self.emit_event(RuntimeEvent::RuntimeStateChange(RuntimeStateChangeEvent {
            from,
            to: RuntimeState::Retired,
        }));
        let inputs_pending_drain = self.ledger.iter().filter(|(_, s)| !s.is_terminal()).count();
        Ok(RetireReport {
            inputs_abandoned: 0,
            inputs_pending_drain,
        })
    }

    pub fn reset(&mut self) -> Result<ResetReport, RuntimeDriverError> {
        if self.state_machine.is_running() {
            return Err(RuntimeDriverError::Internal(
                "cannot reset while running".into(),
            ));
        }

        // Ingress authority: Reset
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
        if let Some(from) = self
            .state_machine
            .reset_to_idle()
            .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?
        {
            self.emit_event(RuntimeEvent::RuntimeStateChange(RuntimeStateChangeEvent {
                from,
                to: RuntimeState::Idle,
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

        let from = self.state_machine.state();
        self.state_machine
            .transition(RuntimeState::Destroyed)
            .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?;
        self.emit_event(RuntimeEvent::RuntimeStateChange(RuntimeStateChangeEvent {
            from,
            to: RuntimeState::Destroyed,
        }));
        let abandoned = self.abandon_all_non_terminal(InputAbandonReason::Destroyed);
        Ok(abandoned)
    }

    pub fn recover_ephemeral(&mut self) -> RecoveryReport {
        // Ingress authority: Recover
        match self.ingress.apply(RuntimeIngressInput::Recover) {
            Ok(transition) => {
                self.process_ingress_effects(&transition.effects);
            }
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "ingress authority rejected Recover"
                );
            }
        }

        let mut recovered = 0;
        let mut abandoned = 0;
        let mut requeued = 0;
        let input_ids: Vec<InputId> = self
            .ledger
            .iter()
            .filter(|(_, s)| !s.is_terminal())
            .map(|(id, _)| id.clone())
            .collect();
        for input_id in input_ids {
            if let Some(state) = self.ledger.get_mut(&input_id) {
                match state.current_state() {
                    InputLifecycleState::Accepted => {
                        if let Some(ref policy) = state.policy {
                            if policy.decision.apply_mode == ApplyMode::Ignore
                                && policy.decision.consume_point == ConsumePoint::OnAccept
                            {
                                let _ = state.apply(InputLifecycleInput::ConsumeOnAccept);
                                abandoned += 1;
                            } else {
                                let _ = state.apply(InputLifecycleInput::QueueAccepted);
                                requeued += 1;
                            }
                        } else {
                            let _ = state.apply(InputLifecycleInput::QueueAccepted);
                            requeued += 1;
                        }
                        recovered += 1;
                    }
                    InputLifecycleState::Staged => {
                        let _ = state.apply(InputLifecycleInput::RollbackStaged);
                        recovered += 1;
                        requeued += 1;
                    }
                    InputLifecycleState::Applied
                    | InputLifecycleState::AppliedPendingConsumption => {
                        recovered += 1;
                    }
                    InputLifecycleState::Queued => {
                        recovered += 1;
                    }
                    _ => {}
                }
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
        if !self.state_machine.state().can_accept_input() {
            return Err(RuntimeDriverError::NotReady {
                state: self.state_machine.state(),
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
        let runtime_idle = self.state_machine.state().is_idle_or_attached();
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
        // --- Ingress authority: classify and apply ---
        let handling_mode = handling_mode_from_policy(&policy);
        let content_shape = ContentShape(input.kind_id().to_string());
        let ingress_input = if policy.apply_mode == ApplyMode::Ignore
            && policy.consume_point == ConsumePoint::OnAccept
        {
            RuntimeIngressInput::AdmitConsumedOnAccept {
                work_id: input_id.clone(),
                content_shape,
                request_id: None,
                reservation_key: None,
                policy: policy.clone(),
            }
        } else {
            RuntimeIngressInput::AdmitQueued {
                work_id: input_id.clone(),
                content_shape,
                handling_mode,
                request_id: None,
                reservation_key: None,
                policy: policy.clone(),
            }
        };
        match self.ingress.apply(ingress_input) {
            Ok(transition) => {
                self.process_ingress_effects(&transition.effects);
            }
            Err(err) => {
                tracing::warn!(
                    input_id = ?input_id,
                    error = %err,
                    "ingress authority rejected accept_input — continuing with existing logic"
                );
            }
        }

        match policy.apply_mode {
            ApplyMode::Ignore => {
                if policy.consume_point == ConsumePoint::OnAccept
                    && let Some(s) = self.ledger.get_mut(&input_id)
                {
                    let _ = s.apply(InputLifecycleInput::ConsumeOnAccept);
                }
            }
            ApplyMode::InjectNow => {
                if let Some(s) = self.ledger.get_mut(&input_id) {
                    s.persisted_input = Some(input.clone());
                    let _ = s.apply(InputLifecycleInput::QueueAccepted);
                }
                self.enqueue_to(handling_mode, input_id.clone(), input);
                self.emit_event(RuntimeEvent::InputLifecycle(InputLifecycleEvent::Queued {
                    input_id: input_id.clone(),
                }));
            }
            ApplyMode::StageRunStart | ApplyMode::StageRunBoundary => {
                if let Some(s) = self.ledger.get_mut(&input_id) {
                    s.persisted_input = Some(input.clone());
                    let _ = s.apply(InputLifecycleInput::QueueAccepted);
                }
                match policy.queue_mode {
                    QueueMode::Coalesce => {
                        if let Some((existing_id, _)) = self.existing_superseded_input(&input) {
                            // Remove from both queues — the existing input might be
                            // in either queue depending on its handling mode.
                            let _ = self.queue.remove(&existing_id);
                            let _ = self.steer_queue.remove(&existing_id);
                            if let Some(existing_state) = self.ledger.get_mut(&existing_id) {
                                let _ = crate::coalescing::apply_coalescing(
                                    existing_state,
                                    input_id.clone(),
                                );
                            }
                        }
                        self.enqueue_to(handling_mode, input_id.clone(), input);
                    }
                    QueueMode::Supersede => {
                        if let Some((existing_id, _)) = self.existing_superseded_input(&input) {
                            let _ = self.queue.remove(&existing_id);
                            let _ = self.steer_queue.remove(&existing_id);
                            if let Some(existing_state) = self.ledger.get_mut(&existing_id) {
                                let _ = crate::coalescing::apply_supersession(
                                    existing_state,
                                    input_id.clone(),
                                );
                            }
                        }
                        self.enqueue_to(handling_mode, input_id.clone(), input);
                    }
                    QueueMode::Priority => {
                        self.enqueue_front_to(handling_mode, input_id.clone(), input);
                    }
                    QueueMode::Fifo | QueueMode::None => {
                        self.enqueue_to(handling_mode, input_id.clone(), input);
                    }
                }
                self.emit_event(RuntimeEvent::InputLifecycle(InputLifecycleEvent::Queued {
                    input_id: input_id.clone(),
                }));
            }
        }
        if runtime_idle && policy.wake_mode == WakeMode::WakeIfIdle {
            self.wake_requested = true;
        }
        if runtime_idle
            && matches!(
                policy.drain_policy,
                DrainPolicy::Immediate | DrainPolicy::SteerBatch
            )
        {
            self.process_requested = true;
        }
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
                // Ingress authority: RunCompleted
                if let Some(current_run) = self.ingress.current_run().cloned()
                    && current_run == run_id
                {
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
                }

                self.consume_inputs(&consumed_input_ids, &run_id)
                    .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?;
                self.state_machine
                    .complete_run()
                    .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?;
            }
            RunEvent::RunFailed { ref run_id, .. } => {
                // Ingress authority: RunFailed
                if let Some(current_run) = self.ingress.current_run().cloned()
                    && current_run == *run_id
                {
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
                }

                let staged_ids: Vec<InputId> = self
                    .ledger
                    .iter()
                    .filter(|(_, s)| s.current_state() == InputLifecycleState::Staged)
                    .map(|(id, _)| id.clone())
                    .collect();
                self.rollback_staged(&staged_ids)
                    .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?;
                if self.has_queued_input_outside(&staged_ids) {
                    self.wake_requested = true;
                }
                self.state_machine
                    .complete_run()
                    .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?;
            }
            RunEvent::RunCancelled { ref run_id, .. } => {
                // Ingress authority: RunCancelled
                if let Some(current_run) = self.ingress.current_run().cloned()
                    && current_run == *run_id
                {
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
                }

                let staged_ids: Vec<InputId> = self
                    .ledger
                    .iter()
                    .filter(|(_, s)| s.current_state() == InputLifecycleState::Staged)
                    .map(|(id, _)| id.clone())
                    .collect();
                self.rollback_staged(&staged_ids)
                    .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?;
                if self.has_queued_input_outside(&staged_ids) {
                    self.wake_requested = true;
                }
                self.state_machine
                    .complete_run()
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

                let from = self.state_machine.state();
                self.state_machine
                    .transition(RuntimeState::Stopped)
                    .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?;
                self.emit_event(RuntimeEvent::RuntimeStateChange(RuntimeStateChangeEvent {
                    from,
                    to: RuntimeState::Stopped,
                }));
                self.abandon_all_non_terminal(InputAbandonReason::Destroyed);
                self.queue.drain();
                self.steer_queue.drain();
            }
            RuntimeControlCommand::Resume => {
                if self.state_machine.state() == RuntimeState::Recovering {
                    self.state_machine
                        .transition(RuntimeState::Idle)
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
        self.state_machine.state()
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

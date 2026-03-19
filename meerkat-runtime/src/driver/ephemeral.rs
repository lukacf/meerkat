//! EphemeralRuntimeDriver -- in-memory runtime driver for ephemeral sessions.
//!
//! Implements `RuntimeDriver` with:
//! - Input acceptance with validation, policy resolution, dedup, supersession
//! - InputState lifecycle management via InputLifecycleAuthority
//! - InputQueue FIFO management
//! - S24 ephemeral recovery
//! - S25 retire/reset/destroy lifecycle operations

use meerkat_core::lifecycle::{InputId, RunEvent, RunId};

use crate::accept::AcceptOutcome;
use crate::durability::{DurabilityError, validate_durability};
use crate::identifiers::LogicalRuntimeId;
use crate::input::Input;
use crate::input_ledger::InputLedger;
use crate::input_lifecycle_authority::{InputLifecycleError, InputLifecycleInput};
use crate::input_state::{InputAbandonReason, InputLifecycleState, InputState, PolicySnapshot};
use crate::policy::{ApplyMode, ConsumePoint, DrainPolicy, QueueMode, WakeMode};
use crate::policy_table::DefaultPolicyTable;
use crate::queue::InputQueue;
use crate::runtime_event::{
    InputLifecycleEvent, RuntimeEvent, RuntimeEventEnvelope, RuntimeStateChangeEvent,
};
use crate::runtime_state::RuntimeState;
use crate::state_machine::RuntimeStateMachine;
use crate::traits::{
    DestroyReport, RecoveryReport, ResetReport, RetireReport, RuntimeControlCommand,
    RuntimeDriverError,
};

/// Ephemeral runtime driver -- all state in-memory.
#[derive(Clone)]
pub struct EphemeralRuntimeDriver {
    runtime_id: LogicalRuntimeId,
    state_machine: RuntimeStateMachine,
    ledger: InputLedger,
    queue: InputQueue,
    events: Vec<RuntimeEventEnvelope>,
    wake_requested: bool,
    process_requested: bool,
    silent_comms_intents: Vec<String>,
}

impl EphemeralRuntimeDriver {
    pub fn new(runtime_id: LogicalRuntimeId) -> Self {
        let sm = RuntimeStateMachine::from_state(RuntimeState::Idle);
        Self {
            runtime_id,
            state_machine: sm,
            ledger: InputLedger::new(),
            queue: InputQueue::new(),
            events: Vec::new(),
            wake_requested: false,
            process_requested: false,
            silent_comms_intents: Vec::new(),
        }
    }

    pub fn set_silent_comms_intents(&mut self, intents: Vec<String>) {
        self.silent_comms_intents = intents;
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
    pub fn enqueue_recovered_input(&mut self, input_id: InputId, input: Input) {
        self.queue.enqueue(input_id, input);
    }
    pub fn enqueue_front_input(&mut self, input_id: InputId, input: Input) {
        self.queue.enqueue_front(input_id, input);
    }
    pub fn has_queued_input(&self, input_id: &InputId) -> bool {
        self.queue
            .input_ids()
            .iter()
            .any(|queued_id| queued_id == input_id)
    }
    fn has_queued_input_outside(&self, excluded: &[InputId]) -> bool {
        self.queue
            .input_ids()
            .iter()
            .any(|queued_id| !excluded.iter().any(|excluded_id| excluded_id == queued_id))
    }
    fn existing_superseded_input(
        &self,
        input: &Input,
    ) -> Option<(InputId, crate::coalescing::CoalescingResult)> {
        self.queue.input_ids().into_iter().find_map(|queued_id| {
            let existing = self.ledger.get(&queued_id)?.persisted_input.as_ref()?;
            let result = crate::coalescing::check_supersession(input, existing, &self.runtime_id);
            match result {
                crate::coalescing::CoalescingResult::Supersedes { .. } => Some((queued_id, result)),
                crate::coalescing::CoalescingResult::Standalone => None,
            }
        })
    }
    pub fn remove_input(&mut self, input_id: &InputId) {
        let _ = self.queue.remove(input_id);
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
        let queued = self.queue.dequeue()?;
        Some((queued.input_id, queued.input))
    }

    pub fn stage_input(
        &mut self,
        input_id: &InputId,
        run_id: &RunId,
    ) -> Result<(), InputLifecycleError> {
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
            if let Some(state) = self.ledger.get_mut(input_id)
                && state.current_state() == InputLifecycleState::Staged
            {
                state.apply(InputLifecycleInput::RollbackStaged)?;
                requeue_input = state.persisted_input.clone();
            }
            if !self.has_queued_input(input_id)
                && let Some(input) = requeue_input
            {
                self.queue.enqueue(input_id.clone(), input);
            }
        }
        Ok(())
    }

    pub fn retire(&mut self) -> Result<RetireReport, RuntimeDriverError> {
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
        let abandoned = self.abandon_all_non_terminal(InputAbandonReason::Reset);
        self.queue.drain();
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
                self.queue.enqueue(input_id.clone(), input);
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
                            let _ = self.queue.remove(&existing_id);
                            if let Some(existing_state) = self.ledger.get_mut(&existing_id) {
                                let _ = crate::coalescing::apply_coalescing(
                                    existing_state,
                                    input_id.clone(),
                                );
                            }
                        }
                        self.queue.enqueue(input_id.clone(), input);
                    }
                    QueueMode::Supersede => {
                        if let Some((existing_id, _)) = self.existing_superseded_input(&input) {
                            let _ = self.queue.remove(&existing_id);
                            if let Some(existing_state) = self.ledger.get_mut(&existing_id) {
                                let _ = crate::coalescing::apply_supersession(
                                    existing_state,
                                    input_id.clone(),
                                );
                            }
                        }
                        self.queue.enqueue(input_id.clone(), input);
                    }
                    QueueMode::Priority => {
                        self.queue.enqueue_front(input_id.clone(), input);
                    }
                    QueueMode::Fifo | QueueMode::None => {
                        self.queue.enqueue(input_id.clone(), input);
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
                self.consume_inputs(&consumed_input_ids, &run_id)
                    .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?;
                self.state_machine
                    .complete_run()
                    .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?;
            }
            RunEvent::RunFailed { .. } => {
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
            RunEvent::RunCancelled { .. } => {
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

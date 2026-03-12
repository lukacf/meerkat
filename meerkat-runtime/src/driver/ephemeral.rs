//! EphemeralRuntimeDriver — in-memory runtime driver for ephemeral sessions.
//!
//! Implements `RuntimeDriver` with:
//! - Input acceptance with validation, policy resolution, dedup, supersession
//! - InputState lifecycle management via InputStateMachine
//! - InputQueue FIFO management
//! - §24 ephemeral recovery
//! - §25 retire/reset/destroy lifecycle operations

use meerkat_core::lifecycle::{InputId, RunEvent, RunId};

use crate::accept::AcceptOutcome;
use crate::durability::{DurabilityError, validate_durability};
use crate::identifiers::LogicalRuntimeId;
use crate::input::Input;
use crate::input_ledger::InputLedger;
use crate::input_machine::{InputStateMachine, InputStateMachineError};
use crate::input_state::{InputAbandonReason, InputLifecycleState, InputState, PolicySnapshot};
use crate::policy::{ApplyMode, ConsumePoint, WakeMode};
use crate::policy_table::DefaultPolicyTable;
use crate::queue::InputQueue;
use crate::runtime_event::{
    InputLifecycleEvent, RuntimeEvent, RuntimeEventEnvelope, RuntimeStateChangeEvent,
};
use crate::runtime_state::RuntimeState;
use crate::state_machine::RuntimeStateMachine;
use crate::traits::{
    RecoveryReport, ResetReport, RetireReport, RuntimeControlCommand, RuntimeDriverError,
};

/// Ephemeral runtime driver — all state in-memory.
#[derive(Clone)]
pub struct EphemeralRuntimeDriver {
    /// Logical identity of this runtime.
    runtime_id: LogicalRuntimeId,
    /// Runtime state machine.
    state_machine: RuntimeStateMachine,
    /// Input state ledger.
    ledger: InputLedger,
    /// Input queue.
    queue: InputQueue,
    /// Emitted events (buffered for consumers to poll).
    events: Vec<RuntimeEventEnvelope>,
    /// Whether wake was requested.
    wake_requested: bool,
    /// Whether immediate processing was requested without a normal wake.
    process_requested: bool,
}

impl EphemeralRuntimeDriver {
    /// Create a new ephemeral runtime driver.
    pub fn new(runtime_id: LogicalRuntimeId) -> Self {
        // Ephemeral starts directly in Idle (skip Initializing)
        let sm = RuntimeStateMachine::from_state(RuntimeState::Idle);

        Self {
            runtime_id,
            state_machine: sm,
            ledger: InputLedger::new(),
            queue: InputQueue::new(),
            events: Vec::new(),
            wake_requested: false,
            process_requested: false,
        }
    }

    /// Check if the runtime is idle.
    pub fn is_idle(&self) -> bool {
        self.state_machine.is_idle()
    }

    /// Start a new run (Idle → Running).
    pub fn start_run(
        &mut self,
        run_id: RunId,
    ) -> Result<(), crate::runtime_state::RuntimeStateTransitionError> {
        self.state_machine.start_run(run_id)
    }

    /// Complete a run (Running → Idle), returning the finished RunId.
    pub fn complete_run(
        &mut self,
    ) -> Result<RunId, crate::runtime_state::RuntimeStateTransitionError> {
        self.state_machine.complete_run()
    }

    /// Check if a wake was requested (and clear the flag).
    pub fn take_wake_requested(&mut self) -> bool {
        std::mem::take(&mut self.wake_requested)
    }

    /// Check if immediate processing was requested (and clear the flag).
    pub fn take_process_requested(&mut self) -> bool {
        std::mem::take(&mut self.process_requested)
    }

    /// Get pending events (and drain them).
    pub fn drain_events(&mut self) -> Vec<RuntimeEventEnvelope> {
        std::mem::take(&mut self.events)
    }

    /// Get the queue for external inspection.
    pub fn queue(&self) -> &InputQueue {
        &self.queue
    }

    /// Enqueue a recovered input payload back into the runtime queue.
    pub fn enqueue_recovered_input(&mut self, input_id: InputId, input: Input) {
        self.queue.enqueue(input_id, input);
    }

    /// Check whether a specific input is already queued.
    pub fn has_queued_input(&self, input_id: &InputId) -> bool {
        self.queue
            .input_ids()
            .iter()
            .any(|queued_id| queued_id == input_id)
    }

    /// Check whether the queue contains work outside a specific excluded set.
    fn has_queued_input_outside(&self, excluded: &[InputId]) -> bool {
        self.queue
            .input_ids()
            .iter()
            .any(|queued_id| !excluded.iter().any(|excluded_id| excluded_id == queued_id))
    }

    /// Remove an input entirely (used when durable persistence fails).
    pub fn remove_input(&mut self, input_id: &InputId) {
        let _ = self.queue.remove(input_id);
        let _ = self.ledger.remove(input_id);
    }

    /// Get the ledger for external inspection.
    pub fn ledger(&self) -> &InputLedger {
        &self.ledger
    }

    /// Get mutable access to the ledger (for recovery injection).
    pub fn ledger_mut(&mut self) -> &mut InputLedger {
        &mut self.ledger
    }

    /// Snapshot all tracked input states.
    pub fn input_states_snapshot(&self) -> Vec<InputState> {
        self.ledger.iter().map(|(_, state)| state.clone()).collect()
    }

    /// Remove a previously accepted input from the ledger/queue.
    pub fn forget_input(&mut self, input_id: &InputId) {
        let _ = self.queue.remove(input_id);
        let _ = self.ledger.remove(input_id);
        self.wake_requested = false;
        self.process_requested = false;
    }

    /// Get mutable access to the state machine (for external lifecycle control).
    pub fn state_machine_mut(&mut self) -> &mut RuntimeStateMachine {
        &mut self.state_machine
    }

    /// Dequeue the next input for processing.
    pub fn dequeue_next(&mut self) -> Option<(InputId, Input)> {
        let queued = self.queue.dequeue()?;
        Some((queued.input_id, queued.input))
    }

    /// Stage an input (transition Queued → Staged).
    pub fn stage_input(
        &mut self,
        input_id: &InputId,
        run_id: &RunId,
    ) -> Result<(), InputStateMachineError> {
        let state =
            self.ledger
                .get_mut(input_id)
                .ok_or(InputStateMachineError::InvalidTransition {
                    from: InputLifecycleState::Accepted,
                    to: InputLifecycleState::Staged,
                })?;
        state.last_run_id = Some(run_id.clone());
        InputStateMachine::transition(state, InputLifecycleState::Staged, None)?;
        self.emit_event(RuntimeEvent::InputLifecycle(InputLifecycleEvent::Staged {
            input_id: input_id.clone(),
            run_id: run_id.clone(),
        }));
        Ok(())
    }

    /// Mark an input as applied (transition Staged → Applied).
    pub fn apply_input(
        &mut self,
        input_id: &InputId,
        run_id: &RunId,
    ) -> Result<(), InputStateMachineError> {
        let state =
            self.ledger
                .get_mut(input_id)
                .ok_or(InputStateMachineError::InvalidTransition {
                    from: InputLifecycleState::Staged,
                    to: InputLifecycleState::Applied,
                })?;
        InputStateMachine::transition(state, InputLifecycleState::Applied, None)?;
        InputStateMachine::transition(state, InputLifecycleState::AppliedPendingConsumption, None)?;
        self.emit_event(RuntimeEvent::InputLifecycle(InputLifecycleEvent::Applied {
            input_id: input_id.clone(),
            run_id: run_id.clone(),
        }));
        Ok(())
    }

    /// Consume inputs after a successful run (AppliedPendingConsumption → Consumed).
    pub fn consume_inputs(
        &mut self,
        input_ids: &[InputId],
        run_id: &RunId,
    ) -> Result<(), InputStateMachineError> {
        for input_id in input_ids {
            if let Some(state) = self.ledger.get_mut(input_id)
                && state.current_state == InputLifecycleState::AppliedPendingConsumption
            {
                InputStateMachine::transition(state, InputLifecycleState::Consumed, None)?;
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

    /// Rollback staged inputs to queued (on run failure).
    pub fn rollback_staged(&mut self, input_ids: &[InputId]) -> Result<(), InputStateMachineError> {
        for input_id in input_ids {
            let mut requeue_input = None;
            if let Some(state) = self.ledger.get_mut(input_id)
                && state.current_state == InputLifecycleState::Staged
            {
                InputStateMachine::transition(
                    state,
                    InputLifecycleState::Queued,
                    Some("run failed, rollback".into()),
                )?;
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

    /// Retire: abandon all non-terminal inputs.
    pub fn retire(&mut self) -> Result<RetireReport, RuntimeDriverError> {
        let from = self.state_machine.state();
        self.state_machine
            .transition(RuntimeState::Retired)
            .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?;

        self.emit_event(RuntimeEvent::RuntimeStateChange(RuntimeStateChangeEvent {
            from,
            to: RuntimeState::Retired,
        }));

        let abandoned = self.abandon_all_non_terminal(InputAbandonReason::Retired);
        Ok(RetireReport {
            inputs_abandoned: abandoned,
        })
    }

    /// Reset: abandon all non-terminal inputs and return to Idle.
    pub fn reset(&mut self) -> Result<ResetReport, RuntimeDriverError> {
        let abandoned = self.abandon_all_non_terminal(InputAbandonReason::Reset);

        // Drain queue
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

    /// Destroy: abandon everything, transition to Destroyed.
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

    /// §24 Ephemeral recovery.
    pub fn recover_ephemeral(&mut self) -> RecoveryReport {
        let mut recovered = 0;
        let mut abandoned = 0;
        let mut requeued = 0;

        // Collect IDs first to avoid borrow issues
        let input_ids: Vec<InputId> = self
            .ledger
            .iter()
            .filter(|(_, s)| !s.is_terminal())
            .map(|(id, _)| id.clone())
            .collect();

        for input_id in input_ids {
            if let Some(state) = self.ledger.get_mut(&input_id) {
                match state.current_state {
                    InputLifecycleState::Accepted => {
                        // Accepted → Queued (or → Consumed if Ignore+OnAccept)
                        if let Some(ref policy) = state.policy {
                            if policy.decision.apply_mode == ApplyMode::Ignore
                                && policy.decision.consume_point == ConsumePoint::OnAccept
                            {
                                let _ = InputStateMachine::transition(
                                    state,
                                    InputLifecycleState::Consumed,
                                    Some("recovery: Ignore+OnAccept".into()),
                                );
                                abandoned += 1;
                            } else {
                                let _ = InputStateMachine::transition(
                                    state,
                                    InputLifecycleState::Queued,
                                    Some("recovery: Accepted→Queued".into()),
                                );
                                requeued += 1;
                            }
                        } else {
                            let _ = InputStateMachine::transition(
                                state,
                                InputLifecycleState::Queued,
                                Some("recovery: Accepted→Queued (no policy)".into()),
                            );
                            requeued += 1;
                        }
                        recovered += 1;
                    }
                    InputLifecycleState::Staged => {
                        // Staged → Queued (no persisted evidence in ephemeral)
                        let _ = InputStateMachine::transition(
                            state,
                            InputLifecycleState::Queued,
                            Some("recovery: Staged→Queued (no evidence)".into()),
                        );
                        recovered += 1;
                        requeued += 1;
                    }
                    InputLifecycleState::Applied
                    | InputLifecycleState::AppliedPendingConsumption => {
                        // Applied stays Applied (side effects already happened)
                        recovered += 1;
                    }
                    InputLifecycleState::Queued => {
                        // Already queued, nothing to do
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

    // ---- Internal helpers ----

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

    fn abandon_all_non_terminal(&mut self, reason: InputAbandonReason) -> usize {
        let non_terminal_ids: Vec<InputId> = self
            .ledger
            .iter()
            .filter(|(_, s)| !s.is_terminal())
            .map(|(id, _)| id.clone())
            .collect();

        let mut count = 0;
        for id in &non_terminal_ids {
            if let Some(state) = self.ledger.get_mut(id)
                && InputStateMachine::abandon(state, reason.clone()).is_ok()
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
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl crate::traits::RuntimeDriver for EphemeralRuntimeDriver {
    async fn accept_input(&mut self, input: Input) -> Result<AcceptOutcome, RuntimeDriverError> {
        // Check runtime state
        if !self.state_machine.state().can_accept_input() {
            return Err(RuntimeDriverError::NotReady {
                state: self.state_machine.state(),
            });
        }

        // Validate durability
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

        // Check idempotency dedup
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

        // Resolve policy
        let runtime_idle = self.state_machine.is_idle();
        let policy = DefaultPolicyTable::resolve(&input, runtime_idle);

        // Store policy snapshot
        if let Some(s) = self.ledger.get_mut(&input_id) {
            s.policy = Some(PolicySnapshot {
                version: policy.policy_version,
                decision: policy.clone(),
            });
        }

        // Emit accepted event
        self.emit_event(RuntimeEvent::InputLifecycle(
            InputLifecycleEvent::Accepted {
                input_id: input_id.clone(),
            },
        ));

        // Apply policy decision
        match policy.apply_mode {
            ApplyMode::Ignore => {
                if policy.consume_point == ConsumePoint::OnAccept {
                    // Immediately consume
                    if let Some(s) = self.ledger.get_mut(&input_id) {
                        let _ = InputStateMachine::transition(
                            s,
                            InputLifecycleState::Consumed,
                            Some("Ignore+OnAccept".into()),
                        );
                    }
                }
            }
            ApplyMode::InjectNow => {
                // Queue for immediate processing
                if let Some(s) = self.ledger.get_mut(&input_id) {
                    s.persisted_input = Some(input.clone());
                    let _ = InputStateMachine::transition(s, InputLifecycleState::Queued, None);
                }
                self.queue.enqueue(input_id.clone(), input);
                self.emit_event(RuntimeEvent::InputLifecycle(InputLifecycleEvent::Queued {
                    input_id: input_id.clone(),
                }));
            }
            ApplyMode::StageRunStart | ApplyMode::StageRunBoundary => {
                // Queue for run boundary
                if let Some(s) = self.ledger.get_mut(&input_id) {
                    s.persisted_input = Some(input.clone());
                    let _ = InputStateMachine::transition(s, InputLifecycleState::Queued, None);
                }
                self.queue.enqueue(input_id.clone(), input);
                self.emit_event(RuntimeEvent::InputLifecycle(InputLifecycleEvent::Queued {
                    input_id: input_id.clone(),
                }));
            }
        }

        // Set wake flag
        if runtime_idle && policy.wake_mode == WakeMode::WakeIfIdle {
            self.wake_requested = true;
        }
        if runtime_idle && policy.apply_mode == ApplyMode::InjectNow {
            self.process_requested = true;
        }

        // Get the final state
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
        // Ephemeral driver doesn't need to handle external runtime events
        Ok(())
    }

    async fn on_run_event(&mut self, event: RunEvent) -> Result<(), RuntimeDriverError> {
        match event {
            RunEvent::RunCompleted {
                run_id,
                consumed_input_ids,
            } => {
                // Consume all contributing inputs
                self.consume_inputs(&consumed_input_ids, &run_id)
                    .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?;

                // Complete the run in state machine
                self.state_machine
                    .complete_run()
                    .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?;
            }
            RunEvent::RunFailed { .. } => {
                // Rollback any staged inputs
                let staged_ids: Vec<InputId> = self
                    .ledger
                    .iter()
                    .filter(|(_, s)| s.current_state == InputLifecycleState::Staged)
                    .map(|(id, _)| id.clone())
                    .collect();
                self.rollback_staged(&staged_ids)
                    .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?;

                if self.has_queued_input_outside(&staged_ids) {
                    self.wake_requested = true;
                }

                // Return to idle
                self.state_machine
                    .complete_run()
                    .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?;
            }
            RunEvent::RunCancelled { .. } => {
                // Same as failure — rollback staged
                let staged_ids: Vec<InputId> = self
                    .ledger
                    .iter()
                    .filter(|(_, s)| s.current_state == InputLifecycleState::Staged)
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
            RunEvent::RunStarted { .. } => {
                // Informational — no state change needed
            }
            RunEvent::BoundaryApplied {
                run_id, receipt, ..
            } => {
                // Transition contributing inputs to AppliedPendingConsumption
                for input_id in &receipt.contributing_input_ids {
                    if let Some(state) = self.ledger.get_mut(input_id) {
                        state.last_run_id = Some(run_id.clone());
                        state.last_boundary_sequence = Some(receipt.sequence);
                        // Only transition if Staged (not already Applied)
                        if state.current_state == InputLifecycleState::Staged {
                            let _ = InputStateMachine::transition(
                                state,
                                InputLifecycleState::Applied,
                                None,
                            );
                            let _ = InputStateMachine::transition(
                                state,
                                InputLifecycleState::AppliedPendingConsumption,
                                None,
                            );
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
            _ => {
                // Forward-compatible: unknown RunEvent variants are ignored
            }
        }
        Ok(())
    }

    async fn on_runtime_control(
        &mut self,
        command: RuntimeControlCommand,
    ) -> Result<(), RuntimeDriverError> {
        match command {
            RuntimeControlCommand::Stop => {
                self.state_machine
                    .transition(RuntimeState::Stopped)
                    .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?;
            }
            RuntimeControlCommand::Resume => {
                // For ephemeral, resume just means ensure we're in Idle
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

    fn input_state(&self, input_id: &InputId) -> Option<&InputState> {
        self.ledger.get(input_id)
    }

    fn active_input_ids(&self) -> Vec<InputId> {
        self.ledger.active_input_ids()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::input::*;
    use crate::traits::RuntimeDriver;
    use chrono::Utc;

    fn make_prompt_input(text: &str) -> Input {
        Input::Prompt(PromptInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Operator,
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            text: text.into(),
            turn_metadata: None,
        })
    }

    fn make_peer_terminal(body: &str) -> Input {
        Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "peer-1".into(),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(PeerConvention::ResponseTerminal {
                request_id: "req-1".into(),
                status: ResponseTerminalStatus::Completed,
            }),
            body: body.into(),
        })
    }

    fn make_peer_progress() -> Input {
        Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "peer-1".into(),
                    runtime_id: None,
                },
                durability: InputDurability::Ephemeral,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(PeerConvention::ResponseProgress {
                request_id: "req-1".into(),
                phase: ResponseProgressPhase::InProgress,
            }),
            body: "working...".into(),
        })
    }

    fn make_system_generated() -> Input {
        Input::SystemGenerated(SystemGeneratedInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::System,
                durability: InputDurability::Ephemeral,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            generator: "test".into(),
            content: "system content".into(),
        })
    }

    #[tokio::test]
    async fn accept_prompt_idle_queues_and_wakes() {
        let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
        let input = make_prompt_input("hello");
        let result = driver.accept_input(input).await.unwrap();

        assert!(result.is_accepted());
        assert!(driver.take_wake_requested());
        assert_eq!(driver.queue().len(), 1);
    }

    #[tokio::test]
    async fn accept_prompt_running_queues_and_wakes() {
        let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
        driver.state_machine.start_run(RunId::new()).unwrap();

        let input = make_prompt_input("hello");
        let result = driver.accept_input(input).await.unwrap();

        assert!(result.is_accepted());
        // Prompt always wakes (but runtime is running so wake_mode is still WakeIfIdle,
        // but runtime_idle=false so wake_requested should be false)
        assert!(!driver.take_wake_requested());
    }

    #[tokio::test]
    async fn accept_peer_terminal_idle_wakes() {
        let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
        let input = make_peer_terminal("done");
        let result = driver.accept_input(input).await.unwrap();

        assert!(result.is_accepted());
        assert!(driver.take_wake_requested()); // Terminal peer wakes idle runtime
    }

    #[tokio::test]
    async fn accept_peer_terminal_running_no_wake() {
        let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
        driver.state_machine.start_run(RunId::new()).unwrap();

        let input = make_peer_terminal("done");
        let result = driver.accept_input(input).await.unwrap();

        assert!(result.is_accepted());
        assert!(!driver.take_wake_requested()); // Running → no wake
    }

    #[tokio::test]
    async fn accept_progress_no_wake() {
        let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
        let input = make_peer_progress();
        let result = driver.accept_input(input).await.unwrap();

        assert!(result.is_accepted());
        assert!(!driver.take_wake_requested()); // Progress never wakes
    }

    #[tokio::test]
    async fn accept_system_generated_no_wake_queued() {
        let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
        let input = make_system_generated();
        let result = driver.accept_input(input).await.unwrap();

        // InjectNow should request immediate processing without a normal wake.
        assert!(result.is_accepted());
        assert!(!driver.take_wake_requested());
        assert!(driver.take_process_requested());
        // InjectNow still queues (it will be applied immediately by the runtime loop)
        assert_eq!(driver.queue().len(), 1);
    }

    #[tokio::test]
    async fn dedup_by_idempotency() {
        let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
        let key = crate::identifiers::IdempotencyKey::new("req-1");

        let mut input1 = make_prompt_input("hello");
        if let Input::Prompt(ref mut p) = input1 {
            p.header.idempotency_key = Some(key.clone());
        }
        let result1 = driver.accept_input(input1).await.unwrap();
        assert!(result1.is_accepted());

        let mut input2 = make_prompt_input("hello again");
        if let Input::Prompt(ref mut p) = input2 {
            p.header.idempotency_key = Some(key);
        }
        let result2 = driver.accept_input(input2).await.unwrap();
        assert!(result2.is_deduplicated());
    }

    #[tokio::test]
    async fn reject_derived_prompt() {
        let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
        let input = Input::Prompt(PromptInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::System,
                durability: InputDurability::Derived,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            text: "hi".into(),
            turn_metadata: None,
        });
        let result = driver.accept_input(input).await.unwrap();
        assert!(result.is_rejected());
    }

    #[tokio::test]
    async fn retire_abandons_non_terminal() {
        let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
        driver.accept_input(make_prompt_input("a")).await.unwrap();
        driver.accept_input(make_prompt_input("b")).await.unwrap();

        let report = driver.retire().unwrap();
        assert_eq!(report.inputs_abandoned, 2);
        assert_eq!(driver.runtime_state(), RuntimeState::Retired);
    }

    #[tokio::test]
    async fn reset_abandons_and_drains() {
        let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
        driver.accept_input(make_prompt_input("a")).await.unwrap();

        let report = driver.reset().unwrap();
        assert_eq!(report.inputs_abandoned, 1);
        assert!(driver.queue().is_empty());
    }

    #[tokio::test]
    async fn destroy_transitions_to_terminal() {
        let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
        driver.accept_input(make_prompt_input("a")).await.unwrap();

        let abandoned = driver.destroy().unwrap();
        assert_eq!(abandoned, 1);
        assert!(driver.runtime_state().is_terminal());
    }

    #[tokio::test]
    async fn on_run_completed_consumes() {
        let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));

        // Accept and manually transition to simulate run
        let input = make_prompt_input("hello");
        let input_id = input.id().clone();
        driver.accept_input(input).await.unwrap();
        driver.take_wake_requested();

        // Simulate run start
        let run_id = RunId::new();
        driver.state_machine.start_run(run_id.clone()).unwrap();

        // Stage and apply
        driver.stage_input(&input_id, &run_id).unwrap();
        driver.apply_input(&input_id, &run_id).unwrap();

        // Run completed
        driver
            .on_run_event(RunEvent::RunCompleted {
                run_id: run_id.clone(),
                consumed_input_ids: vec![input_id.clone()],
            })
            .await
            .unwrap();

        // Input should be consumed
        let state = driver.input_state(&input_id).unwrap();
        assert_eq!(state.current_state, InputLifecycleState::Consumed);
        assert!(driver.state_machine.is_idle());
    }

    #[tokio::test]
    async fn on_run_failed_rollbacks() {
        let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));

        let input = make_prompt_input("hello");
        let input_id = input.id().clone();
        driver.accept_input(input).await.unwrap();

        let run_id = RunId::new();
        driver.state_machine.start_run(run_id.clone()).unwrap();
        driver.stage_input(&input_id, &run_id).unwrap();

        // Run failed
        driver
            .on_run_event(RunEvent::RunFailed {
                run_id,
                error: "LLM error".into(),
                recoverable: true,
            })
            .await
            .unwrap();

        // Input should be rolled back to Queued
        let state = driver.input_state(&input_id).unwrap();
        assert_eq!(state.current_state, InputLifecycleState::Queued);
        assert!(driver.state_machine.is_idle());
    }

    #[tokio::test]
    async fn on_run_failed_requests_wake_for_backlog() {
        let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));

        let input1 = make_prompt_input("first");
        let input1_id = input1.id().clone();
        let input2 = make_prompt_input("second");
        driver.accept_input(input1).await.unwrap();
        driver.accept_input(input2).await.unwrap();
        let _ = driver.take_wake_requested();

        let run_id = RunId::new();
        let (dequeued_id, _) = driver.dequeue_next().unwrap();
        assert_eq!(dequeued_id, input1_id);
        driver.state_machine.start_run(run_id.clone()).unwrap();
        driver.stage_input(&input1_id, &run_id).unwrap();

        driver
            .on_run_event(RunEvent::RunFailed {
                run_id,
                error: "LLM error".into(),
                recoverable: true,
            })
            .await
            .unwrap();

        assert!(
            driver.take_wake_requested(),
            "queued work behind a failed run should request another wake"
        );
    }

    #[tokio::test]
    async fn reset_after_retire_returns_runtime_to_idle() {
        let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
        driver.retire().unwrap();
        assert_eq!(driver.runtime_state(), RuntimeState::Retired);

        let report = driver.reset().unwrap();
        assert_eq!(report.inputs_abandoned, 0);
        assert_eq!(driver.runtime_state(), RuntimeState::Idle);

        let accepted = driver
            .accept_input(make_prompt_input("hello"))
            .await
            .unwrap();
        assert!(
            accepted.is_accepted(),
            "reset runtime should accept new input"
        );
    }

    #[tokio::test]
    async fn recovery_counts_queued_as_recovered() {
        let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));

        // Accept an input — policy transitions it to Queued
        let input = make_prompt_input("hello");
        let input_id = input.id().clone();
        driver.accept_input(input).await.unwrap();

        // After accept, state is Queued (policy applied immediately)
        let state = driver.input_state(&input_id).unwrap();
        assert_eq!(state.current_state, InputLifecycleState::Queued);

        // Drain the queue (simulating crash losing queue state)
        driver.queue.drain();

        // Recover
        let report = driver.recover_ephemeral();
        // Queued inputs are counted as recovered (already in correct state)
        assert_eq!(report.inputs_recovered, 1);
        assert_eq!(report.inputs_requeued, 0); // Already Queued, no transition needed
    }

    #[tokio::test]
    async fn recovery_applied_stays_applied() {
        let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));

        let input = make_prompt_input("hello");
        let input_id = input.id().clone();
        driver.accept_input(input).await.unwrap();

        let run_id = RunId::new();
        driver.state_machine.start_run(run_id.clone()).unwrap();
        driver.stage_input(&input_id, &run_id).unwrap();
        driver.apply_input(&input_id, &run_id).unwrap();

        // Recover
        driver
            .state_machine
            .transition(RuntimeState::Recovering)
            .unwrap();
        let report = driver.recover_ephemeral();
        assert_eq!(report.inputs_recovered, 1);

        // Applied stays Applied (side effects already happened)
        let state = driver.input_state(&input_id).unwrap();
        assert_eq!(
            state.current_state,
            InputLifecycleState::AppliedPendingConsumption
        );
    }

    #[tokio::test]
    async fn retired_rejects_input() {
        let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
        driver.retire().unwrap();

        let result = driver.accept_input(make_prompt_input("hello")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn events_emitted_for_lifecycle() {
        let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
        driver
            .accept_input(make_prompt_input("hello"))
            .await
            .unwrap();

        let events = driver.drain_events();
        assert!(!events.is_empty());
        // Should have Accepted and Queued events
        let has_accepted = events.iter().any(|e| {
            matches!(
                &e.event,
                RuntimeEvent::InputLifecycle(InputLifecycleEvent::Accepted { .. })
            )
        });
        let has_queued = events.iter().any(|e| {
            matches!(
                &e.event,
                RuntimeEvent::InputLifecycle(InputLifecycleEvent::Queued { .. })
            )
        });
        assert!(has_accepted);
        assert!(has_queued);
    }

    #[tokio::test]
    async fn progress_peer_staged_boundary() {
        let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
        let input = make_peer_progress();
        let input_id = input.id().clone();
        let result = driver.accept_input(input).await.unwrap();

        assert!(result.is_accepted());
        // Per §17: ResponseProgress → StageRunBoundary + NoWake + Coalesce + OnRunComplete
        let state = driver.input_state(&input_id).unwrap();
        assert_eq!(state.current_state, InputLifecycleState::Queued);
        assert!(!driver.take_wake_requested()); // Progress never wakes
    }

    #[tokio::test]
    async fn destroy_after_retire_succeeds() {
        let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("test"));
        driver.retire().unwrap();
        let abandoned = driver.destroy().unwrap();
        assert_eq!(abandoned, 0); // Already abandoned by retire
        assert_eq!(driver.runtime_state(), RuntimeState::Destroyed);
    }
}

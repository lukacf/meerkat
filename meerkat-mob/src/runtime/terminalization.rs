use crate::error::MobError;
use crate::event::{FlowFailureClass, MobEventKind, NewMobEvent};
use crate::ids::{FlowId, MobId, RunId};
use crate::run::{MobRun, MobRunStatus, mob_machine_run_status_is_terminal};
use crate::store::{MobEventStore, MobRunStore, authority_validating_mob_run_store};
use serde_json::{Map, Value};
use std::sync::Arc;

/// Typed categorization of why a flow run terminated in `Failed`.
///
/// The wire-persisted `MobEventKind::FlowFailed` carries the typed
/// [`FlowFailureClass`] projected via [`FlowFailureCause::class`] plus the
/// display `reason` rendered via [`FlowFailureCause::reason`]. The class is
/// derived ONCE from the typed `MobError` at [`FlowFailureCause::from_step_error`]
/// — no consumer re-parses the display string to recover the failure origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FlowFailureCause {
    /// A step (single- or multi-target dispatch, collection policy, template
    /// render, deadline, or escalation) failed and aborted the run. `class`
    /// is the typed classification of the originating [`MobError`]; `detail`
    /// is its display text.
    StepError {
        class: FlowFailureClass,
        detail: String,
    },
    /// The mob lifecycle rejected the `StartRun` transition during admission.
    AdmissionFailed { detail: String },
    /// Repair fallback for a run already persisted as `Failed` whose original
    /// failure reason is no longer available to reconstruct.
    AlreadyFailed,
}

impl FlowFailureCause {
    /// Classify a typed step-level [`MobError`] ONCE into a typed failure
    /// cause, preserving the display detail.
    pub(super) fn from_step_error(error: &MobError) -> Self {
        let class = match error {
            MobError::FlowTurnTimedOut => FlowFailureClass::StepTimeout,
            MobError::TopologyViolation { .. } => FlowFailureClass::TopologyViolation,
            MobError::SchemaValidation { .. } => FlowFailureClass::SchemaValidation,
            MobError::RunCanceled(_) => FlowFailureClass::RunCanceled,
            MobError::SupervisorEscalation(_) => FlowFailureClass::SupervisorEscalation,
            MobError::InsufficientTargets { .. } => FlowFailureClass::InsufficientTargets,
            MobError::FlowFailed { .. } => FlowFailureClass::StepError,
            _ => FlowFailureClass::Internal,
        };
        Self::StepError {
            class,
            detail: error.to_string(),
        }
    }

    /// Project the typed failure class persisted into the `FlowFailed` event.
    pub(super) fn class(&self) -> FlowFailureClass {
        match self {
            Self::StepError { class, .. } => *class,
            Self::AdmissionFailed { .. } => FlowFailureClass::AdmissionFailed,
            Self::AlreadyFailed => FlowFailureClass::AlreadyFailed,
        }
    }

    /// Render the display string persisted into the wire `FlowFailed` event.
    pub(super) fn reason(&self) -> String {
        match self {
            Self::StepError { detail, .. } | Self::AdmissionFailed { detail } => detail.clone(),
            Self::AlreadyFailed => "run already failed".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub(super) enum TerminalizationTarget {
    Completed { structured_output: Option<Value> },
    Failed { cause: FlowFailureCause },
    Canceled,
}

impl TerminalizationTarget {
    pub(super) fn status(&self) -> MobRunStatus {
        match self {
            Self::Completed { .. } => MobRunStatus::Completed,
            Self::Failed { .. } => MobRunStatus::Failed,
            Self::Canceled => MobRunStatus::Canceled,
        }
    }

    fn event_name(&self) -> &'static str {
        match self {
            Self::Completed { .. } => "FlowCompleted",
            Self::Failed { .. } => "FlowFailed",
            Self::Canceled => "FlowCanceled",
        }
    }

    fn into_event_kind(self, run_id: RunId, flow_id: FlowId) -> MobEventKind {
        match self {
            Self::Completed { structured_output } => MobEventKind::FlowCompleted {
                run_id,
                flow_id,
                structured_output,
            },
            Self::Failed { cause } => MobEventKind::FlowFailed {
                run_id,
                flow_id,
                cause: cause.class(),
                reason: cause.reason(),
            },
            Self::Canceled => MobEventKind::FlowCanceled { run_id, flow_id },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalizationOutcome {
    Transitioned,
    Noop,
}

#[derive(Clone)]
pub(super) struct FlowTerminalizationAuthority {
    run_store: Arc<dyn MobRunStore>,
    events: Arc<dyn MobEventStore>,
    mob_id: MobId,
}

impl FlowTerminalizationAuthority {
    pub(super) fn new(
        run_store: Arc<dyn MobRunStore>,
        events: Arc<dyn MobEventStore>,
        mob_id: MobId,
    ) -> Self {
        Self {
            run_store: authority_validating_mob_run_store(run_store),
            events,
            mob_id,
        }
    }

    /// Commit the terminal run snapshot AND the terminal mob event through the
    /// store's combined seam, so terminal status truth and terminal event
    /// truth cannot split (single SQLite transaction on durable storage).
    ///
    /// Returns `Ok(Some(Transitioned))` when this caller won the CAS and the
    /// terminal event is committed with it, `Ok(None)` when the CAS lost.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn commit_terminalization_with_snapshot(
        &self,
        run_id: &RunId,
        flow_id: FlowId,
        target: TerminalizationTarget,
        expected_status: MobRunStatus,
        expected_flow_state: &crate::run::flow_run::State,
        next_flow_state: &crate::run::flow_run::State,
        authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
    ) -> Result<Option<TerminalizationOutcome>, MobError> {
        let next_status = target.status();
        let event_kind = target.into_event_kind(run_id.clone(), flow_id);
        let stored = self
            .run_store
            .cas_run_snapshot_and_append_terminal_event_with_authority(
                run_id,
                expected_status,
                expected_flow_state,
                next_status,
                next_flow_state,
                authority_inputs,
                self.events.as_ref(),
                NewMobEvent {
                    mob_id: self.mob_id.clone(),
                    timestamp: None,
                    kind: event_kind,
                },
            )
            .await?;
        Ok(stored.map(|_| TerminalizationOutcome::Transitioned))
    }

    pub(super) async fn repair_persisted_terminalization(
        &self,
        run_id: RunId,
        flow_id: FlowId,
        target: TerminalizationTarget,
    ) -> Result<TerminalizationOutcome, MobError> {
        let run = self
            .run_store
            .get_run(&run_id)
            .await?
            .ok_or_else(|| MobError::RunNotFound(run_id.clone()))?;
        if !mob_machine_run_status_is_terminal(&run.run_id, &run.status)? {
            return Ok(TerminalizationOutcome::Noop);
        }
        let target = Self::repair_target_for_run(&run, target);
        let event_name = target.event_name();
        let event_kind = target.into_event_kind(run_id.clone(), flow_id);
        match self
            .events
            .append_terminal_event_if_absent(NewMobEvent {
                mob_id: self.mob_id.clone(),
                timestamp: None,
                kind: event_kind,
            })
            .await
        {
            Ok(Some(_) | None) => Ok(TerminalizationOutcome::Noop),
            Err(append_error) => {
                self.record_terminal_event_append_failure(
                    &run_id,
                    event_name,
                    MobError::from(append_error),
                )
                .await
            }
        }
    }

    fn repair_target_for_run(
        run: &MobRun,
        requested: TerminalizationTarget,
    ) -> TerminalizationTarget {
        match run.status {
            MobRunStatus::Completed => {
                let requested_output = match requested {
                    TerminalizationTarget::Completed { structured_output } => structured_output,
                    _ => None,
                };
                TerminalizationTarget::Completed {
                    structured_output: requested_output
                        .or_else(|| structured_output_from_run_outputs(run)),
                }
            }
            MobRunStatus::Failed => {
                let cause = match requested {
                    TerminalizationTarget::Failed { cause } => cause,
                    _ => FlowFailureCause::AlreadyFailed,
                };
                TerminalizationTarget::Failed { cause }
            }
            MobRunStatus::Canceled => TerminalizationTarget::Canceled,
            MobRunStatus::Pending | MobRunStatus::Running => requested,
        }
    }
}

fn structured_output_from_run_outputs(run: &MobRun) -> Option<Value> {
    if run.root_step_outputs.is_empty() && run.loop_iteration_outputs.is_empty() {
        return None;
    }

    let mut steps = Map::new();
    for (step_id, output) in &run.root_step_outputs {
        steps.insert(step_id.as_str().to_string(), output.clone());
    }
    for iterations in run.loop_iteration_outputs.values() {
        for iteration in iterations {
            for (step_id, output) in iteration {
                steps.insert(step_id.as_str().to_string(), output.clone());
            }
        }
    }

    let mut structured_output = Map::new();
    structured_output.insert("steps".to_string(), Value::Object(steps));
    Some(Value::Object(structured_output))
}

impl FlowTerminalizationAuthority {
    async fn record_terminal_event_append_failure(
        &self,
        run_id: &RunId,
        event_name: &'static str,
        append_error: MobError,
    ) -> Result<TerminalizationOutcome, MobError> {
        let reason =
            format!("terminal run status persisted but {event_name} append failed: {append_error}");
        tracing::error!(
            run_id = %run_id,
            event_name,
            append_error = %append_error,
            "terminal event append divergence surfaced without mutating run provenance"
        );
        Err(MobError::Internal(reason))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FlowFailureCause, FlowTerminalizationAuthority, TerminalizationOutcome,
        TerminalizationTarget,
    };
    use crate::event::{FlowFailureClass, MobEvent, MobEventKind, NewMobEvent};
    use crate::ids::{FlowId, LoopId, MobId, RunId, StepId};
    use crate::run::{MobRun, MobRunProvenanceAuthority, MobRunStatus, StepLedgerEntry};
    use crate::store::{
        InMemoryMobRunStore, MobEventStore, MobRunStore, MobStoreError, private,
        terminal_event_identity,
    };
    use async_trait::async_trait;
    use chrono::Utc;
    use std::collections::HashSet;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    struct RecordingRunStore {
        inner: InMemoryMobRunStore,
    }

    impl RecordingRunStore {
        fn new() -> Self {
            Self {
                inner: InMemoryMobRunStore::new(),
            }
        }
    }

    #[async_trait]
    impl MobRunStore for RecordingRunStore {
        async fn create_run(&self, run: MobRun) -> Result<(), MobStoreError> {
            self.inner.create_run(run).await
        }

        async fn get_run(&self, run_id: &RunId) -> Result<Option<MobRun>, MobStoreError> {
            self.inner.get_run(run_id).await
        }

        async fn list_runs(
            &self,
            mob_id: &MobId,
            flow_id: Option<&FlowId>,
        ) -> Result<Vec<MobRun>, MobStoreError> {
            self.inner.list_runs(mob_id, flow_id).await
        }

        async fn append_step_entry_with_authority(
            &self,
            run_id: &RunId,
            entry: StepLedgerEntry,
            authority: MobRunProvenanceAuthority,
        ) -> Result<(), MobStoreError> {
            self.inner
                .append_step_entry_with_authority(run_id, entry, authority)
                .await
        }

        async fn append_step_entry_if_absent_with_authority(
            &self,
            run_id: &RunId,
            entry: StepLedgerEntry,
            authority: MobRunProvenanceAuthority,
        ) -> Result<bool, MobStoreError> {
            self.inner
                .append_step_entry_if_absent_with_authority(run_id, entry, authority)
                .await
        }

        async fn append_failure_entry_with_authority(
            &self,
            run_id: &RunId,
            entry: crate::run::FailureLedgerEntry,
            authority: MobRunProvenanceAuthority,
        ) -> Result<(), MobStoreError> {
            self.inner
                .append_failure_entry_with_authority(run_id, entry, authority)
                .await
        }

        async fn cas_flow_state_with_authority(
            &self,
            run_id: &RunId,
            expected: &crate::run::flow_run::State,
            next: &crate::run::flow_run::State,
            authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
        ) -> Result<bool, MobStoreError> {
            self.inner
                .cas_flow_state_with_authority(run_id, expected, next, authority_inputs)
                .await
        }

        async fn cas_run_snapshot_with_authority(
            &self,
            run_id: &RunId,
            expected_status: MobRunStatus,
            expected_flow_state: &crate::run::flow_run::State,
            next_status: MobRunStatus,
            next_flow_state: &crate::run::flow_run::State,
            authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
        ) -> Result<bool, MobStoreError> {
            self.inner
                .cas_run_snapshot_with_authority(
                    run_id,
                    expected_status,
                    expected_flow_state,
                    next_status,
                    next_flow_state,
                    authority_inputs,
                )
                .await
        }

        async fn cas_frame_state_with_authority(
            &self,
            run_id: &RunId,
            frame_id: &crate::ids::FrameId,
            expected: Option<&crate::run::FrameSnapshot>,
            next: crate::run::FrameSnapshot,
            authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
        ) -> Result<bool, MobStoreError> {
            self.inner
                .cas_frame_state_with_authority(run_id, frame_id, expected, next, authority_inputs)
                .await
        }

        #[allow(clippy::too_many_arguments)]
        async fn cas_grant_node_slot_with_authority(
            &self,
            run_id: &RunId,
            expected_run_state: &crate::run::flow_run::State,
            next_run_state: crate::run::flow_run::State,
            frame_id: &crate::ids::FrameId,
            expected_frame: &crate::run::FrameSnapshot,
            next_frame: crate::run::FrameSnapshot,
            authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
        ) -> Result<bool, MobStoreError> {
            self.inner
                .cas_grant_node_slot_with_authority(
                    run_id,
                    expected_run_state,
                    next_run_state,
                    frame_id,
                    expected_frame,
                    next_frame,
                    authority_inputs,
                )
                .await
        }

        #[allow(clippy::too_many_arguments)]
        async fn cas_complete_step_and_record_output_with_authority(
            &self,
            run_id: &RunId,
            frame_id: &crate::ids::FrameId,
            expected_frame: &crate::run::FrameSnapshot,
            next_frame: crate::run::FrameSnapshot,
            step_output_key: String,
            step_output: serde_json::Value,
            loop_context: Option<(&crate::ids::LoopId, u64)>,
            authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
        ) -> Result<bool, MobStoreError> {
            self.inner
                .cas_complete_step_and_record_output_with_authority(
                    run_id,
                    frame_id,
                    expected_frame,
                    next_frame,
                    step_output_key,
                    step_output,
                    loop_context,
                    authority_inputs,
                )
                .await
        }

        #[allow(clippy::too_many_arguments)]
        async fn cas_start_loop_with_authority(
            &self,
            run_id: &RunId,
            loop_instance_id: &crate::ids::LoopInstanceId,
            expected_run_state: &crate::run::flow_run::State,
            next_run_state: crate::run::flow_run::State,
            frame_id: &crate::ids::FrameId,
            expected_frame: &crate::run::FrameSnapshot,
            next_frame: crate::run::FrameSnapshot,
            initial_loop: crate::run::LoopSnapshot,
            authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
        ) -> Result<bool, MobStoreError> {
            self.inner
                .cas_start_loop_with_authority(
                    run_id,
                    loop_instance_id,
                    expected_run_state,
                    next_run_state,
                    frame_id,
                    expected_frame,
                    next_frame,
                    initial_loop,
                    authority_inputs,
                )
                .await
        }

        #[allow(clippy::too_many_arguments)]
        async fn cas_loop_request_body_frame_with_authority(
            &self,
            run_id: &RunId,
            loop_instance_id: &crate::ids::LoopInstanceId,
            expected_loop: &crate::run::LoopSnapshot,
            next_loop: crate::run::LoopSnapshot,
            expected_run_state: &crate::run::flow_run::State,
            next_run_state: crate::run::flow_run::State,
            authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
        ) -> Result<bool, MobStoreError> {
            self.inner
                .cas_loop_request_body_frame_with_authority(
                    run_id,
                    loop_instance_id,
                    expected_loop,
                    next_loop,
                    expected_run_state,
                    next_run_state,
                    authority_inputs,
                )
                .await
        }

        #[allow(clippy::too_many_arguments)]
        async fn cas_grant_body_frame_start_with_authority(
            &self,
            run_id: &RunId,
            loop_instance_id: &crate::ids::LoopInstanceId,
            expected_loop: &crate::run::LoopSnapshot,
            next_loop: crate::run::LoopSnapshot,
            frame_id: &crate::ids::FrameId,
            initial_frame: crate::run::FrameSnapshot,
            ledger_entry: crate::run::LoopIterationLedgerEntry,
            expected_run_state: &crate::run::flow_run::State,
            next_run_state: crate::run::flow_run::State,
            authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
        ) -> Result<bool, MobStoreError> {
            self.inner
                .cas_grant_body_frame_start_with_authority(
                    run_id,
                    loop_instance_id,
                    expected_loop,
                    next_loop,
                    frame_id,
                    initial_frame,
                    ledger_entry,
                    expected_run_state,
                    next_run_state,
                    authority_inputs,
                )
                .await
        }

        #[allow(clippy::too_many_arguments)]
        async fn cas_complete_body_frame_with_authority(
            &self,
            run_id: &RunId,
            loop_instance_id: &crate::ids::LoopInstanceId,
            expected_loop: &crate::run::LoopSnapshot,
            next_loop: crate::run::LoopSnapshot,
            frame_id: &crate::ids::FrameId,
            expected_frame: &crate::run::FrameSnapshot,
            next_frame: crate::run::FrameSnapshot,
            expected_run_state: &crate::run::flow_run::State,
            next_run_state: crate::run::flow_run::State,
            authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
        ) -> Result<bool, MobStoreError> {
            self.inner
                .cas_complete_body_frame_with_authority(
                    run_id,
                    loop_instance_id,
                    expected_loop,
                    next_loop,
                    frame_id,
                    expected_frame,
                    next_frame,
                    expected_run_state,
                    next_run_state,
                    authority_inputs,
                )
                .await
        }

        #[allow(clippy::too_many_arguments)]
        async fn cas_complete_loop_with_authority(
            &self,
            run_id: &RunId,
            loop_instance_id: &crate::ids::LoopInstanceId,
            expected_loop: &crate::run::LoopSnapshot,
            next_loop: crate::run::LoopSnapshot,
            frame_id: &crate::ids::FrameId,
            expected_frame: &crate::run::FrameSnapshot,
            next_frame: crate::run::FrameSnapshot,
            expected_run_state: &crate::run::flow_run::State,
            next_run_state: crate::run::flow_run::State,
            authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
        ) -> Result<bool, MobStoreError> {
            self.inner
                .cas_complete_loop_with_authority(
                    run_id,
                    loop_instance_id,
                    expected_loop,
                    next_loop,
                    frame_id,
                    expected_frame,
                    next_frame,
                    expected_run_state,
                    next_run_state,
                    authority_inputs,
                )
                .await
        }
    }

    #[derive(Default)]
    struct FaultInjectedEventStore {
        events: RwLock<Vec<MobEvent>>,
        fail_on_kind: RwLock<HashSet<&'static str>>,
    }

    impl FaultInjectedEventStore {
        async fn fail_appends_for(&self, kind: &'static str) {
            self.fail_on_kind.write().await.insert(kind);
        }

        async fn allow_appends_for(&self, kind: &'static str) {
            self.fail_on_kind.write().await.remove(kind);
        }

        fn kind_label(kind: &MobEventKind) -> &'static str {
            match kind {
                MobEventKind::FlowCompleted { .. } => "FlowCompleted",
                MobEventKind::FlowFailed { .. } => "FlowFailed",
                MobEventKind::FlowCanceled { .. } => "FlowCanceled",
                _ => "Other",
            }
        }
    }

    impl private::MobEventStoreSealed for FaultInjectedEventStore {}

    #[async_trait]
    impl MobEventStore for FaultInjectedEventStore {
        async fn append(&self, event: NewMobEvent) -> Result<MobEvent, MobStoreError> {
            let kind = Self::kind_label(&event.kind);
            if self.fail_on_kind.read().await.contains(kind) {
                return Err(MobStoreError::Internal(format!(
                    "fault-injected append failure for {kind}"
                )));
            }
            let mut events = self.events.write().await;
            let stored = MobEvent {
                cursor: events.len() as u64 + 1,
                timestamp: Utc::now(),
                mob_id: event.mob_id,
                kind: event.kind,
            };
            events.push(stored.clone());
            Ok(stored)
        }

        async fn append_terminal_event_if_absent(
            &self,
            event: NewMobEvent,
        ) -> Result<Option<MobEvent>, MobStoreError> {
            let Some((run_id, flow_id)) = terminal_event_identity(&event.kind) else {
                return Err(MobStoreError::Internal(
                    "append_terminal_event_if_absent requires a terminal flow event".to_string(),
                ));
            };
            let run_id = run_id.clone();
            let flow_id = flow_id.clone();
            let mob_id = event.mob_id.clone();

            let mut events = self.events.write().await;
            if events.iter().any(|existing| {
                existing.mob_id == mob_id
                    && terminal_event_identity(&existing.kind).is_some_and(
                        |(existing_run_id, existing_flow_id)| {
                            existing_run_id == &run_id && existing_flow_id == &flow_id
                        },
                    )
            }) {
                return Ok(None);
            }

            let kind = Self::kind_label(&event.kind);
            if self.fail_on_kind.read().await.contains(kind) {
                return Err(MobStoreError::Internal(format!(
                    "fault-injected append failure for {kind}"
                )));
            }
            let stored = MobEvent {
                cursor: events.len() as u64 + 1,
                timestamp: Utc::now(),
                mob_id: event.mob_id,
                kind: event.kind,
            };
            events.push(stored.clone());
            Ok(Some(stored))
        }

        async fn poll(
            &self,
            after_cursor: u64,
            limit: usize,
        ) -> Result<Vec<MobEvent>, MobStoreError> {
            let events = self.events.read().await;
            Ok(events
                .iter()
                .filter(|event| event.cursor > after_cursor)
                .take(limit)
                .cloned()
                .collect())
        }

        async fn replay_all(&self) -> Result<Vec<MobEvent>, MobStoreError> {
            Ok(self.events.read().await.clone())
        }

        async fn latest_cursor(&self) -> Result<u64, MobStoreError> {
            Ok(self
                .events
                .read()
                .await
                .last()
                .map_or(0, |event| event.cursor))
        }

        async fn append_batch(
            &self,
            events: Vec<NewMobEvent>,
        ) -> Result<Vec<MobEvent>, MobStoreError> {
            let mut results = Vec::with_capacity(events.len());
            for event in events {
                results.push(self.append(event).await?);
            }
            Ok(results)
        }

        async fn clear(&self) -> Result<(), MobStoreError> {
            self.events.write().await.clear();
            Ok(())
        }
    }

    fn sample_run(run_id: RunId, status: MobRunStatus) -> MobRun {
        MobRun::authority_backed_for_steps(
            run_id,
            MobId::from("mob"),
            FlowId::from("flow"),
            [StepId::from("step-1")],
            status,
            serde_json::json!({}),
        )
        .expect("authority-backed sample run")
    }

    #[test]
    fn test_flow_failure_cause_classifies_typed_error_into_typed_event_cause() {
        use crate::error::MobError;

        // The typed MobError is classified ONCE into a typed cause...
        let cause = FlowFailureCause::from_step_error(&MobError::FlowTurnTimedOut);
        assert_eq!(cause.class(), FlowFailureClass::StepTimeout);
        assert_eq!(
            FlowFailureCause::from_step_error(&MobError::TopologyViolation {
                from_role: crate::ids::ProfileName::from("a"),
                to_role: crate::ids::ProfileName::from("b"),
            })
            .class(),
            FlowFailureClass::TopologyViolation
        );
        assert_eq!(
            FlowFailureCause::from_step_error(&MobError::RunCanceled(RunId::new())).class(),
            FlowFailureClass::RunCanceled
        );

        // ...and the persisted FlowFailed event carries that typed class, not
        // only a display string.
        let kind = TerminalizationTarget::Failed { cause }
            .into_event_kind(RunId::new(), FlowId::from("flow"));
        match kind {
            MobEventKind::FlowFailed { cause, reason, .. } => {
                assert_eq!(cause, FlowFailureClass::StepTimeout);
                assert!(!reason.is_empty());
            }
            other => panic!("expected FlowFailed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_terminalization_commits_snapshot_and_terminal_event_through_one_seam() {
        let run_store = Arc::new(RecordingRunStore::new());
        let events = Arc::new(FaultInjectedEventStore::default());
        let authority = FlowTerminalizationAuthority::new(
            run_store.clone(),
            events.clone(),
            MobId::from("mob"),
        );

        let run_id = RunId::new();
        let run = sample_run(run_id.clone(), MobRunStatus::Running);
        let expected_flow_state = run.flow_state.clone();
        let (next_flow_state, authority_input) = run
            .flow_run_command_projection_for_test(
                crate::run::MobMachineFlowRunCommand::TerminalizeCanceled(
                    crate::run::flow_run::inputs::TerminalizeCanceled {},
                ),
            )
            .expect("project canceled run state");
        run_store.create_run(run).await.expect("create run");

        let outcome = authority
            .commit_terminalization_with_snapshot(
                &run_id,
                FlowId::from("flow"),
                TerminalizationTarget::Canceled,
                MobRunStatus::Running,
                &expected_flow_state,
                &next_flow_state,
                vec![authority_input],
            )
            .await
            .expect("terminalize");
        assert_eq!(outcome, Some(TerminalizationOutcome::Transitioned));

        let run = run_store
            .get_run(&run_id)
            .await
            .expect("get run")
            .expect("run exists");
        assert_eq!(run.status, MobRunStatus::Canceled);
        let emitted = events.replay_all().await.expect("replay events");
        assert_eq!(emitted.len(), 1);
        assert!(matches!(emitted[0].kind, MobEventKind::FlowCanceled { .. }));
    }

    #[tokio::test]
    async fn test_terminalization_cas_loss_appends_no_terminal_event() {
        let run_store = Arc::new(RecordingRunStore::new());
        let events = Arc::new(FaultInjectedEventStore::default());
        let authority = FlowTerminalizationAuthority::new(
            run_store.clone(),
            events.clone(),
            MobId::from("mob"),
        );

        let run_id = RunId::new();
        let run = sample_run(run_id.clone(), MobRunStatus::Running);
        let stale_flow_state = run.flow_state.clone();
        let (next_flow_state, authority_input) = run
            .flow_run_command_projection_for_test(
                crate::run::MobMachineFlowRunCommand::TerminalizeCanceled(
                    crate::run::flow_run::inputs::TerminalizeCanceled {},
                ),
            )
            .expect("project canceled run state");
        run_store.create_run(run).await.expect("create run");

        // Wrong expected status → CAS loses → NOTHING is appended (the event
        // cannot exist without the snapshot commit).
        let outcome = authority
            .commit_terminalization_with_snapshot(
                &run_id,
                FlowId::from("flow"),
                TerminalizationTarget::Canceled,
                MobRunStatus::Pending,
                &stale_flow_state,
                &next_flow_state,
                vec![authority_input],
            )
            .await
            .expect("commit attempt");
        assert_eq!(outcome, None, "lost CAS must not report a transition");
        assert!(
            events.replay_all().await.expect("replay events").is_empty(),
            "a lost CAS must not append a terminal event"
        );
        let run = run_store
            .get_run(&run_id)
            .await
            .expect("get run")
            .expect("run exists");
        assert_eq!(run.status, MobRunStatus::Running);
    }

    #[tokio::test]
    async fn test_completed_terminalization_carries_structured_output() {
        let run_store = Arc::new(RecordingRunStore::new());
        let events = Arc::new(FaultInjectedEventStore::default());
        let authority = FlowTerminalizationAuthority::new(
            run_store.clone(),
            events.clone(),
            MobId::from("mob"),
        );

        let run_id = RunId::new();
        let mut run = sample_run(run_id.clone(), MobRunStatus::Running);
        run.root_step_outputs
            .insert(StepId::from("step-1"), serde_json::json!({"answer": 42}));
        let expected_flow_state = run.flow_state.clone();
        let (next_flow_state, authority_input) = run
            .flow_run_command_projection_for_test(
                crate::run::MobMachineFlowRunCommand::TerminalizeCompleted(
                    crate::run::flow_run::inputs::TerminalizeCompleted {},
                ),
            )
            .expect("project completed run state");
        run_store.create_run(run).await.expect("create run");

        let outcome = authority
            .commit_terminalization_with_snapshot(
                &run_id,
                FlowId::from("flow"),
                TerminalizationTarget::Completed {
                    structured_output: Some(serde_json::json!({
                        "steps": {
                            "step-1": {
                                "answer": 42
                            }
                        }
                    })),
                },
                MobRunStatus::Running,
                &expected_flow_state,
                &next_flow_state,
                vec![authority_input],
            )
            .await
            .expect("terminalize");
        assert_eq!(outcome, Some(TerminalizationOutcome::Transitioned));

        let emitted = events.replay_all().await.expect("replay events");
        match &emitted[0].kind {
            MobEventKind::FlowCompleted {
                structured_output, ..
            } => assert_eq!(
                structured_output,
                &Some(serde_json::json!({
                    "steps": {
                        "step-1": {
                            "answer": 42
                        }
                    }
                }))
            ),
            other => panic!("expected FlowCompleted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_completed_terminalization_repair_appends_missing_event() {
        let run_store = Arc::new(RecordingRunStore::new());
        let events = Arc::new(FaultInjectedEventStore::default());
        let authority = FlowTerminalizationAuthority::new(
            run_store.clone(),
            events.clone(),
            MobId::from("mob"),
        );

        let run_id = RunId::new();
        let run = sample_run(run_id.clone(), MobRunStatus::Running);
        let expected_flow_state = run.flow_state.clone();
        let (next_flow_state, authority_input) = run
            .flow_run_command_projection_for_test(
                crate::run::MobMachineFlowRunCommand::TerminalizeCompleted(
                    crate::run::flow_run::inputs::TerminalizeCompleted {},
                ),
            )
            .expect("project completed run state");
        run_store.create_run(run).await.expect("create run");
        events.fail_appends_for("FlowCompleted").await;
        // The non-durable fallback seam surfaces the injected append fault as
        // a typed error after the snapshot commit (the durable SQLite seam has
        // no such window at all — both sides share one transaction).
        authority
            .commit_terminalization_with_snapshot(
                &run_id,
                FlowId::from("flow"),
                TerminalizationTarget::Completed {
                    structured_output: Some(serde_json::json!({
                        "steps": {
                            "step-1": {
                                "answer": 42
                            }
                        }
                    })),
                },
                MobRunStatus::Running,
                &expected_flow_state,
                &next_flow_state,
                vec![authority_input],
            )
            .await
            .expect_err("initial terminal event append should fail");
        events.allow_appends_for("FlowCompleted").await;

        let outcome = authority
            .repair_persisted_terminalization(
                run_id.clone(),
                FlowId::from("flow"),
                TerminalizationTarget::Completed {
                    structured_output: Some(serde_json::json!({
                        "steps": {
                            "step-1": {
                                "answer": 42
                            }
                        }
                    })),
                },
            )
            .await
            .expect("repair terminal event");
        assert_eq!(outcome, TerminalizationOutcome::Noop);

        let emitted = events.replay_all().await.expect("replay events");
        assert_eq!(emitted.len(), 1);
        match &emitted[0].kind {
            MobEventKind::FlowCompleted {
                structured_output, ..
            } => assert_eq!(
                structured_output,
                &Some(serde_json::json!({
                    "steps": {
                        "step-1": {
                            "answer": 42
                        }
                    }
                }))
            ),
            other => panic!("expected FlowCompleted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_completed_terminalization_repair_is_idempotent() {
        let run_store = Arc::new(RecordingRunStore::new());
        let events = Arc::new(FaultInjectedEventStore::default());
        let authority = FlowTerminalizationAuthority::new(
            run_store.clone(),
            events.clone(),
            MobId::from("mob"),
        );

        let run_id = RunId::new();
        let run = sample_run(run_id.clone(), MobRunStatus::Running);
        let expected_flow_state = run.flow_state.clone();
        let (next_flow_state, authority_input) = run
            .flow_run_command_projection_for_test(
                crate::run::MobMachineFlowRunCommand::TerminalizeCompleted(
                    crate::run::flow_run::inputs::TerminalizeCompleted {},
                ),
            )
            .expect("project completed run state");
        run_store.create_run(run).await.expect("create run");

        authority
            .commit_terminalization_with_snapshot(
                &run_id,
                FlowId::from("flow"),
                TerminalizationTarget::Completed {
                    structured_output: Some(serde_json::json!({"steps": {"step-1": "ok"}})),
                },
                MobRunStatus::Running,
                &expected_flow_state,
                &next_flow_state,
                vec![authority_input],
            )
            .await
            .expect("record terminal event");
        let outcome = authority
            .repair_persisted_terminalization(
                run_id.clone(),
                FlowId::from("flow"),
                TerminalizationTarget::Completed {
                    structured_output: Some(serde_json::json!({"steps": {"step-1": "ok"}})),
                },
            )
            .await
            .expect("repair terminal event");

        assert_eq!(outcome, TerminalizationOutcome::Noop);
        assert_eq!(events.replay_all().await.expect("replay events").len(), 1);
    }

    #[tokio::test]
    async fn test_completed_terminalization_repair_uses_persisted_status() {
        let run_store = Arc::new(RecordingRunStore::new());
        let events = Arc::new(FaultInjectedEventStore::default());
        let authority = FlowTerminalizationAuthority::new(
            run_store.clone(),
            events.clone(),
            MobId::from("mob"),
        );

        let run_id = RunId::new();
        let mut run = sample_run(run_id.clone(), MobRunStatus::Completed);
        run.root_step_outputs
            .insert(StepId::from("step-1"), serde_json::json!({"answer": 42}));
        run_store.create_run(run).await.expect("create run");

        let outcome = authority
            .repair_persisted_terminalization(
                run_id.clone(),
                FlowId::from("flow"),
                TerminalizationTarget::Failed {
                    cause: FlowFailureCause::StepError {
                        class: FlowFailureClass::Internal,
                        detail: "fallback failure".to_string(),
                    },
                },
            )
            .await
            .expect("repair terminal event");
        assert_eq!(outcome, TerminalizationOutcome::Noop);

        let emitted = events.replay_all().await.expect("replay events");
        assert_eq!(emitted.len(), 1);
        match &emitted[0].kind {
            MobEventKind::FlowCompleted {
                structured_output, ..
            } => assert_eq!(
                structured_output,
                &Some(serde_json::json!({
                    "steps": {
                        "step-1": {
                            "answer": 42
                        }
                    }
                }))
            ),
            other => panic!("expected FlowCompleted repair, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_completed_terminalization_repair_preserves_loop_outputs() {
        let run_store = Arc::new(RecordingRunStore::new());
        let events = Arc::new(FaultInjectedEventStore::default());
        let authority = FlowTerminalizationAuthority::new(
            run_store.clone(),
            events.clone(),
            MobId::from("mob"),
        );

        let run_id = RunId::new();
        let mut run = sample_run(run_id.clone(), MobRunStatus::Completed);
        let mut iteration = indexmap::IndexMap::new();
        iteration.insert(StepId::from("body-step"), serde_json::json!({"loop": true}));
        run.loop_iteration_outputs
            .insert(LoopId::from("loop-1"), vec![iteration]);
        run_store.create_run(run).await.expect("create run");

        let outcome = authority
            .repair_persisted_terminalization(
                run_id.clone(),
                FlowId::from("flow"),
                TerminalizationTarget::Failed {
                    cause: FlowFailureCause::StepError {
                        class: FlowFailureClass::Internal,
                        detail: "fallback failure".to_string(),
                    },
                },
            )
            .await
            .expect("repair terminal event");
        assert_eq!(outcome, TerminalizationOutcome::Noop);

        let emitted = events.replay_all().await.expect("replay events");
        assert_eq!(emitted.len(), 1);
        match &emitted[0].kind {
            MobEventKind::FlowCompleted {
                structured_output, ..
            } => assert_eq!(
                structured_output,
                &Some(serde_json::json!({
                    "steps": {
                        "body-step": {
                            "loop": true
                        }
                    }
                }))
            ),
            other => panic!("expected FlowCompleted repair, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_terminalization_surfaces_append_error_without_provenance_mutation() {
        let run_store = Arc::new(InMemoryMobRunStore::new());
        let events = Arc::new(FaultInjectedEventStore::default());
        events.fail_appends_for("FlowFailed").await;
        let authority =
            FlowTerminalizationAuthority::new(run_store.clone(), events, MobId::from("mob"));

        let run_id = RunId::new();
        let run = sample_run(run_id.clone(), MobRunStatus::Running);
        let expected_flow_state = run.flow_state.clone();
        let (next_flow_state, authority_input) = run
            .flow_run_command_projection_for_test(
                crate::run::MobMachineFlowRunCommand::TerminalizeFailed(
                    crate::run::flow_run::inputs::TerminalizeFailed {},
                ),
            )
            .expect("project failed run state");
        run_store.create_run(run).await.expect("create run");

        let error = authority
            .commit_terminalization_with_snapshot(
                &run_id,
                FlowId::from("flow"),
                TerminalizationTarget::Failed {
                    cause: FlowFailureCause::StepError {
                        class: FlowFailureClass::Internal,
                        detail: "boom".to_string(),
                    },
                },
                MobRunStatus::Running,
                &expected_flow_state,
                &next_flow_state,
                vec![authority_input],
            )
            .await
            .expect_err("append failure should surface as terminalization error");
        assert!(
            error
                .to_string()
                .contains("terminal run status persisted but terminal event append failed"),
            "error should mention terminal append divergence"
        );

        let run = run_store
            .get_run(&run_id)
            .await
            .expect("get run")
            .expect("run exists");
        assert_eq!(run.status, MobRunStatus::Failed);
        assert!(
            run.failure_ledger.is_empty(),
            "failed terminal append must not invent run provenance outside machine authority"
        );
    }
}

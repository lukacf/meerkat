use super::flow_system_step_id;
use crate::error::MobError;
use crate::event::{MobEventKind, NewMobEvent};
use crate::ids::{FlowId, MobId, RunId};
use crate::run::{FailureLedgerEntry, MobRun, MobRunStatus};
use crate::store::{MobEventStore, MobRunStore};
use chrono::Utc;
use serde_json::{Map, Value};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub(super) enum TerminalizationTarget {
    Completed { structured_output: Option<Value> },
    Failed { reason: String },
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
            Self::Failed { reason } => MobEventKind::FlowFailed {
                run_id,
                flow_id,
                reason,
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
            run_store,
            events,
            mob_id,
        }
    }

    pub(super) async fn record_persisted_terminalization(
        &self,
        run_id: RunId,
        flow_id: FlowId,
        target: TerminalizationTarget,
    ) -> Result<TerminalizationOutcome, MobError> {
        let event_name = target.event_name();
        let event_kind = target.into_event_kind(run_id.clone(), flow_id);
        if let Err(append_error) = self
            .events
            .append(NewMobEvent {
                mob_id: self.mob_id.clone(),
                timestamp: None,
                kind: event_kind,
            })
            .await
        {
            return self
                .record_terminal_event_append_failure(
                    &run_id,
                    event_name,
                    MobError::from(append_error),
                )
                .await;
        }

        Ok(TerminalizationOutcome::Transitioned)
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
        if !run.status.is_terminal() {
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
                let reason = match requested {
                    TerminalizationTarget::Failed { reason } => reason,
                    _ => run
                        .failure_ledger
                        .last()
                        .map(|entry| entry.reason.clone())
                        .unwrap_or_else(|| "run already failed".to_string()),
                };
                TerminalizationTarget::Failed { reason }
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
        if let Err(failure_ledger_error) = self
            .run_store
            .append_failure_entry(
                run_id,
                FailureLedgerEntry {
                    step_id: flow_system_step_id(),
                    reason: reason.clone(),
                    error_report: None,
                    error: None,
                    timestamp: Utc::now(),
                },
            )
            .await
        {
            tracing::error!(
                run_id = %run_id,
                event_name,
                append_error = %append_error,
                ledger_error = %failure_ledger_error,
                "terminal event append divergence could not be recorded in failure ledger"
            );
            return Err(MobError::Internal(format!(
                "terminal run status persisted but {event_name} append failed and failure ledger append also failed: {failure_ledger_error}"
            )));
        }

        tracing::error!(
            run_id = %run_id,
            event_name,
            append_error = %append_error,
            "terminal event append divergence recorded in failure ledger"
        );
        Err(MobError::Internal(reason))
    }
}

#[cfg(test)]
mod tests {
    use super::{FlowTerminalizationAuthority, TerminalizationOutcome, TerminalizationTarget};
    use crate::event::{MobEvent, MobEventKind, NewMobEvent};
    use crate::ids::{FlowId, LoopId, MobId, RunId, StepId};
    use crate::run::{FailureLedgerEntry, MobRun, MobRunStatus, StepLedgerEntry};
    use crate::store::{
        InMemoryMobRunStore, MobEventStore, MobRunStore, MobStoreError, terminal_event_identity,
    };
    use async_trait::async_trait;
    use chrono::Utc;
    use std::collections::HashSet;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    struct RecordingRunStore {
        inner: InMemoryMobRunStore,
        fail_get_run: RwLock<bool>,
    }

    impl RecordingRunStore {
        fn new() -> Self {
            Self {
                inner: InMemoryMobRunStore::new(),
                fail_get_run: RwLock::new(false),
            }
        }

        async fn fail_get_run(&self, fail: bool) {
            *self.fail_get_run.write().await = fail;
        }
    }

    #[async_trait]
    impl MobRunStore for RecordingRunStore {
        async fn create_run(&self, run: MobRun) -> Result<(), MobStoreError> {
            self.inner.create_run(run).await
        }

        async fn get_run(&self, run_id: &RunId) -> Result<Option<MobRun>, MobStoreError> {
            if *self.fail_get_run.read().await {
                return Err(MobStoreError::Internal(
                    "fault-injected get_run failure".to_string(),
                ));
            }
            self.inner.get_run(run_id).await
        }

        async fn list_runs(
            &self,
            mob_id: &MobId,
            flow_id: Option<&FlowId>,
        ) -> Result<Vec<MobRun>, MobStoreError> {
            self.inner.list_runs(mob_id, flow_id).await
        }

        async fn cas_run_status(
            &self,
            run_id: &RunId,
            expected: MobRunStatus,
            next: MobRunStatus,
        ) -> Result<bool, MobStoreError> {
            self.inner.cas_run_status(run_id, expected, next).await
        }

        async fn cas_flow_state(
            &self,
            run_id: &RunId,
            expected: &crate::run::flow_run::State,
            next: &crate::run::flow_run::State,
        ) -> Result<bool, MobStoreError> {
            self.inner.cas_flow_state(run_id, expected, next).await
        }

        async fn cas_run_snapshot(
            &self,
            run_id: &RunId,
            expected_status: MobRunStatus,
            expected_flow_state: &crate::run::flow_run::State,
            next_status: MobRunStatus,
            next_flow_state: &crate::run::flow_run::State,
        ) -> Result<bool, MobStoreError> {
            self.inner
                .cas_run_snapshot(
                    run_id,
                    expected_status,
                    expected_flow_state,
                    next_status,
                    next_flow_state,
                )
                .await
        }

        async fn append_step_entry(
            &self,
            run_id: &RunId,
            entry: StepLedgerEntry,
        ) -> Result<(), MobStoreError> {
            self.inner.append_step_entry(run_id, entry).await
        }

        async fn append_step_entry_if_absent(
            &self,
            run_id: &RunId,
            entry: StepLedgerEntry,
        ) -> Result<bool, MobStoreError> {
            self.inner.append_step_entry_if_absent(run_id, entry).await
        }

        async fn put_step_output(
            &self,
            run_id: &RunId,
            step_id: &StepId,
            output: serde_json::Value,
        ) -> Result<(), MobStoreError> {
            self.inner.put_step_output(run_id, step_id, output).await
        }

        async fn append_failure_entry(
            &self,
            run_id: &RunId,
            entry: FailureLedgerEntry,
        ) -> Result<(), MobStoreError> {
            self.inner.append_failure_entry(run_id, entry).await
        }

        async fn upsert_loop_snapshot(
            &self,
            run_id: &RunId,
            loop_instance_id: &crate::ids::LoopInstanceId,
            snapshot: crate::run::LoopSnapshot,
            ledger_entry: Option<crate::run::LoopIterationLedgerEntry>,
        ) -> Result<(), MobStoreError> {
            self.inner
                .upsert_loop_snapshot(run_id, loop_instance_id, snapshot, ledger_entry)
                .await
        }

        async fn cas_frame_state(
            &self,
            run_id: &RunId,
            frame_id: &crate::ids::FrameId,
            expected: Option<&crate::run::FrameSnapshot>,
            next: crate::run::FrameSnapshot,
        ) -> Result<bool, MobStoreError> {
            self.inner
                .cas_frame_state(run_id, frame_id, expected, next)
                .await
        }

        async fn cas_grant_node_slot(
            &self,
            run_id: &RunId,
            expected_run_state: &crate::run::flow_run::State,
            next_run_state: crate::run::flow_run::State,
            frame_id: &crate::ids::FrameId,
            expected_frame: &crate::run::FrameSnapshot,
            next_frame: crate::run::FrameSnapshot,
        ) -> Result<bool, MobStoreError> {
            self.inner
                .cas_grant_node_slot(
                    run_id,
                    expected_run_state,
                    next_run_state,
                    frame_id,
                    expected_frame,
                    next_frame,
                )
                .await
        }

        async fn cas_complete_step_and_record_output(
            &self,
            run_id: &RunId,
            frame_id: &crate::ids::FrameId,
            expected_frame: &crate::run::FrameSnapshot,
            next_frame: crate::run::FrameSnapshot,
            step_output_key: String,
            step_output: serde_json::Value,
            loop_context: Option<(&crate::ids::LoopId, u64)>,
        ) -> Result<bool, MobStoreError> {
            self.inner
                .cas_complete_step_and_record_output(
                    run_id,
                    frame_id,
                    expected_frame,
                    next_frame,
                    step_output_key,
                    step_output,
                    loop_context,
                )
                .await
        }

        async fn cas_start_loop(
            &self,
            run_id: &RunId,
            loop_instance_id: &crate::ids::LoopInstanceId,
            expected_run_state: &crate::run::flow_run::State,
            next_run_state: crate::run::flow_run::State,
            frame_id: &crate::ids::FrameId,
            expected_frame: &crate::run::FrameSnapshot,
            next_frame: crate::run::FrameSnapshot,
            initial_loop: crate::run::LoopSnapshot,
        ) -> Result<bool, MobStoreError> {
            self.inner
                .cas_start_loop(
                    run_id,
                    loop_instance_id,
                    expected_run_state,
                    next_run_state,
                    frame_id,
                    expected_frame,
                    next_frame,
                    initial_loop,
                )
                .await
        }

        async fn cas_loop_request_body_frame(
            &self,
            run_id: &RunId,
            loop_instance_id: &crate::ids::LoopInstanceId,
            expected_loop: &crate::run::LoopSnapshot,
            next_loop: crate::run::LoopSnapshot,
            expected_run_state: &crate::run::flow_run::State,
            next_run_state: crate::run::flow_run::State,
        ) -> Result<bool, MobStoreError> {
            self.inner
                .cas_loop_request_body_frame(
                    run_id,
                    loop_instance_id,
                    expected_loop,
                    next_loop,
                    expected_run_state,
                    next_run_state,
                )
                .await
        }

        async fn cas_grant_body_frame_start(
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
        ) -> Result<bool, MobStoreError> {
            self.inner
                .cas_grant_body_frame_start(
                    run_id,
                    loop_instance_id,
                    expected_loop,
                    next_loop,
                    frame_id,
                    initial_frame,
                    ledger_entry,
                    expected_run_state,
                    next_run_state,
                )
                .await
        }

        async fn cas_complete_body_frame(
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
        ) -> Result<bool, MobStoreError> {
            self.inner
                .cas_complete_body_frame(
                    run_id,
                    loop_instance_id,
                    expected_loop,
                    next_loop,
                    frame_id,
                    expected_frame,
                    next_frame,
                    expected_run_state,
                    next_run_state,
                )
                .await
        }

        async fn cas_complete_loop(
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
        ) -> Result<bool, MobStoreError> {
            self.inner
                .cas_complete_loop(
                    run_id,
                    loop_instance_id,
                    expected_loop,
                    next_loop,
                    frame_id,
                    expected_frame,
                    next_frame,
                    expected_run_state,
                    next_run_state,
                )
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
        MobRun {
            run_id,
            mob_id: MobId::from("mob"),
            flow_id: FlowId::from("flow"),
            status,
            flow_state: MobRun::flow_state_for_steps([crate::ids::StepId::from("step-1")]).unwrap(),
            activation_params: serde_json::json!({}),
            created_at: Utc::now(),
            completed_at: None,
            step_ledger: Vec::new(),
            failure_ledger: Vec::new(),
            frames: std::collections::BTreeMap::new(),
            loops: std::collections::BTreeMap::new(),
            loop_iteration_ledger: Vec::new(),
            schema_version: 4,
            root_step_outputs: indexmap::IndexMap::new(),
            loop_iteration_outputs: std::collections::BTreeMap::new(),
            flow_authority_inputs: Vec::new(),
        }
    }

    #[tokio::test]
    async fn test_terminalization_records_persisted_terminal_event() {
        let run_store = Arc::new(RecordingRunStore::new());
        let events = Arc::new(FaultInjectedEventStore::default());
        let authority =
            FlowTerminalizationAuthority::new(run_store.clone(), events, MobId::from("mob"));

        let run_id = RunId::new();
        run_store
            .create_run(sample_run(run_id.clone(), MobRunStatus::Pending))
            .await
            .expect("create run");

        let outcome = authority
            .record_persisted_terminalization(
                run_id.clone(),
                FlowId::from("flow"),
                TerminalizationTarget::Canceled,
            )
            .await
            .expect("terminalize");
        assert_eq!(outcome, TerminalizationOutcome::Transitioned);
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
        let mut run = sample_run(run_id.clone(), MobRunStatus::Completed);
        run.root_step_outputs
            .insert(StepId::from("step-1"), serde_json::json!({"answer": 42}));
        run_store.create_run(run).await.expect("create run");

        let outcome = authority
            .record_persisted_terminalization(
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
            .expect("terminalize");
        assert_eq!(outcome, TerminalizationOutcome::Transitioned);

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
    async fn test_completed_terminalization_does_not_read_snapshot_for_structured_output() {
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
        run_store.fail_get_run(true).await;

        let outcome = authority
            .record_persisted_terminalization(
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
            .expect("terminalize");

        assert_eq!(outcome, TerminalizationOutcome::Transitioned);
        assert_eq!(
            events.replay_all().await.expect("replay events").len(),
            1,
            "FlowCompleted should use the target output and avoid an extra run read"
        );
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
        run_store
            .create_run(sample_run(run_id.clone(), MobRunStatus::Completed))
            .await
            .expect("create run");
        events.fail_appends_for("FlowCompleted").await;
        authority
            .record_persisted_terminalization(
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
        run_store
            .create_run(sample_run(run_id.clone(), MobRunStatus::Completed))
            .await
            .expect("create run");

        authority
            .record_persisted_terminalization(
                run_id.clone(),
                FlowId::from("flow"),
                TerminalizationTarget::Completed {
                    structured_output: Some(serde_json::json!({"steps": {"step-1": "ok"}})),
                },
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
                    reason: "fallback failure".to_string(),
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
                    reason: "fallback failure".to_string(),
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
    async fn test_terminalization_records_coherence_failure_ledger_on_append_error() {
        let run_store = Arc::new(InMemoryMobRunStore::new());
        let events = Arc::new(FaultInjectedEventStore::default());
        events.fail_appends_for("FlowFailed").await;
        let authority =
            FlowTerminalizationAuthority::new(run_store.clone(), events, MobId::from("mob"));

        let run_id = RunId::new();
        run_store
            .create_run(sample_run(run_id.clone(), MobRunStatus::Failed))
            .await
            .expect("create run");

        let error = authority
            .record_persisted_terminalization(
                run_id.clone(),
                FlowId::from("flow"),
                TerminalizationTarget::Failed {
                    reason: "boom".to_string(),
                },
            )
            .await
            .expect_err("append failure should surface as terminalization error");
        assert!(
            error
                .to_string()
                .contains("terminal run status persisted but FlowFailed append failed"),
            "error should mention terminal append divergence"
        );

        let run = run_store
            .get_run(&run_id)
            .await
            .expect("get run")
            .expect("run exists");
        assert_eq!(run.status, MobRunStatus::Failed);
        assert!(
            run.failure_ledger.iter().any(|entry| {
                entry.step_id == crate::runtime::flow_system_step_id()
                    && entry.reason.contains("FlowFailed append failed")
            }),
            "failed terminal append should always be mirrored into failure ledger"
        );
    }
}

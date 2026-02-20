use super::flow_system_step_id;
use crate::error::MobError;
use crate::event::{MobEventKind, NewMobEvent};
use crate::ids::{FlowId, MobId, RunId};
use crate::run::{FailureLedgerEntry, MobRunStatus};
use crate::store::{MobEventStore, MobRunStore};
use chrono::Utc;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub(super) enum TerminalizationTarget {
    Completed,
    Failed { reason: String },
    Canceled,
}

impl TerminalizationTarget {
    fn status(&self) -> MobRunStatus {
        match self {
            Self::Completed => MobRunStatus::Completed,
            Self::Failed { .. } => MobRunStatus::Failed,
            Self::Canceled => MobRunStatus::Canceled,
        }
    }

    fn event_name(&self) -> &'static str {
        match self {
            Self::Completed => "FlowCompleted",
            Self::Failed { .. } => "FlowFailed",
            Self::Canceled => "FlowCanceled",
        }
    }

    fn into_event_kind(self, run_id: RunId, flow_id: FlowId) -> MobEventKind {
        match self {
            Self::Completed => MobEventKind::FlowCompleted { run_id, flow_id },
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
pub(super) enum TerminalizationOutcome {
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

    pub(super) async fn terminalize(
        &self,
        run_id: RunId,
        flow_id: FlowId,
        target: TerminalizationTarget,
    ) -> Result<TerminalizationOutcome, MobError> {
        let next = target.status();
        if !self
            .cas_pending_or_running_to_terminal(&run_id, next)
            .await?
        {
            return Ok(TerminalizationOutcome::Noop);
        }

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
                .record_terminal_event_append_failure(&run_id, event_name, append_error)
                .await;
        }

        Ok(TerminalizationOutcome::Transitioned)
    }

    async fn cas_pending_or_running_to_terminal(
        &self,
        run_id: &RunId,
        next: MobRunStatus,
    ) -> Result<bool, MobError> {
        // Retry twice to close the race where a concurrent actor flips Pending->Running
        // between our direct CAS attempts.
        for _ in 0..2 {
            if self
                .run_store
                .cas_run_status(run_id, MobRunStatus::Pending, next.clone())
                .await?
            {
                return Ok(true);
            }
            if self
                .run_store
                .cas_run_status(run_id, MobRunStatus::Running, next.clone())
                .await?
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

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
    use crate::ids::{FlowId, MobId, RunId, StepId};
    use crate::run::{FailureLedgerEntry, MobRun, MobRunStatus, StepLedgerEntry};
    use crate::store::{InMemoryMobRunStore, MobEventStore, MobRunStore};
    use async_trait::async_trait;
    use chrono::Utc;
    use std::collections::HashSet;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    struct RecordingRunStore {
        inner: InMemoryMobRunStore,
        cas_history: RwLock<Vec<(RunId, MobRunStatus, MobRunStatus)>>,
    }

    impl RecordingRunStore {
        fn new() -> Self {
            Self {
                inner: InMemoryMobRunStore::new(),
                cas_history: RwLock::new(Vec::new()),
            }
        }

        async fn cas_history(&self) -> Vec<(RunId, MobRunStatus, MobRunStatus)> {
            self.cas_history.read().await.clone()
        }
    }

    #[async_trait]
    impl MobRunStore for RecordingRunStore {
        async fn create_run(&self, run: MobRun) -> Result<(), crate::error::MobError> {
            self.inner.create_run(run).await
        }

        async fn get_run(&self, run_id: &RunId) -> Result<Option<MobRun>, crate::error::MobError> {
            self.inner.get_run(run_id).await
        }

        async fn list_runs(
            &self,
            mob_id: &MobId,
            flow_id: Option<&FlowId>,
        ) -> Result<Vec<MobRun>, crate::error::MobError> {
            self.inner.list_runs(mob_id, flow_id).await
        }

        async fn cas_run_status(
            &self,
            run_id: &RunId,
            expected: MobRunStatus,
            next: MobRunStatus,
        ) -> Result<bool, crate::error::MobError> {
            self.cas_history
                .write()
                .await
                .push((run_id.clone(), expected.clone(), next.clone()));
            self.inner.cas_run_status(run_id, expected, next).await
        }

        async fn append_step_entry(
            &self,
            run_id: &RunId,
            entry: StepLedgerEntry,
        ) -> Result<(), crate::error::MobError> {
            self.inner.append_step_entry(run_id, entry).await
        }

        async fn append_step_entry_if_absent(
            &self,
            run_id: &RunId,
            entry: StepLedgerEntry,
        ) -> Result<bool, crate::error::MobError> {
            self.inner.append_step_entry_if_absent(run_id, entry).await
        }

        async fn put_step_output(
            &self,
            run_id: &RunId,
            step_id: &StepId,
            output: serde_json::Value,
        ) -> Result<(), crate::error::MobError> {
            self.inner.put_step_output(run_id, step_id, output).await
        }

        async fn append_failure_entry(
            &self,
            run_id: &RunId,
            entry: FailureLedgerEntry,
        ) -> Result<(), crate::error::MobError> {
            self.inner.append_failure_entry(run_id, entry).await
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
        async fn append(&self, event: NewMobEvent) -> Result<MobEvent, crate::error::MobError> {
            let kind = Self::kind_label(&event.kind);
            if self.fail_on_kind.read().await.contains(kind) {
                return Err(crate::error::MobError::Internal(format!(
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

        async fn poll(
            &self,
            after_cursor: u64,
            limit: usize,
        ) -> Result<Vec<MobEvent>, crate::error::MobError> {
            let events = self.events.read().await;
            Ok(events
                .iter()
                .filter(|event| event.cursor > after_cursor)
                .take(limit)
                .cloned()
                .collect())
        }

        async fn replay_all(&self) -> Result<Vec<MobEvent>, crate::error::MobError> {
            Ok(self.events.read().await.clone())
        }

        async fn clear(&self) -> Result<(), crate::error::MobError> {
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
            activation_params: serde_json::json!({}),
            created_at: Utc::now(),
            completed_at: None,
            step_ledger: Vec::new(),
            failure_ledger: Vec::new(),
        }
    }

    #[tokio::test]
    async fn test_terminalization_uses_direct_pending_to_terminal_cas() {
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
            .terminalize(
                run_id.clone(),
                FlowId::from("flow"),
                TerminalizationTarget::Canceled,
            )
            .await
            .expect("terminalize");
        assert_eq!(outcome, TerminalizationOutcome::Transitioned);

        let history = run_store.cas_history().await;
        assert!(
            history
                .iter()
                .any(|(_, expected, next)| *expected == MobRunStatus::Pending
                    && *next == MobRunStatus::Canceled),
            "terminalization must include a direct Pending->Canceled CAS attempt"
        );
        assert!(
            !history
                .iter()
                .any(|(_, expected, next)| *expected == MobRunStatus::Pending
                    && *next == MobRunStatus::Running),
            "terminalization authority must never force Pending->Running promotion"
        );
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
            .create_run(sample_run(run_id.clone(), MobRunStatus::Running))
            .await
            .expect("create run");

        let error = authority
            .terminalize(
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

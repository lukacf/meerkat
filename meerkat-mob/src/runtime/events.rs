use crate::error::MobError;
use crate::event::{MobEvent, MobEventKind, NewMobEvent};
use crate::ids::{FlowId, MeerkatId, MobId, RunId, StepId};
use crate::store::MobEventStore;
use std::sync::Arc;

#[derive(Clone)]
pub struct MobEventEmitter {
    store: Arc<dyn MobEventStore>,
    mob_id: MobId,
}

impl MobEventEmitter {
    pub fn new(store: Arc<dyn MobEventStore>, mob_id: MobId) -> Self {
        Self { store, mob_id }
    }

    async fn append(&self, kind: MobEventKind) -> Result<MobEvent, MobError> {
        self.store
            .append(NewMobEvent {
                mob_id: self.mob_id.clone(),
                timestamp: None,
                kind,
            })
            .await
    }

    pub async fn flow_started(
        &self,
        run_id: RunId,
        flow_id: FlowId,
        params: serde_json::Value,
    ) -> Result<MobEvent, MobError> {
        self.append(MobEventKind::FlowStarted {
            run_id,
            flow_id,
            params,
        })
        .await
    }

    pub async fn flow_completed(
        &self,
        run_id: RunId,
        flow_id: FlowId,
    ) -> Result<MobEvent, MobError> {
        self.append(MobEventKind::FlowCompleted { run_id, flow_id })
            .await
    }

    pub async fn flow_failed(
        &self,
        run_id: RunId,
        flow_id: FlowId,
        reason: String,
    ) -> Result<MobEvent, MobError> {
        self.append(MobEventKind::FlowFailed {
            run_id,
            flow_id,
            reason,
        })
        .await
    }

    pub async fn flow_canceled(
        &self,
        run_id: RunId,
        flow_id: FlowId,
    ) -> Result<MobEvent, MobError> {
        self.append(MobEventKind::FlowCanceled { run_id, flow_id })
            .await
    }

    pub async fn step_dispatched(
        &self,
        run_id: RunId,
        step_id: StepId,
        meerkat_id: MeerkatId,
    ) -> Result<MobEvent, MobError> {
        self.append(MobEventKind::StepDispatched {
            run_id,
            step_id,
            meerkat_id,
        })
        .await
    }

    pub async fn step_target_completed(
        &self,
        run_id: RunId,
        step_id: StepId,
        meerkat_id: MeerkatId,
    ) -> Result<MobEvent, MobError> {
        self.append(MobEventKind::StepTargetCompleted {
            run_id,
            step_id,
            meerkat_id,
        })
        .await
    }

    pub async fn step_target_failed(
        &self,
        run_id: RunId,
        step_id: StepId,
        meerkat_id: MeerkatId,
        reason: String,
    ) -> Result<MobEvent, MobError> {
        self.append(MobEventKind::StepTargetFailed {
            run_id,
            step_id,
            meerkat_id,
            reason,
        })
        .await
    }

    pub async fn step_completed(
        &self,
        run_id: RunId,
        step_id: StepId,
    ) -> Result<MobEvent, MobError> {
        self.append(MobEventKind::StepCompleted { run_id, step_id })
            .await
    }

    pub async fn step_failed(
        &self,
        run_id: RunId,
        step_id: StepId,
        reason: String,
    ) -> Result<MobEvent, MobError> {
        self.append(MobEventKind::StepFailed {
            run_id,
            step_id,
            reason,
        })
        .await
    }

    pub async fn step_skipped(
        &self,
        run_id: RunId,
        step_id: StepId,
        reason: String,
    ) -> Result<MobEvent, MobError> {
        self.append(MobEventKind::StepSkipped {
            run_id,
            step_id,
            reason,
        })
        .await
    }

    pub async fn topology_violation(
        &self,
        from_role: String,
        to_role: String,
    ) -> Result<MobEvent, MobError> {
        self.append(MobEventKind::TopologyViolation { from_role, to_role })
            .await
    }

    pub async fn supervisor_escalation(
        &self,
        run_id: RunId,
        step_id: StepId,
        escalated_to: MeerkatId,
    ) -> Result<MobEvent, MobError> {
        self.append(MobEventKind::SupervisorEscalation {
            run_id,
            step_id,
            escalated_to,
        })
        .await
    }
}

use crate::error::MobError;
use crate::event::{MobEvent, MobEventKind, NewMobEvent};
use crate::ids::{
    AgentIdentity, AgentRuntimeId, FlowId, MeerkatId, MobId, ProfileName, RunId, StepId,
};
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
            .map_err(MobError::from)
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

    pub async fn step_dispatched(
        &self,
        run_id: RunId,
        step_id: StepId,
        agent_identity: MeerkatId,
    ) -> Result<MobEvent, MobError> {
        let target = AgentRuntimeId::initial(AgentIdentity::from(agent_identity.as_str()));
        self.append(MobEventKind::StepDispatched {
            run_id,
            step_id,
            target,
        })
        .await
    }

    pub async fn step_target_completed(
        &self,
        run_id: RunId,
        step_id: StepId,
        agent_identity: MeerkatId,
    ) -> Result<MobEvent, MobError> {
        let target = AgentRuntimeId::initial(AgentIdentity::from(agent_identity.as_str()));
        self.append(MobEventKind::StepTargetCompleted {
            run_id,
            step_id,
            target,
        })
        .await
    }

    pub async fn step_target_failed(
        &self,
        run_id: RunId,
        step_id: StepId,
        agent_identity: MeerkatId,
        reason: String,
    ) -> Result<MobEvent, MobError> {
        let target = AgentRuntimeId::initial(AgentIdentity::from(agent_identity.as_str()));
        self.append(MobEventKind::StepTargetFailed {
            run_id,
            step_id,
            target,
            reason,
            error_report: None,
            error: None,
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
        from_role: ProfileName,
        to_role: ProfileName,
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
            escalated_to: AgentIdentity::from(escalated_to.as_str()),
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::MobEventEmitter;
    use crate::event::MobEventKind;
    use crate::ids::{MeerkatId, MobId, RunId, StepId};
    use crate::store::{InMemoryMobEventStore, MobEventStore};
    use std::sync::Arc;

    #[tokio::test]
    async fn step_target_failed_persists_display_reason_only() {
        let store: Arc<dyn MobEventStore> = Arc::new(InMemoryMobEventStore::new());
        let emitter = MobEventEmitter::new(Arc::clone(&store), MobId::from("mob-display-error"));

        emitter
            .step_target_failed(
                RunId::new(),
                StepId::from("review"),
                MeerkatId::from("reviewer"),
                "LLM failure terminal turn".to_string(),
            )
            .await
            .expect("event append should succeed");

        let events = store
            .replay_all()
            .await
            .expect("event replay should succeed");
        match &events
            .first()
            .expect("step target failed event should persist")
            .kind
        {
            MobEventKind::StepTargetFailed {
                reason,
                error_report,
                error: persisted_error,
                ..
            } => {
                assert_eq!(reason, "LLM failure terminal turn");
                assert_eq!(error_report.as_ref(), None);
                assert_eq!(persisted_error.as_ref(), None);
            }
            other => panic!("expected StepTargetFailed, got {other:?}"),
        }
    }
}

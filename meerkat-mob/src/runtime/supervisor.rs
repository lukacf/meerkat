use super::events::MobEventEmitter;
use super::handle::MobHandle;
use crate::error::MobError;
use crate::ids::{RunId, StepId};
use crate::run::FlowRunConfig;

pub struct Supervisor {
    handle: MobHandle,
    emitter: MobEventEmitter,
}

impl Supervisor {
    pub fn new(handle: MobHandle, emitter: MobEventEmitter) -> Self {
        Self { handle, emitter }
    }

    pub async fn escalate(
        &self,
        config: &FlowRunConfig,
        run_id: &RunId,
        step_id: &StepId,
        reason: &str,
    ) -> Result<(), MobError> {
        let supervisor_role = config
            .supervisor
            .as_ref()
            .ok_or_else(|| MobError::SupervisorEscalation("supervisor not configured".into()))?
            .role
            .clone();

        let escalation_target = self
            .handle
            .list_meerkats()
            .await
            .into_iter()
            .find(|entry| entry.profile.as_str() == supervisor_role)
            .ok_or_else(|| {
                MobError::SupervisorEscalation(format!(
                    "no active supervisor member for role '{supervisor_role}'"
                ))
            })?;

        self.handle
            .internal_turn(
                escalation_target.meerkat_id.clone(),
                format!(
                    "Supervisor escalation for run '{run_id}', step '{step_id}': {reason}"
                ),
            )
            .await?;

        self.emitter
            .supervisor_escalation(
                run_id.clone(),
                step_id.clone(),
                escalation_target.meerkat_id,
            )
            .await?;

        Ok(())
    }

    pub async fn force_reset(&self) -> Result<(), MobError> {
        let ids = self
            .handle
            .list_meerkats()
            .await
            .into_iter()
            .map(|entry| entry.meerkat_id)
            .collect::<Vec<_>>();

        for id in ids {
            self.handle.retire(id).await?;
        }

        Ok(())
    }
}

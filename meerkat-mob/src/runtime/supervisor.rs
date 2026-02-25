use super::events::MobEventEmitter;
use super::handle::MobHandle;
use crate::error::MobError;
use crate::ids::{RunId, StepId};
use crate::run::FlowRunConfig;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use std::time::Duration;

const ESCALATION_TURN_TIMEOUT: Duration = Duration::from_secs(2);

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
            .list_members()
            .await
            .into_iter()
            .find(|entry| entry.profile == supervisor_role)
            .ok_or_else(|| {
                MobError::SupervisorEscalation(format!(
                    "no active supervisor member for role '{supervisor_role}'"
                ))
            })?;

        tokio::time::timeout(
            ESCALATION_TURN_TIMEOUT,
            self.handle.internal_turn(
                escalation_target.meerkat_id.clone(),
                format!("Supervisor escalation for run '{run_id}', step '{step_id}': {reason}"),
            ),
        )
        .await
        .map_err(|_| {
            MobError::SupervisorEscalation(format!(
                "supervisor escalation timed out after {}ms",
                ESCALATION_TURN_TIMEOUT.as_millis()
            ))
        })??;

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
            .list_members()
            .await
            .into_iter()
            .map(|entry| entry.meerkat_id)
            .collect::<Vec<_>>();

        let mut failures = Vec::new();
        for id in ids {
            if let Err(error) = self.handle.retire(id.clone()).await {
                failures.push(format!("{id}: {error}"));
            }
        }

        if !failures.is_empty() {
            return Err(MobError::SupervisorEscalation(format!(
                "force_reset encountered {} retirement error(s): {}",
                failures.len(),
                failures.join("; ")
            )));
        }

        Ok(())
    }
}

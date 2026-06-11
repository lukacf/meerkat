use super::events::MobEventEmitter;
use super::handle::MobHandle;
use crate::error::MobError;
use crate::ids::{AgentIdentity, RunId, StepId};
use crate::machines::mob_machine as mob_dsl;
use crate::run::FlowRunConfig;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Runtime default applied when the flow's `SupervisorSpec` declares no
/// `escalation_turn_timeout_ms`. The declared spec value — not this constant —
/// is the typed owner of the escalation deadline.
const DEFAULT_ESCALATION_TURN_TIMEOUT: Duration = Duration::from_secs(2);

#[cfg(test)]
static TEST_ESCALATION_TURN_TIMEOUT_MS: AtomicU64 = AtomicU64::new(0);

fn escalation_turn_timeout(declared_ms: Option<u64>) -> Duration {
    #[cfg(test)]
    {
        let override_ms = TEST_ESCALATION_TURN_TIMEOUT_MS.load(Ordering::Relaxed);
        if override_ms > 0 {
            return Duration::from_millis(override_ms);
        }
    }

    declared_ms.map_or(DEFAULT_ESCALATION_TURN_TIMEOUT, Duration::from_millis)
}

#[cfg(test)]
pub(crate) fn set_escalation_turn_timeout_for_tests(timeout: Duration) {
    TEST_ESCALATION_TURN_TIMEOUT_MS.store(timeout.as_millis() as u64, Ordering::Relaxed);
}

#[cfg(test)]
pub(crate) fn reset_escalation_turn_timeout_for_tests() {
    TEST_ESCALATION_TURN_TIMEOUT_MS.store(0, Ordering::Relaxed);
}

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
        let supervisor_spec = config
            .supervisor
            .as_ref()
            .ok_or_else(|| MobError::SupervisorEscalation("supervisor not configured".into()))?;
        let supervisor_role = supervisor_spec.role.clone();
        // The escalation deadline is declared flow policy (SupervisorSpec),
        // not a shell constant; the runtime default applies only when the
        // spec declares none.
        let declared_turn_timeout =
            escalation_turn_timeout(supervisor_spec.escalation_turn_timeout_ms);

        // Pure candidate projection: pick the first runnable member matching the
        // configured supervisor role. MobMachine — not the shell — owns whether
        // escalation proceeds or fails closed for "no eligible target".
        let escalation_target = self
            .handle
            .list_runnable_members()
            .await
            .into_iter()
            .find(|entry| entry.role == supervisor_role);

        let dsl_run_id = mob_dsl::RunId::from(run_id.to_string());
        let dsl_step_id = mob_dsl::StepId::from(step_id.as_str());

        let escalation_input = match &escalation_target {
            Some(target) => mob_dsl::MobMachineInput::EscalateToSupervisor {
                run_id: dsl_run_id,
                step_id: dsl_step_id,
                supervisor_identity: mob_dsl::AgentIdentity::from_domain(&target.agent_identity),
                turn_timeout_ms: declared_turn_timeout.as_millis() as u64,
            },
            None => mob_dsl::MobMachineInput::EscalateToSupervisorNoEligibleTarget {
                run_id: dsl_run_id,
                step_id: dsl_step_id,
            },
        };

        let effects = self
            .handle
            .apply_machine_input_effects(escalation_input)
            .await?;

        let mut requested = None;
        let mut failure_cause = None;
        for effect in effects {
            match effect {
                mob_dsl::MobMachineEffect::SupervisorEscalationRequested {
                    turn_timeout_ms,
                    ..
                } => {
                    if requested.replace(turn_timeout_ms).is_some() {
                        return Err(MobError::Internal(
                            "MobMachine emitted multiple supervisor escalation requests".into(),
                        ));
                    }
                }
                mob_dsl::MobMachineEffect::SupervisorEscalationFailed { cause, .. } => {
                    if failure_cause.replace(cause).is_some() {
                        return Err(MobError::Internal(
                            "MobMachine emitted multiple supervisor escalation failures".into(),
                        ));
                    }
                }
                _ => {}
            }
        }

        if let Some(cause) = failure_cause {
            return Err(match cause {
                mob_dsl::SupervisorEscalationFailureCause::NoEligibleSupervisor => {
                    MobError::SupervisorEscalation(format!(
                        "no active supervisor member for role '{supervisor_role}'"
                    ))
                }
                mob_dsl::SupervisorEscalationFailureCause::TimeoutExceeded => {
                    MobError::SupervisorEscalation(
                        "supervisor escalation classified TimeoutExceeded before dispatch".into(),
                    )
                }
            });
        }

        let turn_timeout_ms = requested.ok_or_else(|| {
            MobError::Internal(
                "MobMachine accepted supervisor escalation but emitted no request or failure"
                    .into(),
            )
        })?;
        let escalation_target = escalation_target.ok_or_else(|| {
            MobError::Internal(
                "MobMachine requested supervisor escalation without an eligible target".into(),
            )
        })?;
        let escalation_turn_timeout = Duration::from_millis(turn_timeout_ms);

        tokio::time::timeout(escalation_turn_timeout, async {
            self.handle
                .member(&escalation_target.agent_identity)
                .await?
                .internal_turn(format!(
                    "Supervisor escalation for run '{run_id}', step '{step_id}': {reason}"
                ))
                .await
        })
        .await
        .map_err(|_| {
            MobError::SupervisorEscalation(format!(
                "supervisor escalation timed out after {}ms",
                escalation_turn_timeout.as_millis()
            ))
        })??;

        self.emitter
            .supervisor_escalation(
                run_id.clone(),
                step_id.clone(),
                escalation_target.agent_identity.clone(),
            )
            .await?;

        Ok(())
    }

    pub async fn force_reset(&self) -> Result<(), MobError> {
        let identities: Vec<AgentIdentity> = self
            .handle
            .list_members()
            .await
            .into_iter()
            .map(|entry| entry.agent_identity)
            .collect();

        let mut failures = Vec::new();
        for identity in identities {
            if let Err(error) = self.handle.retire(identity.clone()).await {
                failures.push(format!("{identity}: {error}"));
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

#[cfg(test)]
mod timeout_policy_tests {
    use super::*;

    #[test]
    fn declared_spec_timeout_owns_the_escalation_deadline() {
        reset_escalation_turn_timeout_for_tests();
        // Declared flow policy wins over the runtime default.
        assert_eq!(
            escalation_turn_timeout(Some(7_500)),
            Duration::from_millis(7_500)
        );
        // Absent declaration falls back to the runtime default.
        assert_eq!(
            escalation_turn_timeout(None),
            DEFAULT_ESCALATION_TURN_TIMEOUT
        );
    }
}

use super::resume_topology::{ResumeTopologyOutcome, ResumeTopologyPendingEffect};
use super::*;

pub(in crate::runtime) struct RetainedResumeTopologyEffect {
    effect: Box<ResumeTopologyPendingEffect>,
    retry_tx: oneshot::Sender<()>,
}

impl MobActor {
    pub(super) fn resume_topology_mutation_pending(&self) -> bool {
        self.dsl_authority.state().explicit_resume_topology_pending
            || !self.resume_topology_effect_custody.is_empty()
    }

    pub(super) fn resume_topology_control_is_pending(&self, command: &MobCommand) -> bool {
        if !self.resume_topology_mutation_pending() {
            return false;
        }
        matches!(
            command,
            MobCommand::Spawn { .. }
                | MobCommand::SpawnProvisioned { .. }
                | MobCommand::SpawnAttachedForkedParticipant { .. }
                | MobCommand::Retire { .. }
                | MobCommand::Respawn { .. }
                | MobCommand::ReloadMemberRegistration { .. }
                | MobCommand::ReviveMemberLiveMaterialization { .. }
                | MobCommand::RevivePlacedMember { .. }
                | MobCommand::Wire { .. }
                | MobCommand::Unwire { .. }
                | MobCommand::WireMembersBatch { .. }
                | MobCommand::RotateSupervisor { .. }
                | MobCommand::BindHost { .. }
        )
    }

    fn owns_resume_topology_effect(&self, effect: &ResumeTopologyPendingEffect) -> bool {
        effect.attempt() == self.dsl_authority.state().explicit_resume_attempt.as_ref()
            && self
                .resume_topology_effect_custody
                .held_effects()
                .any(|held| {
                    held.attempt() == effect.attempt()
                        && held.agent_identity() == effect.agent_identity()
                        && held.generation() == effect.generation()
                        && held.fence_token() == effect.fence_token()
                        && held.incarnation() == effect.incarnation()
                        && held.kind() == effect.kind()
                })
    }

    pub(super) fn resume_topology_effect_held(
        &mut self,
        effect: Box<ResumeTopologyPendingEffect>,
        retry_tx: oneshot::Sender<()>,
        attempts: u32,
        automatic_retry: bool,
    ) {
        if !self.owns_resume_topology_effect(&effect) {
            tracing::error!(
                reason = %effect.uncertainty_reason(),
                "resume topology retry lost its exact effect custody"
            );
            self.durable_uncertainty_fail_stop = true;
            return;
        }
        tracing::warn!(
            reason = %effect.uncertainty_reason(),
            attempts,
            "resume topology retains an unsettled remote effect"
        );
        if attempts == 1 && automatic_retry {
            if retry_tx.send(()).is_err() {
                tracing::error!("resume topology effect owner disappeared before retry");
                self.durable_uncertainty_fail_stop = true;
            }
        } else {
            self.retained_resume_topology_effects
                .push(RetainedResumeTopologyEffect { effect, retry_tx });
        }
    }

    pub(super) fn retry_resume_topology_effects(&mut self) {
        for retained in std::mem::take(&mut self.retained_resume_topology_effects) {
            if !self.owns_resume_topology_effect(&retained.effect) {
                tracing::error!("lifecycle control cannot retry a foreign topology effect");
                self.durable_uncertainty_fail_stop = true;
                continue;
            }
            tracing::warn!(
                reason = %retained.effect.uncertainty_reason(),
                "lifecycle control retries the same retained topology effect"
            );
            if retained.retry_tx.send(()).is_err() {
                tracing::error!("retained resume topology effect owner is unavailable");
                self.durable_uncertainty_fail_stop = true;
            }
        }
    }

    pub(super) async fn resume_topology_completed(
        &mut self,
        attempt: mob_dsl::ResumeAttemptId,
        outcome: ResumeTopologyOutcome,
    ) {
        if self.dsl_authority.state().explicit_resume_attempt.as_ref() != Some(&attempt)
            || !self.dsl_authority.state().explicit_resume_topology_pending
        {
            tracing::warn!("ignoring stale resume topology completion");
            return;
        }
        match outcome {
            ResumeTopologyOutcome::Settled(result) => {
                if !self.resume_topology_effect_custody.is_empty()
                    || !self.retained_resume_topology_effects.is_empty()
                {
                    tracing::error!("topology owner reported settlement with retained effects");
                    self.durable_uncertainty_fail_stop = true;
                    return;
                }
                self.resume_topology_settled(attempt, result).await;
            }
            ResumeTopologyOutcome::Unsettled(effect) => {
                tracing::error!(
                    reason = %effect.uncertainty_reason(),
                    "resume topology ended without proving remote effect settlement"
                );
                if !self.owns_resume_topology_effect(&effect) {
                    self.durable_uncertainty_fail_stop = true;
                    return;
                }
                if let Some(pending) = self.pending_resume_lifecycle.take() {
                    let _ = pending
                        .reply_tx
                        .send(Err(MobError::ExternalMemberCleanupUncertain {
                            reason: effect.uncertainty_reason(),
                        }));
                }
                // The generated topology barrier and exact incarnation ledger
                // remain intact: an ended observer is not remote terminality.
            }
            ResumeTopologyOutcome::OwnerLost(reason) => {
                tracing::error!(%reason, "resume topology owner disappeared without settlement");
                self.durable_uncertainty_fail_stop = true;
            }
        }
    }
}

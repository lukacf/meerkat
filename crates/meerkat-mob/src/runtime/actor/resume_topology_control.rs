use super::resume_topology::{ResumeTopologyOutcome, ResumeTopologyPendingEffect};
use super::*;

pub(in crate::runtime) struct RetainedResumeTopologyEffect {
    effect: Box<ResumeTopologyPendingEffect>,
    retry_tx: oneshot::Sender<()>,
}

impl MobActor {
    /// Whether the explicit resume owns its topology reconciliation: MobMachine
    /// holds its topology pending, or an effect of that reconciliation is
    /// still in custody. The graph fence: nothing else mutates the graph
    /// meanwhile.
    pub(super) fn resume_topology_mutation_pending(&self) -> bool {
        self.dsl_authority.state().explicit_resume_topology_pending
            || !self.resume_topology_effect_custody.is_empty()
    }

    /// Whether the explicit resume is past its topology reconciliation and
    /// still owns its post-commit work (the members' operation-binding
    /// restore, and an AutonomousHost orchestrator's queued resume
    /// notification): MobMachine holds the attempt open with its topology
    /// settled.
    pub(super) fn explicit_resume_post_commit_pending(&self) -> bool {
        let state = self.dsl_authority.state();
        state.explicit_resume_attempt.is_some() && state.explicit_resume_topology_settled
    }

    /// The explicit resume's fence for host repair and lifecycle work, which
    /// waits for its whole member work: the topology reconciliation and the
    /// post-commit work after it.
    pub(super) fn explicit_resume_member_work_fenced(&self) -> bool {
        self.resume_topology_mutation_pending() || self.explicit_resume_post_commit_pending()
    }

    pub(super) fn resume_topology_control_is_pending(&self, command: &MobCommand) -> bool {
        match command {
            // Graph work a member's run can wait on (its spawn, wire or
            // retire tool call): it waits only for the topology
            // reconciliation. The post-commit work waits on no member run (a
            // member whose registration already serves its binding is bound
            // without the lanes a run holds, and a notification that runs the
            // orchestrator's turn is handed to the lifecycle owner), so this
            // early fence drop is defence in depth: were post-commit work ever
            // to wait on a run again, deferring this work behind it would let
            // a run that spawns, wires or retires hold the resume, and the
            // resume hold the run, forever. A member whose own post-commit
            // work is in flight still keeps its retire and respawn behind that
            // work (see `resume_member_control_is_pending`).
            MobCommand::Spawn { .. }
            | MobCommand::SpawnProvisioned { .. }
            | MobCommand::SpawnAttachedForkedParticipant { .. }
            | MobCommand::Retire { .. }
            | MobCommand::Respawn { .. }
            | MobCommand::Wire { .. }
            | MobCommand::Unwire { .. }
            | MobCommand::WireMembersBatch { .. } => self.resume_topology_mutation_pending(),
            // Host repair and supervisor work waits for the resume's whole
            // member work, as before.
            MobCommand::ReloadMemberRegistration { .. }
            | MobCommand::ReviveMemberLiveMaterialization { .. }
            | MobCommand::ReleaseMember { .. }
            | MobCommand::RevivePlacedMember { .. }
            | MobCommand::RotateSupervisor { .. }
            | MobCommand::BindHost { .. } => self.explicit_resume_member_work_fenced(),
            _ => false,
        }
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
            self.durable_uncertainty_fail_stop
                .record_unreplied(MobError::Internal(
                    "resume topology retry lost its exact effect custody".to_string(),
                ));
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
                self.durable_uncertainty_fail_stop
                    .record_unreplied(MobError::Internal(
                        "resume topology effect owner disappeared before retry".to_string(),
                    ));
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
                self.durable_uncertainty_fail_stop
                    .record_unreplied(MobError::Internal(
                        "lifecycle control cannot retry a foreign topology effect".to_string(),
                    ));
                continue;
            }
            tracing::warn!(
                reason = %retained.effect.uncertainty_reason(),
                "lifecycle control retries the same retained topology effect"
            );
            if retained.retry_tx.send(()).is_err() {
                tracing::error!("retained resume topology effect owner is unavailable");
                self.durable_uncertainty_fail_stop
                    .record_unreplied(MobError::Internal(
                        "retained resume topology effect owner is unavailable".to_string(),
                    ));
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
                    self.durable_uncertainty_fail_stop
                        .record_unreplied(MobError::Internal(
                            "topology owner reported settlement with retained effects".to_string(),
                        ));
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
                    self.durable_uncertainty_fail_stop.record_unreplied(
                        MobError::ExternalMemberCleanupUncertain {
                            reason: effect.uncertainty_reason(),
                        },
                    );
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
                self.durable_uncertainty_fail_stop
                    .record_unreplied(MobError::Internal(
                        "resume topology owner disappeared without settlement".to_string(),
                    ));
            }
        }
    }
}

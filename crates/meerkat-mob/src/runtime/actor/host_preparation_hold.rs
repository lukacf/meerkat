//! The run-start hold of a member whose role move MobMachine holds (its
//! host preparation cause `RoleMigrationUndeclared`), realized from the
//! machine state at every actor loop boundary.
//!
//! MobMachine refuses every wake of a held member, but a Live member serves
//! its deliveries without a wake. So the actor holds the run starts of each
//! held member's session with the typed reason
//! [`meerkat_runtime::RunStartHoldReason::HostPreparation`], staged, so that
//! a registration of the session applies it too
//! ([`crate::MobHandle::members_holding_run_starts`] lists the member under
//! [`crate::HostRunStartHoldReason::HostPreparation`]). The run in flight, if
//! any, finishes and is never preempted; no new run starts under the
//! member's previous role. Once that run has settled, or at once when none
//! was open, the actor releases the member through MobMachine's release
//! verdict (`ClassifyMemberRelease`), which leaves it Dormant, and the role
//! hold refuses its next wake. When MobMachine ends the hold (an apply that
//! restores the role, the step that commits the new role, a retirement, the
//! destroy, the mob's reset), the actor releases the run-start hold.
//!
//! Level-triggered: the held sessions are derived from the machine state each
//! time and are never journaled. After a restart no member is Live, and the
//! first apply holds a member again.

use super::*;

/// The session of every seated local member whose role move MobMachine
/// holds, by identity.
fn role_held_member_sessions(
    state: &mob_dsl::MobMachineState,
) -> BTreeMap<AgentIdentity, SessionId> {
    state
        .member_host_preparation_causes
        .iter()
        .filter(|(_, cause)| **cause == mob_dsl::HostPreparationCause::RoleMigrationUndeclared)
        .filter(|(identity, _)| {
            state.identity_to_runtime.contains_key(*identity)
                && !state.member_placement.contains_key(*identity)
        })
        .filter_map(|(identity, _)| {
            let session = state.member_session_bindings.get(identity)?;
            SessionId::parse(&session.0)
                .ok()
                .map(|session| (AgentIdentity::from(identity.0.as_str()), session))
        })
        .collect()
}

impl MobActor {
    /// Reconcile the members' `HostPreparation` run-start holds with the
    /// machine state: release the hold of each member MobMachine no longer
    /// holds (or whose session changed), and hold the session of each member
    /// it now holds, releasing a Live one once its run in flight settled.
    pub(super) async fn realize_host_preparation_run_start_holds(&mut self) {
        let held = {
            let state = self.dsl_authority.state();
            if self.host_preparation_holds.is_empty()
                && !state
                    .member_host_preparation_causes
                    .values()
                    .any(|cause| *cause == mob_dsl::HostPreparationCause::RoleMigrationUndeclared)
            {
                return;
            }
            role_held_member_sessions(state)
        };

        let ended: Vec<(AgentIdentity, SessionId)> = self
            .host_preparation_holds
            .iter()
            .filter(|(identity, session)| held.get(*identity) != Some(*session))
            .map(|(identity, session)| (identity.clone(), session.clone()))
            .collect();
        for (identity, session_id) in ended {
            self.host_preparation_holds.remove(&identity);
            if let Err(error) = self
                .provisioner
                .release_member_run_starts(
                    &MemberRef::from_bridge_session_id(session_id),
                    None,
                    meerkat_runtime::RunStartHoldReason::HostPreparation,
                )
                .await
            {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    agent_identity = %identity,
                    %error,
                    "releasing the run-start hold of a member whose role hold ended failed"
                );
                continue;
            }
            // A kickoff a Shutdown kept for the member waited for this
            // release, as it waits for a host's.
            if self.roster.read().await.get(&identity).is_some() {
                Box::pin(self.rearm_kept_kickoff_after_release(&identity)).await;
            }
        }

        for (identity, session_id) in held {
            if self.host_preparation_holds.get(&identity) == Some(&session_id) {
                continue;
            }
            if let Err(error) = self
                .provisioner
                .stage_member_registration_run_start_hold(
                    &MemberRef::from_bridge_session_id(session_id.clone()),
                    meerkat_runtime::RunStartHoldReason::HostPreparation,
                )
                .await
            {
                // The next loop boundary tries again.
                tracing::warn!(
                    mob_id = %self.definition.id,
                    agent_identity = %identity,
                    %session_id,
                    %error,
                    "holding the run starts of a member whose role move is held failed"
                );
                continue;
            }
            let live = self
                .dsl_authority
                .state()
                .member_materialization
                .get(&mob_dsl::AgentIdentity::from_domain(&identity))
                == Some(&mob_dsl::MemberMaterializationPhase::Live);
            let adapter = if live {
                self.runtime_adapter.clone()
            } else {
                None
            };
            let Some(adapter) = adapter else {
                self.host_preparation_holds.insert(identity, session_id);
                continue;
            };
            self.host_preparation_holds
                .insert(identity.clone(), session_id.clone());
            // Release the member once its run in flight settled: the release
            // is MobMachine's verdict, taken while the hold still stands on
            // this session (`ReleaseHeldMember`).
            let current_run = adapter.current_run(&session_id).await;
            let command_tx = self.command_tx.clone();
            self.actor_io_tasks.spawn(async move {
                if let Some(run_id) = current_run {
                    adapter.wait_run_settled(&session_id, &run_id).await;
                }
                let _ = command_tx
                    .send(RoutedMobCommand::internal(MobCommand::ReleaseHeldMember {
                        agent_identity: identity,
                        session_id,
                    }))
                    .await;
            });
        }
    }

    /// Release a held member whose run in flight settled
    /// (`ReleaseHeldMember`): only while its role hold still stands on the
    /// session the hold was taken on. The release is MobMachine's verdict
    /// (`ClassifyMemberRelease`); one refused for work still in flight leaves
    /// the member Live with its run starts held, and the next apply that
    /// holds it tries again.
    pub(super) async fn release_held_member(
        &mut self,
        agent_identity: AgentIdentity,
        session_id: SessionId,
    ) {
        if self.host_preparation_holds.get(&agent_identity) != Some(&session_id) {
            return;
        }
        let (reply_tx, _reply_rx) = oneshot::channel();
        Box::pin(self.handle_release_member(agent_identity, None, reply_tx)).await;
    }
}

//! Identity entries on the actor: the agent lane's refusal of member commands
//! addressed to an entry, and the adoption of a legacy member as an entry.
//!
//! MobMachine decides both (`ResolveAgentLaneMemberAdmission`,
//! `AdoptLegacyIdentityEntry`); the actor realizes the verdicts. See
//! `runtime::identity_entry` for the entry concept and its reads.

use super::*;

impl MobActor {
    /// The agent lane's admission of a member command
    /// ([`MobCommand::agent_lane_member_targets`]): MobMachine answers for
    /// each member the command addresses (`ResolveAgentLaneMemberAdmission`,
    /// a pure verdict in every lifecycle phase), and the first identity entry
    /// among them refuses the command with [`MobError::IdentityMember`].
    /// `Ok(None)` admits it; its own arms decide the rest.
    pub(super) fn agent_lane_identity_member_refusal(
        &self,
        cmd: &MobCommand,
    ) -> Result<Option<MobError>, MobError> {
        for agent_identity in cmd.agent_lane_member_targets() {
            let dsl_identity = mob_dsl::AgentIdentity::from_domain(agent_identity);
            let prepared = self.prepare_dsl_input(
                mob_dsl::MobMachineInput::ResolveAgentLaneMemberAdmission {
                    agent_identity: dsl_identity.clone(),
                },
                "agent_lane_member_admission",
            )?;
            let admission = prepared
                .effects
                .iter()
                .find_map(|effect| match effect {
                    mob_dsl::MobMachineEffect::AgentLaneMemberAdmissionResolved {
                        agent_identity,
                        admission,
                    } if agent_identity == &dsl_identity => Some(*admission),
                    _ => None,
                })
                .ok_or_else(|| {
                    MobError::Internal(format!(
                        "MobMachine answered no agent-lane admission for member '{agent_identity}'"
                    ))
                })?;
            match admission {
                mob_dsl::AgentLaneMemberAdmissionKind::Admitted => {}
                mob_dsl::AgentLaneMemberAdmissionKind::IdentityMember => {
                    return Ok(Some(MobError::IdentityMember {
                        member_id: agent_identity.clone(),
                    }));
                }
            }
        }
        Ok(None)
    }

    /// The adoption of one legacy member as an identity entry
    /// (`AdoptLegacyIdentityEntry`), decided by MobMachine from the handle's
    /// observation of the spawn row of the member's incarnation at
    /// `generation`. An `Adopted` verdict journals the adoption's row (the
    /// member's current binding, unchanged, with the entry's origin) before
    /// it commits; every other verdict changes nothing and is answered as
    /// decided. A mob that is not running answers an invalid transition.
    pub(super) async fn adopt_legacy_identity_entry(
        &mut self,
        agent_identity: AgentIdentity,
        generation: crate::ids::Generation,
        legacy_roster_member: bool,
    ) -> Result<mob_dsl::LegacyIdentityEntryAdoptionKind, MobError> {
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(&agent_identity);
        let prepared = self.prepare_dsl_input_transition(
            mob_dsl::MobMachineInput::AdoptLegacyIdentityEntry {
                agent_identity: dsl_identity.clone(),
                generation: mob_dsl::Generation::from_domain(generation),
                legacy_roster_member,
            },
            "adopt_legacy_identity_entry",
        )?;
        let verdict = prepared
            .transition
            .effects()
            .iter()
            .find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::LegacyIdentityEntryAdoptionResolved {
                    agent_identity,
                    verdict,
                } if agent_identity == &dsl_identity => Some(*verdict),
                _ => None,
            })
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "MobMachine answered no adoption verdict for member '{agent_identity}'"
                ))
            })?;
        match verdict {
            mob_dsl::LegacyIdentityEntryAdoptionKind::Adopted => {}
            mob_dsl::LegacyIdentityEntryAdoptionKind::MobNotRunning => {
                return Err(self.invalid_transition_to(MobState::Running));
            }
            // Every other verdict changes nothing: there is nothing to
            // journal or commit.
            other => return Ok(other),
        }
        let row = legacy_adoption_row(prepared.authority.state(), &agent_identity)?;
        let events = Arc::clone(&self.events);
        let mob_id = self.definition.id.clone();
        let stored = self
            .commit_prepared_dsl_transition_after(prepared, move || async move {
                events
                    .append(NewMobEvent {
                        mob_id,
                        timestamp: None,
                        kind: MobEventKind::MemberSessionBindingRecovered(row),
                    })
                    .await
                    .map_err(MobError::from)
            })
            .await?;
        // The roster takes the row as a replay does; it moves no binding.
        self.roster.write().await.apply_event(&stored);
        Ok(verdict)
    }
}

/// The adoption's row, from the state the adoption's own step left: the
/// member's current incarnation and session binding, unchanged, with the
/// origin MobMachine recorded for the entry. It carries no endpoint: the
/// replay of a binding row without one records nothing about the member's
/// endpoint, so the row moves nothing but the entry.
fn legacy_adoption_row(
    state: &mob_dsl::MobMachineState,
    agent_identity: &AgentIdentity,
) -> Result<crate::event::MemberSessionBindingRecoveredEvent, MobError> {
    let dsl_identity = mob_dsl::AgentIdentity::from_domain(agent_identity);
    let missing = |fact: &str| {
        MobError::Internal(format!(
            "the adoption of '{agent_identity}' as an identity entry left no {fact}"
        ))
    };
    let generation = state
        .identity_runtime_generations
        .get(&dsl_identity)
        .ok_or_else(|| missing("generation"))?;
    let session = state
        .member_session_bindings
        .get(&dsl_identity)
        .ok_or_else(|| missing("session binding"))?;
    let session_id = SessionId::parse(&session.0).map_err(|error| {
        MobError::Internal(format!(
            "'{agent_identity}' holds an invalid session binding: {error}"
        ))
    })?;
    let origin = super::super::identity_entry::machine_entry_origin(state, agent_identity)
        .ok_or_else(|| missing("entry"))?;
    Ok(
        crate::event::MemberSessionBindingRecoveredEvent::identity_entry_adopted(
            agent_identity.clone(),
            AgentRuntimeId::new(
                agent_identity.clone(),
                crate::ids::Generation::new(generation.0),
            ),
            session_id,
            origin,
        ),
    )
}

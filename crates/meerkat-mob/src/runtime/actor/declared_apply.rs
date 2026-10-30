//! The declared apply on the actor
//! ([`super::super::MobHandle::apply_declared_roster`]): one MobMachine step
//! decides a chunk of declared identities (`DeclareIdentityEntry`), and one
//! seals a complete roster (`SealDeclaredRoster`).
//!
//! The verdicts write no journal row: what they record (an unseated entry and
//! its cause, a role hold) is declared again by the next apply, and the seal's
//! mark is recorded on the start row of the retirement it causes. The handle
//! realizes each verdict with the mob's existing steps. See
//! `runtime::declared_apply` for the apply and its answers.

use super::*;
use crate::runtime::declared_apply::{
    DecidedDeclaration, DeclarationVerdict, IdentityDeclaration, SealAnswer,
};

impl MobActor {
    /// Decide one chunk of declarations in one MobMachine step: each
    /// identity's verdict (`DeclareIdentityEntry`), all applied on one
    /// prepared authority and committed once. A declaration no arm takes,
    /// or whose role cannot be resolved, is answered for that identity
    /// alone. A mob that is not running answers an invalid transition.
    pub(super) async fn declare_identity_entries(
        &mut self,
        declarations: Vec<IdentityDeclaration>,
    ) -> Result<Vec<DeclarationVerdict>, MobError> {
        // A spec that names no runtime mode declares its role's: each role
        // is resolved once per chunk.
        let mut role_modes: BTreeMap<ProfileName, crate::MobRuntimeMode> = BTreeMap::new();
        let mut resolved = Vec::with_capacity(declarations.len());
        for declaration in declarations {
            let runtime_mode = match declaration.runtime_mode {
                Some(runtime_mode) => Ok(runtime_mode),
                None => match role_modes.get(&declaration.profile_name) {
                    Some(runtime_mode) => Ok(*runtime_mode),
                    None => self
                        .definition
                        .resolve_profile(
                            &declaration.profile_name,
                            self.realm_profile_store.as_ref(),
                        )
                        .await
                        .map(|profile| {
                            role_modes
                                .insert(declaration.profile_name.clone(), profile.runtime_mode);
                            profile.runtime_mode
                        }),
                },
            };
            resolved.push((declaration, runtime_mode));
        }

        let mut authority = self.dsl_authority.prepare_authority();
        let mut effects = Vec::new();
        let mut verdicts = Vec::with_capacity(resolved.len());
        for (declaration, runtime_mode) in resolved {
            let agent_identity = declaration.agent_identity.clone();
            let runtime_mode = match runtime_mode {
                Ok(runtime_mode) => runtime_mode,
                Err(error) => {
                    verdicts.push(DeclarationVerdict {
                        agent_identity,
                        verdict: Err(error),
                    });
                    continue;
                }
            };
            let dsl_identity = mob_dsl::AgentIdentity::from_domain(&agent_identity);
            // A rejected input leaves the prepared authority as it was.
            let transition = match mob_dsl::MobMachineMutator::apply(
                &mut authority,
                declaration.machine_input(runtime_mode),
            ) {
                Ok(transition) => transition,
                Err(error) => {
                    verdicts.push(DeclarationVerdict {
                        verdict: Err(MobError::MobMachineRejected {
                            context: "declared_apply",
                            reason: format!("'{agent_identity}': {error}"),
                        }),
                        agent_identity,
                    });
                    continue;
                }
            };
            let decided = transition.effects().iter().find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::IdentityEntryDeclared {
                    agent_identity,
                    verdict,
                    archived_session,
                } if agent_identity == &dsl_identity => Some((*verdict, archived_session.clone())),
                _ => None,
            });
            let Some((verdict, archived_session)) = decided else {
                return Err(MobError::Internal(format!(
                    "MobMachine answered no declared apply verdict for '{agent_identity}'"
                )));
            };
            if verdict == mob_dsl::IdentityEntryVerdict::MobNotRunning {
                return Err(self.invalid_transition_to(MobState::Running));
            }
            let archived_session = archived_session
                .map(|session| SessionId::parse(&session.0))
                .transpose()
                .map_err(|error| {
                    MobError::Internal(format!(
                        "'{agent_identity}' holds an invalid archived session record: {error}"
                    ))
                })?;
            effects.extend(transition.into_effects());
            verdicts.push(DeclarationVerdict {
                agent_identity,
                verdict: Ok(DecidedDeclaration {
                    verdict,
                    runtime_mode,
                    archived_session,
                }),
            });
        }
        self.commit_prepared_dsl_input(PreparedDslInput {
            authority,
            effects,
            phase_changed: false,
        })?;
        Ok(verdicts)
    }

    /// Seal a complete declared roster (`SealDeclaredRoster`) in one
    /// MobMachine step, and answer the Declared entries it marked (whose
    /// members the handle retires) and the ones it removed at once. A mob
    /// that is not running answers an invalid transition.
    pub(super) fn seal_declared_roster(
        &mut self,
        desired: BTreeSet<AgentIdentity>,
    ) -> Result<SealAnswer, MobError> {
        let before = self.dsl_authority.state().identity_entry_origins.clone();
        let prepared = self.prepare_dsl_input_transition(
            mob_dsl::MobMachineInput::SealDeclaredRoster {
                desired: desired
                    .iter()
                    .map(mob_dsl::AgentIdentity::from_domain)
                    .collect(),
            },
            "seal_declared_roster",
        )?;
        let verdict = prepared
            .transition
            .effects()
            .iter()
            .find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::DeclaredRosterSealed { verdict } => Some(*verdict),
                _ => None,
            })
            .ok_or_else(|| MobError::Internal("MobMachine answered no seal verdict".to_string()))?;
        if verdict == mob_dsl::DeclaredRosterSealVerdict::MobNotRunning {
            return Err(self.invalid_transition_to(MobState::Running));
        }
        let state = prepared.authority.state();
        let identity =
            |dsl_identity: &mob_dsl::AgentIdentity| AgentIdentity::from(dsl_identity.0.as_str());
        // The marked members whose retirement has not started: the handle
        // retires each, and its start row records the cause.
        let marked = state
            .identity_entry_retiring
            .iter()
            .filter(|dsl_identity| {
                state
                    .identity_to_runtime
                    .get(*dsl_identity)
                    .is_some_and(|runtime| {
                        state.member_state_markers.get(runtime)
                            != Some(&mob_dsl::MobMemberState::Retiring)
                    })
            })
            .map(identity)
            .collect();
        let removed = before
            .keys()
            .filter(|dsl_identity| !state.identity_entry_origins.contains_key(*dsl_identity))
            .map(identity)
            .collect();
        let phase_changed = prepared.transition.from_phase != prepared.transition.to_phase;
        self.commit_prepared_dsl_input(PreparedDslInput {
            authority: prepared.authority,
            effects: prepared.transition.into_effects(),
            phase_changed,
        })?;
        Ok(SealAnswer {
            verdict,
            marked,
            removed,
        })
    }
}

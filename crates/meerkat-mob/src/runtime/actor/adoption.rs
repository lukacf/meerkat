//! Adoption into store custody on the actor (0.9.11; see
//! [`super::super::adoption`]): the change's one commit step.

use super::super::adoption::{AdoptionCommit, AdoptionCommitted, AdoptionSource, adoption_refusal};
use super::super::identity_change::{
    IdentityChangeRefusal, IdentityGrantRequest, NotAdoptableReason, committed_change_events,
    grant_covers_the_commit, now_ms, refused,
};
use super::identity_change::ChangeAdmission;
use super::*;
use crate::identity::{
    CreatedIdentityPresence, CreatedIdentityProvenance, CreatedIdentityRecord, IdentityAdoptedFrom,
    IdentityChangeEntryOutcome, IdentityChangeKind, IdentityChangeReceipt,
    IdentityChangeReceiptEntry, IdentityEntered, OwnedEdgeAdmission, OwnedIdentityEdge,
};
use crate::machines::mob_machine::MobMachineMutator as _;
use crate::store::{CreatedIdentityRowExpectation, CreatedIdentityRowWrite};

impl MobActor {
    /// Commit one adoption into store custody in one actor step: MobMachine
    /// decides the change's admission and the adoption's verdict
    /// (`AdoptIdentityIntoStore`) on one prepared authority, then each of
    /// the member's current edges is declared the adopted identity's owned
    /// edge (`DeclareOwnedIdentityEdge`): an edge to another identity
    /// becomes owned, recorded `adopted`; an edge another identity owns, or
    /// to a member that is no identity, stays plain wiring. The change's
    /// store transaction writes the identity's row, the member's binding row
    /// with the Created origin (a seated member's) and the receipt, and only
    /// then does the machine commit; the roster folds the binding row as a
    /// replay does, which clears the member's spawner. A refusal anywhere
    /// writes nothing.
    pub(super) async fn commit_identity_adoption(
        &mut self,
        commit: AdoptionCommit,
    ) -> Result<AdoptionCommitted, MobError> {
        let store = self
            .identity_member
            .clone()
            .ok_or_else(|| refused(IdentityChangeRefusal::StorageUnsupported))?;
        let mut authority = self.dsl_authority.prepare_authority();
        match self
            .decide_change_admission(
                &mut authority,
                &commit.operation,
                commit.facts,
                commit.grant.as_ref(),
                commit.decided,
                None,
            )
            .await?
        {
            ChangeAdmission::Replayed(receipt) => return Ok(AdoptionCommitted::Replayed(receipt)),
            ChangeAdmission::Admitted => {}
        }
        let agent_identity = mob_dsl::AgentIdentity::from_domain(&commit.identity);
        let transition = authority
            .apply(mob_dsl::MobMachineInput::AdoptIdentityIntoStore {
                agent_identity: agent_identity.clone(),
                source: commit.source.machine_source(),
                generation: commit
                    .source
                    .observed_generation()
                    .map(mob_dsl::Generation::from_domain),
            })
            .map_err(|error| MobError::MobMachineRejected {
                context: "adopt_identity_into_store",
                reason: error.to_string(),
            })?;
        let (verdict, reason) = transition
            .effects()
            .iter()
            .find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::IdentityAdoptionResolved {
                    verdict, reason, ..
                } => Some((*verdict, *reason)),
                _ => None,
            })
            .ok_or_else(|| {
                MobError::Internal("MobMachine decided an adoption with no verdict".into())
            })?;
        if let Some(refusal) = adoption_refusal(verdict, reason, &commit.identity)? {
            return Err(refused(refusal));
        }
        let mut effects = transition.into_effects();
        // The member's facts, which the adoption keeps: its roster entry (a
        // seated member's), its role and its session.
        let entry = self.roster.read().await.get(&commit.identity).cloned();
        let seated_role = authority
            .state()
            .member_profile_names
            .get(&agent_identity)
            .map(|profile| crate::ProfileName::from(profile.as_str()))
            .filter(|_| {
                authority
                    .state()
                    .identity_to_runtime
                    .contains_key(&agent_identity)
            });
        let (spec, from) = match commit.source {
            AdoptionSource::Roster { spec } => {
                if let Some(role) = &seated_role
                    && spec.profile != *role
                {
                    return Err(refused(IdentityChangeRefusal::InvalidRequest {
                        problem: format!(
                            "the roster spec of '{}' names the role '{}', and its member holds \
                             '{role}': apply the roster first",
                            commit.identity, spec.profile
                        ),
                    }));
                }
                (*spec, IdentityAdoptedFrom::Roster)
            }
            AdoptionSource::Worker {
                display_name,
                additional_instructions,
                ..
            } => {
                let Some(entry) = entry.as_ref() else {
                    return Err(MobError::Internal(format!(
                        "MobMachine adopted the worker '{}', which the roster does not hold",
                        commit.identity
                    )));
                };
                if entry.effective_profile_override.is_some()
                    || entry.effective_model_override.is_some()
                {
                    return Err(refused(IdentityChangeRefusal::NotAdoptable {
                        identity: commit.identity.clone(),
                        reason: NotAdoptableReason::ProfileOverride,
                    }));
                }
                let mut spec = crate::identity::CreatedIdentitySpec::new(
                    commit.identity.clone(),
                    entry.role.clone(),
                )
                .with_labels(entry.labels.clone())
                .with_runtime_mode_override(entry.runtime_mode)
                .with_additional_instructions(additional_instructions);
                if let Some(display_name) = display_name {
                    spec = spec.with_display_name(display_name);
                }
                (spec, IdentityAdoptedFrom::Worker)
            }
        };
        spec.validate().map_err(|error| {
            refused(IdentityChangeRefusal::InvalidRequest {
                problem: error.to_string(),
            })
        })?;
        // An agent requester's grant covers the identity's role.
        grant_covers_the_commit(
            commit.grant.as_ref(),
            Some(&IdentityGrantRequest::for_identity(spec.profile.clone())),
        )?;
        // The member's current edges, declared the adopted identity's owned
        // edges on the same authority: MobMachine owns an edge to another
        // identity to it, and leaves an edge another identity owns, or to a
        // member that is no identity, plain wiring.
        let peers: Vec<mob_dsl::AgentIdentity> = authority
            .state()
            .wiring_edges
            .iter()
            .filter(|edge| edge.a == agent_identity || edge.b == agent_identity)
            .map(|edge| {
                if edge.a == agent_identity {
                    edge.b.clone()
                } else {
                    edge.a.clone()
                }
            })
            .collect();
        let mut owned_edges = Vec::new();
        let mut plain_edges = Vec::new();
        for peer in peers {
            let peer_identity = AgentIdentity::from(peer.0.as_str());
            let declared = authority
                .apply(mob_dsl::MobMachineInput::DeclareOwnedIdentityEdge {
                    edge: mob_dsl::WiringEdge::new(agent_identity.clone(), peer),
                    owner: agent_identity.clone(),
                })
                .map_err(|error| MobError::MobMachineRejected {
                    context: "declare_owned_identity_edge",
                    reason: error.to_string(),
                })?;
            let verdict = declared
                .effects()
                .iter()
                .find_map(|effect| match effect {
                    mob_dsl::MobMachineEffect::OwnedIdentityEdgeDeclared { verdict, .. } => {
                        Some(*verdict)
                    }
                    _ => None,
                })
                .ok_or_else(|| {
                    MobError::Internal("MobMachine declared an owned edge with no verdict".into())
                })?;
            match verdict {
                mob_dsl::OwnedEdgeDeclareVerdict::Declared => owned_edges.push(peer_identity),
                mob_dsl::OwnedEdgeDeclareVerdict::AlreadyOwned
                | mob_dsl::OwnedEdgeDeclareVerdict::PeerUnknown => plain_edges.push(peer_identity),
                mob_dsl::OwnedEdgeDeclareVerdict::OwnerInvalid
                | mob_dsl::OwnedEdgeDeclareVerdict::MobNotRunning => {
                    return Err(MobError::Internal(format!(
                        "MobMachine adopted '{}' and refused it its own edge to \
                         '{peer_identity}' ({verdict:?})",
                        commit.identity
                    )));
                }
            }
            effects.extend(declared.into_effects());
        }
        let at_ms = now_ms();
        let former_spawner = entry.as_ref().and_then(|entry| entry.spawned_by.clone());
        let provenance = CreatedIdentityProvenance::entered(
            IdentityEntered::Adopted {
                from,
                former_spawner,
            },
            commit.operation.operation_id.clone(),
            at_ms,
            commit.requester.clone(),
            commit.consent.clone(),
        );
        let invalid = |error: crate::identity::IdentityIntentError| {
            MobError::Internal(format!("an adopted identity's row: {error}"))
        };
        let spec_digest = spec.spec_digest().map_err(invalid)?;
        let record = CreatedIdentityRecord::with_spec(
            self.definition.id.clone(),
            spec,
            1,
            CreatedIdentityPresence::Present,
            provenance,
        )
        .and_then(|record| {
            record.with_owned_edges(
                owned_edges
                    .iter()
                    .map(|peer| OwnedIdentityEdge::new(peer.clone(), OwnedEdgeAdmission::Adopted))
                    .collect(),
            )
        })
        .map_err(invalid)?;
        // The binding row of a seated member: its current incarnation and
        // session, unchanged, with the Created origin. An unseated roster
        // entry owed its seat has none: its seat binds it.
        let events = if authority
            .state()
            .identity_to_runtime
            .contains_key(&agent_identity)
        {
            let row = super::identity_entry_admission::legacy_adoption_row(
                authority.state(),
                &commit.identity,
            )?;
            vec![NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MemberSessionBindingRecovered(row),
            }]
        } else {
            Vec::new()
        };
        let receipt = IdentityChangeReceipt::sealed(
            self.definition.id.clone(),
            commit.operation.operation_id.clone(),
            commit.operation.request_digest.clone(),
            IdentityChangeKind::Adopt,
            vec![
                IdentityChangeReceiptEntry::new(
                    commit.identity.clone(),
                    IdentityChangeEntryOutcome::Committed,
                )
                .with_spec(spec_digest, Some(1))
                .with_adopted_edges(owned_edges, plain_edges),
            ],
            at_ms,
        )
        .and_then(|receipt| {
            receipt.with_attribution(commit.requester.clone(), commit.consent.clone())
        })
        .map_err(|error| MobError::Internal(format!("an adoption's receipt: {error}")))?;
        let prepared = PreparedDslInput {
            authority,
            effects,
            phase_changed: false,
        };
        let written = receipt.clone();
        let stored = self
            .commit_prepared_dsl_input_after(prepared, move || async move {
                let commit = store
                    .commit_identity_change(&crate::store::IdentityChangeWrite {
                        receipt: written,
                        rows: vec![CreatedIdentityRowWrite {
                            record,
                            expected: CreatedIdentityRowExpectation::Absent,
                        }],
                        events,
                    })
                    .await
                    .map_err(MobError::from)?;
                committed_change_events(commit)
            })
            .await?;
        // The roster folds the binding row as a replay does: the member keeps
        // its binding, and its spawner is cleared.
        {
            let mut roster = self.roster.write().await;
            for event in &stored {
                roster.apply_event(event);
            }
        }
        Ok(AdoptionCommitted::Committed(receipt))
    }
}

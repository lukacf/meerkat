//! Owned edges on the actor (0.9.11; see [`super::super::owned_edges`]): the
//! declaration and the end of the edges an identity change lists, decided on
//! the change step's prepared authority; the end of an edge the host's
//! policy reload denies; a plain unwire's admission; and the realization of
//! the owned edges in force whose ends are both seated, each read from
//! MobMachine's verdict before its wire.

use super::*;
use crate::identity::{CreatedIdentityObservation, OwnedIdentityEdge};
use crate::runtime::identity_change::{IdentityChangeRefusal, refused};
use crate::runtime::owned_edges::{OwnedEdgeEndOutcome, owned_edge_candidates};
use crate::store::{CreatedIdentityRowExpectation, CreatedIdentityRowWrite};

/// The machine edge between `a` and `b`.
fn machine_edge(a: &AgentIdentity, b: &AgentIdentity) -> mob_dsl::WiringEdge {
    mob_dsl::WiringEdge::new(
        mob_dsl::AgentIdentity::from_domain(a),
        mob_dsl::AgentIdentity::from_domain(b),
    )
}

/// The realization verdict MobMachine emitted in `effects`.
fn realization_verdict(
    effects: &[mob_dsl::MobMachineEffect],
) -> Result<mob_dsl::OwnedEdgeRealization, MobError> {
    effects
        .iter()
        .find_map(|effect| match effect {
            mob_dsl::MobMachineEffect::OwnedEdgeRealizationResolved { verdict, .. } => {
                Some(*verdict)
            }
            _ => None,
        })
        .ok_or_else(|| {
            MobError::Internal(
                "MobMachine decided an owned edge's realization with no verdict".into(),
            )
        })
}

/// The candidates of a realization staged on a step's prepared `authority`
/// (`owned_edge_candidates` over its state) that MobMachine holds in force
/// (`AdmitOwnedEdgeRealization` answers `Realize`): the owned edges the step
/// wires. Each verdict's effect goes to `effects`.
pub(super) fn owned_pairs_in_force<M: mob_dsl::MobMachineMutator>(
    authority: &mut M,
    candidates: Vec<(AgentIdentity, AgentIdentity)>,
    effects: &mut Vec<mob_dsl::MobMachineEffect>,
) -> Result<Vec<(AgentIdentity, AgentIdentity)>, MobError> {
    let mut pairs = Vec::with_capacity(candidates.len());
    for (local, peer) in candidates {
        let transition = authority
            .apply(mob_dsl::MobMachineInput::AdmitOwnedEdgeRealization {
                edge: machine_edge(&local, &peer),
            })
            .map_err(|error| MobError::MobMachineRejected {
                context: "admit_owned_edge_realization",
                reason: error.to_string(),
            })?;
        let verdict = realization_verdict(transition.effects())?;
        effects.extend(transition.into_effects());
        if verdict == mob_dsl::OwnedEdgeRealization::Realize {
            pairs.push((local, peer));
        }
    }
    Ok(pairs)
}

impl MobActor {
    /// The owned edges of `members` a realization wires now: each joins a
    /// member of `members` to another member, both seated and active, the
    /// mob has not wired it (`owned_edge_candidates`), and MobMachine holds
    /// it in force (`AdmitOwnedEdgeRealization`). An edge one of whose ends
    /// is no identity entry (an unlisted roster-owned identity's name, which
    /// a worker may hold) is left declared and unrealized.
    pub(super) fn owned_pairs_to_realize(
        &mut self,
        members: &[AgentIdentity],
    ) -> Result<Vec<(AgentIdentity, AgentIdentity)>, MobError> {
        let candidates = owned_edge_candidates(self.dsl_authority.state(), members);
        let mut pairs = Vec::with_capacity(candidates.len());
        for (local, peer) in candidates {
            let effects = self.apply_dsl_input_collect_effects(
                mob_dsl::MobMachineInput::AdmitOwnedEdgeRealization {
                    edge: machine_edge(&local, &peer),
                },
                "admit_owned_edge_realization",
            )?;
            if realization_verdict(&effects)? == mob_dsl::OwnedEdgeRealization::Realize {
                pairs.push((local, peer));
            }
        }
        Ok(pairs)
    }

    /// Declare on `authority` each edge `owner`'s change lists
    /// (`DeclareOwnedIdentityEdge`), in order, and answer the edges `owner`
    /// owns now: those MobMachine declared. An edge an identity owns already
    /// gives no custody (`AlreadyOwned`) and is left out; a peer that is no
    /// identity of the mob refuses the change `peer_unknown`.
    pub(super) fn declare_owned_edges<M: mob_dsl::MobMachineMutator>(
        authority: &mut M,
        owner: &AgentIdentity,
        peers: &[OwnedIdentityEdge],
    ) -> Result<Vec<OwnedIdentityEdge>, MobError> {
        let dsl_owner = mob_dsl::AgentIdentity::from_domain(owner);
        let mut declared = Vec::new();
        for owned in peers {
            let transition = authority
                .apply(mob_dsl::MobMachineInput::DeclareOwnedIdentityEdge {
                    edge: machine_edge(owner, &owned.peer),
                    owner: dsl_owner.clone(),
                })
                .map_err(|error| MobError::MobMachineRejected {
                    context: "declare_owned_identity_edge",
                    reason: error.to_string(),
                })?;
            let verdict = transition
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
                mob_dsl::OwnedEdgeDeclareVerdict::Declared => declared.push(owned.clone()),
                mob_dsl::OwnedEdgeDeclareVerdict::AlreadyOwned => {}
                mob_dsl::OwnedEdgeDeclareVerdict::PeerUnknown => {
                    return Err(refused(IdentityChangeRefusal::PeerUnknown {
                        identity: owner.clone(),
                        peer: owned.peer.clone(),
                    }));
                }
                mob_dsl::OwnedEdgeDeclareVerdict::MobNotRunning => {
                    return Err(refused(IdentityChangeRefusal::MobNotRunning));
                }
                mob_dsl::OwnedEdgeDeclareVerdict::OwnerInvalid => {
                    return Err(MobError::Internal(format!(
                        "MobMachine refused '{owner}' as the owner of the edge its own change \
                         lists to '{}'",
                        owned.peer
                    )));
                }
            }
        }
        Ok(declared)
    }

    /// End on `authority` the edge `owner` owns to each of `peers`
    /// (`EndOwnedIdentityEdge`, by its owner's change). A peer `owner` owns
    /// no edge to refuses the change: its own change ends only its own
    /// edges.
    pub(super) fn end_owned_edges_by_owner<M: mob_dsl::MobMachineMutator>(
        authority: &mut M,
        owner: &AgentIdentity,
        peers: &[AgentIdentity],
    ) -> Result<(), MobError> {
        for peer in peers {
            match Self::end_owned_edge(
                authority,
                owner,
                peer,
                mob_dsl::OwnedEdgeEndCause::OwnerChange,
            )? {
                OwnedEdgeEndOutcome::Ended => {}
                OwnedEdgeEndOutcome::NotOwner | OwnedEdgeEndOutcome::Absent => {
                    return Err(refused(IdentityChangeRefusal::InvalidRequest {
                        problem: format!(
                            "'{owner}' owns no edge to '{peer}': its change ends only its own edges"
                        ),
                    }));
                }
            }
        }
        Ok(())
    }

    /// End the edge between `owner` and `peer` on `authority`
    /// (`EndOwnedIdentityEdge`) for `cause`: answers whether it ended, or
    /// another identity owns it, or none does.
    fn end_owned_edge<M: mob_dsl::MobMachineMutator>(
        authority: &mut M,
        owner: &AgentIdentity,
        peer: &AgentIdentity,
        cause: mob_dsl::OwnedEdgeEndCause,
    ) -> Result<OwnedEdgeEndOutcome, MobError> {
        let transition = authority
            .apply(mob_dsl::MobMachineInput::EndOwnedIdentityEdge {
                edge: machine_edge(owner, peer),
                owner: mob_dsl::AgentIdentity::from_domain(owner),
                cause,
            })
            .map_err(|error| MobError::MobMachineRejected {
                context: "end_owned_identity_edge",
                reason: error.to_string(),
            })?;
        let verdict = transition
            .effects()
            .iter()
            .find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::OwnedIdentityEdgeEnded { verdict, .. } => Some(*verdict),
                _ => None,
            })
            .ok_or_else(|| {
                MobError::Internal("MobMachine ended an owned edge with no verdict".into())
            })?;
        match verdict {
            mob_dsl::OwnedEdgeEndVerdict::Ended => Ok(OwnedEdgeEndOutcome::Ended),
            mob_dsl::OwnedEdgeEndVerdict::NotOwner => Ok(OwnedEdgeEndOutcome::NotOwner),
            mob_dsl::OwnedEdgeEndVerdict::Absent => Ok(OwnedEdgeEndOutcome::Absent),
            mob_dsl::OwnedEdgeEndVerdict::NonAuthoritative => {
                Err(refused(IdentityChangeRefusal::CandidateLaunch))
            }
            mob_dsl::OwnedEdgeEndVerdict::ChangesHeld => {
                Err(refused(IdentityChangeRefusal::ChangesHeld))
            }
            mob_dsl::OwnedEdgeEndVerdict::MobNotRunning => {
                Err(refused(IdentityChangeRefusal::MobNotRunning))
            }
        }
    }

    /// End the edge `owner` owns to `peer` because the host's policy reload
    /// denies it (`EndOwnedIdentityEdge`, `PolicyDenied`): MobMachine decides
    /// it on a prepared authority, the owner's row is written without the
    /// edge before the machine commits, and the edge's realization is
    /// unwired once it committed. An edge another identity owns, or none
    /// does, changes nothing.
    pub(super) async fn end_denied_owned_edge(
        &mut self,
        owner: AgentIdentity,
        peer: AgentIdentity,
    ) -> Result<OwnedEdgeEndOutcome, MobError> {
        let store = self
            .identity_member
            .clone()
            .ok_or_else(|| refused(IdentityChangeRefusal::StorageUnsupported))?;
        let current = store
            .observe_created_identity(&self.definition.id, &owner)
            .await?;
        let prepared = self.prepare_dsl_input_transition(
            mob_dsl::MobMachineInput::EndOwnedIdentityEdge {
                edge: machine_edge(&owner, &peer),
                owner: mob_dsl::AgentIdentity::from_domain(&owner),
                cause: mob_dsl::OwnedEdgeEndCause::PolicyDenied,
            },
            "end_owned_identity_edge",
        )?;
        let verdict = prepared
            .transition
            .effects()
            .iter()
            .find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::OwnedIdentityEdgeEnded { verdict, .. } => Some(*verdict),
                _ => None,
            })
            .ok_or_else(|| {
                MobError::Internal("MobMachine ended an owned edge with no verdict".into())
            })?;
        match verdict {
            mob_dsl::OwnedEdgeEndVerdict::Ended => {}
            mob_dsl::OwnedEdgeEndVerdict::NotOwner => return Ok(OwnedEdgeEndOutcome::NotOwner),
            mob_dsl::OwnedEdgeEndVerdict::Absent => return Ok(OwnedEdgeEndOutcome::Absent),
            mob_dsl::OwnedEdgeEndVerdict::NonAuthoritative => {
                return Err(refused(IdentityChangeRefusal::CandidateLaunch));
            }
            mob_dsl::OwnedEdgeEndVerdict::ChangesHeld => {
                return Err(refused(IdentityChangeRefusal::ChangesHeld));
            }
            mob_dsl::OwnedEdgeEndVerdict::MobNotRunning => {
                return Err(refused(IdentityChangeRefusal::MobNotRunning));
            }
        }
        let written = row_without_peers(current, &owner, std::slice::from_ref(&peer))?;
        let committed_owner = owner.clone();
        self.commit_prepared_dsl_transition_after(prepared, move || async move {
            let Some(row) = written else {
                return Ok(());
            };
            if store.settle_created_identity_row(&row).await? {
                Ok(())
            } else {
                Err(MobError::LifecycleOperationPending {
                    intent: format!(
                        "end an owned edge of '{committed_owner}' while its row moved: retry"
                    ),
                })
            }
        })
        .await?;
        Box::pin(self.unwire_ended_owned_edges(&[(owner, peer)])).await;
        Ok(OwnedEdgeEndOutcome::Ended)
    }

    /// The rows of the owners in `owners` (other than `removed`) written
    /// without their edges to `removed`, each checked against the row read
    /// here: a removal ends them with its tombstone, in its own store
    /// transaction. An owner whose row holds no such edge writes nothing.
    pub(super) async fn owner_rows_without_removed_peer(
        &self,
        owners: &BTreeSet<AgentIdentity>,
        removed: &AgentIdentity,
    ) -> Result<Vec<CreatedIdentityRowWrite>, MobError> {
        let store = self
            .identity_member
            .as_ref()
            .ok_or_else(|| refused(IdentityChangeRefusal::StorageUnsupported))?;
        let mut rows = Vec::new();
        for owner in owners.iter().filter(|owner| *owner != removed) {
            let current = store
                .observe_created_identity(&self.definition.id, owner)
                .await?;
            if let Some(row) = row_without_peers(current, owner, std::slice::from_ref(removed))? {
                rows.push(row);
            }
        }
        Ok(rows)
    }

    /// A plain unwire's admission (`AdmitMemberUnwire`): an owned edge in
    /// force takes no plain unwire, refused [`MobError::OwnedIdentityEdge`].
    /// An owned edge one of whose ends is no identity entry is plain wiring
    /// for a member under that end's name.
    pub(super) fn admit_member_unwire(
        &mut self,
        local: &AgentIdentity,
        peer: &AgentIdentity,
    ) -> Result<(), MobError> {
        if local == peer {
            // The unwire refuses an edge of one member itself.
            return Ok(());
        }
        let edge = machine_edge(local, peer);
        let effects = self.apply_dsl_input_collect_effects(
            mob_dsl::MobMachineInput::AdmitMemberUnwire { edge: edge.clone() },
            "admit_member_unwire",
        )?;
        let verdict = effects
            .iter()
            .find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::MemberUnwireAdmissionResolved { verdict, .. } => {
                    Some(*verdict)
                }
                _ => None,
            })
            .ok_or_else(|| {
                MobError::Internal("MobMachine admitted an unwire with no verdict".into())
            })?;
        match verdict {
            mob_dsl::MemberUnwireAdmission::Admitted => Ok(()),
            mob_dsl::MemberUnwireAdmission::OwnedIdentityEdge => {
                let owner = self
                    .dsl_authority
                    .state()
                    .identity_edge_owners
                    .get(&edge)
                    .map(|owner| AgentIdentity::from(owner.0.as_str()))
                    .ok_or_else(|| {
                        MobError::Internal(
                            "MobMachine refused an unwire of an owned edge it holds no owner of"
                                .into(),
                        )
                    })?;
                Err(MobError::OwnedIdentityEdge {
                    local: local.clone(),
                    peer: peer.clone(),
                    owner,
                })
            }
        }
    }

    /// Realize each owned edge in force of `members` whose ends are both
    /// seated and which MobMachine has not wired yet, by the wire's own path.
    /// A realization that fails leaves the edge owed a repair.
    pub(super) async fn realize_owned_edges_of(&mut self, members: &[AgentIdentity]) {
        match self.owned_pairs_to_realize(members) {
            Ok(pairs) => Box::pin(self.realize_owned_pairs(pairs)).await,
            Err(error) => tracing::warn!(
                mob_id = %self.definition.id,
                %error,
                "owned edges were not realized: MobMachine decided no realization"
            ),
        }
    }

    /// Realize every owned edge in force whose ends are both seated and
    /// which MobMachine has not wired yet: a start's pass, after its seats,
    /// for an edge a crash left unrealized, and an import's or an
    /// adoption's after its commit. A mob that is not running wires
    /// nothing: its resume runs the pass again.
    pub(super) async fn realize_every_owned_edge(&mut self) {
        let state = self.dsl_authority.state();
        if state.lifecycle_phase != mob_dsl::MobPhase::Running {
            return;
        }
        let members = crate::runtime::owned_edges::edge_owners(state);
        Box::pin(self.realize_owned_edges_of(&members)).await;
    }

    async fn realize_owned_pairs(&mut self, pairs: Vec<(AgentIdentity, AgentIdentity)>) {
        for (local, peer) in pairs {
            if let Err(error) = Box::pin(self.handle_wire(
                local.clone(),
                super::super::handle::PeerTarget::Local(peer.clone()),
            ))
            .await
            {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    member = %local,
                    peer = %peer,
                    %error,
                    "an owned edge could not be realized"
                );
                let edge = machine_edge(&local, &peer);
                if self.dsl_authority.state().wiring_edges.contains(&edge) {
                    self.resolve_member_edge_realization(
                        &super::member_wake::MemberWiringEdge::Member(edge),
                        false,
                    );
                }
            }
        }
    }

    /// Unwire the realization of each ended owned edge MobMachine still
    /// holds wired, as a plain unwire of it would. A failure is logged: the
    /// edge is owned by no identity any more.
    pub(super) async fn unwire_ended_owned_edges(
        &mut self,
        edges: &[(AgentIdentity, AgentIdentity)],
    ) {
        for (owner, peer) in edges {
            if !self
                .dsl_authority
                .state()
                .wiring_edges
                .contains(&machine_edge(owner, peer))
            {
                continue;
            }
            let unwired = match Box::pin(self.prepare_member_unwire(
                owner.clone(),
                super::super::handle::PeerTarget::Local(peer.clone()),
                None,
                false,
            ))
            .await
            {
                Ok(preparation) => {
                    Box::pin(self.realize_wiring_preparation_inline(preparation)).await
                }
                Err(error) => Err(error),
            };
            if let Err(error) = unwired {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    owner = %owner,
                    peer = %peer,
                    %error,
                    "an ended owned edge's realization could not be unwired"
                );
            }
        }
    }
}

/// `owner`'s row (`current`) written without its edges to `peers`, checked
/// against the row read; `None` when the row holds none of them. A row this
/// build cannot read refuses: its name is reserved.
fn row_without_peers(
    current: Option<CreatedIdentityObservation>,
    owner: &AgentIdentity,
    peers: &[AgentIdentity],
) -> Result<Option<CreatedIdentityRowWrite>, MobError> {
    let row = match current {
        Some(CreatedIdentityObservation::Created(row)) => row,
        Some(CreatedIdentityObservation::Unreadable { .. }) => {
            return Err(refused(IdentityChangeRefusal::NameTaken {
                identity: owner.clone(),
                by: crate::runtime::IdentityNameHolder::Unreadable,
            }));
        }
        None => return Ok(None),
    };
    if !row
        .owned_edges
        .iter()
        .any(|edge| peers.contains(&edge.peer))
    {
        return Ok(None);
    }
    let kept = row
        .owned_edges
        .iter()
        .filter(|edge| !peers.contains(&edge.peer))
        .cloned()
        .collect();
    let expected = CreatedIdentityRowExpectation::Current {
        authority_digest: row.authority_digest.clone(),
    };
    let record = row
        .with_owned_edges(kept)
        .map_err(|error| MobError::Internal(format!("an owned edge's owner row: {error}")))?;
    Ok(Some(CreatedIdentityRowWrite { record, expected }))
}

use crate::event::MobEvent;
use crate::ids::{AgentIdentity, Generation, ProfileName};
use crate::roster::{MobMemberKickoffSnapshot, Roster, RosterAddEntry, RosterEntry};

mod sealed {
    pub trait Sealed {}
}

/// Sealed mutator trait for the event-backed roster projection.
///
/// Only [`RosterAuthority`] implements this trait so that projection mutations
/// flow through one actor-owned facade. Machine facts such as lifecycle,
/// membership admission, profile identity, and wiring authority remain owned by
/// the generated `MobMachine`; this wrapper only updates roster display state.
pub(crate) trait RosterMutator: sealed::Sealed {
    fn add_member(&mut self, entry: RosterAddEntry) -> bool;
    fn remove_member(&mut self, agent_identity: &AgentIdentity) -> bool;
    fn set_kickoff(
        &mut self,
        agent_identity: &AgentIdentity,
        kickoff: Option<MobMemberKickoffSnapshot>,
    ) -> bool;
}

/// Actor-owned facade for the roster projection.
///
/// This registry owns the same data that `Roster` exposes and keeps writes
/// local to the actor. It is not behavior authority for machine facts; runtime
/// control paths must consult generated `MobMachine` state before acting. In
/// particular, roster presence must never decide whether a retire cancels a
/// pending spawn: `ClassifyRetirePendingSpawnDisposition` owns that
/// incarnation-scoped verdict and names the exact pending session when
/// cancellation is authorized.
#[derive(Debug, Clone)]
pub(crate) struct RosterAuthority {
    roster: Roster,
    /// Bumped by every projection mutation, so the actor can tell a
    /// roster-only change from a no-op when deciding whether to wake
    /// machine-state watchers.
    revision: u64,
}

impl sealed::Sealed for RosterAuthority {}

impl RosterAuthority {
    /// Create an authority with an empty roster projection.
    pub(crate) fn new() -> Self {
        Self {
            roster: Roster::new(),
            revision: 0,
        }
    }

    /// Create an authority from an existing roster snapshot/projection.
    pub(crate) fn from_roster(roster: Roster) -> Self {
        Self {
            roster,
            revision: 0,
        }
    }

    /// Monotone mutation revision of this projection.
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    fn bump(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    pub(crate) fn record_creation_event(&mut self, event: &MobEvent) {
        self.roster.creation_history.observe(event);
    }

    pub(crate) fn member_creation_for_session(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Result<Option<crate::MemberCreationSnapshot>, crate::MemberCreationError> {
        self.roster.creation_history.get(session_id)
    }

    /// The creation witness of the incarnation the runtime puts in place of
    /// `entry`'s (a respawn, a rotation): the creation `entry`'s own session
    /// recorded, so the successor keeps the member's creation id, with
    /// Successor provenance naming that session. A caller's spec cannot
    /// author it. Without a session, or a creation token of this member
    /// recorded for it, the witness is unavailable and the successor is
    /// recorded unproven, never as a new root.
    pub(crate) fn successor_creation_witness(
        &self,
        entry: &RosterEntry,
    ) -> crate::MemberCreationSourceWitness {
        let Some(session_id) = entry.member_ref.bridge_session_id() else {
            return crate::MemberCreationSourceWitness::unavailable();
        };
        match self.member_creation_for_session(session_id) {
            Ok(Some(crate::MemberCreationSnapshot {
                creation:
                    crate::MemberCreationRecord {
                        creation_id: Some(creation_id),
                        ..
                    },
                member_binding,
                ..
            })) if member_binding.member == entry.agent_identity.as_str() => {
                crate::MemberCreationSourceWitness {
                    successor: true,
                    source: Some(crate::MemberCreationSource {
                        session_id: session_id.clone(),
                        member_binding,
                        creation_id,
                    }),
                }
            }
            _ => crate::MemberCreationSourceWitness::unavailable(),
        }
    }

    /// Snapshot the current roster for read-only surfaces.
    pub(crate) fn snapshot(&self) -> Roster {
        self.roster.clone()
    }

    pub(crate) fn get(&self, agent_identity: &AgentIdentity) -> Option<&RosterEntry> {
        self.roster.get(agent_identity)
    }

    pub(crate) fn get_by_identity(&self, identity: &AgentIdentity) -> Option<&RosterEntry> {
        self.roster.get_by_identity(identity)
    }

    /// The prompt of the kickoff the current incarnation of `identity` owes,
    /// when its spawn row named one ([`Roster::owed_kickoff_prompt`]).
    pub(crate) fn owed_kickoff_prompt(
        &self,
        identity: &AgentIdentity,
    ) -> Option<&meerkat_core::types::ContentInput> {
        self.roster.owed_kickoff_prompt(identity)
    }

    /// What the unborn seat session `identity` is bound to carries to its
    /// first build ([`Roster::seat_build`]).
    pub(crate) fn seat_build(
        &self,
        identity: &AgentIdentity,
    ) -> Option<std::sync::Arc<crate::runtime::SeatBuild>> {
        self.roster.seat_build(identity)
    }

    /// Whether a journal row of this mob recorded `session_id`'s member comms
    /// endpoint ([`Roster::session_endpoint_recorded`]).
    pub(crate) fn session_endpoint_recorded(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> bool {
        self.roster.session_endpoint_recorded(session_id)
    }

    /// Keep the process-local build inputs of the seat just committed on the
    /// unborn session `session_id` ([`Roster::retain_seat_local_build_inputs`]).
    pub(crate) fn retain_seat_local_build_inputs(
        &mut self,
        session_id: &meerkat_core::types::SessionId,
        local: crate::runtime::SeatLocalBuildInputs,
    ) {
        self.bump();
        self.roster
            .retain_seat_local_build_inputs(session_id, local);
    }

    pub(crate) fn list(&self) -> impl Iterator<Item = &RosterEntry> {
        self.roster.list()
    }

    pub(crate) fn list_all(&self) -> impl Iterator<Item = &RosterEntry> {
        self.roster.list_all()
    }

    pub(crate) fn by_profile(&self, profile: &ProfileName) -> impl Iterator<Item = &RosterEntry> {
        self.roster.by_profile(profile)
    }

    /// Get a specific roster entry.
    pub(crate) fn entry(&self, agent_identity: &AgentIdentity) -> Option<RosterEntry> {
        self.roster.get(agent_identity).cloned()
    }

    /// The roster projection a member restore's turn writes
    /// (`MemberRestoreCommit`): a runtime wake applies the same writes to the
    /// actor's roster that a boot restore applies to the roster it builds.
    /// Counted as a mutation.
    #[cfg(feature = "runtime-adapter")]
    pub(crate) fn restore_projection_mut(&mut self) -> &mut Roster {
        self.bump();
        &mut self.roster
    }

    /// Apply a `MobEvent` through the underlying `Roster` projection so
    /// live actor paths that append events also keep the roster's
    /// projection fields (e.g. `wired_to`) in sync. Mirrors the replay
    /// path's `Roster::apply` — same match arms, same mutations.
    pub(crate) fn apply_event(&mut self, event: &MobEvent) {
        self.bump();
        self.roster.apply(event);
    }

    /// Project the resolved comms identity (peer id and transport key) for an
    /// existing member. Returns whether the member was present.
    pub(crate) fn set_comms_identity(
        &mut self,
        agent_identity: &AgentIdentity,
        peer_id: Option<meerkat_core::comms::PeerId>,
        transport_public_key: Option<String>,
    ) -> bool {
        self.bump();
        self.roster
            .set_comms_identity(agent_identity, peer_id, transport_public_key)
    }

    pub(crate) fn replace_backend_peer_binding_for_identities(
        &mut self,
        identities: &std::collections::BTreeSet<AgentIdentity>,
        next_peer_id: &str,
        next_address: &str,
        bootstrap_token: Option<meerkat_contracts::wire::supervisor_bridge::BridgeBootstrapToken>,
        direct_member_fence: Option<
            meerkat_contracts::wire::supervisor_bridge::BridgeDirectMemberFence,
        >,
    ) -> Vec<(AgentIdentity, Generation, [u8; 32])> {
        self.bump();
        self.roster.replace_backend_peer_binding_for_identities(
            identities,
            next_peer_id,
            next_address,
            bootstrap_token,
            direct_member_fence,
        )
    }
}

impl RosterMutator for RosterAuthority {
    fn add_member(&mut self, entry: RosterAddEntry) -> bool {
        self.bump();
        self.roster.add(entry)
    }

    fn remove_member(&mut self, agent_identity: &AgentIdentity) -> bool {
        self.bump();
        if self.roster.get(agent_identity).is_some() {
            self.roster.remove(agent_identity);
            true
        } else {
            false
        }
    }

    fn set_kickoff(
        &mut self,
        agent_identity: &AgentIdentity,
        kickoff: Option<MobMemberKickoffSnapshot>,
    ) -> bool {
        self.bump();
        self.roster.set_kickoff(agent_identity, kickoff)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        MemberCreationId, MemberCreationProvenance, MemberCreationRecord, MobEventKind, MobId,
    };
    use meerkat_core::SessionId;

    /// The roster of one seated `worker` whose spawn on `session` recorded
    /// `creation`.
    fn seated(session: &SessionId, creation: MemberCreationRecord) -> RosterAuthority {
        let identity = AgentIdentity::from("worker");
        let mut spawned = crate::event::MemberSpawnedEvent::new(
            identity.clone(),
            Generation::INITIAL,
            crate::FenceToken::new(1),
            crate::AgentRuntimeId::initial(identity),
            ProfileName::from("worker"),
        )
        .with_bridge_member_ref(Some(crate::event::MemberRef::from_bridge_session_id(
            session.clone(),
        )));
        spawned.creation = creation;
        RosterAuthority::from_roster(Roster::project(&[MobEvent {
            cursor: 1,
            timestamp: chrono::Utc::now(),
            mob_id: MobId::from("test"),
            kind: MobEventKind::MemberSpawned(spawned),
        }]))
    }

    /// The creation record the runtime gives the incarnation that replaces
    /// the seated `worker` on `next`.
    fn successor_record(roster: &RosterAuthority, next: &SessionId) -> MemberCreationRecord {
        let entry = roster
            .get(&AgentIdentity::from("worker"))
            .expect("the worker is seated");
        roster
            .successor_creation_witness(entry)
            .successor_record(next)
    }

    #[test]
    fn a_successor_keeps_the_members_creation_and_is_unproven_without_a_recorded_token() {
        let retired = SessionId::new();
        let next = SessionId::new();
        let id = MemberCreationId::new();
        let roster = seated(
            &retired,
            MemberCreationRecord {
                creation_id: Some(id),
                provenance: MemberCreationProvenance::Root,
            },
        );

        // The next incarnation keeps the creation id and names the retired
        // incarnation's session and binding as its predecessor.
        let kept = successor_record(&roster, &next);
        assert_eq!(kept.creation_id, Some(id));
        assert!(matches!(
            &kept.provenance,
            MemberCreationProvenance::Successor {
                predecessor_session_id,
                predecessor_member_binding,
                predecessor_creation_id,
            } if *predecessor_session_id == retired
                && predecessor_member_binding.member == "worker"
                && *predecessor_creation_id == id
        ));

        // An incarnation is never its own predecessor.
        let same_session = successor_record(&roster, &retired);
        assert_eq!(same_session.provenance, MemberCreationProvenance::Unproven);
        assert!(same_session.creation_id.is_some_and(|fresh| fresh != id));

        // A spawn that recorded no creation token (an older journal) leaves
        // nothing to keep: the next incarnation is unproven with a new id,
        // never a new root.
        let legacy = seated(&retired, MemberCreationRecord::default());
        let unproven = successor_record(&legacy, &next);
        assert_eq!(unproven.provenance, MemberCreationProvenance::Unproven);
        assert!(unproven.creation_id.is_some());
    }
}

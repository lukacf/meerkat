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
/// control paths must consult generated `MobMachine` state before acting.
#[derive(Debug, Clone)]
pub(crate) struct RosterAuthority {
    roster: Roster,
}

impl sealed::Sealed for RosterAuthority {}

impl RosterAuthority {
    /// Create an authority with an empty roster projection.
    pub(crate) fn new() -> Self {
        Self {
            roster: Roster::new(),
        }
    }

    /// Create an authority from an existing roster snapshot/projection.
    pub(crate) fn from_roster(roster: Roster) -> Self {
        Self { roster }
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

    /// Apply a `MobEvent` through the underlying `Roster` projection so
    /// live actor paths that append events also keep the roster's
    /// projection fields (e.g. `wired_to`) in sync. Mirrors the replay
    /// path's `Roster::apply` — same match arms, same mutations.
    pub(crate) fn apply_event(&mut self, event: &MobEvent) {
        self.roster.apply(event);
    }

    pub(crate) fn replace_backend_peer_binding_for_identities(
        &mut self,
        identities: &std::collections::BTreeSet<AgentIdentity>,
        next_peer_id: &str,
        next_address: &str,
        bootstrap_token: Option<meerkat_contracts::wire::supervisor_bridge::BridgeBootstrapToken>,
    ) -> Vec<(AgentIdentity, Generation, [u8; 32])> {
        self.roster.replace_backend_peer_binding_for_identities(
            identities,
            next_peer_id,
            next_address,
            bootstrap_token,
        )
    }
}

impl RosterMutator for RosterAuthority {
    fn add_member(&mut self, entry: RosterAddEntry) -> bool {
        self.roster.add(entry)
    }

    fn remove_member(&mut self, agent_identity: &AgentIdentity) -> bool {
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
        self.roster.set_kickoff(agent_identity, kickoff)
    }
}

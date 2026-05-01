use crate::event::MobEvent;
use crate::ids::{AgentIdentity, Generation, MeerkatId, ProfileName};
use crate::roster::{MobMemberKickoffSnapshot, Roster, RosterAddEntry, RosterEntry};

mod sealed {
    pub trait Sealed {}
}

/// Sealed mutator trait for roster canonical state.
///
/// Only [`RosterAuthority`] implements this trait so that all canonical roster
/// mutations flow through a single source of truth. External helpers can take a
/// mutable reference to the authority and drive the lifecycle/wiring transitions
/// without touching the underlying projection directly.
pub(crate) trait RosterMutator: sealed::Sealed {
    fn add_member(&mut self, entry: RosterAddEntry) -> bool;
    fn remove_member(&mut self, agent_identity: &MeerkatId) -> bool;
    fn set_kickoff(
        &mut self,
        agent_identity: &MeerkatId,
        kickoff: Option<MobMemberKickoffSnapshot>,
    ) -> bool;
}

/// Canonical authority for the roster projection.
///
/// This registry owns the same data that `Roster` exposes but enforces that all
/// mutations (add, retire, wire) happen through this sealed authority so that
/// higher-level actors can reason about lifecycle/wiring ordering separately.
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

    pub(crate) fn get(&self, agent_identity: &MeerkatId) -> Option<&RosterEntry> {
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
    pub(crate) fn entry(&self, agent_identity: &MeerkatId) -> Option<RosterEntry> {
        self.roster.get(agent_identity).cloned()
    }

    /// Apply a `MobEvent` through the underlying `Roster` projection so
    /// live actor paths that append events also keep the roster's
    /// projection fields (e.g. `wired_to`) in sync. Mirrors the replay
    /// path's `Roster::apply` — same match arms, same mutations.
    pub(crate) fn apply_event(&mut self, event: &MobEvent) {
        self.roster.apply(event);
    }

    /// Flip a roster entry to `Retiring` state. Mirrors
    /// [`Roster::mark_retiring_by_identity`]. Returns `true` if the member
    /// was present and was flipped to `Retiring`; `false` otherwise.
    pub(crate) fn mark_retiring_by_identity(&mut self, identity: &AgentIdentity) -> bool {
        self.roster.mark_retiring_by_identity(identity)
    }

    pub(crate) fn replace_backend_peer_binding_by_peer_id(
        &mut self,
        prior_peer_id: &str,
        next_peer_id: &str,
        next_address: &str,
        bootstrap_token: Option<meerkat_contracts::wire::supervisor_bridge::BridgeBootstrapToken>,
    ) -> Vec<(AgentIdentity, Generation, Option<[u8; 32]>)> {
        self.roster.replace_backend_peer_binding_by_peer_id(
            prior_peer_id,
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

    fn remove_member(&mut self, agent_identity: &MeerkatId) -> bool {
        if self.roster.get(agent_identity).is_some() {
            self.roster.remove(agent_identity);
            true
        } else {
            false
        }
    }

    fn set_kickoff(
        &mut self,
        agent_identity: &MeerkatId,
        kickoff: Option<MobMemberKickoffSnapshot>,
    ) -> bool {
        self.roster.set_kickoff(agent_identity, kickoff)
    }
}

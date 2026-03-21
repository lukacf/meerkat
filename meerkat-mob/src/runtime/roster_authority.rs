use crate::ids::{MeerkatId, ProfileName};
use crate::roster::{Roster, RosterAddEntry, RosterEntry};
use meerkat_core::types::SessionId;

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
    fn mark_retiring(&mut self, meerkat_id: &MeerkatId) -> bool;
    fn remove_member(&mut self, meerkat_id: &MeerkatId) -> bool;
    fn wire_members(&mut self, a: &MeerkatId, b: &MeerkatId);
    fn unwire_members(&mut self, a: &MeerkatId, b: &MeerkatId);
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

    pub(crate) fn get(&self, meerkat_id: &MeerkatId) -> Option<&RosterEntry> {
        self.roster.get(meerkat_id)
    }

    pub(crate) fn list(&self) -> impl Iterator<Item = &RosterEntry> {
        self.roster.list()
    }

    pub(crate) fn list_all(&self) -> impl Iterator<Item = &RosterEntry> {
        self.roster.list_all()
    }

    pub(crate) fn list_retiring(&self) -> impl Iterator<Item = &RosterEntry> {
        self.roster.list_retiring()
    }

    pub(crate) fn by_profile(&self, profile: &ProfileName) -> impl Iterator<Item = &RosterEntry> {
        self.roster.by_profile(profile)
    }

    pub(crate) fn find_by_label(&self, key: &str, value: &str) -> Option<&RosterEntry> {
        self.roster.find_by_label(key, value)
    }

    pub(crate) fn session_id(&self, meerkat_id: &MeerkatId) -> Option<&SessionId> {
        self.roster.session_id(meerkat_id)
    }

    /// Get a specific roster entry.
    pub(crate) fn entry(&self, meerkat_id: &MeerkatId) -> Option<RosterEntry> {
        self.roster.get(meerkat_id).cloned()
    }

    /// List all members currently `Active`.
    pub(crate) fn active_members(&self) -> Vec<RosterEntry> {
        self.roster.list().cloned().collect()
    }

    /// List the members that have been marked `Retiring`.
    pub(crate) fn retiring_members(&self) -> Vec<RosterEntry> {
        self.roster.list_retiring().cloned().collect()
    }

    pub(crate) fn wiring_edge_state(
        &self,
        a: &MeerkatId,
        b: &MeerkatId,
    ) -> crate::roster::WiringEdgeState {
        self.roster.wiring_edge_state(a, b)
    }

    pub(crate) fn debug_assert_wiring_projection_consistent(&self) {
        self.roster.debug_assert_wiring_projection_consistent()
    }

    pub(crate) fn set_session_id(&mut self, meerkat_id: &MeerkatId, session_id: SessionId) -> bool {
        self.roster.set_session_id(meerkat_id, session_id)
    }
}

impl RosterMutator for RosterAuthority {
    fn add_member(&mut self, entry: RosterAddEntry) -> bool {
        self.roster.add(entry)
    }

    fn mark_retiring(&mut self, meerkat_id: &MeerkatId) -> bool {
        self.roster.mark_retiring(meerkat_id)
    }

    fn remove_member(&mut self, meerkat_id: &MeerkatId) -> bool {
        if self.roster.get(meerkat_id).is_some() {
            self.roster.remove(meerkat_id);
            true
        } else {
            false
        }
    }

    fn wire_members(&mut self, a: &MeerkatId, b: &MeerkatId) {
        self.roster.wire(a, b);
    }

    fn unwire_members(&mut self, a: &MeerkatId, b: &MeerkatId) {
        self.roster.unwire(a, b);
    }
}

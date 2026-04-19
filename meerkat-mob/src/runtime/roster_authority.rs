use crate::ids::{AgentIdentity, Generation, MeerkatId, ProfileName};
use crate::roster::{MobMemberKickoffSnapshot, Roster, RosterAddEntry, RosterEntry};
use meerkat_core::comms::TrustedPeerSpec;
use meerkat_core::types::SessionId;
use std::collections::BTreeSet;

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
    /// Re-project the `Retiring` marker set from the DSL. See
    /// [`Roster::sync_retiring_projection`].
    fn sync_retiring_projection(&mut self, retiring_runtime_ids: &BTreeSet<String>);
    fn remove_member(&mut self, agent_identity: &MeerkatId) -> bool;
    fn wire_members(&mut self, a: &MeerkatId, b: &MeerkatId);
    fn wire_external_peer(
        &mut self,
        local: &MeerkatId,
        peer_name: &MeerkatId,
        spec: TrustedPeerSpec,
    );
    fn unwire_members(&mut self, a: &MeerkatId, b: &MeerkatId);
    fn unwire_external_peer(&mut self, local: &MeerkatId, peer_name: &MeerkatId);
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

    #[allow(dead_code)]
    pub(crate) fn list_retiring(&self) -> impl Iterator<Item = &RosterEntry> {
        self.roster.list_retiring()
    }

    pub(crate) fn by_profile(&self, profile: &ProfileName) -> impl Iterator<Item = &RosterEntry> {
        self.roster.by_profile(profile)
    }

    #[allow(dead_code)]
    pub(crate) fn find_by_label(&self, key: &str, value: &str) -> Option<&RosterEntry> {
        self.roster.find_by_label(key, value)
    }

    /// Get a specific roster entry.
    #[allow(dead_code)]
    pub(crate) fn entry(&self, agent_identity: &MeerkatId) -> Option<RosterEntry> {
        self.roster.get(agent_identity).cloned()
    }

    /// List all members currently `Active`.
    #[allow(dead_code)]
    pub(crate) fn active_members(&self) -> Vec<RosterEntry> {
        self.roster.list().cloned().collect()
    }

    /// List the members that have been marked `Retiring`.
    #[allow(dead_code)]
    pub(crate) fn retiring_members(&self) -> Vec<RosterEntry> {
        self.roster.list_retiring().cloned().collect()
    }

    #[allow(dead_code)]
    pub(crate) fn debug_assert_wiring_projection_consistent(&self) {
        self.roster.debug_assert_wiring_projection_consistent();
    }

    #[allow(dead_code)]
    pub(crate) fn set_bridge_session_id(
        &mut self,
        agent_identity: &MeerkatId,
        bridge_session_id: SessionId,
    ) -> bool {
        self.roster
            .set_bridge_session_id(agent_identity, bridge_session_id)
    }

    pub(crate) fn replace_backend_peer_binding_by_peer_id(
        &mut self,
        prior_peer_id: &str,
        next_peer_id: &str,
        next_address: &str,
        bootstrap_token: Option<meerkat_contracts::wire::supervisor_bridge::BridgeBootstrapToken>,
    ) -> Vec<(AgentIdentity, Generation)> {
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

    fn sync_retiring_projection(&mut self, retiring_runtime_ids: &BTreeSet<String>) {
        self.roster.sync_retiring_projection(retiring_runtime_ids);
    }

    fn remove_member(&mut self, agent_identity: &MeerkatId) -> bool {
        if self.roster.get(agent_identity).is_some() {
            self.roster.remove(agent_identity);
            true
        } else {
            false
        }
    }

    fn wire_members(&mut self, a: &MeerkatId, b: &MeerkatId) {
        self.roster.wire(a, b);
    }

    fn wire_external_peer(
        &mut self,
        local: &MeerkatId,
        peer_name: &MeerkatId,
        spec: TrustedPeerSpec,
    ) {
        self.roster.wire_external(local, peer_name, spec);
    }

    fn unwire_members(&mut self, a: &MeerkatId, b: &MeerkatId) {
        self.roster.unwire(a, b);
    }

    fn unwire_external_peer(&mut self, local: &MeerkatId, peer_name: &MeerkatId) {
        self.roster.unwire_external(local, peer_name);
    }

    fn set_kickoff(
        &mut self,
        agent_identity: &MeerkatId,
        kickoff: Option<MobMemberKickoffSnapshot>,
    ) -> bool {
        self.roster.set_kickoff(agent_identity, kickoff)
    }
}

//! Roster tracking for active meerkats in a mob.
//!
//! The `Roster` is a projection built from `MeerkatSpawned`, `MeerkatRetired`,
//! `PeersWired`, and `PeersUnwired` events.
//!
//! Projection contract:
//! - `Roster` is not canonical transport/comms trust truth.
//! - Canonical wiring side effects (trust edges, notifications, lock discipline)
//!   are performed by runtime orchestration.
//! - `Roster` stores the event/projected peer graph used for read surfaces and
//!   consistency checks.
//! - `wire`/`unwire`/`remove` are projection mutations only.

use crate::event::{MemberRef, MobEvent, MobEventKind};
use crate::ids::{AgentIdentity, AgentRuntimeId, FenceToken, Generation, MeerkatId, ProfileName};
use crate::runtime_mode::MobRuntimeMode;
use meerkat_core::comms::TrustedPeerSpec;
use meerkat_core::time_compat::SystemTime;
use meerkat_core::types::SessionId;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Lifecycle state for a roster member.
///
/// `Retiring` is runtime-only — event projection never produces it
/// (`MeerkatSpawned` creates `Active`; `MeerkatRetired` removes entirely).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum MemberState {
    #[default]
    Active,
    Retiring,
}

/// Resolution state for a member's initial autonomous kickoff turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MobMemberKickoffPhase {
    Pending,
    Starting,
    CallbackPending,
    Started,
    Failed,
    Cancelled,
}

/// Durable projected snapshot of a member's kickoff state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct MobMemberKickoffSnapshot {
    pub phase: MobMemberKickoffPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub updated_at: SystemTime,
}

/// A single member entry in the roster.
///
/// Public fields use the identity-native model (0.6). Bridge-level fields
/// (`meerkat_id`, `member_ref`, `peer_id`, `external_peer_specs`) are
/// `pub(crate)` for internal dispatch only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RosterEntry {
    // --- Identity-native public fields ---
    /// Stable member identity.
    pub agent_identity: AgentIdentity,
    /// Generation counter for this incarnation.
    pub generation: Generation,
    /// Fence token for stale-command rejection.
    pub fence_token: FenceToken,
    /// Composite runtime id.
    pub agent_runtime_id: AgentRuntimeId,
    /// Profile name this member was spawned from.
    pub role: ProfileName,
    /// Runtime mode for this member.
    #[serde(default)]
    pub runtime_mode: MobRuntimeMode,
    /// Lifecycle state (Active or Retiring).
    #[serde(default)]
    pub state: MemberState,
    /// Set of peer identities this member is wired to.
    pub wired_to: BTreeSet<AgentIdentity>,
    /// Application-defined labels for this member.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    /// Projected kickoff state for autonomous initial turn resolution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kickoff: Option<MobMemberKickoffSnapshot>,

    // --- Internal bridge fields (pub(crate)) ---
    /// Legacy meerkat identifier for bridge dispatch.
    pub(crate) meerkat_id: MeerkatId,
    /// Backend-neutral bridge identity.
    pub(crate) member_ref: MemberRef,
    /// Public comms peer identifier when this member exposes a comms runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) peer_id: Option<String>,
    /// Trusted specs for external peers keyed by their projected peer name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) external_peer_specs: BTreeMap<MeerkatId, TrustedPeerSpec>,
    /// Effective profile override from `SpawnTooling::Profile` resolution.
    ///
    /// When a member is spawned with an explicit tooling profile (inline or realm),
    /// the resolved profile is stored here so respawn/restore can use it instead
    /// of re-resolving from the definition. `None` means the definition profile
    /// (keyed by `self.role`) is authoritative.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_profile_override: Option<crate::profile::Profile>,
}

/// Directed projection presence state for an undirected peer edge.
///
/// This reflects only roster-projected `wired_to` membership; it does not
/// imply comms trust/runtime side effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WiringEdgeState {
    Absent,
    AOnly,
    BOnly,
    Bidirectional,
}

/// Parameters for adding a new member to the roster.
pub(crate) struct RosterAddEntry {
    pub(crate) agent_identity: AgentIdentity,
    pub(crate) generation: Generation,
    pub(crate) fence_token: FenceToken,
    pub(crate) agent_runtime_id: AgentRuntimeId,
    pub(crate) meerkat_id: MeerkatId,
    pub(crate) role: ProfileName,
    pub(crate) runtime_mode: MobRuntimeMode,
    pub(crate) member_ref: MemberRef,
    pub(crate) peer_id: Option<String>,
    pub(crate) labels: BTreeMap<String, String>,
    pub(crate) effective_profile_override: Option<crate::profile::Profile>,
}

/// Tracks active members and their wiring in a mob.
///
/// Built by replaying events. Shared via `Arc<RwLock<Roster>>` between
/// the actor (writes) and handle (reads).
///
/// The primary index is `AgentIdentity`. A secondary `meerkat_index`
/// maps legacy `MeerkatId` keys to the canonical identity for internal
/// bridge dispatch.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Roster {
    entries: BTreeMap<AgentIdentity, RosterEntry>,
    /// Reverse index: MeerkatId -> AgentIdentity for bridge lookup.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    meerkat_index: BTreeMap<MeerkatId, AgentIdentity>,
}

impl Roster {
    /// Create an empty roster.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a roster from a sequence of mob events.
    pub fn project(events: &[MobEvent]) -> Self {
        let mut roster = Self::new();
        for event in events {
            roster.apply(event);
        }
        roster
    }

    /// Apply a single event to update roster state.
    pub fn apply(&mut self, event: &MobEvent) {
        match &event.kind {
            MobEventKind::MeerkatSpawned {
                meerkat_id,
                role,
                runtime_mode,
                member_ref,
                labels,
            } => {
                // Legacy event: derive identity from meerkat_id.
                let identity = AgentIdentity::from(meerkat_id.as_str());
                self.add(RosterAddEntry {
                    agent_identity: identity.clone(),
                    generation: Generation::INITIAL,
                    fence_token: FenceToken::new(0),
                    agent_runtime_id: AgentRuntimeId::initial(identity),
                    meerkat_id: meerkat_id.clone(),
                    role: role.clone(),
                    runtime_mode: *runtime_mode,
                    member_ref: member_ref.clone(),
                    peer_id: None,
                    labels: labels.clone(),
                    effective_profile_override: None,
                });
            }
            MobEventKind::MemberSpawned {
                agent_identity,
                generation,
                fence_token,
                agent_runtime_id,
                role,
                runtime_mode,
                labels,
                bridge_member_ref,
            } => {
                let meerkat_id = MeerkatId::from(agent_identity.as_str());
                let member_ref = bridge_member_ref.clone().unwrap_or_else(|| {
                    MemberRef::from_bridge_session_id(SessionId::from_uuid(uuid::Uuid::nil()))
                });
                self.add(RosterAddEntry {
                    agent_identity: agent_identity.clone(),
                    generation: *generation,
                    fence_token: *fence_token,
                    agent_runtime_id: agent_runtime_id.clone(),
                    meerkat_id,
                    role: role.clone(),
                    runtime_mode: *runtime_mode,
                    member_ref,
                    peer_id: None,
                    labels: labels.clone(),
                    effective_profile_override: None,
                });
            }
            MobEventKind::MeerkatRetired { meerkat_id, .. } => {
                self.remove_by_meerkat_id(meerkat_id);
            }
            MobEventKind::MemberRetired { agent_identity, .. } => {
                self.remove_by_identity(agent_identity);
            }
            MobEventKind::MemberReset {
                agent_identity,
                new_generation,
                fence_token,
                agent_runtime_id,
                ..
            } => {
                if let Some(entry) = self.entries.get_mut(agent_identity) {
                    entry.generation = *new_generation;
                    entry.fence_token = *fence_token;
                    entry.agent_runtime_id = agent_runtime_id.clone();
                }
            }
            MobEventKind::PeersWired { a, b } => {
                self.wire(a, b);
            }
            MobEventKind::ExternalPeerWired { local, spec } => {
                let peer_name = MeerkatId::from(spec.name.clone());
                self.wire_external(local, &peer_name, spec.clone());
            }
            MobEventKind::ExternalPeerUnwired { local, peer_name } => {
                self.unwire_external(local, peer_name);
            }
            MobEventKind::PeersUnwired { a, b } => {
                self.unwire(a, b);
            }
            MobEventKind::MeerkatKickoffUpdated {
                meerkat_id,
                kickoff,
            } => {
                self.set_kickoff(meerkat_id, Some(kickoff.clone()));
            }
            MobEventKind::MobReset => {
                self.entries.clear();
                self.meerkat_index.clear();
            }
            _ => {}
        }
    }

    /// Add a member to the roster.
    pub(crate) fn add(&mut self, entry: RosterAddEntry) -> bool {
        let identity = entry.agent_identity.clone();
        let meerkat_id = entry.meerkat_id.clone();
        self.meerkat_index.insert(meerkat_id, identity.clone());
        self.entries
            .insert(
                identity,
                RosterEntry {
                    agent_identity: entry.agent_identity,
                    generation: entry.generation,
                    fence_token: entry.fence_token,
                    agent_runtime_id: entry.agent_runtime_id,
                    meerkat_id: entry.meerkat_id,
                    role: entry.role,
                    member_ref: entry.member_ref,
                    runtime_mode: entry.runtime_mode,
                    peer_id: entry.peer_id,
                    state: MemberState::default(),
                    wired_to: BTreeSet::new(),
                    external_peer_specs: BTreeMap::new(),
                    labels: entry.labels,
                    kickoff: None,
                    effective_profile_override: entry.effective_profile_override,
                },
            )
            .is_none()
    }

    /// Remove a member by AgentIdentity.
    pub fn remove_by_identity(&mut self, identity: &AgentIdentity) {
        if let Some(removed) = self.entries.remove(identity) {
            self.meerkat_index.remove(&removed.meerkat_id);
            // Remove this identity from all other entries' wired_to sets
            for entry in self.entries.values_mut() {
                entry.wired_to.remove(identity);
            }
        }
    }

    /// Remove a member by legacy MeerkatId (bridge lookup).
    pub fn remove_by_meerkat_id(&mut self, meerkat_id: &MeerkatId) {
        if let Some(identity) = self.meerkat_index.remove(meerkat_id) {
            self.remove_by_identity(&identity);
        }
    }

    /// Remove a member — delegates to bridge-based removal.
    pub fn remove(&mut self, meerkat_id: &MeerkatId) {
        self.remove_by_meerkat_id(meerkat_id);
    }

    /// Resolve a MeerkatId to AgentIdentity via the bridge index.
    pub(crate) fn resolve_identity(&self, meerkat_id: &MeerkatId) -> Option<&AgentIdentity> {
        self.meerkat_index.get(meerkat_id)
    }

    /// Get a roster entry by AgentIdentity.
    pub fn get_by_identity(&self, identity: &AgentIdentity) -> Option<&RosterEntry> {
        self.entries.get(identity)
    }

    /// Get a mutable roster entry by AgentIdentity.
    pub(crate) fn get_by_identity_mut(
        &mut self,
        identity: &AgentIdentity,
    ) -> Option<&mut RosterEntry> {
        self.entries.get_mut(identity)
    }

    /// Wire two members in the roster projection (bidirectional set update).
    ///
    /// Uses MeerkatId bridge lookup, then applies wiring at the identity level.
    pub fn wire(&mut self, a: &MeerkatId, b: &MeerkatId) {
        let id_a = self.meerkat_index.get(a).cloned();
        let id_b = self.meerkat_index.get(b).cloned();
        if let (Some(ref id_a), Some(ref id_b)) = (id_a, id_b) {
            if let Some(entry_a) = self.entries.get_mut(id_a) {
                entry_a.wired_to.insert(id_b.clone());
            }
            if let Some(entry_b) = self.entries.get_mut(id_b) {
                entry_b.wired_to.insert(id_a.clone());
            }
        }
    }

    /// Wire a local member to an external peer name.
    pub fn wire_external(
        &mut self,
        local: &MeerkatId,
        peer_name: &MeerkatId,
        spec: TrustedPeerSpec,
    ) {
        let local_id = self.meerkat_index.get(local).cloned();
        if let Some(ref local_id) = local_id {
            let peer_identity = AgentIdentity::from(peer_name.as_str());
            if let Some(entry) = self.entries.get_mut(local_id) {
                entry.wired_to.insert(peer_identity);
                entry.external_peer_specs.insert(peer_name.clone(), spec);
            }
        }
    }

    /// Unwire two members in the roster projection (bidirectional set update).
    pub fn unwire(&mut self, a: &MeerkatId, b: &MeerkatId) {
        let id_a = self.meerkat_index.get(a).cloned();
        let id_b = self.meerkat_index.get(b).cloned();
        if let (Some(ref id_a), Some(ref id_b)) = (id_a, id_b) {
            if let Some(entry_a) = self.entries.get_mut(id_a) {
                entry_a.wired_to.remove(id_b);
            }
            if let Some(entry_b) = self.entries.get_mut(id_b) {
                entry_b.wired_to.remove(id_a);
            }
        }
    }

    /// Unwire a local member from an external peer.
    pub fn unwire_external(&mut self, local: &MeerkatId, peer_name: &MeerkatId) {
        let local_id = self.meerkat_index.get(local).cloned();
        if let Some(ref local_id) = local_id {
            let peer_identity = AgentIdentity::from(peer_name.as_str());
            if let Some(entry) = self.entries.get_mut(local_id) {
                entry.wired_to.remove(&peer_identity);
                entry.external_peer_specs.remove(peer_name);
            }
        }
    }

    /// Returns true if both members currently have reciprocal projected edges.
    pub fn has_bidirectional_edge(&self, a: &MeerkatId, b: &MeerkatId) -> bool {
        matches!(self.wiring_edge_state(a, b), WiringEdgeState::Bidirectional)
    }

    /// Returns the directed projection presence state for edge `(a, b)`.
    pub fn wiring_edge_state(&self, a: &MeerkatId, b: &MeerkatId) -> WiringEdgeState {
        let id_a = self.meerkat_index.get(a);
        let id_b = self.meerkat_index.get(b);
        let a_has_b = id_a
            .and_then(|ia| self.entries.get(ia))
            .zip(id_b)
            .is_some_and(|(entry, ib)| entry.wired_to.contains(ib));
        let b_has_a = id_b
            .and_then(|ib| self.entries.get(ib))
            .zip(id_a)
            .is_some_and(|(entry, ia)| entry.wired_to.contains(ia));
        match (a_has_b, b_has_a) {
            (false, false) => WiringEdgeState::Absent,
            (true, false) => WiringEdgeState::AOnly,
            (false, true) => WiringEdgeState::BOnly,
            (true, true) => WiringEdgeState::Bidirectional,
        }
    }

    /// Returns `true` when every projected edge is reciprocal and endpoint-present.
    pub fn is_wiring_projection_consistent(&self) -> bool {
        self.wiring_projection_inconsistencies().is_empty()
    }

    /// Returns canonicalized endpoint pairs with projection inconsistencies.
    pub fn wiring_projection_inconsistencies(&self) -> Vec<(AgentIdentity, AgentIdentity)> {
        let mut inconsistencies = BTreeSet::<(AgentIdentity, AgentIdentity)>::new();
        for (a_id, a_entry) in &self.entries {
            for b_id in &a_entry.wired_to {
                if let Some(b_entry) = self.entries.get(b_id)
                    && !b_entry.wired_to.contains(a_id)
                {
                    let pair = if a_id <= b_id {
                        (a_id.clone(), b_id.clone())
                    } else {
                        (b_id.clone(), a_id.clone())
                    };
                    inconsistencies.insert(pair);
                }
            }
        }
        inconsistencies.into_iter().collect()
    }

    /// Debug-only assertion helper for projection consistency checks.
    pub fn debug_assert_wiring_projection_consistent(&self) {
        let _ = self;
        #[cfg(debug_assertions)]
        {
            let inconsistencies = self.wiring_projection_inconsistencies();
            debug_assert!(
                inconsistencies.is_empty(),
                "roster wiring projection is inconsistent: {inconsistencies:?}"
            );
        }
    }

    /// Get a roster entry by meerkat ID (bridge lookup).
    pub fn get(&self, meerkat_id: &MeerkatId) -> Option<&RosterEntry> {
        let identity = self.meerkat_index.get(meerkat_id)?;
        self.entries.get(identity)
    }

    /// Get a mutable roster entry by meerkat ID (bridge lookup).
    pub(crate) fn get_mut(&mut self, meerkat_id: &MeerkatId) -> Option<&mut RosterEntry> {
        let identity = self.meerkat_index.get(meerkat_id)?.clone();
        self.entries.get_mut(&identity)
    }

    /// Update the member reference for an existing meerkat.
    pub(crate) fn set_member_ref(&mut self, meerkat_id: &MeerkatId, member_ref: MemberRef) -> bool {
        if let Some(entry) = self.get_mut(meerkat_id) {
            entry.member_ref = member_ref;
            return true;
        }
        false
    }

    /// Update the bridge session ID while preserving backend-specific identity.
    pub fn set_bridge_session_id(
        &mut self,
        meerkat_id: &MeerkatId,
        bridge_session_id: SessionId,
    ) -> bool {
        if let Some(entry) = self.get_mut(meerkat_id) {
            entry.member_ref = match &entry.member_ref {
                MemberRef::Session { .. } => MemberRef::from_bridge_session_id(bridge_session_id),
                MemberRef::BackendPeer {
                    peer_id, address, ..
                } => MemberRef::BackendPeer {
                    peer_id: peer_id.clone(),
                    address: address.clone(),
                    session_id: Some(bridge_session_id),
                },
            };
            return true;
        }
        false
    }

    pub fn set_session_id(&mut self, meerkat_id: &MeerkatId, session_id: SessionId) -> bool {
        self.set_bridge_session_id(meerkat_id, session_id)
    }

    /// Update the resolved comms peer id for an existing meerkat.
    pub fn set_peer_id(&mut self, meerkat_id: &MeerkatId, peer_id: Option<String>) -> bool {
        if let Some(entry) = self.get_mut(meerkat_id) {
            entry.peer_id = peer_id;
            return true;
        }
        false
    }

    /// Update the projected kickoff state for an existing meerkat.
    pub fn set_kickoff(
        &mut self,
        meerkat_id: &MeerkatId,
        kickoff: Option<MobMemberKickoffSnapshot>,
    ) -> bool {
        if let Some(entry) = self.get_mut(meerkat_id) {
            entry.kickoff = kickoff;
            return true;
        }
        false
    }

    /// List active roster entries (excludes `Retiring`).
    pub fn list(&self) -> impl Iterator<Item = &RosterEntry> {
        self.entries
            .values()
            .filter(|e| e.state == MemberState::Active)
    }

    /// List all roster entries including `Retiring` members.
    pub fn list_all(&self) -> impl Iterator<Item = &RosterEntry> {
        self.entries.values()
    }

    /// List only `Retiring` members.
    pub fn list_retiring(&self) -> impl Iterator<Item = &RosterEntry> {
        self.entries
            .values()
            .filter(|e| e.state == MemberState::Retiring)
    }

    /// Find active members with a given profile name.
    pub fn by_profile(&self, profile: &ProfileName) -> impl Iterator<Item = &RosterEntry> {
        self.entries
            .values()
            .filter(move |e| e.role == *profile && e.state == MemberState::Active)
    }

    /// Find the first active member matching a label key-value pair.
    pub fn find_by_label(&self, key: &str, value: &str) -> Option<&RosterEntry> {
        self.entries.values().find(|e| {
            e.state == MemberState::Active && e.labels.get(key).is_some_and(|v| v == value)
        })
    }

    /// Find all active members matching a label key-value pair.
    pub fn find_all_by_label<'a>(
        &'a self,
        key: &'a str,
        value: &'a str,
    ) -> impl Iterator<Item = &'a RosterEntry> {
        self.entries.values().filter(move |e| {
            e.state == MemberState::Active && e.labels.get(key).is_some_and(|v| v == value)
        })
    }

    /// Look up the current bridge session ID for a meerkat.
    pub fn bridge_session_id(&self, meerkat_id: &MeerkatId) -> Option<&SessionId> {
        self.get(meerkat_id)?.member_ref.bridge_session_id()
    }

    pub fn session_id(&self, meerkat_id: &MeerkatId) -> Option<&SessionId> {
        self.bridge_session_id(meerkat_id)
    }

    /// Get the set of peer identities wired to a given meerkat.
    pub fn wired_peers_of(&self, meerkat_id: &MeerkatId) -> Option<&BTreeSet<AgentIdentity>> {
        self.get(meerkat_id).map(|e| &e.wired_to)
    }

    /// Number of active members in the roster.
    pub fn len(&self) -> usize {
        self.entries
            .values()
            .filter(|e| e.state == MemberState::Active)
            .count()
    }

    /// Whether the roster has no active members.
    pub fn is_empty(&self) -> bool {
        !self
            .entries
            .values()
            .any(|e| e.state == MemberState::Active)
    }

    /// Mark a member as `Retiring`. Returns `true` if the member was found and
    /// transitioned (i.e. it was `Active`); `false` otherwise.
    pub fn mark_retiring(&mut self, meerkat_id: &MeerkatId) -> bool {
        if let Some(entry) = self.get_mut(meerkat_id)
            && entry.state == MemberState::Active
        {
            entry.state = MemberState::Retiring;
            return true;
        }
        false
    }

    /// Verify that the meerkat_index is coherent with entries.
    ///
    /// Returns true if every meerkat_index key points to a valid entry and
    /// every entry's meerkat_id is in the index.
    pub fn is_index_coherent(&self) -> bool {
        // Every index entry must point to a valid roster entry
        for (mid, aid) in &self.meerkat_index {
            match self.entries.get(aid) {
                Some(entry) if entry.meerkat_id == *mid => {}
                _ => return false,
            }
        }
        // Every roster entry must have its meerkat_id in the index
        for entry in self.entries.values() {
            match self.meerkat_index.get(&entry.meerkat_id) {
                Some(aid) if *aid == entry.agent_identity => {}
                _ => return false,
            }
        }
        true
    }
}

impl RosterEntry {
    pub fn bridge_session_id(&self) -> Option<&SessionId> {
        self.member_ref.bridge_session_id()
    }

    pub fn session_id(&self) -> Option<&SessionId> {
        self.bridge_session_id()
    }

    /// Bridge-internal peer ID for comms wiring.
    pub fn peer_id(&self) -> Option<&str> {
        self.peer_id.as_deref()
    }

    /// Bridge-internal meerkat ID.
    pub fn meerkat_id(&self) -> &MeerkatId {
        &self.meerkat_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::MobId;
    use chrono::Utc;
    use meerkat_core::comms::TrustedPeerSpec;
    use uuid::Uuid;

    fn session_id() -> SessionId {
        SessionId::from_uuid(Uuid::new_v4())
    }

    /// Build a RosterAddEntry from legacy-style fields, deriving identity from meerkat_id.
    fn make_add_entry(
        meerkat_id: MeerkatId,
        profile: ProfileName,
        runtime_mode: MobRuntimeMode,
        member_ref: MemberRef,
        labels: BTreeMap<String, String>,
    ) -> RosterAddEntry {
        let identity = AgentIdentity::from(meerkat_id.as_str());
        RosterAddEntry {
            agent_identity: identity.clone(),
            generation: Generation::INITIAL,
            fence_token: FenceToken::new(0),
            agent_runtime_id: AgentRuntimeId::initial(identity),
            meerkat_id,
            role: profile,
            runtime_mode,
            member_ref,
            peer_id: None,
            labels,
            effective_profile_override: None,
        }
    }

    /// Test helper: adds a member with no labels.
    fn add_member(
        roster: &mut Roster,
        meerkat_id: MeerkatId,
        profile: ProfileName,
        runtime_mode: MobRuntimeMode,
        member_ref: MemberRef,
    ) -> bool {
        roster.add(make_add_entry(
            meerkat_id,
            profile,
            runtime_mode,
            member_ref,
            BTreeMap::new(),
        ))
    }

    fn make_event(cursor: u64, kind: MobEventKind) -> MobEvent {
        MobEvent {
            cursor,
            timestamp: Utc::now(),
            mob_id: MobId::from("test-mob"),
            kind,
        }
    }

    #[test]
    fn test_roster_add_and_get() {
        let mut roster = Roster::new();
        let sid = session_id();
        add_member(
            &mut roster,
            MeerkatId::from("agent-1"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(sid.clone()),
        );
        assert_eq!(roster.len(), 1);
        let entry = roster.get(&MeerkatId::from("agent-1")).unwrap();
        assert_eq!(entry.role.as_str(), "worker");
        assert_eq!(entry.session_id(), Some(&sid));
        assert!(entry.wired_to.is_empty());
    }

    #[test]
    fn test_roster_remove() {
        let mut roster = Roster::new();
        add_member(
            &mut roster,
            MeerkatId::from("agent-1"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );
        add_member(
            &mut roster,
            MeerkatId::from("agent-2"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );
        roster.wire(&MeerkatId::from("agent-1"), &MeerkatId::from("agent-2"));
        roster.remove(&MeerkatId::from("agent-1"));

        assert_eq!(roster.len(), 1);
        assert!(roster.get(&MeerkatId::from("agent-1")).is_none());
        // agent-2 should no longer have agent-1 in wired_to
        let entry2 = roster.get(&MeerkatId::from("agent-2")).unwrap();
        assert!(entry2.wired_to.is_empty());
    }

    #[test]
    fn test_roster_remove_nonexistent_is_noop() {
        let mut roster = Roster::new();
        roster.remove(&MeerkatId::from("nonexistent"));
        assert!(roster.is_empty());
    }

    #[test]
    fn test_set_bridge_session_id_preserves_backend_member_ref_identity() {
        let mut roster = Roster::new();
        let old_sid = session_id();
        add_member(
            &mut roster,
            MeerkatId::from("ext-1"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::BackendPeer {
                peer_id: "peer-ext-1".to_string(),
                address: "https://backend.example.invalid/mesh/ext-1".to_string(),
                session_id: Some(old_sid),
            },
        );

        let new_sid = session_id();
        assert!(roster.set_bridge_session_id(&MeerkatId::from("ext-1"), new_sid.clone()));
        let entry = roster
            .get(&MeerkatId::from("ext-1"))
            .expect("entry should remain present");
        match &entry.member_ref {
            MemberRef::BackendPeer {
                peer_id,
                address,
                session_id,
            } => {
                assert_eq!(peer_id, "peer-ext-1");
                assert_eq!(address, "https://backend.example.invalid/mesh/ext-1");
                assert_eq!(session_id.as_ref(), Some(&new_sid));
            }
            other => panic!("expected backend peer member ref, got {other:?}"),
        }
    }

    #[test]
    fn test_roster_wire_and_unwire() {
        let mut roster = Roster::new();
        add_member(
            &mut roster,
            MeerkatId::from("a"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );
        add_member(
            &mut roster,
            MeerkatId::from("b"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );

        roster.wire(&MeerkatId::from("a"), &MeerkatId::from("b"));

        let peers_a = roster.wired_peers_of(&MeerkatId::from("a")).unwrap();
        assert!(peers_a.contains(&AgentIdentity::from("b")));
        let peers_b = roster.wired_peers_of(&MeerkatId::from("b")).unwrap();
        assert!(peers_b.contains(&AgentIdentity::from("a")));

        roster.unwire(&MeerkatId::from("a"), &MeerkatId::from("b"));

        let peers_a = roster.wired_peers_of(&MeerkatId::from("a")).unwrap();
        assert!(peers_a.is_empty());
        let peers_b = roster.wired_peers_of(&MeerkatId::from("b")).unwrap();
        assert!(peers_b.is_empty());
    }

    #[test]
    fn test_roster_wire_idempotent() {
        let mut roster = Roster::new();
        add_member(
            &mut roster,
            MeerkatId::from("a"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );
        add_member(
            &mut roster,
            MeerkatId::from("b"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );

        roster.wire(&MeerkatId::from("a"), &MeerkatId::from("b"));
        roster.wire(&MeerkatId::from("a"), &MeerkatId::from("b"));

        let peers_a = roster.wired_peers_of(&MeerkatId::from("a")).unwrap();
        assert_eq!(peers_a.len(), 1); // No duplicates (BTreeSet)
    }

    #[test]
    fn test_roster_wire_external_treats_missing_peer_as_external() {
        let mut roster = Roster::new();
        add_member(
            &mut roster,
            MeerkatId::from("a"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );

        roster.wire_external(
            &MeerkatId::from("a"),
            &MeerkatId::from("remote-mob/worker/agent-b"),
            TrustedPeerSpec::new(
                "remote-mob/worker/agent-b",
                "ed25519:remote-b",
                "inproc://remote-mob/worker/agent-b",
            )
            .expect("valid trusted peer spec"),
        );

        let peers_a = roster.wired_peers_of(&MeerkatId::from("a")).unwrap();
        assert!(peers_a.contains(&AgentIdentity::from("remote-mob/worker/agent-b")));
        assert!(
            roster
                .get(&MeerkatId::from("a"))
                .expect("entry should exist")
                .external_peer_specs
                .contains_key(&MeerkatId::from("remote-mob/worker/agent-b"))
        );
        assert!(
            roster.is_wiring_projection_consistent(),
            "missing local peer should be treated as an external projection target"
        );
    }

    #[test]
    fn test_roster_by_profile() {
        let mut roster = Roster::new();
        add_member(
            &mut roster,
            MeerkatId::from("w1"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );
        add_member(
            &mut roster,
            MeerkatId::from("w2"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );
        add_member(
            &mut roster,
            MeerkatId::from("lead"),
            ProfileName::from("orchestrator"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );

        let workers: Vec<_> = roster.by_profile(&ProfileName::from("worker")).collect();
        assert_eq!(workers.len(), 2);

        let orchestrators: Vec<_> = roster
            .by_profile(&ProfileName::from("orchestrator"))
            .collect();
        assert_eq!(orchestrators.len(), 1);
    }

    #[test]
    fn test_roster_list() {
        let mut roster = Roster::new();
        add_member(
            &mut roster,
            MeerkatId::from("a"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );
        add_member(
            &mut roster,
            MeerkatId::from("b"),
            ProfileName::from("lead"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );

        let all: Vec<_> = roster.list().collect();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_roster_project_from_events() {
        let sid1 = session_id();
        let sid2 = session_id();
        let events = vec![
            make_event(
                1,
                MobEventKind::MeerkatSpawned {
                    meerkat_id: MeerkatId::from("a"),
                    role: ProfileName::from("worker"),
                    runtime_mode: MobRuntimeMode::AutonomousHost,
                    member_ref: MemberRef::from_session_id(sid1),
                    labels: BTreeMap::new(),
                },
            ),
            make_event(
                2,
                MobEventKind::MeerkatSpawned {
                    meerkat_id: MeerkatId::from("b"),
                    role: ProfileName::from("worker"),
                    runtime_mode: MobRuntimeMode::AutonomousHost,
                    member_ref: MemberRef::from_session_id(sid2),
                    labels: BTreeMap::new(),
                },
            ),
            make_event(
                3,
                MobEventKind::PeersWired {
                    a: MeerkatId::from("a"),
                    b: MeerkatId::from("b"),
                },
            ),
        ];
        let roster = Roster::project(&events);
        assert_eq!(roster.len(), 2);
        let peers_a = roster.wired_peers_of(&MeerkatId::from("a")).unwrap();
        assert!(peers_a.contains(&AgentIdentity::from("b")));
    }

    #[test]
    fn test_roster_project_with_retire() {
        let sid1 = session_id();
        let sid2 = session_id();
        let events = vec![
            make_event(
                1,
                MobEventKind::MeerkatSpawned {
                    meerkat_id: MeerkatId::from("a"),
                    role: ProfileName::from("worker"),
                    runtime_mode: MobRuntimeMode::AutonomousHost,
                    member_ref: MemberRef::from_session_id(sid1.clone()),
                    labels: BTreeMap::new(),
                },
            ),
            make_event(
                2,
                MobEventKind::MeerkatSpawned {
                    meerkat_id: MeerkatId::from("b"),
                    role: ProfileName::from("worker"),
                    runtime_mode: MobRuntimeMode::AutonomousHost,
                    member_ref: MemberRef::from_session_id(sid2.clone()),
                    labels: BTreeMap::new(),
                },
            ),
            make_event(
                3,
                MobEventKind::PeersWired {
                    a: MeerkatId::from("a"),
                    b: MeerkatId::from("b"),
                },
            ),
            make_event(
                4,
                MobEventKind::MeerkatRetired {
                    meerkat_id: MeerkatId::from("a"),
                    role: ProfileName::from("worker"),
                    member_ref: MemberRef::from_session_id(sid1),
                },
            ),
        ];
        let roster = Roster::project(&events);
        assert_eq!(roster.len(), 1);
        assert!(roster.get(&MeerkatId::from("a")).is_none());
        let peers_b = roster.wired_peers_of(&MeerkatId::from("b")).unwrap();
        assert!(peers_b.is_empty());
    }

    #[test]
    fn test_roster_project_idempotent() {
        let sid = session_id();
        let events = vec![make_event(
            1,
            MobEventKind::MeerkatSpawned {
                meerkat_id: MeerkatId::from("a"),
                role: ProfileName::from("worker"),
                runtime_mode: MobRuntimeMode::AutonomousHost,
                member_ref: MemberRef::from_session_id(sid),
                labels: BTreeMap::new(),
            },
        )];
        let roster1 = Roster::project(&events);
        let roster2 = Roster::project(&events);
        assert_eq!(roster1.len(), roster2.len());
        assert_eq!(
            roster1.get(&MeerkatId::from("a")).unwrap().role,
            roster2.get(&MeerkatId::from("a")).unwrap().role,
        );
    }

    #[test]
    fn test_roster_serde_entry_roundtrip() {
        let entry = RosterEntry {
            agent_identity: AgentIdentity::from("test"),
            generation: Generation::INITIAL,
            fence_token: FenceToken::new(0),
            agent_runtime_id: AgentRuntimeId::initial(AgentIdentity::from("test")),
            meerkat_id: MeerkatId::from("test"),
            role: ProfileName::from("worker"),
            member_ref: MemberRef::from_session_id(session_id()),
            runtime_mode: MobRuntimeMode::AutonomousHost,
            peer_id: None,
            state: MemberState::default(),
            wired_to: {
                let mut s = BTreeSet::new();
                s.insert(AgentIdentity::from("peer-1"));
                s
            },
            external_peer_specs: BTreeMap::new(),
            labels: BTreeMap::new(),
            kickoff: None,
            effective_profile_override: None,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: RosterEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.meerkat_id, entry.meerkat_id);
        assert_eq!(parsed.wired_to.len(), 1);
    }

    #[test]
    fn test_mark_retiring() {
        let mut roster = Roster::new();
        add_member(
            &mut roster,
            MeerkatId::from("a"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );
        assert!(roster.mark_retiring(&MeerkatId::from("a")));
        // Second call returns false (already Retiring)
        assert!(!roster.mark_retiring(&MeerkatId::from("a")));
        // Nonexistent returns false
        assert!(!roster.mark_retiring(&MeerkatId::from("nope")));
    }

    #[test]
    fn test_list_excludes_retiring() {
        let mut roster = Roster::new();
        add_member(
            &mut roster,
            MeerkatId::from("a"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );
        add_member(
            &mut roster,
            MeerkatId::from("b"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );
        roster.mark_retiring(&MeerkatId::from("a"));

        let active: Vec<_> = roster.list().collect();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].meerkat_id, MeerkatId::from("b"));
    }

    #[test]
    fn test_list_all_includes_retiring() {
        let mut roster = Roster::new();
        add_member(
            &mut roster,
            MeerkatId::from("a"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );
        add_member(
            &mut roster,
            MeerkatId::from("b"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );
        roster.mark_retiring(&MeerkatId::from("a"));

        let all: Vec<_> = roster.list_all().collect();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_list_retiring_only() {
        let mut roster = Roster::new();
        add_member(
            &mut roster,
            MeerkatId::from("a"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );
        add_member(
            &mut roster,
            MeerkatId::from("b"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );
        roster.mark_retiring(&MeerkatId::from("a"));

        let retiring: Vec<_> = roster.list_retiring().collect();
        assert_eq!(retiring.len(), 1);
        assert_eq!(retiring[0].meerkat_id, MeerkatId::from("a"));
    }

    #[test]
    fn test_len_and_is_empty_count_active_only() {
        let mut roster = Roster::new();
        assert!(roster.is_empty());
        assert_eq!(roster.len(), 0);

        add_member(
            &mut roster,
            MeerkatId::from("a"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );
        assert_eq!(roster.len(), 1);
        assert!(!roster.is_empty());

        roster.mark_retiring(&MeerkatId::from("a"));
        assert_eq!(roster.len(), 0);
        assert!(roster.is_empty());
    }

    #[test]
    fn test_by_profile_excludes_retiring() {
        let mut roster = Roster::new();
        add_member(
            &mut roster,
            MeerkatId::from("w1"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );
        add_member(
            &mut roster,
            MeerkatId::from("w2"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );
        roster.mark_retiring(&MeerkatId::from("w1"));

        let workers: Vec<_> = roster.by_profile(&ProfileName::from("worker")).collect();
        assert_eq!(workers.len(), 1);
        assert_eq!(workers[0].meerkat_id, MeerkatId::from("w2"));
    }

    #[test]
    fn test_get_returns_retiring() {
        let mut roster = Roster::new();
        add_member(
            &mut roster,
            MeerkatId::from("a"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );
        roster.mark_retiring(&MeerkatId::from("a"));

        let entry = roster.get(&MeerkatId::from("a"));
        assert!(entry.is_some());
        assert_eq!(entry.unwrap().state, MemberState::Retiring);
    }

    #[test]
    fn test_serde_roundtrip_with_state_field() {
        let entry = RosterEntry {
            agent_identity: AgentIdentity::from("test"),
            generation: Generation::INITIAL,
            fence_token: FenceToken::new(0),
            agent_runtime_id: AgentRuntimeId::initial(AgentIdentity::from("test")),
            meerkat_id: MeerkatId::from("test"),
            role: ProfileName::from("worker"),
            member_ref: MemberRef::from_session_id(session_id()),
            runtime_mode: MobRuntimeMode::AutonomousHost,
            peer_id: None,
            state: MemberState::Active,
            wired_to: BTreeSet::new(),
            external_peer_specs: BTreeMap::new(),
            labels: BTreeMap::new(),
            kickoff: None,
            effective_profile_override: None,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: RosterEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.state, MemberState::Active);
    }

    #[test]
    fn test_session_id_convenience_session_member() {
        let mut roster = Roster::new();
        let sid = session_id();
        add_member(
            &mut roster,
            MeerkatId::from("a"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(sid.clone()),
        );
        assert_eq!(roster.bridge_session_id(&MeerkatId::from("a")), Some(&sid));
        assert_eq!(roster.session_id(&MeerkatId::from("a")), Some(&sid));
    }

    #[test]
    fn test_session_id_convenience_backend_peer_with_bridge() {
        let mut roster = Roster::new();
        let sid = session_id();
        add_member(
            &mut roster,
            MeerkatId::from("ext-1"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::BackendPeer {
                peer_id: "peer-ext-1".to_string(),
                address: "https://backend.example.invalid/mesh/ext-1".to_string(),
                session_id: Some(sid.clone()),
            },
        );
        assert_eq!(
            roster.bridge_session_id(&MeerkatId::from("ext-1")),
            Some(&sid)
        );
        assert_eq!(roster.session_id(&MeerkatId::from("ext-1")), Some(&sid));
    }

    #[test]
    fn test_session_id_convenience_backend_peer_no_bridge() {
        let mut roster = Roster::new();
        add_member(
            &mut roster,
            MeerkatId::from("ext-2"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::BackendPeer {
                peer_id: "peer-ext-2".to_string(),
                address: "https://backend.example.invalid/mesh/ext-2".to_string(),
                session_id: None,
            },
        );
        assert_eq!(roster.session_id(&MeerkatId::from("ext-2")), None);
    }

    #[test]
    fn test_session_id_convenience_not_found() {
        let roster = Roster::new();
        assert_eq!(roster.session_id(&MeerkatId::from("nonexistent")), None);
    }

    #[test]
    fn test_serde_roundtrip_missing_state_defaults_to_active() {
        // Simulate serialized data without the optional state field — state should default to Active.
        let json = r#"{"agent_identity":"old","generation":0,"fence_token":0,"agent_runtime_id":{"identity":"old","generation":0},"meerkat_id":"old","role":"worker","member_ref":{"kind":"session","session_id":"00000000-0000-0000-0000-000000000001"},"runtime_mode":"autonomous_host","wired_to":[]}"#;
        let parsed: RosterEntry = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.state, MemberState::Active);
    }

    #[test]
    fn test_project_never_produces_retiring() {
        let sid = session_id();
        let events = vec![make_event(
            1,
            MobEventKind::MeerkatSpawned {
                meerkat_id: MeerkatId::from("a"),
                role: ProfileName::from("worker"),
                runtime_mode: MobRuntimeMode::AutonomousHost,
                member_ref: MemberRef::from_session_id(sid),
                labels: BTreeMap::new(),
            },
        )];
        let roster = Roster::project(&events);
        let entry = roster.get(&MeerkatId::from("a")).unwrap();
        assert_eq!(entry.state, MemberState::Active);
    }

    #[test]
    fn test_roster_labels_populated_from_event() {
        let sid = session_id();
        let mut labels = BTreeMap::new();
        labels.insert("faction".to_string(), "north".to_string());
        labels.insert("tier".to_string(), "1".to_string());
        let events = vec![make_event(
            1,
            MobEventKind::MeerkatSpawned {
                meerkat_id: MeerkatId::from("a"),
                role: ProfileName::from("worker"),
                runtime_mode: MobRuntimeMode::AutonomousHost,
                member_ref: MemberRef::from_session_id(sid),
                labels: labels.clone(),
            },
        )];
        let roster = Roster::project(&events);
        let entry = roster.get(&MeerkatId::from("a")).unwrap();
        assert_eq!(entry.labels, labels);
    }

    #[test]
    fn test_find_by_label_returns_active_member() {
        let mut roster = Roster::new();
        roster.add(make_add_entry(
            MeerkatId::from("a"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
            {
                let mut m = BTreeMap::new();
                m.insert("faction".to_string(), "north".to_string());
                m
            },
        ));
        roster.add(make_add_entry(
            MeerkatId::from("b"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
            {
                let mut m = BTreeMap::new();
                m.insert("faction".to_string(), "south".to_string());
                m
            },
        ));
        let found = roster.find_by_label("faction", "north");
        assert!(found.is_some());
        assert_eq!(found.unwrap().meerkat_id, MeerkatId::from("a"));
    }

    #[test]
    fn test_find_all_by_label_returns_all_matching() {
        let mut roster = Roster::new();
        roster.add(make_add_entry(
            MeerkatId::from("a"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
            {
                let mut m = BTreeMap::new();
                m.insert("tier".to_string(), "1".to_string());
                m
            },
        ));
        roster.add(make_add_entry(
            MeerkatId::from("b"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
            {
                let mut m = BTreeMap::new();
                m.insert("tier".to_string(), "1".to_string());
                m
            },
        ));
        roster.add(make_add_entry(
            MeerkatId::from("c"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
            {
                let mut m = BTreeMap::new();
                m.insert("tier".to_string(), "2".to_string());
                m
            },
        ));
        let found: Vec<_> = roster.find_all_by_label("tier", "1").collect();
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn test_find_by_label_excludes_retiring() {
        let mut roster = Roster::new();
        roster.add(make_add_entry(
            MeerkatId::from("a"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
            {
                let mut m = BTreeMap::new();
                m.insert("faction".to_string(), "north".to_string());
                m
            },
        ));
        roster.mark_retiring(&MeerkatId::from("a"));
        assert!(roster.find_by_label("faction", "north").is_none());
        assert_eq!(roster.find_all_by_label("faction", "north").count(), 0);
    }

    #[test]
    fn test_roster_entry_roundtrip_json() {
        let mut roster = Roster::new();
        add_member(
            &mut roster,
            MeerkatId::from("a"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );
        let entry = roster.get(&MeerkatId::from("a")).unwrap();
        let json = serde_json::to_string(entry).unwrap();
        let parsed: RosterEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.agent_identity, AgentIdentity::from("a"));
        assert_eq!(parsed.generation, Generation::INITIAL);
        assert_eq!(parsed.role.as_str(), "worker");
        assert!(parsed.labels.is_empty());
    }

    // --- Index coherence tests ---

    #[test]
    fn test_index_coherent_after_add_and_remove() {
        let mut roster = Roster::new();
        add_member(
            &mut roster,
            MeerkatId::from("a"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );
        add_member(
            &mut roster,
            MeerkatId::from("b"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );
        assert!(roster.is_index_coherent());

        roster.remove(&MeerkatId::from("a"));
        assert!(roster.is_index_coherent());
        assert!(roster.get(&MeerkatId::from("a")).is_none());
        assert!(roster.get_by_identity(&AgentIdentity::from("a")).is_none());
    }

    #[test]
    fn test_index_coherent_after_event_projection() {
        let events = vec![
            make_event(
                1,
                MobEventKind::MeerkatSpawned {
                    meerkat_id: MeerkatId::from("x"),
                    role: ProfileName::from("worker"),
                    runtime_mode: MobRuntimeMode::AutonomousHost,
                    member_ref: MemberRef::from_session_id(session_id()),
                    labels: BTreeMap::new(),
                },
            ),
            make_event(
                2,
                MobEventKind::MeerkatSpawned {
                    meerkat_id: MeerkatId::from("y"),
                    role: ProfileName::from("lead"),
                    runtime_mode: MobRuntimeMode::AutonomousHost,
                    member_ref: MemberRef::from_session_id(session_id()),
                    labels: BTreeMap::new(),
                },
            ),
        ];
        let roster = Roster::project(&events);
        assert!(roster.is_index_coherent());
        assert_eq!(roster.len(), 2);

        // Verify dual-index access
        let by_mid = roster.get(&MeerkatId::from("x")).unwrap();
        let by_id = roster.get_by_identity(&AgentIdentity::from("x")).unwrap();
        assert_eq!(by_mid.agent_identity, by_id.agent_identity);
    }

    #[test]
    fn test_index_coherent_after_identity_native_events() {
        let identity = AgentIdentity::from("researcher");
        let events = vec![make_event(
            1,
            MobEventKind::MemberSpawned {
                agent_identity: identity.clone(),
                generation: Generation::INITIAL,
                fence_token: FenceToken::new(1),
                agent_runtime_id: AgentRuntimeId::initial(identity.clone()),
                role: ProfileName::from("research"),
                runtime_mode: MobRuntimeMode::AutonomousHost,
                labels: BTreeMap::new(),
                bridge_member_ref: None,
            },
        )];
        let roster = Roster::project(&events);
        assert!(roster.is_index_coherent());
        assert_eq!(roster.len(), 1);

        let entry = roster.get_by_identity(&identity).unwrap();
        assert_eq!(entry.generation, Generation::INITIAL);
        assert_eq!(entry.fence_token, FenceToken::new(1));
    }

    #[test]
    fn test_member_reset_updates_generation() {
        let identity = AgentIdentity::from("worker-1");
        let events = vec![
            make_event(
                1,
                MobEventKind::MemberSpawned {
                    agent_identity: identity.clone(),
                    generation: Generation::INITIAL,
                    fence_token: FenceToken::new(1),
                    agent_runtime_id: AgentRuntimeId::initial(identity.clone()),
                    role: ProfileName::from("worker"),
                    runtime_mode: MobRuntimeMode::AutonomousHost,
                    labels: BTreeMap::new(),
                    bridge_member_ref: None,
                },
            ),
            make_event(
                2,
                MobEventKind::MemberReset {
                    agent_identity: identity.clone(),
                    previous_generation: Generation::INITIAL,
                    new_generation: Generation::new(1),
                    fence_token: FenceToken::new(2),
                    agent_runtime_id: AgentRuntimeId::new(identity.clone(), Generation::new(1)),
                },
            ),
        ];
        let roster = Roster::project(&events);
        assert!(roster.is_index_coherent());
        let entry = roster.get_by_identity(&identity).unwrap();
        assert_eq!(entry.generation, Generation::new(1));
        assert_eq!(entry.fence_token, FenceToken::new(2));
    }

    #[test]
    fn test_resolve_identity_returns_correct_mapping() {
        let mut roster = Roster::new();
        add_member(
            &mut roster,
            MeerkatId::from("mid-1"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );
        assert_eq!(
            roster.resolve_identity(&MeerkatId::from("mid-1")),
            Some(&AgentIdentity::from("mid-1"))
        );
        assert_eq!(
            roster.resolve_identity(&MeerkatId::from("nonexistent")),
            None
        );
    }

    #[test]
    fn test_mob_reset_clears_both_indices() {
        let events = vec![
            make_event(
                1,
                MobEventKind::MeerkatSpawned {
                    meerkat_id: MeerkatId::from("a"),
                    role: ProfileName::from("worker"),
                    runtime_mode: MobRuntimeMode::AutonomousHost,
                    member_ref: MemberRef::from_session_id(session_id()),
                    labels: BTreeMap::new(),
                },
            ),
            make_event(2, MobEventKind::MobReset),
        ];
        let roster = Roster::project(&events);
        assert!(roster.is_empty());
        assert!(roster.is_index_coherent());
        assert!(roster.resolve_identity(&MeerkatId::from("a")).is_none());
    }
}

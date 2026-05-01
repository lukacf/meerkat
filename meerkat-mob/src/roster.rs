//! Roster tracking for active mob members.
//!
//! The `Roster` is a projection built from `MemberSpawned` and `MemberRetired`
//! events.
//!
//! Projection contract:
//! - `Roster` is not canonical transport/comms trust truth.
//! - Canonical wiring side effects (trust edges, notifications, lock discipline)
//!   are performed by runtime orchestration.
//! - `Roster` stores the event/projected peer graph used for read surfaces and
//!   consistency checks.
//! - `wire`/`unwire`/`remove` are projection mutations only.

use crate::event::{MemberRef, MobEvent, MobEventKind};
use crate::ids::{AgentIdentity, AgentRuntimeId, FenceToken, Generation, ProfileName};
use crate::runtime_mode::MobRuntimeMode;
use meerkat_core::comms::{PeerId, TrustedPeerDescriptor};
use meerkat_core::time_compat::SystemTime;
use meerkat_core::types::SessionId;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeMap, BTreeSet};

fn deserialize_optional_peer_id<'de, D>(deserializer: D) -> Result<Option<PeerId>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Option::<String>::deserialize(deserializer)?;
    Ok(raw.and_then(|value| PeerId::parse(&value).ok()))
}

/// Lifecycle state for a roster member.
///
/// `Retiring` is runtime-only — event projection never produces it
/// (`MemberSpawned` creates `Active`; `MemberRetired` removes entirely).
///
/// As of phase 5G, authority over `Retiring` is owned by the `MobMachine`
/// DSL via its `member_state_markers` map.
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
/// (`member_ref`, `peer_id`, `external_peer_specs`) are
/// `pub(crate)` for internal dispatch only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RosterEntry {
    // --- Identity-native public fields ---
    /// Stable member identity.
    #[serde(alias = "meerkat_id")]
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
    /// Backend-neutral bridge identity.
    pub(crate) member_ref: MemberRef,
    /// Canonical comms peer identifier when this member exposes a comms runtime.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_peer_id"
    )]
    pub(crate) peer_id: Option<PeerId>,
    /// Transport/auth signing public key for this member's comms runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) transport_public_key: Option<String>,
    /// Trusted specs for external peers keyed by their projected peer name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) external_peer_specs: BTreeMap<AgentIdentity, TrustedPeerDescriptor>,
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
/// Parameters for adding a new member to the roster.
pub(crate) struct RosterAddEntry {
    pub(crate) agent_identity: AgentIdentity,
    pub(crate) generation: Generation,
    pub(crate) fence_token: FenceToken,
    pub(crate) agent_runtime_id: AgentRuntimeId,
    pub(crate) role: ProfileName,
    pub(crate) runtime_mode: MobRuntimeMode,
    pub(crate) member_ref: MemberRef,
    pub(crate) peer_id: Option<PeerId>,
    pub(crate) transport_public_key: Option<String>,
    pub(crate) labels: BTreeMap<String, String>,
    pub(crate) effective_profile_override: Option<crate::profile::Profile>,
}

/// Tracks active members and their wiring in a mob.
///
/// Built by replaying events. Shared via `Arc<RwLock<Roster>>` between
/// the actor (writes) and handle (reads).
///
/// Keyed by `AgentIdentity`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Roster {
    entries: BTreeMap<AgentIdentity, RosterEntry>,
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
            MobEventKind::MemberSpawned(member_spawned) => {
                let member_ref = member_spawned.bridge_member_ref.clone().unwrap_or_else(|| {
                    MemberRef::from_bridge_session_id(SessionId::from_uuid(uuid::Uuid::nil()))
                });
                self.add(RosterAddEntry {
                    agent_identity: member_spawned.agent_identity.clone(),
                    generation: member_spawned.generation,
                    fence_token: member_spawned.fence_token,
                    agent_runtime_id: member_spawned.agent_runtime_id.clone(),
                    role: member_spawned.role.clone(),
                    runtime_mode: member_spawned.runtime_mode,
                    member_ref,
                    peer_id: None,
                    transport_public_key: None,
                    labels: member_spawned.labels.clone(),
                    effective_profile_override: None,
                });
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
            MobEventKind::MemberKickoffUpdated { member, kickoff } => {
                self.set_kickoff_by_identity(member, Some(kickoff.clone()));
            }
            MobEventKind::MembersWired { a, b } => {
                // Project wiring edge into both endpoints' `wired_to` sets.
                // Restored as part of #29 D-roster-wiring-projection (the
                // edges were DSL-emit observability restored by #27; the
                // Roster projection was the missing half that left
                // `wired_to` empty for all readers).
                if let Some(entry) = self.entries.get_mut(a) {
                    entry.wired_to.insert(b.clone());
                }
                if let Some(entry) = self.entries.get_mut(b) {
                    entry.wired_to.insert(a.clone());
                }
            }
            MobEventKind::MembersUnwired { a, b } => {
                if let Some(entry) = self.entries.get_mut(a) {
                    entry.wired_to.remove(b);
                }
                if let Some(entry) = self.entries.get_mut(b) {
                    entry.wired_to.remove(a);
                }
            }
            MobEventKind::ExternalPeerWired { local, spec } => {
                // External wire: project the descriptor into the local
                // member's `external_peer_specs` and add the external
                // peer's name (as AgentIdentity) to `wired_to`. Resume
                // path replays these events to reinstate trust without
                // consulting a live comms runtime.
                if let Err(reason) =
                    TrustedPeerDescriptor::validate_pubkey_for_peer_id(spec.peer_id, &spec.pubkey)
                {
                    tracing::warn!(
                        local = %local,
                        peer_id = %spec.peer_id,
                        peer_name = %spec.name,
                        reason = %reason,
                        "skipping invalid ExternalPeerWired descriptor during roster projection"
                    );
                    return;
                }
                if let Some(entry) = self.entries.get_mut(local) {
                    let external_identity = AgentIdentity::from(spec.name.as_str());
                    entry.wired_to.insert(external_identity.clone());
                    entry
                        .external_peer_specs
                        .insert(external_identity, spec.clone());
                }
            }
            MobEventKind::ExternalPeerUnwired { local, peer_name } => {
                if let Some(entry) = self.entries.get_mut(local) {
                    let external_identity = AgentIdentity::from(peer_name.as_str());
                    entry.wired_to.remove(&external_identity);
                    entry.external_peer_specs.remove(&external_identity);
                }
            }
            MobEventKind::MobReset => {
                self.entries.clear();
            }
            _ => {}
        }
    }

    /// Add a member to the roster.
    pub(crate) fn add(&mut self, entry: RosterAddEntry) -> bool {
        let identity = entry.agent_identity.clone();
        self.entries
            .insert(
                identity,
                RosterEntry {
                    agent_identity: entry.agent_identity,
                    generation: entry.generation,
                    fence_token: entry.fence_token,
                    agent_runtime_id: entry.agent_runtime_id,
                    role: entry.role,
                    member_ref: entry.member_ref,
                    runtime_mode: entry.runtime_mode,
                    peer_id: entry.peer_id,
                    transport_public_key: entry.transport_public_key,
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
        if self.entries.remove(identity).is_some() {
            // Remove this identity from all other entries' wired_to sets
            for entry in self.entries.values_mut() {
                entry.wired_to.remove(identity);
            }
        }
    }

    /// Remove a member — delegates to identity-based removal.
    pub(crate) fn remove(&mut self, agent_identity: &AgentIdentity) {
        self.remove_by_identity(agent_identity);
    }

    /// Flip a roster entry's `state` to [`MemberState::Retiring`].
    ///
    /// Invoked by the retire pipeline (MobActor::handle_retire_inner) before
    /// the disposal pipeline starts so that live readers of
    /// `list_members()` / `list_all_members()` / `member_status()` observe
    /// the Retiring state while disposal (notify peers, archive session) is
    /// still in flight. Returns `true` when the member existed and was
    /// flipped; `false` when absent or already Retiring.
    ///
    /// The canonical removal still happens in the finally block of
    /// `dispose_member` via `dispose_remove_from_roster`, which calls
    /// `remove_by_identity`. This helper is the observability seam for the
    /// window between retire-event-appended and roster-entry-removed.
    pub(crate) fn mark_retiring_by_identity(&mut self, identity: &AgentIdentity) -> bool {
        match self.entries.get_mut(identity) {
            Some(entry) if entry.state != MemberState::Retiring => {
                entry.state = MemberState::Retiring;
                true
            }
            _ => false,
        }
    }

    /// Resolve an identity — returns the canonical identity if present.
    #[cfg(test)]
    pub(crate) fn resolve_identity(
        &self,
        agent_identity: &AgentIdentity,
    ) -> Option<&AgentIdentity> {
        self.entries.get(agent_identity).map(|e| &e.agent_identity)
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

    /// Get a roster entry by identity.
    #[doc(hidden)]
    pub fn get(&self, agent_identity: &AgentIdentity) -> Option<&RosterEntry> {
        self.entries.get(agent_identity)
    }

    /// Get a mutable roster entry by identity.
    pub(crate) fn get_mut(&mut self, agent_identity: &AgentIdentity) -> Option<&mut RosterEntry> {
        self.entries.get_mut(agent_identity)
    }

    /// Update the bridge session ID while preserving backend-specific identity.
    pub(crate) fn set_bridge_session_id(
        &mut self,
        agent_identity: &AgentIdentity,
        bridge_session_id: SessionId,
    ) -> bool {
        if let Some(entry) = self.get_mut(agent_identity) {
            entry.member_ref = match &entry.member_ref {
                MemberRef::Session { .. } => MemberRef::from_bridge_session_id(bridge_session_id),
                MemberRef::BackendPeer {
                    peer_id,
                    address,
                    pubkey,
                    bootstrap_token,
                    ..
                } => MemberRef::BackendPeer {
                    peer_id: peer_id.clone(),
                    address: address.clone(),
                    pubkey: *pubkey,
                    bootstrap_token: bootstrap_token.clone(),
                    session_id: Some(bridge_session_id),
                },
            };
            return true;
        }
        false
    }

    /// Update the resolved comms identity for an existing meerkat.
    pub(crate) fn set_comms_identity(
        &mut self,
        agent_identity: &AgentIdentity,
        peer_id: Option<PeerId>,
        transport_public_key: Option<String>,
    ) -> bool {
        if let Some(entry) = self.get_mut(agent_identity) {
            entry.peer_id = peer_id;
            entry.transport_public_key = transport_public_key;
            return true;
        }
        false
    }

    /// Update the projected kickoff state for an existing meerkat.
    pub fn set_kickoff(
        &mut self,
        agent_identity: &AgentIdentity,
        kickoff: Option<MobMemberKickoffSnapshot>,
    ) -> bool {
        if let Some(entry) = self.get_mut(agent_identity) {
            entry.kickoff = kickoff;
            return true;
        }
        false
    }

    /// Update the projected kickoff state for an existing member by identity.
    pub fn set_kickoff_by_identity(
        &mut self,
        identity: &AgentIdentity,
        kickoff: Option<MobMemberKickoffSnapshot>,
    ) -> bool {
        if let Some(entry) = self.get_by_identity_mut(identity) {
            entry.kickoff = kickoff;
            return true;
        }
        false
    }

    /// Replace the projected peer-only binding for all entries currently using a
    /// given remote peer id. Returns the identities and generations that changed.
    pub fn replace_backend_peer_binding_by_peer_id(
        &mut self,
        prior_peer_id: &str,
        next_peer_id: &str,
        next_address: &str,
        bootstrap_token: Option<meerkat_contracts::wire::supervisor_bridge::BridgeBootstrapToken>,
    ) -> Vec<(AgentIdentity, Generation, Option<[u8; 32]>)> {
        let mut updated = Vec::new();
        for entry in self.entries.values_mut() {
            match &entry.member_ref {
                MemberRef::BackendPeer {
                    peer_id,
                    pubkey,
                    session_id: None,
                    ..
                } if peer_id == prior_peer_id => {
                    let pubkey = *pubkey;
                    entry.member_ref = MemberRef::BackendPeer {
                        peer_id: next_peer_id.to_string(),
                        address: next_address.to_string(),
                        pubkey,
                        bootstrap_token: bootstrap_token.clone(),
                        session_id: None,
                    };
                    updated.push((entry.agent_identity.clone(), entry.generation, pubkey));
                }
                _ => {}
            }
        }
        updated
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

    /// Find the first entry whose bridge session matches `session_id`.
    pub fn find_by_bridge_session_id(&self, session_id: &SessionId) -> Option<&RosterEntry> {
        self.entries.values().find(|e| {
            e.member_ref
                .bridge_session_id()
                .is_some_and(|sid| sid == session_id)
        })
    }

    /// Returns `true` if any entry has a matching bridge session ID.
    pub fn has_bridge_session(&self, session_id: &SessionId) -> bool {
        self.find_by_bridge_session_id(session_id).is_some()
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

    /// Verify that each entry's key matches its `agent_identity` field.
    pub fn is_index_coherent(&self) -> bool {
        self.entries
            .iter()
            .all(|(key, entry)| *key == entry.agent_identity)
    }
}

impl RosterEntry {
    /// Bridge session ID for this entry's backing session.
    #[doc(hidden)]
    pub fn bridge_session_id(&self) -> Option<&SessionId> {
        self.member_ref.bridge_session_id()
    }

    /// Canonical comms peer ID for this roster entry.
    pub fn peer_id(&self) -> Option<PeerId> {
        self.peer_id
    }

    /// Transport/auth public key for comms wiring, if known.
    pub fn transport_public_key(&self) -> Option<&str> {
        self.transport_public_key.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{MeerkatId, MobId};
    use chrono::Utc;
    use uuid::Uuid;

    fn session_id() -> SessionId {
        SessionId::from_uuid(Uuid::new_v4())
    }

    /// Build a RosterAddEntry.
    fn make_add_entry(
        agent_identity: AgentIdentity,
        profile: ProfileName,
        runtime_mode: MobRuntimeMode,
        member_ref: MemberRef,
        labels: BTreeMap<String, String>,
    ) -> RosterAddEntry {
        RosterAddEntry {
            agent_identity: agent_identity.clone(),
            generation: Generation::INITIAL,
            fence_token: FenceToken::new(0),
            agent_runtime_id: AgentRuntimeId::initial(agent_identity),
            role: profile,
            runtime_mode,
            member_ref,
            peer_id: None,
            transport_public_key: None,
            labels,
            effective_profile_override: None,
        }
    }

    /// Test helper: adds a member with no labels.
    fn add_member(
        roster: &mut Roster,
        agent_identity: AgentIdentity,
        profile: ProfileName,
        runtime_mode: MobRuntimeMode,
        member_ref: MemberRef,
    ) -> bool {
        roster.add(make_add_entry(
            agent_identity,
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

    fn spawned_kind(
        agent_identity: &str,
        role: &str,
        runtime_mode: MobRuntimeMode,
        member_ref: MemberRef,
        labels: BTreeMap<String, String>,
    ) -> MobEventKind {
        let agent_identity = AgentIdentity::from(agent_identity);
        let mut event = crate::event::MemberSpawnedEvent::new(
            agent_identity.clone(),
            Generation::INITIAL,
            FenceToken::new(0),
            AgentRuntimeId::initial(agent_identity),
            ProfileName::from(role),
        )
        .with_bridge_member_ref(Some(member_ref));
        event.runtime_mode = runtime_mode;
        event.labels = labels;
        MobEventKind::MemberSpawned(event)
    }

    fn retired_kind(agent_identity: &str, role: &str) -> MobEventKind {
        MobEventKind::MemberRetired {
            agent_identity: AgentIdentity::from(agent_identity),
            generation: Generation::INITIAL,
            role: ProfileName::from(role),
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
            MemberRef::from_bridge_session_id(sid.clone()),
        );
        assert_eq!(roster.len(), 1);
        let entry = roster.get(&MeerkatId::from("agent-1")).unwrap();
        assert_eq!(entry.role.as_str(), "worker");
        assert_eq!(entry.bridge_session_id(), Some(&sid));
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
            MemberRef::from_bridge_session_id(session_id()),
        );
        add_member(
            &mut roster,
            MeerkatId::from("agent-2"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_bridge_session_id(session_id()),
        );
        roster.remove(&MeerkatId::from("agent-1"));

        assert_eq!(roster.len(), 1);
        assert!(roster.get(&MeerkatId::from("agent-1")).is_none());
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
                pubkey: None,
                bootstrap_token: None,
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
                ..
            } => {
                assert_eq!(peer_id, "peer-ext-1");
                assert_eq!(address, "https://backend.example.invalid/mesh/ext-1");
                assert_eq!(session_id.as_ref(), Some(&new_sid));
            }
            other => panic!("expected backend peer member ref, got {other:?}"),
        }
    }

    #[test]
    fn test_roster_by_profile() {
        let mut roster = Roster::new();
        add_member(
            &mut roster,
            MeerkatId::from("w1"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_bridge_session_id(session_id()),
        );
        add_member(
            &mut roster,
            MeerkatId::from("w2"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_bridge_session_id(session_id()),
        );
        add_member(
            &mut roster,
            MeerkatId::from("lead"),
            ProfileName::from("orchestrator"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_bridge_session_id(session_id()),
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
            MemberRef::from_bridge_session_id(session_id()),
        );
        add_member(
            &mut roster,
            MeerkatId::from("b"),
            ProfileName::from("lead"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_bridge_session_id(session_id()),
        );

        let all: Vec<_> = roster.list().collect();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_roster_project_idempotent() {
        let sid = session_id();
        let events = vec![make_event(
            1,
            spawned_kind(
                "a",
                "worker",
                MobRuntimeMode::AutonomousHost,
                MemberRef::from_bridge_session_id(sid),
                BTreeMap::new(),
            ),
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
    fn test_project_skips_invalid_external_peer_wired_spec() {
        let local = AgentIdentity::from("local");
        let zero_pubkey_external = AgentIdentity::from("remote-mob/worker/zero");
        let mismatch_external = AgentIdentity::from("remote-mob/worker/mismatch");
        let zero_pubkey_spec = TrustedPeerDescriptor::test_only_unsigned_typed(
            zero_pubkey_external.as_str(),
            PeerId::new(),
            "inproc://remote-mob/worker/zero",
        )
        .expect("legacy invalid external peer spec should still decode");
        let mismatch_spec = TrustedPeerDescriptor::test_only_unsigned_typed(
            mismatch_external.as_str(),
            PeerId::new(),
            "inproc://remote-mob/worker/mismatch",
        )
        .expect("legacy invalid external peer spec should still decode")
        .with_pubkey([8u8; 32]);
        let events = vec![
            make_event(
                1,
                spawned_kind(
                    local.as_str(),
                    "worker",
                    MobRuntimeMode::AutonomousHost,
                    MemberRef::from_bridge_session_id(session_id()),
                    BTreeMap::new(),
                ),
            ),
            make_event(
                2,
                MobEventKind::ExternalPeerWired {
                    local: local.clone(),
                    spec: zero_pubkey_spec,
                },
            ),
            make_event(
                3,
                MobEventKind::ExternalPeerWired {
                    local: local.clone(),
                    spec: mismatch_spec,
                },
            ),
        ];

        let roster = Roster::project(&events);
        let entry = roster
            .get_by_identity(&local)
            .expect("local member should project");

        assert!(
            entry.external_peer_specs.is_empty(),
            "invalid external peer specs must not hydrate resume trust state"
        );
        assert!(
            !entry.wired_to.contains(&zero_pubkey_external),
            "zero-pubkey external peer specs must not project as wired edges"
        );
        assert!(
            !entry.wired_to.contains(&mismatch_external),
            "invalid external peer specs must not project as wired edges"
        );
    }

    #[test]
    fn test_project_accepts_valid_external_peer_wired_spec() {
        let local = AgentIdentity::from("local");
        let external = AgentIdentity::from("remote-mob/worker/agent-b");
        let pubkey = [8u8; 32];
        let peer_id = PeerId::from_ed25519_pubkey(&pubkey);
        let spec = TrustedPeerDescriptor::unsigned_with_pubkey(
            external.as_str(),
            peer_id.to_string(),
            pubkey,
            "inproc://remote-mob/worker/agent-b",
        )
        .expect("valid external peer spec");
        let events = vec![
            make_event(
                1,
                spawned_kind(
                    local.as_str(),
                    "worker",
                    MobRuntimeMode::AutonomousHost,
                    MemberRef::from_bridge_session_id(session_id()),
                    BTreeMap::new(),
                ),
            ),
            make_event(
                2,
                MobEventKind::ExternalPeerWired {
                    local: local.clone(),
                    spec,
                },
            ),
        ];

        let roster = Roster::project(&events);
        let entry = roster
            .get_by_identity(&local)
            .expect("local member should project");

        assert!(entry.external_peer_specs.contains_key(&external));
        assert!(entry.wired_to.contains(&external));
    }

    #[test]
    fn test_roster_serde_entry_roundtrip() {
        let entry = RosterEntry {
            agent_identity: AgentIdentity::from("test"),
            generation: Generation::INITIAL,
            fence_token: FenceToken::new(0),
            agent_runtime_id: AgentRuntimeId::initial(AgentIdentity::from("test")),
            role: ProfileName::from("worker"),
            member_ref: MemberRef::from_bridge_session_id(session_id()),
            runtime_mode: MobRuntimeMode::AutonomousHost,
            peer_id: None,
            transport_public_key: None,
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
        assert_eq!(parsed.agent_identity, entry.agent_identity);
        assert_eq!(parsed.wired_to.len(), 1);
    }

    #[test]
    fn test_serde_roundtrip_with_state_field() {
        let entry = RosterEntry {
            agent_identity: AgentIdentity::from("test"),
            generation: Generation::INITIAL,
            fence_token: FenceToken::new(0),
            agent_runtime_id: AgentRuntimeId::initial(AgentIdentity::from("test")),
            role: ProfileName::from("worker"),
            member_ref: MemberRef::from_bridge_session_id(session_id()),
            runtime_mode: MobRuntimeMode::AutonomousHost,
            peer_id: None,
            transport_public_key: None,
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
    fn test_bridge_session_id_via_entry_session_member() {
        let mut roster = Roster::new();
        let sid = session_id();
        add_member(
            &mut roster,
            MeerkatId::from("a"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_bridge_session_id(sid.clone()),
        );
        let entry = roster.get(&MeerkatId::from("a")).unwrap();
        assert_eq!(entry.bridge_session_id(), Some(&sid));
        assert!(roster.find_by_bridge_session_id(&sid).is_some());
    }

    #[test]
    fn test_bridge_session_id_via_entry_backend_peer_with_bridge() {
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
                pubkey: None,
                bootstrap_token: None,
                session_id: Some(sid.clone()),
            },
        );
        let entry = roster.get(&MeerkatId::from("ext-1")).unwrap();
        assert_eq!(entry.bridge_session_id(), Some(&sid));
    }

    #[test]
    fn test_bridge_session_id_via_entry_backend_peer_no_bridge() {
        let mut roster = Roster::new();
        add_member(
            &mut roster,
            MeerkatId::from("ext-2"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::BackendPeer {
                peer_id: "peer-ext-2".to_string(),
                address: "https://backend.example.invalid/mesh/ext-2".to_string(),
                pubkey: None,
                bootstrap_token: None,
                session_id: None,
            },
        );
        let entry = roster.get(&MeerkatId::from("ext-2")).unwrap();
        assert_eq!(entry.bridge_session_id(), None);
    }

    #[test]
    fn test_find_by_bridge_session_id_not_found() {
        let roster = Roster::new();
        let sid = session_id();
        assert!(roster.find_by_bridge_session_id(&sid).is_none());
    }

    #[test]
    fn test_serde_roundtrip_missing_state_defaults_to_active() {
        // Simulate serialized data without the optional state field — state should default to Active.
        let json = r#"{"agent_identity":"old","generation":0,"fence_token":0,"agent_runtime_id":{"identity":"old","generation":0},"role":"worker","member_ref":{"kind":"session","session_id":"00000000-0000-0000-0000-000000000001"},"runtime_mode":"autonomous_host","wired_to":[]}"#;
        let parsed: RosterEntry = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.state, MemberState::Active);
    }

    #[test]
    fn test_serde_accepts_legacy_meerkat_id_alias() {
        // Legacy clients sending `meerkat_id` must still deserialize.
        let json = r#"{"meerkat_id":"old","generation":0,"fence_token":0,"agent_runtime_id":{"identity":"old","generation":0},"role":"worker","member_ref":{"kind":"session","session_id":"00000000-0000-0000-0000-000000000001"},"runtime_mode":"autonomous_host","wired_to":[]}"#;
        let parsed: RosterEntry = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.agent_identity, AgentIdentity::from("old"));
    }

    #[test]
    fn test_project_never_produces_retiring() {
        let sid = session_id();
        let events = vec![make_event(
            1,
            spawned_kind(
                "a",
                "worker",
                MobRuntimeMode::AutonomousHost,
                MemberRef::from_bridge_session_id(sid),
                BTreeMap::new(),
            ),
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
            spawned_kind(
                "a",
                "worker",
                MobRuntimeMode::AutonomousHost,
                MemberRef::from_bridge_session_id(sid),
                labels.clone(),
            ),
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
            MemberRef::from_bridge_session_id(session_id()),
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
            MemberRef::from_bridge_session_id(session_id()),
            {
                let mut m = BTreeMap::new();
                m.insert("faction".to_string(), "south".to_string());
                m
            },
        ));
        let found = roster.find_by_label("faction", "north");
        assert!(found.is_some());
        assert_eq!(found.unwrap().agent_identity, AgentIdentity::from("a"));
    }

    #[test]
    fn test_find_all_by_label_returns_all_matching() {
        let mut roster = Roster::new();
        roster.add(make_add_entry(
            MeerkatId::from("a"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_bridge_session_id(session_id()),
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
            MemberRef::from_bridge_session_id(session_id()),
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
            MemberRef::from_bridge_session_id(session_id()),
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
    fn test_roster_entry_roundtrip_json() {
        let mut roster = Roster::new();
        add_member(
            &mut roster,
            MeerkatId::from("a"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_bridge_session_id(session_id()),
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
            MemberRef::from_bridge_session_id(session_id()),
        );
        add_member(
            &mut roster,
            MeerkatId::from("b"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_bridge_session_id(session_id()),
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
                spawned_kind(
                    "x",
                    "worker",
                    MobRuntimeMode::AutonomousHost,
                    MemberRef::from_bridge_session_id(session_id()),
                    BTreeMap::new(),
                ),
            ),
            make_event(
                2,
                spawned_kind(
                    "y",
                    "lead",
                    MobRuntimeMode::AutonomousHost,
                    MemberRef::from_bridge_session_id(session_id()),
                    BTreeMap::new(),
                ),
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
            MobEventKind::MemberSpawned(crate::event::MemberSpawnedEvent::new(
                identity.clone(),
                Generation::INITIAL,
                FenceToken::new(1),
                AgentRuntimeId::initial(identity.clone()),
                ProfileName::from("research"),
            )),
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
                MobEventKind::MemberSpawned(crate::event::MemberSpawnedEvent::new(
                    identity.clone(),
                    Generation::INITIAL,
                    FenceToken::new(1),
                    AgentRuntimeId::initial(identity.clone()),
                    ProfileName::from("worker"),
                )),
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
            MemberRef::from_bridge_session_id(session_id()),
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
                spawned_kind(
                    "a",
                    "worker",
                    MobRuntimeMode::AutonomousHost,
                    MemberRef::from_bridge_session_id(session_id()),
                    BTreeMap::new(),
                ),
            ),
            make_event(2, MobEventKind::MobReset),
        ];
        let roster = Roster::project(&events);
        assert!(roster.is_empty());
        assert!(roster.is_index_coherent());
        assert!(roster.resolve_identity(&MeerkatId::from("a")).is_none());
    }
}

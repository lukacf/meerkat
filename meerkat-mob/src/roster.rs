//! Roster tracking for active meerkats in a mob.
//!
//! The `Roster` is a projection built from `MeerkatSpawned`, `MeerkatRetired`,
//! `PeersWired`, and `PeersUnwired` events.

use crate::event::{MemberRef, MobEvent, MobEventKind};
use crate::ids::{MeerkatId, ProfileName};
use crate::runtime_mode::MobRuntimeMode;
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

/// A single meerkat entry in the roster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RosterEntry {
    /// Unique meerkat identifier.
    pub meerkat_id: MeerkatId,
    /// Profile name this meerkat was spawned from.
    pub profile: ProfileName,
    /// Backend-neutral identity for this meerkat.
    pub member_ref: MemberRef,
    /// Runtime mode for this member.
    #[serde(default)]
    pub runtime_mode: MobRuntimeMode,
    /// Lifecycle state (Active or Retiring).
    #[serde(default)]
    pub state: MemberState,
    /// Set of peer meerkat IDs this meerkat is wired to.
    pub wired_to: BTreeSet<MeerkatId>,
}

/// Tracks active meerkats and their wiring in a mob.
///
/// Built by replaying events. Shared via `Arc<RwLock<Roster>>` between
/// the actor (writes) and handle (reads).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Roster {
    entries: BTreeMap<MeerkatId, RosterEntry>,
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
            } => {
                self.add(
                    meerkat_id.clone(),
                    role.clone(),
                    *runtime_mode,
                    member_ref.clone(),
                );
            }
            MobEventKind::MeerkatRetired { meerkat_id, .. } => {
                self.remove(meerkat_id);
            }
            MobEventKind::PeersWired { a, b } => {
                self.wire(a, b);
            }
            MobEventKind::PeersUnwired { a, b } => {
                self.unwire(a, b);
            }
            MobEventKind::MobReset => {
                self.entries.clear();
            }
            _ => {}
        }
    }

    /// Add a meerkat to the roster.
    pub fn add(
        &mut self,
        meerkat_id: MeerkatId,
        profile: ProfileName,
        runtime_mode: MobRuntimeMode,
        member_ref: MemberRef,
    ) -> bool {
        self.entries
            .insert(
                meerkat_id.clone(),
                RosterEntry {
                    meerkat_id,
                    profile,
                    member_ref,
                    runtime_mode,
                    state: MemberState::default(),
                    wired_to: BTreeSet::new(),
                },
            )
            .is_none()
    }

    /// Remove a meerkat from the roster. Also removes it from all peer wiring sets.
    pub fn remove(&mut self, meerkat_id: &MeerkatId) {
        if self.entries.remove(meerkat_id).is_some() {
            // Remove this meerkat from all other entries' wired_to sets
            for entry in self.entries.values_mut() {
                entry.wired_to.remove(meerkat_id);
            }
        }
    }

    /// Wire two meerkats together (bidirectional).
    pub fn wire(&mut self, a: &MeerkatId, b: &MeerkatId) {
        if let Some(entry_a) = self.entries.get_mut(a) {
            entry_a.wired_to.insert(b.clone());
        }
        if let Some(entry_b) = self.entries.get_mut(b) {
            entry_b.wired_to.insert(a.clone());
        }
    }

    /// Unwire two meerkats (bidirectional).
    pub fn unwire(&mut self, a: &MeerkatId, b: &MeerkatId) {
        if let Some(entry_a) = self.entries.get_mut(a) {
            entry_a.wired_to.remove(b);
        }
        if let Some(entry_b) = self.entries.get_mut(b) {
            entry_b.wired_to.remove(a);
        }
    }

    /// Get a roster entry by meerkat ID.
    pub fn get(&self, meerkat_id: &MeerkatId) -> Option<&RosterEntry> {
        self.entries.get(meerkat_id)
    }

    /// Update the member reference for an existing meerkat.
    pub fn set_member_ref(&mut self, meerkat_id: &MeerkatId, member_ref: MemberRef) -> bool {
        if let Some(entry) = self.entries.get_mut(meerkat_id) {
            entry.member_ref = member_ref;
            return true;
        }
        false
    }

    /// Update the bridge session ID while preserving backend-specific identity.
    pub fn set_session_id(&mut self, meerkat_id: &MeerkatId, session_id: SessionId) -> bool {
        if let Some(entry) = self.entries.get_mut(meerkat_id) {
            entry.member_ref = match &entry.member_ref {
                MemberRef::Session { .. } => MemberRef::Session { session_id },
                MemberRef::BackendPeer {
                    peer_id, address, ..
                } => MemberRef::BackendPeer {
                    peer_id: peer_id.clone(),
                    address: address.clone(),
                    session_id: Some(session_id),
                },
            };
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

    /// Find active meerkats with a given profile name.
    pub fn by_profile(&self, profile: &ProfileName) -> impl Iterator<Item = &RosterEntry> {
        self.entries
            .values()
            .filter(move |e| e.profile == *profile && e.state == MemberState::Active)
    }

    /// Get the set of peer meerkat IDs wired to a given meerkat.
    pub fn wired_peers_of(&self, meerkat_id: &MeerkatId) -> Option<&BTreeSet<MeerkatId>> {
        self.entries.get(meerkat_id).map(|e| &e.wired_to)
    }

    /// Number of active meerkats in the roster.
    pub fn len(&self) -> usize {
        self.entries
            .values()
            .filter(|e| e.state == MemberState::Active)
            .count()
    }

    /// Whether the roster has no active meerkats.
    pub fn is_empty(&self) -> bool {
        !self
            .entries
            .values()
            .any(|e| e.state == MemberState::Active)
    }

    /// Mark a member as `Retiring`. Returns `true` if the member was found and
    /// transitioned (i.e. it was `Active`); `false` otherwise.
    pub fn mark_retiring(&mut self, meerkat_id: &MeerkatId) -> bool {
        if let Some(entry) = self.entries.get_mut(meerkat_id)
            && entry.state == MemberState::Active
        {
            entry.state = MemberState::Retiring;
            return true;
        }
        false
    }
}

impl RosterEntry {
    pub fn session_id(&self) -> Option<&SessionId> {
        self.member_ref.session_id()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::MobId;
    use chrono::Utc;
    use uuid::Uuid;

    fn session_id() -> SessionId {
        SessionId::from_uuid(Uuid::new_v4())
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
        roster.add(
            MeerkatId::from("agent-1"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(sid.clone()),
        );
        assert_eq!(roster.len(), 1);
        let entry = roster.get(&MeerkatId::from("agent-1")).unwrap();
        assert_eq!(entry.profile.as_str(), "worker");
        assert_eq!(entry.session_id(), Some(&sid));
        assert!(entry.wired_to.is_empty());
    }

    #[test]
    fn test_roster_remove() {
        let mut roster = Roster::new();
        roster.add(
            MeerkatId::from("agent-1"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );
        roster.add(
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
    fn test_set_session_id_preserves_backend_member_ref_identity() {
        let mut roster = Roster::new();
        let old_sid = session_id();
        roster.add(
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
        assert!(roster.set_session_id(&MeerkatId::from("ext-1"), new_sid.clone()));
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
        roster.add(
            MeerkatId::from("a"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );
        roster.add(
            MeerkatId::from("b"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );

        roster.wire(&MeerkatId::from("a"), &MeerkatId::from("b"));

        let peers_a = roster.wired_peers_of(&MeerkatId::from("a")).unwrap();
        assert!(peers_a.contains(&MeerkatId::from("b")));
        let peers_b = roster.wired_peers_of(&MeerkatId::from("b")).unwrap();
        assert!(peers_b.contains(&MeerkatId::from("a")));

        roster.unwire(&MeerkatId::from("a"), &MeerkatId::from("b"));

        let peers_a = roster.wired_peers_of(&MeerkatId::from("a")).unwrap();
        assert!(peers_a.is_empty());
        let peers_b = roster.wired_peers_of(&MeerkatId::from("b")).unwrap();
        assert!(peers_b.is_empty());
    }

    #[test]
    fn test_roster_wire_idempotent() {
        let mut roster = Roster::new();
        roster.add(
            MeerkatId::from("a"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );
        roster.add(
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
    fn test_roster_by_profile() {
        let mut roster = Roster::new();
        roster.add(
            MeerkatId::from("w1"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );
        roster.add(
            MeerkatId::from("w2"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );
        roster.add(
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
        roster.add(
            MeerkatId::from("a"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );
        roster.add(
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
                },
            ),
            make_event(
                2,
                MobEventKind::MeerkatSpawned {
                    meerkat_id: MeerkatId::from("b"),
                    role: ProfileName::from("worker"),
                    runtime_mode: MobRuntimeMode::AutonomousHost,
                    member_ref: MemberRef::from_session_id(sid2),
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
        assert!(peers_a.contains(&MeerkatId::from("b")));
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
                },
            ),
            make_event(
                2,
                MobEventKind::MeerkatSpawned {
                    meerkat_id: MeerkatId::from("b"),
                    role: ProfileName::from("worker"),
                    runtime_mode: MobRuntimeMode::AutonomousHost,
                    member_ref: MemberRef::from_session_id(sid2.clone()),
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
            },
        )];
        let roster1 = Roster::project(&events);
        let roster2 = Roster::project(&events);
        assert_eq!(roster1.len(), roster2.len());
        assert_eq!(
            roster1.get(&MeerkatId::from("a")).unwrap().profile,
            roster2.get(&MeerkatId::from("a")).unwrap().profile,
        );
    }

    #[test]
    fn test_roster_serde_entry_roundtrip() {
        let entry = RosterEntry {
            meerkat_id: MeerkatId::from("test"),
            profile: ProfileName::from("worker"),
            member_ref: MemberRef::from_session_id(session_id()),
            runtime_mode: MobRuntimeMode::AutonomousHost,
            state: MemberState::default(),
            wired_to: {
                let mut s = BTreeSet::new();
                s.insert(MeerkatId::from("peer-1"));
                s
            },
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: RosterEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.meerkat_id, entry.meerkat_id);
        assert_eq!(parsed.wired_to.len(), 1);
    }

    #[test]
    fn test_mark_retiring() {
        let mut roster = Roster::new();
        roster.add(
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
        roster.add(
            MeerkatId::from("a"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );
        roster.add(
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
        roster.add(
            MeerkatId::from("a"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );
        roster.add(
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
        roster.add(
            MeerkatId::from("a"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );
        roster.add(
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

        roster.add(
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
        roster.add(
            MeerkatId::from("w1"),
            ProfileName::from("worker"),
            MobRuntimeMode::AutonomousHost,
            MemberRef::from_session_id(session_id()),
        );
        roster.add(
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
        roster.add(
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
            meerkat_id: MeerkatId::from("test"),
            profile: ProfileName::from("worker"),
            member_ref: MemberRef::from_session_id(session_id()),
            runtime_mode: MobRuntimeMode::AutonomousHost,
            state: MemberState::Active,
            wired_to: BTreeSet::new(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: RosterEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.state, MemberState::Active);
    }

    #[test]
    fn test_serde_roundtrip_missing_state_defaults_to_active() {
        // Simulate old serialized data without the state field
        let json = r#"{"meerkat_id":"old","profile":"worker","member_ref":{"kind":"session","session_id":"00000000-0000-0000-0000-000000000001"},"runtime_mode":"autonomous_host","wired_to":[]}"#;
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
            },
        )];
        let roster = Roster::project(&events);
        let entry = roster.get(&MeerkatId::from("a")).unwrap();
        assert_eq!(entry.state, MemberState::Active);
    }
}

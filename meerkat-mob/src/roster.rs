//! Roster tracking for active meerkats in a mob.
//!
//! The `Roster` is a projection built from `MeerkatSpawned`, `MeerkatRetired`,
//! `PeersWired`, and `PeersUnwired` events.

use crate::event::{MobEvent, MobEventKind};
use crate::ids::{MeerkatId, ProfileName};
use meerkat_core::types::SessionId;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// A single meerkat entry in the roster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RosterEntry {
    /// Unique meerkat identifier.
    pub meerkat_id: MeerkatId,
    /// Profile name this meerkat was spawned from.
    pub profile: ProfileName,
    /// Session ID for this meerkat's agent session.
    pub session_id: SessionId,
    /// Set of peer meerkat IDs this meerkat is wired to.
    pub wired_to: BTreeSet<MeerkatId>,
}

/// Tracks active meerkats and their wiring in a mob.
///
/// Built by replaying events. Shared via `Arc<RwLock<Roster>>` between
/// the actor (writes) and handle (reads).
#[derive(Debug, Clone, Default)]
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
                session_id,
            } => {
                self.add(meerkat_id.clone(), role.clone(), session_id.clone());
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
            _ => {}
        }
    }

    /// Add a meerkat to the roster.
    pub fn add(&mut self, meerkat_id: MeerkatId, profile: ProfileName, session_id: SessionId) {
        self.entries.insert(
            meerkat_id.clone(),
            RosterEntry {
                meerkat_id,
                profile,
                session_id,
                wired_to: BTreeSet::new(),
            },
        );
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

    /// Update the session ID for an existing meerkat.
    pub fn set_session_id(&mut self, meerkat_id: &MeerkatId, session_id: SessionId) -> bool {
        if let Some(entry) = self.entries.get_mut(meerkat_id) {
            entry.session_id = session_id;
            return true;
        }
        false
    }

    /// List all roster entries.
    pub fn list(&self) -> impl Iterator<Item = &RosterEntry> {
        self.entries.values()
    }

    /// Find all meerkats with a given profile name.
    pub fn by_profile(&self, profile: &ProfileName) -> impl Iterator<Item = &RosterEntry> {
        self.entries.values().filter(move |e| e.profile == *profile)
    }

    /// Get the set of peer meerkat IDs wired to a given meerkat.
    pub fn wired_peers_of(&self, meerkat_id: &MeerkatId) -> Option<&BTreeSet<MeerkatId>> {
        self.entries.get(meerkat_id).map(|e| &e.wired_to)
    }

    /// Number of meerkats in the roster.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the roster is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
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
            sid.clone(),
        );
        assert_eq!(roster.len(), 1);
        let entry = roster.get(&MeerkatId::from("agent-1")).unwrap();
        assert_eq!(entry.profile.as_str(), "worker");
        assert_eq!(entry.session_id, sid);
        assert!(entry.wired_to.is_empty());
    }

    #[test]
    fn test_roster_remove() {
        let mut roster = Roster::new();
        roster.add(
            MeerkatId::from("agent-1"),
            ProfileName::from("worker"),
            session_id(),
        );
        roster.add(
            MeerkatId::from("agent-2"),
            ProfileName::from("worker"),
            session_id(),
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
    fn test_roster_wire_and_unwire() {
        let mut roster = Roster::new();
        roster.add(
            MeerkatId::from("a"),
            ProfileName::from("worker"),
            session_id(),
        );
        roster.add(
            MeerkatId::from("b"),
            ProfileName::from("worker"),
            session_id(),
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
            session_id(),
        );
        roster.add(
            MeerkatId::from("b"),
            ProfileName::from("worker"),
            session_id(),
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
            session_id(),
        );
        roster.add(
            MeerkatId::from("w2"),
            ProfileName::from("worker"),
            session_id(),
        );
        roster.add(
            MeerkatId::from("lead"),
            ProfileName::from("orchestrator"),
            session_id(),
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
            session_id(),
        );
        roster.add(
            MeerkatId::from("b"),
            ProfileName::from("lead"),
            session_id(),
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
                    session_id: sid1,
                },
            ),
            make_event(
                2,
                MobEventKind::MeerkatSpawned {
                    meerkat_id: MeerkatId::from("b"),
                    role: ProfileName::from("worker"),
                    session_id: sid2,
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
                    session_id: sid1.clone(),
                },
            ),
            make_event(
                2,
                MobEventKind::MeerkatSpawned {
                    meerkat_id: MeerkatId::from("b"),
                    role: ProfileName::from("worker"),
                    session_id: sid2.clone(),
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
                    session_id: sid1,
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
                session_id: sid,
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
            session_id: session_id(),
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
}

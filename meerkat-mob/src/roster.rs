use std::collections::{BTreeMap, BTreeSet};

use crate::ids::{MeerkatId, ProfileName};
use meerkat_core::types::SessionId;

#[derive(Clone, Debug)]
pub struct RosterEntry {
    pub meerkat_id: MeerkatId,
    pub profile: ProfileName,
    pub session_id: SessionId,
    pub wired_to: BTreeSet<MeerkatId>,
}

#[derive(Clone, Debug, Default)]
pub struct Roster {
    entries: BTreeMap<MeerkatId, RosterEntry>,
}

impl Roster {
    pub fn add(&mut self, entry: RosterEntry) -> bool {
        if self.entries.contains_key(&entry.meerkat_id) {
            return false;
        }
        self.entries.insert(entry.meerkat_id.clone(), entry);
        true
    }

    pub fn remove(&mut self, meerkat_id: &MeerkatId) -> Option<RosterEntry> {
        let removed = self.entries.remove(meerkat_id)?;
        for entry in self.entries.values_mut() {
            entry.wired_to.remove(meerkat_id);
        }
        Some(removed)
    }

    pub fn wire(&mut self, a: &MeerkatId, b: &MeerkatId) -> bool {
        if a == b {
            return false;
        }
        if !self.entries.contains_key(a) || !self.entries.contains_key(b) {
            return false;
        }
        self.entries
            .get_mut(a)
            .expect("entry exists")
            .wired_to
            .insert(b.clone());
        self.entries
            .get_mut(b)
            .expect("entry exists")
            .wired_to
            .insert(a.clone());
        true
    }

    pub fn unwire(&mut self, a: &MeerkatId, b: &MeerkatId) -> bool {
        if a == b {
            return false;
        }
        let mut removed = false;
        if let Some(a_entry) = self.entries.get_mut(a) {
            removed = a_entry.wired_to.remove(b);
        }
        if let Some(b_entry) = self.entries.get_mut(b) {
            removed = b_entry.wired_to.remove(a) || removed;
        }
        removed
    }

    pub fn get(&self, meerkat_id: &MeerkatId) -> Option<&RosterEntry> {
        self.entries.get(meerkat_id)
    }

    pub fn by_profile(&self, profile: &ProfileName) -> Vec<RosterEntry> {
        self.entries
            .values()
            .filter(|entry| &entry.profile == profile)
            .cloned()
            .collect()
    }

    pub fn wired_peers_of(&self, meerkat_id: &MeerkatId) -> Vec<MeerkatId> {
        self.entries
            .get(meerkat_id)
            .map(|entry| entry.wired_to.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn list(&self) -> Vec<RosterEntry> {
        self.entries.values().cloned().collect()
    }

    pub fn contains(&self, meerkat_id: &MeerkatId) -> bool {
        self.entries.contains_key(meerkat_id)
    }

    pub fn clear(&mut self) {
        self.entries.clear()
    }

    pub fn session_ids(&self) -> Vec<SessionId> {
        self.entries.values().map(|entry| entry.session_id.clone()).collect()
    }
}

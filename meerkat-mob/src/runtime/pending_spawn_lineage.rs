use super::actor::PendingSpawn;
use crate::error::MobError;
use crate::ids::MeerkatId;
use std::collections::{BTreeMap, BTreeSet};
use tokio::task::JoinHandle;
use tracing::warn;

/// Tracks staged pending spawns along with their lifecycle task handles.
pub(super) struct PendingSpawnLineage {
    metadata: BTreeMap<u64, PendingSpawn>,
    tasks: BTreeMap<u64, JoinHandle<()>>,
}

pub(super) struct PendingSpawnSlot {
    pub(super) ticket: u64,
    pub(super) spawn: PendingSpawn,
    pub(super) task: Option<JoinHandle<()>>,
}

pub(super) enum PendingSpawnInsertImpact {
    Added,
    Collided,
}

impl PendingSpawnLineage {
    pub(super) fn new() -> Self {
        Self {
            metadata: BTreeMap::new(),
            tasks: BTreeMap::new(),
        }
    }

    pub(super) fn len(&self) -> usize {
        self.metadata.len()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.metadata.is_empty()
    }

    pub(super) fn tickets(&self) -> BTreeSet<u64> {
        self.metadata
            .keys()
            .chain(self.tasks.keys())
            .copied()
            .collect()
    }

    pub(super) fn contains_member(&self, meerkat_id: &MeerkatId) -> bool {
        self.metadata
            .values()
            .any(|pending| &pending.meerkat_id == meerkat_id)
    }

    pub(super) fn insert(
        &mut self,
        spawn_ticket: u64,
        pending: PendingSpawn,
        task: JoinHandle<()>,
    ) -> PendingSpawnInsertImpact {
        let replaced_pending = self.metadata.insert(spawn_ticket, pending);
        let replaced_task = self.tasks.insert(spawn_ticket, task);
        let replaced = replaced_pending.is_some() || replaced_task.is_some();

        if let Some(prev) = replaced_pending {
            let _ = prev.reply_tx.send(Err(MobError::Internal(format!(
                "pending spawn slot collision for ticket {spawn_ticket}"
            ))));
        }
        if let Some(task) = replaced_task {
            task.abort();
        }

        if replaced {
            warn!(
                spawn_ticket,
                "pending spawn slot collision replaced existing entry"
            );
        }

        self.debug_assert_alignment();
        if replaced {
            PendingSpawnInsertImpact::Collided
        } else {
            PendingSpawnInsertImpact::Added
        }
    }

    pub(super) fn take_slot(&mut self, spawn_ticket: u64) -> Option<PendingSpawnSlot> {
        let spawn = self.metadata.remove(&spawn_ticket)?;
        let task = self.tasks.remove(&spawn_ticket);
        Some(PendingSpawnSlot {
            ticket: spawn_ticket,
            spawn,
            task,
        })
    }

    pub(super) fn complete_slot(&mut self, spawn_ticket: u64) -> Option<PendingSpawnSlot> {
        let slot = self.take_slot(spawn_ticket)?;
        self.debug_assert_alignment();
        if let Some(message) = self.alignment_violation(None) {
            warn!(
                spawn_ticket,
                message = %message,
                "pending spawn alignment violated during completion"
            );
        }
        Some(slot)
    }

    pub(super) fn take_for_member(&mut self, meerkat_id: &MeerkatId) -> Vec<PendingSpawnSlot> {
        let tickets: Vec<_> = self
            .metadata
            .iter()
            .filter(|(_, pending)| &pending.meerkat_id == meerkat_id)
            .map(|(&ticket, _)| ticket)
            .collect();
        let mut canceled = Vec::new();
        for ticket in tickets {
            if let Some(slot) = self.take_slot(ticket) {
                canceled.push(slot);
            }
        }
        canceled
    }

    pub(super) fn drain_all(&mut self) -> Vec<PendingSpawnSlot> {
        let tickets: Vec<_> = self.metadata.keys().copied().collect();
        let mut failed = Vec::new();
        for ticket in tickets {
            if let Some(slot) = self.take_slot(ticket) {
                failed.push(slot);
            }
        }
        failed
    }

    pub(super) fn alignment_violation(&self, expected: Option<usize>) -> Option<String> {
        if self.metadata.len() != self.tasks.len() {
            return Some(format!(
                "pending metadata/task length mismatch: metadata={}, tasks={}",
                self.metadata.len(),
                self.tasks.len()
            ));
        }
        let key_aligned = self
            .metadata
            .keys()
            .all(|ticket| self.tasks.contains_key(ticket));
        if !key_aligned {
            return Some("pending metadata/task key mismatch".into());
        }
        if let Some(expected_count) = expected {
            let actual = self.metadata.len();
            if expected_count != actual {
                return Some(format!(
                    "pending count mismatch: expected={expected_count}, actual={actual}"
                ));
            }
        }
        None
    }

    fn debug_assert_alignment(&self) {
        debug_assert!(
            self.metadata.len() == self.tasks.len(),
            "pending spawn metadata/task count mismatch"
        );
    }
}

impl PendingSpawnSlot {
    pub(super) fn fail(mut self, reason: &str) {
        let _ = self
            .spawn
            .reply_tx
            .send(Err(MobError::Internal(reason.into())));
        if let Some(handle) = self.task.take() {
            handle.abort();
        }
    }
}

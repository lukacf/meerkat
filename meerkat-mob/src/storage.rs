//! MobStorage bundle.
//!
//! Groups the event store with a session store for a mob's isolated storage.

use crate::store::{InMemoryMobEventStore, MobEventStore};
use std::sync::Arc;

/// Storage bundle for a mob.
///
/// Contains both the mob event store (structural state) and session
/// store (for meerkat sessions). Each mob has its own isolated storage.
pub struct MobStorage {
    /// Event store for mob structural events.
    pub events: Arc<dyn MobEventStore>,
}

impl MobStorage {
    /// Create a storage bundle with in-memory stores (for tests and ephemeral mobs).
    pub fn in_memory() -> Self {
        Self {
            events: Arc::new(InMemoryMobEventStore::new()),
        }
    }
}

impl std::fmt::Debug for MobStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MobStorage")
            .field("events", &"<dyn MobEventStore>")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{MobEventKind, NewMobEvent};
    use crate::ids::MobId;

    #[tokio::test]
    async fn test_in_memory_storage_creates_working_stores() {
        let storage = MobStorage::in_memory();

        // Event store works
        let event = NewMobEvent {
            mob_id: MobId::from("test"),
            kind: MobEventKind::MobCompleted,
        };
        let stored = storage.events.append(event).await.unwrap();
        assert_eq!(stored.cursor, 1);

        let all = storage.events.replay_all().await.unwrap();
        assert_eq!(all.len(), 1);
    }
}

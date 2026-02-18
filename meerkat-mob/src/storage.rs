use std::sync::Arc;

use meerkat_store::MemoryStore;
use meerkat_store::SessionStore;

use crate::store::MobEventStore;
use crate::InMemoryMobEventStore;

#[derive(Clone)]
pub struct MobStorage {
    pub sessions: Arc<dyn SessionStore>,
    pub events: Arc<dyn MobEventStore>,
}

impl MobStorage {
    pub fn new(sessions: Arc<dyn SessionStore>, events: Arc<dyn MobEventStore>) -> Self {
        Self { sessions, events }
    }

    pub fn in_memory() -> Self {
        Self {
            sessions: Arc::new(MemoryStore::default()),
            events: InMemoryMobEventStore::new(crate::ids::MobId::from("default")),
        }
    }

    pub fn in_memory_with_id(mob_id: crate::ids::MobId) -> Self {
        Self {
            sessions: Arc::new(MemoryStore::default()),
            events: InMemoryMobEventStore::new(mob_id),
        }
    }
}

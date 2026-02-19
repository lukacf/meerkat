use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::RwLock;

use crate::error::MobError;
use crate::event::{MobEvent, NewMobEvent};
use crate::ids::MobId;

#[async_trait]
pub trait MobEventStore: Send + Sync {
    async fn append(&self, event: NewMobEvent) -> Result<MobEvent, MobError>;
    async fn poll(
        &self,
        after_cursor: Option<u64>,
        limit: Option<usize>,
    ) -> Result<Vec<MobEvent>, MobError>;
    async fn replay_all(&self) -> Result<Vec<MobEvent>, MobError>;
}

pub struct InMemoryMobEventStore {
    mob_id: MobId,
    events: RwLock<Vec<MobEvent>>,
}

impl InMemoryMobEventStore {
    pub fn new(mob_id: MobId) -> Arc<Self> {
        Arc::new(Self {
            mob_id,
            events: RwLock::new(Vec::new()),
        })
    }
}

#[async_trait]
impl MobEventStore for InMemoryMobEventStore {
    async fn append(&self, event: NewMobEvent) -> Result<MobEvent, MobError> {
        let mut events = self.events.write().await;
        let cursor = events
            .last()
            .map_or(0, |last| last.cursor.saturating_add(1));
        let mob_event = MobEvent {
            cursor,
            timestamp: event.timestamp.unwrap_or_else(Utc::now),
            mob_id: self.mob_id.clone(),
            kind: event.kind,
        };
        events.push(mob_event.clone());
        Ok(mob_event)
    }

    async fn poll(
        &self,
        after_cursor: Option<u64>,
        limit: Option<usize>,
    ) -> Result<Vec<MobEvent>, MobError> {
        let events = self.events.read().await;
        let mut output: Vec<_> = events
            .iter()
            .filter(|event| after_cursor.is_none_or(|cursor| event.cursor > cursor))
            .cloned()
            .collect();
        if let Some(limit) = limit {
            output.truncate(limit);
        }
        Ok(output)
    }

    async fn replay_all(&self) -> Result<Vec<MobEvent>, MobError> {
        Ok(self.events.read().await.clone())
    }
}

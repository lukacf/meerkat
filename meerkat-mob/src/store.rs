//! Mob event store trait and in-memory implementation.
//!
//! The `MobEventStore` trait defines the persistence contract for mob events.
//! `InMemoryMobEventStore` provides a fast, non-persistent implementation
//! for tests and ephemeral mobs.

use crate::error::MobError;
use crate::event::{MobEvent, NewMobEvent};
use async_trait::async_trait;
use chrono::Utc;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Trait for persisting and querying mob events.
#[async_trait]
pub trait MobEventStore: Send + Sync {
    /// Append a new event to the store.
    ///
    /// Assigns a monotonically increasing cursor and current timestamp.
    async fn append(&self, event: NewMobEvent) -> Result<MobEvent, MobError>;

    /// Poll for events after a given cursor, up to a limit.
    async fn poll(&self, after_cursor: u64, limit: usize) -> Result<Vec<MobEvent>, MobError>;

    /// Replay all events from the beginning.
    async fn replay_all(&self) -> Result<Vec<MobEvent>, MobError>;

    /// Delete all persisted events for this mob store.
    async fn clear(&self) -> Result<(), MobError>;
}

/// In-memory event store for tests and ephemeral mobs.
///
/// Thread-safe via `RwLock`. Events are stored in a `Vec` with monotonically
/// increasing cursors starting at 1.
#[derive(Debug)]
pub struct InMemoryMobEventStore {
    events: Arc<RwLock<Vec<MobEvent>>>,
}

impl InMemoryMobEventStore {
    pub fn new() -> Self {
        Self {
            events: Arc::new(RwLock::new(Vec::new())),
        }
    }
}

impl Default for InMemoryMobEventStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MobEventStore for InMemoryMobEventStore {
    async fn append(&self, event: NewMobEvent) -> Result<MobEvent, MobError> {
        let mut events = self.events.write().await;
        let cursor = events.len() as u64 + 1;
        let stored = MobEvent {
            cursor,
            timestamp: Utc::now(),
            mob_id: event.mob_id,
            kind: event.kind,
        };
        events.push(stored.clone());
        Ok(stored)
    }

    async fn poll(&self, after_cursor: u64, limit: usize) -> Result<Vec<MobEvent>, MobError> {
        let events = self.events.read().await;
        let result: Vec<MobEvent> = events
            .iter()
            .filter(|e| e.cursor > after_cursor)
            .take(limit)
            .cloned()
            .collect();
        Ok(result)
    }

    async fn replay_all(&self) -> Result<Vec<MobEvent>, MobError> {
        let events = self.events.read().await;
        Ok(events.clone())
    }

    async fn clear(&self) -> Result<(), MobError> {
        self.events.write().await.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::MobEventKind;
    use crate::ids::{MeerkatId, MobId, ProfileName};
    use meerkat_core::types::SessionId;
    use uuid::Uuid;

    fn new_event(mob_id: &str, kind: MobEventKind) -> NewMobEvent {
        NewMobEvent {
            mob_id: MobId::from(mob_id),
            kind,
        }
    }

    #[tokio::test]
    async fn test_append_assigns_monotonic_cursors() {
        let store = InMemoryMobEventStore::new();
        let e1 = store
            .append(new_event("mob-1", MobEventKind::MobCompleted))
            .await
            .unwrap();
        let e2 = store
            .append(new_event("mob-1", MobEventKind::MobCompleted))
            .await
            .unwrap();
        assert_eq!(e1.cursor, 1);
        assert_eq!(e2.cursor, 2);
        assert!(e2.timestamp >= e1.timestamp);
    }

    #[tokio::test]
    async fn test_poll_returns_events_after_cursor() {
        let store = InMemoryMobEventStore::new();
        store
            .append(new_event("mob-1", MobEventKind::MobCompleted))
            .await
            .unwrap();
        store
            .append(new_event(
                "mob-1",
                MobEventKind::MeerkatSpawned {
                    meerkat_id: MeerkatId::from("a"),
                    role: ProfileName::from("worker"),
                    session_id: SessionId::from_uuid(Uuid::nil()),
                },
            ))
            .await
            .unwrap();
        store
            .append(new_event(
                "mob-1",
                MobEventKind::PeersWired {
                    a: MeerkatId::from("a"),
                    b: MeerkatId::from("b"),
                },
            ))
            .await
            .unwrap();

        let polled = store.poll(1, 10).await.unwrap();
        assert_eq!(polled.len(), 2);
        assert_eq!(polled[0].cursor, 2);
        assert_eq!(polled[1].cursor, 3);
    }

    #[tokio::test]
    async fn test_poll_respects_limit() {
        let store = InMemoryMobEventStore::new();
        for _ in 0..5 {
            store
                .append(new_event("mob-1", MobEventKind::MobCompleted))
                .await
                .unwrap();
        }
        let polled = store.poll(0, 3).await.unwrap();
        assert_eq!(polled.len(), 3);
    }

    #[tokio::test]
    async fn test_poll_empty_when_no_events_after_cursor() {
        let store = InMemoryMobEventStore::new();
        store
            .append(new_event("mob-1", MobEventKind::MobCompleted))
            .await
            .unwrap();
        let polled = store.poll(1, 10).await.unwrap();
        assert!(polled.is_empty());
    }

    #[tokio::test]
    async fn test_replay_all_returns_all_events() {
        let store = InMemoryMobEventStore::new();
        store
            .append(new_event("mob-1", MobEventKind::MobCompleted))
            .await
            .unwrap();
        store
            .append(new_event(
                "mob-1",
                MobEventKind::MeerkatSpawned {
                    meerkat_id: MeerkatId::from("a"),
                    role: ProfileName::from("worker"),
                    session_id: SessionId::from_uuid(Uuid::nil()),
                },
            ))
            .await
            .unwrap();

        let all = store.replay_all().await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].cursor, 1);
        assert_eq!(all[1].cursor, 2);
    }

    #[tokio::test]
    async fn test_replay_all_empty_store() {
        let store = InMemoryMobEventStore::new();
        let all = store.replay_all().await.unwrap();
        assert!(all.is_empty());
    }
}

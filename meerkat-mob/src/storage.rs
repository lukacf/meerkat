//! MobStorage bundle.
//!
//! Groups the event store with a session store for a mob's isolated storage.

#[cfg(not(target_arch = "wasm32"))]
use crate::store::RedbMobStores;
use crate::store::{
    InMemoryMobEventStore, InMemoryMobRunStore, InMemoryMobSpecStore, MobEventStore, MobRunStore,
    MobSpecStore,
};
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
use std::sync::Arc;

/// Storage bundle for a mob.
///
/// Contains both the mob event store (structural state) and session
/// store (for meerkat sessions). Each mob has its own isolated storage.
pub struct MobStorage {
    /// Event store for mob structural events.
    pub events: Arc<dyn MobEventStore>,
    /// Flow run persistence store.
    pub runs: Arc<dyn MobRunStore>,
    /// Flow spec persistence store.
    pub specs: Arc<dyn MobSpecStore>,
}

impl MobStorage {
    /// Create a storage bundle with in-memory stores (for tests and ephemeral mobs).
    pub fn in_memory() -> Self {
        let (runs, specs) = Self::in_memory_flow_stores();
        Self {
            events: Arc::new(InMemoryMobEventStore::new()),
            runs,
            specs,
        }
    }

    /// Create in-memory run/spec stores for flow persistence.
    pub fn in_memory_flow_stores() -> (Arc<dyn MobRunStore>, Arc<dyn MobSpecStore>) {
        (
            Arc::new(InMemoryMobRunStore::new()),
            Arc::new(InMemoryMobSpecStore::new()),
        )
    }

    /// Build a full storage bundle from a custom event store and in-memory flow stores.
    pub fn with_events(events: Arc<dyn MobEventStore>) -> Self {
        let (runs, specs) = Self::in_memory_flow_stores();
        Self {
            events,
            runs,
            specs,
        }
    }

    /// Create a storage bundle backed by a single shared redb database handle.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn redb(path: impl AsRef<Path>) -> Result<Self, crate::MobError> {
        let stores = RedbMobStores::open(path)?;
        Ok(Self {
            events: Arc::new(stores.event_store()),
            runs: Arc::new(stores.run_store()),
            specs: Arc::new(stores.spec_store()),
        })
    }
}

impl std::fmt::Debug for MobStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MobStorage")
            .field("events", &"<dyn MobEventStore>")
            .field("runs", &"<dyn MobRunStore>")
            .field("specs", &"<dyn MobSpecStore>")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{MobEventKind, NewMobEvent};
    use crate::ids::{MobId, RunId};
    use crate::run::{MobRun, MobRunStatus};
    use chrono::Utc;

    #[tokio::test]
    async fn test_in_memory_storage_creates_working_stores() {
        let storage = MobStorage::in_memory();

        // Event store works
        let event = NewMobEvent {
            mob_id: MobId::from("test"),
            timestamp: None,
            kind: MobEventKind::MobCompleted,
        };
        let stored = storage.events.append(event).await.unwrap();
        assert_eq!(stored.cursor, 1);

        let all = storage.events.replay_all().await.unwrap();
        assert_eq!(all.len(), 1);
    }

    #[tokio::test]
    async fn test_in_memory_flow_stores_create_working_run_and_spec_stores() {
        let (runs, specs) = MobStorage::in_memory_flow_stores();
        let run = MobRun {
            run_id: RunId::new(),
            mob_id: MobId::from("mob"),
            flow_id: crate::FlowId::from("flow"),
            status: MobRunStatus::Pending,
            activation_params: serde_json::json!({}),
            created_at: Utc::now(),
            completed_at: None,
            step_ledger: Vec::new(),
            failure_ledger: Vec::new(),
        };
        runs.create_run(run.clone()).await.unwrap();
        assert!(runs.get_run(&run.run_id).await.unwrap().is_some());

        let definition = crate::definition::MobDefinition::from_toml(
            r#"
[mob]
id = "mob"
[profiles.worker]
model = "test"
"#,
        )
        .unwrap();
        let revision = specs
            .put_spec(&MobId::from("mob"), &definition, None)
            .await
            .unwrap();
        assert_eq!(revision, 1);
    }

    #[tokio::test]
    async fn test_redb_storage_uses_shared_database_handle_for_all_stores() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("mob.redb");
        let storage = MobStorage::redb(&db_path).unwrap();

        let event = NewMobEvent {
            mob_id: MobId::from("mob"),
            timestamp: None,
            kind: MobEventKind::MobCompleted,
        };
        storage.events.append(event).await.unwrap();

        let run = MobRun {
            run_id: RunId::new(),
            mob_id: MobId::from("mob"),
            flow_id: crate::FlowId::from("flow"),
            status: MobRunStatus::Pending,
            activation_params: serde_json::json!({}),
            created_at: Utc::now(),
            completed_at: None,
            step_ledger: Vec::new(),
            failure_ledger: Vec::new(),
        };
        storage.runs.create_run(run.clone()).await.unwrap();
        assert!(storage.runs.get_run(&run.run_id).await.unwrap().is_some());

        let definition = crate::definition::MobDefinition::from_toml(
            r#"
[mob]
id = "mob"
[profiles.worker]
model = "test"
"#,
        )
        .unwrap();
        let revision = storage
            .specs
            .put_spec(&MobId::from("mob"), &definition, None)
            .await
            .unwrap();
        assert_eq!(revision, 1);
    }
}

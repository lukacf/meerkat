//! MobStorage bundle.
//!
//! Groups the event store with a session store for a mob's isolated storage.

use crate::definition::MobDefinition;
use crate::error::MobError;
use crate::event::{MobEvent, MobEventKind, NewMobEvent};
use crate::ids::MobId;
use crate::run::MobRun;
#[cfg(not(target_arch = "wasm32"))]
use crate::store::SqliteMobStores;
use crate::store::{
    InMemoryMobEventStore, InMemoryMobRunStore, InMemoryMobRuntimeMetadataStore,
    InMemoryMobSpecStore, InMemoryRealmProfileStore, MobEventStore, MobRunStore,
    MobRuntimeMetadataStore, MobSpecStore, RealmProfileStore,
};
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
use std::sync::Arc;

/// Mob-owned compatibility handoff for the pre-durable-store CLI registry.
///
/// This is the only public legacy-registry import carrier. Construction replays
/// the event stream through generated MobMachine recovery before the private
/// event payloads can be written to a store.
pub struct MobLegacyRegistryHandoff {
    mob_id: MobId,
    events: Vec<MobEvent>,
    terminal_runs: Vec<MobRun>,
    generated_recovery_phase: crate::runtime::MobState,
}

impl MobLegacyRegistryHandoff {
    /// Validate a legacy CLI registry snapshot through generated MobMachine
    /// recovery and return an opaque handoff that can be imported into
    /// [`MobStorage`].
    pub fn try_from_legacy_registry_snapshot(
        mob_id: MobId,
        legacy_definition: Option<MobDefinition>,
        events: Vec<MobEvent>,
        terminal_runs: Vec<MobRun>,
    ) -> Result<Self, MobError> {
        let events = if events.is_empty() {
            let definition = legacy_definition.ok_or_else(|| {
                MobError::Internal(format!(
                    "legacy mob registry entry '{mob_id}' has no events and no definition"
                ))
            })?;
            if definition.id != mob_id {
                return Err(MobError::Internal(format!(
                    "legacy mob registry definition id mismatch: key='{mob_id}' definition='{}'",
                    definition.id
                )));
            }
            vec![MobEvent {
                cursor: 0,
                timestamp: chrono::Utc::now(),
                mob_id: mob_id.clone(),
                kind: MobEventKind::MobCreated {
                    definition: Box::new(definition),
                },
            }]
        } else {
            events
        };

        for event in &events {
            if event.mob_id != mob_id {
                return Err(MobError::Internal(format!(
                    "legacy mob registry event mob_id mismatch: key='{mob_id}' event='{}'",
                    event.mob_id
                )));
            }
        }

        let definition = events
            .iter()
            .rev()
            .find_map(|event| match &event.kind {
                MobEventKind::MobCreated { definition } => Some(definition.as_ref().clone()),
                _ => None,
            })
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "legacy mob registry entry '{mob_id}' has no MobCreated event"
                ))
            })?;
        if definition.id != mob_id {
            return Err(MobError::Internal(format!(
                "legacy mob registry MobCreated id mismatch: key='{mob_id}' definition='{}'",
                definition.id
            )));
        }

        let epoch_start = events
            .iter()
            .rposition(|event| matches!(event.kind, MobEventKind::MobReset))
            .map_or(0, |pos| pos + 1);
        let generated_recovery_phase = crate::runtime::validate_mob_events_with_generated_recovery(
            &events[epoch_start..],
            &definition,
        )?;

        let mut validated_terminal_runs = Vec::with_capacity(terminal_runs.len());
        for mut run in terminal_runs {
            if run.mob_id != mob_id {
                return Err(MobError::Internal(format!(
                    "legacy mob registry run mob_id mismatch: key='{mob_id}' run='{}'",
                    run.mob_id
                )));
            }
            if !run.status().is_terminal() {
                return Err(MobError::Internal(format!(
                    "legacy mob registry run '{}' is not terminal",
                    run.run_id
                )));
            }
            crate::runtime::recovery::reconcile_run_state(&mut run).map_err(|error| {
                MobError::Internal(format!(
                    "legacy mob registry run '{}' failed generated recovery validation: {error}",
                    run.run_id
                ))
            })?;
            validated_terminal_runs.push(run);
        }

        Ok(Self {
            mob_id,
            events,
            terminal_runs: validated_terminal_runs,
            generated_recovery_phase,
        })
    }
}

/// Storage bundle for a mob.
///
/// Contains both the mob event store (structural state) and session
/// store (for meerkat sessions). Each mob has its own isolated storage.
pub struct MobStorage {
    /// Event store for mob structural events.
    pub(crate) events: Arc<dyn MobEventStore>,
    /// Flow run persistence store.
    pub(crate) runs: Arc<dyn MobRunStore>,
    /// Flow spec persistence store.
    pub(crate) specs: Arc<dyn MobSpecStore>,
    /// Runtime metadata store for supervisor authority and compatibility projections.
    pub(crate) runtime_metadata: Arc<dyn MobRuntimeMetadataStore>,
    /// Realm-scoped reusable profile store.
    pub(crate) realm_profiles: Option<Arc<dyn RealmProfileStore>>,
}

impl MobStorage {
    /// Create a storage bundle with in-memory stores (for tests and ephemeral mobs).
    pub fn in_memory() -> Self {
        let (runs, specs) = Self::in_memory_flow_stores();
        Self {
            events: Arc::new(InMemoryMobEventStore::new()),
            runs,
            specs,
            runtime_metadata: Arc::new(InMemoryMobRuntimeMetadataStore::new()),
            realm_profiles: Some(Arc::new(InMemoryRealmProfileStore::new())),
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
            runtime_metadata: Arc::new(InMemoryMobRuntimeMetadataStore::new()),
            realm_profiles: Some(Arc::new(InMemoryRealmProfileStore::new())),
        }
    }

    /// Build a storage bundle from an event store plus an existing runtime metadata store.
    pub fn with_events_and_runtime_metadata(
        events: Arc<dyn MobEventStore>,
        runtime_metadata: Arc<dyn MobRuntimeMetadataStore>,
    ) -> Self {
        let (runs, specs) = Self::in_memory_flow_stores();
        Self {
            events,
            runs,
            specs,
            runtime_metadata,
            realm_profiles: Some(Arc::new(InMemoryRealmProfileStore::new())),
        }
    }

    /// Build a storage bundle from custom store implementations.
    pub fn custom(
        events: Arc<dyn MobEventStore>,
        runs: Arc<dyn MobRunStore>,
        specs: Arc<dyn MobSpecStore>,
    ) -> Self {
        Self::custom_with_runtime_metadata(
            events,
            runs,
            specs,
            Arc::new(InMemoryMobRuntimeMetadataStore::new()),
        )
    }

    /// Build a storage bundle from custom store implementations, including runtime metadata.
    pub fn custom_with_runtime_metadata(
        events: Arc<dyn MobEventStore>,
        runs: Arc<dyn MobRunStore>,
        specs: Arc<dyn MobSpecStore>,
        runtime_metadata: Arc<dyn MobRuntimeMetadataStore>,
    ) -> Self {
        Self {
            events,
            runs,
            specs,
            runtime_metadata,
            realm_profiles: None,
        }
    }

    /// Import a generated-recovery-validated legacy registry handoff.
    pub async fn import_validated_legacy_registry(
        &self,
        handoff: MobLegacyRegistryHandoff,
    ) -> Result<(), MobError> {
        let MobLegacyRegistryHandoff {
            mob_id: _mob_id,
            events,
            terminal_runs,
            generated_recovery_phase: _generated_recovery_phase,
        } = handoff;
        let events = events
            .into_iter()
            .map(|event| NewMobEvent {
                mob_id: event.mob_id,
                timestamp: Some(event.timestamp),
                kind: event.kind,
            })
            .collect();
        self.events.append_batch(events).await?;

        for run in terminal_runs {
            self.runs.create_run(run).await?;
        }
        Ok(())
    }

    /// Return whether the structural event log is empty.
    pub async fn is_event_log_empty(&self) -> Result<bool, crate::store::MobStoreError> {
        Ok(self.events.latest_cursor().await? == 0)
    }

    /// Create a storage bundle backed by a single SQLite database file.
    ///
    /// Uses WAL mode — no exclusive file lock is held, so the same path
    /// can be reopened after drop within the same process.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn persistent(path: impl AsRef<Path>) -> Result<Self, crate::MobError> {
        let stores = SqliteMobStores::open(path)?;
        Ok(Self {
            events: Arc::new(stores.event_store()),
            runs: Arc::new(stores.run_store()),
            specs: Arc::new(stores.spec_store()),
            runtime_metadata: Arc::new(stores.runtime_metadata_store()),
            realm_profiles: Some(Arc::new(stores.realm_profile_store())),
        })
    }
}

impl std::fmt::Debug for MobStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MobStorage")
            .field("events", &"<dyn MobEventStore>")
            .field("runs", &"<dyn MobRunStore>")
            .field("specs", &"<dyn MobSpecStore>")
            .field("runtime_metadata", &"<dyn MobRuntimeMetadataStore>")
            .field(
                "realm_profiles",
                &self
                    .realm_profiles
                    .as_ref()
                    .map(|_| "<dyn RealmProfileStore>"),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{MobEventKind, NewMobEvent};
    use crate::ids::{MobId, RunId, StepId};
    use crate::run::{MobRun, MobRunStatus};

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
    async fn test_validated_legacy_registry_handoff_imports_events() {
        let definition = crate::definition::MobDefinition::from_toml(
            r#"
[mob]
id = "mob"
[profiles.worker]
model = "test"
"#,
        )
        .unwrap();
        let mob_id = definition.id.clone();
        let handoff = MobLegacyRegistryHandoff::try_from_legacy_registry_snapshot(
            mob_id.clone(),
            Some(definition),
            Vec::new(),
            Vec::new(),
        )
        .unwrap();
        assert_eq!(
            handoff.generated_recovery_phase,
            crate::runtime::MobState::Running
        );

        let storage = MobStorage::in_memory();
        storage
            .import_validated_legacy_registry(handoff)
            .await
            .unwrap();

        let events = storage.events.replay_all().await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].mob_id, mob_id);
        assert!(matches!(events[0].kind, MobEventKind::MobCreated { .. }));
    }

    #[test]
    fn test_legacy_registry_handoff_rejects_machine_rejected_events() {
        let definition = crate::definition::MobDefinition::from_toml(
            r#"
[mob]
id = "mob"
[profiles.worker]
model = "test"
"#,
        )
        .unwrap();
        let mob_id = definition.id.clone();
        let created = MobEvent {
            cursor: 1,
            timestamp: chrono::Utc::now(),
            mob_id: mob_id.clone(),
            kind: MobEventKind::MobCreated {
                definition: Box::new(definition.clone()),
            },
        };
        let completed = MobEvent {
            cursor: 2,
            timestamp: chrono::Utc::now(),
            mob_id: mob_id.clone(),
            kind: MobEventKind::MobCompleted,
        };
        let duplicate_completed = MobEvent {
            cursor: 3,
            timestamp: chrono::Utc::now(),
            mob_id: mob_id.clone(),
            kind: MobEventKind::MobCompleted,
        };

        let error = match MobLegacyRegistryHandoff::try_from_legacy_registry_snapshot(
            mob_id,
            Some(definition),
            vec![created, completed, duplicate_completed],
            Vec::new(),
        ) {
            Ok(_) => panic!("duplicate completed events should be rejected"),
            Err(error) => error,
        };
        assert!(
            error.to_string().contains("MobMachine seeded authority"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn test_in_memory_flow_stores_create_working_run_and_spec_stores() {
        let (runs, specs) = MobStorage::in_memory_flow_stores();
        let run = MobRun::authority_backed_for_steps(
            RunId::new(),
            MobId::from("mob"),
            crate::FlowId::from("flow"),
            [StepId::from("step-1")],
            MobRunStatus::Pending,
            serde_json::json!({}),
        )
        .expect("authority-backed run");
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
    async fn test_persistent_storage_uses_shared_database_for_all_stores() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("mob.db");
        let storage = MobStorage::persistent(&db_path).unwrap();

        let event = NewMobEvent {
            mob_id: MobId::from("mob"),
            timestamp: None,
            kind: MobEventKind::MobCompleted,
        };
        storage.events.append(event).await.unwrap();

        let run = MobRun::authority_backed_for_steps(
            RunId::new(),
            MobId::from("mob"),
            crate::FlowId::from("flow"),
            [StepId::from("step-1")],
            MobRunStatus::Pending,
            serde_json::json!({}),
        )
        .expect("authority-backed run");
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

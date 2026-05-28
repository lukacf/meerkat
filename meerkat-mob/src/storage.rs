//! MobStorage bundle.
//!
//! Groups the event store with a session store for a mob's isolated storage.

#[cfg(not(target_arch = "wasm32"))]
use crate::store::SqliteMobStores;
use crate::store::{
    InMemoryMobEventStore, InMemoryMobRunStore, InMemoryMobRuntimeMetadataStore,
    InMemoryMobSpecStore, InMemoryRealmProfileStore, MobEventStore, MobRunStore,
    MobRuntimeMetadataStore, MobSpecStore, RealmProfileStore, authority_validating_mob_run_store,
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
            authority_validating_mob_run_store(Arc::new(InMemoryMobRunStore::new())),
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
            runs: authority_validating_mob_run_store(runs),
            specs,
            runtime_metadata,
            realm_profiles: None,
        }
    }

    /// Attach the realm-scoped reusable profile store used by mob runtimes.
    pub fn with_realm_profile_store(
        mut self,
        realm_profiles: Option<Arc<dyn RealmProfileStore>>,
    ) -> Self {
        self.realm_profiles = realm_profiles;
        self
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
            runs: authority_validating_mob_run_store(Arc::new(stores.run_store())),
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
    use crate::ids::{FlowId, FrameId, LoopId, LoopInstanceId, MobId, RunId, StepId};
    use crate::run::{
        FailureLedgerEntry, FrameSnapshot, LoopIterationLedgerEntry, LoopSnapshot, MobRun,
        MobRunProvenanceAuthority, MobRunStatus, StepLedgerEntry,
    };
    use crate::store::MobStoreError;
    use std::sync::Mutex;

    struct ForgedRunStore {
        run: Mutex<Option<MobRun>>,
    }

    impl ForgedRunStore {
        fn new(run: Option<MobRun>) -> Self {
            Self {
                run: Mutex::new(run),
            }
        }
    }

    #[async_trait::async_trait]
    impl MobRunStore for ForgedRunStore {
        async fn create_run(&self, run: MobRun) -> Result<(), MobStoreError> {
            *self.run.lock().expect("forged run mutex") = Some(run);
            Ok(())
        }

        async fn get_run(&self, run_id: &RunId) -> Result<Option<MobRun>, MobStoreError> {
            Ok(self
                .run
                .lock()
                .expect("forged run mutex")
                .as_ref()
                .filter(|run| &run.run_id == run_id)
                .cloned())
        }

        async fn list_runs(
            &self,
            mob_id: &MobId,
            flow_id: Option<&FlowId>,
        ) -> Result<Vec<MobRun>, MobStoreError> {
            Ok(self
                .run
                .lock()
                .expect("forged run mutex")
                .as_ref()
                .filter(|run| &run.mob_id == mob_id)
                .filter(|run| flow_id.is_none_or(|flow_id| &run.flow_id == flow_id))
                .cloned()
                .into_iter()
                .collect())
        }

        async fn cas_flow_state_with_authority(
            &self,
            _run_id: &RunId,
            _expected: &crate::run::flow_run::State,
            _next: &crate::run::flow_run::State,
            _authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
        ) -> Result<bool, MobStoreError> {
            Err(MobStoreError::Internal(
                "not implemented in forged store".into(),
            ))
        }

        async fn cas_run_snapshot_with_authority(
            &self,
            _run_id: &RunId,
            _expected_status: MobRunStatus,
            _expected_flow_state: &crate::run::flow_run::State,
            _next_status: MobRunStatus,
            _next_flow_state: &crate::run::flow_run::State,
            _authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
        ) -> Result<bool, MobStoreError> {
            Err(MobStoreError::Internal(
                "not implemented in forged store".into(),
            ))
        }

        async fn append_step_entry_with_authority(
            &self,
            _run_id: &RunId,
            _entry: StepLedgerEntry,
            _authority: MobRunProvenanceAuthority,
        ) -> Result<(), MobStoreError> {
            Err(MobStoreError::Internal(
                "not implemented in forged store".into(),
            ))
        }

        async fn append_step_entry_if_absent_with_authority(
            &self,
            _run_id: &RunId,
            _entry: StepLedgerEntry,
            _authority: MobRunProvenanceAuthority,
        ) -> Result<bool, MobStoreError> {
            Err(MobStoreError::Internal(
                "not implemented in forged store".into(),
            ))
        }

        async fn append_failure_entry_with_authority(
            &self,
            _run_id: &RunId,
            _entry: FailureLedgerEntry,
            _authority: MobRunProvenanceAuthority,
        ) -> Result<(), MobStoreError> {
            Err(MobStoreError::Internal(
                "not implemented in forged store".into(),
            ))
        }

        async fn cas_frame_state_with_authority(
            &self,
            _run_id: &RunId,
            _frame_id: &FrameId,
            _expected: Option<&FrameSnapshot>,
            _next: FrameSnapshot,
            _authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
        ) -> Result<bool, MobStoreError> {
            Err(MobStoreError::Internal(
                "not implemented in forged store".into(),
            ))
        }

        #[allow(clippy::too_many_arguments)]
        async fn cas_grant_node_slot_with_authority(
            &self,
            _run_id: &RunId,
            _expected_run_state: &crate::run::flow_run::State,
            _next_run_state: crate::run::flow_run::State,
            _frame_id: &FrameId,
            _expected_frame: &FrameSnapshot,
            _next_frame: FrameSnapshot,
            _authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
        ) -> Result<bool, MobStoreError> {
            Err(MobStoreError::Internal(
                "not implemented in forged store".into(),
            ))
        }

        #[allow(clippy::too_many_arguments)]
        async fn cas_complete_step_and_record_output_with_authority(
            &self,
            _run_id: &RunId,
            _frame_id: &FrameId,
            _expected_frame: &FrameSnapshot,
            _next_frame: FrameSnapshot,
            _step_output_key: String,
            _step_output: serde_json::Value,
            _loop_context: Option<(&LoopId, u64)>,
            _authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
        ) -> Result<bool, MobStoreError> {
            Err(MobStoreError::Internal(
                "not implemented in forged store".into(),
            ))
        }

        #[allow(clippy::too_many_arguments)]
        async fn cas_start_loop_with_authority(
            &self,
            _run_id: &RunId,
            _loop_instance_id: &LoopInstanceId,
            _expected_run_state: &crate::run::flow_run::State,
            _next_run_state: crate::run::flow_run::State,
            _frame_id: &FrameId,
            _expected_frame: &FrameSnapshot,
            _next_frame: FrameSnapshot,
            _initial_loop: LoopSnapshot,
            _authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
        ) -> Result<bool, MobStoreError> {
            Err(MobStoreError::Internal(
                "not implemented in forged store".into(),
            ))
        }

        #[allow(clippy::too_many_arguments)]
        async fn cas_loop_request_body_frame_with_authority(
            &self,
            _run_id: &RunId,
            _loop_instance_id: &LoopInstanceId,
            _expected_loop: &LoopSnapshot,
            _next_loop: LoopSnapshot,
            _expected_run_state: &crate::run::flow_run::State,
            _next_run_state: crate::run::flow_run::State,
            _authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
        ) -> Result<bool, MobStoreError> {
            Err(MobStoreError::Internal(
                "not implemented in forged store".into(),
            ))
        }

        #[allow(clippy::too_many_arguments)]
        async fn cas_grant_body_frame_start_with_authority(
            &self,
            _run_id: &RunId,
            _loop_instance_id: &LoopInstanceId,
            _expected_loop: &LoopSnapshot,
            _next_loop: LoopSnapshot,
            _frame_id: &FrameId,
            _initial_frame: FrameSnapshot,
            _ledger_entry: LoopIterationLedgerEntry,
            _expected_run_state: &crate::run::flow_run::State,
            _next_run_state: crate::run::flow_run::State,
            _authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
        ) -> Result<bool, MobStoreError> {
            Err(MobStoreError::Internal(
                "not implemented in forged store".into(),
            ))
        }

        #[allow(clippy::too_many_arguments)]
        async fn cas_complete_body_frame_with_authority(
            &self,
            _run_id: &RunId,
            _loop_instance_id: &LoopInstanceId,
            _expected_loop: &LoopSnapshot,
            _next_loop: LoopSnapshot,
            _frame_id: &FrameId,
            _expected_frame: &FrameSnapshot,
            _next_frame: FrameSnapshot,
            _expected_run_state: &crate::run::flow_run::State,
            _next_run_state: crate::run::flow_run::State,
            _authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
        ) -> Result<bool, MobStoreError> {
            Err(MobStoreError::Internal(
                "not implemented in forged store".into(),
            ))
        }

        #[allow(clippy::too_many_arguments)]
        async fn cas_complete_loop_with_authority(
            &self,
            _run_id: &RunId,
            _loop_instance_id: &LoopInstanceId,
            _expected_loop: &LoopSnapshot,
            _next_loop: LoopSnapshot,
            _frame_id: &FrameId,
            _expected_frame: &FrameSnapshot,
            _next_frame: FrameSnapshot,
            _expected_run_state: &crate::run::flow_run::State,
            _next_run_state: crate::run::flow_run::State,
            _authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
        ) -> Result<bool, MobStoreError> {
            Err(MobStoreError::Internal(
                "not implemented in forged store".into(),
            ))
        }
    }

    fn forged_status_run() -> MobRun {
        let mut run = MobRun::authority_backed_for_steps(
            RunId::new(),
            MobId::from("mob"),
            crate::FlowId::from("flow"),
            [StepId::from("step-1")],
            MobRunStatus::Pending,
            serde_json::json!({}),
        )
        .expect("authority-backed run");
        run.status = MobRunStatus::Completed;
        run
    }

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
    async fn custom_run_store_rejects_forged_lifecycle_projection() {
        let forged = forged_status_run();
        let storage = MobStorage::custom(
            Arc::new(InMemoryMobEventStore::new()),
            Arc::new(ForgedRunStore::new(Some(forged.clone()))),
            Arc::new(InMemoryMobSpecStore::new()),
        );

        let read_error = storage
            .runs
            .get_run(&forged.run_id)
            .await
            .expect_err("custom store read must reject forged run projection");
        assert!(
            read_error
                .to_string()
                .contains("not authorized by MobMachine"),
            "unexpected custom read error: {read_error}"
        );

        let list_error = storage
            .runs
            .list_runs(&forged.mob_id, None)
            .await
            .expect_err("custom store list must reject forged run projection");
        assert!(
            list_error
                .to_string()
                .contains("not authorized by MobMachine"),
            "unexpected custom list error: {list_error}"
        );

        let write_error = storage
            .runs
            .create_run(forged)
            .await
            .expect_err("custom store create must reject forged run projection");
        assert!(
            write_error
                .to_string()
                .contains("not authorized by MobMachine"),
            "unexpected custom create error: {write_error}"
        );
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

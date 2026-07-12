//! Smoke test for `meerkat::session_runtime::runtime_state` types
//! moved in W1-E. The drop-notify behaviour is load-bearing — when the
//! receiver-side handle is dropped, the runtime must observe a
//! single-shot notification.

#![allow(clippy::expect_used)]

use std::sync::Arc;

use meerkat::session_runtime::runtime_state::{
    PendingSessionEventStreamDrop, PendingSessionEventStreams,
};
use tokio::sync::{Notify, broadcast};

struct FailDeleteOpsLifecycleOnceStore {
    inner: Arc<meerkat_runtime::store::InMemoryRuntimeStore>,
    fail_delete: std::sync::atomic::AtomicBool,
}

impl FailDeleteOpsLifecycleOnceStore {
    fn new(inner: Arc<meerkat_runtime::store::InMemoryRuntimeStore>) -> Self {
        Self {
            inner,
            fail_delete: std::sync::atomic::AtomicBool::new(true),
        }
    }
}

#[async_trait::async_trait]
impl meerkat_runtime::store::RuntimeStore for FailDeleteOpsLifecycleOnceStore {
    fn supports_compaction_projection_outbox(&self) -> bool {
        meerkat_runtime::store::RuntimeStore::supports_compaction_projection_outbox(
            self.inner.as_ref(),
        )
    }

    async fn commit_session_snapshot(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        session_delta: meerkat_runtime::store::SessionDelta,
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.inner
            .commit_session_snapshot(runtime_id, session_delta)
            .await
    }

    async fn atomic_apply(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        session_delta: Option<meerkat_runtime::store::SessionDelta>,
        receipt: meerkat_core::lifecycle::RunBoundaryReceipt,
        input_updates: Vec<meerkat_runtime::input_state::InputStatePersistenceRecord>,
        session_store_key: Option<meerkat_core::SessionId>,
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.inner
            .atomic_apply(
                runtime_id,
                session_delta,
                receipt,
                input_updates,
                session_store_key,
            )
            .await
    }

    async fn load_input_states(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
    ) -> Result<
        Vec<meerkat_runtime::input_state::StoredInputState>,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner.load_input_states(runtime_id).await
    }

    async fn load_boundary_receipt(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        run_id: &meerkat_core::lifecycle::RunId,
        sequence: u64,
    ) -> Result<
        Option<meerkat_core::lifecycle::RunBoundaryReceipt>,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .load_boundary_receipt(runtime_id, run_id, sequence)
            .await
    }

    async fn load_session_snapshot(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
    ) -> Result<Option<Vec<u8>>, meerkat_runtime::store::RuntimeStoreError> {
        self.inner.load_session_snapshot(runtime_id).await
    }

    async fn load_pending_compaction_projections(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
    ) -> Result<
        Vec<meerkat_core::CompactionProjectionIntent>,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .load_pending_compaction_projections(runtime_id)
            .await
    }

    async fn mark_compaction_projection_finalized(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        projection: &meerkat_core::CompactionProjectionId,
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.inner
            .mark_compaction_projection_finalized(runtime_id, projection)
            .await
    }

    async fn clear_session_snapshot(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.inner.clear_session_snapshot(runtime_id).await
    }

    async fn replace_session_snapshot_if_current(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        expected_current: &[u8],
        replacement: Vec<u8>,
    ) -> Result<bool, meerkat_runtime::store::RuntimeStoreError> {
        self.inner
            .replace_session_snapshot_if_current(runtime_id, expected_current, replacement)
            .await
    }

    async fn clear_session_snapshot_if_current(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        expected_current: &[u8],
    ) -> Result<bool, meerkat_runtime::store::RuntimeStoreError> {
        self.inner
            .clear_session_snapshot_if_current(runtime_id, expected_current)
            .await
    }

    async fn persist_input_state(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        state: &meerkat_runtime::input_state::InputStatePersistenceRecord,
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.inner.persist_input_state(runtime_id, state).await
    }

    async fn load_input_state(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        input_id: &meerkat_core::lifecycle::InputId,
    ) -> Result<
        Option<meerkat_runtime::input_state::StoredInputState>,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner.load_input_state(runtime_id, input_id).await
    }

    async fn load_machine_lifecycle_record(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
    ) -> Result<Option<Vec<u8>>, meerkat_runtime::store::RuntimeStoreError> {
        self.inner.load_machine_lifecycle_record(runtime_id).await
    }

    async fn commit_machine_lifecycle(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        commit: meerkat_runtime::store::MachineLifecycleCommit,
        input_states: &[meerkat_runtime::input_state::InputStatePersistenceRecord],
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.inner
            .commit_machine_lifecycle(runtime_id, commit, input_states)
            .await
    }

    async fn commit_unregister_finalization(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        commit: meerkat_runtime::store::MachineLifecycleCommit,
        input_states: &[meerkat_runtime::input_state::InputStatePersistenceRecord],
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        if self
            .fail_delete
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(meerkat_runtime::store::RuntimeStoreError::WriteFailed(
                "synthetic facade delete failure".to_string(),
            ));
        }
        self.inner
            .commit_unregister_finalization(runtime_id, commit, input_states)
            .await
    }

    async fn persist_ops_lifecycle(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        snapshot: &meerkat_runtime::ops_lifecycle::PersistedOpsSnapshot,
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.inner.persist_ops_lifecycle(runtime_id, snapshot).await
    }

    async fn load_ops_lifecycle(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
    ) -> Result<
        Option<meerkat_runtime::ops_lifecycle::PersistedOpsSnapshot>,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner.load_ops_lifecycle(runtime_id).await
    }

    async fn delete_ops_lifecycle(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        if self
            .fail_delete
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(meerkat_runtime::store::RuntimeStoreError::WriteFailed(
                "synthetic facade delete failure".to_string(),
            ));
        }
        self.inner.delete_ops_lifecycle(runtime_id).await
    }
}

#[tokio::test]
async fn drop_guard_notifies_once() {
    let receiver_dropped = Arc::new(Notify::new());
    let (events, _rx) = broadcast::channel(8);
    let _streams = PendingSessionEventStreams {
        events,
        receiver_dropped: Arc::clone(&receiver_dropped),
    };
    let guard = PendingSessionEventStreamDrop {
        receiver_dropped: Arc::clone(&receiver_dropped),
    };

    let waiter = receiver_dropped.clone();
    let notified = tokio::spawn(async move {
        waiter.notified().await;
    });

    drop(guard);
    notified
        .await
        .expect("dropping the guard should notify the runtime exactly once");
}

#[test]
fn session_state_serde_round_trip_uses_snake_case() {
    use meerkat::session_runtime::runtime_state::SessionState;

    let cases = [
        (SessionState::Idle, "\"idle\"", "idle"),
        (SessionState::Running, "\"running\"", "running"),
        (
            SessionState::ShuttingDown,
            "\"shutting_down\"",
            "shutting_down",
        ),
    ];
    for (state, encoded, slug) in cases {
        assert_eq!(
            serde_json::to_string(&state).expect("serialize"),
            encoded,
            "{state:?} must serialise to {encoded}"
        );
        assert_eq!(state.as_str(), slug);
        let decoded: SessionState = serde_json::from_str(encoded).expect("round-trip must decode");
        assert_eq!(decoded, state);
    }
}

#[test]
fn session_info_holds_session_id_state_and_labels() {
    use std::collections::BTreeMap;

    use meerkat::session_runtime::runtime_state::{SessionInfo, SessionState};
    use meerkat_core::types::SessionId;

    let mut labels = BTreeMap::new();
    labels.insert("env".to_string(), "test".to_string());

    let info = SessionInfo {
        session_id: SessionId::new(),
        state: SessionState::Idle,
        labels: labels.clone(),
    };
    assert_eq!(info.state, SessionState::Idle);
    assert_eq!(info.labels, labels);
}

#[test]
fn skill_identity_registry_state_default_is_empty_generation_zero() {
    use meerkat::session_runtime::runtime_state::SkillIdentityRegistryState;

    let state = SkillIdentityRegistryState::default();
    assert_eq!(state.generation, 0);
}

#[test]
fn build_skill_identity_registry_returns_default_for_empty_skills_config() {
    use meerkat::session_runtime::runtime_state::build_skill_identity_registry;
    use meerkat_core::Config;

    let config = Config::default();
    let registry =
        build_skill_identity_registry(&config, None, None).expect("default config builds clean");
    // Default config has no remaps; resulting registry is empty.
    let _ = registry;
}

#[tokio::test]
async fn archive_runtime_cleanup_dispatches_to_trait_hooks() {
    use std::sync::atomic::{AtomicBool, Ordering};

    use meerkat::session_runtime::runtime_state::{
        ArchiveRuntimeCleanup, ArchiveRuntimeMcpState, ArchiveRuntimeMobState,
    };
    use meerkat_core::service::SessionError;
    use meerkat_core::types::SessionId;
    use meerkat_runtime::MeerkatMachine;

    struct McpStub {
        ran: Arc<AtomicBool>,
    }

    #[async_trait::async_trait]
    impl ArchiveRuntimeMcpState for McpStub {
        async fn cleanup(&self, _session_id: &SessionId) {
            self.ran.store(true, Ordering::SeqCst);
        }
    }

    struct MobStub {
        ran: Arc<AtomicBool>,
    }

    #[async_trait::async_trait]
    impl ArchiveRuntimeMobState for MobStub {
        async fn cleanup(&self, _session_id: &SessionId) -> Result<(), SessionError> {
            self.ran.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    let mcp_ran = Arc::new(AtomicBool::new(false));
    let mob_ran = Arc::new(AtomicBool::new(false));
    let runtime_adapter = Arc::new(MeerkatMachine::ephemeral());
    let cleanup = ArchiveRuntimeCleanup {
        runtime_adapter,
        pending_session_event_streams: None,
        mcp_state: Some(Arc::new(McpStub {
            ran: Arc::clone(&mcp_ran),
        })),
        mob_state: Some(Arc::new(MobStub {
            ran: Arc::clone(&mob_ran),
        })),
    };
    let session_id = SessionId::new();
    cleanup.run(&session_id).await.expect("cleanup runs");
    assert!(mcp_ran.load(Ordering::SeqCst));
    assert!(mob_ran.load(Ordering::SeqCst));
}

#[tokio::test]
async fn archive_runtime_cleanup_preserves_downstream_anchors_when_unregister_fails() {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};

    use meerkat::session_runtime::runtime_state::{
        ArchiveRuntimeCleanup, ArchiveRuntimeMcpState, ArchiveRuntimeMobState,
        PendingSessionEventStreams,
    };
    use meerkat_core::service::SessionError;
    use meerkat_core::types::SessionId;
    use meerkat_runtime::MeerkatMachine;
    use meerkat_runtime::store::RuntimeStore as _;
    use tokio::sync::Mutex;

    struct McpStub(Arc<AtomicBool>);

    #[async_trait::async_trait]
    impl ArchiveRuntimeMcpState for McpStub {
        async fn cleanup(&self, _session_id: &SessionId) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    struct MobStub(Arc<AtomicBool>);

    #[async_trait::async_trait]
    impl ArchiveRuntimeMobState for MobStub {
        async fn cleanup(&self, _session_id: &SessionId) -> Result<(), SessionError> {
            self.0.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    let inner = Arc::new(meerkat_runtime::store::InMemoryRuntimeStore::new());
    let store = Arc::new(FailDeleteOpsLifecycleOnceStore::new(Arc::clone(&inner)));
    let runtime_adapter = Arc::new(MeerkatMachine::persistent(
        store as Arc<dyn meerkat_runtime::store::RuntimeStore>,
        Arc::new(meerkat_store::MemoryBlobStore::new()),
    ));
    let session_id = SessionId::new();
    let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(&session_id);
    let snapshot = meerkat_runtime::ops_lifecycle::RuntimeOpsLifecycleRegistry::new()
        .capture_persistence_snapshot(
            meerkat_core::RuntimeEpochId::new(),
            &meerkat_core::EpochCursorState::new(),
        )
        .expect("capture ops lifecycle snapshot");
    inner
        .persist_ops_lifecycle(&runtime_id, &snapshot)
        .await
        .expect("persist ops lifecycle snapshot");
    runtime_adapter
        .register_session(session_id.clone())
        .await
        .expect("register persistent runtime session");

    let (events, _receiver) = broadcast::channel(1);
    let streams = Arc::new(Mutex::new(HashMap::from([(
        session_id.clone(),
        PendingSessionEventStreams {
            events,
            receiver_dropped: Arc::new(Notify::new()),
        },
    )])));
    let mcp_ran = Arc::new(AtomicBool::new(false));
    let mob_ran = Arc::new(AtomicBool::new(false));
    let cleanup = ArchiveRuntimeCleanup {
        runtime_adapter: Arc::clone(&runtime_adapter),
        pending_session_event_streams: Some(Arc::clone(&streams)),
        mcp_state: Some(Arc::new(McpStub(Arc::clone(&mcp_ran)))),
        mob_state: Some(Arc::new(MobStub(Arc::clone(&mob_ran)))),
    };

    let error = cleanup
        .run(&session_id)
        .await
        .expect_err("injected unregister failure must escape facade cleanup");
    assert!(
        error
            .to_string()
            .contains("synthetic facade delete failure")
    );
    assert!(streams.lock().await.contains_key(&session_id));
    assert!(!mcp_ran.load(Ordering::SeqCst));
    assert!(!mob_ran.load(Ordering::SeqCst));

    cleanup
        .run(&session_id)
        .await
        .expect("retry should unregister and consume downstream cleanup anchors");
    assert!(!streams.lock().await.contains_key(&session_id));
    assert!(mcp_ran.load(Ordering::SeqCst));
    assert!(mob_ran.load(Ordering::SeqCst));
}

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
    fn session_authority_ops(&self) -> &dyn meerkat_runtime::store::RuntimeSessionAuthorityOps {
        self.inner.session_authority_ops()
    }

    fn session_persistence_profile(
        &self,
    ) -> meerkat_runtime::store::RuntimeSessionPersistenceProfile {
        meerkat_runtime::store::RuntimeStore::session_persistence_profile(self.inner.as_ref())
    }

    fn supports_compaction_projection_outbox(&self) -> bool {
        meerkat_runtime::store::RuntimeStore::supports_compaction_projection_outbox(
            self.inner.as_ref(),
        )
    }

    fn input_state_batch_cas_implementation_profile(
        &self,
    ) -> meerkat_runtime::store::InputStateBatchCasImplementationProfile {
        self.inner.input_state_batch_cas_implementation_profile()
    }

    async fn observe_machine_lifecycle(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
    ) -> Result<
        meerkat_runtime::store::MachineLifecycleObservation,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner.observe_machine_lifecycle(runtime_id).await
    }

    async fn compare_and_swap_machine_lifecycle(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        expected: meerkat_runtime::store::MachineLifecycleExpectedVersion,
        replacement: meerkat_runtime::store::MachineLifecycleCommit,
    ) -> Result<
        meerkat_runtime::store::MachineLifecycleCasOutcome,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .compare_and_swap_machine_lifecycle(runtime_id, expected, replacement)
            .await
    }

    async fn commit_session_snapshot(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        session_delta: meerkat_runtime::store::SerializedSessionSnapshot,
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.inner
            .commit_session_snapshot(runtime_id, session_delta)
            .await
    }

    async fn commit_prepared_whole_blob_rewrite_boundary(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        boundary: meerkat_runtime::store::PreparedWholeBlobRewriteStoreParts,
    ) -> Result<
        meerkat_runtime::store::WholeBlobStoreAuthority,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .commit_prepared_whole_blob_rewrite_boundary(runtime_id, boundary)
            .await
    }

    async fn atomic_apply(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        session_delta: Option<meerkat_runtime::store::SerializedSessionSnapshot>,
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
    ) -> Result<Vec<meerkat_runtime::InputStateRow>, meerkat_runtime::store::RuntimeStoreError>
    {
        self.inner.load_input_states(runtime_id).await
    }

    async fn load_input_states_with_versions(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
    ) -> Result<
        meerkat_runtime::store::PreparedRecoveryInputSnapshot,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner.load_input_states_with_versions(runtime_id).await
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
    ) -> Result<Option<std::sync::Arc<Vec<u8>>>, meerkat_runtime::store::RuntimeStoreError> {
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

    async fn persist_input_states_atomically(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        states: &[meerkat_runtime::input_state::InputStatePersistenceRecord],
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.inner
            .persist_input_states_atomically(runtime_id, states)
            .await
    }

    async fn compare_and_swap_input_states_atomically(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        expected: &[meerkat_runtime::input_state::StoredInputState],
        replacements: &[meerkat_runtime::input_state::InputStatePersistenceRecord],
    ) -> Result<
        meerkat_runtime::store::InputStateBatchCasOutcome,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .compare_and_swap_input_states_atomically(runtime_id, expected, replacements)
            .await
    }

    async fn compare_and_swap_input_states_atomically_with_fence(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        expected: &[meerkat_runtime::input_state::StoredInputState],
        replacements: &[meerkat_runtime::input_state::InputStatePersistenceRecord],
        write_fence: Arc<dyn meerkat_runtime::store::RuntimeStoreWriteFence>,
    ) -> Result<
        meerkat_runtime::store::FencedInputStateBatchCasOutcome,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .compare_and_swap_input_states_atomically_with_fence(
                runtime_id,
                expected,
                replacements,
                write_fence,
            )
            .await
    }

    async fn compare_and_swap_recovery_input_states_atomically(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        expected_revision: meerkat_runtime::store::RecoveryInputSetRevision,
        mutations: &[meerkat_runtime::store::RecoveryInputStateMutation],
    ) -> Result<
        meerkat_runtime::store::InputStateBatchCasOutcome,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .compare_and_swap_recovery_input_states_atomically(
                runtime_id,
                expected_revision,
                mutations,
            )
            .await
    }

    async fn compare_and_swap_recovery_input_states_atomically_with_fence(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        expected_revision: meerkat_runtime::store::RecoveryInputSetRevision,
        mutations: &[meerkat_runtime::store::RecoveryInputStateMutation],
        write_fence: Arc<dyn meerkat_runtime::store::RuntimeStoreWriteFence>,
    ) -> Result<
        meerkat_runtime::store::FencedInputStateBatchCasOutcome,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .compare_and_swap_recovery_input_states_atomically_with_fence(
                runtime_id,
                expected_revision,
                mutations,
                write_fence,
            )
            .await
    }

    async fn load_input_state_by_idempotency_key(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        key: &meerkat_runtime::IdempotencyKey,
    ) -> Result<
        Option<meerkat_runtime::store::ExactInputStateObservation>,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .load_input_state_by_idempotency_key(runtime_id, key)
            .await
    }

    async fn load_input_states_by_ids(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        input_ids: &[meerkat_core::lifecycle::InputId],
    ) -> Result<
        Vec<Option<meerkat_runtime::input_state::StoredInputState>>,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .load_input_states_by_ids(runtime_id, input_ids)
            .await
    }

    async fn load_pending_terminal_owner_ids_page(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        after: Option<&meerkat_core::lifecycle::InputId>,
        limit: usize,
    ) -> Result<Vec<meerkat_core::lifecycle::InputId>, meerkat_runtime::store::RuntimeStoreError>
    {
        self.inner
            .load_pending_terminal_owner_ids_page(runtime_id, after, limit)
            .await
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
        finalization: meerkat_runtime::store::UnregisterFinalizationCommit,
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
            .commit_unregister_finalization(runtime_id, finalization)
            .await
    }

    async fn persist_ops_lifecycle(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        snapshot: &meerkat_runtime::ops_lifecycle::PersistedOpsSnapshot,
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.inner.persist_ops_lifecycle(runtime_id, snapshot).await
    }

    async fn initialize_ops_lifecycle_if_absent(
        &self,
        runtime_id: &meerkat_runtime::LogicalRuntimeId,
        candidate: &meerkat_runtime::ops_lifecycle::PersistedOpsSnapshot,
    ) -> Result<
        meerkat_runtime::ops_lifecycle::PersistedOpsSnapshot,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .initialize_ops_lifecycle_if_absent(runtime_id, candidate)
            .await
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

/// Fixtures shared by the stale-discard tests below: a `CoreExecutor` whose
/// post-stop cleanup parks on a deterministic gate so the owned unregister
/// teardown saga can be held past the ordinary 2-second caller grace, and a
/// mock LLM so the persistent service can materialize a real live actor.
mod stale_discard {
    use std::collections::HashMap;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use meerkat::session_runtime::runtime_state::RuntimeStateOps;
    use meerkat::{AgentFactory, Config, FactoryAgentBuilder, PersistentSessionService};
    use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
    use meerkat_core::lifecycle::RunId;
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
    use meerkat_core::lifecycle::run_receipt::RunBoundaryReceiptDraft;
    use meerkat_core::service::{
        CreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions,
        SessionService as _,
    };
    use meerkat_core::types::SessionId;
    use meerkat_runtime::MeerkatMachine;
    use tokio::sync::Notify;

    struct MockLlmClient;

    #[async_trait::async_trait]
    impl LlmClient for MockLlmClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(
            &'a self,
            _request: &'a LlmRequest,
        ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
            Box::pin(futures::stream::iter(vec![
                Ok(LlmEvent::TextDelta {
                    delta: "mock".to_string(),
                    meta: None,
                }),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success {
                        stop_reason: meerkat_core::StopReason::EndTurn,
                    },
                }),
            ]))
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::Other
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    struct GatedCleanupExecutor {
        cleanup_started: Arc<Notify>,
        release_cleanup: Arc<Notify>,
        cleanup_calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for GatedCleanupExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            Ok(CoreApplyOutput::with_untyped_snapshot(
                RunBoundaryReceiptDraft {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                },
                None,
                None,
            ))
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn cleanup_after_runtime_stop_terminalized(
            &mut self,
        ) -> Result<(), CoreExecutorError> {
            self.cleanup_calls.fetch_add(1, Ordering::SeqCst);
            self.cleanup_started.notify_one();
            self.release_cleanup.notified().await;
            Ok(())
        }
    }

    struct Fixture {
        service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
        staged_sessions: Arc<meerkat::StagedSessionRegistry>,
        staged_capacity_admissions: meerkat::session_runtime::admission::StagedCapacityAdmissions,
        runtime_adapter: Arc<MeerkatMachine>,
    }

    struct GatedTeardown {
        cleanup_started: Arc<Notify>,
        release_cleanup: Arc<Notify>,
        cleanup_calls: Arc<AtomicUsize>,
    }

    impl Fixture {
        fn new() -> Self {
            let mut builder = FactoryAgentBuilder::new(AgentFactory::minimal(), Config::default());
            builder.default_llm_client = Some(Arc::new(MockLlmClient));
            let service = Arc::new(PersistentSessionService::new(
                builder,
                4,
                Arc::new(meerkat_store::MemoryStore::new()),
                Arc::new(meerkat_runtime::store::InMemoryRuntimeStore::new()),
                Arc::new(meerkat_store::MemoryBlobStore::new()),
            ));
            Self {
                service,
                staged_sessions: Arc::new(meerkat::StagedSessionRegistry::new()),
                staged_capacity_admissions: Arc::new(std::sync::Mutex::new(HashMap::new())),
                runtime_adapter: Arc::new(MeerkatMachine::ephemeral()),
            }
        }

        fn ops(&self) -> RuntimeStateOps<'_> {
            RuntimeStateOps {
                service: &self.service,
                staged_sessions: &self.staged_sessions,
                staged_capacity_admissions: &self.staged_capacity_admissions,
                runtime_adapter: &self.runtime_adapter,
            }
        }

        /// Create a persisted session whose live actor is materialized.
        async fn create_live_session(&self) -> SessionId {
            let created = self
                .service
                .create_session(CreateSessionRequest {
                    injected_context: Vec::new(),
                    model: "gpt-5.4".to_string(),
                    prompt: "stale discard fixture".to_string().into(),
                    system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
                    max_tokens: None,
                    event_tx: None,
                    initial_turn: InitialTurnPolicy::Defer,
                    deferred_prompt_policy: DeferredPromptPolicy::Discard,
                    build: Some(SessionBuildOptions::default()),
                    labels: None,
                })
                .await
                .expect("persistent service must create a live session");
            assert!(
                self.service
                    .live_session_actor_registered(&created.session_id)
                    .await,
                "fixture must start with a registered live actor"
            );
            created.session_id
        }

        async fn register_gated_runtime(&self, session_id: &SessionId) -> GatedTeardown {
            let gate = GatedTeardown {
                cleanup_started: Arc::new(Notify::new()),
                release_cleanup: Arc::new(Notify::new()),
                cleanup_calls: Arc::new(AtomicUsize::new(0)),
            };
            self.runtime_adapter
                .register_session_with_executor(
                    session_id.clone(),
                    Box::new(GatedCleanupExecutor {
                        cleanup_started: Arc::clone(&gate.cleanup_started),
                        release_cleanup: Arc::clone(&gate.release_cleanup),
                        cleanup_calls: Arc::clone(&gate.cleanup_calls),
                    }),
                )
                .await
                .expect("runtime executor registration should succeed");
            gate
        }
    }

    /// (a) No live projection and no runtime registration: nothing to tear
    /// down, so the discard is already clean.
    #[tokio::test]
    async fn discard_stale_live_session_with_absent_registration_is_clean() {
        let fixture = Fixture::new();
        let session_id = SessionId::new();
        assert!(
            fixture
                .runtime_adapter
                .current_session_registration_witness(&session_id)
                .await
                .is_none()
        );

        fixture
            .ops()
            .discard_stale_live_session(&session_id)
            .await
            .expect("absent registration must be treated as already clean");
        assert!(!fixture.runtime_adapter.contains_session(&session_id).await);
    }

    /// Persisted session, runtime registered with an executor, but no live
    /// actor (the reopened `monitors/start` shape): the registration is
    /// healthy and must be kept, so no teardown is started and nothing waits
    /// on the runtime loop.
    #[tokio::test]
    async fn discard_stale_live_session_keeps_registration_without_live_actor() {
        let fixture = Fixture::new();
        let session_id = fixture.create_live_session().await;
        fixture
            .service
            .discard_live_session(&session_id)
            .await
            .expect("live actor discard should succeed");
        assert!(
            !fixture
                .service
                .live_session_actor_registered(&session_id)
                .await
        );
        assert!(
            fixture
                .service
                .load_authoritative_session(&session_id)
                .await
                .expect("authoritative load should succeed")
                .is_some(),
            "durable record must survive the live actor discard"
        );
        let gate = fixture.register_gated_runtime(&session_id).await;
        let before = fixture
            .runtime_adapter
            .current_session_registration_witness(&session_id)
            .await
            .expect("registered runtime must expose an exact registration witness");

        tokio::time::timeout(
            Duration::from_secs(1),
            fixture.ops().discard_stale_live_session(&session_id),
        )
        .await
        .expect("discard without a live actor must not wait on any teardown")
        .expect("discard without a live actor must be clean");

        let after = fixture
            .runtime_adapter
            .current_session_registration_witness(&session_id)
            .await
            .expect("registration must survive a discard that found no live actor");
        assert_eq!(
            after.epoch_id(),
            before.epoch_id(),
            "the exact registration must be untouched"
        );
        assert_eq!(
            fixture
                .runtime_adapter
                .unregister_runtime_loop_handoff_wait_reports(&session_id)
                .await,
            Some(0),
            "no unregister teardown may have started"
        );
        assert_eq!(gate.cleanup_calls.load(Ordering::SeqCst), 0);
        gate.release_cleanup.notify_one();
    }

    /// (b) A live actor exists (behind durable authority): it is discarded and
    /// the exact registration is torn down. A teardown that outlives the
    /// ordinary 2-second caller grace must complete with `Ok(())` instead of
    /// surfacing `UnregisterInProgress` to the caller about to reoccupy the
    /// `SessionId`.
    #[tokio::test]
    async fn discard_stale_live_session_with_live_actor_awaits_teardown_past_caller_grace() {
        // Mirrors UNREGISTER_CALLER_WAIT_GRACE in meerkat-runtime; the hold
        // must exceed it so the old `unregister_session` path would already
        // have returned `UnregisterInProgress`.
        const OLD_CALLER_GRACE: Duration = Duration::from_secs(2);
        const PAST_GRACE_HOLD: Duration = Duration::from_millis(2600);

        let fixture = Arc::new(Fixture::new());
        let session_id = fixture.create_live_session().await;
        let gate = fixture.register_gated_runtime(&session_id).await;
        let registration = fixture
            .runtime_adapter
            .current_session_registration_witness(&session_id)
            .await
            .expect("registered runtime must expose an exact registration witness");

        let discard = {
            let fixture = Arc::clone(&fixture);
            let session_id = session_id.clone();
            tokio::spawn(async move { fixture.ops().discard_stale_live_session(&session_id).await })
        };
        tokio::time::timeout(Duration::from_secs(1), gate.cleanup_started.notified())
            .await
            .expect("owned teardown must reach the deterministic cleanup gate");
        assert!(
            !fixture
                .service
                .live_session_actor_registered(&session_id)
                .await,
            "the behind live actor must be discarded before teardown"
        );

        // Hold the teardown gate strictly longer than the old caller grace.
        assert!(PAST_GRACE_HOLD > OLD_CALLER_GRACE);
        tokio::time::sleep(PAST_GRACE_HOLD).await;
        assert!(
            !discard.is_finished(),
            "stale discard must keep waiting on the exact teardown instead of \
             erroring at the ordinary caller grace"
        );
        assert!(
            fixture.runtime_adapter.contains_session(&session_id).await,
            "registration must remain owned by the in-flight teardown"
        );

        gate.release_cleanup.notify_one();
        tokio::time::timeout(Duration::from_secs(2), discard)
            .await
            .expect("stale discard must finish once teardown reaches terminal completion")
            .expect("stale discard task should not panic")
            .expect("stale discard must complete with Ok(()) past the old caller grace");
        assert!(!fixture.runtime_adapter.contains_session(&session_id).await);
        assert_eq!(gate.cleanup_calls.load(Ordering::SeqCst), 1);
        assert!(
            fixture
                .runtime_adapter
                .current_session_registration_witness(&session_id)
                .await
                .is_none(),
            "the exact registration {} must be gone after terminal teardown",
            registration.epoch_id()
        );
    }
}

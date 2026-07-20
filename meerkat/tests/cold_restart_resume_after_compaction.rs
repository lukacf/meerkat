//! H2 repro attempt: cold-restart resume AFTER a compaction rewrite.
//!
//! Compaction rewrites the transcript (summary injection + optional shrink)
//! and records a `TranscriptRewriteCommit`. Commit c5831a0f9 fixed the archive
//! path when the durable row lags across a compaction boundary; this test
//! probes the sibling cold-restart resume path: compaction happens, the host
//! dies cold, a fresh service rebuilds the runtime authority over the same
//! sqlite stores, resumes via `materialize_session`, and runs another turn.
//! Any `TranscriptContinuityViolation` / `MonotonicityViolation` /
//! `InvalidTranscriptRewrite` in the resume or post-resume persist is the bug.

#![allow(clippy::expect_used, clippy::unwrap_used)]

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use meerkat::surface::{
        build_runtime_backed_service, default_persistent_executor, materialize_session,
    };
    use meerkat::{
        AgentFactory, Config, CreateSessionRequest, FactoryAgentBuilder, PersistenceBundle,
        PersistentSessionService, Session,
    };
    use meerkat_client::types::LlmStream;
    use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
    use meerkat_core::SessionBuildOptions;
    use meerkat_core::session_store::{SessionFilter, SessionStore, SessionStoreError};
    #[cfg(all(feature = "memory-store-session", feature = "session-compaction"))]
    use meerkat_core::{
        MemoryEnumerationRequest, MemoryIndexBatch, MemoryIndexRequest, MemoryIndexScope,
        MemoryMetadata, MemorySearchScope, MemorySource, MemoryStore, MessageRange, SessionService,
    };
    use meerkat_runtime::RuntimeStore;
    use meerkat_runtime::completion::CompletionOutcome;
    use meerkat_runtime::{Input, MeerkatMachine, PromptInput};
    #[cfg(all(feature = "memory-store-session", feature = "session-compaction"))]
    use meerkat_session::event_store::EventStore;
    use tokio::time::Duration;

    /// Deterministic client whose compaction response actually carries the
    /// history it replaces. Normal turns still return the small `ok` response
    /// used by these persistence tests; summary calls echo prior user text so
    /// assertions can distinguish preserved semantic history from a raw
    /// retained transcript message.
    struct HistorySummarizingTestClient;

    #[async_trait::async_trait]
    impl LlmClient for HistorySummarizingTestClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
            let last_user = request
                .messages
                .iter()
                .rev()
                .find_map(|message| match message {
                    meerkat_core::Message::User(user) => Some(user.text_content()),
                    _ => None,
                });
            let text = if last_user
                .as_deref()
                .is_some_and(|text| text.contains("CONTEXT COMPACTION"))
            {
                let summary = request
                    .messages
                    .iter()
                    .filter_map(|message| match message {
                        meerkat_core::Message::User(user)
                            if !user.text_content().contains("CONTEXT COMPACTION") =>
                        {
                            Some(user.text_content())
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if summary.is_empty() {
                    "empty history".to_string()
                } else {
                    summary
                }
            } else {
                "ok".to_string()
            };
            let events = vec![
                LlmEvent::TextDelta {
                    delta: text,
                    meta: None,
                },
                LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success {
                        stop_reason: meerkat_core::StopReason::EndTurn,
                    },
                },
            ];
            Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::Other
        }

        async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
            Ok(())
        }
    }

    struct CompactionFailureTrackingClient {
        fail_compaction: bool,
        compaction_attempts: Arc<AtomicUsize>,
    }

    impl CompactionFailureTrackingClient {
        fn new(fail_compaction: bool, compaction_attempts: Arc<AtomicUsize>) -> Self {
            Self {
                fail_compaction,
                compaction_attempts,
            }
        }
    }

    #[async_trait::async_trait]
    impl LlmClient for CompactionFailureTrackingClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
            let is_compaction = request.messages.iter().rev().any(|message| {
                matches!(
                    message,
                    meerkat_core::Message::User(user)
                        if user.text_content().contains("CONTEXT COMPACTION")
                )
            });
            if is_compaction {
                self.compaction_attempts.fetch_add(1, Ordering::SeqCst);
                if self.fail_compaction {
                    return Box::pin(futures::stream::iter([Err(LlmError::IncompleteResponse {
                        message: "cold-restart compaction failure".to_string(),
                    })]));
                }
            }

            let text = if is_compaction { "summary" } else { "ok" };
            Box::pin(futures::stream::iter(
                [
                    LlmEvent::TextDelta {
                        delta: text.to_string(),
                        meta: None,
                    },
                    LlmEvent::Done {
                        outcome: LlmDoneOutcome::Success {
                            stop_reason: meerkat_core::StopReason::EndTurn,
                        },
                    },
                ]
                .into_iter()
                .map(Ok),
            ))
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::Other
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    /// Completes the first ordinary turn, summarizes the compaction request,
    /// then asks for a host callback on the ordinary request immediately
    /// following that rewrite.
    #[cfg(all(feature = "memory-store-session", feature = "session-compaction"))]
    struct CallbackPendingAfterCompactionClient {
        ordinary_calls: AtomicUsize,
    }

    #[cfg(all(feature = "memory-store-session", feature = "session-compaction"))]
    impl CallbackPendingAfterCompactionClient {
        fn new() -> Self {
            Self {
                ordinary_calls: AtomicUsize::new(0),
            }
        }
    }

    #[cfg(all(feature = "memory-store-session", feature = "session-compaction"))]
    #[async_trait::async_trait]
    impl LlmClient for CallbackPendingAfterCompactionClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
            let is_compaction = request.messages.iter().rev().any(|message| {
                matches!(
                    message,
                    meerkat_core::Message::User(user)
                        if user.text_content().contains("CONTEXT COMPACTION")
                )
            });
            let events = if is_compaction {
                vec![
                    LlmEvent::TextDelta {
                        delta: "callback compaction summary".to_string(),
                        meta: None,
                    },
                    LlmEvent::Done {
                        outcome: LlmDoneOutcome::Success {
                            stop_reason: meerkat_core::StopReason::EndTurn,
                        },
                    },
                ]
            } else if self.ordinary_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                vec![
                    LlmEvent::TextDelta {
                        delta: "ok".to_string(),
                        meta: None,
                    },
                    LlmEvent::Done {
                        outcome: LlmDoneOutcome::Success {
                            stop_reason: meerkat_core::StopReason::EndTurn,
                        },
                    },
                ]
            } else {
                vec![
                    LlmEvent::ToolCallComplete {
                        id: "toolu_compaction_callback".to_string(),
                        name: "external_callback".to_string(),
                        args: serde_json::json!({ "key": "after-compaction" }),
                        meta: None,
                    },
                    LlmEvent::Done {
                        outcome: LlmDoneOutcome::Success {
                            stop_reason: meerkat_core::StopReason::ToolUse,
                        },
                    },
                ]
            };
            Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::Other
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    #[cfg(all(feature = "memory-store-session", feature = "session-compaction"))]
    struct CallbackPendingDispatcher;

    #[cfg(all(feature = "memory-store-session", feature = "session-compaction"))]
    #[async_trait::async_trait]
    impl meerkat_core::AgentToolDispatcher for CallbackPendingDispatcher {
        fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> {
            Arc::from([Arc::new(meerkat_core::ToolDef::new(
                "external_callback",
                "external callback test tool",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "key": { "type": "string" }
                    }
                }),
            ))])
        }

        async fn dispatch(
            &self,
            call: meerkat_core::ToolCallView<'_>,
        ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
            let args = serde_json::from_str(call.args.get()).unwrap_or_else(|_| {
                serde_json::json!({
                    "raw": call.args.get()
                })
            });
            Err(meerkat_core::ToolError::callback_pending(call.name, args))
        }
    }

    async fn build_service(
        root: &std::path::Path,
    ) -> (
        Arc<PersistentSessionService<FactoryAgentBuilder>>,
        Arc<MeerkatMachine>,
    ) {
        build_service_with_client(root, Arc::new(HistorySummarizingTestClient)).await
    }

    async fn build_service_with_client(
        root: &std::path::Path,
        client: Arc<dyn LlmClient>,
    ) -> (
        Arc<PersistentSessionService<FactoryAgentBuilder>>,
        Arc<MeerkatMachine>,
    ) {
        let (_manifest, persistence) = meerkat::open_realm_persistence_in(
            root,
            "restart-realm",
            Some(meerkat_store::RealmBackend::Sqlite),
            Some(meerkat_store::RealmOrigin::Explicit),
        )
        .await
        .expect("open realm persistence");
        let factory = AgentFactory::new(root.join("sessions"));
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        builder.default_llm_client = Some(client);
        let (service, adapter) = build_runtime_backed_service(builder, 4, persistence);
        (Arc::new(service), adapter)
    }

    #[cfg(all(feature = "memory-store-session", feature = "session-compaction"))]
    fn build_callback_memory_service(
        root: &std::path::Path,
        client: Arc<dyn LlmClient>,
    ) -> (
        Arc<PersistentSessionService<FactoryAgentBuilder>>,
        Arc<MeerkatMachine>,
        Arc<meerkat_runtime::store::SqliteRuntimeStore>,
    ) {
        let sqlite_path = root.join("sessions.sqlite3");
        let session_store: Arc<dyn SessionStore> = Arc::new(
            meerkat::SqliteSessionStore::open(sqlite_path.clone()).expect("open session sqlite"),
        );
        let runtime_store = Arc::new(
            meerkat_runtime::store::SqliteRuntimeStore::new(sqlite_path)
                .expect("open runtime sqlite"),
        );
        let runtime_store_for_bundle: Arc<dyn RuntimeStore> = runtime_store.clone();
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::FsBlobStore::new(root.join("blobs")));
        let bundle = PersistenceBundle::new(session_store, runtime_store_for_bundle, blob_store);

        let factory = AgentFactory::new(root.join("sessions")).memory(true);
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        builder.default_llm_client = Some(client);
        let (service, adapter) = build_runtime_backed_service(builder, 4, bundle);
        (Arc::new(service), adapter, runtime_store)
    }

    #[cfg(all(feature = "memory-store-session", feature = "session-compaction"))]
    fn staged_compaction_projection(
        session_id: &meerkat::SessionId,
    ) -> meerkat_core::CompactionProjectionId {
        serde_json::from_value(serde_json::json!({
            "session_id": session_id,
            "parent_revision": "pre-runtime-commit-parent",
            "revision": "pre-runtime-commit-revision",
            "commit_fingerprint": "sha256:pre-runtime-commit-crash-window",
        }))
        .expect("valid staged compaction projection fixture")
    }

    #[cfg(all(feature = "memory-store-session", feature = "session-compaction"))]
    fn staged_compaction_batch(session_id: &meerkat::SessionId, content: &str) -> MemoryIndexBatch {
        MemoryIndexBatch::single(
            MemoryIndexRequest::new(
                MemoryIndexScope::for_session(session_id.clone()),
                meerkat_core::MemoryIndexableContent::Indexable(content.to_string()),
                MemoryMetadata {
                    session_id: session_id.clone(),
                    source: MemorySource::Compaction {
                        source_range: MessageRange::single(0),
                    },
                    indexed_at: meerkat_core::time_compat::SystemTime::now(),
                },
            )
            .expect("valid staged compaction memory request"),
        )
    }

    /// A process may die after HNSW durably stages discarded history but
    /// before RuntimeStore::atomic_apply commits the transcript + outbox. On
    /// restart the authoritative outbox is empty, and runtime attachment may
    /// ask for reconciliation before any SessionTask exists. That empty
    /// authority must still reach the builder-owned canonical HNSW backend and
    /// abort the orphan instead of declaring a no-op.
    #[cfg(all(feature = "memory-store-session", feature = "session-compaction"))]
    #[tokio::test]
    async fn empty_outbox_aborts_stale_hnsw_stage_before_session_task_materialization() {
        let temp = tempfile::tempdir().expect("tempdir");
        let session_id = meerkat::SessionId::new();
        let projection = staged_compaction_projection(&session_id);
        let memory_dir = temp.path().join("sessions").join("memory");

        // Process one: persist the invisible stage, then die before the paired
        // runtime snapshot/outbox boundary.
        {
            let memory_store = meerkat_memory::HnswMemoryStore::open(&memory_dir)
                .expect("open canonical HNSW store for crash fixture");
            memory_store
                .stage_compaction_batch(
                    projection.clone(),
                    staged_compaction_batch(&session_id, "orphaned pre-commit memory"),
                )
                .await
                .expect("persist orphaned compaction stage");
        }

        // Process two: construct fresh service/runtime owners without
        // materializing the session task.
        let client: Arc<dyn LlmClient> = Arc::new(CallbackPendingAfterCompactionClient::new());
        let (service, _adapter, runtime_store) = build_callback_memory_service(temp.path(), client);
        assert!(
            !service
                .has_live_session(&session_id)
                .await
                .expect("query live session materialization"),
            "regression must reconcile before SessionTask materialization"
        );
        let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(&session_id);
        assert!(
            runtime_store
                .load_pending_compaction_projections(&runtime_id)
                .await
                .expect("load empty runtime outbox")
                .is_empty(),
            "crash-before-atomic-apply must leave no committed runtime outbox"
        );

        service
            .reconcile_runtime_compaction_projections(&session_id, Vec::new())
            .await
            .expect("builder-owned empty authority must abort the absent-session stage");
        assert!(
            !service
                .has_live_session(&session_id)
                .await
                .expect("recheck live session materialization"),
            "reconciliation must not fabricate a SessionTask"
        );

        // Reusing the exact projection id with a different payload is rejected
        // while the original stage exists. Success therefore proves the stale
        // durable row was deleted by the public reconciliation path.
        let memory_store = meerkat_memory::HnswMemoryStore::open(&memory_dir)
            .expect("reopen canonical HNSW after reconciliation");
        memory_store
            .stage_compaction_batch(
                projection.clone(),
                staged_compaction_batch(&session_id, "replacement proof payload"),
            )
            .await
            .expect("stale stage must be gone before replacement proof stage");
        memory_store
            .abort_compaction_batch(&projection)
            .await
            .expect("clean replacement proof stage");
    }

    /// Durable audit-log wrapper that fails exactly the next canonical
    /// TranscriptRewriteCommitted append. Ordinary run events continue to
    /// persist, so the injected fault isolates the post-runtime-commit audit
    /// projection window exercised by the recovery test below.
    #[cfg(all(feature = "memory-store-session", feature = "session-compaction"))]
    struct FailNextRewriteAuditEventStore {
        inner: meerkat_session::event_store::FileEventStore,
        fail_next_rewrite: AtomicBool,
        failed_rewrite_appends: AtomicUsize,
    }

    #[cfg(all(feature = "memory-store-session", feature = "session-compaction"))]
    impl FailNextRewriteAuditEventStore {
        fn new(root: impl Into<std::path::PathBuf>) -> Self {
            Self {
                inner: meerkat_session::event_store::FileEventStore::new(root),
                fail_next_rewrite: AtomicBool::new(false),
                failed_rewrite_appends: AtomicUsize::new(0),
            }
        }

        fn arm(&self) {
            self.fail_next_rewrite.store(true, Ordering::Release);
        }

        fn failed_rewrite_appends(&self) -> usize {
            self.failed_rewrite_appends.load(Ordering::Acquire)
        }
    }

    #[cfg(all(feature = "memory-store-session", feature = "session-compaction"))]
    #[async_trait::async_trait]
    impl meerkat_session::event_store::EventStore for FailNextRewriteAuditEventStore {
        async fn append_envelopes(
            &self,
            session_id: &meerkat::SessionId,
            envelopes: &[meerkat_core::event::EventEnvelope<meerkat_core::AgentEvent>],
        ) -> Result<u64, meerkat_session::event_store::EventStoreError> {
            let is_rewrite_audit = envelopes.iter().any(|envelope| {
                matches!(
                    envelope.payload,
                    meerkat_core::AgentEvent::TranscriptRewriteCommitted { .. }
                )
            });
            if is_rewrite_audit && self.fail_next_rewrite.swap(false, Ordering::AcqRel) {
                self.failed_rewrite_appends.fetch_add(1, Ordering::AcqRel);
                return Err(meerkat_session::event_store::EventStoreError::Io(
                    std::io::Error::other(
                        "injected TranscriptRewriteCommitted audit append failure",
                    ),
                ));
            }
            self.inner.append_envelopes(session_id, envelopes).await
        }

        async fn record_projection_halt(
            &self,
            session_id: &meerkat::SessionId,
            reason: &str,
        ) -> Result<(), meerkat_session::event_store::EventStoreError> {
            self.inner.record_projection_halt(session_id, reason).await
        }

        async fn projection_halt(
            &self,
            session_id: &meerkat::SessionId,
        ) -> Result<
            Option<meerkat_session::event_store::EventProjectionHaltMarker>,
            meerkat_session::event_store::EventStoreError,
        > {
            self.inner.projection_halt(session_id).await
        }

        async fn read_from(
            &self,
            session_id: &meerkat::SessionId,
            from_seq: u64,
        ) -> Result<
            Vec<meerkat_session::event_store::StoredEvent>,
            meerkat_session::event_store::EventStoreError,
        > {
            self.inner.read_from(session_id, from_seq).await
        }

        async fn read_page(
            &self,
            session_id: &meerkat::SessionId,
            from_seq: u64,
            limit: usize,
        ) -> Result<
            Vec<meerkat_session::event_store::StoredEvent>,
            meerkat_session::event_store::EventStoreError,
        > {
            self.inner.read_page(session_id, from_seq, limit).await
        }

        async fn last_seq(
            &self,
            session_id: &meerkat::SessionId,
        ) -> Result<u64, meerkat_session::event_store::EventStoreError> {
            self.inner.last_seq(session_id).await
        }
    }

    #[cfg(all(feature = "memory-store-session", feature = "session-compaction"))]
    fn build_rewrite_audit_failure_memory_service(
        root: &std::path::Path,
        client: Arc<dyn LlmClient>,
    ) -> (
        Arc<PersistentSessionService<FactoryAgentBuilder>>,
        Arc<MeerkatMachine>,
        Arc<meerkat_runtime::store::SqliteRuntimeStore>,
        Arc<FailNextRewriteAuditEventStore>,
    ) {
        let sqlite_path = root.join("sessions.sqlite3");
        let session_store: Arc<dyn SessionStore> = Arc::new(
            meerkat::SqliteSessionStore::open(sqlite_path.clone()).expect("open session sqlite"),
        );
        let runtime_store = Arc::new(
            meerkat_runtime::store::SqliteRuntimeStore::new(sqlite_path)
                .expect("open runtime sqlite"),
        );
        let runtime_store_for_bundle: Arc<dyn RuntimeStore> = runtime_store.clone();
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::FsBlobStore::new(root.join("blobs")));
        let bundle = PersistenceBundle::new(session_store, runtime_store_for_bundle, blob_store);

        let factory = AgentFactory::new(root.join("sessions")).memory(true);
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        builder.default_llm_client = Some(client);
        let (service, adapter) = build_runtime_backed_service(builder, 4, bundle);
        let event_store = Arc::new(FailNextRewriteAuditEventStore::new(
            root.join(".rkat").join("events"),
        ));
        let service = service.with_event_projection(
            event_store.clone(),
            Arc::new(meerkat_session::projector::SessionProjector::new(
                root.join(".rkat"),
            )),
        );
        (Arc::new(service), adapter, runtime_store, event_store)
    }

    /// RuntimeStore fault wrapper that rejects exactly one `atomic_apply`
    /// before delegating any writes. The underlying sqlite store therefore
    /// provides an authoritative empty outbox observation for the runtime
    /// loop's post-error compaction settlement path.
    #[cfg(all(feature = "memory-store-session", feature = "session-compaction"))]
    struct FailNextAtomicApplyStore {
        inner: Arc<meerkat_runtime::store::SqliteRuntimeStore>,
        fail_next: AtomicBool,
    }

    #[cfg(all(feature = "memory-store-session", feature = "session-compaction"))]
    impl FailNextAtomicApplyStore {
        fn new(inner: Arc<meerkat_runtime::store::SqliteRuntimeStore>) -> Self {
            Self {
                inner,
                fail_next: AtomicBool::new(false),
            }
        }

        fn arm(&self) {
            self.fail_next.store(true, Ordering::Release);
        }
    }

    #[cfg(all(feature = "memory-store-session", feature = "session-compaction"))]
    #[async_trait::async_trait]
    impl RuntimeStore for FailNextAtomicApplyStore {
        fn supports_compaction_projection_outbox(&self) -> bool {
            self.inner.supports_compaction_projection_outbox()
        }

        async fn observe_machine_lifecycle(
            &self,
            runtime_id: &meerkat_runtime::LogicalRuntimeId,
        ) -> Result<
            meerkat_runtime::store::MachineLifecycleObservation,
            meerkat_runtime::RuntimeStoreError,
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
            meerkat_runtime::RuntimeStoreError,
        > {
            self.inner
                .compare_and_swap_machine_lifecycle(runtime_id, expected, replacement)
                .await
        }

        fn auth_authority_key(&self) -> Option<String> {
            self.inner.auth_authority_key()
        }

        async fn commit_session_snapshot(
            &self,
            runtime_id: &meerkat_runtime::LogicalRuntimeId,
            session_delta: meerkat_runtime::SessionDelta,
        ) -> Result<(), meerkat_runtime::RuntimeStoreError> {
            self.inner
                .commit_session_snapshot(runtime_id, session_delta)
                .await
        }

        async fn commit_session_transcript_rewrite_snapshot(
            &self,
            runtime_id: &meerkat_runtime::LogicalRuntimeId,
            session_delta: meerkat_runtime::SessionDelta,
            commit: &meerkat_core::TranscriptRewriteCommit,
        ) -> Result<(), meerkat_runtime::RuntimeStoreError> {
            self.inner
                .commit_session_transcript_rewrite_snapshot(runtime_id, session_delta, commit)
                .await
        }

        async fn atomic_apply(
            &self,
            runtime_id: &meerkat_runtime::LogicalRuntimeId,
            session_delta: Option<meerkat_runtime::SessionDelta>,
            receipt: meerkat_core::lifecycle::RunBoundaryReceipt,
            input_updates: Vec<meerkat_runtime::input_state::InputStatePersistenceRecord>,
            session_store_key: Option<meerkat::SessionId>,
        ) -> Result<(), meerkat_runtime::RuntimeStoreError> {
            if self.fail_next.swap(false, Ordering::AcqRel) {
                return Err(meerkat_runtime::RuntimeStoreError::WriteFailed(
                    "injected pre-commit atomic_apply failure".to_string(),
                ));
            }
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

        async fn load_pending_compaction_projections(
            &self,
            runtime_id: &meerkat_runtime::LogicalRuntimeId,
        ) -> Result<Vec<meerkat_core::CompactionProjectionIntent>, meerkat_runtime::RuntimeStoreError>
        {
            self.inner
                .load_pending_compaction_projections(runtime_id)
                .await
        }

        async fn mark_compaction_projection_finalized(
            &self,
            runtime_id: &meerkat_runtime::LogicalRuntimeId,
            projection: &meerkat_core::CompactionProjectionId,
        ) -> Result<(), meerkat_runtime::RuntimeStoreError> {
            self.inner
                .mark_compaction_projection_finalized(runtime_id, projection)
                .await
        }

        async fn load_input_states(
            &self,
            runtime_id: &meerkat_runtime::LogicalRuntimeId,
        ) -> Result<
            Vec<meerkat_runtime::input_state::StoredInputState>,
            meerkat_runtime::RuntimeStoreError,
        > {
            self.inner.load_input_states(runtime_id).await
        }

        async fn load_boundary_receipt(
            &self,
            runtime_id: &meerkat_runtime::LogicalRuntimeId,
            run_id: &meerkat_core::RunId,
            sequence: u64,
        ) -> Result<
            Option<meerkat_core::lifecycle::RunBoundaryReceipt>,
            meerkat_runtime::RuntimeStoreError,
        > {
            self.inner
                .load_boundary_receipt(runtime_id, run_id, sequence)
                .await
        }

        async fn load_session_snapshot(
            &self,
            runtime_id: &meerkat_runtime::LogicalRuntimeId,
        ) -> Result<Option<Vec<u8>>, meerkat_runtime::RuntimeStoreError> {
            self.inner.load_session_snapshot(runtime_id).await
        }

        async fn clear_session_snapshot(
            &self,
            runtime_id: &meerkat_runtime::LogicalRuntimeId,
        ) -> Result<(), meerkat_runtime::RuntimeStoreError> {
            self.inner.clear_session_snapshot(runtime_id).await
        }

        async fn replace_session_snapshot_if_current(
            &self,
            runtime_id: &meerkat_runtime::LogicalRuntimeId,
            expected_current: &[u8],
            replacement: Vec<u8>,
        ) -> Result<bool, meerkat_runtime::RuntimeStoreError> {
            self.inner
                .replace_session_snapshot_if_current(runtime_id, expected_current, replacement)
                .await
        }

        async fn clear_session_snapshot_if_current(
            &self,
            runtime_id: &meerkat_runtime::LogicalRuntimeId,
            expected_current: &[u8],
        ) -> Result<bool, meerkat_runtime::RuntimeStoreError> {
            self.inner
                .clear_session_snapshot_if_current(runtime_id, expected_current)
                .await
        }

        async fn is_runtime_projection_quarantined(
            &self,
            runtime_id: &meerkat_runtime::LogicalRuntimeId,
        ) -> Result<bool, meerkat_runtime::RuntimeStoreError> {
            self.inner
                .is_runtime_projection_quarantined(runtime_id)
                .await
        }

        async fn persist_input_state(
            &self,
            runtime_id: &meerkat_runtime::LogicalRuntimeId,
            state: &meerkat_runtime::input_state::InputStatePersistenceRecord,
        ) -> Result<(), meerkat_runtime::RuntimeStoreError> {
            self.inner.persist_input_state(runtime_id, state).await
        }

        async fn load_input_state(
            &self,
            runtime_id: &meerkat_runtime::LogicalRuntimeId,
            input_id: &meerkat_core::InputId,
        ) -> Result<
            Option<meerkat_runtime::input_state::StoredInputState>,
            meerkat_runtime::RuntimeStoreError,
        > {
            self.inner.load_input_state(runtime_id, input_id).await
        }

        async fn load_machine_lifecycle_record(
            &self,
            runtime_id: &meerkat_runtime::LogicalRuntimeId,
        ) -> Result<Option<Vec<u8>>, meerkat_runtime::RuntimeStoreError> {
            self.inner.load_machine_lifecycle_record(runtime_id).await
        }

        async fn commit_machine_lifecycle(
            &self,
            runtime_id: &meerkat_runtime::LogicalRuntimeId,
            commit: meerkat_runtime::store::MachineLifecycleCommit,
            input_states: &[meerkat_runtime::input_state::InputStatePersistenceRecord],
        ) -> Result<(), meerkat_runtime::RuntimeStoreError> {
            self.inner
                .commit_machine_lifecycle(runtime_id, commit, input_states)
                .await
        }

        async fn commit_unregister_finalization(
            &self,
            runtime_id: &meerkat_runtime::LogicalRuntimeId,
            finalization: meerkat_runtime::store::UnregisterFinalizationCommit,
        ) -> Result<(), meerkat_runtime::RuntimeStoreError> {
            self.inner
                .commit_unregister_finalization(runtime_id, finalization)
                .await
        }

        async fn persist_ops_lifecycle(
            &self,
            runtime_id: &meerkat_runtime::LogicalRuntimeId,
            snapshot: &meerkat_runtime::ops_lifecycle::PersistedOpsSnapshot,
        ) -> Result<(), meerkat_runtime::RuntimeStoreError> {
            self.inner.persist_ops_lifecycle(runtime_id, snapshot).await
        }

        async fn initialize_ops_lifecycle_if_absent(
            &self,
            runtime_id: &meerkat_runtime::LogicalRuntimeId,
            candidate: &meerkat_runtime::ops_lifecycle::PersistedOpsSnapshot,
        ) -> Result<
            meerkat_runtime::ops_lifecycle::PersistedOpsSnapshot,
            meerkat_runtime::RuntimeStoreError,
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
            meerkat_runtime::RuntimeStoreError,
        > {
            self.inner.load_ops_lifecycle(runtime_id).await
        }

        async fn delete_ops_lifecycle(
            &self,
            runtime_id: &meerkat_runtime::LogicalRuntimeId,
        ) -> Result<(), meerkat_runtime::RuntimeStoreError> {
            self.inner.delete_ops_lifecycle(runtime_id).await
        }
    }

    #[cfg(all(feature = "memory-store-session", feature = "session-compaction"))]
    fn build_atomic_apply_failure_memory_service(
        root: &std::path::Path,
        client: Arc<dyn LlmClient>,
    ) -> (
        Arc<PersistentSessionService<FactoryAgentBuilder>>,
        Arc<MeerkatMachine>,
        Arc<FailNextAtomicApplyStore>,
        Arc<meerkat_runtime::store::SqliteRuntimeStore>,
    ) {
        let sqlite_path = root.join("sessions.sqlite3");
        let session_store: Arc<dyn SessionStore> = Arc::new(
            meerkat::SqliteSessionStore::open(sqlite_path.clone()).expect("open session sqlite"),
        );
        let raw_runtime_store = Arc::new(
            meerkat_runtime::store::SqliteRuntimeStore::new(sqlite_path)
                .expect("open runtime sqlite"),
        );
        let fault_store = Arc::new(FailNextAtomicApplyStore::new(Arc::clone(
            &raw_runtime_store,
        )));
        let runtime_store: Arc<dyn RuntimeStore> = fault_store.clone();
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::FsBlobStore::new(root.join("blobs")));
        let bundle = PersistenceBundle::new(session_store, runtime_store, blob_store);

        let factory = AgentFactory::new(root.join("sessions")).memory(true);
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        builder.default_llm_client = Some(client);
        let (service, adapter) = build_runtime_backed_service(builder, 4, bundle);
        (Arc::new(service), adapter, fault_store, raw_runtime_store)
    }

    /// Same request shape as the passing baseline, plus an aggressive
    /// auto-compaction threshold so the estimated-history-tokens trigger fires
    /// on the second pre-LLM boundary (first boundary never compacts).
    fn create_request() -> CreateSessionRequest {
        let build = SessionBuildOptions {
            auto_compact_threshold_override: std::num::NonZeroU64::new(1),
            ..SessionBuildOptions::default()
        };
        CreateSessionRequest {
            injected_context: Vec::new(),
            model: "gpt-5.4".to_string(),
            prompt: meerkat_core::ContentInput::Text(String::new()),
            system_prompt: meerkat::SystemPromptOverride::Set(
                "cold restart resume after compaction contract".to_string(),
            ),
            max_tokens: None,
            event_tx: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(build),
            labels: None,
        }
    }

    #[cfg(all(feature = "memory-store-session", feature = "session-compaction"))]
    fn callback_memory_request() -> CreateSessionRequest {
        let mut request = create_request();
        let build = request
            .build
            .as_mut()
            .expect("compaction request must carry build options");
        build.external_tools = Some(Arc::new(CallbackPendingDispatcher));
        build.override_builtins = meerkat_core::ToolCategoryOverride::Disable;
        build.override_memory = meerkat_core::ToolCategoryOverride::Enable;
        request
    }

    async fn materialize(
        service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
        adapter: &Arc<MeerkatMachine>,
        session: Session,
    ) {
        let service_for_executor = Arc::clone(service);
        let adapter_for_executor = Arc::clone(adapter);
        Box::pin(materialize_session(
            service,
            adapter,
            session,
            create_request(),
            move |session_id| {
                default_persistent_executor(service_for_executor, adapter_for_executor, session_id)
            },
        ))
        .await
        .expect("materialize session");
    }

    #[cfg(all(feature = "memory-store-session", feature = "session-compaction"))]
    async fn materialize_callback_memory_session(
        service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
        adapter: &Arc<MeerkatMachine>,
        session: Session,
    ) {
        let service_for_executor = Arc::clone(service);
        let adapter_for_executor = Arc::clone(adapter);
        Box::pin(materialize_session(
            service,
            adapter,
            session,
            callback_memory_request(),
            move |session_id| {
                default_persistent_executor(service_for_executor, adapter_for_executor, session_id)
            },
        ))
        .await
        .expect("materialize callback-memory session");
    }

    async fn run_prompt(
        adapter: &Arc<MeerkatMachine>,
        session_id: &meerkat::SessionId,
        prompt: &str,
    ) {
        let (_outcome, handle) = adapter
            .accept_input_with_completion(session_id, Input::Prompt(PromptInput::new(prompt, None)))
            .await
            .expect("accept prompt input");
        let handle = handle.expect("completion handle");
        let outcome = tokio::time::timeout(Duration::from_secs(10), handle.wait())
            .await
            .expect("prompt should complete in time")
            .expect("completion waiter should resolve");
        assert!(
            matches!(outcome, CompletionOutcome::Completed(_)),
            "unexpected completion outcome: {outcome:?}"
        );
    }

    fn user_texts(session: &Session) -> Vec<String> {
        session
            .messages()
            .iter()
            .filter_map(|message| match message {
                meerkat_core::Message::User(user) => Some(user.text_content()),
                _ => None,
            })
            .collect()
    }

    fn has_compaction_summary(session: &Session) -> bool {
        session.messages().iter().any(|message| {
            matches!(
                message,
                meerkat_core::Message::User(user)
                    if user.transcript_role == meerkat_core::types::TranscriptUserRole::CompactionSummary
            )
        })
    }

    fn rewrite_commit_count(session: &Session) -> usize {
        session
            .transcript_history_state()
            .expect("transcript history state must deserialize")
            .map(|state| state.commits.len())
            .unwrap_or(0)
    }

    fn compaction_cadence(session: &Session) -> meerkat_core::compact::SessionCompactionCadence {
        serde_json::from_value(
            session
                .metadata()
                .get(meerkat_core::compact::SESSION_COMPACTION_CADENCE_KEY)
                .expect("compaction cadence metadata must be durable")
                .clone(),
        )
        .expect("compaction cadence metadata must deserialize")
    }

    /// One compaction (summary injection rewrite) before the cold restart.
    ///
    /// With `auto_compact_threshold_override = 1`, the estimated-history
    /// trigger fires at the second pre-LLM boundary (turn 2). The compacted
    /// transcript is committed and fully flushed to the runtime store, the
    /// durable row, and the event store before the host dies, so this also
    /// exercises the "row already carries the rewritten transcript" replay
    /// idempotency variant.
    #[tokio::test]
    async fn cold_restart_resume_after_compaction_rewrite() {
        let temp = tempfile::tempdir().expect("tempdir");

        // First host lifetime: create the session and run two turns; the
        // second turn triggers compaction at its pre-LLM boundary.
        let session_id = {
            let (service, adapter) = build_service(temp.path()).await;
            let session = Session::new();
            let session_id = session.id().clone();
            materialize(&service, &adapter, session).await;
            run_prompt(&adapter, &session_id, "first turn before restart").await;
            run_prompt(&adapter, &session_id, "second turn triggers compaction").await;

            let compacted = service
                .load_authoritative_session(&session_id)
                .await
                .expect("authoritative load after compaction turn")
                .expect("session should exist");
            assert!(
                has_compaction_summary(&compacted),
                "test setup failed: compaction did not run before the restart; \
                 messages: {:?}",
                user_texts(&compacted)
            );
            assert!(
                rewrite_commit_count(&compacted) >= 1,
                "test setup failed: compaction left no TranscriptRewriteCommit"
            );
            // Cold stop: the host dies without archiving or retiring anything.
            session_id
        };

        // Second host lifetime: fresh service + runtime authority over the
        // same durable stores.
        let (service, adapter) = build_service(temp.path()).await;
        let resume_source = service
            .load_authoritative_session(&session_id)
            .await
            .expect("authoritative load after restart")
            .expect("session should survive restart");
        assert!(
            has_compaction_summary(&resume_source),
            "authoritative resume source must carry the compacted transcript"
        );
        assert!(
            user_texts(&resume_source)
                .iter()
                .any(|text| text.contains("second turn triggers compaction")),
            "authoritative session must carry pre-restart history"
        );

        materialize(&service, &adapter, resume_source).await;
        run_prompt(&adapter, &session_id, "third turn after restart").await;

        let final_session = service
            .load_authoritative_session(&session_id)
            .await
            .expect("authoritative load after resumed turn")
            .expect("session should still exist");
        let texts = user_texts(&final_session);
        assert!(
            texts
                .iter()
                .any(|t| t.contains("second turn triggers compaction")),
            "history from before the restart must survive: {texts:?}"
        );
        assert!(
            texts.iter().any(|t| t.contains("third turn after restart")),
            "the post-restart turn must be recorded: {texts:?}"
        );
    }

    /// Callback-pending is a committed runtime terminal, not a run failure.
    /// When it follows a durable compaction stage, the same runtime boundary
    /// must atomically carry the typed rewrite into the outbox, finalize the
    /// memory projection, clear the intent, and leave both facts recoverable
    /// after a cold reopen.
    #[cfg(all(feature = "memory-store-session", feature = "session-compaction"))]
    #[tokio::test]
    async fn callback_pending_compaction_projection_finalizes_before_cold_restart() {
        let temp = tempfile::tempdir().expect("tempdir");
        let discarded_source = "discarded callback source before compaction";

        let session_id = {
            let client: Arc<dyn LlmClient> = Arc::new(CallbackPendingAfterCompactionClient::new());
            let (service, adapter, runtime_store) =
                build_callback_memory_service(temp.path(), client);
            let session = Session::new();
            let session_id = session.id().clone();
            materialize_callback_memory_session(&service, &adapter, session).await;

            run_prompt(&adapter, &session_id, discarded_source).await;
            let callback = run_prompt_capture(
                &adapter,
                &session_id,
                "second turn compacts then requests callback",
            )
            .await;
            assert!(
                matches!(
                    &callback,
                    Ok(CompletionOutcome::CallbackPending { tool_name, .. })
                        if tool_name == "external_callback"
                ),
                "compacted turn must resolve as callback-pending: {callback:?}"
            );

            let authoritative = service
                .load_authoritative_session(&session_id)
                .await
                .expect("load authoritative callback-pending session")
                .expect("callback-pending session must exist");
            assert!(
                has_compaction_summary(&authoritative),
                "callback-pending commit must retain the typed compaction rewrite"
            );
            assert!(
                rewrite_commit_count(&authoritative) >= 1,
                "callback-pending commit must retain its TranscriptRewriteCommit"
            );
            assert!(
                authoritative
                    .compaction_projection_intents()
                    .expect("read callback-pending compaction intents")
                    .is_empty(),
                "runtime finalization must clear the session-carried projection intent"
            );

            let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(&session_id);
            assert!(
                runtime_store
                    .load_pending_compaction_projections(&runtime_id)
                    .await
                    .expect("load callback-pending projection outbox")
                    .is_empty(),
                "callback-pending completion must drain the committed runtime outbox"
            );
            let runtime_snapshot = runtime_store
                .load_session_snapshot(&runtime_id)
                .await
                .expect("load callback-pending runtime snapshot")
                .expect("callback-pending runtime snapshot must exist");
            let runtime_session: Session = serde_json::from_slice(&runtime_snapshot)
                .expect("callback-pending runtime snapshot must be a session");
            assert!(has_compaction_summary(&runtime_session));
            assert!(
                runtime_session
                    .compaction_projection_intents()
                    .expect("read runtime snapshot compaction intents")
                    .is_empty(),
                "finalization acknowledgement must clean the authoritative runtime snapshot"
            );

            let memory_store =
                meerkat_memory::HnswMemoryStore::open(temp.path().join("sessions").join("memory"))
                    .expect("reopen finalized memory store in first lifetime");
            let page = memory_store
                .enumerate_scoped(
                    &MemorySearchScope::for_session(session_id.clone()),
                    MemoryEnumerationRequest {
                        limit: 64,
                        offset: 0,
                        source_overlap: None,
                        indexed_after: None,
                    },
                )
                .await
                .expect("enumerate finalized compaction memory");
            let source_record = page
                .records
                .iter()
                .find(|record| record.content.contains(discarded_source))
                .expect("discarded source must be visible only after finalization");
            assert_eq!(source_record.metadata.session_id, session_id);
            assert!(matches!(
                &source_record.metadata.source,
                meerkat_core::MemorySource::Compaction { source_range }
                    if source_range.start() < source_range.end()
            ));
            session_id
        };

        // A new service, runtime store, agent, and HNSW handle over the same
        // files must recover the paired rewrite/index state without reviving
        // a finalized outbox intent.
        let client: Arc<dyn LlmClient> = Arc::new(CallbackPendingAfterCompactionClient::new());
        let (service, adapter, runtime_store) = build_callback_memory_service(temp.path(), client);
        let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(&session_id);
        assert!(
            runtime_store
                .load_pending_compaction_projections(&runtime_id)
                .await
                .expect("load cold-reopened projection outbox")
                .is_empty(),
            "cold reopen must not revive a finalized projection intent"
        );
        let resume_source = service
            .load_authoritative_session(&session_id)
            .await
            .expect("load callback-pending session after cold reopen")
            .expect("callback-pending session must survive cold reopen");
        assert!(has_compaction_summary(&resume_source));
        assert!(
            resume_source
                .compaction_projection_intents()
                .expect("read cold-reopened compaction intents")
                .is_empty()
        );
        materialize_callback_memory_session(&service, &adapter, resume_source).await;

        let reopened_memory =
            meerkat_memory::HnswMemoryStore::open(temp.path().join("sessions").join("memory"))
                .expect("cold reopen finalized memory store");
        let page = reopened_memory
            .enumerate_scoped(
                &MemorySearchScope::for_session(session_id),
                MemoryEnumerationRequest {
                    limit: 64,
                    offset: 0,
                    source_overlap: None,
                    indexed_after: None,
                },
            )
            .await
            .expect("enumerate memory after cold reopen");
        assert!(
            page.records
                .iter()
                .any(|record| record.content.contains(discarded_source)),
            "discarded source memory must survive a cold HNSW reopen"
        );
    }

    /// An audit-log I/O failure happens after the runtime atomically commits
    /// the compacted transcript and outbox and after HNSW publishes the staged
    /// memory. It must not quarantine that valid runtime authority. Recovery
    /// retries the pending outbox, appends the missing audit, clears the intent,
    /// and leaves exactly one visible memory record.
    #[cfg(all(feature = "memory-store-session", feature = "session-compaction"))]
    #[tokio::test]
    async fn rewrite_audit_append_failure_preserves_compaction_outbox_for_idempotent_recovery() {
        let temp = tempfile::tempdir().expect("tempdir");
        let discarded_source = "discarded source before audit append failure";

        let session_id = {
            let client: Arc<dyn LlmClient> = Arc::new(CallbackPendingAfterCompactionClient::new());
            let (service, adapter, runtime_store, event_store) =
                build_rewrite_audit_failure_memory_service(temp.path(), client);
            let session = Session::new();
            let session_id = session.id().clone();
            materialize_callback_memory_session(&service, &adapter, session).await;
            run_prompt(&adapter, &session_id, discarded_source).await;

            event_store.arm();
            let outcome = run_prompt_capture(
                &adapter,
                &session_id,
                "second turn compacts before the rewrite audit append fails",
            )
            .await;
            assert_eq!(
                event_store.failed_rewrite_appends(),
                1,
                "test setup must fail exactly the post-commit rewrite audit append"
            );
            assert!(
                !matches!(outcome, Ok(CompletionOutcome::CallbackPending { .. })),
                "a failed durable audit projection must not report clean callback completion: {outcome:?}"
            );

            let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(&session_id);
            let pending = runtime_store
                .load_pending_compaction_projections(&runtime_id)
                .await
                .expect("load outbox after audit append failure");
            assert_eq!(
                pending.len(),
                1,
                "the committed outbox must remain pending for recovery retry"
            );
            assert!(
                !runtime_store
                    .is_runtime_projection_quarantined(&runtime_id)
                    .await
                    .expect("load runtime projection quarantine marker"),
                "audit infrastructure failure must not quarantine valid runtime authority"
            );
            let snapshot = runtime_store
                .load_session_snapshot(&runtime_id)
                .await
                .expect("load runtime snapshot after audit append failure")
                .expect("valid committed runtime snapshot must survive audit append failure");
            let runtime_session: Session =
                serde_json::from_slice(&snapshot).expect("runtime snapshot is a session");
            assert!(has_compaction_summary(&runtime_session));
            assert_eq!(
                runtime_session
                    .compaction_projection_intents()
                    .expect("read runtime snapshot outbox")
                    .len(),
                1,
                "the surviving snapshot must carry the retryable projection intent"
            );

            let memory_store =
                meerkat_memory::HnswMemoryStore::open(temp.path().join("sessions").join("memory"))
                    .expect("reopen HNSW after audit append failure");
            let page = memory_store
                .enumerate_scoped(
                    &MemorySearchScope::for_session(session_id.clone()),
                    MemoryEnumerationRequest {
                        limit: 64,
                        offset: 0,
                        source_overlap: None,
                        indexed_after: None,
                    },
                )
                .await
                .expect("enumerate memory after audit append failure");
            assert_eq!(
                page.records
                    .iter()
                    .filter(|record| record.content.contains(discarded_source))
                    .count(),
                1,
                "memory is visible once while its paired runtime outbox remains retryable"
            );
            let rewrite_audits = event_store
                .read_from(&session_id, 1)
                .await
                .expect("read event log after injected failure")
                .into_iter()
                .filter(|stored| {
                    matches!(
                        stored.event,
                        meerkat_core::AgentEvent::TranscriptRewriteCommitted { .. }
                    )
                })
                .count();
            assert_eq!(
                rewrite_audits, 0,
                "the injected append failure must leave the audit missing until retry"
            );

            // The failed compatibility projection stops this host and starts
            // the machine-owned unregister saga asynchronously. The
            // completion waiter above proves the run outcome, not that the
            // old host has finished detaching. Join that exact saga before
            // constructing host 2 so cold recovery cannot race a durable
            // Draining prefix from host 1.
            wait_for_canonical_unregister_completion(&adapter, &session_id).await;
            assert_eq!(
                runtime_store
                    .load_pending_compaction_projections(&runtime_id)
                    .await
                    .expect("reload outbox after host-1 unregister")
                    .len(),
                1,
                "host teardown must preserve the committed compaction outbox for cold recovery"
            );
            session_id
        };

        // New service/runtime/agent/HNSW handles model a process restart. Feed
        // it the surviving runtime snapshot directly; runtime recovery owns
        // outbox reconciliation and must converge every derived projection.
        let client: Arc<dyn LlmClient> = Arc::new(CallbackPendingAfterCompactionClient::new());
        let (service, adapter, runtime_store, event_store) =
            build_rewrite_audit_failure_memory_service(temp.path(), client);
        let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(&session_id);
        let snapshot = runtime_store
            .load_session_snapshot(&runtime_id)
            .await
            .expect("load surviving runtime snapshot for recovery")
            .expect("runtime snapshot must survive into the new process");
        let resume_source: Session =
            serde_json::from_slice(&snapshot).expect("recovery snapshot is a session");
        materialize_callback_memory_session(&service, &adapter, resume_source).await;

        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if runtime_store
                    .load_pending_compaction_projections(&runtime_id)
                    .await
                    .expect("poll recovering outbox")
                    .is_empty()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("recovery must finalize the pending compaction outbox");

        let recovered_snapshot = runtime_store
            .load_session_snapshot(&runtime_id)
            .await
            .expect("load recovered runtime snapshot")
            .expect("recovered runtime snapshot must remain authoritative");
        let recovered_session: Session =
            serde_json::from_slice(&recovered_snapshot).expect("recovered snapshot is a session");
        assert!(has_compaction_summary(&recovered_session));
        assert!(
            recovered_session
                .compaction_projection_intents()
                .expect("read recovered snapshot outbox")
                .is_empty(),
            "recovery acknowledgement must clean the runtime snapshot intent"
        );
        assert!(
            !runtime_store
                .is_runtime_projection_quarantined(&runtime_id)
                .await
                .expect("load recovered quarantine marker"),
            "successful retry must retain the runtime snapshot as authority"
        );

        let memory_store =
            meerkat_memory::HnswMemoryStore::open(temp.path().join("sessions").join("memory"))
                .expect("cold reopen recovered HNSW");
        let page = memory_store
            .enumerate_scoped(
                &MemorySearchScope::for_session(session_id.clone()),
                MemoryEnumerationRequest {
                    limit: 64,
                    offset: 0,
                    source_overlap: None,
                    indexed_after: None,
                },
            )
            .await
            .expect("enumerate memory after recovery retry");
        assert_eq!(
            page.records
                .iter()
                .filter(|record| record.content.contains(discarded_source))
                .count(),
            1,
            "idempotent recovery must not duplicate already-published memory"
        );
        let rewrite_audits = event_store
            .read_from(&session_id, 1)
            .await
            .expect("read recovered event log")
            .into_iter()
            .filter(|stored| {
                matches!(
                    stored.event,
                    meerkat_core::AgentEvent::TranscriptRewriteCommitted { .. }
                )
            })
            .count();
        assert!(
            rewrite_audits >= 1,
            "recovery must append the missing rewrite audit"
        );
    }

    /// A rejected RuntimeStore atomic boundary has no committed outbox
    /// authority. The live agent must therefore roll back the compaction
    /// rewrite and delete its exact invisible HNSW stage immediately; later
    /// recovery must have no orphan left to discover or clean up.
    #[cfg(all(feature = "memory-store-session", feature = "session-compaction"))]
    #[tokio::test]
    async fn atomic_apply_failure_aborts_live_compaction_stage_before_teardown() {
        let temp = tempfile::tempdir().expect("tempdir");
        let compaction_attempts = Arc::new(AtomicUsize::new(0));
        let client: Arc<dyn LlmClient> = Arc::new(CompactionFailureTrackingClient::new(
            false,
            Arc::clone(&compaction_attempts),
        ));
        let (service, adapter, fault_store, runtime_store) =
            build_atomic_apply_failure_memory_service(temp.path(), client);
        let session = Session::new();
        let session_id = session.id().clone();
        materialize_callback_memory_session(&service, &adapter, session).await;
        run_prompt(
            &adapter,
            &session_id,
            "source that would be projected if the boundary committed",
        )
        .await;

        let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(&session_id);
        let baseline_snapshot = runtime_store
            .load_session_snapshot(&runtime_id)
            .await
            .expect("load baseline runtime snapshot")
            .expect("first turn must commit a runtime snapshot");
        let baseline: Session =
            serde_json::from_slice(&baseline_snapshot).expect("baseline snapshot is a session");
        assert!(!has_compaction_summary(&baseline));

        fault_store.arm();
        let outcome = run_prompt_capture(
            &adapter,
            &session_id,
            "second turn stages compaction then fails atomic apply",
        )
        .await;
        assert!(
            matches!(
                outcome,
                Ok(CompletionOutcome::CompletedWithFinalizationFailure { .. }
                    | CompletionOutcome::RuntimeTerminated { .. })
                    | Err(_)
            ),
            "a rejected runtime boundary must not report success: {outcome:?}"
        );
        assert_eq!(
            compaction_attempts.load(Ordering::SeqCst),
            1,
            "test setup must reach the compaction stage before atomic_apply fails"
        );
        assert_eq!(
            runtime_store
                .load_session_snapshot(&runtime_id)
                .await
                .expect("reload runtime snapshot after rejected atomic apply"),
            Some(baseline_snapshot),
            "the rejected atomic boundary must leave the old transcript authoritative"
        );
        assert!(
            runtime_store
                .load_pending_compaction_projections(&runtime_id)
                .await
                .expect("load outbox after rejected atomic apply")
                .is_empty(),
            "a pre-commit failure must leave no runtime outbox authority"
        );

        let memory_store =
            meerkat_memory::HnswMemoryStore::open(temp.path().join("sessions").join("memory"))
                .expect("reopen HNSW memory after rejected atomic apply");
        let cleanup = memory_store
            .reconcile_compaction_stages(
                &meerkat_core::MemoryOwner::canonical_session(session_id.clone()),
                &[],
            )
            .await
            .expect("reconcile empty authority after live abort");
        assert_eq!(
            cleanup.aborted_orphans, 0,
            "the live post-error abort must delete the stage before teardown; recovery must find no orphan"
        );
        let page = memory_store
            .enumerate_scoped(
                &MemorySearchScope::for_session(session_id),
                MemoryEnumerationRequest {
                    limit: 64,
                    offset: 0,
                    source_overlap: None,
                    indexed_after: None,
                },
            )
            .await
            .expect("enumerate memory after rejected atomic apply");
        assert!(
            page.records.is_empty(),
            "an uncommitted compaction stage must never become query-visible"
        );
    }

    /// A failed compaction attempt is itself a durable cadence boundary. A
    /// process restart must not forget it and immediately hammer the compaction
    /// path again before `min_turns_between_compactions` has elapsed.
    #[tokio::test]
    async fn failed_compaction_attempt_cadence_survives_cold_restart() {
        let temp = tempfile::tempdir().expect("tempdir");
        let first_lifetime_attempts = Arc::new(AtomicUsize::new(0));

        let session_id = {
            let client: Arc<dyn LlmClient> = Arc::new(CompactionFailureTrackingClient::new(
                true,
                Arc::clone(&first_lifetime_attempts),
            ));
            let (service, adapter) = build_service_with_client(temp.path(), client).await;
            let session = Session::new();
            let session_id = session.id().clone();
            materialize(&service, &adapter, session).await;
            run_prompt(&adapter, &session_id, "first turn before failed compaction").await;
            run_prompt(&adapter, &session_id, "second turn attempts compaction").await;

            assert_eq!(
                first_lifetime_attempts.load(Ordering::SeqCst),
                1,
                "the first process lifetime must execute exactly one failed compaction attempt"
            );
            let persisted = service
                .load_authoritative_session(&session_id)
                .await
                .expect("authoritative load after failed compaction")
                .expect("session should exist");
            let cadence = compaction_cadence(&persisted);
            assert_eq!(cadence.session_boundary_index, 2);
            assert_eq!(cadence.last_compaction_boundary_index, None);
            assert_eq!(cadence.last_compaction_attempt_boundary_index, Some(1));
            session_id
        };

        let restarted_attempts = Arc::new(AtomicUsize::new(0));
        let client: Arc<dyn LlmClient> = Arc::new(CompactionFailureTrackingClient::new(
            false,
            Arc::clone(&restarted_attempts),
        ));
        let (service, adapter) = build_service_with_client(temp.path(), client).await;
        let resume_source = service
            .load_authoritative_session(&session_id)
            .await
            .expect("authoritative load after restart")
            .expect("session should survive restart");
        assert_eq!(
            compaction_cadence(&resume_source).last_compaction_attempt_boundary_index,
            Some(1),
            "the failed-attempt boundary must survive the cold restart"
        );

        materialize(&service, &adapter, resume_source).await;
        run_prompt(
            &adapter,
            &session_id,
            "third turn immediately after failed-attempt restart",
        )
        .await;

        assert_eq!(
            restarted_attempts.load(Ordering::SeqCst),
            0,
            "the first post-restart boundary must be cadence-guarded from immediate retry"
        );
        let persisted = service
            .load_authoritative_session(&session_id)
            .await
            .expect("authoritative load after guarded resumed turn")
            .expect("session should still exist");
        let cadence = compaction_cadence(&persisted);
        assert_eq!(cadence.session_boundary_index, 3);
        assert_eq!(cadence.last_compaction_boundary_index, None);
        assert_eq!(cadence.last_compaction_attempt_boundary_index, Some(1));
    }

    /// Session-store wrapper modeling a host that loses the ability to write
    /// the durable session row just before dying (the kill window between the
    /// runtime-store commits and the row projection write inside
    /// `save_normalized_session` / `persist_normalized_transcript_rewrite_chain`).
    /// All reads and all other stores stay healthy; only durable row writes
    /// fail while armed. Nothing is ever written that real code did not write.
    struct KillWindowSessionStore {
        inner: Arc<dyn SessionStore>,
        fail_row_writes: Arc<AtomicBool>,
    }

    impl KillWindowSessionStore {
        fn fail_if_armed(&self) -> Result<(), SessionStoreError> {
            if self.fail_row_writes.load(Ordering::Acquire) {
                return Err(SessionStoreError::Internal(
                    "kill window: durable session row write lost before host death".to_string(),
                ));
            }
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl SessionStore for KillWindowSessionStore {
        async fn save(&self, session: &Session) -> Result<(), SessionStoreError> {
            self.fail_if_armed()?;
            self.inner.save(session).await
        }

        async fn save_transcript_rewrite(
            &self,
            session: &Session,
            commit: &meerkat_core::TranscriptRewriteCommit,
        ) -> Result<(), SessionStoreError> {
            self.fail_if_armed()?;
            self.inner.save_transcript_rewrite(session, commit).await
        }

        async fn save_authoritative_projection(
            &self,
            session: &Session,
        ) -> Result<(), SessionStoreError> {
            self.fail_if_armed()?;
            self.inner.save_authoritative_projection(session).await
        }

        async fn save_authoritative_projection_if_current_revision(
            &self,
            session: &Session,
            expected_current_revision: Option<String>,
        ) -> Result<(), SessionStoreError> {
            self.fail_if_armed()?;
            self.inner
                .save_authoritative_projection_if_current_revision(
                    session,
                    expected_current_revision,
                )
                .await
        }

        async fn load(
            &self,
            id: &meerkat::SessionId,
        ) -> Result<Option<Session>, SessionStoreError> {
            self.inner.load(id).await
        }

        async fn list(
            &self,
            filter: SessionFilter,
        ) -> Result<Vec<meerkat_core::SessionMeta>, SessionStoreError> {
            self.inner.list(filter).await
        }

        async fn delete(&self, id: &meerkat::SessionId) -> Result<(), SessionStoreError> {
            self.fail_if_armed()?;
            self.inner.delete(id).await
        }

        async fn delete_if_current_revision(
            &self,
            id: &meerkat::SessionId,
            expected_current_revision: &str,
        ) -> Result<bool, SessionStoreError> {
            self.fail_if_armed()?;
            self.inner
                .delete_if_current_revision(id, expected_current_revision)
                .await
        }
    }

    /// Build a runtime-backed service over explicit sqlite stores (same layout
    /// as the sqlite realm backend: session row + runtime snapshots share one
    /// sqlite file, file event store + projector under `.rkat/`), with the
    /// durable session row behind the kill-window wrapper.
    fn build_kill_window_service(
        root: &std::path::Path,
        fail_row_writes: Arc<AtomicBool>,
    ) -> (
        Arc<PersistentSessionService<FactoryAgentBuilder>>,
        Arc<MeerkatMachine>,
        Arc<dyn SessionStore>,
    ) {
        let sqlite_path = root.join("sessions.sqlite3");
        let raw_row_store: Arc<dyn SessionStore> =
            Arc::new(meerkat::SqliteSessionStore::open(sqlite_path.clone()).expect("open sqlite"));
        let session_store: Arc<dyn SessionStore> = Arc::new(KillWindowSessionStore {
            inner: Arc::clone(&raw_row_store),
            fail_row_writes,
        });
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
            meerkat_runtime::store::SqliteRuntimeStore::new(sqlite_path)
                .expect("open runtime sqlite"),
        );
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::FsBlobStore::new(root.join("blobs")));
        let bundle = PersistenceBundle::new(session_store, runtime_store, blob_store);

        let factory = AgentFactory::new(root.join("sessions"));
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        builder.default_llm_client = Some(Arc::new(HistorySummarizingTestClient));
        let (service, adapter) = build_runtime_backed_service(builder, 4, bundle);
        let service = service.with_event_projection(
            Arc::new(meerkat_session::event_store::FileEventStore::new(
                root.join(".rkat").join("events"),
            )),
            Arc::new(meerkat_session::projector::SessionProjector::new(
                root.join(".rkat"),
            )),
        );
        (Arc::new(service), adapter, raw_row_store)
    }

    /// Like `run_prompt`, but returns the outcome (or the error text) instead
    /// of asserting completion — used for the turn that dies mid-persist.
    async fn run_prompt_capture(
        adapter: &Arc<MeerkatMachine>,
        session_id: &meerkat::SessionId,
        prompt: &str,
    ) -> Result<CompletionOutcome, String> {
        let (_outcome, handle) = adapter
            .accept_input_with_completion(session_id, Input::Prompt(PromptInput::new(prompt, None)))
            .await
            .map_err(|err| format!("accept_input failed: {err}"))?;
        let handle = handle.ok_or_else(|| "missing completion handle".to_string())?;
        match tokio::time::timeout(Duration::from_secs(10), handle.wait()).await {
            Err(_) => Err("prompt timed out".to_string()),
            Ok(Err(err)) => Err(format!("completion waiter failed: {err:?}")),
            Ok(Ok(outcome)) => Ok(outcome),
        }
    }

    async fn wait_for_canonical_unregister_completion(
        adapter: &Arc<MeerkatMachine>,
        session_id: &meerkat::SessionId,
    ) {
        let expected_runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(session_id);
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if !adapter.contains_session(session_id).await {
                    break;
                }
                match adapter.unregister_session(session_id).await {
                    Ok(()) => {}
                    Err(meerkat_runtime::RuntimeDriverError::UnregisterInProgress {
                        runtime_id,
                    }) => {
                        assert_eq!(
                            runtime_id, expected_runtime_id,
                            "unregister coordinator reported the wrong runtime identity"
                        );
                    }
                    Err(error) => {
                        assert!(
                            !adapter.contains_session(session_id).await,
                            "unregister coordinator failed for runtime {expected_runtime_id}: {error}"
                        );
                        break;
                    }
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("unregister coordinator should complete before the process boundary");
        assert!(
            !adapter.contains_session(session_id).await,
            "completed unregister must remove runtime {expected_runtime_id}"
        );
    }

    /// Kill between commit points: the host dies (loses durable row writes)
    /// during the compaction turn's boundary persist, AFTER the runtime-store
    /// snapshots and the event-store rewrite audit committed but BEFORE the
    /// durable session row was written. On restart the runtime snapshot is the
    /// preferred authority (post-compaction) while the durable row is still the
    /// pre-compaction transcript. Resume must accept the rewrite chain against
    /// the lagged row and converge it — not fail closed with
    /// `TranscriptContinuityViolation`.
    #[tokio::test]
    async fn cold_restart_resume_after_compaction_with_lagged_durable_row() {
        let temp = tempfile::tempdir().expect("tempdir");
        let fail_row_writes = Arc::new(AtomicBool::new(false));

        // First host lifetime.
        let session_id = {
            let (service, adapter, raw_row_store) =
                build_kill_window_service(temp.path(), Arc::clone(&fail_row_writes));
            let session = Session::new();
            let session_id = session.id().clone();
            materialize(&service, &adapter, session).await;
            run_prompt(&adapter, &session_id, "first turn before crash").await;

            // Arm the kill window: from here on the host can no longer write
            // the durable session row (it is about to die mid-persist).
            fail_row_writes.store(true, Ordering::Release);

            // The compaction turn: runtime snapshots + rewrite audit events
            // commit; the durable row write is best-effort on this path
            // (checkpointer failures are warn-only), so the turn itself may
            // still complete — which is exactly how production reaches this
            // state: runtime authority advanced, row projection stale, host
            // dies before any later write converges it.
            let outcome =
                run_prompt_capture(&adapter, &session_id, "second turn crashes mid-persist").await;
            assert!(
                outcome.is_ok(),
                "test setup failed: the compaction turn could not run at all: {outcome:?}"
            );

            // The durable row must still be the pre-compaction transcript.
            let row = raw_row_store
                .load(&session_id)
                .await
                .expect("load durable row")
                .expect("durable row exists");
            assert!(
                !has_compaction_summary(&row),
                "test setup failed: durable row unexpectedly carries the compacted transcript"
            );

            // The failed compatibility checkpoint stops the runtime after its
            // committed snapshot is durable. Its completion waiter resolves
            // before the independently owned unregister saga necessarily
            // finishes, and the executor itself retains the service/adapter
            // Arcs, so merely leaving this lexical scope is not a process
            // boundary. Join the canonical saga before constructing host 2;
            // otherwise host 2 can race a durable Draining prefix and this
            // transcript-projection test accidentally becomes an unregister
            // recovery test.
            wait_for_canonical_unregister_completion(&adapter, &session_id).await;
            session_id
        };
        let runtime_store =
            meerkat_runtime::store::SqliteRuntimeStore::new(temp.path().join("sessions.sqlite3"))
                .expect("reopen runtime store after kill window");
        let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(&session_id);
        assert_eq!(
            meerkat_runtime::store::load_runtime_state(&runtime_store, &runtime_id)
                .await
                .expect("load completed runtime lifecycle after process boundary"),
            Some(meerkat_runtime::RuntimeState::Idle),
            "host 1 must durably commit the completed unregister lifecycle before host 2 starts"
        );
        let lifecycle_record = runtime_store
            .load_machine_lifecycle_record(&runtime_id)
            .await
            .expect("load completed machine lifecycle record")
            .expect("completed machine lifecycle record must remain queryable");
        let lifecycle_record: serde_json::Value = serde_json::from_slice(&lifecycle_record)
            .expect("completed machine lifecycle record must remain valid JSON");
        assert_eq!(
            lifecycle_record.get("unregister_progress"),
            Some(&serde_json::Value::Null),
            "host 2 must not inherit an unfinished unregister prefix from host 1"
        );
        let runtime_snapshot = runtime_store
            .load_session_snapshot(&runtime_id)
            .await
            .expect("load runtime snapshot after kill window");
        let quarantined = runtime_store
            .is_runtime_projection_quarantined(&runtime_id)
            .await
            .expect("load runtime projection quarantine marker");
        let pending_projections = runtime_store
            .load_pending_compaction_projections(&runtime_id)
            .await
            .expect("load compaction outbox after kill window");
        assert!(
            runtime_snapshot.is_some(),
            "committed runtime snapshot must survive projection failure; quarantined={quarantined}, pending_projections={pending_projections:?}"
        );
        let runtime_snapshot = runtime_snapshot
            .expect("asserted committed runtime snapshot must survive projection failure");
        let runtime_session: Session = serde_json::from_slice(&runtime_snapshot)
            .expect("committed runtime snapshot must remain a Session");
        assert!(
            has_compaction_summary(&runtime_session),
            "projection failure must not discard the committed compacted transcript"
        );
        assert!(
            !quarantined,
            "a failed compatibility projection must not quarantine committed runtime authority"
        );
        assert_eq!(
            pending_projections.len(),
            0,
            "this no-memory-store fixture must not invent a compaction projection outbox"
        );
        // Host restart clears the transient write fault.
        fail_row_writes.store(false, Ordering::Release);

        // Second host lifetime over the same durable stores.
        let (service, adapter, raw_row_store) =
            build_kill_window_service(temp.path(), Arc::clone(&fail_row_writes));
        let resume_source = service
            .load_authoritative_session(&session_id)
            .await
            .expect("authoritative load after restart")
            .expect("session should survive restart");
        assert!(
            has_compaction_summary(&resume_source),
            "runtime snapshot authority must carry the compacted transcript on restart"
        );
        assert!(
            rewrite_commit_count(&resume_source) >= 1,
            "resume source must retain the compaction rewrite commit"
        );

        materialize(&service, &adapter, resume_source).await;
        run_prompt(&adapter, &session_id, "third turn after restart").await;

        let final_session = service
            .load_authoritative_session(&session_id)
            .await
            .expect("authoritative load after resumed turn")
            .expect("session should still exist");
        let texts = user_texts(&final_session);
        assert!(
            texts.iter().any(|t| t.contains("first turn before crash")),
            "history from before the crash must survive: {texts:?}"
        );
        assert!(
            texts.iter().any(|t| t.contains("third turn after restart")),
            "the post-restart turn must be recorded: {texts:?}"
        );
        // The resume persist must have converged the lagged durable row onto
        // the compacted transcript.
        let row = raw_row_store
            .load(&session_id)
            .await
            .expect("load durable row after resume")
            .expect("durable row exists after resume");
        assert!(
            has_compaction_summary(&row),
            "resume must converge the lagged durable row onto the compacted transcript"
        );
    }

    /// Build a runtime-backed service over explicit sqlite stores (no
    /// wrapper), returning the raw sqlite session store so the test can
    /// inspect the incremental head/strand representation directly.
    fn build_plain_sqlite_service(
        root: &std::path::Path,
    ) -> (
        Arc<PersistentSessionService<FactoryAgentBuilder>>,
        Arc<MeerkatMachine>,
        Arc<meerkat::SqliteSessionStore>,
    ) {
        let sqlite_path = root.join("sessions.sqlite3");
        let raw_row_store =
            Arc::new(meerkat::SqliteSessionStore::open(sqlite_path.clone()).expect("open sqlite"));
        let session_store: Arc<dyn SessionStore> = raw_row_store.clone();
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
            meerkat_runtime::store::SqliteRuntimeStore::new(sqlite_path)
                .expect("open runtime sqlite"),
        );
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::FsBlobStore::new(root.join("blobs")));
        let bundle = PersistenceBundle::new(session_store, runtime_store, blob_store);

        let factory = AgentFactory::new(root.join("sessions"));
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        builder.default_llm_client = Some(Arc::new(HistorySummarizingTestClient));
        let (service, adapter) = build_runtime_backed_service(builder, 4, bundle);
        (Arc::new(service), adapter, raw_row_store)
    }

    /// Incremental-store cold-resume equality through the real sqlite store:
    /// create, turn, compact, kill, resume, turn. Pins the OB3 ask-11
    /// contract end to end — the session never writes a legacy whole-blob
    /// row, the persisted head SHRINKS across the compaction, cold resume
    /// reads are head + one strand range, and the resumed transcript is
    /// byte-equal to the pre-kill authority.
    #[tokio::test]
    async fn cold_restart_resume_after_compaction_incremental_head_representation() {
        use meerkat_core::session_store::IncrementalSessionStore;

        let temp = tempfile::tempdir().expect("tempdir");

        // First host lifetime: create + enough turns that compaction not only
        // injects a summary but genuinely SHRINKS the transcript (older turns
        // discarded under the recent-turn budget).
        let (session_id, pre_kill_digest) = {
            let (service, adapter, raw_store) = build_plain_sqlite_service(temp.path());
            let session = Session::new();
            let session_id = session.id().clone();
            materialize(&service, &adapter, session).await;
            for turn in 0..6 {
                run_prompt(&adapter, &session_id, &format!("pre-restart turn {turn}")).await;
            }

            let compacted = service
                .load_authoritative_session(&session_id)
                .await
                .expect("authoritative load after compaction turn")
                .expect("session should exist");
            assert!(
                has_compaction_summary(&compacted),
                "test setup failed: compaction did not run before the restart"
            );

            // The incremental representation is canonical: a head row exists,
            // the head strand covers exactly the live transcript, and no
            // legacy whole-blob row was ever written for this session.
            let inc: Arc<dyn IncrementalSessionStore> = (raw_store.clone()
                as Arc<dyn SessionStore>)
                .as_incremental()
                .expect("sqlite store must expose the incremental capability");
            let head = inc
                .load_head(&session_id)
                .await
                .expect("load_head")
                .expect("head row must exist for an incremental session");
            assert!(head.rewrite_count >= 1, "compaction must adopt a rewrite");
            let slim = raw_store
                .load(&session_id)
                .await
                .expect("slim load")
                .expect("session persisted");
            assert_eq!(slim.messages().len() as u64, head.message_count);
            // The adopted rewrite retains a parent body LONGER than its
            // revision: the persisted head genuinely shrank.
            let rewrites = inc.load_rewrites(&session_id).await.expect("load_rewrites");
            assert!(!rewrites.is_empty());
            assert!(
                rewrites
                    .iter()
                    .any(|record| record.commit.messages_after < record.commit.messages_before),
                "at least one adopted rewrite must shrink the transcript"
            );
            // Slim contract: the durable row carries no inline history state.
            assert!(
                slim.transcript_history_state()
                    .expect("state read")
                    .is_none(),
                "slim load must carry no inline history state"
            );

            let digest =
                meerkat_core::transcript_messages_digest(compacted.messages()).expect("digest");
            // Cold stop: the host dies without archiving or retiring anything.
            (session_id, digest)
        };

        // Second host lifetime: cold resume = head + ONE strand range.
        let (service, adapter, raw_store) = build_plain_sqlite_service(temp.path());
        let inc: Arc<dyn IncrementalSessionStore> = (raw_store.clone() as Arc<dyn SessionStore>)
            .as_incremental()
            .expect("incremental capability");
        let head = inc
            .load_head(&session_id)
            .await
            .expect("load_head")
            .expect("head row survives the restart");
        let strand_rows = inc
            .load_messages(&session_id, &head.strand, 0..head.message_count)
            .await
            .expect("one strand range read");
        assert_eq!(
            meerkat_core::transcript_messages_digest(&strand_rows).expect("digest"),
            head.head_revision,
            "head + one strand range must reproduce the live transcript"
        );

        let resume_source = service
            .load_authoritative_session(&session_id)
            .await
            .expect("authoritative load after restart")
            .expect("session should survive restart");
        assert!(
            has_compaction_summary(&resume_source),
            "resume source must carry the compacted transcript"
        );

        materialize(&service, &adapter, resume_source).await;
        run_prompt(&adapter, &session_id, "third turn after restart").await;

        let final_session = service
            .load_authoritative_session(&session_id)
            .await
            .expect("authoritative load after resumed turn")
            .expect("session should still exist");
        let texts = user_texts(&final_session);
        assert!(
            texts.iter().any(|t| t.contains("third turn after restart")),
            "the post-restart turn must be recorded: {texts:?}"
        );
        // Transcript equality across the kill: the pre-kill authority is a
        // digest-exact prefix of the resumed transcript.
        let prefix_len = resume_pre_kill_prefix_len(&final_session, &pre_kill_digest);
        assert!(prefix_len > 0, "pre-kill transcript must be recoverable");
        // The durable representation stays head-canonical after the resumed
        // turn: the head strand covers exactly the live transcript.
        let head = inc
            .load_head(&session_id)
            .await
            .expect("load_head")
            .expect("head row present");
        let slim = raw_store
            .load(&session_id)
            .await
            .expect("slim load")
            .expect("session persisted");
        assert_eq!(slim.messages().len() as u64, head.message_count);
    }

    /// Longest prefix of `session` whose digest equals `pre_kill_digest`
    /// (0 when none).
    fn resume_pre_kill_prefix_len(session: &Session, pre_kill_digest: &str) -> usize {
        for len in (1..=session.messages().len()).rev() {
            if meerkat_core::transcript_messages_digest(&session.messages()[..len])
                .map(|digest| digest == pre_kill_digest)
                .unwrap_or(false)
            {
                return len;
            }
        }
        0
    }

    /// Multiple compactions including an actual shrink (older turns discarded
    /// under the recent-turn budget) before the cold restart, then resume and
    /// run enough turns to trigger ANOTHER compaction after the restart, so
    /// the post-restart rewrite chain must anchor onto the pre-restart chain.
    #[tokio::test]
    async fn cold_restart_resume_after_compaction_shrink_then_post_restart_compaction() {
        let temp = tempfile::tempdir().expect("tempdir");

        // First host lifetime: enough turns for two compactions; the second
        // compaction discards messages beyond the recent-turn budget (4), so
        // the transcript genuinely shrinks.
        let session_id = {
            let (service, adapter) = build_service(temp.path()).await;
            let session = Session::new();
            let session_id = session.id().clone();
            materialize(&service, &adapter, session).await;
            for turn in 0..6 {
                run_prompt(&adapter, &session_id, &format!("pre-restart turn {turn}")).await;
            }

            let compacted = service
                .load_authoritative_session(&session_id)
                .await
                .expect("authoritative load after compaction turns")
                .expect("session should exist");
            assert!(
                has_compaction_summary(&compacted),
                "test setup failed: compaction did not run before the restart"
            );
            assert!(
                rewrite_commit_count(&compacted) >= 2,
                "test setup failed: expected at least two compaction rewrite commits, got {}",
                rewrite_commit_count(&compacted)
            );
            session_id
        };

        // Second host lifetime.
        let (service, adapter) = build_service(temp.path()).await;
        let resume_source = service
            .load_authoritative_session(&session_id)
            .await
            .expect("authoritative load after restart")
            .expect("session should survive restart");

        materialize(&service, &adapter, resume_source).await;
        // Run enough post-restart turns that the cadence guard
        // (min_turns_between_compactions = 3) expires and compaction fires
        // again on the restarted authority.
        for turn in 0..4 {
            run_prompt(&adapter, &session_id, &format!("post-restart turn {turn}")).await;
        }

        let final_session = service
            .load_authoritative_session(&session_id)
            .await
            .expect("authoritative load after resumed turns")
            .expect("session should still exist");
        let texts = user_texts(&final_session);
        assert!(
            texts.iter().any(|t| t.contains("post-restart turn 3")),
            "the post-restart turns must be recorded: {texts:?}"
        );
    }
}

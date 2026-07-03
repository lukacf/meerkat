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
    use std::sync::atomic::{AtomicBool, Ordering};

    use meerkat::surface::{
        build_runtime_backed_service, default_persistent_executor, materialize_session,
    };
    use meerkat::{
        AgentFactory, Config, CreateSessionRequest, FactoryAgentBuilder, PersistenceBundle,
        PersistentSessionService, Session,
    };
    use meerkat_client::TestClient;
    use meerkat_core::SessionBuildOptions;
    use meerkat_core::session_store::{SessionFilter, SessionStore, SessionStoreError};
    use meerkat_runtime::completion::CompletionOutcome;
    use meerkat_runtime::{Input, MeerkatMachine, PromptInput};
    use tokio::time::Duration;

    async fn build_service(
        root: &std::path::Path,
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
        builder.default_llm_client = Some(Arc::new(TestClient::default()));
        let (service, adapter) = build_runtime_backed_service(builder, 4, persistence);
        (Arc::new(service), adapter)
    }

    /// Same request shape as the passing baseline, plus an aggressive
    /// auto-compaction threshold so the estimated-history-tokens trigger fires
    /// on the second pre-LLM boundary (first boundary never compacts).
    fn create_request() -> CreateSessionRequest {
        let mut build = SessionBuildOptions::default();
        build.auto_compact_threshold_override = std::num::NonZeroU64::new(1);
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
        let bundle = PersistenceBundle::new(session_store, Some(runtime_store), blob_store);

        let factory = AgentFactory::new(root.join("sessions"));
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        builder.default_llm_client = Some(Arc::new(TestClient::default()));
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
            session_id
        };
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

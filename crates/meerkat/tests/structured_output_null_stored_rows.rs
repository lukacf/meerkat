//! A run whose structured output is JSON null leaves stored input rows that
//! read again.
//!
//! A runtime-backed persistent session over a SQLite realm, an output schema
//! whose root admits null, and a scripted model that answers `null`. Each
//! run completes with a structured output of JSON null. The stored input
//! rows of those runs are then read through a second handle on the realm's
//! runtime store, as a later process reads them.
//!
//! Before the fix every such row failed its digest check on every read: the
//! run's result was written with an explicit null and read back without it.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::disallowed_methods
)]

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
mod tests {
    use std::sync::Arc;

    use meerkat::surface::{
        build_runtime_backed_service, default_persistent_executor, materialize_session,
    };
    use meerkat::{
        AgentFactory, Config, CreateSessionRequest, FactoryAgentBuilder, OutputSchema,
        PersistentSessionService, Session,
    };
    use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
    use meerkat_core::SessionBuildOptions;
    use meerkat_runtime::completion::CompletionOutcome;
    use meerkat_runtime::store::{InputStateRow, SqliteRuntimeStore};
    use meerkat_runtime::{Input, LogicalRuntimeId, MeerkatMachine, PromptInput, RuntimeStore};
    use tokio::time::Duration;

    const REALM: &str = "structured-output-null";

    /// Answers every request with the text `null`.
    struct NullClient;

    #[async_trait::async_trait]
    impl LlmClient for NullClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(
            &'a self,
            request: &'a LlmRequest,
        ) -> std::pin::Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>>
        {
            let events = vec![
                LlmEvent::TextDelta {
                    delta: "null".to_string(),
                    meta: None,
                },
                LlmEvent::UsageUpdate {
                    usage: meerkat_core::TurnUsage::host_declared(
                        meerkat_core::Provider::OpenAI,
                        &request.model,
                        meerkat_core::Usage::default(),
                    ),
                },
                LlmEvent::Done {
                    outcome: LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
                },
            ];
            Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::OpenAI
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    async fn build_service(
        root: &std::path::Path,
    ) -> (
        Arc<PersistentSessionService<FactoryAgentBuilder>>,
        Arc<MeerkatMachine>,
    ) {
        let (_manifest, persistence) = meerkat::open_realm_persistence_in(
            root,
            REALM,
            Some(meerkat_store::RealmBackend::Sqlite),
            Some(meerkat_store::RealmOrigin::Explicit),
        )
        .await
        .expect("open realm persistence");
        let factory = AgentFactory::new(root.join("sessions"));
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        builder.default_llm_client = Some(Arc::new(NullClient));
        let (service, adapter) = build_runtime_backed_service(builder, 4, persistence)
            .expect("compose the runtime-backed service");
        (Arc::new(service), adapter)
    }

    /// A session whose output schema admits null at its root.
    fn create_request() -> CreateSessionRequest {
        let schema = OutputSchema::new(serde_json::json!({
            "type": ["object", "null"],
            "properties": {"name": {"type": "string"}}
        }))
        .expect("a schema whose root admits null is accepted");
        CreateSessionRequest {
            injected_context: Vec::new(),
            model: "gpt-5.4".to_string(),
            prompt: meerkat_core::ContentInput::Text(String::new()),
            system_prompt: meerkat::SystemPromptOverride::Set("answer in JSON".to_string()),
            max_tokens: None,
            event_tx: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions {
                output_schema: Some(schema),
                ..SessionBuildOptions::default()
            }),
            labels: None,
        }
    }

    /// Put a prompt on the session and require a run result whose structured
    /// output is JSON null.
    async fn a_run_answers_null(
        adapter: &Arc<MeerkatMachine>,
        session_id: &meerkat_core::types::SessionId,
        text: &str,
    ) {
        let (_outcome, completion) = adapter
            .accept_input_with_completion(
                session_id,
                Input::Prompt(PromptInput::new(text.to_string(), None)),
            )
            .await
            .expect("the prompt is admitted");
        let outcome = tokio::time::timeout(
            Duration::from_secs(30),
            completion.expect("a prompt has a completion").wait(),
        )
        .await
        .expect("the run answers")
        .expect("the completion resolves");
        let CompletionOutcome::Completed(result) = outcome else {
            panic!("the run completes with a result: {outcome:?}");
        };
        assert_eq!(result.structured_output, Some(serde_json::Value::Null));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn stored_rows_of_runs_with_a_null_structured_output_read_again() {
        let root = tempfile::tempdir().expect("temp dir");
        let (service, adapter) = build_service(root.path()).await;
        let session = Session::new();
        let session_id = session.id().clone();
        let service_for_executor = Arc::clone(&service);
        let adapter_for_executor = Arc::clone(&adapter);
        Box::pin(materialize_session(
            &service,
            &adapter,
            session,
            create_request(),
            move |session_id| {
                default_persistent_executor(service_for_executor, adapter_for_executor, session_id)
            },
        ))
        .await
        .expect("materialize the session");

        a_run_answers_null(&adapter, &session_id, "first").await;
        a_run_answers_null(&adapter, &session_id, "second").await;

        // The stored input rows, through a second handle on the realm's
        // runtime store: each is read from its stored bytes.
        let stored =
            SqliteRuntimeStore::open_existing(root.path().join(REALM).join("sessions.sqlite3"))
                .expect("open the realm's runtime store");
        let rows = stored
            .load_input_states(&LogicalRuntimeId::for_session(&session_id))
            .await
            .expect("load the session's stored input rows");
        assert_eq!(rows.len(), 2, "one stored input row per run: {rows:?}");
        for row in &rows {
            assert!(
                matches!(row, InputStateRow::Decoded(_)),
                "a stored input row does not read: {row:?}"
            );
        }
    }
}

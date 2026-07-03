//! Cold-restart resume contract.
//!
//! A host process that dies without archiving must be able to resume its
//! sessions after restart: rebuild the runtime authority over the same durable
//! stores, materialize the persisted session, and continue turns with history
//! intact. The first full-session persist after resume must be accepted by the
//! append-only save guard — the resume projection is not allowed to diverge
//! from the persisted session-store transcript.

#![allow(clippy::expect_used, clippy::unwrap_used)]

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
mod tests {
    use std::sync::Arc;

    use meerkat::surface::{
        build_runtime_backed_service, default_persistent_executor, materialize_session,
    };
    use meerkat::{
        AgentFactory, Config, CreateSessionRequest, FactoryAgentBuilder, PersistentSessionService,
        Session,
    };
    use meerkat_client::TestClient;
    use meerkat_core::SessionBuildOptions;
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

    fn create_request() -> CreateSessionRequest {
        CreateSessionRequest {
            injected_context: Vec::new(),
            model: "gpt-5.4".to_string(),
            prompt: meerkat_core::ContentInput::Text(String::new()),
            system_prompt: meerkat::SystemPromptOverride::Set(
                "cold restart resume contract".to_string(),
            ),
            max_tokens: None,
            event_tx: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions::default()),
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

    /// Ask B regression: the persisted row and the runtime-store snapshot can
    /// carry the same conversation with different construction bookkeeping
    /// (run identity, timestamps) — e.g. a row written by a pre-#808 binary,
    /// or a re-created authority that re-stamped its projection. Resume must
    /// treat the transcript revision as a content address: bookkeeping-only
    /// divergence must not fail the append-only save guard and strand the
    /// session.
    #[tokio::test]
    async fn cold_restart_resume_survives_rebookkept_persisted_row() {
        let temp = tempfile::tempdir().expect("tempdir");

        let session_id = {
            let (_manifest, persistence) = meerkat::open_realm_persistence_in(
                temp.path(),
                "restart-realm",
                Some(meerkat_store::RealmBackend::Sqlite),
                Some(meerkat_store::RealmOrigin::Explicit),
            )
            .await
            .expect("open realm persistence");
            let store = persistence.session_store();
            let factory = AgentFactory::new(temp.path().join("sessions"));
            let mut builder = FactoryAgentBuilder::new(factory, Config::default());
            builder.default_llm_client = Some(Arc::new(TestClient::default()));
            let (service, adapter) = build_runtime_backed_service(builder, 4, persistence);
            let (service, adapter) = (Arc::new(service), adapter);

            let session = Session::new();
            let session_id = session.id().clone();
            materialize(&service, &adapter, session).await;
            run_prompt(&adapter, &session_id, "first turn before restart").await;

            // Re-stamp the persisted row's construction bookkeeping while the
            // runtime-store snapshot keeps the original stamps: the same
            // conversation, divergent bookkeeping.
            let row = store
                .load(&session_id)
                .await
                .expect("load persisted row")
                .expect("row present");
            let mut value = serde_json::to_value(&row).expect("serialize row");
            let messages = value
                .get_mut("messages")
                .and_then(|messages| messages.as_array_mut())
                .expect("row carries a messages array");
            let mut restamped = 0usize;
            for message in messages.iter_mut() {
                let object = message.as_object_mut().expect("message object");
                if object.contains_key("created_at") {
                    object.insert(
                        "created_at".to_string(),
                        serde_json::json!("2001-01-01T00:00:00Z"),
                    );
                    restamped += 1;
                }
                if object.get("role").and_then(|role| role.as_str()) == Some("block_assistant") {
                    object.insert(
                        "identity".to_string(),
                        serde_json::json!({
                            "run_id": "01890a5d-ac96-774b-bcce-b302099a8057"
                        }),
                    );
                }
            }
            assert!(restamped > 0, "expected messages to re-stamp");
            let rebookkept: Session =
                serde_json::from_value(value).expect("re-stamped row deserializes");
            store
                .save_authoritative_projection(&rebookkept)
                .await
                .expect("write re-stamped projection row");
            session_id
        };

        // Cold restart: resume prefers the runtime snapshot (original stamps)
        // and the first post-resume persist proves continuity against the
        // re-stamped row. Content is identical, so this must succeed.
        let (service, adapter) = build_service(temp.path()).await;
        let resume_source = service
            .load_authoritative_session(&session_id)
            .await
            .expect("authoritative load after restart")
            .expect("session should survive restart");
        materialize(&service, &adapter, resume_source).await;
        run_prompt(&adapter, &session_id, "second turn after restart").await;

        let final_session = service
            .load_authoritative_session(&session_id)
            .await
            .expect("authoritative load after resumed turn")
            .expect("session should still exist");
        let texts = user_texts(&final_session);
        assert!(
            texts
                .iter()
                .any(|t| t.contains("first turn before restart")),
            "history from before the restart must survive: {texts:?}"
        );
        assert!(
            texts
                .iter()
                .any(|t| t.contains("second turn after restart")),
            "the post-restart turn must be recorded: {texts:?}"
        );
    }

    #[tokio::test]
    async fn cold_restart_resume_continues_persisted_history() {
        let temp = tempfile::tempdir().expect("tempdir");

        // First host lifetime: create the session and run one turn.
        let session_id = {
            let (service, adapter) = build_service(temp.path()).await;
            let session = Session::new();
            let session_id = session.id().clone();
            materialize(&service, &adapter, session).await;
            run_prompt(&adapter, &session_id, "first turn before restart").await;
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
            user_texts(&resume_source)
                .iter()
                .any(|text| text.contains("first turn before restart")),
            "authoritative session must carry pre-restart history"
        );

        materialize(&service, &adapter, resume_source).await;
        run_prompt(&adapter, &session_id, "second turn after restart").await;

        let final_session = service
            .load_authoritative_session(&session_id)
            .await
            .expect("authoritative load after resumed turn")
            .expect("session should still exist");
        let texts = user_texts(&final_session);
        assert!(
            texts
                .iter()
                .any(|t| t.contains("first turn before restart")),
            "history from before the restart must survive: {texts:?}"
        );
        assert!(
            texts
                .iter()
                .any(|t| t.contains("second turn after restart")),
            "the post-restart turn must be recorded: {texts:?}"
        );
    }
}

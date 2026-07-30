//! Cold-restart resume contract.
//!
//! A host process that dies without archiving must be able to resume its
//! sessions after restart: rebuild the runtime authority over the same durable
//! stores, materialize the persisted session, and continue turns with history
//! intact. The first full-session persist after resume must be accepted by the
//! append-only save guard — the resume projection is not allowed to diverge
//! from the persisted session-store transcript.
//!
//! SAME-PROCESS CAVEAT: most "host lifetimes" below are reconstructed inside
//! one OS process, and "cold stop" means dropping the service/adapter handles
//! — a graceful teardown whose Drop/shutdown paths may settle state a killed
//! host never would. Process-global state also survives those "restarts":
//! the validated transcript-graph decode memo, the slim-materialization
//! substitution memo, and the byte-bound digest-accumulator memo (all honor
//! the `MEERKAT_DISABLE_GRAPH_DECODE_MEMO` kill switch) can serve host 2
//! proofs that host 1 minted in the same process. A defect confined to the
//! true cold-start decode path (full graph validation, revision-body digest
//! checks, accumulator reseeding) can therefore pass these tests while
//! failing a real restart. The base contract is separately proven across two
//! child processes by
//! `cold_restart_resume_continues_persisted_history_across_processes`: one
//! process writes and exits, then a fresh process reopens, reads, and
//! continues the session with all process memos absent.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

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
    use meerkat_core::service::SessionServiceControlExt;
    use meerkat_runtime::completion::CompletionOutcome;
    use meerkat_runtime::{Input, MeerkatMachine, PromptInput};
    use tokio::time::Duration;

    async fn build_service(
        root: &std::path::Path,
    ) -> (
        Arc<PersistentSessionService<FactoryAgentBuilder>>,
        Arc<MeerkatMachine>,
    ) {
        build_service_with_backend(root, meerkat_store::RealmBackend::Sqlite).await
    }

    async fn build_service_with_backend(
        root: &std::path::Path,
        backend: meerkat_store::RealmBackend,
    ) -> (
        Arc<PersistentSessionService<FactoryAgentBuilder>>,
        Arc<MeerkatMachine>,
    ) {
        let (_manifest, persistence) = meerkat::open_realm_persistence_in(
            root,
            "restart-realm",
            Some(backend),
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
        create_request_with_prompt("cold restart resume contract")
    }

    fn create_request_with_prompt(system_prompt: &str) -> CreateSessionRequest {
        CreateSessionRequest {
            injected_context: Vec::new(),
            model: "gpt-5.4".to_string(),
            prompt: meerkat_core::ContentInput::Text(String::new()),
            system_prompt: meerkat::SystemPromptOverride::Set(system_prompt.to_string()),
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
        materialize_with_prompt(service, adapter, session, "cold restart resume contract").await;
    }

    async fn materialize_with_prompt(
        service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
        adapter: &Arc<MeerkatMachine>,
        session: Session,
        system_prompt: &str,
    ) {
        let service_for_executor = Arc::clone(service);
        let adapter_for_executor = Arc::clone(adapter);
        Box::pin(materialize_session(
            service,
            adapter,
            session,
            create_request_with_prompt(system_prompt),
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

    /// Upstream report (0.7.14/0.7.21): every cold restart of a runtime-backed
    /// session lost the transcript when the host supplied another explicit
    /// per-request system message on resume.
    ///
    /// During the first lifetime the runtime records system context (comms
    /// roster, host context) beside the persisted ordered transcript. The
    /// The resume build used to blind-replace `messages[0]` with the
    /// re-assembled base prompt. The correct behavior is to preserve every
    /// existing row and append the explicitly supplied System message at the
    /// resume boundary.
    #[tokio::test]
    async fn cold_restart_resume_survives_runtime_context_appended_prompt() {
        let temp = tempfile::tempdir().expect("tempdir");

        // First host lifetime: create with an explicit prompt, run a turn,
        // append runtime system context, run another turn so the appended
        // prompt is committed to both stores.
        let (session_id, system_rows_before_restart) = {
            let (service, adapter) = build_service(temp.path()).await;
            let session = Session::new();
            let session_id = session.id().clone();
            materialize(&service, &adapter, session).await;
            run_prompt(&adapter, &session_id, "first turn before restart").await;

            let append = meerkat_core::AppendSystemContextRequest {
                content: meerkat_core::CoreRenderable::text("peer roster: lead-1, w-1"),
                source: Some("comms:roster".to_string()),
                idempotency_key: Some("comms:roster:v1".to_string()),
            };
            service
                .append_system_context(&session_id, append)
                .await
                .expect("append ordinary System message");
            run_prompt(&adapter, &session_id, "turn after context append").await;
            let persisted = service
                .load_authoritative_session(&session_id)
                .await
                .expect("load pre-restart authority")
                .expect("pre-restart session exists");
            let systems = persisted
                .messages()
                .iter()
                .filter_map(|message| match message {
                    meerkat_core::Message::System(system) => Some(system.content.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            (session_id, systems)
        };

        // Second host lifetime: resume with the SAME explicit prompt. Exact
        // duplicates are legal ordered messages; no content-based deduplication
        // may reinterpret caller intent.
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
        let projected = final_session.messages_for_model_boundary();
        let system_rows_after_resume = final_session
            .messages()
            .iter()
            .filter_map(|message| match message {
                meerkat_core::Message::System(system) => Some(system.content.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            &system_rows_after_resume[..system_rows_before_restart.len()],
            system_rows_before_restart.as_slice(),
            "resume must preserve all preexisting System rows in exact order"
        );
        assert_eq!(
            system_rows_after_resume.len(),
            system_rows_before_restart.len() + 1,
            "the explicitly supplied resume prompt must append exactly one System row"
        );
        assert_eq!(
            system_rows_after_resume.last(),
            system_rows_before_restart.first(),
            "the repeated explicit prompt must remain an exact duplicate ordered System row"
        );
        assert!(
            projected.iter().any(|message| {
                matches!(
                    message,
                    meerkat_core::Message::System(system)
                        if system.content.contains("peer roster: lead-1, w-1")
                )
            }),
            "interleaved ordinary System history must survive the resume: {projected:?}"
        );
    }

    /// Companion to the ordered-System regression: an explicit changed prompt
    /// on resume is a new ordinary System message at that exact boundary.
    #[tokio::test]
    async fn cold_restart_resume_survives_changed_explicit_prompt() {
        let temp = tempfile::tempdir().expect("tempdir");

        let session_id = {
            let (service, adapter) = build_service(temp.path()).await;
            let session = Session::new();
            let session_id = session.id().clone();
            materialize(&service, &adapter, session).await;
            run_prompt(&adapter, &session_id, "first turn before restart").await;

            let append = meerkat_core::AppendSystemContextRequest {
                content: meerkat_core::CoreRenderable::text("peer roster: lead-1, w-1"),
                source: Some("comms:roster".to_string()),
                idempotency_key: Some("comms:roster:v1".to_string()),
            };
            service
                .append_system_context(&session_id, append)
                .await
                .expect("append ordinary System message");
            run_prompt(&adapter, &session_id, "turn after context append").await;
            session_id
        };

        // Second host lifetime: the caller explicitly supplies changed
        // instructions while resuming. Existing history remains byte-for-byte
        // and the new System message is appended at the resume boundary.
        let (service, adapter) = build_service(temp.path()).await;
        let resume_source = service
            .load_authoritative_session(&session_id)
            .await
            .expect("authoritative load after restart")
            .expect("session should survive restart");
        let system_row_count_before_resume = resume_source
            .messages()
            .iter()
            .filter(|message| matches!(message, meerkat_core::Message::System(_)))
            .count();
        let mut request = create_request();
        request.system_prompt =
            meerkat::SystemPromptOverride::Set("cold restart resume contract v2".to_string());
        let service_for_executor = Arc::clone(&service);
        let adapter_for_executor = Arc::clone(&adapter);
        Box::pin(materialize_session(
            &service,
            &adapter,
            resume_source,
            request,
            move |session_id| {
                default_persistent_executor(service_for_executor, adapter_for_executor, session_id)
            },
        ))
        .await
        .expect("materialize session with changed prompt");
        let after_materialize = service
            .load_authoritative_session(&session_id)
            .await
            .expect("authoritative load after changed-config resume")
            .expect("session should survive changed-config resume");
        assert_eq!(
            after_materialize
                .messages()
                .iter()
                .filter(|message| matches!(message, meerkat_core::Message::System(_)))
                .count(),
            system_row_count_before_resume + 1,
            "explicit changed resume instructions must append exactly one System row"
        );
        assert!(matches!(
            after_materialize.messages().last(),
            Some(meerkat_core::Message::System(system))
                if system.content.contains("cold restart resume contract v2")
        ));
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
        let system_prompt = final_session
            .messages()
            .iter()
            .rev()
            .find_map(|message| match message {
                meerkat_core::Message::System(system) => Some(system.content.clone()),
                _ => None,
            })
            .expect("expected ordered system message");
        assert!(
            system_prompt.contains("cold restart resume contract v2"),
            "the explicit resume System append must be applied: {system_prompt}"
        );
        let projected = final_session.messages_for_model_boundary();
        assert!(
            projected.iter().any(|message| {
                matches!(
                    message,
                    meerkat_core::Message::System(system)
                        if system.content.contains("peer roster: lead-1, w-1")
                )
            }),
            "interleaved ordinary System history must survive as a distinct durable message: {projected:?}"
        );
    }

    /// Seed an accepted-but-unconsumed queued input directly into the runtime
    /// store, exactly as the driver persists one that was admitted but never
    /// consumed before the host died (mirrors the REST persist_input_state
    /// seeding shape). Returns the seeded input id.
    async fn seed_accepted_unconsumed_input(
        runtime_store: &Arc<dyn meerkat_runtime::RuntimeStore>,
        session_id: &meerkat::SessionId,
        prompt: &str,
    ) -> meerkat_core::InputId {
        let persisted_input = Input::Prompt(PromptInput::new(
            prompt,
            Some(
                meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                    execution_kind: Some(
                        meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
                    ),
                    ..Default::default()
                },
            ),
        ));
        let input_id = persisted_input.header().id.clone();
        let policy = meerkat_runtime::DefaultPolicyTable::resolve(&persisted_input, true);
        let mut input_state = meerkat_runtime::InputState::new_accepted(input_id.clone());
        input_state.runtime_semantics = Some(
            meerkat_runtime::ingress_types::RuntimeInputSemantics::try_from_generated_admission(
                &persisted_input,
                true,
            )
            .expect("generated admission semantics"),
        );
        input_state.policy = Some(meerkat_runtime::PolicySnapshot {
            version: policy.policy_version,
            decision: policy,
        });
        input_state.persisted_input = Some(persisted_input);
        let mut seed = meerkat_runtime::input_state::InputStateSeed::new_accepted();
        seed.recovery_lane = Some(meerkat_core::types::HandlingMode::Queue);
        let bundle = meerkat_runtime::input_state::StoredInputState {
            state: input_state,
            seed,
        };
        let record = {
            let mut driver = meerkat_runtime::EphemeralRuntimeDriver::new(
                meerkat_runtime::identifiers::LogicalRuntimeId::new(format!(
                    "cold-restart-persistence-record-{session_id}"
                )),
            );
            driver
                .recover_input_state_persistence_record(bundle)
                .expect("test input-state seed should pass generated recovery authority")
        };
        runtime_store
            .persist_input_state(
                &meerkat_runtime::identifiers::LogicalRuntimeId::for_session(session_id),
                &record,
            )
            .await
            .expect("persist accepted-but-unconsumed input state");
        input_id
    }

    /// JSONL-realm parity with the sqlite cold-restart contract: the realm's
    /// sqlite runtime companion (`runtime.sqlite3`) must recover queued-input
    /// bookkeeping, run-boundary receipts, and the runtime session snapshot
    /// across a simulated host restart, and resumed turns must continue the
    /// persisted history.
    #[cfg(feature = "jsonl-store")]
    #[tokio::test]
    async fn cold_restart_resume_jsonl_realm_recovers_runtime_authority() {
        let temp = tempfile::tempdir().expect("tempdir");

        // First host lifetime: create the session, run one turn, and accept
        // one more input that is never consumed before the host dies.
        let (session_id, queued_input_id) = {
            let (service, adapter) =
                build_service_with_backend(temp.path(), meerkat_store::RealmBackend::Jsonl).await;
            let session = Session::new();
            let session_id = session.id().clone();
            materialize(&service, &adapter, session).await;
            run_prompt(&adapter, &session_id, "first turn before restart").await;
            let queued_input_id = seed_accepted_unconsumed_input(
                &service.runtime_store(),
                &session_id,
                "queued prompt accepted before the crash",
            )
            .await;
            // Cold stop: the host dies without archiving or retiring anything,
            // leaving the accepted input unconsumed.
            (session_id, queued_input_id)
        };

        let realm_paths = meerkat_store::realm_paths_in(temp.path(), "restart-realm");
        assert!(
            realm_paths.runtime_sqlite_path.exists(),
            "jsonl realms must persist runtime authority in the sqlite runtime companion"
        );

        // Second host lifetime: fresh service + runtime authority over the
        // same durable stores.
        let (service, adapter) =
            build_service_with_backend(temp.path(), meerkat_store::RealmBackend::Jsonl).await;
        let runtime_store = service.runtime_store();
        let runtime_id = meerkat_runtime::identifiers::LogicalRuntimeId::for_session(&session_id);
        let recovered_inputs = runtime_store
            .load_input_states_strict(&runtime_id)
            .await
            .expect("input states must load from the reopened runtime companion");
        assert!(
            !recovered_inputs.is_empty(),
            "the first lifetime's run inputs must survive the jsonl-realm restart"
        );
        let (recovered_run_id, recovered_sequence) = recovered_inputs
            .iter()
            .find_map(|stored| {
                stored
                    .seed
                    .last_run_id
                    .clone()
                    .zip(stored.seed.last_boundary_sequence)
            })
            .expect("restart must recover a consumed input with its run-boundary bookkeeping");
        let receipt = runtime_store
            .load_boundary_receipt(&runtime_id, &recovered_run_id, recovered_sequence)
            .await
            .expect("boundary receipt must load from the reopened runtime companion")
            .expect("the first lifetime's run-boundary receipt must survive the restart");
        assert_eq!(
            receipt.run_id, recovered_run_id,
            "recovered receipt must belong to the consumed input's run"
        );

        // The input accepted (but never consumed) before the crash must
        // survive recovery: still tracked, still unconsumed, and still
        // carrying its persisted prompt payload for replay.
        let queued = recovered_inputs
            .iter()
            .find(|stored| stored.state.input_id == queued_input_id)
            .expect("the accepted-but-unconsumed input must survive the jsonl-realm restart");
        assert_eq!(
            queued.seed.phase,
            meerkat_runtime::InputLifecycleState::Queued,
            "the accepted input must recover in the recovery authority's re-processable queued phase, not a fabricated terminal"
        );
        assert!(
            queued.seed.last_run_id.is_none() && queued.seed.terminal_outcome.is_none(),
            "the queued input must not recover with consumed-run bookkeeping"
        );
        match queued
            .state
            .persisted_input
            .as_ref()
            .expect("the queued input must recover its persisted payload")
        {
            Input::Prompt(prompt) => assert_eq!(
                prompt.content.text_content(),
                "queued prompt accepted before the crash",
                "the queued input's prompt payload must survive the restart"
            ),
            other => panic!("expected the queued prompt input to survive, got {other:?}"),
        }

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

    #[tokio::test]
    async fn cold_restart_resume_continues_persisted_history() {
        assert_cold_restart_continues_persisted_history().await;
    }

    /// True process-boundary probe. Process 1 creates the durable realm,
    /// completes a turn, records only the opaque session id, and exits.
    /// Process 2 starts afterward, reopens the realm, loads that session, and
    /// completes another turn. The parent never opens the stores, so no
    /// process-global memo or retained SQLite handle can bridge the boundary.
    #[test]
    fn cold_restart_resume_continues_persisted_history_across_processes() {
        const CHILD_TEST: &str =
            "tests::cold_restart_resume_continues_persisted_history_process_child";
        let executable = std::env::current_exe().expect("test binary path");
        let temp = tempfile::tempdir().expect("cross-process realm tempdir");

        for phase in ["write", "read"] {
            let output = std::process::Command::new(&executable)
                .arg("--exact")
                .arg(CHILD_TEST)
                .arg("--nocapture")
                .env("MEERKAT_COLD_RESTART_PHASE", phase)
                .env("MEERKAT_COLD_RESTART_ROOT", temp.path())
                .env("MEERKAT_DISABLE_GRAPH_DECODE_MEMO", "1")
                .output()
                .unwrap_or_else(|error| panic!("spawn cold-restart {phase} process: {error}"));
            assert!(
                output.status.success(),
                "cold-restart {phase} process failed: {}\nstdout:\n{}\nstderr:\n{}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
    }

    /// Exact-filtered child entrypoint for the process-boundary parent above.
    /// Without an explicit phase this is a no-op during the ordinary test
    /// binary run.
    #[tokio::test]
    async fn cold_restart_resume_continues_persisted_history_process_child() {
        let Some(phase) = std::env::var_os("MEERKAT_COLD_RESTART_PHASE") else {
            return;
        };
        let root = std::env::var_os("MEERKAT_COLD_RESTART_ROOT")
            .map(std::path::PathBuf::from)
            .expect("process child requires MEERKAT_COLD_RESTART_ROOT");
        let session_id_path = root.join("cross-process-session-id");

        match phase.to_str().expect("utf8 cold-restart phase") {
            "write" => {
                let (service, adapter) = build_service(&root).await;
                let session = Session::new();
                let session_id = session.id().clone();
                materialize(&service, &adapter, session).await;
                run_prompt(&adapter, &session_id, "first turn before restart").await;
                let persisted = service
                    .load_authoritative_session(&session_id)
                    .await
                    .expect("authoritative load before writer exit")
                    .expect("writer session should be durable");
                assert!(
                    user_texts(&persisted)
                        .iter()
                        .any(|text| text.contains("first turn before restart")),
                    "writer must prove the pre-restart turn durable before exiting"
                );
                std::fs::write(&session_id_path, format!("{session_id}\n"))
                    .expect("persist opaque session id for the reader process");
            }
            "read" => {
                let raw_session_id =
                    std::fs::read_to_string(&session_id_path).expect("read writer session id");
                let session_id = meerkat::SessionId::parse(raw_session_id.trim())
                    .expect("writer persisted a valid session id");
                let (service, adapter) = build_service(&root).await;
                let resume_source = service
                    .load_authoritative_session(&session_id)
                    .await
                    .expect("authoritative load in reader process")
                    .expect("session should survive writer process exit");
                assert!(
                    user_texts(&resume_source)
                        .iter()
                        .any(|text| text.contains("first turn before restart")),
                    "reader must recover the writer process's history"
                );

                materialize(&service, &adapter, resume_source).await;
                run_prompt(&adapter, &session_id, "second turn after restart").await;
                let final_session = service
                    .load_authoritative_session(&session_id)
                    .await
                    .expect("authoritative load after reader turn")
                    .expect("session should remain durable in reader process");
                let texts = user_texts(&final_session);
                assert!(
                    texts
                        .iter()
                        .any(|text| text.contains("first turn before restart")),
                    "writer history must remain after reader continuation: {texts:?}"
                );
                assert!(
                    texts
                        .iter()
                        .any(|text| text.contains("second turn after restart")),
                    "reader turn must be durable: {texts:?}"
                );
            }
            other => panic!("unknown MEERKAT_COLD_RESTART_PHASE {other:?}"),
        }
    }

    async fn assert_cold_restart_continues_persisted_history() {
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

    /// Chained resume-time system-prompt changes with NO turn in between must
    /// remain ordinary exact appends and must not strand the session.
    #[tokio::test]
    async fn cold_restart_resume_survives_chained_promptless_refresh_boots() {
        let temp = tempfile::tempdir().expect("tempdir");

        // Lifetime 0: create the session and run turns, then cold-stop.
        let session_id = {
            let (service, adapter) = build_service(temp.path()).await;
            let session = Session::new();
            let session_id = session.id().clone();
            materialize_with_prompt(&service, &adapter, session, "member prompt roster v1").await;
            run_prompt(&adapter, &session_id, "the codeword is birch seventeen").await;
            session_id
        };

        // Lifetimes 1..=3: each boot resumes with a changed system prompt
        // (appending one ordered System) and dies before any turn runs.
        for roster in ["roster v2", "roster v3", "roster v4"] {
            let (service, adapter) = build_service(temp.path()).await;
            let resume_source = service
                .load_authoritative_session(&session_id)
                .await
                .expect("authoritative load on refresh-only boot")
                .expect("session should survive refresh-only boots");
            materialize_with_prompt(
                &service,
                &adapter,
                resume_source,
                &format!("member prompt {roster}"),
            )
            .await;
            // Cold stop: no turn, no archive.
        }

        // Final lifetime: another drifted resume, and this time a turn runs.
        // The turn's completed-boundary commit must accept the transcript as
        // a continuation of the persisted head.
        let (service, adapter) = build_service(temp.path()).await;
        let resume_source = service
            .load_authoritative_session(&session_id)
            .await
            .expect("authoritative load on final boot")
            .expect("session should survive to the final boot");
        materialize_with_prompt(&service, &adapter, resume_source, "member prompt roster v5").await;
        run_prompt(&adapter, &session_id, "what was the codeword?").await;

        let final_session = service
            .load_authoritative_session(&session_id)
            .await
            .expect("authoritative load after resumed turn")
            .expect("session should still exist after the resumed turn");
        let texts = user_texts(&final_session);
        assert!(
            texts
                .iter()
                .any(|t| t.contains("the codeword is birch seventeen")),
            "history from before the refresh-only boots must survive: {texts:?}"
        );
        assert!(
            texts.iter().any(|t| t.contains("what was the codeword?")),
            "the resumed turn must be persisted: {texts:?}"
        );
        let ordered_prompts = final_session
            .messages()
            .iter()
            .filter_map(|message| match message {
                meerkat_core::Message::System(system)
                    if system.content.starts_with("member prompt roster") =>
                {
                    Some(system.content.as_str())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            ordered_prompts,
            vec![
                "member prompt roster v1",
                "member prompt roster v2",
                "member prompt roster v3",
                "member prompt roster v4",
                "member prompt roster v5",
            ],
            "each genuine prompt change must append once in order"
        );
    }
}

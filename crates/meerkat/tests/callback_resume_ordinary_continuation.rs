//! Callback resume through an ordinary continuation on a runtime-backed
//! service (#1772).
//!
//! A turn suspends as `CallbackPending`, the host stages the callback result
//! with `stage_tool_results`, and the session is resumed with an ordinary
//! `Input::Continuation` instead of a new content turn. Session turn admission
//! resolves that continuation to `RunPending`, so the agent continues from the
//! staged tool results without a new user message. The resumed run must
//! commit through the runtime's `RunCompleted` authority like any other run.

#![allow(clippy::expect_used, clippy::unwrap_used)]

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use meerkat::surface::{
        build_runtime_backed_service, default_persistent_executor, materialize_session,
    };
    use meerkat::{
        AgentFactory, Config, CreateSessionRequest, FactoryAgentBuilder, PersistentSessionService,
        Session, SessionServiceControlExt as _,
    };
    use meerkat_client::types::LlmStream;
    use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
    use meerkat_core::service::StageToolResultsRequest;
    use meerkat_core::{SessionBuildOptions, ToolResult};
    use meerkat_runtime::completion::CompletionOutcome;
    use meerkat_runtime::{ContinuationInput, Input, MeerkatMachine, PromptInput};
    use tokio::time::Duration;

    const CALLBACK_TOOL: &str = "external_callback";
    const CALLBACK_TOOL_USE_ID: &str = "toolu_ordinary_continuation_callback";
    const CALLBACK_RESULT: &str = "callback result staged by the host";
    const RESUMED_REPLY: &str = "resumed after the staged callback result";

    fn usage(request: &LlmRequest) -> LlmEvent {
        LlmEvent::UsageUpdate {
            usage: meerkat_core::TurnUsage::host_declared(
                meerkat_core::Provider::OpenAI,
                &request.model,
                meerkat_core::Usage::default(),
            ),
        }
    }

    /// First model call asks for the host callback; every later call answers
    /// with text. Records whether the resumed request carried the staged
    /// callback result.
    struct CallbackThenReplyClient {
        calls: AtomicUsize,
        resumed_with_staged_result: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl LlmClient for CallbackThenReplyClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
            let events = if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                vec![
                    LlmEvent::ToolCallComplete {
                        id: CALLBACK_TOOL_USE_ID.to_string(),
                        name: CALLBACK_TOOL.to_string(),
                        args: serde_json::json!({ "key": "ordinary-continuation" }),
                        meta: None,
                    },
                    usage(request),
                    LlmEvent::Done {
                        outcome: LlmDoneOutcome::Success {
                            stop_reason: meerkat_core::StopReason::ToolUse,
                        },
                    },
                ]
            } else {
                let carries_staged_result = request.messages.iter().any(|message| {
                    matches!(
                        message,
                        meerkat_core::Message::ToolResults { results, .. }
                            if results.iter().any(|result| {
                                result.tool_use_id == CALLBACK_TOOL_USE_ID
                                    && result.text_content().contains(CALLBACK_RESULT)
                            })
                    )
                });
                if carries_staged_result {
                    self.resumed_with_staged_result
                        .fetch_add(1, Ordering::SeqCst);
                }
                vec![
                    LlmEvent::TextDelta {
                        delta: RESUMED_REPLY.to_string(),
                        meta: None,
                    },
                    usage(request),
                    LlmEvent::Done {
                        outcome: LlmDoneOutcome::Success {
                            stop_reason: meerkat_core::StopReason::EndTurn,
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

    struct CallbackPendingDispatcher;

    #[async_trait::async_trait]
    impl meerkat_core::AgentToolDispatcher for CallbackPendingDispatcher {
        fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> {
            Arc::from([Arc::new(meerkat_core::ToolDef::new(
                CALLBACK_TOOL,
                "external callback test tool",
                serde_json::json!({
                    "type": "object",
                    "properties": { "key": { "type": "string" } }
                }),
            ))])
        }

        async fn dispatch(
            &self,
            call: meerkat_core::ToolCallView<'_>,
        ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
            let args = serde_json::from_str(call.args.get())
                .unwrap_or_else(|_| serde_json::json!({ "raw": call.args.get() }));
            Err(meerkat_core::ToolError::callback_pending(call.name, args))
        }
    }

    fn create_request() -> CreateSessionRequest {
        let build = SessionBuildOptions {
            external_tools: Some(Arc::new(CallbackPendingDispatcher)),
            override_builtins: meerkat_core::ToolCategoryOverride::Disable,
            ..SessionBuildOptions::default()
        };
        CreateSessionRequest {
            injected_context: Vec::new(),
            model: "gpt-5.4".to_string(),
            prompt: meerkat_core::ContentInput::Text(String::new()),
            system_prompt: meerkat::SystemPromptOverride::Set(
                "callback resume through an ordinary continuation".to_string(),
            ),
            max_tokens: None,
            event_tx: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(build),
            labels: None,
        }
    }

    async fn build_service(
        root: &std::path::Path,
        client: Arc<dyn LlmClient>,
    ) -> (
        Arc<PersistentSessionService<FactoryAgentBuilder>>,
        Arc<MeerkatMachine>,
    ) {
        let (_manifest, persistence) = meerkat::open_realm_persistence_in(
            root,
            "callback-resume-realm",
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

    async fn accept_and_wait(
        adapter: &Arc<MeerkatMachine>,
        session_id: &meerkat::SessionId,
        input: Input,
    ) -> Result<CompletionOutcome, String> {
        let (_outcome, handle) = adapter
            .accept_input_with_completion(session_id, input)
            .await
            .map_err(|err| format!("accept_input failed: {err}"))?;
        let handle = handle.ok_or_else(|| "missing completion handle".to_string())?;
        match tokio::time::timeout(Duration::from_secs(10), handle.wait()).await {
            Err(_) => Err("input did not complete".to_string()),
            Ok(Err(err)) => Err(format!("completion waiter failed: {err:?}")),
            Ok(Ok(outcome)) => Ok(outcome),
        }
    }

    #[tokio::test]
    async fn staged_callback_result_resumes_through_an_ordinary_continuation() {
        let temp = tempfile::tempdir().expect("tempdir");
        let client = Arc::new(CallbackThenReplyClient {
            calls: AtomicUsize::new(0),
            resumed_with_staged_result: AtomicUsize::new(0),
        });
        let (service, adapter) =
            build_service(temp.path(), Arc::clone(&client) as Arc<dyn LlmClient>).await;

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
        .expect("materialize session");

        let suspended = accept_and_wait(
            &adapter,
            &session_id,
            Input::Prompt(PromptInput::new("ask the host", None)),
        )
        .await;
        assert!(
            matches!(
                &suspended,
                Ok(CompletionOutcome::CallbackPending { tool_use_id, tool_name, .. })
                    if tool_use_id == CALLBACK_TOOL_USE_ID && tool_name == CALLBACK_TOOL
            ),
            "first turn must suspend on the host callback: {suspended:?}"
        );

        service
            .stage_tool_results(
                &session_id,
                StageToolResultsRequest {
                    results: vec![ToolResult::new(
                        CALLBACK_TOOL_USE_ID.to_string(),
                        CALLBACK_RESULT.to_string(),
                        false,
                    )],
                },
            )
            .await
            .expect("stage the callback result");

        let resumed = accept_and_wait(
            &adapter,
            &session_id,
            Input::Continuation(ContinuationInput::detached_background_op_completed()),
        )
        .await;
        assert!(
            matches!(&resumed, Ok(CompletionOutcome::Completed(_))),
            "ordinary continuation must resume and complete the staged callback run: {resumed:?}"
        );
        assert_eq!(
            client.resumed_with_staged_result.load(Ordering::SeqCst),
            1,
            "the resumed model call must carry the staged callback result"
        );

        let authoritative = service
            .load_authoritative_session(&session_id)
            .await
            .expect("load authoritative session")
            .expect("session exists");
        let last_assistant = authoritative
            .messages()
            .iter()
            .rev()
            .find_map(|message| match message {
                meerkat_core::Message::BlockAssistant(assistant) => Some(assistant.to_string()),
                _ => None,
            })
            .expect("resumed run must commit an assistant reply");
        assert!(
            last_assistant.contains(RESUMED_REPLY),
            "resumed reply must be committed: {last_assistant}"
        );
    }
}

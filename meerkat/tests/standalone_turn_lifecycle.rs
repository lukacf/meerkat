#![cfg(all(feature = "gemini", not(target_arch = "wasm32")))]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, stream};
use meerkat::{AgentFactory, FactoryAgentBuilder, SessionService};
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::service::{
    CreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy, StartTurnRequest,
};
use meerkat_core::{Config, Message, Provider, StopReason, SystemPromptOverride, TurnUsage, Usage};
use meerkat_session::EphemeralSessionService;
use tokio::sync::Notify;

enum Reply {
    Answer,
    Tool,
    Fail,
    Block,
}

struct Client {
    replies: Mutex<VecDeque<Reply>>,
    calls: AtomicUsize,
    started: Notify,
}

#[async_trait]
impl LlmClient for Client {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let ordinal = self.calls.fetch_add(1, Ordering::SeqCst);
        self.started.notify_one();
        match self.replies.lock().unwrap().pop_front().unwrap() {
            Reply::Block => Box::pin(stream::pending()),
            Reply::Fail => Box::pin(stream::iter([Err(LlmError::AuthenticationFailed {
                message: "synthetic non-retryable failure".into(),
            })])),
            Reply::Tool => Box::pin(stream::iter([
                Ok(LlmEvent::ToolCallComplete {
                    id: format!("synthetic-call-{ordinal}"),
                    name: "synthetic_noop".into(),
                    args: serde_json::json!({}),
                    meta: None,
                }),
                Ok(LlmEvent::UsageUpdate {
                    usage: TurnUsage::host_declared(
                        Provider::Gemini,
                        &request.model,
                        Usage {
                            input_tokens: 1,
                            output_tokens: 1,
                            ..Usage::default()
                        },
                    ),
                }),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success {
                        stop_reason: StopReason::ToolUse,
                    },
                }),
            ])),
            Reply::Answer => Box::pin(stream::iter([
                Ok(LlmEvent::TextDelta {
                    delta: "SYNTHETIC_OK".into(),
                    meta: None,
                }),
                Ok(LlmEvent::UsageUpdate {
                    usage: TurnUsage::host_declared(
                        Provider::Gemini,
                        &request.model,
                        Usage {
                            input_tokens: 1,
                            output_tokens: 1,
                            ..Usage::default()
                        },
                    ),
                }),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success {
                        stop_reason: StopReason::EndTurn,
                    },
                }),
            ])),
        }
    }

    fn provider(&self) -> Provider {
        Provider::Gemini
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

struct NoopTool;

#[async_trait]
impl meerkat_core::AgentToolDispatcher for NoopTool {
    fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
        Arc::from([Arc::new(meerkat_core::types::ToolDef::new(
            "synthetic_noop",
            "Deterministic no-effect test tool",
            serde_json::json!({"type": "object"}),
        ))])
    }

    async fn dispatch(
        &self,
        call: meerkat_core::types::ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::ToolError> {
        if call.name != "synthetic_noop" {
            return Err(meerkat_core::ToolError::not_found(call.name));
        }
        Ok(
            meerkat_core::types::ToolResult::new(call.id.to_owned(), "done".to_owned(), false)
                .into(),
        )
    }
}

fn fixture(
    replies: impl IntoIterator<Item = Reply>,
) -> (
    tempfile::TempDir,
    Arc<Client>,
    EphemeralSessionService<FactoryAgentBuilder>,
) {
    fixture_with_config(replies, Config::default())
}

fn fixture_with_config(
    replies: impl IntoIterator<Item = Reply>,
    config: Config,
) -> (
    tempfile::TempDir,
    Arc<Client>,
    EphemeralSessionService<FactoryAgentBuilder>,
) {
    let directory = tempfile::Builder::new()
        .prefix(".standalone-lifecycle-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .unwrap();
    let client = Arc::new(Client {
        replies: Mutex::new(replies.into_iter().collect()),
        calls: AtomicUsize::new(0),
        started: Notify::new(),
    });
    let factory = AgentFactory::new(directory.path().join("sessions"))
        .project_root(directory.path())
        .memory(false);
    let mut builder = FactoryAgentBuilder::new(factory, config);
    builder.default_llm_client = Some(client.clone());
    builder.default_tool_dispatcher = Some(Arc::new(NoopTool));
    let service = EphemeralSessionService::new(builder, 4);
    (directory, client, service)
}

fn create(initial_turn: InitialTurnPolicy) -> CreateSessionRequest {
    CreateSessionRequest {
        model: "gemini-3.5-flash".into(),
        prompt: "Return the synthetic marker.".into(),
        injected_context: Vec::new(),
        system_prompt: SystemPromptOverride::Set("Return the requested synthetic marker.".into()),
        max_tokens: Some(64),
        event_tx: None,
        initial_turn,
        deferred_prompt_policy: DeferredPromptPolicy::Discard,
        build: None,
        labels: None,
    }
}

fn turn() -> StartTurnRequest {
    StartTurnRequest {
        prompt: "Return the synthetic marker.".into(),
        injected_context: Vec::new(),
        system_prompt: None,
        event_tx: None,
        runtime: Default::default(),
    }
}

#[tokio::test]
async fn standalone_successive_turns_preserve_one_session_for_both_initial_policies() {
    tokio::time::timeout(Duration::from_secs(15), async {
        for initial in [InitialTurnPolicy::RunImmediately, InitialTurnPolicy::Defer] {
            let (_directory, client, service) =
                fixture([Reply::Answer, Reply::Answer, Reply::Answer]);
            let created = service.create_session(create(initial)).await.unwrap();
            let session_id = created.session_id.clone();
            let already_run = if initial == InitialTurnPolicy::RunImmediately {
                assert_eq!(created.text, "SYNTHETIC_OK");
                1
            } else {
                assert_eq!(client.calls.load(Ordering::SeqCst), 0);
                0
            };
            for _ in already_run..3 {
                let result = service.start_turn(&session_id, turn()).await.unwrap();
                assert_eq!(result.session_id, session_id);
                assert_eq!(result.text, "SYNTHETIC_OK");
            }
            assert_eq!(client.calls.load(Ordering::SeqCst), 3);
            service.archive(&session_id).await.unwrap();
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn standalone_provider_failure_then_two_successful_turns() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let (_directory, client, service) = fixture([Reply::Fail, Reply::Answer, Reply::Answer]);
        let session_id = service
            .create_session(create(InitialTurnPolicy::Defer))
            .await
            .unwrap()
            .session_id;
        let failure = service.start_turn(&session_id, turn()).await.unwrap_err();
        assert!(
            failure
                .to_string()
                .contains("synthetic non-retryable failure")
        );
        assert_eq!(client.calls.load(Ordering::SeqCst), 1);
        for _ in 0..2 {
            assert_eq!(
                service.start_turn(&session_id, turn()).await.unwrap().text,
                "SYNTHETIC_OK"
            );
        }
        assert_eq!(client.calls.load(Ordering::SeqCst), 3);
        service.archive(&session_id).await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn standalone_turn_budget_failure_allows_a_new_turn_with_a_fresh_budget() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let mut config = Config::default();
        config.limits.max_turn_duration = Some(Duration::from_millis(200));
        let (_directory, client, service) =
            fixture_with_config([Reply::Block, Reply::Answer], config);
        let session_id = service
            .create_session(create(InitialTurnPolicy::Defer))
            .await
            .unwrap()
            .session_id;
        let result = service.start_turn(&session_id, turn()).await;
        assert!(
            matches!(
                result,
                Err(meerkat_core::service::SessionError::Agent(
                    meerkat_core::AgentError::TerminalFailure {
                        outcome: meerkat_core::turn_execution_authority::TurnTerminalOutcome::TimeBudgetExceeded,
                        cause_kind: meerkat_core::turn_execution_authority::TurnTerminalCauseKind::TimeBudgetExceeded,
                        ..
                    }
                ))
            ),
            "{result:?}"
        );
        assert_eq!(client.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            service.start_turn(&session_id, turn()).await.unwrap().text,
            "SYNTHETIC_OK"
        );
        assert_eq!(client.calls.load(Ordering::SeqCst), 2);
        service.archive(&session_id).await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn standalone_turn_limit_terminal_allows_the_next_turn() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let limit = meerkat_core::config::DEFAULT_MAX_TURNS as usize;
        let (_directory, client, service) = fixture(
            std::iter::repeat_with(|| Reply::Tool)
                .take(limit)
                .chain([Reply::Answer]),
        );
        let session_id = service
            .create_session(create(InitialTurnPolicy::Defer))
            .await
            .unwrap()
            .session_id;
        let result = service.start_turn(&session_id, turn()).await;
        assert!(
            matches!(
                result,
                Err(meerkat_core::service::SessionError::Agent(
                    meerkat_core::AgentError::TerminalFailure {
                        cause_kind: meerkat_core::TurnTerminalCauseKind::TurnLimitReached,
                        ..
                    }
                ))
            ),
            "{result:?}"
        );
        assert_eq!(client.calls.load(Ordering::SeqCst), limit);
        assert_eq!(
            service.start_turn(&session_id, turn()).await.unwrap().text,
            "SYNTHETIC_OK"
        );
        assert_eq!(client.calls.load(Ordering::SeqCst), limit + 1);
        service.archive(&session_id).await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
#[allow(clippy::panic)]
async fn standalone_spent_lifetime_budgets_remain_budget_terminals_not_running_guard_errors() {
    tokio::time::timeout(Duration::from_secs(15), async {
        for (reply, token_cap, tool_cap) in
            [(Reply::Answer, Some(1), None), (Reply::Tool, None, Some(1))]
        {
            let mut config = Config::default();
            config.limits.budget = token_cap;
            let (_directory, client, service) = fixture_with_config([reply], config);
            let mut request = create(InitialTurnPolicy::Defer);
            request.build =
                tool_cap.map(
                    |max_tool_calls| meerkat_core::service::SessionBuildOptions {
                        budget_limits: Some(
                            meerkat_core::BudgetLimits::unlimited()
                                .with_max_tool_calls(max_tool_calls),
                        ),
                        ..Default::default()
                    },
                );
            let session_id = service.create_session(request).await.unwrap().session_id;
            for _ in 0..3 {
                match service.start_turn(&session_id, turn()).await {
                    Ok(result) => assert_eq!(
                        result.terminal_cause_kind,
                        Some(meerkat_core::TurnTerminalCauseKind::BudgetExhausted)
                    ),
                    Err(meerkat_core::service::SessionError::Agent(
                        meerkat_core::AgentError::TerminalFailure {
                            outcome,
                            cause_kind,
                            ..
                        },
                    )) => {
                        assert_eq!(outcome, meerkat_core::TurnTerminalOutcome::BudgetExhausted);
                        assert_eq!(
                            cause_kind,
                            meerkat_core::TurnTerminalCauseKind::BudgetExhausted
                        );
                    }
                    other => panic!("expected the configured budget terminal, got {other:?}"),
                }
                assert_eq!(client.calls.load(Ordering::SeqCst), 1);
            }
            service.archive(&session_id).await.unwrap();
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn standalone_service_interrupt_drops_run_then_allows_two_successful_turns() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let (_directory, client, service) = fixture([Reply::Block, Reply::Answer, Reply::Answer]);
        let session_id = service
            .create_session(create(InitialTurnPolicy::Defer))
            .await
            .unwrap()
            .session_id;
        let interrupt = async {
            client.started.notified().await;
            service.interrupt(&session_id).await.unwrap();
        };
        let (result, ()) = tokio::join!(service.start_turn(&session_id, turn()), interrupt);
        assert!(matches!(
            result,
            Err(meerkat_core::service::SessionError::Agent(
                meerkat_core::AgentError::Cancelled
            ))
        ));
        assert_eq!(client.calls.load(Ordering::SeqCst), 1);
        for _ in 0..2 {
            assert_eq!(
                service.start_turn(&session_id, turn()).await.unwrap().text,
                "SYNTHETIC_OK"
            );
        }
        assert_eq!(client.calls.load(Ordering::SeqCst), 3);
        service.archive(&session_id).await.unwrap();
    })
    .await
    .unwrap();
}

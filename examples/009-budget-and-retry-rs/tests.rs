#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use async_trait::async_trait;
use meerkat_core::error::LlmFailureReason;
use meerkat_core::{
    AgentLlmClient, AssistantBlock, LlmStreamResult, Message, Provider, StopReason, ToolDef,
    TurnUsage, Usage,
};

struct ScriptedClient {
    fail: bool,
}

#[async_trait]
impl AgentLlmClient for ScriptedClient {
    async fn stream_response(
        &self,
        _messages: &[Message],
        _tools: &[Arc<ToolDef>],
        _max_tokens: u32,
        _temperature: Option<f32>,
        _provider_params: Option<&meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
    ) -> Result<LlmStreamResult, AgentError> {
        if self.fail {
            return Err(AgentError::llm(
                "anthropic",
                LlmFailureReason::AuthError,
                "synthetic rejection",
            ));
        }
        Ok(LlmStreamResult::new(
            vec![AssistantBlock::Text {
                text: "measured answer".into(),
                meta: None,
            }],
            StopReason::EndTurn,
            TurnUsage::host_declared(
                Provider::Anthropic,
                self.model(),
                Usage {
                    input_tokens: 1000,
                    output_tokens: 100,
                    ..Usage::default()
                },
            )
            .into_inner(),
        ))
    }
    fn provider(&self) -> Provider {
        Provider::Anthropic
    }
    fn model(&self) -> &str {
        "claude-sonnet-4-6"
    }
}

async fn run_script(limits: BudgetLimits, fail: bool) -> Result<RunResult, AgentError> {
    let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let store = Arc::new(JsonlStore::new(root.path().join("sessions")));
    store.init().await.unwrap();
    let mut agent = AgentBuilder::new()
        .model("claude-sonnet-4-6")
        .budget(limits)
        .build(
            Arc::new(ScriptedClient { fail }),
            Arc::new(EmptyToolDispatcher),
            Arc::new(StoreAdapter::new(store)),
        )
        .await
        .unwrap();
    agent.run("synthetic prompt".into()).await
}

#[tokio::test]
async fn reports_normal_completion_and_measured_budget_exhaustion() {
    let completed = run_script(BudgetLimits::unlimited(), false).await;
    assert!(
        describe_run(completed)
            .unwrap()
            .starts_with("Run completed without budget exhaustion")
    );
    let exhausted = run_script(BudgetLimits::unlimited().with_max_tokens(100), false)
        .await
        .unwrap();
    assert_eq!(
        exhausted.terminal_cause_kind,
        Some(TurnTerminalCauseKind::BudgetExhausted)
    );
    assert!(
        exhausted.usage.total_tokens() > 100,
        "an in-flight call can overshoot"
    );
    assert!(
        describe_run(Ok(exhausted))
            .unwrap()
            .starts_with("Budget exhausted")
    );
}

#[tokio::test]
async fn propagates_permanent_provider_failures() {
    let error = describe_run(run_script(BudgetLimits::unlimited(), true).await).unwrap_err();
    assert!(!matches!(error, AgentError::TimeBudgetExceeded { .. }));
}

#[tokio::test]
async fn recognizes_only_typed_time_budget_failures() {
    let result = run_script(
        BudgetLimits::unlimited().with_max_turn_duration(Duration::ZERO),
        false,
    )
    .await;
    assert!(result.is_err());
    assert!(
        describe_run(result)
            .unwrap()
            .starts_with("Time budget exhausted")
    );
    assert!(describe_run(Err(AgentError::InternalError("not a budget".into()))).is_err());
}

#[test]
fn preview_is_unicode_safe_and_ellipsis_is_truthful() {
    for text in ["", "short", "café", "🦦"] {
        assert_eq!(preview(text, 100), text);
    }
    let exact = "x".repeat(100);
    assert_eq!(preview(&exact, 100), exact);
    for tail in ["é", "🦦", "a"] {
        let text = format!("{}{tail}z", "a".repeat(99));
        let result = preview(&text, 100);
        assert_eq!(result, format!("{}{tail}…", "a".repeat(99)));
        assert_eq!(result.chars().count(), 101);
    }
    assert_eq!(preview("abc", 0), "…");
}

#[tokio::test]
async fn guarded_stores_are_cleaned_up_on_success_and_error() {
    let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    for fail in [false, true] {
        let mut paths = Vec::new();
        let result: Result<(), AgentError> = async {
            let (first, first_path) = scoped_store_dir(root.path()).unwrap();
            let (second, second_path) = scoped_store_dir(root.path()).unwrap();
            paths.extend([first.path().to_path_buf(), second.path().to_path_buf()]);
            for path in [first_path, second_path] {
                let store = Arc::new(JsonlStore::new(path));
                store.init().await.unwrap();
                let mut agent = AgentBuilder::new()
                    .model("claude-sonnet-4-6")
                    .build(
                        Arc::new(ScriptedClient { fail }),
                        Arc::new(EmptyToolDispatcher),
                        Arc::new(StoreAdapter::new(store)),
                    )
                    .await
                    .unwrap();
                agent.run("synthetic prompt".into()).await?;
            }
            Ok(())
        }
        .await;
        assert_eq!(result.is_err(), fail);
        assert!(paths.iter().all(|path| !path.exists()));
    }
}

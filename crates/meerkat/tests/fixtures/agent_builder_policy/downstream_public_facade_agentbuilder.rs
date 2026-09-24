use std::sync::Arc;

use async_trait::async_trait;
use futures::executor::block_on;
use meerkat_core::types::{AssistantBlock, StopReason, Usage};
use meerkat_core::{
    AgentError, AgentLlmClient, AgentSessionStore, AgentToolDispatcher, LlmStreamResult, Message,
    Session, ToolCallView, ToolDef, ToolDispatchOutcome, ToolError,
};

struct NoopClient;

#[async_trait]
impl AgentLlmClient for NoopClient {
    async fn stream_response(
        &self,
        _messages: &[Message],
        _tools: &[Arc<ToolDef>],
        _max_tokens: u32,
        _temperature: Option<f32>,
        _provider_params: Option<&meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
    ) -> Result<LlmStreamResult, AgentError> {
        Ok(LlmStreamResult::new(
            vec![AssistantBlock::Text {
                text: "done".to_string(),
                meta: None,
            }],
            StopReason::EndTurn,
            Usage::default(),
        ))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Other
    }

    fn model(&self) -> &str {
        "mock-model"
    }
}

struct NoopTools;

#[async_trait]
impl AgentToolDispatcher for NoopTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::from([])
    }

    async fn dispatch(&self, _call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        Err(ToolError::execution_failed(
            "public facade smoke does not dispatch tools",
        ))
    }
}

struct NoopStore;

#[async_trait]
impl AgentSessionStore for NoopStore {
    async fn save(&self, _session: &Session) -> Result<(), AgentError> {
        Ok(())
    }

    async fn load(&self, _id: &str) -> Result<Option<Session>, AgentError> {
        Ok(None)
    }
}

fn main() {
    let result = block_on(async {
        meerkat::AgentBuilder::new()
            .build(
                Arc::new(NoopClient),
                Arc::new(NoopTools),
                Arc::new(NoopStore),
            )
            .await
    });
    match result {
        Ok(_) => println!("public facade AgentBuilder constructed an agent"),
        Err(error) => panic!("public facade AgentBuilder failed: {error}"),
    }
}

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use async_trait::async_trait;
use futures::stream;
use meerkat::{
    AgentBuilder, AgentFactory, AgentToolDispatcher, LlmDoneOutcome, LlmEvent, LlmRequest,
    ToolDef, ToolError, ToolResult,
};
use meerkat_core::ToolCallView;
use meerkat_client::LlmClient;
use meerkat_store::MemoryStore;
use meerkat_tools::schema_for;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

#[allow(dead_code)]
#[derive(Debug, JsonSchema, Deserialize)]
struct EchoInput {
    message: String,
}

struct MockLlmClient {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl LlmClient for MockLlmClient {
    fn stream<'a>(
        &'a self,
        _request: &'a LlmRequest,
    ) -> std::pin::Pin<
        Box<dyn futures::Stream<Item = Result<LlmEvent, meerkat_client::LlmError>> + Send + 'a>,
    > {
        let call_index = self.calls.fetch_add(1, Ordering::SeqCst);
        if call_index == 0 {
            Box::pin(stream::iter(vec![
                Ok(LlmEvent::ToolCallComplete {
                    id: "tc-1".into(),
                    name: "echo".into(),
                    args: json!({"message": "hello"}),
                    meta: None,
                }),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success {
                        stop_reason: meerkat::StopReason::ToolUse,
                    },
                }),
            ]))
        } else {
            Box::pin(stream::iter(vec![
                Ok(LlmEvent::TextDelta {
                    delta: "done".to_string(),
                    meta: None,
                }),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success {
                        stop_reason: meerkat::StopReason::EndTurn,
                    },
                }),
            ]))
        }
    }

    fn provider(&self) -> &'static str {
        "mock"
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

struct RecordingDispatcher {
    called: Arc<AtomicBool>,
}

#[async_trait]
impl AgentToolDispatcher for RecordingDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![Arc::new(ToolDef {
            name: "echo".into(),
            description: "Echo tool".into(),
            input_schema: schema_for::<EchoInput>(),
        })]
        .into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
        self.called.store(true, Ordering::SeqCst);
        let args: EchoInput = call
            .parse_args()
            .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
        let payload = json!({ "message": args.message, "tool": call.name });
        Ok(ToolResult::new(
            call.id.to_string(),
            payload.to_string(),
            false,
        ))
    }
}

#[tokio::test]
async fn test_sdk_agentfactory_tool_dispatch() {
    let calls = Arc::new(AtomicUsize::new(0));
    let dispatcher_called = Arc::new(AtomicBool::new(false));

    let factory = AgentFactory::new(".rkat/sessions");
    let llm_client = Arc::new(MockLlmClient { calls });
    let llm_adapter = Arc::new(factory.build_llm_adapter(llm_client, "mock-model").await);

    let store = Arc::new(MemoryStore::new());
    let store_adapter = Arc::new(factory.build_store_adapter(store).await);

    let tools: Arc<dyn AgentToolDispatcher> = Arc::new(RecordingDispatcher {
        called: dispatcher_called.clone(),
    });

    let mut agent = AgentBuilder::new()
        .model("mock-model")
        .max_tokens_per_turn(64)
        .build(llm_adapter, tools, store_adapter)
        .await;

    let result = agent
        .run("Use the echo tool.".to_string())
        .await
        .expect("agent run");

    assert!(
        dispatcher_called.load(Ordering::SeqCst),
        "tool should be dispatched"
    );
    assert!(
        result.text.contains("done"),
        "agent should complete after tool call"
    );
}

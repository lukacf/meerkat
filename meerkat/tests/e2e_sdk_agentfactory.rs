#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use async_trait::async_trait;
use futures::stream;
use meerkat::{
    AgentBuilder, AgentFactory, AgentToolDispatcher, LlmEvent, LlmRequest, ToolDef, ToolError,
};
use meerkat_client::LlmClient;
use meerkat_store::MemoryStore;
use serde_json::{Value, json};

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
                    id: "tc-1".to_string(),
                    name: "echo".to_string(),
                    args: json!({"message": "hello"}),
                    thought_signature: None,
                }),
                Ok(LlmEvent::Done {
                    stop_reason: meerkat::StopReason::ToolUse,
                }),
            ]))
        } else {
            Box::pin(stream::iter(vec![
                Ok(LlmEvent::TextDelta {
                    delta: "done".to_string(),
                }),
                Ok(LlmEvent::Done {
                    stop_reason: meerkat::StopReason::EndTurn,
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
    fn tools(&self) -> Vec<ToolDef> {
        vec![ToolDef {
            name: "echo".to_string(),
            description: "Echo tool".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string" }
                },
                "required": ["message"]
            }),
        }]
    }

    async fn dispatch(&self, name: &str, args: &Value) -> Result<Value, ToolError> {
        self.called.store(true, Ordering::SeqCst);
        let message = args.get("message").cloned().unwrap_or_else(|| json!(""));
        Ok(json!({ "message": message, "tool": name }))
    }
}

#[tokio::test]
async fn e2e_sdk_agentfactory_tool_dispatch() {
    let calls = Arc::new(AtomicUsize::new(0));
    let dispatcher_called = Arc::new(AtomicBool::new(false));

    let factory = AgentFactory::new(".rkat/sessions");
    let llm_client = Arc::new(MockLlmClient { calls });
    let llm_adapter = Arc::new(factory.build_llm_adapter(llm_client, "mock-model"));

    let store = Arc::new(MemoryStore::new());
    let store_adapter = Arc::new(factory.build_store_adapter(store));

    let tools: Arc<dyn AgentToolDispatcher> = Arc::new(RecordingDispatcher {
        called: dispatcher_called.clone(),
    });

    let mut agent = AgentBuilder::new()
        .model("mock-model")
        .max_tokens_per_turn(64)
        .build(llm_adapter, tools, store_adapter);

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

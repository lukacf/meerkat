//! SDK tests for structured output extraction.
//!
//! These tests verify the structured output API works correctly from the SDK perspective.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use futures::stream;
use meerkat::{
    AgentBuilder, AgentFactory, AgentToolDispatcher, LlmDoneOutcome, LlmEvent, LlmRequest,
    OutputSchema, ToolDef, ToolError, ToolResult,
};
use meerkat_client::LlmClient;
use meerkat_core::ToolCallView;
use serde::Deserialize;
use serde_json::{Value, json};
#[path = "support/test_session_store.rs"]
mod test_session_store;
use test_session_store::TestSessionStore;

/// Mock LLM client that returns valid JSON on extraction turn
struct MockLlmClientWithStructuredOutput {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl LlmClient for MockLlmClientWithStructuredOutput {
    fn stream<'a>(
        &'a self,
        _request: &'a LlmRequest,
    ) -> std::pin::Pin<
        Box<dyn futures::Stream<Item = Result<LlmEvent, meerkat_client::LlmError>> + Send + 'a>,
    > {
        let call_index = self.calls.fetch_add(1, Ordering::SeqCst);
        if call_index == 0 {
            // First call - agent work complete
            Box::pin(stream::iter(vec![
                Ok(LlmEvent::TextDelta {
                    delta: "Let me provide the person info.".to_string(),
                    meta: None,
                }),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success {
                        stop_reason: meerkat::StopReason::EndTurn,
                    },
                }),
            ]))
        } else {
            // Second call - extraction turn with valid JSON
            Box::pin(stream::iter(vec![
                Ok(LlmEvent::TextDelta {
                    delta: r#"{"name": "Alice", "age": 30}"#.to_string(),
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

/// Mock LLM client that returns invalid JSON then valid JSON (tests retry)
struct MockLlmClientWithRetry {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl LlmClient for MockLlmClientWithRetry {
    fn stream<'a>(
        &'a self,
        _request: &'a LlmRequest,
    ) -> std::pin::Pin<
        Box<dyn futures::Stream<Item = Result<LlmEvent, meerkat_client::LlmError>> + Send + 'a>,
    > {
        let call_index = self.calls.fetch_add(1, Ordering::SeqCst);
        match call_index {
            0 => {
                // First call - agent work complete
                Box::pin(stream::iter(vec![
                    Ok(LlmEvent::TextDelta {
                        delta: "Processing...".to_string(),
                        meta: None,
                    }),
                    Ok(LlmEvent::Done {
                        outcome: LlmDoneOutcome::Success {
                            stop_reason: meerkat::StopReason::EndTurn,
                        },
                    }),
                ]))
            }
            1 => {
                // Second call - extraction with invalid JSON (missing required field)
                Box::pin(stream::iter(vec![
                    Ok(LlmEvent::TextDelta {
                        delta: r#"{"name": "Bob"}"#.to_string(), // missing 'age'
                        meta: None,
                    }),
                    Ok(LlmEvent::Done {
                        outcome: LlmDoneOutcome::Success {
                            stop_reason: meerkat::StopReason::EndTurn,
                        },
                    }),
                ]))
            }
            _ => {
                // Third call - extraction with valid JSON
                Box::pin(stream::iter(vec![
                    Ok(LlmEvent::TextDelta {
                        delta: r#"{"name": "Bob", "age": 25}"#.to_string(),
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
    }

    fn provider(&self) -> &'static str {
        "mock"
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

struct EmptyDispatcher;

#[async_trait]
impl AgentToolDispatcher for EmptyDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::new([])
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
        Err(ToolError::NotFound {
            name: call.name.to_string(),
        })
    }
}

fn person_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "age": {"type": "integer"}
        },
        "required": ["name", "age"]
    })
}

#[derive(Debug, Deserialize, PartialEq)]
struct Person {
    name: String,
    age: u32,
}

#[tokio::test]
async fn sdk_structured_output_extraction_succeeds() -> Result<(), Box<dyn std::error::Error>> {
    let calls = Arc::new(AtomicUsize::new(0));

    let factory = AgentFactory::new(".rkat/sessions");
    let llm_client = Arc::new(MockLlmClientWithStructuredOutput {
        calls: calls.clone(),
    });
    let llm_adapter = Arc::new(factory.build_llm_adapter(llm_client, "mock-model").await);

    let store = Arc::new(TestSessionStore::new());
    let store_adapter = Arc::new(factory.build_store_adapter(store).await);

    let tools: Arc<dyn AgentToolDispatcher> = Arc::new(EmptyDispatcher);

    let mut agent = AgentBuilder::new()
        .model("mock-model")
        .max_tokens_per_turn(64)
        .output_schema(OutputSchema::new(person_schema())?)
        .build(llm_adapter, tools, store_adapter)
        .await;

    let result = agent
        .run("Tell me about a person.".to_string())
        .await
        .expect("agent run should succeed");

    // Verify structured output is present
    assert!(
        result.structured_output.is_some(),
        "structured_output should be present"
    );

    // Verify it can be deserialized to the expected type
    let person: Person =
        serde_json::from_value(result.structured_output.unwrap()).expect("deserialize Person");

    assert_eq!(person.name, "Alice");
    assert_eq!(person.age, 30);

    // Verify extraction turn was called (2 calls total: agentic + extraction)
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn sdk_structured_output_retry_on_invalid_json() -> Result<(), Box<dyn std::error::Error>> {
    let calls = Arc::new(AtomicUsize::new(0));

    let factory = AgentFactory::new(".rkat/sessions");
    let llm_client = Arc::new(MockLlmClientWithRetry {
        calls: calls.clone(),
    });
    let llm_adapter = Arc::new(factory.build_llm_adapter(llm_client, "mock-model").await);

    let store = Arc::new(TestSessionStore::new());
    let store_adapter = Arc::new(factory.build_store_adapter(store).await);

    let tools: Arc<dyn AgentToolDispatcher> = Arc::new(EmptyDispatcher);

    let mut agent = AgentBuilder::new()
        .model("mock-model")
        .max_tokens_per_turn(64)
        .output_schema(OutputSchema::new(person_schema())?)
        .structured_output_retries(2) // Allow retries
        .build(llm_adapter, tools, store_adapter)
        .await;

    let result = agent
        .run("Tell me about a person.".to_string())
        .await
        .expect("agent run should succeed after retry");

    // Verify structured output is present
    let person: Person =
        serde_json::from_value(result.structured_output.unwrap()).expect("deserialize Person");

    assert_eq!(person.name, "Bob");
    assert_eq!(person.age, 25);

    // Verify retry occurred (3 calls: agentic + invalid extraction + valid extraction)
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    Ok(())
}

#[tokio::test]
async fn sdk_no_structured_output_without_schema() {
    let calls = Arc::new(AtomicUsize::new(0));

    let factory = AgentFactory::new(".rkat/sessions");
    let llm_client = Arc::new(MockLlmClientWithStructuredOutput { calls });
    let llm_adapter = Arc::new(factory.build_llm_adapter(llm_client, "mock-model").await);

    let store = Arc::new(TestSessionStore::new());
    let store_adapter = Arc::new(factory.build_store_adapter(store).await);

    let tools: Arc<dyn AgentToolDispatcher> = Arc::new(EmptyDispatcher);

    // NO output_schema configured
    let mut agent = AgentBuilder::new()
        .model("mock-model")
        .max_tokens_per_turn(64)
        .build(llm_adapter, tools, store_adapter)
        .await;

    let result = agent
        .run("Hello".to_string())
        .await
        .expect("agent run should succeed");

    // Verify structured_output is NOT present (no extraction turn)
    assert!(
        result.structured_output.is_none(),
        "structured_output should be None without output_schema"
    );
}

#[tokio::test]
async fn sdk_output_schema_builder_pattern() -> Result<(), Box<dyn std::error::Error>> {
    // Test the OutputSchema builder API
    let schema = person_schema();

    let output_schema = OutputSchema::new(schema.clone())?
        .with_name("person")
        .strict();

    assert!(output_schema.strict);
    assert_eq!(output_schema.name, Some("person".to_string()));
    assert_eq!(output_schema.schema.as_value(), &schema);
    Ok(())
}

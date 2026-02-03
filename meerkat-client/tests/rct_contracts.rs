use async_trait::async_trait;
use futures::Stream;
use meerkat_client::error::LlmError;
use meerkat_client::types::{LlmClient, LlmDoneOutcome, LlmEvent, LlmRequest};
use meerkat_client::{LlmClientAdapter, ProviderResolver};
use meerkat_core::{AgentEvent, AgentLlmClient, Provider, StopReason};
use serde_json::json;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::mpsc;

struct MockClient {
    events: Vec<LlmEvent>,
}

#[async_trait]
impl LlmClient for MockClient {
    fn stream<'a>(
        &'a self,
        _request: &'a LlmRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let events = self.events.clone();
        Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
    }

    fn provider(&self) -> &'static str {
        "mock"
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

#[tokio::test]
async fn test_llm_adapter_streaming_contract() -> Result<(), Box<dyn std::error::Error>> {
    let events = vec![
        LlmEvent::TextDelta {
            delta: "hello".to_string(),
        },
        LlmEvent::ToolCallDelta {
            id: "tc1".to_string(),
            name: Some("tool_a".to_string()),
            args_delta: "{\"foo\":".to_string(),
        },
        LlmEvent::ToolCallDelta {
            id: "tc1".to_string(),
            name: None,
            args_delta: "\"bar\"}".to_string(),
        },
        LlmEvent::ToolCallComplete {
            id: "tc2".to_string(),
            name: "tool_b".to_string(),
            args: json!({"baz": true}),
            thought_signature: Some("sig-123".to_string()),
        },
        LlmEvent::Done {
            outcome: LlmDoneOutcome::Success {
                stop_reason: StopReason::EndTurn,
            },
        },
    ];

    let client = Arc::new(MockClient { events });
    let (tx, mut rx) = mpsc::channel(4);
    let adapter = LlmClientAdapter::with_event_channel(client, "model-x".to_string(), tx);

    let result = adapter.stream_response(&[], &[], 128, None, None).await?;

    assert_eq!(result.content(), "hello");
    assert_eq!(result.stop_reason(), StopReason::EndTurn);

    let mut tc1 = None;
    let mut tc2 = None;
    for tc in result.tool_calls() {
        match tc.id.as_str() {
            "tc1" => tc1 = Some(tc),
            "tc2" => tc2 = Some(tc),
            _ => {}
        }
    }

    let tc1 = tc1.ok_or("buffered tool call missing")?;
    assert_eq!(tc1.name, "tool_a");
    assert_eq!(tc1.args, json!({"foo":"bar"}));
    assert!(tc1.thought_signature.is_none());

    let tc2 = tc2.ok_or("complete tool call missing")?;
    assert_eq!(tc2.name, "tool_b");
    assert_eq!(tc2.args, json!({"baz": true}));
    assert_eq!(tc2.thought_signature.as_deref(), Some("sig-123"));

    let event = rx.recv().await.ok_or("text delta event missing")?;
    match event {
        AgentEvent::TextDelta { delta } => assert_eq!(delta, "hello"),
        _ => return Err("unexpected event".into()),
    }

    Ok(())
}

#[test]
fn test_provider_resolution_contract() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("RUN_TEST_RESOLUTION_INNER").is_ok() {
        let key = ProviderResolver::api_key_for(Provider::OpenAI);
        assert_eq!(key.as_deref(), Some("rk-test"));
        return Ok(());
    }

    assert_eq!(
        ProviderResolver::infer_from_model("claude-3"),
        Provider::Anthropic
    );
    assert_eq!(
        ProviderResolver::infer_from_model("gpt-4"),
        Provider::OpenAI
    );
    assert_eq!(
        ProviderResolver::infer_from_model("gemini-1.5"),
        Provider::Gemini
    );
    assert_eq!(
        ProviderResolver::infer_from_model("unknown"),
        Provider::Other
    );

    let status = std::process::Command::new(std::env::current_exe()?)
        .arg("test_provider_resolution_contract")
        .env("RUN_TEST_RESOLUTION_INNER", "1")
        .env("RKAT_OPENAI_API_KEY", "rk-test")
        .env("OPENAI_API_KEY", "native-test")
        .status()?;
    assert!(status.success());

    Ok(())
}

#[test]
fn test_inv_006_provider_inference_uses_resolver() {
    let provider = ProviderResolver::infer_from_model("claude-sonnet-4");
    assert_eq!(provider, Provider::Anthropic);
}

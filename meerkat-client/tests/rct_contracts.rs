use async_trait::async_trait;
use futures::Stream;
use meerkat_client::error::LlmError;
use meerkat_client::types::{LlmClient, LlmDoneOutcome, LlmEvent, LlmRequest};
use meerkat_client::{LlmClientAdapter, ProviderResolver};
use meerkat_core::{
    AgentEvent, AgentLlmClient, AssistantBlock, Provider, ProviderMeta, StopReason,
};
use serde_json::json;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
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
            meta: None,
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
            id: "tc1".to_string(),
            name: "tool_a".to_string(),
            args: json!({"foo":"bar"}),
            meta: None,
        },
        LlmEvent::ToolCallComplete {
            id: "tc2".to_string(),
            name: "tool_b".to_string(),
            args: json!({"baz": true}),
            meta: Some(Box::new(ProviderMeta::Gemini {
                thought_signature: "sig-123".to_string(),
            })),
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

    let mut text = String::new();
    for block in result.blocks() {
        if let AssistantBlock::Text { text: t, .. } = block {
            text.push_str(t);
        }
    }
    assert_eq!(text, "hello");
    assert_eq!(result.stop_reason(), StopReason::EndTurn);

    let mut tc1 = None;
    let mut tc2 = None;
    for block in result.blocks() {
        if let AssistantBlock::ToolUse {
            id,
            name,
            args,
            meta,
        } = block
        {
            match id.as_str() {
                "tc1" => tc1 = Some((name, args, meta)),
                "tc2" => tc2 = Some((name, args, meta)),
                _ => {}
            }
        }
    }

    let tc1 = tc1.ok_or("buffered tool call missing")?;
    assert_eq!(tc1.0, "tool_a");
    let tc1_args: serde_json::Value = serde_json::from_str(tc1.1.get())?;
    assert_eq!(tc1_args, json!({"foo":"bar"}));
    assert!(tc1.2.is_none());

    let tc2 = tc2.ok_or("complete tool call missing")?;
    assert_eq!(tc2.0, "tool_b");
    let tc2_args: serde_json::Value = serde_json::from_str(tc2.1.get())?;
    assert_eq!(tc2_args, json!({"baz": true}));
    match tc2.2.as_deref() {
        Some(ProviderMeta::Gemini { thought_signature }) => {
            assert_eq!(thought_signature, "sig-123");
        }
        other => return Err(format!("unexpected meta: {other:?}").into()),
    }

    let event = rx.recv().await.ok_or("text delta event missing")?;
    match event {
        AgentEvent::TextDelta { delta } => assert_eq!(delta, "hello"),
        _ => return Err("unexpected event".into()),
    }

    Ok(())
}

#[tokio::test]
async fn test_llm_adapter_event_tap_mirrors_text_delta() -> Result<(), Box<dyn std::error::Error>> {
    let events = vec![
        LlmEvent::TextDelta {
            delta: "tap-delta".to_string(),
            meta: None,
        },
        LlmEvent::Done {
            outcome: LlmDoneOutcome::Success {
                stop_reason: StopReason::EndTurn,
            },
        },
    ];

    let client = Arc::new(MockClient { events });
    let tap = meerkat_core::new_event_tap();
    let (tap_tx, mut tap_rx) = mpsc::channel(16);
    {
        let mut guard = tap.lock();
        *guard = Some(meerkat_core::event_tap::EventTapState {
            tx: tap_tx,
            truncated: AtomicBool::new(false),
        });
    }

    let adapter = LlmClientAdapter::new(client, "model-x".to_string()).with_event_tap(tap);
    let _ = adapter.stream_response(&[], &[], 128, None, None).await?;

    let event = tap_rx.recv().await.ok_or("tap event missing")?;
    match event {
        AgentEvent::TextDelta { delta } => assert_eq!(delta, "tap-delta"),
        _ => return Err("unexpected tap event".into()),
    }

    Ok(())
}

#[test]
fn test_provider_resolution_contract() -> Result<(), Box<dyn std::error::Error>> {
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

    let env = std::collections::HashMap::from([
        ("RKAT_OPENAI_API_KEY".to_string(), "rk-test".to_string()),
        ("OPENAI_API_KEY".to_string(), "native-test".to_string()),
    ]);
    let key = ProviderResolver::api_key_for_with_env(Provider::OpenAI, |key| env.get(key).cloned());
    assert_eq!(key.as_deref(), Some("rk-test"));

    Ok(())
}

#[test]
fn test_inv_006_provider_inference_uses_resolver() {
    let provider = ProviderResolver::infer_from_model("claude-sonnet-4");
    assert_eq!(provider, Provider::Anthropic);
}

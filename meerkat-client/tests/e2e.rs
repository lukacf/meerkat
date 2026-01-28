//! E2E tests for LLM clients.
//!
//! Verifies stream normalization and error handling for all providers.

use futures::StreamExt;
use meerkat_client::{
    AnthropicClient, GeminiClient, LlmClient, LlmEvent, LlmRequest, OpenAiClient,
};
use meerkat_core::{Message, StopReason, UserMessage};

fn skip_if_no_anthropic_key() -> Option<String> {
    std::env::var("RKAT_ANTHROPIC_API_KEY")
        .ok()
        .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok())
}

fn skip_if_no_openai_key() -> Option<String> {
    std::env::var("RKAT_OPENAI_API_KEY")
        .ok()
        .or_else(|| std::env::var("OPENAI_API_KEY").ok())
}

fn skip_if_no_gemini_key() -> Option<String> {
    std::env::var("RKAT_GEMINI_API_KEY")
        .ok()
        .or_else(|| std::env::var("GEMINI_API_KEY").ok())
        .or_else(|| std::env::var("GOOGLE_API_KEY").ok())
}

#[tokio::test]
#[ignore = "Requires network access"]
async fn test_anthropic_stream() -> Result<(), Box<dyn std::error::Error>> {
    let Some(api_key) = skip_if_no_anthropic_key() else {
        return Ok(());
    };

    let client = AnthropicClient::new(api_key)?;
    let request = LlmRequest::new(
        "claude-3-haiku-20240307",
        vec![Message::User(UserMessage {
            content: "Say 'Hello'".to_string(),
        })],
    );

    let mut stream = client.stream(&request);
    let mut got_text = false;

    while let Some(result) = stream.next().await {
        match result {
            Ok(LlmEvent::TextDelta { delta }) => {
                if !delta.is_empty() {
                    got_text = true;
                }
            }
            Ok(LlmEvent::Done { .. }) => break,
            Ok(_) => {}
            Err(e) => return Err(format!("Unexpected error: {:?}", e).into()),
        }
    }

    assert!(got_text);
    Ok(())
}

#[tokio::test]
#[ignore = "Requires network access"]
async fn test_anthropic_tool_use() -> Result<(), Box<dyn std::error::Error>> {
    let Some(api_key) = skip_if_no_anthropic_key() else {
        return Ok(());
    };

    let client = AnthropicClient::new(api_key)?;
    let request = LlmRequest::new(
        "claude-3-haiku-20240307",
        vec![Message::User(UserMessage {
            content: "What's the weather in Tokyo?".to_string(),
        })],
    )
    .with_tools(vec![meerkat_core::ToolDef {
        name: "get_weather".to_string(),
        description: "Get weather for a city".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "city": {"type": "string"}
            },
            "required": ["city"]
        }),
    }]);

    let mut stream = client.stream(&request);
    let mut got_tool = false;

    while let Some(result) = stream.next().await {
        match result {
            Ok(LlmEvent::ToolCallDelta { .. }) => {
                got_tool = true;
            }
            Ok(LlmEvent::Done { stop_reason }) => {
                assert_eq!(stop_reason, StopReason::ToolUse);
                break;
            }
            Ok(_) => {}
            Err(e) => return Err(format!("Unexpected error: {:?}", e).into()),
        }
    }

    assert!(got_tool);
    Ok(())
}

#[tokio::test]
#[ignore = "Requires network access"]
async fn test_anthropic_auth_error() -> Result<(), Box<dyn std::error::Error>> {
    let client = AnthropicClient::new("invalid-key".to_string())?;

    let request = LlmRequest::new(
        "claude-3-haiku-20240307",
        vec![Message::User(UserMessage {
            content: "test".to_string(),
        })],
    );

    let mut stream = client.stream(&request);
    let result = stream.next().await;

    let event = result.ok_or("no event")?;
    match event {
        Err(meerkat_client::error::LlmError::AuthenticationFailed { .. }) => {}
        _ => return Err(format!("Expected auth error, got {:?}", event).into()),
    }
    Ok(())
}

#[tokio::test]
#[ignore = "Requires network access"]
async fn test_openai_stream() -> Result<(), Box<dyn std::error::Error>> {
    let Some(api_key) = skip_if_no_openai_key() else {
        return Ok(());
    };

    let client = OpenAiClient::new(api_key);
    let request = LlmRequest::new(
        "gpt-4o-mini",
        vec![Message::User(UserMessage {
            content: "Say 'Hello'".to_string(),
        })],
    );

    let mut stream = client.stream(&request);
    let mut got_text = false;

    while let Some(result) = stream.next().await {
        match result {
            Ok(LlmEvent::TextDelta { delta }) => {
                if !delta.is_empty() {
                    got_text = true;
                }
            }
            Ok(LlmEvent::Done { .. }) => break,
            Ok(_) => {}
            Err(e) => return Err(format!("Unexpected error: {:?}", e).into()),
        }
    }

    assert!(got_text);
    Ok(())
}

#[tokio::test]
#[ignore = "Requires network access"]
async fn test_openai_tool_use() -> Result<(), Box<dyn std::error::Error>> {
    let Some(api_key) = skip_if_no_openai_key() else {
        return Ok(());
    };

    let client = OpenAiClient::new(api_key);
    let request = LlmRequest::new(
        "gpt-4o-mini",
        vec![Message::User(UserMessage {
            content: "What's the weather in Tokyo?".to_string(),
        })],
    )
    .with_tools(vec![meerkat_core::ToolDef {
        name: "get_weather".to_string(),
        description: "Get weather for a city".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "city": {"type": "string"}
            },
            "required": ["city"]
        }),
    }]);

    let mut stream = client.stream(&request);
    let mut got_tool = false;

    while let Some(result) = stream.next().await {
        match result {
            Ok(LlmEvent::ToolCallDelta { .. }) => {
                got_tool = true;
            }
            Ok(LlmEvent::Done { stop_reason }) => {
                assert_eq!(stop_reason, StopReason::ToolUse);
                break;
            }
            Ok(_) => {}
            Err(e) => return Err(format!("Unexpected error: {:?}", e).into()),
        }
    }

    assert!(got_tool);
    Ok(())
}

#[tokio::test]
#[ignore = "Requires network access"]
async fn test_gemini_stream() -> Result<(), Box<dyn std::error::Error>> {
    let Some(api_key) = skip_if_no_gemini_key() else {
        return Ok(());
    };

    let client = GeminiClient::new(api_key);
    let request = LlmRequest::new(
        "gemini-1.5-flash",
        vec![Message::User(UserMessage {
            content: "Say 'Hello'".to_string(),
        })],
    );

    let mut stream = client.stream(&request);
    let mut got_text = false;

    while let Some(result) = stream.next().await {
        match result {
            Ok(LlmEvent::TextDelta { delta }) => {
                if !delta.is_empty() {
                    got_text = true;
                }
            }
            Ok(LlmEvent::Done { .. }) => break,
            Ok(_) => {}
            Err(e) => return Err(format!("Unexpected error: {:?}", e).into()),
        }
    }

    assert!(got_text);
    Ok(())
}

#[tokio::test]
#[ignore = "Requires network access"]
async fn test_gemini_tool_use() -> Result<(), Box<dyn std::error::Error>> {
    let Some(api_key) = skip_if_no_gemini_key() else {
        return Ok(());
    };

    let client = GeminiClient::new(api_key);
    let request = LlmRequest::new(
        "gemini-1.5-flash",
        vec![Message::User(UserMessage {
            content: "What's the weather in Tokyo?".to_string(),
        })],
    )
    .with_tools(vec![meerkat_core::ToolDef {
        name: "get_weather".to_string(),
        description: "Get weather for a city".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "city": {"type": "string"}
            },
            "required": ["city"]
        }),
    }]);

    let mut stream = client.stream(&request);
    let mut got_tool = false;

    while let Some(result) = stream.next().await {
        match result {
            Ok(LlmEvent::ToolCallComplete { .. }) => {
                got_tool = true;
            }
            Ok(LlmEvent::Done { stop_reason }) => {
                assert_eq!(stop_reason, StopReason::ToolUse);
                break;
            }
            Ok(_) => {}
            Err(e) => return Err(format!("Unexpected error: {:?}", e).into()),
        }
    }

    assert!(got_tool);
    Ok(())
}

#[tokio::test]
#[ignore = "Requires network access"]
async fn test_openai_auth_error() -> Result<(), Box<dyn std::error::Error>> {
    let client = OpenAiClient::new("invalid-key".to_string());

    let request = LlmRequest::new(
        "gpt-4o-mini",
        vec![Message::User(UserMessage {
            content: "test".to_string(),
        })],
    );

    let mut stream = client.stream(&request);
    let result = stream.next().await;

    let event = result.ok_or("no event")?;
    match event {
        Err(meerkat_client::error::LlmError::AuthenticationFailed { .. }) => {}
        _ => return Err(format!("Expected auth error, got {:?}", event).into()),
    }
    Ok(())
}

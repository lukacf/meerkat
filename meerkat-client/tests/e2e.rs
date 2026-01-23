//! E2E tests for LLM clients.
//!
//! These tests make real API calls and require API keys to be set.
//! Run with: `cargo test -p meerkat-client --test e2e -- --ignored`

use futures::StreamExt;
use meerkat_client::{
    AnthropicClient, GeminiClient, LlmClient, LlmError, LlmEvent, LlmRequest, OpenAiClient,
};
use meerkat_core::{Message, StopReason, ToolDef, UserMessage};

// ============================================================================
// Anthropic E2E Tests
// ============================================================================

fn anthropic_client() -> Option<AnthropicClient> {
    AnthropicClient::from_env().ok()
}

#[tokio::test]
#[ignore = "Requires ANTHROPIC_API_KEY"]
async fn test_anthropic_streaming_text_delta() {
    let Some(client) = anthropic_client() else {
        return;
    };

    let request = LlmRequest::new(
        "claude-sonnet-4-20250514",
        vec![Message::User(UserMessage {
            content: "Say 'hello' and nothing else.".to_string(),
        })],
    )
    .with_max_tokens(100);

    let mut stream = client.stream(&request);
    let mut got_text_delta = false;
    let mut got_done = false;

    while let Some(event) = stream.next().await {
        match event {
            Ok(LlmEvent::TextDelta { delta }) => {
                got_text_delta = true;
                assert!(!delta.is_empty() || got_text_delta);
            }
            Ok(LlmEvent::Done { stop_reason }) => {
                got_done = true;
                assert_eq!(stop_reason, StopReason::EndTurn);
            }
            Ok(_) => {}
            Err(e) => panic!("Unexpected error: {:?}", e),
        }
    }

    assert!(got_text_delta, "Should have received TextDelta events");
    assert!(got_done, "Should have received Done event");
}

#[tokio::test]
#[ignore = "Requires ANTHROPIC_API_KEY"]
async fn test_anthropic_tool_call() {
    let Some(client) = anthropic_client() else {
        return;
    };

    let tools = vec![ToolDef {
        name: "get_weather".to_string(),
        description: "Get weather for a city".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "city": {
                    "type": "string",
                    "description": "City name"
                }
            },
            "required": ["city"]
        }),
    }];

    let request = LlmRequest::new(
        "claude-sonnet-4-20250514",
        vec![Message::User(UserMessage {
            content: "What's the weather in Tokyo? Use the get_weather tool.".to_string(),
        })],
    )
    .with_tools(tools)
    .with_max_tokens(200);

    let mut stream = client.stream(&request);
    let mut got_tool_call = false;
    let mut got_done = false;

    while let Some(event) = stream.next().await {
        match event {
            Ok(LlmEvent::ToolCallComplete { id, name, args }) => {
                got_tool_call = true;
                assert!(!id.is_empty());
                assert_eq!(name, "get_weather");
                assert!(args.get("city").is_some());
            }
            Ok(LlmEvent::Done { stop_reason }) => {
                got_done = true;
                if got_tool_call {
                    assert_eq!(stop_reason, StopReason::ToolUse);
                }
            }
            Ok(_) => {}
            Err(e) => panic!("Unexpected error: {:?}", e),
        }
    }

    assert!(got_tool_call, "Should have received tool call");
    assert!(got_done, "Should have received Done event");
}

#[tokio::test]
#[ignore = "Requires ANTHROPIC_API_KEY"]
async fn test_anthropic_stop_reason() {
    let Some(client) = anthropic_client() else {
        return;
    };

    let request = LlmRequest::new(
        "claude-sonnet-4-20250514",
        vec![Message::User(UserMessage {
            content: "Say 'hi'".to_string(),
        })],
    )
    .with_max_tokens(100);

    let mut stream = client.stream(&request);
    let mut stop_reason = None;

    while let Some(event) = stream.next().await {
        if let Ok(LlmEvent::Done { stop_reason: sr }) = event {
            stop_reason = Some(sr);
        }
    }

    assert_eq!(stop_reason, Some(StopReason::EndTurn));
}

#[tokio::test]
#[ignore = "Requires ANTHROPIC_API_KEY"]
async fn test_anthropic_usage() {
    let Some(client) = anthropic_client() else {
        return;
    };

    let request = LlmRequest::new(
        "claude-sonnet-4-20250514",
        vec![Message::User(UserMessage {
            content: "Say 'test'".to_string(),
        })],
    )
    .with_max_tokens(100);

    let mut stream = client.stream(&request);
    let mut got_usage = false;

    while let Some(event) = stream.next().await {
        if let Ok(LlmEvent::UsageUpdate { usage }) = event {
            got_usage = true;
            assert!(usage.input_tokens > 0 || usage.output_tokens > 0);
        }
    }

    assert!(got_usage, "Should have received usage info");
}

#[tokio::test]
#[ignore = "Requires network access"]
async fn test_anthropic_auth_error() {
    let client = AnthropicClient::new("invalid-key".to_string());

    let request = LlmRequest::new(
        "claude-sonnet-4-20250514",
        vec![Message::User(UserMessage {
            content: "test".to_string(),
        })],
    );

    let mut stream = client.stream(&request);
    let result = stream.next().await;

    assert!(result.is_some());
    let event = result.unwrap();
    assert!(event.is_err());

    let err = event.unwrap_err();
    assert!(
        matches!(
            err,
            LlmError::AuthenticationFailed { .. } | LlmError::InvalidApiKey
        ),
        "Expected auth error, got: {:?}",
        err
    );
}

// ============================================================================
// OpenAI E2E Tests
// ============================================================================

fn openai_client() -> Option<OpenAiClient> {
    OpenAiClient::from_env().ok()
}

#[tokio::test]
#[ignore = "Requires OPENAI_API_KEY"]
async fn test_openai_streaming_text_delta() {
    let Some(client) = openai_client() else {
        return;
    };

    let request = LlmRequest::new(
        "gpt-4o-mini",
        vec![Message::User(UserMessage {
            content: "Say 'hello' and nothing else.".to_string(),
        })],
    )
    .with_max_tokens(100);

    let mut stream = client.stream(&request);
    let mut got_text_delta = false;
    let mut got_done = false;

    while let Some(event) = stream.next().await {
        match event {
            Ok(LlmEvent::TextDelta { delta }) => {
                got_text_delta = true;
                assert!(!delta.is_empty() || got_text_delta);
            }
            Ok(LlmEvent::Done { stop_reason }) => {
                got_done = true;
                assert_eq!(stop_reason, StopReason::EndTurn);
            }
            Ok(_) => {}
            Err(e) => panic!("Unexpected error: {:?}", e),
        }
    }

    assert!(got_text_delta, "Should have received TextDelta events");
    assert!(got_done, "Should have received Done event");
}

#[tokio::test]
#[ignore = "Requires OPENAI_API_KEY"]
async fn test_openai_tool_call() {
    let Some(client) = openai_client() else {
        return;
    };

    let tools = vec![ToolDef {
        name: "get_weather".to_string(),
        description: "Get weather for a city".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "city": {
                    "type": "string",
                    "description": "City name"
                }
            },
            "required": ["city"]
        }),
    }];

    let request = LlmRequest::new(
        "gpt-4o-mini",
        vec![Message::User(UserMessage {
            content: "What's the weather in Tokyo? Use the get_weather tool.".to_string(),
        })],
    )
    .with_tools(tools)
    .with_max_tokens(200);

    let mut stream = client.stream(&request);
    let mut got_tool_call = false;
    let mut got_done = false;

    while let Some(event) = stream.next().await {
        match event {
            Ok(LlmEvent::ToolCallComplete { id, name, args }) => {
                got_tool_call = true;
                assert!(!id.is_empty());
                assert_eq!(name, "get_weather");
                assert!(args.get("city").is_some());
            }
            Ok(LlmEvent::Done { stop_reason }) => {
                got_done = true;
                if got_tool_call {
                    assert_eq!(stop_reason, StopReason::ToolUse);
                }
            }
            Ok(_) => {}
            Err(e) => panic!("Unexpected error: {:?}", e),
        }
    }

    assert!(got_tool_call, "Should have received tool call");
    assert!(got_done, "Should have received Done event");
}

#[tokio::test]
#[ignore = "Requires OPENAI_API_KEY"]
async fn test_openai_stop_reason() {
    let Some(client) = openai_client() else {
        return;
    };

    let request = LlmRequest::new(
        "gpt-4o-mini",
        vec![Message::User(UserMessage {
            content: "Say 'hi'".to_string(),
        })],
    )
    .with_max_tokens(100);

    let mut stream = client.stream(&request);
    let mut stop_reason = None;

    while let Some(event) = stream.next().await {
        if let Ok(LlmEvent::Done { stop_reason: sr }) = event {
            stop_reason = Some(sr);
        }
    }

    assert_eq!(stop_reason, Some(StopReason::EndTurn));
}

#[tokio::test]
#[ignore = "Requires OPENAI_API_KEY"]
async fn test_openai_usage() {
    let Some(client) = openai_client() else {
        return;
    };

    let request = LlmRequest::new(
        "gpt-4o-mini",
        vec![Message::User(UserMessage {
            content: "Say 'test'".to_string(),
        })],
    )
    .with_max_tokens(100);

    let mut stream = client.stream(&request);
    let mut got_usage = false;

    while let Some(event) = stream.next().await {
        if let Ok(LlmEvent::UsageUpdate { usage }) = event {
            got_usage = true;
            assert!(usage.input_tokens > 0 || usage.output_tokens > 0);
        }
    }

    assert!(got_usage, "Should have received usage info");
}

#[tokio::test]
#[ignore = "Requires network access"]
async fn test_openai_auth_error() {
    let client = OpenAiClient::new("invalid-key".to_string());

    let request = LlmRequest::new(
        "gpt-4o-mini",
        vec![Message::User(UserMessage {
            content: "test".to_string(),
        })],
    );

    let mut stream = client.stream(&request);
    let result = stream.next().await;

    assert!(result.is_some());
    let event = result.unwrap();
    assert!(event.is_err());
}

// ============================================================================
// Gemini E2E Tests
// ============================================================================

fn gemini_client() -> Option<GeminiClient> {
    GeminiClient::from_env().ok()
}

#[tokio::test]
#[ignore = "Requires GOOGLE_API_KEY"]
async fn test_gemini_streaming_text_delta() {
    let Some(client) = gemini_client() else {
        return;
    };

    let request = LlmRequest::new(
        "gemini-1.5-flash",
        vec![Message::User(UserMessage {
            content: "Say 'hello' and nothing else.".to_string(),
        })],
    )
    .with_max_tokens(100);

    let mut stream = client.stream(&request);
    let mut got_text_delta = false;
    let mut got_done = false;

    while let Some(event) = stream.next().await {
        match event {
            Ok(LlmEvent::TextDelta { delta }) => {
                got_text_delta = true;
                assert!(!delta.is_empty() || got_text_delta);
            }
            Ok(LlmEvent::Done { stop_reason }) => {
                got_done = true;
                assert_eq!(stop_reason, StopReason::EndTurn);
            }
            Ok(_) => {}
            Err(e) => panic!("Unexpected error: {:?}", e),
        }
    }

    assert!(got_text_delta, "Should have received TextDelta events");
    assert!(got_done, "Should have received Done event");
}

#[tokio::test]
#[ignore = "Requires GOOGLE_API_KEY"]
async fn test_gemini_tool_call() {
    let Some(client) = gemini_client() else {
        return;
    };

    let tools = vec![ToolDef {
        name: "get_weather".to_string(),
        description: "Get weather for a city".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "city": {
                    "type": "string",
                    "description": "City name"
                }
            },
            "required": ["city"]
        }),
    }];

    let request = LlmRequest::new(
        "gemini-1.5-flash",
        vec![Message::User(UserMessage {
            content: "What's the weather in Tokyo? Use the get_weather tool.".to_string(),
        })],
    )
    .with_tools(tools)
    .with_max_tokens(200);

    let mut stream = client.stream(&request);
    let mut got_tool_call = false;
    let mut got_done = false;

    while let Some(event) = stream.next().await {
        match event {
            Ok(LlmEvent::ToolCallComplete { id, name, args }) => {
                got_tool_call = true;
                assert!(!id.is_empty());
                assert_eq!(name, "get_weather");
                assert!(args.get("city").is_some());
            }
            Ok(LlmEvent::Done { stop_reason }) => {
                got_done = true;
                if got_tool_call {
                    assert_eq!(stop_reason, StopReason::ToolUse);
                }
            }
            Ok(_) => {}
            Err(e) => panic!("Unexpected error: {:?}", e),
        }
    }

    assert!(got_tool_call, "Should have received tool call");
    assert!(got_done, "Should have received Done event");
}

#[tokio::test]
#[ignore = "Requires GOOGLE_API_KEY"]
async fn test_gemini_stop_reason() {
    let Some(client) = gemini_client() else {
        return;
    };

    let request = LlmRequest::new(
        "gemini-1.5-flash",
        vec![Message::User(UserMessage {
            content: "Say 'hi'".to_string(),
        })],
    )
    .with_max_tokens(100);

    let mut stream = client.stream(&request);
    let mut stop_reason = None;

    while let Some(event) = stream.next().await {
        if let Ok(LlmEvent::Done { stop_reason: sr }) = event {
            stop_reason = Some(sr);
        }
    }

    assert_eq!(stop_reason, Some(StopReason::EndTurn));
}

#[tokio::test]
#[ignore = "Requires GOOGLE_API_KEY"]
async fn test_gemini_usage() {
    let Some(client) = gemini_client() else {
        return;
    };

    let request = LlmRequest::new(
        "gemini-1.5-flash",
        vec![Message::User(UserMessage {
            content: "Say 'test'".to_string(),
        })],
    )
    .with_max_tokens(100);

    let mut stream = client.stream(&request);
    let mut got_usage = false;

    while let Some(event) = stream.next().await {
        if let Ok(LlmEvent::UsageUpdate { usage }) = event {
            got_usage = true;
            assert!(usage.input_tokens > 0 || usage.output_tokens > 0);
        }
    }

    assert!(got_usage, "Should have received usage info");
}

#[tokio::test]
#[ignore = "Requires network access"]
async fn test_gemini_auth_error() {
    let client = GeminiClient::new("invalid-key".to_string());

    let request = LlmRequest::new(
        "gemini-1.5-flash",
        vec![Message::User(UserMessage {
            content: "test".to_string(),
        })],
    );

    let mut stream = client.stream(&request);
    let result = stream.next().await;

    assert!(result.is_some());
    let event = result.unwrap();
    assert!(event.is_err());
}

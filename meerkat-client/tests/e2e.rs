//! E2E tests for LLM clients.
//!
//! Verifies stream normalization and error handling for all providers.

use axum::{Router, http::StatusCode, routing::post};
use futures::StreamExt;
use meerkat_client::{
    AnthropicClient, GeminiClient, LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest,
    OpenAiClient,
};
use meerkat_core::{Message, StopReason, UserMessage};
use schemars::JsonSchema;
use serde_json::{Map, Value};
use tokio::net::TcpListener;

struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn spawn_test_server(
    app: Router,
) -> Result<(String, AbortOnDrop), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let base_url = format!("http://{}", addr);

    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    tokio::task::yield_now().await;
    Ok((base_url, AbortOnDrop(handle)))
}

fn schema_for<T: JsonSchema>() -> Value {
    let schema = schemars::schema_for!(T);
    let mut value = serde_json::to_value(&schema).unwrap_or(Value::Null);

    // Some generators omit empty `properties`/`required` for `{}`.
    // Our tool schema contract expects explicit presence of both keys.
    if let Value::Object(ref mut obj) = value {
        if obj.get("type").and_then(Value::as_str) == Some("object") {
            obj.entry("properties".to_string())
                .or_insert_with(|| Value::Object(Map::new()));
            obj.entry("required".to_string())
                .or_insert_with(|| Value::Array(Vec::new()));
        }
    }

    value
}

#[derive(Debug, Clone, JsonSchema)]
#[allow(dead_code)]
struct WeatherArgs {
    city: String,
}

fn skip_if_no_anthropic_key() -> Option<String> {
    if std::env::var("MEERKAT_LIVE_API_TESTS").ok().as_deref() != Some("1") {
        return None;
    }
    std::env::var("RKAT_ANTHROPIC_API_KEY")
        .ok()
        .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok())
}

fn skip_if_no_openai_key() -> Option<String> {
    if std::env::var("MEERKAT_LIVE_API_TESTS").ok().as_deref() != Some("1") {
        return None;
    }
    std::env::var("RKAT_OPENAI_API_KEY")
        .ok()
        .or_else(|| std::env::var("OPENAI_API_KEY").ok())
}

fn skip_if_no_gemini_key() -> Option<String> {
    if std::env::var("MEERKAT_LIVE_API_TESTS").ok().as_deref() != Some("1") {
        return None;
    }
    std::env::var("RKAT_GEMINI_API_KEY")
        .ok()
        .or_else(|| std::env::var("GEMINI_API_KEY").ok())
        .or_else(|| std::env::var("GOOGLE_API_KEY").ok())
}

#[tokio::test]
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
    .with_tools(vec![std::sync::Arc::new(meerkat_core::ToolDef {
        name: "get_weather".to_string(),
        description: "Get weather for a city".to_string(),
        input_schema: schema_for::<WeatherArgs>(),
    })]);

    let mut stream = client.stream(&request);
    let mut got_tool = false;

    while let Some(result) = stream.next().await {
        match result {
            Ok(LlmEvent::ToolCallDelta { .. }) => {
                got_tool = true;
            }
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Success { stop_reason },
            }) => {
                assert_eq!(stop_reason, StopReason::ToolUse);
                break;
            }
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Error { error },
            }) => return Err(format!("Unexpected error: {:?}", error).into()),
            Ok(_) => {}
            Err(e) => return Err(format!("Unexpected error: {:?}", e).into()),
        }
    }

    assert!(got_tool);
    Ok(())
}

#[tokio::test]
async fn test_anthropic_auth_error() -> Result<(), Box<dyn std::error::Error>> {
    let app = Router::new().route(
        "/v1/messages",
        post(|| async { (StatusCode::UNAUTHORIZED, "unauthorized") }),
    );
    let (base_url, _server) = spawn_test_server(app).await?;

    let client = AnthropicClient::new("invalid-key".to_string())?.with_base_url(base_url);

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
        Ok(LlmEvent::Done {
            outcome:
                LlmDoneOutcome::Error {
                    error: LlmError::AuthenticationFailed { .. },
                },
        }) => {}
        other => return Err(format!("Expected auth error, got {:?}", other).into()),
    }
    Ok(())
}

#[tokio::test]
async fn test_anthropic_message_stop_without_newline_yields_done()
-> Result<(), Box<dyn std::error::Error>> {
    const SSE_BODY: &str = concat!(
        "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n",
        "\n",
        "data: {\"type\":\"message_stop\"}"
    );

    let app = Router::new().route(
        "/v1/messages",
        post(|| async { (StatusCode::OK, SSE_BODY) }),
    );
    let (base_url, _server) = spawn_test_server(app).await?;

    let client = AnthropicClient::new("test-key".to_string())?.with_base_url(base_url);
    let request = LlmRequest::new(
        "claude-3-haiku-20240307",
        vec![Message::User(UserMessage {
            content: "test".to_string(),
        })],
    );

    let mut stream = client.stream(&request);
    let mut got_text = false;
    let mut got_done = false;
    let mut stop_reason = None;

    while let Some(result) = stream.next().await {
        match result {
            Ok(LlmEvent::TextDelta { delta }) => {
                if !delta.is_empty() {
                    got_text = true;
                }
            }
            Ok(LlmEvent::Done {
                outcome:
                    LlmDoneOutcome::Success {
                        stop_reason: reason,
                    },
            }) => {
                got_done = true;
                stop_reason = Some(reason);
                break;
            }
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Error { error },
            }) => return Err(format!("Unexpected error: {:?}", error).into()),
            Ok(_) => {}
            Err(e) => return Err(format!("Unexpected error: {:?}", e).into()),
        }
    }

    assert!(got_text);
    assert!(got_done);
    assert_eq!(stop_reason, Some(StopReason::EndTurn));
    Ok(())
}

#[tokio::test]
async fn test_anthropic_message_stop_without_space_prefix_yields_done()
-> Result<(), Box<dyn std::error::Error>> {
    const SSE_BODY: &str = concat!(
        "data:{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n",
        "\n",
        "data:{\"type\":\"message_stop\"}"
    );

    let app = Router::new().route(
        "/v1/messages",
        post(|| async { (StatusCode::OK, SSE_BODY) }),
    );
    let (base_url, _server) = spawn_test_server(app).await?;

    let client = AnthropicClient::new("test-key".to_string())?.with_base_url(base_url);
    let request = LlmRequest::new(
        "claude-3-haiku-20240307",
        vec![Message::User(UserMessage {
            content: "test".to_string(),
        })],
    );

    let mut stream = client.stream(&request);
    let mut got_text = false;
    let mut got_done = false;
    let mut stop_reason = None;

    while let Some(result) = stream.next().await {
        match result {
            Ok(LlmEvent::TextDelta { delta }) => {
                if !delta.is_empty() {
                    got_text = true;
                }
            }
            Ok(LlmEvent::Done {
                outcome:
                    LlmDoneOutcome::Success {
                        stop_reason: reason,
                    },
            }) => {
                got_done = true;
                stop_reason = Some(reason);
                break;
            }
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Error { error },
            }) => return Err(format!("Unexpected error: {:?}", error).into()),
            Ok(_) => {}
            Err(e) => return Err(format!("Unexpected error: {:?}", e).into()),
        }
    }

    assert!(got_text);
    assert!(got_done);
    assert_eq!(stop_reason, Some(StopReason::EndTurn));
    Ok(())
}

#[tokio::test]
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
    .with_tools(vec![std::sync::Arc::new(meerkat_core::ToolDef {
        name: "get_weather".to_string(),
        description: "Get weather for a city".to_string(),
        input_schema: schema_for::<WeatherArgs>(),
    })]);

    let mut stream = client.stream(&request);
    let mut got_tool = false;

    while let Some(result) = stream.next().await {
        match result {
            Ok(LlmEvent::ToolCallDelta { .. }) => {
                got_tool = true;
            }
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Success { stop_reason },
            }) => {
                assert_eq!(stop_reason, StopReason::ToolUse);
                break;
            }
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Error { error },
            }) => return Err(format!("Unexpected error: {:?}", error).into()),
            Ok(_) => {}
            Err(e) => return Err(format!("Unexpected error: {:?}", e).into()),
        }
    }

    assert!(got_tool);
    Ok(())
}

#[tokio::test]
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
    .with_tools(vec![std::sync::Arc::new(meerkat_core::ToolDef {
        name: "get_weather".to_string(),
        description: "Get weather for a city".to_string(),
        input_schema: schema_for::<WeatherArgs>(),
    })]);

    let mut stream = client.stream(&request);
    let mut got_tool = false;

    while let Some(result) = stream.next().await {
        match result {
            Ok(LlmEvent::ToolCallComplete { .. }) => {
                got_tool = true;
            }
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Success { stop_reason },
            }) => {
                assert_eq!(stop_reason, StopReason::ToolUse);
                break;
            }
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Error { error },
            }) => return Err(format!("Unexpected error: {:?}", error).into()),
            Ok(_) => {}
            Err(e) => return Err(format!("Unexpected error: {:?}", e).into()),
        }
    }

    assert!(got_tool);
    Ok(())
}

#[tokio::test]
async fn test_openai_auth_error() -> Result<(), Box<dyn std::error::Error>> {
    let app = Router::new().route(
        "/v1/chat/completions",
        post(|| async { (StatusCode::UNAUTHORIZED, "unauthorized") }),
    );
    let (base_url, _server) = spawn_test_server(app).await?;

    let client = OpenAiClient::new("invalid-key".to_string()).with_base_url(base_url);

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
        Ok(LlmEvent::Done {
            outcome:
                LlmDoneOutcome::Error {
                    error: LlmError::AuthenticationFailed { .. },
                },
        }) => {}
        other => return Err(format!("Expected auth error, got {:?}", other).into()),
    }
    Ok(())
}

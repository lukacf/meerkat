#![cfg(feature = "integration-real-tests")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

//! Live integration tests for LLM clients.
//!
//! Verifies stream normalization and error handling for all providers.

use axum::{Router, http::StatusCode, routing::post};
use futures::StreamExt;
use meerkat_client::{
    AnthropicClient, GeminiClient, LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest,
    OpenAiClient,
};
use meerkat_core::ProviderId;
use meerkat_core::image_generation::{
    GenerateImageExecutionPlan, GenerateImageRequest, ImageContinuityTokenSupport,
    ImageFormatPreference, ImageGenerationIntent, ImageGenerationTargetCapabilities,
    ImageGenerationTargetPreference, ImageOperationId, ImageQualityPreference, ImageSizePreference,
    OpenAiImagesApiEndpoint, OpenAiImagesApiPlan, PromptSource, PromptText, ToolCallId,
};
use meerkat_core::lifecycle::run_primitive::ModelId;
use meerkat_core::{Message, StopReason, UserMessage};
use meerkat_llm_core::{ImageGenerationExecutor, ProviderImageGenerationRequest};
use schemars::JsonSchema;
use serde_json::{Map, Value};
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tokio::net::{TcpListener, TcpStream};

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
    let base_url = format!("http://{addr}");

    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    wait_for_server_ready(addr).await?;
    Ok((base_url, AbortOnDrop(handle)))
}

async fn wait_for_server_ready(addr: SocketAddr) -> Result<(), std::io::Error> {
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        match TcpStream::connect(addr).await {
            Ok(stream) => {
                drop(stream);
                return Ok(());
            }
            Err(err) => {
                if Instant::now() >= deadline {
                    return Err(err);
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    }
}

async fn spawn_test_server_or_skip(
    app: Router,
) -> Result<Option<(String, AbortOnDrop)>, Box<dyn std::error::Error>> {
    match spawn_test_server(app).await {
        Ok(server) => Ok(Some(server)),
        Err(e) => {
            if let Some(io_err) = e.downcast_ref::<std::io::Error>()
                && io_err.kind() == std::io::ErrorKind::PermissionDenied
            {
                return Ok(None);
            }
            Err(e)
        }
    }
}

fn schema_for<T: JsonSchema>() -> Value {
    let schema = schemars::schema_for!(T);
    let mut value = serde_json::to_value(&schema).unwrap_or(Value::Null);

    // Some generators omit empty `properties`/`required` for `{}`.
    // Our tool schema contract expects explicit presence of both keys.
    if let Value::Object(ref mut obj) = value
        && obj.get("type").and_then(Value::as_str) == Some("object")
    {
        obj.entry("properties".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        obj.entry("required".to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
    }

    value
}

#[derive(Debug, Clone, JsonSchema)]
#[allow(dead_code)]
struct WeatherArgs {
    city: String,
}

fn first_env(vars: &[&str]) -> Option<String> {
    for name in vars {
        if let Ok(value) = std::env::var(name) {
            return Some(value);
        }
    }
    None
}

fn anthropic_api_key() -> Option<String> {
    first_env(&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"])
}

fn openai_api_key() -> Option<String> {
    first_env(&["RKAT_OPENAI_API_KEY", "OPENAI_API_KEY"])
}

fn gemini_api_key() -> Option<String> {
    first_env(&["RKAT_GEMINI_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY"])
}

fn tiny_image_request(provider: &str, model: &str) -> GenerateImageRequest {
    GenerateImageRequest::new(
        ImageGenerationIntent::Generate {
            prompt: PromptText::new(
                "Generate a simple original image: one teal square centered on a plain white background.",
            )
            .unwrap(),
            prompt_source: PromptSource::ModelDistilled {
                tool_call_id: ToolCallId::new("live-image-smoke"),
            },
            reference_images: Vec::new(),
        },
        ImageGenerationTargetPreference::Model {
            provider: ProviderId::new(provider),
            model: ModelId::new(model),
        },
        ImageSizePreference::Square1024,
        ImageQualityPreference::Low,
        ImageFormatPreference::Png,
        std::num::NonZeroU32::new(1).unwrap(),
    )
    .unwrap()
}

fn image_capabilities() -> ImageGenerationTargetCapabilities {
    ImageGenerationTargetCapabilities {
        hosted_image_generation_tool: false,
        native_image_output: true,
        custom_tools: false,
        image_search_grounding: false,
        image_continuity_tokens: ImageContinuityTokenSupport::Unsupported,
    }
}

#[tokio::test]
#[ignore = "lane:e2e-live"]
async fn e2e_anthropic_stream() -> Result<(), Box<dyn std::error::Error>> {
    let Some(api_key) = anthropic_api_key() else {
        eprintln!("Skipping: missing ANTHROPIC_API_KEY (or RKAT_ANTHROPIC_API_KEY)");
        return Ok(());
    };
    let client = AnthropicClient::new(api_key)?;
    let request = LlmRequest::new(
        "claude-3-haiku-20240307",
        vec![Message::User(UserMessage::text("Say 'Hello'".to_string()))],
    );

    let mut stream = client.stream(&request);
    let mut got_text = false;

    while let Some(result) = stream.next().await {
        match result {
            Ok(LlmEvent::TextDelta { delta, .. }) => {
                if !delta.is_empty() {
                    got_text = true;
                }
            }
            Ok(LlmEvent::Done { .. }) => break,
            Ok(_) => {}
            Err(e) => return Err(format!("Unexpected error: {e:?}").into()),
        }
    }

    assert!(got_text);
    Ok(())
}

#[tokio::test]
#[ignore = "lane:e2e-smoke"]
async fn e2e_smoke_openai_live_image_generation() -> Result<(), Box<dyn std::error::Error>> {
    let Some(api_key) = openai_api_key() else {
        eprintln!("Skipping OpenAI image smoke: missing OPENAI_API_KEY (or RKAT_OPENAI_API_KEY)");
        return Ok(());
    };
    let model = std::env::var("RKAT_OPENAI_IMAGE_MODEL").unwrap_or_else(|_| "gpt-image-1".into());
    let client = OpenAiClient::new(api_key);
    let request = ProviderImageGenerationRequest {
        operation_id: ImageOperationId::new(uuid::Uuid::new_v4()),
        model: model.clone(),
        generate_request: tiny_image_request("openai", &model),
        execution_plan: GenerateImageExecutionPlan::OpenAiImagesApi {
            model: ModelId::new(model),
            max_count: std::num::NonZeroU32::new(1).unwrap(),
            capabilities: image_capabilities(),
            plan: OpenAiImagesApiPlan {
                endpoint: OpenAiImagesApiEndpoint::Generations,
            },
        },
        projected_messages: Vec::new(),
    };
    let output = client.execute_image_generation(request).await?;
    eprintln!(
        "OpenAI image smoke terminal={:?} images={} warnings={:?}",
        output.terminal,
        output.images.len(),
        output.warnings
    );
    assert!(
        !output.images.is_empty(),
        "OpenAI image smoke should return at least one image"
    );
    assert!(
        output.images[0].base64_data.len() > 256,
        "image payload should contain base64 data"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "lane:e2e-smoke"]
async fn e2e_smoke_gemini_live_image_generation() -> Result<(), Box<dyn std::error::Error>> {
    let Some(api_key) = gemini_api_key() else {
        eprintln!(
            "Skipping Gemini image smoke: missing GEMINI_API_KEY/RKAT_GEMINI_API_KEY/GOOGLE_API_KEY"
        );
        return Ok(());
    };
    let model = std::env::var("RKAT_GEMINI_IMAGE_MODEL")
        .unwrap_or_else(|_| "gemini-3.1-flash-image-preview".into());
    let client = GeminiClient::new(api_key);
    let request = ProviderImageGenerationRequest {
        operation_id: ImageOperationId::new(uuid::Uuid::new_v4()),
        model: model.clone(),
        generate_request: tiny_image_request("gemini", &model),
        execution_plan: GenerateImageExecutionPlan::GeminiNativeImageModel {
            model: ModelId::new(model),
            max_count: std::num::NonZeroU32::new(1).unwrap(),
            capabilities: image_capabilities(),
            plan: meerkat_core::image_generation::GeminiImageTurnPlan {
                projection_snapshot_id: meerkat_core::image_generation::ProjectionSnapshotId::new(
                    uuid::Uuid::new_v4(),
                ),
            },
        },
        projected_messages: Vec::new(),
    };
    let output = client.execute_image_generation(request).await?;
    eprintln!(
        "Gemini image smoke terminal={:?} images={} warnings={:?}",
        output.terminal,
        output.images.len(),
        output.warnings
    );
    assert!(
        !output.images.is_empty(),
        "Gemini image smoke should return at least one image"
    );
    assert!(
        output.images[0].base64_data.len() > 256,
        "image payload should contain base64 data"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "lane:e2e-live"]
async fn e2e_anthropic_tool_use() -> Result<(), Box<dyn std::error::Error>> {
    let Some(api_key) = anthropic_api_key() else {
        eprintln!("Skipping: missing ANTHROPIC_API_KEY (or RKAT_ANTHROPIC_API_KEY)");
        return Ok(());
    };
    let client = AnthropicClient::new(api_key)?;
    let request = LlmRequest::new(
        "claude-3-haiku-20240307",
        vec![Message::User(UserMessage::text(
            "What's the weather in Tokyo?".to_string(),
        ))],
    )
    .with_tools(vec![std::sync::Arc::new(meerkat_core::ToolDef {
        name: "get_weather".into(),
        description: "Get weather for a city".to_string(),
        input_schema: schema_for::<WeatherArgs>(),
        provenance: None,
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
            }) => return Err(format!("Unexpected error: {error:?}").into()),
            Ok(_) => {}
            Err(e) => return Err(format!("Unexpected error: {e:?}").into()),
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
    let Some((base_url, _server)) = spawn_test_server_or_skip(app).await? else {
        return Ok(());
    };

    let client = AnthropicClient::builder("invalid-key".to_string())
        .base_url(base_url)
        .build()?;

    let request = LlmRequest::new(
        "claude-3-haiku-20240307",
        vec![Message::User(UserMessage::text("test".to_string()))],
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
        other => return Err(format!("Expected auth error, got {other:?}").into()),
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
    let Some((base_url, _server)) = spawn_test_server_or_skip(app).await? else {
        return Ok(());
    };

    let client = AnthropicClient::builder("test-key".to_string())
        .base_url(base_url)
        .build()?;
    let request = LlmRequest::new(
        "claude-3-haiku-20240307",
        vec![Message::User(UserMessage::text("test".to_string()))],
    );

    let mut stream = client.stream(&request);
    let mut got_text = false;
    let mut got_done = false;
    let mut stop_reason = None;

    while let Some(result) = stream.next().await {
        match result {
            Ok(LlmEvent::TextDelta { delta, .. }) => {
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
            }) => return Err(format!("Unexpected error: {error:?}").into()),
            Ok(_) => {}
            Err(e) => return Err(format!("Unexpected error: {e:?}").into()),
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
    let Some((base_url, _server)) = spawn_test_server_or_skip(app).await? else {
        return Ok(());
    };

    let client = AnthropicClient::builder("test-key".to_string())
        .base_url(base_url)
        .build()?;
    let request = LlmRequest::new(
        "claude-3-haiku-20240307",
        vec![Message::User(UserMessage::text("test".to_string()))],
    );

    let mut stream = client.stream(&request);
    let mut got_text = false;
    let mut got_done = false;
    let mut stop_reason = None;

    while let Some(result) = stream.next().await {
        match result {
            Ok(LlmEvent::TextDelta { delta, .. }) => {
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
            }) => return Err(format!("Unexpected error: {error:?}").into()),
            Ok(_) => {}
            Err(e) => return Err(format!("Unexpected error: {e:?}").into()),
        }
    }

    assert!(got_text);
    assert!(got_done);
    assert_eq!(stop_reason, Some(StopReason::EndTurn));
    Ok(())
}

#[tokio::test]
async fn test_anthropic_stream_end_without_done_yields_success()
-> Result<(), Box<dyn std::error::Error>> {
    const SSE_BODY: &str = concat!(
        "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n",
        "\n"
    );

    let app = Router::new().route(
        "/v1/messages",
        post(|| async { (StatusCode::OK, SSE_BODY) }),
    );
    let (base_url, _server) = spawn_test_server(app).await?;

    let client = AnthropicClient::builder("test-key".to_string())
        .base_url(base_url)
        .build()?;
    let request = LlmRequest::new(
        "claude-3-haiku-20240307",
        vec![Message::User(UserMessage::text("test".to_string()))],
    );

    let mut stream = client.stream(&request);
    let mut got_text = false;
    let mut got_done = false;
    let mut stop_reason = None;

    while let Some(result) = stream.next().await {
        match result {
            Ok(LlmEvent::TextDelta { delta, .. }) => {
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
            }) => return Err(format!("Unexpected error: {error:?}").into()),
            Ok(_) => {}
            Err(e) => return Err(format!("Unexpected error: {e:?}").into()),
        }
    }

    assert!(got_text);
    assert!(got_done);
    assert_eq!(stop_reason, Some(StopReason::EndTurn));
    Ok(())
}

#[tokio::test]
#[ignore = "lane:e2e-live"]
async fn e2e_openai_stream() -> Result<(), Box<dyn std::error::Error>> {
    let Some(api_key) = openai_api_key() else {
        eprintln!("Skipping: missing OPENAI_API_KEY (or RKAT_OPENAI_API_KEY)");
        return Ok(());
    };
    let client = OpenAiClient::new(api_key);
    let request = LlmRequest::new(
        "gpt-5.2",
        vec![Message::User(UserMessage::text("Say 'Hello'".to_string()))],
    );

    let mut stream = client.stream(&request);
    let mut got_text = false;

    while let Some(result) = stream.next().await {
        match result {
            Ok(LlmEvent::TextDelta { delta, .. }) => {
                if !delta.is_empty() {
                    got_text = true;
                }
            }
            Ok(LlmEvent::Done { .. }) => break,
            Ok(_) => {}
            Err(e) => return Err(format!("Unexpected error: {e:?}").into()),
        }
    }

    assert!(got_text);
    Ok(())
}

#[tokio::test]
#[ignore = "lane:e2e-live"]
async fn e2e_openai_tool_use() -> Result<(), Box<dyn std::error::Error>> {
    let Some(api_key) = openai_api_key() else {
        eprintln!("Skipping: missing OPENAI_API_KEY (or RKAT_OPENAI_API_KEY)");
        return Ok(());
    };
    let client = OpenAiClient::new(api_key);
    let request = LlmRequest::new(
        "gpt-5.2",
        vec![Message::User(UserMessage::text(
            "What's the weather in Tokyo?".to_string(),
        ))],
    )
    .with_tools(vec![std::sync::Arc::new(meerkat_core::ToolDef {
        name: "get_weather".into(),
        description: "Get weather for a city".to_string(),
        input_schema: schema_for::<WeatherArgs>(),
        provenance: None,
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
            }) => return Err(format!("Unexpected error: {error:?}").into()),
            Ok(_) => {}
            Err(e) => return Err(format!("Unexpected error: {e:?}").into()),
        }
    }

    assert!(got_tool);
    Ok(())
}

#[tokio::test]
#[ignore = "lane:e2e-live"]
async fn e2e_gemini_stream() -> Result<(), Box<dyn std::error::Error>> {
    let Some(api_key) = gemini_api_key() else {
        eprintln!("Skipping: missing GOOGLE_API_KEY (or GEMINI_API_KEY/RKAT_GEMINI_API_KEY)");
        return Ok(());
    };
    let client = GeminiClient::new(api_key);
    let request = LlmRequest::new(
        "gemini-1.5-flash",
        vec![Message::User(UserMessage::text("Say 'Hello'".to_string()))],
    );

    let mut stream = client.stream(&request);
    let mut got_text = false;

    while let Some(result) = stream.next().await {
        match result {
            Ok(LlmEvent::TextDelta { delta, .. }) => {
                if !delta.is_empty() {
                    got_text = true;
                }
            }
            Ok(LlmEvent::Done { .. }) => break,
            Ok(_) => {}
            Err(e) => return Err(format!("Unexpected error: {e:?}").into()),
        }
    }

    assert!(got_text);
    Ok(())
}

#[tokio::test]
#[ignore = "lane:e2e-live"]
async fn e2e_gemini_tool_use() -> Result<(), Box<dyn std::error::Error>> {
    let Some(api_key) = gemini_api_key() else {
        eprintln!("Skipping: missing GOOGLE_API_KEY (or GEMINI_API_KEY/RKAT_GEMINI_API_KEY)");
        return Ok(());
    };
    let client = GeminiClient::new(api_key);
    let request = LlmRequest::new(
        "gemini-1.5-flash",
        vec![Message::User(UserMessage::text(
            "What's the weather in Tokyo?".to_string(),
        ))],
    )
    .with_tools(vec![std::sync::Arc::new(meerkat_core::ToolDef {
        name: "get_weather".into(),
        description: "Get weather for a city".to_string(),
        input_schema: schema_for::<WeatherArgs>(),
        provenance: None,
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
            }) => return Err(format!("Unexpected error: {error:?}").into()),
            Ok(_) => {}
            Err(e) => return Err(format!("Unexpected error: {e:?}").into()),
        }
    }

    assert!(got_tool);
    Ok(())
}

#[tokio::test]
async fn test_openai_auth_error() -> Result<(), Box<dyn std::error::Error>> {
    let app = Router::new()
        .route(
            "/v1/responses",
            post(|| async { (StatusCode::UNAUTHORIZED, "unauthorized") }),
        )
        .route(
            "/v1/chat/completions",
            post(|| async { (StatusCode::UNAUTHORIZED, "unauthorized") }),
        );
    let Some((base_url, _server)) = spawn_test_server_or_skip(app).await? else {
        return Ok(());
    };

    let client = OpenAiClient::new_with_base_url("invalid-key".to_string(), base_url);

    let request = LlmRequest::new(
        "gpt-5.2",
        vec![Message::User(UserMessage::text("test".to_string()))],
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
        other => return Err(format!("Expected auth error, got {other:?}").into()),
    }
    Ok(())
}

// =============================================================================
// Structured Output E2E Tests
// =============================================================================
//
// These tests verify structured output works with real API calls.
// Run with: ANTHROPIC_API_KEY=... cargo test -p meerkat-client structured_output -- --ignored

fn person_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "age": {"type": "integer"}
        },
        "required": ["name", "age"]
    })
}

fn nested_profile_schema_without_additional_properties() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "person": {
                "type": "object",
                "properties": {
                    "name": {"type": "string"},
                    "age": {"type": "integer"}
                },
                "required": ["name", "age"]
            },
            "tags": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "label": {"type": "string"}
                    },
                    "required": ["label"]
                }
            }
        },
        "required": ["person", "tags"]
    })
}

fn gemini_supported_rich_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "status": {
                "oneOf": [
                    {"type": "string"},
                    {"type": "null"}
                ]
            },
            "payload": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "score": {"type": "number", "minimum": 0.0, "maximum": 1.0},
                    "category": {"type": "string"}
                },
                "required": ["score", "category"]
            }
        },
        "required": ["status", "payload"],
        "additionalProperties": false
    })
}

/// Collects all text from a stream and returns the final output
async fn collect_stream_text(
    client: &impl LlmClient,
    request: &LlmRequest,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut stream = client.stream(request);
    let mut text = String::new();

    while let Some(result) = stream.next().await {
        match result {
            Ok(LlmEvent::TextDelta { delta, .. }) => {
                text.push_str(&delta);
            }
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Success { .. },
            }) => break,
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Error { error },
            }) => return Err(format!("LLM error: {error:?}").into()),
            Ok(_) => {}
            Err(e) => return Err(format!("Stream error: {e:?}").into()),
        }
    }

    Ok(text)
}

#[tokio::test]
#[ignore = "lane:e2e-live"]
async fn e2e_anthropic_structured_output() -> Result<(), Box<dyn std::error::Error>> {
    let Some(api_key) = anthropic_api_key() else {
        eprintln!("Skipping: missing ANTHROPIC_API_KEY (or RKAT_ANTHROPIC_API_KEY)");
        return Ok(());
    };
    let client = AnthropicClient::new(api_key)?;
    let request = LlmRequest::new(
        "claude-sonnet-4-5",
        vec![Message::User(UserMessage::text(
            "Generate a person named Alice who is 30 years old.".to_string(),
        ))],
    )
    .with_provider_params(
        meerkat_core::lifecycle::run_primitive::ProviderTag::Anthropic(
            meerkat_core::lifecycle::run_primitive::AnthropicProviderTag {
                structured_output: Some(
                    meerkat_core::OutputSchema::new(person_schema()).expect("valid schema"),
                ),
                ..Default::default()
            },
        ),
    );

    let text = collect_stream_text(&client, &request).await?;

    // Verify it's valid JSON matching the schema
    let parsed: Value = serde_json::from_str(&text)?;
    assert!(parsed.get("name").is_some(), "should have name field");
    assert!(parsed.get("age").is_some(), "should have age field");

    Ok(())
}

#[tokio::test]
#[ignore = "lane:e2e-live"]
async fn e2e_openai_structured_output() -> Result<(), Box<dyn std::error::Error>> {
    let Some(api_key) = openai_api_key() else {
        eprintln!("Skipping: missing OPENAI_API_KEY (or RKAT_OPENAI_API_KEY)");
        return Ok(());
    };
    let client = OpenAiClient::new(api_key);
    let request = LlmRequest::new(
        "gpt-5.2",
        vec![Message::User(UserMessage::text(
            "Generate a person named Bob who is 25 years old.".to_string(),
        ))],
    )
    .with_provider_params(meerkat_core::lifecycle::run_primitive::ProviderTag::OpenAi(
        meerkat_core::lifecycle::run_primitive::OpenAiProviderTag {
            structured_output: Some(
                meerkat_core::OutputSchema::new(person_schema()).expect("valid schema"),
            ),
            ..Default::default()
        },
    ));

    let text = collect_stream_text(&client, &request).await?;

    // Verify it's valid JSON matching the schema
    let parsed: Value = serde_json::from_str(&text)?;
    assert!(parsed.get("name").is_some(), "should have name field");
    assert!(parsed.get("age").is_some(), "should have age field");

    Ok(())
}

#[tokio::test]
#[ignore = "lane:e2e-live"]
async fn e2e_gemini_structured_output() -> Result<(), Box<dyn std::error::Error>> {
    let Some(api_key) = gemini_api_key() else {
        eprintln!("Skipping: missing GOOGLE_API_KEY (or GEMINI_API_KEY/RKAT_GEMINI_API_KEY)");
        return Ok(());
    };
    let client = GeminiClient::new(api_key);
    let request = LlmRequest::new(
        "gemini-3-flash-preview",
        vec![Message::User(UserMessage::text(
            "Generate a person named Carol who is 35 years old.".to_string(),
        ))],
    )
    .with_provider_params(meerkat_core::lifecycle::run_primitive::ProviderTag::Gemini(
        meerkat_core::lifecycle::run_primitive::GeminiProviderTag {
            structured_output: Some(
                meerkat_core::OutputSchema::new(person_schema()).expect("valid schema"),
            ),
            ..Default::default()
        },
    ));

    let text = collect_stream_text(&client, &request).await?;

    // Verify it's valid JSON matching the schema
    let parsed: Value = serde_json::from_str(&text)?;
    assert!(parsed.get("name").is_some(), "should have name field");
    assert!(parsed.get("age").is_some(), "should have age field");

    Ok(())
}

#[tokio::test]
#[ignore = "lane:e2e-live"]
async fn e2e_openai_structured_output_strict_nested_schema()
-> Result<(), Box<dyn std::error::Error>> {
    let Some(api_key) = openai_api_key() else {
        eprintln!("Skipping: missing OPENAI_API_KEY (or RKAT_OPENAI_API_KEY)");
        return Ok(());
    };
    let client = OpenAiClient::new(api_key);
    let request = LlmRequest::new(
        "gpt-5.2",
        vec![Message::User(UserMessage::text(
            "Return JSON with person={name:'Dina',age:41} and tags=[{label:'runner'}].",
        ))],
    )
    .with_provider_params(meerkat_core::lifecycle::run_primitive::ProviderTag::OpenAi(
        meerkat_core::lifecycle::run_primitive::OpenAiProviderTag {
            structured_output: Some(
                meerkat_core::OutputSchema::new(
                    nested_profile_schema_without_additional_properties(),
                )
                .expect("valid schema")
                .strict(),
            ),
            ..Default::default()
        },
    ));

    let text = collect_stream_text(&client, &request).await?;
    let parsed: Value = serde_json::from_str(&text)?;
    assert!(parsed.get("person").is_some(), "should have person field");
    assert!(
        parsed["person"].get("name").is_some(),
        "should have nested person.name"
    );
    assert!(
        parsed["tags"].as_array().is_some_and(|arr| !arr.is_empty()),
        "should have non-empty tags array"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "lane:e2e-live"]
async fn e2e_anthropic_structured_output_strict_nested_schema()
-> Result<(), Box<dyn std::error::Error>> {
    let Some(api_key) = anthropic_api_key() else {
        eprintln!("Skipping: missing ANTHROPIC_API_KEY (or RKAT_ANTHROPIC_API_KEY)");
        return Ok(());
    };
    let client = AnthropicClient::new(api_key)?;
    let request = LlmRequest::new(
        "claude-sonnet-4-5",
        vec![Message::User(UserMessage::text(
            "Return JSON with person={name:'Evan',age:29} and tags=[{label:'designer'}].",
        ))],
    )
    .with_provider_params(
        meerkat_core::lifecycle::run_primitive::ProviderTag::Anthropic(
            meerkat_core::lifecycle::run_primitive::AnthropicProviderTag {
                structured_output: Some(
                    meerkat_core::OutputSchema::new(
                        nested_profile_schema_without_additional_properties(),
                    )
                    .expect("valid schema")
                    .strict(),
                ),
                ..Default::default()
            },
        ),
    );

    let text = collect_stream_text(&client, &request).await?;
    let parsed: Value = serde_json::from_str(&text)?;
    assert!(parsed.get("person").is_some(), "should have person field");
    assert!(
        parsed["person"].get("age").is_some(),
        "should have nested person.age"
    );
    assert!(
        parsed["tags"].as_array().is_some_and(|arr| !arr.is_empty()),
        "should have non-empty tags array"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "lane:e2e-live"]
async fn e2e_gemini_structured_output_rich_schema_keywords()
-> Result<(), Box<dyn std::error::Error>> {
    let Some(api_key) = gemini_api_key() else {
        eprintln!("Skipping: missing GOOGLE_API_KEY (or GEMINI_API_KEY/RKAT_GEMINI_API_KEY)");
        return Ok(());
    };
    let client = GeminiClient::new(api_key);
    let request = LlmRequest::new(
        "gemini-3-flash-preview",
        vec![Message::User(UserMessage::text(
            "Return JSON: status='ok', payload={score:0.6, category:'test'}.".to_string(),
        ))],
    )
    .with_provider_params(meerkat_core::lifecycle::run_primitive::ProviderTag::Gemini(
        meerkat_core::lifecycle::run_primitive::GeminiProviderTag {
            structured_output: Some(
                meerkat_core::OutputSchema::new(gemini_supported_rich_schema())
                    .expect("valid schema")
                    .strict(),
            ),
            ..Default::default()
        },
    ));

    let text = collect_stream_text(&client, &request).await?;
    let parsed: Value = serde_json::from_str(&text)?;
    assert!(parsed.get("status").is_some(), "should have status");
    assert!(parsed.get("payload").is_some(), "should have payload");
    assert!(
        parsed["payload"].get("score").is_some(),
        "should have payload.score"
    );
    Ok(())
}

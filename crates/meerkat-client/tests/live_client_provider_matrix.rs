#![cfg(feature = "integration-real-tests")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

//! Live integration tests for LLM clients.
//!
//! Verifies stream normalization and error handling for all providers.
//!
//! No CI lane runs these tests: each live test is ignored and needs the
//! provider's API key. Run them by hand:
//! `cargo test -p meerkat-client --features integration-real-tests --test
//! live_client_provider_matrix -- --ignored`.

use axum::{Router, http::StatusCode, routing::post};
use futures::StreamExt;
use meerkat_client::{
    AnthropicClient, GeminiClient, LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest,
    OpenAiClient,
};
use meerkat_core::ProviderId;
use meerkat_core::image_generation::{
    GenerateImageExecutionPlan, GenerateImageRequest, ImageContinuityTokenSupport,
    ImageFormatPreference, ImageGenerationBackendKind, ImageGenerationIntent,
    ImageGenerationTargetCapabilities, ImageGenerationTargetPreference, ImageOperationId,
    ImageQualityPreference, ImageSizePreference, ProjectionSnapshotId, PromptSource, PromptText,
    ToolCallId,
};
use meerkat_core::lifecycle::run_primitive::ModelId;
use meerkat_core::lifecycle::run_primitive::{
    AnthropicProviderTag, GeminiProviderTag, OpaqueProviderBody, OpenAiProviderTag, ProviderTag,
    ReasoningEffort,
};
use meerkat_core::model_profile::catalog::ImageGenerationModelRoute;
use meerkat_core::{
    AgentLlmClient, AssistantBlock, BlockAssistantMessage, Message, StopReason, UserMessage,
};
use meerkat_gemini::{GeminiImageOutputOptions, GeminiImageTurnPlan};
use meerkat_llm_core::{ImageGenerationExecutor, LlmClientAdapter, ProviderImageGenerationRequest};
use meerkat_openai::{
    OpenAiImageOutputOptions, OpenAiImageProviderParams, OpenAiImagesApiEndpoint,
    OpenAiImagesApiPlan, OpenAiImagesApiRequestShape, OpenAiResponsesImagePlan,
};
use schemars::JsonSchema;
use serde_json::{Map, Value};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::{TcpListener, TcpStream};

/// The text model each provider's tests send: the cheapest current catalog
/// model of that provider by input and output price when chosen (Claude
/// Haiku 5.5, GPT-6 Luna, Gemini 3.1 Flash-Lite). The image smokes use the
/// catalog's image-generation defaults. `provider_matrix_catalog.rs`, a test
/// that needs no provider keys or features, keeps every model id this file
/// names in Core's catalog.
const ANTHROPIC_MODEL: &str = "claude-haiku-5-5";
const OPENAI_MODEL: &str = "gpt-6-luna";
const GEMINI_MODEL: &str = "gemini-3.1-flash-lite";

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

async fn collect_stream_text_and_server_tool_count<C: LlmClient + ?Sized>(
    client: &C,
    request: &LlmRequest,
) -> Result<(String, usize), Box<dyn std::error::Error>> {
    let mut stream = client.stream(request);
    let mut text = String::new();
    let mut server_tool_count = 0;
    while let Some(result) = stream.next().await {
        match result {
            Ok(LlmEvent::TextDelta { delta, .. }) => text.push_str(&delta),
            Ok(LlmEvent::ServerToolContent { .. }) => server_tool_count += 1,
            Ok(LlmEvent::Done { .. }) => break,
            Ok(_) => {}
            Err(e) => return Err(format!("Unexpected error: {e:?}").into()),
        }
    }
    Ok((text, server_tool_count))
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
#[ignore = "live provider matrix: needs provider API keys; run by hand with --ignored"]
async fn e2e_anthropic_stream() -> Result<(), Box<dyn std::error::Error>> {
    let Some(api_key) = anthropic_api_key() else {
        eprintln!("Skipping: missing ANTHROPIC_API_KEY (or RKAT_ANTHROPIC_API_KEY)");
        return Ok(());
    };
    let client = AnthropicClient::new(api_key)?;
    let request = LlmRequest::new(
        ANTHROPIC_MODEL,
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
#[ignore = "live provider matrix: needs provider API keys; run by hand with --ignored"]
async fn e2e_smoke_provider_web_search_all_three() -> Result<(), Box<dyn std::error::Error>> {
    let mut ran = 0usize;
    let prompt = "Use the web search tool for this answer. Search for the current title of https://example.com and answer in one short sentence with the source URL.";

    if let Some(api_key) = openai_api_key() {
        ran += 1;
        let client = OpenAiClient::new(api_key);
        let request = LlmRequest::new(
            OPENAI_MODEL,
            vec![Message::User(UserMessage::text(prompt.to_string()))],
        )
        .with_provider_params(ProviderTag::OpenAi(OpenAiProviderTag {
            web_search: Some(OpaqueProviderBody::from_value(&serde_json::json!({
                "type": "web_search"
            }))),
            ..Default::default()
        }));
        let (text, server_tools) =
            collect_stream_text_and_server_tool_count(&client, &request).await?;
        assert!(
            server_tools > 0,
            "OpenAI should emit web_search server tool content; text={text}"
        );
        assert!(!text.trim().is_empty(), "OpenAI should return text");
    } else {
        eprintln!(
            "Skipping OpenAI web search smoke: missing OPENAI_API_KEY (or RKAT_OPENAI_API_KEY)"
        );
    }

    if let Some(api_key) = anthropic_api_key() {
        ran += 1;
        let client = AnthropicClient::new(api_key)?;
        let request = LlmRequest::new(
            ANTHROPIC_MODEL,
            vec![Message::User(UserMessage::text(prompt.to_string()))],
        )
        .with_provider_params(ProviderTag::Anthropic(AnthropicProviderTag {
            web_search: Some(OpaqueProviderBody::from_value(&serde_json::json!({
                "type": "web_search_20250305",
                "name": "web_search"
            }))),
            ..Default::default()
        }));
        let (text, server_tools) =
            collect_stream_text_and_server_tool_count(&client, &request).await?;
        assert!(
            server_tools > 0,
            "Anthropic should emit web_search server tool content; text={text}"
        );
        assert!(!text.trim().is_empty(), "Anthropic should return text");
    } else {
        eprintln!(
            "Skipping Anthropic web search smoke: missing ANTHROPIC_API_KEY (or RKAT_ANTHROPIC_API_KEY)"
        );
    }

    if let Some(api_key) = gemini_api_key() {
        ran += 1;
        let client = GeminiClient::new(api_key);
        let request = LlmRequest::new(
            GEMINI_MODEL,
            vec![Message::User(UserMessage::text(prompt.to_string()))],
        )
        .with_provider_params(ProviderTag::Gemini(GeminiProviderTag {
            google_search: Some(OpaqueProviderBody::from_value(&serde_json::json!({}))),
            ..Default::default()
        }));
        let (text, server_tools) =
            collect_stream_text_and_server_tool_count(&client, &request).await?;
        assert!(
            server_tools > 0,
            "Gemini should emit google_search grounding metadata; text={text}"
        );
        assert!(!text.trim().is_empty(), "Gemini should return text");
    } else {
        eprintln!(
            "Skipping Gemini web search smoke: missing GOOGLE_API_KEY (or GEMINI_API_KEY/RKAT_GEMINI_API_KEY)"
        );
    }

    if ran == 0 {
        eprintln!("Skipping provider web search smoke: no provider API keys configured");
    }
    Ok(())
}

/// The stateless two-step hosted-search loop on the OpenAI API (`store:
/// false`, the default). Step 1 answers a question that needs a search and
/// names no page, so the model searches. Step 2's request replays every
/// `web_search_call` of step 1 exactly as captured, each search action with
/// the sources it consulted, and the answer with its `url_citation`
/// annotations, and the API accepts that request. A step 1 that made no
/// search action fails on its own message, apart from a replay defect: then
/// the sources path was not exercised.
#[tokio::test]
#[ignore = "live provider matrix: needs provider API keys; run by hand with --ignored"]
async fn e2e_openai_web_search_evidence_replays_into_the_next_step()
-> Result<(), Box<dyn std::error::Error>> {
    let Some(api_key) = openai_api_key() else {
        eprintln!(
            "Skipping OpenAI web search evidence: missing OPENAI_API_KEY (or RKAT_OPENAI_API_KEY)"
        );
        return Ok(());
    };
    let web_search = ProviderTag::OpenAi(OpenAiProviderTag {
        web_search: Some(OpaqueProviderBody::from_value(&serde_json::json!({
            "type": "web_search"
        }))),
        ..Default::default()
    });
    let client = Arc::new(OpenAiClient::new(api_key));
    let adapter = LlmClientAdapter::new(client.clone(), OPENAI_MODEL.to_string())
        .with_provider_params(Some(web_search.clone()));
    let question = Message::User(UserMessage::text(
        "Use the web search tool for this answer. What is the latest stable release of the Rust programming language, and on what date was it released? Answer in one short sentence and cite your source.".to_string(),
    ));
    let step = AgentLlmClient::stream_response(
        &adapter,
        std::slice::from_ref(&question),
        &[],
        2048,
        None,
        None,
    )
    .await?;
    // Step 1's search calls, as captured.
    let captured: Vec<&Value> = step
        .blocks()
        .iter()
        .filter_map(|block| match block {
            AssistantBlock::ServerToolContent { content, .. }
                if content["type"] == "web_search_call" =>
            {
                Some(content)
            }
            _ => None,
        })
        .collect();
    let searches = captured
        .iter()
        .filter(|call| call["action"]["type"] == "search")
        .count();
    assert!(
        searches > 0,
        "no search action: the sources path was not exercised (step 1's web_search_call actions: {:?})",
        captured
            .iter()
            .map(|call| call["action"]["type"].clone())
            .collect::<Vec<_>>()
    );
    let history = vec![
        question,
        Message::BlockAssistant(BlockAssistantMessage::new(
            step.blocks().to_vec(),
            step.stop_reason(),
        )),
        Message::User(UserMessage::text(
            "Which URL did you cite? Answer with the URL only.".to_string(),
        )),
    ];

    // The second step's request, as the client lowers and sends it.
    let request = LlmRequest::new(OPENAI_MODEL, client.project_replay_messages(&history)?)
        .with_provider_params(web_search);
    let body = client.build_request_body(&request)?;
    assert_eq!(body["store"], false, "the loop is stateless: {body}");
    let input = body["input"]
        .as_array()
        .ok_or("the request input is not an array")?;
    let replayed: Vec<&Value> = input
        .iter()
        .filter(|item| item["type"] == "web_search_call")
        .collect();
    assert_eq!(
        replayed, captured,
        "step 2 must replay every web_search_call of step 1 exactly as captured: {body}"
    );
    for call in &replayed {
        if call["action"]["type"] == "search" {
            assert!(
                call["action"]["sources"]
                    .as_array()
                    .is_some_and(|sources| !sources.is_empty()),
                "every replayed search action must carry the sources it consulted: {call}"
            );
        }
    }
    let sources: usize = replayed
        .iter()
        .filter_map(|call| call["action"]["sources"].as_array())
        .map(Vec::len)
        .sum();
    let citations = input
        .iter()
        .filter(|item| item["type"] == "message")
        .filter_map(|item| item["content"].as_array())
        .flatten()
        .filter_map(|part| part["annotations"].as_array())
        .flatten()
        .filter(|annotation| annotation["type"] == "url_citation")
        .count();
    assert!(
        citations > 0,
        "step 2 must replay the answer's url_citation annotations: {body}"
    );

    // The API accepts that request: no 4xx, a successful turn.
    let mut stream = client.stream(&request);
    let mut outcome = None;
    while let Some(event) = stream.next().await {
        if let LlmEvent::Done { outcome: done } = event? {
            outcome = Some(done);
            break;
        }
    }
    assert!(
        matches!(outcome, Some(LlmDoneOutcome::Success { .. })),
        "the API must accept step 2: {outcome:?}"
    );
    println!(
        "OpenAI web search evidence replayed: {} web_search_call item(s) byte-equal, {searches} search action(s) with {sources} source(s), {citations} url_citation annotation(s); the API accepted step 2",
        replayed.len()
    );
    Ok(())
}

#[tokio::test]
#[ignore = "live provider matrix: needs provider API keys; run by hand with --ignored"]
async fn e2e_smoke_openai_live_image_generation() -> Result<(), Box<dyn std::error::Error>> {
    let Some(api_key) = openai_api_key() else {
        eprintln!("Skipping OpenAI image smoke: missing OPENAI_API_KEY (or RKAT_OPENAI_API_KEY)");
        return Ok(());
    };
    let image_model = std::env::var("RKAT_OPENAI_IMAGE_MODEL").unwrap_or_else(|_| {
        meerkat_models::default_image_generation_model(meerkat_core::Provider::OpenAI)
            .expect("the catalog has an OpenAI image-generation default")
            .model_id
            .into()
    });
    // The catalog owns the route: a hosted-tool image model is generated by
    // its tool model inside a Responses call on the route's call model (the
    // requested model itself when the route names none).
    let hosted_route =
        match meerkat_models::image_generation_model(meerkat_core::Provider::OpenAI, &image_model)
            .map(|profile| profile.route)
        {
            Some(ImageGenerationModelRoute::OpenAiHostedResponsesTool {
                provider_call_model_id,
                tool_model_id,
            }) => Some((
                provider_call_model_id.map_or_else(|| image_model.clone(), str::to_string),
                tool_model_id,
            )),
            _ => None,
        };
    let hosted_reasoning_effort = match std::env::var("RKAT_OPENAI_IMAGE_REASONING_EFFORT")
        .as_deref()
    {
        Ok("none") => ReasoningEffort::None,
        Ok("low") | Err(_) => ReasoningEffort::Low,
        Ok("medium") => ReasoningEffort::Medium,
        Ok("high") => ReasoningEffort::High,
        Ok("xhigh") => ReasoningEffort::XHigh,
        Ok(other) => {
            eprintln!("Ignoring unsupported RKAT_OPENAI_IMAGE_REASONING_EFFORT={other}; using low");
            ReasoningEffort::Low
        }
    };
    let client = OpenAiClient::new(api_key);
    let mut hosted_provider_params = OpenAiImageProviderParams::default();
    hosted_provider_params.reasoning_effort = Some(hosted_reasoning_effort);
    hosted_provider_params.web_search = Some(serde_json::json!({
        "search_context_size": "low",
        "external_web_access": true,
        "return_token_budget": "default"
    }));

    let (provider_model, execution_plan) = if let Some((call_model, tool_model)) = hosted_route {
        (
            call_model,
            GenerateImageExecutionPlan {
                provider: ProviderId::new("openai"),
                backend: ImageGenerationBackendKind::HostedTool,
                max_count: std::num::NonZeroU32::new(1).unwrap(),
                capabilities: image_capabilities(),
                requires_scoped_override: false,
                provider_plan: serde_json::to_value(OpenAiResponsesImagePlan {
                    tool_name: "image_generation".into(),
                    model: ModelId::new(tool_model),
                    output: OpenAiImageOutputOptions::default(),
                    provider_params: hosted_provider_params,
                })?,
            },
        )
    } else {
        (
            image_model.clone(),
            GenerateImageExecutionPlan {
                provider: ProviderId::new("openai"),
                backend: ImageGenerationBackendKind::ProviderApi,
                max_count: std::num::NonZeroU32::new(1).unwrap(),
                capabilities: image_capabilities(),
                requires_scoped_override: false,
                provider_plan: serde_json::to_value(OpenAiImagesApiPlan {
                    endpoint: OpenAiImagesApiEndpoint::Generations,
                    request_shape: OpenAiImagesApiRequestShape::GptImage,
                    output: OpenAiImageOutputOptions::default(),
                    provider_params: OpenAiImageProviderParams::default(),
                })?,
            },
        )
    };
    let request = ProviderImageGenerationRequest {
        operation_id: ImageOperationId::new(uuid::Uuid::new_v4()),
        model: provider_model,
        generate_request: tiny_image_request("openai", &image_model),
        execution_plan,
        projected_messages: Vec::new(),
    };
    let output = client.execute_image_generation(request).await?;
    eprintln!(
        "OpenAI image smoke terminal={:?} images={} warnings={:?}",
        output.terminal_observation,
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
#[ignore = "live provider matrix: needs provider API keys; run by hand with --ignored"]
async fn e2e_smoke_gemini_live_image_generation() -> Result<(), Box<dyn std::error::Error>> {
    let Some(api_key) = gemini_api_key() else {
        eprintln!(
            "Skipping Gemini image smoke: missing GEMINI_API_KEY/RKAT_GEMINI_API_KEY/GOOGLE_API_KEY"
        );
        return Ok(());
    };
    let model = std::env::var("RKAT_GEMINI_IMAGE_MODEL").unwrap_or_else(|_| {
        meerkat_models::default_image_generation_model(meerkat_core::Provider::Gemini)
            .expect("the catalog has a Gemini image-generation default")
            .model_id
            .into()
    });
    let client = GeminiClient::new(api_key);
    let request = ProviderImageGenerationRequest {
        operation_id: ImageOperationId::new(uuid::Uuid::new_v4()),
        model: model.clone(),
        generate_request: tiny_image_request("gemini", &model),
        execution_plan: GenerateImageExecutionPlan {
            provider: ProviderId::new("gemini"),
            backend: ImageGenerationBackendKind::NativeModel,
            max_count: std::num::NonZeroU32::new(1).unwrap(),
            capabilities: image_capabilities(),
            requires_scoped_override: true,
            provider_plan: serde_json::to_value(GeminiImageTurnPlan {
                projection_snapshot_id: ProjectionSnapshotId::new(uuid::Uuid::new_v4()),
                output: GeminiImageOutputOptions::default(),
            })?,
        },
        projected_messages: Vec::new(),
    };
    let output = client.execute_image_generation(request).await?;
    eprintln!(
        "Gemini image smoke terminal={:?} images={} warnings={:?}",
        output.terminal_observation,
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
#[ignore = "live provider matrix: needs provider API keys; run by hand with --ignored"]
async fn e2e_anthropic_tool_use() -> Result<(), Box<dyn std::error::Error>> {
    let Some(api_key) = anthropic_api_key() else {
        eprintln!("Skipping: missing ANTHROPIC_API_KEY (or RKAT_ANTHROPIC_API_KEY)");
        return Ok(());
    };
    let client = AnthropicClient::new(api_key)?;
    let request = LlmRequest::new(
        ANTHROPIC_MODEL,
        vec![Message::User(UserMessage::text(
            "What's the weather in Tokyo?".to_string(),
        ))],
    )
    .with_tools(vec![std::sync::Arc::new(meerkat_core::ToolDef {
        audience: Default::default(),
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
                outcome: LlmDoneOutcome::Success { stop_reason, .. },
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
        ANTHROPIC_MODEL,
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
        ANTHROPIC_MODEL,
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
                        ..
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
        ANTHROPIC_MODEL,
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
                        ..
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

/// Rule 8 (Terminal Truth Laundering): a stream that emits content but ends
/// WITHOUT a terminal event (no `message_stop` / `message_delta` / `[DONE]`) is
/// truncated, not successful. It must terminalize as a typed
/// `LlmError::IncompleteResponse` (supplied by `ensure_terminal_done` once the
/// former synthetic-`Done{Success}`-on-clean-EOF branch was removed) — never a
/// laundered `Done{Success{EndTurn}}` that hides the truncation.
#[tokio::test]
async fn test_anthropic_stream_end_without_done_yields_incomplete()
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
        ANTHROPIC_MODEL,
        vec![Message::User(UserMessage::text("test".to_string()))],
    );

    let mut stream = client.stream(&request);
    let mut got_text = false;
    let mut got_incomplete = false;

    while let Some(result) = stream.next().await {
        match result {
            Ok(LlmEvent::TextDelta { delta, .. }) => {
                if !delta.is_empty() {
                    got_text = true;
                }
            }
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Success { stop_reason, .. },
            }) => {
                return Err(format!(
                    "truncated stream must not launder into Done{{Success}}; got stop_reason {stop_reason:?}"
                )
                .into());
            }
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Error { error },
            }) => {
                assert!(
                    matches!(error, LlmError::IncompleteResponse { .. }),
                    "expected IncompleteResponse on truncated stream, got {error:?}"
                );
                got_incomplete = true;
                break;
            }
            Ok(_) => {}
            Err(e) => return Err(format!("Unexpected stream error: {e:?}").into()),
        }
    }

    assert!(got_text, "the content_block_delta text should still arrive");
    assert!(
        got_incomplete,
        "stream-end-without-terminal must yield Done{{Error{{IncompleteResponse}}}}"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "live provider matrix: needs provider API keys; run by hand with --ignored"]
async fn e2e_openai_stream() -> Result<(), Box<dyn std::error::Error>> {
    let Some(api_key) = openai_api_key() else {
        eprintln!("Skipping: missing OPENAI_API_KEY (or RKAT_OPENAI_API_KEY)");
        return Ok(());
    };
    let client = OpenAiClient::new(api_key);
    let request = LlmRequest::new(
        OPENAI_MODEL,
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
#[ignore = "live provider matrix: needs provider API keys; run by hand with --ignored"]
async fn e2e_openai_tool_use() -> Result<(), Box<dyn std::error::Error>> {
    let Some(api_key) = openai_api_key() else {
        eprintln!("Skipping: missing OPENAI_API_KEY (or RKAT_OPENAI_API_KEY)");
        return Ok(());
    };
    let client = OpenAiClient::new(api_key);
    let request = LlmRequest::new(
        OPENAI_MODEL,
        vec![Message::User(UserMessage::text(
            "What's the weather in Tokyo?".to_string(),
        ))],
    )
    .with_tools(vec![std::sync::Arc::new(meerkat_core::ToolDef {
        audience: Default::default(),
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
                outcome: LlmDoneOutcome::Success { stop_reason, .. },
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
#[ignore = "live provider matrix: needs provider API keys; run by hand with --ignored"]
async fn e2e_gemini_stream() -> Result<(), Box<dyn std::error::Error>> {
    let Some(api_key) = gemini_api_key() else {
        eprintln!("Skipping: missing GOOGLE_API_KEY (or GEMINI_API_KEY/RKAT_GEMINI_API_KEY)");
        return Ok(());
    };
    let client = GeminiClient::new(api_key);
    let request = LlmRequest::new(
        GEMINI_MODEL,
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
#[ignore = "live provider matrix: needs provider API keys; run by hand with --ignored"]
async fn e2e_gemini_tool_use() -> Result<(), Box<dyn std::error::Error>> {
    let Some(api_key) = gemini_api_key() else {
        eprintln!("Skipping: missing GOOGLE_API_KEY (or GEMINI_API_KEY/RKAT_GEMINI_API_KEY)");
        return Ok(());
    };
    let client = GeminiClient::new(api_key);
    let request = LlmRequest::new(
        GEMINI_MODEL,
        vec![Message::User(UserMessage::text(
            "What's the weather in Tokyo?".to_string(),
        ))],
    )
    .with_tools(vec![std::sync::Arc::new(meerkat_core::ToolDef {
        audience: Default::default(),
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
                outcome: LlmDoneOutcome::Success { stop_reason, .. },
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
        OPENAI_MODEL,
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
// Run with: ANTHROPIC_API_KEY=... cargo test -p meerkat-client --features
// integration-real-tests --test live_client_provider_matrix structured_output -- --ignored

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
#[ignore = "live provider matrix: needs provider API keys; run by hand with --ignored"]
async fn e2e_anthropic_structured_output() -> Result<(), Box<dyn std::error::Error>> {
    let Some(api_key) = anthropic_api_key() else {
        eprintln!("Skipping: missing ANTHROPIC_API_KEY (or RKAT_ANTHROPIC_API_KEY)");
        return Ok(());
    };
    let client = AnthropicClient::new(api_key)?;
    let request = LlmRequest::new(
        ANTHROPIC_MODEL,
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
#[ignore = "live provider matrix: needs provider API keys; run by hand with --ignored"]
async fn e2e_openai_structured_output() -> Result<(), Box<dyn std::error::Error>> {
    let Some(api_key) = openai_api_key() else {
        eprintln!("Skipping: missing OPENAI_API_KEY (or RKAT_OPENAI_API_KEY)");
        return Ok(());
    };
    let client = OpenAiClient::new(api_key);
    let request = LlmRequest::new(
        OPENAI_MODEL,
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
#[ignore = "live provider matrix: needs provider API keys; run by hand with --ignored"]
async fn e2e_gemini_structured_output() -> Result<(), Box<dyn std::error::Error>> {
    let Some(api_key) = gemini_api_key() else {
        eprintln!("Skipping: missing GOOGLE_API_KEY (or GEMINI_API_KEY/RKAT_GEMINI_API_KEY)");
        return Ok(());
    };
    let client = GeminiClient::new(api_key);
    let request = LlmRequest::new(
        GEMINI_MODEL,
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
#[ignore = "live provider matrix: needs provider API keys; run by hand with --ignored"]
async fn e2e_openai_structured_output_strict_nested_schema()
-> Result<(), Box<dyn std::error::Error>> {
    let Some(api_key) = openai_api_key() else {
        eprintln!("Skipping: missing OPENAI_API_KEY (or RKAT_OPENAI_API_KEY)");
        return Ok(());
    };
    let client = OpenAiClient::new(api_key);
    let request = LlmRequest::new(
        OPENAI_MODEL,
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
#[ignore = "live provider matrix: needs provider API keys; run by hand with --ignored"]
async fn e2e_anthropic_structured_output_strict_nested_schema()
-> Result<(), Box<dyn std::error::Error>> {
    let Some(api_key) = anthropic_api_key() else {
        eprintln!("Skipping: missing ANTHROPIC_API_KEY (or RKAT_ANTHROPIC_API_KEY)");
        return Ok(());
    };
    let client = AnthropicClient::new(api_key)?;
    let request = LlmRequest::new(
        ANTHROPIC_MODEL,
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
#[ignore = "live provider matrix: needs provider API keys; run by hand with --ignored"]
async fn e2e_gemini_structured_output_rich_schema_keywords()
-> Result<(), Box<dyn std::error::Error>> {
    let Some(api_key) = gemini_api_key() else {
        eprintln!("Skipping: missing GOOGLE_API_KEY (or GEMINI_API_KEY/RKAT_GEMINI_API_KEY)");
        return Ok(());
    };
    let client = GeminiClient::new(api_key);
    let request = LlmRequest::new(
        GEMINI_MODEL,
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

//! Structured-output request shapes for the OpenAI-family adapters.
//!
//! The agent loop appends a delimited schema section to the leading system
//! prompt of every request when an output schema is configured, keeps the
//! native schema slot for the extraction request only, and never changes the
//! extraction prompt. These tests compose requests exactly the way the loop
//! does (through `meerkat_core::structured_output`) and pin the lowered wire
//! bodies for the OpenAI Responses API, the ChatGPT Codex backend, and
//! OpenAI-compatible Chat Completions:
//!
//! - main-loop requests carry tools plus the schema section and no native
//!   schema slot;
//! - the section is byte-identical across turns and on the extraction request;
//! - the extraction request carries the native schema slot, non-strict by
//!   default, and no tools;
//! - nothing changes when no schema is configured.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::sync::Arc;

use meerkat_core::lifecycle::run_primitive::{
    OpenAiPromptCacheOptions, OpenAiProviderTag, ProviderTag,
};
use meerkat_core::model_profile::capabilities::OpenAiPromptCacheMode;
use meerkat_core::structured_output::{
    OUTPUT_SCHEMA_INSTRUCTIONS_OPEN, OUTPUT_SCHEMA_INSTRUCTIONS_SEPARATOR,
    project_output_schema_instructions, render_output_schema_instructions,
};
use meerkat_core::{
    AssistantBlock, BlockAssistantMessage, Message, OutputSchema, StopReason, SystemMessage,
    ToolDef, ToolResult, UserMessage,
};
use meerkat_llm_core::{LlmClient, LlmRequest};
use serde_json::{Value, json};

use crate::OpenAiClient;
use crate::client_compatible::{
    OpenAiCompatibleClient, OpenAiCompatibleClientOptions, OpenAiCompatibleMode,
};

pub(crate) const SYSTEM_PROMPT: &str = "You are a careful code reviewer.";
/// The loop's default extraction prompt, verbatim.
pub(crate) const EXTRACTION_PROMPT: &str = "Provide the final output as valid JSON matching the required \
schema. Output ONLY the JSON, no additional text or markdown formatting.";

pub(crate) fn review_schema() -> OutputSchema {
    OutputSchema::new(json!({
        "type": "object",
        "properties": {
            "verdict": {"type": "string", "enum": ["approve", "request_changes"]},
            "comments": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "line": {"type": "integer"},
                        "body": {"type": "string"}
                    },
                    "required": ["path", "line", "body"]
                }
            },
            "summary": {"type": "string"}
        },
        "required": ["verdict", "comments"]
    }))
    .expect("valid schema")
}

pub(crate) fn lookup_tool() -> Arc<ToolDef> {
    Arc::new(ToolDef {
        name: "lookup".into(),
        description: "returns a fixed observation".to_string(),
        input_schema: json!({"type": "object", "properties": {}}),
        provenance: None,
    })
}

/// The three request transcripts of a run that calls one tool, ends with
/// prose, and then needs the extraction request.
pub(crate) struct RunTranscripts {
    pub(crate) turn_one: Vec<Message>,
    pub(crate) turn_two: Vec<Message>,
    pub(crate) extraction: Vec<Message>,
}

pub(crate) fn run_transcripts() -> RunTranscripts {
    let turn_one = vec![
        Message::System(SystemMessage::new(SYSTEM_PROMPT)),
        Message::User(UserMessage::text("review the diff")),
    ];
    let mut turn_two = turn_one.clone();
    turn_two.push(Message::BlockAssistant(BlockAssistantMessage::new(
        vec![AssistantBlock::ToolUse {
            id: "call_1".to_string(),
            name: "lookup".to_string(),
            args: serde_json::value::RawValue::from_string("{}".to_string()).unwrap(),
            meta: None,
        }],
        StopReason::ToolUse,
    )));
    turn_two.push(Message::tool_results(vec![ToolResult::new(
        "call_1".to_string(),
        "observation".to_string(),
        false,
    )]));
    let mut extraction = turn_two.clone();
    extraction.push(Message::BlockAssistant(BlockAssistantMessage::new(
        vec![AssistantBlock::Text {
            text: "I reviewed it.".to_string(),
            meta: None,
        }],
        StopReason::EndTurn,
    )));
    extraction.push(Message::User(UserMessage::text(EXTRACTION_PROMPT)));
    RunTranscripts {
        turn_one,
        turn_two,
        extraction,
    }
}

/// Apply the loop's projection with the section the given client would show.
pub(crate) fn project(
    client: &dyn LlmClient,
    schema: &OutputSchema,
    messages: &[Message],
) -> Vec<Message> {
    let compiled = client.compile_schema(schema).expect("compile");
    let section = render_output_schema_instructions(&compiled.schema);
    let mut projected = messages.to_vec();
    project_output_schema_instructions(&mut projected, &section);
    projected
}

fn expected_system(client: &dyn LlmClient, schema: &OutputSchema) -> String {
    let compiled = client.compile_schema(schema).expect("compile");
    format!(
        "{SYSTEM_PROMPT}\n\n{}",
        render_output_schema_instructions(&compiled.schema)
    )
}

fn main_request(model: &str, messages: Vec<Message>, tag: OpenAiProviderTag) -> LlmRequest {
    let mut request = LlmRequest::new(model, messages).with_tools(vec![lookup_tool()]);
    request.provider_params = Some(ProviderTag::OpenAi(tag));
    request
}

fn extraction_request(
    model: &str,
    messages: Vec<Message>,
    mut tag: OpenAiProviderTag,
    schema: &OutputSchema,
) -> LlmRequest {
    tag.structured_output = Some(schema.clone());
    tag.web_search = None;
    let mut request = LlmRequest::new(model, messages).with_temperature(0.0);
    request.provider_params = Some(ProviderTag::OpenAi(tag));
    request
}

fn explicit_cache_tag() -> OpenAiProviderTag {
    OpenAiProviderTag {
        prompt_cache_enabled: Some(true),
        prompt_cache_key: Some("meerkat:profile:openai:gpt-5.6-luna".to_string()),
        prompt_cache_options: Some(OpenAiPromptCacheOptions {
            mode: Some(OpenAiPromptCacheMode::Explicit),
            ttl: None,
        }),
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// OpenAI Responses
// ---------------------------------------------------------------------------

/// The text of the leading `system` input item, whether lowered as a plain
/// string or as `input_text` parts carrying an explicit cache breakpoint.
fn responses_system_text(body: &Value) -> String {
    let first = &body["input"][0];
    assert_eq!(
        first["role"], "system",
        "system prompt leads the input: {body}"
    );
    match &first["content"] {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part["text"].as_str())
            .collect::<String>(),
        other => panic!("unexpected system content: {other}"),
    }
}

fn responses_tool_names(body: &Value) -> Vec<String> {
    body["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| tool["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn assert_responses_run(client: &OpenAiClient, model: &str, tag: OpenAiProviderTag) {
    let schema = review_schema();
    let transcripts = run_transcripts();
    let expected = expected_system(client, &schema);

    let turn_one = client
        .build_request_body(&main_request(
            model,
            project(client, &schema, &transcripts.turn_one),
            tag.clone(),
        ))
        .expect("turn one body");
    let turn_two = client
        .build_request_body(&main_request(
            model,
            project(client, &schema, &transcripts.turn_two),
            tag.clone(),
        ))
        .expect("turn two body");
    let extraction = client
        .build_request_body(&extraction_request(
            model,
            project(client, &schema, &transcripts.extraction),
            tag,
            &schema,
        ))
        .expect("extraction body");

    for (label, body) in [("turn one", &turn_one), ("turn two", &turn_two)] {
        assert_eq!(responses_system_text(body), expected, "{label}");
        assert_eq!(
            responses_tool_names(body),
            vec!["lookup".to_string()],
            "{label}"
        );
        assert!(
            body["text"].get("format").is_none(),
            "{label}: main turns carry no native schema slot: {body}"
        );
    }
    assert_eq!(
        turn_one["input"][0], turn_two["input"][0],
        "the lowered system item is byte-identical across turns"
    );
    assert_eq!(
        turn_one["input"][0], extraction["input"][0],
        "the extraction request keeps the same system item"
    );

    assert!(extraction.get("tools").is_none(), "no tools on extraction");
    let format = &extraction["text"]["format"];
    assert_eq!(format["type"], "json_schema");
    assert_eq!(format["name"], "output");
    assert_eq!(format["strict"], false, "strict defaults stay false");
    assert_eq!(format["schema"], *schema.schema.as_value());
    let last = extraction["input"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert_eq!(last["role"], "user");
    assert!(
        serde_json::to_string(&last["content"])
            .unwrap()
            .contains(EXTRACTION_PROMPT),
        "the unchanged extraction prompt closes the request: {last}"
    );
}

#[test]
fn responses_main_and_extraction_requests_with_implicit_cache() {
    let client = OpenAiClient::new("test-key".to_string());
    assert_responses_run(&client, "gpt-5.6-luna", OpenAiProviderTag::default());
}

/// GPT-5.6 defaults to explicit prompt-cache breakpoints; the section sits
/// inside the leading system item, before the authored breakpoint.
#[test]
fn responses_main_and_extraction_requests_with_explicit_cache_breakpoints() {
    let client = OpenAiClient::new("test-key".to_string());
    assert_responses_run(&client, "gpt-5.6-luna", explicit_cache_tag());

    let schema = review_schema();
    let body = client
        .build_request_body(&main_request(
            "gpt-5.6-luna",
            project(&client, &schema, &run_transcripts().turn_one),
            explicit_cache_tag(),
        ))
        .unwrap();
    let parts = body["input"][0]["content"].as_array().expect("parts");
    assert_eq!(
        parts.last().unwrap()["prompt_cache_breakpoint"]["mode"],
        "explicit",
        "the breakpoint closes the system prefix, after the section"
    );
}

#[test]
fn responses_strict_schema_is_shown_and_sent_closed() {
    let client = OpenAiClient::new("test-key".to_string());
    let schema = review_schema().strict();
    let projected = project(&client, &schema, &run_transcripts().turn_one);
    let body = client
        .build_request_body(&main_request(
            "gpt-5.6-luna",
            projected.clone(),
            OpenAiProviderTag::default(),
        ))
        .unwrap();
    let system = responses_system_text(&body);
    assert!(
        system.contains(r#""additionalProperties":false"#),
        "an explicitly strict schema is shown as OpenAI compiles it"
    );
    let extraction = client
        .build_request_body(&extraction_request(
            "gpt-5.6-luna",
            projected,
            OpenAiProviderTag::default(),
            &schema,
        ))
        .unwrap();
    assert_eq!(extraction["text"]["format"]["strict"], true);
}

#[test]
fn responses_requests_without_a_schema_are_unchanged() {
    let client = OpenAiClient::new("test-key".to_string());
    let body = client
        .build_request_body(&main_request(
            "gpt-5.6-luna",
            run_transcripts().turn_one,
            OpenAiProviderTag::default(),
        ))
        .unwrap();
    assert_eq!(responses_system_text(&body), SYSTEM_PROMPT);
    assert!(body.get("text").is_none());
    assert!(!body.to_string().contains(OUTPUT_SCHEMA_INSTRUCTIONS_OPEN));
}

// ---------------------------------------------------------------------------
// ChatGPT Codex backend (`instructions`)
// ---------------------------------------------------------------------------

#[test]
fn chatgpt_backend_carries_the_section_in_instructions() {
    let client = OpenAiClient::new("test-key".to_string()).with_chatgpt_backend_wire();
    let schema = review_schema();
    let transcripts = run_transcripts();
    let expected = expected_system(&client, &schema);

    let turn_one = client
        .build_request_body(&main_request(
            "gpt-5.5",
            project(&client, &schema, &transcripts.turn_one),
            OpenAiProviderTag::default(),
        ))
        .expect("turn one");
    let turn_two = client
        .build_request_body(&main_request(
            "gpt-5.5",
            project(&client, &schema, &transcripts.turn_two),
            OpenAiProviderTag::default(),
        ))
        .expect("turn two");
    let extraction = client
        .build_request_body(&extraction_request(
            "gpt-5.5",
            project(&client, &schema, &transcripts.extraction),
            OpenAiProviderTag::default(),
            &schema,
        ))
        .expect("the single merged system prompt stays representable on ChatGPT");

    for body in [&turn_one, &turn_two, &extraction] {
        assert_eq!(body["instructions"], Value::String(expected.clone()));
        assert!(
            body["input"]
                .as_array()
                .unwrap()
                .iter()
                .all(|item| item["role"] != "system"),
            "ChatGPT carries the system prompt only in instructions"
        );
    }
    for (label, body) in [("turn one", &turn_one), ("turn two", &turn_two)] {
        assert_eq!(
            responses_tool_names(body),
            vec!["lookup".to_string()],
            "{label}"
        );
        assert!(
            body["text"].get("format").is_none(),
            "{label}: main turns carry no native schema slot: {body}"
        );
    }
    assert_eq!(
        extraction["tools"],
        json!([]),
        "the ChatGPT wire always sends a tools array; extraction leaves it empty"
    );
    assert_eq!(extraction["text"]["format"]["type"], "json_schema");
    assert_eq!(extraction["text"]["format"]["strict"], false);
    assert_eq!(
        extraction["text"]["format"]["schema"],
        *schema.schema.as_value()
    );
    let last = extraction["input"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert_eq!(last["role"], "user");
    assert!(
        serde_json::to_string(&last["content"])
            .unwrap()
            .contains(EXTRACTION_PROMPT),
        "the unchanged extraction prompt closes the request: {last}"
    );
}

#[test]
fn chatgpt_backend_requests_without_a_schema_are_unchanged() {
    let client = OpenAiClient::new("test-key".to_string()).with_chatgpt_backend_wire();
    let body = client
        .build_request_body(&main_request(
            "gpt-5.5",
            run_transcripts().turn_one,
            OpenAiProviderTag::default(),
        ))
        .unwrap();
    assert_eq!(
        body["instructions"],
        Value::String(SYSTEM_PROMPT.to_string())
    );
    assert!(
        body.get("text")
            .is_none_or(|text| text.get("format").is_none())
    );
    assert!(!body.to_string().contains(OUTPUT_SCHEMA_INSTRUCTIONS_OPEN));
}

/// ChatGPT rejects a second leading system row, which is why the projection
/// merges into the existing prompt instead of adding one.
#[test]
fn chatgpt_backend_accepts_the_projection_when_no_prompt_exists() {
    let client = OpenAiClient::new("test-key".to_string()).with_chatgpt_backend_wire();
    let schema = review_schema();
    let messages = vec![Message::User(UserMessage::text("review"))];
    let body = client
        .build_request_body(&main_request(
            "gpt-5.5",
            project(&client, &schema, &messages),
            OpenAiProviderTag::default(),
        ))
        .expect("an inserted prompt is the single leading system row");
    let compiled = client.compile_schema(&schema).unwrap();
    assert_eq!(
        body["instructions"],
        Value::String(render_output_schema_instructions(&compiled.schema))
    );
}

// ---------------------------------------------------------------------------
// OpenAI-compatible Chat Completions (self-hosted, Copilot chat lowering)
// ---------------------------------------------------------------------------

fn compatible_client() -> OpenAiCompatibleClient {
    OpenAiCompatibleClient::new_with_options(
        OpenAiCompatibleMode::ChatCompletions,
        "remote-model".to_string(),
        "https://example.test".to_string(),
        None,
        OpenAiCompatibleClientOptions {
            supports_temperature: true,
            supports_thinking: false,
            supports_reasoning: false,
            supports_image_tool_results: false,
        },
    )
}

fn chat_system_text(body: &Value) -> String {
    let first = &body["messages"][0];
    assert_eq!(first["role"], "system", "{body}");
    match &first["content"] {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part["text"].as_str())
            .collect::<String>(),
        other => panic!("unexpected system content: {other}"),
    }
}

#[test]
fn chat_completions_main_and_extraction_requests() {
    let client = compatible_client();
    let schema = review_schema();
    let transcripts = run_transcripts();
    let expected = expected_system(&client, &schema);

    let turn_one = client
        .build_chat_completions_body(&main_request(
            "catalog-model",
            project(&client, &schema, &transcripts.turn_one),
            OpenAiProviderTag::default(),
        ))
        .expect("turn one");
    let turn_two = client
        .build_chat_completions_body(&main_request(
            "catalog-model",
            project(&client, &schema, &transcripts.turn_two),
            OpenAiProviderTag::default(),
        ))
        .expect("turn two");
    let extraction = client
        .build_chat_completions_body(&extraction_request(
            "catalog-model",
            project(&client, &schema, &transcripts.extraction),
            OpenAiProviderTag::default(),
            &schema,
        ))
        .expect("extraction");

    for (label, body) in [("turn one", &turn_one), ("turn two", &turn_two)] {
        assert_eq!(chat_system_text(body), expected, "{label}");
        assert_eq!(body["tools"][0]["function"]["name"], "lookup", "{label}");
        assert!(body.get("response_format").is_none(), "{label}");
    }
    assert_eq!(turn_one["messages"][0], turn_two["messages"][0]);
    assert_eq!(turn_one["messages"][0], extraction["messages"][0]);

    assert!(extraction.get("tools").is_none());
    assert_eq!(extraction["temperature"], 0.0);
    let format = &extraction["response_format"];
    assert_eq!(format["type"], "json_schema");
    assert_eq!(format["json_schema"]["strict"], false);
    assert_eq!(format["json_schema"]["schema"], *schema.schema.as_value());
    let last = extraction["messages"].as_array().unwrap().last().unwrap();
    assert_eq!(last["role"], "user");
    assert_eq!(last["content"], EXTRACTION_PROMPT);
}

#[test]
fn chat_completions_with_explicit_cache_keeps_the_section_before_the_breakpoint() {
    let client = compatible_client();
    let schema = review_schema();
    let body = client
        .build_chat_completions_body(&main_request(
            "catalog-model",
            project(&client, &schema, &run_transcripts().turn_one),
            explicit_cache_tag(),
        ))
        .unwrap();
    assert_eq!(chat_system_text(&body), expected_system(&client, &schema));
    // The adapter authors breakpoints on conversation rows, never on the
    // system row, so the section is part of the prefix the first breakpoint
    // closes.
    assert!(
        body["messages"][0]["content"].is_string(),
        "the system row carries no breakpoint of its own: {body}"
    );
    let first_breakpoint = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .position(|message| {
            message["content"].as_array().is_some_and(|parts| {
                parts
                    .iter()
                    .any(|part| part["prompt_cache_breakpoint"]["mode"] == "explicit")
            })
        })
        .expect("explicit mode authors a breakpoint");
    assert!(
        first_breakpoint > 0,
        "the first breakpoint comes after the system row that carries the section"
    );
    assert_eq!(body["messages"][first_breakpoint]["role"], "user");
}

#[test]
fn chat_completions_requests_without_a_schema_are_unchanged() {
    let client = compatible_client();
    let body = client
        .build_chat_completions_body(&main_request(
            "catalog-model",
            run_transcripts().turn_one,
            OpenAiProviderTag::default(),
        ))
        .unwrap();
    assert_eq!(chat_system_text(&body), SYSTEM_PROMPT);
    assert!(body.get("response_format").is_none());
}

// ---------------------------------------------------------------------------
// Comparison with the requests main sends (no section)
// ---------------------------------------------------------------------------

/// The section the loop appends for `schema` when `client` is active.
pub(crate) fn section_for(client: &dyn LlmClient, schema: &OutputSchema) -> String {
    render_output_schema_instructions(&client.compile_schema(schema).expect("compile").schema)
}

/// Copy of `value` with every appended section removed from its strings,
/// counting how many times the section occurred.
fn strip_appended_section(value: &Value, section: &str, hits: &mut usize) -> Value {
    match value {
        Value::String(text) => {
            *hits += text.matches(section).count();
            Value::String(text.replace(
                &format!("{OUTPUT_SCHEMA_INSTRUCTIONS_SEPARATOR}{section}"),
                "",
            ))
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| strip_appended_section(item, section, hits))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| (key.clone(), strip_appended_section(item, section, hits)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// The request built with a schema carries the section exactly once, and
/// with the section removed it is byte-for-byte the request built from the
/// same transcript without a schema (what main sends for that request).
pub(crate) fn assert_only_the_section_differs(
    label: &str,
    with_schema: &Value,
    without_schema: &Value,
    section: &str,
) {
    let mut hits = 0;
    let stripped = strip_appended_section(with_schema, section, &mut hits);
    assert_eq!(
        hits, 1,
        "{label}: the section appears exactly once: {with_schema}"
    );
    assert_eq!(
        with_schema
            .to_string()
            .matches(OUTPUT_SCHEMA_INSTRUCTIONS_OPEN)
            .count(),
        1,
        "{label}: exactly one opening delimiter in the whole request"
    );
    assert!(
        !without_schema
            .to_string()
            .contains(OUTPUT_SCHEMA_INSTRUCTIONS_OPEN),
        "{label}: no section without a schema"
    );
    assert_eq!(
        &stripped, without_schema,
        "{label}: apart from the section the request is unchanged"
    );
}

/// The three requests of a run (turn one with tools, turn two after the tool
/// result, the extraction request with the native slot), with or without the
/// section, for a provider tag of the given family.
pub(crate) fn run_requests(
    client: &dyn LlmClient,
    model: &str,
    schema: &OutputSchema,
    tag: &OpenAiProviderTag,
    project_section: bool,
) -> Vec<(&'static str, LlmRequest)> {
    let transcripts = run_transcripts();
    let messages = |messages: &[Message]| {
        if project_section {
            project(client, schema, messages)
        } else {
            messages.to_vec()
        }
    };
    vec![
        (
            "turn one",
            main_request(model, messages(&transcripts.turn_one), tag.clone()),
        ),
        (
            "turn two",
            main_request(model, messages(&transcripts.turn_two), tag.clone()),
        ),
        (
            "extraction",
            extraction_request(
                model,
                messages(&transcripts.extraction),
                tag.clone(),
                schema,
            ),
        ),
    ]
}

// ---------------------------------------------------------------------------
// GitHub Copilot routes (recorded request bodies)
// ---------------------------------------------------------------------------

#[cfg(feature = "copilot")]
mod copilot_routes {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use axum::{
        Json, Router, extract::State, http::HeaderMap, response::IntoResponse, routing::post,
    };
    use futures::StreamExt;
    use meerkat_copilot::CopilotChatCompletionsClientFactory;
    use meerkat_core::Provider;
    use tokio::net::TcpListener;

    use super::*;

    const RESPONSES_SSE: &str = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",",
        "\"usage\":{\"input_tokens\":4,\"output_tokens\":1}}}\n",
        "data: [DONE]\n",
    );
    const CHAT_SSE: &str = concat!(
        "data: {\"id\":\"c1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"},",
        "\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"c1\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],",
        "\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":1}}\n\n",
        "data: [DONE]\n\n",
    );

    struct Recorded {
        headers: BTreeMap<String, String>,
        body: Value,
    }

    #[derive(Clone)]
    struct CaptureState {
        captures: Arc<Mutex<Vec<Recorded>>>,
        payload: &'static str,
    }

    async fn capture(
        State(state): State<CaptureState>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> impl IntoResponse {
        let headers = headers
            .iter()
            .filter(|(name, _)| !matches!(name.as_str(), "content-length" | "host"))
            .map(|(name, value)| {
                (
                    name.as_str().to_string(),
                    value.to_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        state
            .captures
            .lock()
            .expect("capture lock")
            .push(Recorded { headers, body });
        ([("content-type", "text/event-stream")], state.payload)
    }

    struct CopilotLabelledAuthorizer;

    #[async_trait::async_trait]
    impl meerkat_core::HttpAuthorizer for CopilotLabelledAuthorizer {
        async fn authorize(
            &self,
            request: &mut meerkat_core::HttpAuthorizationRequest<'_>,
        ) -> Result<(), meerkat_core::AuthError> {
            request.headers.push((
                "Authorization".to_string(),
                "Bearer copilot-token".to_string(),
            ));
            Ok(())
        }

        fn label(&self) -> &str {
            meerkat_copilot::GITHUB_COPILOT_AUTHORIZER_LABEL
        }
    }

    /// Stream every request through `client` against a local Copilot API
    /// base serving `path` and return the bodies and headers it sent.
    async fn record(
        path: &str,
        payload: &'static str,
        build: &dyn Fn(String) -> Arc<dyn LlmClient>,
        requests: &[(&'static str, LlmRequest)],
    ) -> Vec<Recorded> {
        let captures = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route(path, post(capture))
            .with_state(CaptureState {
                captures: Arc::clone(&captures),
                payload,
            });
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let client = build(format!("http://{addr}"));
        for (label, request) in requests {
            let mut stream = client.stream(request);
            while let Some(event) = stream.next().await {
                event.unwrap_or_else(|error| panic!("{label}: stream failed: {error}"));
            }
        }
        server.abort();
        let recorded = std::mem::take(&mut *captures.lock().expect("capture lock"));
        assert_eq!(recorded.len(), requests.len(), "one request sent per call");
        recorded
    }

    /// What the OpenAI Copilot route factory builds for a `/responses` model.
    fn copilot_responses_client(api_base: String) -> Arc<dyn LlmClient> {
        Arc::new(
            OpenAiClient::new_with_optional_api_key_and_base_url(None, api_base)
                .with_image_input_support(true)
                .with_authorizer(Arc::new(CopilotLabelledAuthorizer))
                .with_responses_path("responses"),
        )
    }

    /// What the OpenAI Copilot route factory builds for a `/chat/completions`
    /// model.
    fn copilot_openai_chat_client(api_base: String) -> Arc<dyn LlmClient> {
        Arc::new(
            OpenAiCompatibleClient::new_with_options(
                OpenAiCompatibleMode::ChatCompletions,
                "gpt-test".to_string(),
                api_base,
                None,
                OpenAiCompatibleClientOptions {
                    supports_temperature: true,
                    supports_thinking: true,
                    supports_reasoning: true,
                    supports_image_tool_results: false,
                },
            )
            .with_image_input_support(true)
            .with_authorizer(Arc::new(CopilotLabelledAuthorizer))
            .with_provider(Provider::OpenAI),
        )
    }

    /// The Chat Completions client the Gemini Copilot route gets from the
    /// shared factory (the Gemini adapter wraps it and lowers its Gemini tag
    /// to the OpenAI tag used below).
    fn copilot_gemini_chat_client(api_base: String) -> Arc<dyn LlmClient> {
        crate::OpenAiCopilotChatCompletionsClientFactory
            .build(meerkat_copilot::CopilotChatCompletionsClientSpec::new(
                Provider::Gemini,
                "gemini-test".to_string(),
                api_base,
                Arc::new(CopilotLabelledAuthorizer),
                true,
                true,
                true,
                false,
            ))
            .expect("factory client")
    }

    struct RouteRun {
        section: String,
        with_schema: Vec<Recorded>,
    }

    /// Record one run with and one without the section and check the common
    /// guarantees: the section exactly once per request, apart from it every
    /// body and header is what main sends, and the extraction request keeps
    /// its native schema slot.
    async fn assert_route_run(
        path: &str,
        payload: &'static str,
        build: &dyn Fn(String) -> Arc<dyn LlmClient>,
        model: &str,
        tag: OpenAiProviderTag,
    ) -> RouteRun {
        let schema = review_schema();
        let probe = build("http://127.0.0.1:9".to_string());
        let section = section_for(probe.as_ref(), &schema);
        let with_requests = run_requests(probe.as_ref(), model, &schema, &tag, true);
        let without_requests = run_requests(probe.as_ref(), model, &schema, &tag, false);
        let with_schema = record(path, payload, build, &with_requests).await;
        let without_schema = record(path, payload, build, &without_requests).await;
        for ((label, _), (with, without)) in with_requests
            .iter()
            .zip(with_schema.iter().zip(&without_schema))
        {
            assert_only_the_section_differs(label, &with.body, &without.body, &section);
            assert_eq!(
                with.headers, without.headers,
                "{label}: the section changes no header"
            );
            assert_eq!(
                with.headers.get("authorization").map(String::as_str),
                Some("Bearer copilot-token"),
                "{label}: the Copilot authorizer signed the request"
            );
        }
        RouteRun {
            section,
            with_schema,
        }
    }

    #[tokio::test]
    async fn copilot_responses_route_requests_carry_the_section_once() {
        for tag in [OpenAiProviderTag::default(), explicit_cache_tag()] {
            let run = assert_route_run(
                "/responses",
                RESPONSES_SSE,
                &copilot_responses_client,
                "gpt-5.6-luna",
                tag,
            )
            .await;
            let bodies: Vec<&Value> = run.with_schema.iter().map(|r| &r.body).collect();
            assert_eq!(
                responses_system_text(bodies[0]),
                format!("{SYSTEM_PROMPT}\n\n{}", run.section)
            );
            assert_eq!(
                bodies[0]["input"][0], bodies[1]["input"][0],
                "stable across turns"
            );
            assert_eq!(
                bodies[0]["input"][0], bodies[2]["input"][0],
                "kept on extraction"
            );
            for body in &bodies[..2] {
                assert_eq!(responses_tool_names(body), vec!["lookup".to_string()]);
                assert!(body["text"].get("format").is_none());
            }
            let extraction = bodies[2];
            assert!(extraction.get("tools").is_none(), "no tools on extraction");
            assert_eq!(extraction["text"]["format"]["type"], "json_schema");
            assert_eq!(extraction["text"]["format"]["strict"], false);
            assert_eq!(
                extraction["text"]["format"]["schema"],
                *review_schema().schema.as_value()
            );
        }
    }

    fn assert_chat_route(run: &RouteRun) {
        let bodies: Vec<&Value> = run.with_schema.iter().map(|r| &r.body).collect();
        assert_eq!(
            chat_system_text(bodies[0]),
            format!("{SYSTEM_PROMPT}\n\n{}", run.section)
        );
        assert_eq!(
            bodies[0]["messages"][0], bodies[1]["messages"][0],
            "stable across turns"
        );
        assert_eq!(
            bodies[0]["messages"][0], bodies[2]["messages"][0],
            "kept on extraction"
        );
        for body in &bodies[..2] {
            assert_eq!(body["tools"][0]["function"]["name"], "lookup");
            assert!(body.get("response_format").is_none());
        }
        let extraction = bodies[2];
        assert!(extraction.get("tools").is_none(), "no tools on extraction");
        assert_eq!(extraction["response_format"]["type"], "json_schema");
        assert_eq!(
            extraction["response_format"]["json_schema"]["strict"],
            false
        );
        assert_eq!(
            extraction["response_format"]["json_schema"]["schema"],
            *review_schema().schema.as_value()
        );
        let last = extraction["messages"].as_array().unwrap().last().unwrap();
        assert_eq!(last["content"], EXTRACTION_PROMPT);
    }

    #[tokio::test]
    async fn copilot_openai_chat_completions_route_requests_carry_the_section_once() {
        let run = assert_route_run(
            "/chat/completions",
            CHAT_SSE,
            &copilot_openai_chat_client,
            "gpt-test",
            OpenAiProviderTag::default(),
        )
        .await;
        assert_chat_route(&run);
    }

    /// The Gemini Copilot route: the shared Chat Completions factory client,
    /// driven with the OpenAI tag the Gemini adapter lowers to (default on
    /// main turns, `structured_output` on extraction).
    #[tokio::test]
    async fn copilot_gemini_chat_completions_route_requests_carry_the_section_once() {
        let run = assert_route_run(
            "/chat/completions",
            CHAT_SSE,
            &copilot_gemini_chat_client,
            "gemini-test",
            OpenAiProviderTag::default(),
        )
        .await;
        assert_chat_route(&run);
        assert_eq!(run.with_schema[0].body["model"], "gemini-test");
    }
}

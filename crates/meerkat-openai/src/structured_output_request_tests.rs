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
    OUTPUT_SCHEMA_INSTRUCTIONS_OPEN, project_output_schema_instructions,
    render_output_schema_instructions,
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

const SYSTEM_PROMPT: &str = "You are a careful code reviewer.";
/// The loop's default extraction prompt, verbatim.
const EXTRACTION_PROMPT: &str = "Provide the final output as valid JSON matching the required \
schema. Output ONLY the JSON, no additional text or markdown formatting.";

fn review_schema() -> OutputSchema {
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

fn lookup_tool() -> Arc<ToolDef> {
    Arc::new(ToolDef {
        name: "lookup".into(),
        description: "returns a fixed observation".to_string(),
        input_schema: json!({"type": "object", "properties": {}}),
        provenance: None,
    })
}

/// The three request transcripts of a run that calls one tool, ends with
/// prose, and then needs the extraction request.
struct RunTranscripts {
    turn_one: Vec<Message>,
    turn_two: Vec<Message>,
    extraction: Vec<Message>,
}

fn run_transcripts() -> RunTranscripts {
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
fn project(client: &dyn LlmClient, schema: &OutputSchema, messages: &[Message]) -> Vec<Message> {
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
    assert_eq!(responses_tool_names(&turn_one), vec!["lookup".to_string()]);
    assert!(turn_one["text"].get("format").is_none());
    assert_eq!(extraction["text"]["format"]["type"], "json_schema");
    assert_eq!(extraction["text"]["format"]["strict"], false);
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

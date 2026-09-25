//! Structured-output request shapes for the Anthropic Messages adapter.
//!
//! The agent loop appends a delimited schema section to the leading system
//! prompt of every request when an output schema is configured, keeps the
//! native schema slot (`output_config.format`) for the extraction request
//! only, and never changes the extraction prompt. These tests compose
//! requests exactly the way the loop does (through
//! `meerkat_core::structured_output`) and pin the lowered Messages bodies:
//!
//! - main-loop requests carry tools plus the section in `system` and no
//!   `output_config.format`;
//! - the lowered `system` value is byte-identical across turns and on the
//!   extraction request, under both the default and the explicit
//!   system-prefix cache policies;
//! - the extraction request carries the closed-object schema Anthropic
//!   validates against, and no tools;
//! - nothing changes when no schema is configured.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::sync::Arc;

use meerkat_core::lifecycle::run_primitive::{
    AnthropicCacheControlPolicy, AnthropicProviderTag, ProviderTag,
};
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

use crate::AnthropicClient;

const MODEL: &str = "claude-sonnet-4-6";
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
            id: "toolu_1".to_string(),
            name: "lookup".to_string(),
            args: serde_json::value::RawValue::from_string("{}".to_string()).unwrap(),
            meta: None,
        }],
        StopReason::ToolUse,
    )));
    turn_two.push(Message::tool_results(vec![ToolResult::new(
        "toolu_1".to_string(),
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

fn project(client: &AnthropicClient, schema: &OutputSchema, messages: &[Message]) -> Vec<Message> {
    let compiled = client.compile_schema(schema).expect("compile");
    let section = render_output_schema_instructions(&compiled.schema);
    let mut projected = messages.to_vec();
    project_output_schema_instructions(&mut projected, &section);
    projected
}

fn expected_system(client: &AnthropicClient, schema: &OutputSchema) -> String {
    let compiled = client.compile_schema(schema).expect("compile");
    format!(
        "{SYSTEM_PROMPT}\n\n{}",
        render_output_schema_instructions(&compiled.schema)
    )
}

fn main_request(messages: Vec<Message>, tag: AnthropicProviderTag) -> LlmRequest {
    let mut request = LlmRequest::new(MODEL, messages).with_tools(vec![lookup_tool()]);
    request.provider_params = Some(ProviderTag::Anthropic(tag));
    request
}

fn extraction_request(
    messages: Vec<Message>,
    mut tag: AnthropicProviderTag,
    schema: &OutputSchema,
) -> LlmRequest {
    tag.structured_output = Some(schema.clone());
    tag.web_search = None;
    let mut request = LlmRequest::new(MODEL, messages).with_temperature(0.0);
    request.provider_params = Some(ProviderTag::Anthropic(tag));
    request
}

fn system_text(body: &Value) -> String {
    match &body["system"] {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| block["text"].as_str())
            .collect::<String>(),
        other => panic!("unexpected system value: {other}"),
    }
}

fn tool_names(body: &Value) -> Vec<String> {
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

fn assert_messages_run(tag: AnthropicProviderTag) -> Value {
    let client = AnthropicClient::new("test-key".to_string()).expect("client");
    let schema = review_schema();
    let transcripts = run_transcripts();
    let expected = expected_system(&client, &schema);

    let turn_one = client
        .build_request_body(&main_request(
            project(&client, &schema, &transcripts.turn_one),
            tag.clone(),
        ))
        .expect("turn one");
    let turn_two = client
        .build_request_body(&main_request(
            project(&client, &schema, &transcripts.turn_two),
            tag.clone(),
        ))
        .expect("turn two");
    let extraction = client
        .build_request_body(&extraction_request(
            project(&client, &schema, &transcripts.extraction),
            tag,
            &schema,
        ))
        .expect("extraction");

    for (label, body) in [("turn one", &turn_one), ("turn two", &turn_two)] {
        assert_eq!(system_text(body), expected, "{label}");
        assert_eq!(tool_names(body), vec!["lookup".to_string()], "{label}");
        assert!(
            body.get("output_config")
                .and_then(|config| config.get("format"))
                .is_none(),
            "{label}: main turns carry no native schema slot: {body}"
        );
    }
    assert_eq!(
        turn_one["system"], turn_two["system"],
        "the lowered system value is byte-identical across turns"
    );
    assert_eq!(
        turn_one["system"], extraction["system"],
        "the extraction request keeps the same system value"
    );

    assert!(tool_names(&extraction).is_empty(), "no tools on extraction");
    let format = &extraction["output_config"]["format"];
    assert_eq!(format["type"], "json_schema");
    let compiled = client.compile_schema(&schema).unwrap();
    assert_eq!(format["schema"], compiled.schema);
    assert_eq!(
        format["schema"]["additionalProperties"],
        json!(false),
        "Anthropic always sends closed objects"
    );
    let last = extraction["messages"].as_array().unwrap().last().unwrap();
    assert_eq!(last["role"], "user");
    assert!(
        last.to_string().contains(EXTRACTION_PROMPT),
        "the unchanged extraction prompt closes the request: {last}"
    );
    turn_one
}

#[test]
fn messages_main_and_extraction_requests_with_default_cache_policy() {
    let turn_one = assert_messages_run(AnthropicProviderTag::default());
    assert!(
        turn_one["system"].is_string(),
        "one system prompt lowers to a plain string under the default policy"
    );
}

#[test]
fn messages_main_and_extraction_requests_with_system_prefix_cache() {
    let turn_one = assert_messages_run(AnthropicProviderTag {
        cache_control: Some(AnthropicCacheControlPolicy::SystemPrefix),
        ..Default::default()
    });
    let blocks = turn_one["system"].as_array().expect("system blocks");
    assert_eq!(
        blocks.len(),
        1,
        "the section merges into the one system block"
    );
    assert_eq!(blocks[0]["cache_control"]["type"], "ephemeral");
}

#[test]
fn messages_shows_the_closed_schema_it_validates_against() {
    let client = AnthropicClient::new("test-key".to_string()).expect("client");
    let schema = review_schema();
    let expected = expected_system(&client, &schema);
    assert!(expected.contains(r#""additionalProperties":false"#));
    assert!(
        !schema
            .schema
            .as_value()
            .to_string()
            .contains("additionalProperties")
    );
}

#[test]
fn messages_requests_without_a_schema_are_unchanged() {
    let client = AnthropicClient::new("test-key".to_string()).expect("client");
    let body = client
        .build_request_body(&main_request(
            run_transcripts().turn_one,
            AnthropicProviderTag::default(),
        ))
        .unwrap();
    assert_eq!(body["system"], SYSTEM_PROMPT);
    assert!(body.get("output_config").is_none());
    assert!(!body.to_string().contains(OUTPUT_SCHEMA_INSTRUCTIONS_OPEN));
}

#[test]
fn messages_inserted_section_becomes_the_system_prompt() {
    let client = AnthropicClient::new("test-key".to_string()).expect("client");
    let schema = review_schema();
    let messages = vec![Message::User(UserMessage::text("review"))];
    let body = client
        .build_request_body(&main_request(
            project(&client, &schema, &messages),
            AnthropicProviderTag::default(),
        ))
        .unwrap();
    let compiled = client.compile_schema(&schema).unwrap();
    assert_eq!(
        body["system"],
        Value::String(render_output_schema_instructions(&compiled.schema))
    );
    assert_eq!(body["messages"][0]["role"], "user");
}

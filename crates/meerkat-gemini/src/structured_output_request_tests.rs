//! Structured-output request shapes for the Gemini GenerateContent adapter.
//!
//! The agent loop appends a delimited schema section to the leading system
//! prompt of every request when an output schema is configured, keeps the
//! native schema slot (`generationConfig.responseJsonSchema`) for the
//! extraction request only, and never changes the extraction prompt. These
//! tests compose requests exactly the way the loop does (through
//! `meerkat_core::structured_output`) and pin the lowered bodies:
//!
//! - main-loop requests carry function declarations plus the section in
//!   `systemInstruction` and no response schema;
//! - `systemInstruction` is byte-identical across turns and on the extraction
//!   request;
//! - the extraction request carries `responseMimeType` and the schema, and no
//!   tools;
//! - nothing changes when no schema is configured.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::sync::Arc;

use meerkat_core::lifecycle::run_primitive::{GeminiProviderTag, ProviderTag};
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

use crate::GeminiClient;

const MODEL: &str = "gemini-3.5-flash";
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

fn project(client: &GeminiClient, schema: &OutputSchema, messages: &[Message]) -> Vec<Message> {
    let compiled = client.compile_schema(schema).expect("compile");
    let section = render_output_schema_instructions(&compiled.schema);
    let mut projected = messages.to_vec();
    project_output_schema_instructions(&mut projected, &section);
    projected
}

fn main_request(messages: Vec<Message>) -> LlmRequest {
    let mut request = LlmRequest::new(MODEL, messages).with_tools(vec![lookup_tool()]);
    request.provider_params = Some(ProviderTag::Gemini(GeminiProviderTag::default()));
    request
}

fn extraction_request(messages: Vec<Message>, schema: &OutputSchema) -> LlmRequest {
    let mut request = LlmRequest::new(MODEL, messages).with_temperature(0.0);
    request.provider_params = Some(ProviderTag::Gemini(GeminiProviderTag {
        structured_output: Some(schema.clone()),
        ..Default::default()
    }));
    request
}

fn system_instruction_text(body: &Value) -> String {
    body["systemInstruction"]["parts"]
        .as_array()
        .expect("systemInstruction parts")
        .iter()
        .filter_map(|part| part["text"].as_str())
        .collect()
}

fn function_names(body: &Value) -> Vec<String> {
    body["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|tool| tool["functionDeclarations"].as_array())
        .flatten()
        .filter_map(|decl| decl["name"].as_str().map(str::to_string))
        .collect()
}

#[test]
fn generate_content_main_and_extraction_requests() {
    let client = GeminiClient::new("test-key".to_string());
    let schema = review_schema();
    let transcripts = run_transcripts();
    let compiled = client.compile_schema(&schema).unwrap();
    let expected = format!(
        "{SYSTEM_PROMPT}\n\n{}",
        render_output_schema_instructions(&compiled.schema)
    );

    let turn_one = client
        .build_request_body(&main_request(project(
            &client,
            &schema,
            &transcripts.turn_one,
        )))
        .expect("turn one");
    let turn_two = client
        .build_request_body(&main_request(project(
            &client,
            &schema,
            &transcripts.turn_two,
        )))
        .expect("turn two");
    let extraction = client
        .build_request_body(&extraction_request(
            project(&client, &schema, &transcripts.extraction),
            &schema,
        ))
        .expect("extraction");

    for (label, body) in [("turn one", &turn_one), ("turn two", &turn_two)] {
        assert_eq!(system_instruction_text(body), expected, "{label}");
        assert_eq!(
            body["systemInstruction"]["parts"].as_array().unwrap().len(),
            1,
            "{label}: the section merges into the one system part"
        );
        assert_eq!(function_names(body), vec!["lookup".to_string()], "{label}");
        assert!(
            body["generationConfig"].get("responseJsonSchema").is_none(),
            "{label}: main turns carry no response schema"
        );
        assert!(body["generationConfig"].get("responseMimeType").is_none());
    }
    assert_eq!(
        turn_one["systemInstruction"], turn_two["systemInstruction"],
        "systemInstruction is byte-identical across turns"
    );
    assert_eq!(
        turn_one["systemInstruction"], extraction["systemInstruction"],
        "the extraction request keeps the same systemInstruction"
    );

    assert!(
        function_names(&extraction).is_empty(),
        "no tools on extraction"
    );
    assert_eq!(
        extraction["generationConfig"]["responseMimeType"],
        "application/json"
    );
    assert_eq!(
        extraction["generationConfig"]["responseJsonSchema"],
        *schema.schema.as_value(),
        "Gemini sends the schema without destructive lowering"
    );
    assert_eq!(extraction["generationConfig"]["temperature"], 0.0);
    let last = extraction["contents"].as_array().unwrap().last().unwrap();
    assert_eq!(last["role"], "user");
    assert_eq!(last["parts"][0]["text"], EXTRACTION_PROMPT);
}

#[test]
fn generate_content_requests_without_a_schema_are_unchanged() {
    let client = GeminiClient::new("test-key".to_string());
    let body = client
        .build_request_body(&main_request(run_transcripts().turn_one))
        .unwrap();
    assert_eq!(system_instruction_text(&body), SYSTEM_PROMPT);
    assert!(body["generationConfig"].get("responseJsonSchema").is_none());
    assert!(!body.to_string().contains(OUTPUT_SCHEMA_INSTRUCTIONS_OPEN));
}

#[test]
fn generate_content_inserted_section_forms_the_leading_system_prefix() {
    let client = GeminiClient::new("test-key".to_string());
    let schema = review_schema();
    let messages = vec![Message::User(UserMessage::text("review"))];
    let body = client
        .build_request_body(&main_request(project(&client, &schema, &messages)))
        .expect("an inserted prompt is a valid leading system prefix");
    let compiled = client.compile_schema(&schema).unwrap();
    assert_eq!(
        system_instruction_text(&body),
        render_output_schema_instructions(&compiled.schema)
    );
    assert_eq!(body["contents"][0]["role"], "user");
}

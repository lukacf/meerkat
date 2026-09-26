//! Structured-output requests on the Gemini GitHub Copilot route.
//!
//! The route wraps the shared Copilot Chat Completions client in
//! `GeminiCopilotChatClient`, which lowers the Gemini provider tag to the
//! OpenAI tag the Chat Completions wire understands and passes everything
//! else through. These tests drive the exact requests the agent loop sends
//! (turn one with tools, turn two after the tool result, the extraction
//! request with the native schema slot) through the wrapper and capture what
//! reaches the Chat Completions client:
//!
//! - the messages, including the schema section in the leading system
//!   prompt, arrive byte-for-byte as the loop composed them;
//! - the extraction request's native schema moves to the OpenAI
//!   `structured_output` slot (which the Chat Completions client lowers to
//!   `response_format`, pinned in `meerkat-openai`), main turns carry none;
//! - with and without a schema the lowered requests differ only by the
//!   section;
//! - the section shows the schema the Chat Completions client compiles.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use meerkat_core::lifecycle::run_primitive::{GeminiProviderTag, OpenAiProviderTag, ProviderTag};
use meerkat_core::schema::{CompiledSchema, SchemaError};
use meerkat_core::structured_output::{
    OUTPUT_SCHEMA_INSTRUCTIONS_OPEN, OUTPUT_SCHEMA_INSTRUCTIONS_SEPARATOR,
    project_output_schema_instructions, render_output_schema_instructions,
};
use meerkat_core::{
    AssistantBlock, BlockAssistantMessage, Message, OutputSchema, Provider, StopReason,
    SystemMessage, ToolDef, ToolResult, UserMessage,
};
use meerkat_llm_core::{LlmClient, LlmError, LlmRequest, LlmStream};
use serde_json::{Value, json};

use super::GeminiCopilotChatClient;

const MODEL: &str = "gemini-test";
const SYSTEM_PROMPT: &str = "You are a careful code reviewer.";
/// The loop's default extraction prompt, verbatim.
const EXTRACTION_PROMPT: &str = "Provide the final output as valid JSON matching the required \
schema. Output ONLY the JSON, no additional text or markdown formatting.";

/// Stands in for the shared Copilot Chat Completions client: records every
/// request it is asked to stream and compiles schemas the way the
/// OpenAI-compatible client does for a non-strict schema (unchanged), plus a
/// marker so the test can tell whose compilation the section shows.
struct RecordingChatClient {
    seen: Arc<Mutex<Vec<LlmRequest>>>,
}

#[async_trait]
impl LlmClient for RecordingChatClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        self.seen.lock().expect("seen lock").push(request.clone());
        Box::pin(futures::stream::empty())
    }

    fn provider(&self) -> Provider {
        Provider::Gemini
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }

    fn compile_schema(&self, output_schema: &OutputSchema) -> Result<CompiledSchema, SchemaError> {
        let mut schema = output_schema.schema.as_value().clone();
        schema["x-compiled-by"] = json!("chat-completions");
        Ok(CompiledSchema {
            schema,
            warnings: Vec::new(),
        })
    }
}

fn review_schema() -> OutputSchema {
    OutputSchema::new(json!({
        "type": "object",
        "properties": {
            "verdict": {"type": "string", "enum": ["approve", "request_changes"]},
            "summary": {"type": "string"}
        },
        "required": ["verdict"]
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

/// The transcripts of turn one, turn two and the extraction request, built
/// once so the with- and without-schema requests share every other byte.
fn transcripts() -> [Vec<Message>; 3] {
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
    [turn_one, turn_two, extraction]
}

/// The requests the loop sends on the Gemini route: Gemini tags, tools on
/// main turns, the native schema slot and temperature 0 on extraction.
fn loop_requests(transcripts: &[Vec<Message>; 3], schema: &OutputSchema) -> Vec<LlmRequest> {
    let mut turn_one =
        LlmRequest::new(MODEL, transcripts[0].clone()).with_tools(vec![lookup_tool()]);
    turn_one.provider_params = Some(ProviderTag::Gemini(GeminiProviderTag::default()));
    let mut turn_two =
        LlmRequest::new(MODEL, transcripts[1].clone()).with_tools(vec![lookup_tool()]);
    turn_two.provider_params = Some(ProviderTag::Gemini(GeminiProviderTag::default()));
    let mut extraction = LlmRequest::new(MODEL, transcripts[2].clone()).with_temperature(0.0);
    extraction.provider_params = Some(ProviderTag::Gemini(GeminiProviderTag {
        structured_output: Some(schema.clone()),
        ..Default::default()
    }));
    vec![turn_one, turn_two, extraction]
}

fn project_all(transcripts: &[Vec<Message>; 3], section: &str) -> [Vec<Message>; 3] {
    transcripts.clone().map(|mut messages| {
        project_output_schema_instructions(&mut messages, section);
        messages
    })
}

async fn lower_through_the_route(requests: &[LlmRequest]) -> Vec<LlmRequest> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let client = GeminiCopilotChatClient {
        inner: Arc::new(RecordingChatClient {
            seen: Arc::clone(&seen),
        }),
    };
    for request in requests {
        let mut stream = client.stream(request);
        while let Some(event) = futures::StreamExt::next(&mut stream).await {
            event.expect("lowered stream");
        }
    }
    let lowered = std::mem::take(&mut *seen.lock().expect("seen lock"));
    assert_eq!(lowered.len(), requests.len());
    lowered
}

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

fn section_for(schema: &OutputSchema) -> String {
    let client = GeminiCopilotChatClient {
        inner: Arc::new(RecordingChatClient {
            seen: Arc::new(Mutex::new(Vec::new())),
        }),
    };
    render_output_schema_instructions(&client.compile_schema(schema).expect("compile").schema)
}

#[tokio::test]
async fn copilot_route_passes_the_section_through_and_lowers_the_native_slot() {
    let schema = review_schema();
    let section = section_for(&schema);
    let base = transcripts();
    let with_requests = loop_requests(&project_all(&base, &section), &schema);
    let without_requests = loop_requests(&base, &schema);
    let with_lowered = lower_through_the_route(&with_requests).await;
    let without_lowered = lower_through_the_route(&without_requests).await;

    for (index, label) in ["turn one", "turn two", "extraction"].iter().enumerate() {
        let sent = &with_requests[index];
        let lowered = &with_lowered[index];
        assert_eq!(
            serde_json::to_value(&lowered.messages).unwrap(),
            serde_json::to_value(&sent.messages).unwrap(),
            "{label}: the messages, section included, pass through unchanged"
        );
        let Some(Message::System(system)) = lowered.messages.first() else {
            panic!("{label}: the system prompt leads");
        };
        assert_eq!(
            system.content,
            format!("{SYSTEM_PROMPT}\n\n{section}"),
            "{label}"
        );
        assert_eq!(
            lowered
                .messages
                .iter()
                .filter(|message| matches!(message, Message::System(_)))
                .count(),
            1,
            "{label}: one system message"
        );

        let with_value = serde_json::to_value(lowered).unwrap();
        let mut hits = 0;
        let stripped = strip_appended_section(&with_value, &section, &mut hits);
        assert_eq!(hits, 1, "{label}: the section appears exactly once");
        assert_eq!(
            with_value
                .to_string()
                .matches(OUTPUT_SCHEMA_INSTRUCTIONS_OPEN)
                .count(),
            1,
            "{label}"
        );
        assert_eq!(
            stripped,
            serde_json::to_value(&without_lowered[index]).unwrap(),
            "{label}: apart from the section the lowered request is unchanged"
        );
    }

    for lowered in &with_lowered[..2] {
        assert_eq!(lowered.tools.len(), 1, "main turns keep their tools");
        assert_eq!(
            serde_json::to_value(&lowered.provider_params).unwrap(),
            serde_json::to_value(Some(ProviderTag::OpenAi(OpenAiProviderTag::default()))).unwrap(),
            "main turns carry no native schema"
        );
    }
    let extraction = &with_lowered[2];
    assert!(extraction.tools.is_empty(), "no tools on extraction");
    assert_eq!(extraction.temperature, Some(0.0));
    let Some(ProviderTag::OpenAi(tag)) = extraction.provider_params.as_ref() else {
        panic!("the extraction request lowers to the Chat Completions tag");
    };
    assert_eq!(
        tag.structured_output
            .as_ref()
            .map(|schema| schema.schema.as_value().clone()),
        Some(schema.schema.as_value().clone()),
        "the extraction request keeps the native schema slot"
    );
}

/// The section on this route shows the schema the Chat Completions client
/// compiles, because the wrapper delegates schema compilation to it.
#[test]
fn copilot_route_section_shows_the_chat_completions_compiled_schema() {
    let schema = review_schema();
    assert!(section_for(&schema).contains(r#""x-compiled-by":"chat-completions""#));
}

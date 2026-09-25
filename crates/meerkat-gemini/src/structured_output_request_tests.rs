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
    OUTPUT_SCHEMA_INSTRUCTIONS_OPEN, OUTPUT_SCHEMA_INSTRUCTIONS_SEPARATOR,
    project_output_schema_instructions, render_output_schema_instructions,
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

// ---------------------------------------------------------------------------
// Code Assist wrapper (recorded request bodies)
// ---------------------------------------------------------------------------

mod code_assist {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use axum::{
        Json, Router, extract::State, http::HeaderMap, response::IntoResponse, routing::post,
    };
    use futures::StreamExt;
    use tokio::net::TcpListener;

    use super::*;

    const PROJECT: &str = "test-project";
    const SSE_OK: &str = concat!(
        "data: {\"response\":{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"ok\"}]},",
        "\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":3,",
        "\"candidatesTokenCount\":1}},\"traceId\":\"trace-1\"}\n",
    );

    struct Recorded {
        headers: BTreeMap<String, String>,
        body: Value,
    }

    type Captures = Arc<Mutex<Vec<Recorded>>>;

    async fn capture(
        State(captures): State<Captures>,
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
        captures
            .lock()
            .expect("capture lock")
            .push(Recorded { headers, body });
        ([("content-type", "text/event-stream")], SSE_OK)
    }

    fn code_assist_client(base_url: String) -> GeminiClient {
        GeminiClient::new_with_base_url(String::new(), base_url)
            .with_code_assist_wire()
            .with_code_assist_project_id(Some(PROJECT.to_string()))
    }

    /// Stream every request through the real Code Assist client against a
    /// local `v1internal` endpoint and return what it sent.
    async fn record(requests: &[LlmRequest]) -> Vec<Recorded> {
        let captures: Captures = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/v1internal:streamGenerateContent", post(capture))
            .with_state(Arc::clone(&captures));
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let client = code_assist_client(format!("http://{addr}"));
        for request in requests {
            let mut stream = client.stream(request);
            while let Some(event) = stream.next().await {
                event.expect("Code Assist stream");
            }
        }
        server.abort();
        let recorded = std::mem::take(&mut *captures.lock().expect("capture lock"));
        assert_eq!(recorded.len(), requests.len(), "one request sent per call");
        recorded
    }

    fn run_requests(
        client: &GeminiClient,
        schema: &OutputSchema,
        project_section: bool,
    ) -> Vec<LlmRequest> {
        let transcripts = run_transcripts();
        let messages = |messages: &[Message]| {
            if project_section {
                project(client, schema, messages)
            } else {
                messages.to_vec()
            }
        };
        vec![
            main_request(messages(&transcripts.turn_one)),
            main_request(messages(&transcripts.turn_two)),
            extraction_request(messages(&transcripts.extraction), schema),
        ]
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

    /// The per-request `user_prompt_id` is fresh on every call by design; the
    /// rest of the wrapper is compared byte-for-byte.
    fn without_prompt_id(body: &Value) -> Value {
        let mut body = body.clone();
        let removed = body
            .as_object_mut()
            .and_then(|outer| outer.remove("user_prompt_id"));
        assert!(
            removed.as_ref().and_then(Value::as_str).is_some(),
            "Code Assist requests carry a user_prompt_id"
        );
        body
    }

    #[tokio::test]
    async fn code_assist_wrapper_carries_the_section_once_and_is_otherwise_unchanged() {
        let probe = code_assist_client("http://127.0.0.1:9".to_string());
        let schema = review_schema();
        let compiled = probe.compile_schema(&schema).unwrap();
        let section = render_output_schema_instructions(&compiled.schema);
        let with_requests = run_requests(&probe, &schema, true);
        let with_schema = record(&with_requests).await;
        let without_schema = record(&run_requests(&probe, &schema, false)).await;

        for (index, label) in ["turn one", "turn two", "extraction"].iter().enumerate() {
            let with = &with_schema[index];
            let without = &without_schema[index];
            assert_eq!(with.body["model"], MODEL, "{label}");
            assert_eq!(with.body["project"], PROJECT, "{label}");
            assert!(
                with.body.get("systemInstruction").is_none() && with.body.get("contents").is_none(),
                "{label}: public GenerateContent fields stay inside `request`"
            );
            let mut public = with_requests[index].clone();
            public.messages = probe
                .project_replay_messages(&public.messages)
                .expect("replay projection");
            assert_eq!(
                with.body["request"],
                probe.build_request_body(&public).expect("public body"),
                "{label}: the wrapper carries the public body, section included, unchanged"
            );
            let parts = with.body["request"]["systemInstruction"]["parts"]
                .as_array()
                .expect("systemInstruction parts");
            assert_eq!(parts.len(), 1, "{label}: one merged system part");
            assert_eq!(
                parts[0]["text"],
                Value::String(format!("{SYSTEM_PROMPT}\n\n{section}")),
                "{label}"
            );

            let mut hits = 0;
            let stripped =
                strip_appended_section(&without_prompt_id(&with.body), &section, &mut hits);
            assert_eq!(hits, 1, "{label}: the section appears exactly once");
            assert_eq!(
                with.body
                    .to_string()
                    .matches(OUTPUT_SCHEMA_INSTRUCTIONS_OPEN)
                    .count(),
                1,
                "{label}"
            );
            assert!(
                !without
                    .body
                    .to_string()
                    .contains(OUTPUT_SCHEMA_INSTRUCTIONS_OPEN)
            );
            assert_eq!(
                stripped,
                without_prompt_id(&without.body),
                "{label}: apart from the section the wrapped request is unchanged"
            );
            assert_eq!(with.headers, without.headers, "{label}: no header changes");
        }

        let system = |index: usize| with_schema[index].body["request"]["systemInstruction"].clone();
        assert_eq!(system(0), system(1), "byte-identical across turns");
        assert_eq!(system(0), system(2), "the extraction request keeps it");
        for recorded in &with_schema[..2] {
            assert_eq!(
                function_names(&recorded.body["request"]),
                vec!["lookup".to_string()]
            );
            assert!(
                recorded.body["request"]["generationConfig"]
                    .get("responseJsonSchema")
                    .is_none(),
                "main turns carry no response schema"
            );
        }
        let extraction = &with_schema[2].body["request"];
        assert!(
            function_names(extraction).is_empty(),
            "no tools on extraction"
        );
        assert_eq!(
            extraction["generationConfig"]["responseMimeType"],
            "application/json"
        );
        assert_eq!(
            extraction["generationConfig"]["responseJsonSchema"],
            *schema.schema.as_value(),
            "the extraction request keeps the native response schema"
        );
        let last = extraction["contents"].as_array().unwrap().last().unwrap();
        assert_eq!(last["parts"][0]["text"], EXTRACTION_PROMPT);
    }

    /// Without an authored system prompt the inserted section becomes the
    /// wrapped request's only system part.
    #[tokio::test]
    async fn code_assist_inserted_section_is_the_system_instruction() {
        let probe = code_assist_client("http://127.0.0.1:9".to_string());
        let schema = review_schema();
        let compiled = probe.compile_schema(&schema).unwrap();
        let messages = vec![Message::User(UserMessage::text("review"))];
        let recorded = record(&[main_request(project(&probe, &schema, &messages))]).await;
        let request = &recorded[0].body["request"];
        assert_eq!(
            request["systemInstruction"]["parts"],
            json!([{"text": render_output_schema_instructions(&compiled.schema)}])
        );
        assert_eq!(request["contents"][0]["role"], "user");
    }
}

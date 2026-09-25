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
    OUTPUT_SCHEMA_INSTRUCTIONS_OPEN, OUTPUT_SCHEMA_INSTRUCTIONS_SEPARATOR,
    project_output_schema_instructions, render_output_schema_instructions,
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

// ---------------------------------------------------------------------------
// Comparison with the requests main sends (no section)
// ---------------------------------------------------------------------------

/// The section the loop appends for `schema` on this client.
fn section_for(client: &AnthropicClient, schema: &OutputSchema) -> String {
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

/// The body built with a schema carries the section exactly once, and with
/// the section removed it is byte-for-byte the body built from the same
/// transcript without a schema (what main sends for that request).
fn assert_only_the_section_differs(
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
        "{label}: exactly one opening delimiter in the whole body"
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

/// The three requests of a run, with and without the schema section. The
/// extraction request carries the native schema slot in both, exactly as the
/// extraction phase sends it.
fn run_requests(
    client: &AnthropicClient,
    schema: &OutputSchema,
    tag: &AnthropicProviderTag,
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
            main_request(messages(&transcripts.turn_one), tag.clone()),
        ),
        (
            "turn two",
            main_request(messages(&transcripts.turn_two), tag.clone()),
        ),
        (
            "extraction",
            extraction_request(messages(&transcripts.extraction), tag.clone(), schema),
        ),
    ]
}

/// Every Anthropic cache policy, including `system_and_conversation`, lowers
/// the schema-bearing requests to the no-schema requests plus the section.
#[test]
fn messages_requests_differ_from_the_no_schema_requests_only_by_the_section() {
    let client = AnthropicClient::new("test-key".to_string()).expect("client");
    let schema = review_schema();
    let section = section_for(&client, &schema);
    for policy in [
        None,
        Some(AnthropicCacheControlPolicy::Automatic),
        Some(AnthropicCacheControlPolicy::Disabled),
        Some(AnthropicCacheControlPolicy::SystemPrefix),
        Some(AnthropicCacheControlPolicy::SystemAndConversation),
    ] {
        let tag = AnthropicProviderTag {
            cache_control: policy,
            ..Default::default()
        };
        let with_schema = run_requests(&client, &schema, &tag, true);
        let without_schema = run_requests(&client, &schema, &tag, false);
        for ((label, with), (_, without)) in with_schema.iter().zip(&without_schema) {
            let label = format!("{policy:?} {label}");
            let with = client.build_request_body(with).expect("with schema");
            let without = client.build_request_body(without).expect("without schema");
            assert_only_the_section_differs(&label, &with, &without, &section);
        }
    }
}

/// `system_and_conversation` puts a breakpoint on the one system block (which
/// carries the section) and on the trailing conversation rows. The section
/// keeps the system block byte-identical across turns and on extraction, and
/// the extraction request keeps its native schema slot.
#[test]
fn messages_main_and_extraction_requests_with_system_and_conversation_cache() {
    let tag = AnthropicProviderTag {
        cache_control: Some(AnthropicCacheControlPolicy::SystemAndConversation),
        ..Default::default()
    };
    let turn_one = assert_messages_run(tag.clone());
    let blocks = turn_one["system"].as_array().expect("system blocks");
    assert_eq!(
        blocks.len(),
        1,
        "the section merges into the one system block"
    );
    assert_eq!(blocks[0]["cache_control"]["type"], "ephemeral");
    assert!(
        turn_one.get("cache_control").is_none(),
        "no request-wide automatic breakpoint under system_and_conversation"
    );

    let client = AnthropicClient::new("test-key".to_string()).expect("client");
    let schema = review_schema();
    let requests = run_requests(&client, &schema, &tag, true);
    let bodies: Vec<Value> = requests
        .iter()
        .map(|(_, request)| client.build_request_body(request).expect("body"))
        .collect();
    for (body, (label, _)) in bodies.iter().zip(&requests) {
        let last = body["messages"].as_array().unwrap().last().unwrap();
        assert_eq!(
            last["content"]
                .as_array()
                .and_then(|parts| parts.last())
                .map(|part| part["cache_control"]["type"].clone()),
            Some(json!("ephemeral")),
            "{label}: the conversation breakpoint still closes the request: {body}"
        );
    }
    let extraction = &bodies[2];
    assert_eq!(extraction["output_config"]["format"]["type"], "json_schema");
    assert_eq!(
        extraction["output_config"]["format"]["schema"],
        client.compile_schema(&schema).unwrap().schema
    );
    assert!(extraction.get("tools").is_none(), "no tools on extraction");
}

// ---------------------------------------------------------------------------
// Recorded request bodies (claude.ai OAuth marker, Copilot Messages route)
// ---------------------------------------------------------------------------

mod recorded {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use axum::{
        Json, Router, extract::State, http::HeaderMap, response::IntoResponse, routing::post,
    };
    use futures::StreamExt;
    use tokio::net::TcpListener;

    use super::*;

    /// The system block the client prepends for claude.ai OAuth tokens.
    const CLAUDE_AI_OAUTH_MARKER: &str =
        "You are a Claude agent, built on Anthropic's Claude Agent SDK.";

    const SSE_OK: &str = concat!(
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n",
        "data: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n",
        "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n",
        "data: {\"type\":\"content_block_stop\"}\n",
        "data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":1},\"delta\":{\"stop_reason\":\"end_turn\"}}\n",
        "data: {\"type\":\"message_stop\"}\n",
    );

    pub(super) struct Recorded {
        pub(super) headers: BTreeMap<String, String>,
        pub(super) body: Value,
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

    struct LabelledAuthorizer {
        label: &'static str,
    }

    #[async_trait::async_trait]
    impl meerkat_core::HttpAuthorizer for LabelledAuthorizer {
        async fn authorize(
            &self,
            request: &mut meerkat_core::HttpAuthorizationRequest<'_>,
        ) -> Result<(), meerkat_core::AuthError> {
            request
                .headers
                .push(("Authorization".to_string(), "Bearer test-token".to_string()));
            Ok(())
        }

        fn label(&self) -> &'static str {
            self.label
        }
    }

    /// Stream every request through the real client against a local
    /// Messages endpoint and return the bodies and headers it sent.
    pub(super) async fn record(
        build: impl FnOnce(String) -> AnthropicClient,
        requests: &[(&'static str, LlmRequest)],
    ) -> (AnthropicClient, Vec<Recorded>) {
        let captures: Captures = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/v1/messages", post(capture))
            .with_state(Arc::clone(&captures));
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
        (client, recorded)
    }

    fn claude_ai_oauth_client(base_url: String) -> AnthropicClient {
        // The claude.ai OAuth route: the Anthropic API backend with an
        // authorizer carrying the claude.ai OAuth label.
        AnthropicClient::builder(String::new())
            .authorizer(Arc::new(LabelledAuthorizer {
                label: "claude-ai-oauth",
            }))
            .base_url(base_url)
            .build()
            .expect("client")
    }

    #[cfg(feature = "copilot")]
    fn copilot_messages_client(base_url: String) -> AnthropicClient {
        // Exactly what the Anthropic Copilot route factory builds: the Copilot
        // authorizer, no request-wide automatic cache policy.
        AnthropicClient::builder(String::new())
            .authorizer(Arc::new(LabelledAuthorizer {
                label: meerkat_copilot::GITHUB_COPILOT_AUTHORIZER_LABEL,
            }))
            .base_url(base_url)
            .default_cache_control(AnthropicCacheControlPolicy::Disabled)
            .automatic_cache_control_supported(false)
            .build()
            .expect("client")
    }

    /// Record one run with and one without a schema and check the common
    /// guarantees: the section exactly once, the lowered `system` value
    /// byte-identical across turns and on extraction, the extraction request
    /// keeps the native slot and drops tools, and apart from the section
    /// every request (body and headers) is what main sends.
    async fn assert_recorded_run(
        build: fn(String) -> AnthropicClient,
        tag: AnthropicProviderTag,
    ) -> Vec<Recorded> {
        let schema = review_schema();
        let probe = build("http://127.0.0.1:9".to_string());
        let section = section_for(&probe, &schema);
        let (client, with_schema) = record(build, &run_requests(&probe, &schema, &tag, true)).await;
        let (_, without_schema) = record(build, &run_requests(&probe, &schema, &tag, false)).await;

        for ((label, _), (with, without)) in run_requests(&probe, &schema, &tag, true)
            .iter()
            .zip(with_schema.iter().zip(&without_schema))
        {
            assert_only_the_section_differs(label, &with.body, &without.body, &section);
            assert_eq!(
                with.headers, without.headers,
                "{label}: the section changes no header"
            );
        }
        assert_eq!(with_schema[0].body["system"], with_schema[1].body["system"]);
        assert_eq!(with_schema[0].body["system"], with_schema[2].body["system"]);
        for recorded in &with_schema[..2] {
            assert_eq!(tool_names(&recorded.body), vec!["lookup".to_string()]);
            assert!(
                recorded
                    .body
                    .get("output_config")
                    .and_then(|config| config.get("format"))
                    .is_none()
            );
        }
        let extraction = &with_schema[2].body;
        assert!(tool_names(extraction).is_empty(), "no tools on extraction");
        assert_eq!(extraction["output_config"]["format"]["type"], "json_schema");
        assert_eq!(
            extraction["output_config"]["format"]["schema"],
            client.compile_schema(&schema).unwrap().schema,
            "the extraction request keeps the closed-object native schema"
        );
        let last = extraction["messages"].as_array().unwrap().last().unwrap();
        assert!(last.to_string().contains(EXTRACTION_PROMPT));
        with_schema
    }

    #[tokio::test]
    async fn claude_ai_oauth_requests_keep_the_marker_first_and_carry_the_section_once() {
        let recorded =
            assert_recorded_run(claude_ai_oauth_client, AnthropicProviderTag::default()).await;
        let client = claude_ai_oauth_client("http://127.0.0.1:9".to_string());
        let expected = expected_system(&client, &review_schema());
        for recorded in &recorded {
            let blocks = recorded.body["system"]
                .as_array()
                .expect("the marker turns system into blocks");
            assert_eq!(blocks.len(), 2, "{}", recorded.body);
            assert_eq!(blocks[0]["text"], CLAUDE_AI_OAUTH_MARKER);
            assert_eq!(
                blocks[1]["text"],
                Value::String(expected.clone()),
                "the section stays in the authored system block, after the marker"
            );
        }
    }

    #[tokio::test]
    async fn claude_ai_oauth_requests_with_system_prefix_cache() {
        let recorded = assert_recorded_run(
            claude_ai_oauth_client,
            AnthropicProviderTag {
                cache_control: Some(AnthropicCacheControlPolicy::SystemPrefix),
                ..Default::default()
            },
        )
        .await;
        let blocks = recorded[0].body["system"].as_array().expect("blocks");
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["text"], CLAUDE_AI_OAUTH_MARKER);
        assert!(blocks[0].get("cache_control").is_none());
        assert_eq!(
            blocks[1]["cache_control"]["type"], "ephemeral",
            "the breakpoint still closes the system prefix that carries the section"
        );
    }

    /// With no authored system prompt the inserted section is the system
    /// block after the marker; nothing is merged into the marker.
    #[tokio::test]
    async fn claude_ai_oauth_inserted_section_follows_the_marker() {
        let schema = review_schema();
        let probe = claude_ai_oauth_client("http://127.0.0.1:9".to_string());
        let messages = vec![Message::User(UserMessage::text("review"))];
        let request = main_request(
            project(&probe, &schema, &messages),
            AnthropicProviderTag::default(),
        );
        let (_, recorded) = record(claude_ai_oauth_client, &[("inserted", request)]).await;
        let blocks = recorded[0].body["system"].as_array().expect("blocks");
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["text"], CLAUDE_AI_OAUTH_MARKER);
        assert_eq!(
            blocks[1]["text"],
            Value::String(section_for(&probe, &schema))
        );
    }

    #[cfg(feature = "copilot")]
    #[tokio::test]
    async fn copilot_messages_route_requests_carry_the_section_once() {
        let recorded =
            assert_recorded_run(copilot_messages_client, AnthropicProviderTag::default()).await;
        let client = copilot_messages_client("http://127.0.0.1:9".to_string());
        let expected = expected_system(&client, &review_schema());
        for recorded in &recorded {
            assert_eq!(
                recorded.body["system"],
                Value::String(expected.clone()),
                "the Copilot route lowers one system prompt to a plain string"
            );
            assert!(
                !recorded.body.to_string().contains("cache_control"),
                "the Copilot route sends no cache breakpoints: {}",
                recorded.body
            );
            assert!(
                !recorded.body.to_string().contains(CLAUDE_AI_OAUTH_MARKER),
                "the claude.ai marker is not added on the Copilot route"
            );
        }
    }
}

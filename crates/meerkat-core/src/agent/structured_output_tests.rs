//! Structured output through the real agent loop.
//!
//! These tests pin the layered `--schema` contract end to end, against the
//! requests the loop actually composes:
//!
//! - **Schema visibility (A):** when an output schema is configured, every
//!   request of the run carries a delimited instruction section with the
//!   schema, appended to the leading system prompt, byte-identical across
//!   turns (main and extraction), request-only (never written into the
//!   transcript), and absent when no schema is configured.
//! - **Validate-first (B):** when the final reply of the tool loop already
//!   validates, it becomes the structured output and no extraction request is
//!   sent. Anything else runs the extraction path exactly as before: the same
//!   prompt, temperature 0, no tools, the provider's native schema slot, and
//!   the same retry accounting.
//!
//! The provider matrix runs the fallback path once per provider identity the
//! typed structured-output slot distinguishes, so every adapter family sees
//! the request shape it lowers.
#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::too_many_lines
)]

use crate as meerkat_core;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use meerkat_core::lifecycle::run_primitive::{
    AnthropicProviderTag, GeminiProviderTag, OpaqueProviderBody, OpenAiProviderTag,
    ProviderParamsOverride, ProviderTag,
};
use meerkat_core::{
    AgentBuilder, AgentError, AgentEvent, AgentLlmClient, AgentSessionStore, AgentToolDispatcher,
    AssistantBlock, LlmStreamResult, Message, OutputSchema, Provider, StopReason,
    StructuredOutputOrigin, ToolCallView, ToolDef, ToolResult, TurnUsage, Usage,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::extraction::DEFAULT_EXTRACTION_PROMPT;
use crate::structured_output::{
    OUTPUT_SCHEMA_INSTRUCTIONS_CLOSE, OUTPUT_SCHEMA_INSTRUCTIONS_OPEN,
    OUTPUT_SCHEMA_INSTRUCTIONS_SEPARATOR, render_output_schema_instructions,
};

const SYSTEM_PROMPT: &str = "You are a careful code reviewer.";

// ---------------------------------------------------------------------------
// Scripted, recording client
// ---------------------------------------------------------------------------

/// How the client lowers an output schema for validation, standing in for
/// the provider adapters' `compile_schema`.
#[derive(Clone, Copy)]
enum CompileMode {
    /// OpenAI (non-strict), Gemini, Chat Completions: the schema as given.
    Passthrough,
    /// Anthropic: `additionalProperties: false` on every object.
    CloseObjects,
    /// A compiled schema the validator cannot build, to prove a validator
    /// fault falls through to the unchanged extraction path.
    ValidatorFault,
}

/// One request exactly as the loop handed it to the client.
#[derive(Clone)]
struct RecordedCall {
    messages: Vec<Message>,
    tool_names: Vec<String>,
    temperature: Option<f32>,
    provider_params: Option<ProviderParamsOverride>,
}

impl RecordedCall {
    fn system_prompt(&self) -> &str {
        match self.messages.first() {
            Some(Message::System(system)) => &system.content,
            other => panic!("request must lead with a system prompt, got {other:?}"),
        }
    }

    fn last_user_text(&self) -> String {
        match self.messages.last() {
            Some(Message::User(user)) => user.text_content(),
            other => panic!("expected the request to end with a user message, got {other:?}"),
        }
    }
}

struct RecordingSchemaClient {
    provider: Provider,
    model: &'static str,
    compile: CompileMode,
    /// Attach a cache-breakpoint claim over the exact request messages.
    claim_over_request: bool,
    responses: Mutex<VecDeque<LlmStreamResult>>,
    calls: Mutex<Vec<RecordedCall>>,
}

impl RecordingSchemaClient {
    fn new(provider: Provider, responses: Vec<LlmStreamResult>) -> Self {
        let model = match provider {
            Provider::Anthropic => "claude-opus-5",
            Provider::OpenAI => "gpt-5.6-luna",
            Provider::Gemini => "gemini-3.5-flash",
            Provider::SelfHosted => "self-hosted-model",
            _ => "mock-model",
        };
        let compile = match provider {
            Provider::Anthropic => CompileMode::CloseObjects,
            _ => CompileMode::Passthrough,
        };
        Self {
            provider,
            model,
            compile,
            claim_over_request: false,
            responses: Mutex::new(responses.into()),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn with_compile(mut self, compile: CompileMode) -> Self {
        self.compile = compile;
        self
    }

    fn with_claims_over_request(mut self) -> Self {
        self.claim_over_request = true;
        self
    }

    fn calls(&self) -> Vec<RecordedCall> {
        self.calls.lock().unwrap().clone()
    }

    fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

fn close_objects(schema: &mut Value) {
    match schema {
        Value::Object(map) => {
            if map.get("type").and_then(Value::as_str) == Some("object")
                && !map.contains_key("additionalProperties")
            {
                map.insert("additionalProperties".to_string(), Value::Bool(false));
            }
            for value in map.values_mut() {
                close_objects(value);
            }
        }
        Value::Array(items) => {
            for item in items {
                close_objects(item);
            }
        }
        _ => {}
    }
}

#[async_trait]
impl AgentLlmClient for RecordingSchemaClient {
    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[Arc<ToolDef>],
        _max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&ProviderParamsOverride>,
    ) -> Result<LlmStreamResult, AgentError> {
        self.calls.lock().unwrap().push(RecordedCall {
            messages: messages.to_vec(),
            tool_names: tools.iter().map(|tool| tool.name.to_string()).collect(),
            temperature,
            provider_params: provider_params.cloned(),
        });
        let scripted = self.responses.lock().unwrap().pop_front().ok_or_else(|| {
            AgentError::InternalError("RecordingSchemaClient: script exhausted".to_string())
        })?;
        let usage = TurnUsage::host_declared(self.provider, self.model, scripted.usage().clone())
            .into_inner();
        let mut result =
            LlmStreamResult::new(scripted.blocks().to_vec(), scripted.stop_reason(), usage);
        if self.claim_over_request {
            let claim = crate::provider_cache_breakpoint_claim(
                crate::ProviderCacheBreakpointClaimRequest {
                    provider: self.provider,
                    model: self.model,
                    messages,
                    boundary: crate::CacheBreakpointBoundary::SystemProfilePrefix {
                        message_count: 1,
                    },
                    ttl: crate::ProviderCacheTtl::ProviderDefault,
                    rendered_prefix: br#"{"renderer_mode":"structured-output-test"}"#,
                    lowered_request_encoding: crate::LoweredRequestEncoding::OpenAiResponsesJson,
                    lowered_request_body: br#"{"model":"structured-output-test"}"#,
                },
            )
            .expect("claim shape");
            result = result.with_cache_breakpoint_claims(vec![claim]);
        }
        Ok(result)
    }

    fn provider(&self) -> Provider {
        self.provider
    }

    fn model(&self) -> &'static str {
        self.model
    }

    fn compile_schema(
        &self,
        output_schema: &OutputSchema,
    ) -> Result<crate::CompiledSchema, crate::SchemaError> {
        let mut schema = output_schema.schema.as_value().clone();
        match self.compile {
            CompileMode::Passthrough => {}
            CompileMode::CloseObjects => close_objects(&mut schema),
            CompileMode::ValidatorFault => {
                schema = json!({"type": "object", "properties": {"answer": {"type": 7}}});
            }
        }
        Ok(crate::CompiledSchema {
            schema,
            warnings: Vec::new(),
        })
    }
}

fn text(reply: &str) -> LlmStreamResult {
    LlmStreamResult::new(
        vec![AssistantBlock::Text {
            text: reply.to_string(),
            meta: None,
        }],
        StopReason::EndTurn,
        Usage::default(),
    )
}

fn tool_call(id: &str) -> LlmStreamResult {
    LlmStreamResult::new(
        vec![AssistantBlock::ToolUse {
            id: id.to_string(),
            name: "lookup".into(),
            args: serde_json::value::RawValue::from_string("{}".to_string()).unwrap(),
            meta: None,
        }],
        StopReason::ToolUse,
        Usage {
            input_tokens: 100,
            output_tokens: 10,
            ..Usage::default()
        },
    )
}

struct LookupTool;

#[async_trait]
impl AgentToolDispatcher for LookupTool {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::new([Arc::new(ToolDef {
            name: "lookup".into(),
            description: "returns a fixed observation".to_string(),
            input_schema: json!({ "type": "object" }),
            provenance: None,
        })])
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        Ok(ToolResult::new(call.id.to_string(), "observation".to_string(), false).into())
    }
}

struct NoopStore;

#[async_trait]
impl AgentSessionStore for NoopStore {
    async fn save(&self, _session: &meerkat_core::Session) -> Result<(), AgentError> {
        Ok(())
    }

    async fn load(&self, _id: &str) -> Result<Option<meerkat_core::Session>, AgentError> {
        Ok(None)
    }
}

type TestAgent = crate::agent::Agent<RecordingSchemaClient, LookupTool, NoopStore>;

fn base_builder() -> AgentBuilder {
    AgentBuilder::new()
        .system_prompt(SYSTEM_PROMPT)
        .with_turn_state_handle(Arc::new(
            crate::agent::test_turn_state_handle::TestTurnStateHandle::new(),
        ))
}

async fn build(client: &Arc<RecordingSchemaClient>, builder: AgentBuilder) -> TestAgent {
    builder
        .build_standalone(
            Arc::clone(client),
            Arc::new(LookupTool),
            Arc::new(NoopStore),
        )
        .await
}

/// Run once and collect every event the run published.
async fn run_collecting(
    agent: &mut TestAgent,
    prompt: &str,
) -> (Result<crate::types::RunResult, AgentError>, Vec<AgentEvent>) {
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(1024);
    let result = agent.run_with_events(prompt.to_string().into(), tx).await;
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    (result, events)
}

// ---------------------------------------------------------------------------
// Schemas
// ---------------------------------------------------------------------------

/// A review-shaped schema: nested objects, arrays, enums, an optional field,
/// and closed objects.
fn review_schema() -> OutputSchema {
    OutputSchema::new(json!({
        "type": "object",
        "properties": {
            "verdict": {"type": "string", "enum": ["approve", "request_changes"]},
            "summary": {"type": "string"},
            "comments": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "line": {"type": "integer", "minimum": 1},
                        "category": {"type": "string", "enum": ["bug", "style", "perf"]},
                        "body": {"type": "string"}
                    },
                    "required": ["path", "line", "category", "body"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["verdict", "comments"],
        "additionalProperties": false
    }))
    .expect("valid review schema")
}

const VALID_REVIEW: &str = r#"{"verdict":"request_changes","comments":[{"path":"src/lib.rs","line":42,"category":"bug","body":"off by one"}]}"#;

fn expected_review() -> Value {
    serde_json::from_str(VALID_REVIEW).unwrap()
}

fn expected_system_prompt(client: &RecordingSchemaClient, schema: &OutputSchema) -> String {
    let compiled = client.compile_schema(schema).expect("test compile");
    format!(
        "{SYSTEM_PROMPT}{OUTPUT_SCHEMA_INSTRUCTIONS_SEPARATOR}{}",
        render_output_schema_instructions(&compiled.schema)
    )
}

fn structured_output_slot(params: Option<&ProviderParamsOverride>) -> Option<&OutputSchema> {
    match params.and_then(|params| params.provider_tag.as_ref()) {
        Some(ProviderTag::Anthropic(tag)) => tag.structured_output.as_ref(),
        Some(ProviderTag::OpenAi(tag)) => tag.structured_output.as_ref(),
        Some(ProviderTag::Gemini(tag)) => tag.structured_output.as_ref(),
        _ => None,
    }
}

fn native_search_present(params: Option<&ProviderParamsOverride>) -> bool {
    match params.and_then(|params| params.provider_tag.as_ref()) {
        Some(ProviderTag::Anthropic(tag)) => tag.web_search.is_some(),
        Some(ProviderTag::OpenAi(tag)) => tag.web_search.is_some(),
        Some(ProviderTag::Gemini(tag)) => tag.google_search.is_some(),
        _ => false,
    }
}

fn web_search_defaults(provider: Provider) -> Option<ProviderTag> {
    let body = OpaqueProviderBody::from_value(&json!({"type": "web_search"}));
    match provider {
        Provider::Anthropic => Some(ProviderTag::Anthropic(AnthropicProviderTag {
            web_search: Some(body),
            ..Default::default()
        })),
        Provider::OpenAI | Provider::SelfHosted => Some(ProviderTag::OpenAi(OpenAiProviderTag {
            web_search: Some(body),
            ..Default::default()
        })),
        Provider::Gemini => Some(ProviderTag::Gemini(GeminiProviderTag {
            google_search: Some(body),
            ..Default::default()
        })),
        _ => None,
    }
}

fn is_extraction_prompt(message: &Message) -> bool {
    matches!(message, Message::User(user) if user.text_content() == DEFAULT_EXTRACTION_PROMPT)
}

// ---------------------------------------------------------------------------
// A. Schema visibility
// ---------------------------------------------------------------------------

#[tokio::test]
async fn requests_without_a_schema_carry_no_structured_output_section() {
    let client = Arc::new(RecordingSchemaClient::new(
        Provider::OpenAI,
        vec![tool_call("call-1"), text("done")],
    ));
    let mut agent = build(&client, base_builder()).await;

    let result = agent
        .run("review".to_string().into())
        .await
        .expect("run completes");

    assert_eq!(result.text, "done");
    assert!(result.structured_output.is_none());
    let calls = client.calls();
    assert_eq!(calls.len(), 2, "no schema means no extraction request");
    for call in &calls {
        assert_eq!(
            call.system_prompt(),
            SYSTEM_PROMPT,
            "the system prompt must be untouched when no schema is configured"
        );
        assert!(
            !call
                .system_prompt()
                .contains(OUTPUT_SCHEMA_INSTRUCTIONS_OPEN)
        );
    }
}

#[tokio::test]
async fn schema_section_is_present_byte_stable_and_request_only() {
    for provider in [
        Provider::Anthropic,
        Provider::OpenAI,
        Provider::Gemini,
        Provider::SelfHosted,
        Provider::Other,
    ] {
        let schema = review_schema();
        // Tool call, then prose (forces the extraction request), then valid
        // extraction JSON: three requests, all of which must carry the same
        // instruction bytes.
        let client = Arc::new(RecordingSchemaClient::new(
            provider,
            vec![
                tool_call("call-1"),
                text("I reviewed it."),
                text(VALID_REVIEW),
            ],
        ));
        let mut agent = build(&client, base_builder().output_schema(schema.clone())).await;

        let result = agent
            .run("review".to_string().into())
            .await
            .expect("run completes");
        assert_eq!(result.structured_output, Some(expected_review()));

        let calls = client.calls();
        assert_eq!(calls.len(), 3, "{provider:?}: main, main, extraction");
        let expected = expected_system_prompt(&client, &schema);
        for (index, call) in calls.iter().enumerate() {
            assert_eq!(
                call.system_prompt(),
                expected,
                "{provider:?}: request {index} must carry the byte-identical section"
            );
        }
        assert!(expected.contains(OUTPUT_SCHEMA_INSTRUCTIONS_OPEN));
        assert!(expected.ends_with(OUTPUT_SCHEMA_INSTRUCTIONS_CLOSE));

        // Request-only: the committed transcript keeps the plain prompt.
        match agent.session().messages().first() {
            Some(Message::System(system)) => assert_eq!(
                system.content, SYSTEM_PROMPT,
                "{provider:?}: the section must never be written into the transcript"
            ),
            other => panic!("expected the committed system prompt, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn section_shows_the_provider_compiled_schema() {
    let schema = OutputSchema::new(json!({
        "type": "object",
        "properties": {"answer": {"type": "string"}},
        "required": ["answer"]
    }))
    .unwrap();
    let client = Arc::new(RecordingSchemaClient::new(
        Provider::Anthropic,
        vec![text(r#"{"answer":"42"}"#)],
    ));
    let mut agent = build(&client, base_builder().output_schema(schema)).await;
    agent.run("q".to_string().into()).await.expect("run");

    let calls = client.calls();
    assert!(
        calls[0]
            .system_prompt()
            .contains(r#""additionalProperties":false"#),
        "Anthropic validates against closed objects, so the model must be shown them"
    );
}

#[tokio::test]
async fn section_is_identical_across_runs_of_one_session() {
    let schema = review_schema();
    let client = Arc::new(RecordingSchemaClient::new(
        Provider::OpenAI,
        vec![text(VALID_REVIEW), text(VALID_REVIEW)],
    ));
    let mut agent = build(&client, base_builder().output_schema(schema)).await;
    agent.run("first".to_string().into()).await.expect("first");
    agent
        .run("second".to_string().into())
        .await
        .expect("second");

    let calls = client.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].system_prompt(), calls[1].system_prompt());
}

#[tokio::test]
async fn section_is_inserted_when_the_session_has_no_system_prompt() {
    let schema = review_schema();
    let client = Arc::new(RecordingSchemaClient::new(
        Provider::OpenAI,
        vec![text(VALID_REVIEW)],
    ));
    let mut agent = build(
        &client,
        AgentBuilder::new()
            .with_turn_state_handle(Arc::new(
                crate::agent::test_turn_state_handle::TestTurnStateHandle::new(),
            ))
            .output_schema(schema.clone()),
    )
    .await;
    agent.run("q".to_string().into()).await.expect("run");

    let calls = client.calls();
    let compiled = client.compile_schema(&schema).unwrap();
    assert_eq!(
        calls[0].system_prompt(),
        render_output_schema_instructions(&compiled.schema)
    );
    assert!(
        !agent
            .session()
            .messages()
            .iter()
            .any(|message| matches!(message, Message::System(_))),
        "the inserted section must stay request-only"
    );
}

// ---------------------------------------------------------------------------
// B. Validate-first
// ---------------------------------------------------------------------------

#[tokio::test]
async fn validate_first_success_skips_extraction_with_unchanged_result_shape() {
    let schema = review_schema();
    let client = Arc::new(RecordingSchemaClient::new(
        Provider::OpenAI,
        vec![tool_call("call-1"), text(VALID_REVIEW)],
    ));
    let mut agent = build(&client, base_builder().output_schema(schema)).await;

    let (result, events) = run_collecting(&mut agent, "review").await;
    let result = result.expect("run completes");

    assert_eq!(client.call_count(), 2, "no extraction request is sent");
    assert_eq!(result.structured_output, Some(expected_review()));
    assert_eq!(
        result.text, VALID_REVIEW,
        "text stays the primary final reply"
    );
    assert!(result.extraction_error.is_none());
    assert_eq!(result.turns, 2);
    assert_eq!(result.tool_calls, 1);

    // Same event contract as a successful extraction: RunCompleted announces
    // that extraction follows, then ExtractionSucceeded carries the value.
    let run_completed: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::RunCompleted {
                extraction_required,
                structured_output,
                ..
            } => Some((*extraction_required, structured_output.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(run_completed, vec![(true, None)]);
    let succeeded: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ExtractionSucceeded {
                structured_output,
                request_usage,
                origin,
                ..
            } => Some((structured_output.clone(), request_usage.len(), *origin)),
            _ => None,
        })
        .collect();
    assert_eq!(
        succeeded,
        vec![(expected_review(), 0, StructuredOutputOrigin::FinalReply)],
        "the success records that the final reply produced the value and no extraction request ran"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentEvent::ExtractionFailed { .. }))
    );
    let run_completed_index = events
        .iter()
        .position(|event| matches!(event, AgentEvent::RunCompleted { .. }))
        .unwrap();
    let succeeded_index = events
        .iter()
        .position(|event| matches!(event, AgentEvent::ExtractionSucceeded { .. }))
        .unwrap();
    assert!(run_completed_index < succeeded_index);

    // No extraction rows land in the transcript.
    let messages = agent.session().messages();
    assert!(!messages.iter().any(is_extraction_prompt));
    assert!(matches!(messages.last(), Some(Message::BlockAssistant(_))));
    assert_eq!(
        agent.session().last_assistant_text().as_deref(),
        Some(VALID_REVIEW)
    );
}

#[tokio::test]
async fn validate_first_accepts_code_fenced_json() {
    let schema = review_schema();
    let fenced = format!("```json\n{VALID_REVIEW}\n```");
    let client = Arc::new(RecordingSchemaClient::new(
        Provider::Gemini,
        vec![text(&fenced)],
    ));
    let mut agent = build(&client, base_builder().output_schema(schema)).await;
    let result = agent.run("q".to_string().into()).await.expect("run");

    assert_eq!(client.call_count(), 1);
    assert_eq!(result.structured_output, Some(expected_review()));
    assert_eq!(
        result.text, fenced,
        "text is the reply as the model wrote it"
    );
}

#[tokio::test]
async fn validate_first_unwraps_a_named_object_wrapper() {
    let schema = review_schema().with_name("review");
    let wrapped = format!(r#"{{"review":{VALID_REVIEW}}}"#);
    let client = Arc::new(RecordingSchemaClient::new(
        Provider::OpenAI,
        vec![text(&wrapped)],
    ));
    let mut agent = build(&client, base_builder().output_schema(schema)).await;
    let result = agent.run("q".to_string().into()).await.expect("run");

    assert_eq!(client.call_count(), 1);
    assert_eq!(result.structured_output, Some(expected_review()));
}

#[tokio::test]
async fn validate_first_accepts_an_omitted_optional_field_and_an_empty_array() {
    let schema = review_schema();
    let client = Arc::new(RecordingSchemaClient::new(
        Provider::OpenAI,
        vec![text(r#"{"verdict":"approve","comments":[]}"#)],
    ));
    let mut agent = build(&client, base_builder().output_schema(schema)).await;
    let result = agent.run("q".to_string().into()).await.expect("run");

    assert_eq!(client.call_count(), 1);
    assert_eq!(
        result.structured_output,
        Some(json!({"verdict": "approve", "comments": []}))
    );
}

/// A schema with `format` keywords that native constrained decoding enforces
/// on the extraction request (Anthropic always, OpenAI with `strict`).
fn format_schema() -> OutputSchema {
    OutputSchema::new(json!({
        "type": "object",
        "properties": {
            "due": {"type": "string", "format": "date-time"},
            "owner": {"type": "string", "format": "email"}
        },
        "required": ["due", "owner"]
    }))
    .expect("valid schema")
}

const CONFORMING_FORMATS: &str = r#"{"due":"2026-09-25T10:00:00Z","owner":"backend@example.com"}"#;

/// Validate-first asserts declared `format` keywords. A final reply that is
/// the right shape but breaks a format is not accepted: it takes the
/// unchanged extraction path, whose request is the one native decoding
/// constrains. A conforming reply still skips extraction.
#[tokio::test]
async fn validate_first_asserts_declared_formats_and_falls_back_to_extraction() {
    for provider in [Provider::Anthropic, Provider::OpenAI, Provider::Gemini] {
        let client = Arc::new(RecordingSchemaClient::new(
            provider,
            vec![
                text(r#"{"due":"next Friday","owner":"the backend team"}"#),
                text(CONFORMING_FORMATS),
            ],
        ));
        let mut agent = build(&client, base_builder().output_schema(format_schema())).await;
        let (result, events) = run_collecting(&mut agent, "q").await;
        let result = result.unwrap_or_else(|error| panic!("{provider:?}: run failed: {error}"));

        assert_eq!(
            client.call_count(),
            2,
            "{provider:?}: a format violation must not be accepted by validate-first"
        );
        let origins: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::ExtractionSucceeded { origin, .. } => Some(*origin),
                _ => None,
            })
            .collect();
        assert_eq!(
            origins,
            vec![StructuredOutputOrigin::ExtractionRequest],
            "{provider:?}: the value came from the extraction request"
        );
        assert_eq!(
            result.structured_output,
            Some(serde_json::from_str::<Value>(CONFORMING_FORMATS).unwrap()),
            "{provider:?}: the extraction answer is the structured output"
        );
        assert_eq!(
            client.calls()[1].last_user_text(),
            DEFAULT_EXTRACTION_PROMPT,
            "{provider:?}: the fallback is the unchanged extraction request"
        );

        let client = Arc::new(RecordingSchemaClient::new(
            provider,
            vec![text(CONFORMING_FORMATS)],
        ));
        let mut agent = build(&client, base_builder().output_schema(format_schema())).await;
        let result = agent.run("q".to_string().into()).await.expect("run");
        assert_eq!(
            client.call_count(),
            1,
            "{provider:?}: conforming formats still skip extraction"
        );
        assert_eq!(
            result.structured_output,
            Some(serde_json::from_str::<Value>(CONFORMING_FORMATS).unwrap())
        );
    }
}

/// The extraction phase's own validation is unchanged: it treats `format` as
/// an annotation, as it did before validate-first existed, and relies on the
/// provider's native schema slot to constrain the values.
#[tokio::test]
async fn extraction_phase_validation_still_treats_format_as_an_annotation() {
    let off_format = r#"{"due":"soon","owner":"someone"}"#;
    let client = Arc::new(RecordingSchemaClient::new(
        Provider::OpenAI,
        vec![text("prose answer"), text(off_format)],
    ));
    let mut agent = build(&client, base_builder().output_schema(format_schema())).await;
    let result = agent.run("q".to_string().into()).await.expect("run");

    assert_eq!(
        client.call_count(),
        2,
        "extraction runs for the prose reply"
    );
    assert_eq!(
        result.structured_output,
        Some(serde_json::from_str::<Value>(off_format).unwrap()),
        "extraction acceptance is unchanged"
    );
}

/// Every way a final reply can miss the schema falls back to extraction, and
/// the extraction answer (not the rejected reply) becomes the structured
/// output.
#[tokio::test]
async fn validate_first_rejections_fall_back_to_extraction() {
    let cases: &[(&str, &str)] = &[
        ("prose", "Looks good to me."),
        ("prose around json", &format!("Here you go: {VALID_REVIEW}")),
        ("invalid json", r#"{"verdict":"approve","comments":[}"#),
        ("missing required field", r#"{"comments":[]}"#),
        ("enum violation", r#"{"verdict":"maybe","comments":[]}"#),
        (
            "nested type violation",
            r#"{"verdict":"approve","comments":[{"path":"a","line":"ten","category":"bug","body":"b"}]}"#,
        ),
        (
            "nested minimum violation",
            r#"{"verdict":"approve","comments":[{"path":"a","line":0,"category":"bug","body":"b"}]}"#,
        ),
        (
            "additional property",
            r#"{"verdict":"approve","comments":[],"extra":true}"#,
        ),
        (
            "nested additional property",
            r#"{"verdict":"approve","comments":[{"path":"a","line":1,"category":"bug","body":"b","severity":"high"}]}"#,
        ),
        ("array root", "[]"),
    ];
    for (label, reply) in cases {
        let client = Arc::new(RecordingSchemaClient::new(
            Provider::OpenAI,
            vec![text(reply), text(VALID_REVIEW)],
        ));
        let mut agent = build(&client, base_builder().output_schema(review_schema())).await;
        let result = agent
            .run("q".to_string().into())
            .await
            .unwrap_or_else(|error| panic!("{label}: run failed: {error}"));

        assert_eq!(client.call_count(), 2, "{label}: extraction must run");
        assert_eq!(
            result.structured_output,
            Some(expected_review()),
            "{label}: the extraction answer is the structured output"
        );
        assert_eq!(
            result.text, *reply,
            "{label}: text stays the primary final reply"
        );
        let extraction = &client.calls()[1];
        assert_eq!(extraction.last_user_text(), DEFAULT_EXTRACTION_PROMPT);
    }
}

/// The fallback runs today's extraction request unchanged for every provider
/// identity: the default prompt, temperature 0, no tools, the provider's typed
/// native schema slot with the configured (non-strict) schema, native search
/// cleared, and the same instruction section as the main turn.
#[tokio::test]
async fn fallback_extraction_request_is_unchanged_for_every_provider() {
    for provider in [
        Provider::Anthropic,
        Provider::OpenAI,
        Provider::Gemini,
        Provider::SelfHosted,
        Provider::Other,
    ] {
        let schema = review_schema();
        let client = Arc::new(RecordingSchemaClient::new(
            provider,
            vec![
                tool_call("call-1"),
                text("prose answer"),
                text(VALID_REVIEW),
            ],
        ));
        let mut builder = base_builder().output_schema(schema.clone());
        if let Some(defaults) = web_search_defaults(provider) {
            builder = builder.provider_tool_defaults(defaults);
        }
        let mut agent = build(&client, builder).await;

        let result = agent
            .run("review".to_string().into())
            .await
            .expect("run completes");
        assert_eq!(result.structured_output, Some(expected_review()));
        assert_eq!(result.text, "prose answer");

        let calls = client.calls();
        assert_eq!(calls.len(), 3, "{provider:?}");
        for main in &calls[..2] {
            assert_eq!(main.tool_names, vec!["lookup".to_string()]);
            assert!(
                structured_output_slot(main.provider_params.as_ref()).is_none(),
                "{provider:?}: main turns carry no native schema slot"
            );
            assert_eq!(main.temperature, None);
        }
        let extraction = &calls[2];
        assert!(
            extraction.tool_names.is_empty(),
            "{provider:?}: no tools on extraction"
        );
        assert_eq!(extraction.temperature, Some(0.0));
        assert_eq!(extraction.last_user_text(), DEFAULT_EXTRACTION_PROMPT);
        assert_eq!(extraction.system_prompt(), calls[0].system_prompt());
        if provider == Provider::Other {
            assert!(
                structured_output_slot(extraction.provider_params.as_ref()).is_none(),
                "Other has no typed slot and stays prompt-based"
            );
        } else {
            let slot = structured_output_slot(extraction.provider_params.as_ref())
                .unwrap_or_else(|| panic!("{provider:?}: extraction carries the native slot"));
            assert_eq!(
                slot, &schema,
                "{provider:?}: the configured schema, unchanged"
            );
            assert!(!slot.strict, "strict defaults stay false");
            assert!(
                native_search_present(calls[0].provider_params.as_ref()),
                "{provider:?}: main turn keeps native search"
            );
            assert!(
                !native_search_present(extraction.provider_params.as_ref()),
                "{provider:?}: extraction clears native search"
            );
        }

        // The transcript records the extraction exchange as before.
        assert!(agent.session().messages().iter().any(is_extraction_prompt));
    }
}

#[tokio::test]
async fn extraction_retry_after_invalid_extraction_output() {
    let client = Arc::new(RecordingSchemaClient::new(
        Provider::Anthropic,
        vec![
            text("prose"),
            text(r#"{"verdict":"approve"}"#),
            text(VALID_REVIEW),
        ],
    ));
    let mut agent = build(
        &client,
        base_builder()
            .output_schema(review_schema())
            .structured_output_retries(2),
    )
    .await;
    let result = agent.run("q".to_string().into()).await.expect("run");

    assert_eq!(client.call_count(), 3);
    assert_eq!(result.structured_output, Some(expected_review()));
    let retry = &client.calls()[2];
    let retry_prompt = retry.last_user_text();
    assert!(
        retry_prompt.starts_with("The previous output was invalid: Schema validation failed"),
        "the unchanged retry prompt names the failure: {retry_prompt}"
    );
    assert_eq!(retry.temperature, Some(0.0));
    assert!(retry.tool_names.is_empty());
}

/// Validate-first never consumes an extraction attempt: with `retries = 1`
/// the extraction phase still gets exactly two attempts, and the reported
/// attempt count is the same as before validate-first existed.
#[tokio::test]
async fn exhausted_extraction_reports_unchanged_attempts() {
    let client = Arc::new(RecordingSchemaClient::new(
        Provider::OpenAI,
        vec![text("prose"), text("still prose"), text("more prose")],
    ));
    let mut agent = build(
        &client,
        base_builder()
            .output_schema(review_schema())
            .structured_output_retries(1),
    )
    .await;
    let (result, events) = run_collecting(&mut agent, "q").await;
    let result = result.expect("an exhausted extraction still completes the main run");

    assert_eq!(client.call_count(), 3, "main + two extraction attempts");
    assert!(result.structured_output.is_none());
    let error = result.extraction_error.expect("extraction error");
    assert_eq!(error.attempts, 2);
    assert_eq!(error.last_output, "prose");
    assert!(error.reason.contains("Invalid JSON"), "{}", error.reason);
    assert_eq!(result.text, "prose");
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::ExtractionFailed { attempts: 2, .. }))
    );
}

#[tokio::test]
async fn validator_fault_falls_through_to_the_unchanged_extraction_failure() {
    let client = Arc::new(
        RecordingSchemaClient::new(
            Provider::OpenAI,
            vec![text(r#"{"answer":"42"}"#), text(r#"{"answer":"42"}"#)],
        )
        .with_compile(CompileMode::ValidatorFault),
    );
    let schema = OutputSchema::new(json!({
        "type": "object",
        "properties": {"answer": {"type": "string"}},
        "required": ["answer"]
    }))
    .unwrap();
    let mut agent = build(&client, base_builder().output_schema(schema)).await;
    let result = agent.run("q".to_string().into()).await.expect("run");

    assert_eq!(
        client.call_count(),
        2,
        "a validator fault is not a validate-first verdict; extraction runs as before"
    );
    assert!(result.structured_output.is_none());
    let error = result
        .extraction_error
        .expect("the fault is reported as before");
    assert!(
        error.reason.to_lowercase().contains("schema"),
        "reason: {}",
        error.reason
    );
}

/// A schema strict mode cannot express (optional properties, `oneOf`,
/// `pattern`, `minItems`, an open object) keeps its non-strict default: it is
/// shown and sent unchanged, and validate-first enforces every keyword.
#[tokio::test]
async fn schema_strict_mode_cannot_express_is_passed_through_and_enforced() {
    let schema = OutputSchema::new(json!({
        "type": "object",
        "properties": {
            "id": {"type": "string", "pattern": "^[A-Z]{3}-[0-9]+$"},
            "tags": {"type": "array", "items": {"type": "string"}, "minItems": 1},
            "value": {"oneOf": [{"type": "integer"}, {"type": "string"}]},
            "note": {"type": "string"}
        },
        "required": ["id", "tags", "value"]
    }))
    .unwrap();
    assert!(!schema.strict);

    // Valid under every keyword, with an extra property the open object allows.
    let valid = r#"{"id":"ABC-12","tags":["x"],"value":7,"extra":1}"#;
    let client = Arc::new(RecordingSchemaClient::new(
        Provider::OpenAI,
        vec![text(valid)],
    ));
    let mut agent = build(&client, base_builder().output_schema(schema.clone())).await;
    let result = agent.run("q".to_string().into()).await.expect("run");
    assert_eq!(client.call_count(), 1);
    assert_eq!(
        result.structured_output,
        Some(serde_json::from_str(valid).unwrap())
    );
    assert!(
        client.calls()[0]
            .system_prompt()
            .contains(&schema.schema.as_value().to_string()),
        "a non-strict schema is shown exactly as configured"
    );

    for (label, reply) in [
        ("pattern", r#"{"id":"abc","tags":["x"],"value":7}"#),
        ("minItems", r#"{"id":"ABC-1","tags":[],"value":7}"#),
        ("oneOf", r#"{"id":"ABC-1","tags":["x"],"value":1.5}"#),
    ] {
        let client = Arc::new(RecordingSchemaClient::new(
            Provider::OpenAI,
            vec![text(reply), text(valid)],
        ));
        let mut agent = build(&client, base_builder().output_schema(schema.clone())).await;
        let result = agent.run("q".to_string().into()).await.expect("run");
        assert_eq!(client.call_count(), 2, "{label}: falls back to extraction");
        let calls = client.calls();
        let slot = structured_output_slot(calls[1].provider_params.as_ref()).expect("native slot");
        assert!(!slot.strict, "{label}: strict stays false");
        assert_eq!(
            result.structured_output,
            Some(serde_json::from_str(valid).unwrap())
        );
    }
}

// ---------------------------------------------------------------------------
// Runs that end before a final reply
// ---------------------------------------------------------------------------

#[tokio::test]
async fn turn_limit_ends_the_run_without_extraction() {
    let client = Arc::new(RecordingSchemaClient::new(
        Provider::OpenAI,
        vec![tool_call("call-1"), text(VALID_REVIEW)],
    ));
    let mut agent = build(&client, base_builder().output_schema(review_schema())).await;
    agent.config.max_turns = Some(1);

    let error = agent
        .run("q".to_string().into())
        .await
        .expect_err("the turn limit terminalizes the run");
    assert!(
        matches!(
            error,
            AgentError::TerminalFailure {
                cause_kind: crate::TurnTerminalCauseKind::TurnLimitReached,
                ..
            }
        ),
        "unexpected error: {error:?}"
    );
    assert_eq!(client.call_count(), 1, "no extraction after a turn limit");
    assert!(
        client.calls()[0]
            .system_prompt()
            .contains(OUTPUT_SCHEMA_INSTRUCTIONS_OPEN)
    );
    assert!(!agent.session().messages().iter().any(is_extraction_prompt));
}

#[tokio::test]
async fn budget_limit_ends_the_run_without_extraction() {
    let client = Arc::new(RecordingSchemaClient::new(
        Provider::OpenAI,
        vec![tool_call("call-1"), text(VALID_REVIEW)],
    ));
    let mut agent = build(
        &client,
        base_builder()
            .output_schema(review_schema())
            .budget(crate::budget::BudgetLimits {
                max_tokens: Some(50),
                ..Default::default()
            }),
    )
    .await;

    let outcome = agent.run("q".to_string().into()).await;
    if let Ok(result) = &outcome {
        assert!(result.structured_output.is_none());
    }
    assert_eq!(
        client.call_count(),
        1,
        "the budget stops the tool loop and no extraction request is sent"
    );
    assert!(!agent.session().messages().iter().any(is_extraction_prompt));
}

// ---------------------------------------------------------------------------
// Provider cache-breakpoint evidence
// ---------------------------------------------------------------------------

/// A breakpoint authored over a request that carries the section describes a
/// system prompt the transcript does not contain. It is not promoted, and no
/// discard is reported, because nothing moved.
#[tokio::test]
async fn claims_over_projected_requests_are_not_promoted_or_reported() {
    let client = Arc::new(
        RecordingSchemaClient::new(
            Provider::OpenAI,
            vec![tool_call("call-1"), text(VALID_REVIEW)],
        )
        .with_claims_over_request(),
    );
    let mut agent = build(&client, base_builder().output_schema(review_schema())).await;
    let (result, events) = run_collecting(&mut agent, "q").await;
    result.expect("run completes");

    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentEvent::ProviderCacheBreakpointsDiscarded { .. })),
        "a request-only projection is not a moved anchor"
    );
    assert!(
        agent
            .session()
            .authored_cache_breakpoints()
            .expect("evidence decodes")
            .is_empty()
    );
}

/// Without a schema, the same claim binds and is persisted exactly as before.
#[tokio::test]
async fn claims_without_a_schema_are_still_promoted() {
    let client = Arc::new(
        RecordingSchemaClient::new(Provider::OpenAI, vec![tool_call("call-1"), text("done")])
            .with_claims_over_request(),
    );
    let mut agent = build(&client, base_builder()).await;
    let (result, events) = run_collecting(&mut agent, "q").await;
    result.expect("run completes");

    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentEvent::ProviderCacheBreakpointsDiscarded { .. }))
    );
    assert!(
        !agent
            .session()
            .authored_cache_breakpoints()
            .expect("evidence decodes")
            .is_empty(),
        "an unprojected request's claim binds to the transcript"
    );
}

// ---------------------------------------------------------------------------
// Provider-owned system prompts
// ---------------------------------------------------------------------------

/// Gemini explicit context caching owns the system instruction, so the loop
/// leaves such requests untouched. Validate-first and the extraction fallback
/// still apply.
#[tokio::test]
async fn gemini_cached_content_requests_are_not_projected() {
    let cached = ProviderParamsOverride {
        provider_tag: Some(ProviderTag::Gemini(GeminiProviderTag {
            cached_content_name: Some("cachedContents/stable-prefix".to_string()),
            ..Default::default()
        })),
        ..Default::default()
    };

    // Final reply already valid: accepted without an extraction request.
    let client = Arc::new(RecordingSchemaClient::new(
        Provider::Gemini,
        vec![text(VALID_REVIEW)],
    ));
    let mut agent = build(
        &client,
        base_builder()
            .output_schema(review_schema())
            .provider_params(cached.clone()),
    )
    .await;
    let result = agent.run("q".to_string().into()).await.expect("run");
    assert_eq!(client.call_count(), 1);
    assert_eq!(result.structured_output, Some(expected_review()));
    assert_eq!(client.calls()[0].system_prompt(), SYSTEM_PROMPT);

    // Prose: the unchanged extraction path runs, still without a section.
    let client = Arc::new(RecordingSchemaClient::new(
        Provider::Gemini,
        vec![text("prose"), text(VALID_REVIEW)],
    ));
    let mut agent = build(
        &client,
        base_builder()
            .output_schema(review_schema())
            .provider_params(cached),
    )
    .await;
    let result = agent.run("q".to_string().into()).await.expect("run");
    let calls = client.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(result.structured_output, Some(expected_review()));
    for call in &calls {
        assert_eq!(call.system_prompt(), SYSTEM_PROMPT);
    }
    assert!(structured_output_slot(calls[1].provider_params.as_ref()).is_some());
}

//! Structured-output request shapes for the OpenAI Realtime text adapter.
//!
//! The adapter replays the whole transcript as `conversation.item.create`
//! events and keeps canonical System messages as conversation items (never
//! session `instructions`), so the schema section the agent loop appends to
//! the leading system prompt reaches the model as the first System item. The
//! Realtime protocol has no native response-schema slot, so the extraction
//! request relies on the section and the extraction prompt alone.
//!
//! `stream()` opens a WebSocket to the fixed OpenAI Realtime endpoint, so
//! these tests compose the client events it sends from the same helpers it
//! uses (`project_replay_messages`, `convert_messages`, `build_tools`,
//! `realtime_max_output_tokens`, `resolve_realtime_temperature`) in the same
//! order, and pin the serialized events.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use oai_rt_rs::ClientEvent;
use oai_rt_rs::protocol::models::{
    ConversationMode, OutputModalities, ResponseConfig, SessionUpdate, SessionUpdateConfig,
};
use serde_json::{Value, json};

use super::{
    build_tools, convert_messages, realtime_max_output_tokens, resolve_realtime_temperature,
};
use crate::OpenAiRealtimeTextAdapter;
use crate::structured_output_request_tests::{
    EXTRACTION_PROMPT, SYSTEM_PROMPT, assert_only_the_section_differs, project, review_schema,
    run_requests, run_transcripts, section_for,
};
use meerkat_core::lifecycle::run_primitive::OpenAiProviderTag;
use meerkat_core::structured_output::render_output_schema_instructions;
use meerkat_core::{Message, UserMessage};
use meerkat_llm_core::{LlmClient, LlmRequest};

const MODEL: &str = "gpt-realtime-2";

/// The client events `stream()` sends for one request, serialized: the
/// `session.update`, one `conversation.item.create` per replayed item, then
/// `response.create`.
fn realtime_events(adapter: &OpenAiRealtimeTextAdapter, request: &LlmRequest) -> Vec<Value> {
    let mut projected = request.clone();
    projected.messages = adapter
        .project_replay_messages(&request.messages)
        .expect("replay projection");
    let history_items = convert_messages(&projected.messages).expect("convert");
    let tools = build_tools(&projected).expect("tools");
    let mut events = vec![ClientEvent::SessionUpdate {
        event_id: None,
        session: Box::new(SessionUpdate {
            config: SessionUpdateConfig {
                output_modalities: Some(OutputModalities::Text),
                instructions: None,
                tools: Some(tools.clone()),
                ..SessionUpdateConfig::default()
            },
        }),
    }];
    events.extend(
        history_items
            .into_iter()
            .map(|item| ClientEvent::ConversationItemCreate {
                event_id: None,
                previous_item_id: None,
                item: Box::new(item),
            }),
    );
    events.push(ClientEvent::ResponseCreate {
        event_id: None,
        response: Some(Box::new(ResponseConfig {
            conversation: Some(ConversationMode::None),
            output_modalities: Some(OutputModalities::Text),
            instructions: None,
            tools: Some(tools),
            max_output_tokens: Some(realtime_max_output_tokens(projected.max_tokens).unwrap()),
            temperature: resolve_realtime_temperature(projected.temperature).unwrap(),
            ..ResponseConfig::default()
        })),
    });
    events
        .iter()
        .map(|event| serde_json::to_value(event).expect("serialize client event"))
        .collect()
}

fn items(events: &[Value]) -> Vec<&Value> {
    events
        .iter()
        .filter(|event| event["type"] == "conversation.item.create")
        .map(|event| &event["item"])
        .collect()
}

fn input_text(item: &Value) -> String {
    item["content"]
        .as_array()
        .expect("content parts")
        .iter()
        .filter_map(|part| part["text"].as_str())
        .collect()
}

fn tool_names(event: &Value, key: &str) -> Vec<String> {
    event[key]["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| tool["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn realtime_events_carry_the_section_in_the_leading_system_item() {
    let adapter = OpenAiRealtimeTextAdapter::new("test-key");
    let schema = review_schema();
    let section = section_for(&adapter, &schema);
    let tag = OpenAiProviderTag::default();
    let with_requests = run_requests(&adapter, MODEL, &schema, &tag, true);
    let without_requests = run_requests(&adapter, MODEL, &schema, &tag, false);

    let mut first_items = Vec::new();
    for ((label, with), (_, without)) in with_requests.iter().zip(&without_requests) {
        let with_events = realtime_events(&adapter, with);
        let without_events = realtime_events(&adapter, without);
        assert_only_the_section_differs(
            label,
            &Value::Array(with_events.clone()),
            &Value::Array(without_events),
            &section,
        );

        assert_eq!(with_events[0]["type"], "session.update", "{label}");
        assert!(
            with_events[0]["session"].get("instructions").is_none(),
            "{label}: the system prompt is never lifted into session instructions"
        );
        let last = with_events.last().unwrap();
        assert_eq!(last["type"], "response.create", "{label}");
        assert!(last["response"].get("instructions").is_none(), "{label}");

        let items = items(&with_events);
        assert_eq!(items[0]["role"], "system", "{label}: the system item leads");
        assert_eq!(
            input_text(items[0]),
            format!("{SYSTEM_PROMPT}\n\n{section}"),
            "{label}"
        );
        assert!(
            items[1..].iter().all(|item| item["role"] != "system"),
            "{label}: no second system item"
        );
        first_items.push(items[0].clone());
    }
    assert_eq!(
        first_items[0], first_items[1],
        "byte-identical across turns"
    );
    assert_eq!(
        first_items[0], first_items[2],
        "the extraction request keeps it"
    );
}

#[test]
fn realtime_main_turns_keep_tools_and_extraction_has_no_native_schema_slot() {
    let adapter = OpenAiRealtimeTextAdapter::new("test-key");
    let schema = review_schema();
    let requests = run_requests(
        &adapter,
        MODEL,
        &schema,
        &OpenAiProviderTag::default(),
        true,
    );
    for (label, request) in &requests[..2] {
        let events = realtime_events(&adapter, request);
        assert_eq!(
            tool_names(&events[0], "session"),
            vec!["lookup".to_string()],
            "{label}"
        );
        assert_eq!(
            tool_names(events.last().unwrap(), "response"),
            vec!["lookup".to_string()],
            "{label}"
        );
    }

    let (_, extraction) = &requests[2];
    let events = realtime_events(&adapter, extraction);
    assert_eq!(
        events[0]["session"]["tools"],
        json!([]),
        "no tools on extraction"
    );
    let response = &events.last().unwrap()["response"];
    assert_eq!(response["tools"], json!([]));
    let temperature = response["temperature"].as_f64().expect("temperature");
    assert!(
        temperature.abs() < f64::EPSILON,
        "extraction runs at temperature 0"
    );
    let serialized = Value::Array(events.clone()).to_string();
    for native_slot in ["json_schema", "response_format", "\"format\""] {
        assert!(
            !serialized.contains(native_slot),
            "the Realtime protocol has no native schema slot ({native_slot}): {serialized}"
        );
    }
    let items = items(&events);
    let last = items.last().unwrap();
    assert_eq!(last["role"], "user");
    assert_eq!(input_text(last), EXTRACTION_PROMPT);
}

/// The Realtime adapter compiles schemas as a pass-through, so the section
/// shows the configured schema unchanged.
#[test]
fn realtime_section_shows_the_configured_schema() {
    let adapter = OpenAiRealtimeTextAdapter::new("test-key");
    let schema = review_schema();
    assert_eq!(
        section_for(&adapter, &schema),
        render_output_schema_instructions(schema.schema.as_value())
    );
}

#[test]
fn realtime_inserted_section_is_the_only_system_item() {
    let adapter = OpenAiRealtimeTextAdapter::new("test-key");
    let schema = review_schema();
    let messages = vec![Message::User(UserMessage::text("review"))];
    let request = LlmRequest::new(MODEL, project(&adapter, &schema, &messages));
    let events = realtime_events(&adapter, &request);
    let items = items(&events);
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["role"], "system");
    assert_eq!(input_text(items[0]), section_for(&adapter, &schema));
    assert_eq!(items[1]["role"], "user");
}

#[test]
fn realtime_requests_without_a_schema_are_unchanged() {
    let adapter = OpenAiRealtimeTextAdapter::new("test-key");
    let request = LlmRequest::new(MODEL, run_transcripts().turn_one);
    let events = realtime_events(&adapter, &request);
    let items = items(&events);
    assert_eq!(input_text(items[0]), SYSTEM_PROMPT);
    assert!(
        !Value::Array(events.clone())
            .to_string()
            .contains(meerkat_core::structured_output::OUTPUT_SCHEMA_INSTRUCTIONS_OPEN)
    );
}

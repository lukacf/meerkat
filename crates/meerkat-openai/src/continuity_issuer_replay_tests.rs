//! OpenAI-family continuity material is replayed only to the service this
//! process received it from.
//!
//! The public OpenAI API, the ChatGPT backend, Azure OpenAI, GitHub Copilot
//! and self-hosted servers speak one Responses grammar, but encrypted
//! reasoning, item ids and response ids mean something only to the service
//! that issued them. Which service issued a piece of material is known only
//! to the process that received it (`meerkat_core::OpenAiContinuityLedger`):
//! it is not stored, so a transcript read from a store names no issuer.
//!
//! Every transcript here is produced the way a session produces one: the
//! client of a service receives responses from a local server standing in
//! for that service and the committed output becomes the next request's
//! history. The request body that then reaches the wire is checked:
//!
//! - a step this process received from another service, or did not receive
//!   at all (a transcript from a store, a new process, a forgotten
//!   response), is sent in the form every service already gets for a history
//!   from another provider family: no reasoning item, no hosted-tool item, no
//!   provider item id, no `previous_response_id`, tool calls and their
//!   outputs paired by `call_id`;
//! - a step this process received from the service itself replays exactly as
//!   before;
//! - a step is admitted as a whole or not at all.
//!
//! A ledger stands for one process: rows that need a second process, or the
//! same response bytes from two services, give their clients a ledger of
//! their own.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::sync::Arc;

use axum::{Json, Router, response::IntoResponse};
use futures::StreamExt;
use meerkat_core::{
    AssistantBlock, BlockAssistantMessage, Message, OpenAiContinuityLedger, ProviderMeta,
    StopReason, ToolDef, ToolResult, UserMessage,
};
use meerkat_llm_core::{LlmClient, LlmEvent, LlmRequest};
use serde_json::{Value, json};
use tokio::net::TcpListener;

use crate::client_compatible::OpenAiCompatibleMode;
use crate::{AzureOpenAiWireConfig, OpenAiClient, OpenAiCompatibleClient};

const MODEL: &str = "gpt-5.4";
const SELF_HOSTED_REMOTE_MODEL: &str = "served-model";

type Ledger = Arc<OpenAiContinuityLedger>;

/// What one process knows: nothing yet.
fn new_process() -> Ledger {
    Arc::new(OpenAiContinuityLedger::new())
}

// ---------------------------------------------------------------------------
// The wire
// ---------------------------------------------------------------------------

/// The local servers of one test, by name, each standing in for one
/// endpoint of a service: an issuer is a kind of service at an endpoint, so
/// a service keeps its address for the whole test, across the clients and
/// the processes that reach it. Every server answers with the payload set
/// for the next request and captures the body it received.
#[derive(Default)]
struct Wire {
    endpoints: std::collections::HashMap<String, String>,
    payload: String,
    bodies: Vec<Value>,
}

thread_local! {
    static WIRE: std::cell::RefCell<Wire> = std::cell::RefCell::new(Wire::default());
}

async fn capture_body(Json(body): Json<Value>) -> impl IntoResponse {
    let payload = WIRE.with(|wire| {
        let mut wire = wire.borrow_mut();
        wire.bodies.push(body);
        wire.payload.clone()
    });
    ([("content-type", "text/event-stream")], payload)
}

/// The base URL of the endpoint `name` of this test, started on first use.
fn endpoint(name: &str) -> String {
    if let Some(base) = WIRE.with(|wire| wire.borrow().endpoints.get(name).cloned()) {
        return base;
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    let base = format!("http://{}", listener.local_addr().expect("local addr"));
    let listener = TcpListener::from_std(listener).expect("tokio listener");
    tokio::spawn(async move {
        let app = Router::new().fallback(capture_body);
        axum::serve(listener, app).await.expect("serve");
    });
    WIRE.with(|wire| {
        wire.borrow_mut()
            .endpoints
            .insert(name.to_string(), base.clone());
    });
    base
}

/// Every service that speaks the OpenAI Responses grammar, with two
/// self-hosted servers so one self-hosted server is also foreign to another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Service {
    OpenAiApi,
    ChatGptBackend,
    AzureOpenAi,
    Copilot,
    SelfHostedA,
    SelfHostedB,
}

impl Service {
    const ALL: [Self; 6] = [
        Self::OpenAiApi,
        Self::ChatGptBackend,
        Self::AzureOpenAi,
        Self::Copilot,
        Self::SelfHostedA,
        Self::SelfHostedB,
    ];

    /// Another service, to be foreign to this one.
    fn other(self) -> Self {
        Self::ALL
            .into_iter()
            .find(|service| *service != self)
            .expect("another service")
    }

    /// The client that speaks to this service's endpoint of the test in the
    /// process `ledger` stands for, built with the builders the provider
    /// runtime and the factory use for it.
    fn client(self, ledger: &Ledger) -> Arc<dyn LlmClient> {
        let base = endpoint(&format!("{self:?}"));
        let openai = || {
            OpenAiClient::new_with_base_url("test-key".to_string(), base.clone())
                .with_continuity_ledger(Arc::clone(ledger))
        };
        let self_hosted_server = |server: &str| {
            Arc::new(
                self_hosted(base.clone())
                    .with_self_hosted_server(server)
                    .with_continuity_ledger(Arc::clone(ledger)),
            )
        };
        match self {
            Self::OpenAiApi => Arc::new(openai()),
            Self::ChatGptBackend => Arc::new(openai().with_chatgpt_backend_wire()),
            Self::AzureOpenAi => {
                Arc::new(openai().with_azure_openai_wire(AzureOpenAiWireConfig::default()))
            }
            Self::Copilot => Arc::new(
                openai()
                    .with_responses_path("responses")
                    .with_copilot_responses_item_identity()
                    .with_copilot_backend_wire(),
            ),
            Self::SelfHostedA => self_hosted_server("local-a"),
            Self::SelfHostedB => self_hosted_server("local-b"),
        }
    }
}

/// A self-hosted Responses client as the factory builds one, before it
/// declares which configured server it speaks to.
fn self_hosted(base: String) -> OpenAiCompatibleClient {
    OpenAiCompatibleClient::new_with_options(
        OpenAiCompatibleMode::Responses,
        SELF_HOSTED_REMOTE_MODEL.to_string(),
        format!("{base}/v1"),
        None,
        crate::OpenAiCompatibleClientOptions {
            supports_temperature: true,
            supports_thinking: true,
            supports_reasoning: true,
            supports_image_tool_results: false,
        },
    )
    .with_image_input_support(true)
    .with_static_credential()
}

/// A self-hosted client that never declared its server, at the endpoint
/// `name` of the test.
fn undeclared_self_hosted(name: &str, ledger: &Ledger) -> Arc<dyn LlmClient> {
    Arc::new(self_hosted(endpoint(name)).with_continuity_ledger(Arc::clone(ledger)))
}

fn weather_tool() -> Arc<ToolDef> {
    Arc::new(ToolDef {
        audience: Default::default(),
        name: "get_weather".into(),
        description: "Current weather of a city".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {"city": {"type": "string"}},
            "required": ["city"]
        }),
        provenance: None,
    })
}

fn request(messages: Vec<Message>) -> LlmRequest {
    let mut request = LlmRequest::new(MODEL, messages);
    request.tools = vec![weather_tool()];
    request
}

/// Arm the wire for one exchange: the next request is answered with
/// `payload`.
fn answer_with(payload: &str) {
    WIRE.with(|wire| {
        let mut wire = wire.borrow_mut();
        wire.payload = payload.to_string();
        wire.bodies.clear();
    });
}

/// The bodies that reached the wire since [`answer_with`].
fn bodies_on_the_wire() -> Vec<Value> {
    WIRE.with(|wire| std::mem::take(&mut wire.borrow_mut().bodies))
}

/// Send `request` through `client`, answered with `payload`. Returns the
/// bodies that reached the wire and the events the client produced.
async fn send(
    client: Arc<dyn LlmClient>,
    request: &LlmRequest,
    payload: &str,
) -> (Vec<Value>, Vec<LlmEvent>) {
    answer_with(payload);
    let mut events = Vec::new();
    let mut stream = client.stream(request);
    while let Some(event) = stream.next().await {
        events.push(event.unwrap_or_else(|error| panic!("the request is accepted: {error}")));
    }
    drop(stream);
    (bodies_on_the_wire(), events)
}

/// An answer with no output items: for rows that read only the request.
const PLAIN_ANSWER_SSE: &str = concat!(
    "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n",
    "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",",
    "\"usage\":{\"input_tokens\":4,\"output_tokens\":1}}}\n\n",
    "data: [DONE]\n\n",
);

/// The one body `messages` puts on the wire of `client`.
async fn body_on_the_wire(client: Arc<dyn LlmClient>, messages: Vec<Message>) -> Value {
    let (mut bodies, _) = send(client, &request(messages), PLAIN_ANSWER_SSE).await;
    assert_eq!(bodies.len(), 1, "one request reached the wire: {bodies:?}");
    bodies.remove(0)
}

/// One provider step the way a session takes it: `history` goes through the
/// agent's client adapter (projection, prepared request, stream, block
/// assembly) to `client`, which is answered with `response`. Returns the
/// request body and the assistant blocks exactly as the session stores
/// them.
async fn stored_step(
    client: Arc<dyn LlmClient>,
    history: &[Message],
    response: &str,
) -> (Value, Vec<AssistantBlock>) {
    use meerkat_core::AgentLlmClient;
    answer_with(response);
    let adapter = meerkat_llm_core::LlmClientAdapter::new(client, MODEL.to_string());
    let result = adapter
        .stream_response(history, &[weather_tool()], 1024, None, None)
        .await
        .unwrap_or_else(|error| panic!("the step is accepted: {error}"));
    let mut bodies = bodies_on_the_wire();
    assert_eq!(bodies.len(), 1, "one request reached the wire: {bodies:?}");
    (bodies.remove(0), result.blocks().to_vec())
}

// ---------------------------------------------------------------------------
// Responses a service returns
// ---------------------------------------------------------------------------

/// The stream of one response with `output`, with the item events a
/// Responses stream carries for each item.
fn response_stream(response_id: &str, output: &[Value]) -> String {
    let mut events = vec![json!({"type": "response.created", "response": {"id": response_id}})];
    for (index, item) in output.iter().enumerate() {
        events.push(
            json!({"type": "response.output_item.added", "output_index": index, "item": item}),
        );
        if item["type"] == "message" {
            events.push(json!({
                "type": "response.output_text.delta",
                "item_id": item["id"],
                "output_index": index,
                "delta": item["content"][0]["text"],
            }));
        }
        if item["type"] == "web_search_call" {
            for progress in ["in_progress", "searching", "completed"] {
                events.push(json!({
                    "type": format!("response.web_search_call.{progress}"),
                    "item_id": item["id"],
                    "output_index": index,
                    "sequence_number": events.len(),
                }));
            }
        }
        events.push(
            json!({"type": "response.output_item.done", "output_index": index, "item": item}),
        );
    }
    events.push(json!({"type": "response.completed", "response": {
        "id": response_id,
        "status": "completed",
        "output": output,
        "usage": {"input_tokens": 3, "output_tokens": 2, "total_tokens": 5}
    }}));
    let mut payload = String::new();
    for event in events {
        payload.push_str(&format!("data: {event}\n\n"));
    }
    payload.push_str("data: [DONE]\n\n");
    payload
}

fn reasoning_item(turn: &str, step: u8, summary: Option<&str>) -> Value {
    let summary = match summary {
        Some(text) => json!([{"type": "summary_text", "text": text}]),
        None => json!([]),
    };
    json!({
        "type": "reasoning",
        "id": format!("rs_{turn}_{step}"),
        "summary": summary,
        "encrypted_content": format!("ciphertext-{turn}-{step}"),
    })
}

fn call_item(turn: &str) -> Value {
    json!({
        "type": "function_call",
        "id": format!("fc_{turn}"),
        "call_id": format!("call_{turn}"),
        "name": "get_weather",
        "arguments": "{\"city\":\"Oslo\"}",
        "status": "completed",
    })
}

fn message_item(turn: &str, text: &str) -> Value {
    json!({
        "type": "message",
        "id": format!("msg_{turn}"),
        "role": "assistant",
        "status": "completed",
        "phase": "final_answer",
        "content": [{"type": "output_text", "text": text, "annotations": []}],
    })
}

/// The first step of a tool turn: an encrypted reasoning item and the
/// function call it led to.
fn tool_step_response(turn: &str) -> String {
    response_stream(
        &format!("resp_{turn}_1"),
        &[reasoning_item(turn, 1, None), call_item(turn)],
    )
}

/// The answering step: a reasoning item with a summary and a phased
/// assistant message.
fn answer_step_response(turn: &str) -> String {
    response_stream(
        &format!("resp_{turn}_2"),
        &[
            reasoning_item(turn, 2, Some("Summarize.")),
            message_item(turn, "It is 4 C in Oslo."),
        ],
    )
}

/// The item a service returned for a web search it ran itself. Its `id` is
/// issued by the service and lives inside the item, not in provider
/// metadata.
fn web_search_call(turn: &str) -> Value {
    json!({
        "type": "web_search_call",
        "id": format!("ws_{turn}"),
        "status": "completed",
        "action": {"type": "search", "query": "weather in Oslo"}
    })
}

/// One step that reasoned, searched the web on the service's side and
/// answered.
fn hosted_search_response(turn: &str) -> String {
    response_stream(
        &format!("resp_{turn}_1"),
        &[
            reasoning_item(turn, 1, None),
            web_search_call(turn),
            message_item(turn, "It is 4 C in Oslo."),
        ],
    )
}

// ---------------------------------------------------------------------------
// Transcripts a session produces
// ---------------------------------------------------------------------------

fn user(text: &str) -> Message {
    Message::User(UserMessage::text(text.to_string()))
}

fn tool_output(turn: &str) -> Message {
    Message::tool_results(vec![ToolResult::new(
        format!("call_{turn}"),
        "4 C".to_string(),
        false,
    )])
}

/// One provider step on `service` in the process `ledger` stands for:
/// the request body and the assistant message as the session stores it.
async fn step(
    service: Service,
    ledger: &Ledger,
    history: &[Message],
    response: &str,
    stop_reason: StopReason,
) -> (Value, Message) {
    let (body, blocks) = stored_step(service.client(ledger), history, response).await;
    (
        body,
        Message::BlockAssistant(BlockAssistantMessage::new(blocks, stop_reason)),
    )
}

/// A whole tool turn on `service` in the process `ledger` stands for,
/// appended to `history`: the question, the tool step, the tool output and
/// the answer. Returns the transcript and the two request bodies.
async fn tool_turn(
    service: Service,
    ledger: &Ledger,
    mut history: Vec<Message>,
    turn: &str,
    question: &str,
) -> (Vec<Message>, [Value; 2]) {
    history.push(user(question));
    let (first_body, call) = step(
        service,
        ledger,
        &history,
        &tool_step_response(turn),
        StopReason::ToolUse,
    )
    .await;
    history.push(call);
    history.push(tool_output(turn));
    let (second_body, answer) = step(
        service,
        ledger,
        &history,
        &answer_step_response(turn),
        StopReason::EndTurn,
    )
    .await;
    history.push(answer);
    (history, [first_body, second_body])
}

/// The transcript of one tool turn `service` served in the process `ledger`
/// stands for.
async fn served_turn(service: Service, ledger: &Ledger, turn: &str) -> Vec<Message> {
    tool_turn(
        service,
        ledger,
        Vec::new(),
        turn,
        "What is the weather in Oslo?",
    )
    .await
    .0
}

// ---------------------------------------------------------------------------
// The request forms
// ---------------------------------------------------------------------------

fn user_item(text: &str) -> Value {
    json!({"type": "message", "role": "user", "content": text})
}

fn call_input(turn: &str) -> Value {
    json!({
        "type": "function_call",
        "call_id": format!("call_{turn}"),
        "name": "get_weather",
        "arguments": "{\"city\":\"Oslo\"}"
    })
}

fn output_input(turn: &str) -> Value {
    json!({"type": "function_call_output", "call_id": format!("call_{turn}"), "output": "4 C"})
}

/// The tool step as the service returned it.
fn native_tool_step(turn: &str) -> Vec<Value> {
    vec![
        json!({
            "type": "reasoning",
            "id": format!("rs_{turn}_1"),
            "summary": [],
            "encrypted_content": format!("ciphertext-{turn}-1"),
        }),
        call_input(turn),
        output_input(turn),
    ]
}

/// The tool step without anything a service issued.
fn dropped_tool_step(turn: &str) -> Vec<Value> {
    vec![call_input(turn), output_input(turn)]
}

/// The answering step as the service returned it.
fn native_answer_step(turn: &str) -> Vec<Value> {
    vec![
        json!({
            "type": "reasoning",
            "id": format!("rs_{turn}_2"),
            "summary": [{"type": "summary_text", "text": "Summarize."}],
            "encrypted_content": format!("ciphertext-{turn}-2"),
        }),
        json!({
            "type": "message",
            "role": "assistant",
            "id": format!("msg_{turn}"),
            "status": "completed",
            "phase": "final_answer",
            "content": [{"type": "output_text", "text": "It is 4 C in Oslo.", "annotations": []}]
        }),
    ]
}

/// The answering step without anything a service issued.
fn dropped_answer_step() -> Vec<Value> {
    vec![json!({"type": "message", "role": "assistant", "content": "It is 4 C in Oslo."})]
}

/// The input of a whole tool turn whose steps the target admits.
fn native_turn(turn: &str, question: &str) -> Vec<Value> {
    let mut input = vec![user_item(question)];
    input.extend(native_tool_step(turn));
    input.extend(native_answer_step(turn));
    input
}

/// The input of a whole tool turn none of whose steps the target admits.
fn dropped_turn(turn: &str, question: &str) -> Vec<Value> {
    let mut input = vec![user_item(question)];
    input.extend(dropped_tool_step(turn));
    input.extend(dropped_answer_step());
    input
}

fn input_of(parts: impl IntoIterator<Item = Vec<Value>>) -> Value {
    Value::Array(parts.into_iter().flatten().collect())
}

/// What a request must not carry for the steps of `turns` when they are in
/// the dropped form, checked on the whole body and not only on the items the
/// expected form names.
fn assert_carries_no_continuity_of(case: &str, body: &Value, turns: &[&str]) {
    let text = body.to_string();
    for turn in turns {
        for opaque in [
            format!("ciphertext-{turn}"),
            format!("rs_{turn}"),
            format!("msg_{turn}"),
            format!("resp_{turn}"),
            format!("ws_{turn}"),
            format!("fc_{turn}"),
        ] {
            assert!(
                !text.contains(&opaque),
                "{case}: `{opaque}` reached the wire: {text}"
            );
        }
    }
    assert_calls_and_outputs_pair(case, body["input"].as_array().expect("input array"));
}

/// A request wholly in the dropped form: no reasoning item, no hosted-tool
/// item, no provider item id, no response id.
fn assert_wholly_dropped(case: &str, body: &Value) {
    let input = body["input"].as_array().expect("input array");
    for item in input {
        assert_ne!(
            item["type"], "reasoning",
            "{case}: a reasoning item: {item}"
        );
        assert_ne!(
            item["type"], "web_search_call",
            "{case}: a hosted-tool item: {item}"
        );
        assert!(
            item.get("id").is_none(),
            "{case}: a provider-issued item id: {item}"
        );
        assert!(
            item.get("encrypted_content").is_none(),
            "{case}: ciphertext: {item}"
        );
    }
    assert!(
        body.get("previous_response_id").is_none(),
        "{case}: a response id was sent: {body}"
    );
    assert_calls_and_outputs_pair(case, input);
}

/// The pairing every Responses service enforces: each function call has
/// exactly one output after it, each output answers an earlier call, and a
/// reasoning item is followed by an item of its own step.
fn assert_calls_and_outputs_pair(case: &str, input: &[Value]) {
    let mut open: Vec<&str> = Vec::new();
    for (index, item) in input.iter().enumerate() {
        match item["type"].as_str() {
            Some("function_call") => open.push(item["call_id"].as_str().expect("call id")),
            Some("function_call_output") => {
                let call_id = item["call_id"].as_str().expect("call id");
                let position = open.iter().position(|open| *open == call_id);
                assert!(
                    position.is_some(),
                    "{case}: output {call_id} answers no earlier call"
                );
                open.remove(position.expect("checked"));
            }
            Some("reasoning") => {
                let next = input.get(index + 1).map(|next| &next["type"]);
                assert!(
                    next.is_some_and(|kind| kind == "function_call"
                        || kind == "reasoning"
                        || kind == "message"
                        || kind == "web_search_call"),
                    "{case}: a reasoning item without its following item"
                );
            }
            _ => {}
        }
    }
    assert!(open.is_empty(), "{case}: calls without an output: {open:?}");
}

const QUESTION: &str = "What is the weather in Oslo?";
const NEXT_QUESTION: &str = "And tomorrow?";

fn then_asked(mut transcript: Vec<Message>, question: &str) -> Vec<Message> {
    transcript.push(user(question));
    transcript
}

// ---------------------------------------------------------------------------
// Another service, the same service, a new process
// ---------------------------------------------------------------------------

/// A turn one service served is sent to every other service of the same
/// process without its continuity material. Every ordered pair of services.
#[tokio::test]
async fn a_turn_received_from_another_service_is_not_replayed() {
    for source in Service::ALL {
        let process = new_process();
        let transcript = then_asked(served_turn(source, &process, "t").await, NEXT_QUESTION);
        for target in Service::ALL {
            if source == target {
                continue;
            }
            let case = format!("{source:?} -> {target:?}");
            let body = body_on_the_wire(target.client(&process), transcript.clone()).await;
            assert_wholly_dropped(&case, &body);
            assert_carries_no_continuity_of(&case, &body, &["t"]);
            assert_eq!(
                body["input"],
                input_of([dropped_turn("t", QUESTION), vec![user_item(NEXT_QUESTION)]]),
                "{case}"
            );
        }
    }
}

/// The path that switches nothing keeps its continuity while the process
/// lives: a service is sent its own reasoning items, message ids and phases
/// byte for byte, inside the tool loop and on the next turn.
#[tokio::test]
async fn a_turn_received_from_the_same_service_replays_unchanged() {
    for service in Service::ALL {
        let process = new_process();
        let (transcript, [_, inside_the_loop]) =
            tool_turn(service, &process, Vec::new(), "t", QUESTION).await;
        assert_eq!(
            inside_the_loop["input"],
            input_of([vec![user_item(QUESTION)], native_tool_step("t")]),
            "{service:?}: inside the tool loop"
        );
        let body = body_on_the_wire(
            service.client(&process),
            then_asked(transcript, NEXT_QUESTION),
        )
        .await;
        assert_eq!(
            body["input"],
            input_of([native_turn("t", QUESTION), vec![user_item(NEXT_QUESTION)]]),
            "{service:?}: the next turn"
        );
        assert_calls_and_outputs_pair(
            &format!("{service:?}"),
            body["input"].as_array().expect("input array"),
        );
    }
}

/// A new process knows nothing about a stored transcript, the one its own
/// service produced included: the next turn after a cold load goes out in
/// the dropped form, and so does a run that stopped inside its tool loop and
/// is resumed by a new process.
#[tokio::test]
async fn a_new_process_replays_no_stored_material_to_any_service() {
    for service in Service::ALL {
        let earlier = new_process();
        let transcript = served_turn(service, &earlier, "t").await;
        for target in [service, service.other()] {
            let restarted = new_process();
            let case = format!("{service:?}, cold load -> {target:?}");
            let body = body_on_the_wire(
                target.client(&restarted),
                then_asked(transcript.clone(), NEXT_QUESTION),
            )
            .await;
            assert_wholly_dropped(&case, &body);
            assert_carries_no_continuity_of(&case, &body, &["t"]);
            assert_eq!(
                body["input"],
                input_of([dropped_turn("t", QUESTION), vec![user_item(NEXT_QUESTION)]]),
                "{case}"
            );

            // The run stopped after the tool output was recorded.
            let case = format!("{service:?}, cold load inside the tool loop -> {target:?}");
            let body = body_on_the_wire(target.client(&restarted), transcript[..3].to_vec()).await;
            assert_wholly_dropped(&case, &body);
            assert_eq!(
                body["input"],
                input_of([vec![user_item(QUESTION)], dropped_tool_step("t")]),
                "{case}"
            );
        }
    }
}

/// After a cold load the transcript is mixed: the stored steps are of
/// unknown issuer and the steps this process receives are known. One request
/// carries the older steps in the dropped form and the new ones as the
/// service returned them, inside the new tool loop and on the turn after it.
#[tokio::test]
async fn stored_steps_and_received_steps_share_one_request() {
    for service in Service::ALL {
        let earlier = new_process();
        let stored = served_turn(service, &earlier, "old").await;

        let restarted = new_process();
        let (transcript, [first, inside_the_loop]) =
            tool_turn(service, &restarted, stored, "new", NEXT_QUESTION).await;
        let case = format!("{service:?}, first request after the cold load");
        assert_wholly_dropped(&case, &first);
        assert_eq!(
            first["input"],
            input_of([
                dropped_turn("old", QUESTION),
                vec![user_item(NEXT_QUESTION)]
            ]),
            "{case}"
        );

        let case = format!("{service:?}, inside the new tool loop");
        assert_carries_no_continuity_of(&case, &inside_the_loop, &["old"]);
        assert_eq!(
            inside_the_loop["input"],
            input_of([
                dropped_turn("old", QUESTION),
                vec![user_item(NEXT_QUESTION)],
                native_tool_step("new"),
            ]),
            "{case}"
        );

        let case = format!("{service:?}, the turn after");
        let body = body_on_the_wire(
            service.client(&restarted),
            then_asked(transcript, "And on Sunday?"),
        )
        .await;
        assert_carries_no_continuity_of(&case, &body, &["old"]);
        assert_eq!(
            body["input"],
            input_of([
                dropped_turn("old", QUESTION),
                native_turn("new", NEXT_QUESTION),
                vec![user_item("And on Sunday?")],
            ]),
            "{case}"
        );
    }
}

// ---------------------------------------------------------------------------
// A step is admitted as a whole
// ---------------------------------------------------------------------------

fn metadata_mut(block: &mut AssistantBlock) -> Option<&mut ProviderMeta> {
    match block {
        AssistantBlock::Text { meta, .. }
        | AssistantBlock::Transcript { meta, .. }
        | AssistantBlock::Reasoning { meta, .. }
        | AssistantBlock::ToolUse { meta, .. }
        | AssistantBlock::ServerToolContent { meta, .. } => meta.as_deref_mut(),
        _ => None,
    }
}

/// A step in which one item is not the material this process received (here
/// its ciphertext differs) is not this process's step: the whole step goes
/// out in the dropped form, the message id beside the altered reasoning
/// item included. Its neighbours are unaffected.
#[tokio::test]
async fn a_step_with_one_unknown_item_is_dropped_as_a_whole() {
    for service in Service::ALL {
        let process = new_process();
        let mut transcript = served_turn(service, &process, "t").await;
        let Message::BlockAssistant(answer) = &mut transcript[3] else {
            panic!("the answering step");
        };
        let Some(ProviderMeta::OpenAi {
            encrypted_content, ..
        }) = metadata_mut(&mut answer.blocks[0])
        else {
            panic!("the answering step leads with its reasoning item");
        };
        *encrypted_content = Some("other-bytes".to_string());

        let case = format!("{service:?}");
        let body = body_on_the_wire(
            service.client(&process),
            then_asked(transcript, NEXT_QUESTION),
        )
        .await;
        assert!(!body.to_string().contains("other-bytes"), "{case}");
        assert!(!body.to_string().contains("msg_t"), "{case}");
        assert_eq!(
            body["input"],
            input_of([
                vec![user_item(QUESTION)],
                native_tool_step("t"),
                dropped_answer_step(),
                vec![user_item(NEXT_QUESTION)],
            ]),
            "{case}: the altered step is dropped whole, the step before it is kept"
        );
    }
}

/// A process remembers a bounded number of responses, oldest forgotten
/// first, a response at a time. Here it remembers one: the tool step of the
/// turn is forgotten when the answer arrives, so it goes out in the dropped
/// form (its call and output still paired) while the answer replays.
#[tokio::test]
async fn a_forgotten_response_is_dropped_as_a_whole() {
    for service in Service::ALL {
        let process: Ledger = Arc::new(OpenAiContinuityLedger::with_generation(1));
        let transcript = served_turn(service, &process, "t").await;
        let case = format!("{service:?}");
        let body = body_on_the_wire(
            service.client(&process),
            then_asked(transcript, NEXT_QUESTION),
        )
        .await;
        assert_calls_and_outputs_pair(&case, body["input"].as_array().expect("input array"));
        assert_eq!(
            body["input"],
            input_of([
                vec![user_item(QUESTION)],
                dropped_tool_step("t"),
                native_answer_step("t"),
                vec![user_item(NEXT_QUESTION)],
            ]),
            "{case}"
        );
    }
}

/// The same response bytes received from two services identify neither's
/// material: both are sent the dropped form.
#[tokio::test]
async fn material_two_services_both_returned_belongs_to_neither() {
    let process = new_process();
    let first = served_turn(Service::OpenAiApi, &process, "t").await;
    served_turn(Service::Copilot, &process, "t").await;
    for target in [Service::OpenAiApi, Service::Copilot] {
        let case = format!("contested -> {target:?}");
        let body = body_on_the_wire(
            target.client(&process),
            then_asked(first.clone(), NEXT_QUESTION),
        )
        .await;
        assert_wholly_dropped(&case, &body);
        assert_eq!(
            body["input"],
            input_of([dropped_turn("t", QUESTION), vec![user_item(NEXT_QUESTION)]]),
            "{case}"
        );
    }
}

// ---------------------------------------------------------------------------
// Copies inside the process
// ---------------------------------------------------------------------------

/// The issuer is not in the message, so a copy of the transcript inside the
/// process (a fork, a serialization round trip between steps, a rewrite that
/// keeps later steps) neither loses it nor invents it: the copied steps
/// replay to their service as before, and the rows written for them are the
/// rows read back.
#[tokio::test]
async fn copies_inside_the_process_replay_like_the_original() {
    for service in Service::ALL {
        let process = new_process();
        let transcript = served_turn(service, &process, "t").await;
        let expected = input_of([native_turn("t", QUESTION), vec![user_item(NEXT_QUESTION)]]);

        // A fork shares the transcript's messages.
        let mut session = meerkat_core::Session::new();
        for message in &transcript {
            session.push(message.clone());
        }
        let fork = session.fork();
        let body = body_on_the_wire(
            service.client(&process),
            then_asked(fork.messages().to_vec(), NEXT_QUESTION),
        )
        .await;
        assert_eq!(body["input"], expected, "{service:?}: a fork");

        // Written to rows and read back between two steps.
        let rows = serde_json::to_vec(&transcript).expect("rows");
        let read_back: Vec<Message> = serde_json::from_slice(&rows).expect("rows decode");
        assert_eq!(read_back, transcript, "{service:?}: live equals stored");
        let body = body_on_the_wire(
            service.client(&process),
            then_asked(read_back, NEXT_QUESTION),
        )
        .await;
        assert_eq!(body["input"], expected, "{service:?}: rows read back");

        // A rewrite that replaced the first step and its output with a
        // summary and kept the answer.
        let rewritten = vec![
            user("Earlier: the weather tool reported 4 C for Oslo."),
            transcript[3].clone(),
            user(NEXT_QUESTION),
        ];
        let body = body_on_the_wire(service.client(&process), rewritten).await;
        assert_eq!(
            body["input"],
            input_of([
                vec![user_item(
                    "Earlier: the weather tool reported 4 C for Oslo."
                )],
                native_answer_step("t"),
                vec![user_item(NEXT_QUESTION)],
            ]),
            "{service:?}: a rewrite keeps the surviving step's material"
        );
    }
}

/// What a client commits for a response is, byte for byte, what 0.9.12
/// committed for the same response: the issuer reaches no stored row, event
/// or payload. (The constant is the 0.9.12 serializer's output for
/// [`tool_step_response`] and [`answer_step_response`] of turn `t`.)
#[tokio::test]
async fn committed_rows_are_the_rows_0_9_12_wrote() {
    for service in Service::ALL {
        let process = new_process();
        let transcript = served_turn(service, &process, "t").await;
        let steps: Vec<&Message> = transcript
            .iter()
            .filter(|message| matches!(message, Message::BlockAssistant(_)))
            .collect();
        let mut rows = serde_json::to_value(&steps).expect("rows");
        for row in rows.as_array_mut().expect("rows") {
            row.as_object_mut().expect("row").remove("created_at");
        }
        let expected: Value =
            serde_json::from_str(ROWS_0_9_12_WROTE).expect("the pinned rows are JSON");
        assert_eq!(rows, expected, "{service:?}");
        assert!(
            !rows.to_string().contains("issuer"),
            "{service:?}: the issuer reached a row"
        );
    }
}

const ROWS_0_9_12_WROTE: &str = r#"[
  {
    "role": "block_assistant",
    "blocks": [
      {"block_type": "reasoning", "data": {"text": "", "meta": {
        "provider": "open_ai", "id": "rs_t_1", "encrypted_content": "ciphertext-t-1",
        "response_id": "resp_t_1"}}},
      {"block_type": "tool_use", "data": {"id": "call_t", "name": "get_weather",
        "args": {"city": "Oslo"},
        "meta": {"provider": "open_ai_response", "response_id": "resp_t_1"}}}
    ],
    "stop_reason": "tool_use"
  },
  {
    "role": "block_assistant",
    "blocks": [
      {"block_type": "reasoning", "data": {"text": "Summarize.", "meta": {
        "provider": "open_ai", "id": "rs_t_2", "encrypted_content": "ciphertext-t-2",
        "response_id": "resp_t_2"}}},
      {"block_type": "text", "data": {"text": "It is 4 C in Oslo.", "meta": {
        "provider": "open_ai_assistant_message", "id": "msg_t", "phase": "final_answer",
        "response_id": "resp_t_2"}}}
    ],
    "stop_reason": "end_turn"
  }
]"#;

// ---------------------------------------------------------------------------
// The dropped form is a valid request
// ---------------------------------------------------------------------------

/// The repository's own replay validators accept the dropped form: the
/// replay plan verifies the projection (every source block accounted for,
/// tool results adjacent to and exactly matching their tool uses), and the
/// projected transcript holds no reasoning block and no provider metadata.
#[tokio::test]
async fn the_dropped_form_passes_the_replay_validators() {
    for service in Service::ALL {
        let earlier = new_process();
        let stored = served_turn(service, &earlier, "t").await;
        let process = new_process();
        let foreign = served_turn(service.other(), &process, "f").await;
        for (origin, transcript) in [
            ("a stored turn", stored),
            ("another service's turn", foreign),
        ] {
            for (shape, transcript) in [
                (
                    "finished turn",
                    then_asked(transcript.clone(), NEXT_QUESTION),
                ),
                ("tool loop", transcript[..3].to_vec()),
            ] {
                let case = format!("{origin} -> {service:?}, {shape}");
                let client = service.client(&process);
                let projected = client
                    .project_replay_messages(&transcript)
                    .unwrap_or_else(|error| panic!("{case}: the replay plan refuses: {error}"));
                for message in &projected {
                    let Message::BlockAssistant(assistant) = message else {
                        continue;
                    };
                    for block in &assistant.blocks {
                        assert!(
                            !matches!(block, AssistantBlock::Reasoning { .. }),
                            "{case}: a reasoning block survives the projection"
                        );
                        assert!(
                            meerkat_core::replay_provider_metadata(block).is_none(),
                            "{case}: provider metadata survives the projection: {block:?}"
                        );
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// A transcript stored by 0.9.12
// ---------------------------------------------------------------------------

/// One tool turn and its answer exactly as 0.9.12 stored them after an
/// OpenAI Responses backend returned them (`store: false`,
/// `include: reasoning.encrypted_content`): the 0.9.12 serializer wrote
/// these rows.
const STORED_BY_0_9_12: &str = r#"[
  {
    "role": "user",
    "content": "What is the weather in Oslo?",
    "created_at": "2026-11-10T09:00:00Z"
  },
  {
    "role": "block_assistant",
    "blocks": [
      {
        "block_type": "reasoning",
        "data": {
          "text": "",
          "meta": {
            "provider": "open_ai",
            "id": "rs_0a1b",
            "encrypted_content": "gAAAAAB-ciphertext-1",
            "response_id": "resp_0001"
          }
        }
      },
      {
        "block_type": "tool_use",
        "data": {
          "id": "call_weather_1",
          "name": "get_weather",
          "args": {"city":"Oslo"},
          "meta": {
            "provider": "open_ai_response",
            "response_id": "resp_0001"
          }
        }
      }
    ],
    "stop_reason": "tool_use",
    "created_at": "2026-11-10T09:00:00Z"
  },
  {
    "role": "tool_results",
    "results": [
      {
        "tool_use_id": "call_weather_1",
        "content": "4 C, light rain",
        "is_error": false
      }
    ],
    "created_at": "2026-11-10T09:00:00Z"
  },
  {
    "role": "block_assistant",
    "blocks": [
      {
        "block_type": "reasoning",
        "data": {
          "text": "Summarize the reading.",
          "meta": {
            "provider": "open_ai",
            "id": "rs_0c2d",
            "encrypted_content": "gAAAAAB-ciphertext-2",
            "response_id": "resp_0002"
          }
        }
      },
      {
        "block_type": "text",
        "data": {
          "text": "It is 4 C with light rain in Oslo.",
          "meta": {
            "provider": "open_ai_assistant_message",
            "id": "msg_0e3f",
            "phase": "final_answer",
            "response_id": "resp_0002"
          }
        }
      }
    ],
    "stop_reason": "end_turn",
    "created_at": "2026-11-10T09:00:00Z"
  }
]"#;

/// A transcript 0.9.12 stored is read back unchanged and resumed on every
/// service in the dropped form, the service that produced it included, on
/// the next turn and inside its tool loop.
#[tokio::test]
async fn a_transcript_stored_by_0_9_12_resumes_on_every_service() {
    let stored: Value = serde_json::from_str(STORED_BY_0_9_12).expect("fixture is JSON");
    let transcript: Vec<Message> =
        serde_json::from_value(stored.clone()).expect("the stored rows decode");
    assert_eq!(
        serde_json::to_value(&transcript).expect("rows serialize"),
        stored,
        "the stored rows are written back unchanged"
    );
    let call = json!({
        "type": "function_call",
        "call_id": "call_weather_1",
        "name": "get_weather",
        "arguments": "{\"city\":\"Oslo\"}"
    });
    let output = json!({
        "type": "function_call_output", "call_id": "call_weather_1", "output": "4 C, light rain"
    });
    for target in Service::ALL {
        let process = new_process();
        let case = format!("0.9.12 transcript -> {target:?}");
        let body = body_on_the_wire(
            target.client(&process),
            then_asked(transcript.clone(), NEXT_QUESTION),
        )
        .await;
        assert_wholly_dropped(&case, &body);
        assert_eq!(
            body["input"],
            json!([
                user_item(QUESTION),
                call,
                output,
                {"type": "message", "role": "assistant", "content": "It is 4 C with light rain in Oslo."},
                user_item(NEXT_QUESTION),
            ]),
            "{case}"
        );

        let case = format!("0.9.12 transcript -> {target:?}, inside the tool loop");
        let body = body_on_the_wire(target.client(&process), transcript[..3].to_vec()).await;
        assert_wholly_dropped(&case, &body);
        assert_eq!(
            body["input"],
            json!([user_item(QUESTION), call, output]),
            "{case}"
        );
    }
}

// ---------------------------------------------------------------------------
// Hosted-tool items
// ---------------------------------------------------------------------------

/// A step in which the service searched the web itself, then the next
/// question.
async fn hosted_search_turn(service: Service, ledger: &Ledger, turn: &str) -> Vec<Message> {
    let history = vec![user(QUESTION)];
    let (_, answer) = step(
        service,
        ledger,
        &history,
        &hosted_search_response(turn),
        StopReason::EndTurn,
    )
    .await;
    vec![user(QUESTION), answer, user(NEXT_QUESTION)]
}

/// A hosted-tool item carries an id its service issued inside its content
/// and is paired with that step's reasoning item. One this process received
/// from another service, or did not receive, is not replayed and its id
/// reaches the wire nowhere: the model keeps the answer text of the step.
#[tokio::test]
async fn a_hosted_tool_item_is_replayed_only_to_the_service_it_came_from() {
    for service in Service::ALL {
        let process = new_process();
        let transcript = hosted_search_turn(service, &process, "h").await;

        let body = body_on_the_wire(service.client(&process), transcript.clone()).await;
        assert_eq!(
            body["input"],
            json!([
                user_item(QUESTION),
                {
                    "type": "reasoning", "id": "rs_h_1", "summary": [],
                    "encrypted_content": "ciphertext-h-1"
                },
                web_search_call("h"),
                {
                    "type": "message",
                    "role": "assistant",
                    "id": "msg_h",
                    "status": "completed",
                    "phase": "final_answer",
                    "content": [
                        {"type": "output_text", "text": "It is 4 C in Oslo.", "annotations": []}
                    ]
                },
                user_item(NEXT_QUESTION),
            ]),
            "{service:?}: its own hosted-tool item replays between its reasoning and its answer"
        );

        let restarted = new_process();
        for (case, target, ledger) in [
            ("another service", service.other(), &process),
            ("a new process", service, &restarted),
        ] {
            let case = format!("{service:?} -> {case}, hosted web search");
            let body = body_on_the_wire(target.client(ledger), transcript.clone()).await;
            assert_wholly_dropped(&case, &body);
            assert_carries_no_continuity_of(&case, &body, &["h"]);
            assert!(!body.to_string().contains("web_search_call"), "{case}");
            assert_eq!(
                body["input"],
                input_of([
                    vec![user_item(QUESTION)],
                    dropped_answer_step(),
                    vec![user_item(NEXT_QUESTION)],
                ]),
                "{case}"
            );
        }
    }
}

/// A response that searched the web many times carries much material: for
/// each search a reasoning item and a search item, and three progress
/// events on the stream. It is recorded whole, and the next request of the
/// process that received it replays every item of it unchanged, on every
/// service. (No number of items makes a response too large to be known, and
/// the progress events are no material: they are never replayed.)
#[tokio::test]
async fn a_response_with_many_hosted_searches_is_recorded_whole() {
    const SEARCHES: usize = 40;
    let mut output = Vec::new();
    for search in 0..SEARCHES {
        let turn = format!("m{search}");
        output.push(reasoning_item(&turn, 1, None));
        output.push(web_search_call(&turn));
    }
    output.push(message_item("m", "It is 4 C in Oslo."));
    let response = response_stream("resp_m_1", &output);

    for service in Service::ALL {
        let process = new_process();
        let history = vec![user(QUESTION)];
        let (_, answer) = step(service, &process, &history, &response, StopReason::EndTurn).await;
        let transcript = vec![user(QUESTION), answer, user(NEXT_QUESTION)];
        let body = body_on_the_wire(service.client(&process), transcript).await;

        let mut expected = vec![user_item(QUESTION)];
        for item in &output {
            if item["type"] == "reasoning" || item["type"] == "web_search_call" {
                expected.push(item.clone());
            }
        }
        let replayed: Vec<Value> = body["input"]
            .as_array()
            .expect("input")
            .iter()
            .filter(|item| item["type"] == "reasoning" || item["type"] == "web_search_call")
            .cloned()
            .collect();
        assert_eq!(
            replayed,
            expected[1..],
            "{service:?}: every reasoning item and every search item replays, in order"
        );
        let answer = body["input"]
            .as_array()
            .expect("input")
            .iter()
            .find(|item| item["type"] == "message" && item["role"] == "assistant")
            .unwrap_or_else(|| panic!("{service:?}: the answer replays"));
        assert_eq!(
            answer["id"], "msg_m",
            "{service:?}: as the service returned it"
        );
        assert!(
            !body.to_string().contains("response.web_search_call"),
            "{service:?}: no progress event is replayed"
        );
    }
}

/// A response that fails records nothing. The same items, received in a
/// response that then fails, do not make the step known: the transcript is
/// the step exactly as another process stored it from the successful
/// response, and it is sent in the dropped form.
#[tokio::test]
async fn a_failed_response_records_nothing() {
    let failed = {
        let item = reasoning_item("f", 1, None);
        let call = call_item("f");
        let mut payload = String::new();
        for event in [
            json!({"type": "response.created", "response": {"id": "resp_f_1"}}),
            json!({"type": "response.output_item.added", "output_index": 0, "item": item}),
            json!({"type": "response.output_item.done", "output_index": 0, "item": item}),
            json!({"type": "response.output_item.added", "output_index": 1, "item": call}),
            json!({"type": "response.output_item.done", "output_index": 1, "item": call}),
            json!({"type": "response.failed", "response": {
                "id": "resp_f_1",
                "status": "failed",
                "error": {"code": "server_error", "message": "the response failed"}
            }}),
        ] {
            payload.push_str(&format!("data: {event}\n\n"));
        }
        payload.push_str("data: [DONE]\n\n");
        payload
    };
    for service in Service::ALL {
        let history = vec![user(QUESTION)];
        let elsewhere = new_process();
        let (_, blocks) = stored_step(
            service.client(&elsewhere),
            &history,
            &tool_step_response("f"),
        )
        .await;
        let transcript = vec![
            user(QUESTION),
            Message::BlockAssistant(BlockAssistantMessage::new(blocks, StopReason::ToolUse)),
            tool_output("f"),
        ];
        let known = body_on_the_wire(service.client(&elsewhere), transcript.clone()).await;
        assert_eq!(
            known["input"],
            input_of([vec![user_item(QUESTION)], native_tool_step("f")]),
            "{service:?}: the successful response made the step known where it was received"
        );

        let process = new_process();
        let client = service.client(&process);
        answer_with(&failed);
        let outcome: Vec<_> = client.stream(&request(history.clone())).collect().await;
        assert!(
            !outcome.iter().any(|event| matches!(
                event,
                Ok(LlmEvent::Done {
                    outcome: meerkat_llm_core::LlmDoneOutcome::Success { .. }
                })
            )),
            "{service:?}: the response did not succeed: {outcome:?}"
        );
        assert!(
            outcome.iter().any(|event| matches!(
                event,
                Err(_)
                    | Ok(LlmEvent::Done {
                        outcome: meerkat_llm_core::LlmDoneOutcome::Error { .. }
                    })
            )),
            "{service:?}: the response failed: {outcome:?}"
        );
        let body = body_on_the_wire(service.client(&process), transcript).await;
        assert_wholly_dropped(&format!("{service:?}: a failed response"), &body);
    }
}

// ---------------------------------------------------------------------------
// A fallback target, and another wire protocol
// ---------------------------------------------------------------------------

/// A request prepared for one service and then sent by the client of
/// another (a model or route fallback): the sending client applies the rule
/// again for itself, so material the first service admitted does not reach
/// the second.
#[tokio::test]
async fn a_request_prepared_for_one_service_is_not_replayed_by_another() {
    for prepared_for in Service::ALL {
        let process = new_process();
        let sender = prepared_for.other();
        let case = format!("prepared for {prepared_for:?}, sent by {sender:?}");
        let transcript = then_asked(
            served_turn(prepared_for, &process, "t").await,
            NEXT_QUESTION,
        );
        let prepared = prepared_for
            .client(&process)
            .project_replay_messages(&transcript)
            .expect("the first service's projection");
        let body = body_on_the_wire(sender.client(&process), prepared).await;
        assert_wholly_dropped(&case, &body);
        assert_eq!(
            body["input"],
            input_of([dropped_turn("t", QUESTION), vec![user_item(NEXT_QUESTION)]]),
            "{case}"
        );
    }
}

/// Responses continuity material never reaches a Chat Completions wire,
/// whoever issued it: the protocol is a guard of its own, beside the issuer.
/// A self-hosted server's own Responses material, sent to that server's
/// Chat Completions endpoint by the same process, is left out.
#[tokio::test]
async fn responses_material_never_reaches_a_chat_completions_wire() {
    let process = new_process();
    let transcript = then_asked(
        served_turn(Service::SelfHostedA, &process, "t").await,
        NEXT_QUESTION,
    );
    let client = OpenAiCompatibleClient::new_with_options(
        OpenAiCompatibleMode::ChatCompletions,
        SELF_HOSTED_REMOTE_MODEL.to_string(),
        "http://127.0.0.1:9/v1".to_string(),
        None,
        crate::OpenAiCompatibleClientOptions {
            supports_temperature: true,
            supports_thinking: true,
            supports_reasoning: true,
            supports_image_tool_results: false,
        },
    )
    .with_self_hosted_server("local-a")
    .with_continuity_ledger(Arc::clone(&process));
    let projected = client
        .project_replay_messages(&transcript)
        .expect("chat completions projection");
    let body = client
        .build_chat_completions_body(&LlmRequest::new(SELF_HOSTED_REMOTE_MODEL, projected))
        .expect("chat completions body");
    let text = body.to_string();
    for opaque in ["ciphertext", "rs_t", "msg_t", "resp_t", "encrypted_content"] {
        assert!(
            !text.contains(opaque),
            "`{opaque}` reached a Chat Completions wire: {text}"
        );
    }
}

// ---------------------------------------------------------------------------
// Response ids
// ---------------------------------------------------------------------------

/// A request that asks for server-side continuation (`store: true`).
fn continuation_request(transcript: Vec<Message>) -> LlmRequest {
    request(transcript).with_openai_tag_merge(|tag| tag.store = Some(true))
}

/// With `store: true` the response id of the last step becomes
/// `previous_response_id`, and only what followed is sent. That shortcut is
/// taken only when this process received the last step from the target: a
/// step of another service, or of a new process, is never named, and the
/// whole transcript is replayed on the first attempt.
#[tokio::test]
async fn a_response_id_is_sent_only_to_the_service_it_came_from() {
    for service in Service::ALL {
        let process = new_process();
        let transcript = then_asked(served_turn(service, &process, "t").await, NEXT_QUESTION);

        let (bodies, _) = send(
            service.client(&process),
            &continuation_request(transcript.clone()),
            PLAIN_ANSWER_SSE,
        )
        .await;
        assert_eq!(bodies.len(), 1, "{service:?}: one request: {bodies:?}");
        assert_eq!(
            bodies[0]["previous_response_id"], "resp_t_2",
            "{service:?}: its own last step continues server-side"
        );
        assert_eq!(
            bodies[0]["input"],
            json!([user_item(NEXT_QUESTION)]),
            "{service:?}"
        );

        let restarted = new_process();
        for (case, target, ledger) in [
            ("another service", service.other(), &process),
            ("a new process", service, &restarted),
        ] {
            let case = format!("{service:?} -> {case}");
            let (bodies, _) = send(
                target.client(ledger),
                &continuation_request(transcript.clone()),
                PLAIN_ANSWER_SSE,
            )
            .await;
            assert_eq!(bodies.len(), 1, "{case}: one request: {bodies:?}");
            assert!(
                bodies[0].get("previous_response_id").is_none(),
                "{case}: a response id was sent: {}",
                bodies[0]
            );
            assert_eq!(
                bodies[0]["input"],
                input_of([dropped_turn("t", QUESTION), vec![user_item(NEXT_QUESTION)]]),
                "{case}: the whole transcript is replayed"
            );
        }
    }
}

/// In a mixed transcript the shortcut follows the last step: stored steps
/// followed by a step this process received continue from that step, and a
/// received step followed by a stored one does not continue at all.
#[tokio::test]
async fn the_response_id_shortcut_follows_the_last_step() {
    let service = Service::OpenAiApi;
    let earlier = new_process();
    let stored = served_turn(service, &earlier, "old").await;
    let process = new_process();
    let (mixed, _) = tool_turn(service, &process, stored.clone(), "new", NEXT_QUESTION).await;

    let (bodies, _) = send(
        service.client(&process),
        &continuation_request(then_asked(mixed, "And on Sunday?")),
        PLAIN_ANSWER_SSE,
    )
    .await;
    assert_eq!(bodies[0]["previous_response_id"], "resp_new_2");
    assert_eq!(bodies[0]["input"], json!([user_item("And on Sunday?")]));

    let received = served_turn(service, &process, "first").await;
    let mut ends_with_stored = received;
    ends_with_stored.extend(stored);
    let (bodies, _) = send(
        service.client(&process),
        &continuation_request(then_asked(ends_with_stored, "And on Sunday?")),
        PLAIN_ANSWER_SSE,
    )
    .await;
    assert!(bodies[0].get("previous_response_id").is_none());
    assert_eq!(
        bodies[0]["input"],
        input_of([
            native_turn("first", QUESTION),
            dropped_turn("old", QUESTION),
            vec![user_item("And on Sunday?")],
        ])
    );
}

// ---------------------------------------------------------------------------
// The issuer is a kind of service at an endpoint
// ---------------------------------------------------------------------------

/// One kind of service at two endpoints is two issuers: the public API
/// behind another base URL (a gateway) is not sent what the public API
/// returned, and the reverse; each is sent its own.
#[tokio::test]
async fn two_endpoints_of_one_service_kind_are_two_issuers() {
    let process = new_process();
    let gateway = |ledger: &Ledger| -> Arc<dyn LlmClient> {
        Arc::new(
            OpenAiClient::new_with_base_url("test-key".to_string(), endpoint("gateway"))
                .with_continuity_ledger(Arc::clone(ledger)),
        )
    };
    let from_api = then_asked(
        served_turn(Service::OpenAiApi, &process, "t").await,
        NEXT_QUESTION,
    );
    let body = body_on_the_wire(gateway(&process), from_api.clone()).await;
    assert_wholly_dropped("public API -> the public wire at another base URL", &body);

    let history = vec![user(QUESTION)];
    let (_, blocks) = stored_step(gateway(&process), &history, &tool_step_response("g")).await;
    let from_gateway = vec![
        user(QUESTION),
        Message::BlockAssistant(BlockAssistantMessage::new(blocks, StopReason::ToolUse)),
        tool_output("g"),
    ];
    let own = body_on_the_wire(gateway(&process), from_gateway.clone()).await;
    assert_eq!(
        own["input"],
        input_of([vec![user_item(QUESTION)], native_tool_step("g")]),
        "the gateway's own step replays to the gateway"
    );
    let body = body_on_the_wire(Service::OpenAiApi.client(&process), from_gateway).await;
    assert_wholly_dropped("the other base URL -> public API", &body);
}

/// A self-hosted client that never declared its server is the self-hosted
/// server at its base URL: it is sent what it returned itself, another
/// undeclared server at another address is not, a declared server is not,
/// and it is sent nobody else's material.
#[tokio::test]
async fn an_undeclared_self_hosted_server_is_its_base_url() {
    let process = new_process();
    let history = vec![user(QUESTION)];
    let (_, blocks) = stored_step(
        undeclared_self_hosted("box-one", &process),
        &history,
        &tool_step_response("u"),
    )
    .await;
    let own = vec![
        user(QUESTION),
        Message::BlockAssistant(BlockAssistantMessage::new(blocks, StopReason::ToolUse)),
        tool_output("u"),
    ];
    let body = body_on_the_wire(undeclared_self_hosted("box-one", &process), own.clone()).await;
    assert_eq!(
        body["input"],
        input_of([vec![user_item(QUESTION)], native_tool_step("u")]),
        "its own step"
    );
    for (case, client) in [
        (
            "another undeclared server",
            undeclared_self_hosted("box-two", &process),
        ),
        ("a declared server", Service::SelfHostedA.client(&process)),
        ("the public API", Service::OpenAiApi.client(&process)),
    ] {
        let body = body_on_the_wire(client, own.clone()).await;
        assert_wholly_dropped(case, &body);
        assert_eq!(
            body["input"],
            input_of([vec![user_item(QUESTION)], dropped_tool_step("u")]),
            "{case}"
        );
    }
    for source in Service::ALL {
        let case = format!("{source:?} -> undeclared self-hosted server");
        let transcript = then_asked(served_turn(source, &process, "t").await, NEXT_QUESTION);
        let body = body_on_the_wire(undeclared_self_hosted("box-one", &process), transcript).await;
        assert_wholly_dropped(&case, &body);
    }
}

/// One address under two kinds of service is two issuers: a self-hosted
/// client and a public-wire client pointed at the same base URL do not
/// replay each other's material. (The self-hosted client's Responses
/// delegate speaks the public wire; it is not the public API.)
#[tokio::test]
async fn one_address_under_two_service_kinds_is_two_issuers() {
    let process = new_process();
    let public_wire = |ledger: &Ledger| -> Arc<dyn LlmClient> {
        Arc::new(
            OpenAiClient::new_with_base_url("test-key".to_string(), endpoint("shared-address"))
                .with_continuity_ledger(Arc::clone(ledger)),
        )
    };
    let history = vec![user(QUESTION)];
    let (_, blocks) = stored_step(
        undeclared_self_hosted("shared-address", &process),
        &history,
        &tool_step_response("s"),
    )
    .await;
    let from_self_hosted = vec![
        user(QUESTION),
        Message::BlockAssistant(BlockAssistantMessage::new(blocks, StopReason::ToolUse)),
        tool_output("s"),
    ];
    let body = body_on_the_wire(public_wire(&process), from_self_hosted).await;
    assert_wholly_dropped("self-hosted -> public wire, one address", &body);

    let (_, blocks) = stored_step(public_wire(&process), &history, &tool_step_response("w")).await;
    let from_public_wire = vec![
        user(QUESTION),
        Message::BlockAssistant(BlockAssistantMessage::new(blocks, StopReason::ToolUse)),
        tool_output("w"),
    ];
    let body = body_on_the_wire(
        undeclared_self_hosted("shared-address", &process),
        from_public_wire,
    )
    .await;
    assert_wholly_dropped("public wire -> self-hosted, one address", &body);
}

// ---------------------------------------------------------------------------
// The lowering asks the owner too
// ---------------------------------------------------------------------------

/// The body builder a host can call directly, and the continuation plan,
/// ask the replay owner themselves: handed a canonical transcript that was
/// never projected, they lower no material the process did not receive
/// from the client's service, and name no response id of it.
#[tokio::test]
async fn lowering_an_unprojected_transcript_asks_the_owner() {
    let earlier = new_process();
    let stored = then_asked(
        served_turn(Service::OpenAiApi, &earlier, "t").await,
        NEXT_QUESTION,
    );
    let process = new_process();
    let foreign = then_asked(
        served_turn(Service::Copilot, &process, "f").await,
        NEXT_QUESTION,
    );
    let own = then_asked(
        served_turn(Service::OpenAiApi, &process, "o").await,
        NEXT_QUESTION,
    );
    let client = OpenAiClient::new_with_base_url("test-key".to_string(), endpoint("OpenAiApi"))
        .with_continuity_ledger(Arc::clone(&process));

    for (case, transcript, turn) in [
        ("a stored turn", stored, "t"),
        ("another service's turn", foreign, "f"),
    ] {
        let body = client
            .build_request_body(&request(transcript.clone()))
            .expect("request body");
        assert_wholly_dropped(case, &body);
        assert_eq!(
            body["input"],
            input_of([dropped_turn(turn, QUESTION), vec![user_item(NEXT_QUESTION)]]),
            "{case}"
        );
        let (body, plan) = client
            .build_request_body_with_continuation(&continuation_request(transcript))
            .expect("continuation body");
        assert!(
            plan.is_none(),
            "{case}: no continuation from a step that is not the target's"
        );
        assert_wholly_dropped(case, &body);
    }

    // A hosted-tool item carries its id in its content: the lowering omits
    // the item of a step the process did not receive, without a projection
    // having removed it first.
    let searched = hosted_search_turn(Service::OpenAiApi, &earlier, "h").await;
    let body = client
        .build_request_body(&request(searched))
        .expect("request body");
    assert_wholly_dropped("a stored hosted search", &body);
    assert!(
        !body.to_string().contains("web_search_call") && !body.to_string().contains("ws_h"),
        "a stored hosted search is not lowered: {body}"
    );

    let body = client
        .build_request_body(&request(own.clone()))
        .expect("request body");
    assert_eq!(
        body["input"],
        input_of([native_turn("o", QUESTION), vec![user_item(NEXT_QUESTION)]]),
        "its own turn is lowered as the service returned it"
    );
    let (body, plan) = client
        .build_request_body_with_continuation(&continuation_request(own))
        .expect("continuation body");
    assert!(plan.is_some());
    assert_eq!(body["previous_response_id"], "resp_o_2");
}

/// A self-hosted server that config declares is an issuer by its id: when
/// its base URL changes (the server moved, or a second address reaches it)
/// the material the process received from it is still its own. A client
/// that declares no server is the server at its address, so the same move
/// makes it another issuer.
#[tokio::test]
async fn a_declared_self_hosted_server_keeps_its_material_when_its_address_changes() {
    let process = new_process();
    let declared = |address: &str| -> Arc<dyn LlmClient> {
        Arc::new(
            self_hosted(endpoint(address))
                .with_self_hosted_server("local")
                .with_continuity_ledger(Arc::clone(&process)),
        )
    };
    let history = vec![user(QUESTION)];
    let (_, blocks) = stored_step(
        declared("first-address"),
        &history,
        &tool_step_response("d"),
    )
    .await;
    let transcript = vec![
        user(QUESTION),
        Message::BlockAssistant(BlockAssistantMessage::new(blocks, StopReason::ToolUse)),
        tool_output("d"),
    ];
    let body = body_on_the_wire(declared("second-address"), transcript.clone()).await;
    assert_eq!(
        body["input"],
        input_of([vec![user_item(QUESTION)], native_tool_step("d")]),
        "the declared server is the same issuer at its new address"
    );

    let (_, blocks) = stored_step(
        undeclared_self_hosted("first-address", &process),
        &history,
        &tool_step_response("u"),
    )
    .await;
    let transcript = vec![
        user(QUESTION),
        Message::BlockAssistant(BlockAssistantMessage::new(blocks, StopReason::ToolUse)),
        tool_output("u"),
    ];
    let body = body_on_the_wire(
        undeclared_self_hosted("second-address", &process),
        transcript,
    )
    .await;
    assert_wholly_dropped("an undeclared server at another address", &body);
}

// ---------------------------------------------------------------------------
// What is recorded is what is stored
// ---------------------------------------------------------------------------

/// A response whose stream ends at `[DONE]` without a terminal response:
/// the client commits what completed.
fn done_terminated_response(turn: &str) -> String {
    let output = [reasoning_item(turn, 1, None), call_item(turn)];
    let mut payload = format!(
        "data: {}\n\n",
        json!({"type": "response.created", "response": {"id": format!("resp_{turn}_1")}})
    );
    for (index, item) in output.iter().enumerate() {
        for kind in ["response.output_item.added", "response.output_item.done"] {
            payload.push_str(&format!(
                "data: {}\n\n",
                json!({"type": kind, "output_index": index, "item": item})
            ));
        }
    }
    payload.push_str("data: [DONE]\n\n");
    payload
}

/// A response a compatible server streams as events only: the terminal
/// response carries no output, so the session stores the blocks assembled
/// from the events.
fn events_only_response(turn: &str) -> String {
    let reasoning = reasoning_item(turn, 1, None);
    let events = [
        json!({"type": "response.created", "response": {"id": format!("resp_{turn}_1")}}),
        json!({"type": "response.reasoning.done", "item": reasoning}),
        json!({"type": "response.function_call_arguments.done", "call_id": format!("call_{turn}"),
            "name": "get_weather", "arguments": "{\"city\":\"Oslo\"}"}),
        json!({"type": "response.completed", "response": {
            "id": format!("resp_{turn}_1"), "status": "completed",
            "usage": {"input_tokens": 3, "output_tokens": 2, "total_tokens": 5}
        }}),
    ];
    let mut payload = String::new();
    for event in events {
        payload.push_str(&format!("data: {event}\n\n"));
    }
    payload.push_str("data: [DONE]\n\n");
    payload
}

/// Whatever path commits a step (the terminal response's output, the
/// `[DONE]` fallback, the blocks assembled from the events), the blocks the
/// session stores are known to the process that received them: the next
/// request of the tool loop replays the step's reasoning item to its
/// service, and another service gets the dropped form.
#[tokio::test]
async fn every_commit_path_stores_what_was_recorded() {
    for (path, response) in [
        ("terminal output", tool_step_response("p")),
        ("[DONE] fallback", done_terminated_response("p")),
        ("assembled from events", events_only_response("p")),
    ] {
        // The Copilot route takes an item's identity from its output slot
        // and has no events-only form.
        let services: &[Service] = if path == "assembled from events" {
            &[Service::OpenAiApi, Service::SelfHostedA]
        } else {
            &Service::ALL
        };
        for &service in services {
            let case = format!("{service:?}, {path}");
            let process = new_process();
            let history = vec![user(QUESTION)];
            let (_, blocks) = stored_step(service.client(&process), &history, &response).await;
            assert!(
                blocks
                    .iter()
                    .any(|block| matches!(block, AssistantBlock::Reasoning { .. }))
                    && blocks
                        .iter()
                        .any(|block| matches!(block, AssistantBlock::ToolUse { .. })),
                "{case}: the step stores its reasoning item and its call: {blocks:?}"
            );
            let transcript = vec![
                user(QUESTION),
                Message::BlockAssistant(BlockAssistantMessage::new(blocks, StopReason::ToolUse)),
                tool_output("p"),
            ];
            let own = body_on_the_wire(service.client(&process), transcript.clone()).await;
            assert_eq!(
                own["input"],
                input_of([vec![user_item(QUESTION)], native_tool_step("p")]),
                "{case}: the stored step is the recorded step"
            );
            let other = body_on_the_wire(service.other().client(&process), transcript).await;
            assert_wholly_dropped(&case, &other);
        }
    }
}

/// An answer a compatible server streams as events only: the message is
/// stored from its text events, which carry its item id, and the terminal
/// response carries no output.
fn events_only_answer_response(turn: &str) -> String {
    let message = message_item(turn, "It is 4 C in Oslo.");
    let events = [
        json!({"type": "response.created", "response": {"id": format!("resp_{turn}_1")}}),
        json!({"type": "response.output_item.added", "output_index": 0, "item": message}),
        json!({"type": "response.output_text.delta", "item_id": message["id"],
            "output_index": 0, "delta": "It is 4 C in Oslo."}),
        json!({"type": "response.completed", "response": {
            "id": format!("resp_{turn}_1"), "status": "completed",
            "usage": {"input_tokens": 3, "output_tokens": 2, "total_tokens": 5}
        }}),
    ];
    let mut payload = String::new();
    for event in events {
        payload.push_str(&format!("data: {event}\n\n"));
    }
    payload.push_str("data: [DONE]\n\n");
    payload
}

/// A message stored from its stream events alone is known as received: its
/// item id came with the text events, and the next request replays the
/// message under that id to its service and without it to another.
#[tokio::test]
async fn a_message_assembled_from_events_is_the_recorded_message() {
    for service in [Service::OpenAiApi, Service::SelfHostedA] {
        let process = new_process();
        let history = vec![user(QUESTION)];
        let (_, blocks) = stored_step(
            service.client(&process),
            &history,
            &events_only_answer_response("e"),
        )
        .await;
        assert!(
            blocks.iter().any(|block| matches!(
                block,
                AssistantBlock::Text { meta: Some(meta), .. }
                    if matches!(meta.as_ref(), ProviderMeta::OpenAiAssistantMessage { id, .. } if id == "msg_e")
            )),
            "{service:?}: the message is stored with the id its events carried: {blocks:?}"
        );
        let transcript = vec![
            user(QUESTION),
            Message::BlockAssistant(BlockAssistantMessage::new(blocks, StopReason::EndTurn)),
            user(NEXT_QUESTION),
        ];
        let own = body_on_the_wire(service.client(&process), transcript.clone()).await;
        let answer = own["input"]
            .as_array()
            .expect("input")
            .iter()
            .find(|item| item["type"] == "message" && item["role"] == "assistant")
            .unwrap_or_else(|| panic!("{service:?}: the answer replays"));
        assert_eq!(
            answer["id"], "msg_e",
            "{service:?}: the stored message is the recorded message: {answer}"
        );
        let other = body_on_the_wire(service.other().client(&process), transcript).await;
        assert_wholly_dropped(&format!("{service:?}: events-only answer"), &other);
    }
}

// ---------------------------------------------------------------------------
// One decision per request
// ---------------------------------------------------------------------------

/// The decision is made once, when the request is prepared, and travels
/// with it: what the process receives or forgets between the projection and
/// the send changes neither the body nor its measured pressure. Here the
/// process forgets the step (its ledger holds one response per generation
/// and receives two more) after the request was prepared.
#[tokio::test]
async fn a_prepared_request_keeps_its_decision() {
    for service in Service::ALL {
        let process: Ledger = Arc::new(OpenAiContinuityLedger::with_generation(1));
        let history = vec![user(QUESTION)];
        let (_, blocks) =
            stored_step(service.client(&process), &history, &tool_step_response("t")).await;
        let transcript = vec![
            user(QUESTION),
            Message::BlockAssistant(BlockAssistantMessage::new(blocks, StopReason::ToolUse)),
            tool_output("t"),
        ];

        let client = service.client(&process);
        let projection = client
            .project_replay_request(&transcript)
            .expect("projection");
        let prepared =
            meerkat_llm_core::PreparedLlmRequest::from_projection(request(Vec::new()), projection);
        let pressure_before = client
            .prepared_request_pressure(&prepared)
            .expect("pressure")
            .expect("the client measures its request")
            .encoded_bytes;

        // Two later responses push the step out of both generations.
        for later in ["x", "y"] {
            stored_step(
                service.client(&process),
                &history,
                &tool_step_response(later),
            )
            .await;
        }
        let forgotten = body_on_the_wire(service.client(&process), transcript.clone()).await;
        assert_wholly_dropped(
            &format!("{service:?}: the process forgot the step"),
            &forgotten,
        );

        answer_with(PLAIN_ANSWER_SSE);
        let mut stream = client.stream_prepared(&prepared);
        while let Some(event) = stream.next().await {
            event.unwrap_or_else(|error| panic!("the prepared request is accepted: {error}"));
        }
        drop(stream);
        let bodies = bodies_on_the_wire();
        assert_eq!(bodies.len(), 1, "{service:?}");
        assert_eq!(
            bodies[0]["input"],
            input_of([vec![user_item(QUESTION)], native_tool_step("t")]),
            "{service:?}: the prepared request is sent under the decision it was prepared with"
        );
        assert_eq!(
            client
                .prepared_request_pressure(&prepared)
                .expect("pressure")
                .expect("the client measures its request")
                .encoded_bytes,
            pressure_before,
            "{service:?}: the measured request is the sent request"
        );
    }
}

/// The cache claims of a prepared request are authored for the body its
/// projection decided: they do not change when the process forgets the step
/// afterwards, while claims authored without the decision are those of the
/// dropped form.
#[tokio::test]
async fn a_prepared_request_keeps_its_cache_claims() {
    use meerkat_core::lifecycle::run_primitive::OpenAiPromptCacheOptions;
    use meerkat_core::model_profile::capabilities::{OpenAiPromptCacheMode, OpenAiPromptCacheTtl};

    let process: Ledger = Arc::new(OpenAiContinuityLedger::with_generation(1));
    let history = vec![user(QUESTION)];
    let (_, blocks) = stored_step(
        Service::OpenAiApi.client(&process),
        &history,
        &tool_step_response("t"),
    )
    .await;
    let transcript = vec![
        user(QUESTION),
        Message::BlockAssistant(BlockAssistantMessage::new(blocks, StopReason::ToolUse)),
        tool_output("t"),
    ];
    let client = Service::OpenAiApi.client(&process);
    let projection = client
        .project_replay_request(&transcript)
        .expect("projection");
    let cached = LlmRequest::new("gpt-5.6-sol", Vec::new()).with_openai_tag_merge(|tag| {
        tag.prompt_cache_options = Some(OpenAiPromptCacheOptions {
            mode: Some(OpenAiPromptCacheMode::Explicit),
            ttl: Some(OpenAiPromptCacheTtl::ThirtyMinutes),
        });
    });
    let prepared = meerkat_llm_core::PreparedLlmRequest::from_projection(cached, projection);
    let claims = |prepared: &meerkat_llm_core::PreparedLlmRequest| {
        format!(
            "{:?}",
            client
                .prepared_cache_breakpoints(prepared, &transcript)
                .expect("cache claims")
        )
    };
    let authored = client
        .prepared_cache_breakpoints(&prepared, &transcript)
        .expect("cache claims");
    assert!(!authored.is_empty(), "explicit mode authors claims");
    let before = claims(&prepared);

    for later in ["x", "y"] {
        stored_step(
            Service::OpenAiApi.client(&process),
            &history,
            &tool_step_response(later),
        )
        .await;
    }
    assert_eq!(
        claims(&prepared),
        before,
        "the claims are those of the body the request was prepared with"
    );
    let undecided = format!(
        "{:?}",
        client
            .authored_cache_breakpoints(prepared.request(), &transcript)
            .expect("cache claims")
    );
    assert_ne!(
        undecided, before,
        "authored now, without the decision, the claims are those of the dropped form"
    );
}

/// Projecting a projected transcript again changes nothing, under the same
/// process state: the dropped form stays dropped and the admitted form
/// stays admitted.
#[tokio::test]
async fn projecting_twice_is_projecting_once() {
    for service in Service::ALL {
        let earlier = new_process();
        let stored = served_turn(service, &earlier, "old").await;
        let process = new_process();
        let (mixed, _) = tool_turn(service, &process, stored, "new", NEXT_QUESTION).await;
        let client = service.client(&process);
        let once = client.project_replay_messages(&mixed).expect("projection");
        let twice = client.project_replay_messages(&once).expect("projection");
        assert_eq!(once, twice, "{service:?}");
    }
}

// ---------------------------------------------------------------------------
// An identity-only message
// ---------------------------------------------------------------------------

/// A step whose message is empty (it exists only as an identified item)
/// replays to its service as that item. In the dropped form it is left out:
/// an empty assistant message is never sent.
#[tokio::test]
async fn an_empty_identified_message_is_omitted_in_the_dropped_form() {
    let empty_then_call = response_stream(
        "resp_e_1",
        &[
            reasoning_item("e", 1, None),
            json!({
                "type": "message", "id": "msg_e", "role": "assistant", "status": "completed",
                "phase": "commentary",
                "content": [{"type": "output_text", "text": "", "annotations": []}],
            }),
            call_item("e"),
        ],
    );
    for service in Service::ALL {
        let process = new_process();
        let history = vec![user(QUESTION)];
        let (_, blocks) = stored_step(service.client(&process), &history, &empty_then_call).await;
        let transcript = vec![
            user(QUESTION),
            Message::BlockAssistant(BlockAssistantMessage::new(blocks, StopReason::ToolUse)),
            tool_output("e"),
        ];
        let own = body_on_the_wire(service.client(&process), transcript.clone()).await;
        let own_input = own["input"].as_array().expect("input");
        assert!(
            own_input
                .iter()
                .any(|item| item["type"] == "message" && item["id"] == "msg_e"),
            "{service:?}: its own identified empty message replays: {own}"
        );
        let restarted = new_process();
        let body = body_on_the_wire(service.client(&restarted), transcript).await;
        assert_wholly_dropped(&format!("{service:?}, cold load"), &body);
        assert_eq!(
            body["input"],
            input_of([vec![user_item(QUESTION)], dropped_tool_step("e")]),
            "{service:?}: no empty assistant message in the dropped form"
        );
    }
}

// ---------------------------------------------------------------------------
// The process ledger and the GitHub Copilot route
// ---------------------------------------------------------------------------

#[cfg(feature = "copilot")]
mod copilot_route {
    use super::*;
    use crate::runtime::{CopilotOpenAiRouteTarget, copilot_openai_route_client};

    struct CopilotTokenAuthorizer;

    #[async_trait::async_trait]
    impl meerkat_core::HttpAuthorizer for CopilotTokenAuthorizer {
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

    /// The Copilot route puts its client behind the account's capability
    /// gate. The gate carries the route client's replay decision with the
    /// prepared request: after the process forgets the step, or another
    /// service returns the same material, the request is still measured and
    /// sent as it was prepared.
    #[tokio::test]
    async fn the_capability_gate_keeps_the_prepared_decision() {
        for change in [
            "a generation rotates",
            "another service returns the same material",
        ] {
            let process: Ledger = Arc::new(OpenAiContinuityLedger::with_generation(2));
            let history = vec![user(QUESTION)];
            let (_, blocks) = stored_step(
                Service::Copilot.client(&process),
                &history,
                &tool_step_response("t"),
            )
            .await;
            let transcript = vec![
                user(QUESTION),
                Message::BlockAssistant(BlockAssistantMessage::new(
                    blocks.clone(),
                    StopReason::ToolUse,
                )),
                tool_output("t"),
            ];
            let gated = meerkat_copilot::capability_gated_client(
                Service::Copilot.client(&process),
                Some(meerkat_copilot::CopilotModelCapabilities {
                    limits: meerkat_copilot::CopilotModelLimits {
                        max_context_window_tokens: Some(128_000),
                        max_output_tokens: Some(16_000),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
            );
            assert!(
                gated.prompt_token_limit(1024).is_some(),
                "the gate is in place"
            );
            let projection = gated
                .project_replay_request(&transcript)
                .expect("projection");
            let prepared = meerkat_llm_core::PreparedLlmRequest::from_projection(
                request(Vec::new()),
                projection,
            );
            let measured = gated
                .prepared_request_pressure(&prepared)
                .expect("pressure")
                .expect("the client measures its request")
                .encoded_bytes;

            if change == "a generation rotates" {
                for later in ["x", "y", "z"] {
                    stored_step(
                        Service::Copilot.client(&process),
                        &history,
                        &tool_step_response(later),
                    )
                    .await;
                }
            } else {
                process.record_step(
                    &meerkat_core::OpenAiContinuityIssuer::at_base_url(
                        meerkat_core::OpenAiContinuityService::Copilot,
                        "https://elsewhere.example.test",
                    ),
                    &blocks,
                );
            }
            let now = body_on_the_wire(Service::Copilot.client(&process), transcript.clone()).await;
            assert_wholly_dropped(&format!("{change}: decided now"), &now);

            answer_with(PLAIN_ANSWER_SSE);
            let mut stream = gated.stream_prepared(&prepared);
            while let Some(event) = stream.next().await {
                event.unwrap_or_else(|error| panic!("the prepared request is accepted: {error}"));
            }
            drop(stream);
            let bodies = bodies_on_the_wire();
            assert_eq!(bodies.len(), 1, "{change}");
            assert_eq!(
                bodies[0]["input"],
                input_of([vec![user_item(QUESTION)], native_tool_step("t")]),
                "{change}: sent under the decision it was prepared with"
            );
            assert_eq!(
                gated
                    .prepared_request_pressure(&prepared)
                    .expect("pressure")
                    .expect("the client measures its request")
                    .encoded_bytes,
                measured,
                "{change}: the measured request is the sent request"
            );
        }
    }

    /// The client the provider runtime builds for an OpenAI model's GitHub
    /// Copilot `/responses` route. Like every client a host builds, it
    /// shares the process's ledger.
    fn route_client() -> Arc<dyn LlmClient> {
        copilot_openai_route_client(
            meerkat_copilot::CopilotEndpoint::Responses,
            endpoint("copilot-route"),
            Arc::new(CopilotTokenAuthorizer),
            &CopilotOpenAiRouteTarget {
                model: MODEL.to_string(),
                supports_temperature: false,
                supports_image_input: true,
                supports_image_tool_results: true,
            },
            MODEL,
        )
        .expect("Copilot route client")
    }

    /// The public API client a host builds, sharing the process's ledger.
    fn public_api_client() -> Arc<dyn LlmClient> {
        Arc::new(OpenAiClient::new_with_base_url(
            "test-key".to_string(),
            endpoint("public-api"),
        ))
    }

    async fn first_step(client: Arc<dyn LlmClient>, turn: &str) -> Vec<Message> {
        let history = vec![user(QUESTION)];
        let (_, blocks) = stored_step(client, &history, &tool_step_response(turn)).await;
        vec![
            user(QUESTION),
            Message::BlockAssistant(BlockAssistantMessage::new(blocks, StopReason::ToolUse)),
            tool_output(turn),
        ]
    }

    /// The observed failure, in one process: a session's step served by the
    /// public OpenAI API, then the session moves to the GitHub Copilot route.
    /// The route's request carries no reasoning item of the public API.
    #[tokio::test]
    async fn the_copilot_route_is_not_sent_public_api_reasoning() {
        let transcript = first_step(public_api_client(), "public_then_route").await;
        let body = body_on_the_wire(route_client(), transcript).await;
        assert_wholly_dropped("public API -> Copilot route", &body);
        assert_eq!(
            body["input"],
            input_of([
                vec![user_item(QUESTION)],
                dropped_tool_step("public_then_route")
            ])
        );
    }

    /// Two clients of one service in one process share what the process
    /// received: a step one Copilot route client received replays through
    /// another built for the same API base, and does not replay to the
    /// public API.
    #[tokio::test]
    async fn clients_of_one_process_share_what_it_received() {
        let transcript = first_step(route_client(), "route_shared").await;
        let body = body_on_the_wire(route_client(), transcript.clone()).await;
        assert_eq!(
            body["input"],
            input_of([vec![user_item(QUESTION)], native_tool_step("route_shared")])
        );
        let body = body_on_the_wire(public_api_client(), transcript).await;
        assert_wholly_dropped("Copilot route -> public API", &body);
    }
}

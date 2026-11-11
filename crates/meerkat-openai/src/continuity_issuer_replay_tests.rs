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

use std::sync::{Arc, Mutex};

use axum::{Json, Router, extract::State, response::IntoResponse};
use futures::StreamExt;
use meerkat_core::{
    AssistantBlock, BlockAssistantMessage, Message, OpenAiContinuityIssuer, OpenAiContinuityLedger,
    ProviderMeta, StopReason, ToolDef, ToolResult, UserMessage,
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

    /// The client that speaks to this service at `base` in the process
    /// `ledger` stands for, built with the builders the provider runtime and
    /// the factory use for it.
    fn client(self, base: String, ledger: &Ledger) -> Arc<dyn LlmClient> {
        let openai = || {
            OpenAiClient::new_with_base_url("test-key".to_string(), base.clone())
                .with_continuity_ledger(Arc::clone(ledger))
        };
        let self_hosted_server = |server: &str| {
            Arc::new(
                self_hosted(base.clone())
                    .with_continuity_issuer(OpenAiContinuityIssuer::SelfHosted {
                        server: server.to_string(),
                    })
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

/// A self-hosted client that never declared its server.
fn undeclared_self_hosted(base: String, ledger: &Ledger) -> Arc<dyn LlmClient> {
    Arc::new(self_hosted(base).with_continuity_ledger(Arc::clone(ledger)))
}

// ---------------------------------------------------------------------------
// The wire
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Capture {
    bodies: Arc<Mutex<Vec<Value>>>,
    payload: Arc<String>,
}

async fn capture_body(
    State(capture): State<Capture>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    capture.bodies.lock().expect("capture lock").push(body);
    (
        [("content-type", "text/event-stream")],
        capture.payload.as_str().to_owned(),
    )
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

/// Send `request` through the client `build` makes for a local server that
/// answers every path with `payload`. Returns the bodies that reached the
/// wire and the events the client produced.
async fn send(
    build: impl FnOnce(String) -> Arc<dyn LlmClient>,
    request: &LlmRequest,
    payload: &str,
) -> (Vec<Value>, Vec<LlmEvent>) {
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new().fallback(capture_body).with_state(Capture {
        bodies: Arc::clone(&bodies),
        payload: Arc::new(payload.to_string()),
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    let client = build(format!("http://{addr}"));
    let mut events = Vec::new();
    let mut stream = client.stream(request);
    while let Some(event) = stream.next().await {
        events.push(event.unwrap_or_else(|error| panic!("the request is accepted: {error}")));
    }
    drop(stream);
    server.abort();
    let bodies = std::mem::take(&mut *bodies.lock().expect("capture lock"));
    (bodies, events)
}

fn request(messages: Vec<Message>) -> LlmRequest {
    let mut request = LlmRequest::new(MODEL, messages);
    request.tools = vec![weather_tool()];
    request
}

/// An answer with no output items: for rows that read only the request.
const PLAIN_ANSWER_SSE: &str = concat!(
    "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n",
    "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",",
    "\"usage\":{\"input_tokens\":4,\"output_tokens\":1}}}\n\n",
    "data: [DONE]\n\n",
);

/// The one body `messages` puts on the wire of the client `build` makes.
async fn body_on_the_wire(
    build: impl FnOnce(String) -> Arc<dyn LlmClient>,
    messages: Vec<Message>,
) -> Value {
    let (mut bodies, _) = send(build, &request(messages), PLAIN_ANSWER_SSE).await;
    assert_eq!(bodies.len(), 1, "one request reached the wire: {bodies:?}");
    bodies.remove(0)
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

fn committed_blocks(events: Vec<LlmEvent>) -> Vec<AssistantBlock> {
    events
        .into_iter()
        .find_map(|event| match event {
            LlmEvent::AssistantOutput { blocks, .. } => Some(blocks),
            _ => None,
        })
        .expect("the response commits its output")
}

/// One provider step: `history` is sent to `service`, which answers with
/// `response`. Returns the request body and the committed assistant message.
async fn step(
    service: Service,
    ledger: &Ledger,
    history: &[Message],
    response: &str,
    stop_reason: StopReason,
) -> (Value, Message) {
    let (mut bodies, events) = send(
        |base| service.client(base, ledger),
        &request(history.to_vec()),
        response,
    )
    .await;
    assert_eq!(bodies.len(), 1, "one request reached the wire: {bodies:?}");
    let message = Message::BlockAssistant(BlockAssistantMessage::new(
        committed_blocks(events),
        stop_reason,
    ));
    (bodies.remove(0), message)
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
            let body =
                body_on_the_wire(|base| target.client(base, &process), transcript.clone()).await;
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
            |base| service.client(base, &process),
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
                |base| target.client(base, &restarted),
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
            let body = body_on_the_wire(
                |base| target.client(base, &restarted),
                transcript[..3].to_vec(),
            )
            .await;
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
            |base| service.client(base, &restarted),
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

/// A self-hosted client that never declared its server is nobody's issuer:
/// what it receives is recorded for no one and it is sent nobody's material,
/// its own included.
#[tokio::test]
async fn an_undeclared_self_hosted_server_is_replayed_no_continuity() {
    let process = new_process();
    let (mut bodies, events) = send(
        |base| undeclared_self_hosted(base, &process),
        &request(vec![user(QUESTION)]),
        &tool_step_response("u"),
    )
    .await;
    assert_eq!(bodies.len(), 1);
    bodies.clear();
    let own = vec![
        user(QUESTION),
        Message::BlockAssistant(BlockAssistantMessage::new(
            committed_blocks(events),
            StopReason::ToolUse,
        )),
        tool_output("u"),
    ];
    let body = body_on_the_wire(|base| undeclared_self_hosted(base, &process), own).await;
    assert_wholly_dropped("its own step", &body);
    assert_eq!(
        body["input"],
        input_of([vec![user_item(QUESTION)], dropped_tool_step("u")])
    );

    for source in Service::ALL {
        let case = format!("{source:?} -> undeclared self-hosted server");
        let transcript = then_asked(served_turn(source, &process, "t").await, NEXT_QUESTION);
        let body =
            body_on_the_wire(|base| undeclared_self_hosted(base, &process), transcript).await;
        assert_wholly_dropped(&case, &body);
        assert_eq!(
            body["input"],
            input_of([dropped_turn("t", QUESTION), vec![user_item(NEXT_QUESTION)]]),
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
            |base| service.client(base, &process),
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
        let process: Ledger = Arc::new(OpenAiContinuityLedger::with_capacity(1));
        let transcript = served_turn(service, &process, "t").await;
        let case = format!("{service:?}");
        let body = body_on_the_wire(
            |base| service.client(base, &process),
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
            |base| target.client(base, &process),
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
            |base| service.client(base, &process),
            then_asked(fork.messages().to_vec(), NEXT_QUESTION),
        )
        .await;
        assert_eq!(body["input"], expected, "{service:?}: a fork");

        // Written to rows and read back between two steps.
        let rows = serde_json::to_vec(&transcript).expect("rows");
        let read_back: Vec<Message> = serde_json::from_slice(&rows).expect("rows decode");
        assert_eq!(read_back, transcript, "{service:?}: live equals stored");
        let body = body_on_the_wire(
            |base| service.client(base, &process),
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
        let body = body_on_the_wire(|base| service.client(base, &process), rewritten).await;
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
                let client = service.client("http://127.0.0.1:9".to_string(), &process);
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
            |base| target.client(base, &process),
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
        let body = body_on_the_wire(
            |base| target.client(base, &process),
            transcript[..3].to_vec(),
        )
        .await;
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

        let body =
            body_on_the_wire(|base| service.client(base, &process), transcript.clone()).await;
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
            let body =
                body_on_the_wire(|base| target.client(base, ledger), transcript.clone()).await;
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
            .client("http://127.0.0.1:9".to_string(), &process)
            .project_replay_messages(&transcript)
            .expect("the first service's projection");
        let body = body_on_the_wire(|base| sender.client(base, &process), prepared).await;
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
    .with_continuity_issuer(OpenAiContinuityIssuer::SelfHosted {
        server: "local-a".to_string(),
    })
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
            |base| service.client(base, &process),
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
                |base| target.client(base, ledger),
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
        |base| service.client(base, &process),
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
        |base| service.client(base, &process),
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

    /// The client the provider runtime builds for an OpenAI model's GitHub
    /// Copilot `/responses` route. Like every client a host builds, it
    /// shares the process's ledger.
    fn route_client(base: String) -> Arc<dyn LlmClient> {
        copilot_openai_route_client(
            meerkat_copilot::CopilotEndpoint::Responses,
            base,
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
    fn public_api_client(base: String) -> Arc<dyn LlmClient> {
        Arc::new(OpenAiClient::new_with_base_url(
            "test-key".to_string(),
            base,
        ))
    }

    async fn first_step(
        build: impl FnOnce(String) -> Arc<dyn LlmClient>,
        turn: &str,
    ) -> Vec<Message> {
        let (_, events) = send(
            build,
            &request(vec![user(QUESTION)]),
            &tool_step_response(turn),
        )
        .await;
        vec![
            user(QUESTION),
            Message::BlockAssistant(BlockAssistantMessage::new(
                committed_blocks(events),
                StopReason::ToolUse,
            )),
            tool_output(turn),
        ]
    }

    /// The observed failure, in one process: a session's step served by the
    /// public OpenAI API, then the session moves to the GitHub Copilot route.
    /// The route's request carries no reasoning item of the public API.
    #[tokio::test]
    async fn the_copilot_route_is_not_sent_public_api_reasoning() {
        let transcript = first_step(public_api_client, "public_then_route").await;
        let body = body_on_the_wire(route_client, transcript).await;
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
    /// another, and does not replay to the public API.
    #[tokio::test]
    async fn clients_of_one_process_share_what_it_received() {
        let transcript = first_step(route_client, "route_shared").await;
        let body = body_on_the_wire(route_client, transcript.clone()).await;
        assert_eq!(
            body["input"],
            input_of([vec![user_item(QUESTION)], native_tool_step("route_shared")])
        );
        let body = body_on_the_wire(public_api_client, transcript).await;
        assert_wholly_dropped("Copilot route -> public API", &body);
    }
}

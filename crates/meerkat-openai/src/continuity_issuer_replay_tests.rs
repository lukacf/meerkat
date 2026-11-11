//! OpenAI-family continuity material is replayed only to the service that
//! issued it.
//!
//! The public OpenAI API, the ChatGPT backend, Azure OpenAI, GitHub Copilot
//! and self-hosted servers speak one Responses grammar, but encrypted
//! reasoning, item ids and response ids mean something only to the service
//! that issued them. A stored transcript is sent, through the same replay
//! projection and request path every turn uses, to a local server standing in
//! for each service, and the request body that reached the wire is checked:
//!
//! - material issued by another service, or by an unknown one (everything
//!   stored before issuers were recorded), is left out, in the form every
//!   service already receives for a history from another provider family: no
//!   reasoning item, no provider item id, no `previous_response_id`, tool
//!   calls and their outputs paired by `call_id`;
//! - material issued by the service itself replays exactly as before.
//!
//! Stored assistant blocks are written as the JSON a session store holds, so
//! the rows read the persisted shape and not a constructor's.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use axum::{Json, Router, extract::State, response::IntoResponse};
use futures::StreamExt;
use meerkat_core::{
    AssistantBlock, BlockAssistantMessage, Message, OpenAiContinuityIssuer, StopReason, ToolDef,
    ToolResult, UserMessage,
};
use meerkat_llm_core::{LlmClient, LlmEvent, LlmRequest};
use serde_json::{Value, json};
use tokio::net::TcpListener;

use crate::client_compatible::OpenAiCompatibleMode;
use crate::{AzureOpenAiWireConfig, OpenAiClient, OpenAiCompatibleClient};

const MODEL: &str = "gpt-5.4";
const SELF_HOSTED_REMOTE_MODEL: &str = "served-model";

/// An answer with no output items: the rows read the request, not the reply.
const PLAIN_ANSWER_SSE: &str = concat!(
    "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n",
    "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",",
    "\"usage\":{\"input_tokens\":4,\"output_tokens\":1}}}\n\n",
    "data: [DONE]\n\n",
);

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

    /// The issuer as a transcript stores it on provider metadata.
    fn stored_issuer(self) -> Value {
        match self {
            Self::OpenAiApi => json!({"kind": "openai_api"}),
            Self::ChatGptBackend => json!({"kind": "chatgpt_backend"}),
            Self::AzureOpenAi => json!({"kind": "azure_openai"}),
            Self::Copilot => json!({"kind": "copilot"}),
            Self::SelfHostedA => json!({"kind": "self_hosted", "server": "local-a"}),
            Self::SelfHostedB => json!({"kind": "self_hosted", "server": "local-b"}),
        }
    }

    /// The client that speaks to this service at `base`, built with the
    /// builders the provider runtime and the factory use for it.
    fn client(self, base: String) -> Arc<dyn LlmClient> {
        let openai = || OpenAiClient::new_with_base_url("test-key".to_string(), base.clone());
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
            Self::SelfHostedA => Arc::new(self_hosted(base.clone()).with_continuity_issuer(
                OpenAiContinuityIssuer::SelfHosted {
                    server: "local-a".to_string(),
                },
            )),
            Self::SelfHostedB => Arc::new(self_hosted(base.clone()).with_continuity_issuer(
                OpenAiContinuityIssuer::SelfHosted {
                    server: "local-b".to_string(),
                },
            )),
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
fn undeclared_self_hosted(base: String) -> Arc<dyn LlmClient> {
    Arc::new(self_hosted(base))
}

// ---------------------------------------------------------------------------
// Stored transcripts
// ---------------------------------------------------------------------------

fn block(stored: Value) -> AssistantBlock {
    serde_json::from_value(stored).expect("stored assistant block decodes")
}

/// Provider metadata as stored, with `issuer` recorded when one is known.
fn meta(mut stored: Value, issuer: Option<&Value>) -> Value {
    if let Some(issuer) = issuer {
        stored["issuer"] = issuer.clone();
    }
    stored
}

/// The first provider step of a tool turn: an encrypted reasoning item and
/// the function call it led to.
fn stored_tool_step(issuer: Option<&Value>) -> Message {
    Message::BlockAssistant(BlockAssistantMessage::new(
        vec![
            block(json!({"block_type": "reasoning", "data": {
                "text": "",
                "meta": meta(json!({
                    "provider": "open_ai",
                    "id": "rs_1",
                    "encrypted_content": "ciphertext-1",
                    "response_id": "resp_1"
                }), issuer)
            }})),
            block(json!({"block_type": "tool_use", "data": {
                "id": "call_1",
                "name": "get_weather",
                "args": {"city": "Oslo"},
                "meta": meta(json!({
                    "provider": "open_ai_response",
                    "response_id": "resp_1"
                }), issuer)
            }})),
        ],
        StopReason::ToolUse,
    ))
}

fn tool_output() -> Message {
    Message::tool_results(vec![ToolResult::new(
        "call_1".to_string(),
        "4 C".to_string(),
        false,
    )])
}

/// The answering step: a reasoning item with a summary and a phased
/// assistant message with its item id.
fn stored_answer_step(issuer: Option<&Value>) -> Message {
    Message::BlockAssistant(BlockAssistantMessage::new(
        vec![
            block(json!({"block_type": "reasoning", "data": {
                "text": "Summarize.",
                "meta": meta(json!({
                    "provider": "open_ai",
                    "id": "rs_2",
                    "encrypted_content": "ciphertext-2",
                    "response_id": "resp_2"
                }), issuer)
            }})),
            block(json!({"block_type": "text", "data": {
                "text": "It is 4 C in Oslo.",
                "meta": meta(json!({
                    "provider": "open_ai_assistant_message",
                    "id": "msg_1",
                    "phase": "final_answer",
                    "response_id": "resp_2"
                }), issuer)
            }})),
        ],
        StopReason::EndTurn,
    ))
}

fn user(text: &str) -> Message {
    Message::User(UserMessage::text(text.to_string()))
}

/// A finished tool turn followed by the next user message.
fn finished_turn_then_question(issuer: Option<&Value>) -> Vec<Message> {
    vec![
        user("What is the weather in Oslo?"),
        stored_tool_step(issuer),
        tool_output(),
        stored_answer_step(issuer),
        user("And tomorrow?"),
    ]
}

/// A run that stopped inside its tool loop: the call and its output are
/// recorded and the model has not answered yet.
fn interrupted_tool_loop(issuer: Option<&Value>) -> Vec<Message> {
    vec![
        user("What is the weather in Oslo?"),
        stored_tool_step(issuer),
        tool_output(),
    ]
}

// ---------------------------------------------------------------------------
// The request forms
// ---------------------------------------------------------------------------

fn user_item(text: &str) -> Value {
    json!({"type": "message", "role": "user", "content": text})
}

fn call_item() -> Value {
    json!({
        "type": "function_call",
        "call_id": "call_1",
        "name": "get_weather",
        "arguments": "{\"city\":\"Oslo\"}"
    })
}

fn output_item() -> Value {
    json!({"type": "function_call_output", "call_id": "call_1", "output": "4 C"})
}

/// The input a service receives for [`finished_turn_then_question`] when the
/// continuity material is its own: every item as the service returned it.
fn native_input() -> Value {
    json!([
        user_item("What is the weather in Oslo?"),
        {"type": "reasoning", "id": "rs_1", "summary": [], "encrypted_content": "ciphertext-1"},
        call_item(),
        output_item(),
        {
            "type": "reasoning",
            "id": "rs_2",
            "summary": [{"type": "summary_text", "text": "Summarize."}],
            "encrypted_content": "ciphertext-2"
        },
        {
            "type": "message",
            "role": "assistant",
            "id": "msg_1",
            "status": "completed",
            "phase": "final_answer",
            "content": [{"type": "output_text", "text": "It is 4 C in Oslo.", "annotations": []}]
        },
        user_item("And tomorrow?"),
    ])
}

/// The input for the same transcript when the material is not the target's:
/// what the conversation said, with nothing a service issued.
fn dropped_input() -> Value {
    json!([
        user_item("What is the weather in Oslo?"),
        call_item(),
        output_item(),
        {"type": "message", "role": "assistant", "content": "It is 4 C in Oslo."},
        user_item("And tomorrow?"),
    ])
}

/// The dropped form of [`interrupted_tool_loop`].
fn dropped_tool_loop_input() -> Value {
    json!([
        user_item("What is the weather in Oslo?"),
        call_item(),
        output_item(),
    ])
}

/// What a request in the dropped form must not carry, checked on the whole
/// body and not only on the items the expected form names.
fn assert_carries_no_provider_continuity(case: &str, body: &Value) {
    let input = body["input"].as_array().expect("input array");
    for item in input {
        assert_ne!(
            item["type"], "reasoning",
            "{case}: a reasoning item: {item}"
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
        "{case}: a response id of another issuer: {body}"
    );
    let text = body.to_string();
    for opaque in ["ciphertext", "rs_", "msg_1", "resp_"] {
        assert!(
            !text.contains(opaque),
            "{case}: `{opaque}` reached the wire: {text}"
        );
    }
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
                        || kind == "message"),
                    "{case}: a reasoning item without its following item"
                );
            }
            _ => {}
        }
    }
    assert!(open.is_empty(), "{case}: calls without an output: {open:?}");
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

/// The one body `messages` puts on the wire of the client `build` makes.
async fn body_on_the_wire(
    build: impl FnOnce(String) -> Arc<dyn LlmClient>,
    messages: Vec<Message>,
) -> Value {
    let mut request = LlmRequest::new(MODEL, messages);
    request.tools = vec![weather_tool()];
    let (mut bodies, _) = send(build, &request, PLAIN_ANSWER_SSE).await;
    assert_eq!(bodies.len(), 1, "one request reached the wire: {bodies:?}");
    bodies.remove(0)
}

// ---------------------------------------------------------------------------
// Another issuer, the same issuer, an unknown issuer
// ---------------------------------------------------------------------------

/// A transcript whose continuity material one service issued is sent to
/// every other service without it: no reasoning item, no item id, no
/// dangling reference. Every ordered pair of services.
#[tokio::test]
async fn material_of_another_issuer_is_not_replayed() {
    for source in Service::ALL {
        for target in Service::ALL {
            if source == target {
                continue;
            }
            let case = format!("{source:?} -> {target:?}");
            let body = body_on_the_wire(
                |base| target.client(base),
                finished_turn_then_question(Some(&source.stored_issuer())),
            )
            .await;
            assert_carries_no_provider_continuity(&case, &body);
            assert_eq!(body["input"], dropped_input(), "{case}");
        }
    }
}

/// The path that switches nothing keeps its continuity: a service receives
/// its own reasoning items, message ids and phases byte for byte.
#[tokio::test]
async fn material_of_the_same_issuer_replays_unchanged() {
    for service in Service::ALL {
        let body = body_on_the_wire(
            |base| service.client(base),
            finished_turn_then_question(Some(&service.stored_issuer())),
        )
        .await;
        assert_eq!(body["input"], native_input(), "{service:?}");
        assert_calls_and_outputs_pair(
            &format!("{service:?}"),
            body["input"].as_array().expect("input array"),
        );
    }
}

/// Material that names no issuer (everything stored before issuers were
/// recorded) is not provably the target's: no service is sent it.
#[tokio::test]
async fn material_of_an_unknown_issuer_is_not_replayed() {
    for target in Service::ALL {
        let case = format!("unknown -> {target:?}");
        let body = body_on_the_wire(
            |base| target.client(base),
            finished_turn_then_question(None),
        )
        .await;
        assert_carries_no_provider_continuity(&case, &body);
        assert_eq!(body["input"], dropped_input(), "{case}");
    }
}

/// A self-hosted client that never declared its server has no issuer: it is
/// sent nobody's continuity material, a self-hosted server's included.
#[tokio::test]
async fn an_undeclared_self_hosted_server_is_replayed_no_continuity() {
    let issuers = Service::ALL
        .into_iter()
        .map(|service| Some(service.stored_issuer()))
        .chain([None]);
    for issuer in issuers {
        let case = format!("{issuer:?} -> undeclared self-hosted server");
        let body = body_on_the_wire(
            undeclared_self_hosted,
            finished_turn_then_question(issuer.as_ref()),
        )
        .await;
        assert_carries_no_provider_continuity(&case, &body);
        assert_eq!(body["input"], dropped_input(), "{case}");
    }
}

// ---------------------------------------------------------------------------
// The dropped form is a valid request
// ---------------------------------------------------------------------------

/// A run that stopped inside a tool loop before issuers were recorded and
/// resumes afterwards: the next request carries the call and its output and
/// no reasoning item, on every service, and on another issuer's material
/// alike.
#[tokio::test]
async fn an_interrupted_tool_loop_resumes_in_the_dropped_form() {
    for target in Service::ALL {
        let foreign = Service::ALL
            .into_iter()
            .find(|service| *service != target)
            .map(|service| service.stored_issuer());
        for issuer in [None, foreign] {
            let case = format!("{issuer:?} -> {target:?}, inside the tool loop");
            let body = body_on_the_wire(
                |base| target.client(base),
                interrupted_tool_loop(issuer.as_ref()),
            )
            .await;
            assert_carries_no_provider_continuity(&case, &body);
            assert_eq!(body["input"], dropped_tool_loop_input(), "{case}");
        }
    }
}

/// The repository's own replay validators accept the dropped form: the
/// replay plan verifies the projection (every source block accounted for,
/// tool results adjacent to and exactly matching their tool uses), and the
/// projected transcript holds no reasoning block and no provider metadata.
#[tokio::test]
async fn the_dropped_form_passes_the_replay_validators() {
    for target in Service::ALL {
        let foreign = Service::ALL
            .into_iter()
            .find(|service| *service != target)
            .map(|service| service.stored_issuer());
        for issuer in [None, foreign] {
            for (shape, transcript) in [
                (
                    "finished turn",
                    finished_turn_then_question(issuer.as_ref()),
                ),
                ("tool loop", interrupted_tool_loop(issuer.as_ref())),
            ] {
                let case = format!("{issuer:?} -> {target:?}, {shape}");
                let client = target.client("http://127.0.0.1:9".to_string());
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
/// `include: reasoning.encrypted_content`). The same rows are pinned in
/// `meerkat-core` (`transcript_replay::continuity_issuer_tests`), where the
/// 0.9.12 serializer wrote them.
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
/// service in the dropped form, the service that produced it included: the
/// rows name no issuer.
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
    let expected = json!([
        user_item("What is the weather in Oslo?"),
        {
            "type": "function_call",
            "call_id": "call_weather_1",
            "name": "get_weather",
            "arguments": "{\"city\":\"Oslo\"}"
        },
        {"type": "function_call_output", "call_id": "call_weather_1", "output": "4 C, light rain"},
        {"type": "message", "role": "assistant", "content": "It is 4 C with light rain in Oslo."},
        user_item("And tomorrow?"),
    ]);
    for target in Service::ALL {
        let case = format!("0.9.12 transcript -> {target:?}");
        let mut messages = transcript.clone();
        messages.push(user("And tomorrow?"));
        let body = body_on_the_wire(|base| target.client(base), messages).await;
        assert_carries_no_provider_continuity(&case, &body);
        assert_eq!(body["input"], expected, "{case}");
    }
}

// ---------------------------------------------------------------------------
// Response ids
// ---------------------------------------------------------------------------

/// A stored answer whose only continuity is its response id, then a new
/// question, on a request that asks for server-side continuation.
fn continuation_request(issuer: Option<&Value>) -> LlmRequest {
    let answer = Message::BlockAssistant(BlockAssistantMessage::new(
        vec![block(json!({"block_type": "text", "data": {
            "text": "old answer",
            "meta": meta(json!({
                "provider": "open_ai_response",
                "response_id": "resp_stored"
            }), issuer)
        }}))],
        StopReason::EndTurn,
    ));
    LlmRequest::new(
        MODEL,
        vec![user("old question"), answer, user("new question")],
    )
    .with_openai_tag_merge(|tag| tag.store = Some(true))
}

/// With `store: true` a stored response id becomes `previous_response_id`.
/// The id of another issuer, or of an unknown one, is never sent: the
/// request replays the whole transcript instead, on its first attempt.
#[tokio::test]
async fn a_response_id_of_another_or_unknown_issuer_is_never_sent() {
    for target in Service::ALL {
        let foreign = Service::ALL
            .into_iter()
            .find(|service| *service != target)
            .map(|service| service.stored_issuer());
        for issuer in [None, foreign] {
            let case = format!("{issuer:?} -> {target:?}");
            let request = continuation_request(issuer.as_ref());
            let (bodies, _) = send(|base| target.client(base), &request, PLAIN_ANSWER_SSE).await;
            assert_eq!(bodies.len(), 1, "{case}: one request: {bodies:?}");
            let body = &bodies[0];
            assert!(
                body.get("previous_response_id").is_none(),
                "{case}: a response id of another issuer was sent: {body}"
            );
            assert_eq!(
                body["input"],
                json!([
                    user_item("old question"),
                    {"type": "message", "role": "assistant", "content": "old answer"},
                    user_item("new question"),
                ]),
                "{case}: the whole transcript is replayed"
            );
        }
    }
}

/// The issuer's own response id still continues server-side: the request
/// names it and sends only what followed.
#[tokio::test]
async fn a_response_id_of_the_same_issuer_still_continues() {
    for service in Service::ALL {
        let request = continuation_request(Some(&service.stored_issuer()));
        let (bodies, _) = send(|base| service.client(base), &request, PLAIN_ANSWER_SSE).await;
        assert_eq!(bodies.len(), 1, "{service:?}: one request: {bodies:?}");
        assert_eq!(
            bodies[0]["previous_response_id"], "resp_stored",
            "{service:?}"
        );
        assert_eq!(
            bodies[0]["input"],
            json!([user_item("new question")]),
            "{service:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Minting
// ---------------------------------------------------------------------------

/// A response of one reasoning item, one phased message and one function
/// call, with the item events a Responses stream carries for them.
fn response_with_every_item_kind() -> String {
    let reasoning = json!({
        "type": "reasoning", "id": "rs_new", "summary": [], "encrypted_content": "ciphertext-new"
    });
    let message = json!({
        "type": "message", "id": "msg_new", "role": "assistant", "status": "completed",
        "phase": "commentary",
        "content": [{"type": "output_text", "text": "Checking.", "annotations": []}]
    });
    let call = json!({
        "type": "function_call", "id": "fc_new", "call_id": "call_new", "name": "get_weather",
        "arguments": "{\"city\":\"Oslo\"}", "status": "completed"
    });
    let item = |kind: &str, index: u64, item: &Value| json!({"type": kind, "output_index": index, "item": item});
    let events = [
        json!({"type": "response.created", "response": {"id": "resp_new"}}),
        item("response.output_item.added", 0, &reasoning),
        item("response.output_item.done", 0, &reasoning),
        item("response.output_item.added", 1, &message),
        json!({"type": "response.output_text.delta", "item_id": "msg_new", "output_index": 1,
            "delta": "Checking."}),
        item("response.output_item.done", 1, &message),
        item("response.output_item.added", 2, &call),
        item("response.output_item.done", 2, &call),
        json!({"type": "response.completed", "response": {
            "id": "resp_new", "status": "completed",
            "output": [reasoning, message, call],
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

/// The provider metadata of every block of the committed assistant output,
/// as a transcript stores it.
fn committed_metadata(events: &[LlmEvent]) -> Vec<Value> {
    let blocks = events
        .iter()
        .find_map(|event| match event {
            LlmEvent::AssistantOutput { blocks, .. } => Some(blocks),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the response commits its output: {events:?}"));
    blocks
        .iter()
        .filter_map(meerkat_core::replay_provider_metadata)
        .map(|meta| serde_json::to_value(meta).expect("metadata serializes"))
        .collect()
}

async fn minted_metadata(build: impl FnOnce(String) -> Arc<dyn LlmClient>) -> Vec<Value> {
    let mut request = LlmRequest::new(MODEL, vec![user("What is the weather in Oslo?")]);
    request.tools = vec![weather_tool()];
    let (_, events) = send(build, &request, &response_with_every_item_kind()).await;
    let metadata = committed_metadata(&events);
    let providers: Vec<&str> = metadata
        .iter()
        .map(|meta| meta["provider"].as_str().expect("provider tag"))
        .collect();
    assert_eq!(
        providers,
        ["open_ai", "open_ai_assistant_message", "open_ai_response"],
        "reasoning, message and function call each carry their metadata"
    );
    metadata
}

/// Every client records its own service as the issuer of what it mints, on
/// the reasoning item, the message and the function call alike.
#[tokio::test]
async fn minted_metadata_names_the_issuing_service() {
    for service in Service::ALL {
        for meta in minted_metadata(|base| service.client(base)).await {
            assert_eq!(
                meta["issuer"],
                service.stored_issuer(),
                "{service:?}: {meta}"
            );
        }
    }
}

/// A self-hosted client that never declared its server records no issuer,
/// so what it mints is unknown material and is replayed nowhere.
#[tokio::test]
async fn an_undeclared_self_hosted_server_mints_no_issuer() {
    for meta in minted_metadata(undeclared_self_hosted).await {
        assert!(meta.get("issuer").is_none(), "{meta}");
    }
}

/// What a client mints, it is replayed: the committed output of one step,
/// sent back with the tool output, reaches the same service with its
/// reasoning item, and another service without it.
#[tokio::test]
async fn a_minted_step_replays_to_its_issuer_only() {
    for service in Service::ALL {
        let mut request = LlmRequest::new(MODEL, vec![user("What is the weather in Oslo?")]);
        request.tools = vec![weather_tool()];
        let (_, events) = send(
            |base| service.client(base),
            &request,
            &response_with_every_item_kind(),
        )
        .await;
        let blocks = events
            .into_iter()
            .find_map(|event| match event {
                LlmEvent::AssistantOutput { blocks, .. } => Some(blocks),
                _ => None,
            })
            .expect("the response commits its output");
        let transcript = vec![
            user("What is the weather in Oslo?"),
            Message::BlockAssistant(BlockAssistantMessage::new(blocks, StopReason::ToolUse)),
            Message::tool_results(vec![ToolResult::new(
                "call_new".to_string(),
                "4 C".to_string(),
                false,
            )]),
        ];
        let own = body_on_the_wire(|base| service.client(base), transcript.clone()).await;
        assert_eq!(
            own["input"][1],
            json!({
                "type": "reasoning", "id": "rs_new", "summary": [],
                "encrypted_content": "ciphertext-new"
            }),
            "{service:?}: its own reasoning item replays"
        );
        let other = Service::ALL
            .into_iter()
            .find(|other| *other != service)
            .expect("another service");
        let foreign = body_on_the_wire(|base| other.client(base), transcript).await;
        assert_carries_no_provider_continuity(&format!("{service:?} -> {other:?}"), &foreign);
    }
}

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
    /// Copilot `/responses` route.
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

    /// The observed failure: a session produced on the public OpenAI API is
    /// resumed on the GitHub Copilot route. Its request carries no reasoning
    /// item of the public API.
    #[tokio::test]
    async fn the_copilot_route_is_not_sent_public_api_reasoning() {
        let body = body_on_the_wire(
            route_client,
            finished_turn_then_question(Some(&Service::OpenAiApi.stored_issuer())),
        )
        .await;
        assert_carries_no_provider_continuity("public API -> Copilot route", &body);
        assert_eq!(body["input"], dropped_input());
    }

    /// The route client is the Copilot issuer: it mints Copilot material and
    /// is replayed Copilot material.
    #[tokio::test]
    async fn the_copilot_route_mints_and_replays_copilot_material() {
        for meta in minted_metadata(route_client).await {
            assert_eq!(meta["issuer"], Service::Copilot.stored_issuer(), "{meta}");
        }
        let body = body_on_the_wire(
            route_client,
            finished_turn_then_question(Some(&Service::Copilot.stored_issuer())),
        )
        .await;
        assert_eq!(body["input"], native_input());
    }
}

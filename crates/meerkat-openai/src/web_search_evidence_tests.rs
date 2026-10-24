//! A hosted web search's evidence across stateless OpenAI Responses steps.
//!
//! With `store: false` OpenAI keeps nothing of a search, so the next step
//! knows only what its replayed input holds. On the public OpenAI API wire a
//! request with the hosted search tool asks for the sources each search
//! consulted, and the next step replays the `web_search_call` with them and
//! the answer's `url_citation` annotations on its text. Every other
//! Responses wire keeps the shape it had: no sources asked for or replayed,
//! `annotations: []`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use axum::{Json, Router, extract::State, response::IntoResponse, routing::post};
use meerkat_core::AgentLlmClient;
use meerkat_core::lifecycle::run_primitive::OpaqueProviderBody;
use meerkat_llm_core::LlmClientAdapter;
use serde_json::json;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

const ANSWER: &str = "Meerkat is an agent runtime.";

/// The answer's citation of its first word (characters 0..7).
fn citation() -> Value {
    json!({
        "type": "url_citation",
        "url": "https://example.com/meerkat",
        "title": "Meerkat",
        "start_index": 0,
        "end_index": 7
    })
}

fn sources() -> Value {
    json!([
        {"type": "url", "url": "https://example.com/meerkat"},
        {"type": "url", "url": "https://example.com/runtime"}
    ])
}

fn hosted_web_search() -> ProviderTag {
    ProviderTag::OpenAi(OpenAiProviderTag {
        web_search: Some(OpaqueProviderBody::from_value(
            &json!({"type": "web_search"}),
        )),
        ..Default::default()
    })
}

/// Whether the request asks for the sources each search consulted. The wire
/// value is written out here, not taken from the client, so this pins it.
fn asks_for_sources(body: &Value) -> bool {
    body["include"].as_array().is_some_and(|include| {
        include
            .iter()
            .any(|entry| entry.as_str() == Some("web_search_call.action.sources"))
    })
}

/// The answer as one `output_text` part citing its first word.
fn one_part_answer() -> Value {
    json!([{"type": "output_text", "text": ANSWER, "annotations": [citation()]}])
}

/// The same answer as two `output_text` parts, the citation on the second.
/// Its indices count from the start of that part ("an agent runtime."), so
/// on the joined text they would mark the wrong words.
fn two_part_answer() -> Value {
    json!([
        {"type": "output_text", "text": "Meerkat is ", "annotations": []},
        {"type": "output_text", "text": "an agent runtime.", "annotations": [{
            "type": "url_citation",
            "url": "https://example.com/runtime",
            "title": "Runtime",
            "start_index": 0,
            "end_index": 16
        }]}
    ])
}

/// One searched answer as the API streams it: a search call whose action
/// carries the sources only when the request asked for them, then the answer
/// with `answer_parts` as its content.
fn searched_answer_stream(with_sources: bool, answer_parts: &Value) -> String {
    let mut action = json!({"type": "search", "query": "meerkat runtime"});
    if with_sources {
        action["sources"] = sources();
    }
    let response = json!({
        "id": "resp_search",
        "status": "completed",
        "output": [
            {"type": "web_search_call", "id": "ws_1", "status": "completed", "action": action},
            {
                "type": "message",
                "id": "msg_1",
                "role": "assistant",
                "status": "completed",
                "content": answer_parts
            }
        ],
        "usage": {"input_tokens": 12, "output_tokens": 7}
    });
    let searching = json!({
        "type": "response.web_search_call.searching",
        "output_index": 0,
        "item_id": "ws_1",
        "sequence_number": 1
    });
    let completed = json!({"type": "response.completed", "response": response});
    format!("data: {searching}\n\ndata: {completed}\n\ndata: [DONE]\n\n")
}

#[derive(Clone)]
struct SearchStub {
    seen: Arc<Mutex<Vec<Value>>>,
    answer_parts: Value,
}

/// Records each request body and answers it like the API answers a searched
/// question.
async fn searched_answer(
    State(stub): State<SearchStub>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let with_sources = asks_for_sources(&body);
    stub.seen.lock().expect("seen mutex").push(body);
    (
        [("content-type", "text/event-stream")],
        searched_answer_stream(with_sources, &stub.answer_parts),
    )
}

async fn spawn_search_stub(
    answer_parts: Value,
) -> (
    OpenAiClient,
    Arc<Mutex<Vec<Value>>>,
    tokio::task::JoinHandle<()>,
) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/v1/responses", post(searched_answer))
        .with_state(SearchStub {
            seen: seen.clone(),
            answer_parts,
        });
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test server");
    let addr = listener.local_addr().expect("local addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve test server");
    });
    let client = OpenAiClient::new_with_base_url("test-key".to_string(), format!("http://{addr}"));
    (client, seen, server)
}

fn answer_meta() -> Option<Box<ProviderMeta>> {
    Some(Box::new(ProviderMeta::OpenAiAssistantMessage {
        id: "msg_1".to_string(),
        phase: None,
        response_id: Some("resp_search".to_string()),
    }))
}

/// A message's citations block as capture records it: `text_parts` is the
/// number of `output_text` parts of the message, absent in a block recorded
/// before capture counted them.
fn citations_block(
    id: Option<&str>,
    annotations: Value,
    text_parts: Option<u64>,
) -> AssistantBlock {
    let mut content = json!({"type": "message_annotations", "annotations": annotations});
    if let Some(text_parts) = text_parts {
        content["text_parts"] = json!(text_parts);
    }
    AssistantBlock::ServerToolContent {
        id: id.map(str::to_owned),
        kind: ServerToolKind::WebSearch,
        content,
        meta: openai_response_meta(Some("resp_search")),
    }
}

fn search_call_block(action: Value) -> AssistantBlock {
    AssistantBlock::ServerToolContent {
        id: Some("ws_1".to_string()),
        kind: ServerToolKind::WebSearch,
        content: json!({"type": "web_search_call", "id": "ws_1", "status": "completed", "action": action}),
        meta: openai_response_meta(Some("resp_search")),
    }
}

/// A question, its searched answer as the public API wire captures it (the
/// search call with its sources, the answer, its citations), and a follow-up.
fn searched_history(answer_blocks: Vec<AssistantBlock>) -> Vec<Message> {
    vec![
        Message::User(UserMessage::text("What is Meerkat?".to_string())),
        Message::BlockAssistant(BlockAssistantMessage::new(
            answer_blocks,
            StopReason::EndTurn,
        )),
        Message::User(UserMessage::text("Which page says so?".to_string())),
    ]
}

fn captured_answer_blocks() -> Vec<AssistantBlock> {
    vec![
        search_call_block(
            json!({"type": "search", "query": "meerkat runtime", "sources": sources()}),
        ),
        AssistantBlock::Text {
            text: ANSWER.to_string(),
            meta: answer_meta(),
        },
        citations_block(Some("msg_1"), json!([citation()]), Some(1)),
    ]
}

/// The next step's request body for `history`, as `client` lowers it.
fn next_step_body(client: &OpenAiClient, history: &[Message]) -> Value {
    let projected = client
        .project_replay_messages(history)
        .expect("replay projection");
    client
        .build_request_body(
            &LlmRequest::new("gpt-5.5", projected).with_provider_params(hosted_web_search()),
        )
        .expect("request body")
}

fn input_item<'a>(body: &'a Value, kind: &str) -> &'a Value {
    body["input"]
        .as_array()
        .expect("input array")
        .iter()
        .find(|item| item["type"] == kind && (kind != "message" || item["role"] == "assistant"))
        .unwrap_or_else(|| panic!("no {kind} input item in {body}"))
}

fn has_input_item(body: &Value, kind: &str) -> bool {
    body["input"]
        .as_array()
        .expect("input array")
        .iter()
        .any(|item| item["type"] == kind)
}

/// A search one step ran reaches the next stateless step: every request
/// with the hosted search asks for the sources it consulted, and the next
/// step replays the `web_search_call` with them and the answer with its
/// `url_citation` annotations. (Fails-old: nothing asked for the sources, so
/// the API sent none, and the answer replayed `annotations: []`.)
#[tokio::test]
async fn a_searched_answer_reaches_the_next_stateless_step_with_its_sources_and_citations() {
    let (client, seen, server) = spawn_search_stub(one_part_answer()).await;
    let adapter = LlmClientAdapter::new(Arc::new(client), "gpt-5.5".to_string())
        .with_provider_params(Some(hosted_web_search()));
    let question = Message::User(UserMessage::text("What is Meerkat?".to_string()));
    let step = AgentLlmClient::stream_response(
        &adapter,
        std::slice::from_ref(&question),
        &[],
        1024,
        None,
        None,
    )
    .await
    .expect("the searched step");
    let history = vec![
        question,
        Message::BlockAssistant(BlockAssistantMessage::new(
            step.blocks().to_vec(),
            step.stop_reason(),
        )),
        Message::User(UserMessage::text("Which page says so?".to_string())),
    ];
    AgentLlmClient::stream_response(&adapter, &history, &[], 1024, None, None)
        .await
        .expect("the next step");
    server.abort();

    let seen = seen.lock().expect("seen mutex").clone();
    assert_eq!(seen.len(), 2, "{seen:#?}");
    assert!(
        seen.iter().all(asks_for_sources),
        "every request with the hosted search asks for its sources: {seen:#?}"
    );
    let next = &seen[1];
    assert_eq!(
        input_item(next, "web_search_call")["action"]["sources"],
        sources(),
        "{next}"
    );
    let answer = input_item(next, "message");
    assert_eq!(answer["id"], "msg_1", "{answer}");
    assert_eq!(
        answer["content"][0]["annotations"],
        json!([citation()]),
        "{answer}"
    );
    assert!(!has_input_item(next, "message_annotations"), "{next}");
}

/// Controls, unchanged: a request without the hosted search asks for no
/// sources; Chat Completions replays no search evidence at all; a stored
/// continuation (`store: true`) still sends `previous_response_id` and only
/// the history after that response.
#[test]
fn requests_without_the_hosted_search_chat_completions_and_stored_continuation_keep_their_shape() {
    let client = OpenAiClient::new("test-key".to_string());
    let plain = client
        .build_request_body(&LlmRequest::new(
            "gpt-5.5",
            vec![Message::User(UserMessage::text("hello".to_string()))],
        ))
        .expect("request body");
    assert_eq!(
        plain["include"],
        json!(["reasoning.encrypted_content"]),
        "{plain}"
    );
    let plain_non_reasoning = client
        .build_request_body(&LlmRequest::new(
            "gpt-4.1-mini",
            vec![Message::User(UserMessage::text("hello".to_string()))],
        ))
        .expect("request body");
    assert!(
        plain_non_reasoning.get("include").is_none(),
        "{plain_non_reasoning}"
    );

    let chat = project_openai_replay_messages(
        &searched_history(captured_answer_blocks()),
        OpenAiReplayProjectionMode::ChatCompletions,
    )
    .expect("chat projection");
    let Message::BlockAssistant(answer) = &chat[1] else {
        panic!("the answer replays: {chat:?}");
    };
    assert!(
        answer
            .blocks
            .iter()
            .all(|block| !matches!(block, AssistantBlock::ServerToolContent { .. })),
        "{answer:?}"
    );

    let stored = ProviderTag::OpenAi(OpenAiProviderTag {
        web_search: Some(OpaqueProviderBody::from_value(
            &json!({"type": "web_search"}),
        )),
        store: Some(true),
        ..Default::default()
    });
    let projected = client
        .project_replay_messages(&searched_history(captured_answer_blocks()))
        .expect("replay projection");
    let (body, plan) = client
        .build_request_body_with_continuation(
            &LlmRequest::new("gpt-5.5", projected).with_provider_params(stored),
        )
        .expect("continuation body");
    assert!(plan.is_some(), "{body}");
    assert_eq!(body["previous_response_id"], "resp_search", "{body}");
    let input = body["input"].as_array().expect("input array");
    assert_eq!(input.len(), 1, "only the follow-up replays: {body}");
    assert_eq!(input[0]["role"], "user", "{body}");
}

/// A stored continuation with the hosted search asks for the sources too:
/// its response is replayed in full when OpenAI rejects the continuation.
/// (Fails-old: no request asked for them.)
#[test]
fn a_stored_continuation_with_the_hosted_search_asks_for_its_sources() {
    let client = OpenAiClient::new("test-key".to_string());
    let stored = ProviderTag::OpenAi(OpenAiProviderTag {
        web_search: Some(OpaqueProviderBody::from_value(
            &json!({"type": "web_search"}),
        )),
        store: Some(true),
        ..Default::default()
    });
    let projected = client
        .project_replay_messages(&searched_history(captured_answer_blocks()))
        .expect("replay projection");
    let (body, _) = client
        .build_request_body_with_continuation(
            &LlmRequest::new("gpt-5.5", projected).with_provider_params(stored),
        )
        .expect("continuation body");
    assert_eq!(body["previous_response_id"], "resp_search", "{body}");
    assert!(asks_for_sources(&body), "{body}");
}

/// Only `url_citation` entries whose character range fits the replayed
/// text go back; any other annotation, and a range past the text, is
/// dropped, never moved. (Fails-old: nothing went back.)
#[test]
fn only_url_citations_that_fit_their_text_replay() {
    let client = OpenAiClient::new("test-key".to_string());
    let past_the_text = json!({
        "type": "url_citation",
        "url": "https://example.com/runtime",
        "title": "Runtime",
        "start_index": 8,
        "end_index": 80
    });
    let file = json!({"type": "file_citation", "file_id": "file_1", "index": 0});
    let history = searched_history(vec![
        search_call_block(
            json!({"type": "search", "query": "meerkat runtime", "sources": sources()}),
        ),
        AssistantBlock::Text {
            text: ANSWER.to_string(),
            meta: answer_meta(),
        },
        citations_block(
            Some("msg_1"),
            json!([citation(), past_the_text, file]),
            Some(1),
        ),
    ]);
    let body = next_step_body(&client, &history);
    assert_eq!(
        input_item(&body, "message")["content"][0]["annotations"],
        json!([citation()]),
        "{body}"
    );
}

/// An answer whose text came from several `output_text` parts replays no
/// citations: each part's indices count from the start of that part, and
/// capture joins the parts into one text, so a citation of the second part
/// would mark the wrong words. Run through capture, like the two-step row.
/// (Fails-old: the second part's citation replayed on the joined text,
/// misplaced.)
#[tokio::test]
async fn an_answer_from_several_text_parts_replays_no_citations() {
    let (client, seen, server) = spawn_search_stub(two_part_answer()).await;
    let adapter = LlmClientAdapter::new(Arc::new(client), "gpt-5.5".to_string())
        .with_provider_params(Some(hosted_web_search()));
    let question = Message::User(UserMessage::text("What is Meerkat?".to_string()));
    let step = AgentLlmClient::stream_response(
        &adapter,
        std::slice::from_ref(&question),
        &[],
        1024,
        None,
        None,
    )
    .await
    .expect("the searched step");
    let history = vec![
        question,
        Message::BlockAssistant(BlockAssistantMessage::new(
            step.blocks().to_vec(),
            step.stop_reason(),
        )),
        Message::User(UserMessage::text("Which page says so?".to_string())),
    ];
    AgentLlmClient::stream_response(&adapter, &history, &[], 1024, None, None)
        .await
        .expect("the next step");
    server.abort();

    let seen = seen.lock().expect("seen mutex").clone();
    let next = &seen[1];
    let answer = input_item(next, "message");
    assert_eq!(
        answer["content"][0]["text"], "Meerkat is an agent runtime.",
        "{answer}"
    );
    assert_eq!(answer["content"][0]["annotations"], json!([]), "{answer}");
    assert!(!has_input_item(next, "message_annotations"), "{next}");
}

/// Two citations blocks for one message replay none either: which one
/// belongs to the replayed text cannot be told.
#[test]
fn two_citations_blocks_for_one_message_replay_none() {
    let client = OpenAiClient::new("test-key".to_string());
    let history = searched_history(vec![
        AssistantBlock::Text {
            text: ANSWER.to_string(),
            meta: answer_meta(),
        },
        citations_block(Some("msg_1"), json!([citation()]), Some(1)),
        citations_block(Some("msg_1"), json!([citation()]), Some(1)),
    ]);
    let body = next_step_body(&client, &history);
    assert_eq!(
        input_item(&body, "message")["content"][0]["annotations"],
        json!([]),
        "{body}"
    );
    assert!(!has_input_item(&body, "message_annotations"), "{body}");
}

/// A search recorded before this change replays as it did: the call
/// without sources verbatim, and the answer with `annotations: []`, because
/// its citations block does not say how many text parts the answer had (and
/// the oldest blocks carry no message id).
#[test]
fn a_search_recorded_before_this_change_replays_as_it_did() {
    let client = OpenAiClient::new("test-key".to_string());
    let call = json!({"type": "search", "query": "meerkat runtime"});
    let history = searched_history(vec![
        search_call_block(call.clone()),
        AssistantBlock::Text {
            text: ANSWER.to_string(),
            meta: answer_meta(),
        },
        citations_block(Some("msg_1"), json!([citation()]), None),
    ]);
    let body = next_step_body(&client, &history);
    assert_eq!(
        input_item(&body, "web_search_call")["action"],
        call,
        "{body}"
    );
    assert_eq!(
        input_item(&body, "message")["content"][0]["annotations"],
        json!([]),
        "{body}"
    );
    assert!(!has_input_item(&body, "message_annotations"), "{body}");

    let oldest = searched_history(vec![
        AssistantBlock::Text {
            text: ANSWER.to_string(),
            meta: answer_meta(),
        },
        citations_block(None, json!([citation()]), None),
    ]);
    let body = next_step_body(&client, &oldest);
    assert_eq!(
        input_item(&body, "message")["content"][0]["annotations"],
        json!([]),
        "{body}"
    );
}

/// The ChatGPT backend, Azure OpenAI and Copilot wires keep the search shape
/// they had, the same history included: no sources asked for, the call
/// replayed without `action.sources`, the answer with `annotations: []`.
/// Only the public OpenAI API wire, which a live row proves, carries the
/// evidence. (Fails-old on the sources only: the call replayed verbatim.)
#[test]
fn the_chatgpt_azure_and_copilot_wires_keep_the_search_shape_they_had() {
    let wires = [
        (
            "ChatGPT backend",
            OpenAiClient::new("test-key".to_string()).with_chatgpt_backend_wire(),
        ),
        (
            "Azure OpenAI",
            OpenAiClient::new_with_base_url(
                "test-key".to_string(),
                "https://example.openai.azure.com/openai".to_string(),
            )
            .with_azure_openai_wire(AzureOpenAiWireConfig::default()),
        ),
        (
            "Copilot",
            OpenAiClient::new("test-key".to_string()).with_copilot_backend_wire(),
        ),
    ];
    let history = searched_history(captured_answer_blocks());
    for (wire, client) in wires {
        let body = next_step_body(&client, &history);
        assert!(!asks_for_sources(&body), "{wire}: {body}");
        let call = input_item(&body, "web_search_call");
        assert_eq!(
            call["action"],
            json!({"type": "search", "query": "meerkat runtime"}),
            "{wire}: {body}"
        );
        assert_eq!(
            input_item(&body, "message")["content"][0]["annotations"],
            json!([]),
            "{wire}: {body}"
        );
        assert!(
            !has_input_item(&body, "message_annotations"),
            "{wire}: {body}"
        );
    }
    let public = next_step_body(&OpenAiClient::new("test-key".to_string()), &history);
    assert!(asks_for_sources(&public), "{public}");
    assert_eq!(
        input_item(&public, "web_search_call")["action"]["sources"],
        sources(),
        "{public}"
    );
}

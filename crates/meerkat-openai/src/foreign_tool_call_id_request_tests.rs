//! A stored tool-use id that another provider issued, replayed to an OpenAI
//! wire. Request construction only, on synthetic transcripts: no row here
//! sends anything.
//!
//! The Responses lowering writes a stored id it can carry as stored, and
//! any other as a short id derived from it, in the request only. The Chat
//! Completions lowering writes every stored id as stored.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use meerkat_core::{
    AssistantBlock, BlockAssistantMessage, Message, StopReason, ToolResult, UserMessage,
};
use meerkat_llm_core::{LlmClient, LlmRequest, PreparedLlmRequest};
use serde_json::{Value, json};

use crate::OpenAiClient;
use crate::client_compatible::{OpenAiCompatibleClient, OpenAiCompatibleMode};

/// The longest `call_id`, in bytes, a Responses request may carry.
const RESPONSES_CALL_ID_BOUND: usize = 64;

/// A synthetic id of exactly `len` bytes, as a provider with no length rule
/// of its own could issue one. `seed` tells two ids of one length apart.
fn foreign_id(seed: &str, len: usize) -> String {
    let mut id = format!("synthetic-foreign-id-{seed}-");
    while id.len() < len {
        id.push_str("0123456789abcdef");
    }
    id.truncate(len);
    id
}

fn tool_use(id: &str) -> AssistantBlock {
    AssistantBlock::ToolUse {
        id: id.to_string(),
        name: "lookup".to_string(),
        args: serde_json::value::RawValue::from_string("{}".to_string()).unwrap(),
        meta: None,
    }
}

fn tool_result(id: &str) -> ToolResult {
    ToolResult::new(id.to_string(), "lookup result".to_string(), false)
}

/// One tool round: the assistant's call under `id`, then its result.
fn round(id: &str) -> [Message; 2] {
    [
        Message::BlockAssistant(BlockAssistantMessage::new(
            vec![tool_use(id)],
            StopReason::ToolUse,
        )),
        Message::tool_results(vec![tool_result(id)]),
    ]
}

/// A transcript with one tool round per id, between two user rows.
fn transcript(ids: &[&str]) -> Vec<Message> {
    let mut messages = vec![Message::User(UserMessage::text("look it up"))];
    for id in ids {
        messages.extend(round(id));
    }
    messages.push(Message::User(UserMessage::text("and again")));
    messages
}

fn responses_body(messages: Vec<Message>) -> Value {
    OpenAiClient::new("test-key".to_string())
        .build_request_body(&LlmRequest::new("gpt-5.2", messages))
        .expect("the Responses body")
}

fn chat_body(messages: Vec<Message>) -> Value {
    OpenAiCompatibleClient::new(
        OpenAiCompatibleMode::ChatCompletions,
        "qwen3".to_string(),
        "http://localhost:8000/v1".to_string(),
        None,
        true,
        false,
        false,
    )
    .build_chat_completions_body(&LlmRequest::new("qwen3", messages))
    .expect("the Chat Completions body")
}

/// Every `(type, call_id)` of the body's function calls and their outputs,
/// in input order.
fn responses_call_ids(body: &Value) -> Vec<(String, String)> {
    body["input"]
        .as_array()
        .expect("the Responses input array")
        .iter()
        .filter_map(|item| {
            let kind = item["type"].as_str()?;
            matches!(kind, "function_call" | "function_call_output").then(|| {
                (
                    kind.to_string(),
                    item["call_id"]
                        .as_str()
                        .expect("a string call_id")
                        .to_string(),
                )
            })
        })
        .collect()
}

/// The `call_id` of each tool round of `messages`, in order, after checking
/// what every round must keep: a call item followed by its output item, one
/// `call_id` for the two, and that `call_id` within the bound.
fn round_call_ids(messages: Vec<Message>) -> Vec<String> {
    let ids = responses_call_ids(&responses_body(messages));
    assert_eq!(ids.len() % 2, 0, "a call item and an output item per round");
    ids.chunks(2)
        .map(|pair| {
            let [(call_kind, call_id), (output_kind, output_id)] = pair else {
                panic!("a call item and an output item per round");
            };
            assert_eq!(call_kind, "function_call");
            assert_eq!(output_kind, "function_call_output");
            assert_eq!(call_id, output_id, "the output names its call");
            assert!(
                call_id.len() <= RESPONSES_CALL_ID_BOUND,
                "a round carries a call_id of {} bytes; the request may carry at most \
                 {RESPONSES_CALL_ID_BOUND}",
                call_id.len(),
            );
            call_id.clone()
        })
        .collect()
}

#[test]
fn responses_request_carries_a_long_stored_tool_use_id_within_the_call_id_bound() {
    let stored = foreign_id("a", 7000);
    let ids = responses_call_ids(&responses_body(transcript(&[&stored])));
    assert_eq!(
        ids.iter()
            .map(|(kind, _)| kind.as_str())
            .collect::<Vec<_>>(),
        ["function_call", "function_call_output"],
    );
    for (kind, call_id) in &ids {
        assert!(
            call_id.len() <= RESPONSES_CALL_ID_BOUND,
            "the {kind} item carries a call_id of {} bytes for a stored id of {} bytes; \
             the request may carry at most {RESPONSES_CALL_ID_BOUND}",
            call_id.len(),
            stored.len(),
        );
    }
    assert_eq!(ids[0].1, ids[1].1, "the output names its call");
}

#[test]
fn responses_request_keeps_two_long_stored_ids_apart() {
    // The two ids differ only after the first 64 bytes, and only in one
    // byte, so no prefix of them tells them apart.
    let first = format!("{}-1", foreign_id("same", 6998));
    let second = format!("{}-2", foreign_id("same", 6998));
    assert_eq!(first[..6999], second[..6999]);
    let ids = round_call_ids(transcript(&[&first, &second]));
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1], "two stored ids, two call_ids");
}

/// The bound is the last length written as stored: one byte more is
/// derived. Bytes are counted, so an id of few characters and many bytes is
/// derived too.
#[test]
fn responses_request_derives_a_call_id_from_the_first_byte_over_the_bound() {
    let at_bound = foreign_id("edge", RESPONSES_CALL_ID_BOUND);
    let over_bound = foreign_id("edge", RESPONSES_CALL_ID_BOUND + 1);
    let wide = "\u{00e9}".repeat(40);
    assert_eq!(wide.chars().count(), 40);
    assert_eq!(wide.len(), 80);
    let ids = round_call_ids(transcript(&[&at_bound, &over_bound, &wide]));
    assert_eq!(ids[0], at_bound, "an id at the bound is written as stored");
    assert_ne!(ids[1], over_bound);
    assert_ne!(ids[2], wide);
}

/// A history whose ids the request can carry lowers to the bytes it lowered
/// to before any id was derived: this row holds on both sides of that
/// change. The ids are an OpenAI-shaped one, an Anthropic-shaped one, one
/// exactly at the bound, and one a provider reuses in every round.
#[test]
fn responses_request_for_ids_within_the_bound_is_byte_equal_to_the_stored_ids_lowering() {
    let at_bound = foreign_id("edge", RESPONSES_CALL_ID_BOUND);
    let stored = [
        "call_1",
        "toolu_01synthetic",
        at_bound.as_str(),
        "fc_0",
        "fc_0",
    ];
    let body = responses_body(transcript(&stored));

    let mut expected = vec![json!({"type": "message", "role": "user", "content": "look it up"})];
    for id in stored {
        expected.push(json!({
            "type": "function_call",
            "call_id": id,
            "name": "lookup",
            "arguments": "{}"
        }));
        expected.push(json!({
            "type": "function_call_output",
            "call_id": id,
            "output": "lookup result"
        }));
    }
    expected.push(json!({"type": "message", "role": "user", "content": "and again"}));

    assert_eq!(
        serde_json::to_string(&body["input"]).unwrap(),
        serde_json::to_string(&Value::Array(expected)).unwrap(),
    );
}

/// Some providers reuse an id in every round. Each occurrence of a long one
/// gets the same `call_id`, so every output still follows a call of its
/// own id, as it does for a short one.
#[test]
fn responses_request_gives_a_repeated_long_stored_id_one_call_id() {
    let stored = foreign_id("repeat", 3000);
    let ids = round_call_ids(transcript(&[&stored, &stored, &stored]));
    assert_eq!(ids.len(), 3);
    assert!(ids.iter().all(|id| *id == ids[0]));
}

/// The `call_id` depends on the stored id alone: another request, another
/// position, or a request that holds only the result (as a continuation's
/// suffix does) gets the same one.
#[test]
fn responses_request_derives_the_same_call_id_in_every_request() {
    let stored = foreign_id("stable", 500);
    let other = foreign_id("other", 500);
    let alone = round_call_ids(transcript(&[&stored]));
    let later = round_call_ids(transcript(&["call_1", &other, &stored]));
    assert_eq!(alone[0], later[2]);

    let result_only = responses_call_ids(&responses_body(vec![
        Message::tool_results(vec![tool_result(&stored)]),
        Message::User(UserMessage::text("and again")),
    ]));
    assert_eq!(
        result_only,
        [("function_call_output".to_string(), alone[0].clone())]
    );
}

/// A stored id may be, byte for byte, the `call_id` this lowering derives
/// for another stored id. The two rounds still get two `call_id`s: an id of
/// the derived shape is never written as stored.
#[test]
fn responses_request_keeps_a_stored_id_shaped_like_a_derived_one_apart() {
    let long = foreign_id("a", 7000);
    let derived = round_call_ids(transcript(&[&long])).remove(0);
    assert_ne!(derived, long);

    let lookalike = derived.clone();
    let ids = round_call_ids(transcript(&[&long, &lookalike]));
    assert_eq!(ids[0], derived);
    assert_ne!(
        ids[1], derived,
        "the stored id that equals a derived call_id is not written as stored"
    );

    // In the other order, and alone, it lowers the same way.
    let reversed = round_call_ids(transcript(&[&lookalike, &long]));
    assert_eq!(reversed, [ids[1].clone(), ids[0].clone()]);
    assert_eq!(round_call_ids(transcript(&[&lookalike])), [ids[1].clone()]);
}

/// The request a compaction sends: the typed rows of the history, tool
/// rounds included, then the summary prompt as the last user row, with no
/// tools. It is projected and lowered like an ordinary turn, and what the
/// client measures for it is the body with the derived `call_id`.
#[test]
fn responses_summary_request_carries_the_long_stored_id_within_the_bound() {
    let stored = foreign_id("summary", 7000);
    let mut source = transcript(&[&stored]);
    source.push(Message::User(UserMessage::text(
        "Summarize the conversation so far.",
    )));

    let client = OpenAiClient::new("test-key".to_string());
    let projection = client
        .project_replay_request(&source)
        .expect("the replay projection");
    let prepared =
        PreparedLlmRequest::from_projection(LlmRequest::new("gpt-5.2", Vec::new()), projection);
    assert!(prepared.request().tools.is_empty());
    assert_eq!(
        prepared.request().messages,
        source,
        "the projection keeps the stored ids"
    );

    let body = client
        .build_request_body(prepared.request())
        .expect("the Responses body");
    let ids = responses_call_ids(&body);
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0].1, ids[1].1, "the output names its call");
    assert!(ids[0].1.len() <= RESPONSES_CALL_ID_BOUND);
    assert_eq!(
        ids[0].1,
        round_call_ids(transcript(&[&stored]))[0],
        "the summary request and an ordinary turn carry one call_id for the stored id"
    );

    let pressure = client
        .prepared_request_pressure(&prepared)
        .expect("the request pressure")
        .expect("the client measures its request");
    assert_eq!(
        pressure.encoded_bytes,
        serde_json::to_vec(&body).unwrap().len() as u64,
        "the measured request is the lowered one"
    );
}

/// Lowering reads the history and leaves it as it was: the stored ids are
/// still the stored ids, byte for byte.
#[test]
fn responses_lowering_leaves_the_stored_history_unchanged() {
    let stored = foreign_id("a", 7000);
    let history = transcript(&[&stored, "call_1"]);
    let before = serde_json::to_vec(&history).unwrap();

    let request = LlmRequest::new("gpt-5.2", history.clone());
    let client = OpenAiClient::new("test-key".to_string());
    let first = client.build_request_body(&request).expect("the body");
    let second = client.build_request_body(&request).expect("the body");
    assert_eq!(
        serde_json::to_vec(&first).unwrap(),
        serde_json::to_vec(&second).unwrap(),
        "lowering the same history twice gives the same bytes"
    );

    assert_eq!(request.messages, history);
    assert_eq!(serde_json::to_vec(&request.messages).unwrap(), before);
    assert!(
        String::from_utf8(before).unwrap().contains(&stored),
        "the history holds the stored id"
    );
    assert!(!first.to_string().contains(&stored), "the request does not");
}

/// What the Chat Completions lowering writes for the same transcript: the
/// stored id, as stored. This row records the emitted request and nothing
/// else. No bound for this wire is known here, and the row establishes
/// nothing about how any Chat Completions service answers such a request.
#[test]
fn chat_completions_request_carries_the_stored_tool_use_id_as_stored() {
    let stored = foreign_id("a", 7000);
    let body = chat_body(transcript(&[&stored]));
    let messages = body["messages"].as_array().expect("the messages array");
    let call = messages
        .iter()
        .find(|message| message["role"] == "assistant")
        .expect("the assistant message");
    let result = messages
        .iter()
        .find(|message| message["role"] == "tool")
        .expect("the tool message");
    assert_eq!(call["tool_calls"][0]["id"], stored.as_str());
    assert_eq!(result["tool_call_id"], stored.as_str());
}

//! The turn-boundary cache anchor on lowered Anthropic Messages requests.
//!
//! A `fork_off` child is cut at the forker's previous turn end. Its first
//! request is the forker's transcript up to that boundary followed by its own
//! task, so it can only read a provider cache entry written at the boundary.
//! The request-wide automatic breakpoint writes only at the end of each
//! request, so without an explicit anchor a forking turn that starts on a
//! cold cache leaves no such entry (issue #1235).
//!
//! These tests lower the forker's and the child's requests through the same
//! replay projection and body builder the client streams, then:
//!
//! - pin that every request of the forking turn and the child's first request
//!   carry the anchor breakpoint on the previous run's last output block, and
//!   that the lowered prefix through that block, breakpoints included, is
//!   byte-identical between them;
//! - replay the requests against a model of Anthropic's documented cache
//!   (entries are written only at breakpoints; a lookup walks back up to 20
//!   blocks from each breakpoint) for a cold-start and a warm-start forker
//!   turn, and assert the child reads the whole shared prefix.
//!
//! The activation rows at the end hold the same cache argument for an
//! operator instruction appended after history, with a plain System row as
//! the stand-in entry, from turn k through turn k+2, whose request has moved
//! the recent breakpoints of `system_and_conversation` past the entry. Under
//! each cache policy every request is a byte prefix of the next apart from
//! breakpoint markers. The cache model serves each request all of the one
//! before under `automatic` and `system_and_conversation`, and the system
//! prefix under `system_prefix`. Turn k+1 renders the entry after its own
//! prompt, and its breakpoint claims cover only rendered prefixes that are
//! transcript prefixes.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::HashSet;
use std::sync::Arc;

use meerkat_core::lifecycle::RunId;
use meerkat_core::lifecycle::run_primitive::AnthropicCacheControlPolicy;
use meerkat_core::{
    AssistantBlock, BlockAssistantMessage, CacheBreakpointBoundary, Message, ProviderMeta,
    StopReason, SystemMessage, ToolDef, ToolResult, TranscriptMessageIdentity, UserMessage,
};
use meerkat_llm_core::{LlmClient, LlmRequest};
use serde_json::{Value, json};

use crate::AnthropicClient;

const MODEL: &str = "claude-sonnet-4-6";
/// Anthropic's documented lookback window from each breakpoint.
const LOOKBACK_BLOCKS: usize = 20;

fn tools() -> Vec<Arc<ToolDef>> {
    ["calendar_lookup", "fork_off"]
        .into_iter()
        .map(|name| {
            Arc::new(ToolDef {
                audience: Default::default(),
                name: name.into(),
                description: format!("{name} tool"),
                input_schema: json!({"type": "object", "properties": {}}),
                provenance: None,
            })
        })
        .collect()
}

fn stamped(run: &RunId, blocks: Vec<AssistantBlock>, stop_reason: StopReason) -> Message {
    let mut message = BlockAssistantMessage::new(blocks, stop_reason);
    message.identity = TranscriptMessageIdentity::default().with_run_id(run.clone());
    Message::BlockAssistant(message)
}

fn tool_call(run: &RunId, id: &str, name: &str) -> [Message; 2] {
    [
        stamped(
            run,
            vec![AssistantBlock::ToolUse {
                id: id.to_string(),
                name: name.to_string(),
                args: serde_json::value::RawValue::from_string("{}".to_string()).unwrap(),
                meta: None,
            }],
            StopReason::ToolUse,
        ),
        Message::ToolResults {
            results: vec![ToolResult::new(id.to_string(), format!("{id} ok"), false)],
            created_at: meerkat_core::types::message_timestamp_now(),
        },
    ]
}

fn text(text: &str) -> AssistantBlock {
    AssistantBlock::Text {
        text: text.to_string(),
        meta: None,
    }
}

/// The requests of one fork: the forker's previous run, its forking run, and
/// the child's first run.
struct ForkRequests {
    /// The forker's committed transcript at the previous turn end: the fork
    /// boundary. Its last message is the previous run's final answer.
    prefix: Vec<Message>,
    /// The previous run's last request (it ends at the tool results; the
    /// final answer is its response).
    previous_last: Vec<Message>,
    /// The forking run's first request.
    forker_first: Vec<Message>,
    /// The forking run's tool round after `fork_off` returned.
    forker_round: Vec<Message>,
    /// The child's first request.
    child_first: Vec<Message>,
}

fn fork_requests() -> ForkRequests {
    let previous = RunId::new();
    let forker = RunId::new();
    let mut previous_last = vec![
        Message::System(SystemMessage::new("You are the calendar domain agent.")),
        Message::User(UserMessage::text("note tomorrow's agenda")),
    ];
    previous_last.extend(tool_call(&previous, "call-lookup", "calendar_lookup"));
    let mut prefix = previous_last.clone();
    prefix.push(stamped(
        &previous,
        vec![text("Tomorrow: standup at 9, dentist at 14.")],
        StopReason::EndTurn,
    ));
    let mut forker_first = prefix.clone();
    forker_first.push(Message::User(UserMessage::text(
        "fork a child for tomorrow's agenda",
    )));
    let mut forker_round = forker_first.clone();
    forker_round.extend(tool_call(&forker, "call-fork", "fork_off"));
    let mut child_first = prefix.clone();
    child_first.push(Message::User(UserMessage::text(
        "list tomorrow's events (forked task)",
    )));
    ForkRequests {
        prefix,
        previous_last,
        forker_first,
        forker_round,
        child_first,
    }
}

/// Lower `messages` exactly as `stream` does: replay projection, then body.
fn lower(messages: &[Message], policy: Option<AnthropicCacheControlPolicy>) -> Value {
    lower_for(MODEL, messages, policy)
}

/// `lower` for `model`.
fn lower_for(
    model: &str,
    messages: &[Message],
    policy: Option<AnthropicCacheControlPolicy>,
) -> Value {
    let client = AnthropicClient::new("test-key".to_string()).unwrap();
    let mut request = LlmRequest::new(model, messages.to_vec()).with_tools(tools());
    if let Some(policy) = policy {
        request = request.with_anthropic_tag_merge(|tag| tag.cache_control = Some(policy));
    }
    request.messages = client.project_replay_messages(&request.messages).unwrap();
    client.build_request_body(&request).unwrap()
}

/// Lowered `messages` index of the previous run's final answer: every
/// transcript message but the leading System row lowers to one entry.
fn anchor_index(fork: &ForkRequests) -> usize {
    fork.prefix.len() - 2
}

fn has_breakpoint(message: &Value) -> bool {
    message["content"].as_array().is_some_and(|blocks| {
        blocks
            .iter()
            .any(|block| block.get("cache_control").is_some())
    })
}

fn breakpoint_messages(body: &Value) -> Vec<usize> {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .filter_map(|(index, message)| has_breakpoint(message).then_some(index))
        .collect()
}

/// The lowered cacheable prefix through lowered message `through`, as exact
/// bytes: everything the provider hashes ahead of that message's end.
fn prefix_bytes(body: &Value, through: usize) -> String {
    json!({
        "model": body["model"],
        "tools": body["tools"],
        "cache_control": body.get("cache_control"),
        "system": body["system"],
        "messages": body["messages"].as_array().unwrap()[..=through],
    })
    .to_string()
}

// ---------------------------------------------------------------------------
// A model of Anthropic's documented prompt cache
// ---------------------------------------------------------------------------

/// One request flattened into cacheable blocks in render order (tools, then
/// system, then each message's content blocks), with breakpoint positions.
struct CacheView {
    /// `blocks[i]` identifies the prefix through block `i`: the provider keys
    /// entries on content, so breakpoint markers are not part of the key.
    prefixes: Vec<String>,
    breakpoints: Vec<usize>,
}

fn strip_cache_control(value: &Value) -> Value {
    let mut value = value.clone();
    if let Some(object) = value.as_object_mut() {
        object.remove("cache_control");
    }
    value
}

fn cache_view(body: &Value) -> CacheView {
    let mut blocks: Vec<(String, bool)> = Vec::new();
    for tool in body["tools"].as_array().into_iter().flatten() {
        blocks.push((tool.to_string(), tool.get("cache_control").is_some()));
    }
    match &body["system"] {
        Value::Array(system) => {
            for block in system {
                blocks.push((
                    strip_cache_control(block).to_string(),
                    block.get("cache_control").is_some(),
                ));
            }
        }
        Value::Null => {}
        other => blocks.push((other.to_string(), false)),
    }
    for message in body["messages"].as_array().unwrap() {
        let role = message["role"].to_string();
        match &message["content"] {
            Value::Array(content) => {
                for block in content {
                    blocks.push((
                        format!("{role}:{}", strip_cache_control(block)),
                        block.get("cache_control").is_some(),
                    ));
                }
            }
            other => blocks.push((format!("{role}:{other}"), false)),
        }
    }
    let mut breakpoints: Vec<usize> = blocks
        .iter()
        .enumerate()
        .filter_map(|(index, (_, marked))| marked.then_some(index))
        .collect();
    if body.get("cache_control").is_some() {
        // The request-wide automatic breakpoint sits on the last block.
        breakpoints.push(blocks.len() - 1);
    }
    breakpoints.sort_unstable();
    breakpoints.dedup();
    let mut prefixes = Vec::with_capacity(blocks.len());
    let mut running = String::new();
    for (block, _) in blocks {
        running.push_str(&block);
        running.push('\u{1e}');
        prefixes.push(running.clone());
    }
    CacheView {
        prefixes,
        breakpoints,
    }
}

#[derive(Default)]
struct ProviderCache {
    entries: HashSet<String>,
}

impl ProviderCache {
    /// Serve one request: return the number of prefix blocks read from cache,
    /// then write (or refresh) an entry at every breakpoint.
    fn serve(&mut self, body: &Value) -> usize {
        let view = cache_view(body);
        let read = view
            .breakpoints
            .iter()
            .filter_map(|&breakpoint| {
                (breakpoint.saturating_sub(LOOKBACK_BLOCKS - 1)..=breakpoint)
                    .rev()
                    .find(|&index| self.entries.contains(&view.prefixes[index]))
                    .map(|index| index + 1)
            })
            .max()
            .unwrap_or(0);
        for &breakpoint in &view.breakpoints {
            self.entries.insert(view.prefixes[breakpoint].clone());
        }
        read
    }
}

/// Blocks the child shares with the forker: tools, system, and the whole
/// inherited transcript.
fn shared_prefix_blocks(child: &Value) -> usize {
    let view = cache_view(child);
    // Everything but the child's own task message (one text block).
    view.prefixes.len() - 1
}

/// The pre-fix lowering: the same bodies without the explicit anchor.
fn without_anchor(body: &Value, anchor: usize) -> Value {
    let mut body = body.clone();
    for block in body["messages"][anchor]["content"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
    {
        if let Some(object) = block.as_object_mut() {
            object.remove("cache_control");
        }
    }
    body
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn forker_and_child_carry_the_same_anchor_and_prefix_bytes() {
    let fork = fork_requests();
    let anchor = anchor_index(&fork);
    let forker_first = lower(&fork.forker_first, None);
    let forker_round = lower(&fork.forker_round, None);
    let child_first = lower(&fork.child_first, None);

    for (name, body) in [
        ("forker first", &forker_first),
        ("forker tool round", &forker_round),
        ("child first", &child_first),
    ] {
        assert_eq!(
            body["cache_control"],
            json!({"type": "ephemeral"}),
            "{name}: the automatic breakpoint stays request-wide"
        );
        assert_eq!(
            breakpoint_messages(body),
            vec![anchor],
            "{name}: one explicit breakpoint, on the previous run's final answer"
        );
        assert_eq!(
            body["messages"][anchor],
            json!({
                "role": "assistant",
                "content": [{
                    "type": "text",
                    "text": "Tomorrow: standup at 9, dentist at 14.",
                    "cache_control": {"type": "ephemeral"}
                }]
            }),
            "{name}: the anchor block"
        );
    }
    let child_prefix = prefix_bytes(&child_first, anchor);
    assert_eq!(prefix_bytes(&forker_first, anchor), child_prefix);
    assert_eq!(prefix_bytes(&forker_round, anchor), child_prefix);
    // The child's lowered request is exactly the forker's first request with
    // its own task in place of the forker's in-flight prompt.
    let child_messages = child_first["messages"].as_array().unwrap();
    let forker_messages = forker_first["messages"].as_array().unwrap();
    assert_eq!(child_messages.len(), forker_messages.len());
    assert_eq!(
        child_messages[..child_messages.len() - 1],
        forker_messages[..forker_messages.len() - 1]
    );
}

#[test]
fn cold_start_forker_turn_leaves_an_entry_the_child_reads() {
    let fork = fork_requests();
    let anchor = anchor_index(&fork);
    let forker_first = lower(&fork.forker_first, None);
    let forker_round = lower(&fork.forker_round, None);
    let child_first = lower(&fork.child_first, None);
    let shared = shared_prefix_blocks(&child_first);

    // The forking run starts cold: the previous run's entries expired.
    let mut cache = ProviderCache::default();
    assert_eq!(cache.serve(&forker_first), 0, "the forker starts cold");
    cache.serve(&forker_round);
    assert_eq!(
        cache.serve(&child_first),
        shared,
        "the child reads the whole inherited prefix"
    );

    // Control: without the anchor the cold forker's entries all sit after its
    // own prompt, and the child re-bills the transcript.
    let mut cache = ProviderCache::default();
    cache.serve(&without_anchor(&forker_first, anchor));
    cache.serve(&without_anchor(&forker_round, anchor));
    assert_eq!(cache.serve(&without_anchor(&child_first, anchor)), 0);
}

#[test]
fn warm_start_forker_turn_refreshes_the_entry_the_child_reads() {
    let fork = fork_requests();
    let anchor = anchor_index(&fork);
    let previous_last = lower(&fork.previous_last, None);
    let forker_first = lower(&fork.forker_first, None);
    let forker_round = lower(&fork.forker_round, None);
    let child_first = lower(&fork.child_first, None);
    let shared = shared_prefix_blocks(&child_first);

    let mut cache = ProviderCache::default();
    cache.serve(&previous_last);
    let previous_end = cache_view(&previous_last).prefixes.len();
    assert_eq!(
        cache.serve(&forker_first),
        previous_end,
        "the forker's first request reads the previous run's tail entry"
    );
    cache.serve(&forker_round);
    assert_eq!(cache.serve(&child_first), shared);

    // Control: without the anchor the child reaches only the previous run's
    // last request, which ends before the final answer.
    let mut cache = ProviderCache::default();
    cache.serve(&previous_last);
    cache.serve(&without_anchor(&forker_first, anchor));
    cache.serve(&without_anchor(&forker_round, anchor));
    assert_eq!(
        cache.serve(&without_anchor(&child_first, anchor)),
        previous_end
    );
    assert!(previous_end < shared);
}

#[test]
fn system_and_conversation_keeps_the_anchor_within_four_breakpoints() {
    let fork = fork_requests();
    let anchor = anchor_index(&fork);
    let policy = Some(AnthropicCacheControlPolicy::SystemAndConversation);
    let mut long_round = fork.forker_round.clone();
    long_round.extend(tool_call(&RunId::new(), "call-late", "calendar_lookup"));
    // A second tool round of the forking run: stamp it with that run's id.
    let forker_run = match &fork.forker_round[fork.forker_round.len() - 2] {
        Message::BlockAssistant(message) => message.identity.run_id.clone().unwrap(),
        other => panic!("expected the fork_off call, got {other:?}"),
    };
    if let Message::BlockAssistant(message) = &mut long_round[fork.forker_round.len()] {
        message.identity = TranscriptMessageIdentity::default().with_run_id(forker_run);
    }

    let body = lower(&long_round, policy);
    assert!(body.get("cache_control").is_none());
    assert!(
        body["system"][0].get("cache_control").is_some(),
        "the system prefix breakpoint"
    );
    let messages = body["messages"].as_array().unwrap().len();
    assert_eq!(
        breakpoint_messages(&body),
        vec![anchor, messages - 2, messages - 1],
        "system + anchor + the two most recent boundaries"
    );

    // The recent-boundary markers are relative to each request's end, so the
    // child's fall inside the shared prefix; the anchor and the content the
    // provider keys on are the same.
    let child = lower(&fork.child_first, policy);
    assert!(breakpoint_messages(&child).contains(&anchor));
    assert_eq!(
        without_markers(&serde_json::from_str(&prefix_bytes(&child, anchor)).unwrap()),
        without_markers(&serde_json::from_str(&prefix_bytes(&body, anchor)).unwrap())
    );
}

fn without_markers(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .filter(|(key, _)| key.as_str() != "cache_control")
                .map(|(key, value)| (key.clone(), without_markers(value)))
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(without_markers).collect()),
        other => other.clone(),
    }
}

#[test]
fn the_anchor_skips_thinking_blocks() {
    let previous = RunId::new();
    let messages = vec![
        Message::System(SystemMessage::new("system")),
        Message::User(UserMessage::text("first")),
        stamped(
            &previous,
            vec![
                text("answer"),
                AssistantBlock::Reasoning {
                    text: "trailing thought".to_string(),
                    meta: Some(Box::new(ProviderMeta::Anthropic {
                        signature: "sig".to_string(),
                    })),
                },
            ],
            StopReason::EndTurn,
        ),
        Message::User(UserMessage::text("second")),
    ];
    let body = lower(&messages, None);
    let content = body["messages"][1]["content"].as_array().unwrap();
    assert_eq!(content[0]["cache_control"], json!({"type": "ephemeral"}));
    assert!(content[1].get("cache_control").is_none());
}

#[test]
fn a_first_run_request_authors_no_anchor() {
    let body = lower(
        &[
            Message::System(SystemMessage::new("system")),
            Message::User(UserMessage::text("hello")),
        ],
        None,
    );
    assert!(breakpoint_messages(&body).is_empty());
    assert_eq!(body["cache_control"], json!({"type": "ephemeral"}));
}

#[test]
fn system_and_conversation_skips_a_trailing_thinking_block_within_four_slots() {
    let fork = fork_requests();
    let anchor = anchor_index(&fork);
    let forker_run = match &fork.forker_round[fork.forker_round.len() - 2] {
        Message::BlockAssistant(message) => message.identity.run_id.clone().unwrap(),
        other => panic!("expected the fork_off call, got {other:?}"),
    };
    let mut messages = fork.forker_round;
    // A recent tool round whose assistant message ends in a thinking block.
    messages.push(stamped(
        &forker_run,
        vec![
            AssistantBlock::ToolUse {
                id: "call-late".to_string(),
                name: "calendar_lookup".to_string(),
                args: serde_json::value::RawValue::from_string("{}".to_string()).unwrap(),
                meta: None,
            },
            AssistantBlock::Reasoning {
                text: "trailing thought".to_string(),
                meta: Some(Box::new(ProviderMeta::Anthropic {
                    signature: "sig".to_string(),
                })),
            },
        ],
        StopReason::ToolUse,
    ));
    messages.push(Message::ToolResults {
        results: vec![ToolResult::new("call-late".to_string(), "ok".into(), false)],
        created_at: meerkat_core::types::message_timestamp_now(),
    });

    let body = lower(
        &messages,
        Some(AnthropicCacheControlPolicy::SystemAndConversation),
    );
    let lowered = body["messages"].as_array().unwrap();
    let thinking_message = &lowered[lowered.len() - 2]["content"];
    assert_eq!(thinking_message[1]["type"], "thinking");
    assert!(
        thinking_message[1].get("cache_control").is_none(),
        "no breakpoint on the thinking block"
    );
    assert_eq!(
        thinking_message[0]["cache_control"],
        json!({"type": "ephemeral"}),
        "the breakpoint moves to the message's last non-thinking block"
    );
    assert_eq!(
        breakpoint_messages(&body),
        vec![anchor, lowered.len() - 2, lowered.len() - 1]
    );
    assert_eq!(
        body.to_string().matches("\"cache_control\"").count(),
        4,
        "system prefix + anchor + two recent boundaries"
    );
}

// ---------------------------------------------------------------------------
// Operator instruction activation: a System row appended after history
// ---------------------------------------------------------------------------

/// A model whose catalog row accepts mid-conversation System messages, the
/// wire form of an entry appended after history.
const ACTIVATION_MODEL: &str = "claude-opus-5-5";

/// The stand-in for an operator instruction entry.
const ACTIVATION_ENTRY: &str = "Operator instruction: give every time in 24-hour form.";

/// Lowered `messages` index of turn k-1's answer in turn k's request: the
/// anchor of turn k's requests.
const TURN_K_ANCHOR: usize = 1;

/// Lowered `messages` index of turn k's answer in turn k+1's request.
const NEXT_TURN_ANCHOR: usize = 3;

/// Lowered `messages` index of turn k+1's answer in turn k+2's request.
const AFTER_NEXT_ANCHOR: usize = 8;

/// Turn k's request and turn k+1's request around an activation. Turns k-1
/// and k end without tools. Between turns k and k+1 the host appended a
/// System row, which the transcript records before the User row it governs
/// and the client lowers right after that row.
fn activation_requests() -> (Vec<Message>, Vec<Message>) {
    let previous = RunId::new();
    let current = RunId::new();
    let turn_k = vec![
        Message::System(SystemMessage::new("You are the calendar domain agent.")),
        Message::User(UserMessage::text("note tomorrow's agenda")),
        stamped(
            &previous,
            vec![text("Tomorrow: standup at 9, dentist at 14.")],
            StopReason::EndTurn,
        ),
        Message::User(UserMessage::text("move the dentist to 15")),
    ];
    let mut next_turn = turn_k.clone();
    next_turn.extend([
        stamped(
            &current,
            vec![text("Moved: dentist at 15.")],
            StopReason::EndTurn,
        ),
        Message::System(SystemMessage::new(ACTIVATION_ENTRY)),
        Message::User(UserMessage::text("what is left for tomorrow?")),
    ]);
    (turn_k, next_turn)
}

/// Turn k+2's request. Turn k+1 answers after one tool round, so the recent
/// breakpoints of `system_and_conversation` move past the entry: turn k+2
/// lowers `[U1, A1, U2, A2, U3, entry, call, result, A3, U4]` and marks the
/// last three.
fn turn_after_next(next_turn: &[Message]) -> Vec<Message> {
    let run = RunId::new();
    let mut transcript = next_turn.to_vec();
    transcript.extend(tool_call(&run, "call-agenda", "calendar_lookup"));
    transcript.push(stamped(
        &run,
        vec![text("Left for tomorrow: standup at 9.")],
        StopReason::EndTurn,
    ));
    transcript.push(Message::User(UserMessage::text("thanks")));
    transcript
}

fn lower_activation(messages: &[Message], policy: AnthropicCacheControlPolicy) -> Value {
    lower_for(ACTIVATION_MODEL, messages, Some(policy))
}

/// What the provider hashes ahead of `messages`, as exact bytes: the model,
/// the tools, the tool choice, the request-wide breakpoint and `system`.
fn head_bytes(body: &Value) -> String {
    json!({
        "model": body["model"],
        "tools": body["tools"],
        "tool_choice": body.get("tool_choice"),
        "cache_control": body.get("cache_control"),
        "system": body["system"],
    })
    .to_string()
}

/// Each lowered message as exact bytes with its breakpoint markers removed:
/// the content the provider keys its cache on.
fn message_bytes(body: &Value) -> Vec<String> {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|message| without_markers(message).to_string())
        .collect()
}

/// The text of a lowered message content: a bare string or text blocks.
fn content_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| block["text"].as_str())
            .collect(),
        other => panic!("unexpected content: {other}"),
    }
}

/// Build turn k's request, append the entry, build turn k+1's and turn
/// k+2's requests. At each step tools and `system` are byte-equal and the
/// earlier `messages` are a byte prefix of the later ones with the
/// breakpoint markers removed. The entry closes turn k+1's request after the
/// User row it governs and keeps its place in turn k+2's, unmarked. Each
/// request carries exactly the policy's breakpoints, and turn k's anchor
/// lies inside the prefix turn k+1 repeats.
fn assert_activation_keeps_cached_prefix(policy: AnthropicCacheControlPolicy) {
    let (turn_k, next_turn) = activation_requests();
    let after_next = turn_after_next(&next_turn);
    let prior = lower_activation(&turn_k, policy);
    let next = lower_activation(&next_turn, policy);
    let after = lower_activation(&after_next, policy);

    for (earlier, later, added, step) in [
        (&prior, &next, 3, "turn k to k+1"),
        (&next, &after, 4, "turn k+1 to k+2"),
    ] {
        assert_eq!(
            head_bytes(later),
            head_bytes(earlier),
            "{policy:?} {step}: tools and system are byte-equal"
        );
        let earlier_messages = message_bytes(earlier);
        let later_messages = message_bytes(later);
        assert_eq!(
            later_messages.len(),
            earlier_messages.len() + added,
            "{policy:?} {step}"
        );
        assert_eq!(
            later_messages[..earlier_messages.len()],
            earlier_messages[..],
            "{policy:?} {step}: the earlier messages are a byte prefix of the later, markers aside"
        );
    }

    let prior_len = prior["messages"].as_array().unwrap().len();
    let entry_index = prior_len + 2;
    let governed = &next["messages"][entry_index - 1];
    let entry = &next["messages"][entry_index];
    assert_eq!(governed["role"], "user", "{policy:?}: {governed}");
    assert_eq!(entry["role"], "system", "{policy:?}: {entry}");
    assert_eq!(
        content_text(&entry["content"]),
        ACTIVATION_ENTRY,
        "{policy:?}"
    );
    // Turn k+2 has moved every breakpoint past the entry, which keeps the
    // unmarked form of its policy: one text block under
    // `system_and_conversation`, the bare string under the others.
    let unmarked_entry = if policy == AnthropicCacheControlPolicy::SystemAndConversation {
        json!([{"type": "text", "text": ACTIVATION_ENTRY}])
    } else {
        json!(ACTIVATION_ENTRY)
    };
    assert_eq!(
        after["messages"][entry_index]["content"], unmarked_entry,
        "{policy:?}: {after}"
    );

    assert!(TURN_K_ANCHOR < prior_len, "{policy:?}");
    assert!(NEXT_TURN_ANCHOR < entry_index, "{policy:?}");
    let expected = match policy {
        AnthropicCacheControlPolicy::Automatic => [
            vec![TURN_K_ANCHOR],
            vec![NEXT_TURN_ANCHOR],
            vec![AFTER_NEXT_ANCHOR],
        ],
        AnthropicCacheControlPolicy::SystemPrefix => [vec![], vec![], vec![]],
        // The three most recent messages, each set holding its request's
        // anchor; the fix that lowers content as blocks leaves them in place.
        AnthropicCacheControlPolicy::SystemAndConversation => {
            [vec![0, 1, 2], vec![3, 4, 5], vec![7, 8, 9]]
        }
        other => panic!("no activation row for {other:?}"),
    };
    assert_eq!(
        [
            breakpoint_messages(&prior),
            breakpoint_messages(&next),
            breakpoint_messages(&after),
        ],
        expected,
        "{policy:?}: the breakpoints of turns k, k+1 and k+2"
    );
}

#[test]
fn anthropic_activation_keeps_prior_request_a_cached_prefix_automatic() {
    assert_activation_keeps_cached_prefix(AnthropicCacheControlPolicy::Automatic);
}

#[test]
fn anthropic_activation_keeps_prior_request_a_cached_prefix_system_prefix() {
    assert_activation_keeps_cached_prefix(AnthropicCacheControlPolicy::SystemPrefix);
}

#[test]
fn anthropic_activation_keeps_prior_request_a_cached_prefix_system_and_conversation() {
    assert_activation_keeps_cached_prefix(AnthropicCacheControlPolicy::SystemAndConversation);
}

/// Cache-model blocks of `body` through lowered message `through`.
fn blocks_through_message(body: &Value, through: usize) -> usize {
    cache_view(&json!({
        "tools": body["tools"],
        "system": body["system"],
        "messages": body["messages"].as_array().unwrap()[..=through],
    }))
    .prefixes
    .len()
}

#[test]
fn anthropic_cache_model_reads_whole_prior_conversation_after_activation() {
    let (turn_k, next_turn) = activation_requests();
    let after_next = turn_after_next(&next_turn);
    for policy in [
        AnthropicCacheControlPolicy::Automatic,
        AnthropicCacheControlPolicy::SystemAndConversation,
    ] {
        let prior = lower_activation(&turn_k, policy);
        let next = lower_activation(&next_turn, policy);
        let after = lower_activation(&after_next, policy);
        let anchor_prefix = blocks_through_message(&prior, TURN_K_ANCHOR);
        let prior_request = cache_view(&prior).prefixes.len();

        let mut cache = ProviderCache::default();
        cache.serve(&prior);
        let read = cache.serve(&next);
        assert!(
            read >= anchor_prefix,
            "{policy:?}: turn k+1 reads {read} blocks, short of turn k's anchor prefix of \
             {anchor_prefix}"
        );
        assert_eq!(
            read, prior_request,
            "{policy:?}: turn k+1 reads all of turn k's request from cache"
        );
        assert_eq!(
            cache.serve(&after),
            cache_view(&next).prefixes.len(),
            "{policy:?}: turn k+2 reads all of turn k+1's request, the entry included"
        );
    }

    // `system_prefix` caches the system prefix alone, which the activation
    // leaves as it was.
    let prior = lower_activation(&turn_k, AnthropicCacheControlPolicy::SystemPrefix);
    let next = lower_activation(&next_turn, AnthropicCacheControlPolicy::SystemPrefix);
    let after = lower_activation(&after_next, AnthropicCacheControlPolicy::SystemPrefix);
    let system_prefix = cache_view(&json!({
        "tools": next["tools"],
        "system": next["system"],
        "messages": [],
    }))
    .prefixes
    .len();
    let mut cache = ProviderCache::default();
    cache.serve(&prior);
    assert_eq!(cache.serve(&next), system_prefix, "system_prefix: {next}");
    assert_eq!(cache.serve(&after), system_prefix, "system_prefix: {after}");
}

/// Under `system_and_conversation` the client lowers the entry after the
/// User row it governs, so turn k+1's transcript `[S, U1, A1, U2, A2, entry,
/// U3]` lowers to `[U1, A1, U2, A2, U3, entry]` and the window marks A2, U3
/// and the entry. A breakpoint claim binds the transcript prefix through a
/// marked message to the messages rendered through that marker, so it holds
/// only where they are exactly that prefix. The marker on U3 ends the
/// rendered `[U1, A1, U2, A2, U3]`, which is no transcript prefix, so it
/// carries no claim: the claims are the system prefix, turn k's answer and
/// the whole request.
#[test]
fn anthropic_cache_claims_follow_the_rendered_order_of_a_moved_system_row() {
    let policy = AnthropicCacheControlPolicy::SystemAndConversation;
    let (_, next_turn) = activation_requests();
    let client = AnthropicClient::new("test-key".to_string()).unwrap();
    let mut request = LlmRequest::new(ACTIVATION_MODEL, next_turn)
        .with_tools(tools())
        .with_anthropic_tag_merge(|tag| tag.cache_control = Some(policy));
    request.messages = client.project_replay_messages(&request.messages).unwrap();
    let body = client.build_request_body(&request).unwrap();
    assert_eq!(breakpoint_messages(&body), vec![3, 4, 5], "{body}");

    assert_eq!(
        AnthropicClient::anthropic_authored_breakpoint_boundaries(&request, policy, &body).unwrap(),
        vec![
            (
                CacheBreakpointBoundary::SystemProfilePrefix { message_count: 1 },
                0,
            ),
            (
                CacheBreakpointBoundary::TranscriptAfter { message_count: 5 },
                4,
            ),
            (
                CacheBreakpointBoundary::TranscriptAfter { message_count: 7 },
                6,
            ),
        ],
        "the system prefix, turn k's answer and the whole request, each with its lowered count"
    );
    assert_eq!(
        client
            .authored_cache_breakpoints(&request, &request.messages)
            .unwrap()
            .len(),
        3,
        "the client claims exactly those boundaries"
    );
}

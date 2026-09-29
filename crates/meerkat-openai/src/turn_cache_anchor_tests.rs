//! The turn-boundary cache anchor on lowered OpenAI Responses requests.
//!
//! A `fork_off` child is cut at the forker's previous turn end. Its first
//! request is the forker's transcript up to that boundary followed by its own
//! task, so it can only read a provider cache entry written inside that
//! transcript. The implicit breakpoint sits at the end of the latest eligible
//! message, so without an explicit marker a forking turn that starts on a
//! cold cache writes only after its own prompt (issue #1235).
//!
//! These tests lower the forker's and the child's requests through the same
//! replay projection and body builder the client streams, then:
//!
//! - pin that every request of the forking turn and the child's first request
//!   carry an explicit breakpoint on the last input before the previous run's
//!   output, and that the lowered input through that item, breakpoints
//!   included, is byte-identical between them, in both explicit mode and
//!   implicit mode (the GPT-6 default);
//! - replay the requests against a model of OpenAI's documented cache
//!   (entries only at breakpoints, at most four writes per request, lookups
//!   walk back through up to 20 eligible boundaries) for a cold-start and a
//!   warm-start forker turn, and assert the child reads the shared prefix.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::HashSet;
use std::sync::Arc;

use meerkat_core::lifecycle::RunId;
use meerkat_core::lifecycle::run_primitive::OpenAiPromptCacheOptions;
use meerkat_core::model_profile::capabilities::{OpenAiPromptCacheMode, OpenAiPromptCacheTtl};
use meerkat_core::{
    AssistantBlock, BlockAssistantMessage, Message, StopReason, SystemMessage, ToolDef, ToolResult,
    TranscriptMessageIdentity, UserMessage,
};
use meerkat_llm_core::{LlmClient, LlmRequest};
use serde_json::{Value, json};

use crate::OpenAiClient;

/// The catalog's default OpenAI model; its row admits implicit and explicit
/// prompt-cache modes.
const MODEL: &str = "gpt-6-astra";
const LOOKBACK_BOUNDARIES: usize = 20;
const MAX_WRITES: usize = 4;

fn tools() -> Vec<Arc<ToolDef>> {
    ["calendar_lookup", "fork_off"]
        .into_iter()
        .map(|name| {
            Arc::new(ToolDef {
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

struct ForkRequests {
    previous_last: Vec<Message>,
    forker_first: Vec<Message>,
    forker_round: Vec<Message>,
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
        vec![AssistantBlock::Text {
            text: "Tomorrow: standup at 9, dentist at 14.".to_string(),
            meta: None,
        }],
        StopReason::EndTurn,
    ));
    let mut forker_first = prefix.clone();
    forker_first.push(Message::User(UserMessage::text(
        "fork a child for tomorrow's agenda",
    )));
    let mut forker_round = forker_first.clone();
    forker_round.extend(tool_call(&forker, "call-fork", "fork_off"));
    let mut child_first = prefix;
    child_first.push(Message::User(UserMessage::text(
        "list tomorrow's events (forked task)",
    )));
    ForkRequests {
        previous_last,
        forker_first,
        forker_round,
        child_first,
    }
}

/// Lowered `input` index of the previous run's last tool output: the last
/// input item before its final answer. Lowered items: system, user,
/// function_call, function_call_output, assistant answer, then the new input.
const ANCHOR_ITEM: usize = 3;
const ANSWER_ITEM: usize = 4;

fn lower(messages: &[Message], mode: Option<OpenAiPromptCacheMode>) -> Value {
    let client = OpenAiClient::new("test-key".to_string());
    let mut request = LlmRequest::new(MODEL, messages.to_vec()).with_tools(tools());
    if let Some(mode) = mode {
        request = request.with_openai_tag_merge(|tag| {
            tag.prompt_cache_enabled = Some(true);
            tag.prompt_cache_key = Some(format!("meerkat:profile:openai:{MODEL}"));
            tag.prompt_cache_options = Some(OpenAiPromptCacheOptions {
                mode: Some(mode),
                ttl: Some(OpenAiPromptCacheTtl::ThirtyMinutes),
            });
        });
    }
    request.messages = client.project_replay_messages(&request.messages).unwrap();
    client.build_request_body(&request).unwrap()
}

fn marked(item: &Value) -> bool {
    item.to_string().contains("prompt_cache_breakpoint")
}

fn marked_items(body: &Value) -> Vec<usize> {
    body["input"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .filter_map(|(index, item)| marked(item).then_some(index))
        .collect()
}

fn input_through(body: &Value, through: usize) -> Vec<Value> {
    body["input"].as_array().unwrap()[..=through].to_vec()
}

fn top_level_without_input(body: &Value) -> Value {
    let mut body = body.clone();
    body.as_object_mut().unwrap().remove("input");
    body
}

// ---------------------------------------------------------------------------
// A model of OpenAI's documented prompt cache (GPT-5.6 and later)
// ---------------------------------------------------------------------------

fn strip_breakpoints(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .filter(|(key, _)| key.as_str() != "prompt_cache_breakpoint")
                .map(|(key, value)| (key.clone(), strip_breakpoints(value)))
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(strip_breakpoints).collect()),
        other => other.clone(),
    }
}

struct CacheView {
    /// `prefixes[i]`: the request's cache key through input item `i`.
    prefixes: Vec<String>,
    /// Items whose end is a lookup boundary.
    eligible: Vec<usize>,
    breakpoints: Vec<usize>,
}

fn cache_view(body: &Value) -> CacheView {
    let head = json!({
        "model": body["model"],
        "tools": body["tools"],
        "instructions": body.get("instructions"),
    })
    .to_string();
    let items = body["input"].as_array().unwrap();
    let mut prefixes = Vec::with_capacity(items.len());
    let mut running = head;
    let mut eligible = Vec::new();
    for (index, item) in items.iter().enumerate() {
        running.push('\u{1e}');
        running.push_str(&strip_breakpoints(item).to_string());
        prefixes.push(running.clone());
        let is_tool_output = item["type"] == "function_call_output";
        let next_is_tool_output = items
            .get(index + 1)
            .is_some_and(|next| next["type"] == "function_call_output");
        let is_input_message = item["type"] == "message"
            && matches!(item["role"].as_str(), Some("user" | "system" | "developer"));
        if is_input_message || (is_tool_output && !next_is_tool_output) {
            eligible.push(index);
        }
    }
    let explicit_only = body["prompt_cache_options"]["mode"] == "explicit";
    let mut breakpoints: Vec<usize> = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| marked(item).then_some(index))
        .collect();
    if !explicit_only && let Some(latest) = eligible.last() {
        breakpoints.push(*latest);
    }
    breakpoints.sort_unstable();
    breakpoints.dedup();
    CacheView {
        prefixes,
        eligible,
        breakpoints,
    }
}

#[derive(Default)]
struct ProviderCache {
    entries: HashSet<String>,
}

impl ProviderCache {
    /// Serve one request: return the number of input items read from cache,
    /// then write the latest breakpoints that were not already cached (at
    /// most four) and refresh the rest.
    fn serve(&mut self, body: &Value) -> usize {
        let view = cache_view(body);
        let read = view
            .breakpoints
            .iter()
            .filter_map(|&breakpoint| {
                view.eligible
                    .iter()
                    .rev()
                    .filter(|&&index| index <= breakpoint)
                    .take(LOOKBACK_BOUNDARIES)
                    .find(|&&index| self.entries.contains(&view.prefixes[index]))
                    .map(|index| index + 1)
            })
            .max()
            .unwrap_or(0);
        let writes: Vec<usize> = view
            .breakpoints
            .iter()
            .rev()
            .filter(|&&index| !self.entries.contains(&view.prefixes[index]))
            .take(MAX_WRITES)
            .copied()
            .collect();
        for index in writes {
            self.entries.insert(view.prefixes[index].clone());
        }
        read
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

fn assert_forker_and_child_agree(mode: OpenAiPromptCacheMode) {
    let fork = fork_requests();
    let forker_first = lower(&fork.forker_first, Some(mode));
    let forker_round = lower(&fork.forker_round, Some(mode));
    let child_first = lower(&fork.child_first, Some(mode));

    for (name, body) in [
        ("forker first", &forker_first),
        ("forker tool round", &forker_round),
        ("child first", &child_first),
    ] {
        assert!(
            marked(&body["input"][ANCHOR_ITEM]),
            "{mode:?} {name}: the previous run's last input carries a breakpoint: {}",
            body["input"]
        );
        assert_eq!(body["input"][ANSWER_ITEM]["role"], "assistant");
    }
    let child_prefix = input_through(&child_first, ANCHOR_ITEM);
    assert_eq!(input_through(&forker_first, ANCHOR_ITEM), child_prefix);
    assert_eq!(input_through(&forker_round, ANCHOR_ITEM), child_prefix);
    assert_eq!(
        top_level_without_input(&forker_first),
        top_level_without_input(&child_first),
        "{mode:?}: model, tools and cache knobs match"
    );
}

#[test]
fn explicit_mode_forker_and_child_share_the_anchor_breakpoint() {
    assert_forker_and_child_agree(OpenAiPromptCacheMode::Explicit);
}

#[test]
fn implicit_mode_authors_one_anchor_breakpoint_forker_and_child_share() {
    assert_forker_and_child_agree(OpenAiPromptCacheMode::Implicit);
    let fork = fork_requests();
    for messages in [&fork.forker_first, &fork.forker_round, &fork.child_first] {
        assert_eq!(
            marked_items(&lower(messages, Some(OpenAiPromptCacheMode::Implicit))),
            vec![ANCHOR_ITEM],
            "implicit mode adds exactly the turn anchor"
        );
    }
}

#[test]
fn provider_default_mode_authors_no_breakpoint() {
    let fork = fork_requests();
    assert!(marked_items(&lower(&fork.forker_round, None)).is_empty());
}

#[test]
fn cold_start_forker_turn_leaves_an_entry_the_child_reads() {
    for mode in [
        OpenAiPromptCacheMode::Explicit,
        OpenAiPromptCacheMode::Implicit,
    ] {
        let fork = fork_requests();
        let mut cache = ProviderCache::default();
        assert_eq!(
            cache.serve(&lower(&fork.forker_first, Some(mode))),
            0,
            "{mode:?}: the forker starts cold"
        );
        cache.serve(&lower(&fork.forker_round, Some(mode)));
        assert_eq!(
            cache.serve(&lower(&fork.child_first, Some(mode))),
            ANCHOR_ITEM + 1,
            "{mode:?}: the child reads everything up to the previous run's answer"
        );
    }

    // Control: implicit mode without the anchor marker. The cold forker only
    // writes after its own prompt, and the child re-bills the transcript.
    let fork = fork_requests();
    let unanchored = |messages: &[Message]| {
        let mut body = lower(messages, Some(OpenAiPromptCacheMode::Implicit));
        body["input"] = strip_breakpoints(&body["input"]);
        body
    };
    let mut cache = ProviderCache::default();
    cache.serve(&unanchored(&fork.forker_first));
    cache.serve(&unanchored(&fork.forker_round));
    assert_eq!(cache.serve(&unanchored(&fork.child_first)), 0);
}

#[test]
fn warm_start_forker_turn_refreshes_the_entry_the_child_reads() {
    for mode in [
        OpenAiPromptCacheMode::Explicit,
        OpenAiPromptCacheMode::Implicit,
    ] {
        let fork = fork_requests();
        let mut cache = ProviderCache::default();
        cache.serve(&lower(&fork.previous_last, Some(mode)));
        assert_eq!(
            cache.serve(&lower(&fork.forker_first, Some(mode))),
            ANCHOR_ITEM + 1,
            "{mode:?}: the forker's first request reads the previous run's tail"
        );
        cache.serve(&lower(&fork.forker_round, Some(mode)));
        assert_eq!(
            cache.serve(&lower(&fork.child_first, Some(mode))),
            ANCHOR_ITEM + 1,
            "{mode:?}"
        );
    }
}

#[test]
fn implicit_anchor_mode_authors_no_cache_evidence() {
    let fork = fork_requests();
    let client = OpenAiClient::new("test-key".to_string());
    let request_for = |mode| {
        let mut request = LlmRequest::new(MODEL, fork.forker_round.clone())
            .with_tools(tools())
            .with_openai_tag_merge(|tag| {
                tag.prompt_cache_enabled = Some(true);
                tag.prompt_cache_options = Some(OpenAiPromptCacheOptions {
                    mode: Some(mode),
                    ttl: Some(OpenAiPromptCacheTtl::ThirtyMinutes),
                });
            });
        request.messages = client.project_replay_messages(&request.messages).unwrap();
        request
    };
    let implicit = request_for(OpenAiPromptCacheMode::Implicit);
    assert!(
        client
            .authored_cache_breakpoints(&implicit, &implicit.messages)
            .unwrap()
            .is_empty(),
        "the implicit turn anchor stays off the per-request evidence path"
    );
    let explicit = request_for(OpenAiPromptCacheMode::Explicit);
    assert!(
        !client
            .authored_cache_breakpoints(&explicit, &explicit.messages)
            .unwrap()
            .is_empty(),
        "control: explicit mode does author evidence"
    );
}

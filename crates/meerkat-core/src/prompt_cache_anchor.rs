//! Provider prompt-cache anchor at the boundary before the current run.
//!
//! A provider prompt cache serves a request only from an entry written at one
//! of its breakpoints. Automatic breakpoints sit at the end of each request,
//! so every request inside a run writes entries after that run's own input.
//! When the run's first request finds the cache cold, no entry exists at the
//! end of the transcript the run started from, and nothing that shares only
//! that transcript (a `fork_off` child cut at the previous turn end, or the
//! source's own next run after a rewrite of the in-flight turn) can read one.
//!
//! [`prior_run_cache_anchor`] names that boundary from typed transcript state
//! alone, so provider lowerings can keep an explicit breakpoint there on
//! every request of the run: the run's first request writes the entry and
//! each later request reads and refreshes it. Because the anchor is a pure
//! function of the transcript, a fork child whose request is the source's
//! transcript up to the boundary followed by its own task computes the same
//! anchor at the same message and so sends the same breakpoint bytes.

use crate::types::Message;

/// Index of the last message of the transcript that precedes the run the
/// request's latest messages belong to, if typed state identifies one.
///
/// The anchor is always a [`Message::BlockAssistant`]:
///
/// - When the latest assistant message requested no tools, its provider turn
///   ended the run it belongs to, and whatever follows opens a new run. That
///   message is the anchor.
/// - When the latest assistant message requested tools, the request is a tool
///   round of that message's run (its [`crate::TranscriptMessageIdentity`]
///   `run_id`). The anchor is the latest earlier assistant message that does
///   not belong to that run: the end of the previous run's output.
///
/// Returns `None` when the transcript has no assistant message, when the
/// latest tool round carries no run identity (a legacy row that cannot be
/// grouped), or when no earlier run exists.
///
/// Provider lowerings place their breakpoint on the anchor itself or, when
/// the provider only accepts breakpoints on input items, on the last input at
/// or before it.
pub fn prior_run_cache_anchor(messages: &[Message]) -> Option<usize> {
    let (latest_index, latest) =
        messages
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, message)| match message {
                Message::BlockAssistant(assistant) => Some((index, assistant)),
                _ => None,
            })?;
    if !latest.has_tool_calls() {
        return Some(latest_index);
    }
    let current_run = latest.identity.run_id.as_ref()?;
    messages[..latest_index]
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, message)| match message {
            Message::BlockAssistant(assistant)
                if assistant.identity.run_id.as_ref() != Some(current_run) =>
            {
                Some(index)
            }
            _ => None,
        })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::lifecycle::RunId;
    use crate::types::{
        AssistantBlock, BlockAssistantMessage, StopReason, SystemMessage, ToolResult,
        TranscriptMessageIdentity, UserMessage,
    };

    fn user(text: &str) -> Message {
        Message::User(UserMessage::text(text))
    }

    fn reply(run: &RunId, text: &str) -> Message {
        let mut message = BlockAssistantMessage::new(
            vec![AssistantBlock::Text {
                text: text.to_string(),
                meta: None,
            }],
            StopReason::EndTurn,
        );
        message.identity = TranscriptMessageIdentity::default().with_run_id(run.clone());
        Message::BlockAssistant(message)
    }

    fn tool_round(run: Option<&RunId>, call_id: &str) -> [Message; 2] {
        let mut message = BlockAssistantMessage::new(
            vec![AssistantBlock::ToolUse {
                id: call_id.to_string(),
                name: "lookup".into(),
                args: serde_json::value::RawValue::from_string("{}".to_string()).unwrap(),
                meta: None,
            }],
            StopReason::ToolUse,
        );
        if let Some(run) = run {
            message.identity = TranscriptMessageIdentity::default().with_run_id(run.clone());
        }
        [
            Message::BlockAssistant(message),
            Message::ToolResults {
                results: vec![ToolResult::new(call_id.to_string(), "ok".into(), false)],
                created_at: crate::types::message_timestamp_now(),
            },
        ]
    }

    fn previous_turn(previous: &RunId) -> Vec<Message> {
        vec![
            Message::System(SystemMessage::new("system")),
            user("first"),
            reply(previous, "first answer"),
        ]
    }

    #[test]
    fn no_assistant_message_has_no_anchor() {
        let messages = vec![Message::System(SystemMessage::new("system")), user("hi")];
        assert_eq!(prior_run_cache_anchor(&messages), None);
    }

    #[test]
    fn first_request_of_a_run_anchors_on_the_previous_reply() {
        let previous = RunId::new();
        let mut messages = previous_turn(&previous);
        messages.push(user("second"));
        assert_eq!(prior_run_cache_anchor(&messages), Some(2));
    }

    #[test]
    fn tool_rounds_keep_the_anchor_on_the_previous_run() {
        let previous = RunId::new();
        let current = RunId::new();
        let mut messages = previous_turn(&previous);
        messages.push(user("second"));
        messages.extend(tool_round(Some(&current), "call-1"));
        assert_eq!(prior_run_cache_anchor(&messages), Some(2));
        messages.extend(tool_round(Some(&current), "call-2"));
        assert_eq!(prior_run_cache_anchor(&messages), Some(2));
    }

    #[test]
    fn a_fork_child_request_anchors_where_the_forker_does() {
        let previous = RunId::new();
        let forker_run = RunId::new();
        let child_run = RunId::new();
        let prefix = previous_turn(&previous);

        let mut forker_first = prefix.clone();
        forker_first.push(user("fork a child"));
        let mut forker_round = forker_first.clone();
        forker_round.extend(tool_round(Some(&forker_run), "fork"));
        let mut child_first = prefix.clone();
        child_first.push(user("child task"));
        let mut child_round = child_first.clone();
        child_round.extend(tool_round(Some(&child_run), "child-call"));

        for messages in [&forker_first, &forker_round, &child_first, &child_round] {
            assert_eq!(prior_run_cache_anchor(messages), Some(prefix.len() - 1));
        }
    }

    #[test]
    fn a_tool_round_without_run_identity_has_no_anchor() {
        let previous = RunId::new();
        let mut messages = previous_turn(&previous);
        messages.push(user("second"));
        messages.extend(tool_round(None, "call-1"));
        assert_eq!(prior_run_cache_anchor(&messages), None);
    }

    #[test]
    fn a_first_run_tool_round_has_no_earlier_run() {
        let current = RunId::new();
        let mut messages = vec![Message::System(SystemMessage::new("system")), user("hi")];
        messages.extend(tool_round(Some(&current), "call-1"));
        assert_eq!(prior_run_cache_anchor(&messages), None);
    }
}

//! Assistant message identity on the canonical transcript: serde
//! compatibility, digest neutrality, and the explicit semantics for rewrite,
//! fork and removal.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use super::*;
use crate::types::{
    AssistantBlock, AssistantMessageId, BlockAssistantMessage, StopReason, UserMessage,
};

fn assistant(text: &str, id: Option<AssistantMessageId>) -> Message {
    let mut message = BlockAssistantMessage::new(
        vec![AssistantBlock::Text {
            text: text.to_string(),
            meta: None,
        }],
        StopReason::EndTurn,
    );
    message.assistant_message_id = id;
    Message::BlockAssistant(message)
}

fn id_of(message: &Message) -> Option<AssistantMessageId> {
    match message {
        Message::BlockAssistant(assistant) => assistant.assistant_message_id,
        _ => None,
    }
}

fn strip_ids(messages: &[Message]) -> Vec<Message> {
    messages
        .iter()
        .cloned()
        .map(|mut message| {
            if let Message::BlockAssistant(assistant) = &mut message {
                assistant.assistant_message_id = None;
            }
            message
        })
        .collect()
}

/// user, assistant(A "same"), user, assistant(B "same")
fn session_with_two_identical_answers() -> (Session, AssistantMessageId, AssistantMessageId) {
    let first = AssistantMessageId::mint();
    let second = AssistantMessageId::mint();
    let mut session = Session::new();
    session.push(Message::User(UserMessage::text("q1")));
    session.push(assistant("same", Some(first)));
    session.push(Message::User(UserMessage::text("q2")));
    session.push(assistant("same", Some(second)));
    (session, first, second)
}

/// The exact 0.8.44 encoding of an assistant row: no identity key at all.
const LEGACY_ROW: &str = r#"{"role":"block_assistant","blocks":[{"block_type":"text","data":{"text":"hi"}}],"stop_reason":"end_turn","created_at":"2026-09-26T00:00:00Z"}"#;

#[test]
fn legacy_rows_parse_without_id_and_reserialize_byte_identically() {
    let message: Message = serde_json::from_str(LEGACY_ROW).expect("0.8.44 row parses");
    assert_eq!(id_of(&message), None, "an absent id is never fabricated");
    assert_eq!(
        serde_json::to_string(&message).unwrap(),
        LEGACY_ROW,
        "a row without an id keeps the 0.8.44 bytes"
    );
}

#[test]
fn assistant_message_id_round_trips_verbatim() {
    let id = AssistantMessageId::mint();
    let message = assistant("hi", Some(id));
    let encoded = serde_json::to_value(&message).unwrap();
    assert_eq!(
        encoded["assistant_message_id"],
        serde_json::Value::String(id.to_string()),
        "the id serializes as its bare UUID string"
    );
    let decoded: Message = serde_json::from_value(encoded).unwrap();
    assert_eq!(id_of(&decoded), Some(id));
    assert_eq!(id.as_uuid().to_string(), id.to_string());

    let not_a_uuid = serde_json::json!({
        "role": "block_assistant",
        "blocks": [],
        "assistant_message_id": "not-a-uuid",
    });
    assert!(
        serde_json::from_value::<Message>(not_a_uuid).is_err(),
        "a malformed id is rejected, not coerced"
    );
}

#[test]
fn transcript_digests_ignore_assistant_message_identity() {
    let (session, _, _) = session_with_two_identical_answers();
    let mut stripped = Session::new();
    for message in strip_ids(session.messages()) {
        stripped.push(message);
    }

    assert_eq!(
        canonical_transcript_prefix_identity(session.messages()).unwrap(),
        canonical_transcript_prefix_identity(stripped.messages()).unwrap(),
        "the provider-cache prefix identity is id-neutral"
    );
    assert_eq!(
        transcript_messages_digest_uncounted(session.messages()).unwrap(),
        transcript_messages_digest_uncounted(stripped.messages()).unwrap(),
        "transcript content digests are id-neutral"
    );
    assert_eq!(
        session.transcript_revision().unwrap(),
        stripped.transcript_revision().unwrap(),
        "transcript revisions are id-neutral"
    );
}

#[test]
fn generic_rewrite_clears_replacement_ids_and_keeps_everything_else() {
    let (mut session, first, second) = session_with_two_identical_answers();
    let parent = session.transcript_revision().unwrap();
    let forged = AssistantMessageId::mint();

    session
        .commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 3, end: 4 },
            vec![assistant("edited", Some(forged))],
            TranscriptRewriteReason::new("unit-test"),
            Some("unit-test".to_string()),
            Some(parent.clone()),
        )
        .expect("rewrite commits");

    let messages = session.messages();
    assert_eq!(
        id_of(&messages[1]),
        Some(first),
        "rows outside keep their ids"
    );
    assert_eq!(
        id_of(&messages[3]),
        None,
        "a replacement row never carries an id, even a supplied one"
    );
    let parent_rows = session
        .transcript_revision_messages(&parent)
        .unwrap()
        .expect("the parent revision stays readable");
    assert_eq!(
        id_of(&parent_rows[3]),
        Some(second),
        "the replaced id is readable in the parent revision"
    );

    // Removal: the id leaves the active transcript, not the parent revision.
    let before_removal = session.transcript_revision().unwrap();
    session
        .commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
            Vec::new(),
            TranscriptRewriteReason::new("unit-test"),
            Some("unit-test".to_string()),
            Some(before_removal.clone()),
        )
        .expect("removal commits");
    assert!(
        session
            .messages()
            .iter()
            .all(|message| id_of(message) != Some(first))
    );
    let removal_parent = session
        .transcript_revision_messages(&before_removal)
        .unwrap()
        .expect("removal parent readable");
    assert_eq!(id_of(&removal_parent[1]), Some(first));
}

#[test]
fn forks_inherit_occurrences_and_edited_rows_lose_their_id() {
    let (session, first, second) = session_with_two_identical_answers();

    let whole = session.fork();
    assert_eq!(id_of(&whole.messages()[1]), Some(first));
    assert_eq!(id_of(&whole.messages()[3]), Some(second));
    let prefix = session.fork_at(2);
    assert_eq!(id_of(&prefix.messages()[1]), Some(first));

    let block_edit = session
        .fork_replacing(
            3,
            TranscriptReplacement::AssistantBlock {
                block_index: 0,
                block: AssistantBlock::Text {
                    text: "edited".to_string(),
                    meta: None,
                },
            },
        )
        .expect("block edit forks");
    assert_eq!(id_of(&block_edit.messages()[1]), Some(first));
    assert_eq!(
        id_of(&block_edit.messages()[3]),
        None,
        "a block-edited row is new content"
    );

    let whole_edit = session
        .fork_replacing(
            3,
            TranscriptReplacement::Message {
                message: assistant("replacement", Some(AssistantMessageId::mint())),
            },
        )
        .expect("message edit forks");
    assert_eq!(id_of(&whole_edit.messages()[3]), None);
}

#[test]
fn last_assistant_text_occurrence_names_the_exact_row() {
    let text_row = AssistantMessageId::mint();
    let tool_row = AssistantMessageId::mint();
    let mut session = Session::new();
    session.push(Message::User(UserMessage::text("q")));
    session.push(assistant("the answer", Some(text_row)));
    let mut tool_only = BlockAssistantMessage::new(
        vec![AssistantBlock::ToolUse {
            id: "call-1".to_string(),
            name: "lookup".into(),
            args: serde_json::value::RawValue::from_string("{}".to_string()).unwrap(),
            meta: None,
        }],
        StopReason::ToolUse,
    );
    tool_only.assistant_message_id = Some(tool_row);
    session.push(Message::BlockAssistant(tool_only));

    assert_eq!(
        session.last_assistant_text_occurrence(),
        Some(("the answer".to_string(), Some(text_row)))
    );
    assert_eq!(
        session.last_assistant_text(),
        Some("the answer".to_string())
    );

    session.push(assistant("legacy", None));
    assert_eq!(
        session.last_assistant_text_occurrence(),
        Some(("legacy".to_string(), None)),
        "a pre-0.8.45 row is referenced as absent, never guessed"
    );
}

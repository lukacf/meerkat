//! Compact retention for the durable transcript revision graph.
//!
//! The graph used to retain complete historical transcript bodies. Current
//! state retains one full anchor plus ordered rewrite-occurrence edges. Each
//! edge carries only the exact parent advance and rewrite replacement needed to
//! reconstruct an audited endpoint.
//!
//! These tests stay outside the representation's private fields. They inspect
//! the published serde wire for its bounded shape, use `materialize_revision`
//! for historical reconstruction, and cross a full `Session` durable
//! round-trip. The contract they pin is compact growth without weakening exact
//! historical reads.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use meerkat_core::lifecycle::RunId;
use meerkat_core::service::{TranscriptRewriteReason, TranscriptRewriteSelection};
use meerkat_core::session::transcript_messages_digest;
use meerkat_core::types::{
    AssistantBlock, BlockAssistantMessage, ContentBlock, ImageData, Message, StopReason,
    ToolResult, TranscriptMessageIdentity, UserMessage,
};
use meerkat_core::{
    SESSION_TRANSCRIPT_HISTORY_STATE_KEY, Session, TranscriptHistoryState, TranscriptRewriteCommit,
};

const BODY: &str = "with enough body text on every message that a full retained \
                    copy is unmistakably larger than one edited span";
const CURRENT_GRAPH_FORMAT: &str = "anchor_occurrence_edges_v1";

fn user(text: &str) -> Message {
    Message::User(UserMessage::text(text))
}

fn assistant(text: &str) -> Message {
    Message::BlockAssistant(BlockAssistantMessage::new(
        vec![AssistantBlock::Text {
            text: text.to_string(),
            meta: None,
        }],
        StopReason::EndTurn,
    ))
}

fn session_with_turns(turns: usize) -> Session {
    let mut session = Session::new();
    session.append_system_message(format!("system prompt {BODY}"));
    for index in 0..turns {
        session.push(user(&format!("question {index} {BODY}")));
        session.push(assistant(&format!("answer {index} {BODY}")));
    }
    session
}

fn rewrite(
    session: &mut Session,
    start: usize,
    end: usize,
    replacement: Vec<Message>,
    why: &str,
) -> TranscriptRewriteCommit {
    session
        .commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start, end },
            replacement,
            TranscriptRewriteReason::new(why),
            Some("delta-retention-test".to_string()),
            None,
        )
        .expect("rewrite should commit")
}

fn history_value(session: &Session) -> serde_json::Value {
    serde_json::to_value(session).expect("session serializes")["metadata"]
        [SESSION_TRANSCRIPT_HISTORY_STATE_KEY]
        .clone()
}

fn history_bytes(session: &Session) -> usize {
    serde_json::to_vec(&history_value(session))
        .expect("history value serializes")
        .len()
}

fn transcript_bytes(session: &Session) -> usize {
    serde_json::to_vec(session.messages())
        .expect("transcript serializes")
        .len()
}

fn state_of(session: &Session) -> TranscriptHistoryState {
    session
        .transcript_history_state()
        .expect("history state decodes")
        .expect("fixture retains a transcript graph")
}

fn edges(history: &serde_json::Value) -> &[serde_json::Value] {
    history["edges"]
        .as_array()
        .expect("compact durable graph carries an edges array")
}

fn count_named_field(value: &serde_json::Value, field: &str) -> usize {
    match value {
        serde_json::Value::Array(values) => values
            .iter()
            .map(|value| count_named_field(value, field))
            .sum(),
        serde_json::Value::Object(values) => {
            usize::from(values.contains_key(field))
                + values
                    .values()
                    .map(|value| count_named_field(value, field))
                    .sum::<usize>()
        }
        _ => 0,
    }
}

fn assert_compact_graph_wire(history: &serde_json::Value, expected_edges: usize) {
    assert_eq!(
        history["format"], CURRENT_GRAPH_FORMAT,
        "fixture must use the current compact graph wire"
    );
    assert!(
        history.get("anchor").is_some(),
        "compact graph must carry one anchor"
    );
    assert!(
        history["anchor"]["messages"].is_array(),
        "the anchor is the graph's one retained full transcript"
    );
    assert_eq!(
        edges(history).len(),
        expected_edges,
        "one occurrence edge must be retained per audited rewrite"
    );
    assert!(
        history.get("revisions").is_none()
            && history.get("commits").is_none()
            && history.get("head").is_none(),
        "the deleted parallel full-body graph must not reappear"
    );
    assert_eq!(
        count_named_field(history, "messages"),
        1,
        "only the anchor may carry a full `messages` vector"
    );
    for (index, edge) in edges(history).iter().enumerate() {
        assert!(
            edge["rewrite"]["replacement"].is_array(),
            "edge {index} must carry an explicit rewrite replacement"
        );
        assert!(
            edge.get("parent_body").is_none() && edge.get("revision_body").is_none(),
            "edge {index} must not retain full endpoint bodies"
        );
    }
}

fn assert_materialized_messages(
    state: &TranscriptHistoryState,
    revision: &str,
    expected: &serde_json::Value,
) {
    let body = state
        .materialize_revision(revision)
        .unwrap_or_else(|error| panic!("revision {revision} must materialize: {error}"));
    assert_eq!(
        serde_json::to_value(body.messages).expect("materialized messages serialize"),
        *expected,
        "revision {revision} must reconstruct byte-faithfully"
    );
}

#[test]
fn single_message_rewrite_retains_the_edit_not_the_document() {
    let mut session = session_with_turns(200);
    assert_eq!(session.messages().len(), 401);
    let transcript = transcript_bytes(&session);
    let parent_revision = session.transcript_revision().expect("parent revision");
    let parent_messages =
        serde_json::to_value(session.messages()).expect("parent transcript serializes");

    let commit = rewrite(
        &mut session,
        200,
        201,
        vec![user(&format!("edited question {BODY}"))],
        "single-message-edit",
    );
    let child_messages =
        serde_json::to_value(session.messages()).expect("child transcript serializes");

    let history = history_value(&session);
    assert_compact_graph_wire(&history, 1);
    let edge = &edges(&history)[0];
    assert_eq!(edge["rewrite"]["at"], serde_json::Value::from(200u64));
    assert_eq!(
        edge["rewrite"]["replacement"]
            .as_array()
            .expect("replacement is an array")
            .len(),
        1,
        "a one-message rewrite carries exactly one replacement message"
    );
    assert_eq!(edge["parent_advance"]["kind"], "exact_append");
    assert!(
        edge["parent_advance"]
            .get("appended")
            .and_then(serde_json::Value::as_array)
            .is_none_or(Vec::is_empty),
        "the first rewrite begins directly at the anchor"
    );

    let retained = history_bytes(&session);
    assert!(
        retained < transcript + 4096,
        "one rewrite must retain one anchor transcript plus a bounded edge: \
         {retained} B against a {transcript} B transcript"
    );

    let state = state_of(&session);
    assert_eq!(state.commit_count(), 1);
    assert_eq!(state.head(), commit.revision);
    assert_materialized_messages(&state, &parent_revision, &parent_messages);
    assert_materialized_messages(&state, &commit.revision, &child_messages);
}

#[test]
fn stacked_rewrites_reconstruct_exactly_with_compact_growth() {
    let mut session = session_with_turns(120);
    let mut lived: Vec<(String, serde_json::Value)> = Vec::new();

    for round in 0..8 {
        lived.push((
            session.transcript_revision().expect("parent revision"),
            serde_json::to_value(session.messages()).expect("parent transcript serializes"),
        ));
        let target = 40 + round * 4;
        let commit = rewrite(
            &mut session,
            target,
            target + 1,
            vec![user(&format!("stacked edit {round} {BODY}"))],
            "stacked-edit",
        );
        lived.push((
            commit.revision,
            serde_json::to_value(session.messages()).expect("child transcript serializes"),
        ));

        let audited_graph = session
            .metadata()
            .get(SESSION_TRANSCRIPT_HISTORY_STATE_KEY)
            .cloned()
            .expect("rewrite installs audited graph");
        session.push(user(&format!("follow-up {round} {BODY}")));
        session.push(assistant(&format!("reply {round} {BODY}")));
        assert_eq!(
            session.metadata().get(SESSION_TRANSCRIPT_HISTORY_STATE_KEY),
            Some(&audited_graph),
            "ordinary appends must not manufacture retained graph revisions"
        );
    }

    let history = history_value(&session);
    assert_compact_graph_wire(&history, 8);
    for (index, edge) in edges(&history).iter().enumerate() {
        assert_eq!(
            edge["rewrite"]["replacement"]
                .as_array()
                .expect("replacement array")
                .len(),
            1,
            "edge {index} must retain only its one-message edit"
        );
    }

    let state = state_of(&session);
    assert_eq!(state.commit_count(), 8);
    assert_eq!(
        state.retained_revision_count(),
        17,
        "one anchor plus two exact endpoints per occurrence"
    );
    for (revision, messages) in &lived {
        assert_materialized_messages(&state, revision, messages);
    }

    let decoded: TranscriptHistoryState =
        serde_json::from_value(history.clone()).expect("compact graph decodes");
    assert_eq!(
        serde_json::to_value(&decoded).expect("compact graph re-encodes"),
        history,
        "current compact wire must round-trip exactly"
    );

    let transcript = transcript_bytes(&session);
    let retained = serde_json::to_vec(&history)
        .expect("history serializes")
        .len();
    assert!(
        retained < transcript * 2,
        "eight stacked rewrites must cost about one transcript plus bounded \
         edges: {retained} B against a {transcript} B transcript"
    );
}

fn flip_text(value: &mut serde_json::Value) {
    let message = value
        .as_array_mut()
        .expect("message array")
        .iter_mut()
        .find(|message| message["role"] == "user" && message["content"].is_string())
        .expect("array carries a text user message");
    let text = message["content"]
        .as_str()
        .expect("text user message content")
        .to_string();
    message["content"] = serde_json::Value::String(format!("{text} flipped"));
}

#[test]
fn tampered_compact_anchor_or_edge_refuses_typed_decode() {
    let mut session = session_with_turns(6);
    rewrite(
        &mut session,
        3,
        4,
        vec![user(&format!("edited question {BODY}"))],
        "mid-transcript-edit",
    );
    let history = history_value(&session);
    assert_compact_graph_wire(&history, 1);
    serde_json::from_value::<TranscriptHistoryState>(history.clone())
        .expect("unmodified compact graph decodes");

    let mut anchor_flip = history.clone();
    flip_text(&mut anchor_flip["anchor"]["messages"]);
    serde_json::from_value::<TranscriptHistoryState>(anchor_flip)
        .expect_err("a flipped anchor must refuse typed decode");

    let mut edge_flip = history;
    flip_text(&mut edge_flip["edges"][0]["rewrite"]["replacement"]);
    serde_json::from_value::<TranscriptHistoryState>(edge_flip)
        .expect_err("a flipped rewrite replacement must refuse typed decode");
}

#[test]
fn an_edge_with_an_unresolvable_base_refuses_typed_decode() {
    let mut session = session_with_turns(6);
    rewrite(
        &mut session,
        3,
        4,
        vec![user(&format!("edited question {BODY}"))],
        "mid-transcript-edit",
    );
    let mut history = history_value(&session);
    assert_compact_graph_wire(&history, 1);
    history["edges"][0]["base_revision"] =
        serde_json::Value::String(format!("sha256:{}", "0".repeat(64)));

    serde_json::from_value::<TranscriptHistoryState>(history)
        .expect_err("an edge with an unresolvable base must refuse typed decode");
}

#[test]
fn index_zero_rewrite_retains_one_message() {
    let mut session = session_with_turns(200);
    let transcript = transcript_bytes(&session);

    rewrite(
        &mut session,
        0,
        1,
        vec![Message::User(UserMessage::text(format!(
            "replaced row {BODY}"
        )))],
        "index-zero-replacement",
    );

    let history = history_value(&session);
    assert_compact_graph_wire(&history, 1);
    let edge = &edges(&history)[0];
    assert_eq!(edge["rewrite"]["at"], serde_json::Value::from(0u64));
    assert_eq!(
        edge["rewrite"]["replacement"]
            .as_array()
            .expect("replacement is an array")
            .len(),
        1,
        "an index-zero rewrite retains exactly the replaced message"
    );

    let retained = history_bytes(&session);
    assert!(
        retained < transcript + 4096,
        "an index-zero rewrite must stay O(edit): {retained} B against a \
         {transcript} B transcript"
    );
}

#[test]
fn retained_revisions_read_back_their_exact_transcripts() {
    let mut session = session_with_turns(60);
    let mut expected: Vec<(String, serde_json::Value)> = Vec::new();

    for round in 0..4 {
        expected.push((
            session.transcript_revision().expect("live revision"),
            serde_json::to_value(session.messages()).expect("transcript serializes"),
        ));
        let target = 20 + round * 3;
        let commit = rewrite(
            &mut session,
            target,
            target + 1,
            vec![user(&format!("edited {round} {BODY}"))],
            "read-back-edit",
        );
        expected.push((
            commit.revision,
            serde_json::to_value(session.messages()).expect("rewritten transcript serializes"),
        ));
        session.push(user(&format!("after {round} {BODY}")));
        session.push(assistant(&format!("reply {round} {BODY}")));
    }
    assert_compact_graph_wire(&history_value(&session), 4);

    let assert_reads_back = |session: &Session, what: &str| {
        for (revision, messages) in &expected {
            let restored = session
                .transcript_revision_messages(revision)
                .expect("revision read decodes")
                .unwrap_or_else(|| panic!("{what}: revision {revision} must still be retained"));
            assert_eq!(
                serde_json::to_value(restored).expect("restored transcript serializes"),
                *messages,
                "{what}: revision {revision} must read back byte-faithfully"
            );
        }
    };

    assert_reads_back(&session, "live session");

    let raw = serde_json::to_vec(&session).expect("durable document");
    let restored: Session = serde_json::from_slice(&raw).expect("durable document decodes");
    assert_reads_back(&restored, "after a durable round trip");
    assert_eq!(
        history_value(&restored),
        history_value(&session),
        "durable round-trip must preserve compact graph bytes"
    );
}

fn materialized_bodies(session: &Session) -> Vec<(String, serde_json::Value)> {
    let state = state_of(session);
    let mut revisions = vec![state.anchor().revision().to_string()];
    for commit in state.commits() {
        for revision in [&commit.parent_revision, &commit.revision] {
            if !revisions.contains(revision) {
                revisions.push(revision.clone());
            }
        }
    }
    revisions
        .into_iter()
        .map(|revision| {
            let body = state
                .materialize_revision(&revision)
                .unwrap_or_else(|error| panic!("revision {revision} materializes: {error}"));
            (
                revision,
                serde_json::to_value(body.messages).expect("retained body serializes"),
            )
        })
        .collect()
}

#[test]
fn compact_edges_are_byte_faithful_over_rich_content() {
    let mut session = Session::new();
    session.append_system_message(format!("system prompt {BODY}"));

    let mut pictured = UserMessage::text(format!("look at this {BODY}"));
    pictured.content.push(ContentBlock::Image {
        media_type: "image/png".to_string(),
        data: ImageData::Inline {
            data: "iVBORw0KGgoAAAANSUhEUg==".to_string(),
        },
    });
    pictured.identity = TranscriptMessageIdentity::default().with_run_id(RunId::new());
    pictured.created_at = chrono::DateTime::from_timestamp(1_700_000_000, 123_456_789)
        .expect("fixed message timestamp");
    session.push(Message::User(pictured));

    session.push(Message::BlockAssistant(BlockAssistantMessage::new(
        vec![
            AssistantBlock::Text {
                text: format!("calling a tool {BODY}"),
                meta: None,
            },
            AssistantBlock::ToolUse {
                id: "tc_rich_1".to_string(),
                name: "read_file".to_string(),
                args: serde_json::value::RawValue::from_string(
                    r#"{"path":"/tmp/a.txt","opts":{"deep":true,"n":3}}"#.to_string(),
                )
                .expect("raw tool arguments"),
                meta: None,
            },
        ],
        StopReason::ToolUse,
    )));
    session.push(Message::tool_results(vec![ToolResult::with_blocks(
        "tc_rich_1".to_string(),
        vec![
            ContentBlock::Text {
                text: format!("file contents {BODY}"),
            },
            ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: ImageData::Inline {
                    data: "iVBORw0KGgoAAAANSUhEUgAAAAE=".to_string(),
                },
            },
            ContentBlock::Structured {
                data: serde_json::value::RawValue::from_string(
                    r#"{"z":1,"a":{"y":2,"x":3}}"#.to_string(),
                )
                .expect("raw structured tool result"),
            },
        ],
        false,
    )]));
    session.push(assistant(&format!("done {BODY}")));
    session.push(user(&format!("thanks {BODY}")));

    let end = session.messages().len();
    rewrite(
        &mut session,
        end - 1,
        end,
        vec![user(&format!("thanks, revised {BODY}"))],
        "rich-content-edit",
    );

    let history = history_value(&session);
    assert_compact_graph_wire(&history, 1);
    let state = state_of(&session);
    let wire_anchor_messages: Vec<Message> =
        serde_json::from_value(history["anchor"]["messages"].clone())
            .expect("wire anchor messages decode");
    let direct_anchor_rows = state
        .anchor()
        .messages()
        .iter()
        .map(|message| serde_json::to_vec(message).expect("direct anchor row serializes"))
        .collect::<Vec<_>>();
    let buffered_anchor_rows = wire_anchor_messages
        .iter()
        .map(|message| serde_json::to_vec(message).expect("buffered anchor row serializes"))
        .collect::<Vec<_>>();
    assert_eq!(
        direct_anchor_rows, buffered_anchor_rows,
        "durable message serialization must canonicalize opaque JSON so exact row \
         lineage survives metadata buffering"
    );
    assert_eq!(
        transcript_messages_digest(state.anchor().messages()).expect("anchor digest"),
        state.anchor().revision(),
        "fixture construction must bind the rich anchor to its semantic identity"
    );
    assert_eq!(
        transcript_messages_digest(&wire_anchor_messages).expect("metadata-buffered anchor digest"),
        state.anchor().revision(),
        "opaque JSON key order normalized by metadata buffering must not change \
         the anchor's semantic identity"
    );
    let before = materialized_bodies(&session);
    assert!(
        {
            let materialized =
                serde_json::to_string(&before).expect("materialized bodies serialize");
            materialized.contains("tc_rich_1")
                && materialized.contains(r#""a":{"x":3,"y":2},"z":1"#)
        },
        "fixture must actually retain the rich content it claims to test"
    );

    let raw = serde_json::to_vec(&session).expect("durable document");
    let restored: Session = serde_json::from_slice(&raw).expect("durable document decodes");

    assert_eq!(
        materialized_bodies(&restored),
        before,
        "compact edges must reproduce retained bodies byte-faithfully, including \
         fields the transcript digest intentionally excludes"
    );
    assert_eq!(
        history_value(&restored),
        history,
        "re-encoding the decoded graph must reproduce the same durable bytes"
    );
}

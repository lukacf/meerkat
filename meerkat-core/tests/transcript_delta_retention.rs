//! Delta retention for the durable transcript revision graph.
//!
//! `session_transcript_history_state_v1` used to retain every past revision
//! IN FULL, INLINE, forever: a rewrite pushed the parent transcript and the
//! rewritten transcript as two complete message vectors, so a one-message edit
//! to a 371-message transcript retained two full copies. Measured on a real
//! production dump: one identity's durable document was 82.3 MB carrying
//! 1.29 MB of live conversation, 98 retained revisions averaging 847 KB;
//! fleet-wide 490.6 MB of documents for 6.55 MB of conversation.
//!
//! The durable form is now an anchor plus a chain of inverse splices: the
//! first retained body carries its messages, every later body carries only
//! the span that distinguishes it from an earlier one. Retention costs
//! `O(edit)` per revision instead of `O(document)`.
//!
//! These tests pin that through the real seams — `commit_transcript_rewrite`,
//! `Session` serialization, `Session` deserialization — never by assembling a
//! graph by hand. Two facts are asserted together throughout: the new bytes
//! are small, AND the full-body spelling those same typed values would have
//! produced is large. A bound that only the fix can satisfy is the only bound
//! worth asserting.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use meerkat_core::lifecycle::RunId;
use meerkat_core::service::{TranscriptRewriteReason, TranscriptRewriteSelection};
use meerkat_core::types::{
    AssistantBlock, BlockAssistantMessage, ContentBlock, ImageData, Message, StopReason,
    ToolResult, TranscriptMessageIdentity, UserMessage,
};
use meerkat_core::{SESSION_TRANSCRIPT_HISTORY_STATE_KEY, Session, TranscriptHistoryState};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const BODY: &str = "with enough body text on every message that a full retained \
                    copy is unmistakably larger than one edited span";

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

/// A session with `turns` prior conversational turns and a system prompt.
fn session_with_turns(turns: usize) -> Session {
    let mut session = Session::new();
    session.append_system_message(format!("system prompt {BODY}"));
    for index in 0..turns {
        session.push(user(&format!("question {index} {BODY}")));
        session.push(assistant(&format!("answer {index} {BODY}")));
    }
    session
}

fn rewrite(session: &mut Session, start: usize, end: usize, replacement: Vec<Message>, why: &str) {
    session
        .commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start, end },
            replacement,
            TranscriptRewriteReason::new(why),
            Some("delta-retention-test".to_string()),
            None,
        )
        .expect("rewrite should commit");
}

// ---------------------------------------------------------------------------
// Measurement helpers
// ---------------------------------------------------------------------------

/// The durable transcript-graph value exactly as it sits in session metadata.
fn history_value(session: &Session) -> serde_json::Value {
    serde_json::to_value(session).expect("session serializes")["metadata"]
        [SESSION_TRANSCRIPT_HISTORY_STATE_KEY]
        .clone()
}

/// Bytes the retained graph costs in the durable document.
fn history_bytes(session: &Session) -> usize {
    serde_json::to_vec(&history_value(session))
        .expect("history value serializes")
        .len()
}

/// Bytes one copy of the live transcript costs.
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

/// The graph value the PRE-DELTA writer would have produced for these exact
/// typed values.
///
/// Not hand-forged content: `TranscriptRevisionBody`'s own `Serialize` IS the
/// old per-body spelling (`{revision, parent_revision?, messages, created_at}`),
/// and it is still accepted on decode. This is the counterfactual the byte
/// bounds below are measured against, and the input to the reconstruction
/// equality proof.
fn full_body_encoding(state: &TranscriptHistoryState) -> serde_json::Value {
    let mut value = serde_json::Map::new();
    value.insert(
        "head".to_string(),
        serde_json::Value::String(state.head.clone()),
    );
    if !state.commits.is_empty() {
        value.insert(
            "commits".to_string(),
            serde_json::to_value(&state.commits).expect("commits serialize"),
        );
    }
    if !state.revisions.is_empty() {
        value.insert(
            "revisions".to_string(),
            serde_json::Value::Array(
                state
                    .revisions
                    .iter()
                    .map(|body| serde_json::to_value(body).expect("full body serializes"))
                    .collect(),
            ),
        );
    }
    if state.digest_format != 0 {
        value.insert(
            "digest_format".to_string(),
            serde_json::Value::from(state.digest_format),
        );
    }
    serde_json::Value::Object(value)
}

fn entries(history: &serde_json::Value) -> &[serde_json::Value] {
    history["revisions"]
        .as_array()
        .expect("durable graph carries a revisions array")
}

/// Guard against a vacuous pass: every byte claim below is only meaningful if
/// the chain actually spliced something.
fn assert_chain_is_spliced(history: &serde_json::Value) {
    let entries = entries(history);
    assert!(
        entries.len() >= 2,
        "fixture must retain more than the anchor"
    );
    assert!(
        entries[0].get("messages").is_some() && entries[0].get("rebase").is_none(),
        "the first entry is the chain anchor and carries full messages: {:?}",
        entries[0]
    );
    for (index, entry) in entries.iter().enumerate().skip(1) {
        assert!(
            entry.get("rebase").is_some() && entry.get("messages").is_none(),
            "entry {index} must be an inverse splice, not a second full copy: {entry:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 1. One edited message costs one edited message
// ---------------------------------------------------------------------------

/// A single-message rewrite of a ~400-message transcript must retain bytes
/// proportional to the EDIT. The pre-delta writer retained two complete
/// transcripts here; the bound asserted is one that only the delta form can
/// satisfy, and the counterfactual is measured in the same test so the bound
/// cannot silently become trivial.
#[test]
fn single_message_rewrite_retains_the_edit_not_the_document() {
    let mut session = session_with_turns(200);
    assert_eq!(session.messages().len(), 401);
    let transcript = transcript_bytes(&session);

    rewrite(
        &mut session,
        200,
        201,
        vec![user(&format!("edited question {BODY}"))],
        "single-message-edit",
    );

    let history = history_value(&session);
    assert_chain_is_spliced(&history);
    let retained = history_bytes(&session);

    // The splice names the anchor and the exact span that moved.
    let entries = entries(&history);
    assert_eq!(entries.len(), 2, "one rewrite retains its two endpoints");
    let rebase = &entries[1]["rebase"];
    assert_eq!(
        rebase["base"], entries[0]["revision"],
        "the splice must rebase on the chain anchor"
    );
    assert_eq!(rebase["at"], serde_json::Value::from(200u64));
    assert_eq!(rebase["removed"], serde_json::Value::from(1u64));
    assert_eq!(
        rebase["insert"]
            .as_array()
            .expect("splice payload is an array")
            .len(),
        1,
        "a one-message rewrite carries exactly one message"
    );

    // What the old representation would have cost for this same graph.
    let counterfactual = serde_json::to_vec(&full_body_encoding(&state_of(&session)))
        .expect("counterfactual serializes")
        .len();
    assert!(
        counterfactual >= transcript * 2,
        "the pre-delta spelling must carry two full transcripts \
         (counterfactual {counterfactual} B, one transcript {transcript} B)"
    );

    // The delta form: one anchor transcript plus the edit and its commit.
    assert!(
        retained < transcript + 4096,
        "a one-message rewrite must retain one transcript plus the edit, \
         got {retained} B against a {transcript} B transcript \
         (pre-delta spelling: {counterfactual} B)"
    );
}

// ---------------------------------------------------------------------------
// 2. Stacked rewrites reconstruct exactly what the full-body form held
// ---------------------------------------------------------------------------

/// Eight stacked rewrites, each separated by ordinary appends. Every rewrite
/// promotes its live parent and new result into audited endpoint bodies; the
/// intervening appends themselves must leave the graph bytes untouched. The
/// delta chain must decode to precisely the graph the full-body spelling of
/// the same typed values decodes to — reconstruction is not "close enough",
/// it is equality — while costing a small multiple of ONE transcript rather
/// than one per retained revision.
#[test]
fn stacked_rewrites_reconstruct_the_full_body_graph_exactly() {
    let mut session = session_with_turns(120);
    // Captured OUTSIDE the graph, before anything is encoded: the transcript
    // each revision names, as it actually existed. This is what a full-body
    // retention would have stored verbatim, so replaying the splice chain back
    // to these exact bytes is the reconstruction proof — nothing here is
    // derived from the encoding under test.
    let mut lived: Vec<(String, serde_json::Value)> = Vec::new();
    for round in 0..8 {
        lived.push((
            session.transcript_revision().expect("live revision"),
            serde_json::to_value(session.messages()).expect("live transcript serializes"),
        ));
        let target = 40 + round * 4;
        rewrite(
            &mut session,
            target,
            target + 1,
            vec![user(&format!("stacked edit {round} {BODY}"))],
            "stacked-edit",
        );
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

    let state = state_of(&session);
    assert_eq!(state.commits.len(), 8, "eight audited rewrites");
    assert!(
        state.revisions.len() >= 9,
        "stacked rewrites must retain their endpoints, got {}",
        state.revisions.len()
    );

    let delta = history_value(&session);
    assert_chain_is_spliced(&delta);
    let full = full_body_encoding(&state);

    let from_delta: TranscriptHistoryState =
        serde_json::from_value(delta.clone()).expect("delta chain decodes");
    let from_full: TranscriptHistoryState =
        serde_json::from_value(full.clone()).expect("full-body graph decodes");

    // Reconstruction: every revision the session actually lived through comes
    // back out of the splice chain byte for byte.
    for (revision, messages) in &lived {
        let body = from_delta
            .revisions
            .iter()
            .find(|body| body.revision == *revision)
            .unwrap_or_else(|| panic!("revision {revision} must still be retained"));
        assert_eq!(
            serde_json::to_value(&body.messages).expect("retained body serializes"),
            *messages,
            "revision {revision} must replay to the transcript it identifies"
        );
    }

    // Both spellings of the same graph decode to the same graph, and the chain
    // round-trips through its own encoding unchanged.
    assert_eq!(
        full_body_encoding(&from_delta),
        full_body_encoding(&from_full),
        "the splice chain and the full-body spelling must decode identically"
    );
    assert_eq!(
        serde_json::to_value(&from_delta).expect("re-encode"),
        delta,
        "re-encoding a decoded chain must reproduce the same durable bytes"
    );

    let transcript = transcript_bytes(&session);
    let delta_len = serde_json::to_vec(&delta).expect("delta serializes").len();
    let full_len = serde_json::to_vec(&full).expect("full serializes").len();
    assert!(
        full_len > transcript * 8,
        "the pre-delta spelling must carry a transcript per retained revision \
         ({full_len} B against a {transcript} B transcript)"
    );
    assert!(
        delta_len < transcript * 2,
        "eight stacked rewrites must cost about one transcript plus eight edits, \
         got {delta_len} B against a {transcript} B transcript \
         (pre-delta spelling: {full_len} B)"
    );
}

// ---------------------------------------------------------------------------
// 3. Content addressing still refuses a tampered retained body
// ---------------------------------------------------------------------------

/// Flip one message's text inside `value`, wherever the delta chain put it.
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

/// Both halves of the chain must remain content-addressed. Flipping bytes in
/// the anchor corrupts every body spliced off it; flipping bytes in a splice
/// payload corrupts exactly the body that splice reconstructs. Neither may
/// decode.
#[test]
fn tampered_retained_bodies_refuse_typed_at_decode() {
    let mut session = session_with_turns(6);
    // A user-message edit, so the splice payload is a text user message the
    // flip below can reach — the anchor carries them too.
    rewrite(
        &mut session,
        3,
        4,
        vec![user(&format!("edited question {BODY}"))],
        "mid-transcript-edit",
    );
    let document = serde_json::to_value(&session).expect("session serializes");
    assert_chain_is_spliced(&document["metadata"][SESSION_TRANSCRIPT_HISTORY_STATE_KEY]);

    // Control: the unmutated document decodes, so each refusal below is
    // caused by its mutation and nothing else.
    serde_json::from_value::<Session>(document.clone()).expect("unmutated document decodes");

    let mut anchor_flip = document.clone();
    {
        let entry =
            &mut anchor_flip["metadata"][SESSION_TRANSCRIPT_HISTORY_STATE_KEY]["revisions"][0usize];
        flip_text(&mut entry["messages"]);
    }
    let error = serde_json::from_value::<Session>(anchor_flip)
        .expect_err("a flipped chain anchor must refuse typed at decode");
    assert!(
        error.to_string().contains("transcript revision body"),
        "anchor flip must name the revision-body digest mismatch, got: {error}"
    );

    let mut splice_flip = document;
    {
        let entry =
            &mut splice_flip["metadata"][SESSION_TRANSCRIPT_HISTORY_STATE_KEY]["revisions"][1usize];
        assert!(
            entry["rebase"]["insert"]
                .as_array()
                .is_some_and(|insert| !insert.is_empty()),
            "fixture must carry a non-empty splice payload to flip"
        );
        flip_text(&mut entry["rebase"]["insert"]);
    }
    let error = serde_json::from_value::<Session>(splice_flip)
        .expect_err("a flipped splice payload must refuse typed at decode");
    assert!(
        error.to_string().contains("transcript revision body"),
        "splice flip must name the revision-body digest mismatch, got: {error}"
    );
}

/// Reconstruction is a total function of what the array actually carries: a
/// splice whose base no earlier entry materializes is unreadable, never
/// silently empty and never resolved from somewhere else.
#[test]
fn a_splice_with_an_unresolvable_base_refuses_typed() {
    let mut session = session_with_turns(6);
    rewrite(
        &mut session,
        3,
        4,
        vec![user(&format!("edited question {BODY}"))],
        "mid-transcript-edit",
    );
    let mut history = history_value(&session);
    assert!(
        history["revisions"][1]["rebase"].is_object(),
        "entry 1 must be a splice for this test to mean anything"
    );
    history["revisions"][1]["rebase"]["base"] =
        serde_json::Value::String(format!("sha256:{}", "0".repeat(64)));

    let error = serde_json::from_value::<TranscriptHistoryState>(history)
        .expect_err("an unresolvable base must refuse typed");
    assert!(
        error.to_string().contains("rebases on"),
        "refusal must name the unresolvable base, got: {error}"
    );
}

// ---------------------------------------------------------------------------
// 4. The pathological case: an index-0 rewrite shares no prefix
// ---------------------------------------------------------------------------

/// An index-zero rewrite shares no prefix with its parent, so a prefix-only
/// delta would degenerate to a full copy. The splice is prefix AND suffix
/// anchored, so the retained payload is the one replaced message.
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
    assert_chain_is_spliced(&history);
    let entries = entries(&history);
    assert_eq!(entries.len(), 2, "one rewrite retains its two endpoints");

    let rebase = &entries[1]["rebase"];
    assert_eq!(rebase["at"], serde_json::Value::from(0u64));
    assert_eq!(rebase["removed"], serde_json::Value::from(1u64));
    assert_eq!(
        rebase["insert"]
            .as_array()
            .expect("splice payload is an array")
            .len(),
        1,
        "an index-0 single-message rewrite retains exactly the replaced message"
    );

    let retained = history_bytes(&session);
    assert!(
        retained < transcript + 4096,
        "an index-0 rewrite must stay O(edit): {retained} B against a \
         {transcript} B transcript"
    );
}

// ---------------------------------------------------------------------------
// 5. Reading an old revision back still returns that exact transcript
// ---------------------------------------------------------------------------

/// The read path is what retention is FOR. After four stacked rewrites, and
/// again after a full durable round trip, every retained revision must hand
/// back the exact transcript it identifies — reconstructed from the splice
/// chain, compared as serialized bytes rather than by the same `PartialEq` the
/// encoder uses to find the splice.
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
        rewrite(
            &mut session,
            target,
            target + 1,
            vec![user(&format!("edited {round} {BODY}"))],
            "read-back-edit",
        );
        session.push(user(&format!("after {round} {BODY}")));
        session.push(assistant(&format!("reply {round} {BODY}")));
    }
    assert_chain_is_spliced(&history_value(&session));

    let assert_reads_back = |session: &Session, what: &str| {
        for (revision, messages) in &expected {
            let restored = session
                .transcript_revision_messages(revision)
                .expect("revision read decodes")
                .unwrap_or_else(|| panic!("{what}: revision {revision} must still be retained"));
            assert_eq!(
                serde_json::to_value(&restored).expect("restored transcript serializes"),
                *messages,
                "{what}: revision {revision} must read back its exact transcript"
            );
        }
    };

    assert_reads_back(&session, "live session");

    let raw = serde_json::to_vec(&session).expect("durable document");
    let restored: Session = serde_json::from_slice(&raw).expect("durable document decodes");
    assert_reads_back(&restored, "after a durable round trip");
}

// ---------------------------------------------------------------------------
// 6. The splice is byte-faithful over rich content
// ---------------------------------------------------------------------------

/// The encoder decides "these two messages are the same" with `Message`'s
/// `PartialEq`, and `AssistantBlock`'s is hand-written (tool-use arguments
/// compare by their raw JSON bytes). If that equality were ever weaker than
/// byte equality, a splice would elide a difference and reconstruction would
/// lose it — invisibly, because the transcript digest erases exactly the
/// fields most at risk (message `created_at`, transcript identity, image
/// inline-vs-blob form). This round trip carries all of them.
#[test]
fn splices_are_byte_faithful_over_rich_content() {
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
        ],
        false,
    )]));
    session.push(assistant(&format!("done {BODY}")));
    session.push(user(&format!("thanks {BODY}")));

    // Rewrite the trailing user message: the tool-use / tool-results pair
    // stays intact, and every rich message lands in the shared span the
    // splice elides — which is exactly what must survive verbatim.
    let end = session.messages().len();
    rewrite(
        &mut session,
        end - 1,
        end,
        vec![user(&format!("thanks, revised {BODY}"))],
        "rich-content-edit",
    );

    let history = history_value(&session);
    assert_chain_is_spliced(&history);

    let bodies = |session: &Session| {
        state_of(session)
            .revisions
            .into_iter()
            .map(|body| {
                (
                    body.revision,
                    serde_json::to_value(&body.messages).expect("retained body serializes"),
                )
            })
            .collect::<Vec<_>>()
    };
    let before = bodies(&session);
    assert!(
        serde_json::to_string(&before)
            .expect("retained bodies serialize")
            .contains("tc_rich_1"),
        "fixture must actually retain the rich content it claims to test"
    );

    let raw = serde_json::to_vec(&session).expect("durable document");
    let restored: Session = serde_json::from_slice(&raw).expect("durable document decodes");

    assert_eq!(
        bodies(&restored),
        before,
        "splicing must reproduce every retained body byte for byte, including \
         the fields the transcript digest erases"
    );
    assert_eq!(
        history_value(&restored),
        history,
        "re-encoding the decoded chain must reproduce the same durable bytes"
    );
}

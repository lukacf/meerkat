//! Bounded transcript-history retention.
//!
//! A session's rewrite graph used to retain every message the session ever
//! produced: one pre-rewrite anchor plus the parent advance of every rewrite.
//! Compaction shrank the live transcript but grew the document. Retention
//! re-anchors the graph after each compaction at the oldest retained
//! occurrence: commits and rolling digests stay, older bodies go.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use meerkat_core::types::{Message, UserMessage};
use meerkat_core::{
    Session, TranscriptEditError, TranscriptHistoryRetention, TranscriptRewriteReason,
    TranscriptRewriteSelection,
};

/// Written by the release/0.8.51 code before retention existed: six
/// full-transcript rewrites, then one live-tail message.
const PRE_RETENTION_FIXTURE: &str = include_str!("fixtures/pre_retention_six_rewrite_session.json");

fn retention(count: usize) -> TranscriptHistoryRetention {
    TranscriptHistoryRetention::from_count(count).expect("non-zero retention")
}

/// One compaction-shaped cycle: append a turn's worth of rows, then rewrite
/// the whole transcript down to one summary row.
fn compaction_cycle(session: &mut Session, cycle: usize) {
    for turn in 0..8 {
        session.push(Message::User(UserMessage::text(format!(
            "cycle {cycle} turn {turn}: {}",
            "history ".repeat(64)
        ))));
    }
    let end = session.messages().len();
    session
        .commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 0, end },
            vec![Message::User(UserMessage::text(format!(
                "summary of cycle {cycle}"
            )))],
            TranscriptRewriteReason::new("compaction"),
            Some("retention-test".to_string()),
            None,
        )
        .expect("rewrite");
}

fn persisted_bytes(session: &Session) -> usize {
    session.to_persisted_bytes().expect("encode").len()
}

fn reload(session: &Session) -> Session {
    serde_json::from_slice(&session.to_persisted_bytes().expect("encode")).expect("decode")
}

fn graph(session: &Session) -> meerkat_core::TranscriptHistoryState {
    session
        .transcript_history_state()
        .expect("graph decodes")
        .expect("graph present")
}

#[test]
fn whole_blob_document_bytes_stay_bounded_across_many_compaction_cycles() {
    let mut retained = Session::new();
    let mut unbounded = Session::new();
    let mut retained_sizes = Vec::new();
    for cycle in 0..60 {
        compaction_cycle(&mut retained, cycle);
        retained
            .retire_transcript_history(retention(3))
            .expect("retire");
        compaction_cycle(&mut unbounded, cycle);
        retained_sizes.push(persisted_bytes(&retained));
    }

    // Once the window is full, every whole-blob save is the same size: the
    // anchor, three retained occurrences and the small retired commit list.
    let steady = retained_sizes[9];
    let commit_overhead_slack = 64 * 1024;
    for (cycle, size) in retained_sizes.iter().enumerate().skip(10) {
        assert!(
            *size <= steady + commit_overhead_slack,
            "cycle {cycle}: whole-blob document grew to {size} bytes (steady state {steady})"
        );
    }
    let unbounded_size = persisted_bytes(&unbounded);
    assert!(
        unbounded_size > 5 * retained_sizes[59],
        "the un-retained document ({unbounded_size} bytes) should dwarf the retained one ({})",
        retained_sizes[59]
    );

    let state = graph(&retained);
    assert_eq!(state.commit_count(), 60);
    assert_eq!(state.retired_count(), 57);
    assert_eq!(
        state
            .commits()
            .map(|commit| commit.rewrite_generation)
            .collect::<Vec<_>>(),
        (1..=60).collect::<Vec<u64>>(),
        "every rewrite commit stays as audit authority"
    );
}

#[test]
fn retirement_keeps_the_rolling_rewrite_and_graph_identity() {
    let mut retained = Session::new();
    for cycle in 0..7 {
        compaction_cycle(&mut retained, cycle);
    }
    let whole = graph(&retained);
    assert_eq!(
        retained
            .retire_transcript_history(retention(2))
            .expect("retire"),
        5
    );
    let anchored = graph(&retained);
    assert_eq!(anchored.graph_prefix(), whole.graph_prefix());
    assert_eq!(anchored.rewrite_prefix(), whole.rewrite_prefix());
    assert_eq!(anchored.head(), whole.head());
    assert_eq!(
        anchored.commits().collect::<Vec<_>>(),
        whole.commits().collect::<Vec<_>>()
    );
    assert_eq!(
        anchored.oldest_retained_revision(),
        whole.commit(4).unwrap().revision
    );

    // Retained occurrences materialize exactly as before.
    for index in 5..7 {
        let commit = whole.commit(index).unwrap();
        assert_eq!(
            anchored
                .materialize_revision(&commit.revision)
                .unwrap()
                .messages,
            whole
                .materialize_revision(&commit.revision)
                .unwrap()
                .messages
        );
    }

    // The re-anchored document reloads and keeps extending.
    let mut reloaded = reload(&retained);
    assert_eq!(graph(&reloaded).graph_prefix(), whole.graph_prefix());
    compaction_cycle(&mut reloaded, 7);
    assert_eq!(graph(&reloaded).commit_count(), 8);
    assert_eq!(graph(&reload(&reloaded)).commit_count(), 8);
}

#[test]
fn retired_revisions_are_refused_naming_the_oldest_retained_revision() {
    let mut session = Session::new();
    for cycle in 0..6 {
        compaction_cycle(&mut session, cycle);
    }
    let whole = graph(&session);
    session
        .retire_transcript_history(retention(2))
        .expect("retire");
    let state = graph(&session);
    let oldest = state.oldest_retained_revision().to_string();
    let retired = whole.commit(1).unwrap().revision.clone();
    assert!(state.is_retired_revision(&retired));

    let assert_refusal = |error: TranscriptEditError| match error {
        TranscriptEditError::TranscriptRevisionRetired {
            revision,
            oldest_retained_revision,
            retired_rewrites,
        } => {
            assert_eq!(revision, retired);
            assert_eq!(oldest_retained_revision, oldest);
            assert_eq!(retired_rewrites, 4);
        }
        other => panic!("expected a typed retired-revision refusal, got {other:?}"),
    };

    assert_refusal(state.materialize_revision(&retired).unwrap_err());
    assert_refusal(
        session
            .retained_transcript_revision_rows(&retired)
            .unwrap_err(),
    );
    let validated = session
        .validated_transcript_history_state()
        .unwrap()
        .unwrap();
    assert_refusal(validated.project_at_revision(&retired).unwrap_err());
    let retired_commit = whole.commit(1).unwrap();
    assert!(matches!(
        validated.materialize_rewrite_child(retired_commit),
        Err(TranscriptEditError::TranscriptRevisionRetired { .. })
    ));
    assert!(matches!(
        validated
            .prove_commit_suffix_after(&meerkat_core::TranscriptRewritePrefixAccumulator::empty()),
        Err(TranscriptEditError::TranscriptRevisionRetired { .. })
    ));

    // Audit receipts need commits and prefixes only, so a receipt can still
    // be proved from inside the retired prefix.
    let receipt = validated
        .audit_receipt_starting_with(retired_commit)
        .expect("receipt from a retired occurrence");
    assert_eq!(receipt.commits().len(), 5);
    assert_eq!(receipt.commits()[0], *retired_commit);

    // At and after the window, reads still work.
    assert!(
        session
            .retained_transcript_revision_rows(&oldest)
            .unwrap()
            .is_some()
    );
    assert!(validated.project_at_revision(state.head()).is_ok());
}

#[test]
fn pre_retention_session_loads_unchanged_and_re_anchors_on_its_next_rewrite() {
    let fixture: serde_json::Value = serde_json::from_str(PRE_RETENTION_FIXTURE).unwrap();
    let mut session: Session = serde_json::from_str(PRE_RETENTION_FIXTURE).expect("old file loads");
    let whole = graph(&session);
    assert_eq!(whole.commit_count(), 6);
    assert_eq!(whole.retired_count(), 0);

    // Loading preserves the history; an explicit write uses the current envelope.
    assert_eq!(fixture["version"], 3);
    let reencoded: serde_json::Value =
        serde_json::from_slice(&session.to_persisted_bytes().unwrap()).unwrap();
    assert_eq!(reencoded["version"], 4);
    let mut expected = fixture;
    expected["version"] = serde_json::json!(4);
    assert_eq!(reencoded, expected);

    // The next compaction re-anchors it under the default retention.
    compaction_cycle(&mut session, 6);
    let retired = session
        .retire_transcript_history(TranscriptHistoryRetention::default())
        .expect("retire");
    assert_eq!(
        retired,
        7 - TranscriptHistoryRetention::DEFAULT_RETAINED_REWRITES
    );
    let reloaded = reload(&session);
    let state = graph(&reloaded);
    assert_eq!(state.commit_count(), 7);
    assert_eq!(state.retired_count(), retired);
    assert_eq!(
        state.commits().take(6).collect::<Vec<_>>(),
        whole.commits().collect::<Vec<_>>(),
        "the old file's commits survive re-anchoring"
    );
    assert!(persisted_bytes(&reloaded) < PRE_RETENTION_FIXTURE.len());
}

#[test]
fn a_tampered_retired_prefix_is_rejected_on_load() {
    let mut session = Session::new();
    for cycle in 0..5 {
        compaction_cycle(&mut session, cycle);
    }
    session
        .retire_transcript_history(retention(2))
        .expect("retire");
    let mut value: serde_json::Value =
        serde_json::from_slice(&session.to_persisted_bytes().unwrap()).unwrap();
    let retired = &mut value["metadata"]["session_transcript_history_state_v1"]["retired"];
    assert!(
        retired.is_object(),
        "a re-anchored graph serializes its retired prefix"
    );

    let mut dropped = value.clone();
    dropped["metadata"]["session_transcript_history_state_v1"]["retired"]["commits"]
        .as_array_mut()
        .unwrap()
        .remove(0);
    assert!(
        serde_json::from_value::<Session>(dropped)
            .and_then(|session| session.transcript_history_state())
            .is_err(),
        "a retired prefix missing a commit must not validate"
    );

    let mut relabelled = value;
    relabelled["metadata"]["session_transcript_history_state_v1"]["retired"]["commits"][0]["reason"] =
        serde_json::json!("forged");
    assert!(
        serde_json::from_value::<Session>(relabelled)
            .and_then(|session| session.transcript_history_state())
            .is_err(),
        "a retired commit that no longer binds the carried rewrite prefix must not validate"
    );
}

#[test]
fn retention_within_the_window_is_a_no_op() {
    let mut session = Session::new();
    for cycle in 0..3 {
        compaction_cycle(&mut session, cycle);
    }
    let before = session.to_persisted_bytes().unwrap();
    assert_eq!(
        session
            .retire_transcript_history(retention(3))
            .expect("retire"),
        0
    );
    assert_eq!(session.to_persisted_bytes().unwrap(), before);
}

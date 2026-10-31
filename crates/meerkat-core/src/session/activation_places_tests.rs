//! Rows for instruction activation rows across same-session rewrites. A
//! rewrite keeps the activation rows when the same rows stand in the same
//! order with the same number of conversation rows before each, whatever
//! their offsets: a keyed prompt update may insert a later version before an
//! activation row, and an ordinary System row before one may be removed.
//! Minting, altering, erasing or reordering an activation row, and moving one
//! across a conversation row, stay refused, for a rewrite and for a restore.
//! A rewrite that moves an activation row to another offset writes its
//! document at envelope 5, since a release that compares activation offsets
//! would refuse its rewrite record.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use super::{InstructionActivationExpectation, InstructionActivationRequest, Session};
use crate::types::{
    InstructionActivationId, InstructionContentDigest, InstructionKey, InstructionNamespace,
    InstructionRevisionId, InstructionRevisionRef, Message, SystemPromptKey, SystemPromptVersion,
    UserMessage,
};
use crate::{
    SystemPromptUpdateRequest, SystemPromptUpdateStatus, TranscriptEditError,
    TranscriptRewriteReason, TranscriptRewriteSelection,
};

fn activation(
    key: &str,
    activation: &str,
    supersedes: Option<&str>,
    body: &str,
) -> InstructionActivationRequest {
    InstructionActivationRequest {
        revision: InstructionRevisionRef {
            namespace: InstructionNamespace::new("app.example").unwrap(),
            key: InstructionKey::new(key).unwrap(),
            revision_id: InstructionRevisionId::new(format!("rev-{activation}")).unwrap(),
            content_sha256: InstructionContentDigest::for_body(body),
        },
        activation_id: InstructionActivationId::new(activation).unwrap(),
        expectation: supersedes.map_or(InstructionActivationExpectation::Absent, |previous| {
            InstructionActivationExpectation::Effective(
                InstructionActivationId::new(previous).unwrap(),
            )
        }),
        supersedes: supersedes.map(|previous| InstructionActivationId::new(previous).unwrap()),
        body: body.to_string(),
    }
}

fn prompt_update(
    expected_version: Option<u64>,
    target_message_index: Option<usize>,
    content: &str,
) -> SystemPromptUpdateRequest {
    SystemPromptUpdateRequest {
        key: SystemPromptKey::new("primary").unwrap(),
        expected_version: expected_version
            .map(|version| SystemPromptVersion::new(version).unwrap()),
        target_message_index,
        content: content.to_string(),
        actor: Some("host-test".to_string()),
        expected_parent_revision: None,
    }
}

fn rewrite(
    session: &mut Session,
    start: usize,
    end: usize,
    replacement: Vec<Message>,
) -> Result<crate::TranscriptRewriteCommit, TranscriptEditError> {
    session.commit_transcript_rewrite(
        TranscriptRewriteSelection::MessageRange { start, end },
        replacement,
        TranscriptRewriteReason::new("activation place test"),
        Some("unit-test".to_string()),
        None,
    )
}

fn envelope_version(session: &Session) -> u64 {
    serde_json::from_slice::<serde_json::Value>(&session.to_persisted_bytes().unwrap())
        .unwrap()["version"]
        .as_u64()
        .unwrap()
}

/// Every rewrite record of the reloaded graph validates with both of its
/// bodies.
fn assert_reloaded_graph_validates(session: &Session) {
    let reloaded = Session::from_persisted_bytes(&session.to_persisted_bytes().unwrap())
        .expect("the reloaded rewrite graph validates");
    assert_eq!(reloaded.messages(), session.messages());
    let history = reloaded
        .validated_transcript_history_state()
        .unwrap()
        .expect("the session holds a rewrite graph");
    for commit in history.commits() {
        let parent = history.materialize_rewrite_parent(commit).unwrap();
        let child = history.materialize_rewrite_child(commit).unwrap();
        crate::TranscriptRewriteRecord::new(commit.clone(), parent, child)
            .expect("the rewrite record validates with both of its bodies");
    }
}

/// `[stale prompt, prompt, activation, user]`: a boot appended a prompt and
/// then an activation, after an earlier boot's prompt.
fn session_with_a_stale_prompt_before_an_activation() -> Session {
    let mut session = Session::new();
    session.append_system_message("A prompt an earlier boot wrote.");
    session.append_system_message("You are a careful assistant.");
    session
        .activate_instruction(activation(
            "tone",
            "act-1",
            None,
            "Answer in a formal tone.",
        ))
        .unwrap();
    session.push(Message::User(UserMessage::text("hello")));
    session
}

/// A keyed system-prompt update inserts each later version right after the
/// current one, so before an activation row that follows the head prompt.
/// The activation row keeps its place among the conversation rows and its
/// order among the activation rows, so both later versions commit, the
/// model reads the newest prompt and then the activation, and every record
/// of the reloaded graph validates.
#[test]
fn update_system_prompt_inserts_a_later_version_before_activation_rows() {
    let mut session = Session::new();
    session.append_system_message("You are a careful assistant.");
    let adopted = session
        .update_system_prompt(prompt_update(None, Some(0), "Prompt version one."))
        .expect("the head prompt is adopted in place");
    assert_eq!(adopted.version, SystemPromptVersion::INITIAL);
    session
        .activate_instruction(activation(
            "tone",
            "act-1",
            None,
            "Answer in a formal tone.",
        ))
        .unwrap();
    session.push(Message::User(UserMessage::text(
        "What is the boiling point?",
    )));
    let activation_row = session.messages()[1].clone();

    for (expected, content) in [(1, "Prompt version two."), (2, "Prompt version three.")] {
        let result = session
            .update_system_prompt(prompt_update(Some(expected), None, content))
            .unwrap_or_else(|error| {
                panic!("a later prompt version commits before the activation row: {error}")
            });
        assert_eq!(result.status, SystemPromptUpdateStatus::Applied);
        assert_eq!(result.version.get(), expected + 1);
    }
    let rows = session.messages().to_vec();
    assert_eq!(
        rows.len(),
        5,
        "three prompt versions, the activation row and the user turn"
    );
    assert_eq!(rows[3], activation_row, "the activation row is unchanged");

    let projected = session.messages_for_model_boundary().unwrap();
    assert!(
        matches!(
            &projected[0],
            Message::System(system) if system.content == "Prompt version three."
        ),
        "the model reads the newest prompt: {projected:?}"
    );
    assert_eq!(
        projected[1], activation_row,
        "and the activation row after it"
    );
    assert_reloaded_graph_validates(&session);
}

/// An ordinary System row before an activation row can be removed: the
/// activation row keeps its place among the conversation rows, only its
/// offset changes.
#[test]
fn a_rewrite_removes_an_ordinary_system_row_before_an_activation_row() {
    let mut session = session_with_a_stale_prompt_before_an_activation();
    let activation_row = session.messages()[2].clone();

    rewrite(&mut session, 0, 1, Vec::new()).unwrap_or_else(|error| {
        panic!("removing an ordinary System row before an activation row commits: {error}")
    });
    assert_eq!(session.messages().len(), 3);
    assert_eq!(
        session.messages()[1],
        activation_row,
        "the activation row is unchanged"
    );
    assert_reloaded_graph_validates(&session);
}

/// An activation row cannot move across a conversation row, in either
/// direction.
#[test]
fn a_rewrite_cannot_move_an_activation_across_a_conversation_row() {
    // [prompt, activation, user]
    let mut session = Session::new();
    session.append_system_message("You are a careful assistant.");
    session
        .activate_instruction(activation(
            "tone",
            "act-1",
            None,
            "Answer in a formal tone.",
        ))
        .unwrap();
    session.push(Message::User(UserMessage::text("hello")));
    let before = session.messages().to_vec();

    let later = rewrite(
        &mut session,
        1,
        3,
        vec![before[2].clone(), before[1].clone()],
    );
    assert!(
        matches!(
            &later,
            Err(TranscriptEditError::InvalidTranscriptShape(message))
                if message.contains("instruction activation")
        ),
        "an activation row cannot move after a conversation row: {later:?}"
    );
    assert_eq!(session.messages(), before.as_slice());

    // [prompt, user, activation]
    let mut session = Session::new();
    session.append_system_message("You are a careful assistant.");
    session.push(Message::User(UserMessage::text("hello")));
    session
        .activate_instruction(activation(
            "tone",
            "act-1",
            None,
            "Answer in a formal tone.",
        ))
        .unwrap();
    let before = session.messages().to_vec();
    let earlier = rewrite(
        &mut session,
        1,
        3,
        vec![before[2].clone(), before[1].clone()],
    );
    assert!(
        matches!(
            &earlier,
            Err(TranscriptEditError::InvalidTranscriptShape(message))
                if message.contains("instruction activation")
        ),
        "an activation row cannot move before a conversation row: {earlier:?}"
    );
    assert_eq!(session.messages(), before.as_slice());
}

/// A rewrite cannot reorder two activation rows of one gap, alter an
/// activation row (any byte), erase one, or mint one.
#[test]
fn a_rewrite_cannot_reorder_alter_or_erase_activation_rows() {
    // [prompt, activation tone, activation style, user]
    let mut session = Session::new();
    session.append_system_message("You are a careful assistant.");
    session
        .activate_instruction(activation(
            "tone",
            "act-1",
            None,
            "Answer in a formal tone.",
        ))
        .unwrap();
    session
        .activate_instruction(activation("style", "act-2", None, "Use short sentences."))
        .unwrap();
    session.push(Message::User(UserMessage::text("hello")));
    let before = session.messages().to_vec();

    let mut altered = before[1].clone();
    let Message::System(system) = &mut altered else {
        panic!("the activation row is a System row");
    };
    system.created_at = chrono::DateTime::<chrono::Utc>::UNIX_EPOCH;

    let mut minted_in = session.clone();
    minted_in
        .activate_instruction(activation("units", "act-3", None, "Use metric units."))
        .unwrap();
    let minted = minted_in.messages().last().cloned().unwrap();

    for (case, start, end, replacement) in [
        ("reorder", 1, 3, vec![before[2].clone(), before[1].clone()]),
        ("alter", 1, 2, vec![altered]),
        ("erase", 1, 2, Vec::new()),
        ("mint", 4, 4, vec![minted]),
    ] {
        let refused = rewrite(&mut session, start, end, replacement);
        assert!(
            matches!(
                &refused,
                Err(TranscriptEditError::InvalidTranscriptShape(_))
            ),
            "{case}: refused: {refused:?}"
        );
        assert_eq!(
            session.messages(),
            before.as_slice(),
            "{case}: nothing changes"
        );
    }
}

/// A restore cannot move an activation row across a conversation row: the
/// revision before a compaction holds more conversation rows before the
/// activation row than the compacted transcript.
#[test]
fn a_restore_cannot_move_an_activation_across_a_conversation_row() {
    // [prompt, user, assistant, activation, user]
    let mut session = Session::new();
    session.append_system_message("You are a careful assistant.");
    session.push(Message::User(UserMessage::text("first turn")));
    session.push(Message::User(UserMessage::text("second turn")));
    session
        .activate_instruction(activation(
            "tone",
            "act-1",
            None,
            "Answer in a formal tone.",
        ))
        .unwrap();
    session.push(Message::User(UserMessage::text("third turn")));
    let before_compaction = session.transcript_revision().unwrap();
    let prompt = session.messages()[0].clone();
    let activation_row = session.messages()[3].clone();
    let last = session.messages()[4].clone();
    session
        .stage_validated_compaction_for_test(
            vec![
                prompt,
                Message::User(UserMessage::compaction_summary("summary")),
                activation_row,
                last,
            ],
            1,
        )
        .unwrap();
    let compacted = session.messages().to_vec();

    let rows = session
        .retained_transcript_revision_rows(&before_compaction)
        .unwrap()
        .expect("the revision before the compaction is retained");
    let refused = session.commit_transcript_revision_restore(
        rows,
        TranscriptRewriteReason::new("restore before the compaction"),
        Some("unit-test".to_string()),
        None,
    );
    assert!(
        matches!(
            &refused,
            Err(TranscriptEditError::InvalidTranscriptShape(_))
        ),
        "the restore is refused: {refused:?}"
    );
    assert_eq!(session.messages(), compacted.as_slice());
}

/// A rewrite that moves an activation row to another offset is written at
/// envelope 5: a release that compares activation offsets would refuse its
/// rewrite record, so it refuses the document typed at its envelope.
#[test]
fn a_rewrite_that_shifts_an_activation_offset_writes_envelope_5() {
    let mut session = session_with_a_stale_prompt_before_an_activation();
    assert_eq!(envelope_version(&session), 4);

    rewrite(&mut session, 0, 1, Vec::new()).unwrap_or_else(|error| {
        panic!("removing an ordinary System row before an activation row commits: {error}")
    });
    assert!(
        session.carries_operator_instruction_facts(),
        "the shifted offset records the envelope-5 marker"
    );
    assert_eq!(session.persisted_envelope_version(), 5);
    assert_eq!(envelope_version(&session), 5);
    let reloaded = Session::from_persisted_bytes(&session.to_persisted_bytes().unwrap()).unwrap();
    assert_eq!(envelope_version(&reloaded), 5, "once 5, always 5");
}

/// A rewrite that leaves every activation row at its offset keeps the
/// document at envelope 4, byte-identical to what released binaries read.
#[test]
fn a_rewrite_that_keeps_every_activation_offset_stays_at_envelope_4() {
    let mut session = session_with_a_stale_prompt_before_an_activation();
    rewrite(
        &mut session,
        0,
        1,
        vec![Message::System(crate::types::SystemMessage::new(
            "A shorter stale prompt.",
        ))],
    )
    .expect("replacing an ordinary System row in place commits");
    assert!(!session.carries_operator_instruction_facts());
    assert_eq!(session.persisted_envelope_version(), 4);
    assert_eq!(envelope_version(&session), 4);
}

/// A rewrite that would move an activation row to another offset, in a
/// session whose stored operator-instruction state does not read, is refused
/// before anything changes: the envelope-5 marker it must record is checked
/// before the commit is installed, so the rewrite is all or nothing.
#[test]
fn a_rewrite_that_cannot_record_envelope_5_changes_nothing() {
    let mut session = session_with_a_stale_prompt_before_an_activation();
    session.set_metadata_unchecked_for_test(
        super::SESSION_INSTRUCTION_STATE_KEY,
        serde_json::json!({ "owner": "a state this release cannot read" }),
    );
    let before = session.messages().to_vec();
    let revision = session.transcript_revision().unwrap();

    let refused = rewrite(&mut session, 0, 1, Vec::new());
    assert!(
        matches!(&refused, Err(TranscriptEditError::HistoryStateMalformed(_))),
        "the rewrite is refused: {refused:?}"
    );
    assert_eq!(
        session.messages(),
        before.as_slice(),
        "the refused rewrite changes no row"
    );
    assert_eq!(
        session.transcript_revision().unwrap(),
        revision,
        "nor the revision"
    );
    assert!(
        session
            .validated_transcript_history_state()
            .unwrap()
            .is_none(),
        "nor records a rewrite"
    );
}

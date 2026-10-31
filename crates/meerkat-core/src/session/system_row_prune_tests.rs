//! Rows for pruning ordinary System rows. One audited rewrite removes the
//! ordinary System rows a host names (each by its index and the digest of
//! its content, as a history read showed it) and, when asked, retires every
//! superseded instruction activation body in the same commit. A named row
//! that is not an ordinary System row, a stale reference, or a removal that
//! would move an operator-instruction entry refuses the whole prune, typed,
//! and writes nothing.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use super::operator_instructions::{
    ActiveOperatorInstruction, InstructionRevision, InstructionSlot,
    OperatorInstructionChangeContent, OperatorInstructionEntryHeader,
    mint_operator_instruction_delta,
};
use super::{
    InstructionActivationExpectation, InstructionActivationRequest, SYSTEM_ROW_PRUNE_REASON_KIND,
    Session, SystemRowPruneRefusal, SystemRowRef,
};
use crate::service::SystemRowPruneRequest;
use crate::types::{
    InstructionActivationId, InstructionContentDigest, InstructionKey, InstructionNamespace,
    InstructionRevisionId, InstructionRevisionRef, Message, SystemMessage, SystemPromptKey,
    UserMessage,
};
use crate::{SystemPromptUpdateRequest, TranscriptEditError};

const LEGACY_ONE: &str = "Legacy prompt one: you plan the team's calendar.";
const LEGACY_TWO: &str = "Legacy prompt two: you plan the team's calendar and travel.";
const CURRENT: &str = "You plan the team's calendar, travel and billing.";
const CALENDAR_ONE: &str = "CALENDAR-ONE: book meetings in the morning only.";
const CALENDAR_TWO: &str = "CALENDAR-TWO: book meetings in the afternoon only.";
const RETIRED_HEADER: &str = "[meerkat-retired-instruction-v1]";

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

fn prompt_update(target_message_index: usize, content: &str) -> SystemPromptUpdateRequest {
    SystemPromptUpdateRequest {
        key: SystemPromptKey::new("primary").unwrap(),
        expected_version: None,
        target_message_index: Some(target_message_index),
        content: content.to_string(),
        actor: Some("host-test".to_string()),
        expected_parent_revision: None,
    }
}

fn style_slot() -> InstructionSlot {
    InstructionSlot {
        namespace: InstructionNamespace::new("app.operator").unwrap(),
        key: InstructionKey::new("style").unwrap(),
    }
}

/// The session's first operator-instruction entry: one delta that activates
/// one slot.
fn entry(session: &Session) -> SystemMessage {
    let body = "Answer in short sentences.";
    mint_operator_instruction_delta(
        OperatorInstructionEntryHeader {
            origin_session_id: session.id().clone(),
            entry_sequence: 1,
            target_generation: 11,
            run_id: None,
        },
        vec![ActiveOperatorInstruction {
            slot: style_slot(),
            revision: InstructionRevision::new(1).unwrap(),
            body: body.to_string(),
        }],
        vec![OperatorInstructionChangeContent::Activate {
            slot: style_slot(),
            revision: InstructionRevision::new(1).unwrap(),
            supersedes: None,
            body: body.to_string(),
        }],
        chrono::DateTime::<chrono::Utc>::UNIX_EPOCH,
    )
    .unwrap()
}

fn user(text: &str) -> Message {
    Message::User(UserMessage::text(text))
}

fn request(remove: Vec<SystemRowRef>) -> SystemRowPruneRequest {
    SystemRowPruneRequest {
        remove,
        retire_superseded_activation_bodies: false,
        expected_parent_revision: None,
        note: Some("legacy prompts".to_string()),
        actor: Some("unit-test".to_string()),
    }
}

/// The reference to the System row at `index`, as a history read shows it.
fn row(session: &Session, index: usize) -> SystemRowRef {
    let Message::System(system) = &session.messages()[index] else {
        panic!("row {index} is a System row");
    };
    SystemRowRef::for_row(index, &system.content)
}

fn refused(error: &TranscriptEditError) -> Option<(usize, SystemRowPruneRefusal)> {
    match error {
        TranscriptEditError::SystemRowPruneRefused { index, cause } => Some((*index, *cause)),
        _ => None,
    }
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

/// `[legacy one, legacy two, current, question]`: two prompts that earlier
/// boots wrote before the current one.
fn session_with_legacy_prompts() -> Session {
    let mut session = Session::new();
    session.append_system_message(LEGACY_ONE);
    session.append_system_message(LEGACY_TWO);
    session.append_system_message(CURRENT);
    session.push(user("What is on the calendar today?"));
    session
}

/// The named ordinary System rows go in one rewrite commit, whose reason
/// kind is `system-row-prune` and whose note is the request's; every other
/// row stays, byte for byte.
#[test]
fn prune_removes_the_named_ordinary_rows_in_one_commit() {
    let mut session = session_with_legacy_prompts();
    let before = session.messages().to_vec();
    let parent = session.transcript_revision().unwrap();
    let pruned = session
        .prune_system_rows(request(vec![row(&session, 0), row(&session, 1)]))
        .unwrap_or_else(|error| panic!("the legacy prompts are pruned: {error}"))
        .expect("the prune commits");
    assert_eq!(pruned.removed_rows, 2);
    assert_eq!(pruned.retired_bodies, 0);
    assert_eq!(pruned.commit.parent_revision, parent);
    assert_eq!(
        pruned.commit.revision,
        session.transcript_revision().unwrap()
    );
    assert_eq!(pruned.commit.reason.kind, SYSTEM_ROW_PRUNE_REASON_KIND);
    assert_eq!(pruned.commit.reason.note.as_deref(), Some("legacy prompts"));
    assert_eq!(session.messages(), &before[2..]);
    let history = session
        .validated_transcript_history_state()
        .unwrap()
        .expect("the prune left a rewrite graph");
    assert_eq!(history.commit_count(), 1, "one commit");
    assert_reloaded_graph_validates(&session);
}

/// A stale digest, an index past the transcript, an index named twice and
/// a row that is not a System row each refuse the whole prune, typed, with
/// the row's index, and nothing is written.
#[test]
fn prune_refuses_a_stale_or_mismatched_row() {
    let mut session = session_with_legacy_prompts();
    let before = session.messages().to_vec();
    let mut stale = row(&session, 0);
    stale.content_sha256 = InstructionContentDigest::for_body("what an earlier read showed");
    let cases = [
        (vec![stale], 0, SystemRowPruneRefusal::DigestMismatch),
        (
            vec![row(&session, 1), SystemRowRef::for_row(9, LEGACY_ONE)],
            9,
            SystemRowPruneRefusal::OutOfBounds,
        ),
        (
            vec![row(&session, 0), row(&session, 0)],
            0,
            SystemRowPruneRefusal::DuplicateIndex,
        ),
        (
            vec![SystemRowRef::for_row(3, "What is on the calendar today?")],
            3,
            SystemRowPruneRefusal::NotASystemRow,
        ),
    ];
    for (remove, index, cause) in cases {
        let refusal = session
            .prune_system_rows(request(remove))
            .expect_err("the prune is refused");
        assert_eq!(refused(&refusal), Some((index, cause)), "{refusal:?}");
        assert_eq!(session.messages(), before.as_slice(), "nothing is written");
    }
    assert!(
        session
            .validated_transcript_history_state()
            .unwrap()
            .is_none(),
        "no rewrite was committed"
    );
}

/// A keyed prompt version, an operator-instruction entry row and an
/// instruction activation row each have their own lifecycle: a prune that
/// names one is refused with its kind, and nothing is written.
#[test]
fn prune_refuses_prompt_version_activation_and_entry_rows() {
    let mut session = Session::new();
    session.append_system_message(CURRENT);
    session
        .update_system_prompt(prompt_update(0, "You plan the team's calendar."))
        .expect("the head prompt is adopted as a keyed version");
    session.push(user("first question"));
    let entry_row = entry(&session);
    session
        .append_operator_instruction_entry(entry_row)
        .expect("the entry row is appended");
    session
        .activate_instruction(activation("tone", "act-1", None, "Answer formally."))
        .unwrap();
    session.push(user("second question"));
    let before = session.messages().to_vec();
    for (index, cause) in [
        (0, SystemRowPruneRefusal::PromptVersionRow),
        (2, SystemRowPruneRefusal::OperatorInstructionEntryRow),
        (3, SystemRowPruneRefusal::InstructionActivationRow),
    ] {
        let refusal = session
            .prune_system_rows(request(vec![row(&session, index)]))
            .expect_err("the prune is refused");
        assert_eq!(refused(&refusal), Some((index, cause)), "{refusal:?}");
        assert_eq!(session.messages(), before.as_slice(), "nothing is written");
    }
}

/// An ordinary System row that an operator-instruction entry row follows in
/// its group of System rows is not removed: the removal would move the
/// entry. A row whose group holds no entry is removed.
#[test]
fn prune_refuses_a_removal_before_an_operator_instruction_entry() {
    let mut session = Session::new();
    session.append_system_message(LEGACY_ONE);
    let entry_row = entry(&session);
    session
        .append_operator_instruction_entry(entry_row)
        .expect("the entry row is appended");
    session.push(user("first question"));
    session.append_system_message(LEGACY_TWO);
    session.push(user("second question"));
    let before = session.messages().to_vec();

    let refusal = session
        .prune_system_rows(request(vec![row(&session, 3), row(&session, 0)]))
        .expect_err("the removal before the entry is refused");
    assert_eq!(
        refused(&refusal),
        Some((0, SystemRowPruneRefusal::BeforeOperatorInstructionEntry)),
        "{refusal:?}"
    );
    assert_eq!(session.messages(), before.as_slice(), "nothing is written");

    let pruned = session
        .prune_system_rows(request(vec![row(&session, 3)]))
        .unwrap_or_else(|error| panic!("a row with no entry in its group is pruned: {error}"))
        .expect("the prune commits");
    assert_eq!(pruned.removed_rows, 1);
    assert_eq!(session.messages().len(), before.len() - 1);
    assert_reloaded_graph_validates(&session);
}

/// Removing an ordinary System row before an instruction activation row
/// commits: the activation row keeps its place among the conversation rows,
/// byte for byte. Its offset moved, so the document is written at envelope
/// 5, and the reloaded graph validates.
#[test]
fn prune_keeps_activation_places() {
    let mut session = Session::new();
    session.append_system_message(LEGACY_ONE);
    session.append_system_message(CURRENT);
    session
        .activate_instruction(activation("tone", "act-1", None, "Answer formally."))
        .unwrap();
    session.push(user("first question"));
    assert_eq!(envelope_version(&session), 4);
    let activation_row = session.messages()[2].clone();

    session
        .prune_system_rows(request(vec![row(&session, 0)]))
        .unwrap_or_else(|error| panic!("the row before the activation is pruned: {error}"))
        .expect("the prune commits");
    assert_eq!(session.messages().len(), 3);
    assert_eq!(
        session.messages()[1],
        activation_row,
        "the activation row is unchanged"
    );
    assert_eq!(
        envelope_version(&session),
        5,
        "the rewrite moved the activation row's offset"
    );
    assert_reloaded_graph_validates(&session);
}

/// With the retire option the same commit retires every superseded
/// activation body in place; each key's newest row keeps its body.
#[test]
fn prune_retires_superseded_bodies_when_asked() {
    let mut session = Session::new();
    session.append_system_message(LEGACY_ONE);
    session
        .activate_instruction(activation("calendar", "act-1", None, CALENDAR_ONE))
        .unwrap();
    session.push(user("first question"));
    session
        .activate_instruction(activation("calendar", "act-2", Some("act-1"), CALENDAR_TWO))
        .unwrap();
    session.push(user("second question"));
    let newest = session.messages()[3].clone();

    let mut retiring = request(vec![row(&session, 0)]);
    retiring.retire_superseded_activation_bodies = true;
    let pruned = session
        .prune_system_rows(retiring)
        .unwrap_or_else(|error| panic!("the prune retires the superseded body: {error}"))
        .expect("the prune commits");
    assert_eq!(pruned.removed_rows, 1);
    assert_eq!(pruned.retired_bodies, 1);
    let rows = session.messages();
    assert_eq!(rows.len(), 4);
    let Message::System(retired) = &rows[0] else {
        panic!("the superseded activation row is a System row");
    };
    assert_eq!(
        retired
            .instruction_activation
            .as_ref()
            .map(|identity| identity.activation_id.as_str()),
        Some("act-1")
    );
    assert!(
        retired.content.starts_with(RETIRED_HEADER),
        "{}",
        retired.content
    );
    assert!(!retired.content.contains(CALENDAR_ONE));
    assert_eq!(
        retired
            .retired_instruction_body
            .map(|marker| marker.body_bytes),
        Some(CALENDAR_ONE.len() as u64)
    );
    assert_eq!(rows[2], newest, "each key's newest row keeps its body");
    assert_eq!(
        session
            .validated_transcript_history_state()
            .unwrap()
            .expect("a rewrite graph")
            .commit_count(),
        1,
        "the removal and the retirement are one commit"
    );
    assert_eq!(envelope_version(&session), 5);
    assert_reloaded_graph_validates(&session);
}

/// The retire option alone, with no row named, retires the superseded
/// bodies in one commit.
#[test]
fn prune_can_retire_without_removing_a_row() {
    let mut session = Session::new();
    session
        .activate_instruction(activation("calendar", "act-1", None, CALENDAR_ONE))
        .unwrap();
    session.push(user("first question"));
    session
        .activate_instruction(activation("calendar", "act-2", Some("act-1"), CALENDAR_TWO))
        .unwrap();
    session.push(user("second question"));

    let mut retiring = request(Vec::new());
    retiring.retire_superseded_activation_bodies = true;
    let pruned = session
        .prune_system_rows(retiring)
        .unwrap_or_else(|error| panic!("the prune retires the superseded body: {error}"))
        .expect("the prune commits");
    assert_eq!(pruned.removed_rows, 0);
    assert_eq!(pruned.retired_bodies, 1);
    assert_eq!(session.messages().len(), 4);
    assert_reloaded_graph_validates(&session);
}

/// A request that names no row, with nothing to retire, writes nothing.
#[test]
fn prune_with_nothing_to_do_writes_nothing() {
    let mut session = session_with_legacy_prompts();
    session
        .activate_instruction(activation("tone", "act-1", None, "Answer formally."))
        .unwrap();
    let before = session.messages().to_vec();
    let quiet = session
        .prune_system_rows(request(Vec::new()))
        .unwrap_or_else(|error| panic!("an empty prune answers: {error}"));
    assert!(quiet.is_none(), "nothing named, nothing written");
    let mut retiring = request(Vec::new());
    retiring.retire_superseded_activation_bodies = true;
    let quiet = session
        .prune_system_rows(retiring)
        .unwrap_or_else(|error| panic!("an empty retirement answers: {error}"));
    assert!(quiet.is_none(), "no superseded body to retire");
    assert_eq!(session.messages(), before.as_slice());
    assert!(
        session
            .validated_transcript_history_state()
            .unwrap()
            .is_none(),
        "no rewrite was committed"
    );
}

/// An ordinary System row with an append identity is an ordinary row: it is
/// removed (an append retried later with its key appends again).
#[test]
fn prune_accepts_an_ordinary_row_with_an_append_identity() {
    let mut session = session_with_legacy_prompts();
    session
        .append_system_message_idempotent(
            "Peer roster: lead, worker.",
            Some("comms:roster".to_string()),
            Some("roster:v1".to_string()),
            crate::types::message_timestamp_now(),
        )
        .unwrap();
    let pruned = session
        .prune_system_rows(request(vec![row(&session, 4)]))
        .unwrap_or_else(|error| panic!("the appended row is pruned: {error}"))
        .expect("the prune commits");
    assert_eq!(pruned.removed_rows, 1);
    assert_eq!(session.messages().len(), 4);
}

/// A stale expected head refuses the prune before anything is checked or
/// written; the current head commits.
#[test]
fn prune_checks_the_expected_parent_revision() {
    let mut session = session_with_legacy_prompts();
    let before = session.messages().to_vec();
    let mut stale = request(vec![row(&session, 0)]);
    stale.expected_parent_revision = Some("sha256:not-the-head".to_string());
    let refusal = session
        .prune_system_rows(stale)
        .expect_err("a stale head refuses the prune");
    assert!(
        matches!(refusal, TranscriptEditError::RevisionConflict { .. }),
        "{refusal:?}"
    );
    assert_eq!(session.messages(), before.as_slice(), "nothing is written");
    let mut current = request(vec![row(&session, 0)]);
    current.expected_parent_revision = Some(session.transcript_revision().unwrap());
    session
        .prune_system_rows(current)
        .unwrap_or_else(|error| panic!("the current head commits: {error}"))
        .expect("the prune commits");
    assert_eq!(session.messages(), &before[1..]);
}

/// A prune that removes an ordinary System row before an instruction
/// activation row moves the activation row to another offset: the document
/// is written at envelope 5, which a release that compares activation
/// offsets refuses at its envelope.
#[test]
fn a_prune_before_an_activation_row_writes_envelope_5() {
    let mut session = Session::new();
    session.append_system_message(LEGACY_ONE);
    session
        .activate_instruction(activation("tone", "act-1", None, "Answer formally."))
        .unwrap();
    session.push(user("first question"));
    assert!(!session.carries_operator_instruction_facts());
    assert_eq!(envelope_version(&session), 4);

    session
        .prune_system_rows(request(vec![row(&session, 0)]))
        .unwrap_or_else(|error| panic!("the row before the activation is pruned: {error}"))
        .expect("the prune commits");
    assert!(session.carries_operator_instruction_facts());
    assert_eq!(envelope_version(&session), 5);
}

/// A prune that moves no activation row (the removed row stands after the
/// last one) and retires no body writes nothing only envelope 5 reads: the
/// document stays at envelope 4.
#[test]
fn a_prune_after_the_last_activation_row_stays_at_envelope_4() {
    let mut session = Session::new();
    session.append_system_message(LEGACY_ONE);
    session
        .activate_instruction(activation("tone", "act-1", None, "Answer formally."))
        .unwrap();
    session.push(user("first question"));
    session.append_system_message(LEGACY_TWO);
    session.push(user("second question"));
    assert_eq!(envelope_version(&session), 4);

    session
        .prune_system_rows(request(vec![row(&session, 3)]))
        .unwrap_or_else(|error| panic!("the row after the activation is pruned: {error}"))
        .expect("the prune commits");
    assert!(!session.carries_operator_instruction_facts());
    assert_eq!(
        envelope_version(&session),
        4,
        "nothing only envelope 5 reads"
    );
    assert_reloaded_graph_validates(&session);
}

/// A retiring prune in a session whose stored operator-instruction state
/// does not read is refused before anything changes: the envelope-5 marker
/// a retired body needs is checked before the commit is installed, so the
/// prune is all or nothing.
#[test]
fn a_retiring_prune_that_cannot_record_envelope_5_changes_nothing() {
    let mut session = Session::new();
    session
        .activate_instruction(activation("calendar", "act-1", None, CALENDAR_ONE))
        .unwrap();
    session.push(user("first question"));
    session
        .activate_instruction(activation("calendar", "act-2", Some("act-1"), CALENDAR_TWO))
        .unwrap();
    session.push(user("second question"));
    session.set_metadata_unchecked_for_test(
        super::SESSION_INSTRUCTION_STATE_KEY,
        serde_json::json!({ "owner": "a state this release cannot read" }),
    );
    let before = session.messages().to_vec();

    let mut retiring = request(Vec::new());
    retiring.retire_superseded_activation_bodies = true;
    let refused = session.prune_system_rows(retiring);
    assert!(
        matches!(&refused, Err(TranscriptEditError::HistoryStateMalformed(_))),
        "the prune is refused: {refused:?}"
    );
    assert_eq!(
        session.messages(),
        before.as_slice(),
        "the refused prune changes no row"
    );
    assert!(
        session
            .validated_transcript_history_state()
            .unwrap()
            .is_none(),
        "nor records a rewrite"
    );
}

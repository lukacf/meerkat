//! Rows for the per-document session envelope. A document that carries an
//! operator-instruction fact (an entry row, or the recorded target state) is
//! written at envelope 5; every other document stays at envelope 4,
//! byte-identical to what the released binaries write and read.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use super::operator_instructions::{
    ActiveOperatorInstruction, InstructionRevision, InstructionSlot,
    OperatorInstructionChangeContent, OperatorInstructionEntryHeader,
    mint_operator_instruction_delta,
};
use super::{
    InstructionTargetOwner, SESSION_INSTRUCTION_STATE_KEY, Session, TranscriptReplacement,
};
use crate::generated::session_persistence_version_authority as current;
use crate::session_store::{SessionHead, TranscriptStrandId};
use crate::types::{InstructionKey, InstructionNamespace, Message, UserMessage};

// Byte-exact released v0.9.8 authority, commit
// 0ecc37a3b13b13691da0983d1361207f83871284, source blob
// 7c24ff235f5dc0f5e747cd6890e2825ecef20e2d: the newest released reader, the
// one a rollback lands on. It is only the envelope-version oracle, not an old
// binary.
#[allow(dead_code)]
mod released_v0_9_8_version_authority {
    include!("../../tests/fixtures/v0_9_8_session_persistence_version_authority.rs.txt");
}

fn envelope_version(bytes: &[u8]) -> u64 {
    serde_json::from_slice::<serde_json::Value>(bytes).unwrap()["version"]
        .as_u64()
        .unwrap()
}

fn plain_session() -> Session {
    let mut session = Session::new();
    session.append_system_message("You are a careful assistant.");
    session.push(Message::User(UserMessage::text("hello")));
    session
}

fn session_with_entry() -> Session {
    let mut session = plain_session();
    let slot = InstructionSlot {
        namespace: InstructionNamespace::new("app.example").unwrap(),
        key: InstructionKey::new("tone").unwrap(),
    };
    let revision = InstructionRevision::new(1).unwrap();
    let body = "Answer in a formal tone.";
    let entry = mint_operator_instruction_delta(
        OperatorInstructionEntryHeader {
            origin_session_id: session.id().clone(),
            entry_sequence: 1,
            target_generation: 1,
            run_id: None,
        },
        vec![ActiveOperatorInstruction {
            slot: slot.clone(),
            revision,
            body: body.to_string(),
        }],
        vec![OperatorInstructionChangeContent::Activate {
            slot,
            revision,
            supersedes: None,
            body: body.to_string(),
        }],
        crate::types::message_timestamp_now(),
    )
    .unwrap();
    session.append_operator_instruction_entry(entry).unwrap();
    session
}

#[test]
fn a_document_with_an_entry_or_target_writes_envelope_5() {
    let plain = plain_session();
    assert_eq!(
        envelope_version(&plain.to_persisted_bytes().unwrap()),
        4,
        "a document without instruction facts keeps envelope 4"
    );

    let with_entry = session_with_entry();
    let bytes = with_entry.to_persisted_bytes().unwrap();
    assert_eq!(envelope_version(&bytes), 5);
    let reloaded = Session::from_persisted_bytes(&bytes).unwrap();
    assert_eq!(reloaded.version(), current::SESSION_VERSION);
    assert_eq!(
        reloaded.to_persisted_bytes().unwrap(),
        bytes,
        "once 5, always 5: a reloaded document writes the same bytes"
    );

    let mut with_target = plain;
    with_target.set_metadata_unchecked(
        SESSION_INSTRUCTION_STATE_KEY,
        serde_json::json!({
            "owner": {"kind": "session"},
            "target_generation": 1,
            "entry_sequence": 0,
            "slots": [],
        }),
    );
    assert_eq!(
        envelope_version(&with_target.to_persisted_bytes().unwrap()),
        5,
        "the recorded target state alone needs envelope 5"
    );
}

#[test]
fn released_authority_refuses_envelope_5_typed() {
    assert_eq!(current::SESSION_VERSION, 5);
    assert_eq!(current::SESSION_ENVELOPE_BASELINE_VERSION, 4);
    assert_eq!(current::session_envelope_version_for_document(false), 4);
    assert_eq!(current::session_envelope_version_for_document(true), 5);
    assert_eq!(
        released_v0_9_8_version_authority::restore_session_envelope_version(
            current::session_envelope_version_for_document(false)
        ),
        Ok(4),
        "the released reader still reads a document without instruction facts"
    );
    let refusal = released_v0_9_8_version_authority::restore_session_envelope_version(
        current::session_envelope_version_for_document(true),
    )
    .expect_err("the released reader refuses envelope 5");
    assert!(
        refusal.to_string().contains("rejected SessionEnvelope"),
        "{refusal}"
    );
    for supported in [3, 4, 5] {
        assert_eq!(
            current::restore_session_envelope_version(supported),
            Ok(current::SESSION_VERSION),
            "envelope {supported} restores to the in-memory current version"
        );
    }
    assert!(current::restore_session_envelope_version(6).is_err());

    // The released reader refuses a whole envelope-5 document typed, at its
    // envelope version, before reading any row.
    let bytes = session_with_entry().to_persisted_bytes().unwrap();
    let observed = u32::try_from(envelope_version(&bytes)).unwrap();
    assert!(released_v0_9_8_version_authority::restore_session_envelope_version(observed).is_err());
}

#[test]
fn head_canonical_accepts_envelope_5_heads_for_write() {
    let with_entry = session_with_entry();
    let head = SessionHead::from_session(&with_entry, TranscriptStrandId::root(), 0).unwrap();
    assert_eq!(head.version, 5);
    head.require_current_envelope_for_write()
        .expect("an envelope-5 head is written");
    assert_eq!(
        head.restored_session_version().unwrap(),
        current::SESSION_VERSION
    );

    let plain_head =
        SessionHead::from_session(&plain_session(), TranscriptStrandId::root(), 0).unwrap();
    assert_eq!(
        plain_head.version, 4,
        "a head without instruction facts stays at 4"
    );
    plain_head
        .require_current_envelope_for_write()
        .expect("an envelope-4 head is still written");

    let mut released = plain_head;
    released.version = 3;
    assert!(
        released.require_current_envelope_for_write().is_err(),
        "a successor never keeps the released v3 envelope"
    );
}

fn is_entry(message: &Message) -> bool {
    matches!(message, Message::System(system) if system.instruction_entry.is_some())
}

/// The recorded instruction state is the whole presence answer: appending
/// an entry row records it, and a session without it is not read for entry
/// rows (no transcript scan on a save path).
#[test]
fn the_state_key_alone_marks_a_document_with_instruction_facts() {
    let mut session = session_with_entry();
    assert!(
        session
            .metadata()
            .contains_key(SESSION_INSTRUCTION_STATE_KEY),
        "appending an entry row records the instruction state"
    );
    assert!(session.carries_operator_instruction_facts());

    session.metadata.remove(SESSION_INSTRUCTION_STATE_KEY);
    assert!(session.messages().iter().any(is_entry));
    assert!(
        !session.carries_operator_instruction_facts(),
        "the answer reads the state key, never the transcript"
    );
    assert_eq!(session.persisted_envelope_version(), 4);
}

/// A fork child that copies entry rows records an unowned instruction
/// state (no target of its own, nothing pending), so it keeps the key the
/// presence answer reads and is still written at envelope 5. A fork whose
/// prefix copies no entry row records nothing.
#[test]
fn a_fork_child_that_copies_entry_rows_records_the_unowned_state_and_writes_envelope_5() {
    // [prompt, user, entry, user]
    let mut source = session_with_entry();
    source.push(Message::User(UserMessage::text("and then?")));
    let replaced = source
        .fork_replacing(
            3,
            TranscriptReplacement::Message {
                message: Message::User(UserMessage::text("and after that?")),
            },
        )
        .expect("an edited fork");
    for (how, child) in [
        ("fork", source.fork()),
        ("fork_at", source.fork_at(3)),
        ("fork_replacing", replaced),
    ] {
        assert!(
            child.messages().iter().any(is_entry),
            "{how}: the child copies the entry row"
        );
        let state = child
            .instruction_target_state()
            .unwrap()
            .unwrap_or_else(|| panic!("{how}: the child records the instruction state"));
        assert_eq!(
            state.owner(),
            &InstructionTargetOwner::Unowned,
            "{how}: the child has no target of its own"
        );
        assert!(child.carries_operator_instruction_facts(), "{how}");
        assert_eq!(
            envelope_version(&child.to_persisted_bytes().unwrap()),
            5,
            "{how}: the child is still written at envelope 5"
        );
    }

    let before_the_entry = source.fork_at(2);
    assert!(!before_the_entry.messages().iter().any(is_entry));
    assert!(
        before_the_entry
            .instruction_target_state()
            .unwrap()
            .is_none()
    );
    assert!(!before_the_entry.carries_operator_instruction_facts());
    assert_eq!(
        envelope_version(&before_the_entry.to_persisted_bytes().unwrap()),
        4,
        "a fork without entry rows stays at envelope 4"
    );
}

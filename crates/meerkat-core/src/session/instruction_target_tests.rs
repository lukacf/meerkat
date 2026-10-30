//! Rows for the session seam of the SessionDocumentMachine
//! operator-instruction region: the recorded target state
//! (`session_instruction_state_v1`), the session-owned write, the mob
//! record, the owner rebind and release, and the run-start application.
//! Every decision is the machine's; these rows pin what the session records
//! from each verdict.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use super::SESSION_INSTRUCTION_STATE_KEY;
use super::Session;
use super::instruction_activation::{
    InstructionActivationErrorCode, InstructionActivationExpectation, InstructionActivationRequest,
};
use super::instruction_target::{
    InstructionDeferReason, InstructionLowering, InstructionRecordDisposition,
    InstructionRecordReceipt, InstructionReleaseDisposition, InstructionRunStart,
    InstructionSetErrorCode, InstructionStateError, InstructionTargetOwner,
    InstructionTargetRecord, InstructionTargetRecordSlot, InstructionWriteDisposition,
    InstructionWriteError, RunStartInstructionOutcome, SessionInstructionChange,
    SessionInstructionWrite,
};
use super::operator_instructions::{
    InstructionChange, InstructionEffect, InstructionEntryIdentity, InstructionRevision,
    InstructionSlot, MAX_OPERATOR_INSTRUCTION_SLOTS, effective_operator_instructions,
};
use crate::types::{
    InstructionActivationId, InstructionContentDigest, InstructionKey, InstructionNamespace,
    InstructionRevisionId, InstructionRevisionRef, Message, UserMessage,
};
use serde_json::json;

const NAMESPACE: &str = "app.example";

fn slot(key: &str) -> InstructionSlot {
    InstructionSlot {
        namespace: InstructionNamespace::new(NAMESPACE).unwrap(),
        key: InstructionKey::new(key).unwrap(),
    }
}

fn revision(value: u64) -> InstructionRevision {
    InstructionRevision::new(value).unwrap()
}

fn activate(key: &str, rev: u64, body: &str) -> SessionInstructionWrite {
    SessionInstructionWrite {
        slot: slot(key),
        revision: revision(rev),
        change: SessionInstructionChange::Activate {
            body: body.to_string(),
            content_sha256: None,
        },
    }
}

fn revoke(key: &str, rev: u64) -> SessionInstructionWrite {
    SessionInstructionWrite {
        slot: slot(key),
        revision: revision(rev),
        change: SessionInstructionChange::Revoke,
    }
}

/// A session whose transcript is still a leading System prefix.
fn fresh_session() -> Session {
    let mut session = Session::new();
    session.append_system_message("You are a careful assistant.");
    session
}

/// A session that has had a conversation.
fn conversed_session() -> Session {
    let mut session = fresh_session();
    session.push(Message::User(UserMessage::text("hello")));
    session
}

fn entries(session: &Session) -> Vec<InstructionEntryIdentity> {
    session
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::System(system) => system.instruction_entry.clone(),
            _ => None,
        })
        .collect()
}

fn code(error: &InstructionWriteError) -> InstructionSetErrorCode {
    error.code()
}

fn record(generation: u64, slots: Vec<(&str, u64, Option<&str>)>) -> InstructionTargetRecord {
    mob_record("mob-1", generation, slots)
}

fn mob_record(
    mob_id: &str,
    generation: u64,
    slots: Vec<(&str, u64, Option<&str>)>,
) -> InstructionTargetRecord {
    InstructionTargetRecord {
        mob_id: mob_id.to_string(),
        agent_identity: "worker".to_string(),
        generation,
        slots: slots
            .into_iter()
            .map(|(key, rev, body)| InstructionTargetRecordSlot {
                slot: slot(key),
                revision: revision(rev),
                body: body.map(str::to_string),
            })
            .collect(),
    }
}

fn mob_owner() -> InstructionTargetOwner {
    owner_mob("mob-1")
}

fn owner_mob(mob_id: &str) -> InstructionTargetOwner {
    InstructionTargetOwner::Mob {
        mob_id: mob_id.to_string(),
        agent_identity: "worker".to_string(),
    }
}

fn fresh_run(session: &mut Session) -> RunStartInstructionOutcome {
    session
        .apply_run_start_instructions(
            InstructionRunStart::FreshRun,
            InstructionLowering::MidConversation,
            None,
        )
        .unwrap()
}

#[test]
fn recorded_target_state_reads_the_envelope_5_shape() {
    let mut session = fresh_session();
    let shape = json!({
        "owner": {"kind": "session"},
        "target_generation": 1,
        "entry_sequence": 0,
        "slots": [],
    });
    session.set_metadata_unchecked_for_test(SESSION_INSTRUCTION_STATE_KEY, shape.clone());
    let state = session
        .instruction_target_state()
        .unwrap()
        .expect("a recorded state");
    assert_eq!(state.owner(), &InstructionTargetOwner::Session);
    assert_eq!(state.target_generation(), 1);
    assert_eq!(state.entry_sequence(), 0);
    assert!(state.slots().is_empty());
    assert_eq!(serde_json::to_value(&state).unwrap(), shape);

    let body = "Be brief.";
    let mob_shape = json!({
        "owner": {"kind": "mob", "mob_id": "mob-1", "agent_identity": "worker"},
        "target_generation": 3,
        "entry_sequence": 2,
        "slots": [{
            "namespace": NAMESPACE,
            "key": "tone",
            "revision": 4,
            "effect": "active",
            "content_sha256": InstructionContentDigest::for_body(body).as_str(),
            "body_bytes": body.len(),
            "pending_body": body,
        }, {
            "namespace": NAMESPACE,
            "key": "zone",
            "revision": 2,
            "effect": "absent",
        }],
    });
    session.set_metadata_unchecked_for_test(SESSION_INSTRUCTION_STATE_KEY, mob_shape.clone());
    let state = session
        .instruction_target_state()
        .unwrap()
        .expect("a recorded state");
    assert_eq!(state.owner(), &mob_owner());
    assert_eq!(state.slots().len(), 2);
    assert_eq!(state.slots()[0].effect, InstructionEffect::Active);
    assert_eq!(state.slots()[0].pending_body.as_deref(), Some(body));
    assert_eq!(state.slots()[1].effect, InstructionEffect::Absent);
    assert_eq!(serde_json::to_value(&state).unwrap(), mob_shape);
    assert!(session.carries_operator_instruction_facts());
}

#[test]
fn a_session_without_a_recorded_target_reads_none() {
    let session = conversed_session();
    assert_eq!(session.instruction_target_state().unwrap(), None);
}

#[test]
fn a_malformed_target_state_is_refused_typed() {
    let digest = InstructionContentDigest::for_body("x");
    let active = |key: &str| {
        json!({
            "namespace": NAMESPACE, "key": key, "revision": 1, "effect": "active",
            "content_sha256": digest.as_str(), "body_bytes": 1,
        })
    };
    let state = |slots: serde_json::Value| json!({"owner": {"kind": "session"}, "target_generation": 1, "entry_sequence": 0, "slots": slots});
    for (case, value) in [
        (
            "an unknown owner",
            json!({"owner": {"kind": "host"}, "target_generation": 1, "entry_sequence": 0, "slots": []}),
        ),
        (
            "slots out of slot order",
            state(json!([active("b"), active("a")])),
        ),
        ("a repeated slot", state(json!([active("a"), active("a")]))),
        (
            "an active slot without a digest",
            state(json!([{
                "namespace": NAMESPACE, "key": "a", "revision": 1, "effect": "active",
                "body_bytes": 1,
            }])),
        ),
        (
            "a pending body that is not the recorded content",
            state(json!([{
                "namespace": NAMESPACE, "key": "a", "revision": 1, "effect": "active",
                "content_sha256": digest.as_str(), "body_bytes": 1, "pending_body": "y",
            }])),
        ),
        (
            "a revoked slot with a body",
            state(json!([{
                "namespace": NAMESPACE, "key": "a", "revision": 1, "effect": "absent",
                "pending_body": "x",
            }])),
        ),
    ] {
        let mut session = fresh_session();
        session.set_metadata_unchecked_for_test(SESSION_INSTRUCTION_STATE_KEY, value);
        assert!(
            matches!(
                session.instruction_target_state(),
                Err(InstructionStateError::MalformedState(_))
            ),
            "{case}"
        );
        let refused = session
            .set_session_instruction(activate("tone", 1, "x"))
            .expect_err(case);
        assert!(
            matches!(refused, InstructionWriteError::Malformed(_)),
            "{case}: {refused:?}"
        );
        assert_eq!(
            session.apply_run_start_instructions(
                InstructionRunStart::FreshRun,
                InstructionLowering::MidConversation,
                None,
            ),
            Err(session.instruction_target_state().unwrap_err()),
            "{case}: a run start refuses a malformed state rather than guess"
        );
    }
}

#[test]
fn session_writes_follow_the_machine_verdicts() {
    let mut session = conversed_session();
    let receipt = session
        .set_session_instruction(activate("tone", 2, "Answer formally."))
        .unwrap();
    assert_eq!(receipt.disposition, InstructionWriteDisposition::Recorded);
    assert_eq!(receipt.target_generation, 1);
    assert_eq!(receipt.slot, slot("tone"));
    assert_eq!(receipt.revision, revision(2));

    // An exact retry is a duplicate and changes nothing.
    let document = session.to_persisted_bytes().unwrap();
    let receipt = session
        .set_session_instruction(activate("tone", 2, "Answer formally."))
        .unwrap();
    assert_eq!(receipt.disposition, InstructionWriteDisposition::Duplicate);
    assert_eq!(receipt.target_generation, 1);
    assert_eq!(session.to_persisted_bytes().unwrap(), document);

    for (case, write, expected) in [
        (
            "the same revision with another body",
            activate("tone", 2, "Answer casually."),
            InstructionSetErrorCode::RevisionConflict,
        ),
        (
            "a lower revision",
            activate("tone", 1, "Answer formally."),
            InstructionSetErrorCode::StaleRevision,
        ),
        (
            "a revoke of a slot never written",
            revoke("style", 1),
            InstructionSetErrorCode::UnknownSlot,
        ),
        (
            "a declared digest that is not the body's",
            SessionInstructionWrite {
                slot: slot("tone"),
                revision: revision(3),
                change: SessionInstructionChange::Activate {
                    body: "Answer formally.".to_string(),
                    content_sha256: Some(InstructionContentDigest::for_body("other")),
                },
            },
            InstructionSetErrorCode::DigestMismatch,
        ),
    ] {
        let refused = session.set_session_instruction(write).expect_err(case);
        assert_eq!(code(&refused), expected, "{case}");
        assert_eq!(
            session.to_persisted_bytes().unwrap(),
            document,
            "{case}: a refusal changes nothing"
        );
    }

    // A higher revision supersedes, a revoke is a revision, and every
    // recorded write advances the target generation.
    let receipt = session
        .set_session_instruction(activate("tone", 3, "Answer warmly."))
        .unwrap();
    assert_eq!(
        (receipt.disposition, receipt.target_generation),
        (InstructionWriteDisposition::Recorded, 2)
    );
    let receipt = session.set_session_instruction(revoke("tone", 4)).unwrap();
    assert_eq!(
        (receipt.disposition, receipt.target_generation),
        (InstructionWriteDisposition::Recorded, 3)
    );

    // Writing records the target only: nothing is effective, no entry row.
    let state = session.instruction_target_state().unwrap().unwrap();
    assert_eq!(state.owner(), &InstructionTargetOwner::Session);
    assert_eq!(state.target_generation(), 3);
    assert_eq!(state.slots().len(), 1);
    assert_eq!(state.slots()[0].effect, InstructionEffect::Absent);
    assert!(entries(&session).is_empty());
    assert!(
        effective_operator_instructions(session.id(), session.messages())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_session_target_holds_at_most_64_slots() {
    let mut session = fresh_session();
    for index in 0..MAX_OPERATOR_INSTRUCTION_SLOTS {
        session
            .set_session_instruction(activate(&format!("k{index:02}"), 1, "x"))
            .unwrap();
    }
    let refused = session
        .set_session_instruction(activate("one-more", 1, "x"))
        .unwrap_err();
    assert_eq!(code(&refused), InstructionSetErrorCode::SlotLimitExceeded);
    // A slot already recorded still takes a new revision at the limit.
    let receipt = session
        .set_session_instruction(activate("k00", 2, "y"))
        .unwrap();
    assert_eq!(receipt.disposition, InstructionWriteDisposition::Recorded);
}

#[test]
fn bodies_stay_within_the_body_and_active_set_budgets() {
    let mut session = fresh_session();
    let quarter = "x".repeat(256 * 1024);
    let refused = session
        .set_session_instruction(activate("big", 1, &format!("{quarter}x")))
        .unwrap_err();
    assert_eq!(code(&refused), InstructionSetErrorCode::BodyTooLarge);
    for key in ["a", "b", "c", "d"] {
        session
            .set_session_instruction(activate(key, 1, &quarter))
            .unwrap();
    }
    let refused = session
        .set_session_instruction(activate("e", 1, "x"))
        .unwrap_err();
    assert_eq!(
        code(&refused),
        InstructionSetErrorCode::ActiveSetBudgetExceeded
    );
    // Revoking one makes room again.
    session.set_session_instruction(revoke("a", 2)).unwrap();
    session
        .set_session_instruction(activate("e", 1, "x"))
        .unwrap();
}

#[test]
fn a_mob_record_is_idempotent_by_generation() {
    let mut session = fresh_session();
    let receipt = session
        .record_instruction_target(record(1, vec![("tone", 1, Some("Be brief."))]))
        .unwrap();
    assert_eq!(
        receipt,
        InstructionRecordReceipt {
            disposition: InstructionRecordDisposition::Recorded,
            target_generation: 1,
        }
    );
    let document = session.to_persisted_bytes().unwrap();
    for repeated in [1, 1] {
        let receipt = session
            .record_instruction_target(record(repeated, vec![("tone", 1, Some("Be brief."))]))
            .unwrap();
        assert_eq!(
            receipt,
            InstructionRecordReceipt {
                disposition: InstructionRecordDisposition::AlreadyRecorded,
                target_generation: 1,
            }
        );
        assert_eq!(session.to_persisted_bytes().unwrap(), document);
    }
    let state = session.instruction_target_state().unwrap().unwrap();
    assert_eq!(state.owner(), &mob_owner());

    // A generation of zero is not a recorded target.
    let refused = session
        .record_instruction_target(record(0, Vec::new()))
        .unwrap_err();
    assert_eq!(code(&refused), InstructionSetErrorCode::InvalidRequest);

    // A mob-owned target refuses session writes; a session-owned one refuses
    // mob records.
    let refused = session
        .set_session_instruction(activate("tone", 9, "x"))
        .unwrap_err();
    assert_eq!(code(&refused), InstructionSetErrorCode::OwnedByMob);
    let mut owned = fresh_session();
    owned
        .set_session_instruction(activate("tone", 1, "x"))
        .unwrap();
    let refused = owned
        .record_instruction_target(record(5, vec![("tone", 1, Some("y"))]))
        .unwrap_err();
    assert_eq!(code(&refused), InstructionSetErrorCode::OwnedBySession);

    // A later generation replaces the whole snapshot.
    let receipt = session
        .record_instruction_target(record(
            3,
            vec![
                ("style", 1, Some("Use short sentences.")),
                ("tone", 2, None),
            ],
        ))
        .unwrap();
    assert_eq!(receipt.target_generation, 3);
    let state = session.instruction_target_state().unwrap().unwrap();
    assert_eq!(state.slots().len(), 2);
}

#[test]
fn a_run_start_applies_pending_changes_in_one_delta_entry() {
    let mut session = conversed_session();
    session
        .set_session_instruction(activate("tone", 1, "Answer formally."))
        .unwrap();
    session
        .set_session_instruction(activate("style", 1, "Use short sentences."))
        .unwrap();
    session
        .set_session_instruction(activate("tone", 2, "Answer warmly."))
        .unwrap();

    let outcome = session
        .apply_run_start_instructions(
            InstructionRunStart::FreshRun,
            InstructionLowering::MidConversation,
            None,
        )
        .unwrap();
    assert_eq!(
        outcome,
        RunStartInstructionOutcome::Applied {
            entry_sequence: 1,
            target_generation: 3,
        }
    );
    let rows = entries(&session);
    assert_eq!(rows.len(), 1, "three writes coalesce into one entry");
    assert_eq!(rows[0].changes().len(), 2);
    assert_eq!(rows[0].target_generation(), 3);
    assert!(
        matches!(
            session.messages().last(),
            Some(Message::System(system)) if system.instruction_entry.is_some()
        ),
        "the entry is appended at the tail, before the run's own inputs"
    );

    let effective = effective_operator_instructions(session.id(), session.messages()).unwrap();
    assert_eq!(
        effective.slot(&slot("tone")).unwrap().body(),
        Some("Answer warmly.")
    );
    assert_eq!(
        effective.slot(&slot("style")).unwrap().body(),
        Some("Use short sentences.")
    );
    let state = session.instruction_target_state().unwrap().unwrap();
    assert_eq!(state.entry_sequence(), 1);
    assert!(
        state.slots().iter().all(|slot| slot.pending_body.is_none()),
        "an applied body lives in its entry row"
    );

    // Nothing is left to apply.
    let outcome = session
        .apply_run_start_instructions(
            InstructionRunStart::FreshRun,
            InstructionLowering::MidConversation,
            None,
        )
        .unwrap();
    assert_eq!(outcome, RunStartInstructionOutcome::NotNeeded);
    assert_eq!(entries(&session).len(), 1);
}

#[test]
fn resume_and_no_pending_boundary_starts_never_apply() {
    let mut session = conversed_session();
    session
        .set_session_instruction(activate("tone", 1, "Answer formally."))
        .unwrap();
    let document = session.to_persisted_bytes().unwrap();
    for run_start in [InstructionRunStart::ResumeRun, InstructionRunStart::NoRun] {
        let outcome = session
            .apply_run_start_instructions(run_start, InstructionLowering::MidConversation, None)
            .unwrap();
        assert_eq!(
            outcome,
            RunStartInstructionOutcome::NotNeeded,
            "{run_start:?}"
        );
        assert_eq!(session.to_persisted_bytes().unwrap(), document);
    }
    assert!(entries(&session).is_empty());
}

#[test]
fn leading_prefix_only_lowering_applies_only_on_a_system_prefix() {
    let mut fresh = fresh_session();
    fresh
        .set_session_instruction(activate("tone", 1, "Answer formally."))
        .unwrap();
    let outcome = fresh
        .apply_run_start_instructions(
            InstructionRunStart::FreshRun,
            InstructionLowering::LeadingPrefixOnly,
            None,
        )
        .unwrap();
    assert!(matches!(
        outcome,
        RunStartInstructionOutcome::Applied { .. }
    ));
    assert!(
        fresh
            .messages()
            .iter()
            .all(|message| matches!(message, Message::System(_))),
        "the entry stays inside the leading System prefix"
    );

    let mut conversed = conversed_session();
    conversed
        .set_session_instruction(activate("tone", 1, "Answer formally."))
        .unwrap();
    let document = conversed.to_persisted_bytes().unwrap();
    let outcome = conversed
        .apply_run_start_instructions(
            InstructionRunStart::FreshRun,
            InstructionLowering::LeadingPrefixOnly,
            None,
        )
        .unwrap();
    assert_eq!(
        outcome,
        RunStartInstructionOutcome::Deferred(InstructionDeferReason::LeadingPrefixOnly)
    );
    assert_eq!(conversed.to_persisted_bytes().unwrap(), document);
}

#[test]
fn unavailable_lowering_defers() {
    for mut session in [fresh_session(), conversed_session()] {
        session
            .set_session_instruction(activate("tone", 1, "Answer formally."))
            .unwrap();
        let outcome = session
            .apply_run_start_instructions(
                InstructionRunStart::FreshRun,
                InstructionLowering::Unavailable,
                None,
            )
            .unwrap();
        assert_eq!(
            outcome,
            RunStartInstructionOutcome::Deferred(InstructionDeferReason::LoweringUnavailable)
        );
        assert!(entries(&session).is_empty());
    }
}

#[test]
fn an_applied_revoke_is_a_revoke_change() {
    let mut session = conversed_session();
    session
        .set_session_instruction(activate("tone", 1, "Answer formally."))
        .unwrap();
    session
        .apply_run_start_instructions(
            InstructionRunStart::FreshRun,
            InstructionLowering::MidConversation,
            None,
        )
        .unwrap();
    session.set_session_instruction(revoke("tone", 2)).unwrap();
    let outcome = session
        .apply_run_start_instructions(
            InstructionRunStart::FreshRun,
            InstructionLowering::MidConversation,
            None,
        )
        .unwrap();
    assert_eq!(
        outcome,
        RunStartInstructionOutcome::Applied {
            entry_sequence: 2,
            target_generation: 2,
        }
    );
    let rows = entries(&session);
    assert_eq!(rows.len(), 2);
    assert!(rows[1].active().is_empty());
    assert!(matches!(
        rows[1].changes(),
        [InstructionChange::Revoke { revision: r, revokes, .. }]
            if *r == revision(2) && *revokes == revision(1)
    ));
    let effective = effective_operator_instructions(session.id(), session.messages()).unwrap();
    assert_eq!(
        effective.slot(&slot("tone")).unwrap().effect(),
        InstructionEffect::Absent
    );
}

#[test]
fn rebind_starts_the_new_owner_from_the_effective_set() {
    let mut session = conversed_session();
    session
        .set_session_instruction(activate("tone", 1, "Answer formally."))
        .unwrap();
    session
        .apply_run_start_instructions(
            InstructionRunStart::FreshRun,
            InstructionLowering::MidConversation,
            None,
        )
        .unwrap();
    session
        .set_session_instruction(activate("style", 1, "Use short sentences."))
        .unwrap();

    session.rebind_instruction_owner(mob_owner()).unwrap();
    let state = session.instruction_target_state().unwrap().unwrap();
    assert_eq!(state.owner(), &mob_owner());
    assert_eq!(state.target_generation(), 0);
    assert_eq!(
        state.slots().len(),
        1,
        "the pending write does not carry over"
    );
    assert_eq!(state.slots()[0].revision, revision(1));
    assert_eq!(state.slots()[0].pending_body, None);
    assert_eq!(
        session
            .apply_run_start_instructions(
                InstructionRunStart::FreshRun,
                InstructionLowering::MidConversation,
                None,
            )
            .unwrap(),
        RunStartInstructionOutcome::NotNeeded
    );

    // Rebinding to the owner the target already has is refused.
    let refused = session.rebind_instruction_owner(mob_owner()).unwrap_err();
    assert_eq!(code(&refused), InstructionSetErrorCode::InvalidRequest);
}

#[test]
fn a_session_without_a_recorded_target_needs_no_entry_and_stays_at_envelope_4() {
    let mut session = conversed_session();
    for run_start in [
        InstructionRunStart::FreshRun,
        InstructionRunStart::ResumeRun,
        InstructionRunStart::NoRun,
    ] {
        assert_eq!(
            session
                .apply_run_start_instructions(run_start, InstructionLowering::MidConversation, None)
                .unwrap(),
            RunStartInstructionOutcome::NotNeeded
        );
    }
    assert!(!session.carries_operator_instruction_facts());
}

fn activation(key: &str, body: &str) -> InstructionActivationRequest {
    InstructionActivationRequest {
        revision: InstructionRevisionRef {
            namespace: InstructionNamespace::new(NAMESPACE).unwrap(),
            key: InstructionKey::new(key).unwrap(),
            revision_id: InstructionRevisionId::new("rev-1").unwrap(),
            content_sha256: InstructionContentDigest::for_body(body),
        },
        activation_id: InstructionActivationId::new(format!("act-{key}")).unwrap(),
        expectation: InstructionActivationExpectation::Absent,
        supersedes: None,
        body: body.to_string(),
    }
}

#[test]
fn one_session_never_mixes_activations_and_operator_instructions_on_a_slot() {
    let mut session = conversed_session();
    session
        .activate_instruction(activation("tone", "Answer formally."))
        .unwrap();
    let refused = session
        .set_session_instruction(activate("tone", 1, "Answer warmly."))
        .unwrap_err();
    assert_eq!(
        code(&refused),
        InstructionSetErrorCode::SlotManagedByActivation
    );
    let refused = session
        .record_instruction_target(record(1, vec![("tone", 1, Some("Answer warmly."))]))
        .unwrap_err();
    assert_eq!(
        code(&refused),
        InstructionSetErrorCode::SlotManagedByActivation
    );

    session
        .set_session_instruction(activate("style", 1, "Use short sentences."))
        .unwrap();
    let refused = session
        .activate_instruction(activation("style", "Use long sentences."))
        .unwrap_err();
    assert_eq!(
        refused.code(),
        InstructionActivationErrorCode::InvalidRequest
    );
}

#[test]
fn a_record_from_another_mob_is_refused_until_its_owner_releases_the_target() {
    let mut session = conversed_session();
    session
        .record_instruction_target(mob_record("mob-a", 2, vec![("tone", 1, Some("Be brief."))]))
        .unwrap();
    assert!(matches!(
        fresh_run(&mut session),
        RunStartInstructionOutcome::Applied { .. }
    ));
    let document = session.to_persisted_bytes().unwrap();

    // Another mob's snapshot, whatever its generation, and its release are
    // refused and change nothing.
    for generation in [1, 2, 9] {
        let refused = session
            .record_instruction_target(mob_record(
                "mob-b",
                generation,
                vec![("tone", 7, Some("Be verbose."))],
            ))
            .unwrap_err();
        assert_eq!(code(&refused), InstructionSetErrorCode::OwnedByOtherMob);
    }
    let refused = session.release_instruction_target("mob-b").unwrap_err();
    assert_eq!(code(&refused), InstructionSetErrorCode::OwnedByOtherMob);
    assert_eq!(session.to_persisted_bytes().unwrap(), document);

    // The owning mob releases: the target empties and is unowned, what is in
    // effect stays, and nothing is pending.
    assert_eq!(
        session.release_instruction_target("mob-a").unwrap(),
        InstructionReleaseDisposition::Released
    );
    let state = session.instruction_target_state().unwrap().unwrap();
    assert_eq!(state.owner(), &InstructionTargetOwner::Unowned);
    assert_eq!(state.target_generation(), 0);
    assert!(state.slots().is_empty());
    assert_eq!(state.entry_sequence(), 1);
    let effective = effective_operator_instructions(session.id(), session.messages()).unwrap();
    assert_eq!(effective.active_count(), 1, "a release applies nothing");
    assert_eq!(
        fresh_run(&mut session),
        RunStartInstructionOutcome::NotNeeded
    );
    let released = session.to_persisted_bytes().unwrap();
    assert_eq!(
        session.release_instruction_target("mob-a").unwrap(),
        InstructionReleaseDisposition::AlreadyUnowned
    );
    assert_eq!(
        session.to_persisted_bytes().unwrap(),
        released,
        "a repeated release writes nothing"
    );

    // Only now may another mob bind; its snapshot replaces the target, so
    // the next run start revokes what the released mob put in effect.
    let receipt = session
        .record_instruction_target(mob_record(
            "mob-b",
            1,
            vec![("style", 1, Some("Use short sentences."))],
        ))
        .unwrap();
    assert_eq!(
        receipt,
        InstructionRecordReceipt {
            disposition: InstructionRecordDisposition::Recorded,
            target_generation: 1,
        }
    );
    let state = session.instruction_target_state().unwrap().unwrap();
    assert_eq!(state.owner(), &owner_mob("mob-b"));
    assert!(matches!(
        fresh_run(&mut session),
        RunStartInstructionOutcome::Applied { .. }
    ));
    let effective = effective_operator_instructions(session.id(), session.messages()).unwrap();
    assert_eq!(effective.active_count(), 1);
    assert!(effective.slot(&slot("style")).is_some());
    assert!(matches!(
        effective.slot(&slot("tone")),
        Some(tone) if tone.effect() == InstructionEffect::Absent
    ));
}

#[test]
fn only_the_owning_mob_releases_a_target() {
    let mut session = conversed_session();
    assert_eq!(
        session.release_instruction_target("mob-a").unwrap(),
        InstructionReleaseDisposition::AlreadyUnowned
    );
    assert_eq!(
        session.instruction_target_state().unwrap(),
        None,
        "a release of nothing records nothing"
    );
    session
        .set_session_instruction(activate("tone", 1, "Answer formally."))
        .unwrap();
    let document = session.to_persisted_bytes().unwrap();
    let refused = session.release_instruction_target("mob-a").unwrap_err();
    assert_eq!(code(&refused), InstructionSetErrorCode::OwnedBySession);
    assert_eq!(session.to_persisted_bytes().unwrap(), document);
}

#[test]
fn a_rebind_never_takes_a_mob_owned_target() {
    let mut session = conversed_session();
    session
        .record_instruction_target(mob_record("mob-a", 1, vec![("tone", 1, Some("Be brief."))]))
        .unwrap();
    let document = session.to_persisted_bytes().unwrap();
    for owner in [
        InstructionTargetOwner::Session,
        owner_mob("mob-b"),
        owner_mob("mob-a"),
        InstructionTargetOwner::Unowned,
    ] {
        let refused = session.rebind_instruction_owner(owner).unwrap_err();
        assert_eq!(code(&refused), InstructionSetErrorCode::InvalidRequest);
    }
    assert_eq!(session.to_persisted_bytes().unwrap(), document);

    // After the owning mob releases it, a host adopts the session standalone.
    session.release_instruction_target("mob-a").unwrap();
    session
        .rebind_instruction_owner(InstructionTargetOwner::Session)
        .unwrap();
    let state = session.instruction_target_state().unwrap().unwrap();
    assert_eq!(state.owner(), &InstructionTargetOwner::Session);
}

#[test]
fn a_snapshot_never_names_a_slot_activations_manage_even_as_absent() {
    let mut session = conversed_session();
    session
        .activate_instruction(activation("tone", "Answer formally."))
        .unwrap();
    let refused = session
        .record_instruction_target(record(1, vec![("tone", 1, None)]))
        .unwrap_err();
    assert_eq!(
        code(&refused),
        InstructionSetErrorCode::SlotManagedByActivation
    );
    assert_eq!(session.instruction_target_state().unwrap(), None);
}

#[test]
fn the_active_budget_counts_the_target_as_the_write_leaves_it() {
    let quarter = "x".repeat(256 * 1024);
    let mut session = fresh_session();
    for key in ["a", "b", "c", "d"] {
        session
            .set_session_instruction(activate(key, 1, &quarter))
            .unwrap();
    }
    let document = session.to_persisted_bytes().unwrap();
    let refused = session
        .set_session_instruction(activate("e", 1, "x"))
        .unwrap_err();
    assert_eq!(
        code(&refused),
        InstructionSetErrorCode::ActiveSetBudgetExceeded
    );
    assert_eq!(
        session.to_persisted_bytes().unwrap(),
        document,
        "a refusal records nothing"
    );
    // A replacement counts only the new body.
    let replacement = "y".repeat(256 * 1024);
    session
        .set_session_instruction(activate("a", 2, &replacement))
        .unwrap();

    // A mob snapshot over the budget is refused as a whole.
    let mut member = fresh_session();
    let refused = member
        .record_instruction_target(record(
            1,
            vec![
                ("a", 1, Some(quarter.as_str())),
                ("b", 1, Some(quarter.as_str())),
                ("c", 1, Some(quarter.as_str())),
                ("d", 1, Some(quarter.as_str())),
                ("e", 1, Some("x")),
            ],
        ))
        .unwrap_err();
    assert_eq!(
        code(&refused),
        InstructionSetErrorCode::ActiveSetBudgetExceeded
    );
    assert_eq!(member.instruction_target_state().unwrap(), None);
}

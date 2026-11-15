//! Rows of the subject key of an `ask_user.questions` decision: the key and
//! the decision id of a call 0.9.15 holds stay what 0.9.15 computed, and a
//! call whose asker, run id and call id do not make a valid subject key
//! verbatim (together over 1,024 bytes, or with a control character) is
//! held under a bounded key.
//!
//! The literals these rows pin were computed outside Rust: the verbatim key
//! `{asker}/{run id}/{call id}`, the bounded key
//! `ask_user.call:{run id}:sha256:{digest}` (the SHA-256 of the asker, the
//! run id's text and the call id, each preceded by its byte length as an
//! 8-byte big-endian unsigned integer), and for each the decision id
//! `decision:` and the SHA-256 of the JSON array `[kind, key]`.

use meerkat_core::approval::decision::{DECISION_PAGE_MAX, DECISION_REFERENCE_MAX_BYTES};
use sha2::{Digest, Sha256};

use super::*;
use crate::decision::wire::{wire_decision, wire_decision_listing};

/// The run of the rows' calls.
const RUN: &str = "018f4e2a-7c3b-7d11-9a55-3f2b6c8d9e01";

fn run() -> RunId {
    RunId::from_uuid(uuid::Uuid::parse_str(RUN).expect("a run id"))
}

/// The rows' question set asked by `asker` in the call `call_id` of
/// `run_id`.
fn hold_for(asker: &str, run_id: &RunId, call_id: &str) -> AskUserHold {
    let mut hold = hold_of(question_set(), run_id, call_id);
    asker.clone_into(&mut hold.asker);
    hold
}

/// The bounded key of a call, computed here from its layout.
fn bounded_key(asker: &str, run_id: &RunId, call_id: &str) -> String {
    let run = run_id.to_string();
    let mut material = Sha256::new();
    for field in [asker, run.as_str(), call_id] {
        material.update(u64::try_from(field.len()).expect("a length").to_be_bytes());
        material.update(field.as_bytes());
    }
    format!("ask_user.call:{run}:sha256:{:x}", material.finalize())
}

/// What `decision`, held under a bounded key, must be: the key has the
/// bounded form's exact bytes for the call, it holds no separator of the
/// verbatim form, and the decision id derives from it.
fn assert_bounded(decision: &StoredDecision, asker: &str, run_id: &RunId, call_id: &str) {
    let key = decision.descriptor.subject_key.as_str();
    assert_eq!(key, bounded_key(asker, run_id, call_id));
    assert_eq!(key.len(), 122, "{key}");
    assert!(!key.contains('/'), "no verbatim key can equal it: {key}");
    assert_eq!(
        decision.decision_id,
        DecisionId::derive(
            &ask_user_questions_kind().expect("kind"),
            &decision.descriptor.subject_key
        )
    );
    assert_eq!(
        decision.descriptor.operation_id.as_str(),
        format!("ask_user:{}", decision.decision_id)
    );
}

/// The key of a call is what its hold seals, in both forms: the verbatim
/// text whenever a decision takes it as a subject key, the bounded form
/// otherwise.
#[test]
fn the_key_of_a_call_is_what_its_hold_seals() {
    let (_clock, service) = fixture();
    let run_id = run();
    let long = "c".repeat(2_000);
    for (asker, call_id) in [
        (ASKER, "call-1"),
        (ASKER, long.as_str()),
        (long.as_str(), "call-1"),
        (ASKER, "call\n1"),
        ("member:\u{7}planner", "call-1"),
    ] {
        let key = ask_user_subject_key(asker, &run_id, call_id).expect("the key");
        let held =
            hold_ask_user(&service, ASKED_AT, hold_for(asker, &run_id, call_id)).expect("the hold");
        assert_eq!(held.descriptor.subject_key, key);
        let verbatim = format!("{asker}/{RUN}/{call_id}");
        if DecisionSubjectKey::new(verbatim.clone()).is_ok() {
            assert_eq!(key.as_str(), verbatim);
        } else {
            assert!(key.as_str().starts_with(ASK_USER_BOUNDED_SUBJECT_PREFIX));
            assert_eq!(key.as_str(), bounded_key(asker, &run_id, call_id));
        }
    }
    assert_eq!(ASK_USER_BOUNDED_SUBJECT_PREFIX, "ask_user.call:");
}

/// The hold of a call exactly as 0.9.15 builds it: the verbatim key, the
/// operation id named after the decision id it derives, the body, its
/// digest, the rendering and the renderer.
fn hold_as_0_9_15(asker: &str, run_id: &RunId, call_id: &str) -> DecisionHoldRequest {
    let kind = DecisionKind::new("ask_user.questions").expect("kind");
    let subject_key =
        DecisionSubjectKey::new(format!("{asker}/{run_id}/{call_id}")).expect("subject key");
    let decision_id = DecisionId::derive(&kind, &subject_key);
    let body = AskUserBody {
        body: "ask_user.questions.v1".to_owned(),
        asker: asker.to_owned(),
        questions: question_set(),
    };
    let text = serde_json::to_string(&body).expect("the body");
    let raw = serde_json::value::RawValue::from_string(text.clone()).expect("raw");
    DecisionHoldRequest::new(
        kind,
        subject_key,
        DecisionOperationId::new(format!("ask_user:{decision_id}")).expect("operation id"),
        Sha256Digest::of(text.as_bytes()),
        DecisionRequestBody::new(raw).expect("request body"),
        requester(),
        approver_id(),
        DEADLINE,
        DecisionRendering::new(rendering_text(&body.questions)).expect("rendering"),
        DecisionRenderer::new("ask_user.render-v1").expect("renderer"),
    )
}

/// A call whose key 0.9.15 accepts keeps that key and its decision id,
/// byte for byte: the short call, and the longest one that fits.
#[test]
fn a_call_that_fits_keeps_the_key_and_the_decision_id_0_9_15_computed() {
    let (_clock, service) = fixture();
    let run_id = run();

    let short =
        hold_ask_user(&service, ASKED_AT, hold_for(ASKER, &run_id, "call-1")).expect("the hold");
    assert_eq!(
        short.descriptor.subject_key.as_str(),
        "member:planner/018f4e2a-7c3b-7d11-9a55-3f2b6c8d9e01/call-1"
    );
    assert_eq!(
        short.decision_id.as_str(),
        "decision:6ce7a78b7fa33f663f1cd7ded8a81105084859438daba6d69abd786db0bbebf7"
    );

    // The longest verbatim key a decision takes: 1,024 bytes.
    let longest_call = "c".repeat(DECISION_REFERENCE_MAX_BYTES - ASKER.len() - RUN.len() - 2);
    let longest = hold_ask_user(&service, ASKED_AT, hold_for(ASKER, &run_id, &longest_call))
        .expect("the hold");
    assert_eq!(
        longest.descriptor.subject_key.as_str(),
        format!("{ASKER}/{RUN}/{longest_call}")
    );
    assert_eq!(longest.descriptor.subject_key.as_str().len(), 1_024);
    assert_eq!(
        longest.decision_id.as_str(),
        "decision:1a2478c89c19de953d67f2030c56258896f7c9980f6e19e3109008749db7e449"
    );
}

/// One byte more than a verbatim key takes: the call is held under the
/// bounded key.
#[test]
fn a_call_one_byte_over_the_limit_is_held_under_the_bounded_key() {
    let (_clock, service) = fixture();
    let run_id = run();
    let call_id = "c".repeat(DECISION_REFERENCE_MAX_BYTES - ASKER.len() - RUN.len() - 1);
    assert_eq!(format!("{ASKER}/{RUN}/{call_id}").len(), 1_025);

    let decision = hold_ask_user(&service, ASKED_AT, hold_for(ASKER, &run_id, &call_id))
        .expect("a call one byte over the limit is held");
    assert_bounded(&decision, ASKER, &run_id, &call_id);
    assert_eq!(
        decision.descriptor.subject_key.as_str(),
        "ask_user.call:018f4e2a-7c3b-7d11-9a55-3f2b6c8d9e01:sha256:\
         ce18b35131419e5e5f1589f401aaa9a3baca387143a049921cb0d189318c1457"
    );
    assert_eq!(
        decision.decision_id.as_str(),
        "decision:dd01984dffa015ea29b394514bbf2ee4cfa81ad8497bdb96c37b8e6c00504f97"
    );
}

/// A call whose provider issued a 2,000-byte tool-call id is held, a replay
/// answers the decision it held, and a person's answer is executed and
/// noticed once.
#[tokio::test]
async fn a_call_with_a_long_tool_call_id_is_held_replayed_and_answered() {
    let (clock, service) = fixture();
    let notices = Arc::new(RecordingNotices::default());
    let mut executor = executor_loop(&service, &notices);
    let run_id = run();
    let call_id = "c".repeat(2_000);

    let decision = hold_ask_user(&service, ASKED_AT, hold_for(ASKER, &run_id, &call_id))
        .expect("a call with a long tool-call id is held");
    assert_bounded(&decision, ASKER, &run_id, &call_id);
    assert_eq!(
        decision.descriptor.subject_key.as_str(),
        "ask_user.call:018f4e2a-7c3b-7d11-9a55-3f2b6c8d9e01:sha256:\
         3d72db732c0b64f3ebe92f77053fd5aefe84cb6bfb0f10978aed680a3ad39ed8"
    );
    assert_eq!(
        decision.decision_id.as_str(),
        "decision:39a62de2ef1568037a12562e718a048cc9371a6a5d4d8f26c54e2e1bdbb8e61c"
    );
    assert_eq!(decision.state.phase, DecisionPhase::Open);
    assert_eq!(decision.descriptor.deadline_at_ms, DEADLINE);
    assert_eq!(
        AskUserBody::of(&decision).expect("the held body"),
        AskUserBody {
            body: "ask_user.questions.v1".to_owned(),
            asker: ASKER.to_owned(),
            questions: question_set(),
        }
    );
    assert_eq!(
        ask_user_result_of(&decision).expect("the result"),
        AskUserResult::asked(decision.decision_id.as_str(), DEADLINE)
    );

    // A replay a minute later answers the decision the call held.
    let later = ASKED_AT + 60_000;
    clock.set(later);
    let replayed = hold_ask_user(&service, later, hold_for(ASKER, &run_id, &call_id))
        .expect("a replay answers the decision it held");
    assert_eq!(replayed, decision);
    assert_eq!(replayed.revision, 1, "a replay writes nothing");

    // Another long call id, of the same length, holds a decision of its own.
    let mut other_call = "c".repeat(1_999);
    other_call.push('d');
    let other = hold_ask_user(&service, later, hold_for(ASKER, &run_id, &other_call))
        .expect("another call");
    assert_bounded(&other, ASKER, &run_id, &other_call);
    assert_ne!(other.decision_id, decision.decision_id);
    assert_eq!(
        service
            .count(&DecisionListFilter::default())
            .expect("count"),
        2
    );

    // The same call with other questions is still a hold conflict.
    let mut another_set = hold_for(ASKER, &run_id, &call_id);
    another_set.questions = other_set();
    let error = hold_ask_user(&service, later, another_set).expect_err("another set");
    assert_eq!(refusal_of(&error), DecisionRefusal::HoldConflict);

    // A person answers; the asker is told once.
    answered(&service, &decision, vec![choice("notify", "yes")]);
    assert_eq!(
        settle(&mut executor, &decision.decision_id).await,
        vec![
            DecisionPassOutcome::Executed,
            DecisionPassOutcome::NoticeSettled
        ]
    );
    assert_eq!(
        notices.delivered(),
        vec![AskUserNotice {
            decision_id: decision.decision_id.clone(),
            asker: ASKER.to_owned(),
            result: AskUserResult::ended(
                AskUserOutcome::Answered,
                decision.decision_id.as_str(),
                vec![unanswered("channel"), choice("notify", "yes")],
            ),
        }]
    );
    assert!(executor.pass().await.outcomes.is_empty());
    assert_eq!(notices.delivered().len(), 1);
}

/// An agent whose host names it with a 2,000-byte key asks: the set is
/// held, the whole key is in the held body, and the notice goes to it.
#[tokio::test]
async fn a_call_of_an_agent_with_a_long_key_is_held_and_answered() {
    let (_clock, service) = fixture();
    let notices = Arc::new(RecordingNotices::default());
    let mut executor = executor_loop(&service, &notices);
    let run_id = run();
    let asker = "a".repeat(2_000);

    let decision = hold_ask_user(&service, ASKED_AT, hold_for(&asker, &run_id, "call-1"))
        .expect("a call of an agent with a long key is held");
    assert_bounded(&decision, &asker, &run_id, "call-1");
    assert_eq!(
        decision.descriptor.subject_key.as_str(),
        "ask_user.call:018f4e2a-7c3b-7d11-9a55-3f2b6c8d9e01:sha256:\
         d4b0e9527689de10ec524f422d4f268659231ed81fd80316aab22423cb056301"
    );
    assert_eq!(
        decision.decision_id.as_str(),
        "decision:06f1ba1a9cb419178f05a9a8d6c440393592aafa3e62914e2886c35d9f5ff478"
    );
    assert_eq!(
        AskUserBody::of(&decision).expect("the held body").asker,
        asker
    );
    let replayed = hold_ask_user(&service, ASKED_AT, hold_for(&asker, &run_id, "call-1"))
        .expect("a replay answers the decision it held");
    assert_eq!(replayed, decision);

    answered(&service, &decision, vec![choice("channel", "stable")]);
    assert_eq!(
        settle(&mut executor, &decision.decision_id).await,
        vec![
            DecisionPassOutcome::Executed,
            DecisionPassOutcome::NoticeSettled
        ]
    );
    let delivered = notices.delivered();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].decision_id, decision.decision_id);
    assert_eq!(delivered[0].asker, asker);
    assert_eq!(delivered[0].result.outcome, AskUserOutcome::Answered);
}

/// Two calls of one run whose verbatim keys would be the same string (the
/// separator falls elsewhere), each too long for a verbatim key: each holds
/// a decision of its own.
#[test]
fn two_long_calls_that_differ_only_in_where_the_separator_falls_do_not_collide() {
    let (_clock, service) = fixture();
    let run_id = run();
    let tail = "t".repeat(2_000);
    let left = ("a".to_owned(), format!("b/{RUN}/{tail}"));
    let right = (format!("a/{RUN}/b"), tail);
    assert_eq!(
        format!("{}/{RUN}/{}", left.0, left.1),
        format!("{}/{RUN}/{}", right.0, right.1),
        "the verbatim keys would be one string"
    );

    let first = hold_ask_user(&service, ASKED_AT, hold_for(&left.0, &run_id, &left.1))
        .expect("the first call is held");
    let second = hold_ask_user(&service, ASKED_AT, hold_for(&right.0, &run_id, &right.1))
        .expect("the second call is held");
    assert_bounded(&first, &left.0, &run_id, &left.1);
    assert_bounded(&second, &right.0, &run_id, &right.1);
    assert_ne!(first.descriptor.subject_key, second.descriptor.subject_key);
    assert_ne!(first.decision_id, second.decision_id);
    assert_eq!(
        first.descriptor.subject_key.as_str(),
        "ask_user.call:018f4e2a-7c3b-7d11-9a55-3f2b6c8d9e01:sha256:\
         b8d409b1e82d0d407b9111983904dace5eb8ea86d16ed5f07b51544f0d436ab8"
    );
    assert_eq!(
        second.descriptor.subject_key.as_str(),
        "ask_user.call:018f4e2a-7c3b-7d11-9a55-3f2b6c8d9e01:sha256:\
         a5d48ae5cbda6a7774564c499ac4181ff38c568bf430f0c117f893a7e088ce87"
    );
    assert_eq!(AskUserBody::of(&first).expect("body").asker, left.0);
    assert_eq!(AskUserBody::of(&second).expect("body").asker, right.0);
    assert_eq!(
        service
            .count(&DecisionListFilter::default())
            .expect("count"),
        2
    );
}

/// The same two calls short enough for a verbatim key have one key, as in
/// 0.9.15: the second is refused as a hold conflict (it names another
/// asker), and the first call's set stands. It never answers the other
/// call's set.
#[test]
fn two_fitting_calls_with_one_verbatim_key_are_a_hold_conflict() {
    let (_clock, service) = fixture();
    let run_id = run();
    let left = ("a".to_owned(), format!("b/{RUN}/t"));
    let right = (format!("a/{RUN}/b"), "t".to_owned());

    let first = hold_ask_user(&service, ASKED_AT, hold_for(&left.0, &run_id, &left.1))
        .expect("the first call is held");
    assert_eq!(
        first.descriptor.subject_key.as_str(),
        format!("a/{RUN}/b/{RUN}/t")
    );
    let error = hold_ask_user(&service, ASKED_AT, hold_for(&right.0, &run_id, &right.1))
        .expect_err("the second call names the first call's key");
    assert_eq!(refusal_of(&error), DecisionRefusal::HoldConflict);
    assert_eq!(service.get(&first.decision_id).expect("read"), first);
    assert_eq!(
        service
            .count(&DecisionListFilter::default())
            .expect("count"),
        1
    );
}

/// A call id or an asker with a control character makes no valid verbatim
/// key: the call is held under the bounded key, and the asker is kept whole
/// in the held body.
#[test]
fn a_call_with_a_control_character_in_its_id_or_its_asker_is_held() {
    let (_clock, service) = fixture();
    let run_id = run();

    let by_call = hold_ask_user(&service, ASKED_AT, hold_for(ASKER, &run_id, "call\n1"))
        .expect("a call id with a line feed is held");
    assert_bounded(&by_call, ASKER, &run_id, "call\n1");
    assert_eq!(
        by_call.descriptor.subject_key.as_str(),
        "ask_user.call:018f4e2a-7c3b-7d11-9a55-3f2b6c8d9e01:sha256:\
         0a0f5b6bc9da8043ef954e3116ed2fef8c30305b83220b99690b9e5507ee5b0f"
    );
    assert_eq!(
        by_call.decision_id.as_str(),
        "decision:72a563ed152d453df0fae7d1578ab59a00b370a4ca4b63110816b9de9389a30d"
    );
    assert_ne!(
        by_call.decision_id,
        hold_ask_user(&service, ASKED_AT, hold_for(ASKER, &run_id, "call1"))
            .expect("the call id without it")
            .decision_id
    );

    let asker = "member:\u{7}planner\tof the\u{9f}night";
    let by_asker = hold_ask_user(&service, ASKED_AT, hold_for(asker, &run_id, "call-1"))
        .expect("an asker with control characters is held");
    assert_bounded(&by_asker, asker, &run_id, "call-1");
    assert_eq!(
        AskUserBody::of(&by_asker).expect("the held body").asker,
        asker
    );
    // The row reads back from its stored form with the asker whole.
    let stored = serde_json::to_string(&by_asker).expect("encode the row");
    let read: StoredDecision = serde_json::from_str(&stored).expect("decode the row");
    assert_eq!(read, by_asker);
    assert_eq!(AskUserBody::of(&read).expect("the held body").asker, asker);
    let projected = serde_json::to_value(wire_decision(&by_asker, true)).expect("encode");
    assert_eq!(projected["request"]["asker"], serde_json::json!(asker));
}

/// A store 0.9.15 wrote holds verbatim keys: the next version finds such a
/// set by a replay of its call, lists it, and a person's answer is executed
/// and noticed once.
#[tokio::test]
async fn a_set_held_under_the_key_0_9_15_wrote_is_found_replayed_and_answered() {
    let dir = tempfile::tempdir().expect("tempdir");
    let clock = ManualClock::at(ASKED_AT);
    let open_store =
        || -> Arc<dyn DecisionStore> { Arc::new(SqliteDecisionStore::in_store_path(dir.path())) };
    let notices = Arc::new(RecordingNotices::default());
    let run_id = run();

    // The row as 0.9.15 writes it.
    let earlier = service_over(open_store(), &clock);
    let written = earlier
        .hold(hold_as_0_9_15(ASKER, &run_id, "call-1"), true)
        .expect("the hold 0.9.15 makes");
    assert_eq!(
        written.decision_id.as_str(),
        "decision:6ce7a78b7fa33f663f1cd7ded8a81105084859438daba6d69abd786db0bbebf7"
    );
    drop(earlier);

    // The next process replays the call: the same decision, nothing written.
    let owner = service_over(open_store(), &clock);
    let mut executor = executor_loop(&owner, &notices);
    let replayed = hold_ask_user(
        &owner,
        ASKED_AT + 60_000,
        hold_for(ASKER, &run_id, "call-1"),
    )
    .expect("a replay finds the set 0.9.15 held");
    assert_eq!(replayed, written);
    assert_eq!(replayed.revision, 1, "a replay writes nothing");
    assert_eq!(
        owner.count(&DecisionListFilter::default()).expect("count"),
        1
    );
    let open = owner
        .list(
            &DecisionListFilter::default()
                .with_kind(ask_user_questions_kind().expect("kind"))
                .with_phase(DecisionPhase::Open),
            None,
            DECISION_PAGE_MAX,
        )
        .expect("list");
    assert_eq!(open.decisions, vec![written.clone()]);

    answered(&owner, &written, vec![choice("notify", "no")]);
    assert_eq!(
        settle(&mut executor, &written.decision_id).await,
        vec![
            DecisionPassOutcome::Executed,
            DecisionPassOutcome::NoticeSettled
        ]
    );
    let delivered = notices.delivered();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].decision_id, written.decision_id);
    assert_eq!(delivered[0].asker, ASKER);
}

/// A set held for a call with a long tool-call id survives a restart: the
/// next process reads it by its decision id, a replay of the call answers
/// it, a person answers it, and its asker is told once.
#[tokio::test]
async fn a_long_id_set_survives_a_restart_and_is_answered_by_its_decision_id() {
    let dir = tempfile::tempdir().expect("tempdir");
    let clock = ManualClock::at(ASKED_AT);
    let open_store =
        || -> Arc<dyn DecisionStore> { Arc::new(SqliteDecisionStore::in_store_path(dir.path())) };
    let notices = Arc::new(RecordingNotices::default());
    let run_id = run();
    let call_id = "c".repeat(2_000);

    let first_owner = service_over(open_store(), &clock);
    let mut first_loop = executor_loop(&first_owner, &notices);
    let decision = hold_ask_user(&first_owner, ASKED_AT, hold_for(ASKER, &run_id, &call_id))
        .expect("a call with a long tool-call id is held");
    let decision_id = decision.decision_id.clone();
    assert_bounded(&decision, ASKER, &run_id, &call_id);
    assert!(first_loop.pass().await.outcomes.is_empty());
    drop(first_loop);
    drop(first_owner);

    // The next process: the set is read by its decision id, as it was held,
    // and a replay of the call answers it.
    let second_owner = service_over(open_store(), &clock);
    let mut second_loop = executor_loop(&second_owner, &notices);
    let found = second_owner.get(&decision_id).expect("the set survived");
    assert_eq!(found, decision);
    assert_eq!(found.state.phase, DecisionPhase::Open);
    let replayed = hold_ask_user(
        &second_owner,
        ASKED_AT + 60_000,
        hold_for(ASKER, &run_id, &call_id),
    )
    .expect("a replay after the restart answers the set");
    assert_eq!(replayed, decision);
    assert!(second_loop.pass().await.outcomes.is_empty());

    answered(&second_owner, &found, vec![choice("channel", "beta")]);
    assert_eq!(
        settle(&mut second_loop, &decision_id).await,
        vec![
            DecisionPassOutcome::Executed,
            DecisionPassOutcome::NoticeSettled
        ]
    );
    let expected = AskUserResult::ended(
        AskUserOutcome::Answered,
        decision_id.as_str(),
        vec![choice("channel", "beta"), unanswered("notify")],
    );
    assert_eq!(
        notices.delivered(),
        vec![AskUserNotice {
            decision_id: decision_id.clone(),
            asker: ASKER.to_owned(),
            result: expected.clone(),
        }]
    );

    // A third process finds nothing left to do, and reads the same result.
    drop(second_loop);
    drop(second_owner);
    let third_owner = service_over(open_store(), &clock);
    let mut third_loop = executor_loop(&third_owner, &notices);
    assert!(third_loop.pass().await.outcomes.is_empty());
    assert_eq!(notices.delivered().len(), 1, "delivered once");
    let settled = third_owner.get(&decision_id).expect("read");
    assert_eq!(settled.state.notice, DecisionNotice::Delivered);
    assert_eq!(ask_user_result_of(&settled).expect("the result"), expected);
}

/// The decision list and a decision's wire form show a set held for a call
/// with a long tool-call id, beside one held under a verbatim key: its key
/// is within a reference's limit and names its run.
#[test]
fn the_decision_list_shows_a_long_id_set() {
    let (_clock, service) = fixture();
    let run_id = run();
    let call_id = "c".repeat(2_000);
    let short =
        hold_ask_user(&service, ASKED_AT, hold_for(ASKER, &run_id, "call-1")).expect("the hold");
    let long = hold_ask_user(&service, ASKED_AT, hold_for(ASKER, &run_id, &call_id))
        .expect("a call with a long tool-call id is held");

    let listing = service
        .list(
            &DecisionListFilter::default()
                .with_kind(ask_user_questions_kind().expect("kind"))
                .with_phase(DecisionPhase::Open),
            None,
            DECISION_PAGE_MAX,
        )
        .expect("list");
    assert!(listing.unreadable.is_empty(), "{:?}", listing.unreadable);
    let mut listed: Vec<&str> = listing
        .decisions
        .iter()
        .map(|row| row.decision_id.as_str())
        .collect();
    listed.sort_unstable();
    let mut held = vec![short.decision_id.as_str(), long.decision_id.as_str()];
    held.sort_unstable();
    assert_eq!(listed, held);

    // A page of one row a read: the long-id set is a row and a cursor like
    // any other.
    let mut walked = Vec::new();
    let mut after: Option<String> = None;
    for _ in 0..4 {
        let page = service
            .list(&DecisionListFilter::default(), after.as_deref(), 1)
            .expect("page");
        walked.extend(page.decisions.iter().map(|row| row.decision_id.to_string()));
        match page.next_after {
            Some(next) => after = Some(next),
            None => break,
        }
    }
    walked.sort_unstable();
    assert_eq!(walked, held);

    let wire = wire_decision_listing(&listing);
    let projected = wire
        .decisions
        .iter()
        .find(|decision| decision.decision_id == long.decision_id.as_str())
        .expect("the long-id set is listed");
    assert_eq!(projected.kind, "ask_user.questions");
    assert_eq!(
        projected.subject_key,
        "ask_user.call:018f4e2a-7c3b-7d11-9a55-3f2b6c8d9e01:sha256:\
         3d72db732c0b64f3ebe92f77053fd5aefe84cb6bfb0f10978aed680a3ad39ed8"
    );
    assert!(projected.subject_key.len() <= DECISION_REFERENCE_MAX_BYTES);
    assert_eq!(
        serde_json::to_value(wire_decision(&long, true)).expect("encode")["request"]["asker"],
        serde_json::json!(ASKER)
    );
}

/// An answer for a decision id nothing holds is refused as not found, as
/// before: so for the id a bounded key nothing was held under derives.
#[test]
fn an_answer_for_an_id_nothing_holds_is_not_found() {
    let (_clock, service) = fixture();
    let held = held(&service);
    let unknown = DecisionId::derive(
        &ask_user_questions_kind().expect("kind"),
        &DecisionSubjectKey::new(bounded_key(ASKER, &run(), &"c".repeat(2_000)))
            .expect("subject key"),
    );
    assert_eq!(
        unknown.as_str(),
        "decision:39a62de2ef1568037a12562e718a048cc9371a6a5d4d8f26c54e2e1bdbb8e61c"
    );

    let body = AskUserBody::of(&held).expect("the held body");
    let answers = AskUserAnswers::for_set(&body.questions, vec![choice("notify", "yes")])
        .expect("valid answers");
    let attested =
        attest_ask_user_answers(person_decides(&held, true), &answers).expect("attested");
    assert!(matches!(
        service.decide(&unknown, &attested, &MayAnswer),
        Err(DecisionError::NotFound(_))
    ));
    assert!(matches!(
        service.get(&unknown),
        Err(DecisionError::NotFound(_))
    ));
    assert_eq!(
        service.get(&held.decision_id).expect("read"),
        held,
        "the held set is untouched"
    );
    assert_eq!(
        service
            .count(&DecisionListFilter::default())
            .expect("count"),
        1
    );
}

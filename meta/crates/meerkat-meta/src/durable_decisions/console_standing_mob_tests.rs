//! The passes of the Console's decision methods over a request whose answer
//! depends on the mob that stands under a mob id
//! ([`handle_console_decision_rpc`]): a pass that needs a standing mob the
//! door has not read answers the mob id, gives no answer and changes
//! nothing, and a mob id is asked only for a decision the caller would
//! otherwise see, and never for a caller who may address every mob.

use meerkat_core::approval::decision::{
    ApproverId, DecisionHoldRequest, DecisionKind, DecisionOperationId, DecisionRenderer,
    DecisionRendering, DecisionRequestBody, DecisionSubjectKey, Sha256Digest,
};
use meerkat_core::{PrincipalKind, PrincipalRef, TrustDomainId};

use super::*;
use crate::access::{AccessController, AccessRule};

/// The launch mob of the rows' Console.
const LAUNCH_MOB: &str = "launch";
/// A mob the rows' host serves beside its launch mob.
const SECOND_MOB: &str = "second";
/// The incarnation the rows' held change names.
const HELD_FOR: &str = "incarnation-of-the-held-change";
/// An administrator of the rows' access configuration.
const ADMIN: &str = "root@example.test";
/// A person who administers decisions by a rule, and whom a rule lets
/// address [`SECOND_MOB`].
const OF_THE_SECOND_MOB: &str = "second@example.test";
/// A person who administers decisions by a rule, and whose rules name no
/// mob.
const OF_THE_LAUNCH_MOB: &str = "launch@example.test";
/// A person a rule lets address [`SECOND_MOB`] and read decisions, who
/// holds no approver id and administers nothing.
const A_READER: &str = "reader@example.test";

/// What a row answers: a step of its scene that failed ends the row with
/// what the step said.
type Row = Result<(), String>;

/// The failure of the step `step` of a row's scene, as the row answers it.
fn failed<E: std::fmt::Debug>(step: &'static str) -> impl Fn(E) -> String {
    move |error| format!("{step}: {error:?}")
}

/// The access configuration of the rows.
fn controller() -> Result<AccessController, String> {
    let mut access: crate::AccessControlConfig =
        toml::from_str(&format!("enabled = true\nadmins = [\"{ADMIN}\"]\n"))
            .map_err(failed("the access configuration"))?;
    access.rules = vec![
        AccessRule {
            id: "views-the-second-mob".to_string(),
            subjects: vec![OF_THE_SECOND_MOB.to_string(), A_READER.to_string()],
            actions: vec![crate::access::ACTION_AGENT_VIEW.to_string()],
            mobs: vec![SECOND_MOB.to_string()],
            ..AccessRule::default()
        },
        AccessRule {
            id: "administers-decisions".to_string(),
            subjects: vec![OF_THE_SECOND_MOB.to_string(), OF_THE_LAUNCH_MOB.to_string()],
            actions: vec![
                ACTION_DECISIONS_VIEW.to_string(),
                ACTION_DECISIONS_ADMIN.to_string(),
            ],
            ..AccessRule::default()
        },
        AccessRule {
            id: "reads-decisions".to_string(),
            subjects: vec![A_READER.to_string()],
            actions: vec![ACTION_DECISIONS_VIEW.to_string()],
            ..AccessRule::default()
        },
    ];
    AccessController::new(access).map_err(failed("the access controller"))
}

/// Decisions kept in `state`, with one open held change of an agent of
/// [`SECOND_MOB`] whose body names the incarnation [`HELD_FOR`]: the
/// decisions, and the held change's id.
fn decisions_with_a_held_change(
    state: &std::path::Path,
) -> Result<(RuntimeDecisions, String), String> {
    let owner = meerkat::ApprovalIssuerSelection::Persistent {
        store_path: state.to_path_buf(),
    }
    .open_decisions();
    let requester = PrincipalRef::in_domain(
        PrincipalKind::Human,
        "uid-1000",
        TrustDomainId::new("rkat-realm-meta").map_err(failed("domain"))?,
    )
    .map_err(failed("requester"))?;
    let body = json!({"mob_id": SECOND_MOB, "mob_incarnation": HELD_FOR});
    let held = owner
        .hold(
            DecisionHoldRequest::new(
                DecisionKind::new("identity.remove").map_err(failed("kind"))?,
                DecisionSubjectKey::new(format!("{SECOND_MOB}@{HELD_FOR}/remove-the-scout"))
                    .map_err(failed("subject"))?,
                DecisionOperationId::new("operation:remove-the-scout")
                    .map_err(failed("operation"))?,
                Sha256Digest::of(body.to_string().as_bytes()),
                DecisionRequestBody::from_value(&body).map_err(failed("body"))?,
                DecisionPrincipalKey::of(&requester).map_err(failed("key"))?,
                ApproverId::new("team:owners").map_err(failed("approver"))?,
                u64::MAX / 2,
                DecisionRendering::new("Remove the scout.").map_err(failed("rendering"))?,
                DecisionRenderer::new("render.v1").map_err(failed("renderer"))?,
            ),
            false,
        )
        .map_err(failed("hold"))?;
    Ok((
        RuntimeDecisions::unavailable().with_owner_for_test(owner),
        held.decision_id.to_string(),
    ))
}

/// One pass of `method` about the decision `decision_id`, as the person
/// `subject` of the rows' access configuration, with the standing mobs
/// `standing_mobs` read.
fn pass(
    decisions: &RuntimeDecisions,
    subject: &str,
    standing_mobs: &StandingMobs,
    method: &str,
    decision_id: &str,
) -> Result<ConsoleDecisionAnswer, String> {
    pass_with(
        decisions,
        subject,
        standing_mobs,
        method,
        json!({"decision_id": decision_id}),
    )
}

/// One pass of `method` with the parameters `params`, as the person
/// `subject` of the rows' access configuration, with the standing mobs
/// `standing_mobs` read.
fn pass_with(
    decisions: &RuntimeDecisions,
    subject: &str,
    standing_mobs: &StandingMobs,
    method: &str,
    params: Value,
) -> Result<ConsoleDecisionAnswer, String> {
    let view = controller()?.view_for_subject(Some(subject));
    let caller = ConsoleDecisionCaller {
        access_view: Some(&view),
        requester: None,
        launch_mob: Some(meerkat_mob::MobId::from(LAUNCH_MOB)),
    };
    handle_console_decision_rpc(
        decisions,
        &caller,
        standing_mobs,
        &ConsoleChangeChecks {
            other_caller: None,
            caller_current: &|| Ok(()),
        },
        false,
        method,
        &json!(1),
        params,
    )
    .ok_or_else(|| format!("{method} is no decision method the Console serves"))
}

/// The standing mobs a door read: [`SECOND_MOB`] at `incarnation`, or with
/// no mob under its id.
fn read_as(incarnation: Option<&str>) -> StandingMobs {
    StandingMobs::from([(SECOND_MOB.to_string(), incarnation.map(ToString::to_string))])
}

/// What a pass answered, in a word for the rows: the mob ids it asks the
/// door to read, the kind of its refusal, or `served`.
fn in_a_word(answer: &ConsoleDecisionAnswer) -> String {
    match answer {
        ConsoleDecisionAnswer::StandingMobsToRead(mob_ids) => {
            format!(
                "reads {}",
                mob_ids.iter().cloned().collect::<Vec<_>>().join(",")
            )
        }
        ConsoleDecisionAnswer::Answer(answer) => match answer["error"]["data"]["kind"].as_str() {
            Some(kind) => kind.to_string(),
            None if answer["result"].is_object() => "served".to_string(),
            None => answer.to_string(),
        },
    }
}

/// Whether the decision `decision_id` is still open.
fn open(decisions: &RuntimeDecisions, decision_id: &str) -> bool {
    DecisionId::new(decision_id).is_ok_and(|decision_id| {
        decisions
            .owner()
            .get(&decision_id)
            .is_ok_and(|row| row.state.phase == DecisionPhase::Open)
    })
}

/// A pass that needs a standing mob the door has not read answers the mob
/// id and changes nothing; with the mob read, the withdraw is served only
/// while the mob the change is held for stands. A caller who administers
/// decisions by a rule, and addresses the held change's mob by a rule,
/// asks to withdraw it: with nothing read the pass answers the mob id and
/// the change stays open; with no mob under the id, and with a mob of
/// another incarnation there, it answers as for an id no decision has and
/// the change stays open; with the mob the change is held for there, it
/// withdraws the change.
#[test]
fn a_withdraw_pass_that_needs_a_standing_mob_asks_for_it_and_withdraws_nothing() -> Row {
    let state = tempfile::tempdir().map_err(failed("state dir"))?;
    let (decisions, held) = decisions_with_a_held_change(state.path())?;
    let withdraw = |standing_mobs: &StandingMobs| -> Result<(String, bool), String> {
        let answer = pass(
            &decisions,
            OF_THE_SECOND_MOB,
            standing_mobs,
            WITHDRAW_METHOD,
            &held,
        )?;
        Ok((in_a_word(&answer), open(&decisions, &held)))
    };
    let with_nothing_read = withdraw(&StandingMobs::new())?;
    let with_no_mob_there = withdraw(&read_as(None))?;
    let with_another_mob_there = withdraw(&read_as(Some("another-incarnation")))?;
    let with_its_mob_there = withdraw(&read_as(Some(HELD_FOR)))?;
    assert_eq!(
        with_nothing_read,
        (format!("reads {SECOND_MOB}"), true),
        "with nothing read the pass answers the mob id and the change stays open"
    );
    assert_eq!(
        with_no_mob_there,
        ("decision_not_found".to_string(), true),
        "with no mob under the id the withdraw answers as for an id no decision has"
    );
    assert_eq!(
        with_another_mob_there,
        ("decision_not_found".to_string(), true),
        "with a mob of another incarnation there it answers the same"
    );
    assert_eq!(
        with_its_mob_there,
        ("served".to_string(), false),
        "with the mob the change is held for there, the change is withdrawn"
    );
    Ok(())
}

/// A read pass that needs a standing mob the door has not read gives no
/// answer: the get and the rendering of a rule's caller answer the mob id
/// with nothing read, the decision with its mob there, and as for an id no
/// decision has with no mob or another mob there.
#[test]
fn a_read_pass_that_needs_a_standing_mob_gives_no_answer() -> Row {
    let state = tempfile::tempdir().map_err(failed("state dir"))?;
    let (decisions, held) = decisions_with_a_held_change(state.path())?;
    for method in [GET_METHOD, RENDERING_METHOD] {
        let read = |standing_mobs: &StandingMobs| -> Result<String, String> {
            Ok(in_a_word(&pass(
                &decisions,
                OF_THE_SECOND_MOB,
                standing_mobs,
                method,
                &held,
            )?))
        };
        assert_eq!(
            [
                read(&StandingMobs::new())?,
                read(&read_as(Some(HELD_FOR)))?,
                read(&read_as(None))?,
                read(&read_as(Some("another-incarnation")))?,
            ],
            [
                format!("reads {SECOND_MOB}"),
                "served".to_string(),
                "decision_not_found".to_string(),
                "decision_not_found".to_string(),
            ],
            "{method}: nothing read, its mob there, no mob there, another mob there"
        );
    }
    Ok(())
}

/// A mob id is asked only for a decision the caller would otherwise see,
/// and never for a caller who may address every mob. With nothing read, an
/// administrator of the access configuration is answered the decision; a
/// caller whose rules name no mob, and a caller a rule lets address the
/// mob who neither holds the decision's approver id nor administers
/// decisions, are answered as for an id no decision has: no pass of theirs
/// asks for a mob.
#[test]
fn a_mob_is_asked_only_for_a_decision_the_caller_would_otherwise_see() -> Row {
    let state = tempfile::tempdir().map_err(failed("state dir"))?;
    let (decisions, held) = decisions_with_a_held_change(state.path())?;
    let get = |subject: &str| -> Result<String, String> {
        Ok(in_a_word(&pass(
            &decisions,
            subject,
            &StandingMobs::new(),
            GET_METHOD,
            &held,
        )?))
    };
    assert_eq!(
        [get(ADMIN)?, get(OF_THE_LAUNCH_MOB)?, get(A_READER)?],
        [
            "served".to_string(),
            "decision_not_found".to_string(),
            "decision_not_found".to_string(),
        ],
        "an administrator, a caller whose rules name no mob, and a caller who would not see \
         the decision"
    );
    Ok(())
}

/// A caller whose rules do not reach a mob makes the door read nothing for
/// it. Two callers administer decisions by a rule, so each would see the
/// held change by the kind's rule; a rule lets one of them address the
/// change's mob, and the rules of the other name no mob. With nothing
/// read, every pass of the first (a list, a get, a rendering, a withdraw)
/// answers the mob id for the door to read. No pass of the other names a
/// mob: its list names no decision, its get, rendering and withdraw answer
/// as for an id no decision has, and the change stays open.
#[test]
fn a_caller_whose_rules_do_not_reach_a_mob_makes_the_door_read_nothing() -> Row {
    let state = tempfile::tempdir().map_err(failed("state dir"))?;
    let (decisions, held) = decisions_with_a_held_change(state.path())?;
    let nothing_read = StandingMobs::new();
    let passes = |subject: &str| -> Result<Vec<ConsoleDecisionAnswer>, String> {
        let mut answers = vec![pass_with(
            &decisions,
            subject,
            &nothing_read,
            LIST_METHOD,
            Value::Null,
        )?];
        for method in [GET_METHOD, RENDERING_METHOD, WITHDRAW_METHOD] {
            answers.push(pass(&decisions, subject, &nothing_read, method, &held)?);
        }
        Ok(answers)
    };
    let of_a_caller_the_rules_reach: Vec<String> =
        passes(OF_THE_SECOND_MOB)?.iter().map(in_a_word).collect();
    let of_a_caller_they_do_not_reach = passes(OF_THE_LAUNCH_MOB)?;
    assert_eq!(
        of_a_caller_the_rules_reach,
        [(); 4].map(|()| format!("reads {SECOND_MOB}")),
        "each pass of a caller a rule lets address the mob answers the mob id to read"
    );
    assert!(
        of_a_caller_they_do_not_reach
            .iter()
            .all(|answer| matches!(answer, ConsoleDecisionAnswer::Answer(_))),
        "no pass of a caller whose rules name no mob asks the door to read a mob: {:?}",
        of_a_caller_they_do_not_reach
            .iter()
            .map(in_a_word)
            .collect::<Vec<_>>()
    );
    let listed = match of_a_caller_they_do_not_reach.first() {
        Some(ConsoleDecisionAnswer::Answer(answer)) => {
            answer["result"]["decisions"].as_array().map(|decisions| {
                decisions.len() + answer["result"]["statuses"].as_array().map_or(0, Vec::len)
            })
        }
        _ => None,
    };
    assert_eq!(listed, Some(0), "its list names no decision");
    assert_eq!(
        of_a_caller_they_do_not_reach
            .iter()
            .skip(1)
            .map(in_a_word)
            .collect::<Vec<_>>(),
        [(); 3].map(|()| "decision_not_found".to_string()),
        "its get, rendering and withdraw answer as for an id no decision has"
    );
    assert!(open(&decisions, &held), "and the change stays open");
    Ok(())
}

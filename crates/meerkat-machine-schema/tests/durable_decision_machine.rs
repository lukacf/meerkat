//! Focused behavioural tests for the canonical `DurableDecisionMachine`.
//!
//! The machine is record-scoped: every test drives ONE decision record. It
//! reads no clock (every timed input carries `now_ms`), compares only the
//! digests and party keys its inputs carry, and holds no presentation,
//! evidence or notice content; the owner service keeps those beside the row.
//!
//! The centrepiece is
//! [`every_command_class_in_every_phase_is_total_and_only_mutates_where_intended`],
//! which drives every command class against every phase and asserts the one
//! intended transition, a byte-identical self-loop with its typed verdict, or
//! the generated kernel's typed refusal for inputs an unheld record never
//! receives: any input but `Hold`, and a `Hold` that names no decision or no
//! operation.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use meerkat_machine_schema::catalog::dsl::durable_decision::{
    DecisionDigest, DecisionParty, DurableDecisionEffect, DurableDecisionInput,
    DurableDecisionMachineAuthority, DurableDecisionMachineMutator, DurableDecisionMachineState,
    DurableDecisionMachineTransition, DurableDecisionMachineTransitionError, DurableDecisionNotice,
    DurableDecisionPhase, DurableDecisionRefusal, DurableDecisionReviewVerdict,
    DurableDecisionStage,
};

type Authority = DurableDecisionMachineAuthority;
type Input = DurableDecisionInput;
type Effect = DurableDecisionEffect;
type Phase = DurableDecisionPhase;
type State = DurableDecisionMachineState;
type Transition = DurableDecisionMachineTransition;
type TransitionError = DurableDecisionMachineTransitionError;

const DECISION: &str = "decision:0001";
const OPERATION: &str = "operation-0001";
const SUBJECT: &str = "sha256:subject";
const OTHER_SUBJECT: &str = "sha256:changed";
const REQUEST: &str = "sha256:request";
const OTHER_REQUEST: &str = "sha256:other-request";
const REQUESTER: &str = "principal:requester";
const APPROVER: &str = "approver:owner";
const OTHER_APPROVER: &str = "approver:other";
const REVIEWER: &str = "principal:reviewer";
const DECIDER: &str = "principal:human";
const EVIDENCE: &str = "sha256:evidence";
const HELD_AT: u64 = 1_000;
const DEADLINE: u64 = 61_000;

fn party(value: &str) -> DecisionParty {
    DecisionParty::from(value)
}

fn digest(value: &str) -> DecisionDigest {
    DecisionDigest::from(value)
}

fn apply(authority: &mut Authority, input: Input) -> Transition {
    DurableDecisionMachineMutator::apply(authority, input)
        .expect("the input must resolve to an explicit typed transition")
}

fn try_apply(authority: &mut Authority, input: Input) -> Result<Transition, TransitionError> {
    DurableDecisionMachineMutator::apply(authority, input)
}

/// The hold of the test decision, with `reviewer` and `notice_routable`.
fn hold_with(reviewer: Option<&str>, notice_routable: bool) -> Input {
    Input::Hold {
        decision_id: DECISION.to_owned(),
        now_ms: HELD_AT,
        deadline_at_ms: DEADLINE,
        subject_digest: digest(SUBJECT),
        operation_id: OPERATION.to_owned(),
        request_digest: digest(REQUEST),
        requester: party(REQUESTER),
        approver_id: party(APPROVER),
        reviewer: reviewer.map(party),
        notice_routable,
    }
}

/// The routable human-stage hold every phase builder starts from.
fn hold() -> Input {
    hold_with(None, true)
}

/// The same decision held with another subject: a conflicting hold.
fn conflicting_hold() -> Input {
    Input::Hold {
        decision_id: DECISION.to_owned(),
        now_ms: HELD_AT,
        deadline_at_ms: DEADLINE,
        subject_digest: digest(OTHER_SUBJECT),
        operation_id: OPERATION.to_owned(),
        request_digest: digest(REQUEST),
        requester: party(REQUESTER),
        approver_id: party(APPROVER),
        reviewer: None,
        notice_routable: true,
    }
}

/// The test hold naming no decision: the owner service never sends it.
fn unnamed_hold() -> Input {
    Input::Hold {
        decision_id: String::new(),
        now_ms: HELD_AT,
        deadline_at_ms: DEADLINE,
        subject_digest: digest(SUBJECT),
        operation_id: OPERATION.to_owned(),
        request_digest: digest(REQUEST),
        requester: party(REQUESTER),
        approver_id: party(APPROVER),
        reviewer: None,
        notice_routable: true,
    }
}

/// The test hold naming no operation: the owner service never sends it.
fn operationless_hold() -> Input {
    Input::Hold {
        decision_id: DECISION.to_owned(),
        now_ms: HELD_AT,
        deadline_at_ms: DEADLINE,
        subject_digest: digest(SUBJECT),
        operation_id: String::new(),
        request_digest: digest(REQUEST),
        requester: party(REQUESTER),
        approver_id: party(APPROVER),
        reviewer: None,
        notice_routable: true,
    }
}

/// One human decision. `valid(approve)` passes every check; each test row
/// changes exactly the field it exercises.
#[derive(Debug, Clone, Copy)]
struct Decision {
    approve: bool,
    approver_id: &'static str,
    decider: &'static str,
    digest: &'static str,
    evidence: Option<&'static str>,
    eligible: bool,
    carried_presentation_at_ms: Option<u64>,
    observed_at_ms: u64,
    now_ms: u64,
}

impl Decision {
    /// Carries a presentation shown at `HELD_AT + 5`, and was observed at
    /// `HELD_AT + 10`, when the owner decides it.
    fn valid(approve: bool) -> Self {
        Self {
            approve,
            approver_id: APPROVER,
            decider: DECIDER,
            digest: SUBJECT,
            evidence: Some(EVIDENCE),
            eligible: true,
            carried_presentation_at_ms: Some(HELD_AT + 5),
            observed_at_ms: HELD_AT + 10,
            now_ms: HELD_AT + 10,
        }
    }

    fn input(self) -> Input {
        Input::Decide {
            approve: self.approve,
            approver_id: party(self.approver_id),
            decider: party(self.decider),
            digest: digest(self.digest),
            evidence_digest: self.evidence.map(str::to_owned),
            eligible: self.eligible,
            carried_presentation_at_ms: self.carried_presentation_at_ms,
            observed_at_ms: self.observed_at_ms,
            now_ms: self.now_ms,
        }
    }
}

fn review(verdict: DurableDecisionReviewVerdict, reviewer: &str, now_ms: u64) -> Input {
    Input::Review {
        verdict,
        reviewer: party(reviewer),
        digest: digest(SUBJECT),
        evidence_digest: Some(EVIDENCE.to_owned()),
        now_ms,
    }
}

/// A presentation recorded when it was shown (`now_ms`).
fn present(audience: &str, subject: &str, complete: bool, now_ms: u64) -> Input {
    present_at(audience, subject, complete, now_ms, now_ms)
}

/// A presentation shown at `presented_at_ms` and recorded at `now_ms`.
fn present_at(
    audience: &str,
    subject: &str,
    complete: bool,
    presented_at_ms: u64,
    now_ms: u64,
) -> Input {
    Input::Present {
        audience: party(audience),
        digest: digest(subject),
        complete,
        presented_at_ms,
        now_ms,
    }
}

fn withdraw(withdrawer: &str, by_host: bool, now_ms: u64) -> Input {
    Input::Withdraw {
        withdrawer: party(withdrawer),
        by_host,
        now_ms,
    }
}

fn expire(now_ms: u64) -> Input {
    Input::Expire { now_ms }
}

fn authorize(request: &str, now_ms: u64) -> Input {
    Input::AuthorizeExecution {
        operation_id: OPERATION.to_owned(),
        request_digest: digest(request),
        now_ms,
    }
}

fn release(now_ms: u64) -> Input {
    Input::ReleaseExecution { now_ms }
}

fn record(request: &str, now_ms: u64) -> Input {
    Input::RecordExecuted {
        operation_id: OPERATION.to_owned(),
        request_digest: digest(request),
        now_ms,
    }
}

fn close_refused(now_ms: u64) -> Input {
    Input::CloseRefused { now_ms }
}

fn settle(delivered: bool) -> Input {
    Input::SettleNotice { delivered }
}

/// An authority parked in `phase` for the routable human-stage test hold.
fn authority_in(phase: Phase) -> Authority {
    let mut authority = Authority::new();
    match phase {
        Phase::Unheld => {}
        Phase::Open => {
            apply(&mut authority, hold());
        }
        Phase::Approved => {
            apply(&mut authority, hold());
            apply(&mut authority, Decision::valid(true).input());
        }
        Phase::Rejected => {
            apply(&mut authority, hold());
            apply(&mut authority, Decision::valid(false).input());
        }
        Phase::Expired => {
            apply(&mut authority, hold());
            apply(&mut authority, expire(DEADLINE));
        }
        Phase::Withdrawn => {
            apply(&mut authority, hold());
            apply(&mut authority, withdraw(REQUESTER, false, HELD_AT + 5));
        }
        Phase::Executed => {
            apply(&mut authority, hold());
            apply(&mut authority, Decision::valid(true).input());
            apply(&mut authority, authorize(REQUEST, HELD_AT + 20));
            apply(&mut authority, record(REQUEST, HELD_AT + 30));
        }
        Phase::ExecutionRefused => {
            apply(&mut authority, hold());
            apply(&mut authority, Decision::valid(true).input());
            apply(&mut authority, close_refused(HELD_AT + 20));
        }
    }
    assert_eq!(authority.state().lifecycle_phase, phase);
    authority
}

const ALL_PHASES: [Phase; 8] = [
    Phase::Unheld,
    Phase::Open,
    Phase::Approved,
    Phase::Rejected,
    Phase::Expired,
    Phase::Withdrawn,
    Phase::Executed,
    Phase::ExecutionRefused,
];

const ENDED_PHASES: [Phase; 5] = [
    Phase::Rejected,
    Phase::Expired,
    Phase::Withdrawn,
    Phase::Executed,
    Phase::ExecutionRefused,
];

/// A stable label for one effect, including a refusal's typed reason.
fn label(effect: &Effect) -> String {
    match effect {
        Effect::Held { .. } => "Held".to_owned(),
        Effect::HoldReplayed { .. } => "HoldReplayed".to_owned(),
        Effect::Presented { .. } => "Presented".to_owned(),
        Effect::Escalated { .. } => "Escalated".to_owned(),
        Effect::Approved { .. } => "Approved".to_owned(),
        Effect::Rejected { .. } => "Rejected".to_owned(),
        Effect::Expired { .. } => "Expired".to_owned(),
        Effect::Withdrawn { .. } => "Withdrawn".to_owned(),
        Effect::ExecutionAuthorized { .. } => "ExecutionAuthorized".to_owned(),
        Effect::ExecutionReleased { .. } => "ExecutionReleased".to_owned(),
        Effect::Executed { .. } => "Executed".to_owned(),
        Effect::ExecutionReplayed { .. } => "ExecutionReplayed".to_owned(),
        Effect::ExecutionRefused { .. } => "ExecutionRefused".to_owned(),
        Effect::NoticeSettled { .. } => "NoticeSettled".to_owned(),
        Effect::Refused { reason, .. } => format!("Refused({reason:?})"),
    }
}

fn labels(transition: &Transition) -> Vec<String> {
    transition.effects().iter().map(label).collect()
}

/// The single typed refusal of a transition.
fn refusal(transition: &Transition) -> DurableDecisionRefusal {
    match transition.effects() {
        [Effect::Refused { reason, .. }] => *reason,
        other => panic!("expected exactly one refusal, got {other:?}"),
    }
}

/// What one (phase, command) pair must do.
#[derive(Debug, Clone, Copy)]
enum Expected {
    /// The one intended transition into `to`, changing the state, with
    /// exactly these effects.
    Advance(Phase, &'static [&'static str]),
    /// A byte-identical self-loop with exactly these effects.
    SelfLoop(&'static [&'static str]),
    /// The generated kernel's typed refusal (`GuardRejected`): no arm of the
    /// input admits the pair (against an unheld record: any input but
    /// `Hold`, and a `Hold` that names no decision or no operation), and the
    /// state is untouched.
    Refused,
}

// ===========================================================================
// Totality
// ===========================================================================

#[test]
fn every_command_class_in_every_phase_is_total_and_only_mutates_where_intended() {
    let commands: [(&str, fn() -> Input); 17] = [
        ("Hold", hold),
        ("HoldOther", conflicting_hold),
        ("HoldUnnamed", unnamed_hold),
        ("HoldNoOperation", operationless_hold),
        ("Present", || present(APPROVER, SUBJECT, true, HELD_AT + 1)),
        ("Decide", || Decision::valid(true).input()),
        ("Review", || {
            review(DurableDecisionReviewVerdict::Allow, REVIEWER, HELD_AT + 1)
        }),
        ("Withdraw", || withdraw(REQUESTER, false, HELD_AT + 1)),
        ("ExpireEarly", || expire(DEADLINE - 1)),
        ("ExpireAtDeadline", || expire(DEADLINE)),
        ("Authorize", || authorize(REQUEST, HELD_AT + 40)),
        ("AuthorizeAtDeadline", || authorize(REQUEST, DEADLINE)),
        ("Release", || release(HELD_AT + 40)),
        ("Record", || record(REQUEST, HELD_AT + 40)),
        ("CloseRefused", || close_refused(HELD_AT + 40)),
        ("SettleDelivered", || settle(true)),
        ("SettleFailed", || settle(false)),
    ];

    let expected = |phase: Phase, command: &str| -> Expected {
        use Expected::{Advance, Refused, SelfLoop};
        let ended = ENDED_PHASES.contains(&phase);
        match (phase, command) {
            (Phase::Unheld, "Hold" | "HoldOther") => Advance(Phase::Open, &["Held"]),
            // A row exists only after a `Hold` that names its decision and
            // its operation, so the owner never sends these.
            (Phase::Unheld, _) => Refused,
            (_, "Hold") => SelfLoop(&["HoldReplayed"]),
            (_, "HoldOther" | "HoldUnnamed" | "HoldNoOperation") => {
                SelfLoop(&["Refused(HoldConflict)"])
            }
            (Phase::Open, "Present") => Advance(Phase::Open, &["Presented"]),
            (Phase::Open, "Decide") => Advance(Phase::Approved, &["Approved"]),
            (Phase::Open, "Review") => SelfLoop(&["Refused(HumanStage)"]),
            (Phase::Open, "Withdraw") => Advance(Phase::Withdrawn, &["Withdrawn"]),
            (Phase::Open | Phase::Approved, "ExpireEarly") => SelfLoop(&[]),
            (Phase::Open | Phase::Approved, "ExpireAtDeadline") => {
                Advance(Phase::Expired, &["Expired"])
            }
            (
                Phase::Open,
                "Authorize" | "AuthorizeAtDeadline" | "Release" | "Record" | "CloseRefused",
            ) => SelfLoop(&["Refused(NotApproved)"]),
            (Phase::Open | Phase::Approved, "SettleDelivered" | "SettleFailed") => {
                SelfLoop(&["Refused(NotOwed)"])
            }
            (_, "Present" | "Decide" | "Review" | "Withdraw") => SelfLoop(&["Refused(NotOpen)"]),
            (Phase::Approved, "Authorize") => Advance(Phase::Approved, &["ExecutionAuthorized"]),
            (Phase::Approved, "AuthorizeAtDeadline") => {
                Advance(Phase::Expired, &["Expired", "Refused(Expired)"])
            }
            (Phase::Approved, "Release" | "Record") => SelfLoop(&["Refused(NotAuthorized)"]),
            (Phase::Approved, "CloseRefused") => {
                Advance(Phase::ExecutionRefused, &["ExecutionRefused"])
            }
            (_, "ExpireEarly" | "ExpireAtDeadline") if ended => SelfLoop(&[]),
            (Phase::Executed, "Record") => SelfLoop(&["ExecutionReplayed"]),
            (_, "Authorize" | "AuthorizeAtDeadline" | "Release" | "Record" | "CloseRefused")
                if ended =>
            {
                SelfLoop(&["Refused(NotApproved)"])
            }
            (_, "SettleDelivered" | "SettleFailed") if ended => Advance(phase, &["NoticeSettled"]),
            (phase, command) => panic!("no expectation for ({phase:?}, {command})"),
        }
    };

    for phase in ALL_PHASES {
        for (command, build) in &commands {
            let mut authority = authority_in(phase);
            let before = authority.state().clone();
            let outcome = try_apply(&mut authority, build());
            match (expected(phase, command), outcome) {
                (Expected::Advance(to, effects), Ok(transition)) => {
                    assert_eq!(
                        authority.state().lifecycle_phase,
                        to,
                        "({phase:?}, {command}) must land in exactly {to:?}"
                    );
                    assert_ne!(
                        authority.state(),
                        &before,
                        "({phase:?}, {command}) is an intended advance"
                    );
                    assert_eq!(
                        labels(&transition),
                        effects,
                        "({phase:?}, {command}) effects"
                    );
                }
                (Expected::SelfLoop(effects), Ok(transition)) => {
                    assert_eq!(
                        authority.state(),
                        &before,
                        "({phase:?}, {command}) must leave the state byte-identical"
                    );
                    assert_eq!(
                        labels(&transition),
                        effects,
                        "({phase:?}, {command}) effects"
                    );
                }
                (Expected::Refused, Err(TransitionError::GuardRejected { phase: at, .. })) => {
                    assert_eq!(at, phase);
                    assert_eq!(
                        authority.state(),
                        &before,
                        "({phase:?}, {command}) a typed refusal never mutates state"
                    );
                }
                (expected, outcome) => {
                    panic!("({phase:?}, {command}) expected {expected:?}, got {outcome:?}")
                }
            }
        }
    }
}

// ===========================================================================
// Hold
// ===========================================================================

#[test]
fn hold_seals_the_binding_and_opens_by_stage() {
    let mut authority = Authority::new();
    let held = apply(&mut authority, hold_with(None, true));
    assert_eq!(
        held.effects(),
        &[Effect::Held {
            decision_id: DECISION.to_owned(),
            deadline_at_ms: DEADLINE,
        }]
    );
    let state = authority.state();
    assert_eq!(state.lifecycle_phase, Phase::Open);
    assert_eq!(state.decision_id, DECISION);
    assert_eq!(state.deadline_at_ms, DEADLINE);
    assert_eq!(state.subject_digest, digest(SUBJECT));
    assert_eq!(state.operation_id, OPERATION);
    assert_eq!(state.request_digest, digest(REQUEST));
    assert_eq!(state.requester, party(REQUESTER));
    assert_eq!(state.approver_id, party(APPROVER));
    assert_eq!(state.reviewer, None);
    assert_eq!(state.stage, DurableDecisionStage::Human);
    assert_eq!(state.notice, DurableDecisionNotice::NotDue);
    assert!(!state.permit_issued);

    // A reviewer opens the review stage; an unroutable requester owes no
    // notice.
    let mut authority = Authority::new();
    apply(&mut authority, hold_with(Some(REVIEWER), false));
    let state = authority.state();
    assert_eq!(state.lifecycle_phase, Phase::Open);
    assert_eq!(state.stage, DurableDecisionStage::Review);
    assert_eq!(state.reviewer, Some(party(REVIEWER)));
    assert_eq!(state.notice, DurableDecisionNotice::NotRequired);

    // A window that is past or empty is refused, and so is a requester that
    // reviews its own request; neither holds anything.
    for (label_text, input, reason) in [
        (
            "an empty window",
            Input::Hold {
                decision_id: DECISION.to_owned(),
                now_ms: DEADLINE,
                deadline_at_ms: DEADLINE,
                subject_digest: digest(SUBJECT),
                operation_id: OPERATION.to_owned(),
                request_digest: digest(REQUEST),
                requester: party(REQUESTER),
                approver_id: party(APPROVER),
                reviewer: None,
                notice_routable: true,
            },
            DurableDecisionRefusal::InvalidWindow,
        ),
        (
            "a self review",
            hold_with(Some(REQUESTER), true),
            DurableDecisionRefusal::SelfReview,
        ),
    ] {
        let mut authority = Authority::new();
        let refused = apply(&mut authority, input);
        assert_eq!(refusal(&refused), reason, "{label_text}");
        assert_eq!(authority.state(), &State::default(), "{label_text}");
    }

    // An unnamed decision is refused by the generated guard: the owner
    // service always derives the id, so no verdict effect exists for it.
    let mut authority = Authority::new();
    let unnamed = try_apply(
        &mut authority,
        Input::Hold {
            decision_id: String::new(),
            now_ms: HELD_AT,
            deadline_at_ms: DEADLINE,
            subject_digest: digest(SUBJECT),
            operation_id: OPERATION.to_owned(),
            request_digest: digest(REQUEST),
            requester: party(REQUESTER),
            approver_id: party(APPROVER),
            reviewer: None,
            notice_routable: true,
        },
    );
    assert!(
        matches!(
            unnamed,
            Err(TransitionError::GuardRejected {
                phase: Phase::Unheld,
                ..
            })
        ),
        "an unnamed decision is refused, got {unnamed:?}"
    );
}

#[test]
fn hold_replays_the_same_binding_and_refuses_another() {
    for phase in ALL_PHASES
        .into_iter()
        .filter(|phase| *phase != Phase::Unheld)
    {
        let mut authority = authority_in(phase);
        let before = authority.state().clone();

        // The same hold, read at another clock, replays.
        let mut replay = hold();
        if let Input::Hold { now_ms, .. } = &mut replay {
            *now_ms = DEADLINE + 1;
        }
        let replayed = apply(&mut authority, replay);
        assert_eq!(
            replayed.effects(),
            &[Effect::HoldReplayed {
                decision_id: DECISION.to_owned()
            }],
            "{phase:?} replays the same hold"
        );
        assert_eq!(authority.state(), &before);

        // Any sealed field that differs is a conflict.
        let variants: [(&str, fn(&mut Input)); 9] = [
            ("decision", |input| {
                if let Input::Hold { decision_id, .. } = input {
                    *decision_id = "decision:0002".to_owned();
                }
            }),
            ("deadline", |input| {
                if let Input::Hold { deadline_at_ms, .. } = input {
                    *deadline_at_ms = DEADLINE + 1;
                }
            }),
            ("subject", |input| {
                if let Input::Hold { subject_digest, .. } = input {
                    *subject_digest = digest(OTHER_SUBJECT);
                }
            }),
            ("operation", |input| {
                if let Input::Hold { operation_id, .. } = input {
                    *operation_id = "operation-0002".to_owned();
                }
            }),
            ("request", |input| {
                if let Input::Hold { request_digest, .. } = input {
                    *request_digest = digest(OTHER_REQUEST);
                }
            }),
            ("requester", |input| {
                if let Input::Hold { requester, .. } = input {
                    *requester = party(DECIDER);
                }
            }),
            ("approver", |input| {
                if let Input::Hold { approver_id, .. } = input {
                    *approver_id = party(OTHER_APPROVER);
                }
            }),
            ("reviewer", |input| {
                if let Input::Hold { reviewer, .. } = input {
                    *reviewer = Some(party(REVIEWER));
                }
            }),
            ("notice route", |input| {
                if let Input::Hold {
                    notice_routable, ..
                } = input
                {
                    *notice_routable = false;
                }
            }),
        ];
        for (field, change) in variants {
            let mut conflicting = hold();
            change(&mut conflicting);
            let refused = apply(&mut authority, conflicting);
            assert_eq!(
                refusal(&refused),
                DurableDecisionRefusal::HoldConflict,
                "{phase:?} refuses a hold with another {field}"
            );
            assert_eq!(authority.state(), &before, "{phase:?} {field}");
        }
    }
}

// ===========================================================================
// Decide
// ===========================================================================

#[test]
fn each_decide_conjunct_alone_refuses() {
    let rows: [(&str, Decision, DurableDecisionRefusal); 7] = [
        (
            "evidence",
            Decision {
                evidence: None,
                ..Decision::valid(true)
            },
            DurableDecisionRefusal::EvidenceRequired,
        ),
        (
            "digest",
            Decision {
                digest: OTHER_SUBJECT,
                ..Decision::valid(true)
            },
            DurableDecisionRefusal::SubjectChanged,
        ),
        (
            "approver",
            Decision {
                approver_id: OTHER_APPROVER,
                ..Decision::valid(true)
            },
            DurableDecisionRefusal::NotEligible,
        ),
        (
            "eligibility",
            Decision {
                eligible: false,
                ..Decision::valid(true)
            },
            DurableDecisionRefusal::NotEligible,
        ),
        (
            "self",
            Decision {
                decider: REQUESTER,
                ..Decision::valid(true)
            },
            DurableDecisionRefusal::SelfApproval,
        ),
        (
            "presentation",
            Decision {
                carried_presentation_at_ms: None,
                ..Decision::valid(true)
            },
            DurableDecisionRefusal::PresentationIncomplete,
        ),
        (
            "presentation order",
            Decision {
                observed_at_ms: HELD_AT + 4,
                ..Decision::valid(true)
            },
            DurableDecisionRefusal::DecidedBeforePresentation,
        ),
    ];
    for approve in [true, false] {
        for (conjunct, decision, reason) in rows {
            let mut authority = authority_in(Phase::Open);
            let before = authority.state().clone();
            let refused = apply(
                &mut authority,
                Decision {
                    approve,
                    ..decision
                }
                .input(),
            );
            assert_eq!(
                refusal(&refused),
                reason,
                "the {conjunct} conjunct alone refuses (approve {approve})"
            );
            assert_eq!(authority.state(), &before, "{conjunct}");
        }
    }

    // The stage conjunct: a human decision while the review stage is open.
    let mut authority = Authority::new();
    apply(&mut authority, hold_with(Some(REVIEWER), true));
    let before = authority.state().clone();
    let refused = apply(&mut authority, Decision::valid(true).input());
    assert_eq!(refusal(&refused), DurableDecisionRefusal::ReviewStage);
    assert_eq!(authority.state(), &before);

    // A presentation recorded earlier stands in for a carried one.
    let mut authority = authority_in(Phase::Open);
    apply(
        &mut authority,
        present(APPROVER, SUBJECT, true, HELD_AT + 1),
    );
    let approved = apply(
        &mut authority,
        Decision {
            carried_presentation_at_ms: None,
            ..Decision::valid(true)
        }
        .input(),
    );
    assert_eq!(labels(&approved), ["Approved"]);

    // An incomplete presentation, or one of another subject, records nothing.
    let mut authority = authority_in(Phase::Open);
    for (input, reason) in [
        (
            present(APPROVER, SUBJECT, false, HELD_AT + 1),
            DurableDecisionRefusal::PresentationIncomplete,
        ),
        (
            present(APPROVER, OTHER_SUBJECT, true, HELD_AT + 1),
            DurableDecisionRefusal::SubjectChanged,
        ),
        (
            present(REVIEWER, SUBJECT, true, HELD_AT + 1),
            DurableDecisionRefusal::WrongAudience,
        ),
    ] {
        let refused = apply(&mut authority, input);
        assert_eq!(refusal(&refused), reason);
        assert_eq!(authority.state().first_presented_to_approver_at_ms, None);
    }
}

#[test]
fn the_first_valid_decision_wins_and_end_phases_latch() {
    let mut authority = authority_in(Phase::Open);
    let approved = apply(&mut authority, Decision::valid(true).input());
    assert_eq!(
        approved.effects(),
        &[Effect::Approved {
            decision_id: DECISION.to_owned(),
            decided_at_ms: HELD_AT + 10,
        }]
    );
    let state = authority.state();
    assert_eq!(state.lifecycle_phase, Phase::Approved);
    assert_eq!(state.evidence_digest.as_deref(), Some(EVIDENCE));
    assert_eq!(state.decided_at_ms, Some(HELD_AT + 10));
    assert_eq!(
        state.first_presented_to_approver_at_ms,
        Some(HELD_AT + 5),
        "a carried presentation is recorded with the time it was shown"
    );
    assert_eq!(state.ended_at_ms, None);
    assert_eq!(state.notice, DurableDecisionNotice::NotDue);

    // A second valid decision, in either direction, is refused.
    let before = authority.state().clone();
    for approve in [true, false] {
        let refused = apply(&mut authority, Decision::valid(approve).input());
        assert_eq!(refusal(&refused), DurableDecisionRefusal::NotOpen);
        assert_eq!(authority.state(), &before);
    }

    // Every end phase latches: no input moves it again, a rolled-back clock
    // included; only the owed notice settles.
    for phase in ENDED_PHASES {
        let mut authority = authority_in(phase);
        let before = authority.state().clone();
        for input in [
            Decision::valid(true).input(),
            review(DurableDecisionReviewVerdict::Allow, REVIEWER, HELD_AT),
            withdraw(REQUESTER, true, HELD_AT),
            present(APPROVER, SUBJECT, true, HELD_AT),
            expire(0),
            expire(DEADLINE + 10),
            authorize(REQUEST, 0),
            release(0),
            close_refused(0),
        ] {
            apply(&mut authority, input);
            assert_eq!(authority.state(), &before, "{phase:?} stays latched");
        }
    }
}

/// A human decision is ordered after the approver's first presentation:
/// observed before it, the decision is refused `DecidedBeforePresentation`
/// and changes nothing; observed in the same millisecond or later, it is
/// decided. The first presentation is the earliest one, recorded or
/// carried, in whatever order the presentations arrive; with none at all
/// the decision stays `PresentationIncomplete`.
#[test]
fn a_human_decision_is_ordered_after_its_first_presentation() {
    const SHOWN_AT: u64 = HELD_AT + 20;
    let observed = |observed_at_ms: u64, carried: Option<u64>, approve: bool| Decision {
        carried_presentation_at_ms: carried,
        observed_at_ms,
        ..Decision::valid(approve)
    };

    // A recorded presentation: a millisecond before it is refused, an
    // approval and a rejection alike; the same millisecond decides.
    for (approve, decided) in [(true, Phase::Approved), (false, Phase::Rejected)] {
        let mut authority = authority_in(Phase::Open);
        apply(
            &mut authority,
            present_at(APPROVER, SUBJECT, true, SHOWN_AT, HELD_AT + 1),
        );
        assert_eq!(
            authority.state().first_presented_to_approver_at_ms,
            Some(SHOWN_AT)
        );
        let before = authority.state().clone();
        let refused = apply(
            &mut authority,
            observed(SHOWN_AT - 1, None, approve).input(),
        );
        assert_eq!(
            refusal(&refused),
            DurableDecisionRefusal::DecidedBeforePresentation,
            "approve {approve}"
        );
        assert_eq!(authority.state(), &before, "the refusal changes nothing");
        apply(&mut authority, observed(SHOWN_AT, None, approve).input());
        assert_eq!(
            authority.state().lifecycle_phase,
            decided,
            "the same millisecond decides"
        );
    }

    // A presentation the decision event carries orders it the same way, and
    // is recorded with its time.
    let mut authority = authority_in(Phase::Open);
    let before = authority.state().clone();
    let refused = apply(
        &mut authority,
        observed(SHOWN_AT - 1, Some(SHOWN_AT), true).input(),
    );
    assert_eq!(
        refusal(&refused),
        DurableDecisionRefusal::DecidedBeforePresentation
    );
    assert_eq!(authority.state(), &before, "the refusal changes nothing");
    let approved = apply(
        &mut authority,
        observed(SHOWN_AT, Some(SHOWN_AT), true).input(),
    );
    assert_eq!(labels(&approved), ["Approved"]);
    assert_eq!(
        authority.state().first_presented_to_approver_at_ms,
        Some(SHOWN_AT)
    );

    // The earliest presentation counts: a carried one shown before the
    // recorded one admits a decision observed between the two.
    let mut authority = authority_in(Phase::Open);
    apply(
        &mut authority,
        present_at(APPROVER, SUBJECT, true, SHOWN_AT, HELD_AT + 1),
    );
    let approved = apply(
        &mut authority,
        observed(SHOWN_AT - 3, Some(SHOWN_AT - 5), true).input(),
    );
    assert_eq!(labels(&approved), ["Approved"]);
    assert_eq!(
        authority.state().first_presented_to_approver_at_ms,
        Some(SHOWN_AT - 5)
    );

    // A later presentation never moves the first one; an earlier one,
    // recorded late, does.
    let mut authority = authority_in(Phase::Open);
    apply(
        &mut authority,
        present_at(APPROVER, SUBJECT, true, SHOWN_AT, HELD_AT + 1),
    );
    apply(
        &mut authority,
        present_at(APPROVER, SUBJECT, true, SHOWN_AT + 10, HELD_AT + 2),
    );
    assert_eq!(
        authority.state().first_presented_to_approver_at_ms,
        Some(SHOWN_AT)
    );
    apply(
        &mut authority,
        present_at(APPROVER, SUBJECT, true, SHOWN_AT - 10, HELD_AT + 3),
    );
    assert_eq!(
        authority.state().first_presented_to_approver_at_ms,
        Some(SHOWN_AT - 10)
    );
    let approved = apply(&mut authority, observed(SHOWN_AT - 10, None, true).input());
    assert_eq!(labels(&approved), ["Approved"]);

    // With no presentation at all, no observed time decides: the decision
    // stays `PresentationIncomplete`.
    for observed_at_ms in [0, SHOWN_AT, DEADLINE - 1] {
        let mut authority = authority_in(Phase::Open);
        let refused = apply(&mut authority, observed(observed_at_ms, None, true).input());
        assert_eq!(
            refusal(&refused),
            DurableDecisionRefusal::PresentationIncomplete,
            "observed at {observed_at_ms}"
        );
    }
}

/// A presentation time of 0 is no reading of when the approver was shown
/// the decision: a presentation recorded at 0 is refused
/// `PresentationIncomplete` and records nothing, and a decision carrying a
/// presentation at 0 is refused the same way, a recorded presentation
/// notwithstanding.
#[test]
fn a_presentation_time_of_zero_is_no_presentation() {
    let mut authority = authority_in(Phase::Open);
    let before = authority.state().clone();
    let refused = apply(
        &mut authority,
        present_at(APPROVER, SUBJECT, true, 0, HELD_AT + 1),
    );
    assert_eq!(
        refusal(&refused),
        DurableDecisionRefusal::PresentationIncomplete
    );
    assert_eq!(authority.state(), &before, "nothing is recorded");

    for recorded in [false, true] {
        let mut authority = authority_in(Phase::Open);
        if recorded {
            apply(
                &mut authority,
                present_at(APPROVER, SUBJECT, true, HELD_AT + 1, HELD_AT + 1),
            );
        }
        let before = authority.state().clone();
        for approve in [true, false] {
            let refused = apply(
                &mut authority,
                Decision {
                    carried_presentation_at_ms: Some(0),
                    ..Decision::valid(approve)
                }
                .input(),
            );
            assert_eq!(
                refusal(&refused),
                DurableDecisionRefusal::PresentationIncomplete,
                "recorded {recorded}, approve {approve}"
            );
            assert_eq!(authority.state(), &before, "the refusal changes nothing");
        }
    }
}

// ===========================================================================
// Deadline
// ===========================================================================

#[test]
fn a_touch_at_the_deadline_records_expiry() {
    let touches: [(&str, fn(u64) -> Input); 4] = [
        ("present", |now_ms| present(APPROVER, SUBJECT, true, now_ms)),
        ("decide", |now_ms| {
            Decision {
                now_ms,
                ..Decision::valid(true)
            }
            .input()
        }),
        ("review", |now_ms| {
            review(DurableDecisionReviewVerdict::Allow, REVIEWER, now_ms)
        }),
        ("withdraw", |now_ms| withdraw(REQUESTER, false, now_ms)),
    ];
    for (touch, build) in touches {
        for now_ms in [DEADLINE, DEADLINE + 5] {
            let mut authority = authority_in(Phase::Open);
            let expired = apply(&mut authority, build(now_ms));
            assert_eq!(
                expired.effects(),
                &[
                    Effect::Expired {
                        decision_id: DECISION.to_owned(),
                        deadline_at_ms: DEADLINE,
                        observed_at_ms: now_ms,
                    },
                    Effect::Refused {
                        decision_id: DECISION.to_owned(),
                        reason: DurableDecisionRefusal::Expired,
                    },
                ],
                "a {touch} at {now_ms} records the expiry"
            );
            let state = authority.state();
            assert_eq!(state.lifecycle_phase, Phase::Expired);
            assert_eq!(state.ended_at_ms, Some(now_ms));
            assert_eq!(
                state.evidence_digest, None,
                "an expired open decision is undecided"
            );
            assert_eq!(state.notice, DurableDecisionNotice::Owed);
        }
    }

    // One millisecond before the deadline the touch still acts.
    let mut authority = authority_in(Phase::Open);
    let approved = apply(
        &mut authority,
        Decision {
            now_ms: DEADLINE - 1,
            ..Decision::valid(true)
        }
        .input(),
    );
    assert_eq!(labels(&approved), ["Approved"]);

    // An early timer fire is byte-identical.
    let mut authority = authority_in(Phase::Open);
    let before = authority.state().clone();
    let early = apply(&mut authority, expire(DEADLINE - 1));
    assert!(early.effects().is_empty());
    assert_eq!(authority.state(), &before);
}

#[test]
fn an_approved_decision_expires_at_its_deadline_unless_a_permit_is_outstanding() {
    // No permit: the approval lapses unexecuted at its deadline, keeping its
    // attested decision, and owes its notice.
    let mut authority = authority_in(Phase::Approved);
    let lapsed = apply(&mut authority, expire(DEADLINE));
    assert_eq!(labels(&lapsed), ["Expired"]);
    let state = authority.state();
    assert_eq!(state.lifecycle_phase, Phase::Expired);
    assert_eq!(state.evidence_digest.as_deref(), Some(EVIDENCE));
    assert_eq!(state.decided_at_ms, Some(HELD_AT + 10));
    assert_eq!(state.notice, DurableDecisionNotice::Owed);

    // An authorize at the deadline with no permit records the expiry; no
    // permit is ever authorized at or after it.
    let mut authority = authority_in(Phase::Approved);
    let refused = apply(&mut authority, authorize(REQUEST, DEADLINE));
    assert_eq!(labels(&refused), ["Expired", "Refused(Expired)"]);
    assert_eq!(authority.state().lifecycle_phase, Phase::Expired);

    // A permanent refusal at the deadline with no permit records the expiry
    // too.
    let mut authority = authority_in(Phase::Approved);
    let refused = apply(&mut authority, close_refused(DEADLINE));
    assert_eq!(labels(&refused), ["Expired", "Refused(Expired)"]);
    assert_eq!(authority.state().lifecycle_phase, Phase::Expired);

    // An outstanding permit holds the expiry off: the owner may already have
    // committed the held request.
    let mut authority = authority_in(Phase::Approved);
    apply(&mut authority, authorize(REQUEST, DEADLINE - 1));
    assert!(authority.state().permit_issued);
    let before = authority.state().clone();
    let held_off = apply(&mut authority, expire(DEADLINE + 1));
    assert!(held_off.effects().is_empty());
    assert_eq!(
        authority.state(),
        &before,
        "an expiry never latches over a permit"
    );
    let refused = apply(&mut authority, authorize(REQUEST, DEADLINE + 1));
    assert_eq!(
        refusal(&refused),
        DurableDecisionRefusal::SettlementOwed,
        "nothing new runs after the deadline"
    );
    assert_eq!(authority.state(), &before);
}

#[test]
fn a_permit_outstanding_across_the_deadline_settles_executed_or_expired() {
    // The commit was found: the settlement records it after the deadline.
    let mut authority = authority_in(Phase::Approved);
    apply(&mut authority, authorize(REQUEST, DEADLINE - 1));
    let settled = apply(&mut authority, record(REQUEST, DEADLINE + 7));
    assert_eq!(
        settled.effects(),
        &[Effect::Executed {
            decision_id: DECISION.to_owned(),
            operation_id: OPERATION.to_owned(),
            request_digest: digest(REQUEST),
            executed_at_ms: DEADLINE + 7,
        }]
    );
    let state = authority.state();
    assert_eq!(state.lifecycle_phase, Phase::Executed);
    assert!(!state.permit_issued);
    assert_eq!(state.executed_request_digest, Some(digest(REQUEST)));

    // No commit was made: the release records the expiry the permit held off.
    let mut authority = authority_in(Phase::Approved);
    apply(&mut authority, authorize(REQUEST, DEADLINE - 1));
    let released = apply(&mut authority, release(DEADLINE + 7));
    assert_eq!(labels(&released), ["ExecutionReleased", "Expired"]);
    let state = authority.state();
    assert_eq!(state.lifecycle_phase, Phase::Expired);
    assert!(!state.permit_issued);
    assert_eq!(state.ended_at_ms, Some(DEADLINE + 7));

    // A permanent refusal after the deadline settles the permit as refused.
    let mut authority = authority_in(Phase::Approved);
    apply(&mut authority, authorize(REQUEST, DEADLINE - 1));
    let closed = apply(&mut authority, close_refused(DEADLINE + 7));
    assert_eq!(labels(&closed), ["ExecutionRefused"]);
    assert_eq!(authority.state().lifecycle_phase, Phase::ExecutionRefused);
    assert!(!authority.state().permit_issued);

    // Before the deadline a release keeps the approval owed: a retry may
    // authorize again.
    let mut authority = authority_in(Phase::Approved);
    apply(&mut authority, authorize(REQUEST, HELD_AT + 20));
    let released = apply(&mut authority, release(HELD_AT + 21));
    assert_eq!(labels(&released), ["ExecutionReleased"]);
    assert_eq!(authority.state().lifecycle_phase, Phase::Approved);
    assert!(!authority.state().permit_issued);
    let again = apply(&mut authority, authorize(REQUEST, HELD_AT + 22));
    assert_eq!(labels(&again), ["ExecutionAuthorized"]);
}

// ===========================================================================
// Review
// ===========================================================================

#[test]
fn review_allows_denies_escalates_and_refuses_another_reviewer() {
    let review_stage = || {
        let mut authority = Authority::new();
        apply(&mut authority, hold_with(Some(REVIEWER), true));
        apply(
            &mut authority,
            present(REVIEWER, SUBJECT, true, HELD_AT + 1),
        );
        assert!(authority.state().presented_to_reviewer);
        authority
    };

    let mut authority = review_stage();
    let allowed = apply(
        &mut authority,
        review(DurableDecisionReviewVerdict::Allow, REVIEWER, HELD_AT + 2),
    );
    assert_eq!(labels(&allowed), ["Approved"]);
    assert_eq!(authority.state().lifecycle_phase, Phase::Approved);
    assert_eq!(authority.state().stage, DurableDecisionStage::Review);

    let mut authority = review_stage();
    let denied = apply(
        &mut authority,
        review(DurableDecisionReviewVerdict::Deny, REVIEWER, HELD_AT + 2),
    );
    assert_eq!(labels(&denied), ["Rejected"]);
    assert_eq!(authority.state().lifecycle_phase, Phase::Rejected);
    assert_eq!(authority.state().notice, DurableDecisionNotice::Owed);

    // An escalation hands the open decision to the human approver, who
    // decides it like any human-stage decision.
    let mut authority = review_stage();
    let escalated = apply(
        &mut authority,
        review(
            DurableDecisionReviewVerdict::Escalate,
            REVIEWER,
            HELD_AT + 2,
        ),
    );
    assert_eq!(labels(&escalated), ["Escalated"]);
    let state = authority.state();
    assert_eq!(state.lifecycle_phase, Phase::Open);
    assert_eq!(state.stage, DurableDecisionStage::Human);
    assert_eq!(state.evidence_digest, None, "an escalation decides nothing");
    let approved = apply(&mut authority, Decision::valid(true).input());
    assert_eq!(labels(&approved), ["Approved"]);

    // Another reviewer, missing evidence, a changed subject and a missing
    // presentation are each refused.
    let mut authority = review_stage();
    let before = authority.state().clone();
    for (input, reason) in [
        (
            review(DurableDecisionReviewVerdict::Allow, DECIDER, HELD_AT + 2),
            DurableDecisionRefusal::WrongReviewer,
        ),
        (
            Input::Review {
                verdict: DurableDecisionReviewVerdict::Allow,
                reviewer: party(REVIEWER),
                digest: digest(SUBJECT),
                evidence_digest: None,
                now_ms: HELD_AT + 2,
            },
            DurableDecisionRefusal::EvidenceRequired,
        ),
        (
            Input::Review {
                verdict: DurableDecisionReviewVerdict::Allow,
                reviewer: party(REVIEWER),
                digest: digest(OTHER_SUBJECT),
                evidence_digest: Some(EVIDENCE.to_owned()),
                now_ms: HELD_AT + 2,
            },
            DurableDecisionRefusal::SubjectChanged,
        ),
    ] {
        let refused = apply(&mut authority, input);
        assert_eq!(refusal(&refused), reason);
        assert_eq!(authority.state(), &before);
    }
    let mut authority = Authority::new();
    apply(&mut authority, hold_with(Some(REVIEWER), true));
    let refused = apply(
        &mut authority,
        review(DurableDecisionReviewVerdict::Allow, REVIEWER, HELD_AT + 2),
    );
    assert_eq!(
        refusal(&refused),
        DurableDecisionRefusal::PresentationIncomplete
    );

    // A review verdict in the human stage is refused.
    let mut authority = authority_in(Phase::Open);
    let refused = apply(
        &mut authority,
        review(DurableDecisionReviewVerdict::Allow, REVIEWER, HELD_AT + 2),
    );
    assert_eq!(refusal(&refused), DurableDecisionRefusal::HumanStage);
}

// ===========================================================================
// Withdraw
// ===========================================================================

#[test]
fn withdraw_is_open_only_by_requester_or_host() {
    for (withdrawer, by_host) in [(REQUESTER, false), (DECIDER, true)] {
        let mut authority = authority_in(Phase::Open);
        let withdrawn = apply(&mut authority, withdraw(withdrawer, by_host, HELD_AT + 3));
        assert_eq!(
            withdrawn.effects(),
            &[Effect::Withdrawn {
                decision_id: DECISION.to_owned(),
                observed_at_ms: HELD_AT + 3,
            }]
        );
        let state = authority.state();
        assert_eq!(state.lifecycle_phase, Phase::Withdrawn);
        assert_eq!(state.ended_at_ms, Some(HELD_AT + 3));
        assert_eq!(state.notice, DurableDecisionNotice::Owed);
    }

    let mut authority = authority_in(Phase::Open);
    let before = authority.state().clone();
    let refused = apply(&mut authority, withdraw(DECIDER, false, HELD_AT + 3));
    assert_eq!(refusal(&refused), DurableDecisionRefusal::NotRequester);
    assert_eq!(authority.state(), &before);

    // A decided decision cannot be withdrawn, even by the host.
    let mut authority = authority_in(Phase::Approved);
    let before = authority.state().clone();
    let refused = apply(&mut authority, withdraw(REQUESTER, true, HELD_AT + 3));
    assert_eq!(refusal(&refused), DurableDecisionRefusal::NotOpen);
    assert_eq!(authority.state(), &before);
}

// ===========================================================================
// Execution
// ===========================================================================

#[test]
fn execution_is_authorized_only_while_approved_with_the_held_binding() {
    let mut authority = authority_in(Phase::Approved);
    let authorized = apply(&mut authority, authorize(REQUEST, HELD_AT + 20));
    assert_eq!(
        authorized.effects(),
        &[Effect::ExecutionAuthorized {
            decision_id: DECISION.to_owned(),
            operation_id: OPERATION.to_owned(),
            request_digest: digest(REQUEST),
            deadline_at_ms: DEADLINE,
        }]
    );
    assert!(authority.state().permit_issued);
    // A retry authorizes again under the same outstanding permit.
    let before = authority.state().clone();
    let again = apply(&mut authority, authorize(REQUEST, HELD_AT + 21));
    assert_eq!(labels(&again), ["ExecutionAuthorized"]);
    assert_eq!(authority.state(), &before);

    // Another request or another operation is refused.
    let mut authority = authority_in(Phase::Approved);
    let before = authority.state().clone();
    for input in [
        authorize(OTHER_REQUEST, HELD_AT + 20),
        Input::AuthorizeExecution {
            operation_id: "operation-0002".to_owned(),
            request_digest: digest(REQUEST),
            now_ms: HELD_AT + 20,
        },
    ] {
        let refused = apply(&mut authority, input);
        assert_eq!(refusal(&refused), DurableDecisionRefusal::BindingMismatch);
        assert_eq!(authority.state(), &before);
    }

    // Nothing but an approved decision is authorized.
    for phase in [Phase::Open, Phase::Rejected, Phase::Executed] {
        let mut authority = authority_in(phase);
        let refused = apply(&mut authority, authorize(REQUEST, HELD_AT + 20));
        assert_eq!(refusal(&refused), DurableDecisionRefusal::NotApproved);
    }
}

#[test]
fn executed_replays_and_a_mismatched_binding_is_refused() {
    let mut authority = authority_in(Phase::Approved);
    // A record without an outstanding permit is refused: no commit can exist.
    let refused = apply(&mut authority, record(REQUEST, HELD_AT + 30));
    assert_eq!(refusal(&refused), DurableDecisionRefusal::NotAuthorized);

    apply(&mut authority, authorize(REQUEST, HELD_AT + 20));
    let refused = apply(&mut authority, record(OTHER_REQUEST, HELD_AT + 30));
    assert_eq!(refusal(&refused), DurableDecisionRefusal::BindingMismatch);
    assert_eq!(authority.state().lifecycle_phase, Phase::Approved);

    let executed = apply(&mut authority, record(REQUEST, HELD_AT + 30));
    assert_eq!(labels(&executed), ["Executed"]);
    let settled = authority.state().clone();
    assert_eq!(settled.executed_request_digest, Some(digest(REQUEST)));
    assert_eq!(settled.notice, DurableDecisionNotice::Owed);

    // The same settlement again changes nothing; another binding is refused.
    let replayed = apply(&mut authority, record(REQUEST, HELD_AT + 99));
    assert_eq!(
        replayed.effects(),
        &[Effect::ExecutionReplayed {
            decision_id: DECISION.to_owned()
        }]
    );
    assert_eq!(authority.state(), &settled);
    let refused = apply(&mut authority, record(OTHER_REQUEST, HELD_AT + 99));
    assert_eq!(refusal(&refused), DurableDecisionRefusal::BindingMismatch);
    assert_eq!(authority.state(), &settled);
}

#[test]
fn close_refused_never_follows_executed() {
    let mut authority = authority_in(Phase::Executed);
    let before = authority.state().clone();
    let refused = apply(&mut authority, close_refused(HELD_AT + 50));
    assert_eq!(refusal(&refused), DurableDecisionRefusal::NotApproved);
    assert_eq!(authority.state(), &before);

    // From an approved decision a permanent refusal closes it and owes the
    // notice; the outstanding permit is settled with it.
    let mut authority = authority_in(Phase::Approved);
    apply(&mut authority, authorize(REQUEST, HELD_AT + 20));
    let closed = apply(&mut authority, close_refused(HELD_AT + 30));
    assert_eq!(
        closed.effects(),
        &[Effect::ExecutionRefused {
            decision_id: DECISION.to_owned(),
            closed_at_ms: HELD_AT + 30,
        }]
    );
    let state = authority.state();
    assert_eq!(state.lifecycle_phase, Phase::ExecutionRefused);
    assert!(!state.permit_issued);
    assert_eq!(state.notice, DurableDecisionNotice::Owed);
    let refused = apply(&mut authority, record(REQUEST, HELD_AT + 31));
    assert_eq!(refusal(&refused), DurableDecisionRefusal::NotApproved);
}

// ===========================================================================
// Notice
// ===========================================================================

#[test]
fn notice_is_owed_once_at_the_end_for_routable_decisions() {
    for phase in ENDED_PHASES {
        let mut authority = authority_in(phase);
        assert_eq!(
            authority.state().notice,
            DurableDecisionNotice::Owed,
            "{phase:?} owes its notice"
        );
        let delivered = apply(&mut authority, settle(true));
        assert_eq!(
            delivered.effects(),
            &[Effect::NoticeSettled {
                decision_id: DECISION.to_owned(),
                delivered: true,
            }]
        );
        assert_eq!(authority.state().notice, DurableDecisionNotice::Delivered);
        let settled = authority.state().clone();
        // The same receipt again is a replay; a contradicting one is refused.
        let replayed = apply(&mut authority, settle(true));
        assert_eq!(labels(&replayed), ["NoticeSettled"]);
        let refused = apply(&mut authority, settle(false));
        assert_eq!(refusal(&refused), DurableDecisionRefusal::NotOwed);
        assert_eq!(authority.state(), &settled);
    }

    // A failed delivery is terminal too.
    let mut authority = authority_in(Phase::Withdrawn);
    apply(&mut authority, settle(false));
    assert_eq!(authority.state().notice, DurableDecisionNotice::Failed);

    // An unroutable requester owes nothing, at any end.
    let mut authority = Authority::new();
    apply(&mut authority, hold_with(None, false));
    apply(&mut authority, Decision::valid(false).input());
    assert_eq!(authority.state().lifecycle_phase, Phase::Rejected);
    assert_eq!(authority.state().notice, DurableDecisionNotice::NotRequired);
    let refused = apply(&mut authority, settle(true));
    assert_eq!(refusal(&refused), DurableDecisionRefusal::NotOwed);

    // Nothing is owed before the end.
    for phase in [Phase::Open, Phase::Approved] {
        let mut authority = authority_in(phase);
        let refused = apply(&mut authority, settle(true));
        assert_eq!(refusal(&refused), DurableDecisionRefusal::NotOwed);
    }
}

// ===========================================================================
// TLC model
// ===========================================================================

/// The machine's CI run is held to every arm: its explicit TLC model gives
/// both key types two values and requires each arm to generate a state
/// within a step bound that reaches the deepest one.
#[test]
fn the_explicit_tlc_model_requires_every_arm_in_ci() {
    let schema = meerkat_machine_schema::catalog::dsl::dsl_durable_decision_machine();
    let model = schema.tlc_model.as_ref().expect("an explicit TLC model");
    assert!(model.require_ci_transition_coverage);
    assert!(
        model.ci_limits.step_limit >= 6,
        "hold, approve, authorize, record, settle and settle again fit the CI step bound"
    );
    schema.validate().expect("the explicit model validates");
    for domain in ["DecisionParty", "DecisionDigest"] {
        let values = model
            .ci
            .named_values
            .iter()
            .find(|(name, _)| name.as_str() == domain)
            .map(|(_, values)| values.len());
        assert_eq!(values, Some(2), "{domain} has two CI values");
    }
}

// ===========================================================================
// Recovery
// ===========================================================================

#[test]
fn recover_from_state_refuses_each_torn_shape() {
    // Every reachable phase recovers unchanged.
    for phase in ALL_PHASES {
        let state = authority_in(phase).state().clone();
        let recovered = Authority::recover_from_state(state.clone())
            .unwrap_or_else(|error| panic!("{phase:?} must recover: {error}"));
        assert_eq!(recovered.state(), &state);
    }

    let open = authority_in(Phase::Open).state().clone();
    let approved = authority_in(Phase::Approved).state().clone();
    let executed = authority_in(Phase::Executed).state().clone();
    let expired = authority_in(Phase::Expired).state().clone();
    let withdrawn = authority_in(Phase::Withdrawn).state().clone();
    let mut evidence_without_time = expired.clone();
    evidence_without_time.evidence_digest = Some(EVIDENCE.to_owned());
    let torn_rows: Vec<(&str, State)> = vec![
        (
            "unheld_is_empty",
            State {
                decision_id: DECISION.to_owned(),
                ..State::default()
            },
        ),
        (
            "held_records_are_named",
            State {
                operation_id: String::new(),
                ..open.clone()
            },
        ),
        (
            "review_stage_names_another_reviewer",
            State {
                stage: DurableDecisionStage::Review,
                reviewer: Some(party(REQUESTER)),
                ..open.clone()
            },
        ),
        (
            "reviewer_presentation_needs_a_reviewer",
            State {
                presented_to_reviewer: true,
                ..open.clone()
            },
        ),
        (
            "first_presentation_is_a_reading",
            State {
                first_presented_to_approver_at_ms: Some(0),
                ..open.clone()
            },
        ),
        (
            "open_has_no_outcome",
            State {
                ended_at_ms: Some(HELD_AT),
                ..open
            },
        ),
        (
            "approved_has_not_ended",
            State {
                ended_at_ms: Some(HELD_AT + 11),
                ..approved.clone()
            },
        ),
        ("evidence_and_decision_time_agree", evidence_without_time),
        (
            "decided_decisions_are_attested",
            State {
                decided_at_ms: Some(DEADLINE),
                ..approved
            },
        ),
        (
            "permit_is_outstanding_only_while_approved",
            State {
                permit_issued: true,
                ..executed.clone()
            },
        ),
        (
            "executed_names_its_held_request",
            State {
                executed_request_digest: Some(digest(OTHER_REQUEST)),
                ..executed.clone()
            },
        ),
        (
            "only_executed_records_an_execution",
            State {
                lifecycle_phase: Phase::ExecutionRefused,
                ..executed
            },
        ),
        (
            "ended_decisions_record_their_end",
            State {
                ended_at_ms: None,
                ..withdrawn.clone()
            },
        ),
        (
            "expiry_never_precedes_its_deadline",
            State {
                ended_at_ms: Some(DEADLINE - 1),
                ..expired
            },
        ),
        (
            "withdrawal_precedes_the_deadline",
            State {
                ended_at_ms: Some(DEADLINE),
                ..withdrawn.clone()
            },
        ),
        (
            "ended_decisions_owe_their_notice",
            State {
                notice: DurableDecisionNotice::NotDue,
                ..withdrawn
            },
        ),
    ];
    for (invariant, torn) in torn_rows {
        match Authority::recover_from_state(torn.clone()) {
            Err(TransitionError::RecoveredStateInvariantRejected {
                invariant: rejected,
                phase,
            }) => {
                assert_eq!(rejected, invariant, "a torn row reports its own invariant");
                assert_eq!(phase, torn.lifecycle_phase);
            }
            Err(other) => panic!("{invariant}: unexpected refusal {other}"),
            Ok(_) => panic!("{invariant}: a torn row must not recover"),
        }
    }
}

//! Focused behavioural tests for the canonical `ToolGrantLifecycleMachine`.
//!
//! The machine is record-scoped: every test drives ONE grant record. It reads
//! no clock (every input carries `now_ms`) and holds no tool-visibility
//! facts; those stay with `MeerkatMachine`.
//!
//! The centrepiece is
//! [`every_command_class_in_every_phase_is_total_and_only_mutates_where_intended`],
//! which drives every command class against every phase and asserts the one
//! intended transition, a byte-identical self-loop, or the generated kernel's
//! typed refusal for inputs an unissued record never receives.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use meerkat_machine_schema::catalog::dsl::tool_grant_lifecycle::{
    ToolGrantEndReason, ToolGrantIssueRefusal, ToolGrantLifecycleEffect, ToolGrantLifecycleInput,
    ToolGrantLifecycleMachineAuthority, ToolGrantLifecycleMachineMutator,
    ToolGrantLifecycleMachineState, ToolGrantLifecycleMachineTransition,
    ToolGrantLifecycleMachineTransitionError, ToolGrantPhase, ToolGrantUseRefusal,
};

type Authority = ToolGrantLifecycleMachineAuthority;
type Input = ToolGrantLifecycleInput;
type Effect = ToolGrantLifecycleEffect;
type Phase = ToolGrantPhase;
type State = ToolGrantLifecycleMachineState;
type Transition = ToolGrantLifecycleMachineTransition;
type TransitionError = ToolGrantLifecycleMachineTransitionError;

const GRANT: &str = "grant-a";
const ISSUED_AT: u64 = 1_000;
const DEADLINE: u64 = 61_000;
/// The default host maximum lifetime: 24 hours.
const MAX_LIFETIME: u64 = 24 * 60 * 60 * 1_000;

fn apply(authority: &mut Authority, input: Input) -> Transition {
    ToolGrantLifecycleMachineMutator::apply(authority, input)
        .expect("the input must resolve to an explicit typed transition")
}

fn try_apply(authority: &mut Authority, input: Input) -> Result<Transition, TransitionError> {
    ToolGrantLifecycleMachineMutator::apply(authority, input)
}

fn issue(grant_id: &str, now_ms: u64, expires_at_ms: u64, max_lifetime_ms: u64) -> Input {
    Input::Issue {
        grant_id: grant_id.to_owned(),
        now_ms,
        expires_at_ms,
        max_lifetime_ms,
    }
}

fn valid_issue() -> Input {
    issue(GRANT, ISSUED_AT, DEADLINE, MAX_LIFETIME)
}

fn expire(now_ms: u64) -> Input {
    Input::Expire { now_ms }
}

fn revoke(now_ms: u64) -> Input {
    Input::Revoke { now_ms }
}

fn admit(now_ms: u64) -> Input {
    Input::AdmitUse { now_ms }
}

/// Build an authority parked in `phase` for the grant `GRANT`, issued at
/// `ISSUED_AT` with the deadline `DEADLINE`.
fn authority_in(phase: Phase) -> Authority {
    let mut authority = Authority::new();
    match phase {
        Phase::Unissued => {}
        Phase::Active => {
            apply(&mut authority, valid_issue());
        }
        Phase::Expired => {
            apply(&mut authority, valid_issue());
            apply(&mut authority, expire(DEADLINE));
        }
        Phase::Revoked => {
            apply(&mut authority, valid_issue());
            apply(&mut authority, revoke(DEADLINE - 1));
        }
    }
    assert_eq!(authority.state().lifecycle_phase, phase);
    authority
}

const ALL_PHASES: [Phase; 4] = [
    Phase::Unissued,
    Phase::Active,
    Phase::Expired,
    Phase::Revoked,
];

/// A stable label for one effect, including its typed reason.
fn effect_label(effect: &Effect) -> &'static str {
    match effect {
        Effect::Issued { .. } => "Issued",
        Effect::Expired { .. } => "Expired",
        Effect::Revoked { .. } => "Revoked",
        Effect::UseAdmitted { .. } => "UseAdmitted",
        Effect::UseRefused {
            reason: ToolGrantUseRefusal::Expired,
            ..
        } => "UseRefused(Expired)",
        Effect::UseRefused {
            reason: ToolGrantUseRefusal::Revoked,
            ..
        } => "UseRefused(Revoked)",
        Effect::IssueRefused {
            reason: ToolGrantIssueRefusal::InvalidWindow,
        } => "IssueRefused(InvalidWindow)",
        Effect::IssueRefused {
            reason: ToolGrantIssueRefusal::AlreadyIssued,
        } => "IssueRefused(AlreadyIssued)",
    }
}

fn labels(transition: &Transition) -> Vec<&'static str> {
    transition.effects().iter().map(effect_label).collect()
}

/// What one (phase, command) pair must do.
#[derive(Debug, Clone, Copy)]
enum Expected {
    /// The one intended transition to `to`, with exactly these effects.
    Advance(Phase, &'static [&'static str]),
    /// A self-loop that leaves the state byte-identical and emits exactly
    /// these effects.
    SelfLoop(&'static [&'static str]),
    /// The generated kernel's typed refusal: no transition is declared for
    /// the pair, and the state is untouched.
    Refused,
}

// ===========================================================================
// Totality
// ===========================================================================

#[test]
fn every_command_class_in_every_phase_is_total_and_only_mutates_where_intended() {
    let commands: [(&str, fn() -> Input); 9] = [
        ("Issue", valid_issue),
        ("IssuePastWindow", || {
            issue(GRANT, DEADLINE, ISSUED_AT, MAX_LIFETIME)
        }),
        ("IssueOverlongWindow", || {
            issue(GRANT, ISSUED_AT, ISSUED_AT + MAX_LIFETIME + 1, MAX_LIFETIME)
        }),
        ("ExpireEarly", || expire(DEADLINE - 1)),
        ("ExpireAtDeadline", || expire(DEADLINE)),
        ("RevokeEarly", || revoke(DEADLINE - 1)),
        ("RevokeAtDeadline", || revoke(DEADLINE)),
        ("AdmitEarly", || admit(DEADLINE - 1)),
        ("AdmitAtDeadline", || admit(DEADLINE)),
    ];

    let expected = |phase: Phase, command: &str| -> Expected {
        use Expected::{Advance, Refused, SelfLoop};
        match (phase, command) {
            (Phase::Unissued, "Issue") => Advance(Phase::Active, &["Issued"]),
            (Phase::Unissued, "IssuePastWindow" | "IssueOverlongWindow") => {
                SelfLoop(&["IssueRefused(InvalidWindow)"])
            }
            // A row exists only after `Issue`, so the owner never sends these.
            (Phase::Unissued, _) => Refused,
            (_, "Issue" | "IssuePastWindow" | "IssueOverlongWindow") => {
                SelfLoop(&["IssueRefused(AlreadyIssued)"])
            }
            (Phase::Active, "ExpireEarly") => SelfLoop(&[]),
            (Phase::Active, "ExpireAtDeadline") => Advance(Phase::Expired, &["Expired"]),
            (Phase::Active, "RevokeEarly") => Advance(Phase::Revoked, &["Revoked"]),
            (Phase::Active, "RevokeAtDeadline") => Advance(Phase::Expired, &["Expired"]),
            (Phase::Active, "AdmitEarly") => SelfLoop(&["UseAdmitted"]),
            (Phase::Active, "AdmitAtDeadline") => {
                Advance(Phase::Expired, &["Expired", "UseRefused(Expired)"])
            }
            (
                Phase::Expired | Phase::Revoked,
                "ExpireEarly" | "ExpireAtDeadline" | "RevokeEarly" | "RevokeAtDeadline",
            ) => SelfLoop(&[]),
            (Phase::Expired, "AdmitEarly" | "AdmitAtDeadline") => {
                SelfLoop(&["UseRefused(Expired)"])
            }
            (Phase::Revoked, "AdmitEarly" | "AdmitAtDeadline") => {
                SelfLoop(&["UseRefused(Revoked)"])
            }
            (phase, command) => panic!("no expectation for ({phase:?}, {command})"),
        }
    };

    for phase in ALL_PHASES {
        for (label, build) in &commands {
            let mut authority = authority_in(phase);
            let before = authority.state().clone();
            let outcome = try_apply(&mut authority, build());
            match (expected(phase, label), outcome) {
                (Expected::Advance(to, effects), Ok(transition)) => {
                    assert_eq!(
                        authority.state().lifecycle_phase,
                        to,
                        "({phase:?}, {label}) must land in exactly {to:?}"
                    );
                    assert_ne!(
                        authority.state(),
                        &before,
                        "({phase:?}, {label}) is an intended advance"
                    );
                    assert_eq!(labels(&transition), effects, "({phase:?}, {label}) effects");
                }
                (Expected::SelfLoop(effects), Ok(transition)) => {
                    assert_eq!(
                        authority.state(),
                        &before,
                        "({phase:?}, {label}) must leave the state byte-identical"
                    );
                    assert_eq!(labels(&transition), effects, "({phase:?}, {label}) effects");
                }
                (
                    Expected::Refused,
                    Err(
                        TransitionError::NoMatchingTransition { phase: at, .. }
                        | TransitionError::GuardRejected { phase: at, .. },
                    ),
                ) => {
                    assert_eq!(at, phase);
                    assert_eq!(
                        authority.state(),
                        &before,
                        "({phase:?}, {label}) a typed refusal never mutates state"
                    );
                }
                (expected, outcome) => {
                    panic!("({phase:?}, {label}) expected {expected:?}, got {outcome:?}")
                }
            }
        }
    }
}

// ===========================================================================
// Issue
// ===========================================================================

#[test]
fn issue_refuses_a_past_empty_or_overlong_window() {
    let refused_windows = [
        ("past", issue(GRANT, 2_000, 1_000, MAX_LIFETIME)),
        ("empty", issue(GRANT, 1_000, 1_000, MAX_LIFETIME)),
        (
            "overlong",
            issue(GRANT, 1_000, 1_000 + MAX_LIFETIME + 1, MAX_LIFETIME),
        ),
    ];
    for (label, input) in refused_windows {
        let mut authority = Authority::new();
        let refused = apply(&mut authority, input);
        assert_eq!(
            refused.effects(),
            &[Effect::IssueRefused {
                reason: ToolGrantIssueRefusal::InvalidWindow
            }],
            "a {label} window is refused as an invalid window"
        );
        assert_eq!(
            authority.state(),
            &State::default(),
            "a refused {label} window leaves the record unissued"
        );
    }

    // The window bounds are inclusive of the maximum and exclusive of `now`.
    for (label, expires_at_ms) in [
        ("one millisecond", 1_001),
        ("exactly the maximum", 1_000 + MAX_LIFETIME),
    ] {
        let mut authority = Authority::new();
        let issued = apply(
            &mut authority,
            issue(GRANT, 1_000, expires_at_ms, MAX_LIFETIME),
        );
        assert_eq!(
            issued.effects(),
            &[Effect::Issued {
                grant_id: GRANT.to_owned(),
                expires_at_ms
            }],
            "a window of {label} is issued"
        );
        let state = authority.state();
        assert_eq!(state.lifecycle_phase, Phase::Active);
        assert_eq!(state.grant_id, GRANT);
        assert_eq!(state.issued_at_ms, 1_000);
        assert_eq!(state.expires_at_ms, expires_at_ms);
        assert_eq!(state.ended_at_ms, None);
        assert_eq!(state.end_reason, None);
    }

    // An unnamed grant is refused by the generated guard: the owner service
    // always mints the id, so no verdict effect exists for it.
    let mut authority = Authority::new();
    let unnamed = try_apply(&mut authority, issue("", ISSUED_AT, DEADLINE, MAX_LIFETIME));
    assert!(
        matches!(
            unnamed,
            Err(TransitionError::GuardRejected {
                phase: Phase::Unissued,
                ..
            })
        ),
        "an unnamed grant is refused, got {unnamed:?}"
    );
    assert_eq!(authority.state(), &State::default());

    // An issued record never issues again, in any issued phase.
    for phase in [Phase::Active, Phase::Expired, Phase::Revoked] {
        let mut authority = authority_in(phase);
        let before = authority.state().clone();
        let again = apply(
            &mut authority,
            issue("grant-b", ISSUED_AT, DEADLINE, MAX_LIFETIME),
        );
        assert_eq!(
            again.effects(),
            &[Effect::IssueRefused {
                reason: ToolGrantIssueRefusal::AlreadyIssued
            }],
            "{phase:?} refuses a second issue"
        );
        assert_eq!(authority.state(), &before);
    }
}

// ===========================================================================
// Use admission
// ===========================================================================

#[test]
fn use_one_millisecond_before_the_deadline_is_admitted_and_at_the_deadline_expires_and_is_refused()
{
    let mut authority = authority_in(Phase::Active);
    let before = authority.state().clone();
    let admitted = apply(&mut authority, admit(DEADLINE - 1));
    assert_eq!(
        admitted.effects(),
        &[Effect::UseAdmitted {
            grant_id: GRANT.to_owned()
        }]
    );
    assert_eq!(
        authority.state(),
        &before,
        "an admitted use never changes the grant"
    );

    let refused = apply(&mut authority, admit(DEADLINE));
    assert_eq!(
        refused.effects(),
        &[
            Effect::Expired {
                grant_id: GRANT.to_owned(),
                expires_at_ms: DEADLINE,
                observed_at_ms: DEADLINE,
            },
            Effect::UseRefused {
                grant_id: GRANT.to_owned(),
                reason: ToolGrantUseRefusal::Expired,
            },
        ],
        "the first use at the deadline observes the expiry, then is refused"
    );
    let state = authority.state();
    assert_eq!(state.lifecycle_phase, Phase::Expired);
    assert_eq!(state.ended_at_ms, Some(DEADLINE));
    assert_eq!(state.end_reason, Some(ToolGrantEndReason::Deadline));

    let expired = authority.state().clone();
    let later = apply(&mut authority, admit(DEADLINE + 5));
    assert_eq!(
        later.effects(),
        &[Effect::UseRefused {
            grant_id: GRANT.to_owned(),
            reason: ToolGrantUseRefusal::Expired,
        }]
    );
    assert_eq!(
        authority.state(),
        &expired,
        "a refused use of an expired grant changes nothing"
    );

    // A use long after the deadline records the time it observed.
    let mut authority = authority_in(Phase::Active);
    apply(&mut authority, admit(DEADLINE + 10_000));
    assert_eq!(authority.state().ended_at_ms, Some(DEADLINE + 10_000));
    assert_eq!(authority.state().lifecycle_phase, Phase::Expired);
}

// ===========================================================================
// Expiry
// ===========================================================================

#[test]
fn expire_before_the_deadline_is_a_byte_identical_noop() {
    let mut authority = authority_in(Phase::Active);
    let before = authority.state().clone();
    for now_ms in [ISSUED_AT, ISSUED_AT + 1, DEADLINE - 1] {
        let early = apply(&mut authority, expire(now_ms));
        assert!(
            early.effects().is_empty(),
            "an early timer fire at {now_ms} emits nothing"
        );
        assert_eq!(early.from_phase, Phase::Active);
        assert_eq!(early.to_phase, Phase::Active);
        assert_eq!(
            authority.state(),
            &before,
            "an early timer fire at {now_ms} is byte-identical"
        );
    }

    let due = apply(&mut authority, expire(DEADLINE));
    assert_eq!(
        due.effects(),
        &[Effect::Expired {
            grant_id: GRANT.to_owned(),
            expires_at_ms: DEADLINE,
            observed_at_ms: DEADLINE,
        }]
    );
    assert_eq!(authority.state().lifecycle_phase, Phase::Expired);
    assert_eq!(authority.state().ended_at_ms, Some(DEADLINE));
    assert_eq!(
        authority.state().end_reason,
        Some(ToolGrantEndReason::Deadline)
    );

    let settled = authority.state().clone();
    let again = apply(&mut authority, expire(DEADLINE + 1));
    assert!(again.effects().is_empty());
    assert_eq!(
        authority.state(),
        &settled,
        "a settled expiry is never recorded twice"
    );
}

// ===========================================================================
// Revocation
// ===========================================================================

#[test]
fn revoke_after_the_deadline_records_expiry_not_revocation() {
    for now_ms in [DEADLINE, DEADLINE + 1_000] {
        let mut authority = authority_in(Phase::Active);
        let late = apply(&mut authority, revoke(now_ms));
        assert_eq!(
            late.effects(),
            &[Effect::Expired {
                grant_id: GRANT.to_owned(),
                expires_at_ms: DEADLINE,
                observed_at_ms: now_ms,
            }],
            "a revoke at {now_ms} records the expiry the grant already reached"
        );
        let state = authority.state();
        assert_eq!(state.lifecycle_phase, Phase::Expired);
        assert_eq!(state.end_reason, Some(ToolGrantEndReason::Deadline));
        assert_eq!(state.ended_at_ms, Some(now_ms));
    }

    let mut authority = authority_in(Phase::Active);
    let revoked = apply(&mut authority, revoke(DEADLINE - 1));
    assert_eq!(
        revoked.effects(),
        &[Effect::Revoked {
            grant_id: GRANT.to_owned(),
            observed_at_ms: DEADLINE - 1,
        }]
    );
    let state = authority.state();
    assert_eq!(state.lifecycle_phase, Phase::Revoked);
    assert_eq!(state.end_reason, Some(ToolGrantEndReason::Revoked));
    assert_eq!(state.ended_at_ms, Some(DEADLINE - 1));

    // Revoke is idempotent on a settled grant.
    for phase in [Phase::Expired, Phase::Revoked] {
        let mut authority = authority_in(phase);
        let before = authority.state().clone();
        let again = apply(&mut authority, revoke(DEADLINE - 1));
        assert!(again.effects().is_empty(), "{phase:?} revoke emits nothing");
        assert_eq!(
            authority.state(),
            &before,
            "{phase:?} revoke changes nothing"
        );
    }
}

// ===========================================================================
// Terminal latch
// ===========================================================================

#[test]
fn terminal_grants_refuse_use_after_a_clock_rollback() {
    for (phase, reason) in [
        (Phase::Expired, ToolGrantUseRefusal::Expired),
        (Phase::Revoked, ToolGrantUseRefusal::Revoked),
    ] {
        let live = authority_in(phase);
        // A restart rebuilds the authority from the persisted state.
        let mut authority =
            Authority::recover_from_state(live.state().clone()).expect("a settled row recovers");
        let before = authority.state().clone();
        // The host clock rolled back to before the deadline, and even before
        // the issue time.
        for now_ms in [ISSUED_AT - 1, ISSUED_AT, DEADLINE - 1] {
            let refused = apply(&mut authority, admit(now_ms));
            assert_eq!(
                refused.effects(),
                &[Effect::UseRefused {
                    grant_id: GRANT.to_owned(),
                    reason,
                }],
                "{phase:?} refuses a use at the rolled-back time {now_ms}"
            );
            for input in [expire(now_ms), revoke(now_ms)] {
                let settled = apply(&mut authority, input);
                assert!(settled.effects().is_empty());
            }
            let reissue = apply(&mut authority, issue(GRANT, now_ms, DEADLINE, MAX_LIFETIME));
            assert_eq!(
                reissue.effects(),
                &[Effect::IssueRefused {
                    reason: ToolGrantIssueRefusal::AlreadyIssued
                }]
            );
            assert_eq!(
                authority.state(),
                &before,
                "{phase:?} stays latched at the rolled-back time {now_ms}"
            );
        }
    }
}

// ===========================================================================
// TLC model
// ===========================================================================

/// The machine's CI run is held to every arm: its explicit TLC model requires
/// each to generate a state, the `IssueRefusedIssued*` arms of an ended grant
/// included, within a step bound that reaches them (issue, end, issue again).
#[test]
fn the_explicit_tlc_model_requires_every_arm_in_ci() {
    let schema = meerkat_machine_schema::catalog::dsl::dsl_tool_grant_lifecycle_machine();
    let model = schema.tlc_model.as_ref().expect("an explicit TLC model");
    assert!(model.require_ci_transition_coverage);
    assert!(
        model.ci_limits.step_limit >= 3,
        "issue, end and issue again fit the CI step bound"
    );
    schema.validate().expect("the explicit model validates");
    for arm in [
        "IssueRefusedIssuedActive",
        "IssueRefusedIssuedExpired",
        "IssueRefusedIssuedRevoked",
    ] {
        assert!(
            schema
                .transitions
                .iter()
                .any(|transition| transition.name.as_str() == arm),
            "{arm} is an arm the CI run must reach"
        );
    }
}

// ===========================================================================
// Recovery
// ===========================================================================

#[test]
fn recover_from_state_rejects_torn_rows() {
    // Every reachable phase recovers unchanged.
    for phase in ALL_PHASES {
        let state = authority_in(phase).state().clone();
        let recovered = Authority::recover_from_state(state.clone())
            .unwrap_or_else(|error| panic!("{phase:?} must recover: {error}"));
        assert_eq!(recovered.state(), &state);
    }

    let issued = authority_in(Phase::Active).state().clone();
    let torn_rows: Vec<(&str, State)> = vec![
        (
            "unissued_is_empty",
            State {
                grant_id: GRANT.to_owned(),
                ..State::default()
            },
        ),
        (
            "issued_window_is_nonempty",
            State {
                expires_at_ms: issued.issued_at_ms,
                ..issued.clone()
            },
        ),
        (
            "active_has_no_end",
            State {
                ended_at_ms: Some(DEADLINE - 1),
                ..issued.clone()
            },
        ),
        (
            "ended_grants_record_their_end",
            State {
                lifecycle_phase: Phase::Revoked,
                ended_at_ms: Some(DEADLINE - 1),
                end_reason: None,
                ..issued.clone()
            },
        ),
        (
            "expiry_never_precedes_its_deadline",
            State {
                lifecycle_phase: Phase::Expired,
                ended_at_ms: Some(DEADLINE - 1),
                end_reason: Some(ToolGrantEndReason::Deadline),
                ..issued.clone()
            },
        ),
        (
            "revocation_precedes_the_deadline",
            State {
                lifecycle_phase: Phase::Revoked,
                ended_at_ms: Some(DEADLINE),
                end_reason: Some(ToolGrantEndReason::Revoked),
                ..issued.clone()
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

    // An end reason that contradicts the terminal phase is torn too.
    for (phase, reason) in [
        (Phase::Expired, ToolGrantEndReason::Revoked),
        (Phase::Revoked, ToolGrantEndReason::Deadline),
    ] {
        let torn = State {
            lifecycle_phase: phase,
            ended_at_ms: Some(if phase == Phase::Expired {
                DEADLINE
            } else {
                DEADLINE - 1
            }),
            end_reason: Some(reason),
            ..issued.clone()
        };
        assert!(
            Authority::recover_from_state(torn).is_err(),
            "{phase:?} with end reason {reason:?} must not recover"
        );
    }
}

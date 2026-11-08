//! Run-scoped grants on the canonical `ToolGrantLifecycleMachine` (version
//! 2): a grant issued for one run admits uses in that run only, ends when
//! its run ends (or when a use from a later run finds it), and refuses a use
//! outside any run without ending. The window still applies to a run-scoped
//! grant: it expires at its deadline whatever its run does.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use meerkat_machine_schema::catalog::dsl::tool_grant_lifecycle::{
    ToolGrantEndReason, ToolGrantIssueRefusal, ToolGrantLifecycleEffect, ToolGrantLifecycleInput,
    ToolGrantLifecycleMachineAuthority, ToolGrantLifecycleMachineMutator,
    ToolGrantLifecycleMachineState, ToolGrantLifecycleMachineTransition,
    ToolGrantLifecycleMachineTransitionError, ToolGrantPhase, ToolGrantRunId, ToolGrantUseRefusal,
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
const MAX_LIFETIME: u64 = 24 * 60 * 60 * 1_000;

fn run(id: &str) -> ToolGrantRunId {
    ToolGrantRunId::from(id)
}

fn apply(authority: &mut Authority, input: Input) -> Transition {
    ToolGrantLifecycleMachineMutator::apply(authority, input)
        .expect("the input must resolve to an explicit typed transition")
}

fn try_apply(authority: &mut Authority, input: Input) -> Result<Transition, TransitionError> {
    ToolGrantLifecycleMachineMutator::apply(authority, input)
}

fn issue_scoped(run_scope: Option<&str>) -> Input {
    Input::Issue {
        grant_id: GRANT.to_owned(),
        now_ms: ISSUED_AT,
        expires_at_ms: DEADLINE,
        max_lifetime_ms: MAX_LIFETIME,
        run_scope: run_scope.map(run),
    }
}

fn admit(now_ms: u64, run_id: Option<&str>) -> Input {
    Input::AdmitUse {
        now_ms,
        run_id: run_id.map(run),
    }
}

fn end_run(run_id: &str, now_ms: u64) -> Input {
    Input::EndRun {
        run_id: run(run_id),
        now_ms,
    }
}

fn end_with_subject(now_ms: u64) -> Input {
    Input::EndWithSubject { now_ms }
}

/// A grant issued for the run `run-1`.
fn scoped() -> Authority {
    let mut authority = Authority::new();
    let issued = apply(&mut authority, issue_scoped(Some("run-1")));
    assert_eq!(
        issued.effects(),
        &[Effect::Issued {
            grant_id: GRANT.to_owned(),
            expires_at_ms: DEADLINE,
        }]
    );
    assert_eq!(authority.state().run_scope, Some(run("run-1")));
    authority
}

/// A grant bounded by its window only.
fn window() -> Authority {
    let mut authority = Authority::new();
    apply(&mut authority, issue_scoped(None));
    assert_eq!(authority.state().run_scope, None);
    authority
}

fn refused(reason: ToolGrantUseRefusal) -> Effect {
    Effect::UseRefused {
        grant_id: GRANT.to_owned(),
        reason,
    }
}

fn run_ended(observed_at_ms: u64) -> Effect {
    Effect::RunEnded {
        grant_id: GRANT.to_owned(),
        observed_at_ms,
    }
}

#[test]
fn machine_admits_a_run_scoped_use_only_in_its_run() {
    // Its own run is admitted, and the admission changes nothing.
    let mut authority = scoped();
    let before = authority.state().clone();
    let admitted = apply(&mut authority, admit(DEADLINE - 1, Some("run-1")));
    assert_eq!(
        admitted.effects(),
        &[Effect::UseAdmitted {
            grant_id: GRANT.to_owned()
        }]
    );
    assert_eq!(authority.state(), &before);

    // A use outside any run is refused, and the grant stays live for its run.
    let outside = apply(&mut authority, admit(DEADLINE - 1, None));
    assert_eq!(
        outside.effects(),
        &[refused(ToolGrantUseRefusal::OutsideRun)]
    );
    assert_eq!(
        authority.state(),
        &before,
        "a use outside a run ends nothing"
    );
    let again = apply(&mut authority, admit(DEADLINE - 1, Some("run-1")));
    assert_eq!(
        again.effects(),
        &[Effect::UseAdmitted {
            grant_id: GRANT.to_owned()
        }]
    );

    // A use from another run ends the grant: a session has one current run,
    // so the grant's own run is over.
    let other = apply(&mut authority, admit(DEADLINE - 1, Some("run-2")));
    assert_eq!(
        other.effects(),
        &[
            run_ended(DEADLINE - 1),
            refused(ToolGrantUseRefusal::RunEnded)
        ]
    );
    let state = authority.state();
    assert_eq!(state.lifecycle_phase, Phase::RunEnded);
    assert_eq!(state.end_reason, Some(ToolGrantEndReason::RunEnded));
    assert_eq!(state.ended_at_ms, Some(DEADLINE - 1));
    assert_eq!(state.run_scope, Some(run("run-1")), "the scope never moves");

    // A window grant admits any run, and a use outside a run.
    let mut authority = window();
    for run_id in [Some("run-1"), Some("run-2"), None] {
        let admitted = apply(&mut authority, admit(DEADLINE - 1, run_id));
        assert_eq!(
            admitted.effects(),
            &[Effect::UseAdmitted {
                grant_id: GRANT.to_owned()
            }],
            "a window grant admits a use from {run_id:?}"
        );
    }
}

#[test]
fn machine_ends_a_grant_only_at_the_end_of_its_own_run() {
    // The end of another run, or of any run for a window grant, is a no-op.
    for (mut authority, label) in [(scoped(), "scoped"), (window(), "window")] {
        let before = authority.state().clone();
        let other = apply(&mut authority, end_run("run-2", DEADLINE - 1));
        assert!(
            other.effects().is_empty(),
            "{label}: the end of another run emits nothing"
        );
        assert_eq!(authority.state(), &before, "{label}: and changes nothing");
    }

    // The end of its own run before the deadline ends it.
    let mut authority = scoped();
    let ended = apply(&mut authority, end_run("run-1", DEADLINE - 1));
    assert_eq!(ended.effects(), &[run_ended(DEADLINE - 1)]);
    assert_eq!(authority.state().lifecycle_phase, Phase::RunEnded);
    assert_eq!(
        authority.state().end_reason,
        Some(ToolGrantEndReason::RunEnded)
    );

    // At or after the deadline the grant already expired: the run's end
    // records that expiry, as a late revoke does.
    for now_ms in [DEADLINE, DEADLINE + 1_000] {
        let mut authority = scoped();
        let late = apply(&mut authority, end_run("run-1", now_ms));
        assert_eq!(
            late.effects(),
            &[Effect::Expired {
                grant_id: GRANT.to_owned(),
                expires_at_ms: DEADLINE,
                observed_at_ms: now_ms,
            }]
        );
        assert_eq!(authority.state().lifecycle_phase, Phase::Expired);
        assert_eq!(
            authority.state().end_reason,
            Some(ToolGrantEndReason::Deadline)
        );
    }

    // The window still applies inside the grant's own run.
    let mut authority = scoped();
    let due = apply(&mut authority, admit(DEADLINE, Some("run-1")));
    assert_eq!(
        due.effects(),
        &[
            Effect::Expired {
                grant_id: GRANT.to_owned(),
                expires_at_ms: DEADLINE,
                observed_at_ms: DEADLINE,
            },
            refused(ToolGrantUseRefusal::Expired),
        ]
    );
    // A use from another run at the deadline records the expiry, not a run
    // end.
    let mut authority = scoped();
    apply(&mut authority, admit(DEADLINE, Some("run-2")));
    assert_eq!(authority.state().lifecycle_phase, Phase::Expired);
}

#[test]
fn a_run_ended_grant_is_terminal_and_latched() {
    let mut authority = scoped();
    apply(&mut authority, end_run("run-1", DEADLINE - 1));
    // A restart rebuilds the machine from its row.
    let mut authority =
        Authority::recover_from_state(authority.state().clone()).expect("the row recovers");
    let before = authority.state().clone();
    // Even with the clock rolled back before the end, and in its own run.
    for now_ms in [ISSUED_AT - 1, ISSUED_AT, DEADLINE - 1, DEADLINE + 1] {
        for run_id in [Some("run-1"), Some("run-2"), None] {
            let refused_use = apply(&mut authority, admit(now_ms, run_id));
            assert_eq!(
                refused_use.effects(),
                &[refused(ToolGrantUseRefusal::RunEnded)],
                "a run-ended grant refuses a use from {run_id:?} at {now_ms}"
            );
        }
        for input in [
            Input::Expire { now_ms },
            Input::Revoke { now_ms },
            end_run("run-1", now_ms),
            end_run("run-2", now_ms),
        ] {
            let settled = apply(&mut authority, input);
            assert!(settled.effects().is_empty());
        }
        let reissue = apply(&mut authority, issue_scoped(Some("run-1")));
        assert_eq!(
            reissue.effects(),
            &[Effect::IssueRefused {
                reason: ToolGrantIssueRefusal::AlreadyIssued
            }]
        );
        assert_eq!(authority.state(), &before, "latched at {now_ms}");
    }

    // The other ended phases settle the end of their run without a change.
    let mut expired = scoped();
    apply(&mut expired, Input::Expire { now_ms: DEADLINE });
    let mut revoked = scoped();
    apply(
        &mut revoked,
        Input::Revoke {
            now_ms: DEADLINE - 1,
        },
    );
    for mut authority in [expired, revoked] {
        let before = authority.state().clone();
        let settled = apply(&mut authority, end_run("run-1", DEADLINE - 1));
        assert!(settled.effects().is_empty());
        assert_eq!(authority.state(), &before);
    }

    // An unissued record has no run to end.
    let mut unissued = Authority::new();
    assert!(matches!(
        try_apply(&mut unissued, end_run("run-1", ISSUED_AT)),
        Err(TransitionError::NoMatchingTransition { .. } | TransitionError::GuardRejected { .. })
    ));
}

/// A run-scoped grant whose subject ends before its run does ends with the
/// subject, as any grant does: revoked, its end reason `SubjectEnded`, its
/// run scope kept. Every later use, one of its own run included, is refused
/// as ended with its subject (which the owner service answers as no live
/// grant), and the end of its run then settles without a change.
#[test]
fn a_run_scoped_grant_ends_with_its_subject_and_refuses_its_own_run() {
    let mut authority = scoped();
    let ended = apply(&mut authority, end_with_subject(DEADLINE - 1));
    assert_eq!(
        ended.effects(),
        &[Effect::Revoked {
            grant_id: GRANT.to_owned(),
            observed_at_ms: DEADLINE - 1,
        }]
    );
    let after = authority.state().clone();
    assert_eq!(after.lifecycle_phase, Phase::Revoked);
    assert_eq!(after.end_reason, Some(ToolGrantEndReason::SubjectEnded));
    assert_eq!(after.ended_at_ms, Some(DEADLINE - 1));
    assert_eq!(after.run_scope, Some(run("run-1")), "the scope never moves");
    for run_id in [Some("run-1"), Some("run-2"), None] {
        let refused_use = apply(&mut authority, admit(DEADLINE - 1, run_id));
        assert_eq!(
            refused_use.effects(),
            &[refused(ToolGrantUseRefusal::SubjectEnded)],
            "a use from {run_id:?} is refused as ended with the subject"
        );
    }
    let settled = apply(&mut authority, end_run("run-1", DEADLINE - 1));
    assert!(settled.effects().is_empty(), "the run's end settles");
    assert_eq!(authority.state(), &after, "and changes nothing");
}

/// A grant whose run already ended settles the end of its subject without a
/// change, at any clock reading: it stays run-ended, and its uses stay
/// refused as run-ended, never as ended with its subject.
#[test]
fn a_run_ended_grant_settles_its_subjects_end_unchanged() {
    let mut authority = scoped();
    apply(&mut authority, end_run("run-1", DEADLINE - 1));
    let before = authority.state().clone();
    for now_ms in [ISSUED_AT, DEADLINE - 1, DEADLINE, DEADLINE + 1] {
        let settled = apply(&mut authority, end_with_subject(now_ms));
        assert!(
            settled.effects().is_empty(),
            "nothing is emitted at {now_ms}"
        );
        assert_eq!(authority.state(), &before, "nothing changes at {now_ms}");
    }
    let refused_use = apply(&mut authority, admit(DEADLINE - 1, Some("run-1")));
    assert_eq!(
        refused_use.effects(),
        &[refused(ToolGrantUseRefusal::RunEnded)]
    );
}

#[test]
fn recover_from_state_rejects_torn_run_scope_rows() {
    let mut ended = scoped();
    apply(&mut ended, end_run("run-1", DEADLINE - 1));
    let ended = ended.state().clone();
    Authority::recover_from_state(ended.clone()).expect("a run-ended row recovers");

    let torn_rows: Vec<(&str, State)> = vec![
        (
            "run_scope_set_only_by_issue",
            State {
                run_scope: Some(run("run-1")),
                ..State::default()
            },
        ),
        (
            "run_ended_records_its_end",
            State {
                end_reason: Some(ToolGrantEndReason::Deadline),
                ..ended.clone()
            },
        ),
        (
            "run_ended_records_its_end",
            State {
                ended_at_ms: Some(DEADLINE),
                ..ended.clone()
            },
        ),
        (
            "run_ended_records_its_end",
            State {
                run_scope: None,
                ..ended.clone()
            },
        ),
        (
            "ended_grants_record_their_end",
            State {
                ended_at_ms: None,
                end_reason: None,
                ..ended
            },
        ),
    ];
    for (invariant, torn) in torn_rows {
        match Authority::recover_from_state(torn.clone()) {
            Err(TransitionError::RecoveredStateInvariantRejected {
                invariant: rejected,
                ..
            }) => assert_eq!(rejected, invariant, "{torn:?}"),
            Err(other) => panic!("{invariant}: unexpected refusal {other}"),
            Ok(_) => panic!("{invariant}: a torn row must not recover: {torn:?}"),
        }
    }
}

/// The explicit TLC model samples two run keys, so a use from another run
/// and the end of another run are explored, and its CI run requires every
/// new arm to generate a state.
#[test]
fn the_explicit_tlc_model_explores_two_runs_and_every_run_scope_arm() {
    let schema = meerkat_machine_schema::catalog::dsl::dsl_tool_grant_lifecycle_machine();
    let model = schema.tlc_model.as_ref().expect("an explicit TLC model");
    assert!(model.require_ci_transition_coverage);
    schema.validate().expect("the explicit model validates");
    for profile in [&model.ci, &model.deep] {
        let runs = profile
            .named_values
            .iter()
            .find(|(name, _)| name.as_str() == "ToolGrantRunId")
            .map(|(_, values)| values.len());
        assert_eq!(runs, Some(2), "two run keys are sampled");
    }
    for arm in [
        "AdmitOtherRun",
        "AdmitOutsideRun",
        "AdmitEndedRunEnded",
        "EndRunMatching",
        "EndRunAfterDeadline",
        "EndRunOther",
        "EndRunSettledExpired",
        "EndRunSettledRevoked",
        "EndRunSettledRunEnded",
        "IssueRefusedIssuedRunEnded",
        "ExpireSettledRunEnded",
        "RevokeSettledRunEnded",
        // The end with its subject of a grant whose run ended.
        "EndWithSubjectSettledRunEnded",
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

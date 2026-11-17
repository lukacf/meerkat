//! Defer-sweep totality pin — the backlog-wedge class killer.
//!
//! The 0.7.23 field failure ("Internal error: DSL rejected
//! DeferInputBehindBacklog: GuardRejected { phase: Attached }" followed by a
//! stranded runtime queue) had one root cause: the runtime loop's
//! whole-failed-batch defer sweep assumed every batch member returned to a
//! work lane, but the machine's own failure realization legitimately resolves
//! members out of the queued world (`ResolveStagedRollbackMaxAttemptsExhausted`
//! abandons an input at the retry cap; boundary-applied members sit in
//! Applied/AppliedPendingConsumption with no lane entry). The shell then
//! treated the machine's own decision as an internal error and dropped the
//! wake, wedging the backlog.
//!
//! The fix is machine-owned totality: `DeferInputBehindBacklog` defers lane
//! members, and `DeferInputBehindBacklogAlreadyResolved` accepts
//! tracked-but-lane-less members that have provably left `Queued` as an
//! explicit no-op. A tracked input claiming `Queued` with no lane entry stays
//! a loud rejection — that is projection corruption, not a resolved member.
//! After exact run cancellation, terminal publication may also archive an
//! input before the defer sweep. The archived no-op requires that exact
//! cancelled terminal run and absence from every live input index.
//!
//! Archived admission was widened once, on purpose, for a refused request:
//! a run whose request was refused with nothing applied ends its inputs
//! with the run (they are not returned to a work lane), and a persistent
//! driver archives them with their receipt before the defer sweep, so the
//! sweep meets inputs the machine no longer tracks. The
//! `DeferInputBehindBacklogEndedWithRefusedRun` no-op accepts such an input
//! only while the machine's own run terminal is that refused request (the
//! run failed, its cause kind is a failed apply, its apply failure cause is
//! `RequestRefused`), for a sweep that is not a cancellation's, and only
//! when the input is absent from every live input index. An untracked input
//! of any other failed batch stays a loud rejection. The arm names no run:
//! a sweep that is not a cancellation's carries none. It is bound to the run
//! terminal the machine holds, which is sound because each sweep directly
//! follows its own run's persisted terminal, before any other run starts.
//!
//! This test pins all four arms: removing a no-op, absorbing corruption, or
//! widening archived admission must remain a deliberate, reviewed change.

use meerkat_machine_schema::catalog::dsl::dsl_meerkat_machine;
use meerkat_machine_schema::identity::{EnumTypeId, EnumVariantId, FieldId, IdentityError};
use meerkat_machine_schema::{Expr, Guard, TriggerMatch};

const DEFER_INPUT: &str = "DeferInputBehindBacklog";

const PHASES: [&str; 5] = ["Idle", "Attached", "Running", "Retired", "Stopped"];

fn has_conjunct(expr: &Expr, expected: &Expr) -> bool {
    match expr {
        Expr::And(terms) => terms.iter().any(|term| has_conjunct(term, expected)),
        _ => expr == expected,
    }
}

fn assert_archived_guards(guards: &[Guard]) -> Result<(), IdentityError> {
    let required = [
        Expr::Neq(
            Box::new(Expr::Binding("cancelled_run_id".into())),
            Box::new(Expr::None),
        ),
        Expr::Eq(
            Box::new(Expr::Field(FieldId::parse("turn_terminal_run_id")?)),
            Box::new(Expr::Binding("cancelled_run_id".into())),
        ),
        Expr::Eq(
            Box::new(Expr::Field(FieldId::parse("terminal_outcome")?)),
            Box::new(Expr::Some(Box::new(Expr::NamedVariant {
                enum_name: EnumTypeId::parse("TurnTerminalOutcome")?,
                variant: EnumVariantId::parse("Cancelled")?,
            }))),
        ),
    ];
    for expected in required {
        assert!(
            guards
                .iter()
                .any(|guard| has_conjunct(&guard.expr, &expected)),
            "archived deferral must require {expected:?}, got {guards:?}"
        );
    }
    for index in [
        "input_phases",
        "input_lane",
        "input_recovery_lanes",
        "input_run_associations",
    ] {
        let expected = Expr::Not(Box::new(Expr::MapContainsKey {
            map: Box::new(Expr::Field(FieldId::parse(index)?)),
            key: Box::new(Expr::Binding("input_id".into())),
        }));
        assert!(
            guards
                .iter()
                .any(|guard| has_conjunct(&guard.expr, &expected)),
            "archived deferral must require absence from {index}, got {guards:?}"
        );
    }
    Ok(())
}

fn assert_absent_from_every_live_index(guards: &[Guard]) -> Result<(), IdentityError> {
    for index in [
        "input_phases",
        "input_lane",
        "input_recovery_lanes",
        "input_run_associations",
    ] {
        let expected = Expr::Not(Box::new(Expr::MapContainsKey {
            map: Box::new(Expr::Field(FieldId::parse(index)?)),
            key: Box::new(Expr::Binding("input_id".into())),
        }));
        assert!(
            guards
                .iter()
                .any(|guard| has_conjunct(&guard.expr, &expected)),
            "archived deferral must require absence from {index}, got {guards:?}"
        );
    }
    Ok(())
}

/// The guards of the no-op for an input that ended with a refused run: the
/// sweep is not a cancellation's, the machine's run terminal is a refused
/// request, and the input is absent from every live input index.
fn assert_refused_run_guards(guards: &[Guard]) -> Result<(), IdentityError> {
    let some_variant = |enum_name: &str, variant: &str| -> Result<Expr, IdentityError> {
        Ok(Expr::Some(Box::new(Expr::NamedVariant {
            enum_name: EnumTypeId::parse(enum_name)?,
            variant: EnumVariantId::parse(variant)?,
        })))
    };
    let required = [
        Expr::Eq(
            Box::new(Expr::Binding("cancelled_run_id".into())),
            Box::new(Expr::None),
        ),
        Expr::Eq(
            Box::new(Expr::Field(FieldId::parse("terminal_outcome")?)),
            Box::new(some_variant("TurnTerminalOutcome", "Failed")?),
        ),
        Expr::Eq(
            Box::new(Expr::Field(FieldId::parse("terminal_cause_kind")?)),
            Box::new(some_variant(
                "TurnTerminalCauseKind",
                "RuntimeApplyFailure",
            )?),
        ),
        Expr::Eq(
            Box::new(Expr::Field(FieldId::parse(
                "last_runtime_apply_failure_cause",
            )?)),
            Box::new(some_variant("RuntimeApplyFailureCause", "RequestRefused")?),
        ),
    ];
    for expected in required {
        assert!(
            guards
                .iter()
                .any(|guard| has_conjunct(&guard.expr, &expected)),
            "deferral of an input that ended with a refused run must require {expected:?}, \
             got {guards:?}"
        );
    }
    assert_absent_from_every_live_index(guards)
}

#[test]
fn defer_input_behind_backlog_is_total_over_resolved_members() -> Result<(), IdentityError> {
    let schema = dsl_meerkat_machine();
    let mut arms: Vec<_> = schema
        .transitions
        .iter()
        .filter(|transition| match &transition.on {
            TriggerMatch::Input { variant, .. } => variant.as_ref() == DEFER_INPUT,
            TriggerMatch::Signal { .. } => false,
        })
        .collect();
    arms.sort_by(|a, b| a.name.as_ref().cmp(b.name.as_ref()));

    // per_phase expands each DSL arm into one named transition per phase.
    let mut expected: Vec<String> = PHASES
        .iter()
        .flat_map(|phase| {
            [
                format!("{DEFER_INPUT}{phase}"),
                format!("{DEFER_INPUT}AlreadyResolved{phase}"),
                format!("{DEFER_INPUT}AlreadyArchived{phase}"),
                format!("{DEFER_INPUT}EndedWithRefusedRun{phase}"),
            ]
        })
        .collect();
    expected.sort();
    let names: Vec<&str> = arms.iter().map(|t| t.name.as_ref()).collect();
    assert_eq!(
        names, expected,
        "the DeferInputBehindBacklog input must have exactly the defer, \
         already-resolved, exact-cancelled already-archived and \
         ended-with-refused-run arms"
    );

    for arm in arms {
        let name = arm.name.as_ref();
        let guards: Vec<String> = arm
            .guards
            .iter()
            .map(|guard| format!("{:?}", guard.expr))
            .collect();
        assert!(
            guards.iter().any(|g| g.contains("input_lane")),
            "{name}: every arm must be guarded on lane membership or its \
             complement, got {guards:?}"
        );
        if name.contains("AlreadyArchived") {
            assert_archived_guards(&arm.guards)?;
            assert!(
                arm.updates.is_empty() && arm.emit.is_empty(),
                "{name}: archived deferral cannot mutate state or fabricate effects"
            );
        } else if name.contains("EndedWithRefusedRun") {
            assert_refused_run_guards(&arm.guards)?;
            assert!(
                arm.updates.is_empty() && arm.emit.is_empty(),
                "{name}: deferral of an input that ended with a refused run cannot mutate \
                 state or fabricate effects"
            );
        } else if name.contains("AlreadyResolved") {
            assert!(
                guards.iter().any(|g| g.contains("input_phases")),
                "{name}: the no-op arm must require the input to be tracked, \
                 got {guards:?}"
            );
            assert!(
                guards
                    .iter()
                    .any(|g| g.contains("Queued") && g.contains("input_phases")),
                "{name}: the no-op arm must exclude inputs still claiming \
                 InputPhase::Queued (tracked-but-lane-less Queued is projection \
                 corruption and must stay a loud rejection), got {guards:?}"
            );
            assert!(
                arm.updates.is_empty(),
                "{name}: the already-resolved arm is an explicit no-op: it must \
                 not mutate admission order or lifecycle state"
            );
            assert!(
                arm.emit.is_empty(),
                "{name}: the already-resolved arm must not fabricate effects"
            );
        } else {
            assert!(
                !arm.updates.is_empty(),
                "{name}: the defer arm must reassign the admission sequence"
            );
        }
    }
    Ok(())
}

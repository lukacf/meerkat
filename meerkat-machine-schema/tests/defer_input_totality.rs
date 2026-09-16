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
//! This test pins all three arms: removing a no-op, absorbing corruption, or
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
            ]
        })
        .collect();
    expected.sort();
    let names: Vec<&str> = arms.iter().map(|t| t.name.as_ref()).collect();
    assert_eq!(
        names, expected,
        "the DeferInputBehindBacklog input must have exactly the defer, \
         already-resolved, and exact-cancelled already-archived arms"
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

//! Queued-input abandon totality pin.
//!
//! Withdrawing one selected input is safe only while the input is still
//! queued: once a run has taken it, ending it belongs to that run. The fact
//! "queued" lives in `MeerkatMachine` (`input_phases`), so the decision is a
//! machine transition with a guard, never a phase read in shell code followed
//! by the unguarded `AbandonInput`.
//!
//! `AbandonQueuedInput` is total over every input id and has exactly four
//! arms:
//!
//! - `AbandonQueuedInput`: the input is `Queued`; it becomes `Abandoned` with
//!   the given reason. It shares the `Queued` guard with `StageForRun`, so a
//!   run taking the input and this abandon cannot both commit. It exists in
//!   the phases that hold inputs, the ones the bulk `AbandonInput` has.
//! - `AbandonQueuedInputNotQueued`: the input is tracked, not queued and not
//!   terminal (a run holds it). Nothing changes.
//! - `AbandonQueuedInputAlreadyTerminal`: the input already reached a
//!   terminal. Nothing changes.
//! - `AbandonQueuedInputUntracked`: the machine does not hold the input (never
//!   admitted here, or archived after its durable terminal). Nothing changes.
//!
//! The three no-op arms exist in every phase, and every arm answers with
//! `QueuedInputAbandonResolved`, so the caller never classifies a guard
//! rejection. Removing an arm, letting a no-op arm mutate, or widening the
//! abandoning arm past `Queued` must stay a deliberate change.

#![allow(clippy::expect_used, clippy::panic)]

use meerkat_machine_schema::catalog::dsl::dsl_meerkat_machine;
use meerkat_machine_schema::{TransitionSchema, TriggerMatch};

const INPUT: &str = "AbandonQueuedInput";
const RESOLVED: &str = "QueuedInputAbandonResolved";

/// The phases that hold inputs: where the bulk `AbandonInput` exists.
const INPUT_PHASES: [&str; 5] = ["Idle", "Attached", "Running", "Retired", "Stopped"];
const ALL_PHASES: [&str; 7] = [
    "Initializing",
    "Idle",
    "Attached",
    "Running",
    "Retired",
    "Stopped",
    "Destroyed",
];

const NOT_QUEUED_PHASES: [&str; 3] = ["Staged", "Applied", "AppliedPendingConsumption"];
const TERMINAL_PHASES: [&str; 4] = ["Consumed", "Superseded", "Coalesced", "Abandoned"];

fn arms_of(input: &str) -> Vec<TransitionSchema> {
    let schema = dsl_meerkat_machine();
    let mut arms: Vec<TransitionSchema> = schema
        .transitions
        .iter()
        .filter(|transition| match &transition.on {
            TriggerMatch::Input { variant, .. } => variant.as_ref() == input,
            TriggerMatch::Signal { .. } => false,
        })
        .cloned()
        .collect();
    arms.sort_by(|a, b| a.name.as_ref().cmp(b.name.as_ref()));
    arms
}

fn is_no_op_arm(name: &str) -> bool {
    name.contains("NotQueued") || name.contains("AlreadyTerminal") || name.contains("Untracked")
}

fn guard_text(arm: &TransitionSchema) -> String {
    arm.guards
        .iter()
        .map(|guard| format!("{:?}", guard.expr))
        .collect::<Vec<_>>()
        .join(" && ")
}

/// Whether the guards name the `InputPhase` variant `phase` as a whole word
/// (`Applied` must not match inside `AppliedPendingConsumption`).
fn names_phase(guards: &str, phase: &str) -> bool {
    guards.contains(&format!("\"{phase}\""))
}

fn emitted(arm: &TransitionSchema) -> Vec<&str> {
    arm.emit.iter().map(|emit| emit.variant.as_ref()).collect()
}

#[test]
fn abandon_queued_input_has_exactly_the_four_arms() {
    let arms = arms_of(INPUT);
    let mut expected: Vec<String> = INPUT_PHASES
        .iter()
        .map(|phase| format!("{INPUT}{phase}"))
        .chain(ALL_PHASES.iter().flat_map(|phase| {
            [
                format!("{INPUT}NotQueued{phase}"),
                format!("{INPUT}AlreadyTerminal{phase}"),
                format!("{INPUT}Untracked{phase}"),
            ]
        }))
        .collect();
    expected.sort();
    let names: Vec<&str> = arms.iter().map(|arm| arm.name.as_ref()).collect();
    assert_eq!(
        names, expected,
        "the AbandonQueuedInput input must have exactly the abandoning arm in \
         the phases that hold inputs and the not-queued, already-terminal and \
         untracked arms in every phase"
    );
}

#[test]
fn only_the_queued_arm_abandons_and_every_arm_answers() {
    let arms = arms_of(INPUT);
    assert!(!arms.is_empty(), "no {INPUT} arm exists");
    for arm in &arms {
        let name = arm.name.as_ref();
        let guards = guard_text(arm);
        assert!(
            guards.contains("input_phases"),
            "{name}: every arm is decided by the machine's input phase, got {guards}"
        );
        assert!(
            emitted(arm).contains(&RESOLVED),
            "{name}: every arm answers with {RESOLVED}, got {:?}",
            emitted(arm)
        );
        if is_no_op_arm(name) {
            assert!(
                arm.updates.is_empty(),
                "{name}: a no-op arm must not change any input state"
            );
            assert_eq!(
                emitted(arm),
                vec![RESOLVED],
                "{name}: a no-op arm must not fabricate a terminal or any other effect"
            );
        } else {
            assert!(
                !arm.updates.is_empty(),
                "{name}: the queued arm records the abandoned terminal"
            );
            assert!(
                emitted(arm).contains(&"RecordTerminalOutcome"),
                "{name}: the queued arm records its terminal outcome, got {:?}",
                emitted(arm)
            );
        }
    }
}

#[test]
fn the_abandoning_arm_requires_queued_and_nothing_else() {
    let mut abandoning = 0;
    for arm in arms_of(INPUT) {
        let name = arm.name.as_ref();
        if is_no_op_arm(name) {
            continue;
        }
        abandoning += 1;
        let guards = guard_text(&arm);
        assert!(
            names_phase(&guards, "Queued"),
            "{name}: the abandoning arm must require InputPhase::Queued, got {guards}"
        );
        for phase in NOT_QUEUED_PHASES.iter().chain(TERMINAL_PHASES.iter()) {
            assert!(
                !names_phase(&guards, phase),
                "{name}: the abandoning arm must not admit {phase}, got {guards}"
            );
        }
    }
    assert_eq!(
        abandoning,
        INPUT_PHASES.len(),
        "one abandoning arm per phase that holds inputs"
    );
}

#[test]
fn the_no_op_arms_partition_the_other_phases() {
    let (mut not_queued, mut already_terminal, mut untracked) = (0, 0, 0);
    for arm in arms_of(INPUT) {
        let name = arm.name.as_ref();
        let guards = guard_text(&arm);
        if name.contains("NotQueued") {
            not_queued += 1;
            for phase in NOT_QUEUED_PHASES {
                assert!(
                    names_phase(&guards, phase),
                    "{name}: the not-queued arm must cover {phase}, got {guards}"
                );
            }
            for phase in TERMINAL_PHASES.iter().chain(["Queued"].iter()) {
                assert!(
                    !names_phase(&guards, phase),
                    "{name}: the not-queued arm must not cover {phase}, got {guards}"
                );
            }
        } else if name.contains("AlreadyTerminal") {
            already_terminal += 1;
            for phase in TERMINAL_PHASES {
                assert!(
                    names_phase(&guards, phase),
                    "{name}: the already-terminal arm must cover {phase}, got {guards}"
                );
            }
            for phase in NOT_QUEUED_PHASES.iter().chain(["Queued"].iter()) {
                assert!(
                    !names_phase(&guards, phase),
                    "{name}: the already-terminal arm must not cover {phase}, got {guards}"
                );
            }
        } else if name.contains("Untracked") {
            untracked += 1;
            assert!(
                guards.contains("Not(") && guards.contains("MapContainsKey"),
                "{name}: the untracked arm must require the input to be absent \
                 from input_phases, got {guards}"
            );
            for phase in NOT_QUEUED_PHASES
                .iter()
                .chain(TERMINAL_PHASES.iter())
                .chain(["Queued"].iter())
            {
                assert!(
                    !names_phase(&guards, phase),
                    "{name}: the untracked arm names no phase, got {guards}"
                );
            }
        }
    }
    assert_eq!(
        (not_queued, already_terminal, untracked),
        (ALL_PHASES.len(), ALL_PHASES.len(), ALL_PHASES.len()),
        "each no-op arm exists in every phase"
    );
}

/// The bulk and teardown terminal stays what it was: one unconditional
/// `AbandonInput` arm per phase, with no queued guard. In-flight inputs of a
/// retire, reset, stop or destroy still end through it.
#[test]
fn the_bulk_abandon_input_keeps_its_single_arm_per_phase() {
    let arms = arms_of("AbandonInput");
    let mut expected: Vec<String> = INPUT_PHASES
        .iter()
        .map(|phase| format!("AbandonInput{phase}"))
        .collect();
    expected.sort();
    let names: Vec<&str> = arms.iter().map(|arm| arm.name.as_ref()).collect();
    assert_eq!(names, expected);
    for arm in &arms {
        let guards = guard_text(arm);
        assert!(
            !names_phase(&guards, "Queued"),
            "{}: the bulk abandon has no queued guard, got {guards}",
            arm.name.as_ref()
        );
    }
}

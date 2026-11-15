//! Completion cleanup rule pin.
//!
//! After a completion waiter is answered, a surface asks
//! `ResolveRuntimeCompletionCleanup` what to do with the session's runtime:
//! retire it (`CleanupRuntime`) or keep it (`RetainRuntime`). The question is
//! asked on a fork of the authority, so no arm may change machine state, and
//! every arm releases the request's pre-admission.
//!
//! The arms, each guarded on an observation that names this session and this
//! runtime registration:
//!
//! - `ResolveRuntimeCompletionCleanupArchived`: an archived session is
//!   cleaned up whatever the outcome, in every phase that has arms.
//! - `ResolveRuntimeCompletionCleanupRuntimeTerminated`: one terminated
//!   input on a runtime that never started or no longer serves
//!   (Initializing, Stopped, and Retired with nothing left to drain) cleans
//!   the runtime up.
//! - `ResolveRuntimeCompletionCleanupInputTerminatedWhileServing`: one
//!   terminated input on a runtime that serves (Idle, Attached, Running)
//!   keeps the runtime: the input ended, the runtime did not.
//! - `ResolveRuntimeCompletionCleanupInputTerminatedWhileDraining`: one
//!   terminated input on a retired runtime that still has work to drain (a
//!   run bound, or an input queued) keeps the runtime, so the drain goes on.
//! - `ResolveRuntimeCompletionCleanupFinalizationFailed`,
//!   `...RuntimeApplyFailed`, `...AbandonedWithoutLiveSession`,
//!   `...CancelledWithoutLiveSession`: clean up, in every phase with arms.
//! - `ResolveRuntimeCompletionCleanupRetain`: every other outcome keeps the
//!   runtime.
//!
//! A destroyed runtime has no arm. Adding or removing an arm, moving a phase
//! between the terminated-input arms, or letting an arm drop a guard or
//! write state must stay a deliberate change.

#![allow(clippy::expect_used, clippy::panic)]

use meerkat_machine_schema::catalog::dsl::dsl_meerkat_machine;
use meerkat_machine_schema::{TransitionSchema, TriggerMatch};

const INPUT: &str = "ResolveRuntimeCompletionCleanup";
const RESOLVED: &str = "RuntimeCompletionCleanupResolved";

/// The phases in which a runtime takes and runs input.
const SERVING_PHASES: [&str; 3] = ["Idle", "Attached", "Running"];
/// The phases with arms in which a runtime does not serve.
const NOT_SERVING_PHASES: [&str; 3] = ["Initializing", "Retired", "Stopped"];
const ARM_PHASES: [&str; 6] = [
    "Initializing",
    "Idle",
    "Attached",
    "Running",
    "Retired",
    "Stopped",
];

const TERMINATED_NOT_SERVING: &str = "ResolveRuntimeCompletionCleanupRuntimeTerminated";
const TERMINATED_WHILE_SERVING: &str = "ResolveRuntimeCompletionCleanupInputTerminatedWhileServing";
const TERMINATED_WHILE_DRAINING: &str =
    "ResolveRuntimeCompletionCleanupInputTerminatedWhileDraining";
/// The guard of the retired runtime's cleanup arm, and of the arm that keeps
/// a retired runtime: each other's complement over the machine's own state.
const NOTHING_LEFT_TO_DRAIN: &str = "nothing_left_to_drain";
const WORK_LEFT_TO_DRAIN: &str = "work_left_to_drain";
const EVERY_PHASE_ARMS: [&str; 6] = [
    "ResolveRuntimeCompletionCleanupArchived",
    "ResolveRuntimeCompletionCleanupFinalizationFailed",
    "ResolveRuntimeCompletionCleanupRuntimeApplyFailed",
    "ResolveRuntimeCompletionCleanupAbandonedWithoutLiveSession",
    "ResolveRuntimeCompletionCleanupCancelledWithoutLiveSession",
    "ResolveRuntimeCompletionCleanupRetain",
];

fn arms() -> Vec<TransitionSchema> {
    let schema = dsl_meerkat_machine();
    let mut arms: Vec<TransitionSchema> = schema
        .transitions
        .iter()
        .filter(|transition| match &transition.on {
            TriggerMatch::Input { variant, .. } => variant.as_ref() == INPUT,
            TriggerMatch::Signal { .. } => false,
        })
        .cloned()
        .collect();
    arms.sort_by(|a, b| a.name.as_ref().cmp(b.name.as_ref()));
    arms
}

fn guard_names(arm: &TransitionSchema) -> Vec<&str> {
    arm.guards.iter().map(|guard| guard.name.as_str()).collect()
}

/// The action an arm answers, read from its one emitted effect.
fn action(arm: &TransitionSchema) -> &'static str {
    let name = arm.name.as_ref();
    assert_eq!(
        arm.emit
            .iter()
            .map(|emit| emit.variant.as_ref())
            .collect::<Vec<_>>(),
        vec![RESOLVED],
        "{name}: a cleanup arm answers with {RESOLVED} and nothing else"
    );
    let fields = format!("{:?}", arm.emit[0].fields);
    let answered: Vec<&'static str> = ["RetainRuntime", "CleanupRuntime", "CleanupArchivedSession"]
        .into_iter()
        .filter(|action| fields.contains(action))
        .collect();
    assert_eq!(
        answered.len(),
        1,
        "{name}: the arm answers exactly one action, got {fields}"
    );
    answered[0]
}

#[test]
fn the_cleanup_rule_has_exactly_its_arms() {
    let mut expected: Vec<String> = EVERY_PHASE_ARMS
        .iter()
        .flat_map(|arm| ARM_PHASES.iter().map(move |phase| format!("{arm}{phase}")))
        .chain(
            NOT_SERVING_PHASES
                .iter()
                .map(|phase| format!("{TERMINATED_NOT_SERVING}{phase}")),
        )
        .chain(
            SERVING_PHASES
                .iter()
                .map(|phase| format!("{TERMINATED_WHILE_SERVING}{phase}")),
        )
        .chain(std::iter::once(format!(
            "{TERMINATED_WHILE_DRAINING}Retired"
        )))
        .collect();
    expected.sort();
    let arms = arms();
    let names: Vec<&str> = arms.iter().map(|arm| arm.name.as_ref()).collect();
    assert_eq!(
        names, expected,
        "the cleanup rule has its six arms in every phase with arms, the \
         terminated-input arm in the phases that do not serve, the \
         terminated-while-serving arm in the phases that serve and the \
         terminated-while-draining arm in Retired; a destroyed runtime has \
         none"
    );
}

#[test]
fn one_terminated_input_keeps_a_serving_runtime_and_cleans_up_one_that_does_not_serve() {
    let mut seen = Vec::new();
    for arm in arms() {
        let name = arm.name.as_ref().to_string();
        let guards = guard_names(&arm);
        if !guards.contains(&"runtime_terminated") {
            continue;
        }
        assert!(
            guards.contains(&"not_archived"),
            "{name}: the terminated-input arms leave an archived session to its own arm"
        );
        // Only the two arms of a retired runtime read the machine's queue
        // state, and each reads one side of it.
        let queue_guard = match (
            guards.contains(&NOTHING_LEFT_TO_DRAIN),
            guards.contains(&WORK_LEFT_TO_DRAIN),
        ) {
            (false, false) => "",
            (true, false) => NOTHING_LEFT_TO_DRAIN,
            (false, true) => WORK_LEFT_TO_DRAIN,
            (true, true) => panic!("{name}: an arm carries both queue guards"),
        };
        for phase in &arm.from {
            seen.push((phase.as_ref().to_string(), queue_guard, action(&arm)));
        }
    }
    seen.sort();
    let mut expected: Vec<(String, &str, &str)> = vec![
        ("Initializing".into(), "", "CleanupRuntime"),
        ("Idle".into(), "", "RetainRuntime"),
        ("Attached".into(), "", "RetainRuntime"),
        ("Running".into(), "", "RetainRuntime"),
        ("Retired".into(), NOTHING_LEFT_TO_DRAIN, "CleanupRuntime"),
        ("Retired".into(), WORK_LEFT_TO_DRAIN, "RetainRuntime"),
        ("Stopped".into(), "", "CleanupRuntime"),
    ];
    expected.sort();
    assert_eq!(
        seen, expected,
        "(phase, queue guard, action) of the terminated-input arms: a \
         terminated input is answered once in every phase with arms, and in \
         Retired by what is left to drain"
    );
}

#[test]
fn every_cleanup_arm_is_bound_to_the_registration_and_writes_nothing() {
    let arms = arms();
    assert!(!arms.is_empty(), "no {INPUT} arm exists");
    for arm in &arms {
        let name = arm.name.as_ref();
        let guards = guard_names(arm);
        for required in [
            "observation_matches_session",
            "observation_matches_runtime_binding",
        ] {
            assert!(
                guards.contains(&required),
                "{name}: every cleanup arm requires {required}, got {guards:?}"
            );
        }
        assert!(
            arm.updates.is_empty(),
            "{name}: the cleanup rule is asked on a fork and must not write state"
        );
        let fields = format!("{:?}", arm.emit[0].fields);
        assert!(
            fields.contains("ReleasePreAdmission"),
            "{name}: every cleanup arm releases the pre-admission, got {fields}"
        );
        // Only the archived arm names the cleanup of an archived session,
        // which also ends what the session owns.
        let expected = if name.starts_with("ResolveRuntimeCompletionCleanupRetain")
            || name.starts_with(TERMINATED_WHILE_SERVING)
            || name.starts_with(TERMINATED_WHILE_DRAINING)
        {
            "RetainRuntime"
        } else if name.starts_with("ResolveRuntimeCompletionCleanupArchived") {
            "CleanupArchivedSession"
        } else {
            "CleanupRuntime"
        };
        assert_eq!(action(arm), expected, "{name}");
    }
}

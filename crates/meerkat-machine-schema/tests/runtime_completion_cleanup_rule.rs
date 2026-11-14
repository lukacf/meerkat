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
//!
//! The outcome the rule is asked about is the one the machine answered for
//! the run. A run whose apply failed is answered where its result is
//! resolved: by `ResolveRuntimeCompletionResult` for the run itself, and by
//! `ResolveAbandonedCompletionResult` for the inputs the failed run
//! abandoned. Each has two arms, split on the apply failure's cause:
//!
//! - `...RuntimeApplyRefused`: the request was refused and nothing was
//!   applied. The outcome is `Abandoned`, which the rule keeps the runtime
//!   for while the live session is present.
//! - `...RuntimeApplyFailed`: every other failed apply. The outcome is
//!   `RuntimeApplyFailed`, which the rule cleans up.
//!
//! The rule itself reads no cause: the split is made once, where the run's
//! outcome is answered.

#![allow(clippy::expect_used, clippy::panic)]

use meerkat_machine_schema::catalog::dsl::dsl_meerkat_machine;
use meerkat_machine_schema::{Expr, TransitionSchema, TriggerMatch};

const INPUT: &str = "ResolveRuntimeCompletionCleanup";
const RESOLVED: &str = "RuntimeCompletionCleanupResolved";

/// One place where the result of a run whose apply failed is answered.
struct FailedApplyResult {
    input: &'static str,
    resolved: &'static str,
    /// The arm that answers every failed apply but a refused request.
    failed: &'static str,
    /// The arm that answers a request refused with nothing applied.
    refused: &'static str,
    /// The guard both arms carry that says the run's apply failed.
    apply_failed: &'static str,
}

/// The result of the failed run itself.
const RUN_RESULT: FailedApplyResult = FailedApplyResult {
    input: "ResolveRuntimeCompletionResult",
    resolved: "RuntimeCompletionResultResolved",
    failed: "ResolveRuntimeCompletionResultRuntimeApplyFailed",
    refused: "ResolveRuntimeCompletionResultRuntimeApplyRefused",
    apply_failed: "machine_runtime_apply_failed",
};
/// The result of the inputs the failed run abandoned.
const ABANDONED_RESULT: FailedApplyResult = FailedApplyResult {
    input: "ResolveAbandonedCompletionResult",
    resolved: "AbandonedCompletionResultResolved",
    failed: "ResolveAbandonedCompletionResultRuntimeApplyFailed",
    refused: "ResolveAbandonedCompletionResultRuntimeApplyRefused",
    apply_failed: "exact_runtime_apply_failure",
};
/// The guard on the cause: of the arm for every other failed apply, and of
/// the arm for a refused request.
const APPLY_NOT_REFUSED: &str = "runtime_apply_not_refused";
const APPLY_WAS_REFUSED: &str = "runtime_apply_refused";
/// The state field that holds the cause of a run's failed apply, and the
/// cause value of a refused request.
const APPLY_FAILURE_CAUSE: &str = "last_runtime_apply_failure_cause";
const REQUEST_REFUSED: &str = "RequestRefused";

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

// ---------------------------------------------------------------------------
// The outcome of a failed apply, which the rule is then asked about
// ---------------------------------------------------------------------------

fn arms_on(input: &str) -> Vec<TransitionSchema> {
    dsl_meerkat_machine()
        .transitions
        .iter()
        .filter(|transition| match &transition.on {
            TriggerMatch::Input { variant, .. } => variant.as_ref() == input,
            TriggerMatch::Signal { .. } => false,
        })
        .cloned()
        .collect()
}

/// The one arm named `name` in `phase`.
fn arm_in(arms: &[TransitionSchema], name: &str, phase: &str) -> TransitionSchema {
    let wanted = format!("{name}{phase}");
    let found: Vec<&TransitionSchema> = arms
        .iter()
        .filter(|arm| arm.name.as_ref() == wanted)
        .collect();
    assert_eq!(found.len(), 1, "exactly one arm {wanted}");
    found[0].clone()
}

fn guard<'a>(arm: &'a TransitionSchema, name: &str) -> &'a Expr {
    let found: Vec<&Expr> = arm
        .guards
        .iter()
        .filter(|guard| guard.name == name)
        .map(|guard| &guard.expr)
        .collect();
    assert_eq!(
        found.len(),
        1,
        "{}: exactly one guard {name}, got {:?}",
        arm.name.as_ref(),
        guard_names(arm)
    );
    found[0]
}

/// Whether `no` is the structural negation of `yes`: `a == b` against
/// `a != b`, and a conjunction against the disjunction of its terms'
/// negations.
fn negates(yes: &Expr, no: &Expr) -> bool {
    match (yes, no) {
        (Expr::Eq(left, right), Expr::Neq(no_left, no_right)) => {
            left == no_left && right == no_right
        }
        (Expr::And(terms), Expr::Or(cases)) => {
            terms.len() == cases.len()
                && terms
                    .iter()
                    .zip(cases)
                    .all(|(term, case)| negates(term, case))
        }
        _ => false,
    }
}

/// The outcome an arm hands to the cleanup rule, read from its one effect.
fn cleanup_outcome(arm: &TransitionSchema, resolved: &str) -> String {
    let name = arm.name.as_ref();
    assert_eq!(
        arm.emit
            .iter()
            .map(|emit| emit.variant.as_ref())
            .collect::<Vec<_>>(),
        vec![resolved],
        "{name}: the arm answers with {resolved} and nothing else"
    );
    let outcome: Vec<String> = arm.emit[0]
        .fields
        .iter()
        .filter(|(field, _)| field.as_str() == "cleanup_outcome")
        .map(|(_, expr)| match expr {
            Expr::NamedVariant { variant, .. } => variant.as_ref().to_string(),
            other => panic!("{name}: the cleanup outcome is a literal, got {other:?}"),
        })
        .collect();
    assert_eq!(outcome.len(), 1, "{name}: one cleanup outcome");
    outcome[0].clone()
}

/// A run whose apply failed is answered, for the run and for the inputs it
/// abandoned, by two arms that differ in one guard and in the outcome they
/// hand to the cleanup rule: a refused request, with nothing applied, is
/// `Abandoned`; every other failed apply is `RuntimeApplyFailed`. The two
/// guards are each other's negation over the stored cause, so exactly one
/// arm answers.
#[test]
fn a_refused_request_is_answered_abandoned_and_every_other_failed_apply_as_a_failed_apply() {
    for result in [RUN_RESULT, ABANDONED_RESULT] {
        let arms = arms_on(result.input);
        for phase in ARM_PHASES {
            let refused = arm_in(&arms, result.refused, phase);
            let failed = arm_in(&arms, result.failed, phase);
            let at = format!("{} in {phase}", result.input);

            let was_refused = guard(&refused, APPLY_WAS_REFUSED);
            let not_refused = guard(&failed, APPLY_NOT_REFUSED);
            assert!(
                negates(was_refused, not_refused),
                "{at}: the guard of the failed arm is the negation of the refused arm's, got \
                 {was_refused:?} and {not_refused:?}"
            );
            let compared = format!("{was_refused:?}");
            assert!(
                compared.contains(APPLY_FAILURE_CAUSE) && compared.contains(REQUEST_REFUSED),
                "{at}: the guards compare {APPLY_FAILURE_CAUSE} with {REQUEST_REFUSED}, got \
                 {compared}"
            );

            // Apart from that guard the two arms ask the same things.
            let mut refused_guards = guard_names(&refused);
            let mut failed_guards = guard_names(&failed);
            refused_guards.retain(|name| *name != APPLY_WAS_REFUSED);
            failed_guards.retain(|name| *name != APPLY_NOT_REFUSED);
            assert_eq!(
                refused_guards, failed_guards,
                "{at}: the other guards of the two arms"
            );
            assert!(
                refused_guards.contains(&result.apply_failed),
                "{at}: both arms answer a run whose apply failed, got {refused_guards:?}"
            );
            assert_eq!(
                (&refused.updates, &refused.to),
                (&failed.updates, &failed.to),
                "{at}: both arms settle the result the same way"
            );

            assert_eq!(
                (
                    cleanup_outcome(&refused, result.resolved),
                    cleanup_outcome(&failed, result.resolved)
                ),
                ("Abandoned".to_string(), "RuntimeApplyFailed".to_string()),
                "{at}: (the outcome of a refused request, the outcome of every other failed \
                 apply)"
            );
            let class = |arm: &TransitionSchema| {
                format!("{:?}", arm.emit[0].fields).contains("AbandonedWithError")
            };
            assert!(
                class(&refused) && class(&failed),
                "{at}: both arms answer the waiter with the failed turn's error"
            );
        }
    }
}

/// The cause is read where a failed apply's result is answered and nowhere
/// else: only those arms read it, and no arm of the cleanup rule does.
#[test]
fn only_the_arms_that_answer_a_failed_apply_read_its_cause() {
    let reads_the_cause = |arm: &TransitionSchema| {
        arm.guards
            .iter()
            .any(|guard| format!("{:?}", guard.expr).contains(APPLY_FAILURE_CAUSE))
    };
    let mut readers: Vec<String> = dsl_meerkat_machine()
        .transitions
        .iter()
        .filter(|arm| reads_the_cause(arm))
        .map(|arm| arm.name.as_ref().to_string())
        .collect();
    readers.sort();
    let mut expected: Vec<String> = [RUN_RESULT, ABANDONED_RESULT]
        .iter()
        .flat_map(|result| [result.failed, result.refused])
        .flat_map(|arm| ARM_PHASES.iter().map(move |phase| format!("{arm}{phase}")))
        .collect();
    expected.sort();
    assert_eq!(
        readers, expected,
        "the arms whose guards read the cause of a failed apply"
    );
    for arm in arms() {
        assert!(
            !reads_the_cause(&arm),
            "{}: the cleanup rule reads the outcome it is asked about, never the cause",
            arm.name.as_ref()
        );
    }
}

/// The cause of a failed apply has a value for a refused request, added
/// after the values it already had.
#[test]
fn the_cause_of_a_failed_apply_names_a_refused_request_last() {
    let schema = dsl_meerkat_machine();
    let causes: Vec<String> = schema
        .named_types
        .iter()
        .filter(|binding| binding.name.as_str() == "RuntimeApplyFailureCause")
        .map(|binding| format!("{:?}", binding.rust))
        .collect();
    assert_eq!(causes.len(), 1, "one binding of the cause's type");
    let values: Vec<&str> = [
        "Unknown",
        "PrimitiveRejected",
        "RuntimeContextApply",
        "RuntimeTurn",
        "HookDenied",
        "HookRuntimeFailure",
        "ExecutorStopped",
        "ExecutorControlFailed",
        "ExecutorInternal",
        REQUEST_REFUSED,
    ]
    .into_iter()
    .filter(|value| causes[0].contains(&format!("\"{value}\"")))
    .collect();
    assert_eq!(
        values.len(),
        10,
        "the cause has its nine values and {REQUEST_REFUSED}, got {}",
        causes[0]
    );
    let position = |value: &str| causes[0].find(&format!("\"{value}\""));
    assert!(
        position("ExecutorInternal") < position(REQUEST_REFUSED),
        "{REQUEST_REFUSED} follows the values the cause already had: {}",
        causes[0]
    );
}

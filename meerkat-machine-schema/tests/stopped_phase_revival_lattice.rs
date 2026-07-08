//! Stopped-phase revival lattice pin — the resume-strand class killer.
//!
//! The 0.7.19–0.7.23 field failures ("guard rejected transition from phase
//! Stopped for input::PublishLocalEndpoint" and its siblings) shared one root
//! cause: `Stopped` was an absorbing phase for resume — the only exit was the
//! teardown arc (`UnregisterSessionStopped`), while resume seams rebound and
//! reused phase-Stopped machines and then fired phase-gated build inputs.
//! Ad-hoc per-input tolerance arms (`HydrateSessionLlmStateStopped`,
//! `PrepareBindingsStopped`, ...) patched one symptom per release.
//!
//! The fix is machine-owned revival: `RegisterSession` and
//! `EnsureSessionWithExecutor` re-admit a stopped session, and the tolerance
//! arms are gone. This test pins the entire Stopped exit lattice: any change
//! to how the machine leaves `Stopped` — an added silent tolerance arm, a
//! removed revival arc — is a deliberate, reviewed edit to the expected set
//! below, never a silent regression.

use std::collections::BTreeMap;

use meerkat_machine_schema::TransitionSchema;
use meerkat_machine_schema::catalog::dsl::dsl_meerkat_machine;

const STOPPED: &str = "Stopped";

/// A transition can fire while the machine phase is `Stopped` iff its
/// (post-`per_phase`-expansion) `from` set admits Stopped AND no
/// `lifecycle_phase` guard excludes it. The catalog constrains phase either
/// through `per_phase` expansion (recorded in `from`) or through
/// `self.lifecycle_phase == Phase::X` guard disjunctions; both are covered.
fn admits_stopped(transition: &TransitionSchema) -> bool {
    let from_admits = transition.from.is_empty()
        || transition
            .from
            .iter()
            .any(|phase| phase.as_ref() == STOPPED);
    if !from_admits {
        return false;
    }
    let mut phase_constrained = false;
    let mut guard_admits = false;
    for guard in &transition.guards {
        let rendered = format!("{:?}", guard.expr);
        if rendered.contains("lifecycle_phase") {
            phase_constrained = true;
            if rendered.contains("Stopped") {
                guard_admits = true;
            }
        }
    }
    !phase_constrained || guard_admits
}

/// The complete set of arcs that leave phase `Stopped`, pinned by name and
/// target phase. Teardown, revival, disposal, destruction — nothing else.
#[test]
fn stopped_phase_exit_lattice_is_exactly_the_pinned_set() {
    let schema = dsl_meerkat_machine();
    let mut exits = BTreeMap::new();
    for transition in &schema.transitions {
        if admits_stopped(transition) && transition.to.as_ref() != STOPPED {
            exits.insert(
                transition.name.as_ref().to_string(),
                transition.to.as_ref().to_string(),
            );
        }
    }

    let expected: BTreeMap<String, String> = [
        // Teardown: fully-drained unregister clears identity and bindings.
        ("UnregisterSessionStopped", "Idle"),
        // Revival: the canonical resume seam re-admits a stopped session,
        // preserving identity and bindings.
        ("RegisterSessionResumesStopped", "Idle"),
        // Totality of RegisterSession over Stopped for a new tenant.
        ("RegisterSessionNewBindingFromStopped", "Idle"),
        // Executor revival: attach intent re-admits and grants the claim.
        ("EnsureSessionWithExecutorStopped", "Attached"),
        // Disposal: retiring a stopped session is a machine transition.
        ("RetireRequestedFromIdle", "Retired"),
        ("RetireRequestedFromIdleUnbound", "Retired"),
        // Destruction.
        ("Destroy", "Destroyed"),
    ]
    .into_iter()
    .map(|(name, to)| (name.to_string(), to.to_string()))
    .collect();

    assert_eq!(
        exits, expected,
        "the Stopped exit lattice changed; every arc out of Stopped is a \
         deliberate lifecycle decision — update this pin only alongside a \
         reviewed machine-authority change"
    );
}

/// The resume intent inputs must each have at least one arm admitting
/// Stopped: without them the phase is absorbing for resume and the strand
/// class returns.
#[test]
fn resume_intent_inputs_are_total_over_stopped() {
    let schema = dsl_meerkat_machine();
    for input in ["RegisterSession", "EnsureSessionWithExecutor", "Retire"] {
        let admitted = schema.transitions.iter().any(|transition| {
            format!("{:?}", transition.on).contains(&format!("\"{input}\""))
                && admits_stopped(transition)
        });
        assert!(
            admitted,
            "input {input} has no arm admitting phase Stopped; the resume \
             seam would wedge stopped sessions permanently"
        );
    }
}

/// The retired per-input tolerance arms must not return: a build input
/// admitted at Stopped hides resume-ordering bugs and re-opens the
/// whack-a-mole. If a genuinely new input must run at Stopped, it belongs
/// in the stop-window management set and in the exit-lattice review above.
#[test]
fn resume_build_inputs_are_not_tolerated_at_stopped() {
    let schema = dsl_meerkat_machine();
    for input in [
        "HydrateSessionLlmState",
        "PrepareBindings",
        "PublishCommittedVisibleSet",
        "SetModelRoutingBaseline",
        "StagePersistentFilter",
        "RequestDeferredTools",
        "SetSilentIntents",
        "PublishLocalEndpoint",
        "ClearLocalEndpoint",
        "AddDirectPeerEndpoint",
        "RemoveDirectPeerEndpoint",
        "ApplyMobPeerOverlay",
    ] {
        let tolerated = schema.transitions.iter().any(|transition| {
            format!("{:?}", transition.on).contains(&format!("\"{input}\""))
                && admits_stopped(transition)
        });
        assert!(
            !tolerated,
            "build input {input} is admitted at phase Stopped; resume must \
             revive first (RegisterSessionResumesStopped), never tolerate \
             per-input Stopped arms"
        );
    }
}

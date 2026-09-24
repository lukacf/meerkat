//! Draining is the machine-owned pre-materialization tombstone.
//!
//! Post-stop service cleanup releases its recovery gate before the final
//! `UnregisterSession` transition. Every input that can authorize a surface to
//! create or attach live resources must therefore reject while registration is
//! `Draining`; otherwise a same-session actor can be recreated in the narrow
//! cleanup-to-finalize window and survive machine teardown.

use meerkat_machine_schema::catalog::dsl::dsl_meerkat_machine;

#[test]
fn every_registration_and_binding_arm_rejects_while_draining() {
    let schema = dsl_meerkat_machine();

    for input in [
        "RegisterSession",
        "PrepareBindings",
        "EnsureSessionWithExecutor",
    ] {
        let transitions = schema
            .transitions
            .iter()
            .filter(|transition| transition.on.variant_str() == input)
            .collect::<Vec<_>>();
        assert!(
            !transitions.is_empty(),
            "expected at least one generated {input} transition"
        );
        for transition in transitions {
            // THE PROPERTY IS "NOTHING REMATERIALIZES DURING TEARDOWN", NOT
            // "EVERY ARM CARRIES not_draining". Those were the same thing until
            // the drain window acquired a TYPED REFUSAL: an arm that fires
            // precisely BECAUSE registration is Draining, in order to reject
            // with a named reason instead of a bare guard rejection. That arm
            // cannot carry `not_draining` - it is the negation of its own
            // trigger - and it rematerializes nothing.
            //
            // Asserting the guard NAME made this test fail when the machine was
            // IMPROVED, which is the worst kind of false red: it fires exactly
            // when someone does the right thing. So both legitimate shapes are
            // named structurally, and anything that is neither still fails.
            let fences_off_draining = transition
                .guards
                .iter()
                .any(|guard| guard.name == "not_draining");
            let refuses_because_draining = transition
                .guards
                .iter()
                .any(|guard| guard.name == "unregister_draining")
                // It must not write state...
                && transition.updates.is_empty()
                // ...and it must publish a typed verdict rather than silently
                // swallowing the request.
                && !transition.emit.is_empty();

            assert!(
                fences_off_draining || refuses_because_draining,
                "{} must either carry the generated not_draining guard, or be a \
                 typed refusal arm (guarded on unregister_draining, no state \
                 updates, emitting a rejection). Neither holds, so a surface \
                 resource can rematerialize during teardown.\n  guards: {:?}\n  \
                 updates: {}\n  emits: {}",
                transition.name,
                transition
                    .guards
                    .iter()
                    .map(|guard| guard.name.as_str())
                    .collect::<Vec<_>>(),
                transition.updates.len(),
                transition.emit.len()
            );
        }
    }
}

#[test]
fn live_open_admission_has_an_explicit_draining_fence() {
    let schema = dsl_meerkat_machine();

    for prefix in [
        "ResolveLiveOpenAdmissionAccepted",
        "ResolveLiveOpenAdmissionSessionAlreadyBound",
        "ResolveLiveOpenAdmissionChannelAlreadyBound",
    ] {
        let transitions = schema
            .transitions
            .iter()
            .filter(|transition| transition.name.as_str().starts_with(prefix))
            .collect::<Vec<_>>();
        assert!(
            !transitions.is_empty(),
            "missing {prefix} transition family"
        );
        for transition in transitions {
            assert!(
                transition
                    .guards
                    .iter()
                    .any(|guard| guard.name == "registration_not_draining"),
                "{} could admit live/open after BeginUnregisterSession",
                transition.name
            );
            assert!(
                transition
                    .guards
                    .iter()
                    .any(|guard| guard.name == "runtime_not_stopping"),
                "{} could admit live/open after a deferred stop request",
                transition.name
            );
            assert!(
                transition
                    .guards
                    .iter()
                    .any(|guard| guard.name == "session_matches_current"),
                "{} could admit live/open against stale session authority",
                transition.name
            );
        }
    }

    let draining_rejections = schema
        .transitions
        .iter()
        .filter(|transition| {
            transition
                .name
                .as_str()
                .starts_with("ResolveLiveOpenAdmissionDraining")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        draining_rejections.len(),
        3,
        "Draining live/open must resolve explicitly in Idle, Attached, and Running"
    );
    for transition in draining_rejections {
        assert!(
            transition
                .guards
                .iter()
                .any(|guard| guard.name == "registration_draining"),
            "{} is not restricted to the unregister drain window",
            transition.name
        );
    }

    let deferred_stop_rejections = schema
        .transitions
        .iter()
        .filter(|transition| {
            transition
                .name
                .as_str()
                .starts_with("ResolveLiveOpenAdmissionStopDeferred")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        deferred_stop_rejections.len(),
        2,
        "deferred-stop live/open must resolve explicitly in Attached and Running"
    );
    for transition in deferred_stop_rejections {
        assert!(
            transition
                .guards
                .iter()
                .any(|guard| guard.name == "registration_not_draining"),
            "{} must remain disjoint from the Draining rejection family",
            transition.name
        );
        assert!(
            transition
                .guards
                .iter()
                .any(|guard| guard.name == "runtime_stop_deferred"),
            "{} is not restricted to the deferred-stop window",
            transition.name
        );
    }
}

/// Public live bookkeeping is orthogonal to session execution lifecycle.
/// Every generated arm must therefore be a per-phase self-loop: Attached and
/// Running cannot collapse to Idle, and terminal cleanup cannot revive a
/// Retired or Stopped session.
#[test]
fn public_live_authority_transitions_preserve_lifecycle_phase() {
    let schema = dsl_meerkat_machine();

    for input in [
        "ResolveLiveOpenAdmission",
        "AbandonLiveOpenAdmission",
        "RecordLiveRefreshQueued",
        "RecordLiveCloseClosed",
        "RecordLiveCommandAccepted",
        "RecordLiveCommandRejected",
        "RecordLiveChannelRequestRejected",
        "RecordLiveChannelStatus",
        "RecordLiveWebrtcTokenIssued",
        "ResolveLiveWebrtcAnswerAdmission",
        "RecordLiveWebrtcAnswerAccepted",
        "RecordLiveWebsocketTokenIssued",
        "ResolveLiveWebsocketTokenAdmission",
    ] {
        let transitions = schema
            .transitions
            .iter()
            .filter(|transition| transition.on.variant_str() == input)
            .collect::<Vec<_>>();
        assert!(
            !transitions.is_empty(),
            "expected generated live authority for {input}"
        );
        for transition in transitions {
            assert_eq!(
                transition.from.len(),
                1,
                "{} must be expanded into one lifecycle-specific arm",
                transition.name
            );
            assert_eq!(
                transition.from[0], transition.to,
                "{} must preserve lifecycle phase for {input}",
                transition.name
            );
        }
    }
}

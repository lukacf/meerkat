//! Focused behavioural tests for the canonical `ForkedParticipantLifecycleMachine`.
//!
//! The machine is record-scoped: every test drives ONE capability record. There
//! is no registry, no map keyed by capability id, and no clock or credential
//! read inside the machine — authentication and expiry arrive as explicit
//! observations on the inputs.
//!
//! The centrepiece is
//! [`every_command_class_in_every_phase_is_total_and_only_mutates_where_intended`],
//! which drives every command class against every phase and asserts either the
//! one intended lifecycle transition or byte-identical state preservation.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::BTreeSet;

use meerkat_machine_schema::catalog::dsl::forked_participant_lifecycle::{
    ForkedParticipantAttachDenial, ForkedParticipantCleanupRejection,
    ForkedParticipantCleanupState, ForkedParticipantExpiryIgnore, ForkedParticipantLifecycleEffect,
    ForkedParticipantLifecycleInput, ForkedParticipantLifecycleMachineAuthority,
    ForkedParticipantLifecycleMachineMutator, ForkedParticipantLifecycleMachineState,
    ForkedParticipantLifecycleMachineTransition, ForkedParticipantLifecycleMachineTransitionError,
    ForkedParticipantLifecycleState, ForkedParticipantReleaseRejection,
    ForkedParticipantReservationRejection, ForkedParticipantRevocationDenial,
};

type Authority = ForkedParticipantLifecycleMachineAuthority;
type Input = ForkedParticipantLifecycleInput;
type Effect = ForkedParticipantLifecycleEffect;
type Phase = ForkedParticipantLifecycleState;
type State = ForkedParticipantLifecycleMachineState;
type Transition = ForkedParticipantLifecycleMachineTransition;
type TransitionError = ForkedParticipantLifecycleMachineTransitionError;

const REQUEST: &str = "fingerprint:source-member/rev-42";
const OTHER_REQUEST: &str = "fingerprint:other-member/rev-7";
const FORK: &str = "fork-activation:branch-1";
const OTHER_FORK: &str = "fork-activation:branch-2";
const ATTACHMENT_A: &str = "attachment-a";
const ATTACHMENT_B: &str = "attachment-b";
const ATTACHMENT_C: &str = "attachment-c";
const BUDGET: u64 = 2;

fn apply(authority: &mut Authority, input: Input) -> Transition {
    ForkedParticipantLifecycleMachineMutator::apply(authority, input)
        .expect("every command class must resolve to an explicit typed transition")
}

fn reserve(fingerprint: &str, max_uses: u64) -> Input {
    Input::Reserve {
        request_fingerprint: fingerprint.to_owned(),
        max_uses,
    }
}

fn record_activation(fingerprint: &str, fork: &str) -> Input {
    Input::RecordForkActivation {
        request_fingerprint: fingerprint.to_owned(),
        fork_activation_id: fork.to_owned(),
    }
}

fn record_activation_failure(fingerprint: &str) -> Input {
    Input::RecordForkActivationFailure {
        request_fingerprint: fingerprint.to_owned(),
    }
}

fn attach(attachment_id: &str, authentication_valid: bool, expired: bool) -> Input {
    Input::Attach {
        attachment_id: attachment_id.to_owned(),
        authentication_valid,
        expired,
    }
}

fn release(attachment_id: &str) -> Input {
    Input::Release {
        attachment_id: attachment_id.to_owned(),
    }
}

fn revoke(authentication_valid: bool) -> Input {
    Input::Revoke {
        authentication_valid,
    }
}

fn observe_expiry(expired: bool) -> Input {
    Input::ObserveExpiry { expired }
}

fn complete_cleanup() -> Input {
    Input::CompleteCleanup {}
}

fn granted(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|id| (*id).to_owned()).collect()
}

/// Reserved + activated capability with the given reuse budget.
fn active_capability(max_uses: u64) -> Authority {
    let mut authority = Authority::new();
    apply(&mut authority, reserve(REQUEST, max_uses));
    apply(&mut authority, record_activation(REQUEST, FORK));
    assert_eq!(authority.state().lifecycle_phase, Phase::Active);
    authority
}

fn attached_capability(max_uses: u64, attachment_id: &str) -> Authority {
    let mut authority = active_capability(max_uses);
    apply(&mut authority, attach(attachment_id, true, false));
    assert_eq!(authority.state().lifecycle_phase, Phase::Attached);
    authority
}

/// One authority per canonical phase, all built from the same request identity
/// and reuse budget so the exhaustive matrix compares like with like.
fn fixture(phase: Phase) -> Authority {
    match phase {
        Phase::Empty => Authority::new(),
        Phase::Reserved => {
            let mut authority = Authority::new();
            apply(&mut authority, reserve(REQUEST, BUDGET));
            authority
        }
        Phase::ActivationFailed => {
            let mut authority = Authority::new();
            apply(&mut authority, reserve(REQUEST, BUDGET));
            apply(&mut authority, record_activation_failure(REQUEST));
            authority
        }
        Phase::Active => active_capability(BUDGET),
        Phase::Attached => attached_capability(BUDGET, ATTACHMENT_A),
        Phase::RevocationPendingAttached => {
            let mut authority = attached_capability(BUDGET, ATTACHMENT_A);
            apply(&mut authority, revoke(true));
            authority
        }
        Phase::ExpiryPendingAttached => {
            let mut authority = attached_capability(BUDGET, ATTACHMENT_A);
            apply(&mut authority, observe_expiry(true));
            authority
        }
        Phase::Revoked => {
            let mut authority = active_capability(BUDGET);
            apply(&mut authority, revoke(true));
            authority
        }
        Phase::Expired => {
            let mut authority = active_capability(BUDGET);
            apply(&mut authority, observe_expiry(true));
            authority
        }
        Phase::Exhausted => {
            let mut authority = attached_capability(BUDGET, ATTACHMENT_A);
            apply(&mut authority, release(ATTACHMENT_A));
            apply(&mut authority, attach(ATTACHMENT_B, true, false));
            apply(&mut authority, release(ATTACHMENT_B));
            authority
        }
    }
}

const ALL_PHASES: [Phase; 10] = [
    Phase::Empty,
    Phase::Reserved,
    Phase::ActivationFailed,
    Phase::Active,
    Phase::Attached,
    Phase::RevocationPendingAttached,
    Phase::ExpiryPendingAttached,
    Phase::Revoked,
    Phase::Expired,
    Phase::Exhausted,
];

fn effects(transition: &Transition) -> Vec<Effect> {
    transition.effects().to_vec()
}

fn single_effect(transition: &Transition) -> Effect {
    let effects = effects(transition);
    assert_eq!(
        effects.len(),
        1,
        "expected exactly one effect, got {effects:?}"
    );
    effects.into_iter().next().expect("one effect")
}

// ---------------------------------------------------------------------------
// Exhaustive command-class x phase matrix
// ---------------------------------------------------------------------------

/// What one command class is allowed to do from one phase.
#[derive(Debug)]
enum Expect {
    /// The one intended lifecycle transition to another phase.
    MovesTo(Phase),
    /// Stays in its phase but mutates a non-phase semantic field (only cleanup
    /// completion does this).
    MutatesInPlace,
    /// Byte-identical state: the arm is a typed rejection, denial, or replay.
    PreservesState,
}

fn clone_input(input: &Input) -> Input {
    match input {
        Input::Reserve {
            request_fingerprint,
            max_uses,
        } => reserve(request_fingerprint, *max_uses),
        Input::RecordForkActivation {
            request_fingerprint,
            fork_activation_id,
        } => record_activation(request_fingerprint, fork_activation_id),
        Input::RecordForkActivationFailure {
            request_fingerprint,
        } => record_activation_failure(request_fingerprint),
        Input::Attach {
            attachment_id,
            authentication_valid,
            expired,
        } => attach(attachment_id, *authentication_valid, *expired),
        Input::Release { attachment_id } => release(attachment_id),
        Input::Revoke {
            authentication_valid,
        } => revoke(*authentication_valid),
        Input::ObserveExpiry { expired } => observe_expiry(*expired),
        Input::CompleteCleanup {} => complete_cleanup(),
    }
}

/// Every command class the machine accepts, paired with the complete list of
/// phases it is allowed to move (or mutate) from. Any (phase, command) pair not
/// listed MUST preserve state exactly.
fn command_matrix() -> Vec<(&'static str, Input, Vec<(Phase, Expect)>)> {
    vec![
        (
            "reserve_exact",
            reserve(REQUEST, BUDGET),
            vec![
                (Phase::Empty, Expect::MovesTo(Phase::Reserved)),
                (Phase::ActivationFailed, Expect::MovesTo(Phase::Reserved)),
            ],
        ),
        (
            "reserve_conflicting",
            reserve(OTHER_REQUEST, 3),
            // An empty record is unbound, so any well-formed request may take
            // it; "conflicting" only means conflicting with an existing binding.
            vec![(Phase::Empty, Expect::MovesTo(Phase::Reserved))],
        ),
        ("reserve_malformed_fingerprint", reserve("", BUDGET), vec![]),
        ("reserve_malformed_budget", reserve(REQUEST, 0), vec![]),
        (
            "activation_exact",
            record_activation(REQUEST, FORK),
            vec![
                (Phase::Reserved, Expect::MovesTo(Phase::Active)),
                (Phase::ActivationFailed, Expect::MovesTo(Phase::Active)),
            ],
        ),
        (
            "activation_conflicting",
            record_activation(OTHER_REQUEST, OTHER_FORK),
            vec![],
        ),
        (
            "activation_malformed",
            record_activation(REQUEST, ""),
            vec![],
        ),
        (
            "activation_failure_exact",
            record_activation_failure(REQUEST),
            vec![(Phase::Reserved, Expect::MovesTo(Phase::ActivationFailed))],
        ),
        (
            "activation_failure_foreign",
            record_activation_failure(OTHER_REQUEST),
            vec![],
        ),
        (
            "attach_authentication_invalid",
            attach(ATTACHMENT_C, false, false),
            vec![],
        ),
        (
            "attach_fresh_identity",
            attach(ATTACHMENT_C, true, false),
            vec![(Phase::Active, Expect::MovesTo(Phase::Attached))],
        ),
        (
            "attach_known_identity",
            attach(ATTACHMENT_A, true, false),
            vec![(Phase::Active, Expect::MovesTo(Phase::Attached))],
        ),
        ("attach_malformed", attach("", true, false), vec![]),
        (
            "attach_with_expiry_observed",
            attach(ATTACHMENT_C, true, true),
            vec![(Phase::Active, Expect::MovesTo(Phase::Expired))],
        ),
        (
            "release_held_identity",
            release(ATTACHMENT_A),
            vec![
                (Phase::Attached, Expect::MovesTo(Phase::Active)),
                (
                    Phase::RevocationPendingAttached,
                    Expect::MovesTo(Phase::Revoked),
                ),
                (
                    Phase::ExpiryPendingAttached,
                    Expect::MovesTo(Phase::Expired),
                ),
            ],
        ),
        ("release_unknown_identity", release("attachment-z"), vec![]),
        ("revoke_authentication_invalid", revoke(false), vec![]),
        (
            "revoke_authenticated",
            revoke(true),
            vec![
                (Phase::Reserved, Expect::MovesTo(Phase::Revoked)),
                (Phase::ActivationFailed, Expect::MovesTo(Phase::Revoked)),
                (Phase::Active, Expect::MovesTo(Phase::Revoked)),
                (
                    Phase::Attached,
                    Expect::MovesTo(Phase::RevocationPendingAttached),
                ),
                (
                    Phase::ExpiryPendingAttached,
                    Expect::MovesTo(Phase::RevocationPendingAttached),
                ),
            ],
        ),
        ("expiry_not_observed", observe_expiry(false), vec![]),
        (
            "expiry_observed",
            observe_expiry(true),
            vec![
                (Phase::Reserved, Expect::MovesTo(Phase::Expired)),
                (Phase::ActivationFailed, Expect::MovesTo(Phase::Expired)),
                (Phase::Active, Expect::MovesTo(Phase::Expired)),
                (
                    Phase::Attached,
                    Expect::MovesTo(Phase::ExpiryPendingAttached),
                ),
            ],
        ),
        (
            "complete_cleanup",
            complete_cleanup(),
            vec![
                (Phase::Revoked, Expect::MutatesInPlace),
                (Phase::Expired, Expect::MutatesInPlace),
                (Phase::Exhausted, Expect::MutatesInPlace),
            ],
        ),
    ]
}

#[test]
fn every_command_class_in_every_phase_is_total_and_only_mutates_where_intended() {
    let matrix = command_matrix();
    assert_eq!(
        matrix.len(),
        21,
        "the matrix must cover every command class"
    );

    let mut checked = 0_usize;
    for (label, input, expectations) in &matrix {
        for phase in ALL_PHASES {
            let mut authority = fixture(phase);
            let before = authority.state().clone();
            assert_eq!(
                before.lifecycle_phase, phase,
                "fixture for {phase:?} must start in its own phase"
            );

            let transition =
                ForkedParticipantLifecycleMachineMutator::apply(&mut authority, clone_input(input))
                    .unwrap_or_else(|error| {
                        panic!(
                            "`{label}` in {phase:?} must be an explicit typed command class: \
                             {error:?}"
                        )
                    });
            assert!(
                !transition.effects().is_empty(),
                "`{label}` in {phase:?} must emit a typed verdict"
            );
            assert_eq!(
                transition.from_phase, phase,
                "`{label}` in {phase:?} matched a transition from the wrong phase"
            );

            let expectation = expectations
                .iter()
                .find(|(expected_phase, _)| *expected_phase == phase)
                .map_or(&Expect::PreservesState, |(_, expectation)| expectation);
            let after = authority.state().clone();

            match expectation {
                Expect::MovesTo(target) => {
                    assert_eq!(
                        after.lifecycle_phase, *target,
                        "`{label}` in {phase:?} must move to {target:?}"
                    );
                    assert_eq!(
                        transition.to_phase, *target,
                        "`{label}` in {phase:?} declared the wrong target phase"
                    );
                }
                Expect::MutatesInPlace => {
                    assert_eq!(
                        after.lifecycle_phase, phase,
                        "`{label}` in {phase:?} must not change phase"
                    );
                    assert_ne!(
                        after, before,
                        "`{label}` in {phase:?} was expected to mutate a semantic field"
                    );
                }
                Expect::PreservesState => {
                    assert_eq!(
                        after, before,
                        "`{label}` in {phase:?} must preserve state exactly (typed \
                         rejection/denial/replay arms never mutate)"
                    );
                    assert_eq!(
                        transition.to_phase, phase,
                        "`{label}` in {phase:?} declared a phase change on a no-op arm"
                    );
                }
            }
            checked += 1;
        }
    }
    assert_eq!(
        checked,
        matrix.len() * ALL_PHASES.len(),
        "the matrix must cover every command class in every phase"
    );
}

#[test]
fn a_terminal_capability_never_reopens_and_never_holds_an_attachment() {
    for phase in [Phase::Revoked, Phase::Expired, Phase::Exhausted] {
        for (label, input, _) in &command_matrix() {
            let mut authority = fixture(phase);
            apply(&mut authority, clone_input(input));
            assert_eq!(
                authority.state().lifecycle_phase,
                phase,
                "`{label}` reopened terminal phase {phase:?}"
            );
            assert_eq!(
                authority.state().active_attachment_id,
                None,
                "`{label}` gave terminal phase {phase:?} an attachment"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Reservation identity
// ---------------------------------------------------------------------------

#[test]
fn empty_record_reserves_on_fingerprint_and_positive_budget() {
    let mut authority = Authority::new();
    let transition = apply(&mut authority, reserve(REQUEST, 3));

    assert_eq!(transition.from_phase, Phase::Empty);
    assert_eq!(transition.to_phase, Phase::Reserved);
    assert_eq!(
        single_effect(&transition),
        Effect::CapabilityReserved {
            request_fingerprint: REQUEST.to_owned(),
            max_uses: 3,
        }
    );
    assert_eq!(authority.state().max_uses, 3);
    assert_eq!(authority.state().use_count, 0);
    assert!(authority.state().granted_attachment_ids.is_empty());
}

#[test]
fn reserve_rejects_malformed_request_without_binding_identity() {
    for input in [reserve("", 3), reserve(REQUEST, 0)] {
        let mut authority = Authority::new();
        let transition = apply(&mut authority, input);

        assert_eq!(transition.to_phase, Phase::Empty);
        assert_eq!(
            single_effect(&transition),
            Effect::ReservationRejected {
                reason: ForkedParticipantReservationRejection::MalformedRequest,
            }
        );
        assert_eq!(authority.state(), &State::default());
    }
}

#[test]
fn exact_reserve_replay_is_typed_and_conflicting_fingerprint_is_typed_reject() {
    let mut authority = Authority::new();
    apply(&mut authority, reserve(REQUEST, 2));
    let before = authority.state().clone();

    let replay = apply(&mut authority, reserve(REQUEST, 2));
    assert_eq!(
        single_effect(&replay),
        Effect::ReservationReplayed {
            request_fingerprint: REQUEST.to_owned(),
        }
    );
    assert_eq!(authority.state(), &before);

    for conflicting in [reserve(OTHER_REQUEST, 2), reserve(REQUEST, 5)] {
        let rejected = apply(&mut authority, conflicting);
        assert_eq!(
            single_effect(&rejected),
            Effect::ReservationRejected {
                reason: ForkedParticipantReservationRejection::FingerprintConflict,
            }
        );
        assert_eq!(authority.state(), &before);
    }
}

#[test]
fn reserve_after_activation_is_typed_already_provisioned() {
    let mut authority = active_capability(1);
    let before = authority.state().clone();

    let transition = apply(&mut authority, reserve(REQUEST, 1));
    assert_eq!(
        single_effect(&transition),
        Effect::ReservationRejected {
            reason: ForkedParticipantReservationRejection::AlreadyProvisioned,
        }
    );
    assert_eq!(authority.state(), &before);
}

// ---------------------------------------------------------------------------
// Durable fork activation identity
// ---------------------------------------------------------------------------

#[test]
fn exact_durable_activation_moves_reserved_to_active_and_replay_converges() {
    let mut authority = Authority::new();
    apply(&mut authority, reserve(REQUEST, 1));

    let activated = apply(&mut authority, record_activation(REQUEST, FORK));
    assert_eq!(activated.to_phase, Phase::Active);
    assert_eq!(
        single_effect(&activated),
        Effect::ForkActivated {
            fork_activation_id: FORK.to_owned(),
        }
    );

    let before = authority.state().clone();
    let replay = apply(&mut authority, record_activation(REQUEST, FORK));
    assert_eq!(replay.to_phase, Phase::Active);
    assert_eq!(
        single_effect(&replay),
        Effect::ForkActivationReplayed {
            fork_activation_id: FORK.to_owned(),
        }
    );
    assert_eq!(authority.state(), &before);
}

#[test]
fn conflicting_activation_is_typed_and_never_rebinds_the_fork() {
    let mut authority = active_capability(1);
    let before = authority.state().clone();

    for conflicting in [
        record_activation(REQUEST, OTHER_FORK),
        record_activation(OTHER_REQUEST, FORK),
    ] {
        let rejected = apply(&mut authority, conflicting);
        assert!(matches!(
            single_effect(&rejected),
            Effect::ActivationRejected { .. }
        ));
        assert_eq!(authority.state(), &before);
    }
}

#[test]
fn activation_rejects_a_foreign_request_and_a_malformed_activation_while_reserved() {
    let mut authority = Authority::new();
    apply(&mut authority, reserve(REQUEST, 1));
    let before = authority.state().clone();

    let foreign = apply(&mut authority, record_activation(OTHER_REQUEST, FORK));
    assert!(matches!(
        single_effect(&foreign),
        Effect::ActivationRejected { .. }
    ));
    assert_eq!(authority.state(), &before);

    let malformed = apply(&mut authority, record_activation(REQUEST, ""));
    assert!(matches!(
        single_effect(&malformed),
        Effect::ActivationRejected { .. }
    ));
    assert_eq!(authority.state(), &before);
}

#[test]
fn create_failure_keeps_the_same_request_retryable_without_letting_another_steal_it() {
    let mut authority = Authority::new();
    apply(&mut authority, reserve(REQUEST, 1));

    let failed = apply(&mut authority, record_activation_failure(REQUEST));
    assert_eq!(failed.to_phase, Phase::ActivationFailed);
    assert_eq!(
        single_effect(&failed),
        Effect::ForkActivationFailed {
            request_fingerprint: REQUEST.to_owned(),
        }
    );

    let before = authority.state().clone();
    let replay = apply(&mut authority, record_activation_failure(REQUEST));
    assert_eq!(
        single_effect(&replay),
        Effect::ForkActivationFailureReplayed {
            request_fingerprint: REQUEST.to_owned(),
        }
    );
    assert_eq!(authority.state(), &before);

    let stolen = apply(&mut authority, reserve(OTHER_REQUEST, 1));
    assert_eq!(
        single_effect(&stolen),
        Effect::ReservationRejected {
            reason: ForkedParticipantReservationRejection::FingerprintConflict,
        }
    );
    assert_eq!(authority.state(), &before);

    let retried = apply(&mut authority, reserve(REQUEST, 1));
    assert_eq!(retried.to_phase, Phase::Reserved);
    assert_eq!(
        single_effect(&retried),
        Effect::CapabilityReserved {
            request_fingerprint: REQUEST.to_owned(),
            max_uses: 1,
        }
    );
}

#[test]
fn a_late_activation_for_the_same_request_resolves_the_failed_state() {
    let mut authority = Authority::new();
    apply(&mut authority, reserve(REQUEST, 1));
    apply(&mut authority, record_activation_failure(REQUEST));

    let recovered = apply(&mut authority, record_activation(REQUEST, FORK));
    assert_eq!(recovered.to_phase, Phase::Active);
    assert_eq!(
        single_effect(&recovered),
        Effect::ForkActivated {
            fork_activation_id: FORK.to_owned(),
        }
    );
    assert_eq!(authority.state().fork_activation_id, FORK);
}

// ---------------------------------------------------------------------------
// Bounded, single-holder attachment
// ---------------------------------------------------------------------------

#[test]
fn invalid_authentication_denies_attach_without_touching_state() {
    let mut authority = active_capability(2);
    let before = authority.state().clone();

    let denied = apply(&mut authority, attach(ATTACHMENT_A, false, false));
    assert_eq!(
        single_effect(&denied),
        Effect::AttachDenied {
            attachment_id: ATTACHMENT_A.to_owned(),
            reason: ForkedParticipantAttachDenial::AuthenticationInvalid,
        }
    );
    assert_eq!(authority.state(), &before);
}

#[test]
fn attach_admits_one_attachment_and_increments_the_use_count_exactly_once() {
    let mut authority = active_capability(2);

    let granted_transition = apply(&mut authority, attach(ATTACHMENT_A, true, false));
    assert_eq!(granted_transition.to_phase, Phase::Attached);
    assert_eq!(
        single_effect(&granted_transition),
        Effect::AttachmentGranted {
            attachment_id: ATTACHMENT_A.to_owned(),
            use_index: 1,
            remaining_uses: 1,
        }
    );
    assert_eq!(authority.state().use_count, 1);
    assert_eq!(
        authority.state().active_attachment_id.as_deref(),
        Some(ATTACHMENT_A)
    );
    assert_eq!(
        authority.state().granted_attachment_ids,
        granted(&[ATTACHMENT_A])
    );
}

#[test]
fn exact_attach_replay_returns_the_original_grant_without_incrementing() {
    let mut authority = attached_capability(3, ATTACHMENT_A);
    let before = authority.state().clone();

    let replay = apply(&mut authority, attach(ATTACHMENT_A, true, false));
    assert_eq!(
        single_effect(&replay),
        Effect::AttachmentGrantReplayed {
            attachment_id: ATTACHMENT_A.to_owned(),
            use_index: 1,
        }
    );
    assert_eq!(authority.state(), &before);
    assert_eq!(authority.state().use_count, 1);
}

#[test]
fn a_different_concurrent_attachment_is_typed_busy() {
    let mut authority = attached_capability(3, ATTACHMENT_A);
    let before = authority.state().clone();

    let denied = apply(&mut authority, attach(ATTACHMENT_B, true, false));
    assert_eq!(
        single_effect(&denied),
        Effect::AttachDenied {
            attachment_id: ATTACHMENT_B.to_owned(),
            reason: ForkedParticipantAttachDenial::Busy,
        }
    );
    assert_eq!(authority.state(), &before);
}

/// Exact dedup over the whole capability lifetime: an intervening attachment
/// must not make an older identity replayable as fresh work.
#[test]
fn an_older_granted_identity_can_never_consume_a_second_use_after_an_intervening_attach() {
    let mut authority = active_capability(3);

    apply(&mut authority, attach(ATTACHMENT_A, true, false));
    apply(&mut authority, release(ATTACHMENT_A));
    apply(&mut authority, attach(ATTACHMENT_B, true, false));
    apply(&mut authority, release(ATTACHMENT_B));
    assert_eq!(authority.state().use_count, 2);
    assert_eq!(
        authority.state().granted_attachment_ids,
        granted(&[ATTACHMENT_A, ATTACHMENT_B])
    );

    let before = authority.state().clone();

    // Replaying A (the OLDER identity) is refused, not granted.
    let replayed_a = apply(&mut authority, attach(ATTACHMENT_A, true, false));
    assert_eq!(
        single_effect(&replayed_a),
        Effect::AttachDenied {
            attachment_id: ATTACHMENT_A.to_owned(),
            reason: ForkedParticipantAttachDenial::AttachmentAlreadyReleased,
        }
    );
    assert_eq!(authority.state(), &before);

    // Replaying B (the most recent identity) is refused identically.
    let replayed_b = apply(&mut authority, attach(ATTACHMENT_B, true, false));
    assert_eq!(
        single_effect(&replayed_b),
        Effect::AttachDenied {
            attachment_id: ATTACHMENT_B.to_owned(),
            reason: ForkedParticipantAttachDenial::AttachmentAlreadyReleased,
        }
    );
    assert_eq!(authority.state(), &before);

    // A duplicate release of the older identity still converges.
    let duplicate_release = apply(&mut authority, release(ATTACHMENT_A));
    assert_eq!(
        single_effect(&duplicate_release),
        Effect::ReleaseReplayed {
            attachment_id: ATTACHMENT_A.to_owned(),
        }
    );
    assert_eq!(authority.state(), &before);

    // The budget still has room, and only a genuinely fresh identity uses it.
    let fresh = apply(&mut authority, attach(ATTACHMENT_C, true, false));
    assert_eq!(
        single_effect(&fresh),
        Effect::AttachmentGranted {
            attachment_id: ATTACHMENT_C.to_owned(),
            use_index: 3,
            remaining_uses: 0,
        }
    );
}

#[test]
fn an_already_granted_identity_is_refused_even_while_another_attachment_holds_the_capability() {
    let mut authority = active_capability(3);
    apply(&mut authority, attach(ATTACHMENT_A, true, false));
    apply(&mut authority, release(ATTACHMENT_A));
    apply(&mut authority, attach(ATTACHMENT_B, true, false));
    let before = authority.state().clone();

    let denied = apply(&mut authority, attach(ATTACHMENT_A, true, false));
    assert_eq!(
        single_effect(&denied),
        Effect::AttachDenied {
            attachment_id: ATTACHMENT_A.to_owned(),
            reason: ForkedParticipantAttachDenial::AttachmentAlreadyReleased,
        }
    );
    assert_eq!(authority.state(), &before);
}

#[test]
fn a_malformed_attachment_identity_is_typed_and_consumes_no_use() {
    let mut authority = active_capability(1);
    let before = authority.state().clone();

    let denied = apply(&mut authority, attach("", true, false));
    assert_eq!(
        single_effect(&denied),
        Effect::AttachDenied {
            attachment_id: String::new(),
            reason: ForkedParticipantAttachDenial::MalformedAttachment,
        }
    );
    assert_eq!(authority.state(), &before);
}

#[test]
fn expired_before_attach_terminalizes_to_expired_with_cleanup_debt() {
    let mut authority = active_capability(2);

    let transition = apply(&mut authority, attach(ATTACHMENT_A, true, true));
    assert_eq!(transition.to_phase, Phase::Expired);
    assert_eq!(
        effects(&transition),
        vec![
            Effect::CapabilityExpired {
                cleanup_pending: true
            },
            Effect::AttachDenied {
                attachment_id: ATTACHMENT_A.to_owned(),
                reason: ForkedParticipantAttachDenial::Expired,
            },
        ]
    );
    assert_eq!(
        authority.state().cleanup_state,
        ForkedParticipantCleanupState::Pending
    );
    assert_eq!(authority.state().use_count, 0);
    assert!(authority.state().granted_attachment_ids.is_empty());
}

#[test]
fn a_one_shot_capability_exhausts_after_its_single_release() {
    let mut authority = attached_capability(1, ATTACHMENT_A);

    let released = apply(&mut authority, release(ATTACHMENT_A));
    assert_eq!(released.to_phase, Phase::Exhausted);
    assert_eq!(
        effects(&released),
        vec![
            Effect::AttachmentReleased {
                attachment_id: ATTACHMENT_A.to_owned(),
                use_count: 1,
            },
            Effect::CapabilityExhausted { use_count: 1 },
        ]
    );
    assert_eq!(
        authority.state().cleanup_state,
        ForkedParticipantCleanupState::Pending
    );
    assert_eq!(authority.state().active_attachment_id, None);

    let denied = apply(&mut authority, attach(ATTACHMENT_B, true, false));
    assert_eq!(
        single_effect(&denied),
        Effect::AttachDenied {
            attachment_id: ATTACHMENT_B.to_owned(),
            reason: ForkedParticipantAttachDenial::Exhausted,
        }
    );
}

#[test]
fn bounded_reuse_admits_exactly_max_uses_grants() {
    let mut authority = active_capability(2);

    let first = apply(&mut authority, attach(ATTACHMENT_A, true, false));
    assert_eq!(
        single_effect(&first),
        Effect::AttachmentGranted {
            attachment_id: ATTACHMENT_A.to_owned(),
            use_index: 1,
            remaining_uses: 1,
        }
    );
    let released = apply(&mut authority, release(ATTACHMENT_A));
    assert_eq!(released.to_phase, Phase::Active);

    let second = apply(&mut authority, attach(ATTACHMENT_B, true, false));
    assert_eq!(
        single_effect(&second),
        Effect::AttachmentGranted {
            attachment_id: ATTACHMENT_B.to_owned(),
            use_index: 2,
            remaining_uses: 0,
        }
    );

    let exhausting = apply(&mut authority, release(ATTACHMENT_B));
    assert_eq!(exhausting.to_phase, Phase::Exhausted);
    assert_eq!(authority.state().use_count, 2);
    assert_eq!(authority.state().max_uses, 2);
    assert_eq!(
        authority.state().granted_attachment_ids,
        granted(&[ATTACHMENT_A, ATTACHMENT_B])
    );
}

#[test]
fn a_recovered_active_record_whose_budget_is_spent_denies_a_fresh_attach() {
    let spent = State {
        lifecycle_phase: Phase::Active,
        request_fingerprint: REQUEST.to_owned(),
        max_uses: 1,
        use_count: 1,
        fork_activation_id: FORK.to_owned(),
        active_attachment_id: None,
        granted_attachment_ids: granted(&[ATTACHMENT_A]),
        cleanup_state: ForkedParticipantCleanupState::NotRequired,
    };
    let mut authority =
        Authority::recover_from_state(spent).expect("a spent-but-active record is well formed");

    let denied = apply(&mut authority, attach(ATTACHMENT_B, true, false));
    assert_eq!(
        single_effect(&denied),
        Effect::AttachDenied {
            attachment_id: ATTACHMENT_B.to_owned(),
            reason: ForkedParticipantAttachDenial::Exhausted,
        }
    );
    assert_eq!(authority.state().use_count, 1);
}

// ---------------------------------------------------------------------------
// Release convergence
// ---------------------------------------------------------------------------

#[test]
fn releasing_an_unknown_attachment_while_attached_is_a_typed_mismatch() {
    let mut authority = attached_capability(2, ATTACHMENT_A);
    let before = authority.state().clone();

    let rejected = apply(&mut authority, release(ATTACHMENT_B));
    assert_eq!(
        single_effect(&rejected),
        Effect::ReleaseRejected {
            attachment_id: ATTACHMENT_B.to_owned(),
            reason: ForkedParticipantReleaseRejection::AttachmentMismatch,
        }
    );
    assert_eq!(authority.state(), &before);
}

#[test]
fn duplicate_release_converges_through_a_typed_replay() {
    let mut authority = attached_capability(2, ATTACHMENT_A);
    apply(&mut authority, release(ATTACHMENT_A));
    let before = authority.state().clone();

    let duplicate = apply(&mut authority, release(ATTACHMENT_A));
    assert_eq!(
        single_effect(&duplicate),
        Effect::ReleaseReplayed {
            attachment_id: ATTACHMENT_A.to_owned(),
        }
    );
    assert_eq!(authority.state(), &before);
}

#[test]
fn duplicate_release_after_terminalization_still_converges() {
    let mut authority = attached_capability(1, ATTACHMENT_A);
    apply(&mut authority, release(ATTACHMENT_A));
    let before = authority.state().clone();

    let duplicate = apply(&mut authority, release(ATTACHMENT_A));
    assert_eq!(
        single_effect(&duplicate),
        Effect::ReleaseReplayed {
            attachment_id: ATTACHMENT_A.to_owned(),
        }
    );
    assert_eq!(authority.state(), &before);
}

#[test]
fn releasing_an_unknown_attachment_is_typed_and_changes_nothing() {
    let mut authority = Authority::new();
    apply(&mut authority, reserve(REQUEST, 1));
    let before = authority.state().clone();

    let rejected = apply(&mut authority, release(ATTACHMENT_A));
    assert_eq!(
        single_effect(&rejected),
        Effect::ReleaseRejected {
            attachment_id: ATTACHMENT_A.to_owned(),
            reason: ForkedParticipantReleaseRejection::NoActiveAttachment,
        }
    );
    assert_eq!(authority.state(), &before);
}

// ---------------------------------------------------------------------------
// Revocation
// ---------------------------------------------------------------------------

#[test]
fn invalid_authentication_denies_revoke_without_touching_state() {
    let mut authority = active_capability(1);
    let before = authority.state().clone();

    let denied = apply(&mut authority, revoke(false));
    assert_eq!(
        single_effect(&denied),
        Effect::RevocationDenied {
            reason: ForkedParticipantRevocationDenial::AuthenticationInvalid,
        }
    );
    assert_eq!(authority.state(), &before);
}

#[test]
fn revoking_a_detached_active_capability_terminalizes_with_cleanup_debt() {
    let mut authority = active_capability(1);

    let revoked = apply(&mut authority, revoke(true));
    assert_eq!(revoked.to_phase, Phase::Revoked);
    assert_eq!(
        single_effect(&revoked),
        Effect::CapabilityRevoked {
            cleanup_pending: true
        }
    );
    assert_eq!(
        authority.state().cleanup_state,
        ForkedParticipantCleanupState::Pending
    );
}

#[test]
fn revoking_a_retryable_create_failed_capability_terminalizes_with_cleanup_debt() {
    let mut authority = Authority::new();
    apply(&mut authority, reserve(REQUEST, 1));
    apply(&mut authority, record_activation_failure(REQUEST));

    let revoked = apply(&mut authority, revoke(true));
    assert_eq!(revoked.to_phase, Phase::Revoked);
    assert_eq!(
        single_effect(&revoked),
        Effect::CapabilityRevoked {
            cleanup_pending: true
        }
    );
}

#[test]
fn revoking_an_attached_capability_defers_cleanup_until_the_exact_release() {
    let mut authority = attached_capability(3, ATTACHMENT_A);

    let pending = apply(&mut authority, revoke(true));
    assert_eq!(pending.to_phase, Phase::RevocationPendingAttached);
    assert_eq!(single_effect(&pending), Effect::RevocationPendingRecorded);
    assert_eq!(
        authority.state().cleanup_state,
        ForkedParticipantCleanupState::Deferred
    );

    let denied = apply(&mut authority, attach(ATTACHMENT_B, true, false));
    assert_eq!(
        single_effect(&denied),
        Effect::AttachDenied {
            attachment_id: ATTACHMENT_B.to_owned(),
            reason: ForkedParticipantAttachDenial::Revoked,
        }
    );

    let too_early = apply(&mut authority, complete_cleanup());
    assert_eq!(
        single_effect(&too_early),
        Effect::CleanupCompletionRejected {
            reason: ForkedParticipantCleanupRejection::AttachmentOutstanding,
        }
    );

    let released = apply(&mut authority, release(ATTACHMENT_A));
    assert_eq!(released.to_phase, Phase::Revoked);
    assert_eq!(
        effects(&released),
        vec![
            Effect::AttachmentReleased {
                attachment_id: ATTACHMENT_A.to_owned(),
                use_count: 1,
            },
            Effect::CapabilityRevoked {
                cleanup_pending: true
            },
        ]
    );
    assert_eq!(
        authority.state().cleanup_state,
        ForkedParticipantCleanupState::Pending
    );
}

#[test]
fn revoke_replay_converges_in_pending_and_terminal_states() {
    let mut authority = attached_capability(2, ATTACHMENT_A);
    apply(&mut authority, revoke(true));
    let pending_state = authority.state().clone();

    let pending_replay = apply(&mut authority, revoke(true));
    assert_eq!(single_effect(&pending_replay), Effect::RevocationConverged);
    assert_eq!(authority.state(), &pending_state);

    apply(&mut authority, release(ATTACHMENT_A));
    let revoked_state = authority.state().clone();

    let terminal_replay = apply(&mut authority, revoke(true));
    assert_eq!(single_effect(&terminal_replay), Effect::RevocationConverged);
    assert_eq!(authority.state(), &revoked_state);
}

#[test]
fn revoking_a_reservation_terminalizes_without_cleanup_debt() {
    let mut authority = Authority::new();
    apply(&mut authority, reserve(REQUEST, 1));

    let revoked = apply(&mut authority, revoke(true));
    assert_eq!(revoked.to_phase, Phase::Revoked);
    assert_eq!(
        single_effect(&revoked),
        Effect::CapabilityRevoked {
            cleanup_pending: false
        }
    );
    assert_eq!(
        authority.state().cleanup_state,
        ForkedParticipantCleanupState::NotRequired
    );
}

// ---------------------------------------------------------------------------
// Expiry observations (the machine never reads a clock)
// ---------------------------------------------------------------------------

#[test]
fn a_not_expired_observation_is_an_explicit_typed_no_op() {
    let mut authority = active_capability(1);
    let before = authority.state().clone();

    let ignored = apply(&mut authority, observe_expiry(false));
    assert_eq!(
        single_effect(&ignored),
        Effect::ExpiryObservationIgnored {
            reason: ForkedParticipantExpiryIgnore::NotExpired,
        }
    );
    assert_eq!(authority.state(), &before);
}

#[test]
fn an_expiry_observation_on_an_empty_record_is_typed_not_provisioned() {
    let mut authority = Authority::new();

    let ignored = apply(&mut authority, observe_expiry(true));
    assert_eq!(
        single_effect(&ignored),
        Effect::ExpiryObservationIgnored {
            reason: ForkedParticipantExpiryIgnore::NotProvisioned,
        }
    );
    assert_eq!(authority.state(), &State::default());
}

#[test]
fn an_expiry_observation_terminalizes_a_detached_capability_with_cleanup_debt() {
    let mut authority = active_capability(1);

    let expired = apply(&mut authority, observe_expiry(true));
    assert_eq!(expired.to_phase, Phase::Expired);
    assert_eq!(
        single_effect(&expired),
        Effect::CapabilityExpired {
            cleanup_pending: true
        }
    );
    assert_eq!(
        authority.state().cleanup_state,
        ForkedParticipantCleanupState::Pending
    );
}

#[test]
fn expiry_while_attached_prevents_new_work_and_waits_for_release() {
    let mut authority = attached_capability(3, ATTACHMENT_A);

    let pending = apply(&mut authority, observe_expiry(true));
    assert_eq!(pending.to_phase, Phase::ExpiryPendingAttached);
    assert_eq!(single_effect(&pending), Effect::ExpiryPendingRecorded);
    assert_eq!(
        authority.state().cleanup_state,
        ForkedParticipantCleanupState::Deferred
    );

    let denied = apply(&mut authority, attach(ATTACHMENT_B, true, false));
    assert_eq!(
        single_effect(&denied),
        Effect::AttachDenied {
            attachment_id: ATTACHMENT_B.to_owned(),
            reason: ForkedParticipantAttachDenial::Expired,
        }
    );

    let repeat = apply(&mut authority, observe_expiry(true));
    assert_eq!(
        single_effect(&repeat),
        Effect::ExpiryObservationIgnored {
            reason: ForkedParticipantExpiryIgnore::AlreadyRecorded,
        }
    );

    let blocked_cleanup = apply(&mut authority, complete_cleanup());
    assert_eq!(
        single_effect(&blocked_cleanup),
        Effect::CleanupCompletionRejected {
            reason: ForkedParticipantCleanupRejection::AttachmentOutstanding,
        }
    );

    let released = apply(&mut authority, release(ATTACHMENT_A));
    assert_eq!(released.to_phase, Phase::Expired);
    assert_eq!(
        effects(&released),
        vec![
            Effect::AttachmentReleased {
                attachment_id: ATTACHMENT_A.to_owned(),
                use_count: 1,
            },
            Effect::CapabilityExpired {
                cleanup_pending: true
            },
        ]
    );
}

#[test]
fn revocation_dominates_a_pending_expiry_while_attached() {
    let mut authority = attached_capability(2, ATTACHMENT_A);
    apply(&mut authority, observe_expiry(true));

    let revoked = apply(&mut authority, revoke(true));
    assert_eq!(revoked.to_phase, Phase::RevocationPendingAttached);
    assert_eq!(single_effect(&revoked), Effect::RevocationPendingRecorded);

    let released = apply(&mut authority, release(ATTACHMENT_A));
    assert_eq!(released.to_phase, Phase::Revoked);
}

// ---------------------------------------------------------------------------
// Cleanup debt
// ---------------------------------------------------------------------------

#[test]
fn cleanup_completes_once_for_a_terminal_detached_record_with_debt_and_replays() {
    let mut authority = attached_capability(1, ATTACHMENT_A);
    apply(&mut authority, release(ATTACHMENT_A));
    assert_eq!(authority.state().lifecycle_phase, Phase::Exhausted);

    let completed = apply(&mut authority, complete_cleanup());
    assert_eq!(single_effect(&completed), Effect::CleanupCompleted);
    assert_eq!(
        authority.state().cleanup_state,
        ForkedParticipantCleanupState::Complete
    );

    let before = authority.state().clone();
    let replay = apply(&mut authority, complete_cleanup());
    assert_eq!(single_effect(&replay), Effect::CleanupCompletionReplayed);
    assert_eq!(authority.state(), &before);
}

#[test]
fn cleanup_is_refused_for_a_live_record_and_for_a_terminal_record_without_debt() {
    let mut live = active_capability(1);
    let not_terminal = apply(&mut live, complete_cleanup());
    assert_eq!(
        single_effect(&not_terminal),
        Effect::CleanupCompletionRejected {
            reason: ForkedParticipantCleanupRejection::NotTerminal,
        }
    );

    let mut revoked_reservation = Authority::new();
    apply(&mut revoked_reservation, reserve(REQUEST, 1));
    apply(&mut revoked_reservation, revoke(true));
    let no_debt = apply(&mut revoked_reservation, complete_cleanup());
    assert_eq!(
        single_effect(&no_debt),
        Effect::CleanupCompletionRejected {
            reason: ForkedParticipantCleanupRejection::NoCleanupDebt,
        }
    );
    assert_eq!(
        revoked_reservation.state().cleanup_state,
        ForkedParticipantCleanupState::NotRequired
    );
}

// ---------------------------------------------------------------------------
// Invariant enforcement on recovery
// ---------------------------------------------------------------------------

fn healthy_active() -> State {
    State {
        lifecycle_phase: Phase::Active,
        request_fingerprint: REQUEST.to_owned(),
        max_uses: 3,
        use_count: 1,
        fork_activation_id: FORK.to_owned(),
        active_attachment_id: None,
        granted_attachment_ids: granted(&[ATTACHMENT_A]),
        cleanup_state: ForkedParticipantCleanupState::NotRequired,
    }
}

fn assert_recovery_rejects(state: State, expected_invariant: &str) {
    match Authority::recover_from_state(state.clone()) {
        Err(TransitionError::RecoveredStateInvariantRejected { invariant, .. }) => {
            assert_eq!(
                invariant, expected_invariant,
                "corrupt state {state:?} was rejected by the wrong invariant"
            );
        }
        Err(other) => panic!("corrupt state {state:?} rejected for the wrong reason: {other:?}"),
        Ok(_) => panic!("corrupt state {state:?} must be rejected on recovery"),
    }
}

#[test]
fn healthy_recovered_states_are_admitted() {
    for phase in ALL_PHASES {
        let state = fixture(phase).state().clone();
        Authority::recover_from_state(state)
            .unwrap_or_else(|error| panic!("healthy {phase:?} state must recover: {error:?}"));
    }
}

#[test]
fn corrupt_recovered_states_are_rejected_by_the_owning_invariant() {
    // A reserved record with a zero reuse budget.
    let mut zero_budget = healthy_active();
    zero_budget.lifecycle_phase = Phase::Reserved;
    zero_budget.max_uses = 0;
    zero_budget.use_count = 0;
    zero_budget.granted_attachment_ids = granted(&[]);
    zero_budget.fork_activation_id = String::new();
    assert_recovery_rejects(zero_budget, "reserved_capability_has_positive_max_uses");

    // Uses beyond the configured budget.
    let mut over_budget = healthy_active();
    over_budget.max_uses = 2;
    over_budget.use_count = 3;
    over_budget.granted_attachment_ids = granted(&[ATTACHMENT_A, ATTACHMENT_B, ATTACHMENT_C]);
    assert_recovery_rejects(over_budget, "use_count_within_max_uses");

    // The dedup ledger disagrees with the use count.
    let mut ledger_mismatch = healthy_active();
    ledger_mismatch.use_count = 2;
    ledger_mismatch.granted_attachment_ids = granted(&[ATTACHMENT_A]);
    assert_recovery_rejects(ledger_mismatch, "granted_attachments_match_use_count");

    // The active holder is not one of the granted identities.
    let mut phantom_holder = healthy_active();
    phantom_holder.lifecycle_phase = Phase::Attached;
    phantom_holder.active_attachment_id = Some(ATTACHMENT_C.to_owned());
    assert_recovery_rejects(phantom_holder, "active_holder_is_a_granted_attachment");

    // A detached phase holding an attachment.
    let mut detached_holder = healthy_active();
    detached_holder.active_attachment_id = Some(ATTACHMENT_A.to_owned());
    assert_recovery_rejects(detached_holder, "attachment_only_while_attached");

    // A terminal record still holding an attachment. `attachment_only_while_attached`
    // owns this shape first; the terminal-specific sibling is pinned separately
    // below.
    let mut terminal_holder = healthy_active();
    terminal_holder.lifecycle_phase = Phase::Exhausted;
    terminal_holder.max_uses = 1;
    terminal_holder.active_attachment_id = Some(ATTACHMENT_A.to_owned());
    terminal_holder.cleanup_state = ForkedParticipantCleanupState::Pending;
    assert_recovery_rejects(terminal_holder, "attachment_only_while_attached");

    // An attached phase with no attachment.
    let mut empty_holder = healthy_active();
    empty_holder.lifecycle_phase = Phase::Attached;
    empty_holder.active_attachment_id = None;
    assert_recovery_rejects(empty_holder, "attached_phase_holds_one_attachment");

    // Cleanup complete while the record is still live and attached.
    let mut complete_while_attached = healthy_active();
    complete_while_attached.lifecycle_phase = Phase::Attached;
    complete_while_attached.active_attachment_id = Some(ATTACHMENT_A.to_owned());
    complete_while_attached.cleanup_state = ForkedParticipantCleanupState::Complete;
    assert_recovery_rejects(
        complete_while_attached,
        "cleanup_complete_requires_detached_terminal",
    );

    // Deferred debt without an attachment blocking it.
    let mut stray_deferred = healthy_active();
    stray_deferred.cleanup_state = ForkedParticipantCleanupState::Deferred;
    assert_recovery_rejects(stray_deferred, "deferred_cleanup_requires_attachment");

    // An empty record carrying capability facts.
    let dirty_empty = State {
        request_fingerprint: REQUEST.to_owned(),
        ..State::default()
    };
    assert_recovery_rejects(dirty_empty, "empty_record_has_no_capability_facts");

    // A pre-activation record carrying grants.
    let mut premature_grant = healthy_active();
    premature_grant.lifecycle_phase = Phase::Reserved;
    premature_grant.fork_activation_id = String::new();
    assert_recovery_rejects(premature_grant, "pre_activation_record_has_no_grants");

    // A usable record with no recorded durable fork.
    let mut unbacked = healthy_active();
    unbacked.fork_activation_id = String::new();
    assert_recovery_rejects(unbacked, "usable_capability_has_fork_activation");
}

/// `terminal_capability_is_detached` is the terminal-specific sibling of
/// `attachment_only_while_attached`. Pinning it directly keeps the terminal
/// guarantee from silently depending on the more general invariant's ordering.
#[test]
fn terminal_detachment_invariant_rejects_a_terminal_record_holding_an_attachment() {
    let schema = meerkat_machine_schema::catalog::dsl::dsl_forked_participant_lifecycle_machine();
    assert!(
        schema
            .invariants
            .iter()
            .any(|invariant| invariant.name == "terminal_capability_is_detached"),
        "machine must declare terminal_capability_is_detached"
    );

    let mut terminal_holder = healthy_active();
    terminal_holder.lifecycle_phase = Phase::Revoked;
    terminal_holder.active_attachment_id = Some(ATTACHMENT_A.to_owned());
    terminal_holder.cleanup_state = ForkedParticipantCleanupState::Pending;
    assert!(
        Authority::recover_from_state(terminal_holder).is_err(),
        "a terminal record holding an attachment must never recover"
    );
}

// ---------------------------------------------------------------------------
// Canonical representation
// ---------------------------------------------------------------------------

/// One canonical snake_case representation, no compatibility aliases: the exact
/// snake_case token round-trips and any other spelling (including PascalCase)
/// is rejected.
#[test]
fn persisted_state_has_exactly_one_canonical_snake_case_representation() {
    let mut authority = attached_capability(2, ATTACHMENT_A);
    apply(&mut authority, revoke(true));
    let state = authority.state().clone();

    let encoded = serde_json::to_value(&state).expect("state serializes");
    assert_eq!(
        encoded["lifecycle_phase"], "revocation_pending_attached",
        "the phase must serialize to its canonical snake_case token"
    );
    assert_eq!(
        encoded["cleanup_state"], "deferred",
        "cleanup state must serialize to its canonical snake_case token"
    );
    assert_eq!(
        encoded["granted_attachment_ids"],
        serde_json::json!([ATTACHMENT_A])
    );

    let decoded: State = serde_json::from_value(encoded).expect("state round-trips");
    assert_eq!(decoded, state);

    for rejected in [
        serde_json::json!("RevocationPendingAttached"),
        serde_json::json!("Revoked"),
        serde_json::json!("revocationPendingAttached"),
    ] {
        assert!(
            serde_json::from_value::<Phase>(rejected.clone()).is_err(),
            "`{rejected}` must not be accepted as a phase spelling"
        );
    }
    assert!(
        serde_json::from_value::<ForkedParticipantCleanupState>(serde_json::json!("Deferred"))
            .is_err(),
        "PascalCase cleanup state must not be accepted"
    );
}

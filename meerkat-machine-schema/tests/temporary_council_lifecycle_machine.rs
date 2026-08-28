//! Focused behavioural tests for the canonical `TemporaryCouncilLifecycleMachine`.
//!
//! The machine is record-scoped: every test drives ONE council record. It reads
//! no clock and holds no mob/member/capability facts — those stay with
//! `MobMachine`, the member machines, and `ForkedParticipantLifecycleMachine`.
//!
//! The centrepiece is
//! [`every_command_class_in_every_phase_is_total_and_only_mutates_where_intended`],
//! which drives every command class against every phase and asserts either the
//! one intended lifecycle transition or byte-identical state preservation.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use meerkat_machine_schema::catalog::dsl::temporary_council_lifecycle::{
    TemporaryCouncilAdvanceRejection, TemporaryCouncilClaimDenial,
    TemporaryCouncilCleanupRejection, TemporaryCouncilExitClass, TemporaryCouncilLifecycleEffect,
    TemporaryCouncilLifecycleInput, TemporaryCouncilLifecycleMachineAuthority,
    TemporaryCouncilLifecycleMachineMutator, TemporaryCouncilLifecycleMachineState,
    TemporaryCouncilLifecycleMachineTransition, TemporaryCouncilLifecycleState,
    TemporaryCouncilOpenRejection, TemporaryCouncilSealRejection,
};

type Authority = TemporaryCouncilLifecycleMachineAuthority;
type Input = TemporaryCouncilLifecycleInput;
type Effect = TemporaryCouncilLifecycleEffect;
type Phase = TemporaryCouncilLifecycleState;
type State = TemporaryCouncilLifecycleMachineState;
type Transition = TemporaryCouncilLifecycleMachineTransition;

const REQUEST: &str = "tcf1:sha256:council-a";
const OTHER_REQUEST: &str = "tcf1:sha256:council-b";
const CLAIM: &str = "claim-a";
const OTHER_CLAIM: &str = "claim-b";

fn apply(authority: &mut Authority, input: Input) -> Transition {
    TemporaryCouncilLifecycleMachineMutator::apply(authority, input)
        .expect("every command class must resolve to an explicit typed transition")
}

fn open(fingerprint: &str) -> Input {
    Input::Open {
        request_fingerprint: fingerprint.to_owned(),
    }
}

fn claim(claim_id: &str, lease_expired: bool) -> Input {
    Input::Claim {
        claim_id: claim_id.to_owned(),
        lease_expired,
    }
}

/// The claim identity/epoch a command must present to pass the fence.
fn held(authority: &Authority) -> (String, u64) {
    let state = authority.state();
    (state.claim_id.clone(), state.claim_epoch)
}

fn start_discussion(authority: &Authority) -> Input {
    let (claim_id, claim_epoch) = held(authority);
    Input::StartDiscussion {
        claim_id,
        claim_epoch,
    }
}

fn start_merge(authority: &Authority) -> Input {
    let (claim_id, claim_epoch) = held(authority);
    Input::StartMerge {
        claim_id,
        claim_epoch,
    }
}

fn seal_result(authority: &Authority) -> Input {
    let (claim_id, claim_epoch) = held(authority);
    Input::SealResult {
        claim_id,
        claim_epoch,
    }
}

fn seal_interrupted(authority: &Authority) -> Input {
    let (claim_id, claim_epoch) = held(authority);
    Input::SealInterruptedResult {
        claim_id,
        claim_epoch,
    }
}

fn cleanup_settled(authority: &Authority) -> Input {
    let (claim_id, claim_epoch) = held(authority);
    Input::RecordCleanupSettled {
        claim_id,
        claim_epoch,
    }
}

fn cleanup_debt(authority: &Authority) -> Input {
    let (claim_id, claim_epoch) = held(authority);
    Input::RecordCleanupDebt {
        claim_id,
        claim_epoch,
    }
}

fn effects(transition: &Transition) -> Vec<Effect> {
    transition.effects().to_vec()
}

fn only_effect(transition: &Transition) -> Effect {
    let effects = effects(transition);
    assert_eq!(
        effects.len(),
        1,
        "every council transition emits exactly one verdict, got {effects:?}"
    );
    effects[0].clone()
}

/// Build an authority parked in `phase` with `REQUEST` bound and `CLAIM` held.
fn authority_in(phase: Phase) -> Authority {
    let mut authority = Authority::new();
    if phase == Phase::Empty {
        return authority;
    }
    apply(&mut authority, open(REQUEST));
    apply(&mut authority, claim(CLAIM, false));
    match phase {
        Phase::Empty => unreachable!("handled above"),
        Phase::Preparing => {}
        Phase::Running => {
            let input = start_discussion(&authority);
            apply(&mut authority, input);
        }
        Phase::Merging => {
            let input = start_discussion(&authority);
            apply(&mut authority, input);
            let input = start_merge(&authority);
            apply(&mut authority, input);
        }
        Phase::Concluded => {
            let input = start_discussion(&authority);
            apply(&mut authority, input);
            let input = start_merge(&authority);
            apply(&mut authority, input);
            let input = seal_result(&authority);
            apply(&mut authority, input);
        }
        Phase::CleanupDebt => {
            let input = start_discussion(&authority);
            apply(&mut authority, input);
            let input = start_merge(&authority);
            apply(&mut authority, input);
            let input = seal_result(&authority);
            apply(&mut authority, input);
            let input = cleanup_debt(&authority);
            apply(&mut authority, input);
        }
        Phase::Settled => {
            let input = start_discussion(&authority);
            apply(&mut authority, input);
            let input = start_merge(&authority);
            apply(&mut authority, input);
            let input = seal_result(&authority);
            apply(&mut authority, input);
            let input = cleanup_settled(&authority);
            apply(&mut authority, input);
        }
    }
    assert_eq!(authority.state().lifecycle_phase, phase);
    authority
}

const ALL_PHASES: [Phase; 7] = [
    Phase::Empty,
    Phase::Preparing,
    Phase::Running,
    Phase::Merging,
    Phase::Concluded,
    Phase::CleanupDebt,
    Phase::Settled,
];

// ===========================================================================
// Request identity binding
// ===========================================================================

#[test]
fn open_binds_one_request_and_replays_the_exact_same_one() {
    let mut authority = Authority::new();
    let opened = apply(&mut authority, open(REQUEST));
    assert!(matches!(
        only_effect(&opened),
        Effect::CouncilOpened { ref request_fingerprint } if request_fingerprint == REQUEST
    ));
    assert_eq!(authority.state().lifecycle_phase, Phase::Preparing);
    assert_eq!(authority.state().request_fingerprint, REQUEST);

    let before = authority.state().clone();
    let replay = apply(&mut authority, open(REQUEST));
    assert!(matches!(
        only_effect(&replay),
        Effect::CouncilOpenReplayed { .. }
    ));
    assert_eq!(
        authority.state(),
        &before,
        "an exact replay never advances the record"
    );
}

#[test]
fn a_conflicting_request_can_never_rebind_a_bound_council_identity() {
    for phase in [
        Phase::Preparing,
        Phase::Running,
        Phase::Merging,
        Phase::Concluded,
        Phase::CleanupDebt,
        Phase::Settled,
    ] {
        let mut authority = authority_in(phase);
        let before = authority.state().clone();
        let rejected = apply(&mut authority, open(OTHER_REQUEST));
        assert!(
            matches!(
                only_effect(&rejected),
                Effect::CouncilOpenRejected {
                    reason: TemporaryCouncilOpenRejection::FingerprintConflict
                }
            ),
            "phase {phase:?} must refuse a different request"
        );
        assert_eq!(
            authority.state(),
            &before,
            "a refused open never mutates state in phase {phase:?}"
        );
    }
}

#[test]
fn an_empty_fingerprint_is_refused_outright() {
    let mut authority = Authority::new();
    let rejected = apply(&mut authority, open(""));
    assert!(matches!(
        only_effect(&rejected),
        Effect::CouncilOpenRejected {
            reason: TemporaryCouncilOpenRejection::MalformedRequest
        }
    ));
    assert_eq!(authority.state(), &State::default());
}

// ===========================================================================
// Advance and result sealing
// ===========================================================================

#[test]
fn the_advance_ladder_is_idempotent_and_monotonic() {
    let mut authority = authority_in(Phase::Preparing);
    let opened_revision = authority.state().revision;

    let started = {
        let input = start_discussion(&authority);
        apply(&mut authority, input)
    };
    assert!(matches!(
        only_effect(&started),
        Effect::DiscussionStarted { .. }
    ));
    assert_eq!(authority.state().lifecycle_phase, Phase::Running);
    assert!(authority.state().revision > opened_revision);

    let replay_state = authority.state().clone();
    let replay = {
        let input = start_discussion(&authority);
        apply(&mut authority, input)
    };
    assert!(matches!(
        only_effect(&replay),
        Effect::DiscussionStartReplayed { .. }
    ));
    assert_eq!(authority.state(), &replay_state);

    let merged = {
        let input = start_merge(&authority);
        apply(&mut authority, input)
    };
    assert!(matches!(only_effect(&merged), Effect::MergeStarted { .. }));
    assert_eq!(authority.state().lifecycle_phase, Phase::Merging);

    let merge_state = authority.state().clone();
    let merge_replay = {
        let input = start_merge(&authority);
        apply(&mut authority, input)
    };
    assert!(matches!(
        only_effect(&merge_replay),
        Effect::MergeStartReplayed { .. }
    ));
    assert_eq!(authority.state(), &merge_state);
}

/// A council that never reached a runnable discussion still enters merge, so
/// the explicit merge-back policy can produce its typed not-attempted outcome.
#[test]
fn a_council_that_never_ran_still_enters_merge_from_preparing() {
    let mut authority = authority_in(Phase::Preparing);
    let merged = {
        let input = start_merge(&authority);
        apply(&mut authority, input)
    };
    assert!(matches!(only_effect(&merged), Effect::MergeStarted { .. }));
    assert_eq!(authority.state().lifecycle_phase, Phase::Merging);
}

#[test]
fn exactly_one_result_is_sealed_and_a_second_class_is_refused() {
    let mut authority = authority_in(Phase::Merging);
    let sealed = {
        let input = seal_result(&authority);
        apply(&mut authority, input)
    };
    assert!(matches!(
        only_effect(&sealed),
        Effect::ResultSealed {
            exit_class: TemporaryCouncilExitClass::Executed,
            ..
        }
    ));
    assert_eq!(authority.state().lifecycle_phase, Phase::Concluded);

    let after_seal = authority.state().clone();
    let replay = {
        let input = seal_result(&authority);
        apply(&mut authority, input)
    };
    assert!(matches!(
        only_effect(&replay),
        Effect::ResultSealReplayed {
            exit_class: TemporaryCouncilExitClass::Executed
        }
    ));
    assert_eq!(authority.state(), &after_seal);

    let conflict = {
        let input = seal_interrupted(&authority);
        apply(&mut authority, input)
    };
    assert!(
        matches!(
            only_effect(&conflict),
            Effect::ResultSealRejected {
                reason: TemporaryCouncilSealRejection::AlreadySealed
            }
        ),
        "an executed result may not be reclassified as interrupted"
    );
    assert_eq!(authority.state(), &after_seal);
}

#[test]
fn a_normal_seal_before_merge_is_refused() {
    for phase in [Phase::Preparing, Phase::Running] {
        let mut authority = authority_in(phase);
        let before = authority.state().clone();
        let rejected = {
            let input = seal_result(&authority);
            apply(&mut authority, input)
        };
        assert!(matches!(
            only_effect(&rejected),
            Effect::ResultSealRejected {
                reason: TemporaryCouncilSealRejection::NotMerging
            }
        ));
        assert_eq!(authority.state(), &before);
    }
}

// ===========================================================================
// Coordinator-interrupted recovery
// ===========================================================================

#[test]
fn a_crash_in_any_unsealed_bound_phase_seals_exactly_one_interrupted_terminal() {
    for phase in [Phase::Preparing, Phase::Running, Phase::Merging] {
        let mut authority = authority_in(phase);
        let sealed = {
            let input = seal_interrupted(&authority);
            apply(&mut authority, input)
        };
        assert!(
            matches!(
                only_effect(&sealed),
                Effect::ResultSealed {
                    exit_class: TemporaryCouncilExitClass::CoordinatorInterrupted,
                    ..
                }
            ),
            "phase {phase:?} must seal an interrupted terminal"
        );
        assert_eq!(authority.state().lifecycle_phase, Phase::Concluded);

        let after = authority.state().clone();
        let replay = {
            let input = seal_interrupted(&authority);
            apply(&mut authority, input)
        };
        assert!(matches!(
            only_effect(&replay),
            Effect::ResultSealReplayed {
                exit_class: TemporaryCouncilExitClass::CoordinatorInterrupted
            }
        ));
        assert_eq!(
            authority.state(),
            &after,
            "a second recovery sweep must never re-seal, so the council can \
             never be re-executed"
        );

        let conflict = {
            let input = seal_result(&authority);
            apply(&mut authority, input)
        };
        assert!(matches!(
            only_effect(&conflict),
            Effect::ResultSealRejected {
                reason: TemporaryCouncilSealRejection::AlreadySealed
            }
        ));
    }
}

// ===========================================================================
// Cleanup convergence
// ===========================================================================

#[test]
fn cleanup_debt_is_retained_across_attempts_and_converges_to_settled() {
    let mut authority = authority_in(Phase::Concluded);
    assert_eq!(authority.state().cleanup_attempts, 0);

    let first = {
        let input = cleanup_debt(&authority);
        apply(&mut authority, input)
    };
    assert!(matches!(
        only_effect(&first),
        Effect::CleanupDebtRecorded { attempts: 1, .. }
    ));
    assert_eq!(authority.state().lifecycle_phase, Phase::CleanupDebt);

    let second = {
        let input = cleanup_debt(&authority);
        apply(&mut authority, input)
    };
    assert!(matches!(
        only_effect(&second),
        Effect::CleanupDebtRecorded { attempts: 2, .. }
    ));
    assert_eq!(authority.state().lifecycle_phase, Phase::CleanupDebt);

    let settled = {
        let input = cleanup_settled(&authority);
        apply(&mut authority, input)
    };
    assert!(matches!(
        only_effect(&settled),
        Effect::CleanupSettled { attempts: 3, .. }
    ));
    assert_eq!(authority.state().lifecycle_phase, Phase::Settled);

    let after = authority.state().clone();
    let replay = {
        let input = cleanup_settled(&authority);
        apply(&mut authority, input)
    };
    assert!(matches!(
        only_effect(&replay),
        Effect::CleanupSettlementReplayed { attempts: 3 }
    ));
    assert_eq!(authority.state(), &after);

    let reopen = {
        let input = cleanup_debt(&authority);
        apply(&mut authority, input)
    };
    assert!(
        matches!(
            only_effect(&reopen),
            Effect::CleanupRejected {
                reason: TemporaryCouncilCleanupRejection::AlreadySettled
            }
        ),
        "a settled council may never be re-opened as debt"
    );
    assert_eq!(authority.state(), &after);
}

#[test]
fn cleanup_cannot_be_recorded_before_a_result_is_sealed() {
    for phase in [
        Phase::Empty,
        Phase::Preparing,
        Phase::Running,
        Phase::Merging,
    ] {
        for build in [cleanup_settled as fn(&Authority) -> Input, cleanup_debt] {
            let mut authority = authority_in(phase);
            let before = authority.state().clone();
            let input = build(&authority);
            let rejected = apply(&mut authority, input);
            assert!(
                matches!(
                    only_effect(&rejected),
                    Effect::CleanupRejected {
                        reason: TemporaryCouncilCleanupRejection::ResultNotSealed
                    }
                ),
                "phase {phase:?} must refuse a cleanup record"
            );
            assert_eq!(authority.state(), &before);
        }
    }
}

// ===========================================================================
// Recovery classification is machine-owned
// ===========================================================================

#[test]
fn the_recovery_verdict_is_a_machine_fact_for_every_phase() {
    let expected: [(Phase, bool, bool, bool); 7] = [
        (Phase::Empty, true, false, true),
        (Phase::Preparing, true, false, true),
        (Phase::Running, true, false, true),
        (Phase::Merging, true, false, true),
        (Phase::Concluded, true, true, true),
        (Phase::CleanupDebt, true, true, true),
        (Phase::Settled, false, true, false),
    ];
    for (phase, unfinished, result_sealed, needs_cleanup) in expected {
        let mut authority = authority_in(phase);
        let before = authority.state().clone();
        let classified = apply(&mut authority, Input::ClassifyRecovery {});
        match only_effect(&classified) {
            Effect::RecoveryClassified {
                unfinished: actual_unfinished,
                result_sealed: actual_sealed,
                needs_cleanup: actual_cleanup,
            } => {
                assert_eq!(actual_unfinished, unfinished, "unfinished for {phase:?}");
                assert_eq!(actual_sealed, result_sealed, "result_sealed for {phase:?}");
                assert_eq!(actual_cleanup, needs_cleanup, "needs_cleanup for {phase:?}");
            }
            other => panic!("expected a recovery verdict for {phase:?}, got {other:?}"),
        }
        assert_eq!(
            authority.state(),
            &before,
            "classification never mutates lifecycle state ({phase:?})"
        );
    }
}

// ===========================================================================
// Coordinator claim / lease / fencing
// ===========================================================================

#[test]
fn a_claim_is_granted_once_then_renewed_for_the_same_holder() {
    let mut authority = Authority::new();
    apply(&mut authority, open(REQUEST));
    assert_eq!(authority.state().claim_epoch, 0);

    let granted = apply(&mut authority, claim(CLAIM, false));
    assert!(matches!(
        only_effect(&granted),
        Effect::ClaimGranted {
            ref claim_id,
            claim_epoch: 1,
            took_over: false,
        } if claim_id == CLAIM
    ));

    let renewed = apply(&mut authority, claim(CLAIM, false));
    assert!(
        matches!(
            only_effect(&renewed),
            Effect::ClaimRenewed {
                ref claim_id,
                claim_epoch: 1,
            } if claim_id == CLAIM
        ),
        "the incumbent renewing its own lease must never advance the epoch"
    );
    assert_eq!(authority.state().claim_epoch, 1);
}

#[test]
fn a_second_coordinator_is_refused_while_the_lease_is_live() {
    let mut authority = authority_in(Phase::Running);
    let before = authority.state().clone();

    let denied = apply(&mut authority, claim(OTHER_CLAIM, false));
    assert!(matches!(
        only_effect(&denied),
        Effect::ClaimDenied {
            reason: TemporaryCouncilClaimDenial::HeldByAnotherCoordinator,
            current_claim_epoch: 1,
        }
    ));
    assert_eq!(
        authority.state(),
        &before,
        "a refused claim never mutates the record"
    );
}

#[test]
fn an_expired_lease_lets_a_second_coordinator_take_over_and_fence_the_first() {
    let mut authority = authority_in(Phase::Running);
    let stale = start_merge(&authority);

    let taken = apply(&mut authority, claim(OTHER_CLAIM, true));
    assert!(matches!(
        only_effect(&taken),
        Effect::ClaimGranted {
            ref claim_id,
            claim_epoch: 2,
            took_over: true,
        } if claim_id == OTHER_CLAIM
    ));

    // The pre-takeover executor's next command carries the old identity.
    let after_takeover = authority.state().clone();
    let fenced = apply(&mut authority, stale);
    assert!(
        matches!(
            only_effect(&fenced),
            Effect::CommandFenced {
                current_claim_epoch: 2
            }
        ),
        "the displaced executor must be fenced, not allowed to advance"
    );
    assert_eq!(
        authority.state(),
        &after_takeover,
        "a fenced command never mutates the record"
    );
}

#[test]
fn every_mutating_command_is_fenced_under_a_stale_claim() {
    let stale_commands: [(&str, fn(&Authority) -> Input); 6] = [
        ("StartDiscussion", start_discussion),
        ("StartMerge", start_merge),
        ("SealResult", seal_result),
        ("SealInterruptedResult", seal_interrupted),
        ("RecordCleanupSettled", cleanup_settled),
        ("RecordCleanupDebt", cleanup_debt),
    ];
    for phase in [
        Phase::Preparing,
        Phase::Running,
        Phase::Merging,
        Phase::Concluded,
        Phase::CleanupDebt,
    ] {
        for (label, build) in stale_commands {
            let mut authority = authority_in(phase);
            let stale = build(&authority);
            apply(&mut authority, claim(OTHER_CLAIM, true));
            let before = authority.state().clone();
            let fenced = apply(&mut authority, stale);
            assert!(
                matches!(only_effect(&fenced), Effect::CommandFenced { .. }),
                "({phase:?}, {label}) under a stale claim must be fenced"
            );
            assert_eq!(
                authority.state(),
                &before,
                "({phase:?}, {label}) fencing never mutates the record"
            );
        }
    }
}

#[test]
fn an_unopened_or_settled_record_cannot_be_claimed() {
    let mut authority = Authority::new();
    let denied = apply(&mut authority, claim(CLAIM, false));
    assert!(matches!(
        only_effect(&denied),
        Effect::ClaimDenied {
            reason: TemporaryCouncilClaimDenial::NotOpened,
            ..
        }
    ));

    let mut authority = authority_in(Phase::Settled);
    let before = authority.state().clone();
    let denied = apply(&mut authority, claim(OTHER_CLAIM, true));
    assert!(
        matches!(
            only_effect(&denied),
            Effect::ClaimDenied {
                reason: TemporaryCouncilClaimDenial::AlreadySettled,
                ..
            }
        ),
        "a fully settled council has no work left to take over"
    );
    assert_eq!(authority.state(), &before);
}

#[test]
fn a_malformed_claim_is_refused_without_touching_the_record() {
    let mut authority = authority_in(Phase::Preparing);
    let before = authority.state().clone();
    let denied = apply(&mut authority, claim("", false));
    assert!(matches!(
        only_effect(&denied),
        Effect::ClaimDenied {
            reason: TemporaryCouncilClaimDenial::MalformedClaim,
            ..
        }
    ));
    assert_eq!(authority.state(), &before);
}

// ===========================================================================
// Totality
// ===========================================================================

/// Every (phase, command class) pair resolves to exactly one typed transition
/// with an EXACT from/to phase, and every non-advancing pair leaves the state
/// byte-identical.
///
/// This is the machine's totality contract. It is written as an exhaustive
/// matrix rather than a sampling because the failure mode it guards against —
/// a self-loop that silently rewinds a phase, or an invariant rejection that
/// replaces a promised typed verdict — is invisible in a spot check.
#[test]
fn every_command_class_in_every_phase_is_total_and_only_mutates_where_intended() {
    let commands: Vec<(&str, fn(&Authority) -> Input)> = vec![
        ("Open", |_| open(REQUEST)),
        ("OpenConflicting", |_| open(OTHER_REQUEST)),
        ("OpenMalformed", |_| open("")),
        ("Claim", |_| claim(CLAIM, false)),
        ("ClaimMalformed", |_| claim("", false)),
        ("ClaimOther", |_| claim(OTHER_CLAIM, false)),
        ("ClaimOtherExpired", |_| claim(OTHER_CLAIM, true)),
        ("StartDiscussion", start_discussion),
        ("StartMerge", start_merge),
        ("SealResult", seal_result),
        ("SealInterruptedResult", seal_interrupted),
        ("RecordCleanupSettled", cleanup_settled),
        ("RecordCleanupDebt", cleanup_debt),
        ("StartDiscussionStale", |_| Input::StartDiscussion {
            claim_id: String::new(),
            claim_epoch: 0,
        }),
        ("StartMergeStale", |_| Input::StartMerge {
            claim_id: String::new(),
            claim_epoch: 0,
        }),
        ("SealResultStale", |_| Input::SealResult {
            claim_id: String::new(),
            claim_epoch: 0,
        }),
        ("SealInterruptedResultStale", |_| {
            Input::SealInterruptedResult {
                claim_id: String::new(),
                claim_epoch: 0,
            }
        }),
        ("RecordCleanupSettledStale", |_| {
            Input::RecordCleanupSettled {
                claim_id: String::new(),
                claim_epoch: 0,
            }
        }),
        ("RecordCleanupDebtStale", |_| Input::RecordCleanupDebt {
            claim_id: String::new(),
            claim_epoch: 0,
        }),
        ("ClassifyRecovery", |_| Input::ClassifyRecovery {}),
    ];

    // The EXACT target phase of every pair that is allowed to advance. Every
    // pair absent from this table must be a byte-identical self-loop.
    let advancing: &[(Phase, &str, Phase)] = &[
        (Phase::Empty, "Open", Phase::Preparing),
        (Phase::Empty, "OpenConflicting", Phase::Preparing),
        // A takeover by a different coordinator on an expired lease advances
        // the claim epoch in every live phase, and NEVER changes the phase.
        (Phase::Preparing, "ClaimOtherExpired", Phase::Preparing),
        (Phase::Running, "ClaimOtherExpired", Phase::Running),
        (Phase::Merging, "ClaimOtherExpired", Phase::Merging),
        (Phase::Concluded, "ClaimOtherExpired", Phase::Concluded),
        (Phase::CleanupDebt, "ClaimOtherExpired", Phase::CleanupDebt),
        (Phase::Preparing, "StartDiscussion", Phase::Running),
        (Phase::Preparing, "StartMerge", Phase::Merging),
        (Phase::Preparing, "SealInterruptedResult", Phase::Concluded),
        (Phase::Running, "StartMerge", Phase::Merging),
        (Phase::Running, "SealInterruptedResult", Phase::Concluded),
        (Phase::Merging, "SealResult", Phase::Concluded),
        (Phase::Merging, "SealInterruptedResult", Phase::Concluded),
        (Phase::Concluded, "RecordCleanupSettled", Phase::Settled),
        (Phase::Concluded, "RecordCleanupDebt", Phase::CleanupDebt),
        (Phase::CleanupDebt, "RecordCleanupSettled", Phase::Settled),
        (Phase::CleanupDebt, "RecordCleanupDebt", Phase::CleanupDebt),
    ];

    for phase in ALL_PHASES {
        for (label, build) in &commands {
            let mut authority = authority_in(phase);
            let before = authority.state().clone();
            let input = build(&authority);
            let transition = apply(&mut authority, input);
            assert!(
                !transition.effects().is_empty(),
                "({phase:?}, {label}) must emit a typed verdict"
            );
            let target = advancing.iter().find_map(|(advancing_phase, command, to)| {
                (*advancing_phase == phase && command == label).then_some(*to)
            });
            match target {
                Some(to) => {
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
                }
                None => {
                    assert_eq!(
                        authority.state().lifecycle_phase,
                        phase,
                        "({phase:?}, {label}) is a self-loop and may never move the phase"
                    );
                    assert_eq!(
                        authority.state(),
                        &before,
                        "({phase:?}, {label}) must leave the state byte-identical"
                    );
                }
            }
        }
    }
}

/// A stale command is fenced in EVERY bound phase, including the terminal one,
/// and never rewinds or resurrects the record.
#[test]
fn a_stale_claim_is_fenced_in_every_bound_phase_including_settled() {
    let stale_commands: [(&str, fn(&Authority) -> Input); 6] = [
        ("StartDiscussion", start_discussion),
        ("StartMerge", start_merge),
        ("SealResult", seal_result),
        ("SealInterruptedResult", seal_interrupted),
        ("RecordCleanupSettled", cleanup_settled),
        ("RecordCleanupDebt", cleanup_debt),
    ];
    for phase in [
        Phase::Preparing,
        Phase::Running,
        Phase::Merging,
        Phase::Concluded,
        Phase::CleanupDebt,
        Phase::Settled,
    ] {
        for (label, build) in stale_commands {
            // Mint the command under the claim held BEFORE the takeover, then
            // let another coordinator take the expired lease. For `Settled`
            // the takeover must happen while the record is still in
            // `CleanupDebt`, because a settled record refuses every claim.
            let (mut authority, displaced) = if phase == Phase::Settled {
                let mut authority = authority_in(Phase::CleanupDebt);
                let displaced = build(&authority);
                apply(&mut authority, claim(OTHER_CLAIM, true));
                let settle = cleanup_settled(&authority);
                apply(&mut authority, settle);
                assert_eq!(authority.state().lifecycle_phase, Phase::Settled);
                (authority, displaced)
            } else {
                let mut authority = authority_in(phase);
                let displaced = build(&authority);
                apply(&mut authority, claim(OTHER_CLAIM, true));
                (authority, displaced)
            };

            let before = authority.state().clone();
            let fenced = apply(&mut authority, displaced);
            assert!(
                matches!(only_effect(&fenced), Effect::CommandFenced { .. }),
                "({phase:?}, {label}) under a displaced claim must be fenced, \
                 not rejected under some other verdict"
            );
            assert_eq!(
                authority.state(),
                &before,
                "({phase:?}, {label}) fencing must leave the record byte-identical"
            );
            assert_eq!(
                authority.state().lifecycle_phase,
                phase,
                "({phase:?}, {label}) fencing must never rewind or resurrect a phase"
            );
        }
    }
}

/// A terminal `Settled` record can never be resurrected or rewound by ANY
/// command class, stale or current.
#[test]
fn a_settled_record_is_never_resurrected_by_any_command() {
    let commands: [(&str, fn(&Authority) -> Input); 9] = [
        ("Open", |_| open(REQUEST)),
        ("OpenConflicting", |_| open(OTHER_REQUEST)),
        ("Claim", |_| claim(CLAIM, false)),
        ("ClaimOtherExpired", |_| claim(OTHER_CLAIM, true)),
        ("StartDiscussion", start_discussion),
        ("StartMerge", start_merge),
        ("SealResult", seal_result),
        ("SealInterruptedResult", seal_interrupted),
        ("RecordCleanupDebt", cleanup_debt),
    ];
    for (label, build) in commands {
        let mut authority = authority_in(Phase::Settled);
        let before = authority.state().clone();
        let input = build(&authority);
        apply(&mut authority, input);
        assert_eq!(
            authority.state(),
            &before,
            "a settled council must be inert under {label}"
        );
    }
}

/// A persisted state that violates a recovery invariant is refused with a
/// typed rejection instead of being silently adopted.
#[test]
fn a_corrupt_recovered_state_is_refused_with_a_typed_invariant_rejection() {
    let cases: [(&str, fn(&mut State)); 6] = [
        ("an empty record carrying a fingerprint", |state| {
            *state = State::default();
            state.request_fingerprint = REQUEST.to_owned();
        }),
        ("a bound record with no fingerprint", |state| {
            state.request_fingerprint = String::new();
        }),
        ("an unsealed phase carrying an exit class", |state| {
            state.lifecycle_phase = Phase::Running;
            state.exit_class = TemporaryCouncilExitClass::Executed;
        }),
        ("a sealed phase with no exit class", |state| {
            state.exit_class = TemporaryCouncilExitClass::Unsealed;
        }),
        ("cleanup attempts on an unsealed record", |state| {
            state.lifecycle_phase = Phase::Preparing;
            state.exit_class = TemporaryCouncilExitClass::Unsealed;
            state.cleanup_attempts = 3;
        }),
        ("a claim identity without an epoch", |state| {
            state.claim_id = CLAIM.to_owned();
            state.claim_epoch = 0;
        }),
    ];

    for (label, corrupt) in cases {
        let mut state = authority_in(Phase::Concluded).state().clone();
        corrupt(&mut state);
        let error = Authority::recover_from_state(state)
            .err()
            .unwrap_or_else(|| panic!("{label} must be refused on recovery"));
        assert!(
            format!("{error:?}").contains("Invariant"),
            "{label} must be refused as a typed invariant rejection, got {error:?}"
        );
    }

    // The honest control: a well-formed persisted state still recovers.
    let healthy = authority_in(Phase::CleanupDebt).state().clone();
    let recovered = Authority::recover_from_state(healthy.clone())
        .expect("a well-formed persisted state must recover");
    assert_eq!(recovered.state(), &healthy);
}

/// A rejected advance never silently changes the phase.
#[test]
fn advance_rejections_are_typed_and_inert() {
    let mut authority = Authority::new();
    let rejected = {
        let input = start_discussion(&authority);
        apply(&mut authority, input)
    };
    assert!(matches!(
        only_effect(&rejected),
        Effect::AdvanceRejected {
            reason: TemporaryCouncilAdvanceRejection::NotOpened
        }
    ));

    let mut authority = authority_in(Phase::Concluded);
    let before = authority.state().clone();
    let rejected = {
        let input = start_discussion(&authority);
        apply(&mut authority, input)
    };
    assert!(matches!(
        only_effect(&rejected),
        Effect::AdvanceRejected {
            reason: TemporaryCouncilAdvanceRejection::AlreadyAdvanced
        }
    ));
    assert_eq!(authority.state(), &before);
}

/// The persisted machine state round-trips through its canonical serde form.
#[test]
fn machine_state_round_trips_through_its_canonical_representation() {
    for phase in ALL_PHASES {
        let authority = authority_in(phase);
        let encoded = serde_json::to_string(authority.state()).expect("encode");
        let decoded: State = serde_json::from_str(&encoded).expect("decode");
        assert_eq!(&decoded, authority.state(), "round trip for {phase:?}");
    }
}

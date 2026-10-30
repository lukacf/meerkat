//! S3 rows: the SessionDocumentMachine operator-instruction region.
//!
//! One row per transition arm, the region's four invariants (through
//! `recover_from_state`), and the schema census the coverage manifest and
//! the instruction boundary TLC audit read. The machine holds the per-session
//! facts it decides from (owner, target generation, slot count, effective
//! active count, entry-sequence high-water); the shell feeds mechanical
//! observations of its typed target set and the transcript, and mirrors the
//! verdict.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use meerkat_machine_schema::canonical_machine_coverage_manifests;
use meerkat_machine_schema::catalog::dsl::dsl_session_document_machine;
use meerkat_machine_schema::catalog::dsl::session_document::{
    InstructionDeferReason, InstructionEffect, InstructionFoldSource, InstructionLowering,
    InstructionRunStart, InstructionTargetOwner, InstructionWriteOutcome, InstructionWriteRefusal,
    SessionDocumentEffect, SessionDocumentInput, SessionDocumentMachineAuthority,
    SessionDocumentMachineMutator, SessionDocumentMachineState, SessionDocumentMachineTransition,
    SessionDocumentMachineTransitionError, SessionId,
};

type Authority = SessionDocumentMachineAuthority;
type Input = SessionDocumentInput;
type Effect = SessionDocumentEffect;
type State = SessionDocumentMachineState;
type Transition = SessionDocumentMachineTransition;
type TransitionError = SessionDocumentMachineTransitionError;

const ARMS: [&str; 19] = [
    "RecoverSessionInstructionState",
    "RecordInstructionTargetAdvance",
    "RecordInstructionTargetAlreadyRecorded",
    "RecordInstructionTargetOwnedBySession",
    "SetSessionInstructionNew",
    "SetSessionInstructionAdvance",
    "SetSessionInstructionDuplicate",
    "SetSessionInstructionStale",
    "SetSessionInstructionConflict",
    "SetSessionInstructionOwnedByMob",
    "SetSessionInstructionSlotLimitExceeded",
    "SetSessionInstructionUnknownSlot",
    "RebindInstructionOwner",
    "ResolveRunStartInstructionsApply",
    "ResolveRunStartInstructionsNoChange",
    "ResolveRunStartInstructionsDeferredLeadingPrefix",
    "ResolveRunStartInstructionsDeferredUnavailable",
    "AuthorizeInstructionFoldAtRunStart",
    "AuthorizeInstructionFoldEffective",
];

const INVARIANTS: [&str; 4] = [
    "instruction_state_maps_lockstep",
    "instruction_owner_present_with_target",
    "instruction_slot_count_bounded",
    "instruction_entry_sequence_covers_effective",
];

fn session() -> SessionId {
    SessionId::from("s")
}

fn apply(authority: &mut Authority, input: Input) -> Transition {
    SessionDocumentMachineMutator::apply(authority, input)
        .expect("the input must resolve to exactly one instruction arm")
}

fn try_apply(authority: &mut Authority, input: Input) -> Result<Transition, TransitionError> {
    SessionDocumentMachineMutator::apply(authority, input)
}

/// The facts a shell seeds from its durable projection before an operation.
#[derive(Debug, Clone, Copy)]
struct Seed {
    owner: InstructionTargetOwner,
    target_generation: u64,
    target_slot_count: u64,
    effective_active_count: u64,
    entry_sequence: u64,
}

impl Seed {
    const EMPTY: Self = Self {
        owner: InstructionTargetOwner::Unowned,
        target_generation: 0,
        target_slot_count: 0,
        effective_active_count: 0,
        entry_sequence: 0,
    };

    fn session_owned(target_generation: u64, target_slot_count: u64) -> Self {
        Self {
            owner: InstructionTargetOwner::Session,
            target_generation,
            target_slot_count,
            ..Self::EMPTY
        }
    }

    fn input(self) -> Input {
        Input::RecoverSessionInstructionState {
            session_id: session(),
            owner: self.owner,
            target_generation: self.target_generation,
            target_slot_count: self.target_slot_count,
            effective_active_count: self.effective_active_count,
            entry_sequence: self.entry_sequence,
        }
    }
}

fn seeded(seed: Seed) -> Authority {
    let mut authority = Authority::new();
    let transition = apply(&mut authority, seed.input());
    assert_eq!(
        transition.effects(),
        [Effect::SessionInstructionStateRecovered].as_slice()
    );
    authority
}

fn record(generation: u64, slot_count: u64) -> Input {
    Input::RecordInstructionTarget {
        session_id: session(),
        generation,
        slot_count,
    }
}

fn set(
    revision: u64,
    effect: InstructionEffect,
    slot_present: bool,
    current_revision: u64,
    current_matches: bool,
) -> Input {
    Input::SetSessionInstruction {
        session_id: session(),
        revision,
        effect,
        slot_present,
        current_revision,
        current_matches,
    }
}

fn rebind(owner: InstructionTargetOwner) -> Input {
    Input::RebindInstructionOwner {
        session_id: session(),
        owner,
    }
}

fn resolve(
    run_start: InstructionRunStart,
    lowering: InstructionLowering,
    transcript_is_system_prefix: bool,
    pending_change_count: u64,
    target_active_count: u64,
) -> Input {
    Input::ResolveRunStartInstructions {
        session_id: session(),
        run_start,
        lowering,
        transcript_is_system_prefix,
        pending_change_count,
        target_active_count,
    }
}

fn fold(at_run_start: bool, lowering: InstructionLowering, target_active_count: u64) -> Input {
    Input::AuthorizeInstructionFold {
        session_id: session(),
        at_run_start,
        lowering,
        target_active_count,
    }
}

fn owner(authority: &Authority) -> Option<InstructionTargetOwner> {
    authority
        .state()
        .session_instruction_owner
        .get(&session())
        .copied()
}

fn generation(authority: &Authority) -> Option<u64> {
    authority
        .state()
        .session_instruction_target_generation
        .get(&session())
        .copied()
}

fn slot_count(authority: &Authority) -> Option<u64> {
    authority
        .state()
        .session_instruction_target_slot_count
        .get(&session())
        .copied()
}

fn effective_count(authority: &Authority) -> Option<u64> {
    authority
        .state()
        .session_instruction_effective_active_count
        .get(&session())
        .copied()
}

fn entry_sequence(authority: &Authority) -> Option<u64> {
    authority
        .state()
        .session_instruction_entry_sequence
        .get(&session())
        .copied()
}

fn recorded(generation: u64, outcome: InstructionWriteOutcome) -> Vec<Effect> {
    vec![Effect::InstructionTargetRecorded {
        generation,
        outcome,
    }]
}

fn refused(refusal: InstructionWriteRefusal) -> Vec<Effect> {
    vec![Effect::InstructionTargetRefused { refusal }]
}

/// Apply `input` and require it to change nothing but emit `effects`.
fn assert_verdict_only(authority: &mut Authority, input: Input, effects: &[Effect]) {
    let before = authority.state().clone();
    let transition = apply(authority, input);
    assert_eq!(transition.effects(), effects);
    assert_eq!(
        authority.state(),
        &before,
        "a verdict-only arm changes no state"
    );
}

// ---------------------------------------------------------------------------
// RecoverSessionInstructionState
// ---------------------------------------------------------------------------

#[test]
fn recover_adopts_a_consistent_projection() {
    let authority = seeded(Seed {
        owner: InstructionTargetOwner::Mob,
        target_generation: 3,
        target_slot_count: 2,
        effective_active_count: 1,
        entry_sequence: 2,
    });
    assert_eq!(owner(&authority), Some(InstructionTargetOwner::Mob));
    assert_eq!(generation(&authority), Some(3));
    assert_eq!(slot_count(&authority), Some(2));
    assert_eq!(effective_count(&authority), Some(1));
    assert_eq!(entry_sequence(&authority), Some(2));

    // A session no target was ever recorded for seeds empty, and an
    // effective set its own entries put there is legal without a target.
    let authority = seeded(Seed {
        effective_active_count: 2,
        entry_sequence: 4,
        ..Seed::EMPTY
    });
    assert_eq!(owner(&authority), Some(InstructionTargetOwner::Unowned));
    assert_eq!(effective_count(&authority), Some(2));
}

#[test]
fn recover_refuses_a_torn_projection() {
    for (case, seed) in [
        (
            "a target without an owner",
            Seed {
                target_slot_count: 1,
                ..Seed::EMPTY
            },
        ),
        (
            "a target generation without an owner",
            Seed {
                target_generation: 1,
                ..Seed::EMPTY
            },
        ),
        (
            "more target slots than the limit",
            Seed::session_owned(1, 65),
        ),
        (
            "an effective set no entry put there",
            Seed {
                effective_active_count: 1,
                ..Seed::EMPTY
            },
        ),
        (
            "more effective slots than the limit",
            Seed {
                effective_active_count: 65,
                entry_sequence: 1,
                ..Seed::EMPTY
            },
        ),
    ] {
        let mut authority = Authority::new();
        let result = try_apply(&mut authority, seed.input());
        assert!(
            result.is_err(),
            "{case}: a torn projection must not seed: {result:?}"
        );
        assert!(
            authority.state().session_instruction_owner.is_empty(),
            "{case}: a refused seed records nothing"
        );
    }
}

#[test]
fn an_unseeded_session_is_refused_by_every_instruction_input() {
    let mut authority = Authority::new();
    for input in [
        record(1, 1),
        set(1, InstructionEffect::Active, false, 0, false),
        rebind(InstructionTargetOwner::Session),
        resolve(
            InstructionRunStart::FreshRun,
            InstructionLowering::MidConversation,
            false,
            1,
            1,
        ),
        resolve(
            InstructionRunStart::ResumeRun,
            InstructionLowering::MidConversation,
            false,
            0,
            0,
        ),
        fold(true, InstructionLowering::MidConversation, 1),
        fold(false, InstructionLowering::Unavailable, 0),
    ] {
        let result = try_apply(&mut authority, input.clone());
        assert!(result.is_err(), "{input:?} must refuse an unseeded session");
    }
    assert!(authority.state().session_instruction_owner.is_empty());
    assert!(
        authority
            .state()
            .session_instruction_entry_sequence
            .is_empty()
    );
}

// ---------------------------------------------------------------------------
// RecordInstructionTarget (a mob delivers a whole member snapshot)
// ---------------------------------------------------------------------------

#[test]
fn record_target_advance_takes_mob_ownership() {
    let mut authority = seeded(Seed::EMPTY);
    let transition = apply(&mut authority, record(2, 3));
    assert_eq!(
        transition.effects(),
        recorded(2, InstructionWriteOutcome::Recorded).as_slice()
    );
    assert_eq!(owner(&authority), Some(InstructionTargetOwner::Mob));
    assert_eq!(generation(&authority), Some(2));
    assert_eq!(slot_count(&authority), Some(3));

    // A later generation replaces the whole snapshot.
    let transition = apply(&mut authority, record(5, 1));
    assert_eq!(
        transition.effects(),
        recorded(5, InstructionWriteOutcome::Recorded).as_slice()
    );
    assert_eq!(generation(&authority), Some(5));
    assert_eq!(slot_count(&authority), Some(1));
    assert_eq!(
        effective_count(&authority),
        Some(0),
        "recording applies nothing"
    );
    assert_eq!(
        entry_sequence(&authority),
        Some(0),
        "recording mints no entry"
    );
}

#[test]
fn record_target_at_or_below_the_recorded_generation_is_already_recorded() {
    let mut authority = seeded(Seed {
        owner: InstructionTargetOwner::Mob,
        target_generation: 4,
        target_slot_count: 2,
        ..Seed::EMPTY
    });
    for repeated in [4, 3, 1] {
        assert_verdict_only(
            &mut authority,
            record(repeated, 7),
            &recorded(4, InstructionWriteOutcome::Duplicate),
        );
    }
}

#[test]
fn record_target_refuses_a_session_owned_target() {
    let mut authority = seeded(Seed::session_owned(2, 1));
    assert_verdict_only(
        &mut authority,
        record(9, 1),
        &refused(InstructionWriteRefusal::OwnedBySession),
    );
}

// ---------------------------------------------------------------------------
// SetSessionInstruction (the session's own host writes one slot)
// ---------------------------------------------------------------------------

#[test]
fn session_write_of_a_new_slot_records_it_and_takes_session_ownership() {
    let mut authority = seeded(Seed::EMPTY);
    let transition = apply(
        &mut authority,
        set(1, InstructionEffect::Active, false, 0, false),
    );
    assert_eq!(
        transition.effects(),
        recorded(1, InstructionWriteOutcome::Recorded).as_slice()
    );
    assert_eq!(owner(&authority), Some(InstructionTargetOwner::Session));
    assert_eq!(generation(&authority), Some(1));
    assert_eq!(slot_count(&authority), Some(1));

    // A second new slot counts, and every record advances the generation.
    let transition = apply(
        &mut authority,
        set(7, InstructionEffect::Active, false, 0, false),
    );
    assert_eq!(
        transition.effects(),
        recorded(2, InstructionWriteOutcome::Recorded).as_slice()
    );
    assert_eq!(slot_count(&authority), Some(2));
}

#[test]
fn session_write_of_a_higher_revision_supersedes() {
    let mut authority = seeded(Seed::session_owned(1, 1));
    let transition = apply(
        &mut authority,
        set(2, InstructionEffect::Active, true, 1, false),
    );
    assert_eq!(
        transition.effects(),
        recorded(2, InstructionWriteOutcome::Recorded).as_slice()
    );
    // A revoke is a revision too.
    let transition = apply(
        &mut authority,
        set(3, InstructionEffect::Absent, true, 2, false),
    );
    assert_eq!(
        transition.effects(),
        recorded(3, InstructionWriteOutcome::Recorded).as_slice()
    );
    assert_eq!(
        slot_count(&authority),
        Some(1),
        "a revision keeps the slot count"
    );
    assert_eq!(owner(&authority), Some(InstructionTargetOwner::Session));
}

#[test]
fn session_write_of_the_same_revision_and_content_is_a_duplicate() {
    let mut authority = seeded(Seed::session_owned(4, 1));
    assert_verdict_only(
        &mut authority,
        set(2, InstructionEffect::Active, true, 2, true),
        &recorded(4, InstructionWriteOutcome::Duplicate),
    );
}

#[test]
fn session_write_of_a_lower_revision_is_refused_stale() {
    let mut authority = seeded(Seed::session_owned(4, 1));
    for current_matches in [false, true] {
        assert_verdict_only(
            &mut authority,
            set(1, InstructionEffect::Active, true, 2, current_matches),
            &refused(InstructionWriteRefusal::StaleRevision),
        );
    }
}

#[test]
fn session_write_of_the_same_revision_with_other_content_is_refused_conflict() {
    let mut authority = seeded(Seed::session_owned(4, 1));
    for effect in [InstructionEffect::Active, InstructionEffect::Absent] {
        assert_verdict_only(
            &mut authority,
            set(2, effect, true, 2, false),
            &refused(InstructionWriteRefusal::RevisionConflict),
        );
    }
}

#[test]
fn session_write_on_a_mob_owned_target_is_refused() {
    let mut authority = seeded(Seed {
        owner: InstructionTargetOwner::Mob,
        target_generation: 3,
        target_slot_count: 1,
        ..Seed::EMPTY
    });
    for input in [
        set(1, InstructionEffect::Active, false, 0, false),
        set(5, InstructionEffect::Active, true, 1, false),
        set(5, InstructionEffect::Absent, true, 1, false),
    ] {
        assert_verdict_only(
            &mut authority,
            input,
            &refused(InstructionWriteRefusal::OwnedByMob),
        );
    }
}

#[test]
fn session_write_of_a_new_slot_past_the_limit_is_refused() {
    let mut authority = seeded(Seed::session_owned(9, 64));
    assert_verdict_only(
        &mut authority,
        set(1, InstructionEffect::Active, false, 0, false),
        &refused(InstructionWriteRefusal::SlotLimitExceeded),
    );
    // A slot already recorded still takes a new revision at the limit.
    let transition = apply(
        &mut authority,
        set(2, InstructionEffect::Active, true, 1, false),
    );
    assert_eq!(
        transition.effects(),
        recorded(10, InstructionWriteOutcome::Recorded).as_slice()
    );
    assert_eq!(slot_count(&authority), Some(64));
}

#[test]
fn session_revoke_of_an_unknown_slot_is_refused() {
    for seed in [Seed::EMPTY, Seed::session_owned(2, 1)] {
        let mut authority = seeded(seed);
        assert_verdict_only(
            &mut authority,
            set(1, InstructionEffect::Absent, false, 0, false),
            &refused(InstructionWriteRefusal::UnknownSlot),
        );
    }
}

// ---------------------------------------------------------------------------
// RebindInstructionOwner
// ---------------------------------------------------------------------------

#[test]
fn rebind_starts_the_new_owner_from_the_effective_set() {
    let mut authority = seeded(Seed {
        owner: InstructionTargetOwner::Mob,
        target_generation: 7,
        target_slot_count: 3,
        effective_active_count: 2,
        entry_sequence: 4,
    });
    let transition = apply(&mut authority, rebind(InstructionTargetOwner::Session));
    assert_eq!(
        transition.effects(),
        [Effect::InstructionOwnerRebound {
            owner: InstructionTargetOwner::Session
        }]
        .as_slice()
    );
    assert_eq!(owner(&authority), Some(InstructionTargetOwner::Session));
    assert_eq!(generation(&authority), Some(0));
    assert_eq!(
        slot_count(&authority),
        Some(2),
        "the target is the effective set"
    );
    assert_eq!(
        effective_count(&authority),
        Some(2),
        "a rebind applies nothing"
    );
    assert_eq!(
        entry_sequence(&authority),
        Some(4),
        "a rebind mints no entry"
    );

    // Rebinding to the owner the target already has, or to no owner, is
    // refused and changes nothing.
    let before = authority.state().clone();
    for to in [
        InstructionTargetOwner::Session,
        InstructionTargetOwner::Unowned,
    ] {
        assert!(try_apply(&mut authority, rebind(to)).is_err(), "{to:?}");
    }
    assert_eq!(authority.state(), &before);
}

// ---------------------------------------------------------------------------
// ResolveRunStartInstructions
// ---------------------------------------------------------------------------

#[test]
fn fresh_run_start_applies_the_pending_changes_in_one_entry() {
    let seed = Seed {
        owner: InstructionTargetOwner::Session,
        target_generation: 3,
        target_slot_count: 2,
        effective_active_count: 0,
        entry_sequence: 0,
    };
    let mut authority = seeded(seed);
    let transition = apply(
        &mut authority,
        resolve(
            InstructionRunStart::FreshRun,
            InstructionLowering::MidConversation,
            false,
            3,
            2,
        ),
    );
    assert_eq!(
        transition.effects(),
        [Effect::InstructionEntryAuthorized {
            entry_sequence: 1,
            generation: 3
        }]
        .as_slice()
    );
    assert_eq!(entry_sequence(&authority), Some(1));
    assert_eq!(
        effective_count(&authority),
        Some(2),
        "the target is now effective"
    );
    assert_eq!(generation(&authority), Some(3), "applying records nothing");

    // A model that lowers System rows only as a leading prefix applies while
    // the transcript is still one.
    let mut authority = seeded(seed);
    let transition = apply(
        &mut authority,
        resolve(
            InstructionRunStart::FreshRun,
            InstructionLowering::LeadingPrefixOnly,
            true,
            1,
            2,
        ),
    );
    assert_eq!(
        transition.effects(),
        [Effect::InstructionEntryAuthorized {
            entry_sequence: 1,
            generation: 3
        }]
        .as_slice()
    );
}

fn pending_seed() -> Seed {
    Seed {
        owner: InstructionTargetOwner::Session,
        target_generation: 2,
        target_slot_count: 2,
        effective_active_count: 1,
        entry_sequence: 1,
    }
}

#[test]
fn resume_run_start_never_applies() {
    let mut authority = seeded(pending_seed());
    for lowering in [
        InstructionLowering::MidConversation,
        InstructionLowering::LeadingPrefixOnly,
    ] {
        assert_verdict_only(
            &mut authority,
            resolve(InstructionRunStart::ResumeRun, lowering, true, 2, 2),
            &[Effect::InstructionEntryNotNeeded],
        );
    }
}

#[test]
fn no_pending_boundary_never_applies() {
    let mut authority = seeded(pending_seed());
    assert_verdict_only(
        &mut authority,
        resolve(
            InstructionRunStart::NoRun,
            InstructionLowering::MidConversation,
            true,
            2,
            2,
        ),
        &[Effect::InstructionEntryNotNeeded],
    );
}

#[test]
fn fresh_run_without_pending_changes_needs_no_entry() {
    let mut authority = seeded(pending_seed());
    assert_verdict_only(
        &mut authority,
        resolve(
            InstructionRunStart::FreshRun,
            InstructionLowering::MidConversation,
            false,
            0,
            1,
        ),
        &[Effect::InstructionEntryNotNeeded],
    );
}

#[test]
fn leading_prefix_only_lowering_defers_after_conversation_rows() {
    let mut authority = seeded(pending_seed());
    assert_verdict_only(
        &mut authority,
        resolve(
            InstructionRunStart::FreshRun,
            InstructionLowering::LeadingPrefixOnly,
            false,
            1,
            2,
        ),
        &[Effect::InstructionApplicationDeferred {
            reason: InstructionDeferReason::LeadingPrefixOnly,
        }],
    );
}

#[test]
fn unavailable_lowering_defers() {
    let mut authority = seeded(pending_seed());
    for transcript_is_system_prefix in [true, false] {
        assert_verdict_only(
            &mut authority,
            resolve(
                InstructionRunStart::FreshRun,
                InstructionLowering::Unavailable,
                transcript_is_system_prefix,
                1,
                2,
            ),
            &[Effect::InstructionApplicationDeferred {
                reason: InstructionDeferReason::LoweringUnavailable,
            }],
        );
    }
}

#[test]
fn an_application_naming_more_active_slots_than_the_target_holds_is_refused() {
    let mut authority = seeded(pending_seed());
    let before = authority.state().clone();
    assert!(
        try_apply(
            &mut authority,
            resolve(
                InstructionRunStart::FreshRun,
                InstructionLowering::MidConversation,
                false,
                1,
                3,
            ),
        )
        .is_err()
    );
    assert_eq!(authority.state(), &before);
}

// ---------------------------------------------------------------------------
// AuthorizeInstructionFold
// ---------------------------------------------------------------------------

#[test]
fn run_start_fold_applies_the_target() {
    for lowering in [
        InstructionLowering::MidConversation,
        InstructionLowering::LeadingPrefixOnly,
    ] {
        let mut authority = seeded(Seed {
            owner: InstructionTargetOwner::Session,
            target_generation: 2,
            target_slot_count: 2,
            effective_active_count: 1,
            entry_sequence: 3,
        });
        let transition = apply(&mut authority, fold(true, lowering, 2));
        assert_eq!(
            transition.effects(),
            [Effect::InstructionFoldAuthorized {
                entry_sequence: 4,
                source: InstructionFoldSource::Target
            }]
            .as_slice(),
            "{lowering:?}"
        );
        assert_eq!(effective_count(&authority), Some(2));
        assert_eq!(entry_sequence(&authority), Some(4));
    }
}

#[test]
fn other_folds_keep_the_effective_set() {
    for (at_run_start, lowering) in [
        (false, InstructionLowering::MidConversation),
        (false, InstructionLowering::LeadingPrefixOnly),
        (false, InstructionLowering::Unavailable),
        (true, InstructionLowering::Unavailable),
    ] {
        let mut authority = seeded(Seed {
            owner: InstructionTargetOwner::Session,
            target_generation: 2,
            target_slot_count: 2,
            effective_active_count: 1,
            entry_sequence: 3,
        });
        let transition = apply(&mut authority, fold(at_run_start, lowering, 2));
        assert_eq!(
            transition.effects(),
            [Effect::InstructionFoldAuthorized {
                entry_sequence: 4,
                source: InstructionFoldSource::Effective
            }]
            .as_slice(),
            "{at_run_start} {lowering:?}"
        );
        assert_eq!(
            effective_count(&authority),
            Some(1),
            "the effective set stays"
        );
        assert_eq!(entry_sequence(&authority), Some(4));
        assert_eq!(generation(&authority), Some(2));
    }
}

// ---------------------------------------------------------------------------
// Invariants
// ---------------------------------------------------------------------------

fn consistent_state() -> State {
    seeded(Seed {
        owner: InstructionTargetOwner::Session,
        target_generation: 2,
        target_slot_count: 1,
        effective_active_count: 1,
        entry_sequence: 1,
    })
    .state()
    .clone()
}

fn rejected_invariant(state: State) -> &'static str {
    match Authority::recover_from_state(state) {
        Err(TransitionError::RecoveredStateInvariantRejected { invariant, .. }) => invariant,
        Err(other) => panic!("unexpected refusal {other}"),
        Ok(_) => panic!("a torn state must not recover"),
    }
}

#[test]
fn every_reached_instruction_state_recovers() {
    let mut authority = seeded(Seed::EMPTY);
    for input in [
        set(1, InstructionEffect::Active, false, 0, false),
        resolve(
            InstructionRunStart::FreshRun,
            InstructionLowering::MidConversation,
            false,
            1,
            1,
        ),
        set(2, InstructionEffect::Absent, true, 1, false),
        fold(false, InstructionLowering::MidConversation, 0),
        rebind(InstructionTargetOwner::Mob),
        record(1, 2),
        fold(true, InstructionLowering::LeadingPrefixOnly, 1),
    ] {
        apply(&mut authority, input);
        let state = authority.state().clone();
        let recovered = Authority::recover_from_state(state.clone())
            .unwrap_or_else(|error| panic!("a reached state recovers: {error}"));
        assert_eq!(recovered.state(), &state);
    }
}

#[test]
fn recovered_state_rejects_instruction_maps_out_of_lockstep() {
    let mut state = consistent_state();
    state.session_instruction_target_generation.clear();
    assert_eq!(rejected_invariant(state), "instruction_state_maps_lockstep");
}

#[test]
fn recovered_state_rejects_a_target_without_an_owner() {
    let mut state = consistent_state();
    state
        .session_instruction_owner
        .insert(session(), InstructionTargetOwner::Unowned);
    assert_eq!(
        rejected_invariant(state),
        "instruction_owner_present_with_target"
    );
}

#[test]
fn recovered_state_rejects_more_slots_than_the_limit() {
    let mut state = consistent_state();
    state
        .session_instruction_target_slot_count
        .insert(session(), 65);
    assert_eq!(rejected_invariant(state), "instruction_slot_count_bounded");
}

#[test]
fn recovered_state_rejects_an_effective_set_without_an_entry() {
    let mut state = consistent_state();
    state
        .session_instruction_entry_sequence
        .insert(session(), 0);
    assert_eq!(
        rejected_invariant(state),
        "instruction_entry_sequence_covers_effective"
    );
}

// ---------------------------------------------------------------------------
// Schema census
// ---------------------------------------------------------------------------

#[test]
fn the_instruction_region_declares_every_arm_and_invariant() {
    let schema = dsl_session_document_machine();
    for arm in ARMS {
        assert!(
            schema
                .transitions
                .iter()
                .any(|transition| transition.name.as_str() == arm),
            "SessionDocumentMachine must declare {arm}"
        );
    }
    for invariant in INVARIANTS {
        assert!(
            schema
                .invariants
                .iter()
                .any(|declared| declared.name == invariant),
            "SessionDocumentMachine must declare invariant {invariant}"
        );
    }
    let manifest = canonical_machine_coverage_manifests()
        .into_iter()
        .find(|manifest| manifest.machine.as_str() == "SessionDocumentMachine")
        .expect("a SessionDocumentMachine coverage manifest");
    for arm in ARMS {
        assert!(
            manifest
                .transition_coverage
                .iter()
                .any(|entry| entry.name == arm && !entry.scenario_ids.is_empty()),
            "a coverage scenario claims {arm}"
        );
    }
    for invariant in INVARIANTS {
        assert!(
            manifest
                .invariant_coverage
                .iter()
                .any(|entry| entry.name == invariant && !entry.scenario_ids.is_empty()),
            "a coverage scenario claims {invariant}"
        );
    }
}

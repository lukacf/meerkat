//! Rows for the SessionDocumentMachine operator-instruction region.
//!
//! One row per transition arm, the region's invariants (through
//! `recover_from_state`), and the schema census the coverage manifest and the
//! instruction boundary TLC audit read. The machine holds every
//! operator-instruction fact it decides from, per session and per slot: the
//! owner and the mob a mob owner is, the target generation, the recorded
//! target and the slots in effect (revision, effect, digest and body bytes
//! per slot), and the entry-sequence high-water. The shell recovers that
//! state, feeds request and transcript facts, and persists what the machine
//! holds afterwards.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::BTreeMap;

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

const ARMS: [&str; 30] = [
    "RecoverSessionInstructionState",
    "RecoverSessionInstructionTarget",
    "RecoverSessionInstructionEffective",
    "RecordInstructionTargetAdvance",
    "RecordInstructionTargetActiveBudgetExceeded",
    "RecordInstructionTargetSlotLimitExceeded",
    "RecordInstructionTargetAlreadyRecorded",
    "RecordInstructionTargetOwnedBySession",
    "RecordInstructionTargetOwnedByOtherMob",
    "SetSessionInstructionNew",
    "SetSessionInstructionAdvance",
    "SetSessionInstructionActiveBudgetExceeded",
    "SetSessionInstructionDuplicate",
    "SetSessionInstructionStale",
    "SetSessionInstructionConflict",
    "SetSessionInstructionOwnedByMob",
    "SetSessionInstructionSlotLimitExceeded",
    "SetSessionInstructionUnknownSlot",
    "RebindInstructionOwner",
    "RebindInstructionOwnerUnowned",
    "ReleaseInstructionTarget",
    "ReleaseInstructionTargetAlreadyUnowned",
    "ReleaseInstructionTargetOwnedByOtherMob",
    "ReleaseInstructionTargetOwnedBySession",
    "ResolveRunStartInstructionsApply",
    "ResolveRunStartInstructionsNoChange",
    "ResolveRunStartInstructionsDeferredLeadingPrefix",
    "ResolveRunStartInstructionsDeferredUnavailable",
    "AuthorizeInstructionFoldAtRunStart",
    "AuthorizeInstructionFoldEffective",
];

const INVARIANTS: [&str; 6] = [
    "instruction_state_maps_lockstep",
    "instruction_owner_present_with_target",
    "instruction_slot_count_bounded",
    "instruction_active_bytes_bounded",
    "instruction_entry_sequence_covers_effective",
    "instruction_mob_owner_names_its_mob",
];

/// The active budget: 1 MiB of Active bodies per target.
const ACTIVE_BUDGET: u64 = 1024 * 1024;

const DIGEST_X: &str = "sha256:1111111111111111111111111111111111111111111111111111111111111111";
const DIGEST_Y: &str = "sha256:2222222222222222222222222222222222222222222222222222222222222222";
const DIGEST_Z: &str = "sha256:3333333333333333333333333333333333333333333333333333333333333333";

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

/// One slot set (a recorded target, the slots in effect or a mob snapshot)
/// as the machine holds it: revision, effect, digest and body bytes per
/// slot key.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Slots {
    revision: BTreeMap<String, u64>,
    effect: BTreeMap<String, InstructionEffect>,
    digest: BTreeMap<String, String>,
    bytes: BTreeMap<String, u64>,
}

impl Slots {
    fn empty() -> Self {
        Self::default()
    }

    fn with(
        mut self,
        slot: &str,
        revision: u64,
        effect: InstructionEffect,
        digest: &str,
        bytes: u64,
    ) -> Self {
        self.revision.insert(slot.to_string(), revision);
        self.effect.insert(slot.to_string(), effect);
        self.digest.insert(slot.to_string(), digest.to_string());
        self.bytes.insert(slot.to_string(), bytes);
        self
    }

    fn active(self, slot: &str, revision: u64, digest: &str, bytes: u64) -> Self {
        self.with(slot, revision, InstructionEffect::Active, digest, bytes)
    }

    fn absent(self, slot: &str, revision: u64) -> Self {
        self.with(slot, revision, InstructionEffect::Absent, "", 0)
    }

    /// `count` Active slots `k00`.. of one byte each.
    fn many(count: usize) -> Self {
        (0..count).fold(Self::empty(), |slots, index| {
            slots.active(&format!("k{index:02}"), 1, DIGEST_X, 1)
        })
    }
}

/// What the shell recovers before an operation, in the machine's three
/// recovery steps.
#[derive(Debug, Clone)]
struct Seed {
    owner: InstructionTargetOwner,
    owner_mob: &'static str,
    target_generation: u64,
    /// The recorded high-water.
    entry_sequence: u64,
    target: Slots,
    effective: Slots,
    /// The target generation the session's last entry row realized.
    effective_generation: u64,
    /// The sequence of the session's last entry row.
    last_entry_sequence: u64,
}

impl Seed {
    fn empty() -> Self {
        Self {
            owner: InstructionTargetOwner::Unowned,
            owner_mob: "",
            target_generation: 0,
            entry_sequence: 0,
            target: Slots::empty(),
            effective: Slots::empty(),
            effective_generation: 0,
            last_entry_sequence: 0,
        }
    }

    fn session_owned(target_generation: u64, target: Slots) -> Self {
        Self {
            owner: InstructionTargetOwner::Session,
            target_generation,
            target,
            ..Self::empty()
        }
    }

    fn mob_owned(mob: &'static str, target_generation: u64, target: Slots) -> Self {
        Self {
            owner: InstructionTargetOwner::Mob,
            owner_mob: mob,
            target_generation,
            target,
            ..Self::empty()
        }
    }

    /// Slots in effect that the last entry row, at `entry_sequence` and
    /// realizing generation 1, put there; the recorded high-water is that
    /// entry's sequence.
    fn in_effect(mut self, entry_sequence: u64, effective: Slots) -> Self {
        self.entry_sequence = entry_sequence;
        self.last_entry_sequence = entry_sequence;
        self.effective_generation = 1;
        self.effective = effective;
        self
    }

    fn inputs(&self) -> [Input; 3] {
        [
            Input::RecoverSessionInstructionState {
                session_id: session(),
                owner: self.owner,
                owner_mob: self.owner_mob.to_string(),
                target_generation: self.target_generation,
                entry_sequence: self.entry_sequence,
            },
            Input::RecoverSessionInstructionTarget {
                session_id: session(),
                revision: self.target.revision.clone(),
                effect: self.target.effect.clone(),
                digest: self.target.digest.clone(),
                bytes: self.target.bytes.clone(),
            },
            Input::RecoverSessionInstructionEffective {
                session_id: session(),
                revision: self.effective.revision.clone(),
                effect: self.effective.effect.clone(),
                digest: self.effective.digest.clone(),
                bytes: self.effective.bytes.clone(),
                generation: self.effective_generation,
                last_entry_sequence: self.last_entry_sequence,
            },
        ]
    }
}

fn seeded(seed: &Seed) -> Authority {
    let mut authority = Authority::new();
    for input in seed.inputs() {
        let transition = apply(&mut authority, input);
        assert_eq!(
            transition.effects(),
            [Effect::SessionInstructionStateRecovered].as_slice()
        );
    }
    authority
}

fn record(mob: &str, generation: u64, slots: &Slots) -> Input {
    Input::RecordInstructionTarget {
        session_id: session(),
        mob_id: mob.to_string(),
        generation,
        revision: slots.revision.clone(),
        effect: slots.effect.clone(),
        digest: slots.digest.clone(),
        bytes: slots.bytes.clone(),
    }
}

fn activate(slot: &str, revision: u64, digest: &str, body_bytes: u64) -> Input {
    Input::SetSessionInstruction {
        session_id: session(),
        slot: slot.to_string(),
        revision,
        effect: InstructionEffect::Active,
        digest: digest.to_string(),
        body_bytes,
    }
}

fn revoke(slot: &str, revision: u64) -> Input {
    Input::SetSessionInstruction {
        session_id: session(),
        slot: slot.to_string(),
        revision,
        effect: InstructionEffect::Absent,
        digest: String::new(),
        body_bytes: 0,
    }
}

fn rebind(owner: InstructionTargetOwner, mob: &str) -> Input {
    Input::RebindInstructionOwner {
        session_id: session(),
        owner,
        mob_id: mob.to_string(),
    }
}

fn release(mob: &str) -> Input {
    Input::ReleaseInstructionTarget {
        session_id: session(),
        mob_id: mob.to_string(),
    }
}

fn resolve(
    run_start: InstructionRunStart,
    lowering: InstructionLowering,
    transcript_is_system_prefix: bool,
) -> Input {
    Input::ResolveRunStartInstructions {
        session_id: session(),
        run_start,
        lowering,
        transcript_is_system_prefix,
    }
}

fn fresh_run(lowering: InstructionLowering, transcript_is_system_prefix: bool) -> Input {
    resolve(
        InstructionRunStart::FreshRun,
        lowering,
        transcript_is_system_prefix,
    )
}

fn fold(at_run_start: bool, lowering: InstructionLowering) -> Input {
    Input::AuthorizeInstructionFold {
        session_id: session(),
        at_run_start,
        lowering,
    }
}

fn owner(authority: &Authority) -> Option<InstructionTargetOwner> {
    authority
        .state()
        .session_instruction_owner
        .get(&session())
        .copied()
}

fn owner_mob(authority: &Authority) -> Option<String> {
    authority
        .state()
        .session_instruction_owner_mob
        .get(&session())
        .cloned()
}

fn generation(authority: &Authority) -> Option<u64> {
    authority
        .state()
        .session_instruction_target_generation
        .get(&session())
        .copied()
}

fn effective_generation(authority: &Authority) -> Option<u64> {
    authority
        .state()
        .session_instruction_effective_generation
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

fn target(authority: &Authority) -> Slots {
    let state = authority.state();
    Slots {
        revision: state.session_instruction_target_revision[&session()].clone(),
        effect: state.session_instruction_target_effect[&session()].clone(),
        digest: state.session_instruction_target_digest[&session()].clone(),
        bytes: state.session_instruction_target_bytes[&session()].clone(),
    }
}

fn effective(authority: &Authority) -> Slots {
    let state = authority.state();
    Slots {
        revision: state.session_instruction_effective_revision[&session()].clone(),
        effect: state.session_instruction_effective_effect[&session()].clone(),
        digest: state.session_instruction_effective_digest[&session()].clone(),
        bytes: state.session_instruction_effective_bytes[&session()].clone(),
    }
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

/// Apply `input` and require that no arm takes it and nothing changes.
fn assert_refused_by_every_arm(authority: &mut Authority, input: Input, case: &str) {
    let before = authority.state().clone();
    let result = try_apply(authority, input);
    assert!(result.is_err(), "{case}: no arm may take it: {result:?}");
    assert_eq!(authority.state(), &before, "{case}: nothing changes");
}

// ---------------------------------------------------------------------------
// Recovery
// ---------------------------------------------------------------------------

#[test]
fn recover_adopts_a_consistent_projection() {
    let projected = Slots::empty()
        .active("tone", 1, DIGEST_X, 9)
        .absent("zone", 2);
    let in_effect = Slots::empty().active("tone", 1, DIGEST_X, 9);
    let authority =
        seeded(&Seed::mob_owned("mob-a", 3, projected.clone()).in_effect(2, in_effect.clone()));
    assert_eq!(owner(&authority), Some(InstructionTargetOwner::Mob));
    assert_eq!(owner_mob(&authority).as_deref(), Some("mob-a"));
    assert_eq!(generation(&authority), Some(3));
    assert_eq!(entry_sequence(&authority), Some(2));
    assert_eq!(target(&authority), projected);
    assert_eq!(effective(&authority), in_effect);

    // A session no target was ever recorded for recovers empty, and slots in
    // effect its own entries put there are legal without a target.
    let in_effect = Slots::empty().active("tone", 4, DIGEST_Y, 3);
    let authority = seeded(&Seed::empty().in_effect(4, in_effect.clone()));
    assert_eq!(owner(&authority), Some(InstructionTargetOwner::Unowned));
    assert_eq!(owner_mob(&authority).as_deref(), Some(""));
    assert_eq!(target(&authority), Slots::empty());
    assert_eq!(effective(&authority), in_effect);
}

#[test]
fn recovering_the_owner_empties_both_slot_sets() {
    let mut authority = seeded(
        &Seed::session_owned(2, Slots::empty().active("tone", 1, DIGEST_X, 1))
            .in_effect(1, Slots::empty().active("tone", 1, DIGEST_X, 1)),
    );
    let [state, _, _] = Seed::session_owned(5, Slots::empty()).inputs();
    apply(&mut authority, state);
    assert_eq!(generation(&authority), Some(5));
    assert_eq!(target(&authority), Slots::empty());
    assert_eq!(effective(&authority), Slots::empty());
}

#[test]
fn recovering_what_is_in_effect_takes_the_later_entry_sequence() {
    let in_effect = Slots::empty().active("tone", 3, DIGEST_X, 5);
    for (recorded, last_row, expected) in [(2, 5, 5), (5, 2, 5), (4, 4, 4), (0, 3, 3)] {
        let seed = Seed {
            entry_sequence: recorded,
            last_entry_sequence: last_row,
            effective_generation: 7,
            effective: in_effect.clone(),
            ..Seed::session_owned(1, Slots::empty())
        };
        let authority = seeded(&seed);
        assert_eq!(
            entry_sequence(&authority),
            Some(expected),
            "recorded {recorded}, last row {last_row}"
        );
        assert_eq!(effective_generation(&authority), Some(7));
    }
}

#[test]
fn recover_refuses_a_torn_projection() {
    let over_budget = Slots::empty()
        .active("a", 1, DIGEST_X, ACTIVE_BUDGET)
        .active("b", 1, DIGEST_Y, 1);
    let mut out_of_lockstep = Slots::empty().active("a", 1, DIGEST_X, 1);
    out_of_lockstep.bytes.clear();
    for (case, seed, failing_step) in [
        (
            "a target generation without an owner",
            Seed {
                target_generation: 1,
                ..Seed::empty()
            },
            0,
        ),
        (
            "a mob owner that names no mob",
            Seed::mob_owned("", 1, Slots::empty()),
            0,
        ),
        (
            "a session owner that names a mob",
            Seed {
                owner_mob: "mob-a",
                ..Seed::session_owned(1, Slots::empty())
            },
            0,
        ),
        (
            "a target without an owner",
            Seed {
                target: Slots::empty().active("a", 1, DIGEST_X, 1),
                ..Seed::empty()
            },
            1,
        ),
        (
            "a target whose maps name different slots",
            Seed::session_owned(1, out_of_lockstep),
            1,
        ),
        (
            "an Active slot without a digest",
            Seed::session_owned(1, Slots::empty().active("a", 1, "", 1)),
            1,
        ),
        (
            "an Absent slot with a digest",
            Seed::session_owned(
                1,
                Slots::empty().with("a", 1, InstructionEffect::Absent, DIGEST_X, 0),
            ),
            1,
        ),
        (
            "an Absent slot with body bytes",
            Seed::session_owned(
                1,
                Slots::empty().with("a", 1, InstructionEffect::Absent, "", 4),
            ),
            1,
        ),
        (
            "a revision of zero",
            Seed::session_owned(1, Slots::empty().active("a", 0, DIGEST_X, 1)),
            1,
        ),
        (
            "more target slots than the limit",
            Seed::session_owned(1, Slots::many(65)),
            1,
        ),
        (
            "a target over the active budget",
            Seed::session_owned(1, over_budget.clone()),
            1,
        ),
        (
            "slots in effect that no entry put there",
            Seed::empty().in_effect(0, Slots::empty().active("a", 1, DIGEST_X, 1)),
            2,
        ),
        (
            "slots in effect over the active budget",
            Seed::empty().in_effect(1, over_budget),
            2,
        ),
    ] {
        let mut authority = Authority::new();
        for (step, input) in seed.inputs().into_iter().enumerate() {
            if step < failing_step {
                apply(&mut authority, input);
            } else {
                assert_refused_by_every_arm(&mut authority, input, case);
                break;
            }
        }
    }
}

#[test]
fn an_unrecovered_session_is_refused_by_every_instruction_input() {
    let mut authority = Authority::new();
    let slots = Slots::empty().active("a", 1, DIGEST_X, 1);
    let [_, target, effective] = Seed::session_owned(1, slots.clone())
        .in_effect(1, slots.clone())
        .inputs();
    for input in [
        target,
        effective,
        record("mob-a", 1, &slots),
        activate("a", 1, DIGEST_X, 1),
        revoke("a", 2),
        rebind(InstructionTargetOwner::Session, ""),
        release("mob-a"),
        fresh_run(InstructionLowering::MidConversation, false),
        resolve(
            InstructionRunStart::ResumeRun,
            InstructionLowering::MidConversation,
            false,
        ),
        fold(true, InstructionLowering::MidConversation),
        fold(false, InstructionLowering::Unavailable),
    ] {
        let case = format!("{input:?}");
        assert_refused_by_every_arm(&mut authority, input, &case);
    }
    assert!(authority.state().session_instruction_owner.is_empty());
}

// ---------------------------------------------------------------------------
// RecordInstructionTarget (a mob delivers a whole member snapshot)
// ---------------------------------------------------------------------------

#[test]
fn record_target_advance_takes_ownership_for_its_mob() {
    let mut authority = seeded(&Seed::empty());
    let snapshot = Slots::empty()
        .active("tone", 2, DIGEST_X, 9)
        .absent("zone", 1);
    let transition = apply(&mut authority, record("mob-a", 2, &snapshot));
    assert_eq!(
        transition.effects(),
        recorded(2, InstructionWriteOutcome::Recorded).as_slice()
    );
    assert_eq!(owner(&authority), Some(InstructionTargetOwner::Mob));
    assert_eq!(owner_mob(&authority).as_deref(), Some("mob-a"));
    assert_eq!(generation(&authority), Some(2));
    assert_eq!(target(&authority), snapshot);

    // A later generation from the same mob replaces the whole snapshot;
    // recording applies nothing and mints no entry.
    let snapshot = Slots::empty().active("style", 1, DIGEST_Y, 4);
    let transition = apply(&mut authority, record("mob-a", 5, &snapshot));
    assert_eq!(
        transition.effects(),
        recorded(5, InstructionWriteOutcome::Recorded).as_slice()
    );
    assert_eq!(target(&authority), snapshot);
    assert_eq!(
        effective(&authority),
        Slots::empty(),
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
    let mut authority = seeded(&Seed::mob_owned(
        "mob-a",
        4,
        Slots::empty().active("tone", 1, DIGEST_X, 1),
    ));
    for repeated in [4, 3, 1] {
        assert_verdict_only(
            &mut authority,
            record("mob-a", repeated, &Slots::many(3)),
            &recorded(4, InstructionWriteOutcome::Duplicate),
        );
    }
}

#[test]
fn record_target_refuses_a_session_owned_target() {
    let mut authority = seeded(&Seed::session_owned(
        2,
        Slots::empty().active("tone", 1, DIGEST_X, 1),
    ));
    assert_verdict_only(
        &mut authority,
        record("mob-a", 9, &Slots::empty()),
        &refused(InstructionWriteRefusal::OwnedBySession),
    );
}

#[test]
fn a_record_from_another_mob_is_refused_until_the_owning_mob_releases_the_target() {
    let in_effect = Slots::empty().active("tone", 1, DIGEST_X, 9);
    let mut authority =
        seeded(&Seed::mob_owned("mob-a", 3, in_effect.clone()).in_effect(1, in_effect.clone()));
    let other = Slots::empty().active("tone", 7, DIGEST_Y, 2);
    for generation in [1, 3, 9] {
        assert_verdict_only(
            &mut authority,
            record("mob-b", generation, &other),
            &refused(InstructionWriteRefusal::OwnedByOtherMob),
        );
    }
    assert_verdict_only(
        &mut authority,
        release("mob-b"),
        &refused(InstructionWriteRefusal::OwnedByOtherMob),
    );

    // The owning mob releases: the target empties and is unowned, and the
    // release applies nothing by itself. The verdict says the target was
    // owned.
    let transition = apply(&mut authority, release("mob-a"));
    assert_eq!(
        transition.effects(),
        [Effect::InstructionTargetReleased { was_owned: true }].as_slice()
    );
    assert_eq!(owner(&authority), Some(InstructionTargetOwner::Unowned));
    assert_eq!(owner_mob(&authority).as_deref(), Some(""));
    assert_eq!(generation(&authority), Some(0));
    assert_eq!(target(&authority), Slots::empty());
    assert_eq!(
        effective(&authority),
        in_effect,
        "a release applies nothing by itself"
    );
    assert_eq!(entry_sequence(&authority), Some(1));

    // Only now may another mob bind, and it starts from an empty target.
    let transition = apply(&mut authority, record("mob-b", 1, &other));
    assert_eq!(
        transition.effects(),
        recorded(1, InstructionWriteOutcome::Recorded).as_slice()
    );
    assert_eq!(owner_mob(&authority).as_deref(), Some("mob-b"));
    assert_eq!(target(&authority), other);
}

#[test]
fn release_empties_the_target_and_the_next_fresh_run_start_revokes_what_was_in_effect() {
    let in_effect = Slots::empty()
        .active("tone", 1, DIGEST_X, 9)
        .active("style", 2, DIGEST_Y, 4);
    for (lowering, transcript_is_system_prefix) in [
        (InstructionLowering::MidConversation, false),
        (InstructionLowering::LeadingPrefixOnly, true),
    ] {
        let mut authority =
            seeded(&Seed::mob_owned("mob-a", 3, in_effect.clone()).in_effect(1, in_effect.clone()));
        apply(&mut authority, release("mob-a"));
        assert_eq!(
            target(&authority),
            Slots::empty(),
            "a release empties the target"
        );
        let transition = apply(
            &mut authority,
            fresh_run(lowering, transcript_is_system_prefix),
        );
        assert_eq!(
            transition.effects(),
            [Effect::InstructionEntryAuthorized {
                entry_sequence: 2,
                generation: 0
            }]
            .as_slice(),
            "{lowering:?}: the released mob's instructions are revoked"
        );
        assert_eq!(
            effective(&authority),
            Slots::empty(),
            "{lowering:?}: nothing of the released mob stays in effect"
        );
        assert_verdict_only(
            &mut authority,
            fresh_run(lowering, transcript_is_system_prefix),
            &[Effect::InstructionEntryNotNeeded],
        );
    }

    // A prefix-only model after conversation rows defers the revoke, and a
    // compaction at the next run's first request folds the empty target.
    let mut authority =
        seeded(&Seed::mob_owned("mob-a", 3, in_effect.clone()).in_effect(1, in_effect.clone()));
    apply(&mut authority, release("mob-a"));
    assert_verdict_only(
        &mut authority,
        fresh_run(InstructionLowering::LeadingPrefixOnly, false),
        &[Effect::InstructionApplicationDeferred {
            reason: InstructionDeferReason::LeadingPrefixOnly,
        }],
    );
    let transition = apply(
        &mut authority,
        fold(true, InstructionLowering::LeadingPrefixOnly),
    );
    assert_eq!(
        transition.effects(),
        [Effect::InstructionFoldAuthorized {
            entry_sequence: 2,
            source: InstructionFoldSource::Target,
            generation: 0
        }]
        .as_slice()
    );
    assert_eq!(effective(&authority), Slots::empty());
}

#[test]
fn record_target_over_the_active_budget_is_refused() {
    let mut authority = seeded(&Seed::empty());
    let over = Slots::empty()
        .active("a", 1, DIGEST_X, ACTIVE_BUDGET - 1)
        .active("b", 1, DIGEST_Y, 2);
    assert_verdict_only(
        &mut authority,
        record("mob-a", 1, &over),
        &refused(InstructionWriteRefusal::ActiveSetBudgetExceeded),
    );
    // Exactly the budget is recorded; Absent slots count nothing.
    let exact = Slots::empty()
        .active("a", 1, DIGEST_X, ACTIVE_BUDGET - 1)
        .active("b", 1, DIGEST_Y, 1)
        .absent("c", 1);
    let transition = apply(&mut authority, record("mob-a", 1, &exact));
    assert_eq!(
        transition.effects(),
        recorded(1, InstructionWriteOutcome::Recorded).as_slice()
    );
}

#[test]
fn record_target_over_the_slot_limit_is_refused() {
    // A snapshot over the slot limit, into an unowned target or from the
    // owning mob, records nothing, and the slot limit is decided before the
    // active budget.
    let over_both = Slots::many(64).active("big", 1, DIGEST_Y, ACTIVE_BUDGET);
    for (case, seed, snapshot) in [
        ("an unowned target", Seed::empty(), Slots::many(65)),
        (
            "the owning mob's target",
            Seed::mob_owned("mob-a", 2, Slots::many(3)),
            Slots::many(65),
        ),
        ("a snapshot over the budget too", Seed::empty(), over_both),
    ] {
        let mut authority = seeded(&seed);
        let before = authority.state().clone();
        let transition = apply(&mut authority, record("mob-a", 3, &snapshot));
        assert_eq!(
            transition.effects(),
            refused(InstructionWriteRefusal::SlotLimitExceeded).as_slice(),
            "{case}"
        );
        assert_eq!(authority.state(), &before, "{case}: nothing is recorded");
    }
    // Another mob's target is refused for its owner first.
    let mut authority = seeded(&Seed::mob_owned("mob-a", 2, Slots::many(3)));
    assert_verdict_only(
        &mut authority,
        record("mob-b", 3, &Slots::many(65)),
        &refused(InstructionWriteRefusal::OwnedByOtherMob),
    );
    // Exactly the limit is recorded.
    let mut authority = seeded(&Seed::empty());
    let transition = apply(&mut authority, record("mob-a", 1, &Slots::many(64)));
    assert_eq!(
        transition.effects(),
        recorded(1, InstructionWriteOutcome::Recorded).as_slice()
    );
    assert_eq!(target(&authority).revision.len(), 64);
}

#[test]
fn a_malformed_snapshot_is_taken_by_no_arm() {
    let mut authority = seeded(&Seed::empty());
    let mut torn = Slots::empty().active("a", 1, DIGEST_X, 1);
    torn.digest.clear();
    for (case, input) in [
        ("a snapshot without a mob", record("", 1, &Slots::empty())),
        (
            "a snapshot whose maps name different slots",
            record("mob-a", 1, &torn),
        ),
    ] {
        assert_refused_by_every_arm(&mut authority, input, case);
    }
}

// ---------------------------------------------------------------------------
// SetSessionInstruction (the session's own host writes one slot)
// ---------------------------------------------------------------------------

#[test]
fn session_write_of_a_new_slot_records_it_and_takes_session_ownership() {
    let mut authority = seeded(&Seed::empty());
    let transition = apply(&mut authority, activate("tone", 1, DIGEST_X, 5));
    assert_eq!(
        transition.effects(),
        recorded(1, InstructionWriteOutcome::Recorded).as_slice()
    );
    assert_eq!(owner(&authority), Some(InstructionTargetOwner::Session));
    assert_eq!(generation(&authority), Some(1));
    assert_eq!(
        target(&authority),
        Slots::empty().active("tone", 1, DIGEST_X, 5)
    );

    // A second new slot joins, and every record advances the generation.
    let transition = apply(&mut authority, activate("style", 7, DIGEST_Y, 3));
    assert_eq!(
        transition.effects(),
        recorded(2, InstructionWriteOutcome::Recorded).as_slice()
    );
    assert_eq!(
        target(&authority),
        Slots::empty()
            .active("tone", 1, DIGEST_X, 5)
            .active("style", 7, DIGEST_Y, 3)
    );
    assert_eq!(
        effective(&authority),
        Slots::empty(),
        "writing applies nothing"
    );
}

#[test]
fn session_write_of_a_higher_revision_supersedes() {
    let mut authority = seeded(&Seed::session_owned(
        1,
        Slots::empty().active("tone", 1, DIGEST_X, 5),
    ));
    let transition = apply(&mut authority, activate("tone", 2, DIGEST_Y, 8));
    assert_eq!(
        transition.effects(),
        recorded(2, InstructionWriteOutcome::Recorded).as_slice()
    );
    assert_eq!(
        target(&authority),
        Slots::empty().active("tone", 2, DIGEST_Y, 8)
    );
    // A revoke is a revision too: Absent, no digest, no body bytes.
    let transition = apply(&mut authority, revoke("tone", 3));
    assert_eq!(
        transition.effects(),
        recorded(3, InstructionWriteOutcome::Recorded).as_slice()
    );
    assert_eq!(target(&authority), Slots::empty().absent("tone", 3));
    assert_eq!(owner(&authority), Some(InstructionTargetOwner::Session));
}

#[test]
fn session_write_of_the_same_revision_and_content_is_a_duplicate() {
    let mut authority = seeded(&Seed::session_owned(
        4,
        Slots::empty()
            .active("tone", 2, DIGEST_X, 5)
            .absent("zone", 3),
    ));
    assert_verdict_only(
        &mut authority,
        activate("tone", 2, DIGEST_X, 5),
        &recorded(4, InstructionWriteOutcome::Duplicate),
    );
    assert_verdict_only(
        &mut authority,
        revoke("zone", 3),
        &recorded(4, InstructionWriteOutcome::Duplicate),
    );
}

#[test]
fn session_write_of_a_lower_revision_is_refused_stale() {
    let mut authority = seeded(&Seed::session_owned(
        4,
        Slots::empty().active("tone", 2, DIGEST_X, 5),
    ));
    for input in [
        activate("tone", 1, DIGEST_X, 5),
        activate("tone", 1, DIGEST_Y, 1),
        revoke("tone", 1),
    ] {
        assert_verdict_only(
            &mut authority,
            input,
            &refused(InstructionWriteRefusal::StaleRevision),
        );
    }
}

#[test]
fn session_write_of_the_same_revision_with_other_content_is_refused_conflict() {
    let mut authority = seeded(&Seed::session_owned(
        4,
        Slots::empty().active("tone", 2, DIGEST_X, 5),
    ));
    for input in [activate("tone", 2, DIGEST_Y, 5), revoke("tone", 2)] {
        assert_verdict_only(
            &mut authority,
            input,
            &refused(InstructionWriteRefusal::RevisionConflict),
        );
    }
}

#[test]
fn session_write_on_a_mob_owned_target_is_refused() {
    let mut authority = seeded(&Seed::mob_owned(
        "mob-a",
        3,
        Slots::empty().active("tone", 1, DIGEST_X, 5),
    ));
    for input in [
        activate("style", 1, DIGEST_Y, 1),
        activate("tone", 5, DIGEST_Y, 1),
        revoke("tone", 5),
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
    let mut authority = seeded(&Seed::session_owned(9, Slots::many(64)));
    assert_verdict_only(
        &mut authority,
        activate("one-more", 1, DIGEST_Y, 1),
        &refused(InstructionWriteRefusal::SlotLimitExceeded),
    );
    // A slot already recorded still takes a new revision at the limit.
    let transition = apply(&mut authority, activate("k00", 2, DIGEST_Y, 1));
    assert_eq!(
        transition.effects(),
        recorded(10, InstructionWriteOutcome::Recorded).as_slice()
    );
    assert_eq!(target(&authority).revision.len(), 64);
}

#[test]
fn session_revoke_of_an_unknown_slot_is_refused() {
    for seed in [
        Seed::empty(),
        Seed::session_owned(2, Slots::empty().active("tone", 1, DIGEST_X, 1)),
    ] {
        let mut authority = seeded(&seed);
        assert_verdict_only(
            &mut authority,
            revoke("style", 1),
            &refused(InstructionWriteRefusal::UnknownSlot),
        );
    }
}

#[test]
fn session_write_over_the_active_budget_is_refused_before_anything_is_recorded() {
    let mut authority = seeded(&Seed::session_owned(
        2,
        Slots::empty().active("a", 1, DIGEST_X, ACTIVE_BUDGET - 4),
    ));
    // A new slot or a higher revision that would exceed the budget.
    assert_verdict_only(
        &mut authority,
        activate("b", 1, DIGEST_Y, 5),
        &refused(InstructionWriteRefusal::ActiveSetBudgetExceeded),
    );
    assert_verdict_only(
        &mut authority,
        activate("a", 2, DIGEST_Y, ACTIVE_BUDGET + 1),
        &refused(InstructionWriteRefusal::ActiveSetBudgetExceeded),
    );
    // Exactly the budget is recorded, and a replacement counts only the new
    // body.
    let transition = apply(&mut authority, activate("b", 1, DIGEST_Y, 4));
    assert_eq!(
        transition.effects(),
        recorded(3, InstructionWriteOutcome::Recorded).as_slice()
    );
    let transition = apply(
        &mut authority,
        activate("a", 2, DIGEST_Z, ACTIVE_BUDGET - 4),
    );
    assert_eq!(
        transition.effects(),
        recorded(4, InstructionWriteOutcome::Recorded).as_slice()
    );
    // A revoke always fits.
    let transition = apply(&mut authority, revoke("a", 3));
    assert_eq!(
        transition.effects(),
        recorded(5, InstructionWriteOutcome::Recorded).as_slice()
    );
}

// ---------------------------------------------------------------------------
// RebindInstructionOwner and ReleaseInstructionTarget
// ---------------------------------------------------------------------------

#[test]
fn rebind_starts_the_new_owner_from_the_slots_in_effect() {
    let in_effect = Slots::empty().active("tone", 2, DIGEST_X, 5);
    let mut authority = seeded(
        &Seed::session_owned(
            7,
            Slots::empty()
                .active("tone", 3, DIGEST_Y, 6)
                .active("style", 1, DIGEST_Z, 2),
        )
        .in_effect(4, in_effect.clone()),
    );
    let transition = apply(&mut authority, rebind(InstructionTargetOwner::Mob, "mob-a"));
    assert_eq!(
        transition.effects(),
        [Effect::InstructionOwnerRebound {
            owner: InstructionTargetOwner::Mob
        }]
        .as_slice()
    );
    assert_eq!(owner(&authority), Some(InstructionTargetOwner::Mob));
    assert_eq!(owner_mob(&authority).as_deref(), Some("mob-a"));
    assert_eq!(generation(&authority), Some(0));
    assert_eq!(
        target(&authority),
        in_effect,
        "no pending write of the previous owner carries over"
    );
    assert_eq!(effective(&authority), in_effect, "a rebind applies nothing");
    assert_eq!(
        entry_sequence(&authority),
        Some(4),
        "a rebind mints no entry"
    );
    assert_verdict_only(
        &mut authority,
        fresh_run(InstructionLowering::MidConversation, false),
        &[Effect::InstructionEntryNotNeeded],
    );
}

#[test]
fn rebind_takes_an_unowned_target_and_a_session_target_only_for_a_mob() {
    // An unowned target binds a session owner (a host's standalone
    // adoption) or a mob (an attach). The new owner starts from an empty
    // target: whatever is still in effect (a released mob's instructions)
    // is revoked at the next fresh run start.
    let in_effect = Slots::empty().active("tone", 1, DIGEST_X, 5);
    for (to, mob) in [
        (InstructionTargetOwner::Session, ""),
        (InstructionTargetOwner::Mob, "mob-a"),
    ] {
        let mut authority = seeded(&Seed::empty().in_effect(2, in_effect.clone()));
        let transition = apply(&mut authority, rebind(to, mob));
        assert_eq!(
            transition.effects(),
            [Effect::InstructionOwnerRebound { owner: to }].as_slice()
        );
        assert_eq!(owner(&authority), Some(to));
        assert_eq!(owner_mob(&authority).as_deref(), Some(mob));
        assert_eq!(generation(&authority), Some(0));
        assert_eq!(
            target(&authority),
            Slots::empty(),
            "{to:?}: an empty target"
        );
        assert_eq!(effective(&authority), in_effect, "a rebind applies nothing");
        let transition = apply(
            &mut authority,
            fresh_run(InstructionLowering::MidConversation, false),
        );
        assert_eq!(
            transition.effects(),
            [Effect::InstructionEntryAuthorized {
                entry_sequence: 3,
                generation: 0
            }]
            .as_slice()
        );
        assert_eq!(effective(&authority), Slots::empty());
    }
    for (case, seed, to, mob) in [
        (
            "a mob-owned target to the session",
            Seed::mob_owned("mob-a", 2, Slots::empty()),
            InstructionTargetOwner::Session,
            "",
        ),
        (
            "a mob-owned target to another mob",
            Seed::mob_owned("mob-a", 2, Slots::empty()),
            InstructionTargetOwner::Mob,
            "mob-b",
        ),
        (
            "a mob-owned target to its own mob",
            Seed::mob_owned("mob-a", 2, Slots::empty()),
            InstructionTargetOwner::Mob,
            "mob-a",
        ),
        (
            "a session-owned target to the session",
            Seed::session_owned(2, Slots::empty()),
            InstructionTargetOwner::Session,
            "",
        ),
        (
            "to no owner",
            Seed::empty(),
            InstructionTargetOwner::Unowned,
            "",
        ),
        (
            "to a mob that names no mob",
            Seed::empty(),
            InstructionTargetOwner::Mob,
            "",
        ),
        (
            "to a session owner that names a mob",
            Seed::empty(),
            InstructionTargetOwner::Session,
            "mob-a",
        ),
    ] {
        let mut authority = seeded(&seed);
        assert_refused_by_every_arm(&mut authority, rebind(to, mob), case);
    }
}

#[test]
fn only_the_owning_mob_releases_a_target() {
    // A target no owner holds answers "already unowned": the verdict says
    // it was not owned, and nothing changes.
    let mut authority = seeded(&Seed::empty());
    assert_verdict_only(
        &mut authority,
        release("mob-a"),
        &[Effect::InstructionTargetReleased { was_owned: false }],
    );
    let mut authority = seeded(&Seed::session_owned(
        2,
        Slots::empty().active("tone", 1, DIGEST_X, 1),
    ));
    assert_verdict_only(
        &mut authority,
        release("mob-a"),
        &refused(InstructionWriteRefusal::OwnedBySession),
    );
}

#[test]
fn the_rebind_arms_check_ownership_in_a_named_guard() {
    // The instruction boundary audit's rebind mutant drops exactly the
    // attach's ownership guard, so each rebind arm keeps its ownership check
    // in a guard of its own.
    let schema = dsl_session_document_machine();
    for (arm, guard) in [
        ("RebindInstructionOwner", "session_owned"),
        ("RebindInstructionOwnerUnowned", "unowned"),
    ] {
        let transition = schema
            .transitions
            .iter()
            .find(|transition| transition.name.as_str() == arm)
            .unwrap_or_else(|| panic!("SessionDocumentMachine must declare {arm}"));
        assert!(
            transition
                .guards
                .iter()
                .any(|declared| declared.name == guard),
            "{arm} must check ownership in its guard `{guard}`"
        );
    }
}

// ---------------------------------------------------------------------------
// ResolveRunStartInstructions
// ---------------------------------------------------------------------------

fn pending_seed() -> Seed {
    Seed::session_owned(
        2,
        Slots::empty()
            .active("tone", 2, DIGEST_Y, 6)
            .absent("zone", 2),
    )
    .in_effect(
        1,
        Slots::empty()
            .active("tone", 1, DIGEST_X, 5)
            .active("zone", 1, DIGEST_Z, 3),
    )
}

#[test]
fn fresh_run_start_applies_the_pending_changes_in_one_entry() {
    for (lowering, transcript_is_system_prefix) in [
        (InstructionLowering::MidConversation, false),
        (InstructionLowering::MidConversation, true),
        (InstructionLowering::LeadingPrefixOnly, true),
    ] {
        let seed = pending_seed();
        let mut authority = seeded(&seed);
        let transition = apply(
            &mut authority,
            fresh_run(lowering, transcript_is_system_prefix),
        );
        assert_eq!(
            transition.effects(),
            [Effect::InstructionEntryAuthorized {
                entry_sequence: 2,
                generation: 2
            }]
            .as_slice(),
            "{lowering:?} {transcript_is_system_prefix}"
        );
        assert_eq!(entry_sequence(&authority), Some(2));
        assert_eq!(
            effective(&authority),
            Slots::empty().active("tone", 2, DIGEST_Y, 6),
            "the target's Active slots are now in effect, and its revoked slot is not"
        );
        assert_eq!(effective_generation(&authority), Some(2));
        assert_eq!(target(&authority), seed.target, "applying records nothing");
        assert_eq!(generation(&authority), Some(2));
        // Nothing is pending after the application.
        assert_verdict_only(
            &mut authority,
            fresh_run(lowering, transcript_is_system_prefix),
            &[Effect::InstructionEntryNotNeeded],
        );
    }
}

#[test]
fn the_machine_decides_what_is_pending_from_its_own_slots() {
    let tone = |revision, digest| Slots::empty().active("tone", revision, digest, 5);
    for (case, target, in_effect, pending) in [
        (
            "the target is what is in effect",
            tone(1, DIGEST_X),
            tone(1, DIGEST_X),
            false,
        ),
        (
            "a revoked slot no entry put in effect",
            Slots::empty().absent("tone", 2),
            Slots::empty(),
            false,
        ),
        ("a new Active slot", tone(1, DIGEST_X), Slots::empty(), true),
        (
            "a higher revision",
            tone(2, DIGEST_X),
            tone(1, DIGEST_X),
            true,
        ),
        (
            "other content at the same revision",
            tone(1, DIGEST_Y),
            tone(1, DIGEST_X),
            true,
        ),
        (
            "a revoke of a slot in effect",
            Slots::empty().absent("tone", 2),
            tone(1, DIGEST_X),
            true,
        ),
        (
            "a slot in effect the target no longer names",
            Slots::empty(),
            tone(1, DIGEST_X),
            true,
        ),
    ] {
        let mut authority = seeded(&Seed::session_owned(1, target).in_effect(1, in_effect));
        let transition = apply(
            &mut authority,
            fresh_run(InstructionLowering::MidConversation, false),
        );
        let expected = if pending {
            vec![Effect::InstructionEntryAuthorized {
                entry_sequence: 2,
                generation: 1,
            }]
        } else {
            vec![Effect::InstructionEntryNotNeeded]
        };
        assert_eq!(transition.effects(), expected.as_slice(), "{case}");
    }
}

#[test]
fn resume_and_no_run_starts_never_apply() {
    let mut authority = seeded(&pending_seed());
    for run_start in [InstructionRunStart::ResumeRun, InstructionRunStart::NoRun] {
        for lowering in [
            InstructionLowering::MidConversation,
            InstructionLowering::LeadingPrefixOnly,
        ] {
            assert_verdict_only(
                &mut authority,
                resolve(run_start, lowering, true),
                &[Effect::InstructionEntryNotNeeded],
            );
        }
    }
}

#[test]
fn an_unowned_target_revokes_what_is_in_effect_at_the_next_fresh_run_start() {
    // An unowned target is empty: whatever is still in effect is revoked
    // at the next fresh run start.
    let mut authority =
        seeded(&Seed::empty().in_effect(3, Slots::empty().active("tone", 1, DIGEST_X, 5)));
    let transition = apply(
        &mut authority,
        fresh_run(InstructionLowering::MidConversation, false),
    );
    assert_eq!(
        transition.effects(),
        [Effect::InstructionEntryAuthorized {
            entry_sequence: 4,
            generation: 0
        }]
        .as_slice()
    );
    assert_eq!(effective(&authority), Slots::empty());

    // A session that never had a target has nothing to revoke.
    let mut authority = seeded(&Seed::empty());
    assert_verdict_only(
        &mut authority,
        fresh_run(InstructionLowering::MidConversation, false),
        &[Effect::InstructionEntryNotNeeded],
    );
}

#[test]
fn leading_prefix_only_lowering_defers_after_conversation_rows() {
    let mut authority = seeded(&pending_seed());
    assert_verdict_only(
        &mut authority,
        fresh_run(InstructionLowering::LeadingPrefixOnly, false),
        &[Effect::InstructionApplicationDeferred {
            reason: InstructionDeferReason::LeadingPrefixOnly,
        }],
    );
}

#[test]
fn unavailable_lowering_defers() {
    let mut authority = seeded(&pending_seed());
    for transcript_is_system_prefix in [true, false] {
        assert_verdict_only(
            &mut authority,
            fresh_run(
                InstructionLowering::Unavailable,
                transcript_is_system_prefix,
            ),
            &[Effect::InstructionApplicationDeferred {
                reason: InstructionDeferReason::LoweringUnavailable,
            }],
        );
    }
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
        let seed = pending_seed();
        let mut authority = seeded(&seed);
        let transition = apply(&mut authority, fold(true, lowering));
        assert_eq!(
            transition.effects(),
            [Effect::InstructionFoldAuthorized {
                entry_sequence: 2,
                source: InstructionFoldSource::Target,
                generation: 2
            }]
            .as_slice(),
            "{lowering:?}"
        );
        assert_eq!(
            effective(&authority),
            Slots::empty().active("tone", 2, DIGEST_Y, 6),
            "{lowering:?}: the target's Active slots are now in effect, and its revoked slot is not"
        );
        assert_eq!(effective_generation(&authority), Some(2));
        assert_eq!(entry_sequence(&authority), Some(2));
    }
}

#[test]
fn a_run_start_fold_of_an_unowned_target_revokes_what_is_in_effect() {
    let mut authority =
        seeded(&Seed::empty().in_effect(1, Slots::empty().active("tone", 1, DIGEST_X, 5)));
    let transition = apply(
        &mut authority,
        fold(true, InstructionLowering::MidConversation),
    );
    assert_eq!(
        transition.effects(),
        [Effect::InstructionFoldAuthorized {
            entry_sequence: 2,
            source: InstructionFoldSource::Target,
            generation: 0
        }]
        .as_slice()
    );
    assert_eq!(effective(&authority), Slots::empty());
}

#[test]
fn other_folds_keep_the_slots_in_effect() {
    for (case, seed, at_run_start, lowering) in [
        (
            "mid-run",
            pending_seed(),
            false,
            InstructionLowering::MidConversation,
        ),
        (
            "mid-run on a prefix-only model",
            pending_seed(),
            false,
            InstructionLowering::LeadingPrefixOnly,
        ),
        (
            "mid-run without a lowering",
            pending_seed(),
            false,
            InstructionLowering::Unavailable,
        ),
        (
            "at a run start without a lowering",
            pending_seed(),
            true,
            InstructionLowering::Unavailable,
        ),
    ] {
        let mut authority = seeded(&seed);
        let transition = apply(&mut authority, fold(at_run_start, lowering));
        assert_eq!(
            transition.effects(),
            [Effect::InstructionFoldAuthorized {
                entry_sequence: 2,
                source: InstructionFoldSource::Effective,
                generation: 1
            }]
            .as_slice(),
            "{case}"
        );
        assert_eq!(effective_generation(&authority), Some(1), "{case}");
        assert_eq!(
            effective(&authority),
            seed.effective,
            "{case}: what is in effect stays"
        );
        assert_eq!(target(&authority), seed.target, "{case}");
        assert_eq!(entry_sequence(&authority), Some(2), "{case}");
    }
}

// ---------------------------------------------------------------------------
// The machine and its re-seed
// ---------------------------------------------------------------------------

/// The machine as the shell records it and re-seeds it before its next
/// operation: the owner, the generation, the high-water and the recorded
/// target as held, and the slots in effect as the session's entry rows leave
/// them, where a revoked slot is not in effect, exactly as a slot no entry
/// named. The last entry row's generation and sequence are the machine's
/// own here: the rows that use this mint no entry after an application.
fn reseeded(authority: &Authority) -> Authority {
    let held = effective(authority);
    let mut in_effect = Slots::empty();
    for (slot, effect) in &held.effect {
        if *effect == InstructionEffect::Active {
            in_effect = in_effect.active(
                slot,
                held.revision[slot],
                &held.digest[slot],
                held.bytes[slot],
            );
        }
    }
    let recorded_target = target(authority);
    let high_water = entry_sequence(authority).expect("a recovered session");
    let mut recovered = Authority::new();
    for input in [
        Input::RecoverSessionInstructionState {
            session_id: session(),
            owner: owner(authority).expect("a recovered session"),
            owner_mob: owner_mob(authority).expect("a recovered session"),
            target_generation: generation(authority).expect("a recovered session"),
            entry_sequence: high_water,
        },
        Input::RecoverSessionInstructionTarget {
            session_id: session(),
            revision: recorded_target.revision,
            effect: recorded_target.effect,
            digest: recorded_target.digest,
            bytes: recorded_target.bytes,
        },
        Input::RecoverSessionInstructionEffective {
            session_id: session(),
            revision: in_effect.revision,
            effect: in_effect.effect,
            digest: in_effect.digest,
            bytes: in_effect.bytes,
            generation: effective_generation(authority).expect("a recovered session"),
            last_entry_sequence: high_water,
        },
    ] {
        let transition = apply(&mut recovered, input);
        assert_eq!(
            transition.effects(),
            [Effect::SessionInstructionStateRecovered].as_slice()
        );
    }
    recovered
}

#[test]
fn the_machine_equals_its_reseed_after_an_applied_revoke_through_rebinds_and_a_lower_write() {
    // A session owner activates a slot at revision 5 and revokes it at
    // revision 6, and each change is applied at a fresh run start.
    let mut machine = seeded(&Seed::empty());
    for input in [
        activate("tone", 5, DIGEST_X, 5),
        fresh_run(InstructionLowering::MidConversation, false),
        revoke("tone", 6),
    ] {
        apply(&mut machine, input);
    }
    let transition = apply(
        &mut machine,
        fresh_run(InstructionLowering::MidConversation, false),
    );
    assert_eq!(
        transition.effects(),
        [Effect::InstructionEntryAuthorized {
            entry_sequence: 2,
            generation: 2
        }]
        .as_slice()
    );
    assert_eq!(
        effective(&machine),
        Slots::empty(),
        "an applied revoke leaves the slot out of effect"
    );
    assert_eq!(target(&machine), Slots::empty().absent("tone", 6));

    // The shell re-seeds before every operation; the machine it holds in
    // memory is exactly what a re-seed recovers.
    let mut production = reseeded(&machine);
    assert_eq!(production.state(), machine.state());

    // A mob attaches (the only rebind of a session-owned target) and later
    // releases the target, a host adopts it, and writes the slot at a
    // revision below the revoked one: on both machines every step takes the
    // same arm and leaves the same state, and the write records a new slot.
    for (step, input) in [
        ("an attach", rebind(InstructionTargetOwner::Mob, "mob-a")),
        ("the mob's release", release("mob-a")),
        (
            "a host's adoption",
            rebind(InstructionTargetOwner::Session, ""),
        ),
        ("a lower write", activate("tone", 3, DIGEST_Y, 4)),
    ] {
        let in_memory = apply(&mut machine, input.clone());
        production = reseeded(&production);
        let after_reseed = apply(&mut production, input);
        assert_eq!(in_memory.effects(), after_reseed.effects(), "{step}");
        assert_eq!(production.state(), machine.state(), "{step}");
        assert_eq!(reseeded(&machine).state(), machine.state(), "{step}");
    }
    assert_eq!(
        target(&machine),
        Slots::empty().active("tone", 3, DIGEST_Y, 4)
    );
    assert_eq!(generation(&machine), Some(1));
}

// ---------------------------------------------------------------------------
// Invariants
// ---------------------------------------------------------------------------

fn consistent_state() -> State {
    seeded(&pending_seed()).state().clone()
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
    let mut authority = seeded(&Seed::empty());
    let snapshot = Slots::empty().active("style", 1, DIGEST_Z, 2);
    for input in [
        activate("tone", 1, DIGEST_X, 5),
        fresh_run(InstructionLowering::MidConversation, false),
        revoke("tone", 2),
        fold(false, InstructionLowering::MidConversation),
        rebind(InstructionTargetOwner::Mob, "mob-a"),
        record("mob-a", 1, &snapshot),
        fold(true, InstructionLowering::LeadingPrefixOnly),
        release("mob-a"),
        record("mob-b", 1, &snapshot),
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
    state.session_instruction_target_bytes.clear();
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
    let many = Slots::many(65);
    state
        .session_instruction_target_revision
        .insert(session(), many.revision);
    state
        .session_instruction_target_effect
        .insert(session(), many.effect);
    state
        .session_instruction_target_digest
        .insert(session(), many.digest);
    state
        .session_instruction_target_bytes
        .insert(session(), many.bytes);
    assert_eq!(rejected_invariant(state), "instruction_slot_count_bounded");
}

#[test]
fn recovered_state_rejects_a_target_over_the_active_budget() {
    let mut state = consistent_state();
    state
        .session_instruction_target_bytes
        .get_mut(&session())
        .unwrap()
        .insert("tone".to_string(), ACTIVE_BUDGET + 1);
    assert_eq!(
        rejected_invariant(state),
        "instruction_active_bytes_bounded"
    );
}

#[test]
fn recovered_state_rejects_slots_in_effect_without_an_entry() {
    let mut state = consistent_state();
    state
        .session_instruction_entry_sequence
        .insert(session(), 0);
    assert_eq!(
        rejected_invariant(state),
        "instruction_entry_sequence_covers_effective"
    );
}

#[test]
fn recovered_state_rejects_a_mob_owner_that_names_no_mob() {
    let mut state = consistent_state();
    state
        .session_instruction_owner
        .insert(session(), InstructionTargetOwner::Mob);
    assert_eq!(
        rejected_invariant(state),
        "instruction_mob_owner_names_its_mob"
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
    let declared_arms = schema
        .transitions
        .iter()
        .filter(|transition| {
            let name = transition.name.as_str();
            name.contains("Instruction")
        })
        .count();
    assert_eq!(declared_arms, ARMS.len(), "every instruction arm has a row");
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

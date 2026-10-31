//! Rows for the MobMachine operator-instruction region.
//!
//! The machine owns every operator-instruction fact of a mob: the desired
//! records (one per slot, with the target every member, a profile's members
//! or one member), the generation every committed write and reset advances,
//! and each seated member's delivery facts (owed, recorded, blocked). These
//! rows drive the schema crate's own instantiation of the machine through
//! its signals: writes and their typed verdicts, the owed computation for
//! profile and member targets and retargets, the seat and unseat hooks, the
//! delivery preparation and its feedback, the reset and the high-water, the
//! release verdict, and the region's invariants (through
//! `recover_from_state`).
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use meerkat_machine_schema::catalog::dsl::dsl_mob_machine;
use meerkat_machine_schema::catalog::dsl::mob_machine::{
    AgentIdentity, AgentRuntimeId, FenceToken, Generation, InstructionEffect,
    InstructionTargetKind, MemberInstructionBlock, MemberInstructionDeferReason,
    MemberSessionDisposal, MobMachineAuthority, MobMachineEffect, MobMachineInput,
    MobMachineMutator, MobMachineSignal, MobMachineState, MobMachineTransition,
    MobMachineTransitionError, MobPhase, OperatorInstructionRefusal,
    PlacedCompletionLifecycleIntentKind, SessionId, SpawnPolicyRuntimeMode,
    operator_instruction_target_applies,
};

type Authority = MobMachineAuthority;
type Signal = MobMachineSignal;
type Effect = MobMachineEffect;

const DIGEST_A: &str = "sha256:1111111111111111111111111111111111111111111111111111111111111111";
const DIGEST_B: &str = "sha256:2222222222222222222222222222222222222222222222222222222222222222";
const SLOT: &str = "[\"app.example\",\"tone\"]";
const OTHER_SLOT: &str = "[\"app.example\",\"style\"]";

/// The per-slot body limit and the active budget the machine enforces.
const BODY_LIMIT: u64 = 256 * 1024;
const ACTIVE_BUDGET: u64 = 1024 * 1024;

const ARMS: [&str; 30] = [
    "SetOperatorInstructionBodyTooLarge",
    "SetOperatorInstructionSlotLimitExceeded",
    "SetOperatorInstructionNewOverBudget",
    "SetOperatorInstructionNew",
    "SetOperatorInstructionStale",
    "SetOperatorInstructionDuplicate",
    "SetOperatorInstructionConflict",
    "SetOperatorInstructionAdvanceOverBudget",
    "SetOperatorInstructionAdvance",
    "SetOperatorInstructionTerminal",
    "RevokeOperatorInstructionUnknownSlot",
    "RevokeOperatorInstructionStale",
    "RevokeOperatorInstructionDuplicate",
    "RevokeOperatorInstructionConflict",
    "RevokeOperatorInstructionAdvance",
    "RevokeOperatorInstructionTerminal",
    "RecoverOperatorInstructionApplied",
    "RecoverOperatorInstructionStale",
    "PrepareMemberInstructionDeliveryNotOwed",
    "PrepareMemberInstructionDeliveryPlaced",
    "PrepareMemberInstructionDeliveryUnborn",
    "PrepareMemberInstructionDeliveryUnbound",
    "PrepareMemberInstructionDeliveryRetiring",
    "PrepareMemberInstructionDeliveryDeliver",
    "PrepareMemberInstructionDeliveryMobNotRunning",
    "RecordMemberInstructionTargetCurrent",
    "RecordMemberInstructionTargetSuperseded",
    "RefuseMemberInstructionTargetBlocked",
    "ResolveMemberInstructionReleaseAuthorized",
    "ResolveMemberInstructionReleaseNotNeeded",
];

const INVARIANTS: [&str; 7] = [
    "operator_instruction_records_lockstep",
    "operator_instruction_records_well_formed",
    "operator_instruction_slot_count_bounded",
    "operator_instruction_active_bytes_bounded",
    "member_instruction_facts_only_for_members",
    "member_instruction_generations_bounded",
    "destroyed_mob_holds_no_operator_instructions",
];

fn identity(name: &str) -> AgentIdentity {
    AgentIdentity::from(name)
}

fn session_of(name: &str) -> SessionId {
    SessionId(format!("session-{name}"))
}

fn runtime_of(name: &str) -> AgentRuntimeId {
    AgentRuntimeId(format!("{name}-runtime"))
}

fn signal(authority: &mut Authority, signal: Signal) -> MobMachineTransition {
    authority
        .apply_signal(signal)
        .expect("the signal must resolve to exactly one arm")
}

/// Seat `name` under `profile` the way the epoch replay does, bound to its
/// session (Dormant: a fresh process has no materialization).
fn seat(authority: &mut Authority, name: &str, profile: &str) {
    seat_unbound(authority, name, profile);
    signal(
        authority,
        Signal::RecoverMemberSessionBinding {
            agent_identity: identity(name),
            agent_runtime_id: runtime_of(name),
            bridge_session_id: session_of(name),
            replacing: None,
        },
    );
}

/// Seat `name` under `profile` without binding a session yet.
fn seat_unbound(authority: &mut Authority, name: &str, profile: &str) {
    signal(
        authority,
        Signal::RecoverRosterMember {
            agent_identity: identity(name),
            agent_runtime_id: runtime_of(name),
            fence_token: FenceToken(1),
            generation: Generation(1),
            profile_name: profile.to_string(),
            runtime_mode: SpawnPolicyRuntimeMode::TurnDriven,
            external_addressable: false,
        },
    );
}

/// Unseat `name` the way the replay of its archived retirement does.
fn unseat(authority: &mut Authority, name: &str) {
    signal(
        authority,
        Signal::RecoverRosterMemberRetired {
            agent_identity: identity(name),
            agent_runtime_id: runtime_of(name),
            generation: Generation(1),
            preserve_machine_topology: false,
            preservation_started: false,
            session_archived: true,
            hosted_incarnation: false,
        },
    );
}

fn set(slot: &str, revision: u64, digest: &str, body_bytes: u64, target: Target) -> Signal {
    let (target_kind, target_profile, target_member) = target.parts();
    Signal::SetOperatorInstruction {
        slot: slot.to_string(),
        revision,
        digest: digest.to_string(),
        body_bytes,
        target_kind,
        target_profile,
        target_member,
    }
}

fn revoke(slot: &str, revision: u64) -> Signal {
    Signal::RevokeOperatorInstruction {
        slot: slot.to_string(),
        revision,
    }
}

#[derive(Debug, Clone)]
enum Target {
    Mob,
    Profile(&'static str),
    Member(&'static str),
}

impl Target {
    fn parts(&self) -> (InstructionTargetKind, String, Option<AgentIdentity>) {
        match self {
            Self::Mob => (InstructionTargetKind::Mob, String::new(), None),
            Self::Profile(profile) => {
                (InstructionTargetKind::Profile, (*profile).to_string(), None)
            }
            Self::Member(member) => (
                InstructionTargetKind::Member,
                String::new(),
                Some(identity(member)),
            ),
        }
    }
}

fn prepare(name: &str) -> Signal {
    Signal::PrepareMemberInstructionDelivery {
        agent_identity: identity(name),
    }
}

fn record(name: &str, generation: u64) -> Signal {
    Signal::RecordMemberInstructionTarget {
        agent_identity: identity(name),
        session_id: session_of(name),
        generation,
    }
}

fn refuse(name: &str, generation: u64, block: MemberInstructionBlock) -> Signal {
    Signal::RefuseMemberInstructionTarget {
        agent_identity: identity(name),
        session_id: session_of(name),
        generation,
        block,
    }
}

fn owed(authority: &Authority, name: &str) -> Option<u64> {
    authority
        .state()
        .member_instruction_owed
        .get(&identity(name))
        .copied()
}

fn recorded(authority: &Authority, name: &str) -> Option<u64> {
    authority
        .state()
        .member_instruction_recorded
        .get(&identity(name))
        .copied()
}

fn blocked(authority: &Authority, name: &str) -> Option<MemberInstructionBlock> {
    authority
        .state()
        .member_instruction_blocked
        .get(&identity(name))
        .copied()
}

fn generation(authority: &Authority) -> u64 {
    authority.state().operator_instruction_generation
}

fn committed(slot: &str, revision: u64, effect: InstructionEffect, generation: u64) -> Vec<Effect> {
    vec![Effect::OperatorInstructionCommitted {
        slot: slot.to_string(),
        revision,
        effect,
        generation,
    }]
}

fn refused(slot: &str, refusal: OperatorInstructionRefusal, current: Option<u64>) -> Vec<Effect> {
    vec![Effect::OperatorInstructionRefused {
        slot: slot.to_string(),
        refusal,
        current_revision: current,
    }]
}

/// Apply `input` and require it to change nothing but emit `effects`.
fn assert_verdict_only(authority: &mut Authority, input: Signal, effects: &[Effect]) {
    let before = authority.state().clone();
    let transition = signal(authority, input);
    assert_eq!(transition.effects(), effects);
    assert_eq!(
        authority.state(),
        &before,
        "a verdict-only arm changes no state"
    );
}

/// No effect of the region wakes, materializes or registers a member.
fn assert_wakes_nothing(transition: &MobMachineTransition) {
    for effect in transition.effects() {
        let name = format!("{effect:?}");
        for forbidden in [
            "MaterializeMember",
            "VerifyMember",
            "RequestRuntimeBinding",
            "PublishWakeRoute",
            "MemberWakeClassified",
        ] {
            assert!(
                !name.starts_with(forbidden),
                "an operator-instruction transition emitted {name}"
            );
        }
    }
}

/// A fresh machine taken into `phase` the way a mob takes itself there.
fn authority_in(phase: MobPhase) -> Authority {
    let mut authority = Authority::new();
    let (intent, input) = match phase {
        MobPhase::Running => return authority,
        MobPhase::Stopped => (
            PlacedCompletionLifecycleIntentKind::Stop,
            MobMachineInput::Stop,
        ),
        MobPhase::Completed => (
            PlacedCompletionLifecycleIntentKind::Complete,
            MobMachineInput::Complete,
        ),
        MobPhase::Destroyed => (
            PlacedCompletionLifecycleIntentKind::Destroy,
            MobMachineInput::Destroy,
        ),
    };
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::BeginPlacedCompletionLifecycleQuiesce { intent },
    )
    .expect("begin the lifecycle quiesce");
    MobMachineMutator::apply(&mut authority, input).expect("enter the phase");
    assert_eq!(authority.state().lifecycle_phase, phase);
    authority
}

// ---------------------------------------------------------------------------
// Writes
// ---------------------------------------------------------------------------

/// A write to a profile owes exactly the members of that profile.
#[test]
fn a_profile_write_owes_only_that_profiles_members() {
    let mut authority = Authority::new();
    seat(&mut authority, "alpha-1", "alpha");
    seat(&mut authority, "alpha-2", "alpha");
    seat(&mut authority, "beta-1", "beta");
    let transition = signal(
        &mut authority,
        set(SLOT, 1, DIGEST_A, 10, Target::Profile("alpha")),
    );
    assert_eq!(
        transition.effects(),
        committed(SLOT, 1, InstructionEffect::Active, 1).as_slice()
    );
    assert_wakes_nothing(&transition);
    assert_eq!(generation(&authority), 1);
    assert_eq!(owed(&authority, "alpha-1"), Some(1));
    assert_eq!(owed(&authority, "alpha-2"), Some(1));
    assert_eq!(
        owed(&authority, "beta-1"),
        None,
        "another profile is not owed"
    );
}

/// A member write owes that member only; a mob write owes every member.
#[test]
fn a_member_write_owes_that_member_and_a_mob_write_owes_every_member() {
    let mut authority = Authority::new();
    seat(&mut authority, "alpha-1", "alpha");
    seat(&mut authority, "beta-1", "beta");
    signal(
        &mut authority,
        set(SLOT, 1, DIGEST_A, 10, Target::Member("beta-1")),
    );
    assert_eq!(owed(&authority, "alpha-1"), None);
    assert_eq!(owed(&authority, "beta-1"), Some(1));
    signal(
        &mut authority,
        set(OTHER_SLOT, 1, DIGEST_B, 10, Target::Mob),
    );
    assert_eq!(owed(&authority, "alpha-1"), Some(2));
    assert_eq!(owed(&authority, "beta-1"), Some(2));
}

/// A retarget owes the members that left the selector (their snapshot
/// revokes the slot) and the members that joined it, and no one else.
#[test]
fn a_retarget_owes_members_that_left_and_joined_the_selector() {
    let mut authority = Authority::new();
    seat(&mut authority, "alpha-1", "alpha");
    seat(&mut authority, "beta-1", "beta");
    seat(&mut authority, "gamma-1", "gamma");
    signal(
        &mut authority,
        set(SLOT, 1, DIGEST_A, 10, Target::Profile("alpha")),
    );
    signal(&mut authority, record("alpha-1", 1));
    assert_eq!(recorded(&authority, "alpha-1"), Some(1));
    assert_eq!(owed(&authority, "alpha-1"), None);

    signal(
        &mut authority,
        set(SLOT, 2, DIGEST_A, 10, Target::Profile("beta")),
    );
    assert_eq!(owed(&authority, "alpha-1"), Some(2), "left the selector");
    assert_eq!(owed(&authority, "beta-1"), Some(2), "joined the selector");
    assert_eq!(owed(&authority, "gamma-1"), None, "never selected");
    assert_eq!(
        recorded(&authority, "alpha-1"),
        Some(1),
        "an owed member keeps what its session recorded"
    );
}

/// Stale, duplicate, conflicting and terminal writes answer typed and
/// change nothing; an unknown slot's revoke is refused.
#[test]
fn write_refusals_are_typed_and_change_nothing() {
    let mut authority = Authority::new();
    seat(&mut authority, "alpha-1", "alpha");
    signal(&mut authority, set(SLOT, 2, DIGEST_A, 10, Target::Mob));
    assert_verdict_only(
        &mut authority,
        set(SLOT, 1, DIGEST_A, 10, Target::Mob),
        &refused(SLOT, OperatorInstructionRefusal::StaleRevision, Some(2)),
    );
    assert_verdict_only(
        &mut authority,
        set(SLOT, 2, DIGEST_A, 10, Target::Mob),
        &[Effect::OperatorInstructionDuplicate {
            slot: SLOT.to_string(),
            revision: 2,
            generation: 1,
        }],
    );
    assert_verdict_only(
        &mut authority,
        set(SLOT, 2, DIGEST_B, 10, Target::Mob),
        &refused(SLOT, OperatorInstructionRefusal::RevisionConflict, Some(2)),
    );
    assert_verdict_only(
        &mut authority,
        set(SLOT, 2, DIGEST_A, 10, Target::Profile("alpha")),
        &refused(SLOT, OperatorInstructionRefusal::RevisionConflict, Some(2)),
    );
    assert_verdict_only(
        &mut authority,
        revoke(OTHER_SLOT, 1),
        &refused(OTHER_SLOT, OperatorInstructionRefusal::UnknownSlot, None),
    );
    assert_verdict_only(
        &mut authority,
        revoke(SLOT, 1),
        &refused(SLOT, OperatorInstructionRefusal::StaleRevision, Some(2)),
    );
    assert_verdict_only(
        &mut authority,
        revoke(SLOT, 2),
        &refused(SLOT, OperatorInstructionRefusal::RevisionConflict, Some(2)),
    );
    for phase in [MobPhase::Completed, MobPhase::Destroyed] {
        let mut terminal = authority_in(phase);
        assert_verdict_only(
            &mut terminal,
            set(SLOT, 1, DIGEST_A, 10, Target::Mob),
            &refused(SLOT, OperatorInstructionRefusal::MobTerminal, None),
        );
        assert_verdict_only(
            &mut terminal,
            revoke(SLOT, 1),
            &refused(SLOT, OperatorInstructionRefusal::MobTerminal, None),
        );
    }
}

/// A revoke is a higher revision whose effect is Absent: it keeps the
/// target, owes its members, and repeats as a duplicate.
#[test]
fn a_revoke_owes_the_targets_members_and_repeats_as_a_duplicate() {
    let mut authority = Authority::new();
    seat(&mut authority, "alpha-1", "alpha");
    seat(&mut authority, "beta-1", "beta");
    signal(
        &mut authority,
        set(SLOT, 1, DIGEST_A, 10, Target::Profile("alpha")),
    );
    signal(&mut authority, record("alpha-1", 1));
    let transition = signal(&mut authority, revoke(SLOT, 2));
    assert_eq!(
        transition.effects(),
        committed(SLOT, 2, InstructionEffect::Absent, 2).as_slice()
    );
    let state = authority.state();
    assert_eq!(
        state.operator_instruction_effect.get(SLOT),
        Some(&InstructionEffect::Absent)
    );
    assert_eq!(
        state
            .operator_instruction_digest
            .get(SLOT)
            .map(String::as_str),
        Some("")
    );
    assert_eq!(state.operator_instruction_body_bytes.get(SLOT), Some(&0));
    assert_eq!(
        state.operator_instruction_target_kind.get(SLOT),
        Some(&InstructionTargetKind::Profile),
        "a revoke keeps the target"
    );
    assert_eq!(owed(&authority, "alpha-1"), Some(2));
    assert_eq!(owed(&authority, "beta-1"), None);
    assert_verdict_only(
        &mut authority,
        revoke(SLOT, 2),
        &[Effect::OperatorInstructionDuplicate {
            slot: SLOT.to_string(),
            revision: 2,
            generation: 2,
        }],
    );
}

/// The per-slot body limit, the slot limit and the active budget are the
/// machine's: a write over any of them is refused typed.
#[test]
fn limits_and_the_active_budget_are_refused_typed() {
    let mut authority = Authority::new();
    assert_verdict_only(
        &mut authority,
        set(SLOT, 1, DIGEST_A, BODY_LIMIT + 1, Target::Mob),
        &refused(SLOT, OperatorInstructionRefusal::BodyTooLarge, None),
    );
    // Full bodies fill the active budget exactly.
    for index in 0..ACTIVE_BUDGET / BODY_LIMIT {
        signal(
            &mut authority,
            set(
                &format!("[\"big\",\"k{index}\"]"),
                1,
                DIGEST_A,
                BODY_LIMIT,
                Target::Mob,
            ),
        );
    }
    assert_verdict_only(
        &mut authority,
        set(SLOT, 1, DIGEST_A, 1, Target::Mob),
        &refused(
            SLOT,
            OperatorInstructionRefusal::ActiveSetBudgetExceeded,
            None,
        ),
    );
    // A revoke frees its bytes; the advance of a revoked slot then fits.
    signal(&mut authority, revoke("[\"big\",\"k0\"]", 2));
    signal(&mut authority, set(SLOT, 1, DIGEST_A, 1, Target::Mob));
    assert_verdict_only(
        &mut authority,
        set("[\"big\",\"k0\"]", 3, DIGEST_A, BODY_LIMIT, Target::Mob),
        &refused(
            "[\"big\",\"k0\"]",
            OperatorInstructionRefusal::ActiveSetBudgetExceeded,
            Some(2),
        ),
    );

    let mut many = Authority::new();
    for index in 0..64 {
        signal(
            &mut many,
            set(
                &format!("[\"many\",\"k{index:02}\"]"),
                1,
                DIGEST_A,
                1,
                Target::Mob,
            ),
        );
    }
    assert_verdict_only(
        &mut many,
        set(SLOT, 1, DIGEST_A, 1, Target::Mob),
        &refused(SLOT, OperatorInstructionRefusal::SlotLimitExceeded, None),
    );
}

// ---------------------------------------------------------------------------
// Seats, delivery and feedback
// ---------------------------------------------------------------------------

/// A member seated after a write is owed the current generation; the
/// snapshot its new session carried closes the obligation.
#[test]
fn a_seat_after_a_write_owes_the_current_generation() {
    let mut authority = Authority::new();
    seat(&mut authority, "before", "alpha");
    assert_eq!(
        owed(&authority, "before"),
        None,
        "no generation, no obligation"
    );
    signal(&mut authority, set(SLOT, 1, DIGEST_A, 10, Target::Mob));
    signal(
        &mut authority,
        set(OTHER_SLOT, 1, DIGEST_B, 10, Target::Member("before")),
    );
    seat(&mut authority, "later", "beta");
    assert_eq!(owed(&authority, "later"), Some(2));
    let transition = signal(&mut authority, record("later", 2));
    assert_eq!(
        transition.effects(),
        [Effect::MemberInstructionTargetRecorded {
            agent_identity: identity("later"),
            session_id: session_of("later"),
            generation: 2,
        }]
        .as_slice()
    );
    assert_eq!(owed(&authority, "later"), None);
    assert_eq!(recorded(&authority, "later"), Some(2));
}

/// A dormant member's delivery is prepared for its session at the current
/// generation, and preparing it wakes nothing; a placed member is blocked
/// until its host takes deliveries.
#[test]
fn a_delivery_is_prepared_for_the_bound_session_and_wakes_nothing() {
    let mut authority = Authority::new();
    seat(&mut authority, "dormant", "alpha");
    signal(&mut authority, set(SLOT, 1, DIGEST_A, 10, Target::Mob));
    signal(
        &mut authority,
        set(OTHER_SLOT, 1, DIGEST_B, 10, Target::Profile("beta")),
    );
    let transition = signal(&mut authority, prepare("dormant"));
    assert_eq!(
        transition.effects(),
        [Effect::MemberInstructionDeliveryPrepared {
            agent_identity: identity("dormant"),
            session_id: session_of("dormant"),
            generation: 2,
        }]
        .as_slice()
    );
    assert_wakes_nothing(&transition);
    assert_eq!(
        owed(&authority, "dormant"),
        Some(1),
        "preparing closes nothing"
    );
    assert_verdict_only(
        &mut authority,
        prepare("nobody"),
        &[Effect::MemberInstructionDeliveryDeferred {
            agent_identity: identity("nobody"),
            reason: MemberInstructionDeferReason::NotOwed,
        }],
    );
    signal(&mut authority, record("dormant", 2));
    assert_eq!(recorded(&authority, "dormant"), Some(2));
}

/// An unborn seat is owed but not delivered until its session is born.
#[test]
fn an_unborn_seat_is_owed_and_deferred_until_its_session_is_born() {
    let mut authority = Authority::new();
    signal(&mut authority, set(SLOT, 1, DIGEST_A, 10, Target::Mob));
    seat(&mut authority, "unborn", "alpha");
    signal(
        &mut authority,
        Signal::RecoverMemberSeatSession {
            agent_identity: identity("unborn"),
            agent_runtime_id: runtime_of("unborn"),
            unborn: true,
        },
    );
    assert_eq!(owed(&authority, "unborn"), Some(1));
    assert_verdict_only(
        &mut authority,
        prepare("unborn"),
        &[Effect::MemberInstructionDeliveryDeferred {
            agent_identity: identity("unborn"),
            reason: MemberInstructionDeferReason::SessionUnborn,
        }],
    );
    signal(
        &mut authority,
        Signal::RecoverMemberSeatSession {
            agent_identity: identity("unborn"),
            agent_runtime_id: runtime_of("unborn"),
            unborn: false,
        },
    );
    let transition = signal(&mut authority, prepare("unborn"));
    assert!(matches!(
        transition.effects(),
        [Effect::MemberInstructionDeliveryPrepared { generation: 1, .. }]
    ));
}

/// Feedback closes an obligation only for the bound session at or above the
/// owed generation; a refusal blocks the member until a write selects it.
#[test]
fn feedback_is_fenced_by_session_and_generation_and_a_refusal_blocks() {
    let mut authority = Authority::new();
    seat(&mut authority, "member", "alpha");
    signal(&mut authority, set(SLOT, 1, DIGEST_A, 10, Target::Mob));
    signal(&mut authority, set(SLOT, 2, DIGEST_B, 10, Target::Mob));
    // Another session's record changes nothing.
    assert_verdict_only(
        &mut authority,
        Signal::RecordMemberInstructionTarget {
            agent_identity: identity("member"),
            session_id: session_of("someone-else"),
            generation: 2,
        },
        &[],
    );
    // A record below the owed generation was superseded.
    assert_verdict_only(&mut authority, record("member", 1), &[]);
    // A refusal blocks the member.
    let transition = signal(
        &mut authority,
        refuse("member", 2, MemberInstructionBlock::OwnedByOtherMob),
    );
    assert_eq!(
        transition.effects(),
        [Effect::MemberInstructionTargetBlocked {
            agent_identity: identity("member"),
            generation: 2,
            block: MemberInstructionBlock::OwnedByOtherMob,
        }]
        .as_slice()
    );
    assert_eq!(owed(&authority, "member"), None);
    assert_eq!(
        blocked(&authority, "member"),
        Some(MemberInstructionBlock::OwnedByOtherMob)
    );
    // A write that does not select it keeps it blocked; one that does
    // owes it again.
    signal(
        &mut authority,
        set(OTHER_SLOT, 1, DIGEST_A, 10, Target::Profile("beta")),
    );
    assert_eq!(
        blocked(&authority, "member"),
        Some(MemberInstructionBlock::OwnedByOtherMob)
    );
    signal(&mut authority, set(SLOT, 3, DIGEST_A, 10, Target::Mob));
    assert_eq!(blocked(&authority, "member"), None);
    assert_eq!(owed(&authority, "member"), Some(4));
}

/// An unseat drops the member's delivery facts; a later seat of the same
/// name is a new member for delivery.
#[test]
fn an_unseat_drops_the_members_delivery_facts() {
    let mut authority = Authority::new();
    seat(&mut authority, "member", "alpha");
    seat(&mut authority, "blocked", "alpha");
    signal(&mut authority, set(SLOT, 1, DIGEST_A, 10, Target::Mob));
    signal(&mut authority, record("member", 1));
    signal(
        &mut authority,
        refuse("blocked", 1, MemberInstructionBlock::OwnedBySession),
    );
    unseat(&mut authority, "member");
    unseat(&mut authority, "blocked");
    assert_eq!(recorded(&authority, "member"), None);
    assert_eq!(owed(&authority, "member"), None);
    assert_eq!(blocked(&authority, "blocked"), None);
}

/// The release verdict: only a host-owned session that the mob recorded or
/// owes a snapshot is released.
#[test]
fn a_release_is_authorized_only_for_a_host_owned_session_the_mob_wrote() {
    let mut authority = Authority::new();
    seat(&mut authority, "member", "alpha");
    let release = |disposal| Signal::ResolveMemberInstructionRelease {
        agent_identity: identity("member"),
        disposal,
    };
    let not_needed = [Effect::MemberInstructionReleaseNotNeeded {
        agent_identity: identity("member"),
    }];
    assert_verdict_only(
        &mut authority,
        release(MemberSessionDisposal::RuntimeReleasedOnlyHostOwned),
        &not_needed,
    );
    signal(&mut authority, set(SLOT, 1, DIGEST_A, 10, Target::Mob));
    for disposal in [
        MemberSessionDisposal::Archived,
        MemberSessionDisposal::KeptForSuccessor,
        MemberSessionDisposal::RuntimeReleasedOnlyNoDurableSessions,
    ] {
        assert_verdict_only(&mut authority, release(disposal), &not_needed);
    }
    assert_verdict_only(
        &mut authority,
        release(MemberSessionDisposal::RuntimeReleasedOnlyHostOwned),
        &[Effect::MemberInstructionReleaseAuthorized {
            agent_identity: identity("member"),
            session_id: session_of("member"),
        }],
    );
}

// ---------------------------------------------------------------------------
// Replay, reset and the high-water
// ---------------------------------------------------------------------------

/// The replay of committed records owes what the live writes owed, and the
/// replay of journaled records closes what the live records closed.
#[test]
fn the_replay_restores_the_records_and_the_obligations() {
    let mut authority = Authority::new();
    seat(&mut authority, "alpha-1", "alpha");
    seat(&mut authority, "beta-1", "beta");
    signal(
        &mut authority,
        Signal::RecoverOperatorInstruction {
            slot: SLOT.to_string(),
            revision: 3,
            effect: InstructionEffect::Active,
            digest: DIGEST_A.to_string(),
            body_bytes: 10,
            target_kind: InstructionTargetKind::Profile,
            target_profile: "alpha".to_string(),
            target_member: None,
            generation: 5,
        },
    );
    assert_eq!(generation(&authority), 5);
    assert_eq!(owed(&authority, "alpha-1"), Some(5));
    assert_eq!(owed(&authority, "beta-1"), None);
    signal(&mut authority, record("alpha-1", 5));
    assert_eq!(recorded(&authority, "alpha-1"), Some(5));
    // A record at or below the held generation changes nothing.
    assert_verdict_only(
        &mut authority,
        Signal::RecoverOperatorInstruction {
            slot: OTHER_SLOT.to_string(),
            revision: 1,
            effect: InstructionEffect::Absent,
            digest: String::new(),
            body_bytes: 0,
            target_kind: InstructionTargetKind::Mob,
            target_profile: String::new(),
            target_member: None,
            generation: 5,
        },
        &[],
    );
}

/// A reset clears the records and the delivery facts and moves the
/// generation past the one it had; the high-water recovered before the
/// epoch replay restores it after a restart, and the seal closes that
/// window.
#[test]
fn a_reset_moves_the_generation_on_and_the_highwater_restores_it() {
    let mut authority = Authority::new();
    signal(&mut authority, set(SLOT, 1, DIGEST_A, 10, Target::Mob));
    signal(&mut authority, set(SLOT, 2, DIGEST_B, 10, Target::Mob));
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::BeginPlacedCompletionLifecycleQuiesce {
            intent: PlacedCompletionLifecycleIntentKind::Reset,
        },
    )
    .expect("begin the reset quiesce");
    MobMachineMutator::apply(&mut authority, MobMachineInput::Reset).expect("reset the mob");
    assert_eq!(
        generation(&authority),
        3,
        "the reset moves past the generation"
    );
    assert!(authority.state().operator_instruction_revision.is_empty());
    seat_unbound(&mut authority, "after-reset", "alpha");
    assert_eq!(
        owed(&authority, "after-reset"),
        Some(3),
        "a member seated after the reset is owed the emptied snapshot"
    );
    let transition = signal(&mut authority, set(SLOT, 1, DIGEST_A, 10, Target::Mob));
    assert_eq!(
        transition.effects(),
        committed(SLOT, 1, InstructionEffect::Active, 4).as_slice(),
        "a write after the reset starts above the high-water"
    );

    let mut resumed = Authority::new();
    signal(
        &mut resumed,
        Signal::RecoverOperatorInstructionGenerationHighwater { generation: 3 },
    );
    assert_eq!(generation(&resumed), 3);
    resumed
        .apply_signal(Signal::SealIdentityGenerationHighwater)
        .expect("seal the high-water window");
    resumed
        .apply_signal(Signal::RecoverOperatorInstructionGenerationHighwater { generation: 9 })
        .expect_err("a sealed machine refuses the high-water recovery");
    assert_eq!(generation(&resumed), 3);
}

/// A destroyed mob holds no record and no delivery fact: the destroy of a
/// mob whose members are owed, recorded and blocked clears all of them.
#[test]
fn a_destroy_clears_every_record_and_fact() {
    let mut authority = Authority::new();
    seat(&mut authority, "recorded", "alpha");
    seat(&mut authority, "blocked", "alpha");
    seat(&mut authority, "owed", "alpha");
    signal(&mut authority, set(SLOT, 1, DIGEST_A, 10, Target::Mob));
    signal(
        &mut authority,
        set(OTHER_SLOT, 1, DIGEST_B, 10, Target::Member("owed")),
    );
    signal(&mut authority, record("recorded", 2));
    signal(
        &mut authority,
        refuse("blocked", 1, MemberInstructionBlock::OwnedBySession),
    );
    assert_eq!(recorded(&authority, "recorded"), Some(2));
    assert_eq!(
        blocked(&authority, "blocked"),
        Some(MemberInstructionBlock::OwnedBySession)
    );
    assert_eq!(owed(&authority, "owed"), Some(2));
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::BeginPlacedCompletionLifecycleQuiesce {
            intent: PlacedCompletionLifecycleIntentKind::Destroy,
        },
    )
    .expect("begin the destroy quiesce");
    MobMachineMutator::apply(&mut authority, MobMachineInput::Destroy)
        .expect("a mob holding operator instructions is destroyed");
    let state = authority.state();
    assert_eq!(state.lifecycle_phase, MobPhase::Destroyed);
    assert!(state.operator_instruction_revision.is_empty());
    assert!(state.operator_instruction_effect.is_empty());
    assert!(state.operator_instruction_digest.is_empty());
    assert!(state.operator_instruction_body_bytes.is_empty());
    assert!(state.operator_instruction_target_kind.is_empty());
    assert!(state.operator_instruction_target_profile.is_empty());
    assert!(state.operator_instruction_target_member.is_empty());
    assert!(state.member_instruction_owed.is_empty());
    assert!(state.member_instruction_recorded.is_empty());
    assert!(state.member_instruction_blocked.is_empty());
}

// ---------------------------------------------------------------------------
// The target predicate, invariants and the schema census
// ---------------------------------------------------------------------------

/// The catalog's Rust predicate agrees with the machine's helper on every
/// target shape: the machine owes exactly the members the predicate
/// selects.
#[test]
fn the_rust_target_predicate_agrees_with_the_machines_owed_set() {
    let members = [("alpha-1", "alpha"), ("beta-1", "beta")];
    for target in [
        Target::Mob,
        Target::Profile("alpha"),
        Target::Profile("missing"),
        Target::Member("beta-1"),
        Target::Member("missing"),
    ] {
        let mut authority = Authority::new();
        for (name, profile) in members {
            seat(&mut authority, name, profile);
        }
        signal(&mut authority, set(SLOT, 1, DIGEST_A, 10, target.clone()));
        let (kind, profile, member) = target.parts();
        for (name, member_profile) in members {
            let selected = operator_instruction_target_applies(
                kind,
                &profile,
                member.as_ref().map(|member| member.0.as_str()),
                Some(member_profile),
                name,
            );
            assert_eq!(
                owed(&authority, name).is_some(),
                selected,
                "{target:?} and {name}: the machine and the predicate agree"
            );
        }
    }
}

fn consistent_state() -> MobMachineState {
    let mut authority = Authority::new();
    seat(&mut authority, "member", "alpha");
    signal(
        &mut authority,
        set(SLOT, 1, DIGEST_A, 10, Target::Profile("alpha")),
    );
    signal(
        &mut authority,
        set(OTHER_SLOT, 1, DIGEST_B, 10, Target::Member("member")),
    );
    signal(&mut authority, record("member", 2));
    authority.state().clone()
}

fn rejected_invariant(state: MobMachineState) -> &'static str {
    match Authority::recover_from_state(state) {
        Err(MobMachineTransitionError::RecoveredStateInvariantRejected { invariant, .. }) => {
            invariant
        }
        Err(other) => panic!("unexpected refusal {other}"),
        Ok(_) => panic!("a torn state must not recover"),
    }
}

#[test]
fn every_reached_operator_instruction_state_recovers() {
    let state = consistent_state();
    let recovered = Authority::recover_from_state(state.clone())
        .unwrap_or_else(|error| panic!("a reached state recovers: {error}"));
    assert_eq!(recovered.state(), &state);
}

#[test]
fn recovered_state_rejects_records_out_of_lockstep() {
    let mut state = consistent_state();
    state.operator_instruction_digest.clear();
    assert_eq!(
        rejected_invariant(state),
        "operator_instruction_records_lockstep"
    );
}

#[test]
fn recovered_state_rejects_a_malformed_record() {
    let mut state = consistent_state();
    state
        .operator_instruction_digest
        .insert(SLOT.to_string(), String::new());
    assert_eq!(
        rejected_invariant(state),
        "operator_instruction_records_well_formed"
    );
}

#[test]
fn recovered_state_rejects_records_over_the_active_budget() {
    let mut state = consistent_state();
    state
        .operator_instruction_body_bytes
        .insert(SLOT.to_string(), BODY_LIMIT);
    state
        .operator_instruction_body_bytes
        .insert(OTHER_SLOT.to_string(), BODY_LIMIT);
    for index in 0..3 {
        let slot = format!("[\"big\",\"k{index}\"]");
        state.operator_instruction_revision.insert(slot.clone(), 1);
        state
            .operator_instruction_effect
            .insert(slot.clone(), InstructionEffect::Active);
        state
            .operator_instruction_digest
            .insert(slot.clone(), DIGEST_A.to_string());
        state
            .operator_instruction_body_bytes
            .insert(slot.clone(), BODY_LIMIT);
        state
            .operator_instruction_target_kind
            .insert(slot.clone(), InstructionTargetKind::Mob);
        state
            .operator_instruction_target_profile
            .insert(slot, String::new());
    }
    assert_eq!(
        rejected_invariant(state),
        "operator_instruction_active_bytes_bounded"
    );
}

#[test]
fn recovered_state_rejects_delivery_facts_for_a_non_member() {
    let mut state = consistent_state();
    state
        .member_instruction_owed
        .insert(identity("stranger"), 1);
    assert_eq!(
        rejected_invariant(state),
        "member_instruction_facts_only_for_members"
    );
}

#[test]
fn recovered_state_rejects_a_fact_above_the_generation() {
    let mut state = consistent_state();
    state
        .member_instruction_recorded
        .insert(identity("member"), 9);
    assert_eq!(
        rejected_invariant(state),
        "member_instruction_generations_bounded"
    );
}

#[test]
fn the_operator_instruction_region_declares_every_arm_and_invariant() {
    let schema = dsl_mob_machine();
    for arm in ARMS {
        assert!(
            schema
                .transitions
                .iter()
                .any(|transition| transition.name.as_str().starts_with(arm)),
            "MobMachine must declare {arm}"
        );
    }
    for invariant in INVARIANTS {
        assert!(
            schema
                .invariants
                .iter()
                .any(|declared| declared.name == invariant),
            "MobMachine must declare invariant {invariant}"
        );
    }
}

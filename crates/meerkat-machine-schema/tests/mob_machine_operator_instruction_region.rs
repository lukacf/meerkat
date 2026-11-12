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
    AgentIdentity, AgentRuntimeId, FenceToken, Generation, HostId, InstructionEffect,
    InstructionTargetKind, MemberInstructionBlock, MemberInstructionDeferReason,
    MemberMaterializationPhase, MemberPeerEndpoint, MemberSessionDisposal, MobId,
    MobMachineAuthority, MobMachineEffect, MobMachineInput, MobMachineMutator, MobMachineSignal,
    MobMachineState, MobMachineTransition, MobMachineTransitionError, MobPhase,
    OperatorInstructionRefusal, PeerAddress, PeerId, PeerName, PeerSigningKey,
    PlacedCompletionLifecycleIntentKind, SessionId, SpawnExecPhase, SpawnPolicyRuntimeMode,
    operator_instruction_target_applies, operator_instruction_target_applies_to_lineage,
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

const ARMS: [&str; 31] = [
    "SetOperatorInstructionBodyTooLarge",
    "SetOperatorInstructionSlotLimitExceeded",
    "SetOperatorInstructionNewOverBudget",
    "SetOperatorInstructionNew",
    "SetOperatorInstructionStale",
    "SetOperatorInstructionDuplicate",
    "SetOperatorInstructionConflict",
    "SetOperatorInstructionAdvanceSlotLimitExceeded",
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
    "PrepareMemberInstructionDeliveryUnborn",
    "PrepareMemberInstructionDeliveryUnbound",
    "PrepareMemberInstructionDeliveryRetiring",
    "PrepareMemberInstructionDeliveryDeliver",
    "PrepareMemberInstructionDeliverySpawning",
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
    MemberAndForks(&'static str),
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
            Self::MemberAndForks(member) => (
                InstructionTargetKind::MemberAndForks,
                String::new(),
                Some(identity(member)),
            ),
        }
    }
}

/// Record that the seated member `child` was forked from a member whose
/// lineage root is `root`.
fn record_lineage(
    authority: &mut Authority,
    child: &str,
    root: &str,
) -> Result<MobMachineTransition, MobMachineTransitionError> {
    MobMachineMutator::apply(
        authority,
        MobMachineInput::RecordMemberInstructionLineage {
            agent_identity: identity(child),
            root_identity: identity(root),
        },
    )
}

fn lineage_root(authority: &Authority, child: &str) -> Option<AgentIdentity> {
    authority
        .state()
        .member_instruction_lineage
        .get(&identity(child))
        .cloned()
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

fn member_slot(index: usize) -> String {
    format!("[\"agents\",\"m{index:03}\"]")
}

fn member_name(index: usize) -> &'static str {
    Box::leak(format!("agent-{index:03}").into_boxed_str())
}

fn shared_slot(index: usize) -> String {
    format!("[\"shared\",\"k{index:02}\"]")
}

fn commits(authority: &mut Authority, input: Signal) -> bool {
    signal(authority, input)
        .effects()
        .iter()
        .any(|effect| matches!(effect, Effect::OperatorInstructionCommitted { .. }))
}

/// The slot limit bounds one member's snapshot (the shared records plus the
/// member's own), not the mob: every agent of a mob can hold its own
/// member-targeted record, however many agents the mob has.
#[test]
fn each_of_many_members_holds_its_own_record() {
    let mut authority = Authority::new();
    for index in 0..200 {
        assert!(
            commits(
                &mut authority,
                set(
                    &member_slot(index),
                    1,
                    DIGEST_A,
                    10,
                    Target::Member(member_name(index)),
                ),
            ),
            "the record of member {index} commits"
        );
    }
    assert_eq!(authority.state().operator_instruction_revision.len(), 200);
    // A revoked record of one member takes no room from another member.
    signal(&mut authority, revoke(&member_slot(0), 2));
    assert!(commits(
        &mut authority,
        set(
            &member_slot(200),
            1,
            DIGEST_A,
            10,
            Target::Member(member_name(200)),
        ),
    ));
}

/// One member's snapshot holds at most 64 records: a member-targeted write
/// that would be its 65th is refused, while another member still takes its
/// own, and a shared write that would overfill the fullest member's snapshot
/// is refused.
#[test]
fn a_members_snapshot_holds_at_most_64_records() {
    let mut authority = Authority::new();
    for index in 0..60 {
        signal(
            &mut authority,
            set(&shared_slot(index), 1, DIGEST_A, 1, Target::Mob),
        );
    }
    for index in 0..4 {
        assert!(commits(
            &mut authority,
            set(
                &format!("[\"own\",\"a{index}\"]"),
                1,
                DIGEST_A,
                1,
                Target::Member("agent-a"),
            ),
        ));
    }
    assert_verdict_only(
        &mut authority,
        set(
            "[\"own\",\"a4\"]",
            1,
            DIGEST_A,
            1,
            Target::Member("agent-a"),
        ),
        &refused(
            "[\"own\",\"a4\"]",
            OperatorInstructionRefusal::SlotLimitExceeded,
            None,
        ),
    );
    // Another member's snapshot has room for its own four.
    for index in 0..4 {
        assert!(
            commits(
                &mut authority,
                set(
                    &format!("[\"own\",\"b{index}\"]"),
                    1,
                    DIGEST_A,
                    1,
                    Target::Member("agent-b"),
                ),
            ),
            "member b's own record {index} commits"
        );
    }
    // A shared record would be the 65th of both members' snapshots.
    assert_verdict_only(
        &mut authority,
        set(&shared_slot(60), 1, DIGEST_A, 1, Target::Profile("alpha")),
        &refused(
            &shared_slot(60),
            OperatorInstructionRefusal::SlotLimitExceeded,
            None,
        ),
    );
    // A new revision of a record already in the snapshot takes no room.
    assert!(commits(
        &mut authority,
        set(
            "[\"own\",\"a0\"]",
            2,
            DIGEST_B,
            1,
            Target::Member("agent-a")
        ),
    ));
}

/// A new revision that moves a record into a full snapshot is refused: a
/// member-targeted record retargeted to every member, and one retargeted to
/// a member whose snapshot is full.
#[test]
fn a_retarget_into_a_full_snapshot_is_refused() {
    let mut authority = Authority::new();
    for index in 0..63 {
        signal(
            &mut authority,
            set(&shared_slot(index), 1, DIGEST_A, 1, Target::Mob),
        );
    }
    signal(
        &mut authority,
        set("[\"own\",\"a\"]", 1, DIGEST_A, 1, Target::Member("agent-a")),
    );
    assert!(commits(
        &mut authority,
        set("[\"own\",\"b\"]", 1, DIGEST_A, 1, Target::Member("agent-b")),
    ));
    // Member b's record as a shared record: the 65th of member a's snapshot.
    assert_verdict_only(
        &mut authority,
        set("[\"own\",\"b\"]", 2, DIGEST_A, 1, Target::Mob),
        &refused(
            "[\"own\",\"b\"]",
            OperatorInstructionRefusal::SlotLimitExceeded,
            Some(1),
        ),
    );
    // Member b's record as member a's: the 65th of member a's snapshot.
    assert_verdict_only(
        &mut authority,
        set("[\"own\",\"b\"]", 2, DIGEST_A, 1, Target::Member("agent-a")),
        &refused(
            "[\"own\",\"b\"]",
            OperatorInstructionRefusal::SlotLimitExceeded,
            Some(1),
        ),
    );
    // To a member whose snapshot has room it moves.
    assert!(commits(
        &mut authority,
        set("[\"own\",\"b\"]", 2, DIGEST_A, 1, Target::Member("agent-c")),
    ));
    // A shared record retargeted to one member leaves every snapshot.
    assert!(commits(
        &mut authority,
        set(&shared_slot(0), 2, DIGEST_A, 1, Target::Member("agent-c")),
    ));
}

/// The active budget bounds one member's snapshot too: five members each
/// hold a full body of their own, and the shared bodies plus a member's own
/// stay within 1 MiB.
#[test]
fn the_active_budget_bounds_a_members_snapshot() {
    let mut authority = Authority::new();
    for index in 0..5 {
        assert!(
            commits(
                &mut authority,
                set(
                    &member_slot(index),
                    1,
                    DIGEST_A,
                    BODY_LIMIT,
                    Target::Member(member_name(index)),
                ),
            ),
            "member {index}'s full body commits"
        );
    }
    // Three full shared bodies fill every such member's snapshot exactly.
    for index in 0..3 {
        assert!(commits(
            &mut authority,
            set(&shared_slot(index), 1, DIGEST_A, BODY_LIMIT, Target::Mob),
        ));
    }
    assert_verdict_only(
        &mut authority,
        set(&shared_slot(3), 1, DIGEST_A, 1, Target::Mob),
        &refused(
            &shared_slot(3),
            OperatorInstructionRefusal::ActiveSetBudgetExceeded,
            None,
        ),
    );
    assert_verdict_only(
        &mut authority,
        set(
            "[\"own\",\"extra\"]",
            1,
            DIGEST_A,
            1,
            Target::Member(member_name(0)),
        ),
        &refused(
            "[\"own\",\"extra\"]",
            OperatorInstructionRefusal::ActiveSetBudgetExceeded,
            None,
        ),
    );
    // A member without a record of its own still has a full body's room.
    assert!(commits(
        &mut authority,
        set(
            &member_slot(5),
            1,
            DIGEST_A,
            BODY_LIMIT,
            Target::Member(member_name(5)),
        ),
    ));
    // A new revision that grows a shared body past a member's room is
    // refused; one that moves a member's body to another full member is too.
    assert_verdict_only(
        &mut authority,
        set(&member_slot(5), 2, DIGEST_A, BODY_LIMIT, Target::Mob),
        &refused(
            &member_slot(5),
            OperatorInstructionRefusal::ActiveSetBudgetExceeded,
            Some(1),
        ),
    );
    assert_verdict_only(
        &mut authority,
        set(
            &member_slot(5),
            2,
            DIGEST_A,
            BODY_LIMIT,
            Target::Member(member_name(0)),
        ),
        &refused(
            &member_slot(5),
            OperatorInstructionRefusal::ActiveSetBudgetExceeded,
            Some(1),
        ),
    );
}

/// Every write's verdict against an independent count: over a long mixed
/// sequence of writes, retargets and revokes for a few members, a write is
/// refused `slot_limit_exceeded` exactly when the snapshot of some member
/// would hold more than 64 records after it, `active_set_budget_exceeded`
/// exactly when some member's Active bodies would pass 1 MiB (and the slot
/// limit holds), and commits otherwise. A record for a member and its forks
/// counts in the snapshot of every other name too (any name may be seated
/// as a fork of that member). The count here is made per member from the
/// record maps, without the machine's helpers.
#[test]
fn every_write_verdict_agrees_with_an_independent_snapshot_count() {
    const MEMBERS: [&str; 4] = ["agent-a", "agent-b", "agent-c", "agent-d"];
    // The widest snapshot (records, Active bytes) of `state` with `slot`
    // written as `target` with `bytes` Active bytes.
    fn widest_after(
        state: &MobMachineState,
        slot: &str,
        target: &Target,
        bytes: u64,
    ) -> (u64, u64) {
        let (kind, _, member) = target.parts();
        // Each record: the name it is member-targeted at, whether its forks
        // take it, and its Active bytes.
        let mut records: Vec<(Option<AgentIdentity>, bool, u64)> = state
            .operator_instruction_revision
            .keys()
            .filter(|other| other.as_str() != slot)
            .map(|other| {
                let other_kind = state.operator_instruction_target_kind.get(other);
                let forks = other_kind == Some(&InstructionTargetKind::MemberAndForks);
                let own = (forks || other_kind == Some(&InstructionTargetKind::Member))
                    .then(|| state.operator_instruction_target_member.get(other).cloned())
                    .flatten();
                let active = state.operator_instruction_effect.get(other)
                    == Some(&InstructionEffect::Active);
                let bytes = if active {
                    state
                        .operator_instruction_body_bytes
                        .get(other)
                        .copied()
                        .unwrap_or(0)
                } else {
                    0
                };
                (own, forks, bytes)
            })
            .collect();
        let forks = kind == InstructionTargetKind::MemberAndForks;
        records.push((
            (forks || kind == InstructionTargetKind::Member)
                .then_some(member)
                .flatten(),
            forks,
            bytes,
        ));
        let shared_count = records.iter().filter(|(own, _, _)| own.is_none()).count() as u64;
        let shared_bytes: u64 = records
            .iter()
            .filter(|(own, _, _)| own.is_none())
            .map(|(_, _, bytes)| bytes)
            .sum();
        let mut widest = (shared_count, shared_bytes);
        for member in MEMBERS {
            let own = records
                .iter()
                .filter(|(own, _, _)| own.as_ref() == Some(&identity(member)));
            let own_count = own.clone().count() as u64;
            let own_bytes = own.map(|(_, _, bytes)| bytes).sum::<u64>();
            // Any name may be seated as a fork of another name: the most
            // another name's records pass to its forks.
            let mut from_another = (0_u64, 0_u64);
            for root in MEMBERS {
                if root == member {
                    continue;
                }
                let passed = records
                    .iter()
                    .filter(|(own, forks, _)| *forks && own.as_ref() == Some(&identity(root)));
                from_another = (
                    from_another.0.max(passed.clone().count() as u64),
                    from_another
                        .1
                        .max(passed.map(|(_, _, bytes)| bytes).sum::<u64>()),
                );
            }
            widest = (
                widest.0.max(shared_count + own_count + from_another.0),
                widest.1.max(shared_bytes + own_bytes + from_another.1),
            );
        }
        widest
    }

    let mut authority = Authority::new();
    let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = |bound: u64| {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (seed >> 33) % bound
    };
    let (mut slot_refusals, mut budget_refusals, mut commits_seen) = (0, 0, 0);
    for _ in 0..4000 {
        let slot = format!("[\"walk\",\"k{:03}\"]", next(140));
        let current = authority
            .state()
            .operator_instruction_revision
            .get(&slot)
            .copied();
        if current.is_some() && next(10) == 0 {
            signal(&mut authority, revoke(&slot, current.unwrap_or(0) + 1));
            continue;
        }
        let target = match next(10) {
            0 => Target::Mob,
            1 => Target::Profile("alpha"),
            other @ 2..=4 => Target::MemberAndForks(MEMBERS[(other % 4) as usize]),
            other => Target::Member(MEMBERS[(other % 4) as usize]),
        };
        // Mostly small bodies, sometimes a large one.
        let bytes = if next(6) == 0 {
            BODY_LIMIT / (1 + next(4))
        } else {
            1 + next(64)
        };
        let (slots_after, bytes_after) = widest_after(authority.state(), &slot, &target, bytes);
        let revision = current.unwrap_or(0) + 1;
        let transition = signal(
            &mut authority,
            set(&slot, revision, DIGEST_A, bytes, target.clone()),
        );
        let expected = if slots_after > 64 {
            refused(
                &slot,
                OperatorInstructionRefusal::SlotLimitExceeded,
                current,
            )
        } else if bytes_after > ACTIVE_BUDGET {
            refused(
                &slot,
                OperatorInstructionRefusal::ActiveSetBudgetExceeded,
                current,
            )
        } else {
            committed(
                &slot,
                revision,
                InstructionEffect::Active,
                generation(&authority),
            )
        };
        assert_eq!(
            transition.effects(),
            expected.as_slice(),
            "{slot} at revision {revision} for {target:?} with {bytes} bytes: the widest \
             snapshot after it holds {slots_after} records and {bytes_after} Active bytes"
        );
        match transition.effects() {
            [
                Effect::OperatorInstructionRefused {
                    refusal: OperatorInstructionRefusal::SlotLimitExceeded,
                    ..
                },
            ] => slot_refusals += 1,
            [Effect::OperatorInstructionRefused { .. }] => budget_refusals += 1,
            _ => commits_seen += 1,
        }
    }
    assert!(
        slot_refusals > 20 && budget_refusals > 20 && commits_seen > 200,
        "the walk reaches both limits: {slot_refusals} slot refusals, {budget_refusals} budget \
         refusals, {commits_seen} commits"
    );
    assert!(
        authority.state().operator_instruction_revision.len() > 64,
        "the mob holds more records than one snapshot's limit"
    );
    Authority::recover_from_state(authority.state().clone())
        .unwrap_or_else(|error| panic!("the reached state recovers: {error}"));
}

/// The replay of a journal that holds more records than one snapshot's
/// limit restores them all: the bound is per member snapshot.
#[test]
fn the_replay_restores_more_records_than_one_snapshot_holds() {
    let mut live = Authority::new();
    seat(&mut live, member_name(7), "alpha");
    for index in 0..100 {
        signal(
            &mut live,
            Signal::RecoverOperatorInstruction {
                slot: member_slot(index),
                revision: 1,
                effect: InstructionEffect::Active,
                digest: DIGEST_A.to_string(),
                body_bytes: BODY_LIMIT,
                target_kind: InstructionTargetKind::Member,
                target_profile: String::new(),
                target_member: Some(identity(member_name(index))),
                generation: index as u64 + 1,
            },
        );
    }
    let state = live.state().clone();
    assert_eq!(state.operator_instruction_revision.len(), 100);
    assert_eq!(generation(&live), 100);
    // Only the record's own member is owed, at its record's generation.
    assert_eq!(owed(&live, member_name(7)), Some(8));
    let recovered = Authority::recover_from_state(state.clone())
        .unwrap_or_else(|error| panic!("a reached state recovers: {error}"));
    assert_eq!(recovered.state(), &state);
}

/// A member-targeted record owes exactly its member, looked up among the
/// seats: a write owes the member it names, a retarget to another member
/// owes the member it leaves and the one it reaches, a revoke owes the
/// member it named, and a record for a member that is not seated owes
/// nobody. Every other seated member keeps what it had, blocked or not.
#[test]
fn a_member_targeted_record_owes_only_the_members_it_names() {
    let mut authority = Authority::new();
    for name in ["agent-a", "agent-b", "agent-c"] {
        seat(&mut authority, name, "alpha");
    }
    // Member c is blocked before any member-targeted write.
    signal(&mut authority, set(OTHER_SLOT, 1, DIGEST_A, 1, Target::Mob));
    signal(
        &mut authority,
        refuse("agent-c", 1, MemberInstructionBlock::OwnedByOtherMob),
    );
    signal(&mut authority, record("agent-a", 1));
    signal(&mut authority, record("agent-b", 1));
    assert_eq!(
        blocked(&authority, "agent-c"),
        Some(MemberInstructionBlock::OwnedByOtherMob)
    );

    signal(
        &mut authority,
        set(SLOT, 1, DIGEST_A, 1, Target::Member("agent-a")),
    );
    assert_eq!(owed(&authority, "agent-a"), Some(2));
    assert_eq!(owed(&authority, "agent-b"), None);
    assert_eq!(
        blocked(&authority, "agent-c"),
        Some(MemberInstructionBlock::OwnedByOtherMob),
        "a record for another member leaves a blocked member blocked"
    );
    signal(&mut authority, record("agent-a", 2));

    // The record moves to member b: both are owed.
    signal(
        &mut authority,
        set(SLOT, 2, DIGEST_A, 1, Target::Member("agent-b")),
    );
    assert_eq!(owed(&authority, "agent-a"), Some(3));
    assert_eq!(owed(&authority, "agent-b"), Some(3));
    assert_eq!(owed(&authority, "agent-c"), None);
    signal(&mut authority, record("agent-a", 3));
    signal(&mut authority, record("agent-b", 3));

    // It moves to member c: member b and the blocked member c are owed,
    // and member c is unblocked.
    signal(
        &mut authority,
        set(SLOT, 3, DIGEST_A, 1, Target::Member("agent-c")),
    );
    assert_eq!(owed(&authority, "agent-a"), None);
    assert_eq!(owed(&authority, "agent-b"), Some(4));
    assert_eq!(owed(&authority, "agent-c"), Some(4));
    assert_eq!(blocked(&authority, "agent-c"), None);
    signal(&mut authority, record("agent-b", 4));
    signal(&mut authority, record("agent-c", 4));

    // A revoke owes the member the record names.
    signal(&mut authority, revoke(SLOT, 4));
    assert_eq!(owed(&authority, "agent-a"), None);
    assert_eq!(owed(&authority, "agent-b"), None);
    assert_eq!(owed(&authority, "agent-c"), Some(5));
    signal(&mut authority, record("agent-c", 5));

    // A record for a member that is not seated, and its move to another
    // such member, owe nobody.
    signal(
        &mut authority,
        set(
            "[\"app.example\",\"absent\"]",
            1,
            DIGEST_A,
            1,
            Target::Member("agent-x"),
        ),
    );
    signal(
        &mut authority,
        set(
            "[\"app.example\",\"absent\"]",
            2,
            DIGEST_A,
            1,
            Target::Member("agent-y"),
        ),
    );
    for name in ["agent-a", "agent-b", "agent-c"] {
        assert_eq!(owed(&authority, name), None, "{name} is owed nothing");
    }
    assert_eq!(generation(&authority), 7);

    // The replay of a member-targeted record over another member's owes
    // both, at the replayed generation.
    signal(
        &mut authority,
        Signal::RecoverOperatorInstruction {
            slot: SLOT.to_string(),
            revision: 5,
            effect: InstructionEffect::Active,
            digest: DIGEST_B.to_string(),
            body_bytes: 1,
            target_kind: InstructionTargetKind::Member,
            target_profile: String::new(),
            target_member: Some(identity("agent-a")),
            generation: 9,
        },
    );
    assert_eq!(owed(&authority, "agent-a"), Some(9));
    assert_eq!(owed(&authority, "agent-b"), None);
    assert_eq!(owed(&authority, "agent-c"), Some(9));
}

/// Measurement, not a check: what the machine pays for member-targeted
/// records at 1,000 and 10,000 edited members, all seated. Run it in a
/// release build (`--release -- --ignored --nocapture`): a debug build
/// validates every invariant after every step. It prints, per size: the
/// seats, the writes of one record per member (the last hundred, where the
/// mob is full), one more member's write, a shared write, one member's
/// delivery preparation and record, the replay of every record into a
/// machine with the members seated, the whole-state invariant check, a
/// fork of the state (what a prepared step of the mob's shell copies), and
/// with the same records and no member seated, a fork and one seat.
#[test]
#[ignore = "measurement: prints machine costs at 1,000 and 10,000 member records"]
fn measure_member_record_costs_in_the_machine() {
    use std::time::Instant;
    for members in [1_000_usize, 10_000] {
        let names: Vec<&'static str> = (0..=members)
            .map(|index| &*Box::leak(format!("agent-{index:05}").into_boxed_str()))
            .collect();
        let slots: Vec<String> = (0..=members)
            .map(|index| format!("[\"agents\",\"m{index:05}\"]"))
            .collect();
        let mut authority = Authority::new();
        let started = Instant::now();
        for name in &names[..members] {
            seat(&mut authority, name, "alpha");
        }
        let seats = started.elapsed();
        let mut last_hundred = std::time::Duration::ZERO;
        let started = Instant::now();
        for (index, (slot, name)) in slots.iter().zip(&names).take(members).enumerate() {
            let write_started = Instant::now();
            assert!(commits(
                &mut authority,
                set(slot, 1, DIGEST_A, 4096, Target::Member(name)),
            ));
            if index + 100 >= members {
                last_hundred += write_started.elapsed();
            }
        }
        let writes = started.elapsed();
        let started = Instant::now();
        assert!(commits(
            &mut authority,
            set(
                &slots[members],
                1,
                DIGEST_A,
                4096,
                Target::Member(names[members]),
            ),
        ));
        let one_more_write = started.elapsed();
        let started = Instant::now();
        assert!(commits(
            &mut authority,
            set(SLOT, 1, DIGEST_A, 64, Target::Mob)
        ));
        let shared_write = started.elapsed();
        let current_generation = generation(&authority);
        let started = Instant::now();
        signal(&mut authority, prepare(names[7]));
        signal(&mut authority, record(names[7], current_generation));
        let delivery = started.elapsed();
        let started = Instant::now();
        let fork = authority.fork();
        let fork_time = started.elapsed();
        let started = Instant::now();
        Authority::recover_from_state(fork.state().clone())
            .unwrap_or_else(|error| panic!("the reached state recovers: {error}"));
        let recover = started.elapsed();

        let mut replayed = Authority::new();
        for name in &names[..members] {
            seat(&mut replayed, name, "alpha");
        }
        let started = Instant::now();
        for (index, (slot, name)) in slots.iter().zip(&names).take(members).enumerate() {
            signal(
                &mut replayed,
                Signal::RecoverOperatorInstruction {
                    slot: slot.clone(),
                    revision: 1,
                    effect: InstructionEffect::Active,
                    digest: DIGEST_A.to_string(),
                    body_bytes: 4096,
                    target_kind: InstructionTargetKind::Member,
                    target_profile: String::new(),
                    target_member: Some(identity(name)),
                    generation: index as u64 + 1,
                },
            );
        }
        let replay = started.elapsed();

        // The same records with no member seated: what a step that has
        // nothing to do with instructions (a seat) and a fork of the state
        // cost because the machine holds a record per member.
        let mut unseated = Authority::new();
        for (slot, name) in slots.iter().zip(&names).take(members) {
            assert!(commits(
                &mut unseated,
                set(slot, 1, DIGEST_A, 4096, Target::Member(name)),
            ));
        }
        let started = Instant::now();
        let unseated_fork = unseated.fork();
        let records_only_fork = started.elapsed();
        drop(unseated_fork);
        let started = Instant::now();
        seat(&mut unseated, "agent-unrelated", "alpha");
        let seat_beside_records = started.elapsed();
        println!(
            "members={members} seats={seats:?} writes_total={writes:?} \
             write_avg_last_100={:?} one_more_member_write={one_more_write:?} \
             shared_write={shared_write:?} prepare_and_record={delivery:?} \
             replay_total={replay:?} replay_avg={:?} fork={fork_time:?} \
             clone_and_recover_from_state={recover:?} \
             fork_records_only={records_only_fork:?} \
             seat_beside_records={seat_beside_records:?}",
            last_hundred / 100,
            replay / members as u32,
        );
    }
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
/// generation, and preparing it wakes nothing.
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

/// A placed member's delivery is prepared exactly like a local member's: for
/// the session bound to it (its session on its member host) at the current
/// generation; the mob carries it to the host over the supervisor bridge.
/// A member host that does not serve the record refuses it, and that
/// feedback blocks the member `member_host_unsupported` until a write
/// selects it again.
#[test]
fn a_placed_member_is_prepared_like_a_local_member() {
    let mut authority = Authority::new();
    seat(&mut authority, "placed", "alpha");
    // A placed member's committed residency: its host, the owner bridge
    // every placement needs, the peer endpoint its spawn ack carried, and
    // its host's live materialization (a placed member is never Dormant).
    let mut state = authority.state().clone();
    state
        .member_placement
        .insert(identity("placed"), HostId("host-1".to_string()));
    state
        .member_materialization
        .insert(identity("placed"), MemberMaterializationPhase::Live);
    state.owner_bridge_session_id = Some(SessionId("owner-bridge".to_string()));
    state
        .member_peer_ids
        .insert(identity("placed"), PeerId("peer-placed".to_string()));
    state.member_peer_endpoints.insert(
        identity("placed"),
        MemberPeerEndpoint::new(
            PeerName("placed".to_string()),
            PeerId("peer-placed".to_string()),
            PeerAddress("tcp://127.0.0.1:4100".to_string()),
            PeerSigningKey([7; 32]),
        ),
    );
    let mut authority = Authority::recover_from_state(state)
        .unwrap_or_else(|error| panic!("a placed member's state: {error}"));
    signal(&mut authority, set(SLOT, 1, DIGEST_A, 10, Target::Mob));
    let transition = signal(&mut authority, prepare("placed"));
    assert_eq!(
        transition.effects(),
        [Effect::MemberInstructionDeliveryPrepared {
            agent_identity: identity("placed"),
            session_id: session_of("placed"),
            generation: 1,
        }]
        .as_slice(),
        "a placed member is prepared, not blocked"
    );
    assert_wakes_nothing(&transition);
    assert_eq!(
        owed(&authority, "placed"),
        Some(1),
        "preparing closes nothing"
    );

    let transition = signal(
        &mut authority,
        refuse("placed", 1, MemberInstructionBlock::MemberHostUnsupported),
    );
    assert_eq!(
        transition.effects(),
        [Effect::MemberInstructionTargetBlocked {
            agent_identity: identity("placed"),
            generation: 1,
            block: MemberInstructionBlock::MemberHostUnsupported,
        }]
        .as_slice()
    );
    assert_eq!(owed(&authority, "placed"), None);
    assert_eq!(recorded(&authority, "placed"), None);
    signal(&mut authority, set(SLOT, 2, DIGEST_B, 10, Target::Mob));
    assert_eq!(blocked(&authority, "placed"), None);
    assert_eq!(owed(&authority, "placed"), Some(2));
    signal(&mut authority, prepare("placed"));
    signal(&mut authority, record("placed", 2));
    assert_eq!(recorded(&authority, "placed"), Some(2));
    assert_eq!(owed(&authority, "placed"), None);
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

/// A member between its membership commit and its activation (its spawn
/// ladder at MembershipCommitted, where an attach's commit leaves it while
/// the activation stages run) is owed and bound, but its delivery defers
/// until CommitSpawnActivation settles the ladder: a failed activation rolls
/// the member back, and a session the mob had delivered to would keep a
/// target no member holds.
#[test]
fn a_member_whose_spawn_is_in_flight_defers_until_its_activation_commits() {
    let mut authority = Authority::new();
    signal(&mut authority, set(SLOT, 1, DIGEST_A, 10, Target::Mob));
    seat(&mut authority, "attached", "alpha");
    let mut state = authority.state().clone();
    state
        .spawn_exec_phase
        .insert(identity("attached"), SpawnExecPhase::MembershipCommitted);
    let mut authority = Authority::recover_from_state(state)
        .unwrap_or_else(|error| panic!("a member between its commit and activation: {error}"));
    assert_eq!(owed(&authority, "attached"), Some(1));
    assert_verdict_only(
        &mut authority,
        prepare("attached"),
        &[Effect::MemberInstructionDeliveryDeferred {
            agent_identity: identity("attached"),
            reason: MemberInstructionDeferReason::SpawnInFlight,
        }],
    );

    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::CommitSpawnActivation {
            agent_identity: identity("attached"),
        },
    )
    .expect("the activation commits");
    let transition = signal(&mut authority, prepare("attached"));
    assert_eq!(
        transition.effects(),
        [Effect::MemberInstructionDeliveryPrepared {
            agent_identity: identity("attached"),
            session_id: session_of("attached"),
            generation: 1,
        }]
        .as_slice()
    );
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
        session_id: session_of("member"),
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
    // A session that is not the member's is never released for it.
    assert_verdict_only(
        &mut authority,
        Signal::ResolveMemberInstructionRelease {
            agent_identity: identity("member"),
            session_id: session_of("someone-else"),
            disposal: MemberSessionDisposal::RuntimeReleasedOnlyHostOwned,
        },
        &not_needed,
    );
}

/// A releasing retire unbinds its member before the member's session is
/// disposed of: the release verdict authorizes the release of the session
/// the retire moved to its runtime's pending retirement, and the delivery
/// of a retiring member that is owed defers as retiring.
#[test]
fn a_releasing_retire_authorizes_the_release_of_the_session_it_retires_from() {
    let mut authority = Authority::new();
    seat(&mut authority, "member", "alpha");
    signal(&mut authority, set(SLOT, 1, DIGEST_A, 10, Target::Mob));
    signal(&mut authority, record("member", 1));
    // The member is Dormant: its releasing retire is archive-only.
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::Retire {
            mob_id: MobId("oi-region".to_string()),
            agent_runtime_id: runtime_of("member"),
            agent_identity: identity("member"),
            generation: Generation(1),
            releasing: Some(session_of("member")),
            session_id: Some(session_of("member")),
        },
    )
    .expect("the member's releasing retire is admitted");
    let state = authority.state();
    assert!(
        !state
            .member_session_bindings
            .contains_key(&identity("member")),
        "the releasing retire unbound the member"
    );
    assert_eq!(
        state
            .runtime_retire_pending_sessions
            .get(&runtime_of("member")),
        Some(&session_of("member"))
    );
    assert_verdict_only(
        &mut authority,
        Signal::ResolveMemberInstructionRelease {
            agent_identity: identity("member"),
            session_id: session_of("member"),
            disposal: MemberSessionDisposal::RuntimeReleasedOnlyHostOwned,
        },
        &[Effect::MemberInstructionReleaseAuthorized {
            agent_identity: identity("member"),
            session_id: session_of("member"),
        }],
    );
    let not_needed = [Effect::MemberInstructionReleaseNotNeeded {
        agent_identity: identity("member"),
    }];
    assert_verdict_only(
        &mut authority,
        Signal::ResolveMemberInstructionRelease {
            agent_identity: identity("member"),
            session_id: session_of("someone-else"),
            disposal: MemberSessionDisposal::RuntimeReleasedOnlyHostOwned,
        },
        &not_needed,
    );
    assert_verdict_only(
        &mut authority,
        Signal::ResolveMemberInstructionRelease {
            agent_identity: identity("member"),
            session_id: session_of("member"),
            disposal: MemberSessionDisposal::Archived,
        },
        &not_needed,
    );
    // A write owes the retiring member, whose delivery defers as retiring.
    signal(&mut authority, set(SLOT, 2, DIGEST_B, 10, Target::Mob));
    assert_eq!(owed(&authority, "member"), Some(2));
    assert_verdict_only(
        &mut authority,
        prepare("member"),
        &[Effect::MemberInstructionDeliveryDeferred {
            agent_identity: identity("member"),
            reason: MemberInstructionDeferReason::Retiring,
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

// ---------------------------------------------------------------------------
// Forks: a record for a member and its forks
// ---------------------------------------------------------------------------

/// A fork-derived member's lineage root is recorded for a seated member
/// that is not its own root; from then on a record written for the root and
/// its forks owes the member too, while a record for the root alone does
/// not. The Rust predicate agrees with the machine.
#[test]
fn a_record_for_a_member_and_its_forks_owes_its_seated_forks() {
    let mut authority = Authority::new();
    for name in ["root", "fork", "bystander"] {
        seat(&mut authority, name, "alpha");
    }
    assert!(
        record_lineage(&mut authority, "fork", "fork").is_err(),
        "a member is never its own root"
    );
    assert!(
        record_lineage(&mut authority, "unseated", "root").is_err(),
        "only a seated member records a lineage"
    );
    record_lineage(&mut authority, "fork", "root").expect("the fork records its root");
    assert_eq!(lineage_root(&authority, "fork"), Some(identity("root")));
    assert_eq!(
        owed(&authority, "fork"),
        None,
        "no generation, no obligation"
    );

    // A record for the root alone leaves the fork out.
    signal(
        &mut authority,
        set(SLOT, 1, DIGEST_A, 10, Target::Member("root")),
    );
    assert_eq!(owed(&authority, "root"), Some(1));
    assert_eq!(owed(&authority, "fork"), None);
    assert_eq!(owed(&authority, "bystander"), None);

    // The same slot rewritten for the root and its forks owes the fork.
    signal(
        &mut authority,
        set(SLOT, 2, DIGEST_B, 10, Target::MemberAndForks("root")),
    );
    assert_eq!(owed(&authority, "root"), Some(2));
    assert_eq!(owed(&authority, "fork"), Some(2));
    assert_eq!(owed(&authority, "bystander"), None);
    for (name, root) in [("root", None), ("fork", Some("root")), ("bystander", None)] {
        assert_eq!(
            operator_instruction_target_applies_to_lineage(
                InstructionTargetKind::MemberAndForks,
                "",
                Some("root"),
                Some("alpha"),
                name,
                root,
            ),
            owed(&authority, name).is_some(),
            "{name}: the machine and the predicate agree"
        );
    }

    // The revoke owes them both again, and so does a move to another
    // member, which the fork then leaves.
    signal(&mut authority, record("root", 2));
    signal(&mut authority, record("fork", 2));
    signal(&mut authority, revoke(SLOT, 3));
    assert_eq!(owed(&authority, "root"), Some(3));
    assert_eq!(owed(&authority, "fork"), Some(3));
    signal(&mut authority, record("root", 3));
    signal(&mut authority, record("fork", 3));
    signal(
        &mut authority,
        set(SLOT, 4, DIGEST_A, 10, Target::Member("bystander")),
    );
    assert_eq!(
        owed(&authority, "fork"),
        Some(4),
        "the fork leaves the record"
    );
    assert_eq!(owed(&authority, "root"), Some(4));
    assert_eq!(owed(&authority, "bystander"), Some(4));
}

/// A member whose lineage is recorded after a write is owed the mob's
/// snapshot at the generation the mob holds: the records for its root and
/// its forks select it from then on. A fork of a fork names its source and
/// the machine records the root, so it is selected the same way. A member
/// seated beside them with no lineage is not.
#[test]
fn a_fork_seated_after_the_write_is_owed_through_its_lineage() {
    let mut authority = Authority::new();
    seat(&mut authority, "root", "alpha");
    signal(
        &mut authority,
        set(SLOT, 1, DIGEST_A, 10, Target::MemberAndForks("root")),
    );
    seat(&mut authority, "fork", "alpha");
    record_lineage(&mut authority, "fork", "root").expect("the fork records its root");
    assert_eq!(
        owed(&authority, "fork"),
        Some(1),
        "the fork is owed the snapshot the mob holds"
    );
    signal(&mut authority, record("fork", 1));
    assert_eq!(owed(&authority, "fork"), None);
    seat(&mut authority, "fork-of-fork", "alpha");
    record_lineage(&mut authority, "fork-of-fork", "fork")
        .expect("a fork of a fork names its source");
    assert_eq!(
        lineage_root(&authority, "fork-of-fork"),
        Some(identity("root")),
        "the machine records the root, never the fork between"
    );
    signal(&mut authority, record("fork-of-fork", 1));
    seat(&mut authority, "bystander", "alpha");
    signal(&mut authority, record("bystander", 1));
    signal(
        &mut authority,
        set(SLOT, 2, DIGEST_B, 10, Target::MemberAndForks("root")),
    );
    assert_eq!(owed(&authority, "fork"), Some(2));
    assert_eq!(owed(&authority, "fork-of-fork"), Some(2));
    assert_eq!(
        owed(&authority, "bystander"),
        None,
        "a member with no lineage is outside the record's selection"
    );
}

/// The lineage record is refused for a member whose session already
/// recorded a snapshot under this seat (that snapshot was built without the
/// lineage, and a generation is recorded once), for a member named as its
/// own root and for a name that is not seated. A refusal changes nothing.
/// A member whose named root descends from the member's own name records
/// no entry.
#[test]
fn a_lineage_record_is_refused_when_it_cannot_hold() {
    let mut authority = Authority::new();
    seat(&mut authority, "root", "alpha");
    signal(
        &mut authority,
        set(SLOT, 1, DIGEST_A, 10, Target::MemberAndForks("root")),
    );
    seat(&mut authority, "late", "alpha");
    signal(&mut authority, record("late", 1));
    assert!(
        matches!(
            record_lineage(&mut authority, "late", "root"),
            Err(MobMachineTransitionError::GuardRejected { .. })
        ),
        "a member that recorded a snapshot takes no lineage"
    );
    assert_eq!(lineage_root(&authority, "late"), None);
    assert_eq!(owed(&authority, "late"), None);
    assert_eq!(recorded(&authority, "late"), Some(1));

    assert!(
        matches!(
            record_lineage(&mut authority, "root", "root"),
            Err(MobMachineTransitionError::GuardRejected { .. })
        ),
        "a member is never its own root"
    );
    seat(&mut authority, "fork", "alpha");
    record_lineage(&mut authority, "fork", "root").expect("the fork records its root");
    // A name that lives again as a fork of one of its earlier forks
    // descends from its own name: the records for that name select it
    // already, so the record commits and writes no entry.
    record_lineage(&mut authority, "root", "fork")
        .expect("a member that descends from its own name records nothing");
    assert_eq!(lineage_root(&authority, "root"), None);
    assert_eq!(lineage_root(&authority, "fork"), Some(identity("root")));
    assert!(
        matches!(
            record_lineage(&mut authority, "nobody", "root"),
            Err(MobMachineTransitionError::GuardRejected { .. })
        ),
        "a name that is not seated takes no lineage"
    );
    assert_eq!(lineage_root(&authority, "root"), None);
    assert_eq!(lineage_root(&authority, "nobody"), None);
}

/// A fork's lineage ends with the fork's identity: a member seated later
/// under the same name is not selected by its predecessor's root's records.
/// A reset clears every lineage.
#[test]
fn a_forks_lineage_ends_with_its_identity() {
    let mut authority = Authority::new();
    seat(&mut authority, "root", "alpha");
    seat(&mut authority, "fork", "alpha");
    record_lineage(&mut authority, "fork", "root").expect("the fork records its root");
    signal(
        &mut authority,
        Signal::EndIdentityLife {
            agent_identity: identity("fork"),
        },
    );
    assert_eq!(
        lineage_root(&authority, "fork"),
        None,
        "the end of the identity ends its lineage"
    );
    signal(
        &mut authority,
        set(SLOT, 1, DIGEST_A, 10, Target::MemberAndForks("root")),
    );
    assert_eq!(owed(&authority, "root"), Some(1));
    assert_eq!(
        owed(&authority, "fork"),
        None,
        "the name is no longer a fork of the root"
    );

    record_lineage(&mut authority, "fork", "root").expect("a new fork records its root");
    unseat(&mut authority, "fork");
    unseat(&mut authority, "root");
    assert_eq!(
        lineage_root(&authority, "fork"),
        Some(identity("root")),
        "a retirement that does not end the identity keeps its lineage"
    );
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::BeginPlacedCompletionLifecycleQuiesce {
            intent: PlacedCompletionLifecycleIntentKind::Reset,
        },
    )
    .expect("begin the reset quiesce");
    MobMachineMutator::apply(&mut authority, MobMachineInput::Reset).expect("reset the mob");
    assert_eq!(lineage_root(&authority, "fork"), None, "a reset clears it");
}

/// The replay of a record for a member and its forks owes the forks the
/// replayed lineage names.
#[test]
fn the_replay_of_a_record_for_forks_owes_the_forks() {
    let mut authority = Authority::new();
    seat(&mut authority, "root", "alpha");
    seat(&mut authority, "fork", "alpha");
    seat(&mut authority, "bystander", "alpha");
    record_lineage(&mut authority, "fork", "root").expect("the replayed lineage");
    signal(
        &mut authority,
        Signal::RecoverOperatorInstruction {
            slot: SLOT.to_string(),
            revision: 1,
            effect: InstructionEffect::Active,
            digest: DIGEST_A.to_string(),
            body_bytes: 10,
            target_kind: InstructionTargetKind::MemberAndForks,
            target_profile: String::new(),
            target_member: Some(identity("root")),
            generation: 1,
        },
    );
    assert_eq!(owed(&authority, "root"), Some(1));
    assert_eq!(owed(&authority, "fork"), Some(1));
    assert_eq!(owed(&authority, "bystander"), None);
}

/// Any name may be seated as a fork of another, so a record for a member
/// and its forks has to fit beside the records of every other name: the
/// widest snapshot is the shared records, the most one name holds of its
/// own, and what another name's records pass to its forks. A record for the
/// member alone is bounded by the member's own snapshot, as before.
#[test]
fn a_record_for_forks_fits_beside_another_names_records() {
    let mut authority = Authority::new();
    for index in 0..60 {
        signal(
            &mut authority,
            set(&shared_slot(index), 1, DIGEST_A, 1, Target::Mob),
        );
    }
    // Another name holds three records of its own.
    for index in 0..3 {
        assert!(commits(
            &mut authority,
            set(
                &format!("[\"own\",\"b{index}\"]"),
                1,
                DIGEST_A,
                1,
                Target::Member("agent-b"),
            ),
        ));
    }
    // One record for agent a and its forks: 60 shared, agent b's three and
    // the one a fork of agent a takes are 64.
    assert!(commits(
        &mut authority,
        set(
            "[\"own\",\"a0\"]",
            1,
            DIGEST_A,
            1,
            Target::MemberAndForks("agent-a"),
        ),
    ));
    // A second one would be the 65th record of a fork of agent a seated
    // under agent b's name.
    assert_verdict_only(
        &mut authority,
        set(
            "[\"own\",\"a1\"]",
            1,
            DIGEST_A,
            1,
            Target::MemberAndForks("agent-a"),
        ),
        &refused(
            "[\"own\",\"a1\"]",
            OperatorInstructionRefusal::SlotLimitExceeded,
            None,
        ),
    );
    // For agent a alone it fits: agent a's own snapshot holds 62.
    assert!(commits(
        &mut authority,
        set(
            "[\"own\",\"a1\"]",
            1,
            DIGEST_A,
            1,
            Target::Member("agent-a"),
        ),
    ));
    // Widening that record to agent a's forks is the same 65th record.
    assert_verdict_only(
        &mut authority,
        set(
            "[\"own\",\"a1\"]",
            2,
            DIGEST_B,
            1,
            Target::MemberAndForks("agent-a"),
        ),
        &refused(
            "[\"own\",\"a1\"]",
            OperatorInstructionRefusal::SlotLimitExceeded,
            Some(1),
        ),
    );
    Authority::recover_from_state(authority.state().clone())
        .unwrap_or_else(|error| panic!("the reached state recovers: {error}"));
}

/// The active budget bounds the same widest snapshot: the bodies another
/// name holds and the bodies a record passes to forks count together.
#[test]
fn the_active_budget_counts_what_forks_take() {
    let mut authority = Authority::new();
    // Agent b holds three full bodies of its own.
    for index in 0..3 {
        assert!(commits(
            &mut authority,
            set(
                &format!("[\"own\",\"b{index}\"]"),
                1,
                DIGEST_A,
                BODY_LIMIT,
                Target::Member("agent-b"),
            ),
        ));
    }
    // One full body for agent a and its forks: a fork of agent a under
    // agent b's name holds four full bodies, the whole budget.
    assert!(commits(
        &mut authority,
        set(
            "[\"own\",\"a0\"]",
            1,
            DIGEST_A,
            BODY_LIMIT,
            Target::MemberAndForks("agent-a"),
        ),
    ));
    assert_verdict_only(
        &mut authority,
        set(
            "[\"own\",\"a1\"]",
            1,
            DIGEST_A,
            1,
            Target::MemberAndForks("agent-a"),
        ),
        &refused(
            "[\"own\",\"a1\"]",
            OperatorInstructionRefusal::ActiveSetBudgetExceeded,
            None,
        ),
    );
    // For agent a alone the byte fits.
    assert!(commits(
        &mut authority,
        set(
            "[\"own\",\"a1\"]",
            1,
            DIGEST_A,
            1,
            Target::Member("agent-a"),
        ),
    ));
    assert_eq!(ACTIVE_BUDGET, 4 * BODY_LIMIT);
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

/// A state whose snapshot for some member holds more than 64 records does
/// not recover: 65 shared records, and 64 shared records plus one member's
/// own.
#[test]
fn recovered_state_rejects_a_snapshot_over_the_slot_limit() {
    fn with_records(shared: usize, own: Option<&str>) -> MobMachineState {
        let mut state = Authority::new().state().clone();
        let mut insert = |slot: String, member: Option<&str>| {
            state.operator_instruction_revision.insert(slot.clone(), 1);
            state
                .operator_instruction_effect
                .insert(slot.clone(), InstructionEffect::Active);
            state
                .operator_instruction_digest
                .insert(slot.clone(), DIGEST_A.to_string());
            state
                .operator_instruction_body_bytes
                .insert(slot.clone(), 1);
            state.operator_instruction_target_kind.insert(
                slot.clone(),
                if member.is_some() {
                    InstructionTargetKind::Member
                } else {
                    InstructionTargetKind::Mob
                },
            );
            state
                .operator_instruction_target_profile
                .insert(slot.clone(), String::new());
            if let Some(member) = member {
                state
                    .operator_instruction_target_member
                    .insert(slot, identity(member));
            }
        };
        for index in 0..shared {
            insert(shared_slot(index), None);
        }
        if let Some(member) = own {
            insert("[\"own\",\"a\"]".to_string(), Some(member));
        }
        state.operator_instruction_generation = 100;
        state
    }
    assert_eq!(
        rejected_invariant(with_records(65, None)),
        "operator_instruction_slot_count_bounded"
    );
    assert_eq!(
        rejected_invariant(with_records(64, Some("agent-a"))),
        "operator_instruction_slot_count_bounded"
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

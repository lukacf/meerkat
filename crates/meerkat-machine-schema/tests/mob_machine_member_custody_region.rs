//! Rows for the MobMachine member custody region.
//!
//! A mob start that cannot restore a seated member's custody for a cause of
//! that member's own (a store read that could not be served or decoded, a
//! hosting refusal, a durable tail it could not prove) records the member
//! degraded: which restore is unrestored and the cause its last attempt met.
//! The record goes when a re-attempt restores the custody or when the
//! member's seat ends, and a destroyed mob holds none. Degraded is not
//! Broken: a record changes no lifecycle, residency, wake or topology fact.
//! These rows drive the schema crate's own instantiation of the machine
//! through its signals, and the region's invariant through
//! `recover_from_state`.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use meerkat_machine_schema::catalog::dsl::dsl_mob_machine;
use meerkat_machine_schema::catalog::dsl::mob_machine::{
    AgentIdentity, AgentRuntimeId, FenceToken, Generation, MemberCustodyCauseKind,
    MemberCustodyRestoreKind, MobMachineAuthority, MobMachineInput, MobMachineMutator,
    MobMachineSignal, MobMachineState, MobMachineTransition, MobMachineTransitionError, MobPhase,
    PlacedCompletionLifecycleIntentKind, SessionId, SpawnPolicyRuntimeMode,
};

type Authority = MobMachineAuthority;
type Signal = MobMachineSignal;

const ARMS: [&str; 5] = [
    "RecordMemberCustodyUnrestoredSeated",
    "RecordMemberCustodyUnrestoredNotSeated",
    "RecordMemberCustodyUnrestoredDestroyed",
    "ResolveMemberCustodyRestoredRecorded",
    "ResolveMemberCustodyRestoredUnrecorded",
];

const INVARIANTS: [&str; 2] = [
    "custody_unrestored_members_are_seated",
    "destroyed_mob_holds_no_member_lifecycle",
];

fn identity(name: &str) -> AgentIdentity {
    AgentIdentity::from(name)
}

fn runtime_of(name: &str) -> AgentRuntimeId {
    AgentRuntimeId(format!("{name}-runtime"))
}

fn signal(authority: &mut Authority, signal: Signal) -> MobMachineTransition {
    authority
        .apply_signal(signal)
        .expect("the signal must resolve to exactly one arm")
}

/// Seat `name` the way the epoch replay does, bound to its session.
fn seat(authority: &mut Authority, name: &str) {
    signal(
        authority,
        Signal::RecoverRosterMember {
            agent_identity: identity(name),
            agent_runtime_id: runtime_of(name),
            fence_token: FenceToken(1),
            generation: Generation(1),
            profile_name: "worker".to_string(),
            runtime_mode: SpawnPolicyRuntimeMode::TurnDriven,
            external_addressable: false,
        },
    );
    signal(
        authority,
        Signal::RecoverMemberSessionBinding {
            agent_identity: identity(name),
            agent_runtime_id: runtime_of(name),
            bridge_session_id: SessionId(format!("session-{name}")),
            replacing: None,
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

fn record(name: &str, restore: MemberCustodyRestoreKind, cause: MemberCustodyCauseKind) -> Signal {
    Signal::RecordMemberCustodyUnrestored {
        agent_identity: identity(name),
        restore,
        cause,
    }
}

fn resolve(name: &str) -> Signal {
    Signal::ResolveMemberCustodyRestored {
        agent_identity: identity(name),
    }
}

/// Take a machine whose members are seated into `phase` the way a start of
/// a mob in that phase does.
fn enter(authority: &mut Authority, phase: MobPhase) {
    let (intent, input) = match phase {
        MobPhase::Running => return,
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
        authority,
        MobMachineInput::BeginPlacedCompletionLifecycleQuiesce { intent },
    )
    .expect("begin the lifecycle quiesce");
    MobMachineMutator::apply(authority, input).expect("enter the phase");
    assert_eq!(authority.state().lifecycle_phase, phase);
}

fn unrestored(
    authority: &Authority,
    name: &str,
) -> Option<(MemberCustodyRestoreKind, MemberCustodyCauseKind)> {
    let state = authority.state();
    let restore = state
        .member_custody_unrestored
        .get(&identity(name))
        .copied();
    let cause = state
        .member_custody_unrestored_causes
        .get(&identity(name))
        .copied();
    assert_eq!(
        restore.is_some(),
        cause.is_some(),
        "the restore and its cause are recorded together"
    );
    restore.zip(cause)
}

/// A seated member's unrestored custody is recorded in every live phase,
/// and the record changes nothing else: the member keeps its seat and the
/// mob its phase. A later record replaces the cause.
#[test]
fn a_seated_members_unrestored_custody_is_recorded_in_every_live_phase() {
    for phase in [MobPhase::Running, MobPhase::Stopped, MobPhase::Completed] {
        let mut authority = Authority::new();
        seat(&mut authority, "worker-1");
        enter(&mut authority, phase);
        let before = authority.state().clone();

        let transition = signal(
            &mut authority,
            record(
                "worker-1",
                MemberCustodyRestoreKind::CleanupDelivery,
                MemberCustodyCauseKind::StoreUnavailable,
            ),
        );
        assert!(
            transition.effects().is_empty(),
            "a record wakes, materializes and registers nothing: {:?}",
            transition.effects()
        );
        assert_eq!(
            unrestored(&authority, "worker-1"),
            Some((
                MemberCustodyRestoreKind::CleanupDelivery,
                MemberCustodyCauseKind::StoreUnavailable
            )),
            "{phase:?}"
        );
        let mut without_record = authority.state().clone();
        without_record.member_custody_unrestored.clear();
        without_record.member_custody_unrestored_causes.clear();
        assert_eq!(
            without_record, before,
            "in {phase:?} the record is the only change: degraded is not Broken"
        );

        signal(
            &mut authority,
            record(
                "worker-1",
                MemberCustodyRestoreKind::CleanupDelivery,
                MemberCustodyCauseKind::StoreUnreadable,
            ),
        );
        assert_eq!(
            unrestored(&authority, "worker-1"),
            Some((
                MemberCustodyRestoreKind::CleanupDelivery,
                MemberCustodyCauseKind::StoreUnreadable
            )),
            "a later record replaces the cause in {phase:?}"
        );
    }
}

/// A record for an identity the mob does not seat, and a record or resolve
/// in a destroyed mob, change nothing.
#[test]
fn a_record_for_an_unseated_identity_or_a_destroyed_mob_changes_nothing() {
    for phase in [MobPhase::Running, MobPhase::Stopped, MobPhase::Completed] {
        let mut authority = Authority::new();
        enter(&mut authority, phase);
        let before = authority.state().clone();
        signal(
            &mut authority,
            record(
                "stranger",
                MemberCustodyRestoreKind::StartupBinding,
                MemberCustodyCauseKind::HostingUnavailable,
            ),
        );
        assert_eq!(authority.state(), &before, "{phase:?}");
    }

    let mut destroyed = Authority::new();
    enter(&mut destroyed, MobPhase::Destroyed);
    let before = destroyed.state().clone();
    signal(
        &mut destroyed,
        record(
            "worker-1",
            MemberCustodyRestoreKind::CleanupDelivery,
            MemberCustodyCauseKind::StoreUnavailable,
        ),
    );
    signal(&mut destroyed, resolve("worker-1"));
    assert_eq!(destroyed.state(), &before);
}

/// A re-attempt that restored the custody resolves the record; a resolve
/// with nothing recorded changes nothing.
#[test]
fn a_restored_custody_resolves_its_record() {
    for phase in [MobPhase::Running, MobPhase::Stopped, MobPhase::Completed] {
        let mut authority = Authority::new();
        seat(&mut authority, "worker-1");
        enter(&mut authority, phase);
        let before = authority.state().clone();
        signal(
            &mut authority,
            record(
                "worker-1",
                MemberCustodyRestoreKind::CleanupDelivery,
                MemberCustodyCauseKind::StoreUnavailable,
            ),
        );
        signal(&mut authority, resolve("worker-1"));
        assert_eq!(unrestored(&authority, "worker-1"), None, "{phase:?}");
        assert_eq!(authority.state(), &before, "{phase:?}");

        signal(&mut authority, resolve("worker-1"));
        assert_eq!(authority.state(), &before, "a second resolve in {phase:?}");
    }
}

/// A member whose seat ends takes its record with it, and a destroy clears
/// every record.
#[test]
fn an_ended_seat_and_a_destroy_take_the_record() {
    let mut authority = Authority::new();
    seat(&mut authority, "worker-1");
    seat(&mut authority, "worker-2");
    for name in ["worker-1", "worker-2"] {
        signal(
            &mut authority,
            record(
                name,
                MemberCustodyRestoreKind::CleanupDelivery,
                MemberCustodyCauseKind::DurableTailUnprovable,
            ),
        );
    }
    unseat(&mut authority, "worker-1");
    assert_eq!(unrestored(&authority, "worker-1"), None);
    assert!(unrestored(&authority, "worker-2").is_some());

    enter(&mut authority, MobPhase::Destroyed);
    assert!(authority.state().member_custody_unrestored.is_empty());
    assert!(
        authority
            .state()
            .member_custody_unrestored_causes
            .is_empty()
    );
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

/// A reached state with a record recovers; a record for an identity the
/// mob does not seat, or a restore without its cause, does not.
#[test]
fn recovered_state_holds_records_only_for_seated_members_and_whole() {
    let mut authority = Authority::new();
    seat(&mut authority, "worker-1");
    signal(
        &mut authority,
        record(
            "worker-1",
            MemberCustodyRestoreKind::CleanupDelivery,
            MemberCustodyCauseKind::StoreUnavailable,
        ),
    );
    let reached = authority.state().clone();
    let recovered = Authority::recover_from_state(reached.clone())
        .unwrap_or_else(|error| panic!("a reached state recovers: {error}"));
    assert_eq!(recovered.state(), &reached);

    let mut unseated = reached.clone();
    unseated.member_custody_unrestored.insert(
        identity("stranger"),
        MemberCustodyRestoreKind::CleanupDelivery,
    );
    unseated.member_custody_unrestored_causes.insert(
        identity("stranger"),
        MemberCustodyCauseKind::StoreUnavailable,
    );
    assert_eq!(
        rejected_invariant(unseated),
        "custody_unrestored_members_are_seated"
    );

    let mut torn = reached;
    torn.member_custody_unrestored_causes.clear();
    assert_eq!(
        rejected_invariant(torn),
        "custody_unrestored_members_are_seated"
    );
}

#[test]
fn the_member_custody_region_declares_every_arm_and_invariant() {
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

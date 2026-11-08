//! Rows for the MobMachine member dispatch admission verdict.
//!
//! A host's tool dispatch admission is process-local, so no member build
//! copies one. Every build of a member whose lineage may run under one (a
//! fork seat, a respawn successor, a rebuild of a member seated from such a
//! lineage) is decided by `ResolveMemberDispatchAdmission` from what the
//! shell observed of the build's lineage and what the host's spawn
//! customizer derived for it: a lineage that carries an admission asks the
//! host first, inherits what it derived, and is refused when it derived
//! none; a lineage that is not known (a fork-derived member whose spawn row
//! records none, as rows written before they recorded one read) asks the
//! host first and takes what it derived, an admission or none, and is never
//! refused; any other lineage needs none. The lineage this machine records
//! for the input's identity at its current incarnation
//! (`member_dispatch_admission_lineage`, set by
//! `RecordMemberDispatchAdmissionLineage`) carries an admission whatever the
//! shell observed, and a record of an earlier incarnation never matches. The
//! verdict changes no state, in any phase. These rows drive the schema
//! crate's own instantiation of the machine through every phase.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use meerkat_machine_schema::catalog::dsl::dsl_mob_machine;
use meerkat_machine_schema::catalog::dsl::mob_machine::{
    AgentIdentity, AgentRuntimeId, FenceToken, Generation,
    MemberDispatchAdmissionHostAnswer as HostAnswer, MemberDispatchAdmissionLineage as Lineage,
    MemberDispatchAdmissionVerdict as Verdict, MobMachineAuthority, MobMachineEffect,
    MobMachineInput, MobMachineMutator, MobMachineSignal, MobPhase,
    PlacedCompletionLifecycleIntentKind, SpawnPolicyRuntimeMode,
};

type Authority = MobMachineAuthority;

/// The identity the verdict rows ask about when no record is involved: no
/// member is seated under it.
const UNSEATED: &str = "unseated-member";

const ARMS: [&str; 4] = [
    "ResolveMemberDispatchAdmissionAskHost",
    "ResolveMemberDispatchAdmissionInherit",
    "ResolveMemberDispatchAdmissionRefused",
    "ResolveMemberDispatchAdmissionNoneRequired",
];

const PHASES: [MobPhase; 4] = [
    MobPhase::Running,
    MobPhase::Stopped,
    MobPhase::Completed,
    MobPhase::Destroyed,
];

const LINEAGES: [Lineage; 7] = [
    Lineage::NoneRecorded,
    Lineage::Recorded,
    Lineage::SourceBuiltGated,
    Lineage::SourceBuiltUngated,
    Lineage::SourceRebuildGated,
    Lineage::SourceRebuildUngated,
    Lineage::Unknown,
];

const HOST_ANSWERS: [HostAnswer; 3] = [
    HostAnswer::NotAsked,
    HostAnswer::Derived,
    HostAnswer::NoneDerived,
];

/// Whether `lineage` runs under an admission: the fork source's build in
/// this process does, a durable record says so, or the host's answer for the
/// source's next build carries one.
fn carries_admission(lineage: Lineage) -> bool {
    matches!(
        lineage,
        Lineage::Recorded | Lineage::SourceBuiltGated | Lineage::SourceRebuildGated
    )
}

/// The verdict the shell mirrors for one pair.
fn expected(lineage: Lineage, host_answer: HostAnswer) -> Verdict {
    if lineage == Lineage::Unknown {
        return match host_answer {
            HostAnswer::NotAsked => Verdict::AskHost,
            HostAnswer::Derived => Verdict::Inherit,
            HostAnswer::NoneDerived => Verdict::NoneRequired,
        };
    }
    if !carries_admission(lineage) {
        return Verdict::NoneRequired;
    }
    match host_answer {
        HostAnswer::NotAsked => Verdict::AskHost,
        HostAnswer::Derived => Verdict::Inherit,
        HostAnswer::NoneDerived => Verdict::Refused,
    }
}

/// Take a fresh machine into `phase` the way a mob in that phase gets there.
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

/// The verdict the machine answers `lineage` and `host_answer` with for an
/// identity no member holds, and the effects that answer it.
fn resolve(
    authority: &mut Authority,
    lineage: Lineage,
    host_answer: HostAnswer,
) -> (Verdict, Vec<MobMachineEffect>) {
    resolve_for(authority, UNSEATED, lineage, host_answer)
}

/// The verdict the machine answers `lineage` and `host_answer` with for
/// `member`, and the effects that answer it.
fn resolve_for(
    authority: &mut Authority,
    member: &str,
    lineage: Lineage,
    host_answer: HostAnswer,
) -> (Verdict, Vec<MobMachineEffect>) {
    let transition = MobMachineMutator::apply(
        authority,
        MobMachineInput::ResolveMemberDispatchAdmission {
            agent_identity: AgentIdentity::from(member),
            lineage,
            host_answer,
        },
    )
    .unwrap_or_else(|error| {
        panic!("{lineage:?} with {host_answer:?} must resolve to exactly one arm: {error}")
    });
    let effects = transition.effects().to_vec();
    let verdicts = effects
        .iter()
        .filter_map(|effect| match effect {
            MobMachineEffect::MemberDispatchAdmissionResolved {
                lineage: echoed_lineage,
                host_answer: echoed_answer,
                verdict,
            } => {
                assert_eq!(
                    (*echoed_lineage, *echoed_answer),
                    (lineage, host_answer),
                    "the verdict echoes the observations it was decided on"
                );
                Some(*verdict)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        verdicts.len(),
        1,
        "exactly one verdict answers {lineage:?} with {host_answer:?}: {effects:?}"
    );
    (verdicts[0], effects)
}

/// Every pair takes its verdict in every phase, and the verdict is the only
/// effect and changes no state: the machine's phase and every fact stay as
/// they were.
#[test]
fn every_pair_takes_its_verdict_in_every_phase_and_changes_nothing() {
    for phase in PHASES {
        let mut authority = Authority::new();
        enter(&mut authority, phase);
        let before = authority.state().clone();
        for lineage in LINEAGES {
            for host_answer in HOST_ANSWERS {
                let (verdict, effects) = resolve(&mut authority, lineage, host_answer);
                assert_eq!(
                    verdict,
                    expected(lineage, host_answer),
                    "{lineage:?} with {host_answer:?} in {phase:?}"
                );
                assert_eq!(
                    effects.len(),
                    1,
                    "the verdict is the only effect in {phase:?}: {effects:?}"
                );
                assert_eq!(
                    authority.state(),
                    &before,
                    "{lineage:?} with {host_answer:?} in {phase:?} changes no state"
                );
            }
        }
    }
}

/// A lineage that runs under an admission is never built without one: the
/// host is asked first, the build inherits only an admission the host
/// derived, and a build the host derived none for is refused.
#[test]
fn a_gated_lineage_is_never_built_without_an_admission() {
    let mut authority = Authority::new();
    for lineage in LINEAGES
        .into_iter()
        .filter(|lineage| carries_admission(*lineage))
    {
        assert_eq!(
            resolve(&mut authority, lineage, HostAnswer::NotAsked).0,
            Verdict::AskHost,
            "{lineage:?}: the host is asked before the build"
        );
        assert_eq!(
            resolve(&mut authority, lineage, HostAnswer::Derived).0,
            Verdict::Inherit,
            "{lineage:?}: the build runs under the admission the host derived"
        );
        assert_eq!(
            resolve(&mut authority, lineage, HostAnswer::NoneDerived).0,
            Verdict::Refused,
            "{lineage:?}: a build the host derived none for is refused"
        );
    }
}

/// A lineage that carries no admission needs none: the build takes what its
/// request carries, whatever the host answered, and the host need not be
/// asked.
#[test]
fn an_ungated_lineage_needs_no_admission() {
    let mut authority = Authority::new();
    for lineage in LINEAGES
        .into_iter()
        .filter(|lineage| !carries_admission(*lineage) && *lineage != Lineage::Unknown)
    {
        for host_answer in HOST_ANSWERS {
            assert_eq!(
                resolve(&mut authority, lineage, host_answer).0,
                Verdict::NoneRequired,
                "{lineage:?} with {host_answer:?}"
            );
        }
    }
}

/// A lineage that is not known is built as a fresh spawn the host's
/// customizer answers: the host is asked first, the build runs under the
/// admission it derived, and under none when it derived none. It is never
/// refused, so a host without a customizer builds it as before.
#[test]
fn an_unknown_lineage_asks_the_host_and_runs_under_its_answer() {
    let mut authority = Authority::new();
    assert_eq!(
        resolve(&mut authority, Lineage::Unknown, HostAnswer::NotAsked).0,
        Verdict::AskHost,
        "the host is asked before the build"
    );
    assert_eq!(
        resolve(&mut authority, Lineage::Unknown, HostAnswer::Derived).0,
        Verdict::Inherit,
        "the build runs under the admission the host derived"
    );
    assert_eq!(
        resolve(&mut authority, Lineage::Unknown, HostAnswer::NoneDerived).0,
        Verdict::NoneRequired,
        "a build the host derived none for runs under none, and is not refused"
    );
}

#[test]
fn the_dispatch_admission_region_declares_every_arm_in_every_phase() {
    let schema = dsl_mob_machine();
    for arm in ARMS {
        for phase in ["Running", "Stopped", "Completed", "Destroyed"] {
            let name = format!("{arm}{phase}");
            assert!(
                schema
                    .transitions
                    .iter()
                    .any(|transition| transition.name.as_str() == name),
                "MobMachine must declare {name}"
            );
        }
    }
    for phase in ["Running", "Stopped"] {
        let name = format!("RecordMemberDispatchAdmissionLineage{phase}");
        assert!(
            schema
                .transitions
                .iter()
                .any(|transition| transition.name.as_str() == name),
            "MobMachine must declare {name}"
        );
    }
}

fn runtime_of(member: &str, generation: u64) -> AgentRuntimeId {
    AgentRuntimeId(format!("{member}-runtime-{generation}"))
}

/// Seat incarnation `generation` of `member` the way the epoch replay does.
fn seat(authority: &mut Authority, member: &str, generation: u64) {
    authority
        .apply_signal(MobMachineSignal::RecoverRosterMember {
            agent_identity: AgentIdentity::from(member),
            agent_runtime_id: runtime_of(member, generation),
            fence_token: FenceToken(generation),
            generation: Generation(generation),
            profile_name: "worker".to_string(),
            runtime_mode: SpawnPolicyRuntimeMode::TurnDriven,
            external_addressable: false,
        })
        .expect("seat the member");
}

/// Retire incarnation `generation` of `member` the way the epoch replay does.
fn retire(authority: &mut Authority, member: &str, generation: u64) {
    authority
        .apply_signal(MobMachineSignal::RecoverRosterMemberRetired {
            agent_identity: AgentIdentity::from(member),
            agent_runtime_id: runtime_of(member, generation),
            generation: Generation(generation),
            preserve_machine_topology: false,
            preservation_started: false,
            session_archived: true,
            hosted_incarnation: false,
        })
        .expect("retire the member");
}

/// Record that incarnation `generation` of `member` runs under a tool
/// dispatch admission.
fn record(
    authority: &mut Authority,
    member: &str,
    generation: u64,
) -> Result<(), meerkat_machine_schema::catalog::dsl::mob_machine::MobMachineTransitionError> {
    MobMachineMutator::apply(
        authority,
        MobMachineInput::RecordMemberDispatchAdmissionLineage {
            agent_identity: AgentIdentity::from(member),
            agent_runtime_id: runtime_of(member, generation),
        },
    )
    .map(|_| ())
}

/// The lineage this machine records for a member's current incarnation
/// carries an admission whatever the shell observed: the host is asked, the
/// build inherits only what it derived, and a build it derived none for is
/// refused, an Unknown or ungated observation included.
#[test]
fn a_recorded_lineage_carries_an_admission_whatever_the_shell_observed() {
    let mut authority = Authority::new();
    seat(&mut authority, "fork-child", 1);
    record(&mut authority, "fork-child", 1).expect("record the seated incarnation");
    for lineage in LINEAGES {
        assert_eq!(
            resolve_for(&mut authority, "fork-child", lineage, HostAnswer::NotAsked).0,
            Verdict::AskHost,
            "{lineage:?}: the host is asked"
        );
        assert_eq!(
            resolve_for(&mut authority, "fork-child", lineage, HostAnswer::Derived).0,
            Verdict::Inherit,
            "{lineage:?}: the build runs under the derived admission"
        );
        assert_eq!(
            resolve_for(
                &mut authority,
                "fork-child",
                lineage,
                HostAnswer::NoneDerived
            )
            .0,
            Verdict::Refused,
            "{lineage:?}: a build the host derived none for is refused"
        );
    }
}

/// A record names one incarnation: after the recorded incarnation is
/// retired and a new one of the identity is seated, the record does not
/// match, and the new incarnation takes the verdict of its observation
/// alone.
#[test]
fn a_record_of_an_earlier_incarnation_never_matches() {
    let mut authority = Authority::new();
    seat(&mut authority, "fork-child", 1);
    record(&mut authority, "fork-child", 1).expect("record the seated incarnation");
    retire(&mut authority, "fork-child", 1);
    seat(&mut authority, "fork-child", 2);
    for lineage in LINEAGES {
        for host_answer in HOST_ANSWERS {
            assert_eq!(
                resolve_for(&mut authority, "fork-child", lineage, host_answer).0,
                expected(lineage, host_answer),
                "{lineage:?} with {host_answer:?} for the new incarnation"
            );
        }
    }
}

/// Only the identity's current, local incarnation is recorded: a record
/// that names another incarnation, or a member not seated, is refused and
/// records nothing.
#[test]
fn only_the_current_incarnation_is_recorded() {
    let mut authority = Authority::new();
    assert!(
        record(&mut authority, "fork-child", 1).is_err(),
        "a member not seated is not recorded"
    );
    seat(&mut authority, "fork-child", 1);
    assert!(
        record(&mut authority, "fork-child", 2).is_err(),
        "another incarnation is not recorded"
    );
    assert_eq!(
        resolve_for(
            &mut authority,
            "fork-child",
            Lineage::NoneRecorded,
            HostAnswer::NotAsked
        )
        .0,
        Verdict::NoneRequired,
        "nothing was recorded"
    );
}

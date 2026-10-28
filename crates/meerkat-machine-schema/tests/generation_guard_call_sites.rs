#![allow(clippy::expect_used, clippy::panic)]

//! MobMachine mints a member generation one past the larger of the retained
//! generation and the recovered high-water
//! (`mob_machine_next_minted_generation`), and replays stored history with
//! any generation at or above the retained one
//! (`mob_machine_next_respawn_generation`). The bounded TLC audit checks the
//! minted helper against the high-water, but not which helper each guard
//! calls, so this ratchet pins the call sites: a mint guard left on the
//! replay helper would mint a generation an earlier reset epoch already used.
//! The replay helper is pinned by use, not only by guard name: no other
//! guard, update or emit of any transition reads it.

use meerkat_machine_schema::{MachineSchema, TransitionSchema, canonical_machine_schemas};

const MINTED: &str = "helper: \"mob_machine_next_minted_generation\"";
const REPLAYED: &str = "helper: \"mob_machine_next_respawn_generation\"";

fn mob_machine() -> MachineSchema {
    canonical_machine_schemas()
        .into_iter()
        .find(|machine| machine.machine.as_str() == "MobMachine")
        .expect("MobMachine")
}

/// Each guard called `name`, as (transition, guard expression in Debug form).
fn guards_named(machine: &MachineSchema, name: &str) -> Vec<(String, String)> {
    machine
        .transitions
        .iter()
        .flat_map(|transition| {
            transition
                .guards
                .iter()
                .filter(|guard| guard.name == name)
                .map(|guard| {
                    (
                        transition.name.as_str().to_string(),
                        format!("{:?}", guard.expr),
                    )
                })
        })
        .collect()
}

fn transition_names(sites: &[(String, String)]) -> Vec<&str> {
    sites
        .iter()
        .map(|(transition, _)| transition.as_str())
        .collect()
}

#[test]
fn every_mint_guard_reads_the_minted_generation() {
    let machine = mob_machine();
    let mints = guards_named(&machine, "generation_is_next");
    assert_eq!(
        transition_names(&mints),
        [
            "CommitSpawnMembershipFresh",
            "CommitSpawnMembershipFreshPeerOnly",
            "CommitSpawnMembershipRemote",
            "BeginSpawnExecFresh",
            "BeginSpawnExecFreshPeerOnly",
            "BeginSpawnExecRemote",
            "CommitDormantSeatRunning",
            "CommitMemberRotationLiveRunning",
            "CommitMemberRotationLiveStopped",
            "CommitMemberRotationRetiredRunning",
            "CommitMemberRotationRetiredStopped",
            "CommitMemberRotationLiveKickoffRunning",
            "CommitMemberRotationLiveKickoffStopped",
            "CommitMemberRotationRetiredKickoffRunning",
            "CommitMemberRotationRetiredKickoffStopped",
        ],
        "the mint sites changed: pin each new one here"
    );
    for (transition, expr) in &mints {
        assert!(
            expr.contains(MINTED) && !expr.contains(REPLAYED),
            "{transition}: generation_is_next must read mob_machine_next_minted_generation only, \
             got {expr}"
        );
    }
}

#[test]
fn the_respawn_probe_reads_the_minted_generation() {
    let machine = mob_machine();
    let probes: Vec<&TransitionSchema> = machine
        .transitions
        .iter()
        .filter(|transition| {
            transition
                .name
                .as_str()
                .starts_with("ComputeRespawnGeneration")
        })
        .collect();
    assert_eq!(probes.len(), 4, "one probe arm per lifecycle phase");
    for probe in probes {
        let guards = format!("{:?}", probe.guards);
        let emits = format!("{:?}", probe.emit);
        assert!(
            guards.contains(MINTED) && !guards.contains(REPLAYED),
            "{}: the probe's guard must read the minted generation, got {guards}",
            probe.name.as_str()
        );
        assert!(
            emits.contains(MINTED) && !emits.contains(REPLAYED),
            "{}: the probe must answer the minted generation, got {emits}",
            probe.name.as_str()
        );
    }
}

#[test]
fn every_replay_guard_ascends_from_the_retained_generation() {
    let machine = mob_machine();
    let replays = guards_named(&machine, "generation_ascends");
    assert_eq!(
        transition_names(&replays),
        [
            "RecoverCommittedPlacedSpawnRunning",
            "RecoverRosterMemberRunning",
            "RecoverRosterMemberResetRunning",
        ],
        "the replay sites changed: pin each new one here"
    );
    for (transition, expr) in &replays {
        assert!(
            expr.contains(REPLAYED) && !expr.contains(MINTED),
            "{transition}: generation_ascends must read mob_machine_next_respawn_generation only, \
             got {expr}"
        );
    }
}

/// The replay helper is read only inside the three pinned replay guards: a
/// transition whose guard has another name, or that reads it in an update
/// or an emit, fails here even though the name-keyed pins above pass.
#[test]
fn only_the_pinned_replay_guards_read_the_replay_helper() {
    let machine = mob_machine();
    let readers: Vec<(String, usize, usize)> = machine
        .transitions
        .iter()
        .filter_map(|transition| {
            let reads = format!("{transition:?}").matches(REPLAYED).count();
            let in_replay_guards = transition
                .guards
                .iter()
                .filter(|guard| guard.name == "generation_ascends")
                .map(|guard| format!("{:?}", guard.expr).matches(REPLAYED).count())
                .sum();
            (reads > 0).then(|| {
                (
                    transition.name.as_str().to_string(),
                    reads,
                    in_replay_guards,
                )
            })
        })
        .collect();
    assert_eq!(
        readers
            .iter()
            .map(|(transition, _, _)| transition.as_str())
            .collect::<Vec<_>>(),
        [
            "RecoverCommittedPlacedSpawnRunning",
            "RecoverRosterMemberRunning",
            "RecoverRosterMemberResetRunning",
        ],
        "a transition outside the pinned replay sites reads the replay helper: {readers:?}"
    );
    for (transition, reads, in_replay_guards) in &readers {
        assert_eq!(
            reads, in_replay_guards,
            "{transition}: the replay helper is read outside its generation_ascends guard"
        );
    }
}

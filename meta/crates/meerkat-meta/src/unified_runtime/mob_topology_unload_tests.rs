//! Rows for the topology control of a mob a host has stored and not loaded.
//!
//! A mob beside the default mob is loaded by the request that names it and
//! unloaded when it is idle. Its topology controller is made with its entry
//! at the load and closed with it at the unload, and its state file is
//! opened by nothing else: not by a start, not while the mob is not loaded.
//! The fixture is the one of the topology rows of a loaded mob
//! ([`TopologyHost`]), a launch that declares its mob to its mob set over a
//! state directory with a topology policy and a state file; the mob set's
//! unload is asked as the rows of the mobs that are not loaded ask it.

#![allow(clippy::expect_used)]

use std::path::PathBuf;
use std::time::Duration;

use meerkat_mob::{AgentIdentity, MobId, MobRuntimeMode, ProfileName};

use super::UnifiedRuntime;
use super::mob_entries::lazy_start_tests::{asleep, known};
use super::mob_entries_tests::{definition, second_mob, set_of, shut_down};
use super::mob_topology_tests::{
    OtherMobs, TopologyHost, alice_bob_in, connect_alice_bob, editable, stdio,
};
use crate::topology_control::{TopologyControlError, TopologyController};

/// Seat `alice` and `bob` in the mob `mob_id` of `host`, turn-driven: the
/// mob is idle between their turns, so its mob set unloads it when asked.
async fn seat_turn_driven(host: &UnifiedRuntime, mob_id: &MobId) {
    let set = set_of(host);
    for member in ["alice", "bob"] {
        let seated = set
            .mob_spawn(
                mob_id,
                ProfileName::from("worker"),
                AgentIdentity::from(member),
                Some(MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await;
        assert!(
            seated.is_ok(),
            "the mob {mob_id} seats {member}: {:?}",
            seated.as_ref().err()
        );
    }
}

/// Have an operator connect `alice` and `bob` in the mob `mob_id` of
/// `host` at revision 0.
async fn connect_in(host: &UnifiedRuntime, mob_id: &MobId, key: &str) {
    let mob = host
        .resolve_mob(Some(mob_id))
        .await
        .expect("the second mob of the host resolves");
    let applied = mob
        .topology_runtime_handle()
        .apply(connect_alice_bob(0, key), "operator")
        .await;
    assert!(
        applied.is_ok(),
        "an apply in the second mob: {:?}",
        applied.as_ref().err()
    );
}

/// Require that a request for the mob `mob_id` of `host` resolves it, and
/// that a read of the mob then answers the operator's connection of `alice`
/// and `bob`, at revision 1.
async fn reads_the_operators_connection(host: &UnifiedRuntime, mob_id: &MobId, when: &str) {
    let resolved = host.resolve_mob(Some(mob_id)).await;
    assert!(
        resolved.is_ok(),
        "{when}: the second mob resolves: {:?}",
        resolved.as_ref().err()
    );
    if let Ok(mob) = resolved {
        let read = mob.topology_runtime_handle().query().await;
        assert!(
            read.as_ref().is_ok_and(|snapshot| {
                snapshot.revision == 1
                    && alice_bob_in(snapshot).is_some_and(|edge| edge.operator_added)
            }),
            "{when}: the operator's connection is read back at revision 1: {read:?}"
        );
        assert_eq!(
            mob.topology_controller().revision().await,
            1,
            "{when}: the second mob's revision is read back"
        );
    }
}

/// The one topology state file of the mob `mob_id` under `fixture`'s state
/// directory.
fn the_state_file_of(fixture: &TopologyHost, mob_id: &MobId) -> PathBuf {
    let files = fixture.state_files_of(mob_id);
    assert_eq!(
        files.len(),
        1,
        "the second mob keeps one topology state file: {files:?}"
    );
    files.into_iter().next().unwrap_or_default()
}

/// A second mob's topology survives an unload and a restart: an operator's
/// connection made while the mob was loaded is read back, at its revision,
/// by the request that loads the mob again after its idle unload, and by
/// the first request for it after a restart of the host, which loads no
/// stored mob at its start.
#[tokio::test]
async fn second_mob_topology_survives_unload_and_restart() {
    let fixture = TopologyHost::new();
    let first = fixture.launch(editable(), OtherMobs::Stored).await;
    let second_id = second_mob(&first, definition("second")).await;
    seat_turn_driven(&first, &second_id).await;
    connect_in(&first, &second_id, "before-the-unload").await;

    asleep(&first, &[&second_id]).await;
    reads_the_operators_connection(&first, &second_id, "after the unload").await;
    shut_down(&first, &[]).await;
    drop(first);

    let host = fixture.launch(editable(), OtherMobs::Stored).await;
    let at_start = known(&host).await;
    assert!(
        at_start.not_loaded.contains(&second_id) && !at_start.loaded.contains(&second_id),
        "a start does not load the second mob: loaded {:?}, not loaded {:?}",
        at_start.loaded,
        at_start.not_loaded
    );
    reads_the_operators_connection(&host, &second_id, "after the restart").await;
    assert_eq!(
        host.topology_controller().revision().await,
        0,
        "the default mob's revision is its own"
    );
    shut_down(&host, &[]).await;
}

/// A start opens no topology state file of a mob it does not load. After a
/// restart the second mob is known and not loaded, and its state file is
/// free: an open of it gets its lock. The request that loads the mob opens
/// the file, and from then on another open is refused.
#[tokio::test]
async fn start_opens_no_topology_file_of_a_dormant_mob() {
    let fixture = TopologyHost::new();
    let first = fixture.launch(editable(), OtherMobs::Stored).await;
    let second_id = second_mob(&first, definition("second")).await;
    let served = first.resolve_mob(Some(&second_id)).await.is_ok();
    assert!(served, "the second mob of the host resolves");
    let file = the_state_file_of(&fixture, &second_id);
    shut_down(&first, &[]).await;
    drop(first);

    let host = fixture.launch(editable(), OtherMobs::Stored).await;
    let at_start = known(&host).await;
    assert!(
        at_start.not_loaded.contains(&second_id) && !at_start.loaded.contains(&second_id),
        "a start does not load the second mob: loaded {:?}, not loaded {:?}",
        at_start.loaded,
        at_start.not_loaded
    );
    let free = TopologyController::load_or_default(editable(), &file).map(|_| ());
    assert!(
        free.is_ok(),
        "a start holds no lock on the state file of a mob that is not loaded: {free:?}"
    );

    let resolved = host.resolve_mob(Some(&second_id)).await;
    assert!(
        resolved.is_ok(),
        "a request loads the second mob: {:?}",
        resolved.as_ref().err()
    );
    let owned = TopologyController::load_or_default(editable(), &file).map(|_| ());
    assert!(
        matches!(owned, Err(TopologyControlError::Persistence(_))),
        "the load of the mob opened its state file, so another open is refused: {owned:?}"
    );
    drop(resolved);
    shut_down(&host, &[]).await;
}

/// A topology read that names a mob that is not loaded loads that mob and
/// answers its topology: the stdio query of a mob the set unloaded answers
/// that mob's revision and its operator's connection, and the mob is
/// loaded after it.
#[tokio::test]
async fn topology_query_wakes_a_dormant_mob() {
    let fixture = TopologyHost::new();
    let host = fixture.launch(editable(), OtherMobs::Stored).await;
    let second_id = second_mob(&host, definition("second")).await;
    seat_turn_driven(&host, &second_id).await;
    connect_in(&host, &second_id, "before-the-unload").await;
    asleep(&host, &[&second_id]).await;

    let answered = stdio(
        &host,
        "meta/topology/query",
        serde_json::json!({"mob_id": second_id.as_str()}),
    )
    .await;
    assert_eq!(
        answered["result"]["authority"],
        second_id.as_str(),
        "the query answers the mob it names: {answered}"
    );
    assert_eq!(
        answered["result"]["revision"], 1,
        "the query reads the mob's own revision: {answered}"
    );
    let after = known(&host).await;
    assert!(
        after.loaded.contains(&second_id),
        "the query loaded the mob it named: loaded {:?}",
        after.loaded
    );
    shut_down(&host, &[]).await;
}

/// The journal of an apply that was interrupted in a mob that is then not
/// loaded waits in that mob's state file, and is recovered when the mob is
/// loaded again and read: the read rolls the interrupted disconnect back,
/// so the mob answers the revision and the operator's connection it had
/// before, and no journal is left.
#[tokio::test]
async fn pending_journal_of_a_dormant_mob_recovers_on_activation() {
    let fixture = TopologyHost::new();
    let host = fixture.launch(editable(), OtherMobs::Stored).await;
    let second_id = second_mob(&host, definition("second")).await;
    seat_turn_driven(&host, &second_id).await;
    connect_in(&host, &second_id, "before-the-unload").await;
    let file = the_state_file_of(&fixture, &second_id);
    asleep(&host, &[&second_id]).await;

    // The mob set unloaded the mob; the runtime's entry of it goes on the
    // runtime's own task and gives the file up then. Until then an open of
    // the file is refused and writes nothing.
    let gives_up_within = tokio::time::Instant::now() + Duration::from_secs(20);
    let interrupted = loop {
        let attempt = TopologyController::interrupt_a_disconnect_in_tests(&file).await;
        if attempt.is_ok() || tokio::time::Instant::now() >= gives_up_within {
            break attempt;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert!(
        interrupted.is_ok(),
        "the state file of the mob that is not loaded takes an interrupted journal: \
         {interrupted:?}"
    );
    let waiting = TopologyController::load_or_default(editable(), &file);
    let waits = match &waiting {
        Ok(controller) => controller.has_pending().await && controller.revision().await == 2,
        Err(_) => false,
    };
    assert!(
        waits,
        "the journal waits in the file while the mob is not loaded: {:?}",
        waiting.as_ref().err()
    );
    drop(waiting);

    reads_the_operators_connection(&host, &second_id, "after the journal's recovery").await;
    let left = match host.resolve_mob(Some(&second_id)).await {
        Ok(mob) => mob.topology_controller().has_pending().await,
        Err(_) => true,
    };
    assert!(!left, "the read recovered the journal: none is left");
    shut_down(&host, &[]).await;
}

/// A load right after an unload gets the lock of the mob's state file,
/// also while an earlier request still holds a clone of the unloaded mob's
/// controller: the new entry's controller keeps the state in the file and
/// reads it, and the earlier clone is refused and writes nothing.
#[tokio::test]
async fn a_load_right_after_an_unload_gets_the_lock_of_the_mobs_topology_state() {
    let fixture = TopologyHost::new();
    let host = fixture.launch(editable(), OtherMobs::Stored).await;
    let second_id = second_mob(&host, definition("second")).await;
    seat_turn_driven(&host, &second_id).await;
    connect_in(&host, &second_id, "before-the-unload").await;
    let earlier = host
        .resolve_mob(Some(&second_id))
        .await
        .expect("the second mob of the host resolves")
        .topology_controller()
        .clone();
    let file = the_state_file_of(&fixture, &second_id);
    asleep(&host, &[&second_id]).await;

    let resolved = host.resolve_mob(Some(&second_id)).await;
    assert!(
        resolved.is_ok(),
        "a request loads the second mob again: {:?}",
        resolved.as_ref().err()
    );
    if let Ok(mob) = resolved {
        assert!(
            !mob.topology_controller()
                .is_the_same_controller_in_tests(&earlier),
            "the mob that was loaded again has a controller of its own"
        );
        assert!(
            mob.topology_controller().keeps_durable_state(),
            "the new controller got the lock: it keeps the mob's state in the file"
        );
        assert_eq!(
            mob.topology_controller().revision().await,
            1,
            "and reads what the mob kept"
        );
    }
    let before = std::fs::read(&file).expect("the state file reads");
    let late = earlier.bind_authority(second_id.to_string()).await;
    assert!(
        matches!(late, Err(TopologyControlError::Persistence(_))),
        "the earlier clone is refused: {late:?}"
    );
    assert!(
        !earlier.keeps_durable_state(),
        "the earlier clone gave the file up"
    );
    assert_eq!(
        std::fs::read(&file).expect("the state file reads"),
        before,
        "the earlier clone wrote nothing"
    );
    shut_down(&host, &[]).await;
}

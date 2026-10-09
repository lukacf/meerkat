//! #1497 P3-B: a fork job's terminal outcome is committed once, for a job the
//! mob spawned, and validated on every read: a corrupt or duplicated event
//! can never rewrite it.

use super::*;
use crate::event::{ForkJobTerminalEvent, detached_outcome_digest};

fn fork_job(job_id: &str) -> ForkJobRecord {
    ForkJobRecord {
        job_id: job_id.to_string(),
        owner_session_id: meerkat_core::SessionId::new(),
        started_at_ms: 1,
        max_run_ms: None,
        prefix_message_count: 0,
        result_label: "fork_off_result".to_string(),
        max_text_bytes: 1024,
        turn_delivery: None,
        retained_work: None,
    }
}

fn terminal(job: &ForkJobRecord, child: &AgentIdentity, text: &str) -> ForkJobTerminalEvent {
    let outcome = serde_json::json!({ "status": "completed", "text": text });
    ForkJobTerminalEvent {
        job_id: job.job_id.clone(),
        child: child.clone(),
        owner_session_id: job.owner_session_id.clone(),
        retained_work: None,
        status: meerkat_core::event::BackgroundJobTerminalStatus::Completed,
        result_digest: detached_outcome_digest(&outcome),
        outcome,
    }
}

/// A mob over `events` with `child` spawned for `job`.
async fn mob_with_fork_job(
    events: &Arc<InMemoryMobEventStore>,
    child: &AgentIdentity,
    job: &ForkJobRecord,
) -> MobHandle {
    let (handle, _service) = create_test_mob_with_events(
        sample_definition(),
        Arc::clone(events) as Arc<dyn MobEventStore>,
    )
    .await;
    let mut spawned = crate::event::MemberSpawnedEvent::new(
        child.clone(),
        Generation::INITIAL,
        FenceToken::new(0),
        AgentRuntimeId::initial(child.clone()),
        ProfileName::from("worker"),
    );
    spawned.fork_job = Some(job.clone());
    events
        .append(NewMobEvent {
            mob_id: handle.mob_id().clone(),
            timestamp: None,
            kind: MobEventKind::MemberSpawned(spawned),
        })
        .await
        .expect("append the fork child's spawn");
    handle
}

#[tokio::test]
async fn a_fork_job_terminal_is_recorded_once_and_never_rewritten() {
    let events = Arc::new(InMemoryMobEventStore::new());
    let child = AgentIdentity::from("child-x");
    let job = fork_job("job-1");
    let handle = mob_with_fork_job(&events, &child, &job).await;

    let unknown = terminal(&fork_job("job-unknown"), &child, "done");
    assert!(matches!(
        handle.record_fork_job_terminal(unknown).await,
        Err(ForkJobTerminalError::UnknownJob { job_id, .. }) if job_id == "job-unknown"
    ));
    let other_child = terminal(&job, &AgentIdentity::from("child-y"), "done");
    assert!(
        matches!(
            handle.record_fork_job_terminal(other_child).await,
            Err(ForkJobTerminalError::UnknownJob { .. })
        ),
        "a job id names a job only with its child"
    );

    let first = terminal(&job, &child, "done");
    handle
        .record_fork_job_terminal(first.clone())
        .await
        .expect("record the terminal");
    handle
        .record_fork_job_terminal(first.clone())
        .await
        .expect("the same terminal again is a no-op");
    let mut forged = first.clone();
    forged.outcome = serde_json::json!({ "status": "completed", "text": "forged" });
    assert!(matches!(
        handle.record_fork_job_terminal(forged.clone()).await,
        Err(ForkJobTerminalError::MismatchedDigest { .. })
    ));
    forged.result_digest = detached_outcome_digest(&forged.outcome);
    assert!(matches!(
        handle.record_fork_job_terminal(forged).await,
        Err(ForkJobTerminalError::ConflictingTerminal { .. })
    ));
    assert_eq!(
        handle
            .fork_job_terminal(&job.job_id, &child)
            .await
            .expect("read the terminal"),
        Some(first.clone())
    );
    assert_eq!(
        handle.fork_job_terminals().await.expect("read terminals"),
        vec![first]
    );
}

/// Validation runs on replay too: a second terminal already in the stream
/// with another outcome fails the read, typed, instead of standing as an
/// outcome.
#[tokio::test]
async fn replaying_a_conflicting_terminal_is_refused() {
    let child = AgentIdentity::from("child-x");
    let job = fork_job("job-1");

    let events = Arc::new(InMemoryMobEventStore::new());
    let handle = mob_with_fork_job(&events, &child, &job).await;
    let first = terminal(&job, &child, "done");
    handle
        .record_fork_job_terminal(first)
        .await
        .expect("record the terminal");
    events
        .append(NewMobEvent {
            mob_id: handle.mob_id().clone(),
            timestamp: None,
            kind: MobEventKind::ForkJobTerminal(terminal(&job, &child, "rewritten")),
        })
        .await
        .expect("append a conflicting terminal behind the handle's back");
    assert!(matches!(
        handle.fork_job_terminal(&job.job_id, &child).await,
        Err(ForkJobTerminalError::ConflictingTerminal { .. })
    ));
}

/// A terminal already in the stream that names no spawned job fails the
/// read, typed.
#[tokio::test]
async fn replaying_a_terminal_for_no_spawned_job_is_refused() {
    let child = AgentIdentity::from("child-x");
    let job = fork_job("job-1");
    let events = Arc::new(InMemoryMobEventStore::new());
    let handle = mob_with_fork_job(&events, &child, &job).await;
    events
        .append(NewMobEvent {
            mob_id: handle.mob_id().clone(),
            timestamp: None,
            kind: MobEventKind::ForkJobTerminal(terminal(&fork_job("job-unknown"), &child, "x")),
        })
        .await
        .expect("append a terminal for no spawned job");
    assert!(matches!(
        handle.fork_job_terminals().await,
        Err(ForkJobTerminalError::UnknownJob { .. })
    ));
}

/// What closes the race between two recorders: the store checks and appends
/// in one step. A recorder whose ledger read went stale (another recorder
/// committed a different outcome after it) is refused at the append, and the
/// stream keeps the one committed outcome.
#[tokio::test]
async fn a_recorder_with_a_stale_read_is_refused_at_the_append() {
    let child = AgentIdentity::from("child-x");
    let job = fork_job("job-1");
    let events = Arc::new(InMemoryMobEventStore::new());
    let handle = mob_with_fork_job(&events, &child, &job).await;

    // Recorder A reads: nothing recorded yet.
    assert!(
        handle
            .fork_job_terminals()
            .await
            .expect("read terminals")
            .is_empty()
    );
    // Recorder B commits its outcome first.
    let winner = terminal(&job, &child, "winner");
    handle
        .record_fork_job_terminal(winner.clone())
        .await
        .expect("record the winner");
    // A's append, after its stale read, is refused.
    assert!(matches!(
        events
            .append_fork_job_terminal_if_absent(NewMobEvent {
                mob_id: handle.mob_id().clone(),
                timestamp: None,
                kind: MobEventKind::ForkJobTerminal(terminal(&job, &child, "loser")),
            })
            .await,
        Err(MobStoreError::CasConflict(_))
    ));
    assert_eq!(
        handle
            .fork_job_terminal(&job.job_id, &child)
            .await
            .expect("the stream stays valid"),
        Some(winner)
    );
}

/// The durable store's atomic record: an exact replay appends nothing, a
/// different outcome for the same job and child is refused, another child's
/// job is its own, and a reset starts an epoch whose job is recorded anew.
#[tokio::test]
async fn the_sqlite_store_records_a_fork_job_terminal_once_per_epoch() {
    let dir = tempfile::tempdir().expect("tempdir");
    let storage = MobStorage::persistent(dir.path().join("mob.db")).expect("open the store");
    let events = Arc::clone(&storage.events);
    let mob_id = MobId::from("terminal-mob");
    let child = AgentIdentity::from("child-x");
    let job = fork_job("job-1");
    let record = |kind: MobEventKind| {
        let (events, mob_id) = (Arc::clone(&events), mob_id.clone());
        async move {
            events
                .append_fork_job_terminal_if_absent(NewMobEvent {
                    mob_id,
                    timestamp: None,
                    kind,
                })
                .await
        }
    };

    let first = terminal(&job, &child, "done");
    assert!(
        record(MobEventKind::ForkJobTerminal(first.clone()))
            .await
            .expect("record")
            .is_some()
    );
    assert!(
        record(MobEventKind::ForkJobTerminal(first.clone()))
            .await
            .expect("an exact replay")
            .is_none()
    );
    assert!(matches!(
        record(MobEventKind::ForkJobTerminal(terminal(
            &job, &child, "other"
        )))
        .await,
        Err(MobStoreError::CasConflict(_))
    ));
    assert!(
        record(MobEventKind::ForkJobTerminal(terminal(
            &job,
            &AgentIdentity::from("child-y"),
            "theirs"
        )))
        .await
        .expect("another child's job")
        .is_some()
    );
    assert!(matches!(
        record(MobEventKind::MobReset).await,
        Err(MobStoreError::Internal(_))
    ));
    events
        .append(NewMobEvent {
            mob_id: mob_id.clone(),
            timestamp: None,
            kind: MobEventKind::MobReset,
        })
        .await
        .expect("reset the mob");
    assert!(
        record(MobEventKind::ForkJobTerminal(terminal(
            &job,
            &child,
            "next epoch"
        )))
        .await
        .expect("record in the new epoch")
        .is_some()
    );
    let terminals = events
        .replay_all()
        .await
        .expect("replay")
        .into_iter()
        .filter(|event| matches!(event.kind, MobEventKind::ForkJobTerminal(_)))
        .count();
    assert_eq!(terminals, 3);
}

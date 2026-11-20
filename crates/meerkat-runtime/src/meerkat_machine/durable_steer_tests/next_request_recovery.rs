//! Steers that join the next request: what a recall, a store failure and the
//! end of the run do around the join.
//!
//! - A recall that comes after the boundary joined several steers answers,
//!   for each, that a run holds it, and changes nothing: all of them stay in
//!   the request.
//! - A join that the store refuses is not published: the request carries
//!   only the steers joined before it, the store keeps the refused one and
//!   every steer behind it `Queued`, and a runtime that reloads the session
//!   from the store delivers each steer exactly once.
//! - On the persistent driver, steers still waiting when the run ends take
//!   one follow-up turn in arrival order.
use super::*;
use crate::queued_input_recall::QueuedInputRecall;

fn memory_store() -> Arc<dyn crate::store::RuntimeStore> {
    Arc::new(crate::store::InMemoryRuntimeStore::new())
}

/// The inputs the most recent boundary take applied, in order.
fn last_take_ids(rig: &DurableSteerRig) -> Vec<InputId> {
    rig.script
        .boundary_takes()
        .last()
        .expect("a boundary was taken")
        .iter()
        .map(|(input_id, _)| input_id.clone())
        .collect()
}

async fn recall_now(rig: &DurableSteerRig, input_id: &InputId) -> Option<QueuedInputRecall> {
    tokio::time::timeout(
        Duration::from_secs(5),
        rig.adapter.recall_queued_input(&rig.session_id, input_id),
    )
    .await
    .expect("a recall answers without waiting for a boundary")
    .expect("recall")
}

/// Three steers join one boundary. A recall of each, newest first, after
/// that answers that the run holds it; none is withdrawn, and all three are
/// consumed with the run.
async fn recall_after_the_boundary_joined_three_steers(rig: DurableSteerRig) {
    let batch = rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    let ids = admit_in_order(
        &rig,
        vec![
            peer_steer("joined payload one"),
            peer_steer("joined payload two"),
            peer_steer("joined payload three"),
        ],
    )
    .await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    assert_eq!(
        last_take_ids(&rig),
        ids,
        "the boundary joined all three before any recall"
    );

    for input_id in ids.iter().rev() {
        let answer = recall_now(&rig, input_id).await;
        assert!(
            matches!(
                &answer,
                Some(QueuedInputRecall::NotQueued {
                    phase: InputLifecycleState::Staged,
                    run_id: Some(_),
                    ..
                })
            ),
            "a steer the boundary joined is no longer queued: {answer:?}"
        );
        assert_eq!(
            rig.phase(input_id).await,
            Some(InputLifecycleState::Staged),
            "the recall changed nothing"
        );
    }

    rig.script.step(RunnerStep::Finish);
    rig.wait_for_phase(&batch, InputLifecycleState::Consumed)
        .await;
    for input_id in &ids {
        rig.wait_for_phase(input_id, InputLifecycleState::Consumed)
            .await;
    }
    assert_eq!(
        rig.script.applied_durable(),
        ids,
        "each stays in the request, applied exactly once"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        rig.script.apply_calls.load(Ordering::SeqCst),
        1,
        "no follow-up turn ran"
    );
}

#[tokio::test]
async fn recall_after_the_boundary_joined_three_steers_answers_not_queued_for_each() {
    recall_after_the_boundary_joined_three_steers(DurableSteerRig::ephemeral().await).await;
}

#[tokio::test]
async fn persistent_recall_after_the_boundary_joined_three_steers_answers_not_queued_for_each() {
    recall_after_the_boundary_joined_three_steers(
        DurableSteerRig::persistent(memory_store()).await,
    )
    .await;
}

/// On the persistent driver, three steers admitted while the model streams
/// are still waiting when the run ends: exactly one follow-up turn starts
/// with all three, in arrival order, and no boundary applied any of them.
#[tokio::test]
async fn persistent_steers_still_waiting_when_the_run_ends_take_one_follow_up_in_arrival_order() {
    let store = memory_store();
    let rig = DurableSteerRig::persistent(Arc::clone(&store)).await;
    let runtime_id = MeerkatMachine::logical_runtime_id(&rig.session_id);
    let batch = rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenStream)
        .await;
    let ids = admit_in_order(
        &rig,
        vec![
            peer_steer("persistent run end payload one"),
            peer_steer("persistent run end payload two"),
            peer_steer("persistent run end payload three"),
        ],
    )
    .await;

    // The model answers: the run ends with no further boundary.
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_apply_calls(2).await;
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_phase(&batch, InputLifecycleState::Consumed)
        .await;
    for input_id in &ids {
        rig.wait_for_phase(input_id, InputLifecycleState::Consumed)
            .await;
    }
    assert!(
        rig.script.applied_durable().is_empty(),
        "no boundary applied a steer"
    );
    assert_eq!(
        rig.script.primitives()[1],
        ids,
        "one follow-up turn starts with all three, in arrival order"
    );
    for input_id in &ids {
        let row = store
            .load_input_state(&runtime_id, input_id)
            .await
            .expect("load consumed row")
            .expect("consumed row persisted");
        assert_eq!(row.seed.phase, InputLifecycleState::Consumed);
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        rig.script.apply_calls.load(Ordering::SeqCst),
        2,
        "exactly one follow-up turn ran"
    );
}

#[cfg(feature = "sqlite-store")]
mod refused_join {
    use super::*;
    use crate::store::RuntimeStore;

    struct Scene {
        rig: DurableSteerRig,
        store: Arc<crate::store::SqliteRuntimeStore>,
        runtime_id: crate::identifiers::LogicalRuntimeId,
        ids: Vec<InputId>,
        _directory: tempfile::TempDir,
    }

    /// A busy run on a SQLite store whose tools are running, with three peer
    /// steers admitted in order and waiting for the open boundary.
    async fn three_steers_waiting_on_sqlite() -> Scene {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(
            crate::store::SqliteRuntimeStore::new(directory.path().join("runtime.sqlite")).unwrap(),
        );
        let rig = DurableSteerRig::persistent(store.clone()).await;
        let runtime_id = MeerkatMachine::logical_runtime_id(&rig.session_id);
        rig.start_busy_turn().await;
        rig.script
            .step_and_wait(RunnerStep::BoundaryThenToolCalls)
            .await;
        let ids = admit_in_order(
            &rig,
            vec![
                peer_steer("refused join payload one"),
                peer_steer("refused join payload two"),
                peer_steer("refused join payload three"),
            ],
        )
        .await;
        Scene {
            rig,
            store,
            runtime_id,
            ids,
            _directory: directory,
        }
    }

    async fn stored_phase(scene: &Scene, input_id: &InputId) -> InputLifecycleState {
        scene
            .store
            .load_input_state(&scene.runtime_id, input_id)
            .await
            .expect("load input row")
            .expect("input row persisted")
            .seed
            .phase
    }

    /// Without a store failure the boundary joins all three: the reading the
    /// refused join is set against.
    #[tokio::test]
    async fn sqlite_three_steers_join_one_boundary_when_every_join_is_persisted() {
        let scene = three_steers_waiting_on_sqlite().await;
        scene
            .rig
            .script
            .step_and_wait(RunnerStep::BoundaryThenToolCalls)
            .await;
        assert_eq!(last_take_ids(&scene.rig), scene.ids);
        for input_id in &scene.ids {
            assert_eq!(
                stored_phase(&scene, input_id).await,
                InputLifecycleState::Staged
            );
        }
    }

    /// The store refuses the write that binds the SECOND of three steers to
    /// the run.
    ///
    /// The request carries the first steer alone: a join that is not durable
    /// is never published, and no steer behind it overtakes it. The store
    /// holds exactly that: the first bound to the run, the second and the
    /// third still `Queued`. After the store takes writes again and a fresh
    /// runtime reloads the session from it, each of the three is delivered
    /// exactly once: none is lost and none is doubled.
    ///
    /// The row first reads the same scene without the refusal (all three
    /// join), so the refusal is the only difference.
    #[tokio::test]
    async fn sqlite_join_the_store_refuses_is_not_published_and_no_steer_is_lost_or_doubled() {
        let unrefused = three_steers_waiting_on_sqlite().await;
        unrefused
            .rig
            .script
            .step_and_wait(RunnerStep::BoundaryThenToolCalls)
            .await;
        assert_eq!(
            last_take_ids(&unrefused.rig),
            unrefused.ids,
            "with every join persisted the boundary joins all three"
        );

        let scene = three_steers_waiting_on_sqlite().await;
        let [first, second, third] = scene.ids.as_slice() else {
            unreachable!("three steers")
        };
        let connection = rusqlite::Connection::open(scene.store.path()).unwrap();
        connection
            .execute_batch(&format!(
                "CREATE TRIGGER refuse_second_join BEFORE UPDATE ON runtime_input_states
             WHEN NEW.runtime_id = '{}' AND NEW.input_id = '{second}'
             BEGIN SELECT RAISE(ABORT, 'second join persist failure'); END;",
                scene.runtime_id
            ))
            .unwrap();

        scene
            .rig
            .script
            .step_and_wait(RunnerStep::BoundaryThenToolCalls)
            .await;
        assert_eq!(
            last_take_ids(&scene.rig),
            vec![first.clone()],
            "the request carries the steer joined before the refused write, and no other"
        );
        let first_row = scene
            .store
            .load_input_state(&scene.runtime_id, first)
            .await
            .expect("load the first steer's row")
            .expect("the first steer's row is stored");
        assert_eq!(first_row.seed.phase, InputLifecycleState::Staged);
        let interrupted_run = first_row
            .seed
            .last_run_id
            .expect("the published steer is bound to its run");
        for input_id in [second, third] {
            assert_eq!(
                stored_phase(&scene, input_id).await,
                InputLifecycleState::Queued,
                "{input_id} keeps its queued row in the store"
            );
        }

        // The store takes writes again, and a fresh runtime reloads the
        // session from it. The first runtime is never driven again.
        connection
            .execute_batch("DROP TRIGGER refuse_second_join;")
            .unwrap();
        let Scene {
            rig,
            store,
            runtime_id,
            ids,
            _directory,
        } = scene;
        let session_id = rig.session_id.clone();
        std::mem::forget(rig);
        let reloaded = DurableSteerRig::with_adapter_for_session(
            Arc::new(
                MeerkatMachine::persistent_without_blobs(store.clone())
                    .expect("persistent machine"),
            ),
            session_id,
        )
        .await;
        let mut finished = 0;
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let mut consumed = true;
                for input_id in &ids {
                    consumed &=
                        reloaded.phase(input_id).await == Some(InputLifecycleState::Consumed);
                }
                if consumed {
                    return;
                }
                if *reloaded.script.applies_observable.borrow() > finished {
                    reloaded.script.step(RunnerStep::Finish);
                    finished += 1;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the reloaded runtime consumes every steer");

        let primitives = reloaded.script.primitives();
        let applied = reloaded.script.applied_durable();
        for input_id in &ids {
            let at_turn_start = contributions(&primitives, input_id);
            let at_a_boundary = applied
                .iter()
                .filter(|applied_id| *applied_id == input_id)
                .count();
            assert_eq!(
                at_turn_start + at_a_boundary,
                1,
                "{input_id} is delivered exactly once after the reload: {at_turn_start} turn start(s), {at_a_boundary} boundary apply(ies)"
            );
            let row = store
                .load_input_state(&runtime_id, input_id)
                .await
                .expect("load consumed row")
                .expect("consumed row persisted");
            assert_eq!(row.seed.phase, InputLifecycleState::Consumed);
            assert_ne!(
                row.seed.last_run_id.as_ref(),
                Some(&interrupted_run),
                "a run of the reloaded runtime consumed {input_id}"
            );
        }
        let order = primitives
            .iter()
            .flatten()
            .filter(|input_id| ids.contains(input_id))
            .cloned()
            .collect::<Vec<_>>();
        if order.len() == ids.len() {
            assert_eq!(order, ids, "turn starts deliver them in arrival order");
        }
    }
}

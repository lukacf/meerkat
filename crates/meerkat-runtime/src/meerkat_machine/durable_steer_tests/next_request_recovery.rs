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

    /// ONE steer whose join write the store refuses: the rule every refused
    /// join write follows, whatever the number of steers at the boundary.
    ///
    /// The boundary publishes nothing and the store keeps the steer
    /// `Queued`. The running runtime is then fenced: it refuses a new
    /// admission, naming the join write as the reason it needs a reload from
    /// the store. A fresh runtime that reloads the session from the store
    /// delivers the steer exactly once.
    #[tokio::test]
    async fn sqlite_one_steer_whose_join_the_store_refuses_fences_the_runtime_until_a_reload() {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(
            crate::store::SqliteRuntimeStore::new(directory.path().join("runtime.sqlite")).unwrap(),
        );
        let rig = DurableSteerRig::persistent(store.clone()).await;
        let runtime_id = MeerkatMachine::logical_runtime_id(&rig.session_id);
        let session_id = rig.session_id.clone();
        let batch = rig.start_busy_turn().await;
        rig.script
            .step_and_wait(RunnerStep::BoundaryThenToolCalls)
            .await;
        let steer = peer_steer("fenced join payload");
        let steer_id = steer.id().clone();
        rig.admit(steer).await;
        rig.wait_for_waiting_delivery().await;
        let connection = rusqlite::Connection::open(store.path()).unwrap();
        connection
            .execute_batch(&format!(
                "CREATE TRIGGER refuse_join BEFORE UPDATE ON runtime_input_states
             WHEN NEW.runtime_id = '{runtime_id}' AND NEW.input_id = '{steer_id}'
             BEGIN SELECT RAISE(ABORT, 'join persist failure'); END;"
            ))
            .unwrap();

        rig.script
            .step_and_wait(RunnerStep::BoundaryThenToolCalls)
            .await;
        assert!(
            last_take_ids(&rig).is_empty(),
            "a join that is not durable is not published"
        );
        let row = store
            .load_input_state(&runtime_id, &steer_id)
            .await
            .expect("load the steer's row")
            .expect("the steer's row is stored");
        assert_eq!(
            row.seed.phase,
            InputLifecycleState::Queued,
            "the store keeps the steer queued"
        );

        // The running runtime is fenced, also once the store takes writes
        // again: a new admission is refused, and the refusal names the join
        // write.
        connection
            .execute_batch("DROP TRIGGER refuse_join;")
            .unwrap();
        let refused = tokio::time::timeout(
            Duration::from_secs(5),
            rig.adapter
                .accept_input_with_completion(&rig.session_id, peer_steer("after the refusal")),
        )
        .await
        .expect("the fenced runtime answers an admission at once");
        match refused {
            Err(RuntimeDriverError::RecoveryRepairBlocked { reason, .. }) => assert!(
                reason.contains("live_boundary_join_commit"),
                "the refusal names the join write: {reason}"
            ),
            Err(other) => panic!("the fenced runtime refused with another error: {other:?}"),
            Ok(_) => panic!("the fenced runtime admitted a new input"),
        }

        // A fresh runtime reloads the session from the store. The first
        // runtime is never driven again.
        std::mem::forget(rig);
        let reloaded = DurableSteerRig::with_adapter_for_session(
            Arc::new(
                MeerkatMachine::persistent_without_blobs(store.clone())
                    .expect("persistent machine"),
            ),
            session_id,
        )
        .await;
        let everything = [batch, steer_id.clone()];
        let mut finished = 0;
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let mut consumed = true;
                for input_id in &everything {
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
        .expect("the reloaded runtime consumes the batch and the steer");
        let at_turn_start = contributions(&reloaded.script.primitives(), &steer_id);
        let at_a_boundary = reloaded
            .script
            .applied_durable()
            .iter()
            .filter(|applied_id| **applied_id == steer_id)
            .count();
        assert_eq!(
            at_turn_start + at_a_boundary,
            1,
            "the steer is delivered exactly once after the reload"
        );
    }
}

/// OBSERVATION rows. Each asserts what happens today, so that a change of it
/// is seen; none states a requirement. The open choice is named in each
/// row's comment.
pub(super) mod observed_today {
    use super::*;

    /// Wait until the boundary's refusal has taken the registered delivery
    /// away.
    async fn wait_for_no_waiting_delivery(rig: &DurableSteerRig) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while rig.state.has_waiting_delivery_for_test() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the boundary refuses the durable delivery");
    }

    /// ORDINARY ACCEPTING REQUEST AFTER A REFUSAL, as the agent loop can
    /// produce it.
    ///
    /// An extraction boundary refuses steer E, and steer F is admitted after
    /// that. In the agent loop a run that has entered extraction makes only
    /// extraction requests until it ends (`extraction_active` is set once and
    /// cleared at the run's terminal), so no ordinary accepting boundary
    /// follows in that run. OBSERVED TODAY: the run's next boundary, again an
    /// extraction boundary, takes neither; the run ends; exactly one
    /// follow-up turn starts with E and then F.
    ///
    /// Open choice: whether a steer admitted after an earlier one was
    /// refused may be delivered ahead of it. Here it has nowhere to go: the
    /// run has no accepting boundary left.
    #[tokio::test]
    async fn observed_a_steer_admitted_after_an_extraction_refusal_follows_the_refused_one_into_one_follow_up()
     {
        let rig = DurableSteerRig::ephemeral().await;
        let batch = rig.start_busy_turn().await;
        let refused = peer_steer("extraction refused payload E");
        let e_id = refused.id().clone();
        rig.admit(refused).await;
        rig.wait_for_waiting_delivery().await;
        rig.script.step(RunnerStep::ExtractionBoundary);
        wait_for_no_waiting_delivery(&rig).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(rig.script.applied_durable().is_empty());
        assert_eq!(rig.phase(&e_id).await, Some(InputLifecycleState::Queued));

        let later = peer_steer("admitted after the refusal payload F");
        let f_id = later.id().clone();
        rig.admit(later).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            rig.steer_queue().await,
            vec![e_id.clone(), f_id.clone()],
            "both wait in the steer lane, in admission order"
        );

        // The run's next request is an extraction request too.
        rig.script.step_and_wait(RunnerStep::OpenNextBoundary).await;
        rig.script
            .step_and_wait(RunnerStep::ExtractionBoundary)
            .await;
        assert!(
            rig.script.applied_durable().is_empty(),
            "no request of the run carries E or F"
        );

        rig.script.step(RunnerStep::Finish);
        rig.wait_for_apply_calls(2).await;
        rig.script.step(RunnerStep::Finish);
        for input_id in [&batch, &e_id, &f_id] {
            rig.wait_for_phase(input_id, InputLifecycleState::Consumed)
                .await;
        }
        assert_eq!(
            rig.script.primitives()[1],
            vec![e_id.clone(), f_id.clone()],
            "one follow-up turn starts with E and then F"
        );
        assert!(rig.script.applied_durable().is_empty());
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            rig.script.apply_calls.load(Ordering::SeqCst),
            2,
            "exactly one follow-up turn ran"
        );
    }

    /// The same refusal with an accepting boundary scripted after it. The
    /// agent loop cannot produce this order (see the row above); the
    /// simulated runner can, and the row reads the runtime's own rule for a
    /// steer that waits behind a refused one.
    ///
    /// OBSERVED TODAY: the accepting boundary carries neither E nor F. F
    /// takes the follow-up turn behind E; nothing overtakes E.
    ///
    /// Open choice: the same as above. If F is to be delivered at an
    /// accepting boundary while E waits for the follow-up, this row changes.
    #[tokio::test]
    async fn observed_an_accepting_boundary_scripted_after_an_extraction_refusal_carries_neither_steer()
     {
        let rig = DurableSteerRig::ephemeral().await;
        let batch = rig.start_busy_turn().await;
        let refused = peer_steer("scripted refusal payload E");
        let e_id = refused.id().clone();
        rig.admit(refused).await;
        rig.wait_for_waiting_delivery().await;
        rig.script.step(RunnerStep::ExtractionBoundary);
        wait_for_no_waiting_delivery(&rig).await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        let later = peer_steer("scripted refusal payload F");
        let f_id = later.id().clone();
        rig.admit(later).await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        rig.script.step_and_wait(RunnerStep::OpenNextBoundary).await;
        rig.script
            .step_and_wait(RunnerStep::BoundaryThenToolCalls)
            .await;
        assert_eq!(
            last_take_ids(&rig),
            Vec::<InputId>::new(),
            "the accepting boundary carries neither E nor F"
        );
        assert_eq!(
            rig.steer_queue().await,
            vec![e_id.clone(), f_id.clone()],
            "both keep their steer-lane place, in admission order"
        );

        rig.script.step(RunnerStep::Finish);
        rig.wait_for_apply_calls(2).await;
        rig.script.step(RunnerStep::Finish);
        for input_id in [&batch, &e_id, &f_id] {
            rig.wait_for_phase(input_id, InputLifecycleState::Consumed)
                .await;
        }
        assert_eq!(
            rig.script.primitives()[1],
            vec![e_id.clone(), f_id.clone()],
            "one follow-up turn starts with E and then F"
        );
        assert!(rig.script.applied_durable().is_empty());
    }

    // -----------------------------------------------------------------
    // PROVEN RESUMABILITY: a restart beside a run that RESUMES.
    // -----------------------------------------------------------------

    /// Leave in `store` what a durable run's round commit leaves from its
    /// first round on: a committed base of the session and, on it, the run's
    /// provisional tail whose run cursor binds `bound` to `run_id`. A restart
    /// that finds such a tail keeps the run open and resumes it. The scripted
    /// runner writes no session document, so the row stands the tail itself;
    /// everything else in these rows goes through the ordinary admission and
    /// the ordinary registration.
    pub(in super::super) async fn stand_continuable_run_tail(
        store: &Arc<dyn crate::store::RuntimeStore>,
        session_id: &SessionId,
        run_id: &RunId,
        bound: &[InputId],
    ) {
        let runtime_id = MeerkatMachine::logical_runtime_id(session_id);
        let mut session = meerkat_core::Session::with_id(session_id.clone());
        session.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text(
                "batch prompt keeps the runtime busy".to_string(),
            ),
        ));
        store
            .commit_session_snapshot(
                &runtime_id,
                crate::store::SerializedSessionSnapshot {
                    session_snapshot: serde_json::to_vec(&session).unwrap().into(),
                },
            )
            .await
            .expect("the session's committed base");
        let base = store
            .load_whole_blob_store_authority(&runtime_id)
            .await
            .expect("read the committed base")
            .expect("a committed base");
        session
            .set_run_cursor(&meerkat_core::SessionRunCursor::new(
                run_id.clone(),
                bound.to_vec(),
                0,
                meerkat_core::RunResumePoint::AwaitingModel,
            ))
            .expect("the run cursor is recorded");
        let candidate = crate::store::PreparedWholeBlobProvisionalTail::prepare_from_session(
            base,
            run_id.clone(),
            1,
            &session,
        )
        .expect("the round's candidate");
        store
            .write_prepared_whole_blob_provisional_tail(&runtime_id, candidate)
            .await
            .expect("the round commit's tail");
    }

    /// What three places hold of one input: the machine's delivery for it,
    /// the class the runtime's ingress keeps, and the persisted row.
    #[derive(Debug, PartialEq, Eq)]
    struct Held {
        machine_delivery: Option<String>,
        runtime_class: Option<String>,
        row_class: Option<String>,
        row_phase: InputLifecycleState,
    }

    fn held(
        machine_delivery: Option<&str>,
        runtime_class: Option<&str>,
        row_class: Option<&str>,
        row_phase: InputLifecycleState,
    ) -> Held {
        Held {
            machine_delivery: machine_delivery.map(str::to_string),
            runtime_class: runtime_class.map(str::to_string),
            row_class: row_class.map(str::to_string),
            row_phase,
        }
    }

    async fn read_held(
        rig: &DurableSteerRig,
        store: &Arc<dyn crate::store::RuntimeStore>,
        input_id: &InputId,
    ) -> Held {
        let driver = {
            let sessions = rig.adapter.sessions.read().await;
            Arc::clone(
                &sessions
                    .get(&rig.session_id)
                    .expect("registered session")
                    .driver,
            )
        };
        let (machine_delivery, runtime_class) = {
            let entry = driver.lock().await;
            let machine_delivery = entry
                .shared_dsl_authority()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .state()
                .input_live_boundary_delivery
                .get(&input_id.to_string())
                .map(|delivery| format!("{delivery:?}"));
            let runtime_class = entry
                .driver_ingress()
                .runtime_semantics(input_id)
                .and_then(|semantics| semantics.live_boundary_delivery())
                .map(|class| format!("{class:?}"));
            (machine_delivery, runtime_class)
        };
        let row = store
            .load_input_state(
                &MeerkatMachine::logical_runtime_id(&rig.session_id),
                input_id,
            )
            .await
            .expect("load the input's row")
            .expect("the input's row is stored");
        Held {
            machine_delivery,
            runtime_class,
            row_class: row
                .state
                .runtime_semantics
                .as_ref()
                .and_then(|semantics| semantics.live_boundary_delivery())
                .map(|class| format!("{class:?}")),
            row_phase: row.seed.phase,
        }
    }

    struct Resumed {
        rig: DurableSteerRig,
        store: Arc<dyn crate::store::RuntimeStore>,
        batch: InputId,
        run_id: RunId,
        /// The steer admitted before the restart, when the scene has one.
        waiting: Option<InputId>,
    }

    /// A persistent run made a round commit and its tools are running; with
    /// `with_waiting_steer`, a steer E is then admitted and waits for the
    /// run's next boundary. The process dies. A fresh runtime registers the
    /// session over the same store, and the SAME run resumes.
    async fn restart_beside_a_resumed_run(with_waiting_steer: bool) -> Resumed {
        let store: Arc<dyn crate::store::RuntimeStore> =
            Arc::new(crate::store::InMemoryRuntimeStore::new());
        let crashed = DurableSteerRig::persistent(Arc::clone(&store)).await;
        let session_id = crashed.session_id.clone();
        let batch = crashed.start_busy_turn().await;
        let run_id = crashed
            .script
            .active_run
            .lock()
            .unwrap()
            .clone()
            .expect("active run");
        stand_continuable_run_tail(&store, &session_id, &run_id, std::slice::from_ref(&batch))
            .await;
        let mut waiting = None;
        if with_waiting_steer {
            let steer = peer_steer("resumed run payload E");
            let e_id = steer.id().clone();
            crashed.admit(steer).await;
            crashed.wait_for_waiting_delivery().await;
            assert_eq!(
                read_held(&crashed, &store, &e_id).await,
                held(
                    Some("DurableAppend"),
                    Some("DurableAppend"),
                    Some("DurableAppend"),
                    InputLifecycleState::Queued
                ),
                "before the restart E is a durable in-turn delivery in all three places"
            );
            waiting = Some(e_id);
        }
        // The process dies with the run open. The first runtime is never
        // driven again.
        std::mem::forget(crashed);

        let rig = DurableSteerRig::with_adapter_for_session(
            Arc::new(
                MeerkatMachine::persistent_without_blobs(Arc::clone(&store))
                    .expect("persistent machine"),
            ),
            session_id,
        )
        .await;
        rig.wait_for_apply_calls(1).await;
        assert_eq!(
            rig.script.applied_runs.lock().unwrap().clone(),
            vec![run_id.clone()],
            "the restart resumes the SAME run, under its own id"
        );
        assert_eq!(
            rig.script.primitives(),
            vec![vec![batch.clone()]],
            "the resumed run carries its own bound input"
        );
        Resumed {
            rig,
            store,
            batch,
            run_id,
            waiting,
        }
    }

    /// The scripted runner commits no session document, so the hand-stood
    /// tail leaves the store before the run's terminal, as in the rows that
    /// stand a tail beside a running run.
    async fn discard_the_stood_tail(resumed: &Resumed) {
        let runtime_id = MeerkatMachine::logical_runtime_id(&resumed.rig.session_id);
        let tail = resumed
            .store
            .load_whole_blob_provisional_tail(&runtime_id)
            .await
            .expect("read the provisional tail")
            .expect("the run's tail stands")
            .authority()
            .clone();
        assert!(
            resumed
                .store
                .discard_whole_blob_provisional_tail(&runtime_id, &tail)
                .await
                .expect("discard the tail")
        );
    }

    /// PROVEN RESUMABILITY, the control: no steer waits at the restart.
    ///
    /// OBSERVED TODAY: a steer F admitted while the resumed run runs is a
    /// durable in-turn delivery in the machine, in the runtime and in its
    /// row; it registers for the run's next boundary and that boundary
    /// carries it. No follow-up turn runs.
    #[tokio::test]
    async fn observed_a_steer_admitted_during_a_resumed_run_joins_its_next_request() {
        let resumed = restart_beside_a_resumed_run(false).await;
        let rig = &resumed.rig;
        let steer = peer_steer("resumed run payload F");
        let f_id = steer.id().clone();
        rig.admit(steer).await;
        rig.wait_for_waiting_delivery().await;
        assert_eq!(
            read_held(rig, &resumed.store, &f_id).await,
            held(
                Some("DurableAppend"),
                Some("DurableAppend"),
                Some("DurableAppend"),
                InputLifecycleState::Queued
            )
        );

        rig.script
            .step_and_wait(RunnerStep::BoundaryThenToolCalls)
            .await;
        assert_eq!(
            last_take_ids(rig),
            vec![f_id.clone()],
            "the resumed run's next request carries F"
        );
        let row = resumed
            .store
            .load_input_state(&MeerkatMachine::logical_runtime_id(&rig.session_id), &f_id)
            .await
            .expect("load F's row")
            .expect("F's row is stored");
        assert_eq!(row.seed.phase, InputLifecycleState::Staged);
        assert_eq!(
            row.seed.last_run_id.as_ref(),
            Some(&resumed.run_id),
            "F is bound to the resumed run"
        );

        discard_the_stood_tail(&resumed).await;
        rig.script.step(RunnerStep::Finish);
        for input_id in [&resumed.batch, &f_id] {
            rig.wait_for_phase(input_id, InputLifecycleState::Consumed)
                .await;
        }
        assert_eq!(rig.script.applied_durable(), vec![f_id]);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            rig.script.apply_calls.load(Ordering::SeqCst),
            1,
            "no follow-up turn ran"
        );
    }

    /// PROVEN RESUMABILITY, the witness: steer E waits for a boundary when
    /// the process dies, the same run resumes, and steer F is admitted while
    /// it runs.
    ///
    /// OBSERVED TODAY, at the cutoff of the resumed run's next request: E is
    /// back in the steer lane with its `Queued` row; the runtime and the row
    /// still class it a durable in-turn delivery, but the new machine holds
    /// NO delivery for it (only an admission writes one). F holds its
    /// delivery in all three places. No delivery registers, the request
    /// carries NEITHER E nor F, and after the run ends exactly one follow-up
    /// turn starts with E and then F. Each is delivered exactly once, and E's
    /// payload is the one it was admitted with.
    ///
    /// Open choice, not asserted here. If E is still eligible in a run that
    /// resumed, both belong in that request, E before F. If a restart ends
    /// E's eligibility for it, F belongs in that request and E in the
    /// follow-up. A stale class or the machine's missing delivery alone does
    /// not decide which.
    #[tokio::test]
    async fn observed_a_steer_waiting_at_a_restart_keeps_itself_and_a_later_one_out_of_the_resumed_run()
     {
        let resumed = restart_beside_a_resumed_run(true).await;
        let rig = &resumed.rig;
        let e_id = resumed
            .waiting
            .clone()
            .expect("the scene has a waiting steer");
        assert_eq!(rig.steer_queue().await, vec![e_id.clone()]);

        let steer = peer_steer("resumed run payload F");
        let f_id = steer.id().clone();
        rig.admit(steer).await;
        tokio::time::sleep(Duration::from_millis(150)).await;

        // The cutoff: what the three places hold of E and of F.
        assert_eq!(
            read_held(rig, &resumed.store, &e_id).await,
            held(
                None,
                Some("DurableAppend"),
                Some("DurableAppend"),
                InputLifecycleState::Queued
            ),
            "E: no machine delivery, the class kept by the runtime and by the row"
        );
        assert_eq!(
            read_held(rig, &resumed.store, &f_id).await,
            held(
                Some("DurableAppend"),
                Some("DurableAppend"),
                Some("DurableAppend"),
                InputLifecycleState::Queued
            ),
            "F: a durable in-turn delivery in all three places"
        );
        let runtime_id = MeerkatMachine::logical_runtime_id(&rig.session_id);
        let e_row = resumed
            .store
            .load_input_state(&runtime_id, &e_id)
            .await
            .expect("load E's row")
            .expect("E's row is stored");
        assert!(
            format!("{:?}", e_row.state.persisted_input).contains("resumed run payload E"),
            "E's row keeps the payload it was admitted with"
        );
        assert!(
            !rig.state.has_waiting_delivery_for_test(),
            "no delivery registers for the resumed run's next boundary"
        );
        assert_eq!(
            rig.steer_queue().await,
            vec![e_id.clone(), f_id.clone()],
            "both wait in the steer lane, in admission order"
        );

        // The resumed run's next request.
        rig.script
            .step_and_wait(RunnerStep::BoundaryThenToolCalls)
            .await;
        assert_eq!(
            last_take_ids(rig),
            Vec::<InputId>::new(),
            "the resumed run's next request carries neither E nor F"
        );

        // The run ends; the follow-up turn.
        discard_the_stood_tail(&resumed).await;
        rig.script.step(RunnerStep::Finish);
        rig.wait_for_apply_calls(2).await;
        rig.script.step(RunnerStep::Finish);
        for input_id in [&resumed.batch, &e_id, &f_id] {
            rig.wait_for_phase(input_id, InputLifecycleState::Consumed)
                .await;
        }
        let primitives = rig.script.primitives();
        assert_eq!(
            primitives[1],
            vec![e_id.clone(), f_id.clone()],
            "one follow-up turn starts with E and then F"
        );
        for input_id in [&e_id, &f_id] {
            assert_eq!(
                contributions(&primitives, input_id),
                1,
                "{input_id} is delivered exactly once"
            );
        }
        assert!(
            rig.script.applied_durable().is_empty(),
            "no boundary applied E or F"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            rig.script.apply_calls.load(Ordering::SeqCst),
            2,
            "exactly one follow-up turn ran"
        );
    }
}

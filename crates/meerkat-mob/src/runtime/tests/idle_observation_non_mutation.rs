//! The pure idle observation changes nothing it reads.
//!
//! These rows reuse the scene of the whole-idle consumer rows
//! (`whole_idle_consumer.rs`, `individual_completion_does_not_make_queued_work_idle`):
//! the rig and the held block in which member A has completed and member B
//! stays queued at attempt zero behind the runtime's queue-authority hook.
//! Where that scene asks the MUTATING `close_if_idle`, these rows ask
//! `MobHandle::observe_idle`, and read the same facts before and after it:
//! the mob's admission-close state, its phase as the scope gate answers it,
//! the roster, each member's status, the stored input rows, the current
//! runs, the runtime phases, the native active-input reads and the turns
//! that reached the session service.
//!
//! The control row puts `close_if_idle` back in the observation's place on
//! the same scene and shows that the same reads then differ, so the
//! non-mutation assertions are able to fail.
//!
//! Outside these rows: a hosted mob that is not loaded (the mob set's row),
//! schedules, remote members and the mobs a member created.
use super::*;
use crate::runtime::admission_close::{CloseIfIdleOutcome, MobAdmissionCloseReason};
use meerkat_runtime::{InputLifecycleState, InputTerminalOutcome, SessionServiceRuntimeExt as _};

const STEP: Duration = Duration::from_secs(10);

fn turn_driven_worker(name: &str) -> SpawnMemberSpec {
    let mut spec = SpawnMemberSpec::new(ProfileName::from("worker"), AgentIdentity::from(name));
    spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    spec
}

const FINISHED_PROMPT: &str = "A completed while B remained queued";
const QUEUED_PROMPT: &str = "B was accepted before A completed";

/// The reused scene, held: member A's input is consumed, member B's input
/// is queued at attempt zero behind the queue-authority hook, which stays
/// held until [`Scene::release_queued_member`].
struct Scene {
    handle: MobHandle,
    service: Arc<MockSessionService>,
    adapter: Arc<meerkat_runtime::MeerkatMachine>,
    finished_member: AgentIdentity,
    queued_member: AgentIdentity,
    finished_session: SessionId,
    queued_session: SessionId,
    finished_id: meerkat_core::lifecycle::InputId,
    queued_id: meerkat_core::lifecycle::InputId,
    queue_release: Option<tokio::sync::oneshot::Sender<()>>,
    queued_completion: Option<meerkat_runtime::CompletionHandle>,
}

/// Everything the rows read before and after the observation. Two reads
/// are equal exactly when nothing the rows can see has changed.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reads {
    /// The mob's admission-close state: `None` while admission is open.
    admission_closed: Option<MobAdmissionCloseReason>,
    /// The mob's phase, asked through the actor's scope gate.
    phase: String,
    /// The roster: every member with its lifecycle status.
    members: String,
    /// Each member's run state, work in flight and health, A then B.
    member_progress: [String; 2],
    /// The stored input rows, A then B.
    inputs: [String; 2],
    /// The current run of each member's session, A then B.
    current_runs: [String; 2],
    /// The runtime phase of each member's session, A then B.
    runtime_phases: [String; 2],
    /// Whether each member's session holds native active input, A then B.
    active_inputs: [bool; 2],
    /// Every turn that reached the session service: a run that was started
    /// is one more entry here.
    started_turns: Vec<(SessionId, String)>,
}

impl Scene {
    async fn held(label: &str) -> Self {
        let (handle, service) =
            create_test_mob(with_unique_mob_id(sample_definition(), label)).await;
        service.set_return_exact_run_result(true);
        let finished_member = AgentIdentity::from(format!("{label}-finished-member").as_str());
        let queued_member = AgentIdentity::from(format!("{label}-queued-member").as_str());
        for member in [&finished_member, &queued_member] {
            handle
                .spawn_spec(turn_driven_worker(member.as_str()))
                .await
                .expect("seat turn-driven member without a kickoff");
        }
        let finished_session = handle
            .resolve_bridge_session_id(&finished_member)
            .await
            .expect("A has a bridge session");
        let queued_session = handle
            .resolve_bridge_session_id(&queued_member)
            .await
            .expect("B has a bridge session");
        assert_ne!(finished_session, queued_session);
        let adapter = MobSessionService::acquire_runtime_adapter(service.as_ref(), None)
            .expect("acquire existing runtime")
            .expect("runtime-backed test mob");
        // This selects the real member-status fallback: Idle with the last
        // recorded work count and Unknown health. It does not forge queue state.
        service.fail_execution_snapshots_for(&queued_session);
        let (queue_entered, queue_release) =
            adapter.arm_runtime_loop_before_queue_authority_test_hook(queued_session.clone());
        let queued_input =
            meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new(QUEUED_PROMPT, None));
        let queued_id = queued_input.id().clone();
        let (queued_admission, queued_completion) = adapter
            .accept_input_with_completion(&queued_session, queued_input)
            .await
            .expect("admit B");
        assert!(queued_admission.is_accepted());
        tokio::time::timeout(STEP, queue_entered)
            .await
            .expect("B reaches queue hook")
            .expect("queue hook signal");

        let finished_input = meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new(
            FINISHED_PROMPT,
            None,
        ));
        let finished_id = finished_input.id().clone();
        assert_ne!(finished_id, queued_id);
        // B's hook stays held throughout A's admission, exact completion and
        // settlement. One bound detects deadlocks.
        let completed = tokio::time::timeout(STEP, async {
            let (finished_admission, completion) = adapter
                .accept_input_with_completion(&finished_session, finished_input)
                .await
                .expect("admit A independently of B's held queue");
            assert!(finished_admission.is_accepted());
            let completed = completion
                .expect("A admission registers exact completion")
                .wait()
                .await;
            adapter.wait_current_run_settled(&finished_session).await;
            completed
        })
        .await
        .expect("A completes while B's queue hook remains held");
        match completed.expect("A exact completion authority answers") {
            meerkat_runtime::CompletionOutcome::Completed(result) => {
                assert_eq!(result.text, FINISHED_PROMPT);
            }
            other => panic!("A must complete successfully: {other:?}"),
        }
        Self {
            handle,
            service,
            adapter,
            finished_member,
            queued_member,
            finished_session,
            queued_session,
            finished_id,
            queued_id,
            queue_release: Some(queue_release),
            queued_completion,
        }
    }

    /// Read everything the rows compare, in one bounded step.
    async fn reads(&self) -> Reads {
        let read = tokio::time::timeout(STEP, async {
            let admission = self.handle.admission_closed();
            let admission_closed = *admission.borrow();
            let mut member_progress = [String::new(), String::new()];
            for (slot, member) in member_progress
                .iter_mut()
                .zip([&self.finished_member, &self.queued_member])
            {
                let status = self
                    .handle
                    .member_status(member)
                    .await
                    .expect("the member status answers");
                *slot = format!(
                    "session={:?} progress={:?}",
                    status.current_bridge_session_id(),
                    status.progress.as_ref().map(|progress| (
                        &progress.run_state,
                        &progress.in_flight_work,
                        &progress.health
                    ))
                );
            }
            Reads {
                admission_closed,
                phase: format!("{:?}", self.handle.status().await),
                members: format!("{:?}", self.handle.list_members().await),
                member_progress,
                inputs: [
                    format!(
                        "{:?}",
                        self.adapter
                            .input_state(&self.finished_session, &self.finished_id)
                            .await
                    ),
                    format!(
                        "{:?}",
                        self.adapter
                            .input_state(&self.queued_session, &self.queued_id)
                            .await
                    ),
                ],
                current_runs: [
                    format!(
                        "{:?}",
                        self.adapter.current_run(&self.finished_session).await
                    ),
                    format!("{:?}", self.adapter.current_run(&self.queued_session).await),
                ],
                runtime_phases: [
                    format!(
                        "{:?}",
                        self.adapter.runtime_state(&self.finished_session).await
                    ),
                    format!(
                        "{:?}",
                        self.adapter.runtime_state(&self.queued_session).await
                    ),
                ],
                active_inputs: [
                    self.adapter
                        .session_has_active_inputs(&self.finished_session)
                        .await,
                    self.adapter
                        .session_has_active_inputs(&self.queued_session)
                        .await,
                ],
                started_turns: self.service.recorded_start_turn_prompts().await,
            }
        })
        .await;
        read.expect("the reads answer within the step bound")
    }

    /// One observation, bounded.
    async fn observe(&self) -> crate::MobIdleObservation {
        tokio::time::timeout(STEP, self.handle.observe_idle())
            .await
            .expect("the observation answers within the step bound")
            .expect("the observation")
    }

    /// B's stored input row is still queued, unattempted and without a run.
    async fn assert_queued_member_was_not_woken(&self) {
        let queued = self
            .adapter
            .input_state(&self.queued_session, &self.queued_id)
            .await
            .expect("read B")
            .expect("B is retained");
        assert_eq!(queued.seed.phase, InputLifecycleState::Queued);
        assert_eq!(queued.seed.attempt_count, 0);
        assert_eq!(queued.seed.last_run_id, None);
        assert_eq!(queued.seed.terminal_outcome, None);
        assert_eq!(self.adapter.current_run(&self.queued_session).await, None);
        assert_eq!(
            self.service.recorded_start_turn_prompts().await,
            vec![(self.finished_session.clone(), FINISHED_PROMPT.to_string())],
            "only A's turn reached the session service: no run was started for B"
        );
    }

    /// A new input to member A is admitted and runs: admission is open.
    async fn assert_a_new_input_is_admitted_and_runs(&self, prompt: &str) {
        let turns_before = self.service.recorded_start_turn_prompts().await.len();
        let input = meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new(prompt, None));
        let ran = tokio::time::timeout(STEP, async {
            let (admission, completion) = self
                .adapter
                .accept_input_with_completion(&self.finished_session, input)
                .await
                .expect("admit the new input");
            assert!(admission.is_accepted());
            let completed = completion
                .expect("the new input registers exact completion")
                .wait()
                .await;
            self.adapter
                .wait_current_run_settled(&self.finished_session)
                .await;
            completed
        })
        .await
        .expect("the new input runs within the step bound");
        match ran.expect("the new input's completion authority answers") {
            meerkat_runtime::CompletionOutcome::Completed(result) => {
                assert_eq!(result.text, prompt);
            }
            other => panic!("the new input must complete successfully: {other:?}"),
        }
        let turns = self.service.recorded_start_turn_prompts().await;
        assert_eq!(turns.len(), turns_before + 1, "the new input ran one turn");
        assert_eq!(
            turns.last(),
            Some(&(self.finished_session.clone(), prompt.to_string()))
        );
    }

    /// Release B's hook and wait until both members have no active input
    /// and the mob reads idle.
    async fn release_queued_member(&mut self) {
        self.queue_release
            .take()
            .expect("the hook is released once")
            .send(())
            .expect("B hook receiver was alive at release");
        let completed = tokio::time::timeout(
            STEP,
            self.queued_completion
                .take()
                .expect("B admission registered completion")
                .wait(),
        )
        .await
        .expect("release alone resolves B completion");
        match completed.expect("B completion authority answers") {
            meerkat_runtime::CompletionOutcome::Completed(result) => {
                assert_eq!(result.text, QUEUED_PROMPT);
            }
            other => panic!("B must complete successfully after release: {other:?}"),
        }
        tokio::time::timeout(
            STEP,
            self.adapter.wait_current_run_settled(&self.queued_session),
        )
        .await
        .expect("B's exact run settles");
        for (session, id) in [
            (&self.finished_session, &self.finished_id),
            (&self.queued_session, &self.queued_id),
        ] {
            let state = self
                .adapter
                .input_state(session, id)
                .await
                .expect("read final input")
                .expect("input is retained");
            assert_eq!(state.seed.phase, InputLifecycleState::Consumed);
            assert_eq!(state.seed.attempt_count, 1);
            assert_eq!(
                state.seed.terminal_outcome,
                Some(InputTerminalOutcome::Consumed)
            );
        }
        let idle = tokio::time::timeout(STEP, async {
            loop {
                if self.observe().await.is_idle() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        assert!(
            idle.is_ok(),
            "the mob reads idle once both members' inputs are consumed: {:?}",
            self.handle.observe_idle().await
        );
    }

    async fn shutdown(self) {
        tokio::time::timeout(STEP, self.handle.shutdown())
            .await
            .expect("shutdown answers")
            .expect("shutdown succeeds");
    }
}

/// The observation answers for the mob it is asked of: with A inactive and
/// B active it answers not idle, with the one run the close would count,
/// while another mob that holds no work answers idle at the same moment;
/// with both members inactive it answers idle.
#[tokio::test]
async fn the_idle_observation_answers_not_idle_with_one_member_active_and_idle_with_none() {
    let mut scene = Scene::held("idle-observed-answer").await;
    let (other_mob, _other_service) = create_test_mob(with_unique_mob_id(
        sample_definition(),
        "idle-observed-answer-other-mob",
    ))
    .await;

    let held = scene.reads().await;
    assert_eq!(
        held.active_inputs,
        [false, true],
        "A has no native active input and B has one: {held:?}"
    );
    let roster = scene.handle.list_members().await;
    for member in [&scene.finished_member, &scene.queued_member] {
        assert!(
            roster.iter().any(|entry| &entry.agent_identity == member),
            "both members are members of the observed mob: {roster:?}"
        );
    }
    let busy = scene.observe().await;
    assert!(
        !busy.is_idle(),
        "B's queued input keeps the mob busy: {busy:?}"
    );
    assert_eq!(busy.busy.runs, 1, "only B's native queued input: {busy:?}");
    assert_eq!(busy.busy.roster_transitions, 0, "{busy:?}");
    assert_eq!(busy.busy.parked_admission, 0, "{busy:?}");
    assert_eq!(busy.busy.foreground_owners, 0, "{busy:?}");
    assert_eq!(busy.admission_closed, None, "{busy:?}");
    let other = tokio::time::timeout(STEP, other_mob.observe_idle())
        .await
        .expect("the other mob's observation answers")
        .expect("the other mob's observation");
    assert!(
        other.is_idle(),
        "a mob that holds no work reads idle while the observed mob does not: {other:?}"
    );

    scene.release_queued_member().await;
    let settled = scene.reads().await;
    assert_eq!(
        settled.active_inputs,
        [false, false],
        "neither member holds native active input: {settled:?}"
    );
    let idle = scene.observe().await;
    assert!(idle.is_idle(), "both members are inactive: {idle:?}");
    assert_eq!(idle.busy.runs, 0, "{idle:?}");

    tokio::time::timeout(STEP, other_mob.shutdown())
        .await
        .expect("the other mob's shutdown answers")
        .expect("the other mob's shutdown succeeds");
    scene.shutdown().await;
}

/// With A inactive and B queued, the observation changes nothing: the
/// admission-close state, the phase, the roster, the members' status, the
/// stored input rows, the current runs, the runtime phases, the active-input
/// reads and the turns that reached the session service read the same
/// before and after it. B was not woken: its row is still queued at attempt
/// zero with no run. Admission is open: a new input to A is admitted and
/// runs while B stays held.
#[tokio::test]
async fn the_idle_observation_changes_nothing_while_one_member_holds_queued_work() {
    let mut scene = Scene::held("idle-observed-held").await;

    let before = scene.reads().await;
    assert_eq!(before.admission_closed, None, "{before:?}");
    assert_eq!(before.active_inputs, [false, true], "{before:?}");
    scene.assert_queued_member_was_not_woken().await;

    let observed = scene.observe().await;
    assert!(!observed.is_idle(), "{observed:?}");

    let after = scene.reads().await;
    assert_eq!(
        before, after,
        "the observation of a busy mob changed something it reads"
    );
    scene.assert_queued_member_was_not_woken().await;
    let again = scene.observe().await;
    assert_eq!(
        observed, again,
        "a second observation reads what the first read"
    );

    scene
        .assert_a_new_input_is_admitted_and_runs("a new input after the observation of a busy mob")
        .await;
    let queued = scene
        .adapter
        .input_state(&scene.queued_session, &scene.queued_id)
        .await
        .expect("read B")
        .expect("B is retained");
    assert_eq!(
        queued.seed.phase,
        InputLifecycleState::Queued,
        "B is still held: A's new input ran independently"
    );

    scene.release_queued_member().await;
    scene.shutdown().await;
}

/// With both members inactive, the observation answers idle and changes
/// nothing: the same reads are equal before and after it, no run was
/// started, and admission is open: the mob seats a new member, and a new
/// input to a member is admitted and runs.
#[tokio::test]
async fn the_idle_observation_of_an_idle_mob_changes_nothing_and_admission_stays_open() {
    let mut scene = Scene::held("idle-observed-idle").await;
    scene.release_queued_member().await;

    let before = scene.reads().await;
    assert_eq!(before.admission_closed, None, "{before:?}");
    assert_eq!(before.active_inputs, [false, false], "{before:?}");

    let observed = scene.observe().await;
    assert!(observed.is_idle(), "{observed:?}");

    let after = scene.reads().await;
    assert_eq!(
        before, after,
        "the observation of an idle mob changed something it reads"
    );
    assert_eq!(
        after.started_turns,
        vec![
            (scene.finished_session.clone(), FINISHED_PROMPT.to_string()),
            (scene.queued_session.clone(), QUEUED_PROMPT.to_string()),
        ],
        "no run was started by the observation"
    );

    tokio::time::timeout(
        STEP,
        scene
            .handle
            .spawn_spec(turn_driven_worker("idle-observed-idle-new-member")),
    )
    .await
    .expect("the spawn answers")
    .expect("a mob that was observed idle still admits a new member");
    scene
        .assert_a_new_input_is_admitted_and_runs("a new input after the observation of an idle mob")
        .await;
    assert!(
        scene.handle.admission_closed().borrow().is_none(),
        "admission is still open"
    );

    scene.shutdown().await;
}

/// THE CONTROL. The same scene with `close_if_idle` in the observation's
/// place. While B is queued the close answers busy with the counts the
/// observation read, and leaves the mob as it was. With both members
/// inactive the close closes: the reads that were equal around the
/// observation now differ, the admission-close state names the idle close,
/// the mob refuses a new member, and the observation itself reports the
/// closed admission. So each non-mutation assertion of the rows above is
/// able to fail.
#[tokio::test]
async fn the_idle_close_in_the_observations_place_changes_the_reads_the_observation_leaves() {
    let mut scene = Scene::held("idle-observed-control").await;

    let observed_busy = scene.observe().await;
    let before_busy_close = scene.reads().await;
    let busy_close = tokio::time::timeout(STEP, scene.handle.close_if_idle())
        .await
        .expect("the deciding close answers")
        .expect("the deciding close");
    match busy_close {
        CloseIfIdleOutcome::Busy(found) => assert_eq!(
            found, observed_busy.busy,
            "the observation read what the close is decided on"
        ),
        other => panic!("B's accepted queue must prevent the idle close: {other:?}"),
    }
    assert_eq!(
        before_busy_close,
        scene.reads().await,
        "a close that answers busy leaves the mob untouched"
    );

    scene.release_queued_member().await;
    let before = scene.reads().await;
    assert_eq!(before.admission_closed, None, "{before:?}");
    let around_observation = {
        let observed = scene.observe().await;
        assert!(observed.is_idle(), "{observed:?}");
        scene.reads().await
    };
    assert_eq!(
        before, around_observation,
        "the observation changed nothing"
    );

    let closed = tokio::time::timeout(STEP, scene.handle.close_if_idle())
        .await
        .expect("the deciding close answers")
        .expect("the deciding close");
    assert_eq!(closed, CloseIfIdleOutcome::Closed);
    let after_close = scene.reads().await;
    assert_ne!(
        before, after_close,
        "the close changed what the observation leaves: the equality the rows assert fails here"
    );
    assert_eq!(
        after_close.admission_closed,
        Some(MobAdmissionCloseReason::IdleClose),
        "the admission-close state changed"
    );
    let refused = tokio::time::timeout(
        STEP,
        scene
            .handle
            .spawn_spec(turn_driven_worker("idle-observed-control-new-member")),
    )
    .await
    .expect("the spawn answers");
    match refused {
        Err(MobError::AdmissionClosed { reason, .. }) => {
            assert_eq!(reason, MobAdmissionCloseReason::IdleClose);
        }
        other => panic!("a closed mob refuses a new member, got {other:?}"),
    }
    let observed_closed = scene.observe().await;
    assert_eq!(
        observed_closed.admission_closed,
        Some(MobAdmissionCloseReason::IdleClose)
    );
    assert!(
        !observed_closed.is_idle(),
        "a mob whose admission is closed does not read idle: {observed_closed:?}"
    );

    scene.shutdown().await;
}

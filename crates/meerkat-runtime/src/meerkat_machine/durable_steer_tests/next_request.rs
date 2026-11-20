//! Every durable steer admitted before a model request's input is fixed joins
//! that request, in admission order.
//!
//! The simulated runner records what each boundary take wrote for the model
//! request that follows it (`RunnerScript::boundary_takes`). These rows pin
//! that one boundary carries every eligible steer admitted up to the moment
//! its input is fixed, each exactly once, and that a steer admitted after
//! that moment joins the following boundary of the same run.
use super::*;
use crate::queued_input_recall::QueuedInputRecall;

fn take_ids(take: &[(InputId, String)]) -> Vec<InputId> {
    take.iter().map(|(input_id, _)| input_id.clone()).collect()
}

/// The durable appends the most recent boundary take applied.
fn last_take(rig: &DurableSteerRig) -> Vec<(InputId, String)> {
    rig.script
        .boundary_takes()
        .last()
        .cloned()
        .expect("a boundary was taken")
}

async fn finish_without_a_follow_up(rig: &DurableSteerRig, delivered: &[InputId]) {
    rig.script.step(RunnerStep::Finish);
    for input_id in delivered {
        rig.wait_for_phase(input_id, InputLifecycleState::Consumed)
            .await;
        assert_eq!(
            contributions(&rig.script.primitives(), input_id),
            0,
            "{input_id} joined the running run; no turn started with it"
        );
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        rig.script.apply_calls.load(Ordering::SeqCst),
        1,
        "every steer joined the running run; no follow-up turn ran"
    );
    let applied = rig.script.applied_durable();
    for input_id in delivered {
        assert_eq!(
            applied
                .iter()
                .filter(|applied_id| *applied_id == input_id)
                .count(),
            1,
            "{input_id} was applied exactly once: {applied:?}"
        );
    }
}

/// Request N returned tool calls and its tools run (the next boundary is
/// open). Steers A and B are admitted in that order. The next model request
/// carries both, A before B.
async fn two_steers_admitted_while_tools_run_both_join_the_next_request(
    rig: DurableSteerRig,
    steer_a: Input,
    steer_b: Input,
    payload_a: &str,
    payload_b: &str,
) {
    rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    let ids = admit_in_order(&rig, vec![steer_a, steer_b]).await;

    // The tools finish: the runner fixes the next request's input.
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    let next_request = last_take(&rig);
    assert_eq!(
        take_ids(&next_request),
        ids,
        "the next model request carries both steers by id, A before B"
    );
    assert!(
        next_request[0].1.contains(payload_a),
        "A's payload: {next_request:?}"
    );
    assert!(
        next_request[1].1.contains(payload_b),
        "B's payload: {next_request:?}"
    );
    assert!(!next_request[0].1.contains(payload_b));
    assert!(!next_request[1].1.contains(payload_a));
    for input_id in &ids {
        assert_eq!(rig.phase(input_id).await, Some(InputLifecycleState::Staged));
    }
    finish_without_a_follow_up(&rig, &ids).await;
    assert_eq!(rig.script.applied_durable(), ids);
}

#[tokio::test]
async fn two_peer_steers_admitted_while_tools_run_both_join_the_next_request() {
    two_steers_admitted_while_tools_run_both_join_the_next_request(
        DurableSteerRig::ephemeral().await,
        peer_steer("steer payload A"),
        peer_steer("steer payload B"),
        "steer payload A",
        "steer payload B",
    )
    .await;
}

#[tokio::test]
async fn persistent_two_peer_steers_admitted_while_tools_run_both_join_the_next_request() {
    let store: Arc<dyn crate::store::RuntimeStore> =
        Arc::new(crate::store::InMemoryRuntimeStore::new());
    two_steers_admitted_while_tools_run_both_join_the_next_request(
        DurableSteerRig::persistent(store).await,
        peer_steer("steer payload A"),
        peer_steer("steer payload B"),
        "steer payload A",
        "steer payload B",
    )
    .await;
}

#[tokio::test]
async fn two_text_prompt_steers_admitted_while_tools_run_both_join_the_next_request() {
    two_steers_admitted_while_tools_run_both_join_the_next_request(
        DurableSteerRig::ephemeral().await,
        text_prompt_steer("text steer payload A"),
        text_prompt_steer("text steer payload B"),
        "text steer payload A",
        "text steer payload B",
    )
    .await;
}

/// The content steer kinds mix in one boundary: a peer message, a text
/// prompt, a detached job's notice, an external event and a local flow step,
/// admitted in that order, all join the next request in that order.
#[tokio::test]
async fn mixed_content_steers_admitted_while_tools_run_all_join_the_next_request_in_order() {
    let rig = DurableSteerRig::ephemeral().await;
    rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    let ids = admit_in_order(
        &rig,
        vec![
            peer_steer("mixed peer payload"),
            text_prompt_steer("mixed text payload"),
            typed_steer("mixed notice payload", ConversationAppendRole::SystemNotice),
            external_event_steer("mixed event payload"),
            flow_step_steer("mixed flow payload"),
        ],
    )
    .await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    let next_request = last_take(&rig);
    assert_eq!(take_ids(&next_request), ids);
    for (index, payload) in [
        "mixed peer payload",
        "mixed text payload",
        "mixed notice payload",
        "mixed event payload",
        "mixed flow payload",
    ]
    .into_iter()
    .enumerate()
    {
        assert!(
            next_request[index].1.contains(payload),
            "{payload} at {index}: {next_request:?}"
        );
    }
    finish_without_a_follow_up(&rig, &ids).await;
}

/// Request N is in flight (the model streams, no boundary is open) while A
/// and B are admitted. The in-flight request is not cancelled: it returns
/// tool calls, and the next request carries both steers.
async fn two_peer_steers_admitted_while_the_model_streams_both_join_the_next_request(
    rig: DurableSteerRig,
) {
    rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenStream)
        .await;
    let ids = admit_in_order(
        &rig,
        vec![
            peer_steer("stream payload A"),
            peer_steer("stream payload B"),
        ],
    )
    .await;
    assert!(
        rig.script.applied_durable().is_empty(),
        "nothing joins the request that is in flight"
    );
    assert_eq!(
        rig.script.interrupts.load(Ordering::SeqCst),
        0,
        "the in-flight request is never cancelled"
    );

    // The in-flight request returns tool calls; the next boundary opens and
    // the runner reaches it.
    rig.script.step_and_wait(RunnerStep::OpenNextBoundary).await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    let next_request = last_take(&rig);
    assert_eq!(take_ids(&next_request), ids);
    assert!(next_request[0].1.contains("stream payload A"));
    assert!(next_request[1].1.contains("stream payload B"));
    assert_eq!(rig.script.interrupts.load(Ordering::SeqCst), 0);
    finish_without_a_follow_up(&rig, &ids).await;
    assert_eq!(rig.script.applied_durable(), ids);
}

#[tokio::test]
async fn two_peer_steers_admitted_while_the_model_streams_both_join_the_next_request_ephemeral() {
    two_peer_steers_admitted_while_the_model_streams_both_join_the_next_request(
        DurableSteerRig::ephemeral().await,
    )
    .await;
}

#[tokio::test]
async fn two_peer_steers_admitted_while_the_model_streams_both_join_the_next_request_persistent() {
    let store: Arc<dyn crate::store::RuntimeStore> =
        Arc::new(crate::store::InMemoryRuntimeStore::new());
    two_peer_steers_admitted_while_the_model_streams_both_join_the_next_request(
        DurableSteerRig::persistent(store).await,
    )
    .await;
}

async fn wait_for_prepare_hold(rig: &DurableSteerRig) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !rig.script.prepare_hold_reached.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("the oldest steer's preparation parks the runner");
}

async fn wait_for_steps_done(rig: &DurableSteerRig, expected: usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while rig.script.steps_done.load(Ordering::SeqCst) < expected {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("the runner step completes");
}

/// The cutoff is the moment the next request's input is fixed, not the
/// earlier moment the runner parked at the boundary. A is admitted and the
/// runner parks with it; B is admitted while the runner is parked and before
/// A's delivery is published. The request the runner then makes carries both.
async fn steer_admitted_after_the_runner_parked_joins_the_same_request(rig: DurableSteerRig) {
    rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    rig.script.prepare_hold_armed.store(true, Ordering::SeqCst);
    let steer_a = peer_steer("parked payload A");
    let a_id = steer_a.id().clone();
    rig.admit(steer_a).await;
    rig.wait_for_waiting_delivery().await;

    // The runner parks at the boundary with A; A's ingress is held before it
    // publishes anything.
    let steps_before = rig.script.steps_done.load(Ordering::SeqCst);
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    wait_for_prepare_hold(&rig).await;
    let steer_b = peer_steer("parked payload B");
    let b_id = steer_b.id().clone();
    rig.admit(steer_b).await;
    assert_eq!(
        rig.script.steps_done.load(Ordering::SeqCst),
        steps_before,
        "the runner is still parked: the next request's input is not fixed yet"
    );
    rig.script
        .prepare_hold_released
        .store(true, Ordering::SeqCst);
    wait_for_steps_done(&rig, steps_before + 1).await;

    let next_request = last_take(&rig);
    assert_eq!(
        take_ids(&next_request),
        vec![a_id.clone(), b_id.clone()],
        "B was admitted before the request's input was fixed: it joins that request"
    );
    assert!(next_request[0].1.contains("parked payload A"));
    assert!(next_request[1].1.contains("parked payload B"));
    finish_without_a_follow_up(&rig, &[a_id, b_id]).await;
}

#[tokio::test]
async fn steer_admitted_after_the_runner_parked_joins_the_same_request_ephemeral() {
    steer_admitted_after_the_runner_parked_joins_the_same_request(
        DurableSteerRig::ephemeral().await,
    )
    .await;
}

#[tokio::test]
async fn steer_admitted_after_the_runner_parked_joins_the_same_request_persistent() {
    let store: Arc<dyn crate::store::RuntimeStore> =
        Arc::new(crate::store::InMemoryRuntimeStore::new());
    steer_admitted_after_the_runner_parked_joins_the_same_request(
        DurableSteerRig::persistent(store).await,
    )
    .await;
}

/// A steer admitted after the next request's input was fixed is retained
/// whole and joins the following boundary of the same run, with every other
/// steer admitted by then.
#[tokio::test]
async fn steers_admitted_after_the_request_input_is_fixed_join_the_following_request() {
    let rig = DurableSteerRig::ephemeral().await;
    rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    let first = admit_in_order(
        &rig,
        vec![peer_steer("early payload A"), peer_steer("early payload B")],
    )
    .await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    assert_eq!(take_ids(&last_take(&rig)), first);

    // That request is in flight now. C and D arrive after its input was
    // fixed: neither is in it, and both join the request after it.
    let later = admit_in_order(
        &rig,
        vec![peer_steer("late payload C"), peer_steer("late payload D")],
    )
    .await;
    assert_eq!(
        rig.script.applied_durable(),
        first,
        "a steer admitted after the cutoff is in no part of that request"
    );
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    let following = last_take(&rig);
    assert_eq!(take_ids(&following), later);
    assert!(following[0].1.contains("late payload C"));
    assert!(following[1].1.contains("late payload D"));

    let all = first
        .iter()
        .chain(later.iter())
        .cloned()
        .collect::<Vec<_>>();
    finish_without_a_follow_up(&rig, &all).await;
    assert_eq!(rig.script.applied_durable(), all);
    assert_eq!(
        rig.script
            .boundary_takes()
            .iter()
            .map(|take| take.len())
            .collect::<Vec<_>>(),
        vec![0, 2, 2],
        "one empty boundary, then two boundaries of two steers each"
    );
}

async fn recall(rig: &DurableSteerRig, input_id: &InputId) -> Option<QueuedInputRecall> {
    tokio::time::timeout(
        Duration::from_secs(5),
        rig.adapter.recall_queued_input(&rig.session_id, input_id),
    )
    .await
    .expect("a recall answers without waiting for a boundary")
    .expect("recall")
}

/// Three steers wait for one boundary and one of them is recalled before the
/// request's input is fixed. The recalled one is in no request and has no
/// row; the other two still join the same request, in order.
async fn recalled_steer_leaves_the_others_in_the_same_request(
    rig: DurableSteerRig,
    recalled_index: usize,
) {
    rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    let ids = admit_in_order(
        &rig,
        vec![
            peer_steer("recall row payload one"),
            peer_steer("recall row payload two"),
            peer_steer("recall row payload three"),
        ],
    )
    .await;
    let recalled = ids[recalled_index].clone();
    let kept = ids
        .iter()
        .filter(|input_id| **input_id != recalled)
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        recall(&rig, &recalled).await,
        Some(QueuedInputRecall::Abandoned {
            input_id: recalled.clone(),
            attempt_count: 0,
        })
    );
    // Whoever is the oldest steer left holds the boundary.
    rig.wait_for_waiting_delivery().await;

    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    let next_request = last_take(&rig);
    assert_eq!(
        take_ids(&next_request),
        kept,
        "the two steers that were not recalled join the same request in order"
    );
    assert!(
        next_request
            .iter()
            .all(|(_, payload)| !payload.contains(match recalled_index {
                0 => "payload one",
                1 => "payload two",
                _ => "payload three",
            })),
        "the recalled payload reaches no request: {next_request:?}"
    );
    finish_without_a_follow_up(&rig, &kept).await;
    assert_eq!(rig.script.applied_durable(), kept);
    assert_eq!(contributions(&rig.script.primitives(), &recalled), 0);
    assert_eq!(
        rig.phase(&recalled).await,
        Some(InputLifecycleState::Abandoned)
    );
}

#[tokio::test]
async fn recalled_head_steer_leaves_the_others_in_the_same_request() {
    recalled_steer_leaves_the_others_in_the_same_request(DurableSteerRig::ephemeral().await, 0)
        .await;
}

#[tokio::test]
async fn persistent_recalled_head_steer_leaves_the_others_in_the_same_request() {
    let store: Arc<dyn crate::store::RuntimeStore> =
        Arc::new(crate::store::InMemoryRuntimeStore::new());
    recalled_steer_leaves_the_others_in_the_same_request(
        DurableSteerRig::persistent(store).await,
        0,
    )
    .await;
}

#[tokio::test]
async fn recalled_middle_steer_leaves_the_others_in_the_same_request() {
    recalled_steer_leaves_the_others_in_the_same_request(DurableSteerRig::ephemeral().await, 1)
        .await;
}

#[tokio::test]
async fn persistent_recalled_middle_steer_leaves_the_others_in_the_same_request() {
    let store: Arc<dyn crate::store::RuntimeStore> =
        Arc::new(crate::store::InMemoryRuntimeStore::new());
    recalled_steer_leaves_the_others_in_the_same_request(
        DurableSteerRig::persistent(store).await,
        1,
    )
    .await;
}

#[tokio::test]
async fn recalled_last_steer_leaves_the_others_in_the_same_request() {
    recalled_steer_leaves_the_others_in_the_same_request(DurableSteerRig::ephemeral().await, 2)
        .await;
}

/// The claim holder is recalled after the runner parked with it, and two
/// more steers are admitted, all before the publication: the holder's claim
/// stays with the lane, the two join that request in order, and the recalled
/// holder reaches no request.
#[tokio::test]
async fn steer_admitted_while_the_holder_was_recalled_after_the_park_joins_that_request() {
    let rig = DurableSteerRig::ephemeral().await;
    rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    rig.script.prepare_hold_armed.store(true, Ordering::SeqCst);
    let holder = peer_steer("holder payload");
    let holder_id = holder.id().clone();
    rig.admit(holder).await;
    rig.wait_for_waiting_delivery().await;

    // The runner parks with the holder; then the holder is recalled and two
    // more steers are admitted, all before anything is published.
    let steps_before = rig.script.steps_done.load(Ordering::SeqCst);
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    wait_for_prepare_hold(&rig).await;
    assert!(
        recall(&rig, &holder_id)
            .await
            .is_some_and(|answer| answer.abandoned())
    );
    let mut later = Vec::new();
    for payload in ["after recall payload one", "after recall payload two"] {
        let steer = peer_steer(payload);
        later.push(steer.id().clone());
        rig.admit(steer).await;
    }
    rig.script
        .prepare_hold_released
        .store(true, Ordering::SeqCst);
    wait_for_steps_done(&rig, steps_before + 1).await;

    let next_request = last_take(&rig);
    assert_eq!(
        take_ids(&next_request),
        later,
        "the two steers admitted before the cutoff join; the recalled holder does not"
    );
    assert!(
        next_request
            .iter()
            .all(|(_, payload)| !payload.contains("holder payload"))
    );
    finish_without_a_follow_up(&rig, &later).await;
    assert_eq!(contributions(&rig.script.primitives(), &holder_id), 0);
    assert_eq!(
        rig.phase(&holder_id).await,
        Some(InputLifecycleState::Abandoned)
    );
}

/// The persistent driver writes one row per joined steer, bound to the run,
/// before the boundary publishes any of them, and the run's terminal receipt
/// names each joined steer exactly once, in admission order.
#[tokio::test]
async fn persistent_joined_steers_each_have_one_row_and_one_place_in_the_receipt() {
    let store: Arc<dyn crate::store::RuntimeStore> =
        Arc::new(crate::store::InMemoryRuntimeStore::new());
    let rig = DurableSteerRig::persistent(Arc::clone(&store)).await;
    let runtime_id = MeerkatMachine::logical_runtime_id(&rig.session_id);
    let batch = rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    let ids = admit_in_order(
        &rig,
        vec![
            peer_steer("receipt payload one"),
            peer_steer("receipt payload two"),
            peer_steer("receipt payload three"),
        ],
    )
    .await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    assert_eq!(take_ids(&last_take(&rig)), ids);

    let mut run_ids = Vec::new();
    for input_id in &ids {
        let row = store
            .load_input_state(&runtime_id, input_id)
            .await
            .expect("load joined row")
            .expect("joined row persisted");
        assert_eq!(row.seed.phase, InputLifecycleState::Staged);
        run_ids.push(row.seed.last_run_id.clone().expect("durable run binding"));
    }
    assert!(
        run_ids.windows(2).all(|pair| pair[0] == pair[1]),
        "every joined steer is bound to the one running run"
    );
    let run_id = run_ids.remove(0);

    rig.script.step(RunnerStep::Finish);
    for input_id in &ids {
        rig.wait_for_phase(input_id, InputLifecycleState::Consumed)
            .await;
    }
    let receipts = store
        .load_committed_boundary_receipts(&runtime_id, &run_id)
        .await
        .expect("load run receipts");
    let terminal = receipts.last().expect("terminal receipt committed");
    let mut expected = vec![batch];
    expected.extend(ids.iter().cloned());
    assert_eq!(
        terminal.contributing_input_ids, expected,
        "the receipt names the batch and each joined steer once, in admission order"
    );
    assert_eq!(rig.script.applied_durable(), ids);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(rig.script.apply_calls.load(Ordering::SeqCst), 1);
}

/// Steers still waiting when the run ends take one follow-up turn together,
/// in arrival order (unchanged).
#[tokio::test]
async fn steers_still_waiting_when_the_run_ends_take_one_follow_up_in_arrival_order() {
    let rig = DurableSteerRig::ephemeral().await;
    rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenStream)
        .await;
    let ids = admit_in_order(
        &rig,
        vec![
            peer_steer("run end payload one"),
            peer_steer("run end payload two"),
            peer_steer("run end payload three"),
        ],
    )
    .await;
    // The model answers: the run ends with no further boundary.
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_apply_calls(2).await;
    rig.script.step(RunnerStep::Finish);
    for input_id in &ids {
        rig.wait_for_phase(input_id, InputLifecycleState::Consumed)
            .await;
    }
    assert!(rig.script.applied_durable().is_empty());
    assert_eq!(
        rig.script.primitives()[1],
        ids,
        "one follow-up turn starts with all three, in arrival order"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(rig.script.apply_calls.load(Ordering::SeqCst), 2);
}

/// A fresh runtime over `store` takes over `session_id` after the process
/// that held it died, and its runs are finished until every one of `inputs`
/// is consumed.
async fn recover_and_drain(
    store: &Arc<dyn crate::store::RuntimeStore>,
    session_id: SessionId,
    inputs: &[InputId],
) -> DurableSteerRig {
    let recovered = DurableSteerRig::with_adapter_for_session(
        Arc::new(
            MeerkatMachine::persistent_without_blobs(Arc::clone(store))
                .expect("persistent machine"),
        ),
        session_id,
    )
    .await;
    let mut finished = 0;
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let mut consumed = true;
            for input_id in inputs {
                consumed &= recovered.phase(input_id).await == Some(InputLifecycleState::Consumed);
            }
            if consumed {
                return;
            }
            if *recovered.script.applies_observable.borrow() > finished {
                recovered.script.step(RunnerStep::Finish);
                finished += 1;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the recovered runtime consumes every input");
    recovered
}

async fn assert_each_delivered_exactly_once_after_recovery(
    store: &Arc<dyn crate::store::RuntimeStore>,
    recovered: &DurableSteerRig,
    steers: &[InputId],
    crashed_run: Option<&RunId>,
) {
    let runtime_id = MeerkatMachine::logical_runtime_id(&recovered.session_id);
    let primitives = recovered.script.primitives();
    let applied = recovered.script.applied_durable();
    for input_id in steers {
        let at_turn_start = contributions(&primitives, input_id);
        let at_a_boundary = applied
            .iter()
            .filter(|applied_id| *applied_id == input_id)
            .count();
        assert_eq!(
            at_turn_start + at_a_boundary,
            1,
            "{input_id} is delivered exactly once after the restart: {at_turn_start} turn start(s), {at_a_boundary} boundary apply(ies)"
        );
        let row = store
            .load_input_state(&runtime_id, input_id)
            .await
            .expect("load consumed row")
            .expect("consumed row persisted");
        assert_eq!(row.seed.phase, InputLifecycleState::Consumed);
        if let Some(crashed_run) = crashed_run {
            assert_ne!(
                row.seed.last_run_id.as_ref(),
                Some(crashed_run),
                "a run of the recovered runtime consumed {input_id}"
            );
        }
    }
    // Arrival order survives the restart: where two steers are delivered at
    // the same place, the earlier admission comes first.
    let order = primitives
        .iter()
        .flatten()
        .chain(applied.iter())
        .filter(|input_id| steers.contains(input_id))
        .cloned()
        .collect::<Vec<_>>();
    if primitives
        .iter()
        .any(|contributors| steers.iter().all(|steer| contributors.contains(steer)))
    {
        assert_eq!(order, steers, "one turn delivers them in arrival order");
    }
}

/// The process dies after three steers were admitted and before the boundary
/// they wait for: no row is lost and none is delivered twice.
#[tokio::test]
async fn persistent_restart_between_admission_and_the_boundary_delivers_each_steer_once() {
    let store: Arc<dyn crate::store::RuntimeStore> =
        Arc::new(crate::store::InMemoryRuntimeStore::new());
    let crashed = DurableSteerRig::persistent(Arc::clone(&store)).await;
    let session_id = crashed.session_id.clone();
    let runtime_id = MeerkatMachine::logical_runtime_id(&session_id);
    let batch = crashed.start_busy_turn().await;
    crashed
        .script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    let steers = admit_in_order(
        &crashed,
        vec![
            peer_steer("restart payload one"),
            peer_steer("restart payload two"),
            peer_steer("restart payload three"),
        ],
    )
    .await;
    for input_id in &steers {
        let row = store
            .load_input_state(&runtime_id, input_id)
            .await
            .expect("load admitted row")
            .expect("admitted row persisted");
        assert_eq!(row.seed.phase, InputLifecycleState::Queued);
    }
    assert!(crashed.script.applied_durable().is_empty());

    // The process dies with the three admitted and the boundary not reached.
    std::mem::forget(crashed);

    let mut all = vec![batch];
    all.extend(steers.iter().cloned());
    let recovered = recover_and_drain(&store, session_id, &all).await;
    assert_each_delivered_exactly_once_after_recovery(&store, &recovered, &steers, None).await;
}

/// The process dies after one boundary joined three steers and the runner
/// applied them to its live image, which is lost: each is recovered for
/// exactly one delivery, none is lost and none is delivered twice.
#[tokio::test]
async fn persistent_crash_after_three_steers_joined_one_boundary_delivers_each_once() {
    let store: Arc<dyn crate::store::RuntimeStore> =
        Arc::new(crate::store::InMemoryRuntimeStore::new());
    let crashed = DurableSteerRig::persistent(Arc::clone(&store)).await;
    let session_id = crashed.session_id.clone();
    let runtime_id = MeerkatMachine::logical_runtime_id(&session_id);
    let batch = crashed.start_busy_turn().await;
    crashed
        .script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    let steers = admit_in_order(
        &crashed,
        vec![
            peer_steer("crash payload one"),
            peer_steer("crash payload two"),
            peer_steer("crash payload three"),
        ],
    )
    .await;
    crashed
        .script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    assert_eq!(crashed.script.applied_durable(), steers);
    let mut crashed_runs = Vec::new();
    for input_id in &steers {
        let row = store
            .load_input_state(&runtime_id, input_id)
            .await
            .expect("load joined row")
            .expect("joined row persisted");
        assert_eq!(row.seed.phase, InputLifecycleState::Staged);
        crashed_runs.push(row.seed.last_run_id.clone().expect("durable run binding"));
    }
    assert!(crashed_runs.windows(2).all(|pair| pair[0] == pair[1]));
    let crashed_run = crashed_runs.remove(0);

    // The process dies with the run in flight: nothing of the run commits
    // and its live image (which held the three appends) is lost.
    std::mem::forget(crashed);

    let mut all = vec![batch];
    all.extend(steers.iter().cloned());
    let recovered = recover_and_drain(&store, session_id, &all).await;
    assert_each_delivered_exactly_once_after_recovery(
        &store,
        &recovered,
        &steers,
        Some(&crashed_run),
    )
    .await;
}

/// An input that only a fresh turn applies (here a prompt steer that routes
/// to another model) keeps its own rule when it is admitted between two
/// durable steers: it joins no boundary and takes the follow-up turn, and
/// the two durable steers join the next request, in their order.
#[tokio::test]
async fn fresh_turn_only_steer_between_two_durable_steers_takes_the_follow_up() {
    let rig = DurableSteerRig::ephemeral().await;
    rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    let before = peer_steer("fresh turn row payload before");
    let before_id = before.id().clone();
    rig.admit(before).await;
    rig.wait_for_waiting_delivery().await;
    let routed = Input::Prompt(crate::input::PromptInput::new(
        "fresh turn row routed prompt",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                model: Some(meerkat_core::lifecycle::run_primitive::ModelId::new(
                    "claude-sonnet-4-5",
                )),
                ..steer_metadata()
            },
        ),
    ));
    let routed_id = routed.id().clone();
    rig.admit(routed).await;
    let after = peer_steer("fresh turn row payload after");
    let after_id = after.id().clone();
    rig.admit(after).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    let next_request = last_take(&rig);
    assert_eq!(
        take_ids(&next_request),
        vec![before_id.clone(), after_id.clone()],
        "the two durable steers join; the fresh-turn-only input does not"
    );
    assert!(
        next_request
            .iter()
            .all(|(_, payload)| !payload.contains("routed prompt"))
    );
    assert_eq!(
        rig.phase(&routed_id).await,
        Some(InputLifecycleState::Queued)
    );

    // The run ends: exactly one follow-up turn starts with the routed prompt.
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_apply_calls(2).await;
    rig.script.step(RunnerStep::Finish);
    for input_id in [&before_id, &routed_id, &after_id] {
        rig.wait_for_phase(input_id, InputLifecycleState::Consumed)
            .await;
    }
    let primitives = rig.script.primitives();
    assert_eq!(contributions(&primitives, &routed_id), 1);
    assert_eq!(contributions(&primitives, &before_id), 0);
    assert_eq!(contributions(&primitives, &after_id), 0);
    assert_eq!(rig.script.applied_durable(), vec![before_id, after_id]);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(rig.script.apply_calls.load(Ordering::SeqCst), 2);
}

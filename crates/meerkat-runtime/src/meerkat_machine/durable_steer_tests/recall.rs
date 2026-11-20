//! A durable steer recalled with `recall_queued_input` before its boundary.
//!
//! One boundary takes every durable steer waiting in the lane, oldest
//! first. These rows pin that a recalled steer gives back the boundary it
//! waited for: the steers behind it in the lane take that same boundary
//! together, the recalled one never reaches the runner, and the recall
//! answers exactly as before.
use super::*;
use crate::input_state::{InputAbandonReason, InputTerminalOutcome};
use crate::queued_input_recall::QueuedInputRecall;

async fn recall(rig: &DurableSteerRig, input_id: &InputId) -> Option<QueuedInputRecall> {
    tokio::time::timeout(
        Duration::from_secs(5),
        rig.adapter.recall_queued_input(&rig.session_id, input_id),
    )
    .await
    .expect("a recall answers without waiting for a boundary")
    .expect("recall")
}

/// Wait until `count` durable deliveries have been prepared and one of them
/// is registered for a boundary.
async fn wait_for_prepared(rig: &DurableSteerRig, count: usize, context: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while rig.script.prepared_request_only_context().len() < count
            || !rig.state.has_waiting_delivery_for_test()
        {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{context}"));
}

/// Three durable steers wait while the runner is at `window`; the oldest is
/// recalled before its boundary. The second and the third take the very next
/// boundary together, in order, and the recalled one is never delivered.
async fn recalled_front_steer_gives_its_boundary_to_the_next(window_open: bool) {
    let rig = DurableSteerRig::ephemeral().await;
    let batch = rig.start_busy_turn().await;
    if window_open {
        // The model asked for tools: the next boundary is open while they run.
        rig.script
            .step_and_wait(RunnerStep::BoundaryThenToolCalls)
            .await;
    } else {
        // The model streams: no boundary is open.
        rig.script
            .step_and_wait(RunnerStep::BoundaryThenStream)
            .await;
    }
    let ids = admit_in_order(
        &rig,
        vec![
            typed_steer("first", ConversationAppendRole::SystemNotice),
            typed_steer("second", ConversationAppendRole::SystemNotice),
            typed_steer("third", ConversationAppendRole::SystemNotice),
        ],
    )
    .await;
    let [first_id, second_id, third_id] = ids.as_slice() else {
        unreachable!("three steers")
    };

    assert_eq!(
        recall(&rig, first_id).await,
        Some(QueuedInputRecall::Abandoned {
            input_id: first_id.clone(),
            attempt_count: 0,
        })
    );
    wait_for_prepared(
        &rig,
        2,
        "the second steer prepares for the boundary the recalled one waited for",
    )
    .await;

    if !window_open {
        rig.script.step_and_wait(RunnerStep::OpenNextBoundary).await;
    }
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    rig.wait_for_phase(second_id, InputLifecycleState::Staged)
        .await;
    rig.wait_for_phase(third_id, InputLifecycleState::Staged)
        .await;
    assert_eq!(
        rig.script
            .boundary_takes()
            .last()
            .expect("the boundary was taken")
            .iter()
            .map(|(input_id, _)| input_id.clone())
            .collect::<Vec<_>>(),
        vec![second_id.clone(), third_id.clone()],
        "the next request carries the second and the third steer, in order"
    );
    rig.script.step(RunnerStep::Finish);
    for input_id in [&batch, second_id, third_id] {
        rig.wait_for_phase(input_id, InputLifecycleState::Consumed)
            .await;
    }

    assert_eq!(
        rig.script.applied_durable(),
        vec![second_id.clone(), third_id.clone()]
    );
    assert_eq!(contributions(&rig.script.primitives(), first_id), 0);
    let recalled = rig.stored(first_id).await.expect("recalled input row");
    assert_eq!(recalled.seed.phase, InputLifecycleState::Abandoned);
    assert_eq!(
        recalled.seed.terminal_outcome,
        Some(InputTerminalOutcome::Abandoned {
            reason: InputAbandonReason::Cancelled,
        })
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        rig.script.apply_calls.load(Ordering::SeqCst),
        1,
        "both joined the running run; no follow-up turn ran"
    );
    assert_eq!(
        recall(&rig, first_id).await,
        Some(QueuedInputRecall::AlreadyTerminal {
            input_id: first_id.clone(),
            terminal: InputTerminalOutcome::Abandoned {
                reason: InputAbandonReason::Cancelled,
            },
        })
    );
}

#[tokio::test]
async fn recalled_durable_steer_gives_its_open_boundary_to_the_next_steer() {
    recalled_front_steer_gives_its_boundary_to_the_next(true).await;
}

#[tokio::test]
async fn recalled_durable_steer_gives_its_next_boundary_to_the_next_steer() {
    recalled_front_steer_gives_its_boundary_to_the_next(false).await;
}

/// Recalling a steer behind the oldest one changes nothing for the others:
/// the oldest and the one behind the recalled one take the next boundary
/// together, in order.
#[tokio::test]
async fn recalled_durable_steer_behind_the_oldest_leaves_the_order_alone() {
    let rig = DurableSteerRig::ephemeral().await;
    rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    let ids = admit_in_order(
        &rig,
        vec![
            typed_steer("first", ConversationAppendRole::SystemNotice),
            typed_steer("second", ConversationAppendRole::SystemNotice),
            typed_steer("third", ConversationAppendRole::SystemNotice),
        ],
    )
    .await;
    let [first_id, second_id, third_id] = ids.as_slice() else {
        unreachable!("three steers")
    };
    assert!(
        recall(&rig, second_id)
            .await
            .is_some_and(|answer| answer.abandoned())
    );

    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    assert_eq!(
        rig.script
            .boundary_takes()
            .last()
            .expect("the boundary was taken")
            .iter()
            .map(|(input_id, _)| input_id.clone())
            .collect::<Vec<_>>(),
        vec![first_id.clone(), third_id.clone()],
        "one boundary carries the two steers that were not recalled"
    );
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_phase(third_id, InputLifecycleState::Consumed)
        .await;
    assert_eq!(
        rig.script.applied_durable(),
        vec![first_id.clone(), third_id.clone()]
    );
    assert_eq!(contributions(&rig.script.primitives(), second_id), 0);
}

/// A recall that comes after the boundary took the steer answers that a run
/// holds it, touches nothing, and the steer is delivered and consumed.
#[tokio::test]
async fn recall_after_the_boundary_took_the_durable_steer_reports_its_run() {
    let rig = DurableSteerRig::ephemeral().await;
    rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    let steer = typed_steer("taken", ConversationAppendRole::SystemNotice);
    let steer_id = steer.id().clone();
    rig.admit(steer).await;
    rig.wait_for_waiting_delivery().await;
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    rig.wait_for_phase(&steer_id, InputLifecycleState::Staged)
        .await;
    rig.wait_for_applied_durable(1).await;

    let answer = recall(&rig, &steer_id).await;
    assert!(
        matches!(
            &answer,
            Some(QueuedInputRecall::NotQueued {
                phase: InputLifecycleState::Staged,
                run_id: Some(_),
                ..
            })
        ),
        "{answer:?}"
    );
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_phase(&steer_id, InputLifecycleState::Consumed)
        .await;
    assert_eq!(rig.script.applied_durable(), vec![steer_id]);
}

/// A recall that commits after the runner parked at the boundary with the
/// steer (its preparation resolved, its ingress not yet back under the
/// gate) still commits before that boundary's request input is fixed. The
/// recalled steer's work keeps the parked boundary for the lane: the
/// recalled one is never delivered and the recall answers `Abandoned`, and
/// the next steer joins that same boundary.
async fn recall_after_the_runner_parked_with_the_steer(rig: DurableSteerRig) {
    rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    rig.script.prepare_hold_armed.store(true, Ordering::SeqCst);
    let ids = admit_in_order(
        &rig,
        vec![
            typed_steer("first", ConversationAppendRole::SystemNotice),
            typed_steer("second", ConversationAppendRole::SystemNotice),
        ],
    )
    .await;
    let [first_id, second_id] = ids.as_slice() else {
        unreachable!("two steers")
    };

    // The runner parks at the boundary with the first steer, and the
    // first steer's preparation resolves; its ingress is held there.
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    tokio::time::timeout(Duration::from_secs(5), async {
        while !rig.script.prepare_hold_reached.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("the first steer's preparation parks");
    assert_eq!(
        recall(&rig, first_id).await,
        Some(QueuedInputRecall::Abandoned {
            input_id: first_id.clone(),
            attempt_count: 0,
        })
    );
    rig.script
        .prepare_hold_released
        .store(true, Ordering::SeqCst);

    rig.wait_for_applied_durable(1).await;
    assert_eq!(
        rig.script.boundary_takes().last().map(|take| take
            .iter()
            .map(|(input_id, _)| input_id.clone())
            .collect::<Vec<_>>()),
        Some(vec![second_id.clone()]),
        "the parked boundary delivers the steer behind the recalled one"
    );
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_phase(second_id, InputLifecycleState::Consumed)
        .await;
    assert_eq!(rig.script.applied_durable(), vec![second_id.clone()]);
    assert_eq!(contributions(&rig.script.primitives(), first_id), 0);
    assert_eq!(
        rig.phase(first_id).await,
        Some(InputLifecycleState::Abandoned)
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        rig.script.apply_calls.load(Ordering::SeqCst),
        1,
        "the second joined the running run; no follow-up turn ran"
    );
}

#[tokio::test]
async fn recall_after_the_runner_parked_with_the_steer_leaves_the_boundary_to_the_next() {
    recall_after_the_runner_parked_with_the_steer(DurableSteerRig::ephemeral().await).await;
}

#[tokio::test]
async fn persistent_recall_after_the_runner_parked_with_the_steer_leaves_the_boundary_to_the_next()
{
    let store: Arc<dyn crate::store::RuntimeStore> =
        Arc::new(crate::store::InMemoryRuntimeStore::new());
    recall_after_the_runner_parked_with_the_steer(DurableSteerRig::persistent(store).await).await;
}

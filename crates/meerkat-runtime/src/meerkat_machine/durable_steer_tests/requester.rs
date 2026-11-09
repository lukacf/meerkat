//! An input carries its original requester (an opaque principal its host
//! supplies), and inputs with different requesters never share a run.
//!
//! These rows drive the durable steer rows' scripted runner through the
//! runtime ingress, the generated machine and the runtime loop: queued and
//! steered inputs of two people, and requester-less ones.
use super::*;
use meerkat_core::{PrincipalKind, PrincipalRef, TrustDomainId};

/// A person the host authenticated: the opaque, qualified requester Core
/// carries and never interprets.
fn person(subject: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::Human,
        subject,
        TrustDomainId::new("oidc-issuer:https://issuer.example").expect("trust domain"),
    )
    .expect("qualified requester")
}

fn with_requester(mut input: Input, requester: PrincipalRef) -> Input {
    input.header_mut().requester = Some(requester);
    input
}

/// A durable steer prompt (a typed text prompt in the Steer lane) from
/// `requester`.
fn steer_from(text: &str, requester: PrincipalRef) -> Input {
    with_requester(text_prompt_steer(text), requester)
}

/// Start a turn for `requester`'s queued prompt and wait until the runner is
/// inside it.
async fn start_busy_turn_for(rig: &DurableSteerRig, requester: PrincipalRef) -> InputId {
    let prompt = with_requester(
        Input::Prompt(crate::input::PromptInput::new(
            "a person's prompt keeps the runtime busy",
            None,
        )),
        requester,
    );
    let input_id = prompt.id().clone();
    let (outcome, _completion) = rig
        .adapter
        .accept_input_with_completion(&rig.session_id, prompt)
        .await
        .expect("the person's prompt is accepted");
    assert!(outcome.is_accepted());
    tokio::time::timeout(Duration::from_secs(5), rig.script.apply_started.notified())
        .await
        .expect("the person's run starts");
    input_id
}

type BoundaryWorkOutcomes = mpsc::UnboundedReceiver<(Option<InputId>, Result<(), String>)>;

/// Report every accepted input's asynchronous boundary work as it settles.
fn observe_boundary_work(rig: &DurableSteerRig) -> BoundaryWorkOutcomes {
    let (outcomes_tx, outcomes) = mpsc::unbounded_channel();
    *rig.adapter
        .test_accepted_boundary_work_outcomes
        .lock()
        .unwrap() = Some(outcomes_tx);
    outcomes
}

/// Wait until `input_id` either registered a live-boundary delivery in the
/// running turn or settled its boundary work without one.
async fn wait_for_boundary_decision(
    rig: &DurableSteerRig,
    outcomes: &mut BoundaryWorkOutcomes,
    settled: &mut Vec<(Option<InputId>, Result<(), String>)>,
    input_id: &InputId,
) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            while let Ok(outcome) = outcomes.try_recv() {
                settled.push(outcome);
            }
            if rig.state.has_waiting_delivery_for_test()
                || rig.state.has_registered_request_only_delivery_for_test()
                || settled
                    .iter()
                    .any(|(settled_id, _)| settled_id.as_ref() == Some(input_id))
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the boundary work of {input_id} decides"));
}

/// Two people's steers queue while a requester-less run is busy. When it
/// ends, each person's steer runs in a run of its own: the runtime loop never
/// batches two requesters into one run.
#[tokio::test]
async fn different_requesters_never_share_a_run() {
    let rig = DurableSteerRig::ephemeral().await;
    let busy = rig.start_busy_turn().await;
    // The model is streaming: no boundary opens before the run ends.
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenStream)
        .await;
    let from_a = steer_from("from a", person("a"));
    let from_b = steer_from("from b", person("b"));
    let (a, b) = (from_a.id().clone(), from_b.id().clone());
    rig.admit(from_a).await;
    rig.admit(from_b).await;

    rig.script.step(RunnerStep::Finish);
    rig.wait_for_apply_calls(2).await;
    assert_eq!(
        rig.script.primitives()[1],
        vec![a.clone()],
        "the follow-up run serves one requester: the second person's steer is not batched into it"
    );
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_apply_calls(3).await;
    assert_eq!(rig.script.primitives()[2], vec![b.clone()]);
    rig.script.step(RunnerStep::Finish);
    for input_id in [&busy, &a, &b] {
        rig.wait_for_phase(input_id, InputLifecycleState::Consumed)
            .await;
    }
    let run_of = |stored: crate::input_state::StoredInputState| stored.seed.last_run_id;
    assert_ne!(
        run_of(rig.stored(&a).await.expect("a")),
        run_of(rig.stored(&b).await.expect("b")),
        "the two requesters' inputs were consumed by two runs"
    );
}

/// A steer from another person never joins a person's running turn, at any
/// boundary: it waits for a run of its own. The person's own steer, admitted
/// before it, still joins their turn. (One admitted after it waits behind
/// it: an_earlier_steer_of_another_requester_runs_before_a_later_one.)
#[tokio::test]
async fn a_steer_from_another_requester_waits_for_its_own_run() {
    let rig = DurableSteerRig::ephemeral().await;
    let mut outcomes = observe_boundary_work(&rig);
    let mut settled = Vec::new();
    let busy = start_busy_turn_for(&rig, person("a")).await;

    // The person's own steer joins their running turn.
    let from_a = steer_from("from a", person("a"));
    let a = from_a.id().clone();
    rig.admit(from_a).await;
    rig.wait_for_waiting_delivery().await;
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    rig.wait_for_applied_durable(1).await;
    assert_eq!(rig.script.applied_durable(), vec![a.clone()]);

    // Another person's steer joins at no later boundary.
    let from_b = steer_from("from b", person("b"));
    let b = from_b.id().clone();
    rig.admit(from_b).await;
    wait_for_boundary_decision(&rig, &mut outcomes, &mut settled, &b).await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    assert_eq!(
        rig.script.applied_durable(),
        vec![a.clone()],
        "another person's steer never joins the running turn"
    );
    assert_eq!(rig.phase(&b).await, Some(InputLifecycleState::Queued));

    // The person's turn answers; the other person's steer runs alone.
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_apply_calls(2).await;
    assert_eq!(rig.script.primitives()[1], vec![b.clone()]);
    rig.script.step(RunnerStep::Finish);
    for input_id in [&busy, &a, &b] {
        rig.wait_for_phase(input_id, InputLifecycleState::Consumed)
            .await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        rig.script.apply_calls.load(Ordering::SeqCst),
        2,
        "two runs: the person's (with their late steer) and the other person's"
    );
}

/// A background completion steered as request-only context: an input
/// without a requester.
fn background_completion() -> Input {
    let mut continuation = crate::input::ContinuationInput::detached_background_op_completed();
    continuation.turn_append = Some(notice_append(
        "background result",
        ConversationAppendRole::SystemNotice,
    ));
    Input::Continuation(continuation)
}

/// After the steer `earlier` (not of `busy`'s requester) took its boundary
/// decision in `busy`'s running turn, the model streams to the end of that
/// turn, so no boundary opens again, and person a, the turn's own requester,
/// steers once more. Returns that later steer's id, the batches of the next
/// two runs, and the (steer, queue) lanes after `earlier`'s decision and after
/// the later steer's.
async fn steer_again_after_another_requester_s_steer(
    rig: &DurableSteerRig,
    outcomes: &mut BoundaryWorkOutcomes,
    busy: &InputId,
    earlier: &InputId,
) -> (InputId, Vec<Vec<InputId>>, String) {
    let mut settled = Vec::new();
    wait_for_boundary_decision(rig, outcomes, &mut settled, earlier).await;
    let lanes_after_earlier = (rig.steer_queue().await, rig.queue().await);

    rig.script
        .step_and_wait(RunnerStep::BoundaryThenStream)
        .await;
    let later_from_a = steer_from("later from a", person("a"));
    let later = later_from_a.id().clone();
    rig.admit(later_from_a).await;
    wait_for_boundary_decision(rig, outcomes, &mut settled, &later).await;
    let lanes_after_later = (rig.steer_queue().await, rig.queue().await);

    rig.script.step(RunnerStep::Finish);
    rig.wait_for_apply_calls(2).await;
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_apply_calls(3).await;
    rig.script.step(RunnerStep::Finish);
    for input_id in [busy, earlier, &later] {
        rig.wait_for_phase(input_id, InputLifecycleState::Consumed)
            .await;
    }
    let runs = rig.script.primitives()[1..3].to_vec();
    let lanes = format!(
        "(steer, queue) lanes after the earlier steer's decision {lanes_after_earlier:?}, \
         after the later steer's {lanes_after_later:?}"
    );
    (later, runs, lanes)
}

/// The Steer lane is FIFO across requesters. Person b's steer may not join
/// person a's running turn: it keeps its place in the Steer lane. Person a's
/// later steer, which missed every boundary of a's turn, waits behind it, so
/// b's earlier steer runs first, each in a run of its own.
#[tokio::test]
async fn an_earlier_steer_of_another_requester_runs_before_a_later_one() {
    let rig = DurableSteerRig::ephemeral().await;
    let mut outcomes = observe_boundary_work(&rig);
    let busy = start_busy_turn_for(&rig, person("a")).await;

    let from_b = steer_from("from b", person("b"));
    let b = from_b.id().clone();
    rig.admit(from_b).await;
    let (later, runs, lanes) =
        steer_again_after_another_requester_s_steer(&rig, &mut outcomes, &busy, &b).await;
    assert_eq!(
        runs,
        vec![vec![b.clone()], vec![later.clone()]],
        "b's steer, admitted first, runs before a's later steer; {lanes}, b = {b}, later = {later}"
    );
}

/// The order holds for a requester-less steer too: a background completion
/// steered while person a's turn runs (request-only context, which may not
/// join a person's turn) keeps its place in the Steer lane, and a's later
/// steer that missed the turn runs after it.
#[tokio::test]
async fn a_requester_less_steer_keeps_its_place_ahead_of_a_later_steer() {
    let rig = DurableSteerRig::ephemeral().await;
    let mut outcomes = observe_boundary_work(&rig);
    let busy = start_busy_turn_for(&rig, person("a")).await;

    let continuation = background_completion();
    let continuation_id = continuation.id().clone();
    rig.admit(continuation).await;
    let (later, runs, lanes) =
        steer_again_after_another_requester_s_steer(&rig, &mut outcomes, &busy, &continuation_id)
            .await;
    assert_eq!(
        runs,
        vec![vec![continuation_id.clone()], vec![later.clone()]],
        "the requester-less steer, admitted first, runs before a's later steer; {lanes}, \
         continuation = {continuation_id}, later = {later}"
    );
}

/// Wait until the boundary work of `input_id` settled (a join, a fallback or
/// a failure).
async fn wait_for_settled_boundary_work(
    outcomes: &mut BoundaryWorkOutcomes,
    settled: &mut Vec<(Option<InputId>, Result<(), String>)>,
    input_id: &InputId,
) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            while let Ok(outcome) = outcomes.try_recv() {
                settled.push(outcome);
            }
            if settled
                .iter()
                .any(|(settled_id, _)| settled_id.as_ref() == Some(input_id))
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the boundary work of {input_id} settles"));
}

/// The order holds at an open boundary for request-only context. A
/// requester-less run is going and person b's steer waits for a run of its
/// own. A later requester-less background completion (request-only context
/// of the run's own key) reaches an open boundary: it is not injected, keeps
/// its place in the Steer lane, and runs after b's steer. The runtime's
/// predicate keeps it out before its realization; were the predicate to let
/// it through, the receipt's order guard would refuse it inside the
/// realization and the accepted input would end instead of keeping its
/// place.
#[tokio::test]
async fn a_later_request_only_steer_at_an_open_boundary_waits_behind_a_person_s_steer() {
    let rig = DurableSteerRig::ephemeral().await;
    let mut outcomes = observe_boundary_work(&rig);
    let mut settled = Vec::new();
    let busy = rig.start_busy_turn().await;

    let from_b = steer_from("from b", person("b"));
    let b = from_b.id().clone();
    rig.admit(from_b).await;
    wait_for_settled_boundary_work(&mut outcomes, &mut settled, &b).await;

    let continuation = background_completion();
    let continuation_id = continuation.id().clone();
    rig.admit(continuation).await;
    wait_for_boundary_decision(&rig, &mut outcomes, &mut settled, &continuation_id).await;
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    wait_for_settled_boundary_work(&mut outcomes, &mut settled, &continuation_id).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        rig.script.request_only_taken.lock().unwrap().is_empty(),
        "the later request-only steer was injected while b's earlier steer waited"
    );
    assert_eq!(
        rig.phase(&continuation_id).await,
        Some(InputLifecycleState::Queued),
        "the later request-only steer keeps its place; settled {settled:?}"
    );

    rig.script.step(RunnerStep::Finish);
    rig.wait_for_apply_calls(2).await;
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_apply_calls(3).await;
    rig.script.step(RunnerStep::Finish);
    for input_id in [&busy, &b, &continuation_id] {
        rig.wait_for_phase(input_id, InputLifecycleState::Consumed)
            .await;
    }
    assert_eq!(
        rig.script.primitives()[1..3].to_vec(),
        vec![vec![b.clone()], vec![continuation_id.clone()]],
        "b's earlier steer runs first, then the completion"
    );
}

/// The order holds at an open boundary across delivery classes. Person a's
/// run is going and an earlier requester-less background completion
/// (request-only context) waits in the Steer lane. Person a's own later
/// durable steer reaches an open boundary: it does not join a's turn and
/// runs after the completion. No durable steer is ahead of it, so the
/// durable class order does not hold it back: only the order across
/// requesters does (the runtime's predicate, and behind it the machine's
/// join guard).
#[tokio::test]
async fn a_person_s_later_steer_at_an_open_boundary_waits_behind_a_request_only_one() {
    let rig = DurableSteerRig::ephemeral().await;
    let mut outcomes = observe_boundary_work(&rig);
    let mut settled = Vec::new();
    let busy = start_busy_turn_for(&rig, person("a")).await;

    let continuation = background_completion();
    let continuation_id = continuation.id().clone();
    rig.admit(continuation).await;
    wait_for_settled_boundary_work(&mut outcomes, &mut settled, &continuation_id).await;

    let later_from_a = steer_from("later from a", person("a"));
    let later = later_from_a.id().clone();
    rig.admit(later_from_a).await;
    wait_for_boundary_decision(&rig, &mut outcomes, &mut settled, &later).await;
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    wait_for_settled_boundary_work(&mut outcomes, &mut settled, &later).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        rig.script.applied_durable().is_empty(),
        "a's later steer joined a's turn while the earlier completion waited: {:?}",
        rig.script.applied_durable()
    );

    rig.script.step(RunnerStep::Finish);
    rig.wait_for_apply_calls(2).await;
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_apply_calls(3).await;
    rig.script.step(RunnerStep::Finish);
    for input_id in [&busy, &continuation_id, &later] {
        rig.wait_for_phase(input_id, InputLifecycleState::Consumed)
            .await;
    }
    assert_eq!(
        rig.script.primitives()[1..3].to_vec(),
        vec![vec![continuation_id.clone()], vec![later.clone()]],
        "the earlier completion runs first, then a's later steer; settled {settled:?}"
    );
}

/// Absence is a requester value too: an input without a requester (here a
/// background completion steered as request-only context) never joins a
/// person's running turn. It runs in a turn of its own after that run ends.
#[tokio::test]
async fn a_requester_less_steer_never_joins_a_person_s_run() {
    let rig = DurableSteerRig::ephemeral().await;
    let mut outcomes = observe_boundary_work(&rig);
    let mut settled = Vec::new();
    let busy = start_busy_turn_for(&rig, person("a")).await;

    let continuation = background_completion();
    let continuation_id = continuation.id().clone();
    rig.admit(continuation).await;
    wait_for_boundary_decision(&rig, &mut outcomes, &mut settled, &continuation_id).await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    assert!(
        rig.script.request_only_taken.lock().unwrap().is_empty(),
        "requester-less context is never injected into a person's running turn"
    );
    assert_eq!(
        rig.phase(&continuation_id).await,
        Some(InputLifecycleState::Queued)
    );

    rig.script.step(RunnerStep::Finish);
    rig.wait_for_apply_calls(2).await;
    assert_eq!(rig.script.primitives()[1], vec![continuation_id.clone()]);
    rig.script.step(RunnerStep::Finish);
    for input_id in [&busy, &continuation_id] {
        rig.wait_for_phase(input_id, InputLifecycleState::Consumed)
            .await;
    }
}

/// The receipt names the requester its host supplied: at admission, after a
/// restart over the same durable store, and after the input completed and its
/// payload retired.
#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn host_requester_survives_restart() {
    let dir = tempfile::tempdir().expect("store dir");
    let store: Arc<dyn crate::store::RuntimeStore> = Arc::new(
        crate::store::sqlite::SqliteRuntimeStore::new(dir.path().join("runtime.sqlite"))
            .expect("sqlite runtime store"),
    );
    let first = DurableSteerRig::persistent(Arc::clone(&store)).await;
    let session_id = first.session_id.clone();
    let runtime_id = MeerkatMachine::logical_runtime_id(&session_id);
    // A requester-less run keeps the runtime busy, so the person's prompt
    // stays queued across the restart.
    let busy = first.start_busy_turn().await;
    let prompt = with_requester(
        Input::Prompt(crate::input::PromptInput::new("queued for a", None)),
        person("a"),
    );
    let queued = prompt.id().clone();
    first.admit(prompt).await;
    let admitted = first.stored(&queued).await.expect("admitted row");
    assert_eq!(
        admitted.state.requester(),
        Some(&person("a")),
        "the admission receipt names the requester"
    );

    // The process stops with the run in flight; the store keeps both rows.
    std::mem::forget(first);
    let restarted = DurableSteerRig::with_adapter_for_session(
        Arc::new(
            MeerkatMachine::persistent_without_blobs(Arc::clone(&store))
                .expect("persistent machine"),
        ),
        session_id,
    )
    .await;
    let reloaded = store
        .load_input_state(&runtime_id, &queued)
        .await
        .expect("load row")
        .expect("row persisted");
    assert_eq!(
        reloaded.state.requester(),
        Some(&person("a")),
        "the receipt names the requester after a restart"
    );

    // Run both to completion: the payload retires, the requester stays.
    for expected_calls in 1..=3 {
        if restarted.phase(&queued).await == Some(InputLifecycleState::Consumed)
            && restarted.phase(&busy).await == Some(InputLifecycleState::Consumed)
        {
            break;
        }
        restarted.wait_for_apply_calls(expected_calls).await;
        restarted.script.step(RunnerStep::Finish);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    restarted
        .wait_for_phase(&queued, InputLifecycleState::Consumed)
        .await;
    // Consumed lands with the run's commit; the payload retires in the
    // receipt's finalization, a later commit. Wait for the final receipt.
    let consumed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let row = store
                .load_input_state(&runtime_id, &queued)
                .await
                .expect("load consumed row")
                .expect("consumed row persisted");
            if row
                .state
                .terminal_completion
                .as_ref()
                .is_some_and(|completion| {
                    matches!(
                        completion.phase,
                        crate::input_state::InputTerminalCompletionPhase::Finalized { .. }
                    )
                })
            {
                return row;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the completed input's receipt is finalized");
    assert!(
        consumed.state.persisted_input.is_none(),
        "the completed input's payload retired"
    );
    assert_eq!(
        consumed.state.requester(),
        Some(&person("a")),
        "the receipt names the requester after the payload retired"
    );
}

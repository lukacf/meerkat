//! Behavioral tests of the generated batch and stage capability boundary.

#![allow(clippy::expect_used)]

use crate::generated::meerkat::{
    InputLane, InputPhase, RecoveredPeerResponseTerminalApplyIntent, RecoveredRunApplyBoundary,
    RecoveredRuntimeExecutionKind, RunId, State,
    command_capabilities::{
        AuthorizedRuntimeLoopBatch, AuthorizedStageForRun, RuntimeLoopBatchSource,
    },
};

fn queued_input(state: &mut State, id: &str, sequence: u64, lane: InputLane, terminal: bool) {
    state.input_phases.insert(id.into(), InputPhase::Queued);
    state.input_lane.insert(id.into(), lane);
    state.input_recovery_lanes.insert(id.into(), lane);
    state.input_admission_seq.insert(id.into(), sequence);
    state.input_is_prompt.insert(id.into(), false);
    state
        .input_runtime_boundary
        .insert(id.into(), RecoveredRunApplyBoundary::RunStart);
    state
        .input_runtime_execution_kind
        .insert(id.into(), RecoveredRuntimeExecutionKind::ContentTurn);
    if terminal {
        state
            .input_runtime_peer_response_terminal_apply_intent
            .insert(
                id.into(),
                RecoveredPeerResponseTerminalApplyIntent::AppendContentAndRun,
            );
    }
}

fn selected(state: &State, source: RuntimeLoopBatchSource) -> Vec<String> {
    let plan = AuthorizedRuntimeLoopBatch::authorize_runtime_loop_batch_from_state(state)
        .expect("queued owner state must produce a batch");
    assert_eq!(plan.source(), source);
    plan.input_ids().to_vec()
}

fn terminal_singletons(lane: InputLane, source: RuntimeLoopBatchSource) {
    let mut state = State::default();
    // IDs intentionally sort opposite to admission order.
    queued_input(&mut state, "z-first-terminal", 10, lane, true);
    queued_input(&mut state, "a-second-terminal", 20, lane, true);
    queued_input(&mut state, "ordinary-backlog", 30, lane, false);
    if lane == InputLane::Steer {
        queued_input(&mut state, "earlier-queue", 1, InputLane::Queue, false);
    }
    for expected in ["z-first-terminal", "a-second-terminal", "ordinary-backlog"] {
        assert_eq!(selected(&state, source), vec![expected.to_owned()]);
        state.input_lane.remove(expected);
        state
            .input_phases
            .insert(expected.into(), InputPhase::Consumed);
    }
    if lane == InputLane::Steer {
        assert_eq!(
            selected(&state, RuntimeLoopBatchSource::Queue),
            ["earlier-queue"]
        );
    } else {
        assert!(
            AuthorizedRuntimeLoopBatch::authorize_runtime_loop_batch_from_state(&state).is_none()
        );
    }
}

#[test]
fn terminal_response_queue_batch_is_singleton() {
    terminal_singletons(InputLane::Queue, RuntimeLoopBatchSource::Queue);
}

#[test]
fn terminal_response_steer_batch_is_singleton() {
    // Fresh ResponseTerminal ingress uses Queue. This exercises the generated
    // capability's recovered-state boundary without inventing public ingress.
    terminal_singletons(InputLane::Steer, RuntimeLoopBatchSource::Steer);
}

#[test]
fn terminal_response_cannot_join_a_multi_input_stage() {
    for (lane, source) in [
        (InputLane::Queue, RuntimeLoopBatchSource::Queue),
        (InputLane::Steer, RuntimeLoopBatchSource::Steer),
    ] {
        let run_id = RunId::from("active-run");
        let mut state = State {
            current_run_id: Some(run_id.clone()),
            ..State::default()
        };
        queued_input(&mut state, "first-terminal", 1, lane, true);
        queued_input(&mut state, "second-terminal", 2, lane, true);
        queued_input(&mut state, "ordinary-peer", 3, lane, false);
        let authorize = |ids: &[&str]| {
            AuthorizedStageForRun::authorize_stage_for_run_from_state(
                &state,
                &ids.iter().map(|id| (*id).to_owned()).collect::<Vec<_>>(),
                &run_id,
                source,
            )
        };
        for ids in [
            ["first-terminal", "second-terminal"],
            ["first-terminal", "ordinary-peer"],
            ["ordinary-peer", "first-terminal"],
        ] {
            assert!(
                authorize(&ids).is_none(),
                "terminal stage must be singular: {ids:?}"
            );
        }
        for id in ["first-terminal", "second-terminal", "ordinary-peer"] {
            assert!(authorize(&[id]).is_some(), "valid singleton stage: {id}");
        }
    }
}

#[test]
fn ordinary_peer_batches_keep_admission_order_and_stage_together() {
    for (lane, source) in [
        (InputLane::Queue, RuntimeLoopBatchSource::Queue),
        (InputLane::Steer, RuntimeLoopBatchSource::Steer),
    ] {
        let run_id = RunId::from("ordinary-run");
        let mut state = State {
            current_run_id: Some(run_id.clone()),
            ..State::default()
        };
        queued_input(&mut state, "z-first", 10, lane, false);
        queued_input(&mut state, "a-second", 20, lane, false);
        queued_input(&mut state, "terminal-barrier", 30, lane, true);
        queued_input(&mut state, "later-peer", 40, lane, false);
        let ids = selected(&state, source);
        assert_eq!(ids, ["z-first", "a-second"]);
        assert!(
            AuthorizedStageForRun::authorize_stage_for_run_from_state(
                &state, &ids, &run_id, source,
            )
            .is_some()
        );
    }
}

#[test]
fn ordinary_queue_batch_keeps_prompt_boundary() {
    let mut state = State::default();
    queued_input(&mut state, "peer", 1, InputLane::Queue, false);
    queued_input(&mut state, "prompt", 2, InputLane::Queue, false);
    queued_input(&mut state, "later-peer", 3, InputLane::Queue, false);
    state.input_is_prompt.insert("prompt".into(), true);
    assert_eq!(selected(&state, RuntimeLoopBatchSource::Queue), ["peer"]);
    state.input_lane.remove("peer");
    assert_eq!(selected(&state, RuntimeLoopBatchSource::Queue), ["prompt"]);
}

#[test]
fn ordinary_steer_batch_keeps_apply_boundary() {
    let mut state = State::default();
    queued_input(&mut state, "first", 1, InputLane::Steer, false);
    queued_input(&mut state, "second", 2, InputLane::Steer, false);
    state
        .input_runtime_boundary
        .insert("second".into(), RecoveredRunApplyBoundary::RunCheckpoint);
    assert_eq!(selected(&state, RuntimeLoopBatchSource::Steer), ["first"]);
}

#[test]
fn ordinary_batches_keep_execution_kind_boundary() {
    for (lane, source) in [
        (InputLane::Queue, RuntimeLoopBatchSource::Queue),
        (InputLane::Steer, RuntimeLoopBatchSource::Steer),
    ] {
        let mut state = State::default();
        queued_input(&mut state, "first", 1, lane, false);
        queued_input(&mut state, "second", 2, lane, false);
        state.input_runtime_execution_kind.insert(
            "second".into(),
            RecoveredRuntimeExecutionKind::ResumePending,
        );
        assert_eq!(selected(&state, source), ["first"]);
    }
}

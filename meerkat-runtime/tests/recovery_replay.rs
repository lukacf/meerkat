#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Phase 1 red-ok chokepoint tests for runtime/store/input-ledger recovery replay.

use std::sync::Arc;

use chrono::Utc;
use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_runtime::PersistentRuntimeDriver;
use meerkat_runtime::identifiers::LogicalRuntimeId;
use meerkat_runtime::input::{
    Input, InputDurability, InputHeader, InputOrigin, InputVisibility, PromptInput,
};
use meerkat_runtime::input_state::{InputLifecycleState, InputState};
use meerkat_runtime::store::{InMemoryRuntimeStore, RuntimeStore};
use meerkat_runtime::traits::RuntimeDriver;
use uuid::Uuid;

fn make_runtime_id(label: &str) -> LogicalRuntimeId {
    LogicalRuntimeId::new(format!("phase1-recovery-{label}-{}", Uuid::now_v7()))
}

fn make_prompt(text: &str) -> Input {
    Input::Prompt(PromptInput {
        header: InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: InputOrigin::Operator,
            durability: InputDurability::Durable,
            visibility: InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        text: text.into(),
        blocks: None,
        turn_metadata: None,
    })
}

fn applied_pending_state(input: &Input, run_id: &RunId, sequence: u64) -> InputState {
    let mut state = InputState::new_accepted(input.id().clone());
    state.persisted_input = Some(input.clone());
    state.durability = Some(InputDurability::Durable);
    state.current_state = InputLifecycleState::AppliedPendingConsumption;
    state.last_run_id = Some(run_id.clone());
    state.last_boundary_sequence = Some(sequence);
    state
}

fn sorted_ids(ids: impl IntoIterator<Item = InputId>) -> Vec<String> {
    let mut ids = ids.into_iter().map(|id| id.to_string()).collect::<Vec<_>>();
    ids.sort();
    ids
}

#[tokio::test]
#[ignore = "Phase 1 red-ok runtime recovery chokepoint suite"]
async fn recovery_replay_red_ok_requeues_missing_boundary_contributors_through_persistent_driver() {
    let runtime_id = make_runtime_id("missing-receipt");
    let run_id = RunId::new();
    let first = make_prompt("first replay contribution");
    let second = make_prompt("second replay contribution");
    let first_id = first.id().clone();
    let second_id = second.id().clone();
    let expected = sorted_ids(vec![first_id.clone(), second_id.clone()]);
    let store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());

    store
        .persist_input_state(&runtime_id, &applied_pending_state(&first, &run_id, 0))
        .await
        .expect("persist first applied state");
    store
        .persist_input_state(&runtime_id, &applied_pending_state(&second, &run_id, 0))
        .await
        .expect("persist second applied state");

    let mut driver = PersistentRuntimeDriver::new(runtime_id.clone(), Arc::clone(&store));
    let report = driver.recover().await.expect("recover persistent driver");
    assert_eq!(
        report.inputs_recovered, 2,
        "missing boundary receipts should roll both contributors back into replay"
    );
    assert_eq!(
        sorted_ids(driver.active_input_ids()),
        expected,
        "recovery should preserve both contributors as active replay inputs"
    );

    for input_id in [&first_id, &second_id] {
        let state = driver.input_state(input_id).expect("driver input state");
        assert_eq!(state.current_state, InputLifecycleState::Queued);

        let stored = store
            .load_input_state(&runtime_id, input_id)
            .await
            .expect("load persisted state")
            .expect("persisted input record");
        assert_eq!(stored.current_state, InputLifecycleState::Queued);
        assert_eq!(stored.last_run_id, Some(run_id.clone()));
        assert_eq!(stored.last_boundary_sequence, Some(0));
    }

    let replayed = vec![
        driver.dequeue_next().expect("first replay input").0,
        driver.dequeue_next().expect("second replay input").0,
    ];
    assert!(
        driver.dequeue_next().is_none(),
        "recovery should requeue only the missing-boundary contributors"
    );
    assert_eq!(
        sorted_ids(replayed),
        expected,
        "replay order should surface the recovered contributors exactly once"
    );
    let _ = RunApplyBoundary::RunStart;
}

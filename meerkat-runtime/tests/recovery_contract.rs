#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Phase 0 external-boundary contract tests for runtime receipt/replay recovery.
//!
//! These tests stay out of the fast suite on purpose. They exercise the real
//! runtime store implementations and both runtime drivers through one outside-in
//! recovery matrix.

use std::sync::Arc;

use chrono::Utc;
use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
use meerkat_core::lifecycle::{InputId, RunBoundaryReceipt, RunEvent, RunId};
use meerkat_runtime::identifiers::LogicalRuntimeId;
use meerkat_runtime::input::{
    Input, InputDurability, InputHeader, InputOrigin, InputVisibility, PromptInput,
};
use meerkat_runtime::input_state::{InputLifecycleState, InputState, InputTerminalOutcome};
use meerkat_runtime::runtime_state::RuntimeState;
use meerkat_runtime::store::{InMemoryRuntimeStore, RuntimeStore, SessionDelta};
use meerkat_runtime::traits::RuntimeDriver;
use meerkat_runtime::{EphemeralRuntimeDriver, PersistentRuntimeDriver};
use tempfile::TempDir;
use uuid::Uuid;

#[cfg(feature = "redb-store")]
use meerkat_runtime::store::RedbRuntimeStore;
#[cfg(feature = "sqlite-store")]
use meerkat_runtime::store::SqliteRuntimeStore;

struct StoreHarness {
    name: &'static str,
    store: Arc<dyn RuntimeStore>,
    _tempdir: Option<TempDir>,
}

fn supported_store_harnesses() -> Vec<StoreHarness> {
    #[allow(unused_mut)]
    let mut harnesses = vec![StoreHarness {
        name: "memory",
        store: Arc::new(InMemoryRuntimeStore::new()),
        _tempdir: None,
    }];

    #[cfg(feature = "sqlite-store")]
    {
        let tempdir = TempDir::new().unwrap();
        let db_path = tempdir.path().join("runtime.sqlite3");
        let store = Arc::new(SqliteRuntimeStore::new(&db_path).unwrap());
        harnesses.push(StoreHarness {
            name: "sqlite",
            store,
            _tempdir: Some(tempdir),
        });
    }

    #[cfg(feature = "redb-store")]
    {
        let tempdir = TempDir::new().unwrap();
        let db_path = tempdir.path().join("runtime.redb");
        let db = Arc::new(redb::Database::create(&db_path).unwrap());
        let store = Arc::new(RedbRuntimeStore::new(db).unwrap());
        harnesses.push(StoreHarness {
            name: "redb",
            store,
            _tempdir: Some(tempdir),
        });
    }

    harnesses
}

fn make_runtime_id(label: &str) -> LogicalRuntimeId {
    LogicalRuntimeId::new(format!("recovery-{label}-{}", Uuid::now_v7()))
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

fn make_session_snapshot() -> Vec<u8> {
    serde_json::to_vec(&meerkat_core::Session::new()).unwrap()
}

fn make_receipt(
    run_id: RunId,
    contributing_input_ids: Vec<InputId>,
    sequence: u64,
) -> RunBoundaryReceipt {
    RunBoundaryReceipt {
        run_id,
        boundary: RunApplyBoundary::RunStart,
        contributing_input_ids,
        conversation_digest: None,
        message_count: 0,
        sequence,
    }
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

fn sorted_id_strings(ids: impl IntoIterator<Item = InputId>) -> Vec<String> {
    let mut ids = ids.into_iter().map(|id| id.to_string()).collect::<Vec<_>>();
    ids.sort();
    ids
}

#[tokio::test]
#[ignore = "Phase 0 external boundary contract"]
async fn recovery_store_contract_commits_authoritative_receipts_across_supported_backends() {
    for harness in supported_store_harnesses() {
        let runtime_id = make_runtime_id(harness.name);
        let run_id = RunId::new();
        let first = make_prompt("first contribution");
        let second = make_prompt("second contribution");
        let first_id = first.id().clone();
        let second_id = second.id().clone();

        let receipt = harness
            .store
            .commit_session_boundary(
                &runtime_id,
                SessionDelta {
                    session_snapshot: make_session_snapshot(),
                },
                run_id.clone(),
                RunApplyBoundary::RunStart,
                vec![first_id.clone(), second_id.clone()],
                vec![
                    InputState::new_accepted(first_id.clone()),
                    InputState::new_accepted(second_id.clone()),
                ],
            )
            .await
            .unwrap();

        assert_eq!(
            receipt.sequence, 0,
            "{}: first authoritative receipt should start at sequence zero",
            harness.name
        );
        assert_eq!(
            receipt.contributing_input_ids,
            vec![first_id.clone(), second_id.clone()],
            "{}: authoritative receipt should preserve contributor order",
            harness.name
        );
        assert!(
            receipt.conversation_digest.is_some(),
            "{}: authoritative receipt should include the session digest",
            harness.name
        );
        assert_eq!(
            receipt.message_count, 0,
            "{}: empty session snapshot should produce zero messages",
            harness.name
        );

        let loaded_receipt = harness
            .store
            .load_boundary_receipt(&runtime_id, &run_id, 0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            loaded_receipt, receipt,
            "{}: stored receipt should round-trip without drift",
            harness.name
        );

        let first_state = harness
            .store
            .load_input_state(&runtime_id, &first_id)
            .await
            .unwrap()
            .unwrap();
        let second_state = harness
            .store
            .load_input_state(&runtime_id, &second_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            first_state.last_run_id,
            Some(run_id.clone()),
            "{}: first contributor should record the authoritative run id",
            harness.name
        );
        assert_eq!(
            first_state.last_boundary_sequence,
            Some(0),
            "{}: first contributor should record the authoritative boundary sequence",
            harness.name
        );
        assert_eq!(
            second_state.last_run_id,
            Some(run_id.clone()),
            "{}: second contributor should record the authoritative run id",
            harness.name
        );
        assert_eq!(
            second_state.last_boundary_sequence,
            Some(0),
            "{}: second contributor should record the authoritative boundary sequence",
            harness.name
        );

        let second_receipt = harness
            .store
            .commit_session_boundary(
                &runtime_id,
                SessionDelta {
                    session_snapshot: make_session_snapshot(),
                },
                run_id.clone(),
                RunApplyBoundary::Immediate,
                vec![second_id.clone()],
                vec![InputState::new_accepted(second_id.clone())],
            )
            .await
            .unwrap();
        assert_eq!(
            second_receipt.sequence, 1,
            "{}: the durable commit seam should mint the next receipt sequence",
            harness.name
        );
    }
}

#[tokio::test]
#[ignore = "Phase 0 external boundary contract"]
async fn recovery_persistent_driver_contract_replays_missing_receipts_and_persists_retire_across_supported_backends()
 {
    for harness in supported_store_harnesses() {
        let runtime_id = make_runtime_id(harness.name);
        let run_id = RunId::new();
        let first = make_prompt("first recovery replay");
        let second = make_prompt("second recovery replay");
        let first_id = first.id().clone();
        let second_id = second.id().clone();
        let expected_ids = sorted_id_strings(vec![first_id.clone(), second_id.clone()]);

        harness
            .store
            .persist_input_state(&runtime_id, &applied_pending_state(&first, &run_id, 0))
            .await
            .unwrap();
        harness
            .store
            .persist_input_state(&runtime_id, &applied_pending_state(&second, &run_id, 0))
            .await
            .unwrap();

        let mut driver = PersistentRuntimeDriver::new(runtime_id.clone(), harness.store.clone());
        let report = driver.recover().await.unwrap();
        assert_eq!(
            report.inputs_recovered, 2,
            "{}: missing boundary receipts should recover both contributors for replay",
            harness.name
        );
        assert_eq!(
            sorted_id_strings(driver.active_input_ids()),
            expected_ids,
            "{}: both contributors should remain active after replay recovery",
            harness.name
        );

        for input_id in [&first_id, &second_id] {
            let state = driver.input_state(input_id).unwrap();
            assert_eq!(
                state.current_state,
                InputLifecycleState::Queued,
                "{}: missing receipts should roll applied contributors back to queued",
                harness.name
            );
            let stored = harness
                .store
                .load_input_state(&runtime_id, input_id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                stored.current_state,
                InputLifecycleState::Queued,
                "{}: recovered replay state should be persisted back to the store",
                harness.name
            );
        }

        let replayed_ids = vec![
            driver.dequeue_next().unwrap().0,
            driver.dequeue_next().unwrap().0,
        ];
        assert!(
            driver.dequeue_next().is_none(),
            "{}: only the recovered contributors should be queued for replay",
            harness.name
        );
        assert_eq!(
            sorted_id_strings(replayed_ids),
            expected_ids,
            "{}: replay queue should contain exactly the recovered contributors",
            harness.name
        );

        let retire_report = driver.retire().await.unwrap();
        assert_eq!(
            retire_report.inputs_pending_drain, 2,
            "{}: retire should preserve the replayable contributors for later drain",
            harness.name
        );
        assert_eq!(
            harness.store.load_runtime_state(&runtime_id).await.unwrap(),
            Some(RuntimeState::Retired),
            "{}: retire should persist the runtime state atomically with input state",
            harness.name
        );

        drop(driver);

        let mut retired_driver =
            PersistentRuntimeDriver::new(runtime_id.clone(), harness.store.clone());
        retired_driver.recover().await.unwrap();
        assert_eq!(
            retired_driver.runtime_state(),
            RuntimeState::Retired,
            "{}: persisted retire state should round-trip through recovery",
            harness.name
        );
        assert_eq!(
            sorted_id_strings(retired_driver.active_input_ids()),
            expected_ids,
            "{}: retire recovery should keep the replayable contributors active",
            harness.name
        );

        let retired_replayed_ids = vec![
            retired_driver.dequeue_next().unwrap().0,
            retired_driver.dequeue_next().unwrap().0,
        ];
        assert!(
            retired_driver.dequeue_next().is_none(),
            "{}: retire recovery should requeue the preserved contributors exactly once",
            harness.name
        );
        assert_eq!(
            sorted_id_strings(retired_replayed_ids),
            expected_ids,
            "{}: retire recovery should surface the same replay contributors",
            harness.name
        );
    }
}

#[tokio::test]
#[ignore = "Phase 0 external boundary contract"]
async fn recovery_persistent_driver_contract_consumes_committed_boundary_contributors_across_supported_backends()
 {
    for harness in supported_store_harnesses() {
        let runtime_id = make_runtime_id(harness.name);
        let run_id = RunId::new();
        let first = make_prompt("first committed contribution");
        let second = make_prompt("second committed contribution");
        let first_id = first.id().clone();
        let second_id = second.id().clone();
        let receipt = make_receipt(run_id.clone(), vec![first_id.clone(), second_id.clone()], 0);

        harness
            .store
            .atomic_apply(
                &runtime_id,
                Some(SessionDelta {
                    session_snapshot: make_session_snapshot(),
                }),
                receipt.clone(),
                vec![
                    applied_pending_state(&first, &run_id, 0),
                    applied_pending_state(&second, &run_id, 0),
                ],
                None,
            )
            .await
            .unwrap();

        let mut driver = PersistentRuntimeDriver::new(runtime_id.clone(), harness.store.clone());
        driver.recover().await.unwrap();

        assert!(
            driver.active_input_ids().is_empty(),
            "{}: committed contributors should not remain active after recovery",
            harness.name
        );
        assert!(
            driver.dequeue_next().is_none(),
            "{}: committed contributors should not be replayed after recovery",
            harness.name
        );
        assert_eq!(
            harness.store.load_runtime_state(&runtime_id).await.unwrap(),
            Some(RuntimeState::Idle),
            "{}: recovery should persist the runtime back to an idle lifecycle state",
            harness.name
        );

        for input_id in [&first_id, &second_id] {
            let recovered = driver.input_state(input_id).unwrap();
            assert_eq!(
                recovered.current_state,
                InputLifecycleState::Consumed,
                "{}: committed contributors should recover as consumed",
                harness.name
            );
            assert_eq!(
                recovered.terminal_outcome,
                Some(InputTerminalOutcome::Consumed),
                "{}: committed contributors should recover with a consumed terminal outcome",
                harness.name
            );

            let stored = harness
                .store
                .load_input_state(&runtime_id, input_id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                stored.current_state,
                InputLifecycleState::Consumed,
                "{}: consumed recovery state should be persisted back to the store",
                harness.name
            );
        }

        let loaded_receipt = harness
            .store
            .load_boundary_receipt(&runtime_id, &run_id, 0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            loaded_receipt.contributing_input_ids,
            vec![first_id.clone(), second_id.clone()],
            "{}: committed receipt should preserve contributor ordering through recovery",
            harness.name
        );
    }
}

#[tokio::test]
#[ignore = "Phase 0 external boundary contract"]
async fn recovery_ephemeral_driver_contract_keeps_applied_boundary_inputs_out_of_replay() {
    let mut driver = EphemeralRuntimeDriver::new(make_runtime_id("ephemeral"));
    let first = make_prompt("first ephemeral contribution");
    let second = make_prompt("second ephemeral contribution");
    let first_id = first.id().clone();
    let second_id = second.id().clone();
    let expected_ids = sorted_id_strings(vec![first_id.clone(), second_id.clone()]);

    driver.accept_input(first).await.unwrap();
    driver.accept_input(second).await.unwrap();
    let _ = driver.take_wake_requested();

    let dequeued_first = driver.dequeue_next().unwrap().0;
    let dequeued_second = driver.dequeue_next().unwrap().0;
    assert_eq!(
        dequeued_first, first_id,
        "ephemeral driver should drain contributors in admission order before recovery"
    );
    assert_eq!(
        dequeued_second, second_id,
        "ephemeral driver should drain contributors in admission order before recovery"
    );

    let run_id = RunId::new();
    driver.start_run(run_id.clone()).unwrap();
    driver.stage_input(&first_id, &run_id).unwrap();
    driver.stage_input(&second_id, &run_id).unwrap();
    driver
        .on_run_event(RunEvent::BoundaryApplied {
            run_id: run_id.clone(),
            receipt: make_receipt(run_id, vec![first_id.clone(), second_id.clone()], 0),
            session_snapshot: Some(make_session_snapshot()),
        })
        .await
        .unwrap();

    driver
        .state_machine_mut()
        .transition(RuntimeState::Recovering)
        .unwrap();

    let report = driver.recover().await.unwrap();
    assert_eq!(
        report.inputs_recovered, 2,
        "ephemeral recovery should preserve both applied contributors in memory"
    );
    assert_eq!(
        sorted_id_strings(driver.active_input_ids()),
        expected_ids,
        "ephemeral recovery should keep the same contributors active"
    );

    for input_id in [&first_id, &second_id] {
        let state = driver.input_state(input_id).unwrap();
        assert_eq!(
            state.current_state,
            InputLifecycleState::AppliedPendingConsumption,
            "ephemeral recovery should not replay already-applied contributors"
        );
    }
    assert!(
        driver.dequeue_next().is_none(),
        "ephemeral recovery should not requeue already-applied contributors"
    );
}

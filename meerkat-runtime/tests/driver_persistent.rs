#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use chrono::Utc;
use meerkat_core::BlobStore;
use meerkat_core::lifecycle::InputId;
use meerkat_core::types::{ContentBlock, ImageData};
use meerkat_runtime::{
    InMemoryRuntimeStore, Input, InputDurability, InputHeader, InputLifecycleState, InputOrigin,
    InputState, InputVisibility, LogicalRuntimeId, PersistentRuntimeDriver, PromptInput,
    RuntimeDriver, RuntimeStore,
};
use meerkat_store::MemoryBlobStore;

fn memory_blob_store() -> Arc<dyn BlobStore> {
    Arc::new(MemoryBlobStore::new())
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

fn make_multimodal_prompt(text: &str, label: &str) -> Input {
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
        blocks: Some(vec![
            ContentBlock::Text {
                text: text.to_string(),
            },
            ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: ImageData::Inline {
                    data: format!("base64-{label}"),
                },
            },
        ]),
        turn_metadata: None,
    })
}

#[tokio::test]
async fn durable_before_ack() {
    let store = Arc::new(InMemoryRuntimeStore::new());
    let rid = LogicalRuntimeId::new("test");
    let mut driver = PersistentRuntimeDriver::new(rid.clone(), store.clone(), memory_blob_store());

    let input = make_prompt("hello");
    let input_id = input.id().clone();
    let outcome = driver.accept_input(input).await.unwrap();
    assert!(outcome.is_accepted());

    // Verify state was persisted to store BEFORE we returned
    let stored = store.load_input_state(&rid, &input_id).await.unwrap();
    assert!(stored.is_some());
    assert!(stored.unwrap().persisted_input.is_some());
}

#[tokio::test]
async fn dedup_not_persisted() {
    let store = Arc::new(InMemoryRuntimeStore::new());
    let rid = LogicalRuntimeId::new("test");
    let mut driver = PersistentRuntimeDriver::new(rid.clone(), store.clone(), memory_blob_store());

    let key = meerkat_runtime::identifiers::IdempotencyKey::new("req-1");
    let mut input1 = make_prompt("hello");
    if let Input::Prompt(ref mut p) = input1 {
        p.header.idempotency_key = Some(key.clone());
    }
    driver.accept_input(input1).await.unwrap();

    let mut input2 = make_prompt("hello again");
    if let Input::Prompt(ref mut p) = input2 {
        p.header.idempotency_key = Some(key);
    }
    let outcome = driver.accept_input(input2).await.unwrap();
    assert!(outcome.is_deduplicated());

    // Only one state in store
    let states = store.load_input_states(&rid).await.unwrap();
    assert_eq!(states.len(), 1);
}

#[tokio::test]
async fn recover_from_store() {
    let store = Arc::new(InMemoryRuntimeStore::new());
    let rid = LogicalRuntimeId::new("test");

    // Pre-populate store with a state (simulating crash recovery)
    let input = make_prompt("hello");
    let input_id = input.id().clone();
    let mut state = InputState::new_accepted(input_id.clone());
    state.persisted_input = Some(input.clone());
    state.durability = Some(InputDurability::Durable);
    store.persist_input_state(&rid, &state).await.unwrap();

    // Create a fresh driver (simulating restart)
    let mut driver = PersistentRuntimeDriver::new(rid, store, memory_blob_store());

    // Recover
    let report = driver.recover().await.unwrap();
    assert_eq!(report.inputs_recovered, 1);

    // State should now be in the driver
    assert!(driver.input_state(&input_id).is_some());
    let dequeued = driver.dequeue_next();
    assert!(
        dequeued.is_some(),
        "Recovered queued input should be re-enqueued"
    );
    let (queued_id, queued_input) = dequeued.unwrap();
    assert_eq!(queued_id, input_id);
    assert_eq!(queued_input.id(), &input_id);
}

#[tokio::test]
async fn recover_rebuilds_dedup_index() {
    let store = Arc::new(InMemoryRuntimeStore::new());
    let rid = LogicalRuntimeId::new("test");
    let key = meerkat_runtime::identifiers::IdempotencyKey::new("dedup-key");

    // Pre-populate store with a state that has an idempotency key
    let input_id = InputId::new();
    let mut state = InputState::new_accepted(input_id.clone());
    state.idempotency_key = Some(key.clone());
    state.durability = Some(InputDurability::Durable);
    store.persist_input_state(&rid, &state).await.unwrap();

    // Create a fresh driver and recover
    let mut driver = PersistentRuntimeDriver::new(rid, store, memory_blob_store());
    driver.recover().await.unwrap();

    // Now try to accept a new input with the same idempotency key
    let mut dup_input = make_prompt("duplicate");
    if let Input::Prompt(ref mut p) = dup_input {
        p.header.idempotency_key = Some(key);
    }
    let outcome = driver.accept_input(dup_input).await.unwrap();
    assert!(
        outcome.is_deduplicated(),
        "After recovery, dedup index should be rebuilt so duplicates are caught"
    );
}

#[tokio::test]
async fn recover_filters_ephemeral_inputs() {
    let store = Arc::new(InMemoryRuntimeStore::new());
    let rid = LogicalRuntimeId::new("test");

    // Pre-populate with an ephemeral input state
    let input_id = InputId::new();
    let mut state = InputState::new_accepted(input_id.clone());
    state.durability = Some(InputDurability::Ephemeral);
    store.persist_input_state(&rid, &state).await.unwrap();

    // Create fresh driver and recover
    let mut driver = PersistentRuntimeDriver::new(rid, store, memory_blob_store());
    let report = driver.recover().await.unwrap();

    // Ephemeral input should NOT be recovered (it shouldn't survive restart)
    assert!(
        driver.input_state(&input_id).is_none(),
        "Ephemeral inputs should be filtered during recovery"
    );
    assert_eq!(report.inputs_recovered, 0);
}

#[tokio::test]
async fn boundary_applied_persists_atomically() {
    use meerkat_core::lifecycle::RunId;
    use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
    use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;

    let store = Arc::new(InMemoryRuntimeStore::new());
    let rid = LogicalRuntimeId::new("test");
    let mut driver = PersistentRuntimeDriver::new(rid.clone(), store.clone(), memory_blob_store());

    // Accept and manually process an input
    let input = make_prompt("hello");
    let input_id = input.id().clone();
    driver.accept_input(input).await.unwrap();

    let run_id = RunId::new();
    driver.start_run(run_id.clone()).unwrap();
    driver.stage_input(&input_id, &run_id).unwrap();

    // Fire BoundaryApplied — this should persist atomically
    let receipt = RunBoundaryReceipt {
        run_id: run_id.clone(),
        boundary: RunApplyBoundary::RunStart,
        contributing_input_ids: vec![input_id.clone()],
        conversation_digest: None,
        message_count: 1,
        sequence: 0,
    };
    driver
        .on_run_event(meerkat_core::lifecycle::RunEvent::BoundaryApplied {
            run_id: run_id.clone(),
            receipt: receipt.clone(),
            session_snapshot: Some(b"session-data".to_vec()),
        })
        .await
        .unwrap();

    // Verify the receipt was persisted via atomic_apply
    let loaded = store.load_boundary_receipt(&rid, &run_id, 0).await.unwrap();
    assert!(
        loaded.is_some(),
        "BoundaryApplied should persist the receipt via atomic_apply"
    );
}

#[tokio::test]
async fn durable_runtime_input_externalizes_inline_images_before_ack() {
    let store = Arc::new(InMemoryRuntimeStore::new());
    let rid = LogicalRuntimeId::new("test");
    let mut driver = PersistentRuntimeDriver::new(rid.clone(), store.clone(), memory_blob_store());

    let input = make_multimodal_prompt("hello", "driver");
    let input_id = input.id().clone();
    let outcome = driver.accept_input(input).await.unwrap();
    assert!(outcome.is_accepted());

    let stored = store
        .load_input_state(&rid, &input_id)
        .await
        .unwrap()
        .expect("persisted input should exist");
    let persisted_input = stored
        .persisted_input
        .expect("accepted durable input should be persisted");
    match persisted_input {
        Input::Prompt(prompt) => {
            let blocks = prompt.blocks.expect("multimodal blocks should persist");
            assert!(
                blocks.iter().any(|block| matches!(
                    block,
                    ContentBlock::Image {
                        data: ImageData::Blob { .. },
                        ..
                    }
                )),
                "persisted runtime input should externalize image bytes"
            );
            assert!(
                !blocks.iter().any(|block| matches!(
                    block,
                    ContentBlock::Image {
                        data: ImageData::Inline { .. },
                        ..
                    }
                )),
                "persisted runtime input must not retain inline image bytes"
            );
        }
        other => panic!("expected prompt input, got {other:?}"),
    }
}

#[tokio::test]
async fn retire_preserves_inputs_for_drain() {
    let store = Arc::new(InMemoryRuntimeStore::new());
    let rid = LogicalRuntimeId::new("test");
    let mut driver = PersistentRuntimeDriver::new(rid.clone(), store.clone(), memory_blob_store());

    let input = make_prompt("hello");
    let input_id = input.id().clone();
    driver.accept_input(input).await.unwrap();

    let report = driver.retire().await.unwrap();
    assert_eq!(report.inputs_abandoned, 0);
    assert_eq!(report.inputs_pending_drain, 1);

    // Input is still queued, not abandoned
    let stored = store
        .load_input_state(&rid, &input_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.current_state(),
        meerkat_runtime::input_state::InputLifecycleState::Queued
    );
}

#[tokio::test]
async fn reset_persists_abandoned_inputs() {
    let store = Arc::new(InMemoryRuntimeStore::new());
    let rid = LogicalRuntimeId::new("test");
    let mut driver = PersistentRuntimeDriver::new(rid.clone(), store.clone(), memory_blob_store());

    let input = make_prompt("hello");
    let input_id = input.id().clone();
    driver.accept_input(input).await.unwrap();

    let report = driver.reset().await.unwrap();
    assert_eq!(report.inputs_abandoned, 1);

    let stored = store
        .load_input_state(&rid, &input_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.current_state(),
        meerkat_runtime::input_state::InputLifecycleState::Abandoned
    );
}

#[tokio::test]
async fn recover_consumes_committed_applied_pending_inputs() {
    use meerkat_core::lifecycle::RunId;
    use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
    use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;

    let store = Arc::new(InMemoryRuntimeStore::new());
    let rid = LogicalRuntimeId::new("test");
    let input = make_prompt("already committed");
    let input_id = input.id().clone();
    let run_id = RunId::new();

    let mut state = InputState::new_accepted(input_id.clone());
    state.persisted_input = Some(input);
    state.durability = Some(InputDurability::Durable);
    // Transition through authority: Accepted -> Queued -> Staged -> Applied -> AppliedPendingConsumption
    use meerkat_runtime::input_lifecycle_authority::InputLifecycleInput;
    state.apply(InputLifecycleInput::QueueAccepted).unwrap();
    state
        .apply(InputLifecycleInput::StageForRun {
            run_id: run_id.clone(),
        })
        .unwrap();
    state
        .apply(InputLifecycleInput::MarkApplied {
            run_id: run_id.clone(),
        })
        .unwrap();
    state
        .apply(InputLifecycleInput::MarkAppliedPendingConsumption {
            boundary_sequence: 0,
        })
        .unwrap();
    store.persist_input_state(&rid, &state).await.unwrap();
    store
        .atomic_apply(
            &rid,
            None,
            RunBoundaryReceipt {
                run_id: run_id.clone(),
                boundary: RunApplyBoundary::RunStart,
                contributing_input_ids: vec![input_id.clone()],
                conversation_digest: None,
                message_count: 1,
                sequence: 0,
            },
            vec![state.clone()],
            None,
        )
        .await
        .unwrap();

    let mut driver = PersistentRuntimeDriver::new(rid, store, memory_blob_store());
    driver.recover().await.unwrap();

    let recovered = driver.input_state(&input_id);
    assert!(
        recovered.is_some(),
        "committed input should remain queryable after recovery"
    );
    let Some(recovered) = recovered else {
        unreachable!("asserted some recovery state above");
    };
    assert_eq!(recovered.current_state(), InputLifecycleState::Consumed);
    assert!(
        driver.active_input_ids().is_empty(),
        "committed applied inputs should not stay active after recovery"
    );
    assert!(
        driver.dequeue_next().is_none(),
        "committed applied inputs should not be replayed after recovery"
    );
}

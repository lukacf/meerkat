#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Phase 2 contract tests for ops lifecycle persistence and recovery.
//!
//! TDD RED: these tests define the persistence contract. Implementation
//! in P2-B through P2-E makes them pass.

use std::sync::Arc;

use meerkat_core::completion_feed::CompletionFeed;
use meerkat_core::ops_lifecycle::{
    OperationKind, OperationResult, OperationSpec, OpsLifecycleRegistry,
};
use meerkat_core::runtime_epoch::{EpochCursorState, RuntimeEpochId};
use meerkat_core::types::SessionId;
use meerkat_runtime::{PersistedOpsSnapshot, RuntimeOpsLifecycleRegistry};

fn bg_spec(name: &str) -> OperationSpec {
    OperationSpec {
        id: meerkat_core::ops_lifecycle::OperationId::new(),
        kind: OperationKind::BackgroundToolOp,
        owner_session_id: SessionId::new(),
        display_name: name.into(),
        source_label: "test-persistence".into(),
        child_session_id: None,
        expect_peer_channel: false,
    }
}

fn op_result(id: &meerkat_core::ops_lifecycle::OperationId) -> OperationResult {
    OperationResult {
        id: id.clone(),
        content: "done".into(),
        is_error: false,
        duration_ms: 42,
        tokens_used: 0,
    }
}

// ─── Serde round-trip ───

#[test]
fn persisted_ops_snapshot_serde_round_trip() {
    let registry = RuntimeOpsLifecycleRegistry::new();
    let cursor_state = Arc::new(EpochCursorState::new());
    let epoch_id = RuntimeEpochId::new();

    // Register and complete an operation
    let spec = bg_spec("serde-roundtrip");
    let op_id = spec.id.clone();
    registry.register_operation(spec).unwrap();
    registry.provisioning_succeeded(&op_id).unwrap();
    registry
        .complete_operation(&op_id, op_result(&op_id))
        .unwrap();

    let snapshot = registry.capture_persistence_snapshot(epoch_id.clone(), &cursor_state);

    // Serialize and deserialize
    let json = serde_json::to_string(&snapshot).expect("serialize");
    let restored: PersistedOpsSnapshot = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(restored.epoch_id, epoch_id);
    assert_eq!(restored.completion_entries.len(), 1);
    assert_eq!(restored.completion_entries[0].operation_id, op_id);
    assert_eq!(restored.operation_specs.len(), 1);
    assert!(restored.operation_specs.contains_key(&op_id));
}

#[test]
fn persisted_ops_snapshot_preserves_cursor_values() {
    let registry = RuntimeOpsLifecycleRegistry::new();
    let cursor_state = Arc::new(EpochCursorState::from_recovered(5, 3, 2));

    let snapshot = registry.capture_persistence_snapshot(RuntimeEpochId::new(), &cursor_state);

    assert_eq!(snapshot.cursors.agent_applied_cursor, 5);
    assert_eq!(snapshot.cursors.runtime_observed_seq, 3);
    assert_eq!(snapshot.cursors.runtime_last_injected_seq, 2);
}

// ─── Recovery — feed visibility ───

#[test]
fn recovered_registry_contains_terminal_completion_entries() {
    let registry = RuntimeOpsLifecycleRegistry::new();
    let cursor_state = Arc::new(EpochCursorState::new());

    // Register 3 ops, complete 2
    let specs: Vec<_> = (0..3).map(|i| bg_spec(&format!("op-{i}"))).collect();
    for spec in &specs {
        registry.register_operation(spec.clone()).unwrap();
        registry.provisioning_succeeded(&spec.id).unwrap();
    }
    registry
        .complete_operation(&specs[0].id, op_result(&specs[0].id))
        .unwrap();
    registry
        .complete_operation(&specs[1].id, op_result(&specs[1].id))
        .unwrap();
    // specs[2] stays in-progress (non-terminal)

    let snapshot = registry.capture_persistence_snapshot(RuntimeEpochId::new(), &cursor_state);

    // Recover
    let recovered = RuntimeOpsLifecycleRegistry::from_recovered(snapshot);
    let feed = recovered
        .completion_feed()
        .expect("recovered registry should have feed");

    // Feed should contain the 2 terminal entries
    let batch = feed.list_since(0);
    assert_eq!(
        batch.entries.len(),
        2,
        "recovered feed should contain 2 terminal entries, got {}",
        batch.entries.len()
    );
}

#[test]
fn recovered_registry_strips_non_terminal_ops() {
    let registry = RuntimeOpsLifecycleRegistry::new();
    let cursor_state = Arc::new(EpochCursorState::new());

    let terminal_spec = bg_spec("terminal");
    let nonterminal_spec = bg_spec("in-progress");

    registry.register_operation(terminal_spec.clone()).unwrap();
    registry.provisioning_succeeded(&terminal_spec.id).unwrap();
    registry
        .complete_operation(&terminal_spec.id, op_result(&terminal_spec.id))
        .unwrap();

    registry
        .register_operation(nonterminal_spec.clone())
        .unwrap();
    registry
        .provisioning_succeeded(&nonterminal_spec.id)
        .unwrap();

    let snapshot = registry.capture_persistence_snapshot(RuntimeEpochId::new(), &cursor_state);
    let recovered = RuntimeOpsLifecycleRegistry::from_recovered(snapshot);

    // Terminal op present
    assert!(
        recovered.snapshot(&terminal_spec.id).is_some(),
        "terminal op should survive recovery"
    );
    // Non-terminal op stripped
    assert!(
        recovered.snapshot(&nonterminal_spec.id).is_none(),
        "non-terminal op should NOT survive recovery"
    );
}

#[test]
fn recovered_feed_list_since_persisted_cursor_returns_unsurfaced() {
    let registry = RuntimeOpsLifecycleRegistry::new();
    // Simulate: agent has seen up to cursor=0, two completions arrived
    let cursor_state = Arc::new(EpochCursorState::new()); // all zeros

    let spec_a = bg_spec("unsurfaced-a");
    let spec_b = bg_spec("unsurfaced-b");
    registry.register_operation(spec_a.clone()).unwrap();
    registry.provisioning_succeeded(&spec_a.id).unwrap();
    registry
        .complete_operation(&spec_a.id, op_result(&spec_a.id))
        .unwrap();

    registry.register_operation(spec_b.clone()).unwrap();
    registry.provisioning_succeeded(&spec_b.id).unwrap();
    registry
        .complete_operation(&spec_b.id, op_result(&spec_b.id))
        .unwrap();

    let snapshot = registry.capture_persistence_snapshot(RuntimeEpochId::new(), &cursor_state);

    // Recover
    let recovered = RuntimeOpsLifecycleRegistry::from_recovered(snapshot);
    let feed = recovered
        .completion_feed()
        .expect("recovered registry should have feed");

    // list_since(0) should return both entries (cursor was 0, entries are seq 1 and 2)
    let batch = feed.list_since(0);
    assert_eq!(
        batch.entries.len(),
        2,
        "list_since(0) should return 2 unsurfaced entries"
    );

    // list_since(watermark) should return nothing
    let batch_at_watermark = feed.list_since(batch.watermark);
    assert!(
        batch_at_watermark.entries.is_empty(),
        "list_since(watermark) should return nothing"
    );
}

#[test]
fn recovered_registry_clears_wait_state() {
    let registry = RuntimeOpsLifecycleRegistry::new();
    let cursor_state = Arc::new(EpochCursorState::new());

    let spec = bg_spec("wait-clear");
    registry.register_operation(spec.clone()).unwrap();
    registry.provisioning_succeeded(&spec.id).unwrap();
    registry
        .complete_operation(&spec.id, op_result(&spec.id))
        .unwrap();

    let snapshot = registry.capture_persistence_snapshot(RuntimeEpochId::new(), &cursor_state);
    let recovered = RuntimeOpsLifecycleRegistry::from_recovered(snapshot);

    // Should be usable — no panic from stale wait state
    let new_spec = bg_spec("post-recovery-op");
    let result = recovered.register_operation(new_spec);
    assert!(
        result.is_ok(),
        "recovered registry should accept new operations"
    );
}

#[test]
fn recovered_epoch_id_matches_persisted() {
    let registry = RuntimeOpsLifecycleRegistry::new();
    let cursor_state = Arc::new(EpochCursorState::new());
    let epoch_id = RuntimeEpochId::new();

    let snapshot = registry.capture_persistence_snapshot(epoch_id.clone(), &cursor_state);
    assert_eq!(snapshot.epoch_id, epoch_id, "persisted epoch_id must match");
}

#[test]
fn recovered_feed_watermark_matches_persisted_sequence() {
    let registry = RuntimeOpsLifecycleRegistry::new();
    let cursor_state = Arc::new(EpochCursorState::new());

    // Complete 3 ops to advance the sequence
    for i in 0..3 {
        let spec = bg_spec(&format!("watermark-{i}"));
        let id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&id).unwrap();
        registry.complete_operation(&id, op_result(&id)).unwrap();
    }

    let snapshot = registry.capture_persistence_snapshot(RuntimeEpochId::new(), &cursor_state);
    let recovered = RuntimeOpsLifecycleRegistry::from_recovered(snapshot);
    let feed = recovered.completion_feed().unwrap();

    // Watermark should be at least 3 (3 completions)
    assert!(
        feed.watermark() >= 3,
        "recovered feed watermark ({}) should be >= 3",
        feed.watermark()
    );
}

// ─── Snapshot captures all feed entries, not just authority-retained ops ───

#[test]
fn snapshot_captures_entries_beyond_authority_retention() {
    // Use a registry with max_completed=2 so authority evicts early
    let registry = meerkat_runtime::RuntimeOpsLifecycleRegistry::with_config(
        meerkat_runtime::OpsLifecycleConfig {
            max_completed: 2,
            max_concurrent: None,
        },
    );
    let cursor_state = Arc::new(EpochCursorState::new());

    // Complete 4 ops — authority keeps last 2, but feed buffer keeps all 4
    for i in 0..4 {
        let spec = bg_spec(&format!("evict-{i}"));
        let id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&id).unwrap();
        registry.complete_operation(&id, op_result(&id)).unwrap();
    }

    let snapshot = registry.capture_persistence_snapshot(RuntimeEpochId::new(), &cursor_state);

    // Snapshot should contain all 4 completion entries (from feed buffer),
    // even though authority only retains 2 terminal ops
    assert_eq!(
        snapshot.completion_entries.len(),
        4,
        "snapshot should capture all feed entries ({} found), not just authority-retained",
        snapshot.completion_entries.len()
    );

    // Authority state should only have 2 ops (eviction)
    assert_eq!(
        snapshot.authority_state.operation_count(),
        2,
        "authority should retain max_completed=2 ops"
    );
}

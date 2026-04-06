#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Contract tests for RuntimeSessionAdapter::prepare_bindings() and
//! SessionRuntimeBindings identity semantics.

use meerkat_core::ops_lifecycle::{
    OperationKind, OperationResult, OperationSpec, OpsLifecycleRegistry,
};
use meerkat_core::types::SessionId;
use meerkat_runtime::RuntimeSessionAdapter;
use std::sync::Arc;

fn test_op_spec(name: &str) -> OperationSpec {
    OperationSpec {
        id: meerkat_core::ops_lifecycle::OperationId::new(),
        kind: OperationKind::BackgroundToolOp,
        owner_session_id: SessionId::new(),
        display_name: name.into(),
        source_label: "test-bindings".into(),
        child_session_id: None,
        expect_peer_channel: false,
    }
}

// ─── Identity: prepare_bindings returns matching session_id ───

#[tokio::test]
async fn prepare_bindings_returns_matching_session_id() {
    let adapter = Arc::new(RuntimeSessionAdapter::ephemeral());
    let session_id = SessionId::new();

    let bindings = adapter.prepare_bindings(session_id.clone()).await.unwrap();

    assert_eq!(
        bindings.session_id, session_id,
        "bindings.session_id must match the requested session_id"
    );
}

// ─── Identity: same epoch_id for same session on repeated calls ───

#[tokio::test]
async fn prepare_bindings_returns_stable_epoch_id() {
    let adapter = Arc::new(RuntimeSessionAdapter::ephemeral());
    let session_id = SessionId::new();

    let first = adapter.prepare_bindings(session_id.clone()).await.unwrap();
    let second = adapter.prepare_bindings(session_id.clone()).await.unwrap();

    assert_eq!(
        first.epoch_id, second.epoch_id,
        "repeated prepare_bindings for the same session must return the same epoch_id"
    );
}

// ─── Identity: different epoch_id for different sessions ───

#[tokio::test]
async fn prepare_bindings_returns_different_epoch_for_different_sessions() {
    let adapter = Arc::new(RuntimeSessionAdapter::ephemeral());
    let session_a = SessionId::new();
    let session_b = SessionId::new();

    let bindings_a = adapter.prepare_bindings(session_a).await.unwrap();
    let bindings_b = adapter.prepare_bindings(session_b).await.unwrap();

    assert_ne!(
        bindings_a.epoch_id, bindings_b.epoch_id,
        "different sessions must get different epoch_ids"
    );
}

// ─── Identity: same registry instance across calls ───

#[tokio::test]
async fn prepare_bindings_returns_same_registry_instance() {
    let adapter = Arc::new(RuntimeSessionAdapter::ephemeral());
    let session_id = SessionId::new();

    let first = adapter.prepare_bindings(session_id.clone()).await.unwrap();

    // Register an operation through the first binding's registry
    let spec = test_op_spec("identity-check");
    let op_id = spec.id.clone();
    first.ops_lifecycle.register_operation(spec).unwrap();

    // Get bindings again — should see the same operation
    let second = adapter.prepare_bindings(session_id.clone()).await.unwrap();
    let snapshot = second.ops_lifecycle.snapshot(&op_id);

    assert!(
        snapshot.is_some(),
        "second prepare_bindings must share the same registry as the first"
    );
}

// ─── Identity: prepare_bindings is idempotent with register_session ───

#[tokio::test]
async fn prepare_bindings_idempotent_with_prior_registration() {
    let adapter = Arc::new(RuntimeSessionAdapter::ephemeral());
    let session_id = SessionId::new();

    // Pre-register (as some surfaces do)
    adapter.register_session(session_id.clone()).await;

    // prepare_bindings should reuse the existing entry
    let bindings = adapter.prepare_bindings(session_id.clone()).await.unwrap();
    assert_eq!(bindings.session_id, session_id);

    // Register an op through prepare_bindings registry
    let spec = test_op_spec("idempotent-check");
    let op_id = spec.id.clone();
    bindings.ops_lifecycle.register_operation(spec).unwrap();

    // The direct ops_lifecycle_registry should see it too
    let direct_registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("session should be registered");

    assert!(
        direct_registry.snapshot(&op_id).is_some(),
        "prepare_bindings registry must be the same instance as ops_lifecycle_registry"
    );
}

// ─── Completion feed: bindings registry and runtime loop see the same feed ───

#[tokio::test]
async fn bindings_registry_shares_completion_feed_with_adapter() {
    let adapter = Arc::new(RuntimeSessionAdapter::ephemeral());
    let session_id = SessionId::new();

    let bindings = adapter.prepare_bindings(session_id.clone()).await.unwrap();

    // Get completion feed from bindings registry
    let feed = bindings
        .ops_lifecycle
        .completion_feed()
        .expect("runtime registry should have a completion feed");

    // Register and complete an operation
    let spec = test_op_spec("feed-check");
    let op_id = spec.id.clone();
    bindings.ops_lifecycle.register_operation(spec).unwrap();
    bindings
        .ops_lifecycle
        .provisioning_succeeded(&op_id)
        .unwrap();
    bindings
        .ops_lifecycle
        .complete_operation(
            &op_id,
            OperationResult {
                id: op_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .unwrap();

    // Feed from the same registry should see the completion
    let batch = feed.list_since(0);
    assert!(
        !batch.entries.is_empty(),
        "completion feed must contain the terminal entry"
    );
    assert_eq!(batch.entries[0].operation_id, op_id);

    // Feed obtained via the adapter's ops_lifecycle_registry should also see it
    let adapter_registry = adapter.ops_lifecycle_registry(&session_id).await.unwrap();
    let adapter_feed = adapter_registry.completion_feed_handle();
    let adapter_batch = adapter_feed.list_since(0);
    assert_eq!(
        adapter_batch.entries.len(),
        batch.entries.len(),
        "adapter's feed and bindings' feed must be the same"
    );
}

// ─── Phase 3: runtime_build_mode is required (non-Option) ───

#[test]
fn session_build_options_default_has_standalone_ephemeral() {
    let opts = meerkat_core::service::SessionBuildOptions::default();
    assert!(
        matches!(
            opts.runtime_build_mode,
            meerkat_core::RuntimeBuildMode::StandaloneEphemeral
        ),
        "Default SessionBuildOptions must have runtime_build_mode = StandaloneEphemeral"
    );
}

#[test]
fn session_build_options_debug_shows_build_mode() {
    let opts = meerkat_core::service::SessionBuildOptions::default();
    let debug = format!("{opts:?}");
    assert!(
        debug.contains("StandaloneEphemeral"),
        "Debug output must show the build mode variant"
    );
}

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Phase 1 red-ok target-definition tests for async operation lifecycle contracts.

use meerkat_core::ops_lifecycle::{
    OperationCompletionWatch, OperationKind, OperationTerminalOutcome,
};

#[tokio::test]
async fn ops_lifecycle_integration_red_ok_completion_watch_surfaces_terminal_outcome() {
    let (tx, watch) = OperationCompletionWatch::channel();
    tx.send(OperationTerminalOutcome::Retired)
        .expect("watch channel should accept a terminal outcome");

    assert_eq!(watch.wait().await, OperationTerminalOutcome::Retired);
}

#[test]
fn ops_lifecycle_integration_red_ok_operation_kinds_define_peer_expectations() {
    assert!(
        OperationKind::MobMemberChild.expects_peer_channel(),
        "mob helper-agent flows should require a peer-ready handoff"
    );
    assert!(
        !OperationKind::BackgroundToolOp.expects_peer_channel(),
        "background async operations should not require a peer-ready handoff"
    );
}

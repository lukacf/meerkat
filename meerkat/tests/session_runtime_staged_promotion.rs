//! Smoke test for `meerkat::session_runtime::staged_promotion`.
//!
//! Wider coverage of `PendingPromotionCleanup` lives in
//! `meerkat-rpc`'s integration tests because constructing a real
//! `PromotingSlot` + `StagedSessionRegistry` requires the runtime
//! plumbing the rpc crate already provides. Here we just assert the
//! `Mode` enum's discriminants compile and Copy/PartialEq hold.

#![cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use meerkat::session_runtime::staged_promotion::{
    PendingPromotionCleanupMode, StagedTaskJoinError, await_service_apply_runtime_turn,
    await_service_apply_runtime_turn_with_recoverable_admission,
};
use meerkat_core::types::SessionId;

#[test]
fn cleanup_mode_variants_are_distinct_and_copy() {
    let restore = PendingPromotionCleanupMode::Restore;
    let finish = PendingPromotionCleanupMode::Finish;
    assert_ne!(restore, finish);

    let restore_copy = restore;
    let finish_copy = finish;
    assert_eq!(restore, restore_copy);
    assert_eq!(finish, finish_copy);
}

/// Regression: when the spawned task vanishes before reporting a
/// result, the awaiter must surface a typed [`StagedTaskJoinError`]
/// carrying the original session id so surfaces can map it to their
/// wire shape (`RpcError`, `axum::Response`, etc.) without losing the
/// session identity. Closes the receiver explicitly to mimic the
/// "spawned task dropped" scenario.
#[tokio::test]
async fn await_service_apply_runtime_turn_returns_typed_join_error() {
    let session_id = SessionId::new();
    let (tx, rx) = tokio::sync::oneshot::channel::<
        Result<
            meerkat_core::lifecycle::core_executor::CoreApplyOutput,
            meerkat_core::service::SessionError,
        >,
    >();
    drop(tx);
    let err: StagedTaskJoinError = match await_service_apply_runtime_turn(&session_id, rx).await {
        Ok(_) => panic!("expected join error after sender drop"),
        Err(err) => err,
    };
    assert_eq!(err.session_id, session_id);
    let formatted = err.to_string();
    let id_str = session_id.to_string();
    assert!(formatted.contains(id_str.as_str()), "{formatted}");
}

/// Mirror test for the recoverable variant: the typed
/// [`StagedTaskJoinError`] must carry the session id even when the
/// recoverable channel includes an `Option<ActiveCapacityGuard>` slot.
#[tokio::test]
async fn await_service_apply_runtime_turn_with_recoverable_admission_returns_typed_join_error() {
    let session_id = SessionId::new();
    let (tx, rx) = tokio::sync::oneshot::channel::<(
        Result<
            meerkat_core::lifecycle::core_executor::CoreApplyOutput,
            meerkat_core::service::SessionError,
        >,
        Option<meerkat::session_runtime::admission::ActiveCapacityGuard>,
    )>();
    drop(tx);
    let err =
        match await_service_apply_runtime_turn_with_recoverable_admission(&session_id, rx).await {
            Ok(_) => panic!("expected join error after sender drop"),
            Err(err) => err,
        };
    assert_eq!(err.session_id, session_id);
}

//! An owned rollback never holds the turn-finalization boundary (B) while it
//! waits for the session's live lifecycle lease. A concurrent raw unregister
//! takes that lease first and keeps it into its surface cleanup, which takes
//! B: a rollback that still held B there would close the cycle.

use super::*;
use crate::runtime::host_materialize::{MaterializeServeError, published_actor_rollback_refusal};
use crate::runtime::provisioner::{
    PreparedServiceActorTransaction, RuntimeTurnFinalizationBoundaryLease,
    arm_owned_actor_cleanup_released_boundary_pause_for_test,
    arm_owned_actor_cleanup_rollback_pause_for_test, install_prepared_mob_session_executor_handles,
};

/// Bound for every wait below: a deadlock never resolves, while a correct run
/// takes milliseconds.
const DEADLOCK_BOUND: Duration = Duration::from_secs(10);

fn member_create_request(session_id: &SessionId) -> CreateSessionRequest {
    CreateSessionRequest {
        injected_context: Vec::new(),
        model: "claude-sonnet-4-5".to_string(),
        prompt: "member".to_string().into(),
        system_prompt: meerkat_core::SystemPromptOverride::Inherit,
        max_tokens: None,
        event_tx: None,
        build: Some(meerkat_core::service::SessionBuildOptions {
            resume_session: Some(Session::with_id(session_id.clone())),
            ..Default::default()
        }),
        initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
        deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
        labels: None,
    }
}

/// One provisioning transaction for a fresh session: B, a registration its
/// own preparation created, its provisional executor handles, and the exact
/// actor witness its create published. B is a real exclusive lock, so the
/// unregister saga's surface cleanup contends on it.
async fn prepared_actor_transaction() -> (
    std::sync::Arc<meerkat_runtime::MeerkatMachine>,
    SessionId,
    PreparedServiceActorTransaction,
) {
    let service = std::sync::Arc::new(MockSessionService::new());
    let _ = service.install_non_reentrant_turn_finalization_gate();
    let session_service: std::sync::Arc<dyn MobSessionService> = service;
    let machine = std::sync::Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let boundary = RuntimeTurnFinalizationBoundaryLease::acquire(&session_service, &session_id)
        .await
        .expect("turn-finalization boundary");
    let prepared = machine
        .prepare_session_materialization(session_id.clone())
        .await
        .expect("prepared materialization");
    let actor_witness_slot = meerkat_session::LiveSessionActorWitnessSlot::default();
    install_prepared_mob_session_executor_handles(
        std::sync::Arc::clone(&session_service),
        std::sync::Arc::clone(&machine),
        &prepared,
        actor_witness_slot.clone(),
    )
    .await
    .expect("provisional executor handles");
    session_service
        .create_session_with_actor_witness_under_runtime_turn_boundary(
            member_create_request(&session_id),
            None,
            &actor_witness_slot,
        )
        .await
        .expect("member actor");
    assert!(
        actor_witness_slot.witness().is_some(),
        "the create published its exact actor witness"
    );
    let transaction = PreparedServiceActorTransaction::new(
        session_id.clone(),
        session_service,
        prepared,
        boundary,
        actor_witness_slot,
    )
    .expect("actor transaction");
    (machine, session_id, transaction)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn owned_actor_cleanup_rolls_back_without_deadlocking_a_raw_unregister_saga() {
    let (machine, session_id, transaction) = prepared_actor_transaction().await;

    // Abort it: its cleanup discards the actor under B, then pauses there.
    let (cleanup_paused, release_cleanup) =
        arm_owned_actor_cleanup_rollback_pause_for_test(session_id.clone());
    let cleanup = tokio::spawn(transaction.abort());
    tokio::time::timeout(DEADLOCK_BOUND, cleanup_paused)
        .await
        .expect("the owned cleanup reached its pause with B held")
        .expect("pause signal");

    // A raw unregister of the same session: its saga takes the live
    // lifecycle lease, then its surface cleanup waits for B.
    machine.test_set_unregister_caller_wait_grace(Duration::from_mins(1));
    let saga_holds_lease = machine.test_signal_next_unregister_saga_live_lease();
    let unregister = tokio::spawn({
        let machine = std::sync::Arc::clone(&machine);
        let session_id = session_id.clone();
        async move { machine.unregister_session(&session_id).await }
    });
    tokio::time::timeout(DEADLOCK_BOUND, saga_holds_lease)
        .await
        .expect("the raw unregister saga took the live lifecycle lease")
        .expect("lease signal");

    // Resume the rollback while the saga holds the lease and waits for B.
    let _ = release_cleanup.send(());
    tokio::time::timeout(DEADLOCK_BOUND, cleanup)
        .await
        .expect("the rollback finished: it must not wait for the saga's lease while holding B")
        .expect("cleanup task")
        .expect("exact owned cleanup");
    tokio::time::timeout(DEADLOCK_BOUND, unregister)
        .await
        .expect("the raw unregister saga finished")
        .expect("unregister task")
        .expect("raw unregister");
    assert!(
        !machine.contains_session(&session_id).await,
        "the prepared registration is gone"
    );
}

/// Between the cleanup's boundary release and its rollback, a raw unregister
/// may remove the prepared registration and the same session id may be
/// registered again. That successor is a new registration with its own
/// claim: the stale rollback has no effect on it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_re_registration_in_the_released_boundary_window_is_left_alone() {
    let (machine, session_id, transaction) = prepared_actor_transaction().await;

    // Abort it: its cleanup discards the actor under B, releases B, then
    // pauses before its rollback.
    let (boundary_released, resume_cleanup) =
        arm_owned_actor_cleanup_released_boundary_pause_for_test(session_id.clone());
    let cleanup = tokio::spawn(transaction.abort());
    tokio::time::timeout(DEADLOCK_BOUND, boundary_released)
        .await
        .expect("the owned cleanup released B and paused before its rollback")
        .expect("pause signal");

    // B is free, so a raw unregister of the session completes; the same
    // session id is then registered again.
    tokio::time::timeout(DEADLOCK_BOUND, machine.unregister_session(&session_id))
        .await
        .expect("the raw unregister finished")
        .expect("raw unregister");
    assert!(!machine.contains_session(&session_id).await);
    let mut successor = machine
        .prepare_session_materialization(session_id.clone())
        .await
        .expect("successor registration");

    // The paused rollback runs against the successor's registration.
    let _ = resume_cleanup.send(());
    tokio::time::timeout(DEADLOCK_BOUND, cleanup)
        .await
        .expect("the stale rollback finished")
        .expect("cleanup task")
        .expect("a rollback of a claim that is no longer current has no effect");
    assert!(
        machine.contains_session(&session_id).await,
        "the successor registration is untouched"
    );
    assert!(
        successor.rollback_now().await.expect("successor rollback"),
        "the successor still owns its current claim"
    );
    assert!(!machine.contains_session(&session_id).await);
}

/// The release-build check behind the boundary-held rollbacks in
/// `host_materialize`: an empty witness slot lets the rollback run, and a
/// published actor turns it into a typed refusal that fail-stops the serving
/// actor. Those rollbacks all run before their own create publishes into
/// their fresh slot, so no existing hook reaches the refusal there.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_boundary_held_rollback_after_actor_publication_is_refused_with_a_typed_error() {
    let empty = meerkat_session::LiveSessionActorWitnessSlot::default();
    assert!(
        published_actor_rollback_refusal(&SessionId::new(), &empty, "install failed").is_none(),
        "an empty slot keeps the boundary-held rollback"
    );

    let (_machine, session_id, transaction) = prepared_actor_transaction().await;
    let refusal = published_actor_rollback_refusal(
        &session_id,
        transaction.actor_witness_slot(),
        "install failed",
    )
    .expect("a published actor refuses the boundary-held rollback");
    assert!(refusal.requires_actor_fail_stop(), "{refusal}");
    assert!(
        matches!(
            &refusal,
            MaterializeServeError::UnrecordedSessionCleanup {
                session_id: refused,
                original,
                ..
            } if *refused == session_id.to_string() && original == "install failed"
        ),
        "{refusal:?}"
    );
    tokio::time::timeout(DEADLOCK_BOUND, transaction.abort())
        .await
        .expect("the owned cleanup finished")
        .expect("exact owned cleanup");
}

//! The application's exact close-state implementation against retained Core
//! calls. Direct routers here qualify cleanup only; the installed reset runner
//! separately exercises Driver's real gateway factory callback and Python owner.
use super::*;
use crate::application_driver::local_round::LocalRoundCleanup;

async fn exercise(
    native: &str,
    body: impl for<'a> FnOnce(&'a mut NativeCase, &'a mut LocalRoundCleanup)
        -> Pin<Box<dyn Future<Output = ()> + 'a>>,
) {
    let mut case = NativeCase::new("ready", native, false).await;
    let mut state = LocalRoundCleanup::default();
    let result = AssertUnwindSafe(body(&mut case, &mut state)).catch_unwind().await;
    // Return only the exact owner the application cleanup recovered, including
    // its retained Child on timeout. The existing all-exit fixture joins it.
    if let Some(owner) = state.take_owner_for_test() {
        case.protocol.owner = Some(owner);
    }
    let cleanup = case.cleanup().await;
    assert!(cleanup.is_empty(), "cleanup failures: {cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

#[tokio::test]
async fn gateway_close_keeps_held_offer_then_recovers_exact_owner() {
    exercise("normal", |case, state| Box::pin(async move {
        let session = case.fixture.register().await;
        let first = case.route(&session);
        case.add(first, ExternalToolDeltaPhase::Applied).await;
        let closed = case.close(first).await;
        case.assert_settled(closed);
        let next = case.route(&session);
        case.add(next, ExternalToolDeltaPhase::Applied).await;
        assert_eq!(case.native_pids().len(), 2);
        assert_eq!(case.protocol.pids().len(), 1);
        assert!(state.join(&case.attempts, &case.guards,
            tokio::time::Instant::now() + Duration::from_millis(20)).await.is_err());
        let premature = state.take_owner_for_test();
        let returned_early = premature.is_some();
        if let Some(owner) = premature { case.protocol.owner = Some(owner); }
        assert!(!returned_early, "held native guard still owns the offer");
        let attempts = case.attempts.clone();
        let guards = case.guards.clone();
        let mut joined = Box::pin(state.join_until_settled(&attempts, &guards));
        assert!(tokio::time::timeout(Duration::from_millis(20), &mut joined).await.is_err(),
            "the retained host join cannot return while its process is held");
        let closed = case.close(next).await;
        case.assert_settled(closed);
        tokio::time::timeout(BOUND, &mut joined).await
            .expect("same retained join completes after native process close")
            .expect("same guard calls and local owner settle after native close");
        drop(joined);
        state.join(&case.attempts, &case.guards, tokio::time::Instant::now() + BOUND)
            .await.expect("joined close replays without taking another owner");
        case.protocol.owner = state.take_owner_for_test();
        let owner = case.protocol.owner.as_ref().unwrap();
        assert_eq!(owner.resources.root, case.protocol.selected_root);
        assert!(!owner.has_pending_round());
    })).await;
}

#[tokio::test]
async fn gateway_close_retains_failed_finish_after_core_forgets_record() {
    exercise("abnormal_exit", |case, state| Box::pin(async move {
        let session = case.fixture.register().await;
        let route = case.route(&session);
        case.add(route, ExternalToolDeltaPhase::Applied).await;
        case.close(route).await;
        assert!(state.join(&case.attempts, &case.guards,
            tokio::time::Instant::now() + BOUND).await.is_err());
        assert!(case.guards.close_and_join(tokio::time::Instant::now() + BOUND)
            .await.is_empty(), "ended guard records were already consumed");
        assert!(state.join(&case.attempts, &case.guards,
            tokio::time::Instant::now() + BOUND).await.is_err(),
            "empty replay cannot erase an actual failed finish");
        case.protocol.owner = state.take_owner_for_test();
        let owner = case.protocol.owner.as_ref().unwrap();
        assert_eq!(owner.resources.root, case.protocol.selected_root);
        assert!(owner.poisoned && !owner.has_pending_round());
    })).await;
}

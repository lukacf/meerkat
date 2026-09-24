//! T7 — in-process refresh dedup (Phase 4a).
//!
//! RCT Mini choke-point: given N concurrent `with_refresh` calls for the
//! same `TokenKey`, exactly one underlying refresh function fires. The
//! test asserts this via an `AtomicUsize` counter.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures::FutureExt;
use meerkat_auth_core::auth_store::{
    CredentialMutationError, CredentialMutationOutcome, InMemoryCoordinator, PersistedTokens,
    RefreshCoordinator, RefreshError, TokenKey,
};

fn fresh_tokens() -> PersistedTokens {
    PersistedTokens::api_key("refreshed-secret")
}

#[tokio::test]
async fn concurrent_resolves_for_same_key_trigger_one_refresh() {
    let coord = Arc::new(InMemoryCoordinator::new());
    let counter = Arc::new(AtomicUsize::new(0));
    let key = TokenKey::parse("dev", "default_openai").expect("valid slugs");

    let mut handles = Vec::new();
    for _ in 0..8 {
        let coord = Arc::clone(&coord);
        let counter = Arc::clone(&counter);
        let key = key.clone();
        handles.push(tokio::spawn(async move {
            coord
                .with_refresh(
                    key,
                    Box::new(move || {
                        async move {
                            counter.fetch_add(1, Ordering::SeqCst);
                            tokio::time::sleep(Duration::from_millis(20)).await;
                            Ok::<_, RefreshError>(fresh_tokens())
                        }
                        .boxed()
                    }),
                )
                .await
        }));
    }

    for h in handles {
        let tokens = h.await.unwrap().unwrap();
        assert_eq!(tokens.primary_secret.as_deref(), Some("refreshed-secret"));
    }

    let fired = counter.load(Ordering::SeqCst);
    assert_eq!(
        fired, 1,
        "expected exactly 1 underlying refresh, observed {fired}",
    );
}

#[tokio::test]
async fn sequential_refreshes_are_not_short_circuited() {
    // After a refresh terminates, the next call with the same key must
    // re-run the refresh (no terminal caching in the coordinator itself).
    let coord = InMemoryCoordinator::new();
    let counter = Arc::new(AtomicUsize::new(0));
    let key = TokenKey::parse("dev", "x").expect("valid slugs");

    for _ in 0..3 {
        let counter = Arc::clone(&counter);
        coord
            .with_refresh(
                key.clone(),
                Box::new(move || {
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        Ok::<_, RefreshError>(fresh_tokens())
                    }
                    .boxed()
                }),
            )
            .await
            .unwrap();
    }

    assert_eq!(counter.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn refresh_error_is_observed_by_all_waiters() {
    // If the single underlying refresh fails, every concurrent waiter sees
    // the same error (shared future result).
    let coord = Arc::new(InMemoryCoordinator::new());
    let key = TokenKey::parse("dev", "x").expect("valid slugs");

    let mut handles = Vec::new();
    for _ in 0..4 {
        let coord = Arc::clone(&coord);
        let key = key.clone();
        handles.push(tokio::spawn(async move {
            coord
                .with_refresh(
                    key,
                    Box::new(move || {
                        async move {
                            tokio::time::sleep(Duration::from_millis(10)).await;
                            Err::<PersistedTokens, _>(RefreshError::Refresh("upstream 401".into()))
                        }
                        .boxed()
                    }),
                )
                .await
        }));
    }

    for h in handles {
        let res = h.await.unwrap();
        match res {
            Err(RefreshError::Refresh(msg)) => assert_eq!(msg, "upstream 401"),
            other => panic!("expected Refresh error, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn refreshes_for_distinct_keys_run_in_parallel() {
    let coord = InMemoryCoordinator::new();
    let counter_a = Arc::new(AtomicUsize::new(0));
    let counter_b = Arc::new(AtomicUsize::new(0));

    let ca = Arc::clone(&counter_a);
    let cb = Arc::clone(&counter_b);
    let (res_a, res_b) = tokio::join!(
        coord.with_refresh(
            TokenKey::parse("dev", "a").expect("valid slugs"),
            Box::new(move || {
                async move {
                    ca.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, RefreshError>(fresh_tokens())
                }
                .boxed()
            }),
        ),
        coord.with_refresh(
            TokenKey::parse("dev", "b").expect("valid slugs"),
            Box::new(move || {
                async move {
                    cb.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, RefreshError>(fresh_tokens())
                }
                .boxed()
            }),
        ),
    );
    res_a.unwrap();
    res_b.unwrap();
    assert_eq!(counter_a.load(Ordering::SeqCst), 1);
    assert_eq!(counter_b.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn exclusive_mutations_serialize_without_coalescing() {
    let coord = Arc::new(InMemoryCoordinator::new());
    let key = TokenKey::parse("dev", "exclusive").expect("valid slugs");
    let calls = Arc::new(AtomicUsize::new(0));
    let in_flight = Arc::new(AtomicUsize::new(0));
    let max_in_flight = Arc::new(AtomicUsize::new(0));

    let mut tasks = Vec::new();
    for index in 0..2 {
        let coord = Arc::clone(&coord);
        let key = key.clone();
        let calls = Arc::clone(&calls);
        let in_flight = Arc::clone(&in_flight);
        let max_in_flight = Arc::clone(&max_in_flight);
        tasks.push(tokio::spawn(async move {
            coord
                .with_exclusive_mutation(
                    key,
                    Box::new(move || {
                        async move {
                            calls.fetch_add(1, Ordering::SeqCst);
                            let active = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                            max_in_flight.fetch_max(active, Ordering::SeqCst);
                            tokio::time::sleep(Duration::from_millis(20)).await;
                            in_flight.fetch_sub(1, Ordering::SeqCst);
                            Ok::<_, CredentialMutationError>(CredentialMutationOutcome::Persisted(
                                PersistedTokens::api_key(format!("mutation-{index}")),
                            ))
                        }
                        .boxed()
                    }),
                )
                .await
        }));
    }
    for task in tasks {
        task.await.unwrap().unwrap();
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(max_in_flight.load(Ordering::SeqCst), 1);
}

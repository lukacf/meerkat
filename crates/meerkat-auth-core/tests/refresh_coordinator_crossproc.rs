//! T8 — cross-process refresh dedup (Phase 4a).
//!
//! RCT Mini choke-point: when two independent processes race to refresh
//! the same binding, the OS-level lockfile serializes them so exactly one
//! refresh runs at a time. We prove it by spawning N child processes that
//! each read-modify-write a counter file inside the lock. If the lock is
//! broken, concurrent read-modify-write races lose updates; if intact, the
//! counter equals N.
//!
//! Child-mode entry point: the test binary re-executes itself with the
//! env var `MEERKAT_REFRESH_CROSSPROC_CHILD=1` set; in that mode the test
//! `crossproc_child_runner` performs the locked increment and exits.

#![cfg(all(not(target_arch = "wasm32"), feature = "file-lock"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures::FutureExt;
use meerkat_auth_core::auth_store::{
    CredentialMutationError, CredentialMutationOutcome, FileLockCoordinator, FileTokenStore,
    PersistedTokens, RefreshCoordinator, RefreshError, TokenKey, TokenStore, TokenStoreBackend,
};

const CHILD_ENV: &str = "MEERKAT_REFRESH_CROSSPROC_CHILD";
const COUNTER_ENV: &str = "MEERKAT_REFRESH_CROSSPROC_COUNTER";
const BACKEND_ROOT_ENV: &str = "MEERKAT_REFRESH_CROSSPROC_BACKEND_ROOT";

fn read_counter(path: &PathBuf) -> u64 {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0)
}

fn write_counter(path: &PathBuf, value: u64) {
    std::fs::write(path, value.to_string()).unwrap();
}

#[tokio::test]
async fn crossproc_child_runner() {
    // In child mode: perform a locked read-modify-write on the counter,
    // sleep to amplify contention, then exit. The test returns early
    // (green) when not in child mode so the parent test can sequence it.
    if std::env::var(CHILD_ENV).is_err() {
        return;
    }
    let counter = PathBuf::from(std::env::var(COUNTER_ENV).unwrap());
    let backend_root = PathBuf::from(std::env::var(BACKEND_ROOT_ENV).unwrap());

    let persistence = TokenStoreBackend::File { root: backend_root }
        .open_with_refresh_authority()
        .expect("file backend opens with refresh authority");
    let coord = persistence.refresh_coordinator();
    let key = TokenKey::parse("crossproc", "counter").expect("valid slugs");
    let counter_inside = counter.clone();
    let result = coord
        .with_refresh(
            key,
            Box::new(move || {
                async move {
                    let current = read_counter(&counter_inside);
                    // Delay inside the critical section to force contention.
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    write_counter(&counter_inside, current + 1);
                    Ok::<_, RefreshError>(PersistedTokens::api_key("child"))
                }
                .boxed()
            }),
        )
        .await;
    result.unwrap();
    // Child exits cleanly so its output is captured by cargo test.
}

#[tokio::test]
async fn two_processes_racing_produce_no_lost_updates() {
    if std::env::var(CHILD_ENV).is_ok() {
        return; // child re-entry — handled by crossproc_child_runner.
    }

    let temp = tempfile::tempdir().unwrap();
    let counter = temp.path().join("counter.txt");
    let backend_root = temp.path().join("credentials");
    write_counter(&counter, 0);

    let exe = std::env::current_exe().unwrap();
    let n_children: u64 = 4;

    // Spawn N child processes that each refresh through a new
    // FileLockCoordinator pointing at the same lock_dir.
    let mut handles = Vec::new();
    for i in 0..n_children {
        let exe = exe.clone();
        let counter = counter.clone();
        let backend_root = backend_root.clone();
        handles.push(tokio::task::spawn_blocking(
            move || -> std::io::Result<()> {
                let status = Command::new(&exe)
                    .arg("--exact")
                    .arg("crossproc_child_runner")
                    .arg("--nocapture")
                    .env(CHILD_ENV, "1")
                    .env(COUNTER_ENV, counter)
                    .env(BACKEND_ROOT_ENV, &backend_root)
                    .env("CHILD_INDEX", i.to_string())
                    .status()?;
                if !status.success() {
                    return Err(std::io::Error::other(format!(
                        "child {i} failed with status {status:?}"
                    )));
                }
                Ok(())
            },
        ));
    }

    for h in handles {
        h.await.unwrap().unwrap();
    }

    let final_count = read_counter(&counter);
    assert_eq!(
        final_count, n_children,
        "expected {n_children} locked increments, got {final_count} (lock failed to serialize refreshes)",
    );
}

#[tokio::test]
async fn in_process_two_coordinators_share_lock_dir() {
    // Weaker invariant, doesn't need subprocess: two separate
    // FileLockCoordinators in the same process pointing at the same
    // lock_dir serialize their refreshes. This proves the OS-level lock is
    // actually taken (not just dedup-within-coordinator).
    if std::env::var(CHILD_ENV).is_ok() {
        return;
    }

    let temp = tempfile::tempdir().unwrap();
    let lock_dir = temp.path().to_path_buf();
    let a = Arc::new(FileLockCoordinator::new(lock_dir.clone()));
    let b = Arc::new(FileLockCoordinator::new(lock_dir.clone()));
    let key = TokenKey::parse("dev", "x").expect("valid slugs");

    let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let in_flight = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let max_in_flight = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let mut handles = Vec::new();
    for coord in [a, b] {
        let key = key.clone();
        let counter = Arc::clone(&counter);
        let in_flight = Arc::clone(&in_flight);
        let max_in_flight = Arc::clone(&max_in_flight);
        handles.push(tokio::spawn(async move {
            coord
                .with_exclusive_mutation(
                    key,
                    Box::new(move || {
                        async move {
                            let cur =
                                in_flight.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                            max_in_flight.fetch_max(cur, std::sync::atomic::Ordering::SeqCst);
                            tokio::time::sleep(Duration::from_millis(30)).await;
                            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            in_flight.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                            Ok::<_, CredentialMutationError>(CredentialMutationOutcome::Persisted(
                                PersistedTokens::api_key("ok"),
                            ))
                        }
                        .boxed()
                    }),
                )
                .await
        }));
    }

    for h in handles {
        h.await.unwrap().unwrap();
    }
    assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(
        max_in_flight.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "two coordinators with the same lock_dir must serialize",
    );
}

#[tokio::test]
async fn coordinated_logout_cannot_be_followed_by_stale_refresh_save() {
    if std::env::var(CHILD_ENV).is_ok() {
        return;
    }

    let temp = tempfile::tempdir().unwrap();
    let lock_dir = temp.path().join("locks");
    let store: Arc<dyn TokenStore> = Arc::new(FileTokenStore::new(temp.path().join("credentials")));
    let refresh_coordinator = Arc::new(FileLockCoordinator::new(lock_dir.clone()));
    let logout_coordinator = Arc::new(FileLockCoordinator::new(lock_dir));
    let key = TokenKey::parse("dev", "openai").expect("valid key");
    let original = PersistedTokens::api_key("credential-a");
    let refreshed = PersistedTokens::api_key("credential-b");
    store.save(&key, &original).await.unwrap();

    let (refresh_entered_tx, refresh_entered_rx) = tokio::sync::oneshot::channel();
    let release_refresh = Arc::new(tokio::sync::Notify::new());
    let refresh_task = {
        let store = Arc::clone(&store);
        let key = key.clone();
        let refreshed = refreshed.clone();
        let release_refresh = Arc::clone(&release_refresh);
        tokio::spawn(async move {
            refresh_coordinator
                .with_refresh(
                    key.clone(),
                    Box::new(move || {
                        async move {
                            let loaded = store.load(&key).await.unwrap();
                            assert_eq!(loaded, Some(original));
                            refresh_entered_tx.send(()).unwrap();
                            release_refresh.notified().await;
                            store.save(&key, &refreshed).await.unwrap();
                            Ok::<_, RefreshError>(refreshed)
                        }
                        .boxed()
                    }),
                )
                .await
        })
    };
    refresh_entered_rx.await.unwrap();

    let (logout_attempted_tx, logout_attempted_rx) = tokio::sync::oneshot::channel();
    let logout_entered = Arc::new(AtomicBool::new(false));
    let logout_task = {
        let store = Arc::clone(&store);
        let key = key.clone();
        let logout_entered = Arc::clone(&logout_entered);
        tokio::spawn(async move {
            logout_attempted_tx.send(()).unwrap();
            logout_coordinator
                .with_exclusive_mutation(
                    key.clone(),
                    Box::new(move || {
                        async move {
                            logout_entered.store(true, Ordering::SeqCst);
                            assert_eq!(
                                store.load(&key).await.unwrap(),
                                Some(PersistedTokens::api_key("credential-b"))
                            );
                            store.clear(&key).await.unwrap();
                            Ok::<_, CredentialMutationError>(CredentialMutationOutcome::Cleared)
                        }
                        .boxed()
                    }),
                )
                .await
        })
    };
    logout_attempted_rx.await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !logout_entered.load(Ordering::SeqCst),
        "logout must wait while a stale refresh owns the shared key lock"
    );

    release_refresh.notify_one();
    assert_eq!(refresh_task.await.unwrap().unwrap(), refreshed);
    assert_eq!(
        logout_task.await.unwrap().unwrap(),
        CredentialMutationOutcome::Cleared
    );
    assert_eq!(store.load(&key).await.unwrap(), None);
}

#[tokio::test]
async fn replacement_login_cannot_be_overwritten_by_stale_refresh() {
    if std::env::var(CHILD_ENV).is_ok() {
        return;
    }

    let temp = tempfile::tempdir().unwrap();
    let lock_dir = temp.path().join("locks");
    let store: Arc<dyn TokenStore> = Arc::new(FileTokenStore::new(temp.path().join("credentials")));
    let refresh_coordinator = Arc::new(FileLockCoordinator::new(lock_dir.clone()));
    let login_coordinator = Arc::new(FileLockCoordinator::new(lock_dir));
    let key = TokenKey::parse("dev", "anthropic").expect("valid key");
    let original = PersistedTokens::api_key("credential-a");
    let refreshed = PersistedTokens::api_key("credential-b");
    let replacement = PersistedTokens::api_key("credential-c");
    store.save(&key, &original).await.unwrap();

    let (refresh_entered_tx, refresh_entered_rx) = tokio::sync::oneshot::channel();
    let release_refresh = Arc::new(tokio::sync::Notify::new());
    let refresh_task = {
        let store = Arc::clone(&store);
        let key = key.clone();
        let refreshed = refreshed.clone();
        let release_refresh = Arc::clone(&release_refresh);
        tokio::spawn(async move {
            refresh_coordinator
                .with_refresh(
                    key.clone(),
                    Box::new(move || {
                        async move {
                            let loaded = store.load(&key).await.unwrap();
                            assert_eq!(loaded, Some(original));
                            refresh_entered_tx.send(()).unwrap();
                            release_refresh.notified().await;
                            store.save(&key, &refreshed).await.unwrap();
                            Ok::<_, RefreshError>(refreshed)
                        }
                        .boxed()
                    }),
                )
                .await
        })
    };
    refresh_entered_rx.await.unwrap();

    let (login_attempted_tx, login_attempted_rx) = tokio::sync::oneshot::channel();
    let login_entered = Arc::new(AtomicBool::new(false));
    let login_task = {
        let store = Arc::clone(&store);
        let key = key.clone();
        let replacement = replacement.clone();
        let login_entered = Arc::clone(&login_entered);
        tokio::spawn(async move {
            login_attempted_tx.send(()).unwrap();
            login_coordinator
                .with_exclusive_mutation(
                    key.clone(),
                    Box::new(move || {
                        async move {
                            login_entered.store(true, Ordering::SeqCst);
                            assert_eq!(
                                store.load(&key).await.unwrap(),
                                Some(PersistedTokens::api_key("credential-b"))
                            );
                            store.save(&key, &replacement).await.unwrap();
                            Ok::<_, CredentialMutationError>(CredentialMutationOutcome::Persisted(
                                replacement,
                            ))
                        }
                        .boxed()
                    }),
                )
                .await
        })
    };
    login_attempted_rx.await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !login_entered.load(Ordering::SeqCst),
        "replacement login must wait while a stale refresh owns the shared key lock"
    );

    release_refresh.notify_one();
    assert_eq!(refresh_task.await.unwrap().unwrap(), refreshed);
    assert_eq!(
        login_task.await.unwrap().unwrap(),
        CredentialMutationOutcome::Persisted(replacement.clone())
    );
    assert_eq!(store.load(&key).await.unwrap(), Some(replacement));
}

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

#![cfg(all(not(target_arch = "wasm32"), feature = "refresh-file-lock"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use futures::FutureExt;
use meerkat_client::auth_store::{
    FileLockCoordinator, PersistedTokens, RefreshCoordinator, RefreshError, TokenKey,
};

const CHILD_ENV: &str = "MEERKAT_REFRESH_CROSSPROC_CHILD";
const COUNTER_ENV: &str = "MEERKAT_REFRESH_CROSSPROC_COUNTER";
const LOCK_DIR_ENV: &str = "MEERKAT_REFRESH_CROSSPROC_LOCK_DIR";

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
    let lock_dir = PathBuf::from(std::env::var(LOCK_DIR_ENV).unwrap());

    let coord = FileLockCoordinator::new(lock_dir);
    let key = TokenKey::new("crossproc", "counter");
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
    let lock_dir = temp.path().join("locks");
    write_counter(&counter, 0);

    let exe = std::env::current_exe().unwrap();
    let n_children: u64 = 4;

    // Spawn N child processes that each refresh through a new
    // FileLockCoordinator pointing at the same lock_dir.
    let mut handles = Vec::new();
    for i in 0..n_children {
        let exe = exe.clone();
        let counter = counter.clone();
        let lock_dir = lock_dir.clone();
        handles.push(tokio::task::spawn_blocking(
            move || -> std::io::Result<()> {
                let status = Command::new(&exe)
                    .arg("--exact")
                    .arg("crossproc_child_runner")
                    .arg("--nocapture")
                    .env(CHILD_ENV, "1")
                    .env(COUNTER_ENV, counter)
                    .env(LOCK_DIR_ENV, &lock_dir)
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
    let key = TokenKey::new("dev", "x");

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
                .with_refresh(
                    key,
                    Box::new(move || {
                        async move {
                            let cur =
                                in_flight.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                            max_in_flight.fetch_max(cur, std::sync::atomic::Ordering::SeqCst);
                            tokio::time::sleep(Duration::from_millis(30)).await;
                            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            in_flight.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                            Ok::<_, RefreshError>(PersistedTokens::api_key("ok"))
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

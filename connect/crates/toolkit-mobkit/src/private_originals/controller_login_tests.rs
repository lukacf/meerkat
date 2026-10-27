//! Actual blocking-writer custody, with a mechanical login owner and a held
//! write closure. These controls do not authenticate a Human or issue a grant.
use super::*;
use crate::private_host::{ConsoleLoginHandle, ConsoleLoginSelection, ConsoleLogins};
use std::{sync::atomic::{AtomicUsize, Ordering}, time::{Duration, SystemTime, UNIX_EPOCH}};

fn login_owner() -> (tokio::sync::watch::Sender<bool>, ConsoleLogins, ConsoleLoginHandle) {
    let (connection, owner) = ConsoleLogins::fixture_owner();
    let expires_at_ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64 + 60_000;
    let login = owner.fixture_enroll(2, ConsoleLoginSelection {
        issuer: "controller-writer-fixture".into(), subject: "fixture-subject".into(),
        login_scope_id: "fixture-login".into(), expires_at_ms,
    }).unwrap();
    (connection, owner, login)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn approved_login_remains_with_blocking_writer_after_waiter_cancellation() {
    let (_connection, logins, login) = login_owner();
    let (entered, observe_entry) = tokio::sync::oneshot::channel();
    let (release, wait_release) = std::sync::mpsc::channel();
    let writes = Arc::new(AtomicUsize::new(0));
    let count = writes.clone();
    let worker = tokio::task::spawn_blocking(move || {
        with_controller_login(None, Some(&login), || {
            count.fetch_add(1, Ordering::SeqCst);
            entered.send(()).unwrap();
            wait_release.recv().unwrap();
            Ok(())
        })
    });
    let waiter = tokio::spawn(async move { worker.await.unwrap() });
    observe_entry.await.unwrap();
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    let mut retirement = logins.fixture_retire(2).unwrap();
    assert!(tokio::time::timeout(Duration::from_millis(20), &mut retirement).await.is_err(),
        "the detached blocking writer still owns the same login reservation");
    release.send(()).unwrap();
    let observed = tokio::time::timeout(Duration::from_secs(2), retirement).await.unwrap().unwrap();
    assert!(observed.fenced && observed.reservations_drained && !observed.uncertain);
    assert_eq!(writes.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retired_approved_login_refuses_the_queued_writer_before_its_write() {
    let (_connection, logins, login) = login_owner();
    let (queued, observe_queue) = tokio::sync::oneshot::channel();
    let (release, wait_release) = std::sync::mpsc::channel();
    let writes = Arc::new(AtomicUsize::new(0));
    let count = writes.clone();
    let worker = tokio::task::spawn_blocking(move || {
        queued.send(()).unwrap();
        wait_release.recv().unwrap();
        with_controller_login(None, Some(&login), || {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    });
    observe_queue.await.unwrap();
    let observed = logins.fixture_retire(2).unwrap().await.unwrap();
    assert!(observed.fenced && observed.reservations_drained && !observed.uncertain);
    release.send(()).unwrap();
    assert!(matches!(worker.await.unwrap(), Err(HostLifetimeError::Denied)));
    assert_eq!(writes.load(Ordering::SeqCst), 0);
}

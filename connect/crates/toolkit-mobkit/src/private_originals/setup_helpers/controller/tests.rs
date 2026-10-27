//! Mechanical lifetime controls. These supply no native Human, grant or
//! dispatch authority; the joined actual acquisition fixture owns that lane.
use super::*;
use crate::private_host::{ConsoleLoginSelection, ConsoleLogins};
use std::{sync::atomic::{AtomicUsize, Ordering}, time::{Duration, SystemTime, UNIX_EPOCH}};

fn enroll(logins: &ConsoleLogins, id: u64) -> ConsoleLoginHandle {
    logins.fixture_enroll(id, ConsoleLoginSelection {
        issuer: "https://fixture.invalid".into(), subject: "retained-user".into(),
        login_scope_id: format!("login-{id}"),
        expires_at_ms: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64 + 60_000,
    }).unwrap()
}

#[tokio::test]
async fn retired_contributor_prevents_issue_and_settles_earlier_reservations() {
    let (_connection, logins) = ConsoleLogins::fixture_owner();
    let first = enroll(&logins, 1);
    let retired = enroll(&logins, 2);
    let second_done = logins.fixture_retire(2).unwrap().await.unwrap();
    assert!(second_done.fenced && second_done.reservations_drained && !second_done.uncertain);
    let calls = AtomicUsize::new(0);
    assert_eq!(with_helper_logins(&[first, retired], || {
        calls.fetch_add(1, Ordering::SeqCst); Ok(())
    }), Err(HelperChildIssuanceRefusal::Denied));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let first_done = logins.fixture_retire(1).unwrap().await.unwrap();
    assert!(first_done.fenced && first_done.reservations_drained && !first_done.uncertain);
}

#[tokio::test]
async fn entered_blocking_writer_outlives_caller_and_retirement_joins_it() {
    let (_connection, logins) = ConsoleLogins::fixture_owner();
    let login = enroll(&logins, 1);
    let (entered, observed) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::channel();
    let (done, completed) = tokio::sync::oneshot::channel();
    let writer = tokio::task::spawn_blocking(move || {
        let result = with_helper_logins(&[login], || {
            entered.send(()).unwrap();
            released.recv().unwrap();
            Ok(())
        });
        done.send(result).unwrap();
    });
    tokio::time::timeout(Duration::from_secs(2), observed).await.unwrap().unwrap();
    // Dropping the caller's join handle does not drop already entered work.
    drop(writer);
    let mut retirement = logins.fixture_retire(1).unwrap();
    assert!(tokio::time::timeout(Duration::from_millis(20), &mut retirement).await.is_err());
    release.send(()).unwrap();
    assert_eq!(tokio::time::timeout(Duration::from_secs(2), completed).await.unwrap().unwrap(), Ok(()));
    let retired = tokio::time::timeout(Duration::from_secs(2), retirement).await.unwrap().unwrap();
    assert!(retired.fenced && retired.reservations_drained && !retired.uncertain);
}

#[tokio::test]
async fn uncertain_native_response_drains_without_claiming_no_effect() {
    let (_connection, logins) = ConsoleLogins::fixture_owner();
    let login = enroll(&logins, 1);
    assert_eq!(with_helper_logins(&[login], || Err::<(), _>(HelperChildIssuanceRefusal::Uncertain)),
        Err(HelperChildIssuanceRefusal::Uncertain));
    let retired = logins.fixture_retire(1).unwrap().await.unwrap();
    assert!(retired.fenced && retired.reservations_drained && retired.uncertain);
}

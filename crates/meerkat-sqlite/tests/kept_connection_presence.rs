//! A connection kept open across reads holds its database's presence lock
//! (`<file>.mpresence`) shared for its whole life, and the maintenance fence
//! takes that lock exclusively after the fence lock, so maintenance never
//! works under a kept connection.
//!
//! The kept connection is a bare shared lock on the presence file here, so
//! the protocol is stated through the lock files alone.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::fs::{File, OpenOptions};
use std::path::Path;
use std::time::{Duration, Instant};

use meerkat_sqlite::{ExclusiveFence, OperationGuard, SqliteStoreError};

/// Wait until the fence thread holds `<db>.mfence`: a guard is refused typed.
/// A guard admitted before that is dropped at once and tried again.
fn wait_for_the_fence_lock(db: &Path) {
    let started = Instant::now();
    loop {
        match OperationGuard::for_database(db) {
            Err(SqliteStoreError::MaintenanceFenceHeld { .. }) => return,
            Ok(guard) => drop(guard),
            Err(other) => panic!("unexpected guard error: {other}"),
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the waiting fence never took its fence lock"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Hold `<db>.mpresence` shared, as a kept connection does.
fn kept_connection(db: &Path) -> File {
    let mut name = db.file_name().expect("file name").to_os_string();
    name.push(".mpresence");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(db.with_file_name(name))
        .expect("open the presence lock file");
    file.try_lock_shared().expect("shared presence");
    file
}

#[test]
fn the_fence_is_refused_while_a_connection_is_kept_and_taken_once_it_closes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("runtime.sqlite3");
    let kept = kept_connection(&db);

    assert!(
        ExclusiveFence::try_acquire(&db).expect("io").is_none(),
        "the fence must not be taken while a connection is kept"
    );
    let err = ExclusiveFence::acquire(&db, Duration::from_millis(200))
        .expect_err("a kept connection that stays open outlives the deadline");
    assert!(
        matches!(err, SqliteStoreError::MaintenanceFenceHeld { .. }),
        "refused typed: {err}"
    );
    // A refused fence holds nothing: ordinary operations go on.
    OperationGuard::for_database(&db).expect("guarded operations go on");

    // Released as a kept connection releases its presence: unlocked, then
    // closed.
    kept.unlock().expect("release the presence");
    drop(kept);
    let fence = ExclusiveFence::try_acquire(&db)
        .expect("io")
        .expect("taken once the kept connection closed");
    drop(fence);
}

/// While maintenance holds the fence lock and waits for the presence lock,
/// the kept connection's next read is refused typed, and the fence completes
/// as soon as the connection closes.
#[test]
fn a_kept_connections_next_read_is_refused_while_the_fence_waits_for_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("runtime.sqlite3");
    let kept = kept_connection(&db);

    let waiting = {
        let db = db.clone();
        std::thread::spawn(move || ExclusiveFence::acquire(&db, Duration::from_secs(10)))
    };
    wait_for_the_fence_lock(&db);
    assert!(
        !waiting.is_finished(),
        "the fence waits for the kept connection to close"
    );
    let err = OperationGuard::for_database(&db)
        .expect_err("the kept connection's next read is refused while the fence waits");
    assert!(
        matches!(err, SqliteStoreError::MaintenanceFenceHeld { .. }),
        "refused typed: {err}"
    );

    let closed = Instant::now();
    // Released as a kept connection releases its presence: unlocked, then
    // closed.
    kept.unlock().expect("release the presence");
    drop(kept);
    let fence = waiting
        .join()
        .expect("thread")
        .expect("the fence completes once the kept connection closed");
    assert!(
        closed.elapsed() < Duration::from_secs(2),
        "the fence completes at its next try after the close: {:?}",
        closed.elapsed()
    );
    drop(fence);
}

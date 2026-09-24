//! Per-operation maintenance-fence guards.
//!
//! The SQLite stores deliberately hold no long-lived connection (or, where
//! they do, still perform discrete operations), so a fence honored only at
//! realm open cannot quiesce them: a process that opened its store before the
//! fence was acquired would keep writing straight through it. The fence is
//! therefore enforced **per operation**: every store operation takes a
//! [`OperationGuard`] (a shared OS file lock co-located with the database
//! file), and offline migration takes the [`ExclusiveFence`] and waits for
//! outstanding guards and in-flight operations to drain before touching
//! bytes. Stores built on this crate get this for free — which is also why
//! no store may roll its own opener.
//!
//! Lock files live next to the database as `<file>.mfence`. They are
//! advisory OS locks (flock on unix, `LockFileEx` on Windows), cheap to take
//! per operation, and correct across processes as well as between handles in
//! one process.
//!
//! Fail-open rule: a medium where the lock file cannot be created (read-only
//! snapshot mounts, for example) is a medium where no fence can be held
//! either, so guard acquisition degrades to a no-op there. If the lock file
//! exists and is exclusively held, acquisition fails with the typed
//! [`SqliteStoreError::MaintenanceFenceHeld`] — storage is under offline
//! maintenance and the operation must not proceed.
//!
//! The [`ConnectionProfile::Maintenance`](crate::profile::ConnectionProfile)
//! profile is the fence *holder's* profile; maintenance work runs under the
//! exclusive fence and does not take shared guards.

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::error::SqliteStoreError;

/// Lock paths exclusively held BY THIS PROCESS.
///
/// Holder self-admission: the maintenance verb holds the exclusive fence
/// while reusing production store code paths in-process (bulk checkpoint
/// adoption runs through the ordinary, CAS-hardened store machinery).
/// Those operations must pass their own fence — and an OS-level shared
/// re-lock from the same process would deadlock against the held exclusive
/// lock — so guard acquisition consults this registry first.
fn held_exclusive_locks() -> &'static Mutex<HashSet<PathBuf>> {
    static REGISTRY: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashSet::new()))
}

fn registry_key(lock_path: &Path) -> PathBuf {
    std::fs::canonicalize(lock_path).unwrap_or_else(|_| lock_path.to_path_buf())
}

fn process_holds_exclusive(lock_path: &Path) -> bool {
    held_exclusive_locks()
        .lock()
        .map(|held| held.contains(&registry_key(lock_path)))
        .unwrap_or(false)
}

/// Suffix appended to a database file name to form its fence lock file.
pub const FENCE_LOCK_SUFFIX: &str = "mfence";

/// The fence lock file for a database path: `<file>.mfence`.
pub fn fence_lock_path(db_path: &Path) -> PathBuf {
    let mut name = db_path.file_name().unwrap_or_default().to_os_string();
    name.push(".");
    name.push(FENCE_LOCK_SUFFIX);
    db_path.with_file_name(name)
}

fn open_lock_file(lock_path: &Path) -> Option<File> {
    // Fail-open: if the lock file can neither be opened nor created, no
    // fence can exist on this medium either (see module docs).
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .ok()
}

/// RAII shared guard for one store operation on one database file.
///
/// Held for the duration of the operation; dropping it releases the lock.
#[derive(Debug)]
pub struct OperationGuard {
    _lock: Option<File>,
}

impl OperationGuard {
    /// Acquire the shared guard for an operation on `db_path`.
    ///
    /// Returns [`SqliteStoreError::MaintenanceFenceHeld`] when the exclusive
    /// maintenance fence is held. In-memory databases and media where no
    /// lock file can exist yield a no-op guard.
    pub fn for_database(db_path: &Path) -> Result<Self, SqliteStoreError> {
        if db_path.file_name().is_none() || db_path.as_os_str() == ":memory:" {
            return Ok(Self { _lock: None });
        }
        let lock_path = fence_lock_path(db_path);
        if process_holds_exclusive(&lock_path) {
            // Fence-holder self-admission (see `held_exclusive_locks`).
            return Ok(Self { _lock: None });
        }
        let Some(file) = open_lock_file(&lock_path) else {
            return Ok(Self { _lock: None });
        };
        match file.try_lock_shared() {
            Ok(()) => Ok(Self { _lock: Some(file) }),
            Err(err) if is_would_block(&err) => Err(SqliteStoreError::MaintenanceFenceHeld {
                path: db_path.to_path_buf(),
            }),
            // Filesystems without lock support: fail open, same rationale.
            Err(_) => Ok(Self { _lock: None }),
        }
    }
}

/// The exclusive side of the maintenance fence for one database file.
///
/// Acquiring it waits for outstanding [`OperationGuard`]s to drain (bounded
/// by `deadline`); holding it makes every subsequent guarded operation fail
/// typed. The Phase 6 migration machinery composes one of these per database
/// file it intends to touch, across both candidate roots.
#[derive(Debug)]
pub struct ExclusiveFence {
    _lock: File,
    lock_path: PathBuf,
}

impl ExclusiveFence {
    /// Try to take the fence without waiting.
    pub fn try_acquire(db_path: &Path) -> Result<Option<Self>, SqliteStoreError> {
        let lock_path = fence_lock_path(db_path);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)?;
        match file.try_lock() {
            Ok(()) => {
                if let Ok(mut held) = held_exclusive_locks().lock() {
                    held.insert(registry_key(&lock_path));
                }
                Ok(Some(Self {
                    _lock: file,
                    lock_path,
                }))
            }
            Err(err) if is_would_block(&err) => Ok(None),
            Err(err) => Err(SqliteStoreError::Io(std::io::Error::other(err))),
        }
    }

    /// Take the fence, waiting up to `deadline` for in-flight operations to
    /// drain.
    pub fn acquire(db_path: &Path, deadline: Duration) -> Result<Self, SqliteStoreError> {
        let started = Instant::now();
        loop {
            if let Some(fence) = Self::try_acquire(db_path)? {
                return Ok(fence);
            }
            if started.elapsed() >= deadline {
                return Err(SqliteStoreError::MaintenanceFenceHeld {
                    path: db_path.to_path_buf(),
                });
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// The lock file this fence holds.
    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }
}

impl Drop for ExclusiveFence {
    fn drop(&mut self) {
        if let Ok(mut held) = held_exclusive_locks().lock() {
            held.remove(&registry_key(&self.lock_path));
        }
    }
}

fn is_would_block(err: &std::fs::TryLockError) -> bool {
    matches!(err, std::fs::TryLockError::WouldBlock)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn shared_guards_coexist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("db.sqlite3");
        let g1 = OperationGuard::for_database(&db).expect("g1");
        let g2 = OperationGuard::for_database(&db).expect("g2");
        drop((g1, g2));
    }

    #[test]
    fn exclusive_fence_blocks_foreign_operations_typed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("db.sqlite3");
        // Simulate ANOTHER process holding the exclusive fence: a raw
        // exclusive lock on the fence file without the holder registry entry
        // (the registry is what self-admits the in-process holder).
        let lock_path = fence_lock_path(&db);
        let foreign = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .expect("open lock file");
        foreign.try_lock().expect("foreign exclusive lock");
        let err = OperationGuard::for_database(&db).expect_err("fence must block foreign ops");
        assert!(matches!(err, SqliteStoreError::MaintenanceFenceHeld { .. }));
        drop(foreign);
        OperationGuard::for_database(&db).expect("released after drop");
    }

    #[test]
    fn exclusive_fence_waits_for_guard_drain() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("db.sqlite3");
        let guard = OperationGuard::for_database(&db).expect("guard");
        assert!(
            ExclusiveFence::try_acquire(&db).expect("io").is_none(),
            "fence must not be acquirable while an operation is in flight"
        );
        let handle = std::thread::spawn(move || {
            ExclusiveFence::acquire(&db, Duration::from_secs(5)).expect("acquire")
        });
        std::thread::sleep(Duration::from_millis(100));
        drop(guard);
        let fence = handle.join().expect("thread");
        assert!(fence.lock_path().ends_with("db.sqlite3.mfence"));
    }

    #[test]
    fn fence_holder_operations_self_admit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("db.sqlite3");
        let fence = ExclusiveFence::try_acquire(&db)
            .expect("io")
            .expect("acquired");
        // The holder process's own store operations pass their guards while
        // the fence is held (bulk maintenance reuses production store code).
        let guard = OperationGuard::for_database(&db).expect("holder self-admission");
        drop(guard);
        drop(fence);
        // After release, ordinary shared guards work as usual...
        OperationGuard::for_database(&db).expect("released");
        // ...and a second process-side exclusive acquisition still excludes.
        let fence = ExclusiveFence::try_acquire(&db)
            .expect("io")
            .expect("reacquired");
        drop(fence);
    }

    #[test]
    fn in_memory_paths_get_noop_guards() {
        let guard = OperationGuard::for_database(Path::new(":memory:")).expect("noop");
        drop(guard);
    }

    #[test]
    fn fence_lock_path_shape() {
        assert_eq!(
            fence_lock_path(Path::new("/a/b/sessions.sqlite3")),
            PathBuf::from("/a/b/sessions.sqlite3.mfence")
        );
    }
}

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
//! The one connection kept open across operations is a kept reader (the
//! runtime store's delivery-generation reader). It holds the presence lock
//! ([`KeptConnectionPresence`]) shared for its whole life, from before its
//! connection opens until after it closes, and still takes an
//! [`OperationGuard`] for each of its reads. Maintenance takes the presence
//! lock exclusively after the fence lock, so it touches no bytes while a kept
//! connection is open.
//!
//! Lock files live next to the database as `<file>.mfence` (the fence lock)
//! and `<file>.mpresence` (the presence lock). They are advisory OS locks
//! (flock on unix, `LockFileEx` on Windows), cheap to take per operation, and
//! correct across processes as well as between handles in one process.
//!
//! Lock order: a kept reader holds its presence lock for its whole life and
//! takes the fence lock for each read by try only, never waiting for it;
//! maintenance takes the fence lock, then waits for the presence lock. Once
//! maintenance holds the fence lock a kept reader's next read is refused and
//! the reader closes, and an idle one closes at its owner's idle bound, so
//! the wait ends; neither side waits for a lock while holding one the other
//! waits for.
//!
//! Fail-open rule: a medium where the lock file cannot be created (read-only
//! snapshot mounts, for example) is a medium where no fence can be held
//! either, so guard acquisition degrades to a no-op there. If the lock file
//! exists and is exclusively held, acquisition fails with the typed
//! [`SqliteStoreError::MaintenanceFenceHeld`] — storage is under offline
//! maintenance and the operation must not proceed. Presence fails closed
//! instead: where the presence lock cannot be held, no connection is kept
//! and the reader opens one per read.
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

/// Suffix appended to a database file name to form its presence lock file.
pub const PRESENCE_LOCK_SUFFIX: &str = "mpresence";

/// The fence lock file for a database path: `<file>.mfence`.
pub fn fence_lock_path(db_path: &Path) -> PathBuf {
    sibling_lock_path(db_path, FENCE_LOCK_SUFFIX)
}

/// The presence lock file for a database path: `<file>.mpresence`.
pub fn presence_lock_path(db_path: &Path) -> PathBuf {
    sibling_lock_path(db_path, PRESENCE_LOCK_SUFFIX)
}

fn sibling_lock_path(db_path: &Path, suffix: &str) -> PathBuf {
    let mut name = db_path.file_name().unwrap_or_default().to_os_string();
    name.push(".");
    name.push(suffix);
    db_path.with_file_name(name)
}

/// A lock file this process holds an OS lock on, through a descriptor it
/// opened itself. Dropping it unlocks the file, then closes it. The lock
/// belongs to the open file description, and a process spawned meanwhile
/// holds a copy of the descriptor until it executes its program, which would
/// keep a lock released by the close alone. Each value opened its own
/// description, so the unlock releases no other holder's lock.
#[derive(Debug)]
struct HeldLock(File);

impl std::ops::Deref for HeldLock {
    type Target = File;

    fn deref(&self) -> &File {
        &self.0
    }
}

impl Drop for HeldLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
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
    _lock: Option<HeldLock>,
}

impl OperationGuard {
    /// Require actual descriptor-scoped shared custody for online administration.
    /// Unlike ordinary I/O this has no maintenance-holder shortcut or fail-open
    /// fallback. Validate the database before creating any lock sidecar.
    pub fn try_for_database_strict(db_path: &Path) -> Result<Self, SqliteStoreError> {
        if db_path.file_name().is_none() || db_path.as_os_str() == ":memory:" {
            return Err(SqliteStoreError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "strict operation custody requires an existing physical database",
            )));
        }
        crate::profile::validate_database_file(db_path)?;
        if !db_path.is_file() {
            return Err(SqliteStoreError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "strict operation database is missing",
            )));
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
        {
            Err(SqliteStoreError::Io(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "strict operation custody requires descriptor-scoped locking",
            )))
        }
        #[cfg(any(target_os = "macos", target_os = "linux", windows))]
        {
            let path = fence_lock_path(db_path);
            crate::profile::validate_database_file(&path)?;
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)?;
            crate::profile::validate_database_file(&path)?;
            if !file.metadata()?.is_file() {
                return Err(SqliteStoreError::Io(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "strict operation lock is not a regular file",
                )));
            }
            match file.try_lock_shared() {
                Ok(()) => Ok(Self {
                    _lock: Some(HeldLock(file)),
                }),
                Err(error) if is_would_block(&error) => {
                    Err(SqliteStoreError::MaintenanceFenceHeld {
                        path: db_path.to_path_buf(),
                    })
                }
                Err(error) => Err(SqliteStoreError::Io(std::io::Error::other(error))),
            }
        }
    }

    /// Acquire the shared guard for an operation on `db_path`.
    ///
    /// Returns [`SqliteStoreError::MaintenanceFenceHeld`] when the exclusive
    /// maintenance fence is held. In-memory databases and media where no
    /// lock file can exist yield a no-op guard.
    pub fn for_database(db_path: &Path) -> Result<Self, SqliteStoreError> {
        if db_path.file_name().is_none() || db_path.as_os_str() == ":memory:" {
            return Ok(Self { _lock: None });
        }
        // Refuse an unsafe database file before planting its fence sibling.
        crate::profile::validate_database_file(db_path)?;
        let lock_path = fence_lock_path(db_path);
        if process_holds_exclusive(&lock_path) {
            // Fence-holder self-admission (see `held_exclusive_locks`).
            return Ok(Self { _lock: None });
        }
        let Some(file) = open_lock_file(&lock_path) else {
            return Ok(Self { _lock: None });
        };
        match file.try_lock_shared() {
            Ok(()) => Ok(Self {
                _lock: Some(HeldLock(file)),
            }),
            Err(err) if is_would_block(&err) => Err(SqliteStoreError::MaintenanceFenceHeld {
                path: db_path.to_path_buf(),
            }),
            // Filesystems without lock support: fail open, same rationale.
            Err(_) => Ok(Self { _lock: None }),
        }
    }

    /// Admit an operation that finds `db_path` missing and answers without
    /// touching it. The fence covers its database file whether or not the
    /// file exists (an archive by rename removes the file while the fence is
    /// held), so a missing database under the exclusive fence of another
    /// holder fails typed with [`SqliteStoreError::MaintenanceFenceHeld`],
    /// as [`Self::for_database`] does. Nothing is created: the lock file is
    /// opened only if it exists, and an absent lock file holds no fence. The
    /// in-process fence holder is admitted, as by [`Self::for_database`].
    pub fn admit_missing_database(db_path: &Path) -> Result<(), SqliteStoreError> {
        if db_path.file_name().is_none() || db_path.as_os_str() == ":memory:" {
            return Ok(());
        }
        let lock_path = fence_lock_path(db_path);
        if process_holds_exclusive(&lock_path) {
            return Ok(());
        }
        let Ok(file) = OpenOptions::new().read(true).write(true).open(&lock_path) else {
            return Ok(());
        };
        match file.try_lock_shared() {
            Err(err) if is_would_block(&err) => Err(SqliteStoreError::MaintenanceFenceHeld {
                path: db_path.to_path_buf(),
            }),
            // Admitted: the shared lock is released at once.
            Ok(()) => {
                drop(HeldLock(file));
                Ok(())
            }
            // A filesystem without lock support: fail open, as
            // `for_database`.
            Err(_) => Ok(()),
        }
    }
}

/// Shared presence of a connection kept open across reads of one database
/// file (a kept reader).
///
/// The reader takes it before its connection opens and drops it after the
/// connection closes: in the reader, declare the connection before the
/// presence, so the close (and any checkpoint it runs) finishes before the
/// lock goes. Between reads the reader holds no [`OperationGuard`]; each read
/// takes one, and a refused guard
/// ([`SqliteStoreError::MaintenanceFenceHeld`]) closes the connection and
/// drops this presence. [`ExclusiveFence`] takes the presence lock
/// exclusively after the fence lock (the module documents the lock order).
#[derive(Debug)]
pub struct KeptConnectionPresence {
    _lock: HeldLock,
}

impl KeptConnectionPresence {
    /// Take shared presence for a connection to be kept open on `db_path`.
    ///
    /// `None` means no connection may be kept now; the caller opens one per
    /// read instead:
    /// - this process holds the exclusive fence of `db_path`: the holder
    ///   registry has the database's `.mfence` path, canonicalized as the
    ///   registry keys it. The holder's own reads pass their guards through
    ///   the registry and keep nothing, so the holder never waits on itself;
    /// - maintenance holds the presence lock;
    /// - the database file is missing (nothing to keep, and no presence file
    ///   is planted next to it), has no file identity (as on every platform
    ///   but unix), or is refused (see
    ///   [`validate_database_file`](crate::profile::validate_database_file)),
    ///   or the presence lock file cannot be created or locked (a read-only
    ///   medium, a filesystem without locks): presence that cannot be held
    ///   is never assumed;
    /// - `db_path` is in-memory or has no file name.
    pub fn acquire(db_path: &Path) -> Option<Self> {
        if db_path.file_name().is_none() || db_path.as_os_str() == ":memory:" {
            return None;
        }
        if process_holds_exclusive(&fence_lock_path(db_path)) {
            return None;
        }
        // Refuse a missing or unsafe database file before planting its
        // presence sibling.
        let Ok(Some(_)) = crate::profile::validate_database_file(db_path) else {
            return None;
        };
        let file = open_lock_file(&presence_lock_path(db_path))?;
        file.try_lock_shared().ok()?;
        Some(Self {
            _lock: HeldLock(file),
        })
    }
}

/// The exclusive side of the maintenance fence for one database file.
///
/// It holds the fence lock and the presence lock. Acquiring it waits for
/// outstanding [`OperationGuard`]s to drain and for kept connections
/// ([`KeptConnectionPresence`]) to close, both bounded by one `deadline`;
/// holding it makes every subsequent guarded operation fail typed and keeps
/// any connection from being kept. The Phase 6 migration machinery composes
/// one of these per database file it intends to touch, across both candidate
/// roots.
#[derive(Debug)]
pub struct ExclusiveFence {
    _lock: HeldLock,
    _presence: HeldLock,
    lock_path: PathBuf,
}

impl ExclusiveFence {
    /// Try to take the fence without waiting: both locks, or neither.
    pub fn try_acquire(db_path: &Path) -> Result<Option<Self>, SqliteStoreError> {
        let lock_path = fence_lock_path(db_path);
        let Some(fence) = try_lock_exclusive(&lock_path)? else {
            return Ok(None);
        };
        // Dropping `fence` on either early return releases the fence lock.
        let Some(presence) = try_lock_exclusive(&presence_lock_path(db_path))? else {
            return Ok(None);
        };
        Ok(Some(Self::held(fence, presence, lock_path)))
    }

    /// Take the fence, waiting up to `deadline` for in-flight operations to
    /// drain and kept connections to close.
    ///
    /// The fence lock comes first: from then on every guarded operation is
    /// refused, a kept reader's next read included, and the reader closes.
    /// The presence lock follows within the same deadline; a reader that
    /// reads nothing closes at its owner's idle bound, so a caller that can
    /// meet a kept reader passes a deadline longer than that bound plus one
    /// read. Past the deadline both locks are released and the fence fails
    /// typed ([`SqliteStoreError::MaintenanceFenceHeld`]).
    ///
    /// Blocks the calling thread (bounded by `deadline`); async callers
    /// should run it in `spawn_blocking`, or a kept reader on the blocked
    /// runtime cannot close.
    pub fn acquire(db_path: &Path, deadline: Duration) -> Result<Self, SqliteStoreError> {
        let started = Instant::now();
        let lock_path = fence_lock_path(db_path);
        let fence = loop {
            if let Some(fence) = try_lock_exclusive(&lock_path)? {
                break fence;
            }
            if started.elapsed() >= deadline {
                return Err(SqliteStoreError::MaintenanceFenceHeld {
                    path: db_path.to_path_buf(),
                });
            }
            std::thread::sleep(FENCE_POLL);
        };
        let presence_path = presence_lock_path(db_path);
        loop {
            if let Some(presence) = try_lock_exclusive(&presence_path)? {
                return Ok(Self::held(fence, presence, lock_path));
            }
            if started.elapsed() >= deadline {
                // Dropping `fence` releases the fence lock.
                return Err(SqliteStoreError::MaintenanceFenceHeld {
                    path: db_path.to_path_buf(),
                });
            }
            std::thread::sleep(FENCE_POLL);
        }
    }

    /// Record both held locks in the holder registry (see
    /// `held_exclusive_locks`): only a fence that holds both admits its
    /// holder's own operations.
    fn held(fence: HeldLock, presence: HeldLock, lock_path: PathBuf) -> Self {
        if let Ok(mut held) = held_exclusive_locks().lock() {
            held.insert(registry_key(&lock_path));
        }
        Self {
            _lock: fence,
            _presence: presence,
            lock_path,
        }
    }

    /// The lock file this fence holds.
    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }
}

/// How often [`ExclusiveFence::acquire`] retries a held lock.
const FENCE_POLL: Duration = Duration::from_millis(25);

/// Take `lock_path` exclusively without waiting; `None` while another holder
/// has it.
fn try_lock_exclusive(lock_path: &Path) -> Result<Option<HeldLock>, SqliteStoreError> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(HeldLock(file))),
        Err(err) if is_would_block(&err) => Ok(None),
        Err(err) => Err(SqliteStoreError::Io(std::io::Error::other(err))),
    }
}

impl Drop for ExclusiveFence {
    /// The holder registry forgets the fence first, so this process admits
    /// its own operations no more; its two locks are released after, as
    /// their fields drop.
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
        foreign.unlock().expect("release the foreign fence");
        drop(foreign);
        OperationGuard::for_database(&db).expect("released after drop");
    }

    /// A missing database is refused typed while another holder holds its
    /// fence, admitted otherwise (no lock file, a released fence, or this
    /// process's own fence), and nothing is ever created for it.
    #[test]
    fn a_missing_database_honours_a_held_fence_and_creates_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("db.sqlite3");
        OperationGuard::admit_missing_database(&db).expect("no lock file: no fence");
        assert!(
            !fence_lock_path(&db).exists(),
            "admission creates no lock file"
        );

        let lock_path = fence_lock_path(&db);
        let foreign = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .expect("open lock file");
        foreign.try_lock().expect("foreign exclusive lock");
        let err = OperationGuard::admit_missing_database(&db)
            .expect_err("a foreign fence covers the missing database");
        assert!(matches!(err, SqliteStoreError::MaintenanceFenceHeld { .. }));
        foreign.unlock().expect("release the foreign fence");
        drop(foreign);
        OperationGuard::admit_missing_database(&db).expect("released after drop");

        let own = ExclusiveFence::try_acquire(&db)
            .expect("io")
            .expect("own fence");
        OperationGuard::admit_missing_database(&db).expect("the fence holder is admitted");
        drop(own);
        assert!(!db.exists(), "nothing created the database");
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
        assert_eq!(
            presence_lock_path(Path::new("/a/b/sessions.sqlite3")),
            PathBuf::from("/a/b/sessions.sqlite3.mpresence")
        );
    }

    /// A database file at `dir/db.sqlite3`: presence is only taken for a
    /// file that exists.
    fn existing_database(dir: &Path) -> PathBuf {
        let db = dir.join("db.sqlite3");
        std::fs::write(&db, b"").expect("database file");
        db
    }

    /// A kept connection's presence keeps the fence from being taken, and a
    /// refused attempt leaves no lock behind: guarded operations go on.
    #[test]
    fn kept_presence_refuses_the_fence_and_a_refused_fence_holds_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = existing_database(dir.path());
        let presence = KeptConnectionPresence::acquire(&db).expect("presence");
        assert!(
            ExclusiveFence::try_acquire(&db).expect("io").is_none(),
            "the fence must not be taken while a connection is kept"
        );
        OperationGuard::for_database(&db).expect("a refused try holds no fence lock");
        let err = ExclusiveFence::acquire(&db, Duration::from_millis(200))
            .expect_err("a kept connection outlives the deadline");
        assert!(matches!(err, SqliteStoreError::MaintenanceFenceHeld { .. }));
        OperationGuard::for_database(&db).expect("an expired wait holds no fence lock");
        drop(presence);
        let fence = ExclusiveFence::try_acquire(&db)
            .expect("io")
            .expect("taken once the kept connection closed");
        drop(fence);
    }

    /// While maintenance holds the fence lock and waits for the presence
    /// lock, a kept reader's next read is refused typed at once, and the
    /// fence completes when the reader closes.
    #[test]
    fn a_waiting_fence_refuses_the_kept_readers_next_read_and_completes_when_it_closes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = existing_database(dir.path());
        let presence = KeptConnectionPresence::acquire(&db).expect("presence");
        let waiting = {
            let db = db.clone();
            std::thread::spawn(move || ExclusiveFence::acquire(&db, Duration::from_secs(10)))
        };
        // The fence lock is taken first; the reader's next guard is refused.
        let started = Instant::now();
        loop {
            match OperationGuard::for_database(&db) {
                Err(SqliteStoreError::MaintenanceFenceHeld { .. }) => break,
                Ok(guard) => drop(guard),
                Err(other) => panic!("unexpected guard error: {other}"),
            }
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "the waiting fence never took its fence lock"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            !waiting.is_finished(),
            "the fence waits for the kept reader"
        );
        drop(presence);
        let fence = waiting
            .join()
            .expect("thread")
            .expect("fence after the reader closed");
        assert!(
            KeptConnectionPresence::acquire(&db).is_none(),
            "no connection is kept while the fence is held"
        );
        drop(fence);
        KeptConnectionPresence::acquire(&db).expect("presence after release");
    }

    /// No connection is kept while maintenance holds the database, whether
    /// this process holds the fence (the holder registry) or another does.
    #[test]
    fn presence_is_refused_while_the_fence_is_held() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = existing_database(dir.path());
        let own = ExclusiveFence::try_acquire(&db)
            .expect("io")
            .expect("own fence");
        assert!(KeptConnectionPresence::acquire(&db).is_none());
        // The holder's own reads still pass their guards (self-admission).
        OperationGuard::for_database(&db).expect("holder self-admission");
        drop(own);

        // Another holder: a raw exclusive presence lock without the registry.
        let foreign = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(presence_lock_path(&db))
            .expect("open presence lock file");
        foreign.try_lock().expect("foreign exclusive presence lock");
        assert!(KeptConnectionPresence::acquire(&db).is_none());
        foreign.unlock().expect("release the foreign presence lock");
        drop(foreign);
        KeptConnectionPresence::acquire(&db).expect("presence after release");
    }

    /// A database file that is refused gets no presence sibling planted.
    #[test]
    fn presence_plants_nothing_next_to_a_refused_database() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("db.sqlite3");
        std::fs::create_dir(&db).expect("a directory where the database belongs");
        assert!(KeptConnectionPresence::acquire(&db).is_none());
        assert!(!presence_lock_path(&db).exists());
        assert!(KeptConnectionPresence::acquire(Path::new(":memory:")).is_none());
    }

    /// A missing database has nothing to keep, and gets no presence sibling.
    #[test]
    fn presence_plants_nothing_next_to_a_missing_database() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("db.sqlite3");
        assert!(KeptConnectionPresence::acquire(&db).is_none());
        assert!(!presence_lock_path(&db).exists());
        assert_eq!(
            std::fs::read_dir(dir.path()).expect("read dir").count(),
            0,
            "nothing planted"
        );
    }

    /// A process this test spawns that holds a copy of one of the test's
    /// descriptors, as its standard input, until it is dropped: what any process
    /// spawned meanwhile holds until it executes its program, kept for as long as
    /// the test needs.
    #[cfg(unix)]
    struct DescriptorCopy(std::process::Child);

    #[cfg(unix)]
    impl DescriptorCopy {
        /// Spawn a process holding a copy of `file`'s descriptor.
        fn spawn(file: &std::fs::File) -> Self {
            let copy = file.try_clone().expect("a copy of the descriptor");
            #[allow(clippy::disallowed_methods)] // the test's own process, which holds the copy
            let child = std::process::Command::new("sleep")
                .arg("600")
                .stdin(copy)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("a process holding the copy");
            Self(child)
        }
    }

    #[cfg(unix)]
    impl Drop for DescriptorCopy {
        fn drop(&mut self) {
            // On request (MEERKAT_TEST_DESCRIPTOR_COPY_LINGER_MS), the copy stays open
            // that long first, so an fd sampler watching the run sees it.
            if let Some(linger) = std::env::var("MEERKAT_TEST_DESCRIPTOR_COPY_LINGER_MS")
                .ok()
                .and_then(|millis| millis.parse::<u64>().ok())
            {
                std::thread::sleep(std::time::Duration::from_millis(linger));
            }
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// A released fence is free at once while a process spawned meanwhile
    /// still holds copies of its descriptors: operations pass, a connection
    /// can be kept, and the fence can be taken again.
    #[cfg(unix)]
    #[test]
    fn a_released_fence_is_free_while_a_spawned_process_holds_its_descriptors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = existing_database(dir.path());
        let fence = ExclusiveFence::try_acquire(&db)
            .expect("fence")
            .expect("the fence is free");
        let copies = [
            DescriptorCopy::spawn(&fence._lock),
            DescriptorCopy::spawn(&fence._presence),
        ];
        drop(fence);
        OperationGuard::for_database(&db).expect("an operation passes a released fence");
        assert!(
            KeptConnectionPresence::acquire(&db).is_some(),
            "a connection can be kept once the fence is released"
        );
        assert!(
            ExclusiveFence::try_acquire(&db).expect("fence").is_some(),
            "a released fence can be taken again"
        );
        drop(copies);
    }

    /// A released operation guard leaves the fence free at once while a
    /// process spawned meanwhile still holds a copy of its descriptor.
    #[cfg(unix)]
    #[test]
    fn a_released_operation_guard_is_free_while_a_spawned_process_holds_its_descriptor() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = existing_database(dir.path());
        let guard = OperationGuard::for_database(&db).expect("guard");
        let copy = DescriptorCopy::spawn(guard._lock.as_ref().expect("a held guard"));
        drop(guard);
        assert!(
            ExclusiveFence::try_acquire(&db).expect("fence").is_some(),
            "the fence is free once the guard is released"
        );
        drop(copy);
    }

    /// A released presence leaves the fence free at once while a process
    /// spawned meanwhile still holds a copy of its descriptor.
    #[cfg(unix)]
    #[test]
    fn a_released_presence_is_free_while_a_spawned_process_holds_its_descriptor() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = existing_database(dir.path());
        let presence = KeptConnectionPresence::acquire(&db).expect("presence");
        let copy = DescriptorCopy::spawn(&presence._lock);
        drop(presence);
        assert!(
            ExclusiveFence::try_acquire(&db).expect("fence").is_some(),
            "the fence is free once the presence is released"
        );
        drop(copy);
    }
}

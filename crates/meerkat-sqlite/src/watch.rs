//! Cross-process change watch for one SQLite database file.
//!
//! Another process's commit to a database is invisible to this process's
//! in-memory signals. This watch turns file-system notifications on the
//! database, its `-wal` and `-shm` sidecars and their directory into
//! coalesced ticks for a consumer that then reads durable state.
//!
//! A notification is a HINT, never a guarantee: SQLite writes WAL frames
//! before the WAL index makes the commit visible, so a reader woken by an
//! event can still read the old state, and no later event is promised. The
//! watch therefore also ticks on a bounded sweep interval; the consumer's
//! sweep read is the eventual-wake guarantee. Neither the watch nor the sweep
//! ever defines permission, custody or expiry: correctness rests with the
//! consumer's durable reads and its own fences.
//!
//! This is the contract the mob event bus has used since its external watch
//! landed, extracted so every store shares one implementation.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher as _};

/// Default sweep interval: the bounded eventual-wake guarantee.
pub const SQLITE_WATCH_SWEEP: Duration = Duration::from_millis(5_000);
/// Settle delay after a notification, so a burst of events (a commit touches
/// the WAL and its index) coalesces into one tick.
const SQLITE_WATCH_COALESCE: Duration = Duration::from_millis(10);
const SQLITE_WATCH_RECOVERY_BACKOFF_MIN: Duration = Duration::from_millis(100);
const SQLITE_WATCH_RECOVERY_BACKOFF_MAX: Duration = Duration::from_millis(30_000);

/// Why the consumer is being asked to look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqliteWatchTick {
    /// A relevant file-system notification arrived (coalesced).
    Changed,
    /// The sweep interval elapsed without a notification.
    Sweep,
}

/// What the consumer's tick achieved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqliteWatchControl {
    /// The tick was handled.
    Handled,
    /// The consumer's durable read failed: ticks are suppressed until a
    /// bounded exponential backoff elapses, so a notification storm cannot
    /// hammer a failing store.
    Failed,
    /// The consumer is gone: stop the watch.
    Stop,
}

/// A running watch. Dropping it stops the notifications, and the worker
/// thread exits at its next wait.
pub struct SqliteChangeWatch {
    _watcher: notify::RecommendedWatcher,
}

impl std::fmt::Debug for SqliteChangeWatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteChangeWatch").finish_non_exhaustive()
    }
}

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

/// The database file and its sidecars.
pub fn sqlite_watch_paths(path: &Path) -> Vec<PathBuf> {
    vec![
        path.to_path_buf(),
        sidecar_path(path, "-wal"),
        sidecar_path(path, "-shm"),
    ]
}

/// Whether a notification concerns the database (its files or directory).
pub fn sqlite_watch_event_relevant(
    event: &notify::Event,
    parent: &Path,
    watched_paths: &[PathBuf],
) -> bool {
    if matches!(event.kind, notify::EventKind::Access(_)) {
        return false;
    }
    if event.paths.is_empty() {
        return true;
    }
    event.paths.iter().any(|path| {
        path == parent
            || watched_paths.iter().any(|watched| {
                path == watched
                    || (path.parent() == watched.parent()
                        && path.file_name() == watched.file_name())
            })
    })
}

/// Start watching `db_path`. `on_tick` runs on a dedicated thread named
/// `thread_name`, once per coalesced notification and once per `sweep`
/// without one.
pub fn start_sqlite_change_watch(
    db_path: &Path,
    thread_name: &str,
    sweep: Duration,
    mut on_tick: impl FnMut(SqliteWatchTick) -> SqliteWatchControl + Send + 'static,
) -> Result<SqliteChangeWatch, String> {
    let parent = db_path
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("{} has no parent directory", db_path.display()))?;
    let watched_paths = sqlite_watch_paths(db_path);
    let (wake_tx, wake_rx) = mpsc::channel::<()>();

    let callback_parent = parent.clone();
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        match result {
            Ok(event) if sqlite_watch_event_relevant(&event, &callback_parent, &watched_paths) => {
                let _ = wake_tx.send(());
            }
            Ok(_) => {}
            Err(error) => {
                // The sweep bounds anything a watcher error lost.
                tracing::warn!(error = %error, "sqlite change watch reported an error");
            }
        }
    })
    .map_err(|error| format!("cannot create the sqlite change watcher: {error}"))?;
    watcher
        .watch(&parent, RecursiveMode::NonRecursive)
        .map_err(|error| format!("cannot watch {}: {error}", parent.display()))?;

    thread::Builder::new()
        .name(thread_name.to_string())
        .spawn(move || {
            let mut recovery_backoff = SQLITE_WATCH_RECOVERY_BACKOFF_MIN;
            let mut recovery_deadline: Option<Instant> = None;
            loop {
                let wait = recovery_deadline
                    .map(|deadline| deadline.saturating_duration_since(Instant::now()))
                    .unwrap_or(sweep);
                let tick = match wake_rx.recv_timeout(wait) {
                    Ok(()) => SqliteWatchTick::Changed,
                    Err(mpsc::RecvTimeoutError::Timeout) => SqliteWatchTick::Sweep,
                    // The watcher (and every sender) is gone: stop.
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };
                if tick == SqliteWatchTick::Changed {
                    thread::sleep(SQLITE_WATCH_COALESCE);
                }
                while wake_rx.try_recv().is_ok() {}
                if recovery_deadline.is_some_and(|deadline| Instant::now() < deadline) {
                    // A notification storm must not bypass a failed read's
                    // recovery deadline.
                    continue;
                }
                match on_tick(tick) {
                    SqliteWatchControl::Handled => {
                        recovery_deadline = None;
                        recovery_backoff = SQLITE_WATCH_RECOVERY_BACKOFF_MIN;
                    }
                    SqliteWatchControl::Failed => {
                        recovery_deadline = Some(Instant::now() + recovery_backoff);
                        recovery_backoff = recovery_backoff
                            .saturating_mul(2)
                            .min(SQLITE_WATCH_RECOVERY_BACKOFF_MAX);
                    }
                    SqliteWatchControl::Stop => break,
                }
            }
        })
        .map_err(|error| format!("cannot start the sqlite change watch thread: {error}"))?;

    Ok(SqliteChangeWatch { _watcher: watcher })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_write_to_the_database_ticks_changed() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("store.sqlite3");
        std::fs::write(&db, b"").unwrap();
        let (tick_tx, tick_rx) = mpsc::channel();
        let _watch =
            start_sqlite_change_watch(&db, "test-watch", SQLITE_WATCH_SWEEP, move |tick| {
                let _ = tick_tx.send(tick);
                SqliteWatchControl::Handled
            })
            .expect("watch starts");
        std::fs::write(sidecar_path(&db, "-wal"), b"frame").unwrap();
        let tick = tick_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the write is observed within the hang guard");
        assert_eq!(tick, SqliteWatchTick::Changed);
    }

    #[test]
    fn the_sweep_ticks_without_a_notification() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("store.sqlite3");
        let (tick_tx, tick_rx) = mpsc::channel();
        let _watch =
            start_sqlite_change_watch(&db, "test-sweep", Duration::from_millis(50), move |tick| {
                let _ = tick_tx.send(tick);
                SqliteWatchControl::Handled
            })
            .expect("watch starts");
        let tick = tick_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the sweep ticks within the hang guard");
        assert_eq!(tick, SqliteWatchTick::Sweep);
    }

    #[test]
    fn stop_ends_the_worker() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("store.sqlite3");
        let (tick_tx, tick_rx) = mpsc::channel();
        let _watch =
            start_sqlite_change_watch(&db, "test-stop", Duration::from_millis(20), move |tick| {
                let _ = tick_tx.send(tick);
                SqliteWatchControl::Stop
            })
            .expect("watch starts");
        tick_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("one tick");
        assert!(
            tick_rx.recv_timeout(Duration::from_millis(500)).is_err(),
            "no tick after Stop: the worker ended (its sender is gone)"
        );
    }
}

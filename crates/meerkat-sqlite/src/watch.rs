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
//! Registering for notifications is a hint too, and nothing waits for it. The
//! operating system can hold a registration without bound (macOS FSEvents
//! registration has taken over 30 seconds) and nothing can interrupt it, so
//! the sweep worker starts first and the registration runs on a thread of its
//! own: starting a watch returns at once, and a registration that fails leaves
//! the watch on its sweep alone. [`SqliteChangeWatch::registration`] reports
//! where it stands.
//!
//! This is the contract the mob event bus has used since its external watch
//! landed, extracted so every store shares one implementation.
//!
//! Teardown is explicit. Dropping a watch, or [`SqliteWatchStopHandle::request_stop`],
//! only asks the worker to return and never blocks: a tick already running
//! finishes on its own thread, unjoined. [`SqliteChangeWatch::stop_and_join`]
//! is the one operation that waits for the worker, and it cannot do so from
//! the worker's own thread. Neither waits for a registration that is still
//! running: a stop cancels it, and its thread discards its result when the
//! platform call returns. That thread is never detached: it stays with its
//! watch and, once the watch is dropped, with the process
//! ([`retained_registrations`]) until it returns.

use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, mpsc};
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
/// Notification registrations a process runs at once. A stalled platform
/// (macOS FSEvents) holds each one for as long as it stalls, with two threads
/// apiece, so a stall holds at most 128 threads; past this many, a new watch
/// runs on its sweep alone instead of adding two more. A watch refused at the
/// cap never registers later: it stays on its sweep for the rest of its life,
/// which for a store held open is the life of the process. That is still
/// correct, since correctness rests on the sweep and a notification only
/// wakes the consumer sooner. The cap fits the bursts of a normal startup,
/// such as parallel store opens. A queue that registers a refused watch once
/// a place frees was considered and closed: the cliff needs more than 64
/// registrations in flight at once.
const MAX_REGISTRATIONS_IN_FLIGHT: usize = 64;
/// Registration threads running now in this process, for watches alive or
/// dropped.
static REGISTRATIONS_IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

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

/// How [`SqliteChangeWatch::stop_and_join`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum SqliteWatchJoin {
    /// The worker returned and was joined: no tick is running and none will
    /// run.
    Joined,
    /// The worker was joined after a tick panicked: no tick is running and
    /// none will run.
    JoinedAfterPanic,
    /// Called on the worker's own thread, from inside a tick. The stop is
    /// requested, but a thread cannot join itself: the worker returns after
    /// the current tick, unjoined. The join handle stays in this watch, so
    /// whoever holds the watch must keep it and join it from another thread.
    /// Never a settled outcome.
    UnjoinedOnWorkerThread,
}

/// Where a watch's notification registration stands
/// ([`SqliteChangeWatch::registration`]). Notifications only shorten a wake:
/// the sweep runs whatever this says.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SqliteWatchRegistration {
    /// Running on its own thread, which the operating system can hold without
    /// bound. Until it ends, only the sweep wakes the consumer.
    Pending,
    /// Registered. Its notifications stop when the watch does.
    Registered,
    /// Failed, or its thread could not start: only the sweep wakes the
    /// consumer.
    Failed(String),
    /// The watch stopped before the registration ended; its thread discarded
    /// the result.
    Cancelled,
}

/// Asks a watch's worker to return, without owning the worker. Cheap to clone;
/// never blocks and never joins.
#[derive(Debug, Clone)]
pub struct SqliteWatchStopHandle {
    control: Arc<WatchControl>,
}

impl SqliteWatchStopHandle {
    /// Ask the worker to return. Once it observes the request it starts no
    /// further tick; a tick already running finishes first. Never blocks.
    pub fn request_stop(&self) {
        self.control.request_stop();
    }
}

#[derive(Debug)]
struct WatchControl {
    stop_requested: Arc<AtomicBool>,
    wake: mpsc::Sender<()>,
}

impl WatchControl {
    fn request_stop(&self) {
        self.stop_requested.store(true, Ordering::Release);
        // Wake a worker waiting for its next tick. A worker that already
        // returned has dropped the receiver; nothing is left to wake.
        let _ = self.wake.send(());
    }
}

/// Join custody of a watch's worker thread.
enum WorkerCustody {
    Running(thread::JoinHandle<()>),
    Joined(SqliteWatchJoin),
}

/// A registration's result, shared by its thread and its watch.
enum RegistrationSlot {
    Pending,
    /// Registered; the source until the watch stops.
    Registered(Option<Box<dyn WatchSource>>),
    Failed(String),
    Cancelled,
}

/// One running registration's place among [`MAX_REGISTRATIONS_IN_FLIGHT`],
/// given back when its thread ends, or when the thread never starts.
struct InFlightPlace;

impl InFlightPlace {
    /// Take a place, or say how many registrations hold them all.
    fn take() -> Result<Self, usize> {
        REGISTRATIONS_IN_FLIGHT
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |running| {
                (running < MAX_REGISTRATIONS_IN_FLIGHT).then_some(running + 1)
            })
            .map(|_| Self)
    }
}

impl Drop for InFlightPlace {
    fn drop(&mut self) {
        REGISTRATIONS_IN_FLIGHT.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A registration thread and what it registers.
struct RegistrationThread {
    handle: thread::JoinHandle<()>,
    directory: PathBuf,
    name: String,
    started: Instant,
}

/// Custody of a watch's notification registration: its result, and its
/// thread until that is joined.
struct RegistrationCustody {
    slot: Arc<Mutex<RegistrationSlot>>,
    /// `None` once joined, or when the thread never started.
    thread: Option<RegistrationThread>,
}

impl RegistrationCustody {
    /// Register `targets` on a thread of its own, named after the watch. A
    /// thread that cannot start, or a process already running
    /// [`MAX_REGISTRATIONS_IN_FLIGHT`] registrations, leaves the registration
    /// failed: the sweep alone wakes the consumer, for the rest of the
    /// watch's life.
    fn spawn(
        watch_name: &str,
        backend: Box<dyn WatchBackend>,
        targets: WatchTargets,
        wake: mpsc::Sender<()>,
        stop_requested: Arc<AtomicBool>,
    ) -> Self {
        let slot = Arc::new(Mutex::new(RegistrationSlot::Pending));
        let place = match InFlightPlace::take() {
            Ok(place) => place,
            Err(running) => {
                settle_failed(
                    &slot,
                    format!(
                        "{running} sqlite change watch notification registrations are already \
                         pending (at most {MAX_REGISTRATIONS_IN_FLIGHT})"
                    ),
                );
                return Self { slot, thread: None };
            }
        };
        let thread_slot = Arc::clone(&slot);
        let directory = targets.parent.clone();
        let name = format!("{watch_name}-register");
        let started = Instant::now();
        let spawned = thread::Builder::new().name(name.clone()).spawn(move || {
            // Given back as the thread ends, however it ends.
            let _place = place;
            register(
                backend.as_ref(),
                &targets,
                wake,
                &stop_requested,
                &thread_slot,
            );
        });
        match spawned {
            Ok(handle) => Self {
                slot,
                thread: Some(RegistrationThread {
                    handle,
                    directory,
                    name,
                    started,
                }),
            },
            Err(error) => {
                settle_failed(
                    &slot,
                    format!("cannot start the sqlite change watch registration thread: {error}"),
                );
                Self { slot, thread: None }
            }
        }
    }

    fn state(&self) -> SqliteWatchRegistration {
        match &*lock(&self.slot) {
            RegistrationSlot::Pending => SqliteWatchRegistration::Pending,
            RegistrationSlot::Registered(_) => SqliteWatchRegistration::Registered,
            RegistrationSlot::Failed(reason) => SqliteWatchRegistration::Failed(reason.clone()),
            RegistrationSlot::Cancelled => SqliteWatchRegistration::Cancelled,
        }
    }

    /// Stop the notifications, once the stop flag is set: drop a registered
    /// source, and join the thread if it has returned. A registration still
    /// running discards its result when it ends.
    fn stop(&mut self) {
        let source = match &mut *lock(&self.slot) {
            RegistrationSlot::Registered(source) => source.take(),
            _ => None,
        };
        drop(source);
        if self
            .thread
            .as_ref()
            .is_some_and(|thread| thread.handle.is_finished())
            && let Some(thread) = self.thread.take()
        {
            let _ = thread.handle.join();
        }
    }

    /// The watch is going: stop, and hand a thread still running to the
    /// process's custody.
    fn release(&mut self) {
        self.stop();
        if let Some(thread) = self.thread.take() {
            retain_registration(thread);
        }
    }
}

/// A running watch: custody of its worker thread and of its notification
/// registration.
///
/// Dropping it requests the stop and never joins its worker: the worker
/// returns at its next wake, after any tick that is running, unjoined. Use
/// [`Self::stop_and_join`] to wait for the worker.
pub struct SqliteChangeWatch {
    control: Arc<WatchControl>,
    registration: RegistrationCustody,
    worker: WorkerCustody,
}

impl std::fmt::Debug for SqliteChangeWatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteChangeWatch").finish_non_exhaustive()
    }
}

impl SqliteChangeWatch {
    /// A handle that can request this watch's stop after the watch itself has
    /// moved elsewhere.
    pub fn stop_handle(&self) -> SqliteWatchStopHandle {
        SqliteWatchStopHandle {
            control: Arc::clone(&self.control),
        }
    }

    /// Ask the worker to return; see [`SqliteWatchStopHandle::request_stop`].
    pub fn request_stop(&self) {
        self.control.request_stop();
    }

    /// Whether the calling thread is this watch's worker, i.e. the caller is
    /// running inside one of its ticks.
    pub fn on_worker_thread(&self) -> bool {
        self.worker_thread_id() == Some(thread::current().id())
    }

    /// The worker thread's identity until the worker is joined.
    pub fn worker_thread_id(&self) -> Option<thread::ThreadId> {
        match &self.worker {
            WorkerCustody::Running(worker) => Some(worker.thread().id()),
            WorkerCustody::Joined(_) => None,
        }
    }

    /// Whether the worker has returned (joined or not). A returned worker
    /// runs no tick again.
    pub fn is_finished(&self) -> bool {
        match &self.worker {
            WorkerCustody::Running(worker) => worker.is_finished(),
            WorkerCustody::Joined(_) => true,
        }
    }

    /// Where the notification registration stands. The sweep runs whatever
    /// it says.
    pub fn registration(&self) -> SqliteWatchRegistration {
        self.registration.state()
    }

    /// Request the stop, stop the notifications, and wait for the worker to
    /// return: a tick that is running finishes first. Unbounded by design: it
    /// waits exactly as long as that tick runs.
    ///
    /// It never waits for a notification registration that is still running:
    /// the stop cancels it, and its thread stays in this watch until it
    /// returns ([`Self::registration`] says
    /// [`SqliteWatchRegistration::Pending`] until then). A later call joins
    /// that thread once it has.
    ///
    /// On the worker's own thread it cannot wait; it returns
    /// [`SqliteWatchJoin::UnjoinedOnWorkerThread`] and keeps the join handle
    /// in this watch, so a later call from another thread still joins.
    /// Calling it again after a join returns the recorded outcome.
    pub fn stop_and_join(&mut self) -> SqliteWatchJoin {
        self.control.request_stop();
        self.registration.stop();
        if self.on_worker_thread() {
            return SqliteWatchJoin::UnjoinedOnWorkerThread;
        }
        let outcome = match std::mem::replace(
            &mut self.worker,
            WorkerCustody::Joined(SqliteWatchJoin::Joined),
        ) {
            WorkerCustody::Running(worker) => match worker.join() {
                Ok(()) => SqliteWatchJoin::Joined,
                Err(_) => SqliteWatchJoin::JoinedAfterPanic,
            },
            WorkerCustody::Joined(outcome) => outcome,
        };
        self.worker = WorkerCustody::Joined(outcome);
        outcome
    }
}

impl Drop for SqliteChangeWatch {
    fn drop(&mut self) {
        // Never wait here: the last owner can drop a watch on any thread,
        // including its own worker's. A registration still running goes to
        // the process's custody.
        self.control.request_stop();
        self.registration.release();
    }
}

/// Registration threads still running after their watch was dropped.
static RETAINED_REGISTRATIONS: OnceLock<Mutex<Vec<RegistrationThread>>> = OnceLock::new();

fn retained() -> &'static Mutex<Vec<RegistrationThread>> {
    RETAINED_REGISTRATIONS.get_or_init(Mutex::default)
}

/// A notification registration still running after its watch was dropped
/// ([`retained_registrations`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SqliteWatchRetainedRegistration {
    /// The directory it registers.
    pub directory: PathBuf,
    /// The name of its thread.
    pub thread_name: String,
    /// How long it has been running.
    pub running_for: Duration,
}

/// The notification registrations this process still holds for watches that
/// are gone. Each one is blocked in the operating system, which nothing can
/// interrupt, and discards its result when that call returns. Joins the ones
/// that have returned; never waits.
pub fn retained_registrations() -> Vec<SqliteWatchRetainedRegistration> {
    let mut retained = lock(retained());
    reap(&mut retained);
    retained
        .iter()
        .map(|thread| SqliteWatchRetainedRegistration {
            directory: thread.directory.clone(),
            thread_name: thread.name.clone(),
            running_for: thread.started.elapsed(),
        })
        .collect()
}

fn retain_registration(thread: RegistrationThread) {
    let mut retained = lock(retained());
    reap(&mut retained);
    retained.push(thread);
}

/// Join the retained threads that have returned.
fn reap(retained: &mut Vec<RegistrationThread>) {
    let (returned, running): (Vec<_>, Vec<_>) = std::mem::take(retained)
        .into_iter()
        .partition(|thread| thread.handle.is_finished());
    *retained = running;
    for thread in returned {
        let _ = thread.handle.join();
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

fn sqlite_watch_targets(db_path: &Path) -> Result<(PathBuf, Vec<PathBuf>), String> {
    let file_name = db_path
        .file_name()
        .ok_or_else(|| format!("{} has no database filename", db_path.display()))?;
    let parent = db_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent = parent.canonicalize().map_err(|error| {
        format!(
            "cannot resolve {} for sqlite watch: {error}",
            parent.display()
        )
    })?;
    // FSEvents reports canonical paths, including /private/var for /var on
    // macOS. Resolve only the existing directory: database and sidecar files
    // may not exist yet, or may have been removed when an event is delivered.
    let watched_paths = sqlite_watch_paths(&parent.join(file_name));
    Ok((parent, watched_paths))
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

/// The directory a watch registers, and the database files whose changes in
/// it wake the worker.
struct WatchTargets {
    parent: PathBuf,
    watched_paths: Vec<PathBuf>,
}

/// Creates a watch's notification source: notify's recommended watcher, or a
/// held fake under the `test-support` feature.
trait WatchBackend: Send {
    /// A source that sends on `wake` for each relevant change once it is
    /// registered.
    fn create(
        &self,
        targets: &WatchTargets,
        wake: mpsc::Sender<()>,
    ) -> Result<Box<dyn WatchSource>, String>;
}

/// A created notification source. Dropping it stops its notifications.
trait WatchSource: Send {
    /// Register `directory`. Returns once the platform has registered it,
    /// which has no bound: macOS FSEvents registration can take tens of
    /// seconds.
    fn watch(&mut self, directory: &Path) -> Result<(), String>;
}

/// notify's recommended watcher: FSEvents on macOS, inotify on Linux.
struct NotifyBackend;

impl WatchBackend for NotifyBackend {
    fn create(
        &self,
        targets: &WatchTargets,
        wake: mpsc::Sender<()>,
    ) -> Result<Box<dyn WatchSource>, String> {
        let parent = targets.parent.clone();
        let watched_paths = targets.watched_paths.clone();
        let watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
            match result {
                Ok(event) if sqlite_watch_event_relevant(&event, &parent, &watched_paths) => {
                    let _ = wake.send(());
                }
                Ok(_) => {}
                Err(error) => {
                    // The sweep bounds anything a watcher error lost.
                    tracing::warn!(error = %error, "sqlite change watch reported an error");
                }
            }
        })
        .map_err(|error| format!("cannot create the sqlite change watcher: {error}"))?;
        Ok(Box::new(NotifySource(watcher)))
    }
}

struct NotifySource(notify::RecommendedWatcher);

impl WatchSource for NotifySource {
    fn watch(&mut self, directory: &Path) -> Result<(), String> {
        self.0
            .watch(directory, RecursiveMode::NonRecursive)
            .map_err(|error| format!("cannot watch {}: {error}", directory.display()))
    }
}

/// The backend that registers a watch of `parent`: notify's, unless test
/// support holds the registrations in that directory.
#[cfg_attr(not(any(test, feature = "test-support")), allow(unused_variables))]
fn watch_backend(parent: &Path) -> Box<dyn WatchBackend> {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(held) = test_support::held_backend(parent) {
        return held;
    }
    Box::new(NotifyBackend)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Start watching `db_path`. `on_tick` runs on a dedicated thread named
/// `thread_name`, once per coalesced notification and once per `sweep`
/// without one, until the watch is stopped or `on_tick` returns
/// [`SqliteWatchControl::Stop`].
///
/// Returns as soon as that worker runs. The notification registration runs
/// on a thread of its own, `<thread_name>-register`, which ends once the
/// platform has registered (at once with inotify);
/// [`SqliteChangeWatch::registration`] reports it. While the platform holds
/// registrations, each one holds that thread, and on macOS notify's run-loop
/// thread too, until the platform call returns. At most 64 run at once in a
/// process: a watch started while 64 are pending runs on its sweep alone
/// (`Failed`) for the rest of its life and logs a warning. Errors only when
/// the database's directory cannot be resolved or the worker cannot start.
pub fn start_sqlite_change_watch(
    db_path: &Path,
    thread_name: &str,
    sweep: Duration,
    on_tick: impl FnMut(SqliteWatchTick) -> SqliteWatchControl + Send + 'static,
) -> Result<SqliteChangeWatch, String> {
    start_with_backend(db_path, thread_name, sweep, on_tick, watch_backend)
}

fn start_with_backend(
    db_path: &Path,
    thread_name: &str,
    sweep: Duration,
    mut on_tick: impl FnMut(SqliteWatchTick) -> SqliteWatchControl + Send + 'static,
    backend: impl FnOnce(&Path) -> Box<dyn WatchBackend>,
) -> Result<SqliteChangeWatch, String> {
    let (parent, watched_paths) = sqlite_watch_targets(db_path)?;
    let backend = backend(&parent);
    let targets = WatchTargets {
        parent,
        watched_paths,
    };
    let (wake_tx, wake_rx) = mpsc::channel::<()>();
    let stop_requested = Arc::new(AtomicBool::new(false));
    let worker_stop_requested = Arc::clone(&stop_requested);

    // The sweep is the guarantee: its worker starts first, before anything
    // that can block.
    let worker = thread::Builder::new()
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
                if worker_stop_requested.load(Ordering::Acquire) {
                    break;
                }
                if tick == SqliteWatchTick::Changed {
                    thread::sleep(SQLITE_WATCH_COALESCE);
                }
                while wake_rx.try_recv().is_ok() {}
                // A stop requested while the burst settled still precedes
                // this tick.
                if worker_stop_requested.load(Ordering::Acquire) {
                    break;
                }
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

    let registration = RegistrationCustody::spawn(
        thread_name,
        backend,
        targets,
        wake_tx.clone(),
        Arc::clone(&stop_requested),
    );
    Ok(SqliteChangeWatch {
        control: Arc::new(WatchControl {
            stop_requested,
            wake: wake_tx,
        }),
        registration,
        worker: WorkerCustody::Running(worker),
    })
}

/// The registration thread: create the source and register it, then hand it
/// to the watch, or discard it when the watch stopped meanwhile.
fn register(
    backend: &dyn WatchBackend,
    targets: &WatchTargets,
    wake: mpsc::Sender<()>,
    stop_requested: &AtomicBool,
    slot: &Mutex<RegistrationSlot>,
) {
    if stop_requested.load(Ordering::Acquire) {
        *lock(slot) = RegistrationSlot::Cancelled;
        return;
    }
    let mut source = match backend.create(targets, wake) {
        Ok(source) => source,
        Err(reason) => {
            settle_error(slot, stop_requested, reason);
            return;
        }
    };
    // notify 8.2.0's inotify watcher ignores a failed start of its event-loop
    // thread; registering then panics, and so would its drop, aborting the
    // process. Contain the panic and leak that source (a sender and a waker)
    // instead of dropping it. An error, unlike a panic, comes from a live
    // event loop, so its source drops normally.
    match std::panic::catch_unwind(AssertUnwindSafe(|| source.watch(&targets.parent))) {
        Ok(Ok(())) => {}
        Ok(Err(reason)) => {
            drop(source);
            settle_error(slot, stop_requested, reason);
            return;
        }
        Err(_) => {
            std::mem::forget(source);
            settle_failed(
                slot,
                "registering the notification source panicked: its event loop is not running"
                    .to_string(),
            );
            return;
        }
    }
    {
        // The stop side sets its flag before it reads the slot, so a flag
        // read under the slot lock never leaves a source live past a stop.
        let mut settled = lock(slot);
        if !stop_requested.load(Ordering::Acquire) {
            *settled = RegistrationSlot::Registered(Some(source));
            return;
        }
    }
    // The watch stopped while the platform registered: discard the source
    // here, so its notifications never start, then settle.
    drop(source);
    *lock(slot) = RegistrationSlot::Cancelled;
}

/// Settle a registration that returned an error: failed, with a warning, while
/// its watch runs on its sweep alone; cancelled, quietly, once the watch has
/// stopped, since nothing is left to run on the sweep.
fn settle_error(slot: &Mutex<RegistrationSlot>, stop_requested: &AtomicBool, reason: String) {
    if stop_requested.load(Ordering::Acquire) {
        *lock(slot) = RegistrationSlot::Cancelled;
    } else {
        settle_failed(slot, reason);
    }
}

fn settle_failed(slot: &Mutex<RegistrationSlot>, reason: String) {
    tracing::warn!(
        %reason,
        "sqlite change watch notifications are unavailable; the watch runs on its sweep alone"
    );
    *lock(slot) = RegistrationSlot::Failed(reason);
}

/// Held watch registrations, for tests of code that starts SQLite change
/// watches. Every watch started on a database in a held directory blocks in
/// its notification registration, as a stalled platform registration (macOS
/// FSEvents) does, until the hold is released.
#[cfg(any(test, feature = "test-support"))]
pub mod test_support {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError, mpsc};
    use std::time::{Duration, Instant};

    use super::{WatchBackend, WatchSource, WatchTargets, lock};

    /// Held directories, by canonical path.
    static HOLDS: OnceLock<Mutex<HashMap<PathBuf, Arc<Hold>>>> = OnceLock::new();

    fn holds() -> &'static Mutex<HashMap<PathBuf, Arc<Hold>>> {
        HOLDS.get_or_init(Mutex::default)
    }

    #[derive(Default)]
    struct Hold {
        gate: Mutex<Gate>,
        changed: Condvar,
        live_sources: AtomicUsize,
        dropped_sources: AtomicUsize,
    }

    #[derive(Default)]
    struct Gate {
        released: bool,
        entered: usize,
    }

    /// Holds the notification registrations of the watches on databases in
    /// one directory. Dropping it releases them and ends the hold.
    pub struct HeldRegistrations {
        directory: PathBuf,
        hold: Arc<Hold>,
    }

    /// Hold the registration of every watch started from now on for a
    /// database in `directory` (an existing directory, matched by its
    /// canonical path), until the returned hold is released or dropped. A
    /// held registration has created its source and blocks in registering
    /// it; once released it registers, without ever notifying.
    pub fn hold_registrations_in(directory: &Path) -> std::io::Result<HeldRegistrations> {
        let directory = directory.canonicalize()?;
        let hold = Arc::new(Hold::default());
        let mut holds = lock(holds());
        if holds.contains_key(&directory) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("{} is already held", directory.display()),
            ));
        }
        holds.insert(directory.clone(), Arc::clone(&hold));
        Ok(HeldRegistrations { directory, hold })
    }

    impl HeldRegistrations {
        /// Wait, at most `bound`, until `count` registrations have entered
        /// the hold. Whether they had.
        pub fn wait_entered(&self, count: usize, bound: Duration) -> bool {
            let deadline = Instant::now() + bound;
            let mut gate = lock(&self.hold.gate);
            while gate.entered < count {
                let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                    return false;
                };
                gate = self
                    .hold
                    .changed
                    .wait_timeout(gate, left)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
            }
            true
        }

        /// Let every held registration register, and every later one in the
        /// directory register at once.
        pub fn release(&self) {
            lock(&self.hold.gate).released = true;
            self.hold.changed.notify_all();
        }

        /// Sources created through this hold and not dropped yet.
        pub fn live_sources(&self) -> usize {
            self.hold.live_sources.load(Ordering::SeqCst)
        }

        /// Sources created through this hold and dropped since.
        pub fn dropped_sources(&self) -> usize {
            self.hold.dropped_sources.load(Ordering::SeqCst)
        }
    }

    impl Drop for HeldRegistrations {
        fn drop(&mut self) {
            lock(holds()).remove(&self.directory);
            self.release();
        }
    }

    /// The held backend for a watch of `parent`, while a hold covers it.
    pub(super) fn held_backend(parent: &Path) -> Option<Box<dyn WatchBackend>> {
        let hold = lock(holds()).get(parent).cloned()?;
        Some(Box::new(HeldBackend(hold)))
    }

    struct HeldBackend(Arc<Hold>);

    impl WatchBackend for HeldBackend {
        fn create(
            &self,
            _targets: &WatchTargets,
            wake: mpsc::Sender<()>,
        ) -> Result<Box<dyn WatchSource>, String> {
            self.0.live_sources.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(HeldSource {
                hold: Arc::clone(&self.0),
                _wake: wake,
            }))
        }
    }

    struct HeldSource {
        hold: Arc<Hold>,
        /// Kept for the source's life, as a real source keeps its wake.
        _wake: mpsc::Sender<()>,
    }

    impl WatchSource for HeldSource {
        fn watch(&mut self, _directory: &Path) -> Result<(), String> {
            let mut gate = lock(&self.hold.gate);
            gate.entered += 1;
            self.hold.changed.notify_all();
            while !gate.released {
                gate = self
                    .hold
                    .changed
                    .wait(gate)
                    .unwrap_or_else(PoisonError::into_inner);
            }
            Ok(())
        }
    }

    impl Drop for HeldSource {
        fn drop(&mut self) {
            self.hold.live_sources.fetch_sub(1, Ordering::SeqCst);
            self.hold.dropped_sources.fetch_add(1, Ordering::SeqCst);
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// Wait, within the hang guard, until the watch's registration ends.
    fn settled_registration(watch: &SqliteChangeWatch) -> SqliteWatchRegistration {
        let deadline = Instant::now() + HANG_GUARD;
        loop {
            let registration = watch.registration();
            if registration != SqliteWatchRegistration::Pending || Instant::now() >= deadline {
                return registration;
            }
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn retained_in(directory: &Path) -> Vec<SqliteWatchRetainedRegistration> {
        retained_registrations()
            .into_iter()
            .filter(|retained| retained.directory == directory)
            .collect()
    }

    fn assert_database_write_ticks_changed(db: &Path) {
        std::fs::write(db, b"").unwrap();
        let (tick_tx, tick_rx) = mpsc::channel();
        let watch = start_sqlite_change_watch(db, "test-watch", SQLITE_WATCH_SWEEP, move |tick| {
            let _ = tick_tx.send(tick);
            SqliteWatchControl::Handled
        })
        .expect("watch starts");
        // Notifications are registered after the start returns: a write
        // before that is seen by the sweep alone.
        assert_eq!(
            settled_registration(&watch),
            SqliteWatchRegistration::Registered
        );
        std::fs::write(sidecar_path(db, "-wal"), b"frame").unwrap();
        let tick = tick_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the write is observed within the hang guard");
        assert_eq!(tick, SqliteWatchTick::Changed);
    }

    #[test]
    fn a_write_to_the_database_ticks_changed() {
        let dir = tempfile::tempdir().unwrap();
        assert_database_write_ticks_changed(&dir.path().join("store.sqlite3"));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_directory_targets_match_canonical_events_for_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        let actual = dir.path().join("actual");
        let alias = dir.path().join("alias");
        std::fs::create_dir(&actual).unwrap();
        std::os::unix::fs::symlink(&actual, &alias).unwrap();
        let db = alias.join("store.sqlite3");
        let (parent, watched_paths) = sqlite_watch_targets(&db).unwrap();
        let canonical_parent = actual.canonicalize().unwrap();
        assert_eq!(parent, canonical_parent);
        assert_eq!(
            watched_paths,
            sqlite_watch_paths(&canonical_parent.join("store.sqlite3"))
        );
        assert!(!db.exists());
        for path in &watched_paths {
            assert!(!path.exists());
            for kind in [
                notify::EventKind::Create(notify::event::CreateKind::File),
                notify::EventKind::Remove(notify::event::RemoveKind::File),
            ] {
                let event = notify::Event::new(kind).add_path(path.clone());
                assert!(sqlite_watch_event_relevant(&event, &parent, &watched_paths));
            }
        }
        for path in [
            canonical_parent.join("other.sqlite3-wal"),
            canonical_parent.join("other").join("store.sqlite3-wal"),
        ] {
            let event = notify::Event::new(notify::EventKind::Any).add_path(path);
            assert!(!sqlite_watch_event_relevant(
                &event,
                &parent,
                &watched_paths
            ));
        }
        let access = notify::Event::new(notify::EventKind::Access(notify::event::AccessKind::Any))
            .add_path(watched_paths[0].clone());
        assert!(!sqlite_watch_event_relevant(
            &access,
            &parent,
            &watched_paths
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_write_through_a_symlinked_directory_ticks_changed() {
        let dir = tempfile::tempdir().unwrap();
        let actual = dir.path().join("actual");
        let alias = dir.path().join("alias");
        std::fs::create_dir(&actual).unwrap();
        std::os::unix::fs::symlink(&actual, &alias).unwrap();
        assert_database_write_ticks_changed(&alias.join("store.sqlite3"));
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

    /// A WAL database holding one row, as the stores keep theirs.
    fn wal_database(dir: &Path) -> PathBuf {
        let db = dir.join("store.sqlite3");
        let conn = crate::open(&db, crate::ConnectionProfile::PRIMARY).unwrap();
        conn.execute_batch("CREATE TABLE t (v INTEGER); INSERT INTO t (v) VALUES (1);")
            .unwrap();
        db
    }

    /// Counts ticks, and reports once when the worker drops its tick closure,
    /// which it does as it returns.
    struct TickProbe {
        ticks: Arc<AtomicUsize>,
        returned: mpsc::Sender<()>,
    }

    impl TickProbe {
        fn tick(&self) {
            self.ticks.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl Drop for TickProbe {
        fn drop(&mut self) {
            let _ = self.returned.send(());
        }
    }

    #[test]
    fn stop_and_join_waits_for_the_tick_in_flight() {
        let dir = tempfile::tempdir().unwrap();
        let db = wal_database(dir.path());
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let finished = Arc::new(AtomicBool::new(false));
        let tick_finished = Arc::clone(&finished);
        let tick_db = db.clone();
        let mut watch = start_sqlite_change_watch(
            &db,
            "test-held-tick",
            Duration::from_millis(20),
            move |_tick| {
                if tick_finished.load(Ordering::SeqCst) {
                    return SqliteWatchControl::Handled;
                }
                // The held tick keeps a read connection open, as a store
                // catch-up does.
                let conn = crate::open(&tick_db, crate::ConnectionProfile::ReadOnly).unwrap();
                let _ = entered_tx.send(());
                let _ = release_rx.recv();
                let value: i64 = conn
                    .query_row("SELECT v FROM t", [], |row| row.get(0))
                    .unwrap();
                assert_eq!(value, 1);
                drop(conn);
                tick_finished.store(true, Ordering::SeqCst);
                SqliteWatchControl::Handled
            },
        )
        .expect("watch starts");
        entered_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("a tick is held within the hang guard");

        let (stopping_tx, stopping_rx) = mpsc::channel();
        let stopper_finished = Arc::clone(&finished);
        let stopper = thread::spawn(move || {
            let _ = stopping_tx.send(());
            let outcome = watch.stop_and_join();
            (outcome, stopper_finished.load(Ordering::SeqCst))
        });
        stopping_rx.recv().unwrap();
        release_tx.send(()).unwrap();
        let (outcome, finished_at_return) = stopper.join().unwrap();

        assert!(
            finished_at_return,
            "stop_and_join returned while a tick still held its connection"
        );
        assert_eq!(outcome, SqliteWatchJoin::Joined);
        // The joined worker can never open the database again, so nothing
        // recreates a file while the directory goes: no sleep, no retry.
        std::fs::remove_dir_all(dir.path()).unwrap();
        assert!(!dir.path().exists());
    }

    #[test]
    fn stop_and_join_on_the_worker_thread_is_unjoined_and_keeps_custody() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("store.sqlite3");
        let custody: Arc<std::sync::Mutex<Option<SqliteChangeWatch>>> = Arc::default();
        let tick_custody = Arc::clone(&custody);
        let (outcome_tx, outcome_rx) = mpsc::channel();
        let watch = start_sqlite_change_watch(
            &db,
            "test-self-stop",
            Duration::from_millis(20),
            move |_| {
                let outcome = tick_custody
                    .lock()
                    .unwrap()
                    .as_mut()
                    .map(SqliteChangeWatch::stop_and_join);
                if let Some(outcome) = outcome {
                    let _ = outcome_tx.send(outcome);
                }
                SqliteWatchControl::Handled
            },
        )
        .expect("watch starts");
        *custody.lock().unwrap() = Some(watch);

        let on_worker = outcome_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("a tick stops its own watch within the hang guard");
        assert_eq!(on_worker, SqliteWatchJoin::UnjoinedOnWorkerThread);

        // The self-stop kept the join handle, so from here the worker is
        // really joined rather than reported as gone.
        let mut watch = custody
            .lock()
            .unwrap()
            .take()
            .expect("the watch stays in custody");
        assert!(!watch.on_worker_thread());
        assert_eq!(watch.stop_and_join(), SqliteWatchJoin::Joined);
        assert!(watch.is_finished());
        assert_eq!(
            watch.stop_and_join(),
            SqliteWatchJoin::Joined,
            "a repeated stop reports the recorded join"
        );
    }

    #[test]
    fn dropping_the_watch_requests_the_stop_without_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("store.sqlite3");
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let (returned_tx, returned_rx) = mpsc::channel();
        let ticks = Arc::new(AtomicUsize::new(0));
        let probe = TickProbe {
            ticks: Arc::clone(&ticks),
            returned: returned_tx,
        };
        let watch =
            start_sqlite_change_watch(&db, "test-drop", Duration::from_millis(20), move |_| {
                probe.tick();
                let _ = entered_tx.send(());
                let _ = release_rx.recv();
                SqliteWatchControl::Handled
            })
            .expect("watch starts");
        entered_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("a tick is held within the hang guard");

        // Dropping never waits for the held tick: this thread releases the
        // tick only after the drop returned.
        drop(watch);
        release_tx.send(()).unwrap();
        returned_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the worker returns after the held tick");
        assert_eq!(
            ticks.load(Ordering::SeqCst),
            1,
            "no tick starts after the stop request"
        );
    }

    /// How long a held registration may keep a start from returning, or the
    /// sweep from ticking, before the test calls it held.
    const HELD_BOUND: Duration = Duration::from_secs(5);
    const HANG_GUARD: Duration = Duration::from_secs(10);

    #[test]
    fn a_held_registration_does_not_hold_the_start() {
        let dir = tempfile::tempdir().unwrap();
        let held = test_support::hold_registrations_in(dir.path()).unwrap();
        let db = dir.path().join("store.sqlite3");
        let (started_tx, started_rx) = mpsc::channel();
        let starter = thread::spawn(move || {
            let watch =
                start_sqlite_change_watch(&db, "test-held-start", SQLITE_WATCH_SWEEP, |_| {
                    SqliteWatchControl::Handled
                });
            let _ = started_tx.send(());
            watch
        });
        assert!(
            held.wait_entered(1, HANG_GUARD),
            "the registration reached its platform call"
        );
        let started = started_rx.recv_timeout(HELD_BOUND);
        held.release();
        let watch = starter.join().unwrap().expect("watch starts");
        assert_eq!(
            started,
            Ok(()),
            "the start waited for the held notification registration"
        );
        drop(watch);
    }

    #[test]
    fn the_sweep_ticks_while_the_registration_is_held() {
        let dir = tempfile::tempdir().unwrap();
        let held = test_support::hold_registrations_in(dir.path()).unwrap();
        let db = dir.path().join("store.sqlite3");
        let (tick_tx, tick_rx) = mpsc::channel();
        let starter = thread::spawn(move || {
            start_sqlite_change_watch(
                &db,
                "test-held-sweep",
                Duration::from_millis(20),
                move |tick| {
                    let _ = tick_tx.send(tick);
                    SqliteWatchControl::Handled
                },
            )
        });
        assert!(
            held.wait_entered(1, HANG_GUARD),
            "the registration reached its platform call"
        );
        let tick = tick_rx.recv_timeout(HELD_BOUND);
        held.release();
        let watch = starter.join().unwrap().expect("watch starts");
        assert_eq!(
            tick,
            Ok(SqliteWatchTick::Sweep),
            "no sweep ran while the notification registration was held"
        );
        drop(watch);
    }

    /// A source whose registration fails, as inotify's does past the user's
    /// watch limit.
    struct FailingBackend;

    struct FailingSource;

    impl WatchBackend for FailingBackend {
        fn create(
            &self,
            _targets: &WatchTargets,
            _wake: mpsc::Sender<()>,
        ) -> Result<Box<dyn WatchSource>, String> {
            Ok(Box::new(FailingSource))
        }
    }

    impl WatchSource for FailingSource {
        fn watch(&mut self, directory: &Path) -> Result<(), String> {
            Err(format!(
                "cannot watch {}: the watch limit is reached",
                directory.display()
            ))
        }
    }

    #[test]
    fn a_failed_registration_keeps_the_sweep() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("store.sqlite3");
        let (tick_tx, tick_rx) = mpsc::channel();
        let watch = start_with_backend(
            &db,
            "test-failed-registration",
            Duration::from_millis(20),
            move |tick| {
                let _ = tick_tx.send(tick);
                SqliteWatchControl::Handled
            },
            |_| Box::new(FailingBackend),
        )
        .expect("a failed notification registration leaves the watch on its sweep");
        assert_eq!(tick_rx.recv_timeout(HANG_GUARD), Ok(SqliteWatchTick::Sweep));
        drop(watch);
    }

    #[test]
    fn stopping_cancels_a_held_registration_and_keeps_its_thread() {
        let dir = tempfile::tempdir().unwrap();
        let held = test_support::hold_registrations_in(dir.path()).unwrap();
        let directory = dir.path().canonicalize().unwrap();
        let db = dir.path().join("store.sqlite3");
        let mut watch =
            start_sqlite_change_watch(&db, "test-held-stop", Duration::from_millis(20), |_| {
                SqliteWatchControl::Handled
            })
            .expect("watch starts");
        assert!(
            held.wait_entered(1, HANG_GUARD),
            "the registration reached its platform call"
        );

        // The stop joins the worker and never waits for the registration.
        assert_eq!(watch.stop_and_join(), SqliteWatchJoin::Joined);
        assert_eq!(watch.registration(), SqliteWatchRegistration::Pending);
        assert_eq!(held.live_sources(), 1);

        // Once the platform call returns, the registration discards its source
        // on its own thread instead of handing it to the stopped watch.
        held.release();
        assert_eq!(
            settled_registration(&watch),
            SqliteWatchRegistration::Cancelled
        );
        assert_eq!((held.live_sources(), held.dropped_sources()), (0, 1));
        // Its thread stayed with this watch, which joins it now.
        assert!(retained_in(&directory).is_empty());
        assert_eq!(watch.stop_and_join(), SqliteWatchJoin::Joined);
    }

    #[test]
    fn dropping_a_watch_retains_its_held_registration_until_it_returns() {
        let dir = tempfile::tempdir().unwrap();
        let held = test_support::hold_registrations_in(dir.path()).unwrap();
        let directory = dir.path().canonicalize().unwrap();
        let db = dir.path().join("store.sqlite3");
        let watch =
            start_sqlite_change_watch(&db, "test-held-drop", Duration::from_millis(20), |_| {
                SqliteWatchControl::Handled
            })
            .expect("watch starts");
        assert!(
            held.wait_entered(1, HANG_GUARD),
            "the registration reached its platform call"
        );

        // Dropping never waits for the registration and never detaches it:
        // the process keeps its thread, and reports it.
        drop(watch);
        let retained = retained_in(&directory);
        assert_eq!(retained.len(), 1, "{retained:?}");
        assert_eq!(retained[0].thread_name, "test-held-drop-register");
        assert_eq!(held.live_sources(), 1);

        // Once the platform call returns, the registration discards its
        // source, and the process joins its thread.
        held.release();
        let deadline = Instant::now() + HANG_GUARD;
        while !retained_in(&directory).is_empty() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
        assert!(
            retained_in(&directory).is_empty(),
            "the process joins a retained registration once it returns"
        );
        assert_eq!((held.live_sources(), held.dropped_sources()), (0, 1));
    }

    #[test]
    fn a_failed_registration_reports_its_reason() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("store.sqlite3");
        let watch = start_with_backend(
            &db,
            "test-failed-reason",
            SQLITE_WATCH_SWEEP,
            |_| SqliteWatchControl::Handled,
            |_| Box::new(FailingBackend),
        )
        .expect("watch starts");
        let registration = settled_registration(&watch);
        assert!(
            matches!(
                &registration,
                SqliteWatchRegistration::Failed(reason)
                    if reason.contains("the watch limit is reached")
            ),
            "{registration:?}"
        );
    }

    /// Set in the child process of the panicking-registration test.
    const PANIC_CHILD: &str = "MEERKAT_TEST_SQLITE_WATCH_PANIC_CHILD";
    /// What that child prints once its watch has survived.
    const PANIC_CHILD_DONE: &str = "SQLITE_WATCH_PANIC_CONTAINED";

    /// A source whose registration panics and whose drop panics again, as
    /// notify 8.2.0's inotify watcher does when its event-loop thread never
    /// started: dropping it while the first panic unwinds aborts the process.
    struct PanickingBackend;

    struct PanickingSource;

    impl WatchBackend for PanickingBackend {
        fn create(
            &self,
            _targets: &WatchTargets,
            _wake: mpsc::Sender<()>,
        ) -> Result<Box<dyn WatchSource>, String> {
            Ok(Box::new(PanickingSource))
        }
    }

    #[allow(clippy::panic)]
    impl WatchSource for PanickingSource {
        fn watch(&mut self, _directory: &Path) -> Result<(), String> {
            panic!("the notification event loop is gone");
        }
    }

    #[allow(clippy::panic)]
    impl Drop for PanickingSource {
        fn drop(&mut self) {
            panic!("the notification event loop is gone");
        }
    }

    /// The registration panics in a child process: a panic during a panic
    /// aborts the whole process, so only a child can show the watch survives.
    #[test]
    fn a_panicking_registration_leaves_the_sweep_running() {
        if std::env::var_os(PANIC_CHILD).is_some() {
            let dir = tempfile::tempdir().unwrap();
            let db = dir.path().join("store.sqlite3");
            let (tick_tx, tick_rx) = mpsc::channel();
            let watch = start_with_backend(
                &db,
                "test-panicking-registration",
                Duration::from_millis(20),
                move |tick| {
                    let _ = tick_tx.send(tick);
                    SqliteWatchControl::Handled
                },
                |_| Box::new(PanickingBackend),
            )
            .expect("watch starts");
            let registration = settled_registration(&watch);
            assert!(
                matches!(registration, SqliteWatchRegistration::Failed(_)),
                "{registration:?}"
            );
            assert_eq!(tick_rx.recv_timeout(HANG_GUARD), Ok(SqliteWatchTick::Sweep));
            drop(watch);
            println!("\n{PANIC_CHILD_DONE}");
            return;
        }
        #[allow(clippy::disallowed_methods)] // the test re-runs its own binary
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "watch::tests::a_panicking_registration_leaves_the_sweep_running",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(PANIC_CHILD, "1")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains(PANIC_CHILD_DONE),
            "the watch did not survive its panicking registration ({}):\n{stdout}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Set in the child process that runs one test of this module alone.
    const ALONE_CHILD: &str = "MEERKAT_TEST_SQLITE_WATCH_ALONE_CHILD";
    /// What that child prints once the test passed.
    const ALONE_CHILD_DONE: &str = "SQLITE_WATCH_ALONE_TEST_PASSED";

    /// Run `body`, the body of this module's test `test`, alone in a child
    /// process of this test binary, and require it to pass there. The
    /// registrations in flight are counted across the process, so a test
    /// that takes every place shares its process with no other watch test,
    /// whatever runs the tests: nextest's process per test, or plain `cargo
    /// test`'s concurrent threads in one process.
    fn alone_in_child_process(test: &str, body: impl FnOnce()) {
        if std::env::var_os(ALONE_CHILD).is_some() {
            body();
            println!("\n{ALONE_CHILD_DONE}");
            return;
        }
        #[allow(clippy::disallowed_methods)] // the test re-runs its own binary
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                &format!("watch::tests::{test}"),
                "--nocapture",
                "--test-threads=1",
            ])
            .env(ALONE_CHILD, "1")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains(ALONE_CHILD_DONE),
            "{test} failed alone in its child process ({}):\n{stdout}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn registrations_past_the_cap_leave_new_watches_on_their_sweep() {
        alone_in_child_process(
            "registrations_past_the_cap_leave_new_watches_on_their_sweep",
            || {
                // Alone in its process: no other registration holds a place.
                assert_eq!(REGISTRATIONS_IN_FLIGHT.load(Ordering::Acquire), 0);
                let dir = tempfile::tempdir().unwrap();
                let held = test_support::hold_registrations_in(dir.path()).unwrap();
                let watches: Vec<_> = (0..MAX_REGISTRATIONS_IN_FLIGHT)
                    .map(|index| {
                        start_sqlite_change_watch(
                            &dir.path().join(format!("store-{index}.sqlite3")),
                            "test-cap-held",
                            SQLITE_WATCH_SWEEP,
                            |_| SqliteWatchControl::Handled,
                        )
                        .expect("watch starts")
                    })
                    .collect();
                assert!(
                    held.wait_entered(MAX_REGISTRATIONS_IN_FLIGHT, HANG_GUARD),
                    "every registration under the cap reached its platform call"
                );

                // One more watch starts with no registration thread, on its sweep
                // alone.
                let (tick_tx, tick_rx) = mpsc::channel();
                let over = start_sqlite_change_watch(
                    &dir.path().join("store-over.sqlite3"),
                    "test-cap-over",
                    Duration::from_millis(20),
                    move |tick| {
                        let _ = tick_tx.send(tick);
                        SqliteWatchControl::Handled
                    },
                )
                .expect("a watch past the cap still starts");
                let registration = over.registration();
                assert!(
                    matches!(
                        &registration,
                        SqliteWatchRegistration::Failed(reason) if reason.contains("already pending")
                    ),
                    "{registration:?}"
                );
                assert!(
                    over.registration.thread.is_none(),
                    "no registration thread started past the cap"
                );
                assert_eq!(held.live_sources(), MAX_REGISTRATIONS_IN_FLIGHT);
                assert_eq!(tick_rx.recv_timeout(HANG_GUARD), Ok(SqliteWatchTick::Sweep));

                // Released, every held registration settles and gives its place back,
                // and a fresh watch registers again.
                held.release();
                for watch in &watches {
                    assert_eq!(
                        settled_registration(watch),
                        SqliteWatchRegistration::Registered
                    );
                }
                let deadline = Instant::now() + HANG_GUARD;
                while REGISTRATIONS_IN_FLIGHT.load(Ordering::Acquire) > 0
                    && Instant::now() < deadline
                {
                    thread::sleep(Duration::from_millis(2));
                }
                let fresh_dir = tempfile::tempdir().unwrap();
                let fresh = start_sqlite_change_watch(
                    &fresh_dir.path().join("store.sqlite3"),
                    "test-cap-fresh",
                    SQLITE_WATCH_SWEEP,
                    |_| SqliteWatchControl::Handled,
                )
                .expect("watch starts");
                assert_eq!(
                    settled_registration(&fresh),
                    SqliteWatchRegistration::Registered,
                    "a fresh watch registers once every place is given back"
                );
                drop(watches);
                drop(over);
            },
        );
    }

    /// A burst of 64 registrations held at once, as a parallel startup holds
    /// them while FSEvents stalls, all reach their platform call: none is
    /// refused at the cap and left on its sweep for life. The burst is
    /// written out, not taken from the cap, so a lower cap fails here.
    #[test]
    fn a_held_burst_of_64_registrations_all_reach_their_platform_call() {
        alone_in_child_process(
            "a_held_burst_of_64_registrations_all_reach_their_platform_call",
            || {
                const BURST: usize = 64;
                // Alone in its process: no other registration holds a place.
                assert_eq!(REGISTRATIONS_IN_FLIGHT.load(Ordering::Acquire), 0);
                let dir = tempfile::tempdir().unwrap();
                let held = test_support::hold_registrations_in(dir.path()).unwrap();
                let watches: Vec<_> = (0..BURST)
                    .map(|index| {
                        start_sqlite_change_watch(
                            &dir.path().join(format!("store-{index}.sqlite3")),
                            "test-burst-held",
                            SQLITE_WATCH_SWEEP,
                            |_| SqliteWatchControl::Handled,
                        )
                        .expect("watch starts")
                    })
                    .collect();
                let refused = watches
                    .iter()
                    .filter(|watch| {
                        matches!(watch.registration(), SqliteWatchRegistration::Failed(_))
                    })
                    .count();
                assert_eq!(refused, 0, "registrations of the burst refused at the cap");
                assert!(
                    held.wait_entered(BURST, HANG_GUARD),
                    "every registration of the burst reached its platform call"
                );
                held.release();
                for watch in &watches {
                    assert_eq!(
                        settled_registration(watch),
                        SqliteWatchRegistration::Registered
                    );
                }
            },
        );
    }

    /// Every WARN event this test process emitted, with the name of the
    /// thread that emitted it.
    static WARNINGS: Mutex<Vec<(Option<String>, String)>> = Mutex::new(Vec::new());

    /// A process-wide subscriber that records WARN events into [`WARNINGS`]: a
    /// registration settles on its own thread, which a thread-local
    /// subscriber would never see.
    struct WarningCapture;

    struct MessageField(String);

    impl tracing::field::Visit for MessageField {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            if field.name() == "message" {
                self.0 = format!("{value:?}");
            }
        }
    }

    impl tracing::Subscriber for WarningCapture {
        fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
            *metadata.level() == tracing::Level::WARN
        }

        fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }

        fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

        fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

        fn event(&self, event: &tracing::Event<'_>) {
            let mut message = MessageField(String::new());
            event.record(&mut message);
            lock(&WARNINGS).push((thread::current().name().map(str::to_string), message.0));
        }

        fn enter(&self, _span: &tracing::span::Id) {}

        fn exit(&self, _span: &tracing::span::Id) {}
    }

    /// Install [`WarningCapture`] as this process's global subscriber, once.
    fn capture_warnings() {
        static INSTALLED: OnceLock<bool> = OnceLock::new();
        let installed = *INSTALLED
            .get_or_init(|| tracing::subscriber::set_global_default(WarningCapture).is_ok());
        assert!(
            installed,
            "the warning capture is this process's global subscriber"
        );
    }

    fn warnings_from(thread_name: &str) -> Vec<String> {
        lock(&WARNINGS)
            .iter()
            .filter(|(name, _)| name.as_deref() == Some(thread_name))
            .map(|(_, message)| message.clone())
            .collect()
    }

    /// A source whose registration waits for its release, then fails, as an
    /// inotify watch past the user's limit does.
    struct GatedFailingBackend {
        entered: mpsc::Sender<()>,
        release: Mutex<Option<mpsc::Receiver<()>>>,
    }

    struct GatedFailingSource {
        entered: mpsc::Sender<()>,
        release: mpsc::Receiver<()>,
    }

    impl WatchBackend for GatedFailingBackend {
        fn create(
            &self,
            _targets: &WatchTargets,
            _wake: mpsc::Sender<()>,
        ) -> Result<Box<dyn WatchSource>, String> {
            let release = lock(&self.release)
                .take()
                .ok_or_else(|| "created twice".to_string())?;
            Ok(Box::new(GatedFailingSource {
                entered: self.entered.clone(),
                release,
            }))
        }
    }

    impl WatchSource for GatedFailingSource {
        fn watch(&mut self, directory: &Path) -> Result<(), String> {
            let _ = self.entered.send(());
            let _ = self.release.recv();
            Err(format!(
                "cannot watch {}: the watch limit is reached",
                directory.display()
            ))
        }
    }

    /// Start a watch whose registration fails once released. Returns the
    /// watch, the signal that its registration entered, and its release.
    fn start_failing_after_release(
        dir: &Path,
        name: &str,
    ) -> (SqliteChangeWatch, mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let backend = GatedFailingBackend {
            entered: entered_tx,
            release: Mutex::new(Some(release_rx)),
        };
        let watch = start_with_backend(
            &dir.join("store.sqlite3"),
            name,
            SQLITE_WATCH_SWEEP,
            |_| SqliteWatchControl::Handled,
            move |_| Box::new(backend),
        )
        .expect("watch starts");
        (watch, entered_rx, release_tx)
    }

    #[test]
    fn a_registration_failing_after_its_watch_stopped_is_cancelled_quietly() {
        capture_warnings();
        // A live watch whose registration fails reports it and warns once:
        // the capture sees registration threads.
        let live_dir = tempfile::tempdir().unwrap();
        let (live, live_entered, live_release) =
            start_failing_after_release(live_dir.path(), "test-fail-live");
        live_entered
            .recv_timeout(HANG_GUARD)
            .expect("the live registration reached its platform call");
        live_release.send(()).unwrap();
        assert!(matches!(
            settled_registration(&live),
            SqliteWatchRegistration::Failed(_)
        ));
        assert_eq!(warnings_from("test-fail-live-register").len(), 1);

        // A watch that stopped first is gone: its failed registration is
        // cancelled, with no warning about a sweep nobody runs any more.
        let stopped_dir = tempfile::tempdir().unwrap();
        let (mut stopped, stopped_entered, stopped_release) =
            start_failing_after_release(stopped_dir.path(), "test-fail-stopped");
        stopped_entered
            .recv_timeout(HANG_GUARD)
            .expect("the registration reached its platform call");
        assert_eq!(stopped.stop_and_join(), SqliteWatchJoin::Joined);
        stopped_release.send(()).unwrap();
        assert_eq!(
            (
                settled_registration(&stopped),
                warnings_from("test-fail-stopped-register")
            ),
            (SqliteWatchRegistration::Cancelled, Vec::<String>::new())
        );
    }
}

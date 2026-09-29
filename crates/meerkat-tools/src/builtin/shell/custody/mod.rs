//! Durable, incarnation-bound custody for owned shell tool process groups.
//!
//! The in-process [`super::process_lifecycle::OwnedProcessGroup`] guard only
//! contains a tool while the process that spawned it is alive. When the host
//! (gateway) is SIGKILLed, that guard dies with it and the tool's process
//! group keeps running. Custody closes that gap with a record that outlives
//! the host:
//!
//! 1. **Reserve** - before spawn, a record naming the scope, this host
//!    incarnation, and the host's own process identity and environment (boot,
//!    pid namespace) is written.
//! 2. **Gated spawn** - the tool starts in a fresh process group behind a
//!    spawn gate: a `/bin/sh` prologue blocks on a pipe whose only writer is
//!    the host. If the host dies before releasing the gate, the prologue reads
//!    EOF and exits without running the command. No process can execute the
//!    command without a record naming it.
//! 3. **Spawned** - the leader's pid and kernel start stamp (pid-reuse proof)
//!    are written, and only then is the gate released.
//! 4. **Settle** - after the in-process guard proves containment the record
//!    is removed, or, for a process spawned inside a run, replaced by an
//!    `Exited` marker kept until the run reaches a durable terminal
//!    ([`InterruptedToolEvidence::run_ended`]): until then a host crash
//!    would replay the run and repeat the tool, so the marker is the evidence
//!    that the run already executed it.
//! 5. **Recover** - a later incarnation opening the same scope must first
//!    settle every record left by earlier incarnations: verify the group's
//!    identity, SIGKILL it, and wait for every member's exit through kernel
//!    exit notification (pidfd on Linux, kqueue `EVFILT_PROC` on macOS).
//!    [`ProcessCustody`] handles can only be obtained through that recovery,
//!    so holding one is the proof that the scope's earlier tools have ceased.
//!
//! **Multi-process model.** Several host processes may share a custody root
//! (a realm served by more than one process, or a sweep in one process while
//! another serves a session). Every read-modify-write of a scope's records by
//! a recoverer (recovery, sweep, interrupted-run evidence) holds the scope's
//! settlement lock: an in-process mutex plus `flock(2)` on the scope
//! directory itself, re-validated against the directory's identity so a
//! directory removed and recreated meanwhile is re-locked. Hosts write and
//! remove only their own incarnation's records, without that lock, and a
//! recoverer never rewrites a record whose host is still running: host
//! liveness is decided first (pid plus kernel start stamp, which cannot be
//! reused), so a live host's record is left untouched and reported as
//! [`ProcessCustodyError::PriorIncarnationAlive`], and a record that host
//! removes is never resurrected.
//!
//! **Other pid namespaces.** A record from the same boot but another pid
//! namespace (a container restarted without a host reboot, or a sibling
//! container sharing the root) names pids that cannot be probed or signalled
//! from here. Liveness is instead proven through the kernel: every host
//! incarnation holds an exclusive `flock(2)` on
//! `<root>/.incarnations/<incarnation>.lock` for its whole lifetime (created
//! under a temporary name, locked, then renamed, so the file is never visible
//! unlocked), and flock works across pid namespaces on one kernel and
//! filesystem. Recovery takes that lock without blocking:
//!
//! - acquired: the kernel released the owner's lock, so the host process has
//!   exited. Its tools ran in its container, which a restart tears down with
//!   every process in its pid namespace, so the record is settled as
//!   [`ToolProcessCessation::ForeignIncarnationEnded`] without signalling;
//! - held: a live sibling host owns the record, which fails closed with
//!   [`ProcessCustodyError::ForeignPidNamespace`] (`Running`);
//! - lock file missing (a record written before incarnation locks), or the
//!   root on a network or userspace filesystem where flock is not a reliable
//!   proof: fail closed with `Unverifiable`.
//!
//! Lock files of ended incarnations that no unsettled record names any more
//! are removed by the realm sweep. Only a changed boot proves an environment
//! ended without a lock.
//!
//! **Durability.** Records must survive the death of the host *process*, and
//! the page cache already guarantees that: a file written and renamed by a
//! process that is then SIGKILLed stays visible to every later process. The
//! only extra step is one plain `fsync(2)` of the record data before the
//! rename (not `F_FULLFSYNC`, and no directory syncs), so a power loss can
//! leave the previous record or none, never a torn one. Whatever a power loss
//! does leave was written in a boot that has ended, and recovery settles it
//! as [`ToolProcessCessation::PriorEnvironmentEnded`] from its recorded boot
//! identity: every process of that boot is gone.
//!
//! **Classification.** Every observation that shows a pid cannot be one of
//! ours (absent, another user's, unreadable, a thread id, a stamp mismatch)
//! is a typed outcome, never an I/O error, so a reused pid can never block
//! recovery. Custody covers ordinary tools that stay in their process group
//! and run as the host's user. A process that leaves its group (`setsid`,
//! `setpgid`) is daemon containment and out of scope.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as sys;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as sys;

mod gate;

pub(super) use gate::SpawnGate;

use std::collections::HashMap;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::io::Write as _;
use std::os::fd::AsRawFd as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, OnceLock, PoisonError, Weak};
use std::time::{Duration, Instant};

use meerkat_core::tool_process::{
    InterruptedRunInputs, InterruptedToolCall, InterruptedToolEvidence,
    InterruptedToolEvidenceError, InterruptedToolSettlement,
};
use meerkat_core::types::SessionId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use super::custody_types::{
    ForeignIncarnationLiveness, ProcessCustodyError, ProcessCustodyRecoveryReport,
    ProcessCustodySweepReport, RecoveredToolProcess, ScopeSweep, ToolProcessCessation,
    ToolProcessSpawner,
};

/// Directory, under a realm runtime root, that holds custody records.
pub const PROCESS_CUSTODY_DIR: &str = "tool_process_custody";

/// Directory, under a custody root, of per-incarnation liveness locks. Its
/// name is not a valid scope, so sweeps never treat it as a session.
const INCARNATIONS_DIR: &str = ".incarnations";
const LOCK_EXTENSION: &str = "lock";

const RECORD_EXTENSION: &str = "json";
const TEMP_EXTENSION: &str = "tmp";
/// Custody record format. The envelope fields of [`RecordEnvelope`] are
/// frozen across versions so an older build can still recognise a newer
/// record's owner and environment.
const RECORD_VERSION: u32 = 1;
/// Upper bound on waiting for a killed group's exit notifications. SIGKILL
/// cannot be ignored, so this only elapses for a process stuck in an
/// uninterruptible kernel wait; recovery then fails closed.
const CESSATION_DEADLINE: Duration = Duration::from_secs(30);

/// Kernel start stamp of one process. Together with the pid it defeats pid
/// reuse: a recycled pid never carries the same stamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProcessStartStamp {
    /// Linux: boot id plus start time in clock ticks since that boot.
    LinuxBoot { boot_id: Uuid, start_ticks: u64 },
    /// macOS: absolute start time.
    Darwin { start_sec: u64, start_usec: u64 },
}

impl ProcessStartStamp {
    /// Whether `self` started no later than `other`. `None` when the stamps
    /// come from different boots (or platforms) and so cannot be ordered.
    fn not_after(&self, other: &Self) -> Option<bool> {
        match (self, other) {
            (
                Self::LinuxBoot {
                    boot_id: a,
                    start_ticks: ta,
                },
                Self::LinuxBoot {
                    boot_id: b,
                    start_ticks: tb,
                },
            ) if a == b => Some(ta <= tb),
            (
                Self::Darwin {
                    start_sec: sa,
                    start_usec: ua,
                },
                Self::Darwin {
                    start_sec: sb,
                    start_usec: ub,
                },
            ) => Some((sa, ua) <= (sb, ub)),
            _ => None,
        }
    }
}

/// A process named by pid plus kernel start stamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub pid: i32,
    pub start: ProcessStartStamp,
}

impl ProcessIdentity {
    /// Identity of a process of ours; `None` when `pid` is absent or not
    /// ours to observe.
    fn capture(pid: i32) -> std::io::Result<Option<Self>> {
        Ok(match sys::probe(pid)? {
            ProcessProbe::Observed(snapshot) => Some(Self {
                pid,
                start: snapshot.start,
            }),
            ProcessProbe::Absent | ProcessProbe::Foreign => None,
        })
    }

    /// True only when this exact process (pid and start stamp) has not
    /// exited. Absent, foreign, and reused pids are not running.
    fn is_running(&self) -> std::io::Result<bool> {
        sys::is_running(self)
    }
}

/// The boot and pid namespace a process lives in. Pids, sessions and start
/// stamps are only comparable between processes of one environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum HostEnvironment {
    Linux {
        boot_id: Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pid_namespace_dev: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pid_namespace_ino: Option<u64>,
    },
    /// `boot_session` is absent in records written before boot identity was
    /// recorded; such records are handled as an unknown boot.
    Darwin {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        boot_session: Option<Uuid>,
    },
}

/// How a recorded environment relates to the current one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EnvironmentRelation {
    Same,
    /// Provably another boot (or another operating system): everything
    /// recorded there has ended, and its pids name nothing here.
    Ended,
    /// Not provably the same or different (an identity was not recorded).
    /// Recovery proceeds with per-process checks, which classify reused and
    /// foreign pids safely.
    Unknown,
    /// The same boot but another pid namespace: its pids name nothing
    /// observable here, and nothing proves its processes ended.
    ForeignNamespace,
}

impl HostEnvironment {
    fn relation_to(&self, current: &Self) -> EnvironmentRelation {
        match (self, current) {
            (
                Self::Linux {
                    boot_id: recorded_boot,
                    pid_namespace_dev: recorded_dev,
                    pid_namespace_ino: recorded_ino,
                },
                Self::Linux {
                    boot_id: current_boot,
                    pid_namespace_dev: current_dev,
                    pid_namespace_ino: current_ino,
                },
            ) => {
                if recorded_boot != current_boot {
                    return EnvironmentRelation::Ended;
                }
                match (recorded_dev, recorded_ino, current_dev, current_ino) {
                    (Some(rd), Some(ri), Some(cd), Some(ci)) if (rd, ri) == (cd, ci) => {
                        EnvironmentRelation::Same
                    }
                    (Some(_), Some(_), Some(_), Some(_)) => EnvironmentRelation::ForeignNamespace,
                    _ => EnvironmentRelation::Unknown,
                }
            }
            (
                Self::Darwin {
                    boot_session: Some(recorded),
                },
                Self::Darwin {
                    boot_session: Some(current),
                },
            ) => {
                if recorded == current {
                    EnvironmentRelation::Same
                } else {
                    EnvironmentRelation::Ended
                }
            }
            (Self::Darwin { .. }, Self::Darwin { .. }) => EnvironmentRelation::Unknown,
            // A record from another operating system cannot name a process
            // of this host.
            _ => EnvironmentRelation::Ended,
        }
    }
}

/// What a kernel probe of one pid established.
enum ProcessProbe {
    Absent,
    /// The pid names a process that cannot be ours: another user's, or one we
    /// are not permitted to inspect.
    Foreign,
    Observed(ProcessSnapshot),
}

/// Point-in-time kernel facts about one process of ours. Zombies are
/// included: only kernel exit notification decides that a member ceased.
struct ProcessSnapshot {
    pgid: i32,
    start: ProcessStartStamp,
}

enum ExitWaitOutcome {
    AllExited,
    DeadlineElapsed,
}

/// One lifetime of this host process. Custody records written by the current
/// incarnation are live-owned by in-process guards and are never recovered.
#[derive(Debug)]
struct CustodyIncarnation {
    id: Uuid,
    host: ProcessIdentity,
    environment: HostEnvironment,
}

static INCARNATION: OnceLock<CustodyIncarnation> = OnceLock::new();

fn incarnation() -> Result<&'static CustodyIncarnation, ProcessCustodyError> {
    if let Some(incarnation) = INCARNATION.get() {
        return Ok(incarnation);
    }
    let pid = i32::try_from(std::process::id()).map_err(|_| {
        ProcessCustodyError::io(
            "capture host process identity",
            std::io::Error::other("host pid out of range"),
        )
    })?;
    let host = ProcessIdentity::capture(pid)
        .map_err(|error| ProcessCustodyError::io("capture host process identity", error))?
        .ok_or_else(|| {
            ProcessCustodyError::io(
                "capture host process identity",
                std::io::Error::from(std::io::ErrorKind::NotFound),
            )
        })?;
    let environment = sys::host_environment()
        .map_err(|error| ProcessCustodyError::io("capture host environment", error))?;
    Ok(INCARNATION.get_or_init(|| CustodyIncarnation {
        id: Uuid::new_v4(),
        host,
        environment,
    }))
}

/// Process groups this incarnation spawned and has not yet proven exited,
/// across every scope and spawner (custody-bound shell calls, background
/// shell jobs, command hooks), keyed by group id with the leader's start
/// stamp when it was captured.
///
/// A recorded group id that names one of them cannot be an earlier
/// incarnation's group: group ids are unique while a group exists. This
/// guards recovery when earlier and current tools share a session (for
/// example a supervisor-inherited session), where start-time and session
/// checks alone cannot tell incarnations apart. Entries are released once
/// the group is proven exited; an entry whose leader started before the
/// recorded leader is stale and ignored.
static LIVE_GROUPS: Mutex<BTreeMap<i32, Option<ProcessStartStamp>>> = Mutex::new(BTreeMap::new());

/// Upper bound on one blocking exit wait of a release watcher. The watcher
/// re-lists and waits again; the bound only limits how long a stale listing
/// is trusted, never whether a group is considered exited.
const RELEASE_WATCH_ROTATION: Duration = Duration::from_secs(600);

fn live_groups() -> std::sync::MutexGuard<'static, BTreeMap<i32, Option<ProcessStartStamp>>> {
    LIVE_GROUPS.lock().unwrap_or_else(PoisonError::into_inner)
}

fn register_live_group(pgid: i32, leader_start: Option<ProcessStartStamp>) {
    live_groups().insert(pgid, leader_start);
}

/// Release `pgid` only if the entry still belongs to the same leader, so a
/// newer group that reused the id stays registered.
fn release_live_group(pgid: i32, leader_start: Option<ProcessStartStamp>) {
    let mut groups = live_groups();
    if groups.get(&pgid) == Some(&leader_start) {
        groups.remove(&pgid);
    }
}

/// Whether a live registered group holds `pgid` and supersedes a recorded
/// leader with `recorded_start`. An entry without a captured leader stamp is
/// trusted conservatively; an entry whose leader started before the recorded
/// leader is stale (that group ended before the recorded one began).
fn live_group_supersedes(pgid: i32, recorded_start: &ProcessStartStamp) -> bool {
    match live_groups().get(&pgid) {
        None => false,
        Some(None) => true,
        Some(Some(registered)) => recorded_start.not_after(registered) == Some(true),
    }
}

/// Keep `pgid` registered until every member of the group (started no
/// earlier than its leader) has exited, observed through kernel exit
/// notification, then release it. Runs on its own thread because a group may
/// live as long as its tool. If the group cannot be observed the entry is
/// kept; its leader stamp lets later recoveries recognise it as stale.
/// A custody record handed to a release watcher, settled once its group is
/// proven exited.
struct WatchedRecord {
    dir: PathBuf,
    path: PathBuf,
    record: CustodyRecord,
}

fn release_when_group_exits(
    pgid: i32,
    leader_start: Option<ProcessStartStamp>,
    record: Option<WatchedRecord>,
) {
    let spawned = std::thread::Builder::new()
        .name("meerkat-custody-group-watch".to_owned())
        .spawn(move || {
            if let Err(error) = watch_until_group_exits(pgid, leader_start, record.as_ref()) {
                // Only a group that cannot be observed keeps its entry; its
                // leader stamp lets later recoveries recognise it as stale.
                tracing::warn!(
                    %error,
                    pgid,
                    "custody group release watcher cannot observe the group; entry kept"
                );
            }
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, pgid, "could not start a custody group release watcher");
    }
}

/// Wait on kernel exit notification (pidfd on Linux, kqueue `EVFILT_PROC` on
/// macOS) for every member of the group started no earlier than its leader,
/// re-listing after each round, then release the entry.
fn watch_until_group_exits(
    pgid: i32,
    leader_start: Option<ProcessStartStamp>,
    record: Option<&WatchedRecord>,
) -> std::io::Result<()> {
    loop {
        let mut live = Vec::new();
        for member in observed_members(pgid)? {
            let descends =
                leader_start.is_none_or(|leader| leader.not_after(&member.start) == Some(true));
            if descends && member.is_running()? {
                live.push(member);
            }
        }
        if live.is_empty() {
            // Proven exited: the record, if any, has no process left to
            // guard.
            if let Some(watched) = record {
                finish_record_blocking(&watched.dir, &watched.path, &watched.record, true)?;
            }
            release_live_group(pgid, leader_start);
            return Ok(());
        }
        sys::ExitWatch::new(&live)?.wait_all(Instant::now() + RELEASE_WATCH_ROTATION)?;
    }
}

/// Register a process group this incarnation just spawned (with
/// `process_group(0)`, so the group id equals `leader_pid`) as live until it
/// is proven exited. Recovery never treats a registered live group as an
/// earlier incarnation's tool. Call right after spawning, before the leader
/// is reaped. For spawners outside custody: background shell jobs, command
/// hooks.
pub fn track_owned_process_group(leader_pid: i32) {
    let leader_start = match sys::probe(leader_pid) {
        Ok(ProcessProbe::Observed(snapshot)) => Some(snapshot.start),
        // Already gone or unreadable: track by id alone.
        Ok(ProcessProbe::Absent | ProcessProbe::Foreign) | Err(_) => None,
    };
    register_live_group(leader_pid, leader_start);
    release_when_group_exits(leader_pid, leader_start, None);
}

/// Per-scope recovery locks, so recoveries of one scope in this process
/// (a session build and a realm sweep, or two builds) never interleave.
static SCOPE_LOCKS: Mutex<BTreeMap<PathBuf, Arc<Mutex<()>>>> = Mutex::new(BTreeMap::new());

fn scope_lock(dir: &Path) -> Arc<Mutex<()>> {
    Arc::clone(
        SCOPE_LOCKS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(dir.to_path_buf())
            .or_default(),
    )
}

/// Name of a scope's settlement lock file, inside the scope directory. It is
/// neither a record nor a temporary file, so listings ignore it.
const SCOPE_LOCK_FILE: &str = ".lock";

/// Take the cross-process half of a scope's settlement lock: `flock(2)` on
/// the scope's lock file, opened for writing so the lock also works where
/// flock is emulated with POSIX locks (NFS). Callers hold the in-process
/// [`scope_lock`] first. Returns `None` when the scope directory does not
/// exist (the scope holds no records). The lock is re-validated against the
/// lock file's identity, so a scope removed (and possibly recreated) while
/// this caller waited is locked afresh.
fn lock_scope_dir(dir: &Path) -> std::io::Result<Option<nix::fcntl::Flock<std::fs::File>>> {
    let path = dir.join(SCOPE_LOCK_FILE);
    loop {
        let mut file = match std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
        {
            Ok(file) => file,
            // The scope directory does not exist: nothing to settle.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let locked = loop {
            match nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusive) {
                Ok(locked) => break locked,
                Err((returned, nix::errno::Errno::EINTR)) => file = returned,
                Err((_, errno)) => return Err(std::io::Error::from(errno)),
            }
        };
        match scope_lock_is_current(&path, &locked) {
            Ok(true) => return Ok(Some(locked)),
            // Replaced while we waited: lock the current file.
            Ok(false) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if !dir.exists() {
                    return Ok(None);
                }
            }
            Err(error) => return Err(error),
        }
    }
}

/// Whether the locked file is still the one at `path`.
fn scope_lock_is_current(path: &Path, locked: &std::fs::File) -> std::io::Result<bool> {
    use std::os::unix::fs::MetadataExt as _;
    let held = locked.metadata()?;
    let current = std::fs::metadata(path)?;
    Ok(current.dev() == held.dev() && current.ino() == held.ino())
}

/// Remove a scope directory that holds nothing but its lock file. Call while
/// holding the scope's settlement lock: waiters on the removed lock file
/// re-validate and find the scope gone. A concurrent reservation recreates
/// the directory (see `write_record_blocking`).
fn remove_scope_dir_if_empty(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let only_lock = entries
        .filter_map(Result::ok)
        .all(|entry| entry.file_name() == SCOPE_LOCK_FILE);
    if only_lock {
        let _ = std::fs::remove_file(dir.join(SCOPE_LOCK_FILE));
        let _ = std::fs::remove_dir(dir);
    }
}

/// Live custody-bound spawns inside runs, per scope directory and run, and
/// whether that run has already ended (reached a durable terminal). A spawn
/// that finishes after its run ended leaves no `Exited` marker; entries only
/// exist while such spawns are live.
#[derive(Debug, Default)]
struct RunSpawns {
    live: usize,
    ended: bool,
}

type RunSpawnKey = (PathBuf, meerkat_core::RunId);

static RUN_SPAWNS: LazyLock<Mutex<HashMap<RunSpawnKey, RunSpawns>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn run_spawns() -> std::sync::MutexGuard<'static, HashMap<RunSpawnKey, RunSpawns>> {
    RUN_SPAWNS.lock().unwrap_or_else(PoisonError::into_inner)
}

fn run_spawn_started(dir: &Path, run_id: &meerkat_core::RunId) {
    run_spawns()
        .entry((dir.to_path_buf(), run_id.clone()))
        .or_default()
        .live += 1;
}

/// One spawn of `run_id` finished; returns whether the run already ended.
fn run_spawn_finished(dir: &Path, run_id: &meerkat_core::RunId) -> bool {
    let mut spawns = run_spawns();
    let key = (dir.to_path_buf(), run_id.clone());
    let Some(entry) = spawns.get_mut(&key) else {
        return false;
    };
    let ended = entry.ended;
    entry.live = entry.live.saturating_sub(1);
    if entry.live == 0 {
        spawns.remove(&key);
    }
    ended
}

/// The owner proved the record's process group exited (or the command never
/// ran, when `ran` is false). Remove the record, or keep an `Exited` marker
/// for a process that ran inside a run that has not ended yet. Holds the
/// in-process scope lock so it serializes with
/// [`InterruptedToolEvidence::run_ended`].
fn finish_record_blocking(
    dir: &Path,
    path: &Path,
    record: &CustodyRecord,
    ran: bool,
) -> std::io::Result<()> {
    let lock = scope_lock(dir);
    let _serialized = lock.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(run_id) = record.run_id.as_ref() else {
        return remove_record(path);
    };
    let ended = run_spawn_finished(dir, run_id);
    if ended || !ran {
        return remove_record(path);
    }
    let mut marker = record.clone();
    marker.phase = CustodyPhase::Exited;
    let bytes = serde_json::to_vec(&marker).map_err(std::io::Error::other)?;
    let temp = temp_path(dir, marker.entry_id, marker.incarnation);
    write_record_blocking(dir, path, &temp, &bytes)
}

/// Incarnation locks this process holds, per custody root, for its lifetime.
static HELD_INCARNATION_LOCKS: Mutex<BTreeMap<PathBuf, nix::fcntl::Flock<std::fs::File>>> =
    Mutex::new(BTreeMap::new());

fn incarnation_lock_path(root: &Path, incarnation: Uuid) -> PathBuf {
    root.join(INCARNATIONS_DIR)
        .join(format!("{incarnation}.{LOCK_EXTENSION}"))
}

/// Hold this incarnation's liveness lock under `root` for the rest of the
/// process lifetime. The file is created and locked under a temporary name,
/// then renamed into place, so a visible lock file is always locked by its
/// owner from the moment it appears.
fn hold_incarnation_lock(root: &Path, incarnation: &CustodyIncarnation) -> std::io::Result<()> {
    let mut held = HELD_INCARNATION_LOCKS
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if held.contains_key(root) {
        return Ok(());
    }
    let dir = root.join(INCARNATIONS_DIR);
    let path = incarnation_lock_path(root, incarnation.id);
    let temp = dir.join(format!(
        "{}.{LOCK_EXTENSION}.{TEMP_EXTENSION}",
        incarnation.id
    ));
    loop {
        std::fs::create_dir_all(&dir)?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temp)?;
        // Blocking: a concurrent sweep may hold (and is about to reap) the
        // not yet locked temporary file; the rename below then retries.
        let mut file = file;
        let locked = loop {
            match nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusive) {
                Ok(locked) => break locked,
                Err((returned, nix::errno::Errno::EINTR)) => file = returned,
                Err((_, errno)) => return Err(std::io::Error::from(errno)),
            }
        };
        match std::fs::rename(&temp, &path) {
            Ok(()) => {
                held.insert(root.to_path_buf(), locked);
                return Ok(());
            }
            // A sweep reaped the temporary file before it was locked: retry.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
}

/// What an incarnation's liveness lock shows.
enum IncarnationLock {
    /// The lock was free: its owner has exited (the kernel releases a flock
    /// only when every descriptor of it is closed).
    Released,
    /// A live process holds it.
    Held,
    /// No lock can prove anything: the file is missing (written before
    /// incarnation locks), or `root` is on a filesystem where flock is not a
    /// reliable cross-process proof.
    Unprovable,
}

fn foreign_incarnation_lock(root: &Path, incarnation: Uuid) -> std::io::Result<IncarnationLock> {
    if !sys::lock_filesystem_is_local(root)? {
        return Ok(IncarnationLock::Unprovable);
    }
    try_incarnation_lock(&incarnation_lock_path(root, incarnation))
        .map(|lock| match lock {
            Some(_released) => IncarnationLock::Released,
            None => IncarnationLock::Held,
        })
        .or_else(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Ok(IncarnationLock::Unprovable)
            } else {
                Err(error)
            }
        })
}

/// Take a lock file without blocking: `Some` when it was free (the caller
/// now holds it), `None` when another holder has it.
fn try_incarnation_lock(path: &Path) -> std::io::Result<Option<nix::fcntl::Flock<std::fs::File>>> {
    // Opened for writing, so the lock also works where flock is emulated
    // with POSIX locks.
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    loop {
        match nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusiveNonblock) {
            Ok(locked) => return Ok(Some(locked)),
            Err((_, nix::errno::Errno::EWOULDBLOCK)) => return Ok(None),
            Err((returned, nix::errno::Errno::EINTR)) => file = returned,
            Err((_, errno)) => return Err(std::io::Error::from(errno)),
        }
    }
}

/// Remove the lock files (and interrupted lock creations) of incarnations
/// that have ended and that no unsettled record under `root` names any more.
/// Runs after a sweep settled every scope it could.
fn reap_ended_incarnation_locks(root: &Path, current: Uuid) -> std::io::Result<()> {
    let dir = root.join(INCARNATIONS_DIR);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let named = incarnations_named_by_unsettled_records(root)?;
    for entry in entries {
        let path = entry?.path();
        let Some(owner) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.split('.').next())
            .and_then(|stem| Uuid::parse_str(stem).ok())
        else {
            continue;
        };
        if owner == current || named.contains(&owner) {
            continue;
        }
        match try_incarnation_lock(&path) {
            // Held while removed: no one can be waiting on an ended owner.
            Ok(Some(_ended)) => remove_record(&path)?,
            Ok(None) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Incarnations named by records under `root` that still need their owner's
/// liveness decided (every phase but settled interrupted-run evidence).
fn incarnations_named_by_unsettled_records(root: &Path) -> std::io::Result<BTreeSet<Uuid>> {
    let mut named = BTreeSet::new();
    for scope in std::fs::read_dir(root)? {
        let scope = scope?.path();
        if !scope.is_dir() {
            continue;
        }
        let records = match std::fs::read_dir(&scope) {
            Ok(records) => records,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for record in records {
            let path = record?.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some(RECORD_EXTENSION) {
                continue;
            }
            match read_listed_record(&path) {
                Ok(ListedRecord::Current(record))
                    if matches!(record.phase, CustodyPhase::Interrupted { .. }) => {}
                Ok(ListedRecord::Current(record)) => {
                    named.insert(record.incarnation);
                }
                Ok(ListedRecord::OtherVersion(envelope)) => {
                    named.insert(envelope.incarnation);
                }
                Ok(ListedRecord::Gone) => {}
                // An unreadable record may name anyone: keep every lock.
                Err(error) => return Err(std::io::Error::other(error.to_string())),
            }
        }
    }
    Ok(named)
}

/// Custody roots already swept by this process.
static SWEPT_ROOTS: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

/// Settle every scope under `root` on a background thread, once per root per
/// process. This covers sessions that are never resumed: their
/// earlier-incarnation tools are killed (or proven gone) with the same fence
/// and typed outcomes as a session build. Scopes a live host still serves
/// are left for that host. Outcomes are logged; use
/// [`ProcessCustody::sweep`] to obtain the typed report.
pub fn sweep_process_custody_once(root: PathBuf) {
    if !SWEPT_ROOTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(root.clone())
    {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("meerkat-custody-sweep".to_owned())
        .spawn(move || match sweep_blocking(&root) {
            Ok(report) => log_sweep(&report),
            Err(error) => {
                tracing::warn!(%error, "process custody sweep could not list its root");
            }
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "could not start the process custody sweep");
    }
}

fn log_sweep(report: &ProcessCustodySweepReport) {
    for scope in &report.scopes {
        match &scope.outcome {
            Ok(settled) => {
                for recovered in &settled.recovered {
                    tracing::warn!(
                        scope = %scope.scope,
                        entry_id = %recovered.entry_id,
                        prior_incarnation = %recovered.prior_incarnation,
                        tool_call_id = ?recovered.tool_call_id,
                        spawner = ?recovered.spawner,
                        cessation = ?recovered.cessation,
                        "custody sweep settled a process left by a prior host incarnation"
                    );
                }
            }
            Err(error) => tracing::info!(
                scope = %scope.scope,
                %error,
                "custody sweep left a scope for its session build"
            ),
        }
    }
}

fn sweep_blocking(root: &Path) -> Result<ProcessCustodySweepReport, ProcessCustodyError> {
    let incarnation = incarnation()?;
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ProcessCustodySweepReport::default());
        }
        Err(error) => return Err(ProcessCustodyError::io("list custody scopes", error)),
    };
    let mut scopes = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| ProcessCustodyError::io("list custody scopes", error))?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let scope = ProcessCustodyScope(name);
        if scope.validate().is_err() || !entry.path().is_dir() {
            continue;
        }
        scopes.push(scope);
    }
    scopes.sort_by(|a, b| a.0.cmp(&b.0));
    let mut report = ProcessCustodySweepReport::default();
    for scope in scopes {
        let dir = root.join(scope.as_str());
        let outcome =
            recover_scope_blocking(&dir, incarnation, Instant::now() + CESSATION_DEADLINE);
        report.scopes.push(ScopeSweep {
            scope: scope.0,
            outcome,
        });
    }
    if let Err(error) = reap_ended_incarnation_locks(root, incarnation.id) {
        tracing::warn!(%error, "custody sweep could not reap ended incarnation locks");
    }
    Ok(report)
}

/// The unit whose earlier tools must cease before new work is admitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessCustodyScope(String);

impl ProcessCustodyScope {
    /// Custody scoped to one session.
    pub fn session(session_id: &SessionId) -> Self {
        Self(session_id.to_string())
    }

    fn validate(&self) -> Result<(), ProcessCustodyError> {
        let valid = !self.0.is_empty()
            && self.0.len() <= 128
            && self
                .0
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
        if valid {
            Ok(())
        } else {
            Err(ProcessCustodyError::InvalidScope(self.0.clone()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
enum CustodyPhase {
    /// Recorded before spawn. A process may exist, but it is held at the
    /// spawn gate and cannot run the command unless its host releases it.
    Reserved,
    /// Leader identity recorded; the gate may have been released. The
    /// process group id equals the leader pid.
    Spawned {
        leader: ProcessIdentity,
        /// Session id of the leader at spawn; every group member inherits it.
        session_leader: i32,
    },
    /// Settled by a later incarnation's recovery while its run may still have
    /// been in flight. Kept as durable interrupted-run evidence until the
    /// runtime settles the run's inputs and tells the model; no process is
    /// associated with it any more.
    Interrupted {
        cessation: ToolProcessCessation,
        settlement: InterruptedToolSettlement,
    },
    /// The process, spawned inside a run, exited and its owner proved the
    /// whole group gone, but the run has not ended yet. Kept as evidence
    /// that the run already executed the tool until the run reaches a
    /// durable terminal; no process is associated with it any more.
    Exited,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CustodyRecord {
    version: u32,
    entry_id: Uuid,
    scope: String,
    incarnation: Uuid,
    host: ProcessIdentity,
    environment: HostEnvironment,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(default)]
    spawner: ToolProcessSpawner,
    /// The run the process was spawned in, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    run_id: Option<meerkat_core::RunId>,
    #[serde(flatten)]
    phase: CustodyPhase,
}

/// Fields every record version carries with this meaning. Read first, so a
/// record of an unknown version can still be matched to its owner and
/// environment.
#[derive(Debug, Deserialize)]
struct RecordEnvelope {
    version: u32,
    entry_id: Uuid,
    incarnation: Uuid,
    #[serde(default)]
    tool_call_id: Option<String>,
    #[serde(default)]
    environment: Option<serde_json::Value>,
}

/// Custody authority for one scope in the current host incarnation.
///
/// The only constructor is [`ProcessCustody::recover_and_open`], which first
/// settles every earlier-incarnation record for the scope.
#[derive(Debug)]
pub struct ProcessCustody {
    dir: PathBuf,
    scope: ProcessCustodyScope,
    incarnation: &'static CustodyIncarnation,
}

/// Custody handles this process has open, per scope directory. While one is
/// alive its scope was already recovered by this incarnation, and every later
/// record in it is this incarnation's own, so opening the scope again reuses
/// the handle instead of recovering again.
static OPEN_SCOPES: Mutex<BTreeMap<PathBuf, Weak<ProcessCustody>>> = Mutex::new(BTreeMap::new());

fn open_scope(dir: &Path) -> Option<Arc<ProcessCustody>> {
    let mut open = OPEN_SCOPES.lock().unwrap_or_else(PoisonError::into_inner);
    open.retain(|_, custody| custody.strong_count() > 0);
    open.get(dir).and_then(Weak::upgrade)
}

impl ProcessCustody {
    /// Settle every earlier-incarnation custody record for `scope` under
    /// `root`, then return the scope's custody handle.
    ///
    /// Earlier tools are proven stopped (or never started, or provably gone)
    /// before this returns `Ok`; any doubt is a typed error and yields no
    /// handle. See [`ProcessCustodyError`] for the operator action per error.
    /// A scope this process already holds open is not recovered again: the
    /// open handle is returned with an empty report.
    pub async fn recover_and_open(
        root: &Path,
        scope: ProcessCustodyScope,
    ) -> Result<(Arc<Self>, ProcessCustodyRecoveryReport), ProcessCustodyError> {
        scope.validate()?;
        let incarnation = incarnation()?;
        let dir = root.join(scope.as_str());
        if let Some(open) = open_scope(&dir) {
            return Ok((open, ProcessCustodyRecoveryReport::default()));
        }
        // Held before any record of this incarnation can exist under `root`,
        // so other pid namespaces can prove whether this host still runs.
        let lock_root = root.to_path_buf();
        tokio::task::spawn_blocking(move || hold_incarnation_lock(&lock_root, incarnation))
            .await
            .map_err(|error| {
                ProcessCustodyError::io("join incarnation lock", std::io::Error::other(error))
            })?
            .map_err(|error| ProcessCustodyError::io("hold incarnation lock", error))?;
        let recovery_dir = dir.clone();
        let report = tokio::task::spawn_blocking(move || {
            recover_scope_blocking(
                &recovery_dir,
                incarnation,
                Instant::now() + CESSATION_DEADLINE,
            )
        })
        .await
        .map_err(|error| {
            ProcessCustodyError::io("join custody recovery", std::io::Error::other(error))
        })??;
        let custody = Arc::new(Self {
            dir: dir.clone(),
            scope,
            incarnation,
        });
        OPEN_SCOPES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(dir, Arc::downgrade(&custody));
        Ok((custody, report))
    }

    pub fn scope(&self) -> &ProcessCustodyScope {
        &self.scope
    }

    /// Settle every scope under `root` (a realm's custody root), including
    /// sessions that are never resumed. Only listing `root` itself can fail;
    /// each scope carries its own outcome.
    pub async fn sweep(root: &Path) -> Result<ProcessCustodySweepReport, ProcessCustodyError> {
        let root = root.to_path_buf();
        tokio::task::spawn_blocking(move || sweep_blocking(&root))
            .await
            .map_err(|error| {
                ProcessCustodyError::io("join custody sweep", std::io::Error::other(error))
            })?
    }

    /// Reserve custody for a process that will run `program args...`, and
    /// return the gated command to configure and spawn. The command cannot
    /// run until [`PreparedCustodySpawn::spawned`] records the spawned
    /// leader and releases the gate; if that never happens (the host dies,
    /// or the caller drops the preparation) the command never runs.
    ///
    /// The returned command already runs in a fresh process group; callers
    /// must not change the process group, and should set `kill_on_drop`.
    pub async fn prepare_spawn(
        self: &Arc<Self>,
        spawner: ToolProcessSpawner,
        tool_call_id: Option<&str>,
        run_id: Option<&meerkat_core::RunId>,
        program: &OsStr,
        args: &[OsString],
    ) -> Result<(PreparedCustodySpawn, tokio::process::Command), ProcessCustodyError> {
        let reservation = self.reserve(spawner, tool_call_id, run_id).await?;
        let gate = SpawnGate::new(reservation.entry_id())
            .map_err(|error| ProcessCustodyError::io("create spawn gate", error))?;
        let mut command = gate
            .command_argv(program, args)
            .map_err(|error| ProcessCustodyError::io("build gated command", error))?;
        command.process_group(0);
        Ok((PreparedCustodySpawn { reservation, gate }, command))
    }

    /// Reserve custody for one tool process before it is spawned.
    pub(super) async fn reserve(
        self: &Arc<Self>,
        spawner: ToolProcessSpawner,
        tool_call_id: Option<&str>,
        run_id: Option<&meerkat_core::RunId>,
    ) -> Result<CustodyReservation, ProcessCustodyError> {
        let record = CustodyRecord {
            version: RECORD_VERSION,
            entry_id: Uuid::new_v4(),
            scope: self.scope.as_str().to_owned(),
            incarnation: self.incarnation.id,
            host: self.incarnation.host,
            environment: self.incarnation.environment,
            tool_call_id: tool_call_id.map(str::to_owned),
            spawner,
            run_id: run_id.cloned(),
            phase: CustodyPhase::Reserved,
        };
        let path = record_path(&self.dir, record.entry_id);
        write_record(&self.dir, &path, &record).await?;
        if let Some(run_id) = record.run_id.as_ref() {
            run_spawn_started(&self.dir, run_id);
        }
        Ok(CustodyReservation {
            path: Some(path),
            dir: self.dir.clone(),
            record,
        })
    }
}

/// A reserved, gated spawn (see [`ProcessCustody::prepare_spawn`]).
#[derive(Debug)]
pub struct PreparedCustodySpawn {
    reservation: CustodyReservation,
    gate: SpawnGate,
}

impl PreparedCustodySpawn {
    /// Record the spawned leader, then release the gate so the command runs.
    ///
    /// On error the gate stays closed and the command never runs; the caller
    /// must reap `child` (with `kill_on_drop` set, dropping it suffices).
    pub async fn spawned(
        self,
        child: &tokio::process::Child,
    ) -> Result<CustodyGuard, ProcessCustodyError> {
        let Self {
            mut reservation,
            mut gate,
        } = self;
        gate.spawned();
        let pid = child
            .id()
            .and_then(|pid| i32::try_from(pid).ok())
            .ok_or_else(|| {
                ProcessCustodyError::io(
                    "capture spawned leader pid",
                    std::io::Error::from(std::io::ErrorKind::NotFound),
                )
            })?;
        reservation.record_spawned(pid).await?;
        gate.release()
            .map_err(|error| ProcessCustodyError::io("release spawn gate", error))?;
        Ok(CustodyGuard { reservation })
    }
}

/// Custody of one spawned, released process group. Dropping it keeps the
/// record until the group is proven exited, so a host that dies meanwhile
/// leaves the group to recovery.
#[derive(Debug)]
pub struct CustodyGuard {
    reservation: CustodyReservation,
}

impl CustodyGuard {
    /// Remove the record now: the caller has proven the whole group exited.
    pub async fn settle(self) {
        self.reservation.settle().await;
    }

    /// Keep the record until kernel exit notification proves the group
    /// exited, then remove it. For owners that cannot prove containment
    /// themselves (for example a command hook whose background members may
    /// outlive it).
    pub fn settle_when_exited(self) {
        self.reservation.retain();
    }
}

fn record_path(dir: &Path, entry_id: Uuid) -> PathBuf {
    dir.join(format!("{entry_id}.{RECORD_EXTENSION}"))
}

/// Temp files carry their writer's incarnation, so recovery only ever
/// deletes interrupted writes of earlier incarnations, never a live write.
fn temp_path(dir: &Path, entry_id: Uuid, incarnation: Uuid) -> PathBuf {
    dir.join(format!("{entry_id}.{incarnation}.{TEMP_EXTENSION}"))
}

fn temp_incarnation(path: &Path) -> Option<Uuid> {
    let stem = path.file_stem()?.to_str()?;
    let (_, incarnation) = stem.rsplit_once('.')?;
    Uuid::parse_str(incarnation).ok()
}

/// A custody record for one tool process owned by this incarnation.
///
/// Dropping a reservation that never reached [`Self::record_spawned`] removes
/// its record: the spawn gate it guarded was never released, so no command
/// ran. Once spawned, only [`Self::settle`] (after containment is proven)
/// removes the record; otherwise the next incarnation recovers it.
#[derive(Debug)]
pub(super) struct CustodyReservation {
    path: Option<PathBuf>,
    dir: PathBuf,
    record: CustodyRecord,
}

impl CustodyReservation {
    /// Identity of this custody entry; it doubles as the spawn-gate token.
    pub(super) fn entry_id(&self) -> Uuid {
        self.record.entry_id
    }

    fn leader(&self) -> Option<ProcessIdentity> {
        match &self.record.phase {
            CustodyPhase::Spawned { leader, .. } => Some(*leader),
            CustodyPhase::Reserved | CustodyPhase::Interrupted { .. } | CustodyPhase::Exited => {
                None
            }
        }
    }

    fn watched(&self, path: PathBuf) -> WatchedRecord {
        WatchedRecord {
            dir: self.dir.clone(),
            path,
            record: self.record.clone(),
        }
    }

    /// Record the spawned leader's identity. Call before releasing the spawn
    /// gate.
    pub(super) async fn record_spawned(
        &mut self,
        leader_pid: i32,
    ) -> Result<(), ProcessCustodyError> {
        let leader = ProcessIdentity::capture(leader_pid)
            .map_err(|error| ProcessCustodyError::io("capture tool leader identity", error))?
            .ok_or_else(|| {
                ProcessCustodyError::io(
                    "capture tool leader identity",
                    std::io::Error::from(std::io::ErrorKind::NotFound),
                )
            })?;
        let session_leader = nix::unistd::getsid(Some(nix::unistd::Pid::from_raw(leader_pid)))
            .map_err(|error| {
                ProcessCustodyError::io("read tool session id", std::io::Error::from(error))
            })?
            .as_raw();
        register_live_group(leader.pid, Some(leader.start));
        // Mark the reservation spawned before the write starts. If this
        // future is cancelled mid-write, Drop must not remove a record the
        // still-running blocking write may yet rename into place; the
        // unreleased gate guarantees the command never runs, and recovery
        // settles whichever record landed.
        self.record.phase = CustodyPhase::Spawned {
            leader,
            session_leader,
        };
        let Some(path) = self.path.as_ref() else {
            return Ok(());
        };
        write_record(&self.dir, path, &self.record).await
    }

    /// Settle the record once the in-process guard has proven containment:
    /// remove it, or keep an `Exited` marker until its run commits. A failed
    /// write is harmless for containment: recovery later finds the group
    /// gone.
    pub(super) async fn settle(mut self) {
        if let Some(leader) = self.leader() {
            release_live_group(leader.pid, Some(leader.start));
        }
        if let Some(path) = self.path.take() {
            let dir = self.dir.clone();
            let record = self.record.clone();
            let ran = self.leader().is_some();
            let result = tokio::task::spawn_blocking(move || {
                finish_record_blocking(&dir, &path, &record, ran)
            })
            .await;
            if !matches!(result, Ok(Ok(()))) {
                tracing::warn!("failed to settle shell custody record");
            }
        }
    }

    /// Keep the record for recovery by a later incarnation (containment was
    /// not proven in-process). The group stays registered as live until it
    /// is proven exited.
    pub(super) fn retain(mut self) {
        let path = self.path.take();
        if let Some(leader) = self.leader() {
            let watched = path.map(|path| self.watched(path));
            release_when_group_exits(leader.pid, Some(leader.start), watched);
        }
    }
}

impl Drop for CustodyReservation {
    fn drop(&mut self) {
        let Some(path) = self.path.take() else {
            return;
        };
        match self.leader() {
            None => {
                // The gate was never released: nothing ran. The removal is
                // file I/O; keep it off the async worker running this drop.
                let dir = self.dir.clone();
                let record = self.record.clone();
                run_off_async_worker(move || {
                    if let Err(error) = finish_record_blocking(&dir, &path, &record, false) {
                        tracing::warn!(
                            %error,
                            "failed to remove unreleased shell custody reservation"
                        );
                    }
                });
            }
            // Cancelled after spawn: the in-process guard kills the group;
            // keep it registered (and the record for recovery) until its
            // exit is observed.
            Some(leader) => {
                let watched = self.watched(path);
                release_when_group_exits(leader.pid, Some(leader.start), Some(watched));
            }
        }
    }
}

/// Run blocking custody file I/O without blocking an async worker: on the
/// runtime's blocking pool when called from a runtime, inline otherwise.
fn run_off_async_worker(work: impl FnOnce() + Send + 'static) {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => drop(handle.spawn_blocking(work)),
        Err(_) => work(),
    }
}

async fn write_record(
    dir: &Path,
    path: &Path,
    record: &CustodyRecord,
) -> Result<(), ProcessCustodyError> {
    let bytes = serde_json::to_vec(record).map_err(|error| {
        ProcessCustodyError::io("encode custody record", std::io::Error::other(error))
    })?;
    let dir = dir.to_path_buf();
    let path = path.to_path_buf();
    let temp = temp_path(&dir, record.entry_id, record.incarnation);
    tokio::task::spawn_blocking(move || write_record_blocking(&dir, &path, &temp, &bytes))
        .await
        .map_err(|error| {
            ProcessCustodyError::io("join custody record write", std::io::Error::other(error))
        })?
        .map_err(|error| ProcessCustodyError::io("write custody record", error))
}

/// Atomic replace: write a temp file, plain-`fsync` its data (see the module
/// docs for why that is sufficient), then rename it over the record.
fn write_record_blocking(
    dir: &Path,
    path: &Path,
    temp: &Path,
    bytes: &[u8],
) -> std::io::Result<()> {
    let mut file = loop {
        std::fs::create_dir_all(dir)?;
        match std::fs::File::create(temp) {
            Ok(file) => break file,
            // Recovery removes an empty scope directory; recreate it.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    };
    file.write_all(bytes)?;
    nix::unistd::fsync(file.as_raw_fd()).map_err(std::io::Error::from)?;
    drop(file);
    std::fs::rename(temp, path)
}

fn remove_record(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// A listed record file, as read by recovery.
enum ListedRecord {
    /// Removed between listing and reading: already settled.
    Gone,
    Current(CustodyRecord),
    /// A record of another format version, interpreted only through its
    /// frozen envelope.
    OtherVersion(RecordEnvelope),
}

fn read_listed_record(path: &Path) -> Result<ListedRecord, ProcessCustodyError> {
    let corrupt = |reason: String| ProcessCustodyError::CorruptRecord {
        path: path.to_path_buf(),
        reason,
    };
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ListedRecord::Gone);
        }
        Err(error) => return Err(ProcessCustodyError::io("read custody record", error)),
    };
    let envelope: RecordEnvelope =
        serde_json::from_slice(&bytes).map_err(|error| corrupt(error.to_string()))?;
    let named = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(|stem| Uuid::parse_str(stem).ok());
    if named != Some(envelope.entry_id) {
        return Err(corrupt(
            "record name does not match its entry id".to_owned(),
        ));
    }
    if envelope.version != RECORD_VERSION {
        return Ok(ListedRecord::OtherVersion(envelope));
    }
    serde_json::from_slice(&bytes)
        .map(ListedRecord::Current)
        .map_err(|error| corrupt(error.to_string()))
}

fn recover_scope_blocking(
    dir: &Path,
    incarnation: &CustodyIncarnation,
    deadline: Instant,
) -> Result<ProcessCustodyRecoveryReport, ProcessCustodyError> {
    let root = dir.parent().ok_or_else(|| {
        ProcessCustodyError::io(
            "locate custody root",
            std::io::Error::from(std::io::ErrorKind::InvalidInput),
        )
    })?;
    let lock = scope_lock(dir);
    let _serialized = lock.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(_dir_lock) = lock_scope_dir(dir)
        .map_err(|error| ProcessCustodyError::io("lock custody scope", error))?
    else {
        return Ok(ProcessCustodyRecoveryReport::default());
    };
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ProcessCustodyRecoveryReport::default());
        }
        Err(error) => return Err(ProcessCustodyError::io("list custody records", error)),
    };
    let mut paths = Vec::new();
    let mut temps = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|error| ProcessCustodyError::io("list custody records", error))?
            .path();
        match path.extension().and_then(|extension| extension.to_str()) {
            Some(RECORD_EXTENSION) => paths.push(path),
            Some(TEMP_EXTENSION) => temps.push(path),
            _ => {}
        }
    }
    paths.sort();
    // Incarnations whose host is proven gone (not running, or its boot
    // ended). Only their temp files are interrupted writes; a temp of a live
    // host (this one, or a concurrent host sharing the root) may be a write
    // in flight.
    let mut ended_incarnations = BTreeSet::new();

    let mut report = ProcessCustodyRecoveryReport::default();
    for path in paths {
        let settled = match read_listed_record(&path)? {
            ListedRecord::Gone => continue,
            ListedRecord::Current(record) => {
                if record.incarnation == incarnation.id
                    || matches!(record.phase, CustodyPhase::Interrupted { .. })
                {
                    // Live-owned by this incarnation, or already settled
                    // evidence awaiting the runtime.
                    continue;
                }
                let cessation = settle_prior_record(root, &record, incarnation, deadline)?;
                // Settled only once its host is proven gone.
                ended_incarnations.insert(record.incarnation);
                let recovered = RecoveredToolProcess {
                    entry_id: record.entry_id,
                    prior_incarnation: record.incarnation,
                    tool_call_id: record.tool_call_id.clone(),
                    run_id: record.run_id.clone(),
                    spawner: record.spawner.clone(),
                    cessation,
                };
                if record.run_id.is_some() && cessation.may_have_run() {
                    // The process may have had effects inside a run that may
                    // still be in flight: keep the record as durable
                    // interrupted-run evidence until the runtime settles the
                    // run's inputs, instead of letting the run replay.
                    let mut evidence = record;
                    evidence.phase = CustodyPhase::Interrupted {
                        cessation,
                        settlement: InterruptedToolSettlement::Pending,
                    };
                    let bytes = serde_json::to_vec(&evidence).map_err(|error| {
                        ProcessCustodyError::io(
                            "encode interrupted-run evidence",
                            std::io::Error::other(error),
                        )
                    })?;
                    let temp = temp_path(dir, evidence.entry_id, incarnation.id);
                    write_record_blocking(dir, &path, &temp, &bytes).map_err(|error| {
                        ProcessCustodyError::io("record interrupted-run evidence", error)
                    })?;
                    report.recovered.push(recovered);
                    continue;
                }
                recovered
            }
            ListedRecord::OtherVersion(envelope) => {
                if envelope.incarnation == incarnation.id {
                    continue;
                }
                let ended = envelope
                    .environment
                    .and_then(|value| serde_json::from_value::<HostEnvironment>(value).ok())
                    .is_some_and(|recorded| {
                        recorded.relation_to(&incarnation.environment) == EnvironmentRelation::Ended
                    });
                if !ended {
                    return Err(ProcessCustodyError::UnsupportedRecordVersion {
                        path,
                        version: envelope.version,
                    });
                }
                ended_incarnations.insert(envelope.incarnation);
                RecoveredToolProcess {
                    entry_id: envelope.entry_id,
                    prior_incarnation: envelope.incarnation,
                    tool_call_id: envelope.tool_call_id,
                    run_id: None,
                    spawner: ToolProcessSpawner::default(),
                    cessation: ToolProcessCessation::PriorEnvironmentEnded,
                }
            }
        };
        remove_record(&path)
            .map_err(|error| ProcessCustodyError::io("remove settled custody record", error))?;
        report.recovered.push(settled);
    }
    for temp in temps {
        if temp_incarnation(&temp).is_some_and(|owner| ended_incarnations.contains(&owner)) {
            remove_record(&temp).map_err(|error| {
                ProcessCustodyError::io("remove interrupted custody write", error)
            })?;
        }
    }
    // Drop the scope directory once it holds nothing but its lock file.
    remove_scope_dir_if_empty(dir);
    Ok(report)
}

/// Settle one earlier-incarnation record. Host liveness is decided before
/// anything else, so a record whose host still runs is never rewritten or
/// removed (that host may still settle or delete it).
fn settle_prior_record(
    root: &Path,
    record: &CustodyRecord,
    current: &CustodyIncarnation,
    deadline: Instant,
) -> Result<ToolProcessCessation, ProcessCustodyError> {
    match record.environment.relation_to(&current.environment) {
        // Pids and sessions from another boot name nothing here; comparing or
        // signalling them could only hit strangers.
        EnvironmentRelation::Ended => return Ok(ToolProcessCessation::PriorEnvironmentEnded),
        // Another pid namespace of this boot: its pids name nothing here.
        // Only the incarnation's kernel-held lock can prove its host ended.
        EnvironmentRelation::ForeignNamespace => {
            let liveness = match foreign_incarnation_lock(root, record.incarnation)
                .map_err(|error| ProcessCustodyError::io("probe incarnation lock", error))?
            {
                IncarnationLock::Released => {
                    return Ok(match record.phase {
                        CustodyPhase::Exited => ToolProcessCessation::ExitedBeforeCommit,
                        _ => ToolProcessCessation::ForeignIncarnationEnded,
                    });
                }
                // An `Exited` marker guards no process, so no pid needs
                // observing: unless its host provably still runs (and may
                // still commit the run), it settles in any namespace.
                IncarnationLock::Unprovable if matches!(record.phase, CustodyPhase::Exited) => {
                    return Ok(ToolProcessCessation::ExitedBeforeCommit);
                }
                IncarnationLock::Held => ForeignIncarnationLiveness::Running,
                IncarnationLock::Unprovable => ForeignIncarnationLiveness::Unverifiable,
            };
            return Err(ProcessCustodyError::ForeignPidNamespace {
                entry_id: record.entry_id,
                incarnation: record.incarnation,
                liveness,
            });
        }
        EnvironmentRelation::Same | EnvironmentRelation::Unknown => {}
    }
    if record
        .host
        .is_running()
        .map_err(|error| ProcessCustodyError::io("probe prior host", error))?
    {
        // The live host may still supervise the tool, release its spawn
        // gate, or settle the record itself.
        return Err(ProcessCustodyError::PriorIncarnationAlive {
            entry_id: record.entry_id,
            incarnation: record.incarnation,
            host_pid: record.host.pid,
        });
    }
    settle_phase(record, deadline)
}

/// Settle a record whose host is proven gone.
fn settle_phase(
    record: &CustodyRecord,
    deadline: Instant,
) -> Result<ToolProcessCessation, ProcessCustodyError> {
    match &record.phase {
        // Only the (gone) host could have released the gate.
        CustodyPhase::Reserved => Ok(ToolProcessCessation::NeverStarted),
        CustodyPhase::Interrupted { cessation, .. } => Ok(*cessation),
        CustodyPhase::Exited => Ok(ToolProcessCessation::ExitedBeforeCommit),
        CustodyPhase::Spawned {
            leader,
            session_leader,
        } => {
            let pgid = leader.pid;
            if live_group_supersedes(pgid, &leader.start) {
                return Ok(ToolProcessCessation::GroupReassigned);
            }
            let members = match owned_members(leader, *session_leader)
                .map_err(|error| ProcessCustodyError::io("inspect tool process group", error))?
            {
                GroupOwnership::Reassigned => return Ok(ToolProcessCessation::GroupReassigned),
                GroupOwnership::Owned(members) => members,
            };
            if members.is_empty() {
                return Ok(ToolProcessCessation::AlreadyExited);
            }
            kill_group_and_await_exit(record.entry_id, pgid, members, deadline)
        }
    }
}

/// Current members of group `pgid` (zombies included where the platform
/// lists them). Foreign members may be listed but are never ours. Where an
/// empty listing is not authoritative (Linux), it is only believed when
/// `kill(-pgid, 0)` agrees: ESRCH (no such group) or EPERM (the group holds
/// only processes we cannot signal, so none of ours).
fn group_members_checked(pgid: i32) -> std::io::Result<Vec<(i32, ProcessProbe)>> {
    let listed = sys::group_members(pgid)?;
    // Only an empty kernel listing needs the cross-check: listed members that
    // probe as absent (zombies, exits while reading) prove the listing works.
    if listed.is_empty() && !sys::EMPTY_LISTING_IS_AUTHORITATIVE {
        match nix::sys::signal::kill(nix::unistd::Pid::from_raw(-pgid), None) {
            Err(nix::errno::Errno::ESRCH | nix::errno::Errno::EPERM) => {}
            Ok(()) => {
                return Err(std::io::Error::other(
                    "process group exists but its member listing is empty",
                ));
            }
            Err(error) => return Err(std::io::Error::from(error)),
        }
    }
    Ok(listed
        .into_iter()
        .filter(|(_, probe)| match probe {
            ProcessProbe::Observed(member) => member.pgid == pgid,
            ProcessProbe::Absent => false,
            ProcessProbe::Foreign => true,
        })
        .collect())
}

/// Whether the recorded group id still names the recorded tool's group.
enum GroupOwnership {
    /// The id now names a different group; the recorded one is gone.
    Reassigned,
    /// The recorded group; its members of ours (possibly none).
    Owned(Vec<ProcessIdentity>),
}

/// Process group ids are not recycled while the group exists, so the group
/// is still ours when its leader is present with the recorded stamp; a
/// leader pid held by another process (another stamp, another user) proves
/// the recorded group is gone. When the leader is gone, every member of ours
/// must have started no earlier than the leader and share its session: one
/// member that provably does not means a different group holds the id.
fn owned_members(leader: &ProcessIdentity, session_leader: i32) -> std::io::Result<GroupOwnership> {
    let pgid = leader.pid;
    let leader_present = match sys::probe(leader.pid)? {
        ProcessProbe::Observed(snapshot) if snapshot.start == leader.start => true,
        ProcessProbe::Observed(_) | ProcessProbe::Foreign => {
            return Ok(GroupOwnership::Reassigned);
        }
        ProcessProbe::Absent => false,
    };
    let mut members = Vec::new();
    for (pid, probe) in group_members_checked(pgid)? {
        let ProcessProbe::Observed(member) = probe else {
            continue;
        };
        if !leader_present {
            if leader.start.not_after(&member.start) != Some(true) {
                return Ok(GroupOwnership::Reassigned);
            }
            match member_session(pid)? {
                // Exited between listing and this probe: nothing to prove.
                None => continue,
                Some(sid) if sid != session_leader => return Ok(GroupOwnership::Reassigned),
                Some(_) => {}
            }
        }
        members.push(ProcessIdentity {
            pid,
            start: member.start,
        });
    }
    Ok(GroupOwnership::Owned(members))
}

fn member_session(pid: i32) -> std::io::Result<Option<i32>> {
    match nix::unistd::getsid(Some(nix::unistd::Pid::from_raw(pid))) {
        Ok(sid) => Ok(Some(sid.as_raw())),
        Err(nix::errno::Errno::ESRCH | nix::errno::Errno::EPERM) => Ok(None),
        Err(error) => Err(std::io::Error::from(error)),
    }
}

fn observed_members(pgid: i32) -> std::io::Result<Vec<ProcessIdentity>> {
    Ok(group_members_checked(pgid)?
        .into_iter()
        .filter_map(|(pid, probe)| match probe {
            ProcessProbe::Observed(member) => Some(ProcessIdentity {
                pid,
                start: member.start,
            }),
            ProcessProbe::Absent | ProcessProbe::Foreign => None,
        })
        .collect())
}

/// SIGKILL the verified members and wait on kernel exit notification for
/// each, re-listing and re-signalling until the listing is empty or holds
/// only notified zombies. Each round opens one exit handle per member and
/// signals through it (a pidfd on Linux, so a reused pid is never
/// signalled; the group on macOS). A SIGKILLed process cannot fork, so this
/// converges; the deadline only fails recovery closed.
fn kill_group_and_await_exit(
    entry_id: Uuid,
    pgid: i32,
    observed: Vec<ProcessIdentity>,
    deadline: Instant,
) -> Result<ToolProcessCessation, ProcessCustodyError> {
    if !sys::exit_notification_available()
        .map_err(|error| ProcessCustodyError::io("probe exit notification", error))?
    {
        // Refuse before signalling anything: without exit notification
        // cessation could never be proven.
        return Err(ProcessCustodyError::ExitNotificationUnavailable {
            entry_id,
            pgid,
            live_members: observed.len(),
        });
    }
    let mut killed: BTreeSet<i32> = observed.iter().map(|member| member.pid).collect();
    let mut members = observed;
    loop {
        let watch = sys::ExitWatch::new(&members)
            .map_err(|error| ProcessCustodyError::io("watch killed process exit", error))?;
        let refused = watch
            .kill(pgid)
            .map_err(|error| ProcessCustodyError::io("kill prior tool process group", error))?;
        if refused > 0 {
            return Err(ProcessCustodyError::CessationUnproven {
                entry_id,
                pgid,
                live_members: refused,
            });
        }
        match watch
            .wait_all(deadline)
            .map_err(|error| ProcessCustodyError::io("await killed process exit", error))?
        {
            ExitWaitOutcome::AllExited => {}
            ExitWaitOutcome::DeadlineElapsed => {
                return Err(ProcessCustodyError::CessationUnproven {
                    entry_id,
                    pgid,
                    live_members: members.len(),
                });
            }
        }
        members = observed_members(pgid)
            .map_err(|error| ProcessCustodyError::io("inspect killed process group", error))?;
        // Members that already exited are unreaped zombies; their exit
        // watches fire immediately, so the loop only repeats while the
        // listing still changes.
        let unseen = members
            .iter()
            .filter(|member| !killed.contains(&member.pid))
            .count();
        if members.is_empty() || (unseen == 0 && all_exited(&members)?) {
            return Ok(ToolProcessCessation::KilledByRecovery {
                members: killed.len(),
            });
        }
        killed.extend(members.iter().map(|member| member.pid));
    }
}

fn all_exited(members: &[ProcessIdentity]) -> Result<bool, ProcessCustodyError> {
    for member in members {
        if member
            .is_running()
            .map_err(|error| ProcessCustodyError::io("probe killed member", error))?
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn evidence_error(error: impl std::fmt::Display) -> InterruptedToolEvidenceError {
    InterruptedToolEvidenceError {
        reason: error.to_string(),
    }
}

/// Interrupted-run evidence records of one scope.
fn interrupted_records(dir: &Path) -> Result<Vec<(PathBuf, CustodyRecord)>, ProcessCustodyError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(ProcessCustodyError::io("list custody records", error)),
    };
    let mut records = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|error| ProcessCustodyError::io("list custody records", error))?
            .path();
        if path.extension().and_then(|extension| extension.to_str()) != Some(RECORD_EXTENSION) {
            continue;
        }
        if let ListedRecord::Current(record) = read_listed_record(&path)?
            && matches!(record.phase, CustodyPhase::Interrupted { .. })
        {
            records.push((path, record));
        }
    }
    records.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(records)
}

fn interrupted_call(record: &CustodyRecord) -> Option<InterruptedToolCall> {
    let CustodyPhase::Interrupted {
        cessation,
        settlement,
    } = &record.phase
    else {
        return None;
    };
    Some(InterruptedToolCall {
        entry_id: record.entry_id,
        run_id: record.run_id.clone()?,
        tool_call_id: record.tool_call_id.clone(),
        spawner: record.spawner.clone(),
        cessation: *cessation,
        settlement: settlement.clone(),
    })
}

#[async_trait::async_trait]
impl InterruptedToolEvidence for ProcessCustody {
    async fn interrupted_calls(
        &self,
    ) -> Result<Vec<InterruptedToolCall>, InterruptedToolEvidenceError> {
        let dir = self.dir.clone();
        tokio::task::spawn_blocking(move || {
            let lock = scope_lock(&dir);
            let _serialized = lock.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(_dir_lock) = lock_scope_dir(&dir)
                .map_err(|error| ProcessCustodyError::io("lock custody scope", error))?
            else {
                return Ok(Vec::new());
            };
            interrupted_records(&dir).map(|records| {
                records
                    .iter()
                    .filter_map(|(_, r)| interrupted_call(r))
                    .collect()
            })
        })
        .await
        .map_err(evidence_error)?
        .map_err(evidence_error)
    }

    async fn mark_inputs_settled(
        &self,
        entry_ids: &[Uuid],
        inputs: &InterruptedRunInputs,
    ) -> Result<(), InterruptedToolEvidenceError> {
        let dir = self.dir.clone();
        let entry_ids: BTreeSet<Uuid> = entry_ids.iter().copied().collect();
        let inputs = inputs.clone();
        let incarnation = self.incarnation.id;
        tokio::task::spawn_blocking(move || {
            let lock = scope_lock(&dir);
            let _serialized = lock.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(_dir_lock) = lock_scope_dir(&dir)
                .map_err(|error| ProcessCustodyError::io("lock custody scope", error))?
            else {
                return Ok(());
            };
            // Re-listed under the lock: an entry another process already
            // acknowledged is gone and is never rewritten.
            for (path, mut record) in interrupted_records(&dir)? {
                if !entry_ids.contains(&record.entry_id) {
                    continue;
                }
                if let CustodyPhase::Interrupted { settlement, .. } = &mut record.phase {
                    *settlement = InterruptedToolSettlement::InputsSettled(inputs.clone());
                }
                let bytes = serde_json::to_vec(&record).map_err(|error| {
                    ProcessCustodyError::io(
                        "encode interrupted-run evidence",
                        std::io::Error::other(error),
                    )
                })?;
                let temp = temp_path(&dir, record.entry_id, incarnation);
                write_record_blocking(&dir, &path, &temp, &bytes).map_err(|error| {
                    ProcessCustodyError::io("record interrupted-run settlement", error)
                })?;
            }
            Ok::<(), ProcessCustodyError>(())
        })
        .await
        .map_err(evidence_error)?
        .map_err(evidence_error)
    }

    async fn acknowledge(&self, entry_ids: &[Uuid]) -> Result<(), InterruptedToolEvidenceError> {
        let dir = self.dir.clone();
        let paths: Vec<PathBuf> = entry_ids
            .iter()
            .map(|entry_id| record_path(&self.dir, *entry_id))
            .collect();
        tokio::task::spawn_blocking(move || {
            let lock = scope_lock(&dir);
            let _serialized = lock.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(_dir_lock) = lock_scope_dir(&dir)? else {
                return Ok(());
            };
            for path in paths {
                remove_record(&path)?;
            }
            Ok::<(), std::io::Error>(())
        })
        .await
        .map_err(evidence_error)?
        .map_err(evidence_error)
    }

    async fn run_ended(
        &self,
        run_id: &meerkat_core::RunId,
    ) -> Result<(), InterruptedToolEvidenceError> {
        let dir = self.dir.clone();
        let run_id = run_id.clone();
        let incarnation = self.incarnation.id;
        tokio::task::spawn_blocking(move || {
            let lock = scope_lock(&dir);
            let _serialized = lock.lock().unwrap_or_else(PoisonError::into_inner);
            // Spawns of the run still live finish without a marker.
            if let Some(spawns) = run_spawns().get_mut(&(dir.clone(), run_id.clone())) {
                spawns.ended = true;
            }
            let entries = match std::fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(ProcessCustodyError::io("list custody records", error)),
            };
            for entry in entries {
                let path = entry
                    .map_err(|error| ProcessCustodyError::io("list custody records", error))?
                    .path();
                if path.extension().and_then(|extension| extension.to_str())
                    != Some(RECORD_EXTENSION)
                {
                    continue;
                }
                if let ListedRecord::Current(record) = read_listed_record(&path)?
                    && matches!(record.phase, CustodyPhase::Exited)
                    && record.incarnation == incarnation
                    && record.run_id.as_ref() == Some(&run_id)
                {
                    remove_record(&path).map_err(|error| {
                        ProcessCustodyError::io("remove ended run marker", error)
                    })?;
                }
            }
            Ok::<(), ProcessCustodyError>(())
        })
        .await
        .map_err(evidence_error)?
        .map_err(evidence_error)
    }
}

#[cfg(test)]
mod tests;

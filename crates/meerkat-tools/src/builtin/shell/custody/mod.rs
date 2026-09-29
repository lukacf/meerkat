//! Durable, incarnation-bound custody for owned shell tool process groups.
//!
//! The in-process [`super::process_lifecycle::OwnedProcessGroup`] guard only
//! contains a tool while the process that spawned it is alive. When the host
//! (gateway) is SIGKILLed, that guard dies with it and the tool's process
//! group keeps running. Custody closes that gap with a record that outlives
//! the host:
//!
//! 1. **Reserve** - before spawn, a record naming the scope, this host
//!    incarnation, and the host's own process identity is written durably.
//! 2. **Gated spawn** - the tool starts in a fresh process group behind a
//!    spawn gate: a `/bin/sh` prologue blocks on a pipe whose only writer is
//!    the host. If the host dies before releasing the gate, the prologue reads
//!    EOF and exits without running the command. No process can execute the
//!    command without a durable record naming it.
//! 3. **Spawned** - the leader's pid and kernel start stamp (pid-reuse proof)
//!    are written durably, and only then is the gate released.
//! 4. **Settle** - after the in-process guard proves containment the record
//!    is removed.
//! 5. **Recover** - a later incarnation opening the same scope must first
//!    settle every record left by earlier incarnations: verify the group's
//!    identity, SIGKILL it, and wait for every member's exit through kernel
//!    exit notification (pidfd on Linux, kqueue `EVFILT_PROC` on macOS).
//!    [`ProcessCustody`] handles can only be obtained through that recovery,
//!    so holding one is the proof that the scope's earlier tools have ceased.
//!
//! Custody covers ordinary tools that stay in their process group. A process
//! that deliberately leaves its group (`setsid`, `setpgid`) is daemon
//! containment and out of scope.

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

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use meerkat_core::types::SessionId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Directory, under a realm scope root, that holds custody records.
pub const PROCESS_CUSTODY_DIR: &str = "tool_process_custody";

const RECORD_EXTENSION: &str = "json";
const TEMP_EXTENSION: &str = "tmp";
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
    fn capture(pid: i32) -> std::io::Result<Option<Self>> {
        Ok(sys::snapshot(pid)?.map(|snapshot| Self {
            pid,
            start: snapshot.start,
        }))
    }

    /// True only when this exact process (pid and start stamp) has not
    /// exited.
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
        pid_namespace_dev: u64,
        pid_namespace_ino: u64,
    },
    Darwin,
}

/// Point-in-time kernel facts about one process. Zombies are included:
/// only kernel exit notification decides that a member has ceased.
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
    #[serde(flatten)]
    phase: CustodyPhase,
}

/// How recovery established that an earlier incarnation's tool has ceased.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cessation", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ToolProcessCessation {
    /// The host died before releasing the spawn gate, so the command never
    /// started.
    NeverStarted,
    /// The group had no live member when recovery inspected it: the tool had
    /// already finished or been killed. Its result was not delivered.
    AlreadyExited,
    /// Recovery SIGKILLed the group and observed every member exit.
    KilledByRecovery { members: usize },
    /// The earlier incarnation ran in a boot or pid namespace that has since
    /// been replaced. A reboot ends every process; the kernel SIGKILLs every
    /// process of a pid namespace when its init exits. Recovery cannot
    /// observe that environment, so it signals nothing.
    PriorEnvironmentEnded,
}

/// One earlier-incarnation tool settled by recovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RecoveredToolProcess {
    pub entry_id: Uuid,
    pub prior_incarnation: Uuid,
    /// Provider tool-call id of the interrupted call, when known.
    pub tool_call_id: Option<String>,
    pub cessation: ToolProcessCessation,
}

/// Outcome of settling a scope's earlier-incarnation custody records.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ProcessCustodyRecoveryReport {
    pub recovered: Vec<RecoveredToolProcess>,
}

/// Errors establishing or recovering process custody. Every error fails
/// closed: no custody handle is produced and no new scope work is admitted.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProcessCustodyError {
    #[error("invalid process custody scope '{0}'")]
    InvalidScope(String),
    #[error("process custody I/O failed ({context}): {source}")]
    Io {
        context: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("process custody record {path} is unreadable: {reason}")]
    CorruptRecord { path: PathBuf, reason: String },
    #[error(
        "tool process of custody entry {entry_id} is still owned by live host incarnation {incarnation} (pid {host_pid})"
    )]
    PriorIncarnationAlive {
        entry_id: Uuid,
        incarnation: Uuid,
        host_pid: i32,
    },
    #[error(
        "tool process group {pgid} of custody entry {entry_id} did not cease: {live_members} member(s) still running"
    )]
    CessationUnproven {
        entry_id: Uuid,
        pgid: i32,
        live_members: usize,
    },
}

impl ProcessCustodyError {
    fn io(context: &'static str, source: std::io::Error) -> Self {
        Self::Io { context, source }
    }
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

impl ProcessCustody {
    /// Settle every earlier-incarnation custody record for `scope` under
    /// `root`, then return the scope's custody handle.
    ///
    /// Earlier tools are proven stopped (or were never started) before this
    /// returns `Ok`; any doubt is an error and yields no handle.
    pub async fn recover_and_open(
        root: &Path,
        scope: ProcessCustodyScope,
    ) -> Result<(Arc<Self>, ProcessCustodyRecoveryReport), ProcessCustodyError> {
        scope.validate()?;
        let incarnation = incarnation()?;
        let dir = root.join(scope.as_str());
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
        Ok((
            Arc::new(Self {
                dir,
                scope,
                incarnation,
            }),
            report,
        ))
    }

    pub fn scope(&self) -> &ProcessCustodyScope {
        &self.scope
    }

    /// Durably reserve custody for one tool process before it is spawned.
    pub(super) async fn reserve(
        self: &Arc<Self>,
        tool_call_id: Option<&str>,
    ) -> Result<CustodyReservation, ProcessCustodyError> {
        let record = CustodyRecord {
            version: RECORD_VERSION,
            entry_id: Uuid::new_v4(),
            scope: self.scope.as_str().to_owned(),
            incarnation: self.incarnation.id,
            host: self.incarnation.host,
            environment: self.incarnation.environment,
            tool_call_id: tool_call_id.map(str::to_owned),
            phase: CustodyPhase::Reserved,
        };
        let path = self.record_path(record.entry_id);
        write_record(&self.dir, &path, &record).await?;
        Ok(CustodyReservation {
            path: Some(path),
            dir: self.dir.clone(),
            record,
        })
    }

    fn record_path(&self, entry_id: Uuid) -> PathBuf {
        self.dir.join(format!("{entry_id}.{RECORD_EXTENSION}"))
    }
}

/// A durable custody record for one tool process owned by this incarnation.
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

    /// Record the spawned leader's identity durably. Call before releasing
    /// the spawn gate.
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
        // Mark the reservation spawned before the durable write starts. If
        // this future is cancelled mid-write, Drop must not remove a record
        // the still-running blocking write may yet rename into place; the
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

    /// Remove the record once the in-process guard has proven containment.
    /// A failed removal is harmless: recovery later finds the group gone.
    pub(super) async fn settle(mut self) {
        if let Some(path) = self.path.take() {
            let result = tokio::task::spawn_blocking(move || remove_record(&path)).await;
            if !matches!(result, Ok(Ok(()))) {
                tracing::warn!("failed to remove settled shell custody record");
            }
        }
    }

    /// Keep the record for recovery by a later incarnation (containment was
    /// not proven in-process).
    pub(super) fn retain(mut self) {
        self.path = None;
    }
}

impl Drop for CustodyReservation {
    fn drop(&mut self) {
        if let Some(path) = self.path.take()
            && matches!(self.record.phase, CustodyPhase::Reserved)
            && let Err(error) = remove_record(&path)
        {
            tracing::warn!(%error, "failed to remove unreleased shell custody reservation");
        }
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
    tokio::task::spawn_blocking(move || write_record_blocking(&dir, &path, &bytes))
        .await
        .map_err(|error| {
            ProcessCustodyError::io("join custody record write", std::io::Error::other(error))
        })?
        .map_err(|error| ProcessCustodyError::io("write custody record", error))
}

/// Atomic, durable replace: write a temp file, fsync it, rename over the
/// record, fsync the directory.
fn write_record_blocking(dir: &Path, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let temp = path.with_extension(TEMP_EXTENSION);
    {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&temp, path)?;
    std::fs::File::open(dir)?.sync_all()
}

fn remove_record(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    }
    match path.parent() {
        Some(dir) => std::fs::File::open(dir)?.sync_all(),
        None => Ok(()),
    }
}

fn recover_scope_blocking(
    dir: &Path,
    incarnation: &CustodyIncarnation,
    deadline: Instant,
) -> Result<ProcessCustodyRecoveryReport, ProcessCustodyError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ProcessCustodyRecoveryReport::default());
        }
        Err(error) => return Err(ProcessCustodyError::io("list custody records", error)),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|error| ProcessCustodyError::io("list custody records", error))?
            .path();
        match path.extension().and_then(|extension| extension.to_str()) {
            Some(RECORD_EXTENSION) => paths.push(path),
            // An interrupted atomic write; the durable record (if any) is the
            // renamed file, which is what governs.
            Some(TEMP_EXTENSION) => {
                if let Err(error) = std::fs::remove_file(&path)
                    && error.kind() != std::io::ErrorKind::NotFound
                {
                    return Err(ProcessCustodyError::io(
                        "remove interrupted custody write",
                        error,
                    ));
                }
            }
            _ => {}
        }
    }
    paths.sort();

    let mut report = ProcessCustodyRecoveryReport::default();
    for path in paths {
        let record = read_record(&path)?;
        if record.incarnation == incarnation.id {
            continue;
        }
        let cessation = settle_prior_record(&record, incarnation, deadline)?;
        remove_record(&path)
            .map_err(|error| ProcessCustodyError::io("remove settled custody record", error))?;
        report.recovered.push(RecoveredToolProcess {
            entry_id: record.entry_id,
            prior_incarnation: record.incarnation,
            tool_call_id: record.tool_call_id,
            cessation,
        });
    }
    Ok(report)
}

fn read_record(path: &Path) -> Result<CustodyRecord, ProcessCustodyError> {
    let corrupt = |reason: String| ProcessCustodyError::CorruptRecord {
        path: path.to_path_buf(),
        reason,
    };
    let bytes = std::fs::read(path)
        .map_err(|error| ProcessCustodyError::io("read custody record", error))?;
    let record: CustodyRecord =
        serde_json::from_slice(&bytes).map_err(|error| corrupt(error.to_string()))?;
    if record.version != RECORD_VERSION {
        return Err(corrupt(format!(
            "unsupported record version {}",
            record.version
        )));
    }
    let named = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(|stem| Uuid::parse_str(stem).ok());
    if named != Some(record.entry_id) {
        return Err(corrupt(
            "record name does not match its entry id".to_owned(),
        ));
    }
    Ok(record)
}

fn settle_prior_record(
    record: &CustodyRecord,
    current: &CustodyIncarnation,
    deadline: Instant,
) -> Result<ToolProcessCessation, ProcessCustodyError> {
    if record.environment != current.environment {
        // Pids and sessions from another boot or pid namespace name nothing
        // here; comparing or signalling them could only hit strangers.
        return Ok(ToolProcessCessation::PriorEnvironmentEnded);
    }
    let host_alive = record
        .host
        .is_running()
        .map_err(|error| ProcessCustodyError::io("probe prior host", error))?;
    let prior_alive = || ProcessCustodyError::PriorIncarnationAlive {
        entry_id: record.entry_id,
        incarnation: record.incarnation,
        host_pid: record.host.pid,
    };
    match &record.phase {
        // Only the live host could still release the gate.
        CustodyPhase::Reserved if host_alive => Err(prior_alive()),
        CustodyPhase::Reserved => Ok(ToolProcessCessation::NeverStarted),
        CustodyPhase::Spawned {
            leader,
            session_leader,
        } => {
            let pgid = leader.pid;
            let Some(members) = owned_members(leader, *session_leader)
                .map_err(|error| ProcessCustodyError::io("inspect tool process group", error))?
            else {
                return Ok(ToolProcessCessation::AlreadyExited);
            };
            if members.is_empty() {
                return Ok(ToolProcessCessation::AlreadyExited);
            }
            if host_alive {
                // Never kill a tool its live host still supervises.
                return Err(prior_alive());
            }
            kill_group_and_await_exit(record.entry_id, pgid, members, deadline)
        }
    }
}

/// Current members of group `pgid` (zombies included). An empty listing is
/// only believed when the kernel also reports that no process has the group
/// id, so a failed listing can never pass for an empty group.
fn group_members_checked(pgid: i32) -> std::io::Result<Vec<(i32, ProcessSnapshot)>> {
    let members: Vec<_> = sys::group_members(pgid)?
        .into_iter()
        .filter(|(_, member)| member.pgid == pgid)
        .collect();
    if members.is_empty() {
        match nix::sys::signal::kill(nix::unistd::Pid::from_raw(-pgid), None) {
            Err(nix::errno::Errno::ESRCH) => {}
            Ok(()) | Err(_) => {
                return Err(std::io::Error::other(
                    "process group exists but its member listing is empty",
                ));
            }
        }
    }
    Ok(members)
}

/// Members of the recorded group, or `None` when the group id no longer
/// names the recorded tool's group.
///
/// Process group ids are not recycled while the group exists, so the group
/// is still ours when its leader is present with the recorded stamp; a
/// leader pid held by a process with another stamp proves the recorded group
/// is gone. When the leader is gone, every member must have started no
/// earlier than the leader and share its session: one member that provably
/// does not means a different group now holds the id.
fn owned_members(
    leader: &ProcessIdentity,
    session_leader: i32,
) -> std::io::Result<Option<Vec<ProcessIdentity>>> {
    let pgid = leader.pid;
    let leader_present = match sys::snapshot(leader.pid)? {
        Some(snapshot) if snapshot.start != leader.start => return Ok(None),
        Some(_) => true,
        None => false,
    };
    let mut members = Vec::new();
    for (pid, member) in group_members_checked(pgid)? {
        if !leader_present {
            if leader.start.not_after(&member.start) != Some(true) {
                return Ok(None);
            }
            match member_session(pid)? {
                // Exited between listing and this probe: nothing to prove.
                None => continue,
                Some(sid) if sid != session_leader => return Ok(None),
                Some(_) => {}
            }
        }
        members.push(ProcessIdentity {
            pid,
            start: member.start,
        });
    }
    Ok(Some(members))
}

fn member_session(pid: i32) -> std::io::Result<Option<i32>> {
    match nix::unistd::getsid(Some(nix::unistd::Pid::from_raw(pid))) {
        Ok(sid) => Ok(Some(sid.as_raw())),
        Err(nix::errno::Errno::ESRCH) => Ok(None),
        Err(error) => Err(std::io::Error::from(error)),
    }
}

/// SIGKILL the group and wait on kernel exit notification for every member,
/// re-listing and re-signalling until the listing is empty. A SIGKILLed
/// process cannot fork, so this converges; the deadline only fails recovery
/// closed.
fn kill_group_and_await_exit(
    entry_id: Uuid,
    pgid: i32,
    observed: Vec<ProcessIdentity>,
    deadline: Instant,
) -> Result<ToolProcessCessation, ProcessCustodyError> {
    use nix::sys::signal::{Signal, killpg};

    let mut killed: std::collections::BTreeSet<i32> =
        observed.iter().map(|member| member.pid).collect();
    let mut members = observed;
    loop {
        match killpg(nix::unistd::Pid::from_raw(pgid), Signal::SIGKILL) {
            Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
            Err(error) => {
                return Err(ProcessCustodyError::io(
                    "kill prior tool process group",
                    std::io::Error::from(error),
                ));
            }
        }
        let watch = sys::ExitWatch::new(&members)
            .map_err(|error| ProcessCustodyError::io("watch killed process exit", error))?;
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
        members = group_members_checked(pgid)
            .map_err(|error| ProcessCustodyError::io("inspect killed process group", error))?
            .into_iter()
            .map(|(pid, member)| ProcessIdentity {
                pid,
                start: member.start,
            })
            .collect();
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

#[cfg(test)]
mod tests;

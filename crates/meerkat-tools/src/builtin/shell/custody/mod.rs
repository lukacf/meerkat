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
//!    is removed.
//! 5. **Recover** - a later incarnation opening the same scope must first
//!    settle every record left by earlier incarnations: verify the group's
//!    identity, SIGKILL it, and wait for every member's exit through kernel
//!    exit notification (pidfd on Linux, kqueue `EVFILT_PROC` on macOS).
//!    [`ProcessCustody`] handles can only be obtained through that recovery,
//!    so holding one is the proof that the scope's earlier tools have ceased.
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

use std::collections::BTreeSet;
use std::io::Write as _;
use std::os::fd::AsRawFd as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use meerkat_core::types::SessionId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use super::custody_types::{
    ProcessCustodyError, ProcessCustodyRecoveryReport, RecoveredToolProcess, ToolProcessCessation,
};

/// Directory, under a realm runtime root, that holds custody records.
pub const PROCESS_CUSTODY_DIR: &str = "tool_process_custody";

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
    /// Provably another boot or pid namespace: everything recorded there has
    /// ended, and its pids name nothing here.
    Ended,
    /// Not provably the same or different (an identity was not recorded).
    /// Recovery proceeds with per-process checks, which classify reused and
    /// foreign pids safely.
    Unknown,
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
                    (Some(_), Some(_), Some(_), Some(_)) => EnvironmentRelation::Ended,
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

/// Process group ids of this incarnation's custody-bound tools that are not
/// yet proven stopped, across every scope. A recorded group id that names one
/// of them cannot be an earlier incarnation's group: group ids are unique
/// while a group exists. This guards recovery when earlier and current tools
/// share a session (for example a supervisor-inherited session), where
/// start-time and session checks alone cannot tell incarnations apart.
static LIVE_GROUPS: Mutex<BTreeSet<i32>> = Mutex::new(BTreeSet::new());

fn register_live_group(pgid: i32) {
    LIVE_GROUPS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(pgid);
}

fn release_live_group(pgid: i32) {
    LIVE_GROUPS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&pgid);
}

fn is_live_group(pgid: i32) -> bool {
    LIVE_GROUPS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .contains(&pgid)
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

impl ProcessCustody {
    /// Settle every earlier-incarnation custody record for `scope` under
    /// `root`, then return the scope's custody handle.
    ///
    /// Earlier tools are proven stopped (or never started, or provably gone)
    /// before this returns `Ok`; any doubt is a typed error and yields no
    /// handle. See [`ProcessCustodyError`] for the operator action per error.
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

    /// Reserve custody for one tool process before it is spawned.
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
        let path = record_path(&self.dir, record.entry_id);
        write_record(&self.dir, &path, &record).await?;
        Ok(CustodyReservation {
            path: Some(path),
            dir: self.dir.clone(),
            record,
        })
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

    fn leader_pgid(&self) -> Option<i32> {
        match &self.record.phase {
            CustodyPhase::Spawned { leader, .. } => Some(leader.pid),
            CustodyPhase::Reserved => None,
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
        register_live_group(leader.pid);
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

    /// Remove the record once the in-process guard has proven containment.
    /// A failed removal is harmless: recovery later finds the group gone.
    pub(super) async fn settle(mut self) {
        if let Some(pgid) = self.leader_pgid() {
            release_live_group(pgid);
        }
        if let Some(path) = self.path.take() {
            let result = tokio::task::spawn_blocking(move || remove_record(&path)).await;
            if !matches!(result, Ok(Ok(()))) {
                tracing::warn!("failed to remove settled shell custody record");
            }
        }
    }

    /// Keep the record for recovery by a later incarnation (containment was
    /// not proven in-process). The group stays registered as live.
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
            // An interrupted write of an earlier incarnation; the record (if
            // any) is the renamed file, which is what governs. A temp of the
            // current incarnation may be a write in flight: keep it.
            Some(TEMP_EXTENSION) => {
                if temp_incarnation(&path).is_some_and(|owner| owner != incarnation.id) {
                    remove_record(&path).map_err(|error| {
                        ProcessCustodyError::io("remove interrupted custody write", error)
                    })?;
                }
            }
            _ => {}
        }
    }
    paths.sort();

    let mut report = ProcessCustodyRecoveryReport::default();
    for path in paths {
        let settled = match read_listed_record(&path)? {
            ListedRecord::Gone => continue,
            ListedRecord::Current(record) => {
                if record.incarnation == incarnation.id {
                    continue;
                }
                let cessation = settle_prior_record(&record, incarnation, deadline)?;
                RecoveredToolProcess {
                    entry_id: record.entry_id,
                    prior_incarnation: record.incarnation,
                    tool_call_id: record.tool_call_id,
                    cessation,
                }
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
                RecoveredToolProcess {
                    entry_id: envelope.entry_id,
                    prior_incarnation: envelope.incarnation,
                    tool_call_id: envelope.tool_call_id,
                    cessation: ToolProcessCessation::PriorEnvironmentEnded,
                }
            }
        };
        remove_record(&path)
            .map_err(|error| ProcessCustodyError::io("remove settled custody record", error))?;
        report.recovered.push(settled);
    }
    // Drop the scope directory once it is empty. A concurrent reservation
    // recreates it (see `write_record_blocking`); a non-empty directory is
    // left as is.
    let _ = std::fs::remove_dir(dir);
    Ok(report)
}

fn settle_prior_record(
    record: &CustodyRecord,
    current: &CustodyIncarnation,
    deadline: Instant,
) -> Result<ToolProcessCessation, ProcessCustodyError> {
    if record.environment.relation_to(&current.environment) == EnvironmentRelation::Ended {
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
            if is_live_group(pgid) {
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
            if host_alive {
                // Never kill a tool its live host still supervises.
                return Err(prior_alive());
            }
            kill_group_and_await_exit(record.entry_id, pgid, members, deadline)
        }
    }
}

/// Current members of group `pgid` (zombies included). Foreign members may
/// be listed but are never ours. An empty listing is only believed when
/// `kill(-pgid, 0)` agrees: ESRCH (no such group) or EPERM (the group holds
/// only processes we cannot signal, so none of ours).
fn group_members_checked(pgid: i32) -> std::io::Result<Vec<(i32, ProcessProbe)>> {
    let members: Vec<_> = sys::group_members(pgid)?
        .into_iter()
        .filter(|(_, probe)| match probe {
            ProcessProbe::Observed(member) => member.pgid == pgid,
            ProcessProbe::Absent => false,
            ProcessProbe::Foreign => true,
        })
        .collect();
    if members.is_empty() {
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
    Ok(members)
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

/// SIGKILL the verified members (per-member pidfds on Linux, the group on
/// macOS) and wait on kernel exit notification for each, re-listing and
/// re-signalling until the listing is empty or holds only notified zombies.
/// A SIGKILLed process cannot fork, so this converges; the deadline only
/// fails recovery closed.
fn kill_group_and_await_exit(
    entry_id: Uuid,
    pgid: i32,
    observed: Vec<ProcessIdentity>,
    deadline: Instant,
) -> Result<ToolProcessCessation, ProcessCustodyError> {
    let mut killed: BTreeSet<i32> = observed.iter().map(|member| member.pid).collect();
    let mut members = observed;
    loop {
        sys::kill_members(pgid, &members)
            .map_err(|error| ProcessCustodyError::io("kill prior tool process group", error))?;
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

#[cfg(test)]
mod tests;

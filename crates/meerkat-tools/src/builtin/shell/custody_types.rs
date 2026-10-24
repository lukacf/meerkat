//! Platform-independent vocabulary of durable shell process custody.
//!
//! The custody mechanism itself is Linux and macOS only (see
//! `custody/mod.rs`); these types are compiled on every native target so
//! errors can travel typed through dispatcher and agent construction.

use std::path::PathBuf;

pub use meerkat_core::tool_process::{ToolProcessCessation, ToolProcessSpawner};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One earlier-incarnation tool settled by recovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RecoveredToolProcess {
    pub entry_id: Uuid,
    pub prior_incarnation: Uuid,
    /// Provider tool-call id of the interrupted call, when known.
    pub tool_call_id: Option<String>,
    /// The run the process belonged to, when it was spawned inside one.
    pub run_id: Option<meerkat_core::RunId>,
    /// What spawned the process.
    pub spawner: ToolProcessSpawner,
    pub cessation: ToolProcessCessation,
}

/// Outcome of settling a scope's earlier-incarnation custody records.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ProcessCustodyRecoveryReport {
    pub recovered: Vec<RecoveredToolProcess>,
}

/// Result of settling one scope during a realm sweep.
#[derive(Debug)]
#[non_exhaustive]
pub struct ScopeSweep {
    /// The scope (session id) the records belonged to.
    pub scope: String,
    /// The scope's settlement, or why it could not be settled now (for
    /// example [`ProcessCustodyError::PriorIncarnationAlive`] for a session
    /// another live host still serves). A failed scope keeps its records and
    /// is settled again when the session is next built or swept.
    pub outcome: Result<ProcessCustodyRecoveryReport, ProcessCustodyError>,
}

/// Outcome of a realm-level custody sweep over every scope under a custody
/// root, including sessions that are never resumed.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct ProcessCustodySweepReport {
    pub scopes: Vec<ScopeSweep>,
}

/// What the incarnation lock of a host in another pid namespace shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ForeignIncarnationLiveness {
    /// A live process holds the host's incarnation lock: a sibling host
    /// still owns the record.
    Running,
    /// Nothing can prove it either way: the lock file is missing (a record
    /// written before incarnation locks), or the custody root is on a
    /// network or userspace filesystem where `flock(2)` is not a reliable
    /// cross-process proof.
    Unverifiable,
}

impl std::fmt::Display for ForeignIncarnationLiveness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Running => "still running",
            Self::Unverifiable => "not verifiable from here",
        })
    }
}

/// What a Linux host's `/proc` shows of the host when it is not a procfs of
/// the host's own pid namespace (see [`ProcessCustodyError::ForeignProcfs`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ForeignProcfsView {
    /// The host's pid in its own pid namespace.
    pub host_pid: i32,
    /// The host's pid as the procfs shows it (the `Tgid` of the host's own
    /// status record there); `None` when the procfs does not show the host
    /// at all: a procfs of a pid namespace that does not contain the host,
    /// or none mounted.
    pub procfs_pid: Option<i32>,
    /// How many pid namespace levels the procfs's namespace lies above the
    /// host's, when the kernel lists them (`NSpid`, Linux 4.1 and later).
    pub namespace_levels_above: Option<u32>,
}

impl std::fmt::Display for ForeignProcfsView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Some(procfs_pid) = self.procfs_pid else {
            return write!(f, "/proc does not show this host (pid {})", self.host_pid);
        };
        write!(
            f,
            "/proc shows this host (pid {}) as pid {procfs_pid}",
            self.host_pid
        )?;
        match self.namespace_levels_above {
            Some(levels) => write!(f, ", {levels} pid namespace level(s) up"),
            None => Ok(()),
        }
    }
}

impl std::error::Error for ForeignProcfsView {}

/// Errors establishing or recovering process custody.
///
/// Every error fails closed: no custody handle is produced, so no new work
/// for the scope is admitted. Each variant documents the operator action that
/// clears it. Custody records live at
/// `<runtime_root>/tool_process_custody/<session_id>/<entry_id>.json`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProcessCustodyError {
    /// The scope is not a valid path component. A programming error.
    #[error("invalid process custody scope '{0}'")]
    InvalidScope(String),
    /// A custody record could not be written, read, listed or removed, or a
    /// kernel probe failed for a reason other than the process being absent
    /// or not ours. Operator action: fix the named I/O condition (disk full,
    /// permissions on the custody directory) and retry.
    #[error("process custody I/O failed ({context}): {source}")]
    Io {
        context: &'static str,
        #[source]
        source: std::io::Error,
    },
    /// A record is not valid custody JSON, or its file name does not match
    /// its entry id. Operator action: confirm the tool process it named (if
    /// any) is not running, then delete the file at `path`.
    #[error("process custody record {path} is unreadable: {reason}")]
    CorruptRecord { path: PathBuf, reason: String },
    /// A record was written by a custody format this build does not know
    /// (for example by a newer release before a rollback), in the same boot,
    /// by another host incarnation. Its phase and process
    /// identity cannot be interpreted, so cessation cannot be proven.
    /// Operator action: run the newer release again to settle it, or confirm
    /// the tool process is not running and delete the file at `path`.
    #[error("process custody record {path} has unsupported format version {version}")]
    UnsupportedRecordVersion { path: PathBuf, version: u32 },
    /// The host incarnation that owns the entry is still running and may
    /// still supervise the tool (or may still release its spawn gate).
    /// Operator action: stop the live host process `host_pid`, then retry.
    #[error(
        "tool process of custody entry {entry_id} is still owned by live host incarnation {incarnation} (pid {host_pid})"
    )]
    PriorIncarnationAlive {
        entry_id: Uuid,
        incarnation: Uuid,
        host_pid: i32,
    },
    /// The entry was written in this boot but in another pid namespace (for
    /// example another container sharing the realm root), and its host
    /// incarnation's liveness lock does not prove the host ended (see
    /// `ForeignIncarnationLiveness`). Its pids cannot be observed from
    /// here, so nothing was signalled or settled. Operator action: for
    /// `Running`, stop the other host (or let it settle its own session);
    /// for `Unverifiable`, confirm the container that wrote it has stopped,
    /// then delete the entry's record and retry. A reboot also settles it.
    #[error(
        "custody entry {entry_id} of host incarnation {incarnation} was written in another pid namespace of this boot, and its host is {liveness}"
    )]
    ForeignPidNamespace {
        entry_id: Uuid,
        incarnation: Uuid,
        liveness: ForeignIncarnationLiveness,
    },
    /// Recovery SIGKILLed the group but some member did not exit before the
    /// deadline (a process stuck in an uninterruptible kernel wait), or the
    /// kernel refused to let this host signal a member (EPERM, for example a
    /// member that changed credentials). Operator action: end process group
    /// `pgid` with sufficient privilege (for example `kill -KILL -<pgid>`),
    /// or resolve the stuck I/O, then retry.
    #[error(
        "tool process group {pgid} of custody entry {entry_id} did not cease: {live_members} member(s) still running"
    )]
    CessationUnproven {
        entry_id: Uuid,
        pgid: i32,
        live_members: usize,
    },
    /// An earlier incarnation's tool group is still running, but this host
    /// has no kernel exit notification (Linux without `pidfd_open`: kernels
    /// before 5.3, or a seccomp profile that blocks it), so cessation could
    /// never be proven. Nothing was signalled. Operator action: run the host
    /// where pidfds are available, or end process group `pgid` manually and
    /// delete the entry's record, then retry.
    #[error(
        "tool process group {pgid} of custody entry {entry_id} is still running ({live_members} member(s)) and this host has no kernel exit notification"
    )]
    ExitNotificationUnavailable {
        entry_id: Uuid,
        pgid: i32,
        live_members: usize,
    },
    /// This Linux host's `/proc` is not a procfs of its own pid namespace
    /// (for example a host started in a new pid namespace, as by
    /// `unshare --pid --fork`, without a procfs mounted for it), so a pid
    /// read there names another process than the same pid passed to the
    /// host's own syscalls. Custody captured, compared and signalled
    /// nothing, and no tool or hook ran. Operator action: give the host a
    /// procfs of its own pid namespace (`unshare --pid --fork --mount-proc`,
    /// or the container runtime's own `/proc`), then retry.
    #[error(
        "process custody refused ({context}): {view}; custody needs a procfs of this host's own pid namespace"
    )]
    ForeignProcfs {
        context: &'static str,
        view: ForeignProcfsView,
    },
}

impl ProcessCustodyError {
    /// An I/O failure of `context`. A procfs of another pid namespace, which
    /// the platform layer reports through its I/O results, surfaces as
    /// [`Self::ForeignProcfs`].
    #[cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(dead_code))]
    pub(super) fn io(context: &'static str, source: std::io::Error) -> Self {
        match source
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<ForeignProcfsView>())
        {
            Some(view) => Self::ForeignProcfs {
                context,
                view: *view,
            },
            None => Self::Io { context, source },
        }
    }
}

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
    /// (for example by a newer release before a rollback), in the same boot
    /// and pid namespace, by another host incarnation. Its phase and process
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
}

impl ProcessCustodyError {
    #[cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(dead_code))]
    pub(super) fn io(context: &'static str, source: std::io::Error) -> Self {
        Self::Io { context, source }
    }
}

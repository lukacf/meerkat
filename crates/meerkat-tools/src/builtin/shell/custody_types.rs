//! Platform-independent vocabulary of durable shell process custody.
//!
//! The custody mechanism itself is Linux and macOS only (see
//! `custody/mod.rs`); these types are compiled on every native target so
//! errors can travel typed through dispatcher and agent construction.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// How recovery established that an earlier incarnation's tool has ceased.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cessation", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ToolProcessCessation {
    /// The host died before releasing the spawn gate, so the command never
    /// started.
    NeverStarted,
    /// The recorded group had no member left when recovery inspected it: the
    /// tool had already finished or been killed. Its result was not
    /// delivered.
    AlreadyExited,
    /// The recorded leader pid or group id now names a different process or
    /// group (another start stamp, another user, a member that cannot descend
    /// from the recorded leader, or a live group of the current
    /// incarnation). Group ids are not reused while a group exists, so the
    /// recorded group is gone. Nothing was signalled.
    GroupReassigned,
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
    /// deadline (a process stuck in an uninterruptible kernel wait), or a
    /// member could not be signalled. Operator action: inspect and end
    /// process group `pgid` (for example `kill -KILL -<pgid>`, or resolve
    /// the stuck I/O), then retry.
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
    #[cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(dead_code))]
    pub(super) fn io(context: &'static str, source: std::io::Error) -> Self {
        Self::Io { context, source }
    }
}

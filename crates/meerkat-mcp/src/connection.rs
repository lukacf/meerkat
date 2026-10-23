//! MCP connection management

use crate::client_service::{ClientServiceSelection, ConnectedClient, McpClientServiceFactory};
use crate::transport::protected::{
    ProtectedMetadata, ProtectedMetadataState, ProtectedStdioTransport,
};
use crate::transport::sse::{SseClientConfig, SseClientTransport};
use crate::transport::streamable_http::{
    RequestDispatch, RequestDisposition, SessionExpiryRecorder,
};
use crate::transport::{
    EgressGuard, headers_from_map,
    sse::ReqwestSseClient,
    streamable_http::{OAuthBearer, ReqwestStreamableHttpClient},
};
use crate::{McpError, McpStdioLaunchProfile};
use async_trait::async_trait;
use meerkat_auth_core::http_egress::{HttpEgressError, HttpEgressProfile, HttpEgressUnavailable};
use meerkat_auth_core::{McpAuthMode, McpOAuthError, McpServerIdentity};
use meerkat_core::McpServerConfig;
use meerkat_core::ToolDef;
use meerkat_core::auth::{McpServiceSecretRefusal, McpServiceSecretValues};
use meerkat_core::mcp_attachment::{McpAttachmentAuthority, McpStdioSetupFailureClass};
use meerkat_core::mcp_config::{McpHttpTransport, McpTransportConfig};
use meerkat_core::types::ContentBlock;
use rmcp::model::{CallToolRequest, CallToolRequestParams, CallToolResult, ServerResult};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransportConfig, StreamableHttpError,
};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;

/// Connection to an MCP server
pub struct McpConnection {
    config: McpServerConfig,
    connection_id: crate::McpConnectionId,
    protected_metadata: ProtectedMetadataState,
    /// The client service, through its peer; its custody owns the running
    /// service and closes it.
    service: HeldClient,
    /// The stdio server's process, owned until `close` observes its exit.
    stdio_child: Option<StdioChildCustody>,
    /// Set once a Streamable HTTP server drops this connection's session.
    /// Never set for stdio or SSE connections.
    session_expiry: SessionExpiryRecorder,
    /// The OAuth target whose bearer this connection resolves per request.
    oauth_target: Option<McpServerIdentity>,
    /// The caller-held attempt this connection was made in, if any.
    attempt: Option<crate::McpConnectAttempt>,
    /// The account this connection's operations act with, as its owners
    /// stated it when it was made ([`connection_account`]).
    account: Arc<meerkat_core::OperationAccount>,
    /// Full standard discovery, for host replay. Model definitions are a
    /// separate projection and do not carry private UI metadata.
    standard_tools: std::sync::RwLock<Vec<rmcp::model::Tool>>,
}

/// After the process group is killed, how long [`StdioChildCustody::terminate`]
/// keeps waiting for the server to settle (its process exited and its stdout
/// closed by every holder) and for its reap. This bound only guards against a
/// process that escaped the group (for example via `setsid`) and still holds
/// the pipe, or a leader that cannot die, which would otherwise wedge
/// shutdown.
const STDIO_KILL_BACKSTOP: Duration = Duration::from_secs(10);

/// How a failed connect closes a server that never completed initialize:
/// its EOF grace (capped by the custody's own) and its TERM grace (`None`
/// keeps the custody's own). The EOF grace is long enough to observe a
/// server exiting on its own (its exit status is a setup diagnostic) and
/// short enough that the connect ends just past its own deadline; keeping
/// the TERM grace lets a server's SIGTERM handler finish its startup work.
const NEVER_INITIALIZED_CLOSE: (Duration, Option<Duration>) = (Duration::from_millis(250), None);

/// How long a close waits for a remote (process-free) connection's client to
/// end: the default stdio close bound, so a remote close is bounded like a
/// stdio one. Past it the close fails with [`McpError::CloseTimedOut`].
const REMOTE_CLOSE_BOUND: Duration = McpStdioCloseBounds::DEFAULT.close_bound();

/// How often a stdio server's process is checked for exit (without reaping
/// it) while its close waits.
#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
const STDIO_LEADER_EXIT_POLL: Duration = Duration::from_millis(10);

/// How long a stdio MCP server gets to exit on its own when its connection
/// closes, before it is forced.
///
/// Closing first closes the server's stdin. The server then has `eof_grace`
/// to settle, for example after joining work it started. If it has not, its
/// process group gets SIGTERM and `term_grace` more, and then SIGKILL. The
/// graces are phase deadlines on one monotonic clock that starts when the
/// close starts, so a slow client-side cancel uses up the EOF grace instead of
/// extending it, and an expired deadline is acted on before any further
/// output or exit is observed. A close ends within `eof_grace + term_grace`
/// plus a fixed backstop after the kill. A server is "settled" once its
/// process was seen to exit and every holder of its stdout was seen to close
/// it; these deadlines decide only when to escalate. Where either cannot be
/// seen (off Linux, Android and macOS the exit cannot be seen without
/// reaping, and a stdout witness can be missing or fail), the close does not
/// settle early: the server keeps its graces and is escalated against at each
/// deadline.
/// Off Unix the server process alone is waited for and `term_grace` is not
/// used. Both graces are at most [`Self::MAX_GRACE`];
/// [`Self::IMMEDIATE`] escalates at once.
///
/// The bounds are host policy, set on the router
/// ([`crate::McpRouter::with_stdio_close_bounds`]); direct connections use
/// [`Self::DEFAULT`]. Each connection captures the bounds when it spawns its
/// server, so a later change applies only to later connections. They are
/// separate from the router's remove-drain timeout, which bounds in-flight
/// calls before a close starts. The defaults are design choices, not measured
/// bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpStdioCloseBounds {
    eof_grace: Duration,
    term_grace: Duration,
}

/// A refused [`McpStdioCloseBounds`] value.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("MCP stdio close {bound} of {value:?} exceeds the {max:?} maximum")]
pub struct McpStdioCloseBoundsError {
    /// `"eof_grace"` or `"term_grace"`.
    pub bound: &'static str,
    pub value: Duration,
    pub max: Duration,
}

impl McpStdioCloseBounds {
    /// The largest grace either bound accepts.
    pub const MAX_GRACE: Duration = Duration::from_secs(60);
    /// No grace: escalation starts as soon as the close does.
    pub const IMMEDIATE: Self = Self {
        eof_grace: Duration::ZERO,
        term_grace: Duration::ZERO,
    };
    /// The default: 5 s after EOF, then 3 s after SIGTERM.
    pub const DEFAULT: Self = Self {
        eof_grace: Duration::from_secs(5),
        term_grace: Duration::from_secs(3),
    };

    /// Bounds with the given graces. A grace above [`Self::MAX_GRACE`] is
    /// refused, not clamped.
    pub fn new(
        eof_grace: Duration,
        term_grace: Duration,
    ) -> Result<Self, McpStdioCloseBoundsError> {
        for (bound, value) in [("eof_grace", eof_grace), ("term_grace", term_grace)] {
            if value > Self::MAX_GRACE {
                return Err(McpStdioCloseBoundsError {
                    bound,
                    value,
                    max: Self::MAX_GRACE,
                });
            }
        }
        Ok(Self {
            eof_grace,
            term_grace,
        })
    }

    pub fn eof_grace(&self) -> Duration {
        self.eof_grace
    }

    pub fn term_grace(&self) -> Duration {
        self.term_grace
    }

    /// The longest one close under these bounds runs, from its start to its
    /// end: the EOF grace, the TERM grace and the fixed backstop after the
    /// kill. A host that waits for a close derives its deadline from this
    /// bound instead of restating Core's phases.
    pub const fn close_bound(&self) -> Duration {
        self.eof_grace
            .saturating_add(self.term_grace)
            .saturating_add(STDIO_KILL_BACKSTOP)
    }
}

impl Default for McpStdioCloseBounds {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A phase of a stdio server's close: after its stdin closed, after the
/// SIGTERM, after the SIGKILL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum McpStdioClosePhase {
    Eof,
    Term,
    Kill,
}

/// What a close observed of the server's stdout, through a duplicate of the
/// pipe. A closed witness proves only that every handle that could write the
/// pipe was closed, not that the processes holding them exited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpStdioWitnessOutcome {
    /// Every writer closed the pipe during this phase.
    Closed(McpStdioClosePhase),
    /// There was no duplicate to observe (off Unix, or duplicating failed).
    Absent,
    /// Reading the duplicate failed during this phase; nothing more was
    /// observed through it.
    ReadError {
        phase: McpStdioClosePhase,
        kind: std::io::ErrorKind,
    },
    /// Still open when the close stopped watching it: the backstop after the
    /// kill expired, or the group was no longer held for the close.
    StillOpen,
}

/// The result of sending one signal to the server's process group. `Sent`
/// is the signal call's outcome only: it does not show which processes the
/// group held or that any of them exited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpStdioSignalOutcome {
    Sent,
    /// The group had no members (`ESRCH`).
    NoGroup,
    /// Any other failure, as its OS error number.
    Failed(i32),
}

/// What a close saw of the server process's exit before reaping it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpStdioLeaderExitObservation {
    /// Seen to have exited, without reaping it, during this phase.
    Exited(McpStdioClosePhase),
    /// Watched, and not seen to exit before the close stopped watching.
    NotSeen,
    /// Watching failed during this phase, with this OS error number (`None`
    /// when the process had no id). Its exit is unknown from then on, and
    /// with `ECHILD` (no longer a child of this process, so possibly reaped
    /// elsewhere) its group id is no longer held for the close, so the group
    /// is not signalled again.
    Lost {
        phase: McpStdioClosePhase,
        errno: Option<i32>,
    },
    /// This platform cannot see the exit without reaping it (off Linux,
    /// Android and macOS).
    Unobservable,
}

/// How the close ended the server process itself.
#[derive(Debug)]
#[non_exhaustive]
pub enum McpStdioLeaderOutcome {
    /// Reaped with this exit status.
    Reaped(std::process::ExitStatus),
    /// Waiting for it failed.
    WaitFailed(std::io::Error),
    /// Not reaped by the backstop after the kill. It has been killed and is
    /// left to the runtime's orphan reaping.
    NotReaped,
}

/// What closing a stdio MCP server observed, in place of a bare exit status.
#[derive(Debug)]
pub struct McpStdioCloseReceipt {
    /// The phase in which the server settled: its process was seen to exit
    /// ([`McpStdioLeaderExitObservation::Exited`]) and every holder of its
    /// stdout was seen to close it ([`McpStdioWitnessOutcome::Closed`]).
    /// `None` when it did not settle, including whenever either could not be
    /// seen (an unobservable or lost exit, an absent or failed witness).
    pub settled_in: Option<McpStdioClosePhase>,
    pub leader_exit: McpStdioLeaderExitObservation,
    pub witness: McpStdioWitnessOutcome,
    /// The escalation SIGTERM, when the EOF grace expired unsettled and the
    /// group was still held for the close.
    pub sigterm: Option<McpStdioSignalOutcome>,
    /// The escalation SIGKILL, when the TERM grace expired unsettled and the
    /// group was still held for the close.
    pub sigkill: Option<McpStdioSignalOutcome>,
    /// The SIGKILL sent to the group on a Unix close, after settling or
    /// giving up and before the reap, while the unreaped server process still
    /// holds the group id. It signals whatever the server left in its group;
    /// `Sent` does not show that any of it exited. `None` when the group was
    /// no longer held for the close.
    pub final_group_signal: Option<McpStdioSignalOutcome>,
    pub leader: McpStdioLeaderOutcome,
    /// From the start of the close to the reap.
    pub elapsed: Duration,
}

/// Typed owner of a stdio MCP server's process.
///
/// The router (or a direct connection) holds this from the moment the process
/// is spawned, before the handshake, so no connect future exclusively owns the
/// process. [`Self::terminate`] ends it under the custody's
/// [`McpStdioCloseBounds`]: on Unix the server runs in its own process group,
/// which is signalled as a whole (wrappers such as `sh -c`, `npx` or `uvx`
/// make the real server a grandchild), and a duplicate of its stdout shows
/// when every holder has closed the pipe. Elsewhere only the direct child is
/// waited for, killed and reaped. Exactly one `terminate` owns the close.
#[derive(Clone, Default)]
pub(crate) struct StdioChildCustody {
    slot: Arc<std::sync::Mutex<CustodySlot>>,
    bounds: McpStdioCloseBounds,
    /// The setup diagnostics of the attempt this custody was admitted for,
    /// held weakly: its router owns them, so a process or a clone of this
    /// custody never keeps them alive.
    diagnostics: std::sync::Weak<crate::stdio_diagnostics::StdioAttemptRecord>,
    /// The attempt's guard record while its process end is unknown, shared
    /// with a running close. Declared after `slot`: when the last holder of
    /// a held process drops, its group is killed before the guard learns it
    /// was abandoned.
    guard: Arc<GuardHolder>,
    /// Bumped whenever a close's state changes after its task started, so
    /// its waiters recheck the slot.
    close_progress: Arc<tokio::sync::watch::Sender<u64>>,
    /// Whether its server completed the MCP initialize handshake. A failed
    /// connect closes a server that never did with no grace.
    initialized: Arc<std::sync::atomic::AtomicBool>,
    #[cfg(test)]
    spawned: Arc<tokio::sync::watch::Sender<Option<u32>>>,
    #[cfg(all(test, unix))]
    reap_gate: Arc<std::sync::Mutex<Option<StdioReapTestGate>>>,
    #[cfg(all(test, unix))]
    direct_kill_requests: Arc<std::sync::atomic::AtomicUsize>,
}

/// Holds the existing reap await pending so cancellation tests do not depend
/// on how quickly the operating system delivers SIGKILL.
#[cfg(all(test, unix))]
struct StdioReapTestGate {
    entered: tokio::sync::oneshot::Sender<()>,
    resume: tokio::sync::oneshot::Receiver<()>,
}

/// Where a custody's process stands.
#[derive(Default)]
enum CustodySlot {
    /// Nothing spawned yet (or the spawn failed).
    #[default]
    Vacant,
    Held(StdioChild),
    /// A close task, started by a `terminate`, owns the process; its phase
    /// deadlines count from `started`, and it ends within `bound`.
    Closing {
        started: tokio::time::Instant,
        bound: Duration,
    },
    /// That close task finished. Its receipt stays here, shared, for every
    /// caller that asks: the `terminate` that started the close and every
    /// join ([`StdioChildCustody::terminate_or_join`]), so a join never
    /// loses it.
    Closed(Arc<McpStdioCloseReceipt>),
    /// That close task was dropped before finishing (its runtime shut down);
    /// the process group was killed by the drop, without its exit being
    /// observed.
    Abandoned,
    /// Closed before anything was spawned: a later spawn refuses, so no
    /// process starts after the close.
    Retired,
}

/// Why a [`StdioChildCustody::terminate`] call did not close anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CustodyNotHeld {
    NeverSpawned,
    /// Another `terminate` owns the close and has not finished: this says
    /// nothing about its outcome.
    ClosingElsewhere,
    ClosedElsewhere,
    AbandonedElsewhere,
}

#[cfg(unix)]
type StdoutWitness = Box<dyn tokio::io::AsyncRead + Send + Unpin>;

struct StdioChild {
    child: meerkat_sandbox::ProcessChild,
    /// The drain of the server's stderr. Dropped with this process holder
    /// (after its close, or by the fallback), which starts the drain's
    /// bounded grace for the last bytes.
    _stderr_drain: Option<crate::stdio_diagnostics::StderrDrain>,
    /// The server's process group, while it may still need signalling.
    /// Cleared after the final group signal and before the leader's reap (the
    /// unreaped leader keeps the id from being reused), so a reused id is
    /// never signalled.
    #[cfg(unix)]
    process_group: Option<nix::unistd::Pid>,
    #[cfg(unix)]
    stdout_witness: Option<StdoutWitness>,
    /// Test hook: runs right before the close's final leader check.
    #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
    before_final_check: Option<Box<dyn FnOnce() + Send>>,
    /// Test hooks shared with the custody: the gate before the close's reap
    /// and the count of direct-child kills.
    #[cfg(all(test, unix))]
    reap_gate: Arc<std::sync::Mutex<Option<StdioReapTestGate>>>,
    #[cfg(all(test, unix))]
    direct_kill_requests: Arc<std::sync::atomic::AtomicUsize>,
}

impl Drop for StdioChild {
    /// Fallback only, for a custody dropped without `terminate` or a close
    /// dropped before its reap: kill the whole group without waiting (the
    /// child handle's own drop covers the direct child).
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(group) = self.process_group.take() {
            let _ = nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGKILL);
        }
    }
}

/// Marks the custody `Closed`, with the receipt, when its close task
/// finishes, and `Abandoned` when that task is dropped first; either way
/// then wakes the close's waiters.
struct ClosingOwner {
    slot: Arc<std::sync::Mutex<CustodySlot>>,
    close_progress: Arc<tokio::sync::watch::Sender<u64>>,
    guard: Arc<GuardHolder>,
    finished: bool,
}

impl ClosingOwner {
    fn finish(mut self, receipt: McpStdioCloseReceipt) {
        self.finished = true;
        let receipt = Arc::new(receipt);
        let record = {
            let mut slot = lock_slot(&self.slot);
            *slot = CustodySlot::Closed(Arc::clone(&receipt));
            self.guard.take()
        };
        // Outside the slot lock: the guard learns the close's own receipt.
        if let Some(record) = record {
            record.process_ended(crate::McpAttemptProcessEnd::Closed(receipt));
        }
    }
}

impl Drop for ClosingOwner {
    fn drop(&mut self) {
        if !self.finished {
            let record = {
                let mut slot = lock_slot(&self.slot);
                *slot = CustodySlot::Abandoned;
                self.guard.take()
            };
            if let Some(record) = record {
                record.process_ended(crate::McpAttemptProcessEnd::Abandoned);
            }
        }
        self.close_progress
            .send_modify(|seen| *seen = seen.wrapping_add(1));
    }
}

/// An attempt's guard record, held with its process custody until the
/// process end is known. Taken and set only under the custody's slot lock;
/// the record is told outside it.
#[derive(Default)]
pub(crate) struct GuardHolder {
    record: std::sync::Mutex<Option<Arc<crate::attempt_guard::AttemptGuardRecord>>>,
    /// Whether a process was spawned under this custody.
    spawned: std::sync::atomic::AtomicBool,
}

impl GuardHolder {
    fn take(&self) -> Option<Arc<crate::attempt_guard::AttemptGuardRecord>> {
        self.record
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    fn hold(&self, record: Arc<crate::attempt_guard::AttemptGuardRecord>) {
        *self
            .record
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(record);
    }
}

impl Drop for GuardHolder {
    /// The last holder is gone with the process end still unknown: a spawned
    /// process was killed by its holder's drop (abandoned), or nothing was
    /// spawned.
    fn drop(&mut self) {
        let record = self
            .record
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(record) = record {
            record.process_ended(if *self.spawned.get_mut() {
                crate::McpAttemptProcessEnd::Abandoned
            } else {
                crate::McpAttemptProcessEnd::NotSpawned
            });
        }
    }
}

fn lock_slot(slot: &std::sync::Mutex<CustodySlot>) -> std::sync::MutexGuard<'_, CustodySlot> {
    slot.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn confined_stdio_spawn_error(error: std::io::Error) -> McpError {
    match error
        .get_ref()
        .and_then(|source| source.downcast_ref::<meerkat_core::confinement::ConfinementRefusal>())
    {
        Some(refusal) => McpError::Confinement(*refusal),
        None => McpError::Io(error),
    }
}

/// The governed control of one connection attempt, asked once at its
/// physical entry: after preparation (launch binding, custody, credential
/// waits) and immediately before the process spawn or first transport
/// request. Once entered, the attempt keeps that entry: a later policy change
/// does not cancel or rewrite an effect that already began.
#[derive(Default)]
pub(crate) struct PhysicalEntry(Option<Arc<dyn McpAttachmentAuthority>>);

impl PhysicalEntry {
    pub(crate) fn new(control: Option<Arc<dyn McpAttachmentAuthority>>) -> Self {
        Self(control)
    }

    fn enter(&mut self) -> Result<(), McpError> {
        match self.0.take() {
            Some(control) => control
                .authorize_apply()
                .map_err(McpError::AttachmentRefused),
            None => Ok(()),
        }
    }

    /// Hand the pending control to an HTTP attempt's first-request owner,
    /// which asks it at that request's own physical entry.
    fn into_first_request(self) -> crate::transport::streamable_http::FirstRequestEntry {
        crate::transport::streamable_http::FirstRequestEntry::pending(self.0)
    }
}

impl StdioChildCustody {
    /// A custody whose close uses `bounds`.
    pub(crate) fn with_bounds(bounds: McpStdioCloseBounds) -> Self {
        Self {
            bounds,
            ..Self::default()
        }
    }

    /// This custody, recording its attempt's setup diagnostics in `record`
    /// (held weakly).
    #[must_use]
    pub(crate) fn with_diagnostics(
        mut self,
        record: std::sync::Weak<crate::stdio_diagnostics::StdioAttemptRecord>,
    ) -> Self {
        self.diagnostics = record;
        self
    }

    /// The attempt's diagnostics record while its router still holds it.
    /// Callers use it in a short scope and never hold it across an await.
    pub(crate) fn diagnostics_record(
        &self,
    ) -> Option<Arc<crate::stdio_diagnostics::StdioAttemptRecord>> {
        self.diagnostics.upgrade()
    }

    pub(crate) fn diagnostics_handle(
        &self,
    ) -> std::sync::Weak<crate::stdio_diagnostics::StdioAttemptRecord> {
        self.diagnostics.clone()
    }

    /// Hold the attempt's guard record until the process end is known: told
    /// `NotSpawned` when the custody is retired before a spawn, the close's
    /// receipt when its close finishes, `Abandoned` when that close is
    /// dropped or the last holder of a spawned process drops.
    pub(crate) fn hold_guard(&self, record: Arc<crate::attempt_guard::AttemptGuardRecord>) {
        let known = {
            let slot = lock_slot(&self.slot);
            match &*slot {
                CustodySlot::Retired => Some(crate::McpAttemptProcessEnd::NotSpawned),
                CustodySlot::Closed(receipt) => {
                    Some(crate::McpAttemptProcessEnd::Closed(Arc::clone(receipt)))
                }
                CustodySlot::Abandoned => Some(crate::McpAttemptProcessEnd::Abandoned),
                CustodySlot::Vacant | CustodySlot::Held(_) | CustodySlot::Closing { .. } => {
                    self.guard.hold(Arc::clone(&record));
                    None
                }
            }
        };
        if let Some(end) = known {
            record.process_ended(end);
        }
    }

    /// The guard record taken out of the custody when its process end
    /// becomes known, told outside the slot lock.
    fn tell_guard(
        record: Option<Arc<crate::attempt_guard::AttemptGuardRecord>>,
        end: crate::McpAttemptProcessEnd,
    ) {
        if let Some(record) = record {
            record.process_ended(end);
        }
    }

    /// Spawn the server and take custody of it; returns its stdout and stdin
    /// for the MCP transport. A configured working directory
    /// ([`McpStdioConfig::cwd`](meerkat_core::mcp_config::McpStdioConfig::cwd))
    /// must be an existing directory: otherwise nothing is spawned, and the
    /// launch never falls back to another directory.
    async fn spawn(
        &self,
        server: &str,
        stdio: &meerkat_core::mcp_config::McpStdioConfig,
        profile: &McpStdioLaunchProfile,
        entry: &mut PhysicalEntry,
    ) -> Result<(tokio::process::ChildStdout, tokio::process::ChildStdin), McpError> {
        // Inspected before custody is taken, so a slow filesystem never holds
        // the custody lock; the refusal is reported after the custody's own.
        let cwd_refusal = stdio.cwd.as_deref().and_then(working_directory_refusal);
        // Acquire custody before spawning; nothing in this spawn awaits, so
        // nothing separates creating the child from installing its owner.
        let mut slot = lock_slot(&self.slot);
        // A custody closed before any spawn refuses one.
        if matches!(*slot, CustodySlot::Retired) {
            return Err(McpError::RouterShutDown);
        }
        if let Some(reason) = cwd_refusal {
            return Err(meerkat_core::McpStdioCwdError {
                server: server.to_owned(),
                reason,
            }
            .into());
        }
        // Binding validates the exact launch and retained backend before any
        // process exists. Required isolation has no Command fallback.
        let prepared = profile.prepare(stdio)?;
        // The attempt's diagnostics keep a keyed digest of each launch value
        // (never the value) to redact it from the server's stderr tail:
        // resolved secrets and the connector binding first.
        let diagnostics = self.diagnostics_record();
        if let Some(record) = &diagnostics {
            record.track_launch_values(crate::stdio_diagnostics::launch_values_by_priority(stdio));
        }
        // Physical entry: after custody and launch preparation, before the
        // process exists.
        entry.enter()?;
        // A trusted-host server's drained stderr is also written to this
        // process's own stderr, as its inherited stderr was. A confined
        // server's never is: its stderr is a fresh pipe, never a host
        // descriptor.
        let forward_stderr = prepared.is_none();
        let spawn_failed = || {
            if let Some(record) = &diagnostics {
                record.classify(McpStdioSetupFailureClass::SpawnFailed);
            }
        };
        let mut child: meerkat_sandbox::ProcessChild = match prepared {
            Some(prepared) => {
                #[cfg(any(target_os = "macos", target_os = "linux"))]
                {
                    prepared
                        .spawn_with_io(meerkat_sandbox::SpawnIo {
                            stdin: meerkat_sandbox::StdioMode::Piped,
                            stdout: meerkat_sandbox::StdioMode::Piped,
                            stderr: meerkat_sandbox::StdioMode::Piped,
                        })
                        .map_err(|error| {
                            let error = confined_stdio_spawn_error(error);
                            if matches!(error, McpError::Io(_)) {
                                spawn_failed();
                            }
                            error
                        })?
                        .into()
                }
                #[cfg(not(any(target_os = "macos", target_os = "linux")))]
                {
                    let _ = prepared;
                    return Err(
                        meerkat_core::confinement::ConfinementRefusal::UnsupportedRequirement
                            .into(),
                    );
                }
            }
            None => {
                // A launch site: the trusted-host branch of an unconfined
                // `mcp_stdio` launch.
                #[allow(clippy::disallowed_methods)]
                let mut cmd = Command::new(&stdio.command);
                cmd.args(&stdio.args);
                for (key, value) in &stdio.env {
                    cmd.env(key, value);
                }
                // The configured working directory, exactly as written; the
                // child's change into it failing fails the spawn.
                if let Some(cwd) = &stdio.cwd {
                    cmd.current_dir(cwd);
                }
                cmd.stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    // Fallback only: the owner path is `terminate`.
                    .kill_on_drop(true);
                #[cfg(unix)]
                cmd.process_group(0);
                cmd.spawn()
                    .map_err(|e| {
                        spawn_failed();
                        McpError::ConnectionFailed {
                            reason: format!("Failed to spawn process: {e}"),
                        }
                    })?
                    .into()
            }
        };
        let (Some(stdin), Some(stdout)) = (child.take_stdin(), child.take_stdout()) else {
            spawn_failed();
            return Err(McpError::ConnectionFailed {
                reason: "spawned MCP server has no piped stdin/stdout".to_string(),
            });
        };
        // Drained from now on, before anything awaits, so the server never
        // blocks on a full stderr pipe; the newest bytes go to the
        // attempt's tail while its router holds it.
        if let Some(record) = &diagnostics {
            record.mark_spawned();
        }
        let stderr_drain = child.take_stderr().map(|stderr| {
            crate::stdio_diagnostics::start_stderr_drain(
                stderr,
                self.diagnostics_handle(),
                forward_stderr,
            )
        });
        #[cfg(unix)]
        let stdout_witness = {
            use std::os::fd::AsFd as _;
            stdout
                .as_fd()
                .try_clone_to_owned()
                .ok()
                .and_then(|fd| tokio::net::unix::pipe::Receiver::from_owned_fd(fd).ok())
                .map(|witness| Box::new(witness) as StdoutWitness)
        };
        #[cfg(unix)]
        let process_group = child
            .id()
            .and_then(|pid| i32::try_from(pid).ok())
            .map(nix::unistd::Pid::from_raw);
        #[cfg(test)]
        self.spawned.send_replace(child.id());
        let replaced = std::mem::replace(
            &mut *slot,
            CustodySlot::Held(StdioChild {
                child,
                _stderr_drain: stderr_drain,
                #[cfg(unix)]
                process_group,
                #[cfg(unix)]
                stdout_witness,
                #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
                before_final_check: None,
                #[cfg(all(test, unix))]
                reap_gate: Arc::clone(&self.reap_gate),
                #[cfg(all(test, unix))]
                direct_kill_requests: Arc::clone(&self.direct_kill_requests),
            }),
        );
        self.guard
            .spawned
            .store(true, std::sync::atomic::Ordering::Release);
        // One custody holds one process; a replaced one is killed by its drop.
        drop(replaced);
        Ok((stdout, stdin))
    }

    /// [`Self::terminate_from`] starting the close clock now.
    pub(crate) async fn terminate(&self) -> Result<Arc<McpStdioCloseReceipt>, CustodyNotHeld> {
        self.terminate_from(tokio::time::Instant::now()).await
    }

    /// [`Self::terminate_after`] with nothing to run first.
    pub(crate) async fn terminate_from(
        &self,
        started: tokio::time::Instant,
    ) -> Result<Arc<McpStdioCloseReceipt>, CustodyNotHeld> {
        self.terminate_after(started, std::future::ready(()))
            .await
            .1
    }

    /// End the server under this custody's bounds, with phase deadlines
    /// measured from `started` (when the close began), and report what was
    /// observed. `first` (closing the server's stdin) runs at the start of the
    /// close, in its task; its output is returned when this call saw it. Only
    /// the call that takes the process owns the close; any other runs `first`
    /// itself and reports why it did not close, without waiting for or
    /// vouching for the owner's close.
    ///
    /// On Unix the close waits for the server to settle, escalating at each
    /// phase deadline: SIGTERM to the group once the EOF grace has passed,
    /// SIGKILL to the group once the TERM grace has, and giving up
    /// [`STDIO_KILL_BACKSTOP`] after that. A server whose exit or stdout
    /// cannot be seen does not settle, and is escalated against at each
    /// deadline. Then, while the leader is still unreaped (so its id cannot
    /// have been reused), the group gets one more SIGKILL, which signals any
    /// member the server left behind; the group id is disarmed; and only then
    /// is the leader reaped, once, by the same backstop deadline. On Linux,
    /// Android and macOS, while the leader is still taken as this process's child (not
    /// seen to exit, or seen to exit and unreaped), a non-blocking check right
    /// before each escalation and the final group signal confirms it: once it
    /// is not (`ECHILD`), its group id is no longer held, so the group is not
    /// signalled again, and the close does not count as settled. Elsewhere
    /// the leader cannot be checked without reaping it.
    ///
    /// The close, `first` included, runs in a task of its own, started before
    /// this call first waits: dropping the returned future stops only this
    /// wait, and the close still finishes, keeping its receipt for
    /// [`Self::terminate_or_join`]. Only dropping the task itself (its
    /// runtime shutting down) cancels the close: the held child's drop then
    /// kills the group without observing its exit (or, past the final signal,
    /// leaves the killed leader to the runtime's reaping), and the custody
    /// records the close as abandoned.
    pub(crate) async fn terminate_after<T: Send + 'static>(
        &self,
        started: tokio::time::Instant,
        first: impl std::future::Future<Output = T> + Send + 'static,
    ) -> (Option<T>, Result<Arc<McpStdioCloseReceipt>, CustodyNotHeld>) {
        self.terminate_after_under(started, first, self.bounds)
            .await
    }

    /// [`Self::terminate_after`] under `bounds`.
    async fn terminate_after_under<T: Send + 'static>(
        &self,
        started: tokio::time::Instant,
        first: impl std::future::Future<Output = T> + Send + 'static,
        bounds: McpStdioCloseBounds,
    ) -> (Option<T>, Result<Arc<McpStdioCloseReceipt>, CustodyNotHeld>) {
        let mut progress = self.close_progress.subscribe();
        let mut first_output = match self.start_close_under(started, first, bounds) {
            Ok(first_output) => first_output,
            Err((first, not_held)) => return (Some(first.await), Err(not_held)),
        };
        loop {
            let finished = match &*lock_slot(&self.slot) {
                // Shared, not taken: every join reads the same receipt.
                CustodySlot::Closed(receipt) => Some(Ok(Arc::clone(receipt))),
                CustodySlot::Abandoned => Some(Err(CustodyNotHeld::AbandonedElsewhere)),
                CustodySlot::Vacant
                | CustodySlot::Retired
                | CustodySlot::Held(_)
                | CustodySlot::Closing { .. } => None,
            };
            if let Some(finished) = finished {
                return (first_output.try_recv().ok(), finished);
            }
            // The custody holds the sender, so this never fails while waiting.
            let _ = progress.changed().await;
        }
    }

    /// Start this custody's close now, from synchronous code: take the held
    /// process and start its close task, which runs `first` before observing
    /// anything ([`Self::terminate_after`]). A custody never spawned is
    /// retired, so a later spawn refuses. Returns `first`'s output channel,
    /// or `first` back with why there was nothing to start.
    #[allow(clippy::type_complexity)]
    pub(crate) fn start_close<T, F>(
        &self,
        started: tokio::time::Instant,
        first: F,
    ) -> Result<tokio::sync::oneshot::Receiver<T>, (F, CustodyNotHeld)>
    where
        T: Send + 'static,
        F: std::future::Future<Output = T> + Send + 'static,
    {
        self.start_close_under(started, first, self.bounds)
    }

    /// [`Self::start_close`] under `bounds`.
    #[allow(clippy::type_complexity)]
    fn start_close_under<T, F>(
        &self,
        started: tokio::time::Instant,
        first: F,
        bounds: McpStdioCloseBounds,
    ) -> Result<tokio::sync::oneshot::Receiver<T>, (F, CustodyNotHeld)>
    where
        T: Send + 'static,
        F: std::future::Future<Output = T> + Send + 'static,
    {
        let mut never_spawned = None;
        let held = {
            let mut slot = lock_slot(&self.slot);
            let closing = CustodySlot::Closing {
                started,
                bound: bounds.close_bound(),
            };
            match std::mem::replace(&mut *slot, closing) {
                CustodySlot::Held(held) => Ok(held),
                // Closed before any spawn: no process may start after it.
                CustodySlot::Vacant | CustodySlot::Retired => {
                    *slot = CustodySlot::Retired;
                    never_spawned = self.guard.take();
                    Err(CustodyNotHeld::NeverSpawned)
                }
                other => {
                    let not_held = match &other {
                        CustodySlot::Closing { .. } => CustodyNotHeld::ClosingElsewhere,
                        CustodySlot::Closed(_) => CustodyNotHeld::ClosedElsewhere,
                        CustodySlot::Abandoned
                        | CustodySlot::Held(_)
                        | CustodySlot::Vacant
                        | CustodySlot::Retired => CustodyNotHeld::AbandonedElsewhere,
                    };
                    *slot = other;
                    Err(not_held)
                }
            }
        };
        Self::tell_guard(never_spawned, crate::McpAttemptProcessEnd::NotSpawned);
        let held = match held {
            Ok(held) => held,
            Err(not_held) => return Err((first, not_held)),
        };
        let owner = ClosingOwner {
            slot: Arc::clone(&self.slot),
            close_progress: Arc::clone(&self.close_progress),
            guard: Arc::clone(&self.guard),
            finished: false,
        };
        let (first_done, first_output) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let mut held = held;
            let _ = first_done.send(first.await);
            let receipt = close_held(&mut held, bounds, started).await;
            drop(held);
            owner.finish(receipt);
        });
        Ok(first_output)
    }

    /// Whether nothing of this custody is left to report: its close finished
    /// and settled, or it was retired before any spawn.
    fn finished_settled(&self) -> bool {
        match &*lock_slot(&self.slot) {
            CustodySlot::Closed(receipt) => receipt_settled(receipt),
            CustodySlot::Retired => true,
            CustodySlot::Vacant
            | CustodySlot::Held(_)
            | CustodySlot::Closing { .. }
            | CustodySlot::Abandoned => false,
        }
    }

    /// When the close running now will have ended: its start plus its
    /// [`McpStdioCloseBounds::close_bound`]. `None` when no close runs.
    fn close_deadline(&self) -> Option<tokio::time::Instant> {
        match &*lock_slot(&self.slot) {
            CustodySlot::Closing { started, bound } => Some(*started + *bound),
            CustodySlot::Vacant
            | CustodySlot::Held(_)
            | CustodySlot::Closed(_)
            | CustodySlot::Abandoned
            | CustodySlot::Retired => None,
        }
    }

    /// Whether two handles share one custody.
    pub(crate) fn same_custody(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.slot, &other.slot)
    }

    /// Whether a process is held whose close has not finished: held, or
    /// closing.
    pub(crate) fn close_pending(&self) -> bool {
        matches!(
            *lock_slot(&self.slot),
            CustodySlot::Held(_) | CustodySlot::Closing { .. }
        )
    }

    /// Retire a custody whose process was never spawned, so none ever is.
    fn retire_if_vacant(&self) {
        let never_spawned = {
            let mut slot = lock_slot(&self.slot);
            if matches!(*slot, CustodySlot::Vacant) {
                *slot = CustodySlot::Retired;
                self.guard.take()
            } else {
                None
            }
        };
        Self::tell_guard(never_spawned, crate::McpAttemptProcessEnd::NotSpawned);
    }

    /// Record that this custody's server completed the MCP initialize
    /// handshake.
    pub(crate) fn mark_initialized(&self) {
        self.initialized
            .store(true, std::sync::atomic::Ordering::Release);
    }

    /// The close of a failed connect. A server that never completed
    /// initialize closes under [`NEVER_INITIALIZED_CLOSE`]: a cut EOF grace
    /// (enough to observe a server exiting on its own) and its TERM grace,
    /// so the connect ends just past its own deadline. One that did complete
    /// initialize keeps this custody's graces.
    pub(crate) async fn terminate_failed_connect(
        &self,
    ) -> Result<Arc<McpStdioCloseReceipt>, CustodyNotHeld> {
        let bounds = if self.initialized.load(std::sync::atomic::Ordering::Acquire) {
            self.bounds
        } else {
            let (eof_grace, term_grace) = NEVER_INITIALIZED_CLOSE;
            McpStdioCloseBounds {
                eof_grace: self.bounds.eof_grace.min(eof_grace),
                term_grace: term_grace.unwrap_or(self.bounds.term_grace),
            }
        };
        self.terminate_after_under(tokio::time::Instant::now(), std::future::ready(()), bounds)
            .await
            .1
    }

    /// [`Self::terminate_failed_connect`], or, when another call already
    /// started the close, wait for that close to finish.
    pub(crate) async fn terminate_failed_connect_or_join(
        &self,
    ) -> Result<Arc<McpStdioCloseReceipt>, CustodyNotHeld> {
        match self.terminate_failed_connect().await {
            Err(CustodyNotHeld::ClosingElsewhere | CustodyNotHeld::ClosedElsewhere) => {
                self.join_close().await
            }
            terminated => terminated,
        }
    }

    /// [`Self::terminate`], or, when another call already started the close,
    /// wait for that close to finish ([`Self::join_close`]). Either way it
    /// returns that close's one receipt, shared with every other caller.
    pub(crate) async fn terminate_or_join(
        &self,
    ) -> Result<Arc<McpStdioCloseReceipt>, CustodyNotHeld> {
        match self.terminate().await {
            Err(CustodyNotHeld::ClosingElsewhere | CustodyNotHeld::ClosedElsewhere) => {
                self.join_close().await
            }
            terminated => terminated,
        }
    }

    /// Wait for the close another call started to finish, and return its
    /// receipt.
    pub(crate) async fn join_close(&self) -> Result<Arc<McpStdioCloseReceipt>, CustodyNotHeld> {
        let mut progress = self.close_progress.subscribe();
        loop {
            match &*lock_slot(&self.slot) {
                CustodySlot::Closed(receipt) => return Ok(Arc::clone(receipt)),
                CustodySlot::Abandoned => return Err(CustodyNotHeld::AbandonedElsewhere),
                CustodySlot::Vacant | CustodySlot::Retired => {
                    return Err(CustodyNotHeld::NeverSpawned);
                }
                // A custody's one process is closed at most once, so a held
                // process means no close of it was started.
                CustodySlot::Held(_) => return Err(CustodyNotHeld::ClosedElsewhere),
                CustodySlot::Closing { .. } => {}
            }
            let _ = progress.changed().await;
        }
    }

    /// The receipt of this custody's finished close, if it has one.
    pub(crate) fn closed_receipt(&self) -> Option<Arc<McpStdioCloseReceipt>> {
        match &*lock_slot(&self.slot) {
            CustodySlot::Closed(receipt) => Some(Arc::clone(receipt)),
            CustodySlot::Vacant
            | CustodySlot::Retired
            | CustodySlot::Held(_)
            | CustodySlot::Closing { .. }
            | CustodySlot::Abandoned => None,
        }
    }

    /// Test hook: whether a close task owns the process.
    #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
    pub(crate) fn is_closing(&self) -> bool {
        matches!(*lock_slot(&self.slot), CustodySlot::Closing { .. })
    }

    /// Test hook: whether the process is still held, with no close started.
    #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
    pub(crate) fn is_held(&self) -> bool {
        matches!(*lock_slot(&self.slot), CustodySlot::Held(_))
    }

    /// Test hook: wait until a close task has finished (or was dropped).
    #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
    pub(crate) async fn close_finished(&self) {
        let mut progress = self.close_progress.subscribe();
        while !matches!(
            *lock_slot(&self.slot),
            CustodySlot::Closed(_) | CustodySlot::Abandoned
        ) {
            let _ = progress.changed().await;
        }
    }

    /// Test hook: run `hook` right before the held process's close makes its
    /// final leader check.
    #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
    pub(crate) fn before_final_check(&self, hook: impl FnOnce() + Send + 'static) {
        if let CustodySlot::Held(held) = &mut *lock_slot(&self.slot) {
            held.before_final_check = Some(Box::new(hook));
        }
    }

    /// Test hook: replace the stdout witness of the held process.
    #[cfg(all(test, unix))]
    pub(crate) fn replace_witness(&self, witness: Option<StdoutWitness>) {
        if let CustodySlot::Held(held) = &mut *lock_slot(&self.slot) {
            held.stdout_witness = witness;
        }
    }

    /// Test hook: hold the next close's single reap pending, once its final
    /// group signal was sent, until the returned sender fires or drops.
    #[cfg(all(test, unix))]
    fn pause_next_reap(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (entered, waiting) = tokio::sync::oneshot::channel();
        let (resume, resumed) = tokio::sync::oneshot::channel();
        *self
            .reap_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(StdioReapTestGate {
            entered,
            resume: resumed,
        });
        (waiting, resume)
    }

    /// The spawned process id, once spawned.
    #[cfg(all(test, unix))]
    #[allow(clippy::expect_used)]
    pub(crate) async fn spawned_pid(&self) -> u32 {
        let mut spawned = self.spawned.subscribe();
        let pid = spawned
            .wait_for(Option::is_some)
            .await
            .expect("custody owns its spawn signal");
        pid.expect("waited for a spawned pid")
    }
}

/// The phase deadlines of one close, on the clock that started with it.
struct CloseDeadlines {
    eof: tokio::time::Instant,
    #[cfg_attr(not(unix), allow(dead_code))]
    term: tokio::time::Instant,
    backstop: tokio::time::Instant,
}

impl CloseDeadlines {
    fn new(started: tokio::time::Instant, bounds: McpStdioCloseBounds) -> Self {
        let eof = started + bounds.eof_grace();
        let term = eof + bounds.term_grace();
        Self {
            eof,
            term,
            backstop: term + STDIO_KILL_BACKSTOP,
        }
    }
}

/// [`StdioChildCustody::terminate_from`]'s close of the taken process.
#[cfg(unix)]
async fn close_held(
    held: &mut StdioChild,
    bounds: McpStdioCloseBounds,
    started: tokio::time::Instant,
) -> McpStdioCloseReceipt {
    use nix::sys::signal::Signal;
    use tokio::io::AsyncReadExt as _;

    let deadlines = CloseDeadlines::new(started, bounds);
    let leader = held.child.id();
    let mut leader_exit = if cfg!(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos"
    )) {
        McpStdioLeaderExitObservation::NotSeen
    } else {
        McpStdioLeaderExitObservation::Unobservable
    };
    let mut witness_reader = held.stdout_witness.take();
    let mut witness = if witness_reader.is_some() {
        None
    } else {
        Some(McpStdioWitnessOutcome::Absent)
    };
    let mut phase = McpStdioClosePhase::Eof;
    let mut settled_in = None;
    let (mut sigterm, mut sigkill) = (None, None);
    let mut direct_killed = false;
    let mut discard = [0u8; 4096];
    loop {
        if matches!(leader_exit, McpStdioLeaderExitObservation::Exited(_))
            && matches!(witness, Some(McpStdioWitnessOutcome::Closed(_)))
        {
            settled_in = Some(phase);
            break;
        }
        if leader_lost_to_another_reaper(leader_exit) {
            // The leader is not this process's child any more, so nothing
            // holds its group id for the close: signalling the group from
            // here on could reach a reused id.
            held.process_group = None;
            break;
        }
        let can_settle = matches!(
            leader_exit,
            McpStdioLeaderExitObservation::NotSeen | McpStdioLeaderExitObservation::Exited(_)
        ) && matches!(witness, None | Some(McpStdioWitnessOutcome::Closed(_)));
        if !can_settle && phase == McpStdioClosePhase::Kill {
            // Killed, and nothing left that could settle it: the reap below
            // is bounded by the backstop.
            break;
        }
        let deadline = match phase {
            McpStdioClosePhase::Eof => deadlines.eof,
            McpStdioClosePhase::Term => deadlines.term,
            McpStdioClosePhase::Kill => deadlines.backstop,
        };
        // An expired deadline is acted on before any further output or exit,
        // so a server that keeps its stdout busy cannot postpone escalation.
        let expired = tokio::time::Instant::now() >= deadline
            || tokio::select! {
                biased;
                exited = wait_leader_exit(leader),
                    if leader_exit == McpStdioLeaderExitObservation::NotSeen =>
                {
                    leader_exit = match exited {
                        Ok(()) => McpStdioLeaderExitObservation::Exited(phase),
                        Err(errno) => McpStdioLeaderExitObservation::Lost { phase, errno },
                    };
                    false
                }
                read = async {
                    match witness_reader.as_mut() {
                        Some(reader) => reader.read(&mut discard).await,
                        None => std::future::pending().await,
                    }
                }, if witness.is_none() => {
                    match read {
                        // Draining also keeps a server writing while it exits
                        // from blocking on a full pipe.
                        Ok(read) if read > 0 => {}
                        Ok(_) => witness = Some(McpStdioWitnessOutcome::Closed(phase)),
                        Err(error) => {
                            witness = Some(McpStdioWitnessOutcome::ReadError {
                                phase,
                                kind: error.kind(),
                            });
                        }
                    }
                    false
                }
                () = tokio::time::sleep_until(deadline) => true,
            };
        if !expired {
            continue;
        }
        // A deadline can expire before the loop ever watched the leader (an
        // immediate or already spent grace), so, where it can be checked,
        // confirm it is still this process's child right before signalling
        // its group.
        recheck_leader(leader, &mut leader_exit, phase);
        if leader_lost_to_another_reaper(leader_exit) {
            held.process_group = None;
            break;
        }
        match phase {
            McpStdioClosePhase::Eof => {
                sigterm = signal_group(held.process_group, Signal::SIGTERM);
                phase = McpStdioClosePhase::Term;
            }
            McpStdioClosePhase::Term => {
                sigkill = signal_group(held.process_group, Signal::SIGKILL);
                // The direct child too, in case it left its group.
                kill_direct_child(held, &mut direct_killed);
                phase = McpStdioClosePhase::Kill;
            }
            McpStdioClosePhase::Kill => {
                tracing::warn!(
                    "MCP stdio server had not settled by the backstop after it was killed"
                );
                break;
            }
        }
    }
    drop(witness_reader);
    if held.process_group.is_some() {
        #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
        if let Some(before_final_check) = held.before_final_check.take() {
            before_final_check();
        }
        recheck_leader(leader, &mut leader_exit, phase);
        if leader_lost_to_another_reaper(leader_exit) {
            held.process_group = None;
            // Its exit was seen, but the leader is no longer this process's
            // to reap: the receipt cannot vouch for a settled close.
            settled_in = None;
        }
    }
    // The leader is unreaped, so the group id is still ours: signal whatever
    // the server left in its group, then disarm the id before the reap can
    // release it. (Already disarmed when the leader was lost.)
    let final_group_signal = signal_group(held.process_group.take(), Signal::SIGKILL);
    if settled_in.is_none() && final_group_signal.is_some() {
        // Gave up at the backstop: make sure the leader is dying before the
        // single bounded reap below (already so once the escalation killed
        // it).
        kill_direct_child(held, &mut direct_killed);
    }
    #[cfg(test)]
    {
        let gate = held
            .reap_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(gate) = gate {
            let _ = gate.entered.send(());
            let _ = gate.resume.await;
        }
    }
    let leader = reap_by(&mut held.child, deadlines.backstop).await;
    McpStdioCloseReceipt {
        settled_in,
        leader_exit,
        witness: witness.unwrap_or(McpStdioWitnessOutcome::StillOpen),
        sigterm,
        sigkill,
        final_group_signal,
        leader,
        elapsed: started.elapsed(),
    }
}

/// SIGKILL the direct child, at most once per close, without reaping it: an
/// exited leader stays unreaped, so its id and the group id it leads stay
/// held until the close's single reap. A no-op once it has exited.
#[cfg(unix)]
fn kill_direct_child(held: &mut StdioChild, killed: &mut bool) {
    if std::mem::replace(killed, true) {
        return;
    }
    #[cfg(test)]
    held.direct_kill_requests
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let _ = held.child.start_kill_without_reap();
}

/// Whether the leader was found not to be this process's child any more, so
/// the close no longer holds its group id.
#[cfg(unix)]
fn leader_lost_to_another_reaper(leader_exit: McpStdioLeaderExitObservation) -> bool {
    matches!(
        leader_exit,
        McpStdioLeaderExitObservation::Lost { errno: Some(errno), .. }
            if errno == nix::errno::Errno::ECHILD as i32
    )
}

/// Non-blocking check of a leader the close still takes as its child (not
/// seen to exit, or seen to exit and unreaped): records its exit, or the
/// loss of the observation.
#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
fn recheck_leader(
    leader: Option<u32>,
    leader_exit: &mut McpStdioLeaderExitObservation,
    phase: McpStdioClosePhase,
) {
    if !matches!(
        leader_exit,
        McpStdioLeaderExitObservation::NotSeen | McpStdioLeaderExitObservation::Exited(_)
    ) {
        return;
    }
    match poll_leader_exit(leader) {
        Ok(true) if *leader_exit == McpStdioLeaderExitObservation::NotSeen => {
            *leader_exit = McpStdioLeaderExitObservation::Exited(phase);
        }
        Ok(_) => {}
        Err(errno) => *leader_exit = McpStdioLeaderExitObservation::Lost { phase, errno },
    }
}

/// The leader's exit cannot be checked without reaping it here.
#[cfg(all(
    unix,
    not(any(target_os = "linux", target_os = "android", target_os = "macos"))
))]
fn recheck_leader(
    _leader: Option<u32>,
    _leader_exit: &mut McpStdioLeaderExitObservation,
    _phase: McpStdioClosePhase,
) {
}

/// Signal the server's process group, while the close still holds its id.
#[cfg(unix)]
fn signal_group(
    group: Option<nix::unistd::Pid>,
    signal: nix::sys::signal::Signal,
) -> Option<McpStdioSignalOutcome> {
    group.map(|group| match nix::sys::signal::killpg(group, signal) {
        Ok(()) => McpStdioSignalOutcome::Sent,
        Err(nix::errno::Errno::ESRCH) => McpStdioSignalOutcome::NoGroup,
        Err(errno) => McpStdioSignalOutcome::Failed(errno as i32),
    })
}

/// [`StdioChildCustody::terminate_from`]'s close of the taken process, off
/// Unix: wait for the direct child through the EOF grace, then kill it.
#[cfg(not(unix))]
async fn close_held(
    held: &mut StdioChild,
    bounds: McpStdioCloseBounds,
    started: tokio::time::Instant,
) -> McpStdioCloseReceipt {
    let deadlines = CloseDeadlines::new(started, bounds);
    let mut settled_in = None;
    let mut sigkill = None;
    let leader = match tokio::time::timeout_at(deadlines.eof, held.child.wait()).await {
        Ok(Ok(status)) => {
            settled_in = Some(McpStdioClosePhase::Eof);
            McpStdioLeaderOutcome::Reaped(status)
        }
        Ok(Err(error)) => McpStdioLeaderOutcome::WaitFailed(error),
        Err(_) => {
            sigkill = Some(match held.child.start_kill() {
                Ok(()) => McpStdioSignalOutcome::Sent,
                Err(error) => McpStdioSignalOutcome::Failed(error.raw_os_error().unwrap_or(0)),
            });
            reap_by(&mut held.child, deadlines.backstop).await
        }
    };
    McpStdioCloseReceipt {
        settled_in,
        leader_exit: McpStdioLeaderExitObservation::Unobservable,
        witness: McpStdioWitnessOutcome::Absent,
        sigterm: None,
        sigkill,
        final_group_signal: None,
        leader,
        elapsed: started.elapsed(),
    }
}

/// The single reap of a close, bounded by `deadline` (polled at least once,
/// so an already-exited leader is always reaped).
async fn reap_by(
    child: &mut meerkat_sandbox::ProcessChild,
    deadline: tokio::time::Instant,
) -> McpStdioLeaderOutcome {
    match tokio::time::timeout_at(deadline, child.wait()).await {
        Ok(Ok(status)) => McpStdioLeaderOutcome::Reaped(status),
        Ok(Err(error)) => McpStdioLeaderOutcome::WaitFailed(error),
        Err(_) => McpStdioLeaderOutcome::NotReaped,
    }
}

/// Whether `leader` has exited, without reaping it: one non-blocking
/// `waitid` with `WNOWAIT`. Fails, with the OS error number, when the id
/// cannot be waited for (`ECHILD`, or `None` for no id at all): that is not
/// an exit.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn poll_leader_exit(leader: Option<u32>) -> Result<bool, Option<i32>> {
    use nix::sys::wait::{Id, WaitPidFlag, WaitStatus, waitid};
    let Some(pid) = leader.and_then(|pid| i32::try_from(pid).ok()) else {
        return Err(None);
    };
    let pid = nix::unistd::Pid::from_raw(pid);
    let flags = WaitPidFlag::WEXITED | WaitPidFlag::WNOWAIT | WaitPidFlag::WNOHANG;
    loop {
        match waitid(Id::Pid(pid), flags) {
            Err(nix::errno::Errno::EINTR) => {}
            Ok(WaitStatus::StillAlive) => return Ok(false),
            Ok(_) => return Ok(true),
            Err(errno) => return Err(Some(errno as i32)),
        }
    }
}

/// Whether `leader` has exited, without reaping it, on macOS: the same
/// non-blocking `waitid` with `WNOWAIT` through libc, since nix does not
/// expose `waitid` on Apple targets. With `WNOHANG` and no change, `si_pid`
/// stays zero. Only an exit, a kill or a dump counts as an exit: XNU's
/// `waitid` skips a stopped child without `WSTOPPED`, and the `si_code`
/// check keeps it that way should that ever differ.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn poll_leader_exit(leader: Option<u32>) -> Result<bool, Option<i32>> {
    use nix::libc;
    let Some(pid) = leader else {
        return Err(None);
    };
    let flags = libc::WEXITED | libc::WNOWAIT | libc::WNOHANG;
    loop {
        // SAFETY: `siginfo_t` is a plain C record of integers and raw
        // pointers, for which all zero bytes are a valid value.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: `info` is a live `siginfo_t` owned by this frame and is the
        // only buffer `waitid` writes; nothing else is passed by pointer.
        let rc = unsafe { libc::waitid(libc::P_PID, pid, &raw mut info, flags) };
        if rc == 0 {
            return Ok(info.si_pid != 0
                && matches!(
                    info.si_code,
                    libc::CLD_EXITED | libc::CLD_KILLED | libc::CLD_DUMPED
                ));
        }
        match std::io::Error::last_os_error().raw_os_error() {
            Some(libc::EINTR) => {}
            errno => return Err(errno),
        }
    }
}

/// Resolves once `leader` has exited, without reaping it:
/// [`poll_leader_exit`] retried on a short timer so the wait stays abortable.
#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
async fn wait_leader_exit(leader: Option<u32>) -> Result<(), Option<i32>> {
    while !poll_leader_exit(leader)? {
        tokio::time::sleep(STDIO_LEADER_EXIT_POLL).await;
    }
    Ok(())
}

/// The server process's exit cannot be observed without reaping it here;
/// the close never polls this.
#[cfg(all(
    unix,
    not(any(target_os = "linux", target_os = "android", target_os = "macos"))
))]
async fn wait_leader_exit(_leader: Option<u32>) -> Result<(), Option<i32>> {
    std::future::pending().await
}

/// Credential source for OAuth-protected MCP servers.
///
/// A Streamable HTTP connection reads its bearer through
/// `stored_bearer_token` on every request, so the credential owner's current
/// (refreshed) credential is used without a reconnect, and a request without
/// a usable credential is refused before it is sent.
///
/// `interactive_login` is only reached in [`McpAuthMode::Interactive`]. A
/// resolver that completes it commits the credential it returns, so that
/// `stored_bearer_token` yields it from then on. A resolver without a host
/// browser channel returns [`McpOAuthError::HumanAuthorizationRequired`],
/// which the connection reports as the typed
/// [`McpError::AuthorizationRequired`] host status. A host that owns the
/// browser drives `McpOAuthAuthority::login_start`/`login_complete` itself.
#[async_trait]
pub trait McpAuthResolver: Send + Sync {
    async fn stored_bearer_token(
        &self,
        target: &McpServerIdentity,
    ) -> Result<Option<String>, McpOAuthError>;

    async fn interactive_login(
        &self,
        target: &McpServerIdentity,
        www_authenticate: Option<&str>,
    ) -> Result<String, McpOAuthError>;

    /// The host realm whose credentials this resolver serves, if any. A
    /// connection scopes its target to it, so the identity it reports and
    /// resolves is that realm's own.
    fn credential_realm(&self) -> Option<meerkat_core::connection::RealmId> {
        None
    }

    /// The required host egress profile this resolver's own credential
    /// traffic (refresh included) enforces, if any. Under a required
    /// profile, a connection uses only a resolver bound to that same
    /// profile; any other is refused before its first credential read.
    fn http_egress(&self) -> Option<HttpEgressProfile> {
        None
    }

    /// The values of `server`'s declared service secrets
    /// ([`meerkat_core::mcp_config::McpStdioConfig::secret_env`]) for one
    /// launch, read from the custody of the realm whose own configuration
    /// defines the server.
    ///
    /// A connection asks only for a stdio server that declares secrets,
    /// after its own validation and before its final physical-entry check
    /// and process spawn; the values go to that one process and are dropped
    /// before the handshake. The default holds no custody and refuses, so a
    /// resolver that does not serve service secrets never launches a server
    /// that declares them, and no value is ever taken from the host
    /// environment instead.
    async fn stdio_service_secrets(
        &self,
        server: &McpServerConfig,
    ) -> Result<McpServiceSecretValues, McpServiceSecretRefusal> {
        let _ = server;
        Err(McpServiceSecretRefusal::NoCustody)
    }

    /// The account this resolver's credential owner selected for `target`,
    /// read from its local selection only: no bearer is resolved or
    /// refreshed and nothing is sent, so stating it for review costs no
    /// network. The credential a request carries is still checked against
    /// the selection on every read. A resolver that cannot state one
    /// locally answers unavailable.
    fn selected_account(&self, _target: &McpServerIdentity) -> meerkat_core::OperationAccount {
        meerkat_core::OperationAccount::Unavailable
    }
}

#[async_trait]
impl McpAuthResolver for meerkat_auth_core::McpOAuthAuthority {
    /// Unselected targets (no `oauth_account` or `oauth_account_selection`)
    /// keep their stored-only semantics, so servers that need no OAuth
    /// connect as before.
    async fn stored_bearer_token(
        &self,
        target: &McpServerIdentity,
    ) -> Result<Option<String>, McpOAuthError> {
        if !target.is_selected() {
            return self.stored_only().stored_bearer_token(target).await;
        }
        self.stored_bearer_token(target).await
    }

    /// The native authority has no browser: human authorization is a host
    /// obligation, reported as typed status instead of opening anything.
    /// Interactive login needs an account selection.
    async fn interactive_login(
        &self,
        target: &McpServerIdentity,
        _www_authenticate: Option<&str>,
    ) -> Result<String, McpOAuthError> {
        if !target.is_selected() {
            return Err(McpOAuthError::AccountSelectionRequired);
        }
        Err(McpOAuthError::HumanAuthorizationRequired {
            server_name: target.server_name().to_owned(),
        })
    }

    fn credential_realm(&self) -> Option<meerkat_core::connection::RealmId> {
        self.realm().cloned()
    }

    fn http_egress(&self) -> Option<HttpEgressProfile> {
        self.bound_http_egress().cloned()
    }

    /// A Known selection: the subject every credential this owner yields
    /// for the target must prove, qualified by the target server's origin.
    /// A Discover selection's subject lives only in the stored credential,
    /// which this local projection does not read, and an unverified or
    /// legacy target carries no account evidence: both are unavailable.
    fn selected_account(&self, target: &McpServerIdentity) -> meerkat_core::OperationAccount {
        match (
            target.expected_account(),
            server_origin(target.server_url()),
        ) {
            (Some(subject), Some(origin)) => {
                meerkat_core::OperationAccount::selected(origin, subject)
            }
            _ => meerkat_core::OperationAccount::Unavailable,
        }
    }
}

/// The values of a stdio server's declared service secrets for one launch,
/// from `resolver`'s custody (none when the server declares none). Every
/// declared slot must be resolved, and nothing else: a refusal names the
/// server, never a value.
async fn resolve_stdio_service_secrets(
    config: &McpServerConfig,
    stdio: &meerkat_core::mcp_config::McpStdioConfig,
    resolver: Option<&dyn McpAuthResolver>,
) -> Result<Option<McpServiceSecretValues>, McpError> {
    if stdio.secret_env.is_empty() {
        return Ok(None);
    }
    let refused = |refusal| McpError::ServiceSecretRefused {
        server: config.name.clone(),
        refusal,
    };
    let resolver = resolver.ok_or_else(|| refused(McpServiceSecretRefusal::NoCustody))?;
    let values = resolver
        .stdio_service_secrets(config)
        .await
        .map_err(refused)?;
    if !values
        .names()
        .eq(stdio.secret_env.iter().map(String::as_str))
    {
        return Err(refused(McpServiceSecretRefusal::Unavailable {
            reason: "the custody did not resolve exactly the declared slots".to_owned(),
        }));
    }
    Ok(Some(values))
}

/// The configuration one stdio launch uses: `stdio` with the resolved
/// service secrets and the connector binding projection in its environment
/// (borrowed unchanged when there is neither).
fn launch_configuration<'a>(
    stdio: &'a meerkat_core::mcp_config::McpStdioConfig,
    config: &McpServerConfig,
    secrets: Option<McpServiceSecretValues>,
) -> std::borrow::Cow<'a, meerkat_core::mcp_config::McpStdioConfig> {
    if secrets.is_none() && config.connector.is_none() {
        return std::borrow::Cow::Borrowed(stdio);
    }
    let mut launch = stdio.clone();
    if let Some(secrets) = secrets {
        launch.env.extend(secrets.into_env());
    }
    if let Some(connector) = &config.connector {
        launch.env.insert(
            meerkat_core::mcp_config::MCP_CONNECTOR_BINDING_ENV.to_owned(),
            connector.startup_projection().to_string(),
        );
    }
    std::borrow::Cow::Owned(launch)
}

/// Why a configured working directory cannot be launched in now: nothing
/// at the path, something other than a directory, or a path that could not
/// be inspected. `None` for an existing directory (symbolic links followed,
/// as the process's change into it follows them).
fn working_directory_refusal(cwd: &str) -> Option<meerkat_core::McpStdioCwdRefusal> {
    use meerkat_core::McpStdioCwdRefusal;
    match std::fs::metadata(cwd) {
        Ok(metadata) if metadata.is_dir() => None,
        Ok(_) => Some(McpStdioCwdRefusal::NotADirectory),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Some(McpStdioCwdRefusal::Missing)
        }
        Err(_) => Some(McpStdioCwdRefusal::Inaccessible),
    }
}

/// The connection error of work its caller-held attempt cancelled.
fn attempt_cancelled() -> McpError {
    McpError::ConnectionFailed {
        reason: "the MCP connect attempt was cancelled".to_owned(),
    }
}

/// An interactive login, aborted when its attempt is cancelled.
async fn interactive_login_in(
    attempt: Option<&crate::McpConnectAttempt>,
    resolver: &dyn McpAuthResolver,
    target: &McpServerIdentity,
    challenge: Option<&str>,
) -> Result<String, McpError> {
    let login = resolver.interactive_login(target, challenge);
    let logged_in = match attempt {
        Some(attempt) => attempt
            .abortable(login)
            .await
            .ok_or_else(attempt_cancelled)?,
        None => login.await,
    };
    logged_in.map_err(|error| mcp_interactive_error(target, error))
}

/// `scheme://host[:port]` of an http(s) server URL: never its path, query,
/// fragment or userinfo, which can carry credentials.
fn server_origin(url: &str) -> Option<String> {
    let url = reqwest::Url::parse(url).ok()?;
    matches!(url.scheme(), "http" | "https").then(|| url.origin().ascii_serialization())
}

/// The account a connection to `config` acts with, as its owners state it:
/// the host's declaration that the server acts with none, the connector
/// binding's provider-verified account, or the OAuth credential owner's
/// selected account. Anything else is unavailable: a server without a
/// connector binding may still authenticate with its stdio environment,
/// static headers or a stored OAuth credential, so absence states nothing.
fn connection_account(
    config: &McpServerConfig,
    oauth: Option<&OAuthBearer>,
) -> meerkat_core::OperationAccount {
    if config.account == Some(meerkat_core::McpAccountDeclaration::NoAccount) {
        return meerkat_core::OperationAccount::DeclaredNone;
    }
    if let Some(binding) = &config.connector {
        return meerkat_core::OperationAccount::selected(
            binding.issuer.clone(),
            binding.account.clone(),
        );
    }
    oauth.map_or(
        meerkat_core::OperationAccount::Unavailable,
        OAuthBearer::selected_account,
    )
}

impl McpConnection {
    /// Connect to an MCP server and perform the initialize handshake
    pub async fn connect(config: &McpServerConfig) -> Result<Self, McpError> {
        Self::connect_with_mcp_auth(config, McpAuthMode::Stored, None).await
    }

    pub async fn connect_with_mcp_auth(
        config: &McpServerConfig,
        auth_mode: McpAuthMode,
        auth_resolver: Option<Arc<dyn McpAuthResolver>>,
    ) -> Result<Self, McpError> {
        Self::connect_with_services(config, auth_mode, auth_resolver, None).await
    }

    /// Connect with an optional host client-service factory. The first profile
    /// forwards form elicitation only; authentication remains with the resolver.
    /// Each physical attempt receives a fresh service for this exact config.
    pub async fn connect_with_services(
        config: &McpServerConfig,
        auth_mode: McpAuthMode,
        auth_resolver: Option<Arc<dyn McpAuthResolver>>,
        client_factory: Option<Arc<dyn McpClientServiceFactory>>,
    ) -> Result<Self, McpError> {
        Self::connect_with_services_and_stdio_profile(
            config,
            auth_mode,
            auth_resolver,
            client_factory,
            &McpStdioLaunchProfile::trusted_host(),
        )
        .await
    }

    /// Connect with a retained host profile for local process isolation.
    /// Remote HTTP transports are outside this local-process boundary.
    pub async fn connect_with_stdio_profile(
        config: &McpServerConfig,
        profile: &McpStdioLaunchProfile,
    ) -> Result<Self, McpError> {
        Self::connect_with_services_and_stdio_profile(
            config,
            McpAuthMode::Stored,
            None,
            None,
            profile,
        )
        .await
    }

    /// Connect with the same explicit host service and local-launch owners.
    pub async fn connect_with_services_and_stdio_profile(
        config: &McpServerConfig,
        auth_mode: McpAuthMode,
        auth_resolver: Option<Arc<dyn McpAuthResolver>>,
        client_factory: Option<Arc<dyn McpClientServiceFactory>>,
        profile: &McpStdioLaunchProfile,
    ) -> Result<Self, McpError> {
        Self::connect_with_profiles(
            config,
            auth_mode,
            auth_resolver,
            client_factory,
            profile,
            &HttpEgressProfile::trusted_host(),
        )
        .await
    }

    /// Connect under the host's local-launch profile and its HTTP egress
    /// profile. A required egress profile admits the configured URL before
    /// any effect, then every request of the attempt (an SSE server's
    /// message endpoint included) and every resolved address; it follows no
    /// redirect and uses no ambient proxy, and an OAuth resolver must be
    /// bound to the same profile. A refusal is the typed
    /// [`McpError::EgressRefused`] or [`McpError::EgressUnavailable`], with
    /// the existing connection cleanup. Stdio servers are governed by the
    /// launch profile only.
    pub async fn connect_with_profiles(
        config: &McpServerConfig,
        auth_mode: McpAuthMode,
        auth_resolver: Option<Arc<dyn McpAuthResolver>>,
        client_factory: Option<Arc<dyn McpClientServiceFactory>>,
        stdio_profile: &McpStdioLaunchProfile,
        http_egress: &HttpEgressProfile,
    ) -> Result<Self, McpError> {
        Self::connect_with_custody(
            config,
            auth_mode,
            auth_resolver,
            client_factory,
            None,
            ClientCustody::default(),
            stdio_profile,
            http_egress,
            PhysicalEntry::default(),
            None,
        )
        .await
    }

    /// [`Self::connect_with_services`] with the custody a stdio server's
    /// process is deposited into before the handshake (a fresh one when
    /// `None`), and the custody its connected client is deposited into. On
    /// failure the process is terminated before returning. An attempt whose
    /// client custody was closed before it began is refused before any
    /// effect; a client it connects after that close is closed at once and
    /// refused.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn connect_with_custody(
        config: &McpServerConfig,
        auth_mode: McpAuthMode,
        auth_resolver: Option<Arc<dyn McpAuthResolver>>,
        client_factory: Option<Arc<dyn McpClientServiceFactory>>,
        stdio_custody: Option<StdioChildCustody>,
        client_custody: ClientCustody,
        profile: &McpStdioLaunchProfile,
        http_egress: &HttpEgressProfile,
        mut entry: PhysicalEntry,
        attempt: Option<&crate::McpConnectAttempt>,
    ) -> Result<Self, McpError> {
        // An invalid access ceiling, secret declaration or working directory
        // is refused before any process or transport effect.
        config.validate_access()?;
        config.validate_secret_env()?;
        config.validate_cwd()?;
        // So is an attempt its router's close index refused.
        client_custody.admit()?;
        if let McpTransportConfig::Http(http) = &config.transport {
            let target = McpServerIdentity::from_config(config)
                .map_err(mcp_auth_error_to_connection_failed)?;
            if target.is_selected() && auth_resolver.is_none() {
                return Err(McpError::OAuthAccountRejected(
                    McpOAuthError::UnsupportedAccountSelection,
                ));
            }
            // The host egress profile admits the configured destination
            // before any service selection, stream or request.
            http_egress
                .admit(&http.url)
                .map_err(|error| McpError::from_egress(&config.name, error))?;
        }
        let connection_id = crate::McpConnectionId::allocate()?;
        let protected_metadata = ProtectedMetadataState::default();
        // Refusal precedes process spawn, SSE startup and HTTP transport effects.
        let client = ClientServiceSelection::select(config, client_factory.as_deref())?;
        let mut stdio_child = None;
        let service = match &config.transport {
            McpTransportConfig::Stdio(stdio) => {
                // We own the process; rmcp only gets its stdout/stdin.
                let custody = stdio_custody.unwrap_or_default();
                // Declared service secrets are read for this launch only,
                // before the final physical-entry check inside the spawn, so
                // a control revoked while the read waits still refuses with
                // nothing spawned; dropping this future (cancellation or the
                // connect deadline) spawns nothing either.
                let secrets =
                    resolve_stdio_service_secrets(config, stdio, auth_resolver.as_deref()).await?;
                // The launch-only configuration: the declared secrets and the
                // one projection of a connector-bound installation's expected
                // account binding (set over any configured value). It is
                // dropped as soon as the process exists; the connection keeps
                // the nonsecret `config`.
                let launch = launch_configuration(stdio, config, secrets);
                let spawned = custody
                    .spawn(&config.name, &launch, profile, &mut entry)
                    .await;
                drop(launch);
                let (stdout, stdin) = spawned?;
                match client
                    .serve(
                        ProtectedStdioTransport::new(stdout, stdin, protected_metadata.clone())
                            .with_end_record(custody.diagnostics_handle()),
                    )
                    .await
                {
                    Ok(service) => {
                        custody.mark_initialized();
                        if let Some(record) = custody.diagnostics_record() {
                            record.mark_initialized();
                        }
                        match client_custody.deposit(service, custody.bounds.eof_grace()) {
                            Ok(service) => {
                                stdio_child = Some(custody);
                                service
                            }
                            // Its close index closed it while it connected: the
                            // client was closed at once, and the process close
                            // is that index's.
                            Err(refused) => {
                                if let Ok(receipt) = custody.terminate().await {
                                    trace_close_receipt(&config.name, &receipt);
                                }
                                return Err(refused);
                            }
                        }
                    }
                    Err(e) => {
                        if let Some(record) = custody.diagnostics_record() {
                            record.classify(crate::stdio_diagnostics::classify_initialize_error(
                                &e,
                                record.transport_end(),
                            ));
                        }
                        if let Ok(receipt) = custody.terminate_failed_connect().await {
                            trace_close_receipt(&config.name, &receipt);
                            if let Some(record) = custody.diagnostics_record() {
                                record.attach_exit(&receipt);
                            }
                        }
                        return Err(McpError::ConnectionFailed {
                            reason: format!("Failed to establish MCP connection: {e}"),
                        });
                    }
                }
            }
            McpTransportConfig::Http(http) => {
                let headers = headers_from_map(&http.headers)
                    .map_err(|e| McpError::ConnectionFailed { reason: e })?;
                match http.transport.unwrap_or_default() {
                    McpHttpTransport::StreamableHttp => {
                        return Self::connect_streamable_http(
                            config,
                            headers,
                            &http.url,
                            auth_mode,
                            auth_resolver,
                            client_factory,
                            client,
                            &client_custody,
                            entry,
                            http_egress,
                            attempt,
                        )
                        .await;
                    }
                    McpHttpTransport::Sse => {
                        let egress = EgressGuard::new(http_egress.clone());
                        let http_client = ReqwestSseClient::new(headers)
                            .with_protected_metadata(protected_metadata.clone())
                            .with_egress(egress.clone());
                        // Physical entry: the SSE stream opens next.
                        entry.enter()?;
                        let failed = |reason: String| match egress.recorded() {
                            Some(error) => McpError::from_egress(&config.name, error),
                            None => McpError::ConnectionFailed { reason },
                        };
                        let transport = SseClientTransport::start_with_client(
                            http_client,
                            SseClientConfig {
                                sse_endpoint: http.url.clone().into(),
                                use_message_endpoint: None,
                            },
                        )
                        .await
                        .map_err(|e| failed(format!("Failed to establish SSE connection: {e}")))?;
                        let service = client.serve(transport).await.map_err(|e| {
                            failed(format!("Failed to establish MCP connection: {e}"))
                        })?;
                        client_custody.deposit(service, REMOTE_CLOSE_BOUND)?
                    }
                }
            }
        };

        Ok(Self {
            config: config.clone(),
            connection_id,
            protected_metadata,
            service,
            stdio_child,
            session_expiry: Default::default(),
            oauth_target: None,
            attempt: None,
            account: Arc::new(connection_account(config, None)),
            standard_tools: Default::default(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn connect_streamable_http(
        config: &McpServerConfig,
        headers: reqwest::header::HeaderMap,
        url: &str,
        auth_mode: McpAuthMode,
        auth_resolver: Option<Arc<dyn McpAuthResolver>>,
        client_factory: Option<Arc<dyn McpClientServiceFactory>>,
        client: ClientServiceSelection,
        client_custody: &ClientCustody,
        entry: PhysicalEntry,
        http_egress: &HttpEgressProfile,
        attempt: Option<&crate::McpConnectAttempt>,
    ) -> Result<Self, McpError> {
        // The attempt's first request asks the control at its own physical
        // entry, after its credential read; a challenge retry continues it.
        let first_request = entry.into_first_request();
        let has_static_authorization = headers
            .keys()
            .any(|name| name.as_str().eq_ignore_ascii_case("authorization"));
        if has_static_authorization {
            return Self::connect_streamable_http_once(
                config,
                headers,
                url,
                None,
                None,
                client,
                client_custody,
                first_request,
                http_egress,
                attempt,
            )
            .await
            .map_err(|error| error.into_mcp_error(&config.name));
        }
        // Under a required egress profile the credential owner's own traffic
        // (refresh included) must enforce that same profile: an unbound
        // resolver is refused before its first credential read.
        if http_egress.is_required()
            && let Some(resolver) = auth_resolver.as_deref()
            && !resolver
                .http_egress()
                .is_some_and(|bound| bound.same_profile(http_egress))
        {
            return Err(McpError::from_egress(
                &config.name,
                HttpEgressError::Unavailable(HttpEgressUnavailable::CredentialOwnerNotBound),
            ));
        }
        let target =
            McpServerIdentity::from_config(config).map_err(mcp_auth_error_to_connection_failed)?;
        // The host realm's own credential slot, when the resolver serves one.
        let target = match auth_resolver
            .as_deref()
            .and_then(McpAuthResolver::credential_realm)
        {
            Some(realm) => target.in_realm(realm),
            None => target,
        };
        // The connection resolves its bearer per request through the resolver;
        // this connect-time read only decides the path before any transport.
        let bearer = auth_resolver
            .as_ref()
            .map(|resolver| OAuthBearer::new(Arc::clone(resolver), target.clone()));
        let mut has_stored_token = false;
        let mut force_interactive_reauth = false;
        if let Some(resolver) = auth_resolver.as_deref() {
            let stored = match attempt {
                Some(attempt) => attempt
                    .abortable(resolver.stored_bearer_token(&target))
                    .await
                    .ok_or_else(attempt_cancelled)?,
                None => resolver.stored_bearer_token(&target).await,
            };
            match stored {
                Ok(None) if target.is_selected() => {
                    if matches!(auth_mode, McpAuthMode::Interactive) {
                        force_interactive_reauth = true;
                    } else {
                        return Err(McpError::OAuthAccountRejected(
                            McpOAuthError::MissingStoredToken {
                                server_name: config.name.clone(),
                            },
                        ));
                    }
                }
                Ok(token) => has_stored_token = token.is_some(),
                Err(McpOAuthError::ReauthRequired { .. })
                    if matches!(auth_mode, McpAuthMode::Interactive) =>
                {
                    force_interactive_reauth = true;
                }
                Err(error) => return Err(mcp_auth_error_to_connection_failed(error)),
            }
        }
        if force_interactive_reauth {
            let Some(resolver) = auth_resolver else {
                return Err(mcp_auth_error_to_connection_failed(
                    McpOAuthError::ReauthRequired {
                        server_name: config.name.clone(),
                    },
                ));
            };
            interactive_login_in(attempt, resolver.as_ref(), &target, None).await?;
            return Self::connect_streamable_http_once(
                config,
                headers,
                url,
                bearer,
                None,
                client,
                client_custody,
                first_request,
                http_egress,
                attempt,
            )
            .await
            .map_err(|error| error.into_mcp_error(&config.name));
        }
        let first = Self::connect_streamable_http_once(
            config,
            headers.clone(),
            url,
            bearer.clone(),
            Some(crate::transport::streamable_http::AuthChallengeRecorder::default()),
            client,
            client_custody,
            first_request.clone(),
            http_egress,
            attempt,
        )
        .await;
        match first {
            Ok(connection) => Ok(connection),
            Err(err) if auth_failure_suggests_oauth(&err) => {
                let challenge = err.auth_challenge();
                let Some(resolver) = auth_resolver else {
                    return Err(err.into_mcp_error(&config.name));
                };
                match (has_stored_token, auth_mode) {
                    (true, McpAuthMode::Stored) => {
                        return Err(McpError::ConnectionFailed {
                            reason: McpOAuthError::ReauthRequired {
                                server_name: config.name.clone(),
                            }
                            .to_string(),
                        });
                    }
                    (false, McpAuthMode::Stored) => {
                        return Err(McpError::ConnectionFailed {
                            reason: McpOAuthError::MissingStoredToken {
                                server_name: config.name.clone(),
                            }
                            .to_string(),
                        });
                    }
                    (_, McpAuthMode::Interactive) => {
                        interactive_login_in(
                            attempt,
                            resolver.as_ref(),
                            &target,
                            challenge.as_deref(),
                        )
                        .await?;
                    }
                }
                // The first attempt consumed its service. A retry selects a
                // fresh owner for the same config before touching transport.
                let retry_client =
                    ClientServiceSelection::select(config, client_factory.as_deref())?;
                Self::connect_streamable_http_once(
                    config,
                    headers,
                    url,
                    bearer,
                    None,
                    retry_client,
                    client_custody,
                    first_request,
                    http_egress,
                    attempt,
                )
                .await
                .map_err(|error| error.into_mcp_error(&config.name))
            }
            Err(err) => Err(err.into_mcp_error(&config.name)),
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn connect_streamable_http_once(
        config: &McpServerConfig,
        headers: reqwest::header::HeaderMap,
        url: &str,
        bearer: Option<OAuthBearer>,
        recorder: Option<crate::transport::streamable_http::AuthChallengeRecorder>,
        client: ClientServiceSelection,
        client_custody: &ClientCustody,
        first_request: crate::transport::streamable_http::FirstRequestEntry,
        http_egress: &HttpEgressProfile,
        attempt: Option<&crate::McpConnectAttempt>,
    ) -> Result<Self, StreamableConnectError> {
        let connection_id =
            crate::McpConnectionId::allocate().map_err(|_| StreamableConnectError {
                reason: "MCP connection generation unavailable".into(),
                auth: Default::default(),
                refused: None,
                egress: None,
                shut_down: false,
            })?;
        let protected_metadata = ProtectedMetadataState::default();
        let session_expiry = SessionExpiryRecorder::default();
        let recorder = recorder.unwrap_or_default();
        let egress = EgressGuard::new(http_egress.clone());
        let mut http_client =
            ReqwestStreamableHttpClient::new_with_auth_challenge(headers, recorder.clone())
                .with_protected_metadata(protected_metadata.clone())
                .with_session_expiry(session_expiry.clone())
                .with_first_request_entry(first_request.clone())
                .with_egress(egress.clone());
        let oauth_target = bearer.as_ref().map(|bearer| bearer.target().clone());
        let account = Arc::new(connection_account(config, bearer.as_ref()));
        if let Some(bearer) = bearer {
            http_client = http_client.with_oauth_bearer(bearer);
        }
        if let Some(attempt) = attempt {
            http_client = http_client.with_attempt(attempt);
        }
        // Never re-initialize and re-send a request after a session expiry
        // (rmcp defaults to one transparent replay): a call's effect may
        // already have happened, so its outcome is reported as uncertain.
        let transport_config = StreamableHttpClientTransportConfig::with_uri(url.to_string())
            .reinit_on_expired_session(false);
        let transport = StreamableHttpClientTransport::with_client(http_client, transport_config);
        let service = client
            .serve(transport)
            .await
            .map_err(|error| StreamableConnectError {
                reason: format!("Failed to establish MCP connection: {error}"),
                auth: recorder.take(),
                refused: first_request.refusal(),
                egress: egress.recorded(),
                shut_down: false,
            })?;
        let service = client_custody
            .deposit(service, REMOTE_CLOSE_BOUND)
            .map_err(|_| StreamableConnectError {
                reason: McpError::RouterShutDown.to_string(),
                auth: Default::default(),
                refused: None,
                egress: None,
                shut_down: true,
            })?;
        Ok(Self {
            config: config.clone(),
            connection_id,
            protected_metadata,
            service,
            stdio_child: None,
            session_expiry,
            oauth_target,
            attempt: attempt.cloned(),
            account,
            standard_tools: Default::default(),
        })
    }

    /// Default connection timeout in seconds.
    pub const DEFAULT_CONNECT_TIMEOUT_SECS: u32 = 10;

    /// Most `tools/list` pages one tool discovery follows. A server that still
    /// reports a next page after this many is refused with
    /// [`McpError::ToolDiscoveryLimitExceeded`] rather than followed forever.
    pub const MAX_TOOL_DISCOVERY_PAGES: usize = 100;

    /// Most tools one tool discovery accepts across all pages. A server that
    /// lists more is refused with [`McpError::ToolDiscoveryLimitExceeded`].
    pub const MAX_DISCOVERED_TOOLS: usize = 10_000;

    /// Connect to an MCP server, perform handshake, and enumerate tools in a
    /// single timeout-bounded operation.
    ///
    /// This is the preferred entry point for all add/reload paths. The timeout
    /// covers connect + initialize handshake + list_tools as a single budget.
    pub async fn connect_and_enumerate(
        config: &McpServerConfig,
    ) -> Result<(Self, Vec<Arc<ToolDef>>), McpError> {
        Self::connect_and_enumerate_with_mcp_auth(config, McpAuthMode::Stored, None).await
    }

    pub async fn connect_and_enumerate_with_mcp_auth(
        config: &McpServerConfig,
        auth_mode: McpAuthMode,
        auth_resolver: Option<Arc<dyn McpAuthResolver>>,
    ) -> Result<(Self, Vec<Arc<ToolDef>>), McpError> {
        Self::connect_and_enumerate_with_services(config, auth_mode, auth_resolver, None).await
    }

    /// Connect and enumerate under the existing timeout, with an exact-attempt
    /// host service factory. Native staged and synchronous router paths use this.
    pub async fn connect_and_enumerate_with_services(
        config: &McpServerConfig,
        auth_mode: McpAuthMode,
        auth_resolver: Option<Arc<dyn McpAuthResolver>>,
        client_factory: Option<Arc<dyn McpClientServiceFactory>>,
    ) -> Result<(Self, Vec<Arc<ToolDef>>), McpError> {
        Self::connect_and_enumerate_with_services_and_stdio_profile(
            config,
            auth_mode,
            auth_resolver,
            client_factory,
            &McpStdioLaunchProfile::trusted_host(),
        )
        .await
    }

    /// Connect and enumerate under the normal deadline with a retained host
    /// launch profile. Required isolation never falls back to trusted execution.
    pub async fn connect_and_enumerate_with_stdio_profile(
        config: &McpServerConfig,
        profile: &McpStdioLaunchProfile,
    ) -> Result<(Self, Vec<Arc<ToolDef>>), McpError> {
        Self::connect_and_enumerate_with_services_and_stdio_profile(
            config,
            McpAuthMode::Stored,
            None,
            None,
            profile,
        )
        .await
    }

    /// Connect and enumerate with explicit host services and local isolation.
    pub async fn connect_and_enumerate_with_services_and_stdio_profile(
        config: &McpServerConfig,
        auth_mode: McpAuthMode,
        auth_resolver: Option<Arc<dyn McpAuthResolver>>,
        client_factory: Option<Arc<dyn McpClientServiceFactory>>,
        profile: &McpStdioLaunchProfile,
    ) -> Result<(Self, Vec<Arc<ToolDef>>), McpError> {
        Self::connect_and_enumerate_with_profiles(
            config,
            auth_mode,
            auth_resolver,
            client_factory,
            profile,
            &HttpEgressProfile::trusted_host(),
        )
        .await
    }

    /// Connect, initialize and enumerate under the normal deadline, with the
    /// host's local-launch profile and HTTP egress profile (see
    /// [`Self::connect_with_profiles`]). Nothing is published to any router:
    /// the caller owns the returned connection and closes it.
    pub async fn connect_and_enumerate_with_profiles(
        config: &McpServerConfig,
        auth_mode: McpAuthMode,
        auth_resolver: Option<Arc<dyn McpAuthResolver>>,
        client_factory: Option<Arc<dyn McpClientServiceFactory>>,
        stdio_profile: &McpStdioLaunchProfile,
        http_egress: &HttpEgressProfile,
    ) -> Result<(Self, Vec<Arc<ToolDef>>), McpError> {
        let stdio_custody = matches!(config.transport, McpTransportConfig::Stdio(_))
            .then(StdioChildCustody::default);
        Self::connect_and_enumerate_with_custody_and_egress(
            config,
            auth_mode,
            auth_resolver,
            client_factory,
            stdio_custody,
            ClientCustody::default(),
            stdio_profile,
            http_egress,
            None,
        )
        .await
    }

    /// [`Self::connect_and_enumerate_with_custody_and_egress`] for a trusted
    /// host.
    #[cfg(test)]
    pub(crate) async fn connect_and_enumerate_with_custody(
        config: &McpServerConfig,
        auth_mode: McpAuthMode,
        auth_resolver: Option<Arc<dyn McpAuthResolver>>,
        client_factory: Option<Arc<dyn McpClientServiceFactory>>,
        stdio_custody: Option<StdioChildCustody>,
        profile: &McpStdioLaunchProfile,
        control: Option<Arc<dyn McpAttachmentAuthority>>,
    ) -> Result<(Self, Vec<Arc<ToolDef>>), McpError> {
        Self::connect_and_enumerate_with_custody_and_egress(
            config,
            auth_mode,
            auth_resolver,
            client_factory,
            stdio_custody,
            ClientCustody::default(),
            profile,
            &HttpEgressProfile::trusted_host(),
            control,
        )
        .await
    }

    /// [`Self::connect_and_enumerate_with_services`] with a caller-held custody
    /// for a stdio server's process, so the caller can also terminate it while
    /// the attempt is still in flight, and for its client, under the host's
    /// HTTP egress profile. On failure or timeout the process's close has
    /// finished when this returns, unless another caller owns it.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn connect_and_enumerate_with_custody_and_egress(
        config: &McpServerConfig,
        auth_mode: McpAuthMode,
        auth_resolver: Option<Arc<dyn McpAuthResolver>>,
        client_factory: Option<Arc<dyn McpClientServiceFactory>>,
        stdio_custody: Option<StdioChildCustody>,
        client_custody: ClientCustody,
        profile: &McpStdioLaunchProfile,
        http_egress: &HttpEgressProfile,
        control: Option<Arc<dyn McpAttachmentAuthority>>,
    ) -> Result<(Self, Vec<Arc<ToolDef>>), McpError> {
        Self::connect_and_enumerate_in(
            config,
            auth_mode,
            auth_resolver,
            client_factory,
            stdio_custody,
            client_custody,
            profile,
            http_egress,
            control,
            None,
        )
        .await
    }

    /// Connect, initialize and enumerate a Streamable HTTP server within a
    /// caller-held `attempt` (see [`crate::McpConnectAttempt`]), under the
    /// normal deadline and the host's HTTP egress profile. If this future is
    /// dropped before it returns, or it fails, the attempt is cancelled: its
    /// in-flight credential reads and requests are aborted, nothing further
    /// is sent, and nothing is returned. The caller observes the end of the
    /// attempt's work with [`crate::McpConnectAttempt::settle`], and closes a
    /// returned connection with [`Self::close_in_attempt`]. Another transport
    /// is refused before any effect: the attempt observes only Streamable
    /// HTTP work (a stdio server's inspection keeps its process custody and
    /// [`Self::close_with_receipt`]).
    pub async fn connect_and_enumerate_in_attempt(
        config: &McpServerConfig,
        auth_mode: McpAuthMode,
        auth_resolver: Option<Arc<dyn McpAuthResolver>>,
        client_factory: Option<Arc<dyn McpClientServiceFactory>>,
        http_egress: &HttpEgressProfile,
        attempt: &crate::McpConnectAttempt,
    ) -> Result<(Self, Vec<Arc<ToolDef>>), McpError> {
        if !matches!(
            &config.transport,
            McpTransportConfig::Http(http)
                if matches!(http.transport.unwrap_or_default(), McpHttpTransport::StreamableHttp)
        ) {
            return Err(McpError::ConnectionFailed {
                reason: format!(
                    "MCP server '{}': an attempt-scoped inspection serves Streamable HTTP servers only",
                    config.name
                ),
            });
        }
        // The attempt counts this connect as its live work from before its
        // first await until it returns or is dropped: a cancellation request
        // is not the end of its credential reads or requests.
        let _running = attempt.lease();
        let unfinished = attempt.abort_unless_completed();
        let connected = Self::connect_and_enumerate_in(
            config,
            auth_mode,
            auth_resolver,
            client_factory,
            None,
            ClientCustody::default(),
            &McpStdioLaunchProfile::trusted_host(),
            http_egress,
            None,
            Some(attempt),
        )
        .await;
        if connected.is_ok() {
            unfinished.completed();
        }
        connected
    }

    /// Close a connection made in `attempt`, observing its end within
    /// `bound`. The close first lets the transport delete its server
    /// session; if the close has not ended by half the bound, the attempt is
    /// cancelled, aborting the deletion and any other in-flight request. The
    /// close runs on its own task leased by the attempt, so a later
    /// [`crate::McpConnectAttempt::settle`] observes an end this bound did
    /// not. A stdio connection's close is its process custody's own bounded
    /// close, joined only with its receipt; any other connection not made in
    /// `attempt` is closed with its end not observed.
    pub async fn close_in_attempt(
        self,
        attempt: &crate::McpConnectAttempt,
        bound: Duration,
    ) -> crate::McpAttemptSettlement {
        let started = tokio::time::Instant::now();
        if self.stdio_child.is_some() {
            let closed = tokio::time::timeout(bound, self.close_with_receipt()).await;
            return crate::McpAttemptSettlement {
                local: if matches!(closed, Ok(Ok(Some(_)))) {
                    crate::McpLocalTermination::Joined
                } else {
                    crate::McpLocalTermination::NotObserved
                },
                remote_session: crate::McpRemoteSession::NoSession,
            };
        }
        if !self
            .attempt
            .as_ref()
            .is_some_and(|own| own.is_same(attempt))
        {
            let _ = tokio::time::timeout(bound, self.close()).await;
            return crate::McpAttemptSettlement {
                local: crate::McpLocalTermination::NotObserved,
                remote_session: crate::McpRemoteSession::Unconfirmed,
            };
        }
        let service = self.service;
        // The close's own outcome is observed through its end, which the
        // attempt's lease on it records.
        attempt.spawn_retained(async move {
            let _ = service
                .close_until(tokio::time::Instant::now() + REMOTE_CLOSE_BOUND)
                .await;
        });
        let graceful = attempt.settle_until(started + bound / 2).await;
        if graceful.local == crate::McpLocalTermination::Joined {
            return graceful;
        }
        attempt.cancel();
        attempt.settle_until(started + bound).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn connect_and_enumerate_in(
        config: &McpServerConfig,
        auth_mode: McpAuthMode,
        auth_resolver: Option<Arc<dyn McpAuthResolver>>,
        client_factory: Option<Arc<dyn McpClientServiceFactory>>,
        stdio_custody: Option<StdioChildCustody>,
        client_custody: ClientCustody,
        profile: &McpStdioLaunchProfile,
        http_egress: &HttpEgressProfile,
        control: Option<Arc<dyn McpAttachmentAuthority>>,
        attempt: Option<&crate::McpConnectAttempt>,
    ) -> Result<(Self, Vec<Arc<ToolDef>>), McpError> {
        let deadline = ConnectDeadline::for_connect(config, auth_mode, auth_resolver.is_some());

        let server_name = config.name.clone();
        let connected = tokio::time::timeout(deadline.total(), async {
            let conn = Self::connect_with_custody(
                config,
                auth_mode,
                auth_resolver,
                client_factory,
                stdio_custody.clone(),
                client_custody,
                profile,
                http_egress,
                PhysicalEntry::new(control),
                attempt,
            )
            .await?;
            let tools = conn
                .list_tools(&server_name)
                .await?
                .into_iter()
                .map(Arc::new)
                .collect::<Vec<_>>();
            // A connector binding names the server's exact raw operations; a
            // name the server does not list can never be matched, so the
            // binding is refused rather than left silently unused.
            if let Some(connector) = &config.connector
                && let Some(missing) = connector.operations.keys().find(|operation| {
                    !tools.iter().any(|tool| tool.name.as_str() == operation.as_str())
                })
            {
                return Err(McpError::ConnectionFailed {
                    reason: format!(
                        "connector binding names operation '{missing}', which the server does not list"
                    ),
                });
            }
            Ok((conn, tools))
        })
        .await;
        let timed_out = connected.is_err();
        let attempt = connected
            .map_err(|_| deadline.timed_out(&config.name))
            .flatten();
        if timed_out
            && let Some(record) = stdio_custody
                .as_ref()
                .and_then(StdioChildCustody::diagnostics_record)
        {
            // The deadline expired in the phase the attempt had reached.
            record.classify(McpStdioSetupFailureClass::TimedOut);
        }
        if attempt.is_err()
            && let Some(custody) = &stdio_custody
        {
            // The attempt future (and any connection it held) is gone; the
            // process it spawned is not until its owner observes the exit.
            if let Ok(receipt) = custody.terminate_failed_connect().await {
                trace_close_receipt(&server_name, &receipt);
            }
            if let Some(record) = custody.diagnostics_record() {
                if let Some(receipt) = custody.closed_receipt() {
                    record.attach_exit(&receipt);
                }
                record.finish_failed();
            }
        }
        attempt
    }

    /// Transfer this exact connected owner into the protocol wrapper without
    /// another handshake or host-service selection.
    pub fn into_protocol(self) -> crate::McpProtocol {
        crate::McpProtocol::from_connection(
            self.service,
            self.stdio_child,
            self.config.name,
            self.config.access,
            self.protected_metadata,
            self.session_expiry,
        )
    }

    /// Get the config used to create this connection.
    pub fn config(&self) -> &McpServerConfig {
        &self.config
    }

    /// Physical generation of this exact connection.
    pub fn connection_id(&self) -> crate::McpConnectionId {
        self.connection_id
    }

    /// The account this connection's operations act with, as its owners
    /// stated it when it was made. It is fixed for the connection's
    /// lifetime: a changed binding is a new connection generation.
    pub fn operation_account(&self) -> &Arc<meerkat_core::OperationAccount> {
        &self.account
    }

    pub(crate) fn supports_mcp_apps(&self) -> bool {
        self.service.supports_mcp_apps()
    }

    /// Get server info
    pub fn server_info(&self) -> Option<Arc<rmcp::model::ServerInfo>> {
        self.service.peer_info()
    }

    /// List available tools
    pub async fn list_tools(&self, server_name: &str) -> Result<Vec<ToolDef>, McpError> {
        let standard =
            crate::protocol::list_all_standard_tools_with(&self.service, server_name, |error| {
                self.record_discovery_failure(error);
                self.authorization_required(error)
                    .unwrap_or_else(|| crate::protocol::list_tools_failed(error))
            })
            .await?;
        let projected = standard
            .iter()
            .map(|tool| crate::apps::project_tool(tool, server_name))
            .collect::<Result<Vec<_>, _>>()?;
        *self
            .standard_tools
            .write()
            .map_err(|_| McpError::ProtocolError {
                message: "MCP discovery owner is unavailable".into(),
            })? = standard;
        Ok(projected)
    }

    /// An ambiguous duplicate cannot select UI metadata for a result.
    pub(crate) fn standard_tool(&self, name: &str) -> Option<rmcp::model::Tool> {
        let tools = self.standard_tools.read().ok()?;
        let mut matches = tools.iter().filter(|tool| tool.name == name);
        let first = matches.next()?;
        matches.all(|tool| tool == first).then(|| first.clone())
    }

    /// While this connection's attempt is still being set up (its router
    /// holds the attempt's diagnostics), what a failed tool listing
    /// established; afterwards nothing is recorded.
    fn record_discovery_failure(&self, error: &rmcp::ServiceError) {
        if let Some(record) = self
            .stdio_child
            .as_ref()
            .and_then(StdioChildCustody::diagnostics_record)
        {
            record.classify(crate::stdio_diagnostics::classify_service_error(
                error,
                record.transport_end(),
            ));
        }
    }

    /// The typed host status for a request that this OAuth connection's
    /// server refused with a `401`, or that was refused before dispatch for
    /// want of a usable credential. Such a request is never replayed.
    fn authorization_required(&self, error: &rmcp::ServiceError) -> Option<McpError> {
        authorization_required(self.oauth_target.as_ref(), error)
    }

    /// Call a tool, returning multimodal content blocks.
    ///
    /// MCP servers can return text, image, and other content types. Text and
    /// image content are captured as their typed [`ContentBlock`] variants;
    /// resource, audio, and resource-link content the agent loop does not model
    /// are preserved verbatim as [`ContentBlock::Structured`] rather than
    /// silently dropped. Optional `structuredContent` is appended as one
    /// additional Structured block containing that JSON value; a text block
    /// whose content parses to the same JSON value is its serialization and
    /// is not repeated.
    pub async fn call_tool(&self, name: &str, args: &Value) -> Result<Vec<ContentBlock>, McpError> {
        let result = self.call_tool_result(name, args, None).await?;
        crate::protocol::convert_tool_result(result, name)
    }

    pub(crate) async fn call_tool_result(
        &self,
        name: &str,
        args: &Value,
        metadata: Option<serde_json::Map<String, Value>>,
    ) -> Result<CallToolResult, McpError> {
        self.call_tool_result_entering(name, args, metadata, || Ok(None))
            .await
    }

    pub(crate) async fn read_resource_entering(
        &self,
        uri: &str,
        metadata: Option<serde_json::Map<String, Value>>,
        deadline: tokio::time::Instant,
        enter: impl FnOnce() -> Result<(), McpError>,
    ) -> Result<rmcp::model::ReadResourceResult, McpError> {
        if self.session_expiry.expired() {
            return Err(session_dead(&self.config.name));
        }
        let dispatch = RequestDispatch::default();
        let mut request =
            rmcp::model::ReadResourceRequest::new(rmcp::model::ReadResourceRequestParams::new(uri));
        request.extensions.insert(dispatch.clone());
        if let Some(metadata) = metadata {
            self.protected_metadata.register(&metadata)?;
            request.extensions.insert(ProtectedMetadata(metadata));
        }
        let request = request.into();
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(crate::McpCallContextError::Unavailable.into());
        }
        enter()?;
        // Let the request owner report its timeout through the ordinary
        // transport result. Dropping the router's admitted read future would
        // skip its outcome observation and release its credential lease early.
        // A timeout is local unavailability, never proof the remote did no work.
        let result = tokio::time::timeout_at(deadline, async {
            self.service
                .send_request_with_option(
                    request,
                    rmcp::service::PeerRequestOptions::with_timeout(remaining),
                )
                .await?
                .await_response()
                .await
        })
        .await
        .unwrap_or(Err(rmcp::ServiceError::Timeout { timeout: remaining }))
        .map_err(|error| {
            let disposition = dispatch.disposition();
            if matches!(disposition, Some(RequestDisposition::Sent) | None)
                && let Some(refused) = self.authorization_required(&error)
            {
                return refused;
            }
            tool_call_failure(&self.config.name, "resources/read", disposition, &error)
        })?;
        match result {
            ServerResult::ReadResourceResult(result) => {
                if serde_json::to_vec(&result)
                    .map_err(|_| McpError::Serialization("invalid resource response".into()))?
                    .len()
                    > 8 * 1024 * 1024
                {
                    return Err(McpError::ProtocolError {
                        message: "MCP App resource exceeds host limit".into(),
                    });
                }
                Ok(result)
            }
            _ => Err(McpError::ProtocolError {
                message: "unexpected MCP resources/read response".into(),
            }),
        }
    }

    /// Like [`Self::call_tool_result`], running `enter` after every local
    /// request preparation step (parameters, protected metadata registration)
    /// and immediately before the local transport handoff. A refusal from
    /// `enter` sends nothing; nothing that could still fail locally runs after
    /// it. The reviewed boundary is this local handoff: it makes no claim about
    /// when or whether the remote server performs its effect. The
    /// [`crate::transport::protected::SendCustody`] `enter` returns travels with the request to the native
    /// send owner, which may outlive this call.
    pub(crate) async fn call_tool_result_entering(
        &self,
        name: &str,
        args: &Value,
        metadata: Option<serde_json::Map<String, Value>>,
        enter: impl FnOnce() -> Result<Option<crate::transport::protected::SendCustody>, McpError>,
    ) -> Result<CallToolResult, McpError> {
        self.call_tool_result_admitted(
            CallAdmission::Ceiling(self.config.access.as_ref()),
            name,
            args,
            metadata,
            enter,
        )
        .await
    }

    /// [`Self::call_tool_result_entering`] under `admission` instead of this
    /// connection's own ceiling: the router passes the checked route it
    /// resolved the call through, which admits exactly that route's
    /// selected operations.
    pub(crate) async fn call_tool_result_admitted(
        &self,
        admission: CallAdmission<'_>,
        name: &str,
        args: &Value,
        metadata: Option<serde_json::Map<String, Value>>,
        enter: impl FnOnce() -> Result<Option<crate::transport::protected::SendCustody>, McpError>,
    ) -> Result<CallToolResult, McpError> {
        call_tool_on(
            &self.service,
            &self.config.name,
            admission,
            &self.protected_metadata,
            &self.session_expiry,
            self.oauth_target.as_ref(),
            name,
            args,
            metadata,
            enter,
        )
        .await
    }

    /// Call a tool, returning only the text content as a concatenated string.
    ///
    /// This projects every block returned by [`call_tool`](Self::call_tool)
    /// to text, including derived JSON for Structured blocks and media labels.
    pub async fn call_tool_text(&self, name: &str, args: &Value) -> Result<String, McpError> {
        let blocks = self.call_tool(name, args).await?;
        Ok(meerkat_core::types::text_content(&blocks))
    }

    /// Close the connection. A stdio server's stdin is closed, it gets the
    /// grace of its [`McpStdioCloseBounds`] to exit (its whole process group
    /// on Unix) before it is signalled and then killed, and its process has
    /// been reaped when this returns unless the receipt of
    /// [`Self::close_with_receipt`] says otherwise.
    pub async fn close(self) -> Result<(), McpError> {
        self.close_with_receipt().await.map(drop)
    }

    /// [`Self::close`], returning what closing a stdio server observed
    /// (`None` for other transports). The receipt is also traced; it is lost
    /// only when the close itself fails, including a failed reap of the
    /// server process, which is returned as its OS error.
    pub async fn close_with_receipt(self) -> Result<Option<McpStdioCloseReceipt>, McpError> {
        close_connected(&self.config.name, self.service, self.stdio_child).await
    }

    /// This connection's client custody.
    pub(crate) fn client_custody(&self) -> &ClientCustody {
        &self.service.custody
    }

    /// [`Self::close`], reporting what it observed as one typed outcome; it
    /// never fails. A stdio server's receipt is kept whatever its leader
    /// outcome (a failed reap included), and a close of the same process
    /// that another call started is joined for its receipt. A remote
    /// transport whose client closed is [`McpServerCloseOutcome::NoProcess`].
    pub async fn close_with_outcome(self) -> McpServerCloseOutcome {
        let server = self.config.name.clone();
        let observed =
            close_connected_observed(&server, self.service, self.stdio_child, true).await;
        close_outcome(&server, observed)
    }

    /// [`Self::connect`] within `timeout`. A connect that fails or times out
    /// after it spawned a stdio server's process closes that process through
    /// its custody before returning, and reports the close.
    pub async fn connect_within(
        config: &McpServerConfig,
        timeout: Duration,
    ) -> Result<Self, McpConnectFailure> {
        let custody = matches!(config.transport, McpTransportConfig::Stdio(_))
            .then(StdioChildCustody::default);
        let connected = tokio::time::timeout(
            timeout,
            Self::connect_with_custody(
                config,
                McpAuthMode::Stored,
                None,
                None,
                custody.clone(),
                ClientCustody::default(),
                &McpStdioLaunchProfile::trusted_host(),
                &HttpEgressProfile::trusted_host(),
                PhysicalEntry::default(),
                None,
            ),
        )
        .await;
        let (error, timed_out) = match connected {
            Ok(Ok(connection)) => return Ok(connection),
            Ok(Err(error)) => (error, false),
            Err(_) => (
                McpError::ConnectionFailed {
                    reason: format!("Timed out connecting to '{}' ({timeout:?})", config.name),
                },
                true,
            ),
        };
        let close = match custody {
            None => None,
            Some(custody) => match custody.terminate_failed_connect_or_join().await {
                Ok(receipt) => {
                    trace_close_receipt(&config.name, &receipt);
                    Some(McpServerCloseOutcome::Stdio(receipt))
                }
                Err(CustodyNotHeld::NeverSpawned) => None,
                Err(not_held) => Some(not_held_outcome(not_held)),
            },
        };
        Err(McpConnectFailure {
            error,
            timed_out,
            close,
        })
    }
}

/// A [`McpConnection::connect_within`] that did not connect.
#[derive(Debug)]
pub struct McpConnectFailure {
    pub error: McpError,
    /// The connect did not finish within its timeout.
    pub timed_out: bool,
    /// The close of the server process the attempt spawned, run through its
    /// custody before returning; `None` when no process was spawned.
    pub close: Option<McpServerCloseOutcome>,
}

/// What one close of an MCP server connection observed.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum McpServerCloseOutcome {
    /// A stdio server's close: what it observed of the server's process and
    /// stdout, whatever the leader outcome (a failed reap included). A failed
    /// client cancel next to it is traced only; the receipt alone says what
    /// the close observed of the process.
    Stdio(Arc<McpStdioCloseReceipt>),
    /// A connection with no server process (a remote transport) whose
    /// client closed.
    NoProcess,
    /// A stdio server's process was never spawned: nothing to end.
    NeverSpawned,
    /// The server's process close was not this close's, and its receipt
    /// could not be joined.
    NotHeld(McpCustodyNotHeld),
    /// The close failed with no process to observe: a client close failed
    /// on a remote transport, or for a stdio server never spawned.
    CloseFailed(Arc<McpError>),
    /// The task running the close ended without reporting.
    TaskFailed { panicked: bool },
}

impl McpServerCloseOutcome {
    /// Whether this close settled its server, from the receipt's final facts
    /// only (never its `settled_in`): a stdio server's leader was reaped and
    /// every holder of its stdout closed it. A connection with no process,
    /// or a server never spawned, settles trivially. Nothing is promised
    /// about processes outside the server's stdout and its leader.
    #[must_use]
    pub fn is_settled(&self) -> bool {
        match self {
            Self::Stdio(receipt) => receipt_settled(receipt),
            Self::NoProcess | Self::NeverSpawned => true,
            Self::NotHeld(_) | Self::CloseFailed(_) | Self::TaskFailed { .. } => false,
        }
    }
}

/// Why a close has no receipt of its server's process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpCustodyNotHeld {
    /// Another close of the same process owned it and had not finished.
    ClosingElsewhere,
    /// Another close of the same process owned it and finished.
    ClosedElsewhere,
    /// The process's close task was dropped before it finished (its runtime
    /// shut down): its group was killed without its exit being observed.
    AbandonedElsewhere,
}

/// A stdio close settled: its leader was reaped and every holder of its
/// stdout closed it.
fn receipt_settled(receipt: &McpStdioCloseReceipt) -> bool {
    matches!(receipt.leader, McpStdioLeaderOutcome::Reaped(_))
        && matches!(receipt.witness, McpStdioWitnessOutcome::Closed(_))
}

/// Shared custody of one connection attempt's client service, for every
/// transport. The connection reaches its server through the client's peer
/// ([`HeldClient`]); a close takes the running service from here, so it can
/// start without the connection (from its router's [`CloseIndex`]), and
/// every other close of the same client joins that one for its result.
#[derive(Clone, Default)]
pub(crate) struct ClientCustody {
    slot: Arc<std::sync::Mutex<ClientSlot>>,
    /// Bumped whenever the slot changes, so its waiters recheck it.
    progress: Arc<tokio::sync::watch::Sender<u64>>,
}

/// Where a client custody's client stands.
#[derive(Default)]
enum ClientSlot {
    /// No client yet: its connect attempt has not finished its handshake.
    #[default]
    Vacant,
    Held(ConnectedClient),
    /// A close task owns the client and waits for its service until
    /// `started` plus `bound`.
    Closing {
        started: tokio::time::Instant,
        bound: Duration,
    },
    /// That close task finished; its result is shared with every join.
    Closed(ClientClose),
    /// That close task was dropped before finishing (its runtime shut down).
    Abandoned,
    /// Closed before it held a client: its attempt is refused or stopped, and
    /// a client the attempt still connects is closed at once.
    Retired,
    /// Its connect attempt ended without a client: nothing to close.
    Unconnected,
}

/// How one close of a client ended.
#[derive(Debug, Clone)]
pub(crate) enum ClientClose {
    /// Its service ended.
    Ended,
    /// Waiting for its service failed: the service task panicked or was
    /// cancelled.
    WaitFailed(Arc<str>),
    /// Its service had not ended within the close's bound; the wait was
    /// dropped and the service left to end on its own.
    TimedOut(Duration),
}

/// How a client custody ended, as its joins read it.
#[derive(Debug, Clone)]
pub(crate) enum ClientEnd {
    Closed(ClientClose),
    /// The close task was dropped before it finished.
    Abandoned,
    /// The connect attempt ended without a client: there was nothing to
    /// close.
    Unconnected,
}

/// Marks a client custody `Closed` when its close task finishes, and
/// `Abandoned` when that task is dropped first; either way it then wakes the
/// custody's waiters.
struct ClientCloseOwner {
    custody: ClientCustody,
    finished: bool,
}

impl ClientCloseOwner {
    fn finish(mut self, closed: ClientClose) {
        self.finished = true;
        *self.custody.slot() = ClientSlot::Closed(closed);
    }
}

impl Drop for ClientCloseOwner {
    fn drop(&mut self) {
        if !self.finished {
            *self.custody.slot() = ClientSlot::Abandoned;
        }
        self.custody.changed();
    }
}

impl ClientCustody {
    fn slot(&self) -> std::sync::MutexGuard<'_, ClientSlot> {
        self.slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn changed(&self) {
        self.progress
            .send_modify(|seen| *seen = seen.wrapping_add(1));
    }

    /// Whether two handles share one custody.
    fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.slot, &other.slot)
    }

    /// Refuse an attempt whose custody was closed before it began: it has
    /// no effect yet, and gets none.
    fn admit(&self) -> Result<(), McpError> {
        match &*self.slot() {
            ClientSlot::Vacant => Ok(()),
            ClientSlot::Held(_)
            | ClientSlot::Closing { .. }
            | ClientSlot::Closed(_)
            | ClientSlot::Abandoned
            | ClientSlot::Retired
            | ClientSlot::Unconnected => Err(McpError::RouterShutDown),
        }
    }

    /// Hold `client`, the attempt's connected client, and return the
    /// connection's handle to it, whose drop closes it within `bound`. A
    /// custody closed while the attempt connected closes the client at once,
    /// within `bound`, and refuses it.
    fn deposit(&self, client: ConnectedClient, bound: Duration) -> Result<HeldClient, McpError> {
        let peer = rmcp::service::Peer::clone(&client);
        let mcp_apps = client.supports_mcp_apps();
        let mut slot = self.slot();
        if matches!(*slot, ClientSlot::Vacant) {
            *slot = ClientSlot::Held(client);
            drop(slot);
            self.changed();
            return Ok(HeldClient {
                peer,
                mcp_apps,
                custody: self.clone(),
                bound,
            });
        }
        // Closed first (its close index began its close): no client stays
        // held after that.
        let started = tokio::time::Instant::now();
        *slot = ClientSlot::Closing { started, bound };
        drop(slot);
        self.spawn_close(client, started, bound);
        self.changed();
        Err(McpError::RouterShutDown)
    }

    /// Start this client's close now, from synchronous code, waiting for its
    /// service at most `bound`: take the held client and close it in a task
    /// of its own, so dropping any wait for it never stops it. A custody with
    /// no client yet is retired instead. A close already started, or a
    /// custody already finished, is left as it is.
    fn start_close(&self, started: tokio::time::Instant, bound: Duration) {
        let mut slot = self.slot();
        match std::mem::replace(&mut *slot, ClientSlot::Closing { started, bound }) {
            ClientSlot::Held(client) => {
                drop(slot);
                self.spawn_close(client, started, bound);
            }
            ClientSlot::Vacant => {
                *slot = ClientSlot::Retired;
                drop(slot);
            }
            other => {
                *slot = other;
                return;
            }
        }
        self.changed();
    }

    /// Retire a custody that holds no client yet: its attempt is refused or
    /// stopped, and a client it still connects is closed at once.
    fn retire(&self) {
        let mut slot = self.slot();
        if matches!(*slot, ClientSlot::Vacant) {
            *slot = ClientSlot::Retired;
            drop(slot);
            self.changed();
        }
    }

    fn spawn_close(&self, client: ConnectedClient, started: tokio::time::Instant, bound: Duration) {
        let owner = ClientCloseOwner {
            custody: self.clone(),
            finished: false,
        };
        tokio::spawn(async move {
            owner.finish(await_client_close(started, bound, client.cancel()).await);
        });
    }

    /// Wait until this custody's client close, or its attempt, has finished.
    async fn join(&self) -> ClientEnd {
        let mut progress = self.progress.subscribe();
        loop {
            match &*self.slot() {
                ClientSlot::Closed(closed) => return ClientEnd::Closed(closed.clone()),
                ClientSlot::Abandoned => return ClientEnd::Abandoned,
                ClientSlot::Unconnected => return ClientEnd::Unconnected,
                ClientSlot::Vacant
                | ClientSlot::Held(_)
                | ClientSlot::Closing { .. }
                | ClientSlot::Retired => {}
            }
            // The custody holds the sender, so this never fails while waiting.
            let _ = progress.changed().await;
        }
    }

    /// The connect attempt ended. A custody it left without a client has
    /// nothing to close.
    fn attempt_ended(&self) {
        let mut slot = self.slot();
        if matches!(*slot, ClientSlot::Vacant | ClientSlot::Retired) {
            *slot = ClientSlot::Unconnected;
            drop(slot);
            self.changed();
        }
    }

    /// When the close running now will have ended: its start plus its bound.
    /// `None` when no close runs.
    fn close_deadline(&self) -> Option<tokio::time::Instant> {
        match &*self.slot() {
            ClientSlot::Closing { started, bound } => Some(*started + *bound),
            ClientSlot::Vacant
            | ClientSlot::Held(_)
            | ClientSlot::Closed(_)
            | ClientSlot::Abandoned
            | ClientSlot::Retired
            | ClientSlot::Unconnected => None,
        }
    }

    /// Whether its attempt may still connect a client (retired while it
    /// connected): that client is closed at once.
    fn awaits_attempt(&self) -> bool {
        matches!(*self.slot(), ClientSlot::Retired)
    }

    /// Whether nothing of this custody is left to report: its client closed
    /// cleanly, or its attempt ended without one.
    fn finished_settled(&self) -> bool {
        matches!(
            *self.slot(),
            ClientSlot::Closed(ClientClose::Ended) | ClientSlot::Unconnected
        )
    }

    /// Cancel the held client's service without taking it: the service ends
    /// on its own, as when it is dropped, and a later close still joins it.
    fn cancel_held(&self) {
        if let ClientSlot::Held(client) = &*self.slot() {
            client.cancellation_token().cancel();
        }
    }
}

/// Wait for a client's close (its service's end) at most until `started`
/// plus `bound`.
async fn await_client_close<T>(
    started: tokio::time::Instant,
    bound: Duration,
    close: impl std::future::Future<Output = Result<T, tokio::task::JoinError>>,
) -> ClientClose {
    match tokio::time::timeout_at(started + bound, close).await {
        Ok(Ok(_)) => ClientClose::Ended,
        Ok(Err(error)) => ClientClose::WaitFailed(format!("{error:?}").into()),
        Err(_) => ClientClose::TimedOut(bound),
    }
}

/// What a connection holds of its client: the peer it reaches its server
/// through, and the custody that owns the running client and closes it.
/// Dropped without a close, it starts the client's close through the custody
/// (within a Tokio runtime; elsewhere it cancels the service), as dropping
/// the service itself would cancel it, so that close is still observed.
pub(crate) struct HeldClient {
    peer: rmcp::service::Peer<rmcp::RoleClient>,
    mcp_apps: bool,
    custody: ClientCustody,
    /// How long a close started by this handle's drop waits for the service.
    bound: Duration,
}

impl std::ops::Deref for HeldClient {
    type Target = rmcp::service::Peer<rmcp::RoleClient>;

    fn deref(&self) -> &Self::Target {
        &self.peer
    }
}

impl HeldClient {
    /// `client`, held by a custody of its own.
    pub(crate) fn standalone(client: ConnectedClient) -> Self {
        let custody = ClientCustody::default();
        let held = Self {
            peer: rmcp::service::Peer::clone(&client),
            mcp_apps: client.supports_mcp_apps(),
            custody: custody.clone(),
            bound: REMOTE_CLOSE_BOUND,
        };
        *custody.slot() = ClientSlot::Held(client);
        held
    }

    pub(crate) fn supports_mcp_apps(&self) -> bool {
        self.mcp_apps
    }

    /// Close the client, waiting for its service until `deadline`; when
    /// another close of it started first, join that close instead.
    pub(crate) async fn close_until(self, deadline: tokio::time::Instant) -> ClientEnd {
        let started = tokio::time::Instant::now();
        self.custody
            .start_close(started, deadline.saturating_duration_since(started));
        self.custody.join().await
    }
}

/// The existing already-running unit constructors stay source-compatible.
impl From<rmcp::service::RunningService<rmcp::RoleClient, ()>> for HeldClient {
    fn from(service: rmcp::service::RunningService<rmcp::RoleClient, ()>) -> Self {
        Self::standalone(service.into())
    }
}

impl Drop for HeldClient {
    fn drop(&mut self) {
        if tokio::runtime::Handle::try_current().is_ok() {
            self.custody
                .start_close(tokio::time::Instant::now(), self.bound);
        } else {
            self.custody.cancel_held();
        }
    }
}

/// The failure of a client close, as a close of its connection reports it:
/// `None` when its service ended (or there was no client to close).
fn client_close_failure(server: &str, end: &ClientEnd) -> Option<McpError> {
    match end {
        ClientEnd::Closed(ClientClose::Ended) | ClientEnd::Unconnected => None,
        ClientEnd::Closed(ClientClose::WaitFailed(reason)) => Some(McpError::ConnectionFailed {
            reason: format!("Failed to close connection: {reason}"),
        }),
        ClientEnd::Closed(ClientClose::TimedOut(bound)) => Some(McpError::CloseTimedOut {
            server: server.to_owned(),
            bound: *bound,
        }),
        ClientEnd::Abandoned => Some(McpError::ConnectionFailed {
            reason: "Failed to close connection: its close task was dropped before it finished"
                .to_string(),
        }),
    }
}

/// Every close one router owns, kept outside the router lock: each connect
/// attempt's process custody (stdio) and client custody from the moment the
/// attempt is admitted, and so every connection made from them, its earlier
/// closes that ended unsettled included. The router and its adapter share it
/// ([`crate::McpRouterAdapter::begin_close`]).
#[derive(Clone, Default)]
pub(crate) struct CloseIndex(Arc<CloseIndexShared>);

#[derive(Default)]
struct CloseIndexShared {
    state: std::sync::Mutex<CloseIndexState>,
    /// Becomes `true` when the close begins, so admitted attempts stop.
    closing: tokio::sync::watch::Sender<bool>,
    /// The router's attempt guards, cancelled with `RouterClosed` when the
    /// close begins, before any attempt is stopped.
    guards: std::sync::OnceLock<std::sync::Weak<crate::attempt_guard::RouterGuards>>,
}

#[derive(Default)]
struct CloseIndexState {
    /// Set by [`CloseIndex::begin_close`]: admission is frozen, and no
    /// custody is indexed after it.
    closing: bool,
    entries: Vec<IndexedClose>,
}

/// One admitted connect attempt's custodies.
#[derive(Clone)]
struct IndexedClose {
    server: String,
    /// The stdio server's process; `None` for a remote server.
    process: Option<StdioChildCustody>,
    client: ClientCustody,
}

impl CloseIndex {
    fn state(&self) -> std::sync::MutexGuard<'_, CloseIndexState> {
        self.0
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Admit one connect attempt of `server`. This is the one boundary where
    /// a router creates an attempt's custodies, and it is atomic with the
    /// index's admission freeze: under the index lock the custodies are
    /// created (a process custody under `stdio_bounds` for a stdio server)
    /// and indexed, before anything can spawn or connect through them. Once
    /// the index is closing they are created closed instead and not indexed:
    /// the attempt is refused before any effect.
    pub(crate) fn admit(
        &self,
        server: &str,
        stdio_bounds: Option<McpStdioCloseBounds>,
    ) -> AdmittedAttempt {
        let diagnostics = stdio_bounds.map(|_| crate::stdio_diagnostics::StdioAttemptRecord::new());
        let process = stdio_bounds.map(|bounds| {
            let custody = StdioChildCustody::with_bounds(bounds);
            match &diagnostics {
                Some(record) => custody.with_diagnostics(Arc::downgrade(record)),
                None => custody,
            }
        });
        let client = ClientCustody::default();
        let mut state = self.state();
        if state.closing {
            drop(state);
            if let Some(process) = &process {
                process.retire_if_vacant();
            }
            client.retire();
        } else {
            state.entries.retain(|entry| !entry.finished_settled());
            state.entries.push(IndexedClose {
                server: server.to_owned(),
                process: process.clone(),
                client: client.clone(),
            });
        }
        AdmittedAttempt {
            stop: AttemptStop(self.0.closing.subscribe()),
            end: AttemptEnd {
                process: process.clone(),
                client: client.clone(),
            },
            process,
            client,
            diagnostics,
        }
    }

    /// Install the router's attempt guards, cancelled when the close begins.
    pub(crate) fn set_attempt_guards(&self, guards: &Arc<crate::attempt_guard::RouterGuards>) {
        let _ = self.0.guards.set(Arc::downgrade(guards));
    }

    /// Freeze admission and start, now, every close this index covers that
    /// has not started, without any router lock or in-flight call: a held
    /// stdio server's client is closed and then its process, under that
    /// custody's own bounds; a held remote client is closed within
    /// [`McpStdioCloseBounds::close_bound`] of the defaults; a custody that
    /// holds nothing yet is retired, so its attempt stops and spawns or
    /// connects nothing more (a client it still connects is closed at
    /// once). A close already running is joined. A later call returns a
    /// ticket over the same closes. Call it within a Tokio runtime.
    pub(crate) fn begin_close(&self) -> McpCloseTicket {
        // The router is closing: its attempts' guards learn it first, so an
        // attempt stopped below is cancelled as `RouterClosed`.
        if let Some(guards) = self.0.guards.get().and_then(std::sync::Weak::upgrade) {
            guards.close(crate::McpAttemptCancelCause::RouterClosed);
        }
        let now = tokio::time::Instant::now();
        let closes = {
            let mut state = self.state();
            if !state.closing {
                // Freezing: closes that had already ended settled leave
                // nothing to report; every other one stays, an earlier
                // close that ended unsettled included.
                state.entries.retain(|entry| !entry.finished_settled());
                state.closing = true;
            }
            state.entries.clone()
        };
        self.0.closing.send_replace(true);
        let ends = closes
            .iter()
            .map(|close| close.begin(now))
            .fold(now, std::cmp::Ord::max);
        McpCloseTicket {
            closes,
            deadline: ends + McpCloseTicket::JOIN_ALLOWANCE,
        }
    }

    /// Whether [`Self::begin_close`] ran.
    pub(crate) fn is_closing(&self) -> bool {
        self.state().closing
    }

    /// Whether this index holds the attempt `client` is the client custody
    /// of: its close is this index's to report.
    pub(crate) fn covers_client(&self, client: &ClientCustody) -> bool {
        self.state()
            .entries
            .iter()
            .any(|entry| entry.client.same(client))
    }

    /// Whether this index holds the attempt `process` is the process custody
    /// of: its close is this index's to report.
    pub(crate) fn covers_process(&self, process: &StdioChildCustody) -> bool {
        self.state().entries.iter().any(|entry| {
            entry
                .process
                .as_ref()
                .is_some_and(|indexed| indexed.same_custody(process))
        })
    }
}

impl IndexedClose {
    /// Whether nothing of this attempt is left to report.
    fn finished_settled(&self) -> bool {
        match &self.process {
            Some(process) => process.finished_settled(),
            None => self.client.finished_settled(),
        }
    }

    /// Start this close at `now` unless one runs or ran, and return when it
    /// will have ended: its own start plus its bound.
    fn begin(&self, now: tokio::time::Instant) -> tokio::time::Instant {
        match &self.process {
            Some(process) => {
                let client = self.client.clone();
                let eof_grace = process.bounds.eof_grace();
                let eof_deadline = now + eof_grace;
                // As in an ordinary close, the client goes first: its stdin
                // closing is the server's EOF. The client close and its wait
                // end by this close's own EOF deadline, counted from its
                // recorded start however late this first runs.
                let close_client = async move {
                    client.start_close(now, eof_grace);
                    let _ = tokio::time::timeout_at(eof_deadline, client.join()).await;
                };
                match process.start_close(now, close_client) {
                    Ok(_) => now + process.bounds.close_bound(),
                    Err(_) => {
                        // Nothing spawned yet: the attempt stops, and no
                        // client may follow.
                        self.client.retire();
                        process.close_deadline().unwrap_or(now)
                    }
                }
            }
            None => {
                self.client.start_close(now, REMOTE_CLOSE_BOUND);
                match self.client.close_deadline() {
                    Some(ends) => ends,
                    // A client its attempt still connects is closed at once.
                    None if self.client.awaits_attempt() => now + REMOTE_CLOSE_BOUND,
                    None => now,
                }
            }
        }
    }

    /// This attempt's close outcome once it has reported; `None` for a remote
    /// attempt that ended without connecting, which had nothing to close.
    async fn outcome(&self) -> Option<McpServerCloseOutcome> {
        match &self.process {
            Some(process) => Some(match process.join_close().await {
                Ok(receipt) => McpServerCloseOutcome::Stdio(receipt),
                Err(not_held) => not_held_outcome(not_held),
            }),
            None => match self.client.join().await {
                ClientEnd::Unconnected => None,
                end => Some(close_outcome(
                    &self.server,
                    ObservedClose {
                        process: ObservedProcessClose::NoCustody,
                        cancel_failure: client_close_failure(&self.server, &end),
                    },
                )),
            },
        }
    }
}

/// One admitted connect attempt: its custodies, the signal it stops on, and
/// the marker of its end.
pub(crate) struct AdmittedAttempt {
    pub(crate) process: Option<StdioChildCustody>,
    pub(crate) client: ClientCustody,
    pub(crate) stop: AttemptStop,
    pub(crate) end: AttemptEnd,
    /// A stdio attempt's setup diagnostics: the only strong reference, for
    /// the router to hold (its custody holds them weakly).
    pub(crate) diagnostics: Option<Arc<crate::stdio_diagnostics::StdioAttemptRecord>>,
}

/// Resolves once its index begins its close: the attempt stops there,
/// dropping whatever it had not connected.
pub(crate) struct AttemptStop(tokio::sync::watch::Receiver<bool>);

impl AttemptStop {
    pub(crate) async fn requested(mut self) {
        if self.0.wait_for(|closing| *closing).await.is_err() {
            // The index is gone, so no close can begin.
            std::future::pending::<()>().await;
        }
    }
}

/// Marks the end of one connect attempt when dropped (it returned, or it was
/// dropped): a custody it left without a client or a process has nothing to
/// close, and none can come.
pub(crate) struct AttemptEnd {
    process: Option<StdioChildCustody>,
    client: ClientCustody,
}

impl Drop for AttemptEnd {
    fn drop(&mut self) {
        self.client.attempt_ended();
        if let Some(process) = &self.process {
            process.retire_if_vacant();
        }
    }
}

/// The closes [`crate::McpRouterAdapter::begin_close`] started or joined:
/// one per connect attempt its router admitted.
#[must_use]
pub struct McpCloseTicket {
    closes: Vec<IndexedClose>,
    deadline: tokio::time::Instant,
}

impl McpCloseTicket {
    /// How long past its own close bound a close's task may take to report.
    pub const JOIN_ALLOWANCE: Duration = Duration::from_secs(2);

    /// When every close this ticket covers will have reported: the latest
    /// end of a close it started or joined (that close's own start, plus
    /// its [`McpStdioCloseBounds::close_bound`] for a stdio server or the
    /// default close bound for a remote one), plus [`Self::JOIN_ALLOWANCE`].
    #[must_use]
    pub fn deadline(&self) -> tokio::time::Instant {
        self.deadline
    }

    /// One record per close, once it has reported: each stdio server's
    /// process close (its receipt, or why it has none) and each remote
    /// connection's client close ([`McpServerCloseOutcome::NoProcess`] only
    /// when its client ended), for every attempt whose close had not ended
    /// settled when the close began, an earlier close that ended unsettled
    /// included. A remote attempt that ended without connecting had nothing
    /// to close and has no record. No close is aborted.
    pub async fn outcomes(self) -> Vec<crate::McpServerCloseRecord> {
        futures::future::join_all(self.closes.into_iter().map(|close| async move {
            close
                .outcome()
                .await
                .map(|outcome| crate::McpServerCloseRecord {
                    server: close.server,
                    outcome,
                })
        }))
        .await
        .into_iter()
        .flatten()
        .collect()
    }
}

/// The outcome of a close whose custody gave no receipt.
pub(crate) fn not_held_outcome(not_held: CustodyNotHeld) -> McpServerCloseOutcome {
    match not_held {
        CustodyNotHeld::NeverSpawned => McpServerCloseOutcome::NeverSpawned,
        CustodyNotHeld::ClosingElsewhere => {
            McpServerCloseOutcome::NotHeld(McpCustodyNotHeld::ClosingElsewhere)
        }
        CustodyNotHeld::ClosedElsewhere => {
            McpServerCloseOutcome::NotHeld(McpCustodyNotHeld::ClosedElsewhere)
        }
        CustodyNotHeld::AbandonedElsewhere => {
            McpServerCloseOutcome::NotHeld(McpCustodyNotHeld::AbandonedElsewhere)
        }
    }
}

/// The outcome of one observed connection close. A failed client cancel
/// with no process at all is the close's failure; next to a process receipt,
/// or a process close owned elsewhere, it is traced only.
pub(crate) fn close_outcome(server: &str, observed: ObservedClose) -> McpServerCloseOutcome {
    let ObservedClose {
        process,
        cancel_failure,
    } = observed;
    let no_process = matches!(
        process,
        ObservedProcessClose::NoCustody
            | ObservedProcessClose::NotHeld(CustodyNotHeld::NeverSpawned)
    );
    match (process, cancel_failure) {
        (_, Some(failure)) if no_process => McpServerCloseOutcome::CloseFailed(Arc::new(failure)),
        (process, cancel_failure) => {
            if let Some(failure) = cancel_failure {
                tracing::warn!(server, error = %failure, "MCP connection close failed");
            }
            match process {
                ObservedProcessClose::Receipt(receipt) => McpServerCloseOutcome::Stdio(receipt),
                ObservedProcessClose::NoCustody => McpServerCloseOutcome::NoProcess,
                ObservedProcessClose::NotHeld(not_held) => not_held_outcome(not_held),
            }
        }
    }
}

/// What admits one `tools/call` at a connection, before any request is
/// prepared or sent.
#[derive(Debug, Clone, Copy)]
pub(crate) enum CallAdmission<'a> {
    /// The connection's own host access ceiling: every operation without
    /// one.
    Ceiling(Option<&'a meerkat_core::McpServerAccess>),
    /// A host's checked route the router resolved the call through: exactly
    /// the raw operations selected for that route, whatever the ceiling.
    CheckedRoute(&'a std::collections::BTreeSet<String>),
}

impl CallAdmission<'_> {
    pub(crate) fn admits(self, raw_operation: &str) -> bool {
        match self {
            Self::Ceiling(access) => access.is_none_or(|access| access.permits(raw_operation)),
            Self::CheckedRoute(operations) => operations.contains(raw_operation),
        }
    }
}

/// The one `tools/call` path of a connected service, shared by
/// [`McpConnection`] and the [`crate::McpProtocol`] it converts into.
///
/// A known-dead session refuses the call before it is queued. Otherwise the
/// call carries its own [`RequestDispatch`] witness through the transport,
/// and a failure is typed from that witness alone: sent and then answered
/// `404` for its session is uncertain ([`McpError::SessionExpired`]);
/// redirected, as shown by the request's own returned response, and then
/// failed is uncertain ([`McpError::RedirectedOutcomeUncertain`]); refused
/// at transport entry is affirmatively unsent
/// ([`McpError::ServerUnavailable`]); anything else, including no recorded
/// disposition or a transport failure with no response (which can hide a
/// followed redirect), keeps the ordinary [`McpError::ToolCallFailed`].
/// That ordinary failure does not prove the call had no effect. The session
/// is never re-initialized and nothing is re-sent; a followed same-origin
/// redirect is itself more than one physical request.
// The connection's per-call owners (protected metadata, session expiry and
// OAuth target) are passed separately so both callers share one path.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn call_tool_on(
    service: &rmcp::service::Peer<rmcp::RoleClient>,
    server: &str,
    admission: CallAdmission<'_>,
    protected_metadata: &ProtectedMetadataState,
    session_expiry: &SessionExpiryRecorder,
    oauth_target: Option<&McpServerIdentity>,
    name: &str,
    args: &Value,
    metadata: Option<serde_json::Map<String, Value>>,
    enter: impl FnOnce() -> Result<Option<crate::transport::protected::SendCustody>, McpError>,
) -> Result<CallToolResult, McpError> {
    // The host access policy is the connection's own ceiling: an operation it
    // does not admit is refused before any request is prepared or sent,
    // whatever the caller. A checked route admits exactly its own selected
    // operations instead, and nothing else.
    if !admission.admits(name) {
        return Err(McpError::OperationNotPermitted {
            server: server.to_owned(),
            operation: name.to_owned(),
        });
    }
    let params = match args.as_object().cloned() {
        Some(arguments) => CallToolRequestParams::new(name.to_string()).with_arguments(arguments),
        None => CallToolRequestParams::new(name.to_string()),
    };
    // A dropped session is never resumed under another session.
    if session_expiry.expired() {
        return Err(session_dead(server));
    }
    let dispatch = RequestDispatch::default();
    let mut request = CallToolRequest::new(params);
    request.extensions.insert(dispatch.clone());
    if let Some(metadata) = metadata {
        protected_metadata.register(&metadata)?;
        request.extensions.insert(ProtectedMetadata(metadata));
    }
    // Reviewed boundary: the local transport handoff. The single consuming
    // native entry step runs with no local preparation left that could fail
    // before it.
    if let Some(custody) = enter()? {
        // The request carries it to the transport's send owner, which takes
        // it out before encoding and holds it until the bytes are flushed or
        // the writer is retired, even when this call is dropped first.
        request.extensions.insert(custody);
    }
    let request = request.into();
    let result = service.send_request(request).await.map_err(|error| {
        let disposition = dispatch.disposition();
        // An OAuth refusal (a 401, or no usable credential before
        // dispatch) is typed unless the request's own disposition already
        // makes its outcome uncertain or proves it unsent.
        if matches!(disposition, Some(RequestDisposition::Sent) | None)
            && let Some(refused) = authorization_required(oauth_target, &error)
        {
            return refused;
        }
        tool_call_failure(server, name, disposition, &error)
    })?;
    match result {
        ServerResult::CallToolResult(result) => Ok(result),
        _ => Err(McpError::ProtocolError {
            message: "unexpected MCP tools/call response".into(),
        }),
    }
}

/// The typed host status for a request that an OAuth connection's server
/// refused with a `401`, or that was refused before dispatch for want of a
/// usable credential. Such a request is never replayed.
fn authorization_required(
    target: Option<&McpServerIdentity>,
    error: &rmcp::ServiceError,
) -> Option<McpError> {
    let target = target?;
    let rmcp::ServiceError::TransportSend(transport) = error else {
        return None;
    };
    matches!(
        transport
            .error
            .downcast_ref::<StreamableHttpError<reqwest::Error>>(),
        Some(StreamableHttpError::AuthRequired(_))
    )
    .then(|| McpError::AuthorizationRequired {
        target: Box::new(target.clone()),
    })
}

fn session_dead(server: &str) -> McpError {
    McpError::ServerUnavailable {
        server: server.to_owned(),
        state: "session expired; reconnect required".into(),
    }
}

/// The typed failure of one `tools/call`, from its own disposition.
fn tool_call_failure(
    server: &str,
    tool: &str,
    disposition: Option<RequestDisposition>,
    error: &dyn std::fmt::Display,
) -> McpError {
    match disposition {
        // Answered 404 for its own session; not re-sent, and the session is
        // not re-initialized.
        Some(RequestDisposition::SentSessionExpired) => McpError::SessionExpired {
            server: server.to_owned(),
            tool: tool.to_owned(),
        },
        // Nothing was sent: the session was already known dead.
        Some(RequestDisposition::RefusedExpired) => session_dead(server),
        // Nothing was sent: the host egress profile refused the destination.
        Some(RequestDisposition::RefusedEgress(error)) => McpError::from_egress(server, error),
        // Redirected (followed or stopped) and then failed: some hop may
        // have taken effect.
        Some(RequestDisposition::SentRedirected) => McpError::RedirectedOutcomeUncertain {
            server: server.to_owned(),
            tool: tool.to_owned(),
            reason: error.to_string(),
        },
        Some(RequestDisposition::Sent) | None => McpError::ToolCallFailed {
            tool: tool.to_owned(),
            reason: error.to_string(),
        },
    }
}

/// Cancel the client service (closing a stdio server's stdin), then end the
/// server's process under its custody's bounds and observe its exit. For a
/// stdio server the cancel runs inside the custody's close task, which exists
/// before anything is awaited, so dropping this call cannot strand the
/// process between the cancel and its close. The close clock starts before
/// the cancel, which may use at most the EOF grace: past it the cancel is
/// dropped (dropping the transport, which closes the stdin) and escalation
/// proceeds. The process is ended even when the cancel fails. Returns the
/// receipt only when this call owned the process's close; a receipt whose
/// reap failed is returned as that OS error instead.
pub(crate) async fn close_connected(
    server: &str,
    service: HeldClient,
    stdio_child: Option<StdioChildCustody>,
) -> Result<Option<McpStdioCloseReceipt>, McpError> {
    let ObservedClose {
        process,
        cancel_failure,
    } = close_connected_observed(server, service, stdio_child, false).await;
    let receipt = match process {
        // The custody keeps its own copy of the receipt for later joins.
        ObservedProcessClose::Receipt(receipt) => {
            Some(Arc::try_unwrap(receipt).unwrap_or_else(|shared| duplicate_receipt(&shared)))
        }
        ObservedProcessClose::NoCustody | ObservedProcessClose::NotHeld(_) => None,
    };
    // A failed reap (its status consumed by another reaper, say) fails the
    // close with that OS error; the receipt was traced above.
    if let Some(McpStdioCloseReceipt {
        leader: McpStdioLeaderOutcome::WaitFailed(error),
        ..
    }) = receipt
    {
        return Err(McpError::Io(error));
    }
    if let Some(failure) = cancel_failure {
        return Err(failure);
    }
    Ok(receipt)
}

/// An owned copy of a shared receipt. A failed reap's OS error keeps its
/// error number; an error without one keeps its kind and message.
fn duplicate_receipt(receipt: &McpStdioCloseReceipt) -> McpStdioCloseReceipt {
    McpStdioCloseReceipt {
        settled_in: receipt.settled_in,
        leader_exit: receipt.leader_exit,
        witness: receipt.witness,
        sigterm: receipt.sigterm,
        sigkill: receipt.sigkill,
        final_group_signal: receipt.final_group_signal,
        leader: match &receipt.leader {
            McpStdioLeaderOutcome::Reaped(status) => McpStdioLeaderOutcome::Reaped(*status),
            McpStdioLeaderOutcome::WaitFailed(error) => {
                McpStdioLeaderOutcome::WaitFailed(error.raw_os_error().map_or_else(
                    || std::io::Error::new(error.kind(), error.to_string()),
                    std::io::Error::from_raw_os_error,
                ))
            }
            McpStdioLeaderOutcome::NotReaped => McpStdioLeaderOutcome::NotReaped,
        },
        elapsed: receipt.elapsed,
    }
}

/// Everything one [`close_connected_observed`] saw: what happened to the
/// connection's server process, and the client cancel's own failure.
pub(crate) struct ObservedClose {
    pub(crate) process: ObservedProcessClose,
    /// The client cancel failed. `None` when it finished, or outlasted the
    /// EOF grace and was dropped.
    pub(crate) cancel_failure: Option<McpError>,
}

/// What one close saw of its connection's server process.
pub(crate) enum ObservedProcessClose {
    /// The connection held no stdio custody: a remote transport.
    NoCustody,
    /// The process's close: its receipt, whatever its leader outcome (a
    /// failed reap included), traced by the call that ran the close.
    Receipt(Arc<McpStdioCloseReceipt>),
    /// No receipt of the process's close: never spawned, owned elsewhere
    /// (when not joined), or abandoned.
    NotHeld(CustodyNotHeld),
}

/// [`close_connected`] keeping everything it observed: the receipt even when
/// its reap failed, why there is no receipt, and the cancel's failure next
/// to the receipt rather than in its place. With `join`, a close of the same
/// process that another call started is waited for, and its receipt kept.
pub(crate) async fn close_connected_observed(
    server: &str,
    service: HeldClient,
    stdio_child: Option<StdioChildCustody>,
    join: bool,
) -> ObservedClose {
    let started = tokio::time::Instant::now();
    let Some(custody) = stdio_child else {
        // With no process close to bound it, the client close is bounded by
        // the default close bound itself; a close of the same client that
        // started elsewhere is joined.
        let closed = service.close_until(started + REMOTE_CLOSE_BOUND).await;
        return ObservedClose {
            process: ObservedProcessClose::NoCustody,
            cancel_failure: client_close_failure(server, &closed),
        };
    };
    let eof_deadline = started + custody.bounds.eof_grace();
    let cancel_server = server.to_owned();
    let cancel = async move {
        // The client close runs in its custody's own task: past the EOF
        // grace only this wait is dropped.
        match tokio::time::timeout_at(eof_deadline, service.close_until(eof_deadline)).await {
            Ok(ClientEnd::Closed(ClientClose::TimedOut(_))) | Err(_) => {
                tracing::debug!(
                    server = %cancel_server,
                    "MCP client cancel outlasted the EOF grace; dropped"
                );
                None
            }
            Ok(closed) => client_close_failure(&cancel_server, &closed),
        }
    };
    let (cancel_failure, receipt) = custody.terminate_after(started, cancel).await;
    let process = match receipt {
        Ok(receipt) => {
            trace_close_receipt(server, &receipt);
            ObservedProcessClose::Receipt(receipt)
        }
        // The close another call started is this process's close: its
        // receipt (traced by that call) is this close's too.
        Err(CustodyNotHeld::ClosingElsewhere | CustodyNotHeld::ClosedElsewhere) if join => {
            match custody.join_close().await {
                Ok(receipt) => ObservedProcessClose::Receipt(receipt),
                Err(not_held) => ObservedProcessClose::NotHeld(not_held),
            }
        }
        Err(not_held) => {
            tracing::debug!(server, ?not_held, "MCP stdio server close owned elsewhere");
            ObservedProcessClose::NotHeld(not_held)
        }
    };
    ObservedClose {
        process,
        cancel_failure: cancel_failure.flatten(),
    }
}

/// Trace every fact of a close receipt. A close that settled before any
/// escalation and reaped its server is routine; anything else is a warning.
/// Neither says the server's whole group exited: a sent signal and a finished
/// close show only what the receipt records.
pub(crate) fn trace_close_receipt(server: &str, receipt: &McpStdioCloseReceipt) {
    let McpStdioCloseReceipt {
        settled_in,
        leader_exit,
        witness,
        sigterm,
        sigkill,
        final_group_signal,
        leader,
        elapsed,
    } = receipt;
    let elapsed_ms = elapsed.as_millis();
    let routine = settled_in.is_some()
        && sigterm.is_none()
        && sigkill.is_none()
        && matches!(leader, McpStdioLeaderOutcome::Reaped(_));
    if routine {
        tracing::debug!(
            server,
            ?settled_in,
            ?leader_exit,
            ?witness,
            ?final_group_signal,
            ?leader,
            elapsed_ms,
            "MCP stdio server close receipt"
        );
    } else {
        tracing::warn!(
            server,
            ?settled_in,
            ?leader_exit,
            ?witness,
            ?sigterm,
            ?sigkill,
            ?final_group_signal,
            ?leader,
            elapsed_ms,
            "MCP stdio server close escalated or ended unsettled"
        );
    }
}

struct StreamableConnectError {
    reason: String,
    /// Typed auth-failure record captured at the transport boundary: a parsed
    /// `www-authenticate` challenge and/or the `401`/`403` status. Both are
    /// `None` for non-auth failures.
    auth: crate::transport::streamable_http::AuthChallengeState,
    /// The attempt's governed control refused its first request; nothing was
    /// sent.
    refused: Option<meerkat_core::mcp_attachment::McpAttachmentRefusal>,
    /// The host egress profile refused or could not enforce one of the
    /// attempt's requests.
    egress: Option<HttpEgressError>,
    /// Its close index closed the attempt while it connected; the client it
    /// connected was closed at once.
    shut_down: bool,
}

impl StreamableConnectError {
    fn auth_challenge(&self) -> Option<String> {
        self.auth.challenge().map(String::from)
    }

    fn into_mcp_error(self, server: &str) -> McpError {
        if self.shut_down {
            return McpError::RouterShutDown;
        }
        match (self.refused, self.egress) {
            (Some(refusal), _) => McpError::AttachmentRefused(refusal),
            (None, Some(error)) => McpError::from_egress(server, error),
            (None, None) => McpError::ConnectionFailed {
                reason: self.reason,
            },
        }
    }
}

fn auth_failure_suggests_oauth(error: &StreamableConnectError) -> bool {
    !error.shut_down
        && error.refused.is_none()
        && error.egress.is_none()
        && (error.auth.challenge().is_some() || error.auth.status().is_some())
}

fn mcp_interactive_error(target: &McpServerIdentity, error: McpOAuthError) -> McpError {
    match error {
        McpOAuthError::HumanAuthorizationRequired { .. } => McpError::AuthorizationRequired {
            target: Box::new(target.clone()),
        },
        other => mcp_auth_error_to_connection_failed(other),
    }
}

fn mcp_auth_error_to_connection_failed(error: McpOAuthError) -> McpError {
    match error {
        // The credential owner's own traffic was refused by, or cannot
        // enforce, the host egress profile.
        McpOAuthError::EgressRefused {
            server_name,
            refusal,
        } => McpError::from_egress(&server_name, HttpEgressError::Refused(refusal)),
        McpOAuthError::EgressUnavailable {
            server_name,
            reason,
        } => McpError::from_egress(&server_name, HttpEgressError::Unavailable(reason)),
        McpOAuthError::InvalidAccountSelection
        | McpOAuthError::AccountSelectionRequired
        | McpOAuthError::UnsupportedAccountSelection
        | McpOAuthError::RealmMismatch { .. }
        | McpOAuthError::Verification(
            meerkat_auth_core::connector_oauth::ConnectorOAuthRefusal::AccountMismatch,
        ) => McpError::OAuthAccountRejected(error),
        other => McpError::ConnectionFailed {
            reason: other.to_string(),
        },
    }
}

/// How long one MCP connect attempt may take: the server's configured
/// connect timeout, plus the interactive login window when the attempt can
/// open a login.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ConnectDeadline {
    connect_timeout: Duration,
    login_window: Option<Duration>,
}

impl ConnectDeadline {
    pub(crate) fn for_connect(
        config: &McpServerConfig,
        auth_mode: McpAuthMode,
        has_auth_resolver: bool,
    ) -> Self {
        let connect_timeout = Duration::from_secs(u64::from(
            config
                .connect_timeout_secs
                .unwrap_or(McpConnection::DEFAULT_CONNECT_TIMEOUT_SECS),
        ));
        // Only a remote server with an auth resolver, under the interactive
        // mode, can open a login during its connect. A stdio server never
        // can, and a remote server without a resolver has none to open.
        let can_open_login = matches!(auth_mode, McpAuthMode::Interactive)
            && has_auth_resolver
            && matches!(config.transport, McpTransportConfig::Http(_));
        let login_window =
            can_open_login.then_some(meerkat_auth_core::MCP_INTERACTIVE_LOGIN_TIMEOUT);
        Self {
            connect_timeout,
            login_window,
        }
    }

    pub(crate) fn total(self) -> Duration {
        self.connect_timeout + self.login_window.unwrap_or_default()
    }

    /// The error of an attempt that ran out of this deadline. It names the
    /// deadline the attempt actually had.
    pub(crate) fn timed_out(self, server: &str) -> McpError {
        let total = self.total().as_secs();
        let reason = match self.login_window {
            None => format!("Timed out connecting to '{server}' after {total}s"),
            Some(window) => format!(
                "Timed out connecting to '{server}' after {total}s ({}s connect timeout and the {}s interactive login window)",
                self.connect_timeout.as_secs(),
                window.as_secs()
            ),
        };
        McpError::ConnectionFailed { reason }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
pub mod tests {
    use super::*;
    use crate::protocol::{extract_content_blocks, tool_error_reason};

    fn with_connect_timeout(mut config: McpServerConfig, secs: u32) -> McpServerConfig {
        config.connect_timeout_secs = Some(secs);
        config
    }

    /// A stdio server can never open a login, so an interactive-mode connect
    /// of one is bounded by its own connect timeout, not by the interactive
    /// login window: a server that never speaks MCP fails within it.
    #[tokio::test]
    async fn a_stdio_connect_under_interactive_auth_is_bounded_by_its_connect_timeout() {
        let config = with_connect_timeout(
            McpServerConfig::stdio("idle-stdio", "sleep", vec!["30".into()], HashMap::new()),
            1,
        );
        let started = std::time::Instant::now();
        let connected = tokio::time::timeout(
            Duration::from_secs(10),
            McpConnection::connect_and_enumerate_with_mcp_auth(
                &config,
                McpAuthMode::Interactive,
                None,
            ),
        )
        .await
        .expect("a stdio connect never waits for an interactive login window");
        let Err(error) = connected else {
            panic!("a stdio server that never speaks MCP never connects");
        };
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(error.to_string().contains("after 1s"), "{error}");
    }

    /// A remote server without an auth resolver cannot open a login either:
    /// its connect is bounded by its connect timeout.
    #[tokio::test]
    async fn a_remote_connect_without_an_auth_resolver_is_bounded_by_its_connect_timeout() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        // Accepts every connection and never answers.
        let silent = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                held.push(stream);
            }
        });
        let config = with_connect_timeout(
            McpServerConfig::streamable_http("silent-remote", url, HashMap::new()),
            1,
        );
        let connected = tokio::time::timeout(
            Duration::from_secs(10),
            McpConnection::connect_and_enumerate_with_mcp_auth(
                &config,
                McpAuthMode::Interactive,
                None,
            ),
        )
        .await
        .expect("a connect that cannot open a login never waits for its window");
        silent.abort();
        let Err(error) = connected else {
            panic!("a server that never answers never connects");
        };
        assert!(error.to_string().contains("after 1s"), "{error}");
    }

    /// Only a remote connect with an auth resolver, under the interactive
    /// mode, can open a login and so gets the interactive login window.
    #[test]
    fn only_a_remote_connect_with_an_auth_resolver_gets_the_interactive_login_window() {
        let stdio = with_connect_timeout(
            McpServerConfig::stdio("local", "/bin/true", vec![], HashMap::new()),
            1,
        );
        let remote = with_connect_timeout(
            McpServerConfig::streamable_http(
                "remote",
                "https://mcp.example/mcp".to_owned(),
                HashMap::new(),
            ),
            1,
        );
        let one = Duration::from_secs(1);
        let window = meerkat_auth_core::MCP_INTERACTIVE_LOGIN_TIMEOUT;
        for (config, mode, resolver, expected) in [
            (&stdio, McpAuthMode::Interactive, true, one),
            (&stdio, McpAuthMode::Interactive, false, one),
            (&remote, McpAuthMode::Interactive, false, one),
            (&remote, McpAuthMode::Stored, true, one),
            (&remote, McpAuthMode::Interactive, true, one + window),
        ] {
            assert_eq!(
                ConnectDeadline::for_connect(config, mode, resolver).total(),
                expected,
                "{} {mode:?} resolver={resolver}",
                config.name
            );
        }
    }

    /// A stdio server that never completed initialize is closed with a cut
    /// EOF grace when its connect fails: a connect bounded by its connect
    /// timeout ends just past it (here at about 1.25 s), not after the
    /// close's full EOF grace.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_stdio_server_that_never_initializes_is_closed_promptly_when_its_connect_fails() {
        let config = with_connect_timeout(
            McpServerConfig::stdio(
                "never-initializes",
                "sleep",
                vec!["30".into()],
                HashMap::new(),
            ),
            1,
        );
        let started = std::time::Instant::now();
        let connected = tokio::time::timeout(
            Duration::from_secs(20),
            McpConnection::connect_and_enumerate_with_custody(
                &config,
                McpAuthMode::Stored,
                None,
                None,
                Some(StdioChildCustody::with_bounds(McpStdioCloseBounds::DEFAULT)),
                &McpStdioLaunchProfile::trusted_host(),
                None,
            ),
        )
        .await
        .expect("the failed connect returns");
        assert!(
            connected.is_err(),
            "a server that never speaks MCP never connects"
        );
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(3),
            "the connect timeout is 1 s; the close took the EOF grace: {elapsed:?}"
        );
    }

    /// The control: a stdio server that completed initialize keeps its close
    /// grace even when its connect then fails (here its tools/list never
    /// answers): it is stopped only at the end of its EOF grace.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_initialized_stdio_server_keeps_its_close_grace_when_its_connect_fails() {
        // Answers initialize, never tools/list, and ignores stdin EOF.
        let script = r#"while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"sh-fixture","version":"0"}}}\n' "$id" ;;
  esac
done
sleep 30
"#;
        let config = with_connect_timeout(
            McpServerConfig::stdio(
                "initialized-then-silent",
                "/bin/sh",
                vec!["-c".to_owned(), script.to_owned()],
                HashMap::new(),
            ),
            1,
        );
        let eof_grace = Duration::from_millis(1500);
        let started = std::time::Instant::now();
        let connected = tokio::time::timeout(
            Duration::from_secs(20),
            McpConnection::connect_and_enumerate_with_custody(
                &config,
                McpAuthMode::Stored,
                None,
                None,
                Some(StdioChildCustody::with_bounds(
                    McpStdioCloseBounds::new(eof_grace, Duration::from_secs(5))
                        .expect("bounds within the maximum"),
                )),
                &McpStdioLaunchProfile::trusted_host(),
                None,
            ),
        )
        .await
        .expect("the failed connect returns");
        assert!(connected.is_err(), "tools/list never answers");
        let elapsed = started.elapsed();
        assert!(
            elapsed >= Duration::from_secs(1) + eof_grace,
            "an initialized server was closed before its EOF grace: {elapsed:?}"
        );
    }

    /// A connect that runs out of time names the deadline it actually had,
    /// login window included.
    #[test]
    fn a_connect_timeout_names_its_effective_deadline() {
        let stdio = with_connect_timeout(
            McpServerConfig::stdio("local", "/bin/true", vec![], HashMap::new()),
            1,
        );
        let remote = with_connect_timeout(
            McpServerConfig::streamable_http(
                "remote",
                "https://mcp.example/mcp".to_owned(),
                HashMap::new(),
            ),
            1,
        );
        let local = ConnectDeadline::for_connect(&stdio, McpAuthMode::Interactive, true)
            .timed_out("local")
            .to_string();
        assert!(local.contains("after 1s"), "{local}");
        let windowed = ConnectDeadline::for_connect(&remote, McpAuthMode::Interactive, true)
            .timed_out("remote")
            .to_string();
        assert!(windowed.contains("after 301s"), "{windowed}");
        assert!(
            windowed.contains("300s interactive login window"),
            "{windowed}"
        );
    }

    /// A connection states only the account its owners state: the host's
    /// `none` declaration, the connector binding's account, or the OAuth
    /// credential owner's local projection. A server with none of them (no
    /// connector binding included) states nothing.
    #[test]
    fn a_connection_states_only_the_account_its_owners_state() {
        use meerkat_core::{McpAccountDeclaration, OperationAccount};
        struct Projecting(OperationAccount);
        #[async_trait]
        impl McpAuthResolver for Projecting {
            async fn stored_bearer_token(
                &self,
                _target: &McpServerIdentity,
            ) -> Result<Option<String>, McpOAuthError> {
                Ok(None)
            }
            async fn interactive_login(
                &self,
                _target: &McpServerIdentity,
                _www_authenticate: Option<&str>,
            ) -> Result<String, McpOAuthError> {
                Err(McpOAuthError::AccountSelectionRequired)
            }
            fn selected_account(&self, _target: &McpServerIdentity) -> OperationAccount {
                self.0.clone()
            }
        }

        let plain =
            McpServerConfig::stdio("s", "/bin/true", vec![], std::collections::HashMap::new());
        assert_eq!(
            connection_account(&plain, None),
            OperationAccount::Unavailable
        );
        let mut declared = plain.clone();
        declared.account = Some(McpAccountDeclaration::NoAccount);
        assert_eq!(
            connection_account(&declared, None),
            OperationAccount::DeclaredNone
        );
        let mut bound = plain;
        bound.connector = Some(meerkat_core::mcp_config::McpConnectorBinding {
            realm_id: "r".into(),
            slot_id: "mail".into(),
            issuer: "https://issuer.example".into(),
            client: "client".into(),
            resource: "https://api.example".into(),
            strategy_id: "oidc-userinfo-v1".into(),
            account: "subject-7".into(),
            operations: std::collections::BTreeMap::new(),
        });
        assert_eq!(
            connection_account(&bound, None),
            OperationAccount::selected("https://issuer.example", "subject-7")
        );

        let http = McpServerConfig::streamable_http(
            "o",
            "https://mcp.example/api",
            std::collections::HashMap::new(),
        );
        let target = McpServerIdentity::from_server_config("o", "https://mcp.example/api");
        let selected = OperationAccount::selected("https://mcp.example", "subject-7");
        let projected = OAuthBearer::new(Arc::new(Projecting(selected.clone())), target.clone());
        assert_eq!(connection_account(&http, Some(&projected)), selected);
        let silent = OAuthBearer::new(Arc::new(Projecting(OperationAccount::Unavailable)), target);
        assert_eq!(
            connection_account(&http, Some(&silent)),
            OperationAccount::Unavailable
        );
    }

    #[test]
    fn a_server_origin_never_carries_path_query_or_userinfo() {
        assert_eq!(
            server_origin("https://user:pw@mcp.example:8443/api?token=x#f").as_deref(),
            Some("https://mcp.example:8443")
        );
        assert_eq!(
            server_origin("https://mcp.example/api").as_deref(),
            Some("https://mcp.example")
        );
        assert_eq!(server_origin("file:///tmp/socket"), None);
        assert_eq!(server_origin("not a url"), None);
    }

    /// A Streamable HTTP server whose background GET answer is configurable,
    /// and which counts `tools/call` POSTs.
    mod get_ordering {
        use super::*;
        use std::sync::Arc;

        pub(super) struct Server {
            pub(super) get_status: StatusCode,
            pub(super) tool_posts: AtomicUsize,
            pub(super) gets: tokio::sync::Notify,
        }

        async fn post_handler(
            State(server): State<Arc<Server>>,
            body: String,
        ) -> axum::response::Response {
            let message: Value = serde_json::from_str(&body).unwrap();
            let id = message.get("id").cloned();
            let reply = |result: Value| {
                (
                    StatusCode::OK,
                    [
                        ("content-type", "application/json"),
                        ("mcp-session-id", "s-1"),
                    ],
                    serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string(),
                )
                    .into_response()
            };
            match message["method"].as_str().unwrap_or_default() {
                "initialize" => reply(serde_json::json!({
                    "protocolVersion": message["params"]["protocolVersion"],
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "get-ordering", "version": "1"},
                })),
                _ if id.is_none() => StatusCode::ACCEPTED.into_response(),
                "tools/call" => {
                    server.tool_posts.fetch_add(1, Ordering::SeqCst);
                    reply(serde_json::json!({"content": [{"type": "text", "text": "done"}]}))
                }
                _ => reply(serde_json::json!({})),
            }
        }

        async fn get_handler(State(server): State<Arc<Server>>) -> StatusCode {
            server.gets.notify_one();
            server.get_status
        }

        pub(super) async fn start(get_status: StatusCode) -> (Arc<Server>, String) {
            let server = Arc::new(Server {
                get_status,
                tool_posts: AtomicUsize::new(0),
                gets: tokio::sync::Notify::new(),
            });
            let app = Router::new()
                .route(
                    "/mcp",
                    post(post_handler)
                        .get(get_handler)
                        .delete(|| async { StatusCode::OK }),
                )
                .with_state(Arc::clone(&server));
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/mcp", listener.local_addr().unwrap());
            tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            (server, url)
        }
    }

    /// A session the server invalidated before its first GET: the GET 404
    /// expires it, and a later tool call is refused unsent.
    #[tokio::test]
    async fn a_session_invalidated_before_its_first_get_refuses_calls_unsent() {
        let (server, url) = get_ordering::start(StatusCode::NOT_FOUND).await;
        let config = McpServerConfig::streamable_http("get-ordering", url, HashMap::new());
        let connection = McpConnection::connect(&config).await.unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            connection.session_expiry.recorded(),
        )
        .await
        .expect("the background GET 404 is recorded");
        let refused = connection
            .call_tool("effect", &serde_json::json!({}))
            .await
            .expect_err("an expired session refuses the call");
        assert!(
            matches!(refused, McpError::ServerUnavailable { .. }),
            "{refused:?}"
        );
        assert_eq!(server.tool_posts.load(Ordering::SeqCst), 0);
        let _ = connection.close().await;
    }

    /// Paired control: a healthy POST-only server answering the GET with 405
    /// (the spec's "no SSE stream") stays usable.
    #[tokio::test]
    async fn a_post_only_server_answering_get_with_405_stays_usable() {
        let (server, url) = get_ordering::start(StatusCode::METHOD_NOT_ALLOWED).await;
        let config = McpServerConfig::streamable_http("get-ordering", url, HashMap::new());
        let connection = McpConnection::connect(&config).await.unwrap();
        // Wait for the client's own classification of the GET, not for the
        // server's answer: only then is "no expiry" a settled fact.
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            connection.session_expiry.stream_unsupported_classified(),
        )
        .await
        .expect("the background GET 405 is classified");
        connection
            .call_tool("effect", &serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(server.tool_posts.load(Ordering::SeqCst), 1);
        assert!(!connection.session_expiry.expired());
        let _ = connection.close().await;
    }

    /// A call's failure is typed from its own disposition only: there is no
    /// connection-wide input, so a queued call that failed before sending is
    /// never labelled uncertain because another call expired the session.
    #[test]
    fn call_failures_are_typed_from_their_own_request_disposition() {
        let failure = |disposition| tool_call_failure("srv", "effect", disposition, &"cause");
        assert!(matches!(
            failure(Some(RequestDisposition::SentSessionExpired)),
            McpError::SessionExpired { ref server, ref tool } if server == "srv" && tool == "effect"
        ));
        assert!(matches!(
            failure(Some(RequestDisposition::RefusedExpired)),
            McpError::ServerUnavailable { .. }
        ));
        // No final disposition recorded (a pre-send failure such as the frame
        // bound, or an unfinished send): ordinary failure, never typed as
        // uncertain from connection state.
        assert!(matches!(failure(None), McpError::ToolCallFailed { .. }));
        assert!(matches!(
            failure(Some(RequestDisposition::Sent)),
            McpError::ToolCallFailed { .. }
        ));
        assert!(matches!(
            failure(Some(RequestDisposition::SentRedirected)),
            McpError::RedirectedOutcomeUncertain { ref tool, .. } if tool == "effect"
        ));
        // Refused by the host egress profile before anything was connected:
        // affirmatively unsent, typed, never uncertain.
        assert!(matches!(
            failure(Some(RequestDisposition::RefusedEgress(HttpEgressError::Refused(
                meerkat_auth_core::http_egress::HttpEgressRefusal::AddressNotAdmitted
            )))),
            McpError::EgressRefused { ref server, refusal: meerkat_auth_core::http_egress::HttpEgressRefusal::AddressNotAdmitted }
                if server == "srv"
        ));
    }
    use async_trait::async_trait;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::post;
    use axum::{Json, Router};
    use rmcp::model::Content;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tokio::net::TcpListener;

    #[test]
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn confined_stdio_spawn_preserves_exact_refusals_and_original_io() {
        use meerkat_core::confinement::ConfinementRefusal;

        for refusal in [
            ConfinementRefusal::InvalidRequirement,
            ConfinementRefusal::InvalidLaunch,
            ConfinementRefusal::UnsupportedRequirement,
            ConfinementRefusal::BackendUnavailable,
            ConfinementRefusal::PreparationFailed,
        ] {
            assert!(matches!(
                confined_stdio_spawn_error(std::io::Error::other(refusal)),
                McpError::Confinement(actual) if actual == refusal,
            ));
        }
        for error in [
            std::io::Error::from_raw_os_error(nix::libc::EPIPE),
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, "ordinary IO"),
            std::io::Error::other(ConfinementRefusal::BackendUnavailable.to_string()),
        ] {
            let kind = error.kind();
            let raw = error.raw_os_error();
            let message = error.to_string();
            let retained = match confined_stdio_spawn_error(error) {
                McpError::Io(error) => Some(error),
                _ => None,
            }
            .expect("ordinary IO must remain IO, including refusal text lookalikes");
            assert_eq!(retained.kind(), kind);
            assert_eq!(retained.raw_os_error(), raw);
            assert_eq!(retained.to_string(), message);
        }
    }

    /// Test that content block extraction works correctly with multiple text items
    #[test]
    fn test_extract_content_blocks_multiple_text() {
        let contents = vec![
            Content::text("Line 1"),
            Content::text("Line 2"),
            Content::text("Line 3"),
        ];

        let blocks = extract_content_blocks(contents);
        assert_eq!(blocks.len(), 3);
        assert_eq!(
            blocks[0],
            ContentBlock::Text {
                text: "Line 1".to_string()
            }
        );
        assert_eq!(
            blocks[1],
            ContentBlock::Text {
                text: "Line 2".to_string()
            }
        );
        assert_eq!(
            blocks[2],
            ContentBlock::Text {
                text: "Line 3".to_string()
            }
        );
    }

    /// Test that content block extraction works with single text item
    #[test]
    fn test_extract_content_blocks_single_text() {
        let contents = vec![Content::text("Only line")];

        let blocks = extract_content_blocks(contents);
        assert_eq!(blocks.len(), 1);
        assert_eq!(
            blocks[0],
            ContentBlock::Text {
                text: "Only line".to_string()
            }
        );
    }

    /// Test that content block extraction returns empty vec for empty input
    #[test]
    fn test_extract_content_blocks_empty() {
        let contents: Vec<Content> = Vec::new();
        let blocks = extract_content_blocks(contents);
        assert!(blocks.is_empty());
    }

    /// Test that image content from MCP is captured as ContentBlock::Image
    #[test]
    fn mcp_call_tool_captures_image() {
        let contents = vec![Content::image("aW1hZ2VkYXRh", "image/png")];

        let blocks = extract_content_blocks(contents);
        assert_eq!(blocks.len(), 1);
        assert_eq!(
            blocks[0],
            ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: "aW1hZ2VkYXRh".into(),
            }
        );
    }

    /// Test that mixed text and image content is preserved in order
    #[test]
    fn mcp_call_tool_mixed_text_and_image() {
        let contents = vec![
            Content::text("description of the image"),
            Content::image("cG5nZGF0YQ==", "image/png"),
            Content::text("additional context"),
        ];

        let blocks = extract_content_blocks(contents);
        assert_eq!(blocks.len(), 3);
        assert!(
            matches!(&blocks[0], ContentBlock::Text { text } if text == "description of the image")
        );
        assert!(matches!(
            &blocks[1],
            ContentBlock::Image { media_type, data, .. }
                if media_type == "image/png"
                    && matches!(data, meerkat_core::ImageData::Inline { data } if data == "cG5nZGF0YQ==")
        ));
        assert!(matches!(&blocks[2], ContentBlock::Text { text } if text == "additional context"));
    }

    /// Test that text-only responses still work as before
    #[test]
    fn mcp_call_tool_text_only_compat() {
        let contents = vec![Content::text("Hello, MCP!")];

        let blocks = extract_content_blocks(contents);
        assert_eq!(blocks.len(), 1);
        assert_eq!(
            blocks[0],
            ContentBlock::Text {
                text: "Hello, MCP!".to_string()
            }
        );

        // Verify text_content projection matches legacy behavior
        let text = meerkat_core::types::text_content(&blocks);
        assert_eq!(text, "Hello, MCP!");
    }

    /// Regression: an unmodeled content variant (embedded resource) must be
    /// preserved as `Structured` JSON, never silently dropped (no `_ => None`
    /// launder). The data the server returned survives the conversion.
    #[test]
    fn mcp_call_tool_preserves_unmodeled_variant() {
        let contents = vec![
            Content::text("before"),
            Content::embedded_text("file:///doc.txt", "resource body"),
        ];
        let blocks = extract_content_blocks(contents);
        assert_eq!(blocks.len(), 2, "no content variant may be dropped");
        assert!(matches!(&blocks[0], ContentBlock::Text { text } if text == "before"));
        match &blocks[1] {
            ContentBlock::Structured { data } => {
                let rendered = data.get();
                assert!(
                    rendered.contains("resource body"),
                    "structured passthrough must preserve the resource body verbatim, got: {rendered}"
                );
            }
            other => panic!("expected Structured passthrough, got {other:?}"),
        }
    }

    /// Regression: when a tool errors, the typed reason must carry the
    /// server-authored content detail, not a fixed `"Tool returned error"`
    /// string that launders the cause away.
    #[test]
    fn mcp_tool_error_reason_carries_server_detail() {
        let content = vec![Content::text("disk quota exceeded")];
        let reason = tool_error_reason(&extract_content_blocks(content));
        assert_eq!(reason, "disk quota exceeded");
    }

    /// An errored result with no content still produces a non-empty, honest
    /// reason rather than an empty string.
    #[test]
    fn mcp_tool_error_reason_handles_empty_content() {
        let reason = tool_error_reason(&[]);
        assert_eq!(reason, "tool returned error with no content");
    }

    /// RCT: Verify MCP initialize handshake works
    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn test_mcp_initialize_handshake() {
        let server_path = mcp_test_server::fixture_binary();

        let config = McpServerConfig::stdio(
            "test-server",
            server_path.to_string_lossy().to_string(),
            vec![],
            HashMap::new(),
        );

        // Connect (includes initialize handshake)
        let conn = McpConnection::connect(&config)
            .await
            .expect("Failed to connect to MCP server");

        // Verify we got server info back
        let info = conn.server_info();
        assert!(info.is_some(), "Server should return info after initialize");
        let info = info.unwrap();
        assert_eq!(info.server_info.name, "mcp-test-server");

        // Clean up
        conn.close().await.expect("Failed to close connection");
    }

    /// RCT: Verify tools/list schema parsing
    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn test_mcp_tools_list_schema_parse() {
        let server_path = mcp_test_server::fixture_binary();

        let config = McpServerConfig::stdio(
            "test-server",
            server_path.to_string_lossy().to_string(),
            vec![],
            HashMap::new(),
        );

        let conn = McpConnection::connect(&config)
            .await
            .expect("Failed to connect");

        // List tools
        let tools = conn
            .list_tools("test-server")
            .await
            .expect("Failed to list tools");

        // Verify we got the expected tools
        assert!(!tools.is_empty(), "Should have at least one tool");

        // Find the echo tool
        let echo_tool = tools.iter().find(|t| t.name == "echo");
        assert!(echo_tool.is_some(), "Should have echo tool");
        let echo_tool = echo_tool.unwrap();
        assert!(
            !echo_tool.description.is_empty(),
            "Echo tool should have description"
        );

        // Verify schema has expected structure
        let schema = &echo_tool.input_schema;
        assert_eq!(
            schema.get("type").and_then(|v| v.as_str()),
            Some("object"),
            "Schema should be object type"
        );
        assert!(
            schema.get("properties").is_some(),
            "Schema should have properties"
        );

        // Find the add tool and verify its schema
        let add_tool = tools.iter().find(|t| t.name == "add");
        assert!(add_tool.is_some(), "Should have add tool");
        let add_schema = &add_tool.unwrap().input_schema;
        let props = add_schema.get("properties").unwrap();
        assert!(
            props.get("a").is_some(),
            "Add tool should have 'a' property"
        );
        assert!(
            props.get("b").is_some(),
            "Add tool should have 'b' property"
        );

        conn.close().await.expect("Failed to close connection");
    }

    /// RCT: Verify tools/call round-trip (returns Vec<ContentBlock>)
    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn test_mcp_tools_call_round_trip() {
        let server_path = mcp_test_server::fixture_binary();

        let config = McpServerConfig::stdio(
            "test-server",
            server_path.to_string_lossy().to_string(),
            vec![],
            HashMap::new(),
        );

        let conn = McpConnection::connect(&config)
            .await
            .expect("Failed to connect");

        // Test echo tool -- returns Vec<ContentBlock>
        let blocks = conn
            .call_tool("echo", &serde_json::json!({"message": "Hello, MCP!"}))
            .await
            .expect("Echo call failed");
        assert_eq!(meerkat_core::types::text_content(&blocks), "Hello, MCP!");

        // Test add tool
        let blocks = conn
            .call_tool("add", &serde_json::json!({"a": 5, "b": 3}))
            .await
            .expect("Add call failed");
        assert_eq!(meerkat_core::types::text_content(&blocks), "8");

        // Test fail tool returns error
        let result = conn
            .call_tool("fail", &serde_json::json!({"message": "Expected error"}))
            .await;
        assert!(result.is_err(), "Fail tool should return error");

        conn.close().await.expect("Failed to close connection");
    }

    pub(crate) struct HttpMcpTestState {
        accepted_token: &'static str,
        pub(crate) seen_authorizations: Mutex<Vec<Option<String>>>,
    }

    pub(crate) async fn spawn_http_mcp_server(
        accepted_token: &'static str,
    ) -> (String, Arc<HttpMcpTestState>) {
        let state = Arc::new(HttpMcpTestState {
            accepted_token,
            seen_authorizations: Mutex::new(Vec::new()),
        });
        let app = Router::new()
            .route("/mcp", post(http_mcp_handler))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (url, state)
    }

    async fn http_mcp_handler(
        State(state): State<Arc<HttpMcpTestState>>,
        headers: HeaderMap,
        Json(request): Json<Value>,
    ) -> impl IntoResponse {
        let authorization = headers
            .get(http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        state
            .seen_authorizations
            .lock()
            .unwrap()
            .push(authorization.clone());

        if authorization.as_deref() != Some(&format!("Bearer {}", state.accepted_token)) {
            return (
                StatusCode::UNAUTHORIZED,
                [(
                    http::header::WWW_AUTHENTICATE,
                    "Bearer resource_metadata=\"/.well-known/oauth-protected-resource/mcp\"",
                )],
            )
                .into_response();
        }

        let Some(id) = request.get("id").cloned() else {
            return StatusCode::ACCEPTED.into_response();
        };
        let response = match request.get("method").and_then(|method| method.as_str()) {
            Some("initialize") => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "tools": {} },
                    "serverInfo": {
                        "name": "http-mcp-test-server",
                        "version": "0.1.0"
                    }
                }
            }),
            Some("tools/list") => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": [{
                        "name": "echo",
                        "description": "Echo input",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "message": { "type": "string" }
                            }
                        }
                    }]
                }
            }),
            method => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {
                    "code": -32601,
                    "message": format!("unsupported method {method:?}")
                }
            }),
        };
        (StatusCode::OK, Json(response)).into_response()
    }

    /// Models the credential owner: a completed interactive login commits
    /// its token, which every later per-request read returns.
    pub(crate) struct FakeMcpAuthResolver {
        stored_token: Mutex<Option<String>>,
        stored_reauth_required: AtomicBool,
        interactive_token: String,
        interactive_delay: Option<Duration>,
        interactive_calls: AtomicUsize,
        challenges: Mutex<Vec<Option<String>>>,
        human_authorization_required: bool,
    }

    impl FakeMcpAuthResolver {
        pub(crate) fn new(stored_token: Option<&str>, interactive_token: &str) -> Self {
            Self {
                stored_token: Mutex::new(stored_token.map(ToOwned::to_owned)),
                stored_reauth_required: AtomicBool::new(false),
                interactive_token: interactive_token.to_owned(),
                interactive_delay: None,
                interactive_calls: AtomicUsize::new(0),
                challenges: Mutex::new(Vec::new()),
                human_authorization_required: false,
            }
        }

        fn with_interactive_delay(mut self, delay: Duration) -> Self {
            self.interactive_delay = Some(delay);
            self
        }

        fn with_stored_reauth_required(self) -> Self {
            self.stored_reauth_required.store(true, Ordering::SeqCst);
            self
        }

        pub(crate) fn with_human_authorization_required(mut self) -> Self {
            self.human_authorization_required = true;
            self
        }

        /// The stored credential slot, for a test that "logs in".
        pub(crate) fn stored_token_for_testing(&self) -> std::sync::MutexGuard<'_, Option<String>> {
            self.stored_token.lock().unwrap()
        }
    }

    #[async_trait]
    impl McpAuthResolver for FakeMcpAuthResolver {
        async fn stored_bearer_token(
            &self,
            target: &McpServerIdentity,
        ) -> Result<Option<String>, McpOAuthError> {
            if self.stored_reauth_required.load(Ordering::SeqCst) {
                return Err(McpOAuthError::ReauthRequired {
                    server_name: target.server_name().to_string(),
                });
            }
            Ok(self.stored_token.lock().unwrap().clone())
        }

        async fn interactive_login(
            &self,
            _target: &McpServerIdentity,
            www_authenticate: Option<&str>,
        ) -> Result<String, McpOAuthError> {
            if let Some(delay) = self.interactive_delay {
                tokio::time::sleep(delay).await;
            }
            self.interactive_calls.fetch_add(1, Ordering::SeqCst);
            self.challenges
                .lock()
                .unwrap()
                .push(www_authenticate.map(ToOwned::to_owned));
            if self.human_authorization_required {
                return Err(McpOAuthError::HumanAuthorizationRequired {
                    server_name: _target.server_name().to_owned(),
                });
            }
            *self.stored_token.lock().unwrap() = Some(self.interactive_token.clone());
            self.stored_reauth_required.store(false, Ordering::SeqCst);
            Ok(self.interactive_token.clone())
        }
    }

    #[tokio::test]
    async fn mcp_oauth_human_authorization_is_typed_with_target_and_carries_no_secret() {
        let (url, state) = spawn_http_mcp_server("interactive-token").await;
        let config = McpServerConfig::streamable_http("glean", url, HashMap::new());
        let resolver =
            Arc::new(FakeMcpAuthResolver::new(None, "unused").with_human_authorization_required());

        let error = match McpConnection::connect_and_enumerate_with_mcp_auth(
            &config,
            McpAuthMode::Interactive,
            Some(resolver.clone()),
        )
        .await
        {
            Ok(_) => panic!("unauthorized server must not connect"),
            Err(error) => error,
        };
        let McpError::AuthorizationRequired { target } = &error else {
            panic!("expected typed authorization-required, got {error:?}");
        };
        assert_eq!(**target, McpServerIdentity::from_config(&config).unwrap());
        let rendered = format!("{error} {error:?}");
        for secret in ["authorize", "state=", "code=", "resource_metadata"] {
            assert!(!rendered.contains(secret), "{secret:?} leaked: {rendered}");
        }
        assert_eq!(resolver.interactive_calls.load(Ordering::SeqCst), 1);
        assert!(
            state
                .seen_authorizations
                .lock()
                .unwrap()
                .iter()
                .all(Option::is_none),
            "no credential may be sent without completed host authorization"
        );
    }

    #[tokio::test]
    async fn mcp_oauth_stored_token_is_injected_for_streamable_http() {
        let (url, state) = spawn_http_mcp_server("stored-token").await;
        let config = McpServerConfig::streamable_http("glean", url, HashMap::new());
        let resolver = Arc::new(FakeMcpAuthResolver::new(Some("stored-token"), "unused"));

        let (conn, tools) = McpConnection::connect_and_enumerate_with_mcp_auth(
            &config,
            McpAuthMode::Stored,
            Some(resolver),
        )
        .await
        .expect("stored token should connect");

        assert!(tools.iter().any(|tool| tool.name == "echo"));
        assert!(
            state
                .seen_authorizations
                .lock()
                .unwrap()
                .iter()
                .any(|header| header.as_deref() == Some("Bearer stored-token")),
            "streamable HTTP requests should include the stored bearer token"
        );
        conn.close().await.expect("Failed to close connection");
    }

    #[tokio::test]
    async fn mcp_oauth_interactive_login_retries_streamable_http_connect() {
        let (url, state) = spawn_http_mcp_server("interactive-token").await;
        let config = McpServerConfig::streamable_http("glean", url, HashMap::new());
        let resolver = Arc::new(FakeMcpAuthResolver::new(None, "interactive-token"));

        let (conn, tools) = McpConnection::connect_and_enumerate_with_mcp_auth(
            &config,
            McpAuthMode::Interactive,
            Some(resolver.clone()),
        )
        .await
        .expect("interactive login should retry and connect");

        assert!(tools.iter().any(|tool| tool.name == "echo"));
        assert_eq!(resolver.interactive_calls.load(Ordering::SeqCst), 1);
        assert!(
            resolver
                .challenges
                .lock()
                .unwrap()
                .iter()
                .any(|challenge| challenge
                    .as_deref()
                    .is_some_and(|value| value.contains("resource_metadata"))),
            "interactive login should receive the WWW-Authenticate challenge"
        );
        let seen = state.seen_authorizations.lock().unwrap().clone();
        assert!(
            seen.iter().any(Option::is_none),
            "first connect attempt should be unauthenticated"
        );
        assert!(
            seen.iter()
                .any(|header| header.as_deref() == Some("Bearer interactive-token")),
            "retry should include the interactive bearer token"
        );
        conn.close().await.expect("Failed to close connection");
    }

    #[tokio::test]
    async fn mcp_oauth_interactive_login_gets_browser_timeout_budget() {
        let (url, _state) = spawn_http_mcp_server("interactive-token").await;
        let mut config = McpServerConfig::streamable_http("glean", url, HashMap::new());
        config.connect_timeout_secs = Some(1);
        let resolver = Arc::new(
            FakeMcpAuthResolver::new(None, "interactive-token")
                .with_interactive_delay(Duration::from_millis(1200)),
        );

        let (conn, tools) = McpConnection::connect_and_enumerate_with_mcp_auth(
            &config,
            McpAuthMode::Interactive,
            Some(resolver),
        )
        .await
        .expect("interactive login should not be cut off by the normal connect timeout");

        assert!(tools.iter().any(|tool| tool.name == "echo"));
        conn.close().await.expect("Failed to close connection");
    }

    #[tokio::test]
    async fn mcp_oauth_interactive_mode_recovers_from_stored_reauth_required() {
        let (url, state) = spawn_http_mcp_server("interactive-token").await;
        let config = McpServerConfig::streamable_http("glean", url, HashMap::new());
        let resolver = Arc::new(
            FakeMcpAuthResolver::new(None, "interactive-token").with_stored_reauth_required(),
        );

        let (conn, tools) = McpConnection::connect_and_enumerate_with_mcp_auth(
            &config,
            McpAuthMode::Interactive,
            Some(resolver.clone()),
        )
        .await
        .expect("interactive mode should reauth when stored credentials require it");

        assert!(tools.iter().any(|tool| tool.name == "echo"));
        assert_eq!(resolver.interactive_calls.load(Ordering::SeqCst), 1);
        assert!(
            state
                .seen_authorizations
                .lock()
                .unwrap()
                .iter()
                .any(|header| header.as_deref() == Some("Bearer interactive-token")),
            "reauth retry should connect with the interactive token"
        );
        conn.close().await.expect("Failed to close connection");
    }

    #[tokio::test]
    async fn mcp_oauth_interactive_mode_recovers_from_server_rejected_stored_token() {
        let (url, state) = spawn_http_mcp_server("interactive-token").await;
        let config = McpServerConfig::streamable_http("glean", url, HashMap::new());
        let resolver = Arc::new(FakeMcpAuthResolver::new(
            Some("stale-stored-token"),
            "interactive-token",
        ));

        let (conn, tools) = McpConnection::connect_and_enumerate_with_mcp_auth(
            &config,
            McpAuthMode::Interactive,
            Some(resolver.clone()),
        )
        .await
        .expect("interactive mode should reauth when the server rejects a stored token");

        assert!(tools.iter().any(|tool| tool.name == "echo"));
        assert_eq!(resolver.interactive_calls.load(Ordering::SeqCst), 1);
        assert!(
            resolver
                .challenges
                .lock()
                .unwrap()
                .iter()
                .any(|challenge| challenge
                    .as_deref()
                    .is_some_and(|value| value.contains("resource_metadata"))),
            "server-rejected stored tokens should pass the auth challenge into reauth"
        );
        let seen = state.seen_authorizations.lock().unwrap().clone();
        assert!(
            seen.iter()
                .any(|header| header.as_deref() == Some("Bearer stale-stored-token")),
            "first connect attempt should use the stored token"
        );
        assert!(
            seen.iter()
                .any(|header| header.as_deref() == Some("Bearer interactive-token")),
            "reauth retry should use the interactive token"
        );
        conn.close().await.expect("Failed to close connection");
    }

    /// A real stdio child in custody whose close has no grace, so it
    /// escalates straight to its one direct kill.
    #[cfg(unix)]
    async fn custody_test_child() -> (
        StdioChildCustody,
        (tokio::process::ChildStdout, tokio::process::ChildStdin),
    ) {
        let custody = StdioChildCustody::with_bounds(McpStdioCloseBounds::IMMEDIATE);
        let pipes = custody
            .spawn(
                "sleeper",
                &meerkat_core::mcp_config::McpStdioConfig {
                    command: "/bin/sh".to_string(),
                    args: vec!["-c".to_string(), "exec sleep 60".to_string()],
                    env: HashMap::new(),
                    secret_env: std::collections::BTreeSet::new(),
                    cwd: None,
                },
                &McpStdioLaunchProfile::trusted_host(),
                &mut PhysicalEntry::default(),
            )
            .await
            .expect("spawn real stdio child");
        (custody, pipes)
    }

    #[cfg(unix)]
    pub(crate) fn assert_child_reaped(pid: u32) {
        assert_eq!(
            nix::sys::wait::waitpid(
                nix::unistd::Pid::from_raw(i32::try_from(pid).expect("valid child pid")),
                Some(nix::sys::wait::WaitPidFlag::WNOHANG),
            ),
            Err(nix::errno::Errno::ECHILD),
            "custody must already have reaped its direct child"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stdio_custody_cancelled_termination_can_be_reaped_by_the_retained_owner() {
        let (custody, _pipes) = custody_test_child().await;
        let pid = custody.spawned_pid().await;
        let (waiting, resume) = custody.pause_next_reap();
        let first = tokio::spawn({
            let custody = custody.clone();
            async move { custody.terminate().await }
        });
        waiting.await.expect("termination reached its reap await");
        assert_eq!(
            custody
                .direct_kill_requests
                .load(std::sync::atomic::Ordering::Relaxed),
            1,
            "the first waiter requests termination before reaping"
        );
        first.abort();
        assert!(
            first
                .await
                .expect_err("first waiter was aborted")
                .is_cancelled()
        );
        drop(resume);

        let receipt = custody
            .terminate_or_join()
            .await
            .expect("cancellation leaves the child in retained custody, and its receipt to a join");
        assert!(
            matches!(receipt.leader, McpStdioLeaderOutcome::Reaped(_)),
            "retained owner reaps the child: {receipt:?}"
        );
        assert_child_reaped(pid);
        assert_eq!(
            custody
                .direct_kill_requests
                .load(std::sync::atomic::Ordering::Relaxed),
            1,
            "reaping retries must not signal the process id again"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stdio_custody_concurrent_termination_waits_for_the_reap_owner() {
        let (custody, _pipes) = custody_test_child().await;
        let pid = custody.spawned_pid().await;
        let (waiting, resume) = custody.pause_next_reap();
        let first = tokio::spawn({
            let custody = custody.clone();
            async move { custody.terminate().await }
        });
        waiting.await.expect("termination reached its reap await");
        let mut second = Box::pin(custody.terminate_or_join());
        let second_waited = futures::poll!(second.as_mut()).is_pending();

        resume.send(()).expect("release the first reap");
        let receipt = first
            .await
            .expect("first waiter joins")
            .expect("first waiter owns the child");
        assert!(
            matches!(receipt.leader, McpStdioLeaderOutcome::Reaped(_)),
            "first waiter reaps the child: {receipt:?}"
        );
        if second_waited {
            let joined = second.await.expect("the joined close was not abandoned");
            assert!(
                Arc::ptr_eq(&joined, &receipt),
                "a join reads the first waiter's receipt, not nothing"
            );
        }
        assert_child_reaped(pid);
        assert!(
            second_waited,
            "a concurrent close must wait for the owned reap"
        );
    }

    #[cfg(feature = "workspace-tests")]
    #[cfg(unix)]
    #[tokio::test]
    async fn stdio_custody_close_reports_a_reap_failure() {
        use rmcp::ServiceExt as _;

        let (client_io, server_io) = tokio::io::duplex(8192);
        let (client, server) = tokio::join!(
            ClientServiceSelection::Default.serve(client_io),
            mcp_test_server::FormTestServer::default().serve(server_io),
        );
        let client = client.expect("initialize client");
        let server = server.expect("initialize server");
        let (custody, _pipes) = custody_test_child().await;
        let pid = custody.spawned_pid().await;
        let (waiting, resume) = custody.pause_next_reap();
        let close = tokio::spawn(close_connected(
            "reap-failure",
            HeldClient::standalone(client),
            Some(custody),
        ));
        waiting.await.expect("close reached its reap await");

        // Fault injection: another process owner consumes the status before
        // Child::wait can reap it. The close must preserve that OS error.
        tokio::task::spawn_blocking(move || {
            nix::sys::wait::waitpid(
                nix::unistd::Pid::from_raw(i32::try_from(pid).expect("valid child pid")),
                None,
            )
        })
        .await
        .expect("external waiter joins")
        .expect("external waiter consumes the exit status");
        resume.send(()).expect("release the failed reap");
        let result = close.await.expect("close task joins");
        server.cancel().await.expect("server task joins");
        assert!(
            matches!(result, Err(McpError::Io(ref error)) if error.raw_os_error() == Some(nix::errno::Errno::ECHILD as i32)),
            "close must return the typed reap error, got {result:?}"
        );
    }

    /// C9: a client close whose service does not end within its bound is
    /// `TimedOut`, its connection close's typed `CloseTimedOut` failure,
    /// never settled.
    #[tokio::test]
    async fn a_client_close_past_its_bound_is_a_typed_close_failure() {
        let bound = Duration::from_millis(50);
        let closed = await_client_close(
            tokio::time::Instant::now(),
            bound,
            std::future::pending::<Result<(), tokio::task::JoinError>>(),
        )
        .await;
        assert!(
            matches!(closed, ClientClose::TimedOut(waited) if waited == bound),
            "{closed:?}"
        );
        let outcome = close_outcome(
            "remote",
            ObservedClose {
                process: ObservedProcessClose::NoCustody,
                cancel_failure: client_close_failure("remote", &ClientEnd::Closed(closed)),
            },
        );
        assert!(
            matches!(
                &outcome,
                McpServerCloseOutcome::CloseFailed(error)
                    if matches!(
                        **error,
                        McpError::CloseTimedOut { ref server, bound: waited }
                            if server == "remote" && waited == bound
                    )
            ),
            "{outcome:?}"
        );
        assert!(!outcome.is_settled());
    }

    /// A connector-bound stdio server starts with its expected account
    /// binding in `MEERKAT_CONNECTOR_BINDING`, over any value the configured
    /// environment sets; an unbound server's environment is unchanged. A
    /// binding that names an operation the server does not list (a bare
    /// name for a prefixed tool) is refused at connect.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn a_connector_bound_stdio_server_starts_with_its_expected_binding() {
        use crate::stdio_test_fixture::{PidReport, sh_mcp_server_args_listing};
        use meerkat_core::mcp_config::{
            MCP_CONNECTOR_BINDING_ENV, McpConnectorBinding, McpConnectorOperation,
        };
        let binding = |operation: &str| McpConnectorBinding {
            realm_id: "tenant-a".into(),
            slot_id: "mail-work".into(),
            issuer: "https://issuer.example".into(),
            client: "client".into(),
            resource: "https://mail.example".into(),
            strategy_id: "oidc-userinfo-v1".into(),
            account: "subject-a".into(),
            operations: std::collections::BTreeMap::from([(
                operation.to_owned(),
                McpConnectorOperation {
                    required_scopes: Default::default(),
                    destination_origin: "https://mail.example".into(),
                    read_only: false,
                },
            )]),
        };
        for bound in [Some("gmail_read_text_part"), None, Some("read_text_part")] {
            let mut report = PidReport::new("connector-binding");
            let mut config = McpServerConfig::stdio(
                "bound",
                "/bin/sh",
                sh_mcp_server_args_listing(
                    &format!(
                        "printf '%s\\n' \"${MCP_CONNECTOR_BINDING_ENV}\" > '{}'\n",
                        report.path().display()
                    ),
                    &["gmail_read_text_part"],
                ),
                HashMap::from([(
                    MCP_CONNECTOR_BINDING_ENV.to_owned(),
                    "configured".to_owned(),
                )]),
            );
            config.connector = bound.map(binding);
            let connected = McpConnection::connect_and_enumerate(&config).await;

            let seen = report.line().await;

            let expected = match bound {
                Some(operation) => binding(operation).startup_projection().to_string(),
                None => "configured".to_owned(),
            };
            assert_eq!(seen, expected);
            match (bound, connected) {
                (Some("read_text_part"), Err(McpError::ConnectionFailed { reason })) => {
                    assert!(reason.contains("does not list"), "{reason}");
                }
                (_, Ok((conn, tools))) if bound != Some("read_text_part") => {
                    assert_eq!(tools.len(), 1);
                    conn.close().await.expect("close");
                }
                (bound, other) => panic!("{bound:?}: unexpected {:?}", other.err()),
            }
        }
    }

    /// `close` owns the stdio server's exit: with no grace the server is
    /// signalled with its whole process group right after its stdin closes,
    /// even when it keeps running past EOF, and has exited when `close`
    /// returns. (Fails-old: rmcp gave the server up to 3 s after EOF, then
    /// killed only the direct child, so a wrapped server's grandchild outlived
    /// `close`.)
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn close_returns_after_the_stdio_servers_process_group_exits() {
        use crate::stdio_test_fixture::{PidReport, process_exited, sh_mcp_server_args};
        let mut report = PidReport::new("close-group");
        let config = McpServerConfig::stdio(
            "eof-ignoring",
            "/bin/sh",
            sh_mcp_server_args(Some(report.path())),
            HashMap::new(),
        );
        let conn = connect_stdio_with_bounds(&config, McpStdioCloseBounds::IMMEDIATE).await;
        let pids = report.pids().await;
        let (wrapper, server) = (pids[0], pids[1]);

        let receipt = close_owned(conn).await;

        assert_eq!(receipt.sigterm, Some(McpStdioSignalOutcome::Sent));
        assert!(
            matches!(receipt.leader, McpStdioLeaderOutcome::Reaped(_)),
            "{receipt:?}"
        );
        assert_child_reaped(wrapper);
        assert!(
            process_exited(server),
            "stdio server's grandchild {server} outlived close"
        );
    }

    /// A failed handshake terminates the spawned server before the error is
    /// returned. (Fails-old: the transport drop only scheduled the kill.)
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn failed_handshake_returns_after_the_stdio_server_exits() {
        // Closes its stdout (the handshake fails on EOF) but keeps running,
        // and ignores SIGTERM, so a close still outlasts the connect's
        // timeout below.
        let config = McpServerConfig::stdio(
            "closes-stdout",
            "/bin/sh",
            vec![
                "-c".to_string(),
                "trap '' TERM; exec >&-; exec sleep 60".to_string(),
            ],
            HashMap::new(),
        );
        let custody = StdioChildCustody::with_bounds(graces(200, 200));

        let result = McpConnection::connect_and_enumerate_with_custody(
            &config,
            McpAuthMode::Stored,
            None,
            None,
            Some(custody.clone()),
            &McpStdioLaunchProfile::trusted_host(),
            None,
        )
        .await;

        assert!(result.is_err(), "a server without stdout cannot connect");
        let pid = custody.spawned_pid().await;
        assert_child_reaped(pid);
        assert_eq!(
            custody.terminate().await.err(),
            Some(CustodyNotHeld::ClosedElsewhere),
            "the failed attempt already closed and reaped the server"
        );

        // C6/T7: a bounded connect whose timeout fires while its failed
        // handshake is still closing the server joins that close, and
        // reports its final native facts: the server closed its stdout and
        // ignored EOF, so it was terminated and reaped. The close began
        // before the timeout fired, so it is the handshake's own close.
        let timeout = Duration::from_secs(2);
        let called = tokio::time::Instant::now();
        let Err(failure) = McpConnection::connect_within(&config, timeout).await else {
            panic!("a server without stdout cannot connect");
        };
        let returned = called.elapsed();
        assert!(failure.timed_out, "{failure:?}");
        let Some(McpServerCloseOutcome::Stdio(receipt)) = &failure.close else {
            panic!("the joined close's receipt: {failure:?}");
        };
        assert!(
            matches!(receipt.leader, McpStdioLeaderOutcome::Reaped(_)),
            "{receipt:?}"
        );
        assert!(
            matches!(receipt.witness, McpStdioWitnessOutcome::Closed(_)),
            "{receipt:?}"
        );
        assert_eq!(receipt.sigterm, Some(McpStdioSignalOutcome::Sent));
        assert!(
            returned.saturating_sub(receipt.elapsed) < timeout,
            "the close began before the connect's timeout: returned after {returned:?}, \
             close took {:?}",
            receipt.elapsed
        );
    }

    /// A direct connection refuses an invalid access ceiling before it
    /// spawns anything.
    #[cfg(unix)]
    #[tokio::test]
    async fn direct_connect_refuses_an_empty_access_ceiling_before_spawning() {
        let temp = tempfile::tempdir().expect("tempdir");
        let marker = temp.path().join("spawned");
        let mut config = McpServerConfig::stdio(
            "empty-ceiling",
            "/bin/sh",
            vec!["-c".into(), format!("touch '{}'", marker.display())],
            HashMap::new(),
        );
        config.access = Some(meerkat_core::McpServerAccess::allowing(Vec::<String>::new()));
        let refused = McpConnection::connect(&config)
            .await
            .err()
            .expect("an empty ceiling is refused");
        assert!(
            matches!(refused, McpError::InvalidAccessPolicy(_)),
            "{refused:?}"
        );
        assert!(!marker.exists(), "the server was never spawned");
    }

    /// Connect a stdio server whose close uses `bounds`.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    async fn connect_stdio_with_bounds(
        config: &McpServerConfig,
        bounds: McpStdioCloseBounds,
    ) -> McpConnection {
        let (conn, tools) = McpConnection::connect_and_enumerate_with_custody(
            config,
            McpAuthMode::Stored,
            None,
            None,
            Some(StdioChildCustody::with_bounds(bounds)),
            &McpStdioLaunchProfile::trusted_host(),
            None,
        )
        .await
        .expect("fixture server completes the handshake");
        assert!(tools.is_empty());
        conn
    }

    /// Close `conn`, which owns its stdio server's close.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    async fn close_owned(conn: McpConnection) -> McpStdioCloseReceipt {
        conn.close_with_receipt()
            .await
            .expect("close")
            .expect("this close owned the stdio server")
    }

    #[cfg(unix)]
    fn graces(eof_ms: u64, term_ms: u64) -> McpStdioCloseBounds {
        McpStdioCloseBounds::new(
            Duration::from_millis(eof_ms),
            Duration::from_millis(term_ms),
        )
        .expect("bounds within the maximum")
    }

    #[cfg(unix)]
    fn marker_path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "meerkat-mcp-{tag}-{}-{}.marker",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default()
        ))
    }

    #[cfg(unix)]
    fn take_marker(marker: &std::path::Path) -> String {
        let content = std::fs::read_to_string(marker).unwrap_or_default();
        let _ = std::fs::remove_file(marker);
        content.trim().to_owned()
    }

    /// A server that joins work it starts on stdin EOF finishes that work
    /// within the EOF grace and is never escalated against. The final group
    /// signal finds the group because the leader is still unreaped when it is
    /// sent: with no other member left, a signal sent after the reap would
    /// find no group. (Fails-old: close SIGKILLed the group right after EOF,
    /// so the work never finished.)
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn close_lets_a_server_join_its_work_after_stdin_eof() {
        use crate::stdio_test_fixture::{process_group_gone, sh_mcp_server_script};
        let marker = marker_path("eof-join");
        let config = McpServerConfig::stdio(
            "eof-join",
            "/bin/sh",
            sh_mcp_server_script(
                "",
                &format!(
                    "(sleep 0.5; echo joined > '{}') & wait $!\nexit 0\n",
                    marker.display()
                ),
            ),
            HashMap::new(),
        );
        let conn = connect_stdio_with_bounds(&config, graces(20_000, 20_000)).await;
        let leader = conn
            .stdio_child
            .clone()
            .expect("stdio custody")
            .spawned_pid()
            .await;

        let receipt = close_owned(conn).await;

        assert_eq!(
            receipt.settled_in,
            Some(McpStdioClosePhase::Eof),
            "{receipt:?}"
        );
        assert_eq!(
            receipt.leader_exit,
            McpStdioLeaderExitObservation::Exited(McpStdioClosePhase::Eof)
        );
        assert_eq!(
            receipt.witness,
            McpStdioWitnessOutcome::Closed(McpStdioClosePhase::Eof)
        );
        assert_eq!((receipt.sigterm, receipt.sigkill), (None, None));
        #[cfg(not(target_os = "macos"))]
        assert_eq!(
            receipt.final_group_signal,
            Some(McpStdioSignalOutcome::Sent),
            "the final group signal must precede the leader's reap"
        );
        // Darwin refuses a group signal with EPERM when the group's only
        // member is the leader that has exited and is not reaped yet (a
        // zombie cannot be signalled). EPERM rather than ESRCH still shows
        // the group, and so its unreaped leader, existed when it was sent.
        #[cfg(target_os = "macos")]
        {
            let zombie_group = McpStdioSignalOutcome::Failed(nix::libc::EPERM);
            assert!(
                receipt.final_group_signal == Some(McpStdioSignalOutcome::Sent)
                    || receipt.final_group_signal == Some(zombie_group),
                "the final group signal must precede the leader's reap: {receipt:?}"
            );
        }
        assert!(
            matches!(&receipt.leader, McpStdioLeaderOutcome::Reaped(status) if status.success()),
            "{receipt:?}"
        );
        assert_eq!(
            take_marker(&marker),
            "joined",
            "the server's work was cut short"
        );
        assert!(
            process_group_gone(leader),
            "the server's group outlived close"
        );
    }

    /// A server that ignores EOF but handles SIGTERM gets the SIGTERM at the
    /// EOF deadline and exits through its handler within the TERM grace.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn close_signals_sigterm_at_the_eof_deadline() {
        use crate::stdio_test_fixture::sh_mcp_server_script;
        let marker = marker_path("term-join");
        let config = McpServerConfig::stdio(
            "term-join",
            "/bin/sh",
            sh_mcp_server_script(
                "",
                &format!(
                    "trap \"echo joined > '{}'; exit 0\" TERM\nwhile :; do sleep 0.1; done\n",
                    marker.display()
                ),
            ),
            HashMap::new(),
        );
        let conn = connect_stdio_with_bounds(&config, graces(500, 20_000)).await;

        let receipt = close_owned(conn).await;

        assert_eq!(
            receipt.settled_in,
            Some(McpStdioClosePhase::Term),
            "{receipt:?}"
        );
        assert_eq!(receipt.sigterm, Some(McpStdioSignalOutcome::Sent));
        assert_eq!(receipt.sigkill, None);
        assert!(
            receipt.elapsed >= Duration::from_millis(500),
            "SIGTERM came before the EOF deadline: {:?}",
            receipt.elapsed
        );
        assert_eq!(
            take_marker(&marker),
            "joined",
            "the TERM handler did not run"
        );
    }

    /// A server that ignores both EOF and SIGTERM is killed at the TERM
    /// deadline, settles in the kill phase, and has exited, with its group,
    /// when close returns.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn close_kills_a_server_that_ignores_eof_and_sigterm() {
        use crate::stdio_test_fixture::{process_exited, process_group_gone, sh_mcp_server_script};
        let config = McpServerConfig::stdio(
            "unresponsive",
            "/bin/sh",
            // An ignored disposition survives exec, so sleep ignores TERM too.
            sh_mcp_server_script("", "trap '' TERM\nexec sleep 600\n"),
            HashMap::new(),
        );
        let conn = connect_stdio_with_bounds(&config, graces(300, 300)).await;
        let leader = conn
            .stdio_child
            .clone()
            .expect("stdio custody")
            .spawned_pid()
            .await;

        let receipt = close_owned(conn).await;

        assert_eq!(
            receipt.settled_in,
            Some(McpStdioClosePhase::Kill),
            "{receipt:?}"
        );
        assert_eq!(receipt.sigterm, Some(McpStdioSignalOutcome::Sent));
        assert_eq!(receipt.sigkill, Some(McpStdioSignalOutcome::Sent));
        assert!(
            receipt.elapsed >= Duration::from_millis(600),
            "killed before the TERM deadline: {:?}",
            receipt.elapsed
        );
        assert!(process_exited(leader), "server {leader} outlived close");
        assert!(
            process_group_gone(leader),
            "the server's group outlived close"
        );
    }

    /// A server that exits on EOF but leaves a process in its group that
    /// does not hold its stdout settles on EOF, and the final group signal
    /// ends what it left.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn close_ends_what_a_server_leaves_in_its_group() {
        use crate::stdio_test_fixture::{PidReport, exits_within, sh_mcp_server_script};
        let mut report = PidReport::new("straggler");
        let config = McpServerConfig::stdio(
            "straggler",
            "/bin/sh",
            sh_mcp_server_script(
                "",
                &format!(
                    "sleep 600 >/dev/null 2>&1 &\necho $! > '{}'\nexit 0\n",
                    report.path().display()
                ),
            ),
            HashMap::new(),
        );
        let conn = connect_stdio_with_bounds(&config, graces(20_000, 20_000)).await;

        let receipt = close_owned(conn).await;
        let straggler = report.pids().await[0];

        assert_eq!(
            receipt.settled_in,
            Some(McpStdioClosePhase::Eof),
            "{receipt:?}"
        );
        assert_eq!(
            receipt.final_group_signal,
            Some(McpStdioSignalOutcome::Sent)
        );
        assert!(
            exits_within(straggler, Duration::from_secs(5)).await,
            "process {straggler} the server left in its group outlived close"
        );
    }

    /// A server that exits on EOF while a process it left still holds its
    /// stdout has not settled: the open witness keeps it out of the EOF
    /// phase, and the SIGTERM ends the holder.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn an_orphan_holding_stdout_keeps_the_close_out_of_the_eof_phase() {
        use crate::stdio_test_fixture::sh_mcp_server_script;
        let config = McpServerConfig::stdio(
            "stdout-orphan",
            "/bin/sh",
            sh_mcp_server_script("", "sleep 600 &\nexit 0\n"),
            HashMap::new(),
        );
        let conn = connect_stdio_with_bounds(&config, graces(300, 20_000)).await;

        let receipt = close_owned(conn).await;

        assert_eq!(
            receipt.leader_exit,
            McpStdioLeaderExitObservation::Exited(McpStdioClosePhase::Eof)
        );
        assert_eq!(
            receipt.witness,
            McpStdioWitnessOutcome::Closed(McpStdioClosePhase::Term),
            "{receipt:?}"
        );
        assert_eq!(receipt.settled_in, Some(McpStdioClosePhase::Term));
        assert_eq!(receipt.sigterm, Some(McpStdioSignalOutcome::Sent));
    }

    /// A server that closes its stdout on EOF but keeps running has not
    /// exited: a closed witness alone does not settle the close.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn a_server_closing_stdout_on_eof_is_not_taken_as_exited() {
        use crate::stdio_test_fixture::sh_mcp_server_script;
        let config = McpServerConfig::stdio(
            "closes-stdout-on-eof",
            "/bin/sh",
            sh_mcp_server_script("", "exec >&-\nexec sleep 600\n"),
            HashMap::new(),
        );
        let conn = connect_stdio_with_bounds(&config, graces(300, 20_000)).await;

        let receipt = close_owned(conn).await;

        assert_eq!(
            receipt.witness,
            McpStdioWitnessOutcome::Closed(McpStdioClosePhase::Eof),
            "{receipt:?}"
        );
        assert_eq!(
            receipt.leader_exit,
            McpStdioLeaderExitObservation::Exited(McpStdioClosePhase::Term)
        );
        assert_eq!(receipt.settled_in, Some(McpStdioClosePhase::Term));
    }

    /// Without a stdout witness the close cannot see whether the server's
    /// group let go of its stdout, so even the server's own exit does not
    /// settle it: the close keeps its graces and escalates at each deadline,
    /// and the receipt says the witness was absent rather than closed.
    /// (Fails-old: the server's exit settled a close without a witness.)
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn a_missing_witness_does_not_settle_the_close() {
        use crate::stdio_test_fixture::sh_mcp_server_script;
        let config = McpServerConfig::stdio(
            "no-witness",
            "/bin/sh",
            sh_mcp_server_script("", "exit 0\n"),
            HashMap::new(),
        );
        let conn = connect_stdio_with_bounds(&config, graces(300, 300)).await;
        conn.stdio_child
            .as_ref()
            .expect("stdio custody")
            .replace_witness(None);

        let receipt = close_owned(conn).await;

        assert_eq!(receipt.witness, McpStdioWitnessOutcome::Absent);
        assert_eq!(receipt.settled_in, None, "{receipt:?}");
        assert_eq!(
            receipt.leader_exit,
            McpStdioLeaderExitObservation::Exited(McpStdioClosePhase::Eof)
        );
        assert!(receipt.sigterm.is_some() && receipt.sigkill.is_some());
        assert!(
            receipt.elapsed >= Duration::from_millis(600),
            "the close cut its graces short: {:?}",
            receipt.elapsed
        );
        assert!(matches!(receipt.leader, McpStdioLeaderOutcome::Reaped(_)));
    }

    /// A witness whose read fails is reported as a read error, distinct from
    /// a closed or absent one, and like an absent one it cannot settle the
    /// close. (Fails-old: the server's exit settled the close.)
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn a_failing_witness_is_reported_as_a_read_error() {
        use crate::stdio_test_fixture::sh_mcp_server_script;
        struct FailingRead;
        impl tokio::io::AsyncRead for FailingRead {
            fn poll_read(
                self: std::pin::Pin<&mut Self>,
                _cx: &mut std::task::Context<'_>,
                _buf: &mut tokio::io::ReadBuf<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::task::Poll::Ready(Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe)))
            }
        }
        let config = McpServerConfig::stdio(
            "failing-witness",
            "/bin/sh",
            sh_mcp_server_script("", "exit 0\n"),
            HashMap::new(),
        );
        let conn = connect_stdio_with_bounds(&config, graces(300, 300)).await;
        conn.stdio_child
            .as_ref()
            .expect("stdio custody")
            .replace_witness(Some(Box::new(FailingRead)));

        let receipt = close_owned(conn).await;

        assert_eq!(
            receipt.witness,
            McpStdioWitnessOutcome::ReadError {
                phase: McpStdioClosePhase::Eof,
                kind: std::io::ErrorKind::BrokenPipe,
            }
        );
        assert_eq!(receipt.settled_in, None, "{receipt:?}");
        assert!(receipt.sigterm.is_some());
    }

    /// Spawn `script` under `custody` as a plain process (no MCP handshake),
    /// and close its stdin and the transport's end of its stdout.
    #[cfg(unix)]
    async fn spawn_plain(custody: &StdioChildCustody, script: &str) {
        let config = McpServerConfig::stdio(
            "plain",
            "/bin/sh",
            vec!["-c".to_string(), script.to_string()],
            HashMap::new(),
        );
        let McpTransportConfig::Stdio(stdio) = &config.transport else {
            unreachable!("a stdio config");
        };
        let (stdout, stdin) = custody
            .spawn(
                &config.name,
                stdio,
                &McpStdioLaunchProfile::trusted_host(),
                &mut PhysicalEntry::default(),
            )
            .await
            .expect("spawn the plain server");
        drop((stdout, stdin));
    }

    /// A server that keeps its stdout busy past every deadline is still
    /// escalated against on time: an expired deadline is acted on before
    /// more output. (Fails-old: ready output was always taken before the
    /// expired deadline, so the close never escalated.)
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn busy_stdout_does_not_postpone_escalation() {
        let custody = StdioChildCustody::with_bounds(graces(200, 200));
        // An ignored disposition survives exec, so `yes` ignores TERM too.
        spawn_plain(&custody, "trap '' TERM\nexec yes").await;

        let receipt = tokio::time::timeout(Duration::from_secs(20), custody.terminate())
            .await
            .expect("busy stdout postponed the close past its deadlines")
            .expect("this terminate owned the close");

        assert_eq!(receipt.sigterm, Some(McpStdioSignalOutcome::Sent));
        assert_eq!(receipt.sigkill, Some(McpStdioSignalOutcome::Sent));
        assert_eq!(
            receipt.settled_in,
            Some(McpStdioClosePhase::Kill),
            "{receipt:?}"
        );
        assert!(
            receipt.elapsed < Duration::from_secs(5),
            "escalation came late: {:?}",
            receipt.elapsed
        );
    }

    /// A server that closes its stdout on EOF and then cleans up keeps its
    /// grace for the cleanup: a closed stdout is not taken as its exit, which
    /// off Linux, Android and macOS cannot be seen at all. (Fails-old: off Linux a
    /// closed witness settled the close, and the final SIGKILL cut the
    /// cleanup short.)
    #[cfg(unix)]
    #[tokio::test]
    async fn closed_stdout_does_not_cut_a_servers_cleanup_short() {
        let marker = marker_path("closed-stdout-cleanup");
        let custody = StdioChildCustody::with_bounds(graces(3_000, 3_000));
        spawn_plain(
            &custody,
            &format!(
                "while read -r _; do :; done\nexec >&-\nsleep 0.5\necho cleaned > '{}'\n",
                marker.display()
            ),
        )
        .await;

        let receipt = custody
            .terminate()
            .await
            .expect("this terminate owned the close");

        assert_eq!(take_marker(&marker), "cleaned", "{receipt:?}");
        assert_eq!(
            receipt.witness,
            McpStdioWitnessOutcome::Closed(McpStdioClosePhase::Eof)
        );
        if cfg!(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos"
        )) {
            assert_eq!(receipt.settled_in, Some(McpStdioClosePhase::Eof));
            assert_eq!(receipt.sigterm, None);
            // The exit was seen without reaping: the close's own reap took it.
            assert_eq!(
                receipt.leader_exit,
                McpStdioLeaderExitObservation::Exited(McpStdioClosePhase::Eof)
            );
            assert!(
                matches!(receipt.leader, McpStdioLeaderOutcome::Reaped(_)),
                "{receipt:?}"
            );
        } else {
            assert_eq!(receipt.settled_in, None);
            assert_eq!(
                receipt.leader_exit,
                McpStdioLeaderExitObservation::Unobservable
            );
        }
    }

    /// A server reaped out from under the close (as by a host that ignores
    /// SIGCHLD) is not taken as having exited, and its group, whose id is no
    /// longer held, is not signalled: the close reports the lost observation
    /// and stops. That holds with no grace too, where the deadline expires
    /// before the close ever watched the server. (Fails-old: `ECHILD` counted
    /// as the server's exit, and an expired deadline signalled the group
    /// without checking.)
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn a_server_reaped_elsewhere_is_reported_lost() {
        for bounds in [graces(20_000, 20_000), McpStdioCloseBounds::IMMEDIATE] {
            let custody = StdioChildCustody::with_bounds(bounds);
            spawn_plain(&custody, "exit 0").await;
            let pid = custody.spawned_pid().await;
            nix::sys::wait::waitpid(
                nix::unistd::Pid::from_raw(i32::try_from(pid).expect("pid fits i32")),
                None,
            )
            .expect("reap the server here, as another reaper would");

            let receipt = custody
                .terminate()
                .await
                .expect("this terminate owned the close");

            assert_eq!(
                receipt.leader_exit,
                McpStdioLeaderExitObservation::Lost {
                    phase: McpStdioClosePhase::Eof,
                    errno: Some(nix::errno::Errno::ECHILD as i32),
                },
                "{bounds:?}: {receipt:?}"
            );
            assert_eq!(receipt.settled_in, None, "{bounds:?}");
            assert_eq!(
                (receipt.sigterm, receipt.sigkill, receipt.final_group_signal),
                (None, None, None),
                "{bounds:?}"
            );
            assert!(!matches!(receipt.leader, McpStdioLeaderOutcome::Reaped(_)));
            assert!(receipt.elapsed < Duration::from_secs(5), "{bounds:?}");
        }
    }

    /// A server reaped elsewhere after the close saw it exit and its stdout
    /// close, but before the final check, does not leave a settled receipt:
    /// the loss clears the settlement, the group is not signalled, and the
    /// failed reap of our own is reported as such. (Fails-old: the receipt
    /// kept `settled_in` beside a lost leader.)
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn a_server_reaped_elsewhere_after_settling_is_not_settled() {
        let custody = StdioChildCustody::with_bounds(graces(20_000, 20_000));
        spawn_plain(&custody, "exit 0").await;
        let pid = custody.spawned_pid().await;
        custody.before_final_check(move || {
            nix::sys::wait::waitpid(
                nix::unistd::Pid::from_raw(i32::try_from(pid).expect("pid fits i32")),
                None,
            )
            .expect("reap the server here, as another reaper would");
        });

        let receipt = custody
            .terminate()
            .await
            .expect("this terminate owned the close");

        assert_eq!(
            receipt.witness,
            McpStdioWitnessOutcome::Closed(McpStdioClosePhase::Eof),
            "{receipt:?}"
        );
        assert_eq!(
            receipt.leader_exit,
            McpStdioLeaderExitObservation::Lost {
                phase: McpStdioClosePhase::Eof,
                errno: Some(nix::errno::Errno::ECHILD as i32),
            }
        );
        assert_eq!(receipt.settled_in, None);
        assert_eq!(receipt.final_group_signal, None);
        assert!(
            matches!(receipt.leader, McpStdioLeaderOutcome::WaitFailed(_)),
            "{receipt:?}"
        );
    }

    /// Poll `future` once and drop it.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    async fn poll_once_and_drop<F: std::future::Future>(future: F) {
        let mut future = std::pin::pin!(future);
        std::future::poll_fn(|cx| {
            let _ = future.as_mut().poll(cx);
            std::task::Poll::Ready(())
        })
        .await;
    }

    /// A connection close dropped while its client cancel is still running
    /// does not strand the server: the close task, cancel included, exists
    /// once the close is first polled, and finishes on its own. (Fails-old:
    /// the cancel ran before the close task existed, so the dropped close
    /// left the process held with no close started.)
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn a_close_dropped_during_its_cancel_still_closes() {
        use crate::stdio_test_fixture::sh_mcp_server_args;
        let config = McpServerConfig::stdio(
            "eof-ignoring",
            "/bin/sh",
            sh_mcp_server_args(None),
            HashMap::new(),
        );
        let custody = StdioChildCustody::with_bounds(graces(300, 20_000));
        let (conn, _) = McpConnection::connect_and_enumerate_with_custody(
            &config,
            McpAuthMode::Stored,
            None,
            None,
            Some(custody.clone()),
            &McpStdioLaunchProfile::trusted_host(),
            None,
        )
        .await
        .expect("fixture server completes the handshake");

        poll_once_and_drop(conn.close()).await;

        assert!(
            !custody.is_held(),
            "the dropped close left the process held with no close started"
        );
        let receipt = custody
            .terminate_or_join()
            .await
            .expect("the close finished and the dropped close left its receipt");
        assert_eq!(
            receipt.settled_in,
            Some(McpStdioClosePhase::Term),
            "{receipt:?}"
        );
    }

    /// A receipt is consumed only when its waiter takes it: a `terminate`
    /// dropped after its close finished, but before it was polled again,
    /// leaves the receipt to a join. (Fails-old: the receipt was handed to
    /// the dropped waiter's channel, so the join got nothing.)
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn a_receipt_sent_to_a_dropped_waiter_is_kept() {
        let custody = StdioChildCustody::with_bounds(graces(20_000, 20_000));
        spawn_plain(&custody, "exit 0").await;
        // Boxed so that dropping it drops the future itself.
        let mut waiter = Box::pin(custody.terminate());
        std::future::poll_fn(|cx| {
            assert!(
                waiter.as_mut().poll(cx).is_pending(),
                "the close just started"
            );
            std::task::Poll::Ready(())
        })
        .await;
        custody.close_finished().await;

        drop(waiter);

        let receipt = custody
            .terminate_or_join()
            .await
            .expect("the close finished and its receipt is joined");
        assert_eq!(receipt.settled_in, Some(McpStdioClosePhase::Eof));
    }

    /// Two closes of one custody: exactly one owns it and returns the
    /// receipt; the other reports that the close is owned elsewhere instead
    /// of vouching for it, and a later one that it was closed.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn concurrent_terminates_have_exactly_one_owner() {
        use crate::stdio_test_fixture::sh_mcp_server_args;
        let config = McpServerConfig::stdio(
            "eof-ignoring",
            "/bin/sh",
            sh_mcp_server_args(None),
            HashMap::new(),
        );
        let custody = StdioChildCustody::with_bounds(graces(300, 20_000));
        let (conn, _) = McpConnection::connect_and_enumerate_with_custody(
            &config,
            McpAuthMode::Stored,
            None,
            None,
            Some(custody.clone()),
            &McpStdioLaunchProfile::trusted_host(),
            None,
        )
        .await
        .expect("fixture server completes the handshake");

        // Dropping the connection closes the server's stdin; its custody
        // clone keeps the process.
        drop(conn);

        let (first, second) = tokio::join!(custody.terminate(), custody.terminate());

        let receipt = first.expect("the first terminate owned the close");
        assert_eq!(
            receipt.settled_in,
            Some(McpStdioClosePhase::Term),
            "{receipt:?}"
        );
        assert_eq!(second.err(), Some(CustodyNotHeld::ClosingElsewhere));
        assert_eq!(
            custody.terminate().await.err(),
            Some(CustodyNotHeld::ClosedElsewhere)
        );
    }

    /// Dropping the wait for a close does not cut the close short: it runs
    /// in a task of its own, escalates on its deadlines, and a later join
    /// gets the receipt the dropped wait left. (Fails-old: dropping the wait
    /// cancelled the close, and the group was SIGKILLed at once.)
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn a_dropped_close_wait_leaves_the_close_running() {
        use crate::stdio_test_fixture::{PidReport, exits_within, sh_mcp_server_args};
        let mut report = PidReport::new("dropped-close");
        let config = McpServerConfig::stdio(
            "eof-ignoring",
            "/bin/sh",
            sh_mcp_server_args(Some(report.path())),
            HashMap::new(),
        );
        let custody = StdioChildCustody::with_bounds(graces(300, 20_000));
        let (conn, _) = McpConnection::connect_and_enumerate_with_custody(
            &config,
            McpAuthMode::Stored,
            None,
            None,
            Some(custody.clone()),
            &McpStdioLaunchProfile::trusted_host(),
            None,
        )
        .await
        .expect("fixture server completes the handshake");
        let pids = report.pids().await;
        let (wrapper, server) = (pids[0], pids[1]);

        let dropped = tokio::time::timeout(Duration::from_millis(100), conn.close()).await;
        assert!(dropped.is_err(), "the close was still in its EOF grace");

        let receipt = custody
            .terminate_or_join()
            .await
            .expect("the close finished and the dropped wait left its receipt");
        assert_eq!(
            receipt.settled_in,
            Some(McpStdioClosePhase::Term),
            "{receipt:?}"
        );
        assert_eq!(receipt.sigterm, Some(McpStdioSignalOutcome::Sent));
        assert_eq!(receipt.sigkill, None);
        assert!(
            exits_within(wrapper, Duration::from_secs(5)).await,
            "stdio server {wrapper} outlived its close"
        );
        assert!(
            exits_within(server, Duration::from_secs(5)).await,
            "stdio server's grandchild {server} outlived its close"
        );
    }

    /// Only dropping the close task itself, as its runtime shuts down,
    /// cancels a close: the held child's drop still kills the whole group,
    /// and the custody records the close as abandoned rather than closed.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_close_cancelled_with_its_runtime_still_kills_the_group() {
        use crate::stdio_test_fixture::{PidReport, exits_within, sh_mcp_server_args};
        let runtime = || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime")
        };
        let custody = StdioChildCustody::with_bounds(graces(30_000, 30_000));
        let closing = runtime();
        let pids = closing.block_on(async {
            let mut report = PidReport::new("cancelled-close");
            let config = McpServerConfig::stdio(
                "eof-ignoring",
                "/bin/sh",
                sh_mcp_server_args(Some(report.path())),
                HashMap::new(),
            );
            let (conn, _) = McpConnection::connect_and_enumerate_with_custody(
                &config,
                McpAuthMode::Stored,
                None,
                None,
                Some(custody.clone()),
                &McpStdioLaunchProfile::trusted_host(),
                None,
            )
            .await
            .expect("fixture server completes the handshake");
            let pids = report.pids().await;
            tokio::spawn(conn.close());
            tokio::time::timeout(Duration::from_secs(5), async {
                while !custody.is_closing() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("the close started");
            pids
        });
        drop(closing);

        runtime().block_on(async {
            for pid in pids {
                assert!(
                    exits_within(pid, Duration::from_secs(5)).await,
                    "stdio process {pid} outlived its cancelled close"
                );
            }
            assert_eq!(
                custody.terminate_or_join().await.err(),
                Some(CustodyNotHeld::AbandonedElsewhere)
            );
        });
    }

    #[test]
    fn stdio_close_bounds_refuse_graces_above_the_maximum() {
        let over = McpStdioCloseBounds::MAX_GRACE + Duration::from_millis(1);
        let refused = McpStdioCloseBounds::new(over, Duration::ZERO).expect_err("eof over max");
        assert_eq!(refused.bound, "eof_grace");
        assert_eq!(refused.value, over);
        let refused = McpStdioCloseBounds::new(Duration::ZERO, over).expect_err("term over max");
        assert_eq!(refused.bound, "term_grace");
        let max = McpStdioCloseBounds::new(
            McpStdioCloseBounds::MAX_GRACE,
            McpStdioCloseBounds::MAX_GRACE,
        )
        .expect("the maximum itself is accepted");
        assert_eq!(max.eof_grace(), McpStdioCloseBounds::MAX_GRACE);
        assert_eq!(McpStdioCloseBounds::default(), McpStdioCloseBounds::DEFAULT);
        assert_eq!(
            McpStdioCloseBounds::DEFAULT.eof_grace(),
            Duration::from_secs(5)
        );
        assert_eq!(
            McpStdioCloseBounds::DEFAULT.term_grace(),
            Duration::from_secs(3)
        );
        assert_eq!(McpStdioCloseBounds::IMMEDIATE.eof_grace(), Duration::ZERO);
    }

    // --- Per-request OAuth bearer on established connections -----------------

    /// An OAuth-protected MCP server whose accepted bearer the test rotates,
    /// recording the `Authorization` header of every `tools/call`.
    struct RotatingMcpState {
        accepted: Mutex<String>,
        tool_call_authorizations: Mutex<Vec<Option<String>>>,
    }

    impl RotatingMcpState {
        fn accept(&self, token: &str) {
            *self.accepted.lock().unwrap() = token.to_owned();
        }

        fn tool_calls(&self) -> Vec<Option<String>> {
            self.tool_call_authorizations.lock().unwrap().clone()
        }
    }

    async fn spawn_rotating_mcp_server(accepted: &str) -> (String, Arc<RotatingMcpState>) {
        let state = Arc::new(RotatingMcpState {
            accepted: Mutex::new(accepted.to_owned()),
            tool_call_authorizations: Mutex::new(Vec::new()),
        });
        let app = Router::new()
            .route("/mcp", post(rotating_mcp_handler))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (url, state)
    }

    async fn rotating_mcp_handler(
        State(state): State<Arc<RotatingMcpState>>,
        headers: HeaderMap,
        Json(request): Json<Value>,
    ) -> axum::response::Response {
        let authorization = headers
            .get(http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        let method = request.get("method").and_then(Value::as_str);
        if method == Some("tools/call") {
            state
                .tool_call_authorizations
                .lock()
                .unwrap()
                .push(authorization.clone());
        }
        let accepted = format!("Bearer {}", state.accepted.lock().unwrap());
        if authorization.as_deref() != Some(accepted.as_str()) {
            return (
                StatusCode::UNAUTHORIZED,
                [(
                    http::header::WWW_AUTHENTICATE,
                    "Bearer error=\"invalid_token\", resource_metadata=\"/.well-known/oauth-protected-resource/mcp\"",
                )],
            )
                .into_response();
        }
        let Some(id) = request.get("id").cloned() else {
            return StatusCode::ACCEPTED.into_response();
        };
        let result = match method {
            Some("initialize") => serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "rotating-mcp-test-server", "version": "0.1.0" }
            }),
            Some("tools/list") => serde_json::json!({
                "tools": [{
                    "name": "echo",
                    "description": "Echo input",
                    "inputSchema": { "type": "object", "properties": {} }
                }]
            }),
            Some("tools/call") => serde_json::json!({
                "content": [{ "type": "text", "text": "echoed" }]
            }),
            other => {
                return (
                    StatusCode::OK,
                    Json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32601, "message": format!("unsupported {other:?}") }
                    })),
                )
                    .into_response();
            }
        };
        (
            StatusCode::OK,
            Json(serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result })),
        )
            .into_response()
    }

    /// The native credential owner as seen through the resolver: the stored
    /// credential changes when the owner refreshes or loses it.
    struct StoredCredentialResolver {
        stored: Mutex<Result<Option<String>, ()>>,
    }

    impl StoredCredentialResolver {
        fn new(token: &str) -> Self {
            Self {
                stored: Mutex::new(Ok(Some(token.to_owned()))),
            }
        }

        fn store(&self, token: &str) {
            *self.stored.lock().unwrap() = Ok(Some(token.to_owned()));
        }

        fn require_reauth(&self) {
            *self.stored.lock().unwrap() = Err(());
        }
    }

    #[async_trait]
    impl McpAuthResolver for StoredCredentialResolver {
        async fn stored_bearer_token(
            &self,
            target: &McpServerIdentity,
        ) -> Result<Option<String>, McpOAuthError> {
            self.stored
                .lock()
                .unwrap()
                .clone()
                .map_err(|()| McpOAuthError::ReauthRequired {
                    server_name: target.server_name().to_owned(),
                })
        }

        async fn interactive_login(
            &self,
            target: &McpServerIdentity,
            _www_authenticate: Option<&str>,
        ) -> Result<String, McpOAuthError> {
            Err(McpOAuthError::HumanAuthorizationRequired {
                server_name: target.server_name().to_owned(),
            })
        }
    }

    /// A governed control whose apply decision the test can change.
    struct SwitchableAttachment {
        server: McpServerConfig,
        allow: AtomicBool,
        checks: AtomicUsize,
    }

    impl McpAttachmentAuthority for SwitchableAttachment {
        fn server(&self) -> &McpServerConfig {
            &self.server
        }
        fn authorize_apply(
            &self,
        ) -> Result<(), meerkat_core::mcp_attachment::McpAttachmentRefusal> {
            self.checks.fetch_add(1, Ordering::SeqCst);
            if self.allow.load(Ordering::SeqCst) {
                Ok(())
            } else {
                Err(meerkat_core::mcp_attachment::McpAttachmentRefusal::Denied)
            }
        }
        fn authorize_persist(
            &self,
            _scope: meerkat_core::mcp_config::McpScope,
        ) -> Result<(), meerkat_core::mcp_attachment::McpAttachmentRefusal> {
            Err(meerkat_core::mcp_attachment::McpAttachmentRefusal::Denied)
        }
    }

    /// A stored credential whose second read (the first request's own
    /// per-request read, after the connect-time read) is held until released.
    struct HeldSecondRead {
        token: &'static str,
        reads: AtomicUsize,
        held: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }

    #[async_trait]
    impl McpAuthResolver for HeldSecondRead {
        async fn stored_bearer_token(
            &self,
            _target: &McpServerIdentity,
        ) -> Result<Option<String>, McpOAuthError> {
            if self.reads.fetch_add(1, Ordering::SeqCst) == 1 {
                self.held.notify_one();
                self.release.notified().await;
            }
            Ok(Some(self.token.to_owned()))
        }

        async fn interactive_login(
            &self,
            target: &McpServerIdentity,
            _www_authenticate: Option<&str>,
        ) -> Result<String, McpOAuthError> {
            Err(McpOAuthError::HumanAuthorizationRequired {
                server_name: target.server_name().to_owned(),
            })
        }
    }

    async fn held_http_attempt(
        allow_after_release: bool,
    ) -> (
        Result<(McpConnection, Vec<Arc<ToolDef>>), McpError>,
        Arc<SwitchableAttachment>,
        Arc<HttpMcpTestState>,
    ) {
        let (url, state) = spawn_http_mcp_server("held-token").await;
        let config = McpServerConfig::streamable_http("held", url, HashMap::new());
        let resolver = Arc::new(HeldSecondRead {
            token: "held-token",
            reads: AtomicUsize::new(0),
            held: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        let control = Arc::new(SwitchableAttachment {
            server: config.clone(),
            allow: AtomicBool::new(true),
            checks: AtomicUsize::new(0),
        });
        let attempt = tokio::spawn({
            let resolver: Arc<dyn McpAuthResolver> = resolver.clone();
            let control: Arc<dyn McpAttachmentAuthority> = control.clone();
            async move {
                McpConnection::connect_and_enumerate_with_custody(
                    &config,
                    McpAuthMode::Stored,
                    Some(resolver),
                    None,
                    None,
                    &McpStdioLaunchProfile::trusted_host(),
                    Some(control),
                )
                .await
            }
        });
        tokio::time::timeout(Duration::from_secs(10), resolver.held.notified())
            .await
            .expect("the first request reaches its own credential read");
        // The connect-time read passed, but the first request has not been
        // prepared: the control is not asked yet and nothing was sent.
        assert_eq!(control.checks.load(Ordering::SeqCst), 0);
        assert!(state.seen_authorizations.lock().unwrap().is_empty());
        control.allow.store(allow_after_release, Ordering::SeqCst);
        resolver.release.notify_one();
        let result = attempt.await.expect("attempt task");
        (result, control, state)
    }

    #[tokio::test]
    async fn http_attachment_refused_during_the_first_requests_credential_read_sends_nothing() {
        let (result, control, state) = held_http_attempt(false).await;
        assert!(matches!(
            result,
            Err(McpError::AttachmentRefused(
                meerkat_core::mcp_attachment::McpAttachmentRefusal::Denied
            ))
        ));
        assert_eq!(control.checks.load(Ordering::SeqCst), 1);
        assert!(
            state.seen_authorizations.lock().unwrap().is_empty(),
            "zero first-request arrivals"
        );
    }

    #[tokio::test]
    async fn http_attachment_enters_once_at_its_first_request_and_keeps_its_entry() {
        let (result, control, state) = held_http_attempt(true).await;
        let (conn, tools) = result.expect("the allowed attempt connects");
        assert!(!tools.is_empty());
        // Initialize, the initialized notification and tools/list all went
        // out under the one entry.
        assert!(state.seen_authorizations.lock().unwrap().len() >= 2);
        assert_eq!(control.checks.load(Ordering::SeqCst), 1);
        conn.close().await.expect("close");
    }

    async fn connect_rotating(
        token: &str,
    ) -> (
        McpServerConfig,
        McpConnection,
        Arc<RotatingMcpState>,
        Arc<StoredCredentialResolver>,
    ) {
        let (url, state) = spawn_rotating_mcp_server(token).await;
        let config = McpServerConfig::streamable_http("rotating", url, HashMap::new());
        let resolver = Arc::new(StoredCredentialResolver::new(token));
        let (conn, tools) = McpConnection::connect_and_enumerate_with_mcp_auth(
            &config,
            McpAuthMode::Interactive,
            Some(resolver.clone()),
        )
        .await
        .expect("the stored credential connects");
        assert!(tools.iter().any(|tool| tool.name == "echo"));
        (config, conn, state, resolver)
    }

    /// G5: an established connection reads the bearer from the credential
    /// owner on every request, so a refreshed credential is used without a
    /// reconnect. (Fails-old: the connect-time token stayed on the transport.)
    #[tokio::test]
    async fn oauth_connection_resolves_the_bearer_per_request() {
        let (_config, conn, state, resolver) = connect_rotating("token-a").await;
        state.accept("token-b");
        resolver.store("token-b");

        let blocks = conn
            .call_tool("echo", &serde_json::json!({}))
            .await
            .expect("the refreshed credential is used");

        assert_eq!(meerkat_core::types::text_content(&blocks), "echoed");
        assert_eq!(
            state.tool_calls(),
            vec![Some("Bearer token-b".to_owned())],
            "the call carries the owner's current credential"
        );
        conn.close().await.expect("close");
    }

    /// G5: a 401 on an established connection is the typed host status, and
    /// the refused call is not replayed. (Fails-old: an untyped tool failure.)
    #[tokio::test]
    async fn oauth_call_refused_with_401_is_typed_and_not_replayed() {
        let (config, conn, state, _resolver) = connect_rotating("token-a").await;
        state.accept("token-after-revocation");

        let error = conn
            .call_tool("echo", &serde_json::json!({}))
            .await
            .expect_err("the provider revoked the credential");

        let McpError::AuthorizationRequired { target } = &error else {
            panic!("expected typed authorization-required, got {error:?}");
        };
        assert_eq!(**target, McpServerIdentity::from_config(&config).unwrap());
        assert_eq!(
            state.tool_calls(),
            vec![Some("Bearer token-a".to_owned())],
            "exactly one dispatch: the refused call is never replayed"
        );
        conn.close().await.expect("close");
    }

    /// G5: when the owner has no usable credential (reauthentication
    /// required), the call fails typed before anything is sent.
    /// (Fails-old: the stale connect-time token was still sent.)
    #[tokio::test]
    async fn oauth_call_without_a_usable_credential_fails_typed_before_dispatch() {
        let (_config, conn, state, resolver) = connect_rotating("token-a").await;
        resolver.require_reauth();

        let error = conn
            .call_tool("echo", &serde_json::json!({}))
            .await
            .expect_err("no usable credential");

        assert!(
            matches!(error, McpError::AuthorizationRequired { .. }),
            "got {error:?}"
        );
        assert!(
            state.tool_calls().is_empty(),
            "nothing is dispatched without a usable credential"
        );
        conn.close().await.expect("close");
    }
}

#[cfg(test)]
#[path = "pagination_tests.rs"]
mod pagination_tests;

#[cfg(test)]
#[path = "structured_result_tests.rs"]
mod structured_result_tests;

#[cfg(test)]
#[path = "process_confinement_tests.rs"]
mod process_confinement_tests;

#[cfg(all(test, feature = "workspace-tests"))]
#[path = "service_secret_tests.rs"]
mod service_secret_tests;

#[cfg(test)]
#[path = "stdio_cwd_tests.rs"]
mod stdio_cwd_tests;

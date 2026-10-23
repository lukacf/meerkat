//! MCP router for multi-server routing
//!
//! The router is a shell that manages connections, async tasks, and tool
//! caching. Runtime/session construction injects a generated machine-owned
//! [`ExternalToolSurfaceHandle`] as the surface owner. Construction without a
//! bound handle fails closed for lifecycle mutations until that owner is
//! supplied.

use crate::connection::StdioChildCustody;
use crate::external_tool_surface_authority::{
    ExternalToolSurfaceEffect, ExternalToolSurfaceError, ExternalToolSurfaceInput,
    ExternalToolSurfacePhase, ExternalToolSurfaceTransition, RemovalTimingInfo, StagedSurfaceOp,
    SurfaceBaseState, SurfaceDeltaOperation, SurfaceDeltaPhase, SurfaceId, TurnNumber,
};
use crate::generated::{
    protocol_surface_completion::{self, SurfaceCompletionObligation},
    protocol_surface_snapshot_alignment::{self, SurfaceSnapshotAlignmentObligation},
};
use crate::{McpAuthResolver, McpConnection, McpError, McpServerCloseOutcome};
use async_trait::async_trait;
use meerkat_auth_core::{McpAuthMode, McpServerIdentity};
use meerkat_core::AgentToolDispatcher;
use meerkat_core::ExternalToolUpdate;
use meerkat_core::McpServerConfig;
use meerkat_core::ToolCatalogCapabilities;
use meerkat_core::ToolCatalogEntry;
use meerkat_core::error::ToolError;
use meerkat_core::event::{ExternalToolDelta, ExternalToolDeltaPhase, ToolConfigChangeOperation};
use meerkat_core::handles::{
    DslTransitionError, ExternalToolSurfaceEffect as CoreSurfaceEffect, ExternalToolSurfaceHandle,
    ExternalToolSurfaceInput as CoreSurfaceInput,
    ExternalToolSurfaceTransition as CoreSurfaceTransition, McpServerLifecycleHandle,
    SurfaceDiagnosticSnapshot, SurfaceSnapshot,
};
use meerkat_core::mcp_attachment::{McpAttachmentAuthority, McpAttachmentRefusal};
use meerkat_core::mcp_config::McpTransportConfig;
use meerkat_core::types::ToolDef;
use meerkat_core::types::{ContentBlock, ToolCallView, ToolResult};
use meerkat_core::{
    ExternalToolSurfaceBaseState, ExternalToolSurfaceDeltaOperation, ExternalToolSurfaceDeltaPhase,
    ExternalToolSurfaceEntrySnapshot, ExternalToolSurfaceFailureCause,
    ExternalToolSurfaceGlobalPhase, ExternalToolSurfacePendingOp, ExternalToolSurfaceSnapshot,
    ExternalToolSurfaceStagedOp,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock as StdRwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

const PENDING_CHANNEL_CAPACITY: usize = 32;

/// MCP server lifecycle state used by staged router apply.
///
/// This is a public API projection of the surface owner's internal state.
/// The canonical truth lives in the active owner path; this enum is derived
/// from it for backward-compatible API consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpServerLifecycleState {
    Active,
    Removing {
        draining_since: Instant,
        timeout_at: Instant,
    },
    Removed,
}

/// Reload target for staged reload operations.
#[derive(Debug, Clone)]
pub enum McpReloadTarget {
    ServerName(String),
    Config(McpServerConfig),
}

impl From<&str> for McpReloadTarget {
    fn from(value: &str) -> Self {
        Self::ServerName(value.to_string())
    }
}

impl From<String> for McpReloadTarget {
    fn from(value: String) -> Self {
        Self::ServerName(value)
    }
}

impl From<McpServerConfig> for McpReloadTarget {
    fn from(value: McpServerConfig) -> Self {
        Self::Config(value)
    }
}

pub type McpLifecyclePhase = ExternalToolDeltaPhase;
pub type McpLifecycleAction = ExternalToolDelta;

/// Typed per-server boundary-apply rejection (K14).
///
/// Carried on [`McpApplyDelta`] so a partially-rejected boundary apply is a
/// typed per-server fact in the result, not a `tracing::warn!`-and-continue.
#[derive(Debug, Clone)]
pub struct McpBoundaryRejection {
    pub server: String,
    pub error: ExternalToolSurfaceError,
}

/// Result of applying staged MCP operations.
#[derive(Debug, Clone, Default)]
pub struct McpApplyDelta {
    pub added_servers: Vec<String>,
    pub removed_servers: Vec<String>,
    pub reloaded_servers: Vec<String>,
    pub lifecycle_actions: Vec<McpLifecycleAction>,
    pub degraded_removals: Vec<String>,
    /// Staged intents whose `ApplyBoundary` the surface owner rejected.
    pub rejected_boundaries: Vec<McpBoundaryRejection>,
}

/// Return value of [`McpRouter::apply_staged`].
#[derive(Debug)]
pub struct McpApplyResult {
    pub delta: McpApplyDelta,
    pub pending_count: usize,
}

/// Where one governed live add stands, from this router's own accepted
/// outcome of exactly that add. Its sender travels from staging with the
/// staged control, is keyed by the accepted completion obligation once the
/// boundary schedules the attempt, and is resolved where the surface owner
/// decides that attempt. Dropping the router drops every sender.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AttemptState {
    Pending,
    Installed {
        connection_generation: u64,
    },
    Rejected(String),
    /// The attempt failed: its detail, and the typed stdio setup failure its
    /// transport established, if any. The class has no server text and
    /// stays with the attempt; its stderr tail is retained by the router
    /// only (see [`McpRouter::stdio_stderr_tail`]).
    Failed {
        detail: String,
        failure: Option<meerkat_core::mcp_attachment::McpStdioSetupFailure>,
    },
    Superseded,
}

impl AttemptState {
    fn failed(detail: impl Into<String>) -> Self {
        Self::Failed {
            detail: detail.into(),
            failure: None,
        }
    }
}

/// The one failed stdio attempt a router retains per server: its typed
/// failure and its diagnostics record (with the stderr tail).
struct RetainedStdioFailure {
    failure: meerkat_core::mcp_attachment::McpStdioSetupFailure,
    record: Arc<crate::stdio_diagnostics::StdioAttemptRecord>,
}

pub(crate) type AttemptSender = tokio::sync::watch::Sender<AttemptState>;
pub(crate) type AttemptReceiver = tokio::sync::watch::Receiver<AttemptState>;

fn resolve_attempt(attempt: Option<&AttemptSender>, state: AttemptState) {
    if let Some(attempt) = attempt {
        let _ = attempt.send(state);
    }
}

/// What one tool is on the exact connection generation an accepted attempt
/// installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstalledOperation {
    /// That connection is current and a published route serves the
    /// operation on it.
    Callable,
    /// That connection is current, but no published route serves the
    /// operation.
    NotRouted,
    /// The server's current connection is another one, or none.
    Replaced,
}

struct PendingResult {
    obligation: SurfaceCompletionObligation,
    /// The exact config this attempt connected with.
    config: McpServerConfig,
    result: Result<(McpConnection, Vec<Arc<ToolDef>>), McpError>,
    /// The attempt's guard record, when its guard was begun: told the open
    /// outcome when this result is processed.
    guard: Option<Arc<crate::attempt_guard::AttemptGuardRecord>>,
}

/// One connection attempt the surface owner accepted as failed with
/// [`McpError::AuthorizationRequired`]: its exact target, the config it
/// attempted and the operation it ran. Only this record can be retried after
/// a login ([`McpRouter::retry_authorized`]).
#[derive(Debug, Clone)]
struct AwaitingAuthorization {
    target: McpServerIdentity,
    config: McpServerConfig,
    operation: AwaitingOperation,
    /// For a [`AwaitingOperation::Released`] record: a sign-in for exactly
    /// this target completed while the release's own removal was still
    /// staged or settling. The server is staged again at the first boundary
    /// after that removal settles, with no further sign-in; the flag stays
    /// set until the owner accepts that add.
    signed_in_during_release: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AwaitingOperation {
    Add,
    Reload,
    /// The server was connected and its credential was released (signed
    /// out, or refused at a call). Its removal is staged; the record outlives
    /// that removal so a later sign-in adds the server back to this router.
    Released,
}

/// What [`McpRouter::retry_authorized`] did for one completed login target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorizedRetry {
    /// The failed operation was staged again with its retained config. It
    /// applies at the next boundary and resolves the credential then.
    Staged,
    /// No accepted, unsuperseded failure awaits authorization for exactly
    /// this target (absent, another URL or account, removed, or retried).
    NotAwaiting,
    /// A newer staged intent (including a remove) or an operation in flight
    /// owns the server; it is left alone.
    AlreadyPending,
    /// The server's credential was released and the removal that release
    /// staged has not settled yet. The sign-in is retained: the server is
    /// staged again at the first boundary after that removal settles, with
    /// no further sign-in.
    AfterRelease,
    /// The router was shut down.
    RouterClosed,
}

/// What [`McpRouter::credential_released`] did for one released credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleasedCredential {
    /// A connected server using exactly this credential is no longer usable:
    /// its removal is staged (its tools are unpublished at the next
    /// boundary) and it awaits sign-in, so a later login adds it back.
    Unpublished,
    /// No connected server uses exactly this credential (absent, another
    /// URL or account, or not connected).
    NotActive,
    /// A newer staged intent (including a remove) or an operation in flight
    /// owns the server; it is left alone.
    AlreadyPending,
    /// The router was shut down.
    RouterClosed,
}

struct CompletedLifecycleUpdate {
    action: McpLifecycleAction,
}

/// Fail-closed surface handle used until generated machine authority is bound.
struct UnboundExternalToolSurfaceHandle;

impl UnboundExternalToolSurfaceHandle {
    fn new() -> Self {
        Self
    }

    fn reject(context: &'static str) -> DslTransitionError {
        DslTransitionError::guard_rejected(
            context,
            "external tool surface lifecycle requires generated machine authority",
        )
    }

    fn empty_snapshot() -> SurfaceDiagnosticSnapshot {
        SurfaceDiagnosticSnapshot {
            surface_phase: ExternalToolSurfaceGlobalPhase::Operating,
            known_surfaces: BTreeSet::new(),
            visible_surfaces: BTreeSet::new(),
            snapshot_epoch: 0,
            snapshot_aligned_epoch: 0,
            has_pending_or_staged: false,
            entries: Vec::new(),
        }
    }
}

impl ExternalToolSurfaceHandle for UnboundExternalToolSurfaceHandle {
    fn apply_surface_input(
        &self,
        _input: CoreSurfaceInput,
    ) -> Result<CoreSurfaceTransition, DslTransitionError> {
        Err(Self::reject(
            "UnboundExternalToolSurfaceHandle::apply_surface_input",
        ))
    }

    fn register(&self, _surface_id: String) -> Result<(), DslTransitionError> {
        Err(Self::reject("UnboundExternalToolSurfaceHandle::register"))
    }

    fn stage_add(&self, _surface_id: String, _now_ms: u64) -> Result<(), DslTransitionError> {
        Err(Self::reject("UnboundExternalToolSurfaceHandle::stage_add"))
    }

    fn stage_remove(&self, _surface_id: String, _now_ms: u64) -> Result<(), DslTransitionError> {
        Err(Self::reject(
            "UnboundExternalToolSurfaceHandle::stage_remove",
        ))
    }

    fn stage_reload(&self, _surface_id: String, _now_ms: u64) -> Result<(), DslTransitionError> {
        Err(Self::reject(
            "UnboundExternalToolSurfaceHandle::stage_reload",
        ))
    }

    fn apply_boundary(
        &self,
        _surface_id: String,
        _now_ms: u64,
        _staged_intent_sequence: u64,
        _applied_at_turn: u64,
    ) -> Result<(), DslTransitionError> {
        Err(Self::reject(
            "UnboundExternalToolSurfaceHandle::apply_boundary",
        ))
    }

    fn mark_pending_succeeded(
        &self,
        _surface_id: String,
        _pending_task_sequence: u64,
        _staged_intent_sequence: u64,
    ) -> Result<(), DslTransitionError> {
        Err(Self::reject(
            "UnboundExternalToolSurfaceHandle::mark_pending_succeeded",
        ))
    }

    fn mark_pending_failed(
        &self,
        _surface_id: String,
        _pending_task_sequence: u64,
        _staged_intent_sequence: u64,
        _cause: ExternalToolSurfaceFailureCause,
    ) -> Result<(), DslTransitionError> {
        Err(Self::reject(
            "UnboundExternalToolSurfaceHandle::mark_pending_failed",
        ))
    }

    fn call_started(&self, _surface_id: String) -> Result<(), DslTransitionError> {
        Err(Self::reject(
            "UnboundExternalToolSurfaceHandle::call_started",
        ))
    }

    fn call_finished(&self, _surface_id: String) -> Result<(), DslTransitionError> {
        Err(Self::reject(
            "UnboundExternalToolSurfaceHandle::call_finished",
        ))
    }

    fn finalize_removal_clean(&self, _surface_id: String) -> Result<(), DslTransitionError> {
        Err(Self::reject(
            "UnboundExternalToolSurfaceHandle::finalize_removal_clean",
        ))
    }

    fn finalize_removal_forced(&self, _surface_id: String) -> Result<(), DslTransitionError> {
        Err(Self::reject(
            "UnboundExternalToolSurfaceHandle::finalize_removal_forced",
        ))
    }

    fn snapshot_aligned(&self, _epoch: u64) -> Result<(), DslTransitionError> {
        Err(Self::reject(
            "UnboundExternalToolSurfaceHandle::snapshot_aligned",
        ))
    }

    fn shutdown_surface(&self) -> Result<(), DslTransitionError> {
        Err(Self::reject(
            "UnboundExternalToolSurfaceHandle::shutdown_surface",
        ))
    }

    fn surface_snapshot(&self, _surface_id: &str) -> Option<SurfaceSnapshot> {
        None
    }

    fn diagnostic_snapshot(&self) -> SurfaceDiagnosticSnapshot {
        Self::empty_snapshot()
    }

    fn visible_surfaces(&self) -> BTreeSet<String> {
        BTreeSet::new()
    }

    fn removing_surfaces(&self) -> BTreeSet<String> {
        BTreeSet::new()
    }

    fn pending_surfaces(&self) -> BTreeSet<String> {
        BTreeSet::new()
    }

    fn has_pending_or_staged(&self) -> bool {
        false
    }

    fn snapshot_epoch(&self) -> u64 {
        0
    }

    fn snapshot_aligned_epoch(&self) -> u64 {
        0
    }
}

/// How a router holds its external tool surface handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpSurfaceOwnership {
    /// The surface is the router's own (a standalone construction's private
    /// handle): shutting the router down shuts the whole surface down.
    Owned,
    /// The surface belongs to someone else, such as a session registration
    /// that outlives this router: shutting the router down releases only
    /// the lineages this router staged, and the surface stays open for the
    /// next holder.
    Borrowed,
}

/// The staged intent lineages a router recorded, per surface id. Each was
/// read from the `StageRecorded` effect of the router's own stage input, so
/// it names exactly the staging this router made.
pub type HeldSurfaceLineages = Arc<Mutex<BTreeMap<String, BTreeSet<u64>>>>;

/// How the router holds its current surface handle, and whether its hold has
/// ended, shared with whoever can rebind that handle.
#[derive(Debug)]
pub struct SurfaceHold {
    ownership: McpSurfaceOwnership,
    ended: bool,
}

impl SurfaceHold {
    /// A rebind onto another owner's surface makes the hold `Borrowed`.
    /// Refused (`false`) once the router ended its hold: nothing may be
    /// published to a router that already released or shut its surface.
    pub(crate) fn rebind_borrowed(&mut self) -> bool {
        if self.ended {
            return false;
        }
        self.ownership = McpSurfaceOwnership::Borrowed;
        true
    }
}

/// The router's [`SurfaceHold`], shared with whoever can rebind its handle.
pub type SurfaceOwnershipSlot = Arc<Mutex<SurfaceHold>>;

enum SurfaceOwner {
    Runtime {
        handle_slot: Arc<StdRwLock<Arc<dyn ExternalToolSurfaceHandle>>>,
        ownership: SurfaceOwnershipSlot,
        held: HeldSurfaceLineages,
    },
}

impl SurfaceOwner {
    fn runtime(handle: Arc<dyn ExternalToolSurfaceHandle>) -> Self {
        Self::Runtime {
            handle_slot: Arc::new(StdRwLock::new(handle)),
            ownership: Arc::new(Mutex::new(SurfaceHold {
                ownership: McpSurfaceOwnership::Borrowed,
                ended: false,
            })),
            held: HeldSurfaceLineages::default(),
        }
    }

    fn ownership_slot(&self) -> SurfaceOwnershipSlot {
        match self {
            Self::Runtime { ownership, .. } => Arc::clone(ownership),
        }
    }

    fn ownership(&self) -> McpSurfaceOwnership {
        self.ownership_slot()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .ownership
    }

    fn set_ownership(&mut self, value: McpSurfaceOwnership) {
        self.ownership_slot()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .ownership = value;
    }

    fn held_lineages(&self) -> HeldSurfaceLineages {
        match self {
            Self::Runtime { held, .. } => Arc::clone(held),
        }
    }

    /// End this router's hold on its surface: shut an owned surface down, or
    /// release only this router's own lineages of a borrowed one. One
    /// exclusive claim of the handle slot captures the handle, its ownership
    /// and the held lineages together and marks the hold ended, and the
    /// cleanup applies to exactly that captured handle. A rebind, which
    /// replaces all three under the same write lock, therefore happens
    /// entirely before the claim (and is what gets released) or after it, in
    /// which case it publishes nothing and compensates what it seeded.
    fn end_holding(&self) {
        let Self::Runtime { handle_slot, .. } = self;
        let handle = handle_slot
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let ownership = {
            let slot = self.ownership_slot();
            let mut hold = slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            hold.ended = true;
            hold.ownership
        };
        match ownership {
            McpSurfaceOwnership::Owned => {
                if let Err(error) = handle.apply_surface_input(CoreSurfaceInput::Shutdown) {
                    tracing::debug!("owned surface not shut down: {error}");
                }
            }
            McpSurfaceOwnership::Borrowed => self.release_held_on(handle.as_ref()),
        }
    }

    fn release_held_on(&self, handle: &dyn ExternalToolSurfaceHandle) {
        let held = std::mem::take(
            &mut *self
                .held_lineages()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        release_lineages(handle, held);
    }

    fn runtime_handle_slot(&self) -> Option<Arc<StdRwLock<Arc<dyn ExternalToolSurfaceHandle>>>> {
        match self {
            Self::Runtime { handle_slot, .. } => Some(Arc::clone(handle_slot)),
        }
    }

    fn runtime_handle(&self) -> Option<Arc<dyn ExternalToolSurfaceHandle>> {
        match self {
            Self::Runtime { handle_slot, .. } => Some(
                handle_slot
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone(),
            ),
        }
    }

    fn diagnostic_snapshot(&self) -> ExternalToolSurfaceSnapshot {
        match self {
            Self::Runtime { handle_slot, .. } => {
                let handle = handle_slot
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                snapshot_from_handle(handle.diagnostic_snapshot())
            }
        }
    }

    fn surface_snapshot(&self, surface_id: &str) -> Option<SurfaceSnapshot> {
        match self {
            Self::Runtime { .. } => self
                .runtime_handle()
                .and_then(|handle| handle.surface_snapshot(surface_id)),
        }
    }

    fn set_removal_timeout(
        &mut self,
        removal_timeout: Duration,
    ) -> Result<(), ExternalToolSurfaceError> {
        match self {
            Self::Runtime { handle_slot, .. } => {
                let timeout_ms = removal_timeout.as_millis().min(u128::from(u64::MAX)) as u64;
                let handle = handle_slot
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                handle
                    .apply_surface_input(CoreSurfaceInput::SetRemovalTimeout { timeout_ms })
                    .map(|_| ())
                    .map_err(|error| runtime_surface_error("SetRemovalTimeout", error))
            }
        }
    }

    fn apply(
        &self,
        input: ExternalToolSurfaceInput,
    ) -> Result<ExternalToolSurfaceTransition, ExternalToolSurfaceError> {
        match self {
            Self::Runtime {
                handle_slot, held, ..
            } => {
                // Held across the apply and its lineage record, so a rebind
                // (which replaces the handle and the held lineages together
                // under the write lock) never pairs one surface's lineage
                // with another surface.
                let handle = handle_slot
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                Self::apply_runtime(handle.as_ref(), held, input)
            }
        }
    }

    fn apply_runtime(
        handle: &dyn ExternalToolSurfaceHandle,
        held: &HeldSurfaceLineages,
        input: ExternalToolSurfaceInput,
    ) -> Result<ExternalToolSurfaceTransition, ExternalToolSurfaceError> {
        let (input_name, core_input, applied_at_turn) = match input {
            ExternalToolSurfaceInput::StageAdd { surface_id } => (
                "StageAdd",
                CoreSurfaceInput::StageAdd {
                    surface_id: surface_id.0,
                    now_ms: McpRouter::now_ms(),
                },
                None,
            ),
            ExternalToolSurfaceInput::StageRemove { surface_id } => (
                "StageRemove",
                CoreSurfaceInput::StageRemove {
                    surface_id: surface_id.0,
                    now_ms: McpRouter::now_ms(),
                },
                None,
            ),
            ExternalToolSurfaceInput::StageReload { surface_id } => (
                "StageReload",
                CoreSurfaceInput::StageReload {
                    surface_id: surface_id.0,
                    now_ms: McpRouter::now_ms(),
                },
                None,
            ),
            ExternalToolSurfaceInput::ApplyBoundary {
                surface_id,
                staged_intent_sequence,
                applied_at_turn,
            } => (
                "ApplyBoundary",
                CoreSurfaceInput::ApplyBoundary {
                    surface_id: surface_id.0,
                    now_ms: McpRouter::now_ms(),
                    staged_intent_sequence,
                    applied_at_turn: applied_at_turn.0,
                },
                Some(applied_at_turn),
            ),
            ExternalToolSurfaceInput::PendingSucceeded {
                surface_id,
                operation: _operation,
                pending_task_sequence,
                staged_intent_sequence,
                applied_at_turn,
            } => (
                "PendingSucceeded",
                CoreSurfaceInput::MarkPendingSucceeded {
                    surface_id: surface_id.0,
                    pending_task_sequence,
                    staged_intent_sequence,
                },
                Some(applied_at_turn),
            ),
            ExternalToolSurfaceInput::PendingFailed {
                surface_id,
                operation: _operation,
                pending_task_sequence,
                staged_intent_sequence,
                applied_at_turn,
                cause,
            } => (
                "PendingFailed",
                CoreSurfaceInput::MarkPendingFailed {
                    surface_id: surface_id.0,
                    pending_task_sequence,
                    staged_intent_sequence,
                    cause,
                },
                Some(applied_at_turn),
            ),
            ExternalToolSurfaceInput::CallStarted { surface_id } => (
                "CallStarted",
                CoreSurfaceInput::CallStarted {
                    surface_id: surface_id.0,
                },
                None,
            ),
            ExternalToolSurfaceInput::CallFinished { surface_id } => (
                "CallFinished",
                CoreSurfaceInput::CallFinished {
                    surface_id: surface_id.0,
                },
                None,
            ),
            ExternalToolSurfaceInput::FinalizeRemovalClean {
                surface_id,
                applied_at_turn,
            } => (
                "FinalizeRemovalClean",
                CoreSurfaceInput::FinalizeRemovalClean {
                    surface_id: surface_id.0,
                },
                Some(applied_at_turn),
            ),
            ExternalToolSurfaceInput::FinalizeRemovalForced {
                surface_id,
                applied_at_turn,
            } => (
                "FinalizeRemovalForced",
                CoreSurfaceInput::FinalizeRemovalForced {
                    surface_id: surface_id.0,
                },
                Some(applied_at_turn),
            ),
            ExternalToolSurfaceInput::SnapshotAligned { snapshot_epoch } => (
                "SnapshotAligned",
                CoreSurfaceInput::SnapshotAligned {
                    epoch: snapshot_epoch,
                },
                None,
            ),
            ExternalToolSurfaceInput::Shutdown => ("Shutdown", CoreSurfaceInput::Shutdown, None),
        };
        handle
            .apply_surface_input(core_input)
            .map(|transition| {
                record_stage_lineages(held, &transition);
                runtime_transition_from_core(input_name, transition, applied_at_turn)
            })
            .map_err(|error| runtime_surface_error(input_name, error))
    }

    fn staged_intents_in_order(&self) -> Vec<(SurfaceId, StagedSurfaceOp, u64)> {
        match self {
            Self::Runtime { .. } => {
                let mut staged = self
                    .diagnostic_snapshot()
                    .entries
                    .into_iter()
                    .filter_map(|entry| {
                        let staged_op = staged_surface_op_from_snapshot(entry.staged_op);
                        if staged_op == StagedSurfaceOp::None {
                            return None;
                        }
                        Some((
                            SurfaceId::from(entry.surface_id),
                            staged_op,
                            entry.staged_intent_sequence,
                        ))
                    })
                    .collect::<Vec<_>>();
                staged.sort_by_key(|(_, _, sequence)| *sequence);
                staged
            }
        }
    }

    fn pending_count(&self) -> usize {
        self.diagnostic_snapshot()
            .entries
            .into_iter()
            .filter(|entry| entry.pending_op != ExternalToolSurfacePendingOp::None)
            .count()
    }

    fn pending_surfaces(&self) -> Vec<SurfaceId> {
        self.diagnostic_snapshot()
            .entries
            .into_iter()
            .filter(|entry| entry.pending_op != ExternalToolSurfacePendingOp::None)
            .map(|entry| SurfaceId::from(entry.surface_id))
            .collect()
    }

    fn visible_surfaces(&self) -> Vec<SurfaceId> {
        self.diagnostic_snapshot()
            .entries
            .into_iter()
            .filter(|entry| entry.visible)
            .map(|entry| SurfaceId::from(entry.surface_id))
            .collect()
    }

    fn removing_surfaces(&self) -> Vec<SurfaceId> {
        self.diagnostic_snapshot()
            .entries
            .into_iter()
            .filter(|entry| entry.base_state == ExternalToolSurfaceBaseState::Removing)
            .map(|entry| SurfaceId::from(entry.surface_id))
            .collect()
    }

    fn snapshot_epoch(&self) -> u64 {
        self.diagnostic_snapshot().snapshot_epoch
    }

    fn surface_base(&self, id: &SurfaceId) -> SurfaceBaseState {
        self.diagnostic_snapshot()
            .entries
            .into_iter()
            .find(|entry| entry.surface_id == id.0)
            .map(|entry| surface_base_from_snapshot(entry.base_state))
            .unwrap_or(SurfaceBaseState::Absent)
    }

    fn inflight_call_count(&self, id: &SurfaceId) -> u64 {
        self.diagnostic_snapshot()
            .entries
            .into_iter()
            .find(|entry| entry.surface_id == id.0)
            .map(|entry| entry.inflight_call_count)
            .unwrap_or(0)
    }

    fn removal_timing(&self, id: &SurfaceId) -> Option<RemovalTimingInfo> {
        match self {
            Self::Runtime { .. } => {
                let entry = self.surface_snapshot(&id.0)?;
                let draining_since_ms = entry.removal_draining_since_ms?;
                let timeout_at_ms = entry.removal_timeout_at_ms?;
                Some(RemovalTimingInfo {
                    draining_since: instant_from_epoch_ms(draining_since_ms),
                    timeout_at: instant_from_epoch_ms(timeout_at_ms),
                    applied_at_turn: TurnNumber(entry.removal_applied_at_turn.unwrap_or(0)),
                })
            }
        }
    }
}

/// Release exactly `held`'s lineages on `handle`. The surface clears
/// whichever staged, pending or active slot still carries exactly that
/// lineage and refuses the rest. Releasing an active slot can also cancel a
/// different, staged Reload that depends on that active base.
pub(crate) fn release_lineages(
    handle: &dyn ExternalToolSurfaceHandle,
    held: BTreeMap<String, BTreeSet<u64>>,
) {
    for (surface_id, lineages) in held {
        for lineage in lineages {
            if let Err(error) = handle.apply_surface_input(CoreSurfaceInput::Release {
                surface_id: surface_id.clone(),
                lineage,
            }) {
                // A lineage the surface no longer carries (completed,
                // replaced, removed, or a closed registration) has
                // nothing of this router's left to release.
                tracing::debug!(
                    surface = %surface_id,
                    lineage,
                    "surface lineage not released: {error}"
                );
            }
        }
    }
}

/// Record the lineage a stage input of this router just recorded.
fn record_stage_lineages(held: &HeldSurfaceLineages, transition: &CoreSurfaceTransition) {
    for effect in &transition.effects {
        if let CoreSurfaceEffect::StageRecorded {
            surface_id,
            staged_intent_sequence,
        } = effect
        {
            held.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .entry(surface_id.clone())
                .or_default()
                .insert(*staged_intent_sequence);
        }
    }
}

fn runtime_transition_from_core(
    transition_name: &str,
    transition: CoreSurfaceTransition,
    applied_at_turn: Option<TurnNumber>,
) -> ExternalToolSurfaceTransition {
    ExternalToolSurfaceTransition {
        transition_name: transition_name.to_string(),
        phase: phase_from_snapshot(transition.phase),
        effects: transition
            .effects
            .into_iter()
            .filter_map(|effect| runtime_effect_from_core(effect, applied_at_turn))
            .collect(),
    }
}

/// `None` for the stager's lineage receipt, which the owner records itself.
fn runtime_effect_from_core(
    effect: CoreSurfaceEffect,
    applied_at_turn: Option<TurnNumber>,
) -> Option<ExternalToolSurfaceEffect> {
    Some(match effect {
        CoreSurfaceEffect::ScheduleSurfaceCompletion {
            surface_id,
            operation,
            pending_task_sequence,
            staged_intent_sequence,
            applied_at_turn,
        } => ExternalToolSurfaceEffect::ScheduleSurfaceCompletion {
            surface_id: SurfaceId::from(surface_id),
            operation: surface_delta_operation_from_core(operation),
            pending_task_sequence,
            staged_intent_sequence,
            applied_at_turn: TurnNumber(applied_at_turn),
        },
        CoreSurfaceEffect::RefreshVisibleSurfaceSet { snapshot_epoch } => {
            ExternalToolSurfaceEffect::RefreshVisibleSurfaceSet { snapshot_epoch }
        }
        CoreSurfaceEffect::EmitExternalToolDelta {
            surface_id,
            operation,
            phase,
            cause,
        } => ExternalToolSurfaceEffect::EmitExternalToolDelta {
            surface_id: SurfaceId::from(surface_id),
            operation: surface_delta_operation_from_core(operation),
            phase: surface_delta_phase_from_core(phase),
            cause,
            settled: matches!(
                phase,
                ExternalToolSurfaceDeltaPhase::Applied
                    | ExternalToolSurfaceDeltaPhase::Failed
                    | ExternalToolSurfaceDeltaPhase::Forced
            ),
            applied_at_turn: applied_at_turn.unwrap_or(TurnNumber(0)),
        },
        CoreSurfaceEffect::CloseSurfaceConnection { surface_id } => {
            ExternalToolSurfaceEffect::CloseSurfaceConnection {
                surface_id: SurfaceId::from(surface_id),
            }
        }
        CoreSurfaceEffect::RejectSurfaceCall { surface_id, cause } => {
            ExternalToolSurfaceEffect::RejectSurfaceCall {
                surface_id: SurfaceId::from(surface_id),
                cause,
            }
        }
        CoreSurfaceEffect::StageRecorded { .. } => return None,
    })
}

fn runtime_surface_error(input_name: &str, error: DslTransitionError) -> ExternalToolSurfaceError {
    ExternalToolSurfaceError {
        input_name: input_name.to_string(),
        reason: error.to_string(),
    }
}

fn phase_from_snapshot(phase: ExternalToolSurfaceGlobalPhase) -> ExternalToolSurfacePhase {
    match phase {
        ExternalToolSurfaceGlobalPhase::Operating => ExternalToolSurfacePhase::Operating,
        ExternalToolSurfaceGlobalPhase::Shutdown => ExternalToolSurfacePhase::Shutdown,
    }
}

fn surface_delta_operation_from_core(
    operation: ExternalToolSurfaceDeltaOperation,
) -> SurfaceDeltaOperation {
    match operation {
        ExternalToolSurfaceDeltaOperation::None => SurfaceDeltaOperation::None,
        ExternalToolSurfaceDeltaOperation::Add => SurfaceDeltaOperation::Add,
        ExternalToolSurfaceDeltaOperation::Remove => SurfaceDeltaOperation::Remove,
        ExternalToolSurfaceDeltaOperation::Reload => SurfaceDeltaOperation::Reload,
    }
}

fn surface_delta_phase_from_core(phase: ExternalToolSurfaceDeltaPhase) -> SurfaceDeltaPhase {
    match phase {
        ExternalToolSurfaceDeltaPhase::None => SurfaceDeltaPhase::None,
        ExternalToolSurfaceDeltaPhase::Pending => SurfaceDeltaPhase::Pending,
        ExternalToolSurfaceDeltaPhase::Applied => SurfaceDeltaPhase::Applied,
        ExternalToolSurfaceDeltaPhase::Draining => SurfaceDeltaPhase::Draining,
        ExternalToolSurfaceDeltaPhase::Failed => SurfaceDeltaPhase::Failed,
        ExternalToolSurfaceDeltaPhase::Forced => SurfaceDeltaPhase::Forced,
    }
}

fn surface_base_from_snapshot(state: ExternalToolSurfaceBaseState) -> SurfaceBaseState {
    match state {
        ExternalToolSurfaceBaseState::Absent => SurfaceBaseState::Absent,
        ExternalToolSurfaceBaseState::Active => SurfaceBaseState::Active,
        ExternalToolSurfaceBaseState::Removing => SurfaceBaseState::Removing,
        ExternalToolSurfaceBaseState::Removed => SurfaceBaseState::Removed,
    }
}

fn staged_surface_op_from_snapshot(op: ExternalToolSurfaceStagedOp) -> StagedSurfaceOp {
    match op {
        ExternalToolSurfaceStagedOp::None => StagedSurfaceOp::None,
        ExternalToolSurfaceStagedOp::Add => StagedSurfaceOp::Add,
        ExternalToolSurfaceStagedOp::Remove => StagedSurfaceOp::Remove,
        ExternalToolSurfaceStagedOp::Reload => StagedSurfaceOp::Reload,
    }
}

fn instant_from_epoch_ms(epoch_ms: u64) -> Instant {
    let now_ms = McpRouter::now_ms();
    let now = Instant::now();
    if epoch_ms >= now_ms {
        now.checked_add(Duration::from_millis(epoch_ms - now_ms))
            .unwrap_or(now)
    } else {
        now.checked_sub(Duration::from_millis(now_ms - epoch_ms))
            .unwrap_or(now)
    }
}

fn snapshot_from_handle(snapshot: SurfaceDiagnosticSnapshot) -> ExternalToolSurfaceSnapshot {
    let SurfaceDiagnosticSnapshot {
        surface_phase,
        visible_surfaces,
        snapshot_epoch,
        snapshot_aligned_epoch,
        entries,
        ..
    } = snapshot;
    let entries = entries
        .into_iter()
        .map(|entry| entry_snapshot_from_handle(entry, &visible_surfaces))
        .collect();

    ExternalToolSurfaceSnapshot {
        phase: surface_phase,
        snapshot_epoch,
        snapshot_aligned_epoch,
        entries,
    }
}

fn entry_snapshot_from_handle(
    entry: SurfaceSnapshot,
    visible_surfaces: &BTreeSet<String>,
) -> ExternalToolSurfaceEntrySnapshot {
    ExternalToolSurfaceEntrySnapshot {
        visible: visible_surfaces.contains(&entry.surface_id),
        surface_id: entry.surface_id,
        // Typed cross-crate handle contract: `None` means the DSL never
        // recorded a value for this surface, so the projection defaults
        // to the `Absent` / `None` variant per the contract invariants.
        base_state: entry
            .base_state
            .unwrap_or(ExternalToolSurfaceBaseState::Absent),
        has_removal_timing: entry.removal_draining_since_ms.is_some()
            || entry.removal_timeout_at_ms.is_some()
            || entry.removal_applied_at_turn.is_some(),
        pending_op: entry.pending_op,
        staged_op: entry.staged_op,
        staged_intent_sequence: entry.staged_intent_sequence.unwrap_or(0),
        pending_task_sequence: entry.pending_task_sequence.unwrap_or(0),
        pending_lineage_sequence: entry.pending_lineage_sequence.unwrap_or(0),
        inflight_call_count: entry.inflight_calls,
        last_delta_operation: entry
            .last_delta_operation
            .unwrap_or(ExternalToolSurfaceDeltaOperation::None),
        last_delta_phase: entry
            .last_delta_phase
            .unwrap_or(ExternalToolSurfaceDeltaPhase::None),
    }
}

fn merge_snapshot_alignment(
    slot: &mut Option<SurfaceSnapshotAlignmentObligation>,
    candidate: SurfaceSnapshotAlignmentObligation,
) {
    match slot {
        Some(current) if current.snapshot_epoch >= candidate.snapshot_epoch => {}
        _ => *slot = Some(candidate),
    }
}

fn latest_snapshot_alignment(
    effects: &[ExternalToolSurfaceEffect],
) -> Option<SurfaceSnapshotAlignmentObligation> {
    let core_effects = core_surface_effects(effects);
    let mut latest = None;
    for obligation in protocol_surface_snapshot_alignment::extract_obligations(&core_effects) {
        merge_snapshot_alignment(&mut latest, obligation);
    }
    latest
}

fn core_surface_operation(op: SurfaceDeltaOperation) -> ExternalToolSurfaceDeltaOperation {
    match op {
        SurfaceDeltaOperation::None => ExternalToolSurfaceDeltaOperation::None,
        SurfaceDeltaOperation::Add => ExternalToolSurfaceDeltaOperation::Add,
        SurfaceDeltaOperation::Remove => ExternalToolSurfaceDeltaOperation::Remove,
        SurfaceDeltaOperation::Reload => ExternalToolSurfaceDeltaOperation::Reload,
    }
}

fn local_surface_operation(op: ExternalToolSurfaceDeltaOperation) -> SurfaceDeltaOperation {
    match op {
        ExternalToolSurfaceDeltaOperation::None => SurfaceDeltaOperation::None,
        ExternalToolSurfaceDeltaOperation::Add => SurfaceDeltaOperation::Add,
        ExternalToolSurfaceDeltaOperation::Remove => SurfaceDeltaOperation::Remove,
        ExternalToolSurfaceDeltaOperation::Reload => SurfaceDeltaOperation::Reload,
    }
}

/// Map a completion-obligation operation to the lifecycle-action vocabulary.
///
/// Completion obligations only exist for Add/Reload pending spawns; `Remove`
/// never spawns a pending task and `None` is unreachable on an obligation.
/// Both map to `Add` so a fault report on a malformed obligation still names
/// a concrete operation rather than being dropped.
fn obligation_lifecycle_operation(
    op: ExternalToolSurfaceDeltaOperation,
) -> ToolConfigChangeOperation {
    match op {
        ExternalToolSurfaceDeltaOperation::Reload => ToolConfigChangeOperation::Reload,
        ExternalToolSurfaceDeltaOperation::Remove => ToolConfigChangeOperation::Remove,
        ExternalToolSurfaceDeltaOperation::Add | ExternalToolSurfaceDeltaOperation::None => {
            ToolConfigChangeOperation::Add
        }
    }
}

fn core_surface_effects(effects: &[ExternalToolSurfaceEffect]) -> Vec<CoreSurfaceEffect> {
    effects
        .iter()
        .map(|effect| match effect {
            ExternalToolSurfaceEffect::ScheduleSurfaceCompletion {
                surface_id,
                operation,
                pending_task_sequence,
                staged_intent_sequence,
                applied_at_turn,
            } => CoreSurfaceEffect::ScheduleSurfaceCompletion {
                surface_id: surface_id.0.clone(),
                operation: core_surface_operation(*operation),
                pending_task_sequence: *pending_task_sequence,
                staged_intent_sequence: *staged_intent_sequence,
                applied_at_turn: applied_at_turn.0,
            },
            ExternalToolSurfaceEffect::RefreshVisibleSurfaceSet { snapshot_epoch } => {
                CoreSurfaceEffect::RefreshVisibleSurfaceSet {
                    snapshot_epoch: *snapshot_epoch,
                }
            }
            ExternalToolSurfaceEffect::EmitExternalToolDelta {
                surface_id,
                operation,
                phase,
                cause,
                ..
            } => CoreSurfaceEffect::EmitExternalToolDelta {
                surface_id: surface_id.0.clone(),
                operation: core_surface_operation(*operation),
                phase: match phase {
                    SurfaceDeltaPhase::None => ExternalToolSurfaceDeltaPhase::None,
                    SurfaceDeltaPhase::Pending => ExternalToolSurfaceDeltaPhase::Pending,
                    SurfaceDeltaPhase::Applied => ExternalToolSurfaceDeltaPhase::Applied,
                    SurfaceDeltaPhase::Draining => ExternalToolSurfaceDeltaPhase::Draining,
                    SurfaceDeltaPhase::Failed => ExternalToolSurfaceDeltaPhase::Failed,
                    SurfaceDeltaPhase::Forced => ExternalToolSurfaceDeltaPhase::Forced,
                },
                cause: *cause,
            },
            ExternalToolSurfaceEffect::CloseSurfaceConnection { surface_id } => {
                CoreSurfaceEffect::CloseSurfaceConnection {
                    surface_id: surface_id.0.clone(),
                }
            }
            ExternalToolSurfaceEffect::RejectSurfaceCall { surface_id, cause } => {
                CoreSurfaceEffect::RejectSurfaceCall {
                    surface_id: surface_id.0.clone(),
                    cause: *cause,
                }
            }
        })
        .collect()
}

fn lifecycle_operation_from_effects(
    effects: &[ExternalToolSurfaceEffect],
    expected_phase: SurfaceDeltaPhase,
) -> Option<ToolConfigChangeOperation> {
    effects.iter().find_map(|effect| match effect {
        ExternalToolSurfaceEffect::EmitExternalToolDelta {
            operation, phase, ..
        } if *phase == expected_phase => match operation {
            SurfaceDeltaOperation::Add => Some(ToolConfigChangeOperation::Add),
            SurfaceDeltaOperation::Remove => Some(ToolConfigChangeOperation::Remove),
            SurfaceDeltaOperation::Reload => Some(ToolConfigChangeOperation::Reload),
            SurfaceDeltaOperation::None => None,
        },
        _ => None,
    })
}

/// Exact connection and raw provider operation behind a projected tool name.
/// Uses the configured server name; it does not derive names from OAuth accounts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpToolRoute {
    pub server_name: String,
    pub raw_operation: String,
}

/// One operation a checked route offers now: the raw operation and the
/// server's genuine definition of it, under the name the route exposes.
#[derive(Debug, Clone)]
pub(crate) struct CheckedOperation {
    pub(crate) raw_operation: String,
    pub(crate) tool: Arc<ToolDef>,
}

/// A checked route's own conjunct for one call through it, which the
/// router's call path asks at the final transport handoff: after every
/// preparation await, immediately before the native reviewed entry.
pub(crate) trait CheckedEntry: Send + Sync {
    /// The raw operations the route admits instead of the access ceiling.
    fn operations(&self) -> &BTreeSet<String>;

    /// Reserve the single entry of the dispatched plan's occurrence through
    /// the route, refusing unless the route is still open, the dispatch is
    /// still its session's under actual bound work and a prepared
    /// authorization for exactly this call and plan, the connection is still
    /// the generation the plan names, and that occurrence never entered
    /// through the route before.
    fn reserve_entry(
        &self,
        call: ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
        connection_generation: u64,
    ) -> Result<(), ToolError>;

    /// Release a reservation whose native entry refused: nothing entered.
    fn release_entry(&self);
}

/// Every raw route excluded by one exposed-name collision.
///
/// Select distinct names using [`McpServerConfig::tool_names`] and reload the
/// affected configuration. This is a projection diagnostic, not an instruction
/// to rename or retry automatically. Divergent definitions of the same raw
/// operation are separately excluded and cannot be repaired by an alias.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpToolNameCollision {
    pub exposed_name: String,
    pub routes: Vec<McpToolRoute>,
}

impl std::fmt::Display for McpToolNameCollision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "MCP exposed name {:?} collides between ",
            self.exposed_name
        )?;
        for (index, route) in self.routes.iter().enumerate() {
            if index > 0 {
                write!(f, ", ")?;
            }
            write!(f, "({:?}, {:?})", route.server_name, route.raw_operation)?;
        }
        write!(
            f,
            "; select distinct exposed names with McpServerConfig.tool_names"
        )
    }
}

#[derive(Debug, Clone)]
struct RouterProjectionSnapshot {
    #[allow(dead_code)]
    epoch: u64,
    tool_routes: HashMap<String, McpToolRoute>,
    name_collisions: Arc<[McpToolNameCollision]>,
    catalog_entries: Arc<[ToolCatalogEntry]>,
    visible_tools: Arc<[Arc<ToolDef>]>,
}

impl Default for RouterProjectionSnapshot {
    fn default() -> Self {
        Self {
            epoch: 0,
            tool_routes: HashMap::new(),
            name_collisions: Arc::from([]),
            catalog_entries: Arc::from([]),
            visible_tools: Arc::from([]),
        }
    }
}

/// Shell-level server entry. Holds connection, config, and tools.
/// Lifecycle state is owned by the surface owner, not by this struct.
struct ServerEntry {
    config: McpServerConfig,
    connection: Option<McpConnection>,
    tools: Vec<Arc<ToolDef>>,
    /// Shell-level atomic for inflight call RAII guards. The surface owner owns
    /// the canonical count; this atomic is for the `InflightCallGuard` pattern
    /// used by the `&self` call_tool path.
    active_calls: AtomicUsize,
}

struct InflightCallGuard<'a> {
    active_calls: &'a AtomicUsize,
    owner: &'a SurfaceOwner,
    surface_id: SurfaceId,
    progress: &'a tokio::sync::watch::Sender<u64>,
    finished: bool,
}

impl<'a> InflightCallGuard<'a> {
    fn new(
        active_calls: &'a AtomicUsize,
        owner: &'a SurfaceOwner,
        surface_id: SurfaceId,
        progress: &'a tokio::sync::watch::Sender<u64>,
    ) -> Self {
        active_calls.fetch_add(1, Ordering::AcqRel);
        Self {
            active_calls,
            owner,
            surface_id,
            progress,
            finished: false,
        }
    }

    fn finish(mut self) -> Result<(), McpError> {
        self.finished = true;
        self.owner
            .apply(ExternalToolSurfaceInput::CallFinished {
                surface_id: self.surface_id.clone(),
            })
            .map_err(|error| McpError::ServerUnavailable {
                server: self.surface_id.to_string(),
                state: format!("Surface owner rejected CallFinished: {error}"),
            })?;
        Ok(())
    }
}

impl Drop for InflightCallGuard<'_> {
    fn drop(&mut self) {
        if !self.finished {
            // Cancellation cannot skip the canonical lifetime transition.
            // There is no result channel during Drop; retain a fixed diagnostic
            // if the native owner refuses this cleanup obligation.
            if self
                .owner
                .apply(ExternalToolSurfaceInput::CallFinished {
                    surface_id: self.surface_id.clone(),
                })
                .is_err()
            {
                tracing::error!("surface owner rejected cancelled MCP CallFinished");
            }
        }
        self.active_calls.fetch_sub(1, Ordering::AcqRel);
        self.progress
            .send_modify(|seen| *seen = seen.wrapping_add(1));
    }
}

/// One close an [`McpRouter`] owned, and what it observed.
#[derive(Debug, Clone)]
pub struct McpServerCloseRecord {
    pub server: String,
    pub outcome: McpServerCloseOutcome,
}

/// What [`McpRouter::shutdown_with_report`] observed: one record per close
/// the router owned. Every connect attempt's close comes from its original
/// custody through the router's close index: each established connection,
/// each pending attempt's process, each failed attempt's own close (its
/// process, or a remote attempt's connected client) and every close still
/// running from an earlier remove, reload or replace, plus each earlier close
/// of those kinds that ended unsettled. A close the router ran outside its
/// index (an attempt refused after the close began) is recorded by the
/// router itself.
#[derive(Debug)]
pub struct McpRouterShutdownReport {
    servers: Vec<McpServerCloseRecord>,
    guards: Vec<crate::McpGuardCompletionRecord>,
}

impl McpRouterShutdownReport {
    /// Every close record, in no particular order. A server can have more
    /// than one: an earlier close that ended unsettled, and its last one.
    #[must_use]
    pub fn servers(&self) -> &[McpServerCloseRecord] {
        &self.servers
    }

    /// Whether every close settled ([`McpServerCloseOutcome::is_settled`]).
    /// Guard calls do not count ([`Self::guards_completed`]).
    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.servers.iter().all(|close| close.outcome.is_settled())
    }

    /// Every attempt guard call of this router that the shutdown awaited, in
    /// no particular order: its begins (bounded by the cancel bound) and
    /// finishes (bounded by the settle bound), and the guards still held.
    /// Empty without installed guards ([`McpRouter::with_attempt_guards`]).
    #[must_use]
    pub fn guards(&self) -> &[crate::McpGuardCompletionRecord] {
        &self.guards
    }

    /// Whether every guard call the shutdown awaited was observed to end:
    /// none is [`crate::McpGuardCompletion::TimedOut`] or
    /// [`crate::McpGuardCompletion::Held`].
    #[must_use]
    pub fn guards_completed(&self) -> bool {
        self.guards.iter().all(|record| {
            !matches!(
                record.completion,
                crate::McpGuardCompletion::TimedOut | crate::McpGuardCompletion::Held
            )
        })
    }

    /// A report of exactly these records, for a host's own tests.
    #[doc(hidden)]
    #[must_use]
    pub fn from_records(records: Vec<McpServerCloseRecord>) -> Self {
        Self {
            servers: records,
            guards: Vec::new(),
        }
    }
}

/// The outcome of closing a pending connect attempt's process at shutdown:
/// the receipt of the close this call ran, or of the one it joined (a failed
/// attempt's own close included).
async fn pending_close_outcome(server: &str, custody: &StdioChildCustody) -> McpServerCloseOutcome {
    match custody.terminate_or_join().await {
        Ok(receipt) => {
            crate::connection::trace_close_receipt(server, &receipt);
            McpServerCloseOutcome::Stdio(receipt)
        }
        Err(not_held) => crate::connection::not_held_outcome(not_held),
    }
}

/// Router for MCP tool calls across multiple servers.
///
/// All lifecycle state transitions flow through the active surface owner.
/// Runtime/session routers receive a runtime-owned handle from factory wiring;
/// standalone routers use a local compatibility handle. The router manages
/// connections, async tasks, tool caching, and effect execution.
///
/// Add and reload operations are non-blocking: `apply_staged()` spawns
/// background connection tasks and returns immediately. Completions are
/// delivered via [`take_external_updates`](Self::take_external_updates).
pub struct McpRouter {
    surface_owner: SurfaceOwner,

    servers: HashMap<String, ServerEntry>,
    projection: Arc<RouterProjectionSnapshot>,
    staged_payloads: HashMap<String, McpServerConfig>,
    /// The prepared attachment control of a governed staged add, owned with
    /// its staged payload under the same router borrow: a payload and its
    /// control are installed, replaced, withdrawn and applied together, and
    /// the control travels with the exact payload to the connection's
    /// physical entry.
    staged_controls: HashMap<String, Arc<dyn McpAttachmentAuthority>>,
    /// The attempt of each governed staged add, staged and withdrawn with
    /// its control.
    staged_attempts: HashMap<String, AttemptSender>,
    /// The attempt of each scheduled governed add, keyed by its accepted
    /// completion obligation (server, pending task sequence).
    pending_attempts: HashMap<(String, u64), AttemptSender>,
    /// The typed refusal staged with each refused add's payload
    /// ([`Self::stage_add_refused`]); it moves with the payload to the
    /// boundary, where the add fails before anything about it is resolved.
    refused_payloads: HashMap<String, meerkat_core::confinement::ConfinementRefusal>,
    /// Servers attached under a governed control, and governed adds a
    /// staged removal withdrew. Their descriptor never changes or reconnects
    /// without a new control (no reload) until the removal's actual
    /// disposition at a boundary: the surface is not attached, or its
    /// removal finalized.
    governed_servers: std::collections::HashSet<String>,

    // --- Async pending infrastructure ---
    pending_tx: mpsc::Sender<PendingResult>,
    /// Poison-safe mutex wrapping the receiver.
    pending_rx: Mutex<mpsc::Receiver<PendingResult>>,
    pending_obligations: HashMap<String, SurfaceCompletionObligation>,
    /// Background connect-and-enumerate tasks this router spawned. The router
    /// owns them: `shutdown` aborts and joins every one still running, so no
    /// connect attempt outlives the router. Finished tasks are reaped as new
    /// ones are spawned.
    connect_tasks: tokio::task::JoinSet<()>,
    /// Custody of the stdio server process of every connect attempt whose
    /// result the router has not processed yet, keyed by surface and pending
    /// task sequence. The process is deposited here before its handshake, so
    /// `shutdown` terminates it and observes its exit even when the attempt is
    /// aborted mid-handshake.
    pending_child_custody: HashMap<(String, u64), StdioChildCustody>,
    /// The setup diagnostics of every stdio connect attempt whose result
    /// the router has not processed yet, keyed like
    /// `pending_child_custody`: their only strong owner while in flight.
    pending_stdio_diagnostics:
        HashMap<(String, u64), Arc<crate::stdio_diagnostics::StdioAttemptRecord>>,
    /// Host status: per server, the latest accepted stdio setup failure the
    /// transport classified, with its stderr tail. At most one per server,
    /// from its accepted failure until the next attempt for the server is
    /// admitted, the next accepted completion, its removal, its
    /// connection's close, shutdown or the router's drop. Never projected
    /// to the agent.
    failed_stdio_attempts: HashMap<String, RetainedStdioFailure>,
    /// Connection closes the router started (remove, reload, replace, and
    /// rejected completions). Finished closes are reaped on each effect pass;
    /// `shutdown` joins the rest, so every closed server has exited by the
    /// time it returns. Dropping the router without `shutdown` aborts them;
    /// the custody's drop fallback then kills each server without observing
    /// its exit.
    closing: tokio::task::JoinSet<McpServerCloseRecord>,
    /// The server each running close task is closing, by task id, so a task
    /// that ends without reporting is still attributed.
    closing_servers: HashMap<tokio::task::Id, String>,
    /// Running close tasks whose close the close index holds: the index
    /// reports that close, so the task's own record is not kept.
    indexed_closes: std::collections::HashSet<tokio::task::Id>,
    /// Closes outside the close index reaped before shutdown that did not
    /// settle, kept for the shutdown report.
    unsettled_closes: Vec<McpServerCloseRecord>,
    /// Every connect attempt's custodies, from the attempt's admission,
    /// shared with the adapter outside the router lock.
    close_index: crate::connection::CloseIndex,
    pending_snapshot_alignment: Option<SurfaceSnapshotAlignmentObligation>,
    /// Queued canonical lifecycle deltas for async completions.
    completed_updates: VecDeque<CompletedLifecycleUpdate>,
    /// Host-channel status: servers whose latest connection attempt ended in
    /// [`McpError::AuthorizationRequired`]. Never projected to the agent.
    awaiting_authorization: BTreeMap<String, AwaitingAuthorization>,
    /// Connected servers a call found without a usable credential
    /// ([`McpError::AuthorizationRequired`] at the call), with the exact
    /// target the call reported. Calls only hold `&self`, so the next
    /// boundary ([`Self::apply_staged`]) releases them before applying.
    calls_refused_unauthorized: Mutex<Vec<(String, McpServerIdentity)>>,
    /// Optional session-scoped MCP server lifecycle handle
    /// (Phase 5G / T5g). When bound, every handshake event mirrors into
    /// the session's MeerkatMachine DSL `mcp_server_states`. Standalone
    /// callers (tests, fixtures) leave this `None` and the DSL mirror is
    /// skipped — shell-level behavior stays identical.
    ///
    /// Stored behind `Arc<StdRwLock<...>>` so the adapter (which wraps the
    /// router in a `tokio` async lock) can bind the handle late — after
    /// construction, via a sync trait method — without needing to block on
    /// the async router lock.
    mcp_lifecycle_handle: Arc<StdRwLock<Option<Arc<dyn McpServerLifecycleHandle>>>>,
    mcp_auth_mode: McpAuthMode,
    mcp_auth_resolver: Option<Arc<dyn McpAuthResolver>>,
    client_service_factory: Option<Arc<dyn crate::McpClientServiceFactory>>,
    /// Immutable for this router, including all later add and reload attempts.
    stdio_launch_profile: crate::McpStdioLaunchProfile,
    /// The host's HTTP egress profile, retained by every later add, reload
    /// and restore attempt of a remote server.
    http_egress_profile: crate::HttpEgressProfile,
    call_context_provider: Option<Arc<dyn crate::McpCallContextProvider>>,
    /// Close policy each stdio connection captures when it spawns its server.
    stdio_close_bounds: crate::McpStdioCloseBounds,
    /// Bumped whenever the router makes progress a waiter could be blocked
    /// on: a tool call finishing (a draining server's in-flight count drops)
    /// or a connect attempt delivering its result. [`McpProgressWait`]
    /// subscribes before reading state, so no progress is missed.
    progress: Arc<tokio::sync::watch::Sender<u64>>,
    /// Connect attempts spawned, and those that have delivered their result
    /// to the pending channel (counted by the attempt before it signals
    /// `progress`).
    connect_attempts_spawned: u64,
    connect_results_delivered: Arc<std::sync::atomic::AtomicU64>,
    /// This router construction's identity.
    construction: crate::McpConstructionId,
    /// The host's attempt guards and this router's part of them, when
    /// installed ([`Self::with_attempt_guards`]).
    attempt_guards: Option<Arc<crate::attempt_guard::RouterGuards>>,
    /// Set by [`Self::shutdown_with_report`], so the router's drop does not
    /// treat a shut-down router as dropped.
    shut_down: bool,
}

impl Drop for McpRouter {
    /// A router dropped without its shutdown cancels its attempts' guards as
    /// `RouterDropped`; their calls stay tracked by the installation, which
    /// [`crate::McpAttemptGuards::close_and_join`] joins.
    fn drop(&mut self) {
        if !self.shut_down
            && let Some(guards) = &self.attempt_guards
        {
            guards.close(crate::McpAttemptCancelCause::RouterDropped);
        }
    }
}

/// A typed wait for MCP router progress, taken under the router lock and
/// awaited without it: it resolves when a tool call finishes or a connect
/// result arrives after it was taken, at its deadline (the earliest removal
/// timeout of a draining server), or at once when progress is already
/// possible.
#[derive(Debug)]
pub(crate) struct McpProgressWait {
    progress: Option<tokio::sync::watch::Receiver<u64>>,
    deadline: Option<Instant>,
}

impl McpProgressWait {
    fn ready() -> Self {
        Self {
            progress: None,
            deadline: None,
        }
    }

    /// Wait for progress, the wait's own deadline, or `limit`, whichever
    /// comes first.
    pub(crate) async fn wait(self, limit: Option<tokio::time::Instant>) {
        let Some(mut progress) = self.progress else {
            return;
        };
        let deadline = match (self.deadline.map(tokio::time::Instant::from_std), limit) {
            (Some(own), Some(limit)) => Some(own.min(limit)),
            (own, limit) => own.or(limit),
        };
        let changed = progress.changed();
        match deadline {
            Some(deadline) => {
                tokio::select! {
                    _ = changed => {}
                    () = tokio::time::sleep_until(deadline) => {}
                }
            }
            None => {
                let _ = changed.await;
            }
        }
    }
}

impl McpRouter {
    /// The exact operation `tool_name` routes to now: its registered server,
    /// that server's current physical connection generation, the raw
    /// operation, the connector approval of the descriptor that connection
    /// was opened from and the account the connection acts with. `None` when the name routes nowhere or the server has no
    /// live connection. A nonsecret fact for the operation policy owner; no
    /// configuration or credential leaves the router.
    /// What this router exposes now for one exact raw operation of `server`
    /// (see [`meerkat_core::McpOperationObservation`]), from one read of the
    /// current projection, the server's current connection and its accepted
    /// surface state. A current route on a live connection is callable even
    /// while a newer replacement is staged; a staged or connecting server
    /// without one is pending, never callable. Nothing here is a permit.
    pub fn observe_operation(
        &self,
        server: &str,
        raw_operation: &str,
    ) -> meerkat_core::McpOperationObservation {
        use meerkat_core::McpOperationObservation as Observation;
        let routed = self
            .projection
            .tool_routes
            .values()
            .any(|route| route.server_name == server && route.raw_operation == raw_operation);
        if routed
            && let Some(connection) = self
                .servers
                .get(server)
                .and_then(|entry| entry.connection.as_ref())
        {
            return Observation::Callable {
                connection_generation: connection.connection_id().generation(),
                connector: connection.config().connector.as_ref().map(|binding| {
                    meerkat_core::McpConnectorIdentity {
                        realm_id: binding.realm_id.clone(),
                        slot_id: binding.slot_id.clone(),
                        issuer: binding.issuer.clone(),
                        client: binding.client.clone(),
                        account: binding.account.clone(),
                    }
                }),
            };
        }
        let Some(snapshot) = self.surface_owner.surface_snapshot(server) else {
            return Observation::Absent;
        };
        if snapshot.pending_op != meerkat_core::ExternalToolSurfacePendingOp::None
            || snapshot.staged_op != meerkat_core::ExternalToolSurfaceStagedOp::None
        {
            return Observation::Pending;
        }
        if snapshot.base_state != Some(meerkat_core::ExternalToolSurfaceBaseState::Active)
            && snapshot.last_delta_phase
                == Some(meerkat_core::ExternalToolSurfaceDeltaPhase::Failed)
        {
            return Observation::Failed;
        }
        Observation::Absent
    }

    /// What `tool` of `server` is now on exactly the connection generation an
    /// accepted attempt installed: the server's current connection must be
    /// that generation, and the published routes must serve the tool on it.
    pub(crate) fn installed_operation(
        &self,
        server: &str,
        connection_generation: u64,
        tool: &str,
    ) -> InstalledOperation {
        let current = self
            .servers
            .get(server)
            .and_then(|entry| entry.connection.as_ref())
            .is_some_and(|connection| {
                connection.connection_id().generation() == connection_generation
            });
        if !current {
            return InstalledOperation::Replaced;
        }
        if self
            .projection
            .tool_routes
            .values()
            .any(|route| route.server_name == server && route.raw_operation == tool)
        {
            InstalledOperation::Callable
        } else {
            InstalledOperation::NotRouted
        }
    }

    /// Process connection results that have arrived, as lifecycle polling
    /// does, while leaving their notices queued for the normal delivery.
    pub(crate) fn progress_pending(&mut self) {
        self.drain_pending();
    }

    pub(crate) fn operation_target(
        &self,
        tool_name: &str,
    ) -> Option<meerkat_core::ToolOperationTarget> {
        let route = self.projection.tool_routes.get(tool_name)?;
        let connection = self.servers.get(&route.server_name)?.connection.as_ref()?;
        Some(meerkat_core::ToolOperationTarget::McpServerOperation {
            server: route.server_name.clone(),
            connection_generation: connection.connection_id().generation(),
            raw_operation: route.raw_operation.clone(),
            connector: connection.config().connector_approval(),
            account: Arc::clone(connection.operation_account()),
        })
    }

    /// The operations a checked route to `server` offers now: of exactly
    /// `operations`, the genuine definitions the server lists on its current
    /// connection, each under the server's configured exposed name (its raw
    /// name otherwise). Like the projection, an operation the server lists
    /// with divergent definitions, one whose provenance names another
    /// source, and an exposed name two of them share are withheld; so is an
    /// exposed name the ordinary projection already routes. Nothing is
    /// offered while the server is not visible or has no current
    /// connection. The access ceiling is not consulted: the route's own
    /// selection is its admission.
    pub(crate) fn checked_operations(
        &self,
        server: &str,
        operations: &BTreeSet<String>,
    ) -> Vec<CheckedOperation> {
        let visible = self
            .surface_owner
            .visible_surfaces()
            .into_iter()
            .any(|sid| sid.0 == server);
        let Some(entry) = self.servers.get(server) else {
            return Vec::new();
        };
        if !visible || entry.connection.is_none() {
            return Vec::new();
        }
        let mut listed: BTreeMap<&str, &Arc<ToolDef>> = BTreeMap::new();
        let mut divergent: BTreeSet<&str> = BTreeSet::new();
        for tool in &entry.tools {
            let raw = tool.name.as_str();
            if !operations.contains(raw) {
                continue;
            }
            match listed.get(raw) {
                Some(existing) if !Self::tool_definitions_equivalent(existing, tool) => {
                    divergent.insert(raw);
                }
                Some(_) => {}
                None => {
                    listed.insert(raw, tool);
                }
            }
        }
        let mut offered: BTreeMap<String, Option<CheckedOperation>> = BTreeMap::new();
        for (raw, tool) in listed {
            if divergent.contains(raw)
                || tool.provenance.as_ref().is_some_and(|source| {
                    source.kind != meerkat_core::types::ToolSourceKind::Mcp
                        || source.source_id.as_str() != server
                })
            {
                continue;
            }
            let exposed = entry.config.tool_names.get(raw).map_or(raw, String::as_str);
            if self.projection.tool_routes.contains_key(exposed) {
                continue;
            }
            let tool = if exposed == tool.name.as_str() {
                Arc::clone(tool)
            } else {
                let mut renamed = (**tool).clone();
                renamed.name = exposed.into();
                Arc::new(renamed)
            };
            offered
                .entry(exposed.to_string())
                .and_modify(|shared| *shared = None)
                .or_insert(Some(CheckedOperation {
                    raw_operation: raw.to_string(),
                    tool,
                }));
        }
        offered.into_values().flatten().collect()
    }

    /// The operation target of `server`'s `raw_operation` reached through
    /// the checked route `route`, on the server's current connection.
    pub(crate) fn checked_operation_target(
        &self,
        route: &str,
        server: &str,
        raw_operation: &str,
    ) -> Option<meerkat_core::ToolOperationTarget> {
        let connection = self.servers.get(server)?.connection.as_ref()?;
        Some(
            meerkat_core::ToolOperationTarget::CheckedMcpServerOperation {
                route: route.to_string(),
                server: server.to_string(),
                connection_generation: connection.connection_id().generation(),
                raw_operation: raw_operation.to_string(),
                connector: connection.config().connector_approval(),
                account: Arc::clone(connection.operation_account()),
            },
        )
    }

    /// The declared mutation semantics of `server`'s `raw_operation`, from
    /// the call-context provider for its current connection.
    pub(crate) fn checked_mutation_class(
        &self,
        server: &str,
        raw_operation: &str,
    ) -> meerkat_core::ToolMutationClass {
        let Some(connection) = self
            .servers
            .get(server)
            .and_then(|entry| entry.connection.as_ref())
        else {
            return meerkat_core::ToolMutationClass::Unknown;
        };
        self.call_context_provider
            .as_ref()
            .map_or(meerkat_core::ToolMutationClass::Unknown, |provider| {
                provider.tool_mutation_class(connection.config(), raw_operation)
            })
    }

    /// Dispatch `call` as `server`'s `raw_operation` through the checked
    /// route that owns `entry`: the ordinary call path (call accounting,
    /// call-context and connector preparation, the reviewed entry at the
    /// transport handoff), admitted by the route's selection instead of the
    /// access ceiling, with the route's own conjunct reserved at that
    /// handoff.
    pub(crate) async fn dispatch_checked(
        &self,
        entry: &dyn CheckedEntry,
        server: &str,
        raw_operation: &str,
        call: ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
        if context.read_only_execution_required()
            && self.checked_mutation_class(server, raw_operation)
                != meerkat_core::ToolMutationClass::ReadOnly
        {
            return Err(ToolError::access_denied(call.name));
        }
        let args = meerkat_core::ToolCallArguments::from_raw_json(call.args)
            .map_err(|err| ToolError::invalid_arguments(call.name, err.to_string()))?;
        let route = McpToolRoute {
            server_name: server.to_string(),
            raw_operation: raw_operation.to_string(),
        };
        let result = self
            .call_route_with_context(
                &route,
                Some(entry),
                call,
                args.as_value(),
                context,
                |result| {
                    let (blocks, is_error) =
                        crate::protocol::project_tool_result(result, call.name)?;
                    Ok(ToolResult::with_blocks(
                        call.id.to_string(),
                        blocks,
                        is_error,
                    ))
                },
            )
            .await
            .map_err(|error| tool_call_error(call.name, error))?;
        Ok(result.into())
    }

    fn with_surface_owner(surface_owner: SurfaceOwner) -> Self {
        Self::with_surface_owner_and_stdio_profile(
            surface_owner,
            crate::McpStdioLaunchProfile::trusted_host(),
        )
    }

    fn with_surface_owner_and_stdio_profile(
        surface_owner: SurfaceOwner,
        stdio_launch_profile: crate::McpStdioLaunchProfile,
    ) -> Self {
        let (tx, rx) = mpsc::channel(PENDING_CHANNEL_CAPACITY);
        Self {
            surface_owner,
            servers: HashMap::new(),
            projection: Arc::new(RouterProjectionSnapshot::default()),
            staged_payloads: HashMap::new(),
            staged_controls: HashMap::new(),
            staged_attempts: HashMap::new(),
            pending_attempts: HashMap::new(),
            refused_payloads: HashMap::new(),
            governed_servers: std::collections::HashSet::new(),
            pending_tx: tx,
            pending_rx: Mutex::new(rx),
            pending_obligations: HashMap::new(),
            connect_tasks: tokio::task::JoinSet::new(),
            pending_child_custody: HashMap::new(),
            pending_stdio_diagnostics: HashMap::new(),
            failed_stdio_attempts: HashMap::new(),
            closing: tokio::task::JoinSet::new(),
            closing_servers: HashMap::new(),
            indexed_closes: std::collections::HashSet::new(),
            unsettled_closes: Vec::new(),
            close_index: crate::connection::CloseIndex::default(),
            pending_snapshot_alignment: None,
            completed_updates: VecDeque::new(),
            awaiting_authorization: BTreeMap::new(),
            calls_refused_unauthorized: Mutex::new(Vec::new()),
            mcp_lifecycle_handle: Arc::new(StdRwLock::new(None)),
            mcp_auth_mode: McpAuthMode::Stored,
            mcp_auth_resolver: None,
            client_service_factory: None,
            stdio_launch_profile,
            http_egress_profile: crate::HttpEgressProfile::trusted_host(),
            call_context_provider: None,
            stdio_close_bounds: crate::McpStdioCloseBounds::DEFAULT,
            progress: Arc::new(tokio::sync::watch::Sender::new(0)),
            connect_attempts_spawned: 0,
            connect_results_delivered: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            construction: crate::McpConstructionId::allocate(),
            attempt_guards: None,
            shut_down: false,
        }
    }

    /// Return the shared handle slot for late-binding a session's MCP server
    /// lifecycle DSL handle (Phase 5G / T5g). The adapter (which wraps the
    /// router in a tokio async lock) calls this once at router construction
    /// to obtain a clone it can write into from the sync
    /// `AgentToolDispatcher::bind_mcp_server_lifecycle_handle` method.
    pub fn mcp_lifecycle_handle_slot(
        &self,
    ) -> Arc<StdRwLock<Option<Arc<dyn McpServerLifecycleHandle>>>> {
        Arc::clone(&self.mcp_lifecycle_handle)
    }

    /// Return the shared runtime surface-handle slot. Runtime-backed session
    /// builds replace the router's default ephemeral DSL owner with the
    /// session-owned MeerkatMachine handle through this slot.
    pub fn external_surface_handle_slot(
        &self,
    ) -> Option<Arc<StdRwLock<Arc<dyn ExternalToolSurfaceHandle>>>> {
        self.surface_owner.runtime_handle_slot()
    }

    /// Route a lifecycle transition into the bound session DSL mirror.
    ///
    /// K14: rejected applies are typed faults, not debug-swallowed log lines.
    /// Synchronous ingress paths (`stage_add`, `stage_reload`) propagate the
    /// `Err` directly; async completion paths convert it into a `Failed`
    /// lifecycle action on the canonical action channel. With no handle bound
    /// (standalone routers, tests) the mirror is absent by construction and
    /// the apply is `Ok`.
    fn with_lifecycle_handle<F>(&self, server_name: &str, f: F) -> Result<(), McpError>
    where
        F: FnOnce(&dyn McpServerLifecycleHandle) -> Result<(), DslTransitionError>,
    {
        let guard = match self.mcp_lifecycle_handle.read() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::warn!(
                    "mcp_lifecycle_handle RwLock poisoned; recovering — DSL mirror may drift"
                );
                poisoned.into_inner()
            }
        };
        match guard.as_deref() {
            None => Ok(()),
            Some(handle) => f(handle).map_err(|source| McpError::LifecycleMirrorRejected {
                server: server_name.to_string(),
                source,
            }),
        }
    }

    fn notify_lifecycle_connect_pending(&self, server_name: &str) -> Result<(), McpError> {
        self.with_lifecycle_handle(server_name, |handle| {
            handle.apply_connect_pending(server_name)
        })
    }

    fn notify_lifecycle_connected(&self, server_name: &str) -> Result<(), McpError> {
        self.with_lifecycle_handle(server_name, |handle| handle.apply_connected(server_name))
    }

    fn notify_lifecycle_failed(&self, server_name: &str, failure: &str) -> Result<(), McpError> {
        self.with_lifecycle_handle(server_name, |handle| {
            handle.apply_failed(server_name, failure)
        })
    }

    fn notify_lifecycle_disconnected(&self, server_name: &str) -> Result<(), McpError> {
        self.with_lifecycle_handle(server_name, |handle| handle.apply_disconnected(server_name))
    }

    fn notify_lifecycle_reload(&self, server_name: &str) -> Result<(), McpError> {
        self.with_lifecycle_handle(server_name, |handle| handle.apply_reload(server_name))
    }

    /// Create a new empty router without a generated surface authority, whose
    /// local stdio servers launch as the trusted host, with no OS
    /// confinement.
    ///
    /// Lifecycle mutations fail closed until runtime/session construction binds
    /// a generated [`ExternalToolSurfaceHandle`] through
    /// [`Self::new_with_surface_handle_and_stdio_profile`] or late adapter
    /// binding.
    #[deprecated(
        since = "0.9.2",
        note = "launches local stdio servers as the trusted host without naming it; name the profile with `McpRouter::new_with_stdio_profile(McpStdioLaunchProfile::trusted_host())`, or pass the host's"
    )]
    pub fn new() -> Self {
        Self::new_with_stdio_profile(crate::McpStdioLaunchProfile::trusted_host())
    }

    /// [`Self::new`] with the host's immutable requirement for every local
    /// process (see [`Self::new_with_surface_handle_and_stdio_profile`]),
    /// retained when a generated surface handle is bound later.
    pub fn new_with_stdio_profile(profile: crate::McpStdioLaunchProfile) -> Self {
        let surface_handle: Arc<dyn ExternalToolSurfaceHandle> =
            Arc::new(UnboundExternalToolSurfaceHandle::new());
        Self::with_surface_owner_and_stdio_profile(SurfaceOwner::runtime(surface_handle), profile)
    }

    /// Create a new empty router with a runtime-backed surface handle, whose
    /// local stdio servers launch as the trusted host, with no OS
    /// confinement.
    #[deprecated(
        since = "0.9.2",
        note = "launches local stdio servers as the trusted host without naming it; name the profile with `McpRouter::new_with_surface_handle_and_stdio_profile(handle, McpStdioLaunchProfile::trusted_host())`, or pass the host's"
    )]
    pub fn new_with_surface_handle(surface_handle: Arc<dyn ExternalToolSurfaceHandle>) -> Self {
        Self::with_surface_owner(SurfaceOwner::runtime(surface_handle))
    }

    /// Create a router with an immutable host requirement for every local
    /// process. It cannot be downgraded after activation or through reload.
    /// The profile neither authorizes callers nor confines remote servers.
    pub fn new_with_surface_handle_and_stdio_profile(
        surface_handle: Arc<dyn ExternalToolSurfaceHandle>,
        profile: crate::McpStdioLaunchProfile,
    ) -> Self {
        Self::with_surface_owner_and_stdio_profile(SurfaceOwner::runtime(surface_handle), profile)
    }

    /// Declare how this router holds its surface handle. A supplied handle
    /// is `Borrowed` unless the caller created it for this router alone.
    pub fn with_surface_ownership(mut self, ownership: McpSurfaceOwnership) -> Self {
        self.surface_owner.set_ownership(ownership);
        self
    }

    /// How this router holds its surface handle.
    pub fn surface_ownership(&self) -> McpSurfaceOwnership {
        self.surface_owner.ownership()
    }

    /// The lineages this router staged, shared so a late surface rebind can
    /// replace them with the lineages it re-derives on the new surface.
    pub fn held_surface_lineages(&self) -> HeldSurfaceLineages {
        self.surface_owner.held_lineages()
    }

    /// How this router holds its surface, shared so a late surface rebind
    /// onto another owner's surface makes the router `Borrowed`.
    pub fn surface_ownership_slot(&self) -> SurfaceOwnershipSlot {
        self.surface_owner.ownership_slot()
    }

    pub fn with_mcp_auth(
        mut self,
        mode: McpAuthMode,
        resolver: Option<Arc<dyn McpAuthResolver>>,
    ) -> Self {
        self.mcp_auth_mode = mode;
        self.mcp_auth_resolver = resolver;
        self
    }

    /// Install the host's HTTP egress profile for every remote server this
    /// router connects, including all later add, reload and restore
    /// attempts. A required profile is an execution boundary, not a
    /// permission: each refused destination fails its own connection with
    /// the typed [`McpError::EgressRefused`]; the session continues. Stdio
    /// servers are governed by the launch profile only.
    pub fn with_http_egress_profile(mut self, profile: crate::HttpEgressProfile) -> Self {
        self.http_egress_profile = profile;
        self
    }

    /// Select an optional host form-elicitation service for each exact native
    /// connection attempt. This does not configure AgentFactory or SDK surfaces.
    pub fn with_client_service_factory(
        mut self,
        factory: Arc<dyn crate::McpClientServiceFactory>,
    ) -> Self {
        self.client_service_factory = Some(factory);
        self
    }

    /// Install trusted per-call preparation for exact connected destinations.
    pub fn with_call_context_provider(
        mut self,
        provider: Arc<dyn crate::McpCallContextProvider>,
    ) -> Self {
        self.call_context_provider = Some(provider);
        self
    }

    /// Set how long stdio servers get to exit when their connection closes
    /// (remove, reload, replace, shutdown). Each connection captures the
    /// bounds when it spawns its server, so this applies to connections
    /// started afterwards.
    pub fn with_stdio_close_bounds(mut self, bounds: crate::McpStdioCloseBounds) -> Self {
        self.stdio_close_bounds = bounds;
        self
    }

    /// Guard every stdio connect attempt of this router with the host's
    /// installation, for `owner` (see [`crate::McpAttemptGuards`]). Before
    /// each attempt's secret resolution, physical entry or spawn, the
    /// installation's factory begins the attempt's guard; it is finished
    /// exactly once, after both the attempt's open outcome and its process
    /// end are known. Only stdio attempts are guarded: remote (HTTP and
    /// SSE) attempts are neither guarded nor refused by it.
    ///
    /// A router takes one installation: a second call is refused (it
    /// changes nothing, and the first installation and owner stand).
    #[must_use]
    pub fn with_attempt_guards(
        mut self,
        guards: crate::McpAttemptGuards,
        owner: crate::McpConstructionOwner,
    ) -> Self {
        if self.attempt_guards.is_some() {
            tracing::warn!(
                "MCP attempt guards are already installed on this router; the first installation stands"
            );
            return self;
        }
        let guards = crate::attempt_guard::RouterGuards::new(&guards, self.construction, owner);
        self.close_index.set_attempt_guards(&guards);
        self.attempt_guards = Some(guards);
        self
    }

    /// This router construction's identity: the
    /// [`crate::McpAttemptKey::construction`] of its attempts.
    #[must_use]
    pub fn construction_id(&self) -> crate::McpConstructionId {
        self.construction
    }

    /// Create a new router with a custom remove-drain timeout, whose local
    /// stdio servers launch as the trusted host, with no OS confinement.
    #[deprecated(
        since = "0.9.2",
        note = "launches local stdio servers as the trusted host without naming it; name the profile with `McpRouter::new_with_stdio_profile(McpStdioLaunchProfile::trusted_host())`, or pass the host's, and call `set_removal_timeout`"
    )]
    pub fn new_with_removal_timeout(removal_timeout: Duration) -> Self {
        let mut router = Self::new_with_stdio_profile(crate::McpStdioLaunchProfile::trusted_host());
        if let Err(error) = router.set_removal_timeout(removal_timeout) {
            tracing::warn!(
                timeout_ms = removal_timeout.as_millis(),
                error = %error,
                "Surface owner rejected set_removal_timeout during router construction"
            );
        }
        router
    }

    /// Create a new router with a custom remove-drain timeout and
    /// runtime-backed surface handle, whose local stdio servers launch as the
    /// trusted host, with no OS confinement.
    #[deprecated(
        since = "0.9.2",
        note = "launches local stdio servers as the trusted host without naming it; name the profile with `McpRouter::new_with_surface_handle_and_stdio_profile(handle, McpStdioLaunchProfile::trusted_host())`, or pass the host's, and call `set_removal_timeout`"
    )]
    pub fn new_with_surface_handle_and_removal_timeout(
        surface_handle: Arc<dyn ExternalToolSurfaceHandle>,
        removal_timeout: Duration,
    ) -> Self {
        let mut router = Self::new_with_surface_handle_and_stdio_profile(
            surface_handle,
            crate::McpStdioLaunchProfile::trusted_host(),
        );
        if let Err(error) = router.set_removal_timeout(removal_timeout) {
            tracing::warn!(
                timeout_ms = removal_timeout.as_millis(),
                error = %error,
                "Surface owner rejected set_removal_timeout during router construction"
            );
        }
        router
    }

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
            .unwrap_or(0)
    }

    fn turn_number_from_staged_sequence(&self, staged_sequence: u64) -> TurnNumber {
        if staged_sequence > 0 {
            TurnNumber(staged_sequence)
        } else {
            // Defensive fallback: staged intents should always have non-zero sequence.
            TurnNumber(self.surface_owner.snapshot_epoch())
        }
    }

    fn turn_number_from_snapshot_epoch(&self) -> TurnNumber {
        TurnNumber(self.surface_owner.snapshot_epoch())
    }

    fn record_snapshot_alignment(
        &mut self,
        snapshot_alignment: Option<SurfaceSnapshotAlignmentObligation>,
    ) {
        if let Some(snapshot_alignment) = snapshot_alignment {
            merge_snapshot_alignment(&mut self.pending_snapshot_alignment, snapshot_alignment);
        }
    }

    /// Override remove-drain timeout.
    pub fn set_removal_timeout(
        &mut self,
        removal_timeout: Duration,
    ) -> Result<(), ExternalToolSurfaceError> {
        self.surface_owner.set_removal_timeout(removal_timeout)
    }

    /// Snapshot the live external tool-surface state.
    pub fn external_tool_surface_snapshot(&self) -> meerkat_core::ExternalToolSurfaceSnapshot {
        self.surface_owner.diagnostic_snapshot()
    }

    /// Backward-compatible immediate add path.
    ///
    /// Prefer [`stage_add`](Self::stage_add) + [`apply_staged`](Self::apply_staged)
    /// for boundary semantics. A governed name is refused here as in
    /// [`stage_add`](Self::stage_add): this path carries no control.
    pub async fn add_server(&mut self, config: McpServerConfig) -> Result<(), McpError> {
        self.refuse_ungoverned_change(&config.name)?;
        self.install_active_server(config).await?;
        let _ = self.publish_projection_snapshot();
        Ok(())
    }

    /// Stage a server add for the next boundary apply.
    pub fn stage_add(&mut self, config: McpServerConfig) -> Result<(), McpError> {
        // A governed name changes only under a new control.
        self.refuse_ungoverned_change(&config.name)?;
        self.stage_add_payload(config)
    }

    /// Stage a governed server add under its prepared attachment control.
    ///
    /// The payload and the control are installed together, replacing any
    /// earlier staged payload and control for the name; the control must be
    /// for this exact descriptor. It is asked once, at the connection's
    /// physical entry, after any preparation wait.
    pub fn stage_add_governed(
        &mut self,
        config: McpServerConfig,
        control: Arc<dyn McpAttachmentAuthority>,
    ) -> Result<(), McpError> {
        self.stage_add_governed_attempt(config, control).map(|_| ())
    }

    /// [`Self::stage_add_governed`], returning this add's own attempt.
    pub(crate) fn stage_add_governed_attempt(
        &mut self,
        config: McpServerConfig,
        control: Arc<dyn McpAttachmentAuthority>,
    ) -> Result<AttemptReceiver, McpError> {
        if control.server() != &config {
            return Err(McpError::AttachmentRefused(McpAttachmentRefusal::Mismatch));
        }
        let name = config.name.clone();
        self.stage_add_payload(config)?;
        self.staged_controls.insert(name.clone(), control);
        // This add's own attempt; one it replaces before the boundary is
        // superseded.
        let (attempt, observed) = tokio::sync::watch::channel(AttemptState::Pending);
        resolve_attempt(
            self.staged_attempts.insert(name, attempt).as_ref(),
            AttemptState::Superseded,
        );
        Ok(observed)
    }

    /// Stage an add the host refuses: its definition comes from a source the
    /// host does not trust (for example an `mcp.toml` a confined process can
    /// write). It follows an ordinary add's lifecycle and fails at its
    /// boundary with `refusal`, the model seeing the ordinary failed-add
    /// notice; nothing about it is authorized, connected or spawned.
    pub fn stage_add_refused(
        &mut self,
        config: McpServerConfig,
        refusal: meerkat_core::confinement::ConfinementRefusal,
    ) -> Result<(), McpError> {
        self.refuse_ungoverned_change(&config.name)?;
        let name = config.name.clone();
        // A refused definition is never connected, so it is not validated:
        // one that would not validate is reported refused like any other,
        // and never fails the rest of the session's setup.
        self.stage_payload(config)?;
        self.refused_payloads.insert(name, refusal);
        Ok(())
    }

    fn refuse_ungoverned_change(&self, server_name: &str) -> Result<(), McpError> {
        if self.staged_controls.contains_key(server_name)
            || self.governed_servers.contains(server_name)
        {
            return Err(McpError::AttachmentRefused(McpAttachmentRefusal::Mismatch));
        }
        Ok(())
    }

    fn stage_add_payload(&mut self, config: McpServerConfig) -> Result<(), McpError> {
        Self::validate_tool_names(&config)?;
        self.stage_payload(config)
    }

    /// Stage `config`'s add without validating it.
    fn stage_payload(&mut self, config: McpServerConfig) -> Result<(), McpError> {
        let server_name = config.name.clone();
        // A new payload replaces any refused one staged under the name.
        self.refused_payloads.remove(&server_name);
        let sid = SurfaceId::from(server_name.as_str());
        match self
            .surface_owner
            .apply(ExternalToolSurfaceInput::StageAdd { surface_id: sid })
        {
            Ok(_) => {
                // K14: the lifecycle DSL mirror must accept the transition
                // before any shell mirror mutation. On rejection the staged
                // payload is NOT installed and the typed fault propagates;
                // the owner-side staged intent then fails explicitly at
                // `apply_staged` with a missing-payload protocol error.
                self.notify_lifecycle_connect_pending(&server_name)?;
                self.staged_payloads.insert(server_name, config);
                Ok(())
            }
            Err(error) => {
                tracing::warn!(
                    server = %server_name,
                    error = %error,
                    "Surface owner rejected StageAdd"
                );
                Err(error.into())
            }
        }
    }

    /// Stage a server remove intent for the next boundary apply.
    ///
    /// This only records intent. Removal lifecycle starts on `apply_staged`.
    pub fn stage_remove(&mut self, server_name: impl Into<String>) -> Result<(), McpError> {
        let server_name = server_name.into();
        let sid = SurfaceId::from(server_name.as_str());
        if let Err(error) = self
            .surface_owner
            .apply(ExternalToolSurfaceInput::StageRemove { surface_id: sid })
        {
            tracing::warn!(
                server = %server_name,
                error = %error,
                "Surface owner rejected StageRemove"
            );
            return Err(error.into());
        }
        self.staged_payloads.remove(&server_name);
        self.refused_payloads.remove(&server_name);
        // A removal withdraws a staged governed add with its control. The
        // removal is only intent and can itself be replaced before apply, so
        // the name stays governed until its actual disposition.
        if self.staged_controls.remove(&server_name).is_some() {
            self.governed_servers.insert(server_name.clone());
        }
        resolve_attempt(
            self.staged_attempts.remove(&server_name).as_ref(),
            AttemptState::Superseded,
        );
        // An accepted removal withdraws any pending human-authorization ask.
        self.awaiting_authorization.remove(&server_name);
        // And releases the server's retained setup failure and stderr tail.
        self.failed_stdio_attempts.remove(&server_name);
        Ok(())
    }

    /// Stage a server reload by server name (reuse existing config) or full config.
    pub fn stage_reload<T: Into<McpReloadTarget>>(&mut self, target: T) -> Result<(), McpError> {
        let (server_name, requested_payload) = match target.into() {
            McpReloadTarget::ServerName(server_name) => (server_name, None),
            McpReloadTarget::Config(config) => {
                Self::validate_tool_names(&config)?;
                (config.name.clone(), Some(config))
            }
        };
        // A governed server never reconnects or changes descriptor without a
        // new control; its reload is a governed add.
        self.refuse_ungoverned_change(&server_name)?;
        let sid = SurfaceId::from(server_name.as_str());
        match self
            .surface_owner
            .apply(ExternalToolSurfaceInput::StageReload { surface_id: sid })
        {
            Ok(_) => {
                // K14: rejected mirror apply propagates typed; the staged
                // payload bookkeeping below only runs on accepted apply.
                self.notify_lifecycle_reload(&server_name)?;
                // Lifecycle legality is owner-owned. Shell payload lookup is
                // execution context only and does not decide whether StageReload
                // is legal.
                let payload = requested_payload.or_else(|| {
                    self.servers
                        .get(&server_name)
                        .map(|entry| entry.config.clone())
                });
                if let Some(payload) = payload {
                    self.staged_payloads.insert(server_name, payload);
                } else {
                    tracing::warn!(
                        server = %server_name,
                        "Surface owner accepted StageReload but no execution payload is available; apply_staged will surface this as a protocol error"
                    );
                    self.staged_payloads.remove(&server_name);
                }
                Ok(())
            }
            Err(error) => {
                tracing::warn!(
                    server = %server_name,
                    error = %error,
                    "Surface owner rejected StageReload"
                );
                Err(error.into())
            }
        }
    }

    /// Apply staged operations at a boundary (non-blocking for add/reload).
    ///
    /// All state transitions flow through the surface owner. The shell spawns
    /// background tasks and executes effects.
    pub async fn apply_staged(&mut self) -> Result<McpApplyResult, McpError> {
        let mut delta = McpApplyDelta::default();
        // 1. Drain pending results from background tasks.
        self.drain_pending();
        // 2. Servers a call found without a usable credential are released
        // before this boundary applies, so their removal applies now.
        self.release_servers_refused_at_call()?;
        // 3. Sign-ins kept through a settled release are staged again, so
        // they apply in this boundary.
        self.restage_retained_sign_ins(&mut delta);
        let staged_intents = self.surface_owner.staged_intents_in_order();
        let mut snapshot_alignment = None;

        for (surface_id, staged_op, staged_sequence) in staged_intents {
            let server_name = surface_id.0.clone();
            let config = match staged_op {
                StagedSurfaceOp::Add => Some(
                    self.staged_payloads
                        .get(&server_name)
                        .cloned()
                        .ok_or_else(|| McpError::ProtocolError {
                            message: format!(
                                "staged add for '{server_name}' is missing its staged payload"
                            ),
                        })?,
                ),
                StagedSurfaceOp::Reload => Some(
                    self.staged_payloads
                        .get(&server_name)
                        .cloned()
                        .ok_or_else(|| McpError::ProtocolError {
                            message: format!(
                                "staged reload for '{server_name}' is missing its staged payload"
                            ),
                        })?,
                ),
                StagedSurfaceOp::Remove | StagedSurfaceOp::None => None,
            };
            // A removal reaching its boundary for a surface that is neither
            // attached nor connecting (its governed add was withdrawn before
            // it ever applied) has no connection to protect: that is its
            // disposition. Any other stays governed until its removal
            // finalizes.
            // The marker is released only once that boundary is accepted; a
            // rejected boundary (for example a stale sequence) keeps it.
            let no_connection_removal = matches!(staged_op, StagedSurfaceOp::Remove)
                && matches!(
                    self.surface_owner.surface_base(&surface_id),
                    SurfaceBaseState::Absent | SurfaceBaseState::Removed
                )
                && !self.surface_owner.pending_surfaces().contains(&surface_id);
            let applied_at_turn = self.turn_number_from_staged_sequence(staged_sequence);
            match self
                .surface_owner
                .apply(ExternalToolSurfaceInput::ApplyBoundary {
                    surface_id,
                    staged_intent_sequence: staged_sequence,
                    applied_at_turn,
                }) {
                Ok(transition) => {
                    // The control moves with this exact payload into its
                    // connection; a rejected boundary keeps both staged.
                    let (control, attempt) = match staged_op {
                        StagedSurfaceOp::Add => (
                            self.staged_controls.remove(&server_name),
                            self.staged_attempts.remove(&server_name),
                        ),
                        _ => (None, None),
                    };
                    if control.is_some() {
                        self.governed_servers.insert(server_name.clone());
                    }
                    if no_connection_removal {
                        self.governed_servers.remove(&server_name);
                    }
                    self.execute_effects(
                        &transition.effects,
                        &mut delta,
                        config.as_ref(),
                        control,
                        attempt,
                        &mut snapshot_alignment,
                    );
                    if matches!(staged_op, StagedSurfaceOp::Add | StagedSurfaceOp::Reload) {
                        self.staged_payloads.remove(&server_name);
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        server = %server_name,
                        error = %error,
                        "Surface owner rejected ApplyBoundary for staged intent"
                    );
                    // The exact staged attempt's typed answer: its boundary
                    // was refused.
                    if matches!(staged_op, StagedSurfaceOp::Add) {
                        resolve_attempt(
                            self.staged_attempts.remove(&server_name).as_ref(),
                            AttemptState::Rejected(error.to_string()),
                        );
                    }
                    // K14: the rejection is a typed per-server fact on the
                    // apply result, not just a log line.
                    delta.rejected_boundaries.push(McpBoundaryRejection {
                        server: server_name,
                        error,
                    });
                }
            }
        }

        self.process_removals(&mut delta, &mut snapshot_alignment)
            .await?;
        self.record_snapshot_alignment(snapshot_alignment);
        let published = self.publish_projection_snapshot();
        if published {
            let obligation = self.pending_snapshot_alignment.take();
            self.align_snapshot_if_requested(obligation);
        }

        let pending_count = self.surface_owner.pending_count();
        Ok(McpApplyResult {
            delta,
            pending_count,
        })
    }

    /// Execute effects returned by the surface owner.
    fn execute_effects(
        &mut self,
        effects: &[ExternalToolSurfaceEffect],
        delta: &mut McpApplyDelta,
        config: Option<&McpServerConfig>,
        mut control: Option<Arc<dyn McpAttachmentAuthority>>,
        mut attempt: Option<AttemptSender>,
        snapshot_alignment: &mut Option<SurfaceSnapshotAlignmentObligation>,
    ) {
        self.reap_closing();
        // Extract obligation tokens from ScheduleSurfaceCompletion effects
        // before iterating — these are consumed by spawn_pending.
        let core_effects = core_surface_effects(effects);
        let mut obligations = protocol_surface_completion::extract_obligations(&core_effects)
            .into_iter()
            .collect::<VecDeque<_>>();
        let mut snapshot_obligations =
            protocol_surface_snapshot_alignment::extract_obligations(&core_effects)
                .into_iter()
                .collect::<VecDeque<_>>();

        for effect in effects {
            match effect {
                ExternalToolSurfaceEffect::ScheduleSurfaceCompletion { operation, .. } => {
                    if let Some(config) = config {
                        if !matches!(
                            operation,
                            SurfaceDeltaOperation::Add | SurfaceDeltaOperation::Reload
                        ) {
                            continue;
                        }
                        if let Some(obligation) = obligations.pop_front() {
                            self.spawn_pending(
                                config.clone(),
                                control.take(),
                                attempt.take(),
                                obligation,
                            );
                        }
                    }
                }
                ExternalToolSurfaceEffect::RefreshVisibleSurfaceSet { .. } => {
                    if let Some(obligation) = snapshot_obligations.pop_front() {
                        merge_snapshot_alignment(snapshot_alignment, obligation);
                    }
                }
                ExternalToolSurfaceEffect::EmitExternalToolDelta {
                    surface_id,
                    operation,
                    phase,
                    ..
                } => {
                    let mcp_operation = match operation {
                        SurfaceDeltaOperation::Add => ToolConfigChangeOperation::Add,
                        SurfaceDeltaOperation::Remove => ToolConfigChangeOperation::Remove,
                        SurfaceDeltaOperation::Reload => ToolConfigChangeOperation::Reload,
                        SurfaceDeltaOperation::None => continue,
                    };
                    let mcp_phase = match phase {
                        SurfaceDeltaPhase::Pending => McpLifecyclePhase::Pending,
                        SurfaceDeltaPhase::Applied => McpLifecyclePhase::Applied,
                        SurfaceDeltaPhase::Draining => McpLifecyclePhase::Draining,
                        SurfaceDeltaPhase::Failed => McpLifecyclePhase::Failed,
                        SurfaceDeltaPhase::Forced => McpLifecyclePhase::Forced,
                        SurfaceDeltaPhase::None => continue,
                    };
                    delta.lifecycle_actions.push(McpLifecycleAction::new(
                        surface_id.0.clone(),
                        mcp_operation,
                        mcp_phase,
                    ));
                }
                ExternalToolSurfaceEffect::CloseSurfaceConnection { surface_id } => {
                    let conn = self.servers.get_mut(&surface_id.0).and_then(|entry| {
                        entry.tools.clear();
                        entry.connection.take()
                    });
                    self.spawn_close(surface_id.0.clone(), conn);
                    self.failed_stdio_attempts.remove(&surface_id.0);
                    // K14: the close effect comes from an already-accepted
                    // surface transition, so it must execute; a rejected
                    // lifecycle mirror apply propagates as a typed Failed
                    // action on the canonical lifecycle channel instead of
                    // being debug-swallowed.
                    if let Err(error) = self.notify_lifecycle_disconnected(&surface_id.0) {
                        self.completed_updates.push_back(CompletedLifecycleUpdate {
                            action: McpLifecycleAction::new(
                                surface_id.0.clone(),
                                ToolConfigChangeOperation::Remove,
                                McpLifecyclePhase::Failed,
                            )
                            .with_detail(Some(error.to_string())),
                        });
                    }
                    // A release-staged removal keeps its sign-in record: the
                    // server awaits sign-in to be added back. Any other
                    // removal withdraws the ask.
                    // A sign-in kept on that record is staged again at the
                    // next boundary ([`Self::restage_retained_sign_ins`]).
                    if self
                        .awaiting_authorization
                        .get(&surface_id.0)
                        .is_some_and(|awaiting| awaiting.operation != AwaitingOperation::Released)
                    {
                        self.awaiting_authorization.remove(&surface_id.0);
                    }
                    delta.removed_servers.push(surface_id.0.clone());
                }
                ExternalToolSurfaceEffect::RejectSurfaceCall { .. } => {
                    // Rejections are handled inline during call_tool, not here.
                }
            }
        }
        // An accepted boundary that scheduled no connection for this add
        // leaves its attempt nothing to wait for.
        resolve_attempt(
            attempt.as_ref(),
            AttemptState::failed("the accepted boundary scheduled no connection"),
        );
    }

    /// Spawn a background task to connect and enumerate tools for a server.
    fn spawn_pending(
        &mut self,
        config: McpServerConfig,
        control: Option<Arc<dyn McpAttachmentAuthority>>,
        attempt: Option<AttemptSender>,
        obligation: SurfaceCompletionObligation,
    ) {
        let server_name = config.name.clone();
        let refused = self.refused_payloads.remove(&server_name);
        // A newer attempt for the server supersedes its retained failure.
        self.failed_stdio_attempts.remove(&server_name);
        if let Some(attempt) = attempt {
            self.pending_attempts.insert(
                (server_name.clone(), obligation.pending_task_sequence),
                attempt,
            );
        }
        self.pending_obligations
            .insert(server_name, obligation.clone());
        if let Some(refusal) = refused {
            self.spawn_refused(config, obligation, refusal);
            return;
        }

        // The attempt's custodies are created and indexed together, under
        // the close index's admission freeze, before anything can spawn or
        // connect: an attempt admitted after the index began its close gets
        // them closed, and is refused before any effect.
        let crate::connection::AdmittedAttempt {
            process: stdio_custody,
            client: client_custody,
            stop,
            end,
            diagnostics,
        } = self.close_index.admit(
            &obligation.surface_id,
            matches!(config.transport, McpTransportConfig::Stdio(_))
                .then_some(self.stdio_close_bounds),
        );
        if let Some(record) = diagnostics {
            self.pending_stdio_diagnostics.insert(
                (
                    obligation.surface_id.clone(),
                    obligation.pending_task_sequence,
                ),
                record,
            );
        }
        if let Some(custody) = &stdio_custody {
            self.pending_child_custody.insert(
                (
                    obligation.surface_id.clone(),
                    obligation.pending_task_sequence,
                ),
                custody.clone(),
            );
        }
        let tx = self.pending_tx.clone();
        let progress = Arc::clone(&self.progress);
        let delivered = Arc::clone(&self.connect_results_delivered);
        self.connect_attempts_spawned = self.connect_attempts_spawned.wrapping_add(1);
        let auth_mode = self.mcp_auth_mode;
        let auth_resolver = self.mcp_auth_resolver.clone();
        let client_factory = self.client_service_factory.clone();
        let stdio_profile = self.stdio_launch_profile.clone();
        let http_egress = self.http_egress_profile.clone();
        // Only stdio attempts are guarded.
        let attempt_guards = self
            .attempt_guards
            .clone()
            .filter(|_| crate::attempt_guard::guards_transport(&config));
        let attempt = obligation.pending_task_sequence;
        let operation = crate::attempt_guard::attempt_operation(obligation.operation);
        // Reap tasks that already finished so the owned set stays bounded.
        while self.connect_tasks.try_join_next().is_some() {}
        self.connect_tasks.spawn(async move {
            let mut guard = None;
            let result = {
                // Marks the attempt's end once the connect below is done or
                // dropped.
                let _end = end;
                tokio::select! {
                    biased;
                    // The index began its close: the attempt stops here.
                    () = stop.requested() => Err(McpError::RouterShutDown),
                    result = async {
                        // The guard begins before secret resolution, the
                        // physical entry or any spawn; its custody holds it.
                        if let Some(guards) = &attempt_guards {
                            guard = guards.begin_attempt(&config, attempt, operation).await?;
                            if let (Some(record), Some(custody)) = (&guard, &stdio_custody) {
                                custody.hold_guard(Arc::clone(record));
                            }
                        }
                        McpConnection::connect_and_enumerate_with_custody_and_egress(
                            &config,
                            auth_mode,
                            auth_resolver,
                            client_factory,
                            stdio_custody,
                            client_custody,
                            &stdio_profile,
                            &http_egress,
                            control,
                        )
                        .await
                    } => result,
                }
            };
            let sent = tx
                .send(PendingResult {
                    obligation,
                    config,
                    result,
                    guard,
                })
                .await;
            delivered.fetch_add(1, Ordering::AcqRel);
            progress.send_modify(|seen| *seen = seen.wrapping_add(1));
            if let Err(error) = sent {
                // The router is gone; this task is the result's last owner.
                let server_name = error.0.obligation.surface_id.clone();
                McpRouter::close_result_connection_if_present(server_name, error.0.result).await;
            }
        });
    }

    /// Complete a refused add's attempt at once with its typed refusal: no
    /// custody is admitted and nothing is authorized, connected or spawned.
    fn spawn_refused(
        &mut self,
        config: McpServerConfig,
        obligation: SurfaceCompletionObligation,
        refusal: meerkat_core::confinement::ConfinementRefusal,
    ) {
        let tx = self.pending_tx.clone();
        let progress = Arc::clone(&self.progress);
        let delivered = Arc::clone(&self.connect_results_delivered);
        self.connect_attempts_spawned = self.connect_attempts_spawned.wrapping_add(1);
        while self.connect_tasks.try_join_next().is_some() {}
        self.connect_tasks.spawn(async move {
            // Nothing was entered, so a router that is gone leaves nothing
            // to close.
            let _ = tx
                .send(PendingResult {
                    obligation,
                    config,
                    result: Err(McpError::Confinement(refusal)),
                    guard: None,
                })
                .await;
            delivered.fetch_add(1, Ordering::AcqRel);
            progress.send_modify(|seen| *seen = seen.wrapping_add(1));
        });
    }

    /// Drain completed pending results from the background channel.
    fn drain_pending(&mut self) {
        self.reap_closing();
        let results: Vec<PendingResult> = {
            let mut rx = match self.pending_rx.lock() {
                Ok(guard) => guard,
                Err(poisoned) => {
                    tracing::warn!(
                        "MCP pending_rx mutex was poisoned; recovering. \
                         Router state may be inconsistent."
                    );
                    poisoned.into_inner()
                }
            };
            let mut buf = Vec::new();
            while let Ok(result) = rx.try_recv() {
                buf.push(result);
            }
            buf
        };
        let mut snapshot_alignment = None;
        for result in results {
            if let Some(obligation) = self.process_pending_result(result) {
                merge_snapshot_alignment(&mut snapshot_alignment, obligation);
            }
        }
        self.record_snapshot_alignment(snapshot_alignment);
        if self.pending_snapshot_alignment.is_some() {
            let published = self.publish_projection_snapshot();
            if published {
                let obligation = self.pending_snapshot_alignment.take();
                self.align_snapshot_if_requested(obligation);
            }
        }
    }

    fn process_pending_result(
        &mut self,
        result: PendingResult,
    ) -> Option<SurfaceSnapshotAlignmentObligation> {
        let PendingResult {
            obligation,
            config,
            result,
            guard,
        } = result;
        let server_name = obligation.surface_id.clone();
        // The exact attempt this accepted obligation scheduled, if governed.
        let attempt = self
            .pending_attempts
            .remove(&(server_name.clone(), obligation.pending_task_sequence));
        // The result (a connection, or an error after the process exited) now
        // carries the process; the attempt's custody entry is done.
        let attempt_custody = self
            .pending_child_custody
            .remove(&(server_name.clone(), obligation.pending_task_sequence));
        // A success or a stale result drops it here; only an accepted
        // failure keeps it.
        let attempt_diagnostics = self
            .pending_stdio_diagnostics
            .remove(&(server_name.clone(), obligation.pending_task_sequence));
        let open = |outcome| {
            if let Some(guard) = &guard {
                guard.open(outcome);
            }
        };
        match result {
            Ok((conn, tools)) => {
                let tool_count = tools.len();

                match self
                    .surface_owner
                    .apply(ExternalToolSurfaceInput::PendingSucceeded {
                        surface_id: SurfaceId(obligation.surface_id.clone()),
                        operation: local_surface_operation(obligation.operation),
                        pending_task_sequence: obligation.pending_task_sequence,
                        staged_intent_sequence: obligation.staged_intent_sequence,
                        applied_at_turn: TurnNumber(obligation.applied_at_turn),
                    }) {
                    Ok(transition) => {
                        self.pending_obligations.remove(&server_name);
                        // The owner accepted this attempt's success: any
                        // authorization ask for the server is answered.
                        self.awaiting_authorization.remove(&server_name);
                        self.failed_stdio_attempts.remove(&server_name);
                        let Some(operation) = lifecycle_operation_from_effects(
                            &transition.effects,
                            SurfaceDeltaPhase::Applied,
                        ) else {
                            tracing::error!(
                                server = %server_name,
                                "Surface owner accepted PendingSucceeded without an applied lifecycle delta"
                            );
                            open(crate::McpAttemptOpenOutcome::NotInstalled {
                                reason: crate::McpAttemptNotInstalled::InstallRefused,
                            });
                            resolve_attempt(
                                attempt.as_ref(),
                                AttemptState::failed(
                                    "the accepted connection carried no applied delta",
                                ),
                            );
                            return None;
                        };
                        let snapshot_alignment = latest_snapshot_alignment(&transition.effects);

                        // K14: the lifecycle DSL mirror must accept the
                        // connected transition before the shell installs the
                        // server entry. On rejection the fresh connection is
                        // closed, the shell mirror stays unchanged, and the
                        // typed fault propagates as a Failed action on the
                        // canonical lifecycle channel (the same channel
                        // background connection failures use).
                        if let Err(error) = self.notify_lifecycle_connected(&server_name) {
                            open(crate::McpAttemptOpenOutcome::NotInstalled {
                                reason: crate::McpAttemptNotInstalled::InstallRefused,
                            });
                            resolve_attempt(
                                attempt.as_ref(),
                                AttemptState::failed(error.to_string()),
                            );
                            self.spawn_close(server_name.clone(), Some(conn));
                            self.completed_updates.push_back(CompletedLifecycleUpdate {
                                action: McpLifecycleAction::new(
                                    server_name,
                                    operation,
                                    McpLifecyclePhase::Failed,
                                )
                                .with_detail(Some(error.to_string())),
                            });
                            return snapshot_alignment;
                        }

                        // For reload: close old connection.
                        if obligation.operation == ExternalToolSurfaceDeltaOperation::Reload
                            && let Some(old_entry) = self.servers.get_mut(&server_name)
                        {
                            let old_conn = old_entry.connection.take();
                            self.spawn_close(server_name.clone(), old_conn);
                        }

                        // Install the new entry (or replace existing).
                        let connection_generation = conn.connection_id().generation();
                        open(crate::McpAttemptOpenOutcome::Installed {
                            connection_generation,
                        });
                        let config = conn.config().clone();
                        let new_entry = ServerEntry {
                            config,
                            connection: Some(conn),
                            tools,
                            active_calls: AtomicUsize::new(0),
                        };

                        if let Some(old_entry) = self.servers.insert(server_name.clone(), new_entry)
                            && obligation.operation == ExternalToolSurfaceDeltaOperation::Add
                        {
                            self.spawn_close(old_entry.config.name, old_entry.connection);
                        }

                        self.completed_updates.push_back(CompletedLifecycleUpdate {
                            action: McpLifecycleAction::new(
                                server_name,
                                operation,
                                McpLifecyclePhase::Applied,
                            )
                            .with_tool_count(Some(tool_count)),
                        });
                        resolve_attempt(
                            attempt.as_ref(),
                            AttemptState::Installed {
                                connection_generation,
                            },
                        );
                        snapshot_alignment
                    }
                    Err(e) => {
                        tracing::warn!(
                            server = %server_name,
                            error = %e,
                            "Surface owner rejected PendingSucceeded"
                        );
                        open(crate::McpAttemptOpenOutcome::NotInstalled {
                            reason: crate::McpAttemptNotInstalled::Superseded,
                        });
                        resolve_attempt(attempt.as_ref(), AttemptState::Superseded);
                        self.spawn_close(server_name, Some(conn));
                        None
                    }
                }
            }
            Err(err) => {
                // The guard learns the open outcome (accepted or stale): the
                // typed refusal, or the setup failure with the attempt's own
                // classified failure.
                open(crate::attempt_guard::open_for_error(
                    &err,
                    attempt_diagnostics.as_ref().and_then(|record| {
                        let (phase, class) = record.failure()?;
                        Some(meerkat_core::mcp_attachment::McpStdioSetupFailure::new(
                            server_name.clone(),
                            obligation.pending_task_sequence,
                            phase,
                            class,
                            record.captured(),
                        ))
                    }),
                ));
                // The failed attempt closed its own process (or left that
                // close to its close index), and what that close observes is
                // still the router's to report: the close index holds it. A
                // custody outside the index is handled here.
                if let Some(custody) =
                    attempt_custody.filter(|custody| !self.close_index.covers_process(custody))
                {
                    match custody.closed_receipt() {
                        Some(receipt) => self.retain_if_unsettled(McpServerCloseRecord {
                            server: server_name.clone(),
                            outcome: McpServerCloseOutcome::Stdio(receipt),
                        }),
                        None if custody.close_pending() => {
                            self.spawn_custody_close(server_name.clone(), custody);
                        }
                        None => {}
                    }
                }
                // K14: a rejected mirror apply is a typed fault folded into
                // the Failed action's detail below (the canonical lifecycle
                // channel), never debug-swallowed.
                let mirror_fault = self
                    .notify_lifecycle_failed(&server_name, &err.to_string())
                    .err();

                let (snapshot_alignment, operation) = match self.surface_owner.apply(
                    ExternalToolSurfaceInput::PendingFailed {
                        surface_id: SurfaceId(obligation.surface_id.clone()),
                        operation: local_surface_operation(obligation.operation),
                        pending_task_sequence: obligation.pending_task_sequence,
                        staged_intent_sequence: obligation.staged_intent_sequence,
                        applied_at_turn: TurnNumber(obligation.applied_at_turn),
                        cause: ExternalToolSurfaceFailureCause::PendingFailed,
                    },
                ) {
                    Ok(transition) => {
                        self.pending_obligations.remove(&server_name);
                        let stdio_failure = attempt_diagnostics.and_then(|record| {
                            let (phase, class) = record.failure()?;
                            Some(RetainedStdioFailure {
                                failure: meerkat_core::mcp_attachment::McpStdioSetupFailure::new(
                                    server_name.clone(),
                                    obligation.pending_task_sequence,
                                    phase,
                                    class,
                                    record.captured(),
                                ),
                                record,
                            })
                        });
                        resolve_attempt(
                            attempt.as_ref(),
                            AttemptState::Failed {
                                detail: err.to_string(),
                                failure: stdio_failure
                                    .as_ref()
                                    .map(|retained| retained.failure.clone()),
                            },
                        );
                        self.record_accepted_failure(&server_name, &err, config, &obligation);
                        // The latest accepted failure is the one retained.
                        match stdio_failure {
                            Some(retained) => {
                                self.failed_stdio_attempts
                                    .insert(server_name.clone(), retained);
                            }
                            None => {
                                self.failed_stdio_attempts.remove(&server_name);
                            }
                        }
                        let Some(operation) = lifecycle_operation_from_effects(
                            &transition.effects,
                            SurfaceDeltaPhase::Failed,
                        ) else {
                            tracing::error!(
                                server = %server_name,
                                "Surface owner accepted PendingFailed without a failed lifecycle delta"
                            );
                            return None;
                        };
                        (latest_snapshot_alignment(&transition.effects), operation)
                    }
                    Err(e) => {
                        tracing::warn!(
                            server = %server_name,
                            error = %e,
                            "Surface owner rejected PendingFailed"
                        );
                        resolve_attempt(attempt.as_ref(), AttemptState::Superseded);
                        // Even when the surface owner rejects the failure
                        // input, a rejected lifecycle mirror apply must still
                        // surface typed on the canonical action channel.
                        if let Some(mirror_fault) = mirror_fault {
                            self.completed_updates.push_back(CompletedLifecycleUpdate {
                                action: McpLifecycleAction::new(
                                    server_name,
                                    obligation_lifecycle_operation(obligation.operation),
                                    McpLifecyclePhase::Failed,
                                )
                                .with_detail(Some(mirror_fault.to_string())),
                            });
                        }
                        return None;
                    }
                };

                // Only the accepted completion can publish this observation.
                // Generic transport errors and diagnostic lookalikes stay generic.
                let confinement_refusal = match &err {
                    McpError::Confinement(refusal) => Some(*refusal),
                    _ => None,
                };

                tracing::warn!(
                    server = %server_name,
                    error = %err,
                    op = ?obligation.operation,
                    "MCP server background connection failed"
                );

                let detail = match mirror_fault {
                    None => err.to_string(),
                    Some(mirror_fault) => {
                        format!("{err}; {mirror_fault}")
                    }
                };
                self.completed_updates.push_back(CompletedLifecycleUpdate {
                    action: McpLifecycleAction::new(
                        server_name,
                        operation,
                        McpLifecyclePhase::Failed,
                    )
                    .with_detail(Some(detail))
                    .with_confinement_refusal(confinement_refusal),
                });
                snapshot_alignment
            }
        }
    }

    /// Host-channel status: the MCP targets whose latest connection attempt is
    /// waiting for a human to authorize them through the host's browser
    /// channel (see `McpOAuthAuthority::login_start`). This is a host query,
    /// not an agent event: it carries only the typed target, never an
    /// authorize URL, state or code. A later successful attempt or removal
    /// clears the entry. It reflects background results already drained by
    /// the normal lifecycle polling; it drains nothing itself.
    pub fn servers_awaiting_authorization(&self) -> Vec<McpServerIdentity> {
        self.awaiting_authorization
            .values()
            .map(|awaiting| awaiting.target.clone())
            .collect()
    }

    /// The typed setup failure of `server`'s retained failed stdio attempt
    /// (see `failed_stdio_attempts`): host status, never projected to the
    /// agent. It reflects results already drained and drains nothing.
    pub(crate) fn stdio_setup_failure(
        &self,
        server: &str,
    ) -> Option<meerkat_core::mcp_attachment::McpStdioSetupFailure> {
        self.failed_stdio_attempts
            .get(server)
            .map(|retained| retained.failure.clone())
    }

    /// The redacted stderr tail of `server`'s retained failed stdio attempt,
    /// when it is that exact `attempt` (any attempt for `None`). SENSITIVE:
    /// host and operator only.
    pub(crate) fn stdio_stderr_tail(
        &self,
        server: &str,
        attempt: Option<u64>,
    ) -> Option<meerkat_core::mcp_attachment::McpStdioStderrTail> {
        let retained = self.failed_stdio_attempts.get(server)?;
        if attempt.is_some_and(|attempt| attempt != retained.failure.attempt) {
            return None;
        }
        retained.record.stderr_tail()
    }

    /// Whether the router still retains the stderr tail of `server`'s exact
    /// failed `attempt`.
    pub(crate) fn retains_stdio_tail(&self, server: &str, attempt: u64) -> bool {
        self.failed_stdio_attempts
            .get(server)
            .is_some_and(|retained| {
                retained.failure.attempt == attempt && retained.record.captured()
            })
    }

    /// Record the authorization outcome of a failure the surface owner just
    /// accepted. Only an accepted completion reaches here, so a stale result
    /// can neither insert an old record nor erase a newer one. An
    /// `AuthorizationRequired` failure is retained only while no newer
    /// intent (a remove included) is staged for the server.
    fn record_accepted_failure(
        &mut self,
        server_name: &str,
        error: &McpError,
        config: McpServerConfig,
        obligation: &SurfaceCompletionObligation,
    ) {
        self.awaiting_authorization.remove(server_name);
        let McpError::AuthorizationRequired { target } = error else {
            return;
        };
        let operation = match obligation.operation {
            ExternalToolSurfaceDeltaOperation::Add => AwaitingOperation::Add,
            ExternalToolSurfaceDeltaOperation::Reload => AwaitingOperation::Reload,
            _ => return,
        };
        if self.has_staged_intent(server_name) {
            return;
        }
        self.awaiting_authorization.insert(
            server_name.to_owned(),
            AwaitingAuthorization {
                target: (**target).clone(),
                config,
                operation,
                signed_in_during_release: false,
            },
        );
    }

    fn has_staged_intent(&self, server_name: &str) -> bool {
        self.surface_owner
            .staged_intents_in_order()
            .iter()
            .any(|(surface_id, _, _)| surface_id.0 == server_name)
    }

    /// Whether the removal a credential release staged for this server is
    /// still staged, or applied and still closing the connection. A newer
    /// staged add or reload is not that removal.
    fn release_removal_unsettled(&self, server_name: &str) -> bool {
        let staged = self
            .surface_owner
            .staged_intents_in_order()
            .into_iter()
            .find(|(surface_id, _, _)| surface_id.0 == server_name)
            .map(|(_, op, _)| op);
        match staged {
            Some(StagedSurfaceOp::Remove) => true,
            Some(_) => false,
            None => self
                .servers
                .get(server_name)
                .is_some_and(|entry| entry.connection.is_some()),
        }
    }

    /// Stage again every released server whose sign-in landed while its
    /// release removal settled, once that removal has closed the connection,
    /// exactly as [`Self::retry_authorized`] would have. Runs at the start of
    /// each boundary, so the add applies in that boundary; it reads the
    /// current credential when it connects.
    ///
    /// A newer staged intent or an operation in flight for the server wins
    /// and the sign-in stays kept. A staging refusal is reported as a failed
    /// add lifecycle action in this boundary's result and the sign-in stays
    /// kept, so a later boundary tries again. Once the owner accepts the
    /// add, its payload is installed before the lifecycle mirror, and a
    /// mirror refusal is reported the same way without undoing the accepted
    /// intent.
    fn restage_retained_sign_ins(&mut self, delta: &mut McpApplyDelta) {
        let due: Vec<String> = self
            .awaiting_authorization
            .iter()
            .filter(|(_, awaiting)| {
                awaiting.operation == AwaitingOperation::Released
                    && awaiting.signed_in_during_release
            })
            .filter(|(name, _)| {
                self.servers
                    .get(name.as_str())
                    .is_none_or(|entry| entry.connection.is_none())
            })
            .map(|(name, _)| name.clone())
            .collect();
        for server_name in due {
            let surface_id = SurfaceId::from(server_name.as_str());
            if self.has_staged_intent(&server_name)
                || self.surface_owner.pending_surfaces().contains(&surface_id)
            {
                continue;
            }
            let Some(config) = self
                .awaiting_authorization
                .get(&server_name)
                .map(|awaiting| awaiting.config.clone())
            else {
                continue;
            };
            if let Err(error) = Self::validate_tool_names(&config).and_then(|()| {
                self.surface_owner
                    .apply(ExternalToolSurfaceInput::StageAdd { surface_id })
                    .map(drop)
                    .map_err(McpError::from)
            }) {
                tracing::warn!(
                    server = %server_name,
                    %error,
                    "could not stage the released server again after its sign-in; kept for the next boundary"
                );
                delta.lifecycle_actions.push(
                    McpLifecycleAction::new(
                        server_name.clone(),
                        ToolConfigChangeOperation::Add,
                        McpLifecyclePhase::Failed,
                    )
                    .with_detail(Some(error.to_string())),
                );
                continue;
            }
            // The owner accepted the add: the kept sign-in is answered and
            // the payload is installed before the lifecycle mirror.
            if let Some(record) = self.awaiting_authorization.get_mut(&server_name) {
                record.signed_in_during_release = false;
            }
            self.staged_payloads.insert(server_name.clone(), config);
            if let Err(error) = self.notify_lifecycle_connect_pending(&server_name) {
                delta.lifecycle_actions.push(
                    McpLifecycleAction::new(
                        server_name.clone(),
                        ToolConfigChangeOperation::Add,
                        McpLifecyclePhase::Failed,
                    )
                    .with_detail(Some(error.to_string())),
                );
            }
        }
    }

    /// After a successful login for `target`, stage again the one failed
    /// operation that awaits exactly that target, with the config it
    /// attempted. It applies at the next boundary through the normal
    /// lifecycle and reads the credential then; this copies no credential.
    ///
    /// A newer staged intent (including a remove) or an operation in flight
    /// for the server wins and is left alone. Pending results are drained
    /// first, so an accepted result supersedes or refreshes the record.
    /// A staging fault is returned typed; the accepted intent keeps its
    /// payload, so the next boundary does not fail for a missing payload.
    pub fn retry_authorized(
        &mut self,
        target: &McpServerIdentity,
    ) -> Result<AuthorizedRetry, McpError> {
        self.drain_pending();
        // A call refusal queued for this target before the sign-in describes
        // the credential that was replaced: the server stays connected and
        // usable with the new one, so the refusal no longer releases it.
        self.calls_refused_unauthorized
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|(_, refused)| refused != target);
        let Some((server_name, awaiting)) = self
            .awaiting_authorization
            .iter()
            .find(|(_, awaiting)| awaiting.target == *target)
            .map(|(name, awaiting)| (name.clone(), awaiting.clone()))
        else {
            return Ok(AuthorizedRetry::NotAwaiting);
        };
        let surface_id = SurfaceId::from(server_name.as_str());
        if awaiting.operation == AwaitingOperation::Released
            && self.release_removal_unsettled(&server_name)
        {
            // An explicit remove withdraws this record, so a staged remove
            // here is the release's own; keep the sign-in for its settlement.
            if let Some(record) = self.awaiting_authorization.get_mut(&server_name) {
                record.signed_in_during_release = true;
            }
            return Ok(AuthorizedRetry::AfterRelease);
        }
        if self.has_staged_intent(&server_name)
            || self.surface_owner.pending_surfaces().contains(&surface_id)
        {
            return Ok(AuthorizedRetry::AlreadyPending);
        }
        let input = match awaiting.operation {
            AwaitingOperation::Add | AwaitingOperation::Released => {
                Self::validate_tool_names(&awaiting.config)?;
                ExternalToolSurfaceInput::StageAdd { surface_id }
            }
            AwaitingOperation::Reload => ExternalToolSurfaceInput::StageReload { surface_id },
        };
        if let Err(error) = self.surface_owner.apply(input) {
            tracing::warn!(
                server = %server_name,
                error = %error,
                "Surface owner rejected the post-login retry"
            );
            return Err(error.into());
        }
        // The owner accepted the intent: its payload is installed before the
        // lifecycle mirror, so a mirror fault cannot leave the boundary
        // without it.
        self.staged_payloads
            .insert(server_name.clone(), awaiting.config);
        match awaiting.operation {
            AwaitingOperation::Add | AwaitingOperation::Released => {
                self.notify_lifecycle_connect_pending(&server_name)?;
            }
            AwaitingOperation::Reload => self.notify_lifecycle_reload(&server_name)?,
        }
        Ok(AuthorizedRetry::Staged)
    }

    /// After the credential for `target` was released (signed out), stop
    /// offering every connected server that uses exactly that credential.
    /// Calls are already refused at the bearer, which reads the credential
    /// per request; this unpublishes the tools. Each such server gets the
    /// ordinary staged removal (applied at the next boundary through the
    /// surface owner) and is recorded as awaiting sign-in with the config it
    /// connected with, so a later login for the same target
    /// ([`Self::retry_authorized`]) adds it back to this router.
    ///
    /// The server's identity is derived from its config exactly as a
    /// connection derives the target it reports (server name, URL, account
    /// selection, and the realm this router's resolver serves), so another
    /// URL, account or realm never matches. A newer staged intent or an operation in flight
    /// for the server wins and is left alone.
    pub fn credential_released(
        &mut self,
        target: &McpServerIdentity,
    ) -> Result<ReleasedCredential, McpError> {
        self.drain_pending();
        let mut matching: Vec<String> = self
            .servers
            .iter()
            .filter(|(_, entry)| entry.connection.is_some())
            .filter(|(_, entry)| {
                self.connected_identity(&entry.config)
                    .is_some_and(|id| id == *target)
            })
            .map(|(name, _)| name.clone())
            .collect();
        matching.sort();
        let mut outcome = ReleasedCredential::NotActive;
        for server_name in matching {
            // A sign-out after a sign-in retained during this server's
            // release supersedes it.
            if let Some(record) = self.awaiting_authorization.get_mut(&server_name)
                && record.operation == AwaitingOperation::Released
            {
                record.signed_in_during_release = false;
            }
            match self.release_server(&server_name, target)? {
                ReleasedCredential::Unpublished => outcome = ReleasedCredential::Unpublished,
                ReleasedCredential::AlreadyPending if outcome == ReleasedCredential::NotActive => {
                    outcome = ReleasedCredential::AlreadyPending;
                }
                _ => {}
            }
        }
        Ok(outcome)
    }

    /// The identity a connection for `config` resolves and reports: its
    /// config identity, scoped to the realm this router's resolver serves
    /// (as `McpConnection` scopes it), so it equals the host's target.
    fn connected_identity(&self, config: &McpServerConfig) -> Option<McpServerIdentity> {
        let identity = McpServerIdentity::from_config(config).ok()?;
        Some(
            match self
                .mcp_auth_resolver
                .as_deref()
                .and_then(McpAuthResolver::credential_realm)
            {
                Some(realm) => identity.in_realm(realm),
                None => identity,
            },
        )
    }

    /// Stage the removal of one connected server whose credential for
    /// `target` is gone, keeping its config as a sign-in record.
    fn release_server(
        &mut self,
        server_name: &str,
        target: &McpServerIdentity,
    ) -> Result<ReleasedCredential, McpError> {
        let Some(config) = self
            .servers
            .get(server_name)
            .filter(|entry| entry.connection.is_some())
            .map(|entry| entry.config.clone())
        else {
            return Ok(ReleasedCredential::NotActive);
        };
        let surface_id = SurfaceId::from(server_name);
        if self.has_staged_intent(server_name)
            || self.surface_owner.pending_surfaces().contains(&surface_id)
        {
            return Ok(ReleasedCredential::AlreadyPending);
        }
        self.stage_remove(server_name)?;
        self.awaiting_authorization.insert(
            server_name.to_owned(),
            AwaitingAuthorization {
                target: target.clone(),
                config,
                operation: AwaitingOperation::Released,
                signed_in_during_release: false,
            },
        );
        tracing::info!(
            server = %server_name,
            "MCP credential released; the server is unpublished and awaits sign-in"
        );
        Ok(ReleasedCredential::Unpublished)
    }

    /// Release every connected server a call found without a usable
    /// credential, if it still connects with exactly the reported target.
    fn release_servers_refused_at_call(&mut self) -> Result<(), McpError> {
        let refused = std::mem::take(
            &mut *self
                .calls_refused_unauthorized
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for (server_name, target) in refused {
            let still_this_target = self.servers.get(&server_name).is_some_and(|entry| {
                self.connected_identity(&entry.config)
                    .is_some_and(|id| id == target)
            });
            if still_this_target {
                self.release_server(&server_name, &target)?;
            }
        }
        Ok(())
    }

    /// Drain pending results and return queued canonical lifecycle actions.
    pub fn take_lifecycle_actions(&mut self) -> Vec<McpLifecycleAction> {
        self.drain_pending();
        self.completed_updates
            .drain(..)
            .map(|update| update.action)
            .collect()
    }

    /// Drain pending results and return queued external update notices.
    pub fn take_external_updates(&mut self) -> ExternalToolUpdate {
        self.drain_pending();
        let pending = self.pending_sources_snapshot();

        ExternalToolUpdate {
            notices: self
                .completed_updates
                .drain(..)
                .map(|update| update.action)
                .collect(),
            pending,
        }
    }

    /// Returns true if there are pending background operations or undelivered notices.
    pub fn has_pending_or_notices(&self) -> bool {
        self.surface_owner.pending_count() > 0 || !self.completed_updates.is_empty()
    }

    /// Snapshot the names of tool sources still connecting/loading.
    pub fn pending_sources_snapshot(&self) -> Vec<String> {
        self.surface_owner
            .pending_surfaces()
            .into_iter()
            .map(|surface_id| surface_id.0)
            .collect()
    }

    /// Backward-compatible immediate install path. Bypasses staged/boundary
    /// flow and directly drives the surface owner through Stage -> Apply -> Success.
    async fn install_active_server(&mut self, config: McpServerConfig) -> Result<(), McpError> {
        Self::validate_tool_names(&config)?;
        let server_name = config.name.clone();
        let sid = SurfaceId::from(server_name.as_str());

        if let Err(e) = self
            .surface_owner
            .apply(ExternalToolSurfaceInput::StageAdd {
                surface_id: sid.clone(),
            })
        {
            tracing::warn!(
                server = %server_name,
                error = %e,
                "Surface owner rejected StageAdd in install path"
            );
            return Err(McpError::ProtocolError {
                message: format!("surface owner rejected StageAdd: {e}"),
            });
        }
        let staged_sequence = self
            .surface_owner
            .staged_intents_in_order()
            .into_iter()
            .find(|(surface_id, _, _)| *surface_id == sid)
            .map(|(_, _, sequence)| sequence)
            .unwrap_or(0);
        let applied_at_turn = self.turn_number_from_staged_sequence(staged_sequence);
        let boundary_effects =
            match self
                .surface_owner
                .apply(ExternalToolSurfaceInput::ApplyBoundary {
                    surface_id: sid.clone(),
                    staged_intent_sequence: staged_sequence,
                    applied_at_turn,
                }) {
                Ok(t) => t.effects,
                Err(e) => {
                    tracing::warn!(
                        server = %server_name,
                        error = %e,
                        "Surface owner rejected ApplyBoundary in install path"
                    );
                    return Err(McpError::ProtocolError {
                        message: format!("surface owner rejected ApplyBoundary: {e}"),
                    });
                }
            };
        // Extract and immediately consume the obligation for the synchronous install path.
        let core_boundary_effects = core_surface_effects(&boundary_effects);
        let mut obligations =
            protocol_surface_completion::extract_obligations(&core_boundary_effects);
        let obligation = match obligations.pop() {
            Some(obligation) => obligation,
            None => {
                return Err(McpError::ProtocolError {
                    message: "surface owner ApplyBoundary emitted no surface completion obligation"
                        .into(),
                });
            }
        };

        // Created and indexed under the close index's admission freeze, as in
        // `spawn_pending`.
        let crate::connection::AdmittedAttempt {
            process: stdio_custody,
            client: client_custody,
            stop,
            end,
            diagnostics: _diagnostics,
        } = self.close_index.admit(
            &server_name,
            matches!(config.transport, McpTransportConfig::Stdio(_))
                .then_some(self.stdio_close_bounds),
        );
        // Only stdio attempts are guarded, as in `spawn_pending`.
        let attempt_guards = self
            .attempt_guards
            .clone()
            .filter(|_| crate::attempt_guard::guards_transport(&config));
        let attempt = obligation.pending_task_sequence;
        let operation = crate::attempt_guard::attempt_operation(obligation.operation);
        let mut guard = None;
        let result = {
            let _end = end;
            tokio::select! {
                biased;
                () = stop.requested() => Err(McpError::RouterShutDown),
                result = async {
                    if let Some(guards) = &attempt_guards {
                        guard = guards.begin_attempt(&config, attempt, operation).await?;
                        if let (Some(record), Some(custody)) = (&guard, &stdio_custody) {
                            custody.hold_guard(Arc::clone(record));
                        }
                    }
                    McpConnection::connect_and_enumerate_with_custody_and_egress(
                        &config,
                        self.mcp_auth_mode,
                        self.mcp_auth_resolver.clone(),
                        self.client_service_factory.clone(),
                        stdio_custody,
                        client_custody,
                        &self.stdio_launch_profile,
                        &self.http_egress_profile,
                        None,
                    )
                    .await
                } => result,
            }
        };
        let confinement_error = result.as_ref().err().and_then(|error| match error {
            McpError::Confinement(refusal) => Some(*refusal),
            _ => None,
        });
        let egress_error = result.as_ref().err().and_then(McpError::egress_error);
        let service_secret_refusal = result.as_ref().err().and_then(|error| match error {
            McpError::ServiceSecretRefused { refusal, .. } => Some(refusal.clone()),
            _ => None,
        });
        // A guard's or the attachment control's refusal stays typed.
        let attachment_refusal = result.as_ref().err().and_then(|error| match error {
            McpError::AttachmentRefused(refusal) => Some(refusal.clone()),
            _ => None,
        });
        let connect_error = result.as_ref().err().map(ToString::to_string);
        let snapshot_alignment = self.process_pending_result(PendingResult {
            obligation,
            config: config.clone(),
            result,
            guard,
        });

        self.record_snapshot_alignment(snapshot_alignment);
        let published = self.publish_projection_snapshot();
        if published {
            let obligation = self.pending_snapshot_alignment.take();
            self.align_snapshot_if_requested(obligation);
        }

        if let Some(refusal) = confinement_error {
            return Err(McpError::Confinement(refusal));
        }
        if let Some(error) = egress_error {
            return Err(McpError::from_egress(&server_name, error));
        }
        if let Some(refusal) = service_secret_refusal {
            return Err(McpError::ServiceSecretRefused {
                server: server_name,
                refusal,
            });
        }
        if let Some(refusal) = attachment_refusal {
            return Err(McpError::AttachmentRefused(refusal));
        }
        if let Some(message) = connect_error {
            return Err(McpError::ProtocolError { message });
        }

        if self.servers.contains_key(&server_name) {
            Ok(())
        } else {
            Err(McpError::ProtocolError {
                message: "surface owner did not activate MCP server after successful connect"
                    .into(),
            })
        }
    }

    /// Process removal finalization based on owner-owned timeout tracking.
    ///
    /// A surface-owner rejection of a finalize-removal input is authoritative
    /// divergence: the router has computed a finalized set the machine refuses
    /// to commit, so router/surface state would silently diverge if we
    /// continued. Fail closed by surfacing the rejection as a typed
    /// [`McpError`] instead of warn-and-continue.
    async fn process_removals(
        &mut self,
        delta: &mut McpApplyDelta,
        snapshot_alignment: &mut Option<SurfaceSnapshotAlignmentObligation>,
    ) -> Result<(), McpError> {
        let now = Instant::now();
        let mut finalized: Vec<(String, bool)> = Vec::new();

        let removing = self.surface_owner.removing_surfaces();
        for sid in &removing {
            let inflight = self.surface_owner.inflight_call_count(sid);
            let timing = self.surface_owner.removal_timing(sid);

            if inflight == 0 {
                finalized.push((sid.0.clone(), false));
            } else if let Some(timing) = timing
                && now >= timing.timeout_at
            {
                finalized.push((sid.0.clone(), true));
            }
        }

        for (server_name, degraded) in finalized {
            let sid = SurfaceId::from(server_name.as_str());
            let applied_at_turn = match self.surface_owner.removal_timing(&sid) {
                Some(timing) => timing.applied_at_turn,
                None => {
                    tracing::warn!(
                        server = %server_name,
                        "removal finalization missing owner timing; falling back to snapshot epoch"
                    );
                    self.turn_number_from_snapshot_epoch()
                }
            };
            let input = if degraded {
                ExternalToolSurfaceInput::FinalizeRemovalForced {
                    surface_id: sid,
                    applied_at_turn,
                }
            } else {
                ExternalToolSurfaceInput::FinalizeRemovalClean {
                    surface_id: sid,
                    applied_at_turn,
                }
            };
            let transition =
                self.surface_owner
                    .apply(input)
                    .map_err(|e| McpError::ProtocolError {
                        message: format!(
                            "surface owner rejected finalize removal for '{server_name}': {e}"
                        ),
                    })?;
            self.execute_effects(
                &transition.effects,
                delta,
                None,
                None,
                None,
                snapshot_alignment,
            );
            // The removal's disposition: the name is no longer governed.
            self.governed_servers.remove(&server_name);
            if degraded {
                delta.degraded_removals.push(server_name);
            }
            self.progress
                .send_modify(|seen| *seen = seen.wrapping_add(1));
        }
        Ok(())
    }

    fn align_snapshot_if_requested(
        &mut self,
        snapshot_alignment: Option<SurfaceSnapshotAlignmentObligation>,
    ) {
        let Some(obligation) = snapshot_alignment else {
            return;
        };
        if let Err(error) = self
            .surface_owner
            .apply(ExternalToolSurfaceInput::SnapshotAligned {
                snapshot_epoch: obligation.snapshot_epoch,
            })
        {
            tracing::warn!(
                snapshot_epoch = obligation.snapshot_epoch,
                error = %error,
                "Surface owner rejected SnapshotAligned"
            );
        }
    }

    async fn close_entry_connection(server_name: String, conn: Option<McpConnection>) {
        if let Some(conn) = conn
            && let Err(e) = conn.close().await
        {
            tracing::debug!("Error closing MCP connection '{}': {}", server_name, e);
        }
    }

    async fn close_result_connection_if_present(
        server_name: String,
        result: Result<(McpConnection, Vec<Arc<ToolDef>>), McpError>,
    ) {
        if let Ok((conn, _)) = result {
            Self::close_entry_connection(server_name, Some(conn)).await;
        }
    }

    /// Close `conn` in a router-owned task: the router does not block on the
    /// close, and `shutdown` joins it.
    fn spawn_close(&mut self, server_name: String, conn: Option<McpConnection>) {
        let Some(conn) = conn else {
            return;
        };
        self.reap_closing();
        self.spawn_close_task(server_name, conn);
    }

    /// Close (or join the close of) a connect attempt's process through its
    /// custody, in a router-owned task that `shutdown` joins.
    fn spawn_custody_close(&mut self, server: String, custody: StdioChildCustody) {
        let name = server.clone();
        let indexed = self.close_index.covers_process(&custody);
        let task = self.closing.spawn(async move {
            let outcome = pending_close_outcome(&server, &custody).await;
            McpServerCloseRecord { server, outcome }
        });
        self.closing_servers.insert(task.id(), name);
        if indexed {
            self.indexed_closes.insert(task.id());
        }
    }

    /// [`Self::spawn_close`] without reaping finished closes first.
    fn spawn_close_task(&mut self, server_name: String, conn: McpConnection) {
        let server = server_name.clone();
        let indexed = self.close_index.covers_client(conn.client_custody());
        let task = self.closing.spawn(async move {
            let outcome = conn.close_with_outcome().await;
            McpServerCloseRecord { server, outcome }
        });
        self.closing_servers.insert(task.id(), server_name);
        if indexed {
            self.indexed_closes.insert(task.id());
        }
    }

    /// Reap finished closes so the set stays bounded in a long-lived router,
    /// keeping each one outside the close index that did not settle for the
    /// shutdown report (the index keeps its own).
    fn reap_closing(&mut self) {
        while let Some(joined) = self.closing.try_join_next_with_id() {
            let (record, indexed) = self.closed_record(joined);
            Self::report_close(&record);
            if !indexed {
                self.retain_if_unsettled(record);
            }
        }
    }

    /// The record of one joined close task, and whether the close index
    /// holds its close; a task that ended without reporting is recorded
    /// against the server it was closing.
    fn closed_record(
        &mut self,
        joined: Result<(tokio::task::Id, McpServerCloseRecord), tokio::task::JoinError>,
    ) -> (McpServerCloseRecord, bool) {
        match joined {
            Ok((id, record)) => {
                self.closing_servers.remove(&id);
                (record, self.indexed_closes.remove(&id))
            }
            Err(error) => {
                let id = error.id();
                let record = McpServerCloseRecord {
                    server: self.closing_servers.remove(&id).unwrap_or_default(),
                    outcome: McpServerCloseOutcome::TaskFailed {
                        panicked: error.is_panic(),
                    },
                };
                (record, self.indexed_closes.remove(&id))
            }
        }
    }

    fn retain_if_unsettled(&mut self, record: McpServerCloseRecord) {
        if !record.outcome.is_settled() {
            self.unsettled_closes.push(record);
        }
    }

    /// Trace a close's failure; receipts and closes owned elsewhere were
    /// traced by the close itself.
    fn report_close(record: &McpServerCloseRecord) {
        let server = &record.server;
        match &record.outcome {
            McpServerCloseOutcome::CloseFailed(error) => {
                tracing::warn!(server = %server, error = %error, "MCP connection close failed");
            }
            McpServerCloseOutcome::TaskFailed { panicked: true } => {
                tracing::error!(server = %server, "MCP connection close task panicked");
            }
            McpServerCloseOutcome::TaskFailed { panicked: false } => {
                tracing::warn!(server = %server, "MCP connection close task was cancelled");
            }
            McpServerCloseOutcome::Stdio(_)
            | McpServerCloseOutcome::NoProcess
            | McpServerCloseOutcome::NeverSpawned
            | McpServerCloseOutcome::NotHeld(_) => {}
        }
    }

    /// Refuse a server whose exposure map, access policy, secret declaration
    /// or working directory is invalid, before any lifecycle or transport
    /// effect.
    fn validate_tool_names(config: &McpServerConfig) -> Result<(), McpError> {
        use meerkat_core::tool_catalog::{TOOL_CATALOG_LOAD_NAME, TOOL_CATALOG_SEARCH_NAME};
        config.validate_access()?;
        config.validate_secret_env()?;
        config.validate_cwd()?;
        for (raw_operation, exposed) in &config.tool_names {
            let reason = if raw_operation.is_empty() {
                Some("raw operation must not be empty")
            } else if exposed.is_empty()
                || exposed.len() > 64
                || !exposed
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
            {
                Some("exposed name must contain 1-64 ASCII letters, digits, underscores or hyphens")
            } else if exposed == TOOL_CATALOG_SEARCH_NAME || exposed == TOOL_CATALOG_LOAD_NAME {
                Some("exposed name is reserved for the catalog control plane")
            } else {
                None
            };
            if let Some(reason) = reason {
                return Err(McpError::InvalidToolNameMapping {
                    server: config.name.clone(),
                    raw_operation: raw_operation.clone(),
                    reason,
                });
            }
        }
        Ok(())
    }

    fn tool_definitions_equivalent(left: &ToolDef, right: &ToolDef) -> bool {
        left.description == right.description
            && left.input_schema == right.input_schema
            && left.provenance == right.provenance
            && left.audience == right.audience
    }

    fn publish_projection_snapshot(&mut self) -> bool {
        let epoch = self.surface_owner.snapshot_epoch();
        let server_names: BTreeSet<String> = self
            .surface_owner
            .visible_surfaces()
            .into_iter()
            .map(|sid| sid.0)
            .collect();

        let mut tool_routes: HashMap<String, McpToolRoute> = HashMap::new();
        let mut canonical_tools: BTreeMap<String, Arc<ToolDef>> = BTreeMap::new();
        let mut collision_routes: BTreeMap<String, Vec<McpToolRoute>> = BTreeMap::new();
        let mut collided: BTreeSet<String> = BTreeSet::new();
        let mut complete = true;

        for server_name in server_names {
            let Some(entry) = self.servers.get(&server_name) else {
                tracing::warn!(
                    server = %server_name,
                    "Visible server has no shell entry during projection publish; publishing conservative snapshot without this server"
                );
                complete = false;
                continue;
            };
            let mut server_tools: BTreeMap<String, Arc<ToolDef>> = BTreeMap::new();
            let mut same_server_divergent: BTreeSet<String> = BTreeSet::new();

            for tool in &entry.tools {
                let tool_name = tool.name.to_string();
                if let Some(existing) = server_tools.get(&tool_name) {
                    if !Self::tool_definitions_equivalent(existing, tool) {
                        tracing::warn!(
                            tool = %tool.name,
                            server = %server_name,
                            "MCP projection saw divergent duplicate tool definitions from one server; excluding the tool from the published catalog (ambiguous => denied)"
                        );
                        same_server_divergent.insert(tool_name);
                        complete = false;
                    }
                    continue;
                }
                server_tools.insert(tool_name, Arc::clone(tool));
            }

            for name in &same_server_divergent {
                server_tools.remove(name);
                let exposed = entry.config.tool_names.get(name).unwrap_or(name);
                collided.insert(exposed.clone());
            }

            for (raw_operation, tool) in server_tools {
                // An operation outside the server's access ceiling is not
                // offered or routed under any name (its connection also
                // refuses the call).
                if !entry.config.permits_operation(&raw_operation) {
                    continue;
                }
                let exposed = entry
                    .config
                    .tool_names
                    .get(&raw_operation)
                    .unwrap_or(&raw_operation)
                    .clone();
                // Discovery owns provenance. Renaming must never repair or
                // reinterpret a conflicting source identity.
                if tool.provenance.as_ref().is_some_and(|source| {
                    source.kind != meerkat_core::types::ToolSourceKind::Mcp
                        || source.source_id.as_str() != server_name
                }) {
                    collided.insert(exposed.clone());
                    complete = false;
                    continue;
                }
                let route = McpToolRoute {
                    server_name: server_name.clone(),
                    raw_operation,
                };
                // Distinct raw operations from the SAME server can collide too.
                if let Some(existing) = tool_routes.get(&exposed) {
                    collision_routes
                        .entry(exposed.clone())
                        .or_insert_with(|| vec![existing.clone()])
                        .push(route);
                    tracing::warn!(
                        tool = %exposed,
                        existing_server = %existing.server_name,
                        server = %server_name,
                        "MCP exposure name has multiple raw routes; excluding every colliding owner"
                    );
                    collided.insert(exposed.clone());
                    complete = false;
                    continue;
                }
                let projected = if exposed == tool.name.as_str() {
                    tool
                } else {
                    let mut projected = (*tool).clone();
                    projected.name = exposed.clone().into();
                    Arc::new(projected)
                };
                tool_routes.insert(exposed.clone(), route);
                canonical_tools.insert(exposed, projected);
            }
        }

        // Remove every collided name so a tool exposed by two servers is
        // routable from neither (the first writer is also dropped here).
        for name in &collided {
            tool_routes.remove(name);
            canonical_tools.remove(name);
        }

        let catalog_entries: Arc<[ToolCatalogEntry]> = canonical_tools
            .values()
            .map(|tool| {
                if let Some(provenance) = tool.provenance.clone() {
                    ToolCatalogEntry::session_deferred(Arc::clone(tool), true, provenance)
                } else {
                    ToolCatalogEntry::session_inline(Arc::clone(tool), true)
                }
            })
            .collect::<Vec<_>>()
            .into();
        let visible_tools: Arc<[Arc<ToolDef>]> = catalog_entries
            .iter()
            .map(|entry| Arc::clone(&entry.tool))
            .collect::<Vec<_>>()
            .into();

        let name_collisions: Arc<[McpToolNameCollision]> = collision_routes
            .into_iter()
            .map(|(exposed_name, routes)| McpToolNameCollision {
                exposed_name,
                routes,
            })
            .collect::<Vec<_>>()
            .into();
        self.projection = Arc::new(RouterProjectionSnapshot {
            epoch,
            tool_routes,
            name_collisions,
            catalog_entries,
            visible_tools,
        });
        complete
    }

    /// Current exposed-name collisions from the same published routing snapshot.
    /// Every listed route is excluded until explicit configuration resolves it.
    pub fn tool_name_collisions(&self) -> Arc<[McpToolNameCollision]> {
        Arc::clone(&self.projection.name_collisions)
    }

    fn projection_tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.projection.visible_tools)
    }

    fn projection_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        Arc::clone(&self.projection.catalog_entries)
    }

    /// Get current lifecycle state for a server.
    ///
    /// Derives the lifecycle state from the surface owner's canonical state.
    pub fn server_lifecycle_state(&self, server_name: &str) -> Option<McpServerLifecycleState> {
        let sid = SurfaceId::from(server_name);
        let base = self.surface_owner.surface_base(&sid);
        match base {
            SurfaceBaseState::Active => Some(McpServerLifecycleState::Active),
            SurfaceBaseState::Removing => {
                let timing = self.surface_owner.removal_timing(&sid);
                match timing {
                    Some(t) => Some(McpServerLifecycleState::Removing {
                        draining_since: t.draining_since,
                        timeout_at: t.timeout_at,
                    }),
                    None => {
                        tracing::warn!(
                            server = %server_name,
                            "surface owner reported Removing without generated removal timing"
                        );
                        None
                    }
                }
            }
            SurfaceBaseState::Removed => Some(McpServerLifecycleState::Removed),
            SurfaceBaseState::Absent => None,
        }
    }

    /// List names of all active (non-removed) servers.
    pub fn active_server_names(&self) -> Vec<String> {
        self.surface_owner
            .visible_surfaces()
            .into_iter()
            .map(|sid| sid.0)
            .collect()
    }

    /// Returns true if any server is currently in Removing state.
    pub fn has_removing_servers(&self) -> bool {
        !self.surface_owner.removing_surfaces().is_empty()
    }

    /// What the removal drain waits on next: `None` when no server is
    /// draining; otherwise a wait that resolves at once if a draining server
    /// can finalize now (no call in flight, or its removal timeout passed),
    /// else on the next finished call or the earliest removal timeout.
    pub(crate) fn removal_progress_wait(&self) -> Option<McpProgressWait> {
        let removing = self.surface_owner.removing_surfaces();
        if removing.is_empty() {
            return None;
        }
        let progress = self.progress.subscribe();
        let now = Instant::now();
        let mut deadline: Option<Instant> = None;
        for sid in &removing {
            if self.surface_owner.inflight_call_count(sid) == 0 {
                return Some(McpProgressWait::ready());
            }
            if let Some(timing) = self.surface_owner.removal_timing(sid) {
                if now >= timing.timeout_at {
                    return Some(McpProgressWait::ready());
                }
                deadline = Some(deadline.map_or(timing.timeout_at, |d| d.min(timing.timeout_at)));
            }
        }
        Some(McpProgressWait {
            progress: Some(progress),
            deadline,
        })
    }

    /// Whether a spawned connect attempt has not yet delivered its result.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn connect_results_outstanding(&self) -> bool {
        self.connect_results_delivered.load(Ordering::Acquire) != self.connect_attempts_spawned
    }

    /// A wait for the next connect result (or finished call); taken before
    /// reading pending state, so a result delivered after the read is seen.
    pub(crate) fn progress_wait(&self) -> McpProgressWait {
        McpProgressWait {
            progress: Some(self.progress.subscribe()),
            deadline: None,
        }
    }

    /// List all visible tools from active servers.
    pub fn list_tools(&self) -> &[Arc<ToolDef>] {
        self.projection.visible_tools.as_ref()
    }

    /// Progress only server removals (drain/timeout finalization) without applying staged ops.
    ///
    /// Fails closed if the surface owner rejects a finalize-removal: a rejected
    /// finalize is authoritative divergence (router computed a finalized set the
    /// machine refuses to commit), not a benign no-op.
    pub async fn progress_removals(&mut self) -> Result<McpApplyDelta, McpError> {
        let mut delta = McpApplyDelta::default();
        let mut snapshot_alignment = None;
        self.process_removals(&mut delta, &mut snapshot_alignment)
            .await?;
        self.record_snapshot_alignment(snapshot_alignment);
        let published = self.publish_projection_snapshot();
        if published {
            let obligation = self.pending_snapshot_alignment.take();
            self.align_snapshot_if_requested(obligation);
        }
        Ok(delta)
    }

    /// Call a tool by name, retaining the historical content-only error API.
    pub async fn call_tool(&self, name: &str, args: &Value) -> Result<Vec<ContentBlock>, McpError> {
        let raw = serde_json::value::to_raw_value(args)
            .map_err(|_| McpError::Serialization("invalid MCP arguments".into()))?;
        self.call_tool_with_context(
            ToolCallView {
                id: "",
                name,
                args: &raw,
            },
            args,
            &meerkat_core::ToolDispatchContext::default(),
            |result| crate::protocol::convert_tool_result(result, name),
        )
        .await
    }

    fn app_connection<'a>(
        &'a self,
        source: &str,
        invocation: &crate::apps::McpAppInvocation,
    ) -> Result<&'a McpConnection, McpError> {
        self.app_source_connection(source, &invocation.registration, &invocation.tool)
    }

    fn app_source_connection<'a>(
        &'a self,
        source: &str,
        registration: &crate::apps::McpAppRegistration,
        tool: &rmcp::model::Tool,
    ) -> Result<&'a McpConnection, McpError> {
        let route = self
            .projection
            .tool_routes
            .get(source)
            .ok_or_else(|| McpError::ToolNotFound(source.into()))?;
        let connection = self
            .servers
            .get(&route.server_name)
            .and_then(|entry| entry.connection.as_ref())
            .ok_or_else(|| McpError::ServerNotFound(route.server_name.clone()))?;
        if !connection.supports_mcp_apps()
            || route.raw_operation != tool.name.as_ref()
            || crate::apps::McpAppRegistration::from_connection(connection) != *registration
            || !matches!(
                self.server_lifecycle_state(&route.server_name),
                Some(McpServerLifecycleState::Active)
            )
        {
            return Err(McpError::CallContext(crate::McpCallContextError::Denied));
        }
        Ok(connection)
    }

    fn validate_app_call(
        &self,
        call: ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
    ) -> Result<(), McpError> {
        let Some(binding) = context.application_binding() else {
            return Ok(());
        };
        if binding.extension != crate::apps::MCP_APPS_EXTENSION {
            return Err(McpError::CallContext(crate::McpCallContextError::Denied));
        }
        let binding: crate::apps::McpAppCallBinding =
            serde_json::from_value(binding.payload.clone())
                .map_err(|_| crate::McpCallContextError::Denied)?;
        let connection = self.app_connection(&binding.source_tool, &binding.invocation)?;
        let route = self
            .projection
            .tool_routes
            .get(call.name)
            .ok_or_else(|| McpError::ToolNotFound(call.name.into()))?;
        let target = connection
            .standard_tool(&route.raw_operation)
            .ok_or(crate::McpCallContextError::Denied)?;
        if route.server_name != binding.invocation.registration.server
            || route.raw_operation != binding.target
            || !crate::apps::tool_audience(&target)?.allows_app()
        {
            return Err(McpError::CallContext(crate::McpCallContextError::Denied));
        }
        Ok(())
    }

    async fn read_app_resource(
        &self,
        source: &str,
        registration: &crate::apps::McpAppRegistration,
        tool: &rmcp::model::Tool,
        uri: &str,
        context: &meerkat_core::ToolDispatchContext,
    ) -> Result<rmcp::model::ReadResourceResult, McpError> {
        use meerkat_core::authorization::{
            AuthorizationOperation, OperationAuthorizationFacts, OperationObservedOutcome,
            PreparedAuthorizationBinding, PreparedOperationCheck, SourceAuthorizationFacts,
            SourceAuthorizationTarget, SourceAuthorizationUse,
        };
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        let connection = self.app_source_connection(source, registration, tool)?;
        let entry = self
            .servers
            .get(&registration.server)
            .ok_or(crate::McpCallContextError::Unavailable)?;
        let sid = SurfaceId::from(registration.server.as_str());
        let transition = self
            .surface_owner
            .apply(ExternalToolSurfaceInput::CallStarted {
                surface_id: sid.clone(),
            })
            .map_err(|_| crate::McpCallContextError::Unavailable)?;
        if transition
            .effects
            .iter()
            .any(|effect| matches!(effect, ExternalToolSurfaceEffect::RejectSurfaceCall { .. }))
        {
            return Err(McpError::CallContext(crate::McpCallContextError::Denied));
        }
        let lifetime = InflightCallGuard::new(
            &entry.active_calls,
            &self.surface_owner,
            sid,
            &self.progress,
        );
        let result = async {
            let target = || crate::McpResourceTarget {
                config: connection.config(),
                connection_id: connection.connection_id(),
                uri,
            };
            let prepared = if let Some(work) = context.work_authorization() {
                let target = self
                    .call_context_provider
                    .as_ref()
                    .and_then(|provider| provider.resource_authorization_target(target()))
                    .ok_or(crate::McpCallContextError::Unavailable)?;
                let facts = OperationAuthorizationFacts {
                    operation_id: meerkat_core::OperationId::new(),
                    execution_scope: work.execution_scope().clone(),
                    run_id: None,
                    context_revision: None,
                    operation: AuthorizationOperation::Source(SourceAuthorizationFacts {
                        target: SourceAuthorizationTarget::External(target),
                        usage: SourceAuthorizationUse::Read,
                    }),
                };
                Some(
                    PreparedOperationCheck::prepare(
                        work.clone(),
                        PreparedAuthorizationBinding::new(facts),
                    )
                    .and_then(|check| check.current())
                    .map_err(|error| McpError::EntryRefused(Box::new(error.into())))?,
                )
            } else {
                None
            };
            let preparation = match &self.call_context_provider {
                Some(provider) => {
                    tokio::time::timeout_at(deadline, provider.prepare_resource(target(), context))
                        .await
                        .map_err(|_| crate::McpCallContextError::Unavailable)??
                }
                None => None,
            };
            // A connector-bound server's reads need their connector
            // preparation, as its calls do. Without one (no provider, or a
            // provider that does not serve this destination) the read is
            // refused locally, never sent unbound.
            if preparation.is_none() && connection.config().connector.is_some() {
                return Err(McpError::CallContext(
                    crate::McpCallContextError::Unavailable,
                ));
            }
            let (metadata, _lease) = match preparation {
                Some(prepared) => (Some(prepared.metadata), Some(prepared.guard)),
                None => (None, None),
            };
            if let Some(control) = context.tool_application_control() {
                tokio::time::timeout_at(deadline, control.revalidate_async())
                    .await
                    .map_err(|_| crate::McpCallContextError::Unavailable)?
                    .map_err(|error| McpError::EntryRefused(Box::new(error.into())))?;
            }
            let prepared = prepared
                .map(|check| check.current())
                .transpose()
                .map_err(|error| McpError::EntryRefused(Box::new(error.into())))?;
            // This guard is created after the credential lease so cancellation
            // records the local unavailable outcome before releasing the lease.
            // Callers can have a deadline shorter than this read's own deadline.
            struct EnteredRead(Option<PreparedOperationCheck>);
            impl Drop for EnteredRead {
                fn drop(&mut self) {
                    if let Some(check) = self.0.take()
                        && check
                            .observe_outcome(OperationObservedOutcome::SourceReadUnavailable)
                            .is_err()
                    {
                        tracing::error!(
                            "MCP resource cancellation outcome observation unavailable"
                        );
                    }
                }
            }
            let mut entered = EnteredRead(None);
            let result = connection
                .read_resource_entering(uri, metadata, deadline, || {
                    self.app_source_connection(source, registration, tool)?;
                    if let Some(control) = context.tool_application_control() {
                        control
                            .revalidate()
                            .map_err(|error| McpError::EntryRefused(Box::new(error.into())))?;
                    }
                    if let Some(check) = &prepared {
                        let current = check
                            .current()
                            .map_err(|error| McpError::EntryRefused(Box::new(error.into())))?;
                        current
                            .require_unreviewed_entry()
                            .map_err(|error| McpError::EntryRefused(Box::new(error.into())))?;
                        current
                            .observe_entry(meerkat_core::OperationEntryBoundary::McpResourceRead)
                            .map_err(|_| crate::McpCallContextError::Unavailable)?;
                        entered.0 = Some(current);
                    }
                    Ok(())
                })
                .await;
            // Taking the exact entered check prevents Drop from recording a
            // second or different outcome, including if observation itself fails.
            if let Some(check) = entered.0.take() {
                check
                    .observe_outcome(if result.is_ok() {
                        OperationObservedOutcome::SourceReadMaterialized
                    } else {
                        OperationObservedOutcome::SourceReadUnavailable
                    })
                    .map_err(|_| crate::McpCallContextError::Unavailable)?;
                check
                    .current()
                    .map_err(|error| McpError::EntryRefused(Box::new(error.into())))?;
            }
            if let Some(control) = context.tool_application_control() {
                tokio::time::timeout_at(deadline, control.revalidate_async())
                    .await
                    .map_err(|_| crate::McpCallContextError::Unavailable)?
                    .map_err(|error| McpError::EntryRefused(Box::new(error.into())))?;
            }
            self.app_source_connection(source, registration, tool)?;
            result
        }
        .await;
        lifetime.finish()?;
        result
    }

    async fn call_tool_with_context<T>(
        &self,
        call: ToolCallView<'_>,
        args: &Value,
        context: &meerkat_core::ToolDispatchContext,
        project: impl FnOnce(rmcp::model::CallToolResult) -> Result<T, McpError>,
    ) -> Result<T, McpError> {
        let route = self
            .projection
            .tool_routes
            .get(call.name)
            .ok_or_else(|| McpError::ToolNotFound(call.name.to_string()))?;
        self.call_route_with_context(route, None, call, args, context, project)
            .await
    }

    /// One call of `route`: under its server's current access ceiling, or,
    /// through a checked route's `route_entry`, under exactly that route's
    /// selected operations and its own conjunct at the transport handoff.
    async fn call_route_with_context<T>(
        &self,
        route: &McpToolRoute,
        route_entry: Option<&dyn CheckedEntry>,
        call: ToolCallView<'_>,
        args: &Value,
        context: &meerkat_core::ToolDispatchContext,
        project: impl FnOnce(rmcp::model::CallToolResult) -> Result<T, McpError>,
    ) -> Result<T, McpError> {
        let server_name = &route.server_name;
        let entry = self
            .servers
            .get(server_name)
            .ok_or_else(|| McpError::ServerNotFound(server_name.clone()))?;
        let conn = entry
            .connection
            .as_ref()
            .ok_or_else(|| McpError::ServerNotFound(server_name.clone()))?;
        // The current connection's access ceiling (or the checked route's
        // own selection), before call accounting, host preparation and
        // review entry (its `tools/call` checks the same admission again
        // before sending).
        let admission = match route_entry {
            None => crate::connection::CallAdmission::Ceiling(conn.config().access.as_ref()),
            Some(entry) => crate::connection::CallAdmission::CheckedRoute(entry.operations()),
        };
        if !admission.admits(&route.raw_operation) {
            return Err(McpError::OperationNotPermitted {
                server: server_name.clone(),
                operation: route.raw_operation.clone(),
            });
        }
        let sid = SurfaceId::from(server_name.as_str());
        match self
            .surface_owner
            .apply(ExternalToolSurfaceInput::CallStarted {
                surface_id: sid.clone(),
            }) {
            Ok(transition) => {
                for effect in &transition.effects {
                    if let ExternalToolSurfaceEffect::RejectSurfaceCall { cause, .. } = effect {
                        return Err(McpError::ServerUnavailable {
                            server: server_name.clone(),
                            state: cause.as_str().to_owned(),
                        });
                    }
                }
            }
            Err(error) => {
                return Err(McpError::ServerUnavailable {
                    server: server_name.clone(),
                    state: error.to_string(),
                });
            }
        }
        let lifetime = InflightCallGuard::new(
            &entry.active_calls,
            &self.surface_owner,
            sid,
            &self.progress,
        );
        let result = async {
            let origin = meerkat_core::WireCallOrigin::Unavailable;
            let preparation = match &self.call_context_provider {
                Some(provider) => {
                    provider
                        .prepare(
                            crate::McpCallTarget {
                                config: conn.config(),
                                connection_id: conn.connection_id(),
                                raw_operation: &route.raw_operation,
                                origin: &origin,
                            },
                            call,
                            context,
                        )
                        .await?
                }
                None => None,
            };
            // A connector-bound server's calls need their connector
            // preparation. Without one (no provider, a binding the provider
            // refused to install, or a provider that does not serve it) the
            // call is refused locally, never sent unbound.
            if preparation.is_none() && conn.config().connector.is_some() {
                return Err(McpError::CallContext(
                    crate::McpCallContextError::Unavailable,
                ));
            }
            // The guard remains owned by this future until transport and
            // conversion finish, and drops immediately if the future is cancelled.
            let (metadata, _lease, final_check) = match preparation {
                Some(prepared) => {
                    let mut metadata = prepared.metadata;
                    if metadata.contains_key(meerkat_core::CALL_ORIGIN_META_KEY)
                        || metadata.contains_key("progressToken")
                    {
                        return Err(McpError::CallContext(crate::McpCallContextError::Denied));
                    }
                    metadata.insert(
                        meerkat_core::CALL_ORIGIN_META_KEY.to_string(),
                        serde_json::to_value(origin)
                            .map_err(|_| crate::McpCallContextError::Unavailable)?,
                    );
                    let lease: Arc<dyn Send + Sync> = Arc::from(prepared.guard);
                    (Some(metadata), Some(lease), prepared.final_check)
                }
                None => (None, None, None),
            };
            if let Some(control) = context.tool_application_control() {
                control
                    .revalidate_async()
                    .await
                    .map_err(|error| McpError::EntryRefused(Box::new(error.into())))?;
            }
            self.validate_app_call(call, context)?;
            // The native entry step runs inside the connection, after its
            // final request preparation and immediately before the local
            // transport handoff. The preparation's own final check, fallible,
            // runs first, so a refusal there leaves review unspent; then the
            // single consuming reviewed entry. The preparation lease and the
            // entered review's custody travel with the request to the native
            // send owner, and this future also holds the lease until the call
            // completes.
            let send_lease = _lease.clone();
            let generation = conn.connection_id().generation();
            let result = conn
                .call_tool_result_admitted(admission, &route.raw_operation, args, metadata, || {
                    self.validate_app_call(call, context)?;
                    if let Some(control) = context.tool_application_control() {
                        control
                            .revalidate()
                            .map_err(|error| McpError::EntryRefused(Box::new(error.into())))?;
                    }
                    // What the check guards stays held through the
                    // reviewed entry and the handoff, with the request.
                    let checked = final_check
                        .map(|check| check().map_err(McpError::CallContext))
                        .transpose()?;
                    // A checked route's own conjunct, after every await:
                    // its single entry is reserved here, and released only
                    // if the native entry below refuses.
                    if let Some(entry) = route_entry {
                        entry
                            .reserve_entry(call, context, generation)
                            .map_err(|error| McpError::EntryRefused(Box::new(error)))?;
                    }
                    let (entering, review) = context
                        .enter_reviewed_effect_held(call, None)
                        .map_err(|error| {
                            if let Some(entry) = route_entry {
                                entry.release_entry();
                            }
                            McpError::EntryRefused(Box::new(error))
                        })?;
                    Ok(Some(crate::transport::protected::SendCustody::new((
                        send_lease, checked, review, entering,
                    ))))
                })
                .await?;
            project(result)
        }
        .await;
        // A call that found no usable credential releases the server at the
        // next boundary, as a sign-out in this process would.
        if let Err(McpError::AuthorizationRequired { target }) = &result {
            self.calls_refused_unauthorized
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((server_name.clone(), (**target).clone()));
        }
        // Refuse a result whose canonical completion was rejected.
        lifetime.finish()?;
        result
    }

    /// This router's close index, for its adapter.
    pub(crate) fn close_index(&self) -> crate::connection::CloseIndex {
        self.close_index.clone()
    }

    /// Gracefully shutdown all connections.
    ///
    /// [`Self::shutdown_with_report`], discarding the report.
    pub async fn shutdown(self) {
        let _ = self.shutdown_with_report().await;
    }

    /// Gracefully shut down all connections, and report what every close the
    /// router owned observed ([`McpRouterShutdownReport`]).
    ///
    /// Returns once every one of those closes has ended; none is aborted. A
    /// stdio server's close ends within
    /// [`crate::McpStdioCloseBounds::close_bound`] of the bounds its
    /// connection captured, counted from when that close started.
    pub async fn shutdown_with_report(mut self) -> McpRouterShutdownReport {
        // Start every close now, through the close index: attempts stop, and
        // the closes below join these. The ticket reports every close the
        // index holds, from its original custody.
        let ticket = self.close_index.begin_close();
        // First consume any finished pending tasks through normal processing so
        // stale completion payloads close their transports via existing paths.
        self.drain_pending();
        // An owned surface is shut down; a borrowed one outlives this router,
        // which releases only what it staged.
        self.surface_owner.end_holding();
        self.pending_obligations.clear();
        self.pending_snapshot_alignment = None;
        self.completed_updates.clear();
        self.staged_payloads.clear();
        self.awaiting_authorization.clear();
        self.failed_stdio_attempts.clear();
        self.pending_stdio_diagnostics.clear();
        self.staged_controls.clear();
        self.staged_attempts.clear();
        self.pending_attempts.clear();
        self.refused_payloads.clear();
        let (replacement_tx, _replacement_rx) = mpsc::channel(PENDING_CHANNEL_CAPACITY);
        let old_pending_tx = std::mem::replace(&mut self.pending_tx, replacement_tx);
        drop(old_pending_tx);
        // Abort and join every connect task still running, then close the
        // stdio process of every attempt whose result was never processed.
        // The custody outlives the aborted attempt, and so does a close the
        // attempt already started (after a failed handshake or enumeration):
        // that close runs in its own task, which the close below joins.
        self.connect_tasks.abort_all();
        while let Some(joined) = self.connect_tasks.join_next().await {
            if let Err(error) = joined
                && error.is_panic()
            {
                tracing::warn!("MCP connect task panicked before shutdown: {error}");
            }
        }
        // Every connect task has ended, so the channel now holds every result
        // delivered after the drain above.
        let drained_results: Vec<PendingResult> = {
            let mut rx = match self.pending_rx.lock() {
                Ok(guard) => guard,
                Err(poisoned) => {
                    tracing::warn!(
                        "MCP pending_rx mutex was poisoned during shutdown; recovering receiver"
                    );
                    poisoned.into_inner()
                }
            };
            let mut drained = Vec::new();
            while let Ok(result) = rx.try_recv() {
                drained.push(result);
            }
            drained
        };
        // Reap closes that already ended, keeping each unsettled one; every
        // close below then runs as a router-owned task joined at the end, so
        // servers use their close grace concurrently rather than in turn.
        self.reap_closing();
        let mut delivered = Vec::new();
        for pending in drained_results {
            if let Ok((conn, _)) = pending.result {
                // The delivered connection carries the attempt's custody, so
                // its close is that process's one close.
                self.pending_child_custody.remove(&(
                    pending.obligation.surface_id.clone(),
                    pending.obligation.pending_task_sequence,
                ));
                delivered.push((pending.obligation.surface_id, conn));
            }
        }
        for ((server, _), custody) in std::mem::take(&mut self.pending_child_custody) {
            self.spawn_custody_close(server, custody);
        }
        for (server, conn) in delivered {
            self.spawn_close_task(server, conn);
        }
        let servers = std::mem::take(&mut self.servers);
        for (_, entry) in servers {
            if let Some(conn) = entry.connection {
                self.spawn_close_task(entry.config.name, conn);
            }
        }
        let mut closes = std::mem::take(&mut self.unsettled_closes);
        while let Some(joined) = self.closing.join_next_with_id().await {
            let (record, indexed) = self.closed_record(joined);
            Self::report_close(&record);
            if !indexed {
                closes.push(record);
            }
        }
        for record in ticket.outcomes().await {
            Self::report_close(&record);
            closes.push(record);
        }
        let _ = self.publish_projection_snapshot();
        // The router's guard calls: its begins (the cancel bound), then its
        // finishes (the settle bound), now that every close ended.
        let guards = match &self.attempt_guards {
            Some(guards) => guards.join().await,
            None => Vec::new(),
        };
        self.shut_down = true;
        McpRouterShutdownReport {
            servers: closes,
            guards,
        }
    }

    /// Test hook: set a server's in-flight call count, through the surface
    /// owner like real calls. Fails closed when the server is not installed
    /// or the owner rejects a call transition (the session machine accepts
    /// calls only while attached or running), instead of leaving the shell
    /// and the owner disagreeing about the count.
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_inflight_calls_for_testing(
        &mut self,
        server_name: &str,
        count: usize,
    ) -> Result<(), McpError> {
        let sid = SurfaceId::from(server_name);
        let entry = self
            .servers
            .get_mut(server_name)
            .ok_or_else(|| McpError::ServerNotFound(server_name.to_string()))?;
        let current = entry.active_calls.load(Ordering::Acquire);
        let input = |started: bool| {
            if started {
                ExternalToolSurfaceInput::CallStarted {
                    surface_id: sid.clone(),
                }
            } else {
                ExternalToolSurfaceInput::CallFinished {
                    surface_id: sid.clone(),
                }
            }
        };
        let (started, steps) = if count > current {
            (true, count - current)
        } else {
            (false, current - count)
        };
        for _ in 0..steps {
            self.surface_owner.apply(input(started)).map_err(|error| {
                McpError::ServerUnavailable {
                    server: server_name.to_string(),
                    state: format!("surface owner rejected the test call transition: {error}"),
                }
            })?;
            if started {
                entry.active_calls.fetch_add(1, Ordering::AcqRel);
            } else {
                entry.active_calls.fetch_sub(1, Ordering::AcqRel);
            }
        }
        self.progress
            .send_modify(|seen| *seen = seen.wrapping_add(1));
        Ok(())
    }
}

#[async_trait]
impl AgentToolDispatcher for McpRouter {
    async fn resolve_tool_application(
        &self,
        source_tool: &str,
        request: &meerkat_core::ToolApplicationRequest,
        invocation: &serde_json::Value,
        context: &meerkat_core::ToolDispatchContext,
    ) -> Result<meerkat_core::tool_application::ToolApplicationResolution, ToolError> {
        use meerkat_core::ToolApplicationOperation;
        use meerkat_core::tool_application::{ToolApplicationBinding, ToolApplicationResolution};
        if request.extension != crate::apps::MCP_APPS_EXTENSION {
            return Err(ToolError::access_denied(source_tool));
        }
        let invocation: crate::apps::McpAppInvocation = serde_json::from_value(invocation.clone())
            .map_err(|_| ToolError::access_denied(source_tool))?;
        let uri = crate::apps::tool_ui_resource_uri(&invocation.tool)
            .ok_or_else(|| ToolError::access_denied(source_tool))?;
        match &request.operation {
            ToolApplicationOperation::Resolve => {
                let can_call_tools = self.app_connection(source_tool, &invocation).is_ok();
                Ok(ToolApplicationResolution::Value(serde_json::json!({
                    "tool": invocation.tool, "arguments": invocation.arguments,
                    "result": invocation.result, "resource": invocation.resource,
                    "canCallTools": can_call_tools,
                })))
            }
            ToolApplicationOperation::ReadResource { uri: requested } => {
                if requested == uri
                    && let Some(resource) = &invocation.resource
                {
                    return serde_json::to_value(resource)
                        .map(ToolApplicationResolution::Value)
                        .map_err(|_| ToolError::execution_failed("invalid cached MCP resource"));
                }
                self.read_app_resource(
                    source_tool,
                    &invocation.registration,
                    &invocation.tool,
                    requested,
                    context,
                )
                .await
                .map_err(|error| tool_call_error(source_tool, error))
                .and_then(|value| {
                    serde_json::to_value(value)
                        .map(ToolApplicationResolution::Value)
                        .map_err(|_| ToolError::execution_failed("invalid MCP resource"))
                })
            }
            ToolApplicationOperation::CallTool { name, .. } => {
                let connection = self
                    .app_connection(source_tool, &invocation)
                    .map_err(|error| tool_call_error(source_tool, error))?;
                let target = connection
                    .standard_tool(name)
                    .ok_or_else(|| ToolError::access_denied(name))?;
                if !crate::apps::tool_audience(&target)
                    .map_err(|error| tool_call_error(name, error))?
                    .allows_app()
                {
                    return Err(ToolError::access_denied(name));
                }
                let mut routes = self.projection.tool_routes.iter().filter(|(_, route)| {
                    route.server_name == invocation.registration.server
                        && route.raw_operation == *name
                });
                let (canonical_name, _) = routes
                    .next()
                    .ok_or_else(|| ToolError::access_denied(name))?;
                if routes.next().is_some() {
                    return Err(ToolError::access_denied(name));
                }
                let payload = serde_json::to_value(crate::apps::McpAppCallBinding {
                    source_tool: source_tool.into(),
                    invocation,
                    target: name.clone(),
                })
                .map_err(|_| ToolError::access_denied(name))?;
                Ok(ToolApplicationResolution::Call {
                    name: canonical_name.clone(),
                    binding: ToolApplicationBinding::new(crate::apps::MCP_APPS_EXTENSION, payload),
                    project_result: crate::apps::project_app_result,
                })
            }
        }
    }

    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.projection_tools()
    }

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        ToolCatalogCapabilities {
            exact_catalog: true,
            may_require_catalog_control_plane: true,
        }
    }

    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        self.projection_catalog()
    }

    fn pending_catalog_sources(&self) -> Arc<[String]> {
        self.pending_sources_snapshot().into()
    }

    fn tool_mutation_class(&self, tool_name: &str) -> meerkat_core::ToolMutationClass {
        let Some(route) = self.projection.tool_routes.get(tool_name) else {
            return meerkat_core::ToolMutationClass::Unknown;
        };
        let Some(connection) = self
            .servers
            .get(&route.server_name)
            .and_then(|entry| entry.connection.as_ref())
        else {
            return meerkat_core::ToolMutationClass::Unknown;
        };
        self.call_context_provider
            .as_ref()
            .map_or(meerkat_core::ToolMutationClass::Unknown, |provider| {
                provider.tool_mutation_class(connection.config(), &route.raw_operation)
            })
    }

    /// Routed MCP tools carry review to their local transport handoff: the
    /// connection runs `enter_reviewed_effect` exactly once, after
    /// call-context and request preparation, immediately before the send.
    fn review_entry_support(
        &self,
        tool_name: &str,
    ) -> meerkat_core::approval::review::ReviewEntrySupport {
        if self.projection.tool_routes.contains_key(tool_name) {
            meerkat_core::approval::review::ReviewEntrySupport::ConsumesAtEntry
        } else {
            meerkat_core::approval::review::ReviewEntrySupport::Unsupported
        }
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
        self.dispatch_with_context(call, &meerkat_core::ToolDispatchContext::default())
            .await
    }

    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
        // The adapter holds the router read guard here. Recheck the exact
        // live destination after any asynchronous outer consequence decision.
        if context.read_only_execution_required()
            && self.tool_mutation_class(call.name) != meerkat_core::ToolMutationClass::ReadOnly
        {
            return Err(ToolError::access_denied(call.name));
        }
        let args = meerkat_core::ToolCallArguments::from_raw_json(call.args)
            .map_err(|err| ToolError::invalid_arguments(call.name, err.to_string()))?;
        let app_action = context
            .application_binding()
            .is_some_and(|binding| binding.extension == crate::apps::MCP_APPS_EXTENSION);
        let host_source = self
            .projection
            .tool_routes
            .get(call.name)
            .and_then(|route| {
                let connection = self.servers.get(&route.server_name)?.connection.as_ref()?;
                if !connection.supports_mcp_apps() {
                    return None;
                }
                let tool = connection.standard_tool(&route.raw_operation)?;
                if crate::apps::tool_ui_resource_uri(&tool).is_none() && !app_action {
                    return None;
                }
                Some((connection, tool))
            });
        let prefetch_ui = !app_action && host_source.is_some();
        let host_source = host_source.map(|(connection, tool)| {
            (
                crate::apps::McpAppRegistration::from_connection(connection),
                tool,
            )
        });
        // Load the declared view before the effectful tool call. Optional UI IO
        // after a successful call could otherwise consume an outer deadline and
        // discard a known tool result. These are exact native source facts, not
        // a fabricated invocation; resource reads retain their own authorization.
        let resource = if prefetch_ui
            && let Some((registration, tool)) = &host_source
            && let Some(uri) = crate::apps::tool_ui_resource_uri(tool)
        {
            self.read_app_resource(call.name, registration, tool, uri, context)
                .await
                .ok()
        } else {
            None
        };
        let result = self
            .call_tool_with_context(call, args.as_value(), context, |result| {
                let host_invocation = host_source
                    .map(|(registration, tool)| {
                        serde_json::to_value(crate::apps::McpAppInvocation {
                            registration,
                            tool,
                            arguments: args.as_value().clone(),
                            result: result.clone(),
                            resource,
                        })
                        .map_err(|_| McpError::Serialization("invalid MCP host result".into()))
                    })
                    .transpose()?;
                let (blocks, is_error) = crate::protocol::project_tool_result(result, call.name)?;
                let mut result = ToolResult::with_blocks(call.id.to_string(), blocks, is_error);
                if let Some(invocation) = host_invocation {
                    result
                        .host_metadata
                        .insert(crate::apps::MCP_APPS_EXTENSION.into(), invocation);
                }
                Ok(result)
            })
            .await
            .map_err(|error| tool_call_error(call.name, error))?;
        Ok(result.into())
    }

    fn external_tool_surface_snapshot(&self) -> Option<meerkat_core::ExternalToolSurfaceSnapshot> {
        Some(McpRouter::external_tool_surface_snapshot(self))
    }
}

/// An unbound router whose local stdio servers launch as the trusted host,
/// with no OS confinement, as
/// `McpRouter::new_with_stdio_profile(McpStdioLaunchProfile::trusted_host())`
/// names it.
impl Default for McpRouter {
    fn default() -> Self {
        Self::new_with_stdio_profile(crate::McpStdioLaunchProfile::trusted_host())
    }
}

/// The tool-surface error of a failed MCP `tools/call`. A native entry
/// refusal keeps its exact typed tool error instead of being flattened into an
/// execution string.
fn tool_call_error(tool: &str, error: McpError) -> ToolError {
    match error {
        McpError::ToolNotFound(name) => ToolError::NotFound { name },
        McpError::EntryRefused(error) => *error,
        McpError::Confinement(refusal) => ToolError::ConfinementRefused { refusal },
        // Refused locally by the host egress profile before anything was
        // sent: the tool cannot currently be called from this host.
        McpError::EgressRefused { .. } | McpError::EgressUnavailable { .. } => {
            ToolError::unavailable(
                tool,
                meerkat_core::ToolUnavailableReason::NotCurrentlyCallable,
            )
        }
        // Refused by the server's access policy before anything was sent.
        McpError::OperationNotPermitted { .. } => ToolError::access_denied(tool),
        // The call-context provider's typed verdict, before anything was
        // sent: a denial stays a denial, and an unavailable context stays an
        // availability result.
        McpError::CallContext(crate::McpCallContextError::Denied) => ToolError::access_denied(tool),
        McpError::CallContext(crate::McpCallContextError::Unavailable) => ToolError::unavailable(
            tool,
            meerkat_core::ToolUnavailableReason::TemporarilyUnavailable,
        ),
        // Its own session 404, or a redirect shown by its own response, then
        // a failure: neither success nor denial. Nothing is re-sent and the
        // session is not re-initialized; a redirected call can have been
        // more than one physical request.
        error @ (McpError::SessionExpired { .. } | McpError::RedirectedOutcomeUncertain { .. }) => {
            ToolError::outcome_uncertain(tool, error.to_string())
        }
        other => ToolError::execution_failed(other.to_string()),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn native_entry_refusal_survives_the_router_error_mapping() {
        let refusals = [
            ToolError::ReviewUnavailable {
                kind: meerkat_core::ReviewUnavailableKind::DeadlineExpired,
            },
            ToolError::ReviewUnsatisfied {
                kind: meerkat_core::ReviewUnsatisfiedKind::ContextChanged,
                approval_id: None,
            },
            // The owner's consent reference survives the router mapping.
            ToolError::ReviewUnsatisfied {
                kind: meerkat_core::ReviewUnsatisfiedKind::HumanConsentRequired,
                approval_id: Some(meerkat_core::ApprovalId::new()),
            },
            ToolError::AuthorizationRefused {
                refusal: meerkat_core::authorization::OperationRefused::new(
                    meerkat_core::authorization::OperationRefusalKind::Denied,
                ),
            },
        ];
        for refusal in refusals {
            let mapped = tool_call_error("tool", McpError::EntryRefused(Box::new(refusal.clone())));
            assert_eq!(mapped, refusal, "typed entry refusal is not laundered");
        }
        assert_eq!(
            tool_call_error(
                "tool",
                McpError::Confinement(
                    meerkat_core::confinement::ConfinementRefusal::UnsupportedRequirement,
                )
            ),
            ToolError::ConfinementRefused {
                refusal: meerkat_core::confinement::ConfinementRefusal::UnsupportedRequirement,
            },
        );
        assert!(matches!(
            tool_call_error(
                "tool",
                McpError::ToolCallFailed {
                    tool: "t".into(),
                    reason: "transport".into(),
                }
            ),
            ToolError::ExecutionFailed { .. }
        ));
    }

    #[test]
    fn call_context_verdicts_keep_their_typed_feedback() {
        let denied = tool_call_error(
            "tool",
            McpError::CallContext(crate::McpCallContextError::Denied),
        );
        assert_eq!(
            denied,
            ToolError::AccessDenied {
                name: "tool".into()
            }
        );
        assert_eq!(
            denied.to_error_payload(),
            serde_json::json!({
                "error": "access_denied",
                "message": "Tool 'tool' is not allowed by policy",
            })
        );

        let unavailable = tool_call_error(
            "tool",
            McpError::CallContext(crate::McpCallContextError::Unavailable),
        );
        assert_eq!(
            unavailable,
            ToolError::Unavailable {
                name: "tool".into(),
                reason: meerkat_core::ToolUnavailableReason::TemporarilyUnavailable,
            }
        );
        assert_eq!(
            unavailable.to_error_payload(),
            serde_json::json!({
                "error": "tool_unavailable",
                "message": format!(
                    "Tool 'tool' is currently unavailable: {}",
                    meerkat_core::ToolUnavailableReason::TemporarilyUnavailable
                ),
            })
        );
    }
    use crate::connection::McpConnection;
    use meerkat_core::ExternalToolSurfaceFailureCause;
    use meerkat_core::event::ToolConfigChangeOperation;
    use std::collections::HashMap;
    use std::path::Path;

    fn async_connect_test_timeout() -> Duration {
        Duration::from_secs((McpConnection::DEFAULT_CONNECT_TIMEOUT_SECS as u64) + 5)
    }

    fn test_server_config(name: &str, path: &Path) -> McpServerConfig {
        McpServerConfig::stdio(
            name,
            path.to_string_lossy().to_string(),
            vec![],
            HashMap::new(),
        )
    }

    #[cfg(feature = "workspace-tests")]
    fn generated_surface_handle() -> Arc<dyn ExternalToolSurfaceHandle> {
        Arc::new(meerkat_runtime::RuntimeExternalToolSurfaceHandle::ephemeral())
    }

    #[cfg(feature = "workspace-tests")]
    fn generated_handle_owner_router() -> McpRouter {
        McpRouter::new_with_surface_handle_and_stdio_profile(
            generated_surface_handle(),
            crate::McpStdioLaunchProfile::trusted_host(),
        )
    }

    #[cfg(feature = "workspace-tests")]
    fn generated_handle_owner_router_with_timeout(removal_timeout: Duration) -> McpRouter {
        let mut router = McpRouter::new_with_surface_handle_and_stdio_profile(
            generated_surface_handle(),
            crate::McpStdioLaunchProfile::trusted_host(),
        );
        router
            .set_removal_timeout(removal_timeout)
            .expect("the generated surface accepts a removal timeout");
        router
    }

    #[test]
    fn an_expired_session_types_the_call_outcome_as_uncertain() {
        let error = tool_call_error(
            "effect",
            McpError::SessionExpired {
                server: "remote".into(),
                tool: "effect".into(),
            },
        );
        assert!(
            matches!(&error, ToolError::OutcomeUncertain { name, .. } if name == "effect"),
            "{error:?}"
        );
        assert_eq!(error.error_code(), "outcome_uncertain");
        assert_eq!(
            meerkat_core::ops::ToolDispatchTerminalErrorKind::from(&error),
            meerkat_core::ops::ToolDispatchTerminalErrorKind::OutcomeUncertain
        );
        // A call refused unsent on a dead session is not uncertain.
        let refused = tool_call_error(
            "effect",
            McpError::ServerUnavailable {
                server: "remote".into(),
                state: "session expired; reconnect required".into(),
            },
        );
        assert_eq!(refused.error_code(), "execution_failed");
    }

    #[derive(Default)]
    struct RecordingSurfaceHandle {
        inputs: Mutex<Vec<CoreSurfaceInput>>,
    }

    impl RecordingSurfaceHandle {
        fn recorded_inputs(&self) -> Vec<CoreSurfaceInput> {
            self.inputs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    impl ExternalToolSurfaceHandle for RecordingSurfaceHandle {
        fn apply_surface_input(
            &self,
            input: CoreSurfaceInput,
        ) -> Result<CoreSurfaceTransition, DslTransitionError> {
            let effects = match &input {
                CoreSurfaceInput::MarkPendingFailed {
                    surface_id, cause, ..
                } => {
                    vec![CoreSurfaceEffect::EmitExternalToolDelta {
                        surface_id: surface_id.clone(),
                        operation: ExternalToolSurfaceDeltaOperation::Add,
                        phase: ExternalToolSurfaceDeltaPhase::Failed,
                        cause: Some(*cause),
                    }]
                }
                _ => Vec::new(),
            };
            self.inputs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(input);
            Ok(CoreSurfaceTransition {
                phase: ExternalToolSurfaceGlobalPhase::Operating,
                effects,
            })
        }

        fn register(&self, _surface_id: String) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn stage_add(&self, surface_id: String, now_ms: u64) -> Result<(), DslTransitionError> {
            self.apply_surface_input(CoreSurfaceInput::StageAdd { surface_id, now_ms })
                .map(|_| ())
        }

        fn stage_remove(&self, surface_id: String, now_ms: u64) -> Result<(), DslTransitionError> {
            self.apply_surface_input(CoreSurfaceInput::StageRemove { surface_id, now_ms })
                .map(|_| ())
        }

        fn stage_reload(&self, surface_id: String, now_ms: u64) -> Result<(), DslTransitionError> {
            self.apply_surface_input(CoreSurfaceInput::StageReload { surface_id, now_ms })
                .map(|_| ())
        }

        fn apply_boundary(
            &self,
            surface_id: String,
            now_ms: u64,
            staged_intent_sequence: u64,
            applied_at_turn: u64,
        ) -> Result<(), DslTransitionError> {
            self.apply_surface_input(CoreSurfaceInput::ApplyBoundary {
                surface_id,
                now_ms,
                staged_intent_sequence,
                applied_at_turn,
            })
            .map(|_| ())
        }

        fn mark_pending_succeeded(
            &self,
            surface_id: String,
            pending_task_sequence: u64,
            staged_intent_sequence: u64,
        ) -> Result<(), DslTransitionError> {
            self.apply_surface_input(CoreSurfaceInput::MarkPendingSucceeded {
                surface_id,
                pending_task_sequence,
                staged_intent_sequence,
            })
            .map(|_| ())
        }

        fn mark_pending_failed(
            &self,
            surface_id: String,
            pending_task_sequence: u64,
            staged_intent_sequence: u64,
            cause: ExternalToolSurfaceFailureCause,
        ) -> Result<(), DslTransitionError> {
            self.apply_surface_input(CoreSurfaceInput::MarkPendingFailed {
                surface_id,
                pending_task_sequence,
                staged_intent_sequence,
                cause,
            })
            .map(|_| ())
        }

        fn call_started(&self, surface_id: String) -> Result<(), DslTransitionError> {
            self.apply_surface_input(CoreSurfaceInput::CallStarted { surface_id })
                .map(|_| ())
        }

        fn call_finished(&self, surface_id: String) -> Result<(), DslTransitionError> {
            self.apply_surface_input(CoreSurfaceInput::CallFinished { surface_id })
                .map(|_| ())
        }

        fn finalize_removal_clean(&self, surface_id: String) -> Result<(), DslTransitionError> {
            self.apply_surface_input(CoreSurfaceInput::FinalizeRemovalClean { surface_id })
                .map(|_| ())
        }

        fn finalize_removal_forced(&self, surface_id: String) -> Result<(), DslTransitionError> {
            self.apply_surface_input(CoreSurfaceInput::FinalizeRemovalForced { surface_id })
                .map(|_| ())
        }

        fn snapshot_aligned(&self, epoch: u64) -> Result<(), DslTransitionError> {
            self.apply_surface_input(CoreSurfaceInput::SnapshotAligned { epoch })
                .map(|_| ())
        }

        fn shutdown_surface(&self) -> Result<(), DslTransitionError> {
            self.apply_surface_input(CoreSurfaceInput::Shutdown)
                .map(|_| ())
        }

        fn surface_snapshot(&self, _surface_id: &str) -> Option<SurfaceSnapshot> {
            None
        }

        fn diagnostic_snapshot(&self) -> SurfaceDiagnosticSnapshot {
            SurfaceDiagnosticSnapshot {
                surface_phase: ExternalToolSurfaceGlobalPhase::Operating,
                known_surfaces: BTreeSet::new(),
                visible_surfaces: BTreeSet::new(),
                snapshot_epoch: 0,
                snapshot_aligned_epoch: 0,
                has_pending_or_staged: false,
                entries: Vec::new(),
            }
        }

        fn visible_surfaces(&self) -> BTreeSet<String> {
            BTreeSet::new()
        }

        fn removing_surfaces(&self) -> BTreeSet<String> {
            BTreeSet::new()
        }

        fn pending_surfaces(&self) -> BTreeSet<String> {
            BTreeSet::new()
        }

        fn has_pending_or_staged(&self) -> bool {
            false
        }

        fn snapshot_epoch(&self) -> u64 {
            0
        }

        fn snapshot_aligned_epoch(&self) -> u64 {
            0
        }
    }

    #[test]
    fn runtime_owner_pending_failed_preserves_typed_failure_cause() {
        let handle = Arc::new(RecordingSurfaceHandle::default());
        let owner = SurfaceOwner::runtime(handle.clone());

        let transition = owner
            .apply(ExternalToolSurfaceInput::PendingFailed {
                surface_id: SurfaceId::from("typed-failure"),
                operation: SurfaceDeltaOperation::Add,
                pending_task_sequence: 7,
                staged_intent_sequence: 11,
                applied_at_turn: TurnNumber(13),
                cause: ExternalToolSurfaceFailureCause::PendingFailed,
            })
            .expect("runtime pending failure");
        assert!(transition.effects.iter().any(|effect| matches!(
            effect,
            ExternalToolSurfaceEffect::EmitExternalToolDelta {
                phase: SurfaceDeltaPhase::Failed,
                cause: Some(ExternalToolSurfaceFailureCause::PendingFailed),
                ..
            }
        )));

        let recorded = handle.recorded_inputs();
        assert_eq!(recorded.len(), 1);
        let CoreSurfaceInput::MarkPendingFailed { cause, .. } = recorded[0] else {
            panic!(
                "expected MarkPendingFailed core input, got {:?}",
                recorded[0]
            );
        };
        assert_eq!(cause, ExternalToolSurfaceFailureCause::PendingFailed);
        assert_eq!(cause.as_str(), "pending_failed");
    }

    fn complete_add(router: &mut McpRouter, server_name: &str) {
        let sid = SurfaceId::from(server_name);
        // Read the per-surface sequences from the owner's surface snapshot
        // after each transition rather than `staged_intents_in_order()[0]` +
        // a hardcoded `pending_task_sequence: 1`. The `[0]` form and the
        // hardcoded task sequence only happen to be correct for the FIRST
        // surface; once a second surface is staged they diverge and
        // PendingSucceeded fails its task/lineage sequence guards. The
        // snapshot exposes the authoritative staged/pending/lineage sequences
        // minted by the handle for this surface.
        router
            .surface_owner
            .apply(ExternalToolSurfaceInput::StageAdd {
                surface_id: sid.clone(),
            })
            .expect("stage add");
        let staged_sequence = router
            .surface_owner
            .surface_snapshot(server_name)
            .and_then(|snap| snap.staged_intent_sequence)
            .expect("staged intent sequence after stage add");
        router
            .surface_owner
            .apply(ExternalToolSurfaceInput::ApplyBoundary {
                surface_id: sid.clone(),
                staged_intent_sequence: staged_sequence,
                applied_at_turn: TurnNumber(staged_sequence),
            })
            .expect("apply add boundary");
        let pending = router
            .surface_owner
            .surface_snapshot(server_name)
            .expect("surface snapshot after apply boundary");
        router
            .surface_owner
            .apply(ExternalToolSurfaceInput::PendingSucceeded {
                surface_id: sid,
                operation: SurfaceDeltaOperation::Add,
                pending_task_sequence: pending
                    .pending_task_sequence
                    .expect("pending task sequence after apply boundary"),
                staged_intent_sequence: pending
                    .pending_lineage_sequence
                    .expect("pending lineage sequence after apply boundary"),
                applied_at_turn: TurnNumber(staged_sequence),
            })
            .expect("add success");
    }

    #[cfg(feature = "workspace-tests")]
    #[test]
    fn setup_refusal_projection_requires_accepted_completion_and_exact_error_type() {
        use meerkat_core::confinement::ConfinementRefusal;
        for reload in [false, true] {
            for typed_refusal in [false, true] {
                let mut router = generated_handle_owner_router();
                let server = "projection-target";
                let surface_id = SurfaceId::from(server);
                if reload {
                    complete_add(&mut router, server);
                }
                let stage = if reload {
                    ExternalToolSurfaceInput::StageReload {
                        surface_id: surface_id.clone(),
                    }
                } else {
                    ExternalToolSurfaceInput::StageAdd {
                        surface_id: surface_id.clone(),
                    }
                };
                router
                    .surface_owner
                    .apply(stage)
                    .expect("stage actual owner");
                let sequence = router
                    .surface_owner
                    .surface_snapshot(server)
                    .unwrap()
                    .staged_intent_sequence
                    .unwrap();
                let transition = router
                    .surface_owner
                    .apply(ExternalToolSurfaceInput::ApplyBoundary {
                        surface_id,
                        staged_intent_sequence: sequence,
                        applied_at_turn: TurnNumber(sequence),
                    })
                    .expect("owner mints actual completion obligation");
                let effects = core_surface_effects(&transition.effects);
                let obligation = protocol_surface_completion::extract_obligations(&effects)
                    .pop()
                    .expect("completion obligation");
                let error = if typed_refusal {
                    McpError::Confinement(ConfinementRefusal::UnsupportedRequirement)
                } else {
                    McpError::Io(std::io::Error::other(
                        ConfinementRefusal::UnsupportedRequirement.to_string(),
                    ))
                };
                router.process_pending_result(PendingResult {
                    obligation: obligation.clone(),
                    config: McpServerConfig::stdio(
                        obligation.surface_id.clone(),
                        "/bin/true",
                        Vec::new(),
                        HashMap::new(),
                    ),
                    result: Err(error),
                    guard: None,
                });
                let notice = router
                    .completed_updates
                    .pop_front()
                    .expect("accepted failed notice")
                    .action;
                assert_eq!(notice.phase, McpLifecyclePhase::Failed);
                assert_eq!(
                    notice.operation,
                    if reload {
                        ToolConfigChangeOperation::Reload
                    } else {
                        ToolConfigChangeOperation::Add
                    }
                );
                let host = serde_json::to_value(notice.to_tool_config_changed_payload()).unwrap();
                if typed_refusal {
                    assert_eq!(
                        host["status_info"]["confinement_refusal"],
                        "unsupported_requirement"
                    );
                } else {
                    assert!(host["status_info"].get("confinement_refusal").is_none());
                }
                assert!(router.completed_updates.is_empty());
                // The settled obligation is now stale. Even a typed error on
                // its replay cannot publish a second refusal notice.
                router.process_pending_result(PendingResult {
                    config: McpServerConfig::stdio(
                        obligation.surface_id.clone(),
                        "/bin/true",
                        Vec::new(),
                        HashMap::new(),
                    ),
                    obligation,
                    result: Err(McpError::Confinement(ConfinementRefusal::InvalidLaunch)),
                    guard: None,
                });
                assert!(router.completed_updates.is_empty());
            }
        }
    }

    #[cfg(feature = "workspace-tests")]
    #[test]
    fn core_handle_owner_add_apply_success_makes_surface_active_visible() {
        let mut router = generated_handle_owner_router();

        complete_add(&mut router, "runtime-add");

        let snapshot = router.external_tool_surface_snapshot();
        let entry = snapshot
            .entries
            .iter()
            .find(|entry| entry.surface_id == "runtime-add")
            .expect("runtime-add surface");
        assert!(entry.visible);
        assert_eq!(entry.base_state, ExternalToolSurfaceBaseState::Active);
        assert_eq!(entry.pending_op, ExternalToolSurfacePendingOp::None);
    }

    #[cfg(feature = "workspace-tests")]
    #[test]
    fn core_handle_owner_reload_rejects_before_active_and_accepts_after_active() {
        let mut router = generated_handle_owner_router();
        let sid = SurfaceId::from("runtime-reload");

        assert!(
            router
                .surface_owner
                .apply(ExternalToolSurfaceInput::StageReload {
                    surface_id: sid.clone(),
                })
                .is_err(),
            "reload before active must be rejected by DSL"
        );

        complete_add(&mut router, "runtime-reload");
        router
            .surface_owner
            .apply(ExternalToolSurfaceInput::StageReload {
                surface_id: sid.clone(),
            })
            .expect("reload after active");
        let staged_sequence = router.surface_owner.staged_intents_in_order()[0].2;
        let transition = router
            .surface_owner
            .apply(ExternalToolSurfaceInput::ApplyBoundary {
                surface_id: sid,
                staged_intent_sequence: staged_sequence,
                applied_at_turn: TurnNumber(staged_sequence),
            })
            .expect("apply reload boundary");
        assert!(transition.effects.iter().any(|effect| matches!(
            effect,
            ExternalToolSurfaceEffect::ScheduleSurfaceCompletion {
                operation: SurfaceDeltaOperation::Reload,
                ..
            }
        )));
    }

    #[cfg(feature = "workspace-tests")]
    #[test]
    fn core_handle_owner_remove_finalize_and_call_guards_are_owner_owned() {
        let mut router = generated_handle_owner_router();
        complete_add(&mut router, "runtime-remove");
        let sid = SurfaceId::from("runtime-remove");

        router
            .surface_owner
            .apply(ExternalToolSurfaceInput::CallStarted {
                surface_id: sid.clone(),
            })
            .expect("first call");
        router
            .surface_owner
            .apply(ExternalToolSurfaceInput::CallStarted {
                surface_id: sid.clone(),
            })
            .expect("second call");
        router
            .surface_owner
            .apply(ExternalToolSurfaceInput::CallFinished {
                surface_id: sid.clone(),
            })
            .expect("finish one call");
        assert_eq!(router.surface_owner.inflight_call_count(&sid), 1);

        router
            .surface_owner
            .apply(ExternalToolSurfaceInput::StageRemove {
                surface_id: sid.clone(),
            })
            .expect("stage remove");
        let staged_sequence = router.surface_owner.staged_intents_in_order()[0].2;
        router
            .surface_owner
            .apply(ExternalToolSurfaceInput::ApplyBoundary {
                surface_id: sid.clone(),
                staged_intent_sequence: staged_sequence,
                applied_at_turn: TurnNumber(staged_sequence),
            })
            .expect("apply remove boundary");

        let rejected = router
            .surface_owner
            .apply(ExternalToolSurfaceInput::CallStarted {
                surface_id: sid.clone(),
            })
            .expect("call while removing is modeled rejection");
        assert!(rejected.effects.iter().any(|effect| matches!(
            effect,
            ExternalToolSurfaceEffect::RejectSurfaceCall { cause, .. }
                if *cause == ExternalToolSurfaceFailureCause::SurfaceDraining
        )));
        assert_eq!(router.surface_owner.inflight_call_count(&sid), 1);

        assert!(
            router
                .surface_owner
                .apply(ExternalToolSurfaceInput::FinalizeRemovalClean {
                    surface_id: sid.clone(),
                    applied_at_turn: TurnNumber(staged_sequence),
                })
                .is_err(),
            "clean finalize must reject while inflight calls remain"
        );
        router
            .surface_owner
            .apply(ExternalToolSurfaceInput::FinalizeRemovalForced {
                surface_id: sid.clone(),
                applied_at_turn: TurnNumber(staged_sequence),
            })
            .expect("forced finalize");
        assert_eq!(
            router.surface_owner.surface_base(&sid),
            SurfaceBaseState::Removed
        );
        assert_eq!(router.surface_owner.inflight_call_count(&sid), 0);
    }

    /// Surface handle decorator that delegates every read/transition to a real
    /// generated handle, but rejects finalize-removal inputs. Used to prove the
    /// router fails closed (does not silently continue with diverged state)
    /// when the surface owner rejects a finalize the router already computed.
    struct RejectFinalizeSurfaceHandle {
        inner: Arc<dyn ExternalToolSurfaceHandle>,
    }

    #[cfg(feature = "workspace-tests")]
    impl RejectFinalizeSurfaceHandle {
        fn new() -> Self {
            Self {
                inner: generated_surface_handle(),
            }
        }
    }

    impl ExternalToolSurfaceHandle for RejectFinalizeSurfaceHandle {
        fn apply_surface_input(
            &self,
            input: CoreSurfaceInput,
        ) -> Result<CoreSurfaceTransition, DslTransitionError> {
            match input {
                CoreSurfaceInput::FinalizeRemovalClean { .. }
                | CoreSurfaceInput::FinalizeRemovalForced { .. } => {
                    Err(DslTransitionError::guard_rejected(
                        "RejectFinalizeSurfaceHandle",
                        "finalize removal forcibly rejected for test",
                    ))
                }
                other => self.inner.apply_surface_input(other),
            }
        }

        fn register(&self, surface_id: String) -> Result<(), DslTransitionError> {
            self.inner.register(surface_id)
        }

        fn stage_add(&self, surface_id: String, now_ms: u64) -> Result<(), DslTransitionError> {
            self.inner.stage_add(surface_id, now_ms)
        }

        fn stage_remove(&self, surface_id: String, now_ms: u64) -> Result<(), DslTransitionError> {
            self.inner.stage_remove(surface_id, now_ms)
        }

        fn stage_reload(&self, surface_id: String, now_ms: u64) -> Result<(), DslTransitionError> {
            self.inner.stage_reload(surface_id, now_ms)
        }

        fn apply_boundary(
            &self,
            surface_id: String,
            now_ms: u64,
            staged_intent_sequence: u64,
            applied_at_turn: u64,
        ) -> Result<(), DslTransitionError> {
            self.inner
                .apply_boundary(surface_id, now_ms, staged_intent_sequence, applied_at_turn)
        }

        fn mark_pending_succeeded(
            &self,
            surface_id: String,
            pending_task_sequence: u64,
            staged_intent_sequence: u64,
        ) -> Result<(), DslTransitionError> {
            self.inner.mark_pending_succeeded(
                surface_id,
                pending_task_sequence,
                staged_intent_sequence,
            )
        }

        fn mark_pending_failed(
            &self,
            surface_id: String,
            pending_task_sequence: u64,
            staged_intent_sequence: u64,
            cause: ExternalToolSurfaceFailureCause,
        ) -> Result<(), DslTransitionError> {
            self.inner.mark_pending_failed(
                surface_id,
                pending_task_sequence,
                staged_intent_sequence,
                cause,
            )
        }

        fn call_started(&self, surface_id: String) -> Result<(), DslTransitionError> {
            self.inner.call_started(surface_id)
        }

        fn call_finished(&self, surface_id: String) -> Result<(), DslTransitionError> {
            self.inner.call_finished(surface_id)
        }

        fn finalize_removal_clean(&self, surface_id: String) -> Result<(), DslTransitionError> {
            self.apply_surface_input(CoreSurfaceInput::FinalizeRemovalClean { surface_id })
                .map(|_| ())
        }

        fn finalize_removal_forced(&self, surface_id: String) -> Result<(), DslTransitionError> {
            self.apply_surface_input(CoreSurfaceInput::FinalizeRemovalForced { surface_id })
                .map(|_| ())
        }

        fn snapshot_aligned(&self, epoch: u64) -> Result<(), DslTransitionError> {
            self.inner.snapshot_aligned(epoch)
        }

        fn shutdown_surface(&self) -> Result<(), DslTransitionError> {
            self.inner.shutdown_surface()
        }

        fn surface_snapshot(&self, surface_id: &str) -> Option<SurfaceSnapshot> {
            self.inner.surface_snapshot(surface_id)
        }

        fn diagnostic_snapshot(&self) -> SurfaceDiagnosticSnapshot {
            self.inner.diagnostic_snapshot()
        }

        fn visible_surfaces(&self) -> BTreeSet<String> {
            self.inner.visible_surfaces()
        }

        fn removing_surfaces(&self) -> BTreeSet<String> {
            self.inner.removing_surfaces()
        }

        fn pending_surfaces(&self) -> BTreeSet<String> {
            self.inner.pending_surfaces()
        }

        fn has_pending_or_staged(&self) -> bool {
            self.inner.has_pending_or_staged()
        }

        fn snapshot_epoch(&self) -> u64 {
            self.inner.snapshot_epoch()
        }

        fn snapshot_aligned_epoch(&self) -> u64 {
            self.inner.snapshot_aligned_epoch()
        }
    }

    /// Surface handle decorator that delegates every read/transition to a real
    /// generated handle, but rejects `CallFinished` inputs. Used to prove the
    /// router fails closed when the surface owner rejects the finish for a tool
    /// call whose underlying transport already returned a result (fix #7).
    struct RejectCallFinishedSurfaceHandle {
        inner: Arc<dyn ExternalToolSurfaceHandle>,
    }

    #[cfg(feature = "workspace-tests")]
    impl RejectCallFinishedSurfaceHandle {
        fn new() -> Self {
            Self {
                inner: generated_surface_handle(),
            }
        }
    }

    impl ExternalToolSurfaceHandle for RejectCallFinishedSurfaceHandle {
        fn apply_surface_input(
            &self,
            input: CoreSurfaceInput,
        ) -> Result<CoreSurfaceTransition, DslTransitionError> {
            match input {
                CoreSurfaceInput::CallFinished { .. } => Err(DslTransitionError::guard_rejected(
                    "RejectCallFinishedSurfaceHandle",
                    "call finished forcibly rejected for test",
                )),
                other => self.inner.apply_surface_input(other),
            }
        }

        fn register(&self, surface_id: String) -> Result<(), DslTransitionError> {
            self.inner.register(surface_id)
        }

        fn stage_add(&self, surface_id: String, now_ms: u64) -> Result<(), DslTransitionError> {
            self.inner.stage_add(surface_id, now_ms)
        }

        fn stage_remove(&self, surface_id: String, now_ms: u64) -> Result<(), DslTransitionError> {
            self.inner.stage_remove(surface_id, now_ms)
        }

        fn stage_reload(&self, surface_id: String, now_ms: u64) -> Result<(), DslTransitionError> {
            self.inner.stage_reload(surface_id, now_ms)
        }

        fn apply_boundary(
            &self,
            surface_id: String,
            now_ms: u64,
            staged_intent_sequence: u64,
            applied_at_turn: u64,
        ) -> Result<(), DslTransitionError> {
            self.inner
                .apply_boundary(surface_id, now_ms, staged_intent_sequence, applied_at_turn)
        }

        fn mark_pending_succeeded(
            &self,
            surface_id: String,
            pending_task_sequence: u64,
            staged_intent_sequence: u64,
        ) -> Result<(), DslTransitionError> {
            self.inner.mark_pending_succeeded(
                surface_id,
                pending_task_sequence,
                staged_intent_sequence,
            )
        }

        fn mark_pending_failed(
            &self,
            surface_id: String,
            pending_task_sequence: u64,
            staged_intent_sequence: u64,
            cause: ExternalToolSurfaceFailureCause,
        ) -> Result<(), DslTransitionError> {
            self.inner.mark_pending_failed(
                surface_id,
                pending_task_sequence,
                staged_intent_sequence,
                cause,
            )
        }

        fn call_started(&self, surface_id: String) -> Result<(), DslTransitionError> {
            self.inner.call_started(surface_id)
        }

        fn call_finished(&self, surface_id: String) -> Result<(), DslTransitionError> {
            self.apply_surface_input(CoreSurfaceInput::CallFinished { surface_id })
                .map(|_| ())
        }

        fn finalize_removal_clean(&self, surface_id: String) -> Result<(), DslTransitionError> {
            self.inner.finalize_removal_clean(surface_id)
        }

        fn finalize_removal_forced(&self, surface_id: String) -> Result<(), DslTransitionError> {
            self.inner.finalize_removal_forced(surface_id)
        }

        fn snapshot_aligned(&self, epoch: u64) -> Result<(), DslTransitionError> {
            self.inner.snapshot_aligned(epoch)
        }

        fn shutdown_surface(&self) -> Result<(), DslTransitionError> {
            self.inner.shutdown_surface()
        }

        fn surface_snapshot(&self, surface_id: &str) -> Option<SurfaceSnapshot> {
            self.inner.surface_snapshot(surface_id)
        }

        fn diagnostic_snapshot(&self) -> SurfaceDiagnosticSnapshot {
            self.inner.diagnostic_snapshot()
        }

        fn visible_surfaces(&self) -> BTreeSet<String> {
            self.inner.visible_surfaces()
        }

        fn removing_surfaces(&self) -> BTreeSet<String> {
            self.inner.removing_surfaces()
        }

        fn pending_surfaces(&self) -> BTreeSet<String> {
            self.inner.pending_surfaces()
        }

        fn has_pending_or_staged(&self) -> bool {
            self.inner.has_pending_or_staged()
        }

        fn snapshot_epoch(&self) -> u64 {
            self.inner.snapshot_epoch()
        }

        fn snapshot_aligned_epoch(&self) -> u64 {
            self.inner.snapshot_aligned_epoch()
        }
    }

    /// Gate for fix #7: a surface-owner rejection of `CallFinished` is
    /// authoritative divergence. The router must fail closed (return a typed
    /// `ServerUnavailable` fault) instead of warn-and-returning the tool result
    /// with router/machine inflight-call state diverged. Mirrors the
    /// fail-closed `CallStarted` rejection path.
    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn call_finished_rejection_fails_closed_not_returning_result() {
        let server_path = mcp_test_server::fixture_binary();

        let mut router = McpRouter::new_with_surface_handle_and_stdio_profile(
            Arc::new(RejectCallFinishedSurfaceHandle::new()),
            crate::McpStdioLaunchProfile::trusted_host(),
        );
        router
            .add_server(test_server_config("test-server", &server_path))
            .await
            .expect("add_server");

        // The underlying transport succeeds, but the surface owner rejects the
        // CallFinished apply; the result must NOT be surfaced.
        let err = router
            .call_tool("echo", &serde_json::json!({"message": "hi"}))
            .await
            .expect_err("CallFinished rejection must fail closed, not return the result");
        assert!(
            matches!(err, McpError::ServerUnavailable { .. }),
            "expected a typed ServerUnavailable fault, got {err:?}"
        );
    }

    /// Gate for dogma row #112: a surface-owner rejection of a finalize-removal
    /// is authoritative divergence. The router must fail closed (return a typed
    /// error) instead of warn-and-continuing with router/machine state diverged.
    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn finalize_removal_rejection_fails_closed_not_silently_continue() {
        let mut router = McpRouter::new_with_surface_handle_and_stdio_profile(
            Arc::new(RejectFinalizeSurfaceHandle::new()),
            crate::McpStdioLaunchProfile::trusted_host(),
        );
        let sid = SurfaceId::from("reject-finalize");

        // Drive the surface to Active, then to Removing with zero inflight calls
        // so process_removals decides to clean-finalize it.
        complete_add(&mut router, "reject-finalize");
        router
            .surface_owner
            .apply(ExternalToolSurfaceInput::StageRemove {
                surface_id: sid.clone(),
            })
            .expect("stage remove");
        let staged_sequence = router.surface_owner.staged_intents_in_order()[0].2;
        router
            .surface_owner
            .apply(ExternalToolSurfaceInput::ApplyBoundary {
                surface_id: sid.clone(),
                staged_intent_sequence: staged_sequence,
                applied_at_turn: TurnNumber(staged_sequence),
            })
            .expect("apply remove boundary");

        assert_eq!(
            router.surface_owner.inflight_call_count(&sid),
            0,
            "surface should have no inflight calls so finalize-clean is attempted"
        );
        assert!(
            router.surface_owner.removing_surfaces().contains(&sid),
            "surface should be in the removing set"
        );

        // process_removals computes the finalized set then asks the surface owner
        // to commit it; the owner rejects. The OLD behavior warned and returned
        // Ok with diverged state — this must now be a typed error.
        let result = router.progress_removals().await;
        let err = result.expect_err(
            "surface-owner rejection of finalize removal must fail closed, not silently continue",
        );
        assert!(
            matches!(err, McpError::ProtocolError { .. }),
            "expected a typed ProtocolError fault, got {err:?}"
        );
    }

    /// Insert a shell `ServerEntry` directly with the given tool names. Used to
    /// stage tool-name overlap between two visible servers without standing up
    /// real MCP connections.
    fn insert_entry_with_tools(router: &mut McpRouter, server_name: &str, tool_names: &[&str]) {
        let tools: Vec<Arc<ToolDef>> = tool_names
            .iter()
            .map(|name| {
                Arc::new(ToolDef::new(
                    *name,
                    format!("tool {name} from {server_name}"),
                    serde_json::json!({"type": "object"}),
                ))
            })
            .collect();
        router.servers.insert(
            server_name.to_string(),
            ServerEntry {
                config: McpServerConfig::stdio(
                    server_name,
                    "noop".to_string(),
                    vec![],
                    HashMap::new(),
                ),
                connection: None,
                tools,
                active_calls: AtomicUsize::new(0),
            },
        );
    }

    fn insert_entry_with_tool_defs(router: &mut McpRouter, server_name: &str, tools: Vec<ToolDef>) {
        router.servers.insert(
            server_name.to_string(),
            ServerEntry {
                config: McpServerConfig::stdio(
                    server_name,
                    "noop".to_string(),
                    vec![],
                    HashMap::new(),
                ),
                connection: None,
                tools: tools.into_iter().map(Arc::new).collect(),
                active_calls: AtomicUsize::new(0),
            },
        );
    }

    /// Gate for fixes #6: a tool name exposed by two different servers is
    /// ambiguous and must be dispatchable from NEITHER. The OLD behavior was
    /// last-wins (silently route to an arbitrary owner). The published
    /// projection must exclude the collided name entirely while leaving each
    /// server's non-colliding tools routable.
    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn duplicate_tool_name_across_servers_is_excluded_from_projection() {
        let mut router = generated_handle_owner_router();

        // Two active, visible servers.
        complete_add(&mut router, "server-a");
        complete_add(&mut router, "server-b");

        // Both expose "shared"; each also exposes a unique tool.
        insert_entry_with_tools(&mut router, "server-a", &["shared", "only_a"]);
        insert_entry_with_tools(&mut router, "server-b", &["shared", "only_b"]);

        let complete = router.publish_projection_snapshot();
        assert!(
            !complete,
            "a tool-name collision is an incomplete projection (fail closed)"
        );

        let collisions = router.tool_name_collisions();
        assert_eq!(
            collisions.as_ref(),
            &[McpToolNameCollision {
                exposed_name: "shared".into(),
                routes: vec![
                    McpToolRoute {
                        server_name: "server-a".into(),
                        raw_operation: "shared".into()
                    },
                    McpToolRoute {
                        server_name: "server-b".into(),
                        raw_operation: "shared".into()
                    },
                ],
            }]
        );
        assert!(
            collisions[0]
                .to_string()
                .contains("McpServerConfig.tool_names")
        );
        let projection = Arc::clone(&router.projection);

        // The colliding name is absent from routing and the catalog.
        assert!(
            !projection.tool_routes.contains_key("shared"),
            "collided tool name must not be routable from any server"
        );
        assert!(
            projection
                .catalog_entries
                .iter()
                .all(|entry| entry.tool.name.as_str() != "shared"),
            "collided tool name must be excluded from the published catalog"
        );

        // Non-colliding tools from each server remain routable.
        assert_eq!(
            projection
                .tool_routes
                .get("only_a")
                .map(|route| route.server_name.as_str()),
            Some("server-a")
        );
        assert_eq!(
            projection
                .tool_routes
                .get("only_b")
                .map(|route| route.server_name.as_str()),
            Some("server-b")
        );
        assert!(
            projection
                .catalog_entries
                .iter()
                .any(|entry| entry.tool.name.as_str() == "only_a")
        );
        assert!(
            projection
                .catalog_entries
                .iter()
                .any(|entry| entry.tool.name.as_str() == "only_b")
        );

        // Dispatching the ambiguous name fails not_found (denied, never routed).
        let err = router
            .call_tool("shared", &serde_json::json!({}))
            .await
            .expect_err("collided tool name must not dispatch");
        assert!(
            matches!(err, McpError::ToolNotFound(_)),
            "ambiguous tool name must be denied as not_found, got {err:?}"
        );
    }

    fn map_names(router: &mut McpRouter, server: &str, names: &[(&str, &str)]) {
        router
            .servers
            .get_mut(server)
            .expect("test server")
            .config
            .tool_names = names
            .iter()
            .map(|(raw, exposed)| ((*raw).into(), (*exposed).into()))
            .collect();
    }

    #[cfg(feature = "workspace-tests")]
    #[test]
    fn explicit_tool_names_keep_raw_route_and_source_provenance() {
        let mut router = generated_handle_owner_router();
        for (server, exposed) in [("a", "home_search"), ("b", "work_search")] {
            complete_add(&mut router, server);
            let mut tool = ToolDef::new(
                "search",
                "provider description",
                serde_json::json!({"type":"object"}),
            );
            tool.provenance = Some(meerkat_core::ToolProvenance {
                kind: meerkat_core::ToolSourceKind::Mcp,
                source_id: server.into(),
                artifact: None,
            });
            insert_entry_with_tool_defs(&mut router, server, vec![tool.clone(), tool]);
            map_names(
                &mut router,
                server,
                &[("search", exposed), ("unlisted", "no_phantom")],
            );
        }
        assert!(router.publish_projection_snapshot());
        assert_eq!(router.projection.tool_routes.len(), 2);
        for (server, exposed) in [("a", "home_search"), ("b", "work_search")] {
            assert_eq!(
                router.projection.tool_routes.get(exposed),
                Some(&McpToolRoute {
                    server_name: server.into(),
                    raw_operation: "search".into(),
                })
            );
            let tool = router
                .list_tools()
                .iter()
                .find(|tool| tool.name == exposed)
                .unwrap();
            assert_eq!(tool.provenance.as_ref().unwrap().source_id.as_str(), server);
            assert_eq!(
                tool.provenance.as_ref().unwrap().kind,
                meerkat_core::ToolSourceKind::Mcp
            );
            assert_eq!(tool.description, "provider description");
            assert_eq!(router.servers[server].tools[0].name, "search");
        }
        assert!(!router.projection.tool_routes.contains_key("search"));
        assert!(!router.projection.tool_routes.contains_key("no_phantom"));
    }

    #[cfg(feature = "workspace-tests")]
    #[test]
    fn exposed_collisions_exclude_all_routes_within_and_across_servers() {
        let mut router = generated_handle_owner_router();
        complete_add(&mut router, "a");
        complete_add(&mut router, "b");
        insert_entry_with_tools(&mut router, "a", &["first", "second", "untouched"]);
        insert_entry_with_tools(&mut router, "b", &["third"]);
        map_names(
            &mut router,
            "a",
            &[("first", "collision"), ("second", "collision")],
        );
        map_names(&mut router, "b", &[("third", "collision")]);
        assert!(!router.publish_projection_snapshot());
        assert_eq!(router.projection.tool_routes.len(), 1);
        assert!(router.projection.tool_routes.contains_key("untouched"));
        assert_eq!(router.list_tools().len(), 1);
        assert_eq!(
            router.tool_name_collisions().as_ref(),
            &[McpToolNameCollision {
                exposed_name: "collision".into(),
                routes: vec![
                    McpToolRoute {
                        server_name: "a".into(),
                        raw_operation: "first".into()
                    },
                    McpToolRoute {
                        server_name: "a".into(),
                        raw_operation: "second".into()
                    },
                    McpToolRoute {
                        server_name: "b".into(),
                        raw_operation: "third".into()
                    },
                ],
            }]
        );

        // A configured alias can collide with an unmapped operation too.
        map_names(
            &mut router,
            "a",
            &[("first", "untouched"), ("second", "safe")],
        );
        map_names(&mut router, "b", &[("third", "safe")]);
        assert!(!router.publish_projection_snapshot());
        assert!(router.projection.tool_routes.is_empty());
        assert!(router.list_tools().is_empty());
        assert_eq!(router.tool_name_collisions().len(), 2);
        map_names(
            &mut router,
            "a",
            &[("first", "a_first"), ("second", "a_second")],
        );
        map_names(&mut router, "b", &[("third", "b_third")]);
        assert!(router.publish_projection_snapshot());
        assert_eq!(router.list_tools().len(), 4);
        assert!(router.tool_name_collisions().is_empty());
    }

    #[cfg(feature = "workspace-tests")]
    #[test]
    fn divergent_raw_definitions_cannot_be_repaired_by_an_alias() {
        let mut router = generated_handle_owner_router();
        complete_add(&mut router, "a");
        insert_entry_with_tool_defs(
            &mut router,
            "a",
            vec![
                ToolDef::new("search", "first", serde_json::json!({"type":"object"})),
                ToolDef::new("search", "second", serde_json::json!({"type":"object"})),
            ],
        );
        map_names(&mut router, "a", &[("search", "home_search")]);
        assert!(!router.publish_projection_snapshot());
        assert!(router.list_tools().is_empty());
        assert!(router.projection.tool_routes.is_empty());
    }

    #[cfg(feature = "workspace-tests")]
    #[test]
    fn exposure_does_not_repair_mismatched_provenance() {
        for (kind, source) in [
            (meerkat_core::ToolSourceKind::Builtin, "a"),
            (meerkat_core::ToolSourceKind::Mcp, "other-server"),
        ] {
            let mut router = generated_handle_owner_router();
            complete_add(&mut router, "a");
            let mut tool = ToolDef::new("search", "provider", serde_json::json!({"type":"object"}));
            tool.provenance = Some(meerkat_core::ToolProvenance {
                kind,
                source_id: source.into(),
                artifact: None,
            });
            insert_entry_with_tool_defs(&mut router, "a", vec![tool]);
            map_names(&mut router, "a", &[("search", "home_search")]);
            assert!(!router.publish_projection_snapshot());
            assert!(router.list_tools().is_empty());
            assert!(router.projection.tool_routes.is_empty());
        }
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn invalid_tool_names_refuse_before_staging_or_connection() {
        use meerkat_core::tool_catalog::{TOOL_CATALOG_LOAD_NAME, TOOL_CATALOG_SEARCH_NAME};
        let mut router = generated_handle_owner_router();
        let before = router.external_tool_surface_snapshot();
        for exposed in [
            "",
            "has space",
            "has.dot",
            "path/name",
            "nonascii_å",
            TOOL_CATALOG_SEARCH_NAME,
            TOOL_CATALOG_LOAD_NAME,
            &"x".repeat(65),
        ] {
            let mut config =
                McpServerConfig::stdio("invalid", "never-execute", vec![], HashMap::new());
            config.tool_names.insert("search".into(), exposed.into());
            assert!(matches!(
                router.stage_add(config.clone()),
                Err(McpError::InvalidToolNameMapping { .. })
            ));
            assert!(matches!(
                router.stage_reload(config.clone()),
                Err(McpError::InvalidToolNameMapping { .. })
            ));
            assert!(matches!(
                router.add_server(config).await,
                Err(McpError::InvalidToolNameMapping { .. })
            ));
            assert_eq!(router.external_tool_surface_snapshot(), before);
            assert!(router.staged_payloads.is_empty());
            assert!(router.servers.is_empty());
        }
        let mut config = McpServerConfig::stdio("valid", "never-execute", vec![], HashMap::new());
        config
            .tool_names
            .insert("provider.operation".into(), "x".repeat(64));
        assert!(McpRouter::validate_tool_names(&config).is_ok());
        config.tool_names.insert("".into(), "valid_exposure".into());
        assert!(matches!(
            McpRouter::validate_tool_names(&config),
            Err(McpError::InvalidToolNameMapping { .. })
        ));
    }

    /// Same-name-same-server re-entry with identical definitions is NOT a
    /// collision; the tool stays routable and the projection complete.
    #[cfg(feature = "workspace-tests")]
    #[test]
    fn same_server_identical_duplicate_definitions_collapse_once() {
        let mut router = generated_handle_owner_router();
        complete_add(&mut router, "solo");
        insert_entry_with_tools(&mut router, "solo", &["dup", "dup", "unique"]);

        let complete = router.publish_projection_snapshot();
        assert!(
            complete,
            "an identical same-server duplicate is not a collision; projection stays complete"
        );

        let projection = Arc::clone(&router.projection);
        assert_eq!(
            projection
                .tool_routes
                .get("dup")
                .map(|route| route.server_name.as_str()),
            Some("solo"),
            "same-server duplicate tool name remains routable"
        );
        assert_eq!(
            projection
                .catalog_entries
                .iter()
                .filter(|entry| entry.tool.name.as_str() == "dup")
                .count(),
            1,
            "identical same-server duplicates collapse to one exact catalog entry"
        );
        assert_eq!(
            projection
                .tool_routes
                .get("unique")
                .map(|route| route.server_name.as_str()),
            Some("solo")
        );
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn same_server_divergent_duplicate_definitions_are_excluded_from_projection() {
        let mut router = generated_handle_owner_router();
        complete_add(&mut router, "solo");
        insert_entry_with_tool_defs(
            &mut router,
            "solo",
            vec![
                ToolDef::new(
                    "dup",
                    "first definition",
                    serde_json::json!({"type": "object", "properties": {"a": {"type": "string"}}}),
                ),
                ToolDef::new(
                    "dup",
                    "first definition",
                    serde_json::json!({"type": "object", "properties": {"b": {"type": "number"}}}),
                ),
                ToolDef::new("unique", "unique", serde_json::json!({"type": "object"})),
            ],
        );

        let complete = router.publish_projection_snapshot();
        assert!(
            !complete,
            "divergent same-server duplicate definitions make the projection incomplete"
        );

        let projection = Arc::clone(&router.projection);
        assert!(
            !projection.tool_routes.contains_key("dup"),
            "divergent same-server duplicate must not be routable"
        );
        assert!(
            projection
                .catalog_entries
                .iter()
                .all(|entry| entry.tool.name.as_str() != "dup"),
            "divergent same-server duplicate must be excluded from the exact catalog"
        );
        assert!(
            projection
                .visible_tools
                .iter()
                .all(|tool| tool.name.as_str() != "dup"),
            "divergent same-server duplicate must be excluded from visible tools"
        );
        assert_eq!(
            projection
                .tool_routes
                .get("unique")
                .map(|route| route.server_name.as_str()),
            Some("solo"),
            "non-ambiguous tools from the same server remain routable"
        );

        let err = router
            .call_tool("dup", &serde_json::json!({}))
            .await
            .expect_err("divergent same-server duplicate must not dispatch");
        assert!(
            matches!(err, McpError::ToolNotFound(_)),
            "ambiguous same-server tool name must be denied as not_found, got {err:?}"
        );
    }

    #[cfg(feature = "workspace-tests")]
    #[test]
    fn same_server_duplicate_description_mismatch_is_divergent() {
        let mut router = generated_handle_owner_router();
        complete_add(&mut router, "solo");
        insert_entry_with_tool_defs(
            &mut router,
            "solo",
            vec![
                ToolDef::new(
                    "dup",
                    "first definition",
                    serde_json::json!({"type": "object"}),
                ),
                ToolDef::new(
                    "dup",
                    "second definition",
                    serde_json::json!({"type": "object"}),
                ),
            ],
        );

        let complete = router.publish_projection_snapshot();
        assert!(
            !complete,
            "same-server duplicate description drift makes the projection incomplete"
        );

        let projection = Arc::clone(&router.projection);
        assert!(
            !projection.tool_routes.contains_key("dup"),
            "description-divergent duplicate must not be routable"
        );
        assert!(
            projection
                .catalog_entries
                .iter()
                .all(|entry| entry.tool.name.as_str() != "dup"),
            "description-divergent duplicate must be excluded from catalog"
        );
    }

    #[cfg(feature = "workspace-tests")]
    #[test]
    fn core_handle_owner_uses_owner_pending_operation_over_completion_hint() {
        let router = generated_handle_owner_router();
        let sid = SurfaceId::from("runtime-wrong-op");

        router
            .surface_owner
            .apply(ExternalToolSurfaceInput::StageAdd {
                surface_id: sid.clone(),
            })
            .expect("stage add");
        let staged_sequence = router.surface_owner.staged_intents_in_order()[0].2;
        router
            .surface_owner
            .apply(ExternalToolSurfaceInput::ApplyBoundary {
                surface_id: sid.clone(),
                staged_intent_sequence: staged_sequence,
                applied_at_turn: TurnNumber(staged_sequence),
            })
            .expect("apply add boundary");

        let transition = router
            .surface_owner
            .apply(ExternalToolSurfaceInput::PendingSucceeded {
                surface_id: sid.clone(),
                operation: SurfaceDeltaOperation::Reload,
                pending_task_sequence: 1,
                staged_intent_sequence: staged_sequence,
                applied_at_turn: TurnNumber(staged_sequence),
            })
            .expect("completion hint must not override machine-owned pending operation");
        assert!(
            transition.effects.iter().any(|effect| matches!(
                effect,
                ExternalToolSurfaceEffect::EmitExternalToolDelta {
                    surface_id,
                    operation: SurfaceDeltaOperation::Add,
                    phase: SurfaceDeltaPhase::Applied,
                    ..
                } if *surface_id == sid
            )),
            "machine-owned pending operation should drive emitted delta: {transition:?}"
        );
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn staged_add_remove_reload_transitions() {
        let server_path = mcp_test_server::fixture_binary();

        let mut router = generated_handle_owner_router();

        router
            .add_server(test_server_config("test-server", &server_path))
            .await
            .expect("add_server");

        assert!(matches!(
            router.server_lifecycle_state("test-server"),
            Some(McpServerLifecycleState::Active)
        ));

        router.stage_reload("test-server").expect("stage reload");
        let result = router.apply_staged().await.expect("apply staged reload");
        assert!(result.pending_count > 0 || !result.delta.reloaded_servers.is_empty());

        tokio::time::sleep(Duration::from_millis(500)).await;
        let ext = router.take_external_updates();
        assert!(
            ext.notices.iter().any(|n| n.target == "test-server")
                || router.server_lifecycle_state("test-server")
                    == Some(McpServerLifecycleState::Active),
        );

        router.stage_remove("test-server").expect("stage remove");
        let result = router.apply_staged().await.expect("apply staged remove");
        assert_eq!(result.delta.removed_servers, vec!["test-server"]);
        assert!(matches!(
            router.server_lifecycle_state("test-server"),
            Some(McpServerLifecycleState::Removed)
        ));
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn remove_is_immediately_hidden_on_boundary_apply() {
        let server_path = mcp_test_server::fixture_binary();

        let mut router = generated_handle_owner_router();
        router
            .add_server(test_server_config("test-server", &server_path))
            .await
            .expect("add_server");

        let has_echo_before = router.list_tools().iter().any(|tool| tool.name == "echo");
        assert!(has_echo_before, "tool should be visible before remove");

        router.stage_remove("test-server").expect("stage remove");
        router.apply_staged().await.expect("apply remove");

        let has_echo_after = router.list_tools().iter().any(|tool| tool.name == "echo");
        assert!(!has_echo_after, "tool should be hidden immediately");
    }

    /// Removing a server closes its connection in a router-owned task, which
    /// `shutdown` joins: the removed server and its process group have exited
    /// when shutdown returns. (Fails-old: the close was a detached task, and
    /// rmcp waited up to 3 s after EOF before killing only the direct child.)
    #[cfg(feature = "workspace-tests")]
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn removed_stdio_server_has_exited_when_shutdown_returns() {
        use crate::stdio_test_fixture::{PidReport, process_exited, sh_mcp_server_args};
        let mut report = PidReport::new("removed-group");
        let mut router = generated_handle_owner_router();
        let config = McpServerConfig::stdio(
            "eof-ignoring",
            "/bin/sh",
            sh_mcp_server_args(Some(report.path())),
            HashMap::new(),
        );
        let (added, pids) = tokio::join!(router.add_server(config), report.pids());
        added.expect("add_server");
        let (wrapper, server) = (pids[0], pids[1]);

        router.stage_remove("eof-ignoring").expect("stage remove");
        let result = router.apply_staged().await.expect("apply remove");
        assert_eq!(result.delta.removed_servers, vec!["eof-ignoring"]);
        router.shutdown().await;

        crate::connection::tests::assert_child_reaped(wrapper);
        assert!(
            process_exited(server),
            "removed stdio server's grandchild {server} outlived router shutdown"
        );
    }

    /// Shutdown routes the close of a still-connecting server's process and
    /// of an established server through the router's owned close tasks, so
    /// their graces run concurrently rather than in turn. Both ignore stdin
    /// EOF and exit on the SIGTERM at their 2 s EOF deadline. (Fails-old:
    /// shutdown awaited each close in turn, at least 4 s here.)
    #[cfg(feature = "workspace-tests")]
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn shutdown_closes_pending_and_established_stdio_servers_concurrently() {
        use crate::stdio_test_fixture::{PidReport, process_exited, sh_mcp_server_args};
        let bounds =
            crate::McpStdioCloseBounds::new(Duration::from_secs(2), Duration::from_secs(10))
                .expect("bounds within the maximum");
        let mut router = generated_handle_owner_router().with_stdio_close_bounds(bounds);
        let mut established = PidReport::new("concurrent-established");
        let mut pending = PidReport::new("concurrent-pending");
        let established_config = McpServerConfig::stdio(
            "eof-ignoring",
            "/bin/sh",
            sh_mcp_server_args(Some(established.path())),
            HashMap::new(),
        );
        // Never answers its handshake, so its connect is still running.
        let pending_config = McpServerConfig::stdio(
            "still-connecting",
            "/bin/sh",
            vec![
                "-c".to_string(),
                format!("echo $$ > '{}'; exec sleep 60", pending.path().display()),
            ],
            HashMap::new(),
        );
        let (added, mut pids) =
            tokio::join!(router.add_server(established_config), established.pids());
        added.expect("add_server");
        router.stage_add(pending_config).expect("stage add");
        router.apply_staged().await.expect("apply staged add");
        pids.extend(pending.pids().await);

        let started = tokio::time::Instant::now();
        router.shutdown().await;
        let elapsed = started.elapsed();

        assert!(
            elapsed >= Duration::from_secs(2),
            "servers were signalled before their EOF deadline: {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_millis(3_500),
            "shutdown closed the servers in turn: {elapsed:?}"
        );
        for pid in pids {
            assert!(process_exited(pid), "stdio process {pid} outlived shutdown");
        }
    }

    /// Shutdown joins the close a failed connect attempt already started
    /// instead of cutting it short: the server, which closes its stdout
    /// (failing the handshake) and handles SIGTERM, gets the SIGTERM at its
    /// EOF deadline and runs its handler. (Fails-old: aborting the attempt
    /// dropped its close, so the group was SIGKILLed at once and the pending
    /// cleanup reported nothing.)
    #[cfg(feature = "workspace-tests")]
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn shutdown_joins_a_close_a_failed_connect_attempt_started() {
        let marker = std::env::temp_dir().join(format!(
            "meerkat-mcp-failed-attempt-{}-{}.marker",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default()
        ));
        let bounds =
            crate::McpStdioCloseBounds::new(Duration::from_millis(500), Duration::from_secs(10))
                .expect("bounds within the maximum");
        let mut router = generated_handle_owner_router().with_stdio_close_bounds(bounds);
        let config = McpServerConfig::stdio(
            "fails-handshake",
            "/bin/sh",
            vec![
                "-c".to_string(),
                format!(
                    "trap \"echo termed > '{}'; exit 0\" TERM\nexec >&-\nwhile :; do sleep 0.1; done\n",
                    marker.display()
                ),
            ],
            HashMap::new(),
        );
        router.stage_add(config).expect("stage add");
        router.apply_staged().await.expect("apply staged add");
        let custody = router
            .pending_child_custody
            .values()
            .next()
            .cloned()
            .expect("the attempt's custody");
        tokio::time::timeout(Duration::from_secs(5), async {
            while !custody.is_closing() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the failed attempt started its close");

        router.shutdown().await;

        let content = std::fs::read_to_string(&marker).unwrap_or_default();
        let _ = std::fs::remove_file(&marker);
        assert_eq!(
            content.trim(),
            "termed",
            "the server did not get its SIGTERM at the EOF deadline"
        );
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn removing_state_rejects_new_calls_and_drains_inflight() {
        let server_path = mcp_test_server::fixture_binary();

        let mut router = generated_handle_owner_router_with_timeout(Duration::from_secs(60));
        router
            .add_server(test_server_config("test-server", &server_path))
            .await
            .expect("add_server");

        router
            .set_inflight_calls_for_testing("test-server", 1)
            .expect("set inflight calls");
        router.stage_remove("test-server").expect("stage remove");
        let result = router.apply_staged().await.expect("apply remove");

        assert!(
            result.delta.removed_servers.is_empty(),
            "should remain removing"
        );
        assert!(matches!(
            router.server_lifecycle_state("test-server"),
            Some(McpServerLifecycleState::Removing { .. })
        ));

        let err = router
            .call_tool("echo", &serde_json::json!({"message": "blocked"}))
            .await
            .expect_err("new calls should be rejected while removing");
        assert!(
            matches!(err, McpError::ToolNotFound(_)),
            "removing surfaces should be absent from the published routing snapshot, got {err:?}"
        );

        router
            .set_inflight_calls_for_testing("test-server", 0)
            .expect("set inflight calls");
        let result = router
            .apply_staged()
            .await
            .expect("apply should finalize drained remove");

        assert_eq!(result.delta.removed_servers, vec!["test-server"]);
        assert!(result.delta.degraded_removals.is_empty());
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn removal_timeout_forces_close_and_reports_degraded_signal() {
        let server_path = mcp_test_server::fixture_binary();

        let mut router = generated_handle_owner_router_with_timeout(Duration::from_millis(10));
        router
            .add_server(test_server_config("test-server", &server_path))
            .await
            .expect("add_server");

        router
            .set_inflight_calls_for_testing("test-server", 1)
            .expect("set inflight calls");
        router.stage_remove("test-server").expect("stage remove");
        let result = router.apply_staged().await.expect("apply remove start");
        assert!(result.delta.removed_servers.is_empty());

        tokio::time::sleep(Duration::from_millis(30)).await;
        let result = router.apply_staged().await.expect("apply timeout finalize");

        assert_eq!(result.delta.removed_servers, vec!["test-server"]);
        assert_eq!(result.delta.degraded_removals, vec!["test-server"]);
        assert!(result.delta.lifecycle_actions.iter().any(|action| {
            action.target == "test-server"
                && action.operation == ToolConfigChangeOperation::Remove
                && action.phase == McpLifecyclePhase::Forced
        }));
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn awaiting_authorization_is_host_status_not_an_agent_notice_payload() {
        use crate::connection::tests::{FakeMcpAuthResolver, spawn_http_mcp_server};

        let (url, _state) = spawn_http_mcp_server("interactive-token").await;
        let config = McpServerConfig::streamable_http("guarded", url, HashMap::new());
        let target = McpServerIdentity::from_config(&config).unwrap();
        let resolver =
            Arc::new(FakeMcpAuthResolver::new(None, "unused").with_human_authorization_required());
        let mut router =
            generated_handle_owner_router().with_mcp_auth(McpAuthMode::Interactive, Some(resolver));
        router.stage_add(config).expect("stage add");
        router.apply_staged().await.expect("apply staged add");

        let deadline = Instant::now() + async_connect_test_timeout();
        let mut failed_detail = None;
        while failed_detail.is_none() {
            let ext = router.take_external_updates();
            failed_detail = ext
                .notices
                .into_iter()
                .find(|n| n.target == "guarded" && n.phase == McpLifecyclePhase::Failed)
                .map(|n| format!("{n:?}"));
            assert!(
                Instant::now() < deadline,
                "timed out waiting for background MCP connect"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(router.servers_awaiting_authorization(), vec![target]);
        let detail = failed_detail.unwrap_or_default();
        for secret in ["authorize", "state=", "code=", "code_challenge"] {
            assert!(!detail.contains(secret), "{secret:?} leaked: {detail}");
        }

        router.stage_remove("guarded").expect("stage remove");
        router.apply_staged().await.expect("apply staged remove");
        assert!(router.servers_awaiting_authorization().is_empty());
    }

    /// A router whose first Add of an OAuth-protected server failed with
    /// `AuthorizationRequired` and was accepted: the retained retry record
    /// exists. The resolver holds no credential until the test "logs in".
    #[cfg(feature = "workspace-tests")]
    pub(crate) async fn router_awaiting_authorization() -> (
        McpRouter,
        McpServerIdentity,
        Arc<crate::connection::tests::FakeMcpAuthResolver>,
        Arc<crate::connection::tests::HttpMcpTestState>,
    ) {
        use crate::connection::tests::{FakeMcpAuthResolver, spawn_http_mcp_server};

        let (url, server) = spawn_http_mcp_server("interactive-token").await;
        let config = McpServerConfig::streamable_http("guarded", url, HashMap::new());
        let target = McpServerIdentity::from_config(&config).unwrap();
        let resolver =
            Arc::new(FakeMcpAuthResolver::new(None, "unused").with_human_authorization_required());
        let mut router = generated_handle_owner_router()
            .with_mcp_auth(McpAuthMode::Interactive, Some(resolver.clone()));
        router.stage_add(config).expect("stage add");
        router.apply_staged().await.expect("apply staged add");
        wait_for_phase(&mut router, McpLifecyclePhase::Failed).await;
        assert_eq!(
            router.servers_awaiting_authorization(),
            vec![target.clone()]
        );
        (router, target, resolver, server)
    }

    /// A router with "guarded" connected under a stored credential for an
    /// explicit account, so the bearer refuses locally once it is gone.
    #[cfg(feature = "workspace-tests")]
    async fn router_connected_under_credential() -> (
        McpRouter,
        McpServerIdentity,
        Arc<crate::connection::tests::FakeMcpAuthResolver>,
        Arc<crate::connection::tests::HttpMcpTestState>,
    ) {
        use crate::connection::tests::{FakeMcpAuthResolver, spawn_http_mcp_server};

        let (url, server) = spawn_http_mcp_server("interactive-token").await;
        let mut config = McpServerConfig::streamable_http("guarded", url, HashMap::new());
        if let McpTransportConfig::Http(http) = &mut config.transport {
            http.oauth_account = Some("subject-7".into());
        }
        let target = McpServerIdentity::from_config(&config).unwrap();
        let resolver = Arc::new(
            FakeMcpAuthResolver::new(Some("interactive-token"), "unused")
                .with_human_authorization_required(),
        );
        let mut router = generated_handle_owner_router()
            .with_mcp_auth(McpAuthMode::Interactive, Some(resolver.clone()));
        router.stage_add(config).expect("stage add");
        router.apply_staged().await.expect("apply staged add");
        wait_for_phase(&mut router, McpLifecyclePhase::Applied).await;
        assert_eq!(router.active_server_names(), vec!["guarded".to_string()]);
        assert!(router.list_tools().iter().any(|tool| tool.name == "echo"));
        (router, target, resolver, server)
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn sign_out_unpublishes_the_connected_server_and_sign_in_adds_it_back() {
        let (mut router, target, resolver, _server) = router_connected_under_credential().await;
        *resolver.stored_token_for_testing() = None;

        assert_eq!(
            router.credential_released(&target).unwrap(),
            ReleasedCredential::Unpublished
        );
        // A duplicate release while the removal is staged changes nothing.
        assert_eq!(
            router.credential_released(&target).unwrap(),
            ReleasedCredential::AlreadyPending
        );
        router.apply_staged().await.expect("apply the release");
        assert!(router.active_server_names().is_empty());
        assert!(!router.list_tools().iter().any(|tool| tool.name == "echo"));
        // It awaits sign-in for exactly its credential.
        assert_eq!(
            router.servers_awaiting_authorization(),
            vec![target.clone()]
        );
        assert_eq!(
            router.credential_released(&target).unwrap(),
            ReleasedCredential::NotActive
        );

        // A new sign-in adds the same server back to this router.
        *resolver.stored_token_for_testing() = Some("interactive-token".into());
        assert_eq!(
            router.retry_authorized(&target).unwrap(),
            AuthorizedRetry::Staged
        );
        router.apply_staged().await.expect("apply the retry");
        wait_for_phase(&mut router, McpLifecyclePhase::Applied).await;
        assert_eq!(router.active_server_names(), vec!["guarded".to_string()]);
        assert!(router.list_tools().iter().any(|tool| tool.name == "echo"));
        assert!(router.servers_awaiting_authorization().is_empty());
        router.shutdown().await;
    }

    /// A resolver serving one realm, as a native host binds it: the
    /// connection reports and resolves realm-scoped targets.
    struct RealmScopedResolver {
        inner: Arc<crate::connection::tests::FakeMcpAuthResolver>,
        realm: meerkat_core::connection::RealmId,
    }

    #[async_trait::async_trait]
    impl McpAuthResolver for RealmScopedResolver {
        async fn stored_bearer_token(
            &self,
            target: &McpServerIdentity,
        ) -> Result<Option<String>, meerkat_auth_core::McpOAuthError> {
            self.inner.stored_bearer_token(target).await
        }

        async fn interactive_login(
            &self,
            target: &McpServerIdentity,
            www_authenticate: Option<&str>,
        ) -> Result<String, meerkat_auth_core::McpOAuthError> {
            self.inner.interactive_login(target, www_authenticate).await
        }

        fn credential_realm(&self) -> Option<meerkat_core::connection::RealmId> {
            Some(self.realm.clone())
        }
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn sign_out_matches_the_realm_scoped_target_the_host_reports() {
        use crate::connection::tests::{FakeMcpAuthResolver, spawn_http_mcp_server};

        let (url, _server) = spawn_http_mcp_server("interactive-token").await;
        let mut config = McpServerConfig::streamable_http("guarded", url, HashMap::new());
        if let McpTransportConfig::Http(http) = &mut config.transport {
            http.oauth_account = Some("subject-7".into());
        }
        let realm = meerkat_core::connection::RealmId::parse("alpha").unwrap();
        let unscoped = McpServerIdentity::from_config(&config).unwrap();
        let target = unscoped.clone().in_realm(realm.clone());
        let fake = Arc::new(
            FakeMcpAuthResolver::new(Some("interactive-token"), "unused")
                .with_human_authorization_required(),
        );
        let resolver = Arc::new(RealmScopedResolver {
            inner: Arc::clone(&fake),
            realm,
        });
        let mut router =
            generated_handle_owner_router().with_mcp_auth(McpAuthMode::Interactive, Some(resolver));
        router.stage_add(config).expect("stage add");
        router.apply_staged().await.expect("apply staged add");
        wait_for_phase(&mut router, McpLifecyclePhase::Applied).await;

        // The realm-less identity is another credential slot: no match.
        assert_eq!(
            router.credential_released(&unscoped).unwrap(),
            ReleasedCredential::NotActive
        );
        // The host's realm-scoped target matches and unpublishes.
        *fake.stored_token_for_testing() = None;
        assert_eq!(
            router.credential_released(&target).unwrap(),
            ReleasedCredential::Unpublished
        );
        router.apply_staged().await.expect("apply the release");
        assert!(router.active_server_names().is_empty());
        assert_eq!(
            router.servers_awaiting_authorization(),
            vec![target.clone()]
        );

        // A sign-in for that same target adds it back.
        *fake.stored_token_for_testing() = Some("interactive-token".into());
        assert_eq!(
            router.retry_authorized(&target).unwrap(),
            AuthorizedRetry::Staged
        );
        router.apply_staged().await.expect("apply the retry");
        wait_for_phase(&mut router, McpLifecyclePhase::Applied).await;
        assert_eq!(router.active_server_names(), vec!["guarded".to_string()]);
        router.shutdown().await;
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn a_release_of_another_credential_leaves_the_server_published() {
        let (mut router, target, _resolver, _server) = router_connected_under_credential().await;
        let mut other_account =
            McpServerConfig::streamable_http("guarded", target.server_url(), HashMap::new());
        if let McpTransportConfig::Http(http) = &mut other_account.transport {
            http.oauth_account = Some("another-account".into());
        }
        let other_account = McpServerIdentity::from_config(&other_account).unwrap();
        let other_url = McpServerIdentity::from_config(&{
            let mut config = McpServerConfig::streamable_http(
                "guarded",
                "http://127.0.0.1:1/other",
                HashMap::new(),
            );
            if let McpTransportConfig::Http(http) = &mut config.transport {
                http.oauth_account = Some("subject-7".into());
            }
            config
        })
        .unwrap();
        for other in [other_account, other_url] {
            assert_ne!(other, target);
            assert_eq!(
                router.credential_released(&other).unwrap(),
                ReleasedCredential::NotActive
            );
        }
        router.apply_staged().await.expect("apply nothing");
        assert_eq!(router.active_server_names(), vec!["guarded".to_string()]);
        assert!(router.servers_awaiting_authorization().is_empty());
        router.shutdown().await;
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn a_newer_staged_intent_wins_over_a_release() {
        let (mut router, target, _resolver, _server) = router_connected_under_credential().await;
        router.stage_reload("guarded").expect("stage reload");
        assert_eq!(
            router.credential_released(&target).unwrap(),
            ReleasedCredential::AlreadyPending
        );
        assert!(router.servers_awaiting_authorization().is_empty());
        router.shutdown().await;
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn a_call_without_a_usable_credential_unpublishes_at_the_next_boundary() {
        let (mut router, target, resolver, server) = router_connected_under_credential().await;
        let seen_before = server.seen_authorizations.lock().unwrap().len();
        // Signed out in another process: this router got no release.
        *resolver.stored_token_for_testing() = None;

        let refused = router
            .call_tool("echo", &serde_json::json!({ "message": "hi" }))
            .await;
        assert!(
            matches!(refused, Err(McpError::AuthorizationRequired { .. })),
            "the call is refused for its missing credential: {refused:?}"
        );
        // Refused before anything was sent.
        assert_eq!(
            server.seen_authorizations.lock().unwrap().len(),
            seen_before
        );
        // Still listed until the boundary; the boundary releases it.
        assert_eq!(router.active_server_names(), vec!["guarded".to_string()]);
        router.apply_staged().await.expect("apply the release");
        assert!(router.active_server_names().is_empty());
        assert!(!router.list_tools().iter().any(|tool| tool.name == "echo"));
        assert_eq!(router.servers_awaiting_authorization(), vec![target]);
        router.shutdown().await;
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn a_sign_in_before_the_release_boundary_restores_the_server_without_another_sign_in() {
        let (mut router, target, resolver, _server) = router_connected_under_credential().await;
        *resolver.stored_token_for_testing() = None;
        assert_eq!(
            router.credential_released(&target).unwrap(),
            ReleasedCredential::Unpublished
        );
        // The sign-in completes before the release's removal applies.
        *resolver.stored_token_for_testing() = Some("interactive-token".into());
        assert_eq!(
            router.retry_authorized(&target).unwrap(),
            AuthorizedRetry::AfterRelease
        );
        // This boundary applies the removal, which closes the connection.
        router.apply_staged().await.expect("apply the release");
        assert!(router.active_server_names().is_empty());
        // The next boundary stages the server again and connects it, with no
        // further sign-in.
        router
            .apply_staged()
            .await
            .expect("apply the retained retry");
        wait_for_phase(&mut router, McpLifecyclePhase::Applied).await;
        assert_eq!(router.active_server_names(), vec!["guarded".to_string()]);
        assert!(router.list_tools().iter().any(|tool| tool.name == "echo"));
        assert!(router.servers_awaiting_authorization().is_empty());
        router.shutdown().await;
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn a_refused_restage_is_reported_and_the_sign_in_is_kept() {
        let (mut router, target, resolver, _server) = router_connected_under_credential().await;
        *resolver.stored_token_for_testing() = None;
        router.credential_released(&target).unwrap();
        *resolver.stored_token_for_testing() = Some("interactive-token".into());
        assert_eq!(
            router.retry_authorized(&target).unwrap(),
            AuthorizedRetry::AfterRelease
        );
        router.apply_staged().await.expect("apply the release");
        assert!(router.active_server_names().is_empty());

        // The owner refuses the re-add: the boundary reports it and the
        // sign-in stays kept.
        let slot = router
            .external_surface_handle_slot()
            .expect("a runtime surface owner");
        let owner = Arc::clone(&*slot.read().unwrap());
        *slot.write().unwrap() = Arc::new(UnboundExternalToolSurfaceHandle::new());
        let refused = router.apply_staged().await.expect("the boundary runs");
        assert!(
            refused.delta.lifecycle_actions.iter().any(|action| {
                action.target == "guarded"
                    && action.operation == ToolConfigChangeOperation::Add
                    && action.phase == McpLifecyclePhase::Failed
            }),
            "the refused re-add is in the boundary result: {:?}",
            refused.delta.lifecycle_actions
        );
        assert!(!router.staged_payloads.contains_key("guarded"));
        assert!(
            router
                .awaiting_authorization
                .get("guarded")
                .is_some_and(|awaiting| awaiting.signed_in_during_release),
            "the completed sign-in is kept, not consumed"
        );

        // With the owner back, the next boundary stages and applies it.
        *slot.write().unwrap() = owner;
        router.apply_staged().await.expect("the boundary applies");
        wait_for_phase(&mut router, McpLifecyclePhase::Applied).await;
        assert_eq!(router.active_server_names(), vec!["guarded".to_string()]);
        assert!(router.servers_awaiting_authorization().is_empty());
        router.shutdown().await;
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn a_sign_out_after_a_retained_sign_in_supersedes_it() {
        let (mut router, target, resolver, _server) = router_connected_under_credential().await;
        *resolver.stored_token_for_testing() = None;
        router.credential_released(&target).unwrap();
        *resolver.stored_token_for_testing() = Some("interactive-token".into());
        assert_eq!(
            router.retry_authorized(&target).unwrap(),
            AuthorizedRetry::AfterRelease
        );
        // Signed out again before the boundary: the retained sign-in is gone.
        *resolver.stored_token_for_testing() = None;
        assert_eq!(
            router.credential_released(&target).unwrap(),
            ReleasedCredential::AlreadyPending
        );
        router.apply_staged().await.expect("apply the release");
        router.apply_staged().await.expect("apply nothing more");
        assert!(router.active_server_names().is_empty());
        assert_eq!(router.servers_awaiting_authorization(), vec![target]);
        router.shutdown().await;
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn an_explicit_remove_during_a_release_withdraws_the_sign_in_record() {
        let (mut router, target, resolver, _server) = router_connected_under_credential().await;
        *resolver.stored_token_for_testing() = None;
        router.credential_released(&target).unwrap();
        // The host removes the server itself: its intent is newer.
        router.stage_remove("guarded").expect("stage remove");
        *resolver.stored_token_for_testing() = Some("interactive-token".into());
        assert_eq!(
            router.retry_authorized(&target).unwrap(),
            AuthorizedRetry::NotAwaiting
        );
        router.apply_staged().await.expect("apply the remove");
        router.apply_staged().await.expect("apply nothing more");
        assert!(router.active_server_names().is_empty());
        assert!(router.servers_awaiting_authorization().is_empty());
        router.shutdown().await;
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn a_sign_in_before_a_queued_call_refusal_keeps_the_server_published() {
        let (mut router, target, resolver, _server) = router_connected_under_credential().await;
        *resolver.stored_token_for_testing() = None;
        let refused = router
            .call_tool("echo", &serde_json::json!({ "message": "hi" }))
            .await;
        assert!(matches!(
            refused,
            Err(McpError::AuthorizationRequired { .. })
        ));
        // Signed in again before the boundary that would release it.
        *resolver.stored_token_for_testing() = Some("interactive-token".into());
        assert_eq!(
            router.retry_authorized(&target).unwrap(),
            AuthorizedRetry::NotAwaiting
        );
        router.apply_staged().await.expect("apply the boundary");
        assert_eq!(router.active_server_names(), vec!["guarded".to_string()]);
        assert!(router.list_tools().iter().any(|tool| tool.name == "echo"));
        assert!(router.servers_awaiting_authorization().is_empty());
        router.shutdown().await;
    }

    pub(crate) async fn wait_for_phase(router: &mut McpRouter, phase: McpLifecyclePhase) {
        let deadline = Instant::now() + async_connect_test_timeout();
        loop {
            let ext = router.take_external_updates();
            if ext
                .notices
                .iter()
                .any(|notice| notice.target == "guarded" && notice.phase == phase)
            {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {phase:?}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn retry_after_login_reconnects_the_failed_first_add_once_at_the_boundary() {
        let (mut router, target, resolver, _server) = router_awaiting_authorization().await;
        *resolver.stored_token_for_testing() = Some("interactive-token".into());

        assert_eq!(
            router.retry_authorized(&target).unwrap(),
            AuthorizedRetry::Staged
        );
        // A duplicate trigger while the retry is staged changes nothing.
        assert_eq!(
            router.retry_authorized(&target).unwrap(),
            AuthorizedRetry::AlreadyPending
        );
        // Nothing connects before the boundary.
        assert!(router.active_server_names().is_empty());

        router.apply_staged().await.expect("apply the retry");
        // A duplicate trigger while the retry is in flight changes nothing.
        assert_eq!(
            router.retry_authorized(&target).unwrap(),
            AuthorizedRetry::AlreadyPending
        );
        wait_for_phase(&mut router, McpLifecyclePhase::Applied).await;
        assert_eq!(router.active_server_names(), vec!["guarded".to_string()]);
        assert!(router.servers_awaiting_authorization().is_empty());
        // The record was answered: a later trigger is not a second retry.
        assert_eq!(
            router.retry_authorized(&target).unwrap(),
            AuthorizedRetry::NotAwaiting
        );
        router.shutdown().await;
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn retry_touches_only_the_exact_retained_target() {
        let (mut router, target, _resolver, _server) = router_awaiting_authorization().await;
        let other_url = McpServerIdentity::from_config(&McpServerConfig::streamable_http(
            "guarded",
            "http://127.0.0.1:1/other",
            HashMap::new(),
        ))
        .unwrap();
        assert_eq!(
            router.retry_authorized(&other_url).unwrap(),
            AuthorizedRetry::NotAwaiting
        );
        let mut with_account =
            McpServerConfig::streamable_http("guarded", target.server_url(), HashMap::new());
        if let McpTransportConfig::Http(http) = &mut with_account.transport {
            http.oauth_account = Some("another-account".into());
        }
        let other_account = McpServerIdentity::from_config(&with_account).unwrap();
        assert_ne!(other_account, target);
        assert_eq!(
            router.retry_authorized(&other_account).unwrap(),
            AuthorizedRetry::NotAwaiting
        );
        assert_eq!(
            router.servers_awaiting_authorization(),
            vec![target.clone()]
        );

        let mut fresh = generated_handle_owner_router();
        assert_eq!(
            fresh.retry_authorized(&target).unwrap(),
            AuthorizedRetry::NotAwaiting
        );
        router.shutdown().await;
        fresh.shutdown().await;
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn a_remove_staged_ahead_of_the_old_failure_wins() {
        use crate::connection::tests::{FakeMcpAuthResolver, spawn_http_mcp_server};

        let (url, _server) = spawn_http_mcp_server("interactive-token").await;
        let config = McpServerConfig::streamable_http("guarded", url, HashMap::new());
        let target = McpServerIdentity::from_config(&config).unwrap();
        let resolver =
            Arc::new(FakeMcpAuthResolver::new(None, "unused").with_human_authorization_required());
        let mut router = generated_handle_owner_router()
            .with_mcp_auth(McpAuthMode::Interactive, Some(resolver.clone()));
        router.stage_add(config).expect("stage add");
        router.apply_staged().await.expect("apply staged add");
        // The remove is staged before the add's failure is observed.
        router.stage_remove("guarded").expect("stage remove");
        let deadline = Instant::now() + async_connect_test_timeout();
        while router.connect_results_delivered.load(Ordering::Acquire) == 0 {
            assert!(Instant::now() < deadline, "connect result never arrived");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        *resolver.stored_token_for_testing() = Some("interactive-token".into());
        // Hook before the removal boundary: the old failure is observed now,
        // and the newer remove keeps it from becoming a retry.
        assert_eq!(
            router.retry_authorized(&target).unwrap(),
            AuthorizedRetry::NotAwaiting
        );
        assert!(router.servers_awaiting_authorization().is_empty());
        router.apply_staged().await.expect("apply staged remove");
        // Hook after the removal boundary: nothing to retry.
        assert_eq!(
            router.retry_authorized(&target).unwrap(),
            AuthorizedRetry::NotAwaiting
        );
        router.apply_staged().await.expect("nothing staged");
        assert!(router.active_server_names().is_empty());
        router.shutdown().await;
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn an_accepted_host_remove_withdraws_the_retry() {
        let (mut router, target, resolver, _server) = router_awaiting_authorization().await;
        *resolver.stored_token_for_testing() = Some("interactive-token".into());
        router.stage_remove("guarded").expect("host remove");
        assert_eq!(
            router.retry_authorized(&target).unwrap(),
            AuthorizedRetry::NotAwaiting
        );
        router.apply_staged().await.expect("apply remove");
        assert!(router.active_server_names().is_empty());
        router.shutdown().await;
    }

    /// A lifecycle mirror that rejects connect-pending only.
    struct RejectConnectPendingMirror;

    impl McpServerLifecycleHandle for RejectConnectPendingMirror {
        fn apply_connect_pending(&self, _server_id: &str) -> Result<(), DslTransitionError> {
            Err(DslTransitionError::guard_rejected(
                "RejectConnectPendingMirror::apply_connect_pending",
                "injected lifecycle mirror rejection",
            ))
        }
        fn apply_connected(&self, _server_id: &str) -> Result<(), DslTransitionError> {
            Ok(())
        }
        fn apply_failed(&self, _server_id: &str, _error: &str) -> Result<(), DslTransitionError> {
            Ok(())
        }
        fn apply_disconnected(&self, _server_id: &str) -> Result<(), DslTransitionError> {
            Ok(())
        }
        fn apply_reload(&self, _server_id: &str) -> Result<(), DslTransitionError> {
            Ok(())
        }
        fn pending_server_ids(&self) -> BTreeSet<String> {
            BTreeSet::new()
        }
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn retry_mirror_fault_is_typed_and_the_boundary_runs_the_retained_config() {
        let (mut router, target, resolver, _server) = router_awaiting_authorization().await;
        let retained = router.awaiting_authorization["guarded"].config.clone();
        *resolver.stored_token_for_testing() = Some("interactive-token".into());
        *router.mcp_lifecycle_handle_slot().write().unwrap() =
            Some(Arc::new(RejectConnectPendingMirror) as Arc<dyn McpServerLifecycleHandle>);

        let error = router
            .retry_authorized(&target)
            .expect_err("a rejected lifecycle mirror is a typed staging fault");
        assert!(
            matches!(
                &error,
                McpError::LifecycleMirrorRejected { server, .. } if server == "guarded"
            ),
            "{error}"
        );
        // The owner accepted the intent and its exact retained config is
        // staged for the boundary.
        assert_eq!(router.staged_payloads.get("guarded"), Some(&retained));
        let applied = router.apply_staged().await.expect("the boundary applies");
        assert!(applied.delta.rejected_boundaries.is_empty());
        wait_for_phase(&mut router, McpLifecyclePhase::Applied).await;
        assert_eq!(router.active_server_names(), vec!["guarded".to_string()]);
        router.shutdown().await;
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn an_owner_staging_rejection_keeps_the_record_for_a_later_retry() {
        let (mut router, target, resolver, _server) = router_awaiting_authorization().await;
        *resolver.stored_token_for_testing() = Some("interactive-token".into());
        let slot = router
            .external_surface_handle_slot()
            .expect("a runtime surface owner");
        let owner = Arc::clone(&*slot.read().unwrap());
        *slot.write().unwrap() = Arc::new(UnboundExternalToolSurfaceHandle::new());

        assert!(
            router.retry_authorized(&target).is_err(),
            "the owner refused the retry"
        );
        assert!(router.staged_payloads.is_empty());
        assert_eq!(
            router.servers_awaiting_authorization(),
            vec![target.clone()]
        );

        *slot.write().unwrap() = owner;
        assert_eq!(
            router.retry_authorized(&target).unwrap(),
            AuthorizedRetry::Staged
        );
        router.apply_staged().await.expect("the boundary applies");
        wait_for_phase(&mut router, McpLifecyclePhase::Applied).await;
        assert_eq!(router.active_server_names(), vec!["guarded".to_string()]);
        router.shutdown().await;
    }

    /// A stale result for an older attempt (owner-rejected) neither erases
    /// nor replaces the newer retry record.
    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn stale_results_cannot_clear_or_replace_a_newer_retry_record() {
        use crate::connection::tests::{FakeMcpAuthResolver, spawn_http_mcp_server};

        let (url, _server) = spawn_http_mcp_server("interactive-token").await;
        let config = McpServerConfig::streamable_http("guarded", url, HashMap::new());
        let target = McpServerIdentity::from_config(&config).unwrap();
        let resolver =
            Arc::new(FakeMcpAuthResolver::new(None, "unused").with_human_authorization_required());
        let mut router = generated_handle_owner_router()
            .with_mcp_auth(McpAuthMode::Interactive, Some(resolver.clone()));
        router.stage_add(config.clone()).expect("stage add");
        router.apply_staged().await.expect("apply staged add");
        let first = router
            .pending_obligations
            .get("guarded")
            .cloned()
            .expect("the first attempt is pending");
        wait_for_phase(&mut router, McpLifecyclePhase::Failed).await;
        // A newer attempt (the retry) fails too and replaces the record.
        assert_eq!(
            router.retry_authorized(&target).unwrap(),
            AuthorizedRetry::Staged
        );
        router.apply_staged().await.expect("apply the retry");
        wait_for_phase(&mut router, McpLifecyclePhase::Failed).await;
        assert_eq!(
            router.servers_awaiting_authorization(),
            vec![target.clone()]
        );

        let mut other = config.clone();
        other.connect_timeout_secs = Some(1);
        // A stale SUCCESS for the old attempt (a real connection) is
        // rejected by the owner: it neither clears nor replaces the record,
        // and its connection is closed.
        *resolver.stored_token_for_testing() = Some("interactive-token".into());
        let stale_connection = McpConnection::connect_and_enumerate_with_custody(
            &config,
            McpAuthMode::Interactive,
            Some(resolver.clone()),
            None,
            None,
            &crate::McpStdioLaunchProfile::trusted_host(),
            None,
        )
        .await
        .expect("a real connection for the stale success");
        *resolver.stored_token_for_testing() = None;
        router.process_pending_result(PendingResult {
            obligation: first.clone(),
            config: other.clone(),
            result: Ok(stale_connection),
            guard: None,
        });
        let record = &router.awaiting_authorization["guarded"];
        assert_eq!(record.config, config, "a stale success replaced the record");
        assert!(router.active_server_names().is_empty());
        for stale in [
            Err(McpError::AuthorizationRequired {
                target: Box::new(target.clone()),
            }),
            Err(McpError::ConnectionFailed {
                reason: "stale failure".into(),
            }),
        ] {
            router.process_pending_result(PendingResult {
                obligation: first.clone(),
                config: other.clone(),
                result: stale,
                guard: None,
            });
            let record = &router.awaiting_authorization["guarded"];
            assert_eq!(record.target, target);
            assert_eq!(record.config, config, "a stale result replaced the record");
        }
        router.shutdown().await;
    }

    /// r1 5 (attempt guards): a success whose obligation is already settled
    /// is stale; it cancels its guard as `NotInstalled{Superseded}` with its
    /// own close. The surface owner refuses a second boundary while an
    /// attempt is pending, so, as in the stale-results test above, the stale
    /// result is replayed for an obligation that already completed.
    #[cfg(feature = "workspace-tests")]
    #[cfg(unix)]
    #[tokio::test]
    async fn a_superseded_success_cancels_as_not_installed() {
        type Finishes = std::sync::Mutex<Vec<(bool, crate::McpAttemptDisposition)>>;
        struct Guard(Arc<Finishes>);
        #[async_trait::async_trait]
        impl crate::McpAttemptGuard for Guard {
            async fn settle(
                self: Box<Self>,
                disposition: crate::McpAttemptDisposition,
            ) -> Result<(), crate::McpGuardFinishError> {
                self.0.lock().unwrap().push((true, disposition));
                Ok(())
            }
            async fn cancel(
                self: Box<Self>,
                disposition: crate::McpAttemptDisposition,
            ) -> Result<(), crate::McpGuardFinishError> {
                self.0.lock().unwrap().push((false, disposition));
                Ok(())
            }
        }
        struct Factory(Arc<Finishes>);
        #[async_trait::async_trait]
        impl crate::McpAttemptGuardFactory for Factory {
            async fn begin(
                &self,
                _request: crate::McpAttemptGuardRequest,
                _cancel: crate::McpAttemptCancellation,
            ) -> Result<Option<Box<dyn crate::McpAttemptGuard>>, McpAttachmentRefusal> {
                Ok(Some(Box::new(Guard(Arc::clone(&self.0)))))
            }
        }

        let finishes: Arc<Finishes> = Arc::default();
        let guards = crate::McpAttemptGuards::new(
            Arc::new(Factory(Arc::clone(&finishes))),
            crate::McpAttemptGuardBounds::DEFAULT,
        );
        // `wait_for_phase` watches the server named "guarded".
        let config = test_server_config("guarded", &mcp_test_server::fixture_binary());
        let mut router = generated_handle_owner_router().with_attempt_guards(
            guards,
            crate::McpConstructionOwner::new(meerkat_core::types::SessionId::new(), None),
        );
        router.stage_add(config.clone()).expect("stage add");
        router.apply_staged().await.expect("apply staged add");
        let first = router
            .pending_obligations
            .get("guarded")
            .cloned()
            .expect("the first attempt is pending");
        wait_for_phase(&mut router, McpLifecyclePhase::Applied).await;

        // A later guarded attempt of the server, whose success arrives for
        // the settled obligation: its custody holds its guard.
        let attempt = first.pending_task_sequence + 1;
        let record = router
            .attempt_guards
            .clone()
            .expect("installed guards")
            .begin_attempt(&config, attempt, crate::McpAttemptOperation::Add)
            .await
            .expect("begun")
            .expect("a guard");
        let custody = StdioChildCustody::with_bounds(crate::McpStdioCloseBounds::DEFAULT);
        custody.hold_guard(Arc::clone(&record));
        let stale = McpConnection::connect_and_enumerate_with_custody(
            &config,
            router.mcp_auth_mode,
            None,
            None,
            Some(custody),
            &crate::McpStdioLaunchProfile::trusted_host(),
            None,
        )
        .await
        .expect("a real connection for the stale success");
        router.process_pending_result(PendingResult {
            obligation: first,
            config: config.clone(),
            result: Ok(stale),
            guard: Some(record),
        });
        tokio::time::timeout(async_connect_test_timeout(), async {
            while finishes.lock().unwrap().is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the stale attempt's finish");
        let (settled, disposition) = finishes.lock().unwrap()[0].clone();
        assert!(!settled, "a superseded attempt is cancelled");
        assert_eq!(disposition.key.attempt, attempt);
        assert_eq!(
            disposition.open,
            crate::McpAttemptOpenOutcome::NotInstalled {
                reason: crate::McpAttemptNotInstalled::Superseded,
            }
        );
        let crate::McpAttemptProcessEnd::Closed(receipt) = &disposition.process else {
            panic!("closed by its own close: {disposition:?}");
        };
        assert!(matches!(
            receipt.leader,
            crate::McpStdioLeaderOutcome::Reaped(_)
        ));
        assert_eq!(router.active_server_names(), vec!["guarded".to_string()]);

        // The installed attempt settles once the shutdown closed it.
        router.shutdown().await;
        tokio::time::timeout(async_connect_test_timeout(), async {
            while finishes.lock().unwrap().len() < 2 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the installed attempt's settle");
        let finishes = finishes.lock().unwrap().clone();
        assert_eq!(finishes.len(), 2, "one finish per attempt: {finishes:#?}");
        assert!(finishes[1].0, "the installed attempt settles");
        assert_ne!(finishes[1].1.key.attempt, attempt);
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn apply_staged_add_is_non_blocking() {
        let server_path = mcp_test_server::fixture_binary();

        let mut router = generated_handle_owner_router();
        router
            .stage_add(test_server_config("test-server", &server_path))
            .expect("stage add");
        let result = router.apply_staged().await.expect("apply staged add");

        assert!(
            result.pending_count > 0,
            "server should be pending after non-blocking add"
        );
        assert!(result.delta.lifecycle_actions.iter().any(|action| {
            action.target == "test-server"
                && action.operation == ToolConfigChangeOperation::Add
                && action.phase == McpLifecyclePhase::Pending
        }));

        let deadline = Instant::now() + async_connect_test_timeout();
        loop {
            let ext = router.take_external_updates();
            if ext
                .notices
                .iter()
                .any(|n| n.target == "test-server" && n.phase == McpLifecyclePhase::Applied)
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for background MCP connect"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        assert!(
            !router.list_tools().is_empty(),
            "tools should be visible after activation"
        );
    }

    #[tokio::test]
    #[ignore = "exercises real subprocess timeout behavior"]
    async fn connect_and_enumerate_times_out() {
        let mut config = McpServerConfig::stdio(
            "hang-server",
            "sleep",
            vec!["60".to_string()],
            HashMap::new(),
        );
        config.connect_timeout_secs = Some(1);

        let result = McpConnection::connect_and_enumerate(&config).await;
        assert!(result.is_err(), "should time out");
        let err_msg = result.err().expect("already checked").to_string();
        assert!(
            err_msg.contains("Timed out"),
            "error should mention timeout: {err_msg}"
        );
    }

    /// A server with an access ceiling offers and admits exactly its allowed
    /// operations, under their exposed names too: the others are neither
    /// published nor callable, through the router or directly on its
    /// connection. The ceiling does not certify the allowed ones read-only.
    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn access_ceiling_admits_only_its_allowed_operations() {
        let server_path = mcp_test_server::fixture_binary();
        let mut config = test_server_config("read-only", &server_path);
        config.access = Some(meerkat_core::McpServerAccess::allowing(["echo"]));
        config.tool_names.insert("add".into(), "plus".into());
        let mut router = generated_handle_owner_router();
        router.add_server(config.clone()).await.expect("add_server");

        let published: Vec<String> = router
            .list_tools()
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        assert_eq!(published, vec!["echo".to_string()]);
        assert_eq!(
            AgentToolDispatcher::tool_mutation_class(&router, "echo"),
            meerkat_core::ToolMutationClass::Unknown,
            "a config ceiling narrows access but does not certify read-only"
        );
        router
            .call_tool("echo", &serde_json::json!({"message": "hi"}))
            .await
            .expect("a listed operation is callable");
        for name in ["add", "plus"] {
            let refused = router
                .call_tool(name, &serde_json::json!({"a": 1, "b": 2}))
                .await
                .expect_err("an operation outside the ceiling is not routed");
            assert!(matches!(refused, McpError::ToolNotFound(_)), "{refused:?}");
        }
        router
            .call_tool("echo", &serde_json::json!({"message": "again"}))
            .await
            .expect("an allowed sibling still works after a refusal");

        let conn = McpConnection::connect(&config).await.expect("connect");
        let refused = conn
            .call_tool("add", &serde_json::json!({"a": 1, "b": 2}))
            .await
            .expect_err("the connection refuses an unlisted operation");
        assert!(
            matches!(
                &refused,
                McpError::OperationNotPermitted { server, operation }
                    if server == "read-only" && operation == "add"
            ),
            "{refused:?}"
        );
        conn.close().await.expect("close");
        router.shutdown().await;
    }

    /// The access ceiling and a connector binding intersect at call time: a
    /// binding broader than the ceiling is valid configuration, an operation
    /// the binding lists but the ceiling refuses is refused locally before
    /// any connector preparation, and an operation both admit is prepared.
    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn access_ceiling_and_connector_binding_intersect_at_call_time() {
        struct CountingProvider(std::sync::Mutex<Vec<String>>);
        #[async_trait::async_trait]
        impl crate::McpCallContextProvider for CountingProvider {
            async fn prepare(
                &self,
                target: crate::McpCallTarget<'_>,
                _: meerkat_core::types::ToolCallView<'_>,
                _: &meerkat_core::ToolDispatchContext,
            ) -> Result<Option<crate::McpCallContext>, crate::McpCallContextError> {
                self.0.lock().unwrap().push(target.raw_operation.to_owned());
                Ok(Some(crate::McpCallContext::new(
                    serde_json::Map::from_iter([(
                        "io.example/connector".to_owned(),
                        serde_json::json!({"authorization": "Bearer fixture"}),
                    )]),
                    (),
                )))
            }
        }
        let server_path = mcp_test_server::fixture_binary();
        let mut config = test_server_config("bound", &server_path);
        config.access = Some(meerkat_core::McpServerAccess::allowing(["echo"]));
        let operation = |destination: &str| meerkat_core::mcp_config::McpConnectorOperation {
            required_scopes: Default::default(),
            destination_origin: destination.to_owned(),
            read_only: false,
        };
        config.connector = Some(meerkat_core::mcp_config::McpConnectorBinding {
            realm_id: "tenant-a".into(),
            slot_id: "work".into(),
            issuer: "https://issuer.example".into(),
            client: "client".into(),
            resource: "https://api.example".into(),
            strategy_id: "oidc-userinfo-v1".into(),
            account: "subject-a".into(),
            operations: BTreeMap::from([
                ("echo".to_owned(), operation("https://api.example")),
                ("add".to_owned(), operation("https://api.example")),
            ]),
        });
        let provider = Arc::new(CountingProvider(std::sync::Mutex::new(Vec::new())));
        let mut router =
            generated_handle_owner_router().with_call_context_provider(provider.clone());
        router
            .add_server(config)
            .await
            .expect("a binding broader than the ceiling is valid configuration");

        let refused = router
            .call_tool("add", &serde_json::json!({"a": 1, "b": 2}))
            .await
            .expect_err("the ceiling refuses an operation the binding lists");
        assert!(
            matches!(
                refused,
                McpError::ToolNotFound(_) | McpError::OperationNotPermitted { .. }
            ),
            "{refused:?}"
        );
        assert!(
            provider.0.lock().unwrap().is_empty(),
            "the refused operation reached no connector preparation"
        );
        router
            .call_tool("echo", &serde_json::json!({"message": "hi"}))
            .await
            .expect("an operation both admit is callable");
        assert_eq!(*provider.0.lock().unwrap(), vec!["echo".to_owned()]);
        router.shutdown().await;
    }

    /// An access ceiling that admits nothing is refused before staging.
    #[test]
    fn an_empty_access_ceiling_is_refused_at_staging() {
        let mut config = McpServerConfig::stdio("empty", "/bin/true", vec![], HashMap::new());
        config.access = Some(meerkat_core::McpServerAccess::allowing(Vec::<String>::new()));
        let refused = McpRouter::validate_tool_names(&config).expect_err("empty ceiling");
        assert!(
            matches!(refused, McpError::InvalidAccessPolicy(_)),
            "{refused:?}"
        );
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn add_remove_add_discards_stale_generation() {
        let server_path = mcp_test_server::fixture_binary();

        let mut router = generated_handle_owner_router();

        router
            .stage_add(test_server_config("test-server", &server_path))
            .expect("stage add");
        let result = router.apply_staged().await.expect("first add");
        assert_eq!(result.pending_count, 1);

        router.stage_remove("test-server").expect("stage remove");
        router.apply_staged().await.expect("remove");

        router
            .stage_add(test_server_config("test-server", &server_path))
            .expect("stage add");
        let result = router.apply_staged().await.expect("second add");
        assert_eq!(result.pending_count, 1);

        let deadline = Instant::now() + async_connect_test_timeout();
        loop {
            let ext = router.take_external_updates();
            if ext
                .notices
                .iter()
                .any(|n| n.target == "test-server" && n.phase == McpLifecyclePhase::Applied)
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for second add to complete"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        assert!(matches!(
            router.server_lifecycle_state("test-server"),
            Some(McpServerLifecycleState::Active)
        ));
    }
}

#[cfg(all(test, feature = "workspace-tests"))]
#[path = "call_context_tests.rs"]
mod call_context_tests;

#[cfg(test)]
#[path = "shutdown_report_tests.rs"]
mod shutdown_report_tests;

#[cfg(all(test, feature = "workspace-tests"))]
#[path = "apps_tests.rs"]
mod apps_tests;

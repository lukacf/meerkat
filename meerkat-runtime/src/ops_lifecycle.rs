//! In-memory runtime implementation of the shared async-operation lifecycle seam.
//!
//! Per-operation canonical lifecycle state lives in the MeerkatMachine DSL
//! authority (`op_statuses`, `op_terminal_outcomes`, `op_kinds`,
//! `op_peer_ready`, `op_progress_counts`, `active_op_count`, `wait_active`,
//! `wait_operation_ids`). This shell layer owns pure mechanics: watcher
//! channels, timestamps, peer handles, snapshot assembly, FIFO eviction
//! bookkeeping, the completion feed buffer, and concurrency-limit / duplicate
//! / peer-expectation admission checks run BEFORE the DSL apply.
//!
//! Per-transition legality ("is `CompleteOp` legal on a `Provisioning` op?")
//! is NOT owned by the shell — it lives in the DSL's `from_status_valid`
//! guards on each op-lifecycle transition. The shell's only job on a
//! `GuardRejected` rejection is to surface the pre-read status back to the
//! caller as [`OpsLifecycleError::InvalidTransition`]; see
//! [`classify_op_rejection`].

use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::task::{Context, Poll};

use meerkat_core::completion_feed::{
    CompletionBatch, CompletionEntry, CompletionFeed, CompletionSeq,
};

#[cfg(target_arch = "wasm32")]
use crate::tokio;
use meerkat_core::lifecycle::{RunId, WaitRequestId};
use meerkat_core::ops_lifecycle::{
    DEFAULT_MAX_COMPLETED, OperationCompletionWatch, OperationId, OperationKind,
    OperationLifecycleSnapshot, OperationPeerHandle, OperationProgressUpdate, OperationResult,
    OperationSpec, OperationStatus, OperationTerminalOutcome, OpsLifecycleError,
    OpsLifecycleRegistry, WaitAllResult, WaitAllSatisfied,
};
use meerkat_core::time_compat::{Instant, SystemTime, UNIX_EPOCH};

use crate::meerkat_machine::dsl as mm_dsl;

// ---------------------------------------------------------------------------
// Serde-only persisted canonical state shells
// ---------------------------------------------------------------------------
//
// These structures preserve the on-disk wire format of `PersistedOpsSnapshot`
// produced by earlier runtime versions. They are pure serde shells — no
// methods beyond read-only field accessors, no authority behavior.

/// Canonical per-operation state as captured in persisted snapshots.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OperationCanonicalState {
    status: OperationStatus,
    kind: OperationKind,
    peer_ready: bool,
    progress_count: u32,
    watcher_count: u32,
    terminal_outcome: Option<OperationTerminalOutcome>,
    terminal_buffered: bool,
}

/// Canonical registry-level state as captured in persisted snapshots.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RegistryCanonicalState {
    operations: HashMap<OperationId, OperationCanonicalState>,
    completed_order: VecDeque<OperationId>,
    max_completed: usize,
    max_concurrent: Option<usize>,
    active_count: usize,
    wait_request_id: Option<WaitRequestId>,
    wait_operation_ids: Vec<OperationId>,
    next_completion_seq: CompletionSeq,
}

impl RegistryCanonicalState {
    /// Maximum completed operations retained at capture time.
    pub fn max_completed(&self) -> usize {
        self.max_completed
    }

    /// Maximum concurrent non-terminal operations at capture time.
    pub fn max_concurrent(&self) -> Option<usize> {
        self.max_concurrent
    }

    /// Number of operations captured in the snapshot.
    pub fn operation_count(&self) -> usize {
        self.operations.len()
    }
}

// ---------------------------------------------------------------------------
// Serializable snapshot for persistence
// ---------------------------------------------------------------------------

/// Serializable snapshot of the ops lifecycle registry state.
///
/// Captured on terminal transitions for durable persistence. Contains
/// canonical state, operation specs, persisted completion feed entries, and
/// consumer cursor values. Wire format preserved verbatim from legacy
/// runtime versions for backward compatibility.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PersistedOpsSnapshot {
    /// Epoch identity at capture time.
    pub epoch_id: meerkat_core::RuntimeEpochId,
    /// Canonical machine-owned state at capture time.
    pub authority_state: RegistryCanonicalState,
    /// Per-operation specs for shell record reconstruction.
    pub operation_specs: HashMap<OperationId, meerkat_core::ops_lifecycle::OperationSpec>,
    /// Persisted completion feed entries (actual contents, not reconstructed).
    pub completion_entries: Vec<CompletionEntry>,
    /// Consumer cursor snapshot at capture time.
    pub cursors: meerkat_core::EpochCursorSnapshot,
}

// ---------------------------------------------------------------------------
// Concrete completion feed buffer
// ---------------------------------------------------------------------------

/// Shared inner state of the completion feed buffer.
///
/// Protected by the registry's `RwLock<ShellState>` for writes, and by its
/// own `RwLock` for reads by external consumers (agent boundary, idle wake).
#[derive(Debug)]
struct FeedBufferInner {
    entries: VecDeque<CompletionEntry>,
    watermark: CompletionSeq,
    max_retained: usize,
}

/// Shared completion feed buffer owned by the runtime registry.
///
/// The registry writes entries under its own write lock. External consumers
/// read through the [`RuntimeCompletionFeed`] handle.
#[derive(Debug)]
struct FeedBuffer {
    inner: RwLock<FeedBufferInner>,
    /// Atomic mirror of watermark for lock-free `watermark()` reads.
    watermark_atomic: AtomicU64,
    /// Notifies all waiters when new entries are appended.
    notify: tokio::sync::Notify,
}

impl FeedBuffer {
    fn new(max_retained: usize) -> Self {
        Self {
            inner: RwLock::new(FeedBufferInner {
                entries: VecDeque::new(),
                watermark: 0,
                max_retained,
            }),
            watermark_atomic: AtomicU64::new(0),
            notify: tokio::sync::Notify::new(),
        }
    }

    fn push(&self, entry: CompletionEntry) {
        let mut inner = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let seq = entry.seq;
        inner.entries.push_back(entry);
        inner.watermark = seq;

        // Evict oldest if over capacity.
        while inner.entries.len() > inner.max_retained {
            inner.entries.pop_front();
        }

        drop(inner);

        self.watermark_atomic.store(seq, Ordering::Release);
        self.notify.notify_waiters();
    }
}

/// Read-only handle to the runtime completion feed.
///
/// Implements [`CompletionFeed`] for external consumers. Obtained via
/// [`RuntimeOpsLifecycleRegistry::completion_feed()`].
#[derive(Debug, Clone)]
pub struct RuntimeCompletionFeed {
    buffer: Arc<FeedBuffer>,
}

impl CompletionFeed for RuntimeCompletionFeed {
    fn watermark(&self) -> CompletionSeq {
        self.buffer.watermark_atomic.load(Ordering::Acquire)
    }

    fn list_since(&self, after_seq: CompletionSeq) -> CompletionBatch {
        let inner = self
            .buffer
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entries: Vec<CompletionEntry> = inner
            .entries
            .iter()
            .filter(|e| e.seq > after_seq)
            .cloned()
            .collect();
        let watermark = inner.watermark;
        CompletionBatch { entries, watermark }
    }

    fn wait_for_advance(
        &self,
        after_seq: CompletionSeq,
    ) -> std::pin::Pin<Box<dyn Future<Output = CompletionSeq> + Send + '_>> {
        Box::pin(async move {
            loop {
                // Register the waiter BEFORE reading the watermark.
                // notify_waiters() in push() only wakes already-registered
                // listeners — if we read first and push() lands between the
                // read and notified().await, the wake is lost.
                let notified = self.buffer.notify.notified();
                let current = self.buffer.watermark_atomic.load(Ordering::Acquire);
                if current > after_seq {
                    return current;
                }
                notified.await;
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Shell-only per-operation record (not part of canonical machine state)
// ---------------------------------------------------------------------------

/// Shell-owned data for a single operation. Canonical lifecycle state lives in
/// the DSL authority; this struct holds I/O concerns that the DSL has no
/// knowledge of.
#[derive(Debug)]
struct ShellRecord {
    spec: OperationSpec,
    peer_handle: Option<OperationPeerHandle>,
    watchers: Vec<tokio::sync::oneshot::Sender<OperationTerminalOutcome>>,
    // Monotonic timestamps for elapsed computation
    created_at: Instant,
    started_at: Option<Instant>,
    completed_at: Option<Instant>,
    // Wall-clock anchor captured at creation for epoch millis
    created_at_wall: SystemTime,
}

#[derive(Debug)]
struct PendingWaitState {
    wait_request_id: WaitRequestId,
    sender: tokio::sync::oneshot::Sender<WaitAllSatisfied>,
}

impl ShellRecord {
    fn new(spec: OperationSpec) -> Self {
        Self {
            spec,
            peer_handle: None,
            watchers: Vec::new(),
            created_at: Instant::now(),
            started_at: None,
            completed_at: None,
            created_at_wall: SystemTime::now(),
        }
    }

    fn epoch_millis(wall_anchor: &SystemTime) -> u64 {
        wall_anchor
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    fn epoch_millis_for_instant(&self, instant: Instant) -> u64 {
        // Compute wall time for a given instant using the wall-clock anchor:
        // wall_time = created_at_wall + (instant - created_at)
        let offset = instant.saturating_duration_since(self.created_at);
        let wall = self.created_at_wall + offset;
        Self::epoch_millis(&wall)
    }

    /// Notify all watchers with the given terminal outcome and drain the list.
    fn notify_watchers(&mut self, outcome: &OperationTerminalOutcome) {
        for watcher in std::mem::take(&mut self.watchers) {
            let _ = watcher.send(outcome.clone());
        }
    }

    /// Mark the completion timestamp.
    fn mark_completed(&mut self) {
        self.completed_at = Some(Instant::now());
    }
}

// ---------------------------------------------------------------------------
// Combined shell state: DSL authority + shell records
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct ShellState {
    /// DSL authority — sole source of truth for per-op canonical state.
    dsl: DslAuthority,
    /// Shell-owned per-operation records (specs, watchers, timestamps, peer handles).
    records: HashMap<OperationId, ShellRecord>,
    /// Pending wait-all coordination (oneshot channel).
    pending_wait: Option<PendingWaitState>,
    /// FIFO ordering of completed operation IDs for bounded eviction.
    completed_order: VecDeque<OperationId>,
    /// Maximum completed operations to retain.
    max_completed: usize,
    /// Maximum concurrent non-terminal operations (None = unlimited).
    max_concurrent: Option<usize>,
    /// Oneshot correlation id for the currently-pending `wait_all` future.
    ///
    /// Barrier membership (`wait_operation_ids`) and activation (`wait_active`)
    /// are DSL-owned. This field is pure transport mechanics — the identity
    /// the oneshot sender is tagged with so `Drop` can correlate cancellation.
    wait_request_id: Option<WaitRequestId>,
    /// Monotonic sequence counter for completion feed entries.
    next_completion_seq: CompletionSeq,
    /// Shared detached-op wake state. When a `BackgroundToolOp` reaches terminal,
    /// sets pending and fires the Notify so the waker task can inject a
    /// continuation into the quiescent session.
    detached_wake: Option<Arc<crate::detached_wake::DetachedWakeState>>,
    /// Shared feed buffer for completion events.
    feed_buffer: Arc<FeedBuffer>,
    /// Persistence channel for durable snapshot writes (set via `set_persistence_channel`).
    persist_tx: Option<crate::tokio::sync::mpsc::Sender<PersistedOpsSnapshot>>,
    /// Epoch ID for persistence snapshots.
    persist_epoch_id: Option<meerkat_core::RuntimeEpochId>,
    /// Shared cursor state for persistence snapshots.
    persist_cursor_state: Option<Arc<meerkat_core::EpochCursorState>>,
}

/// Wrapper around the DSL authority that provides `Debug` output.
///
/// The generated `MeerkatMachineAuthority` does not derive `Debug`, but
/// `ShellState` requires it. This wrapper delegates to the inner state's
/// `Debug` impl.
struct DslAuthority(mm_dsl::MeerkatMachineAuthority);

impl std::fmt::Debug for DslAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DslAuthority")
            .field("state", &self.0.state)
            .finish()
    }
}

/// Create a DSL authority initialized with `lifecycle_phase: Idle` and all
/// ops-related fields at their defaults. Per-op transitions guard only on
/// `op_statuses.contains_key(operation_id)`, so the phase stays in `Idle`
/// permanently (they all `to Idle`).
fn new_ops_dsl_authority() -> DslAuthority {
    let state = mm_dsl::MeerkatMachineState {
        lifecycle_phase: mm_dsl::MeerkatPhase::Idle,
        ..mm_dsl::MeerkatMachineState::default()
    };
    DslAuthority(mm_dsl::MeerkatMachineAuthority::from_state(state))
}

impl ShellState {
    fn new(max_completed: usize, max_concurrent: Option<usize>) -> Self {
        Self {
            dsl: new_ops_dsl_authority(),
            records: HashMap::new(),
            pending_wait: None,
            completed_order: VecDeque::new(),
            max_completed,
            max_concurrent,
            wait_request_id: None,
            next_completion_seq: 0,
            detached_wake: None,
            // Feed buffer is larger than max_completed to absorb bursts.
            // Entries are only evicted by buffer capacity, not by consumer cursor,
            // so the buffer must be large enough that consumers drain before
            // the oldest entry is evicted.
            feed_buffer: Arc::new(FeedBuffer::new(max_completed.saturating_mul(4).max(1024))),
            persist_tx: None,
            persist_epoch_id: None,
            persist_cursor_state: None,
        }
    }

    /// Apply a DSL input, mapping transition errors into
    /// [`OpsLifecycleError::Internal`]. Callers that need to distinguish
    /// guard rejections (legal-transition violations) from internal desync
    /// should use [`Self::dsl_apply_raw`] and classify the error themselves;
    /// this helper is for DSL inputs whose preconditions the caller has
    /// already fully validated (e.g., `RequestWaitAll`, `SatisfyWaitAll`).
    fn dsl_apply(
        &mut self,
        input: mm_dsl::MeerkatMachineInput,
        context: &str,
    ) -> Result<(), OpsLifecycleError> {
        self.dsl_apply_raw(input).map_err(|err| {
            OpsLifecycleError::Internal(format!("DSL rejected ops transition ({context}): {err:?}"))
        })
    }

    /// Apply a DSL input, returning the raw kernel-level rejection so callers
    /// can distinguish `GuardRejected` (a legitimate legality violation, e.g.,
    /// `complete_operation` on a `Provisioning` op) from
    /// `NoMatchingTransition` (a shell/DSL desync). Every op-lifecycle entry
    /// point (complete/fail/abort/cancel/start/retire/terminate/progress)
    /// uses this form and synthesises [`OpsLifecycleError::InvalidTransition`]
    /// on `GuardRejected`.
    fn dsl_apply_raw(
        &mut self,
        input: mm_dsl::MeerkatMachineInput,
    ) -> Result<(), mm_dsl::MeerkatMachineTransitionError> {
        mm_dsl::MeerkatMachineMutator::apply(&mut self.dsl.0, input).map(|_transition| ())
    }

    /// Split a domain terminal outcome into a `(discriminant, payload)` pair
    /// suitable for the DSL's typed `op_terminal_outcomes` +
    /// `op_terminal_payload` fields.
    ///
    /// The discriminant is a typed DSL enum mirror; the payload is the JSON
    /// encoding of the outcome's *inner* payload (e.g., the `OperationResult`
    /// for `Completed`, the error string for `Failed`, the optional reason
    /// for `Aborted` / `Cancelled`, the reason string for `Terminated`). For
    /// `Retired` the payload is empty — it carries no data.
    ///
    /// The shell rehydrates the typed domain outcome via
    /// [`Self::terminal_outcome`], pairing the DSL discriminant with the
    /// companion payload entry.
    fn split_outcome(
        outcome: &OperationTerminalOutcome,
    ) -> (mm_dsl::OperationTerminalOutcomeKind, String) {
        match outcome {
            OperationTerminalOutcome::Completed(result) => (
                mm_dsl::OperationTerminalOutcomeKind::Completed,
                serde_json::to_string(result).unwrap_or_default(),
            ),
            OperationTerminalOutcome::Failed { error } => (
                mm_dsl::OperationTerminalOutcomeKind::Failed,
                serde_json::to_string(error).unwrap_or_default(),
            ),
            OperationTerminalOutcome::Aborted { reason } => (
                mm_dsl::OperationTerminalOutcomeKind::Aborted,
                serde_json::to_string(reason).unwrap_or_default(),
            ),
            OperationTerminalOutcome::Cancelled { reason } => (
                mm_dsl::OperationTerminalOutcomeKind::Cancelled,
                serde_json::to_string(reason).unwrap_or_default(),
            ),
            OperationTerminalOutcome::Retired => {
                (mm_dsl::OperationTerminalOutcomeKind::Retired, String::new())
            }
            OperationTerminalOutcome::Terminated { reason } => (
                mm_dsl::OperationTerminalOutcomeKind::Terminated,
                serde_json::to_string(reason).unwrap_or_default(),
            ),
        }
    }

    /// Read the DSL operation status for `id`, or `None` if not registered.
    fn status(&self, id: &OperationId) -> Option<OperationStatus> {
        let id_key = mm_dsl::OperationId::from_domain(id).0;
        self.dsl
            .0
            .state
            .op_statuses
            .get(&id_key)
            .copied()
            .map(OperationStatus::from)
    }

    /// Read the DSL operation kind for `id`, or `None` if not registered.
    fn kind(&self, id: &OperationId) -> Option<OperationKind> {
        let id_key = mm_dsl::OperationId::from_domain(id).0;
        self.dsl
            .0
            .state
            .op_kinds
            .get(&id_key)
            .copied()
            .map(OperationKind::from)
    }

    /// Read the peer-ready flag for `id`.
    fn peer_ready(&self, id: &OperationId) -> Option<bool> {
        let id_key = mm_dsl::OperationId::from_domain(id).0;
        self.dsl.0.state.op_peer_ready.get(&id_key).copied()
    }

    /// Read the progress counter for `id`.
    fn progress_count(&self, id: &OperationId) -> Option<u32> {
        let id_key = mm_dsl::OperationId::from_domain(id).0;
        self.dsl
            .0
            .state
            .op_progress_counts
            .get(&id_key)
            .map(|v| (*v).min(u32::MAX as u64) as u32)
    }

    /// Read the terminal outcome for `id` by pairing the DSL's typed
    /// discriminant with the companion payload JSON. Returns `None` when the
    /// op has no recorded terminal discriminant.
    fn terminal_outcome(&self, id: &OperationId) -> Option<OperationTerminalOutcome> {
        let id_key = mm_dsl::OperationId::from_domain(id).0;
        let kind = self
            .dsl
            .0
            .state
            .op_terminal_outcomes
            .get(&id_key)
            .copied()?;
        let payload = self
            .dsl
            .0
            .state
            .op_terminal_payload
            .get(&id_key)
            .map(String::as_str)
            .unwrap_or("");
        match kind {
            mm_dsl::OperationTerminalOutcomeKind::Completed => {
                let result = serde_json::from_str::<OperationResult>(payload).ok()?;
                Some(OperationTerminalOutcome::Completed(result))
            }
            mm_dsl::OperationTerminalOutcomeKind::Failed => {
                let error = serde_json::from_str::<String>(payload).unwrap_or_default();
                Some(OperationTerminalOutcome::Failed { error })
            }
            mm_dsl::OperationTerminalOutcomeKind::Aborted => {
                let reason = serde_json::from_str::<Option<String>>(payload)
                    .ok()
                    .flatten();
                Some(OperationTerminalOutcome::Aborted { reason })
            }
            mm_dsl::OperationTerminalOutcomeKind::Cancelled => {
                let reason = serde_json::from_str::<Option<String>>(payload)
                    .ok()
                    .flatten();
                Some(OperationTerminalOutcome::Cancelled { reason })
            }
            mm_dsl::OperationTerminalOutcomeKind::Retired => {
                Some(OperationTerminalOutcome::Retired)
            }
            mm_dsl::OperationTerminalOutcomeKind::Terminated => {
                let reason = serde_json::from_str::<String>(payload).unwrap_or_default();
                Some(OperationTerminalOutcome::Terminated { reason })
            }
        }
    }

    /// Whether the operation is currently tracked in DSL state.
    fn contains(&self, id: &OperationId) -> bool {
        let id_key = mm_dsl::OperationId::from_domain(id).0;
        self.dsl.0.state.op_statuses.contains_key(&id_key)
    }

    /// Number of non-terminal operations (derived from DSL state).
    fn active_count(&self) -> usize {
        self.dsl.0.state.active_op_count as usize
    }

    /// Number of operations currently tracked (including terminal).
    fn operation_count(&self) -> usize {
        self.dsl.0.state.op_statuses.len()
    }

    /// Iterate over all tracked operation IDs (DSL keys converted to domain).
    fn operation_ids(&self) -> Vec<OperationId> {
        self.dsl
            .0
            .state
            .op_statuses
            .keys()
            .filter_map(|k| serde_json::from_str::<OperationId>(k).ok())
            .collect()
    }

    /// Assign and return the next completion sequence number.
    fn next_seq(&mut self) -> CompletionSeq {
        self.next_completion_seq = self.next_completion_seq.saturating_add(1);
        self.next_completion_seq
    }

    /// Build a snapshot from DSL state + shell record.
    fn snapshot(&self, id: &OperationId) -> Option<OperationLifecycleSnapshot> {
        let shell = self.records.get(id)?;
        let kind = self.kind(id)?;
        let status = self.status(id)?;
        let peer_ready = self.peer_ready(id).unwrap_or(false);
        let progress_count = self.progress_count(id).unwrap_or(0);
        let terminal_outcome = self.terminal_outcome(id);

        let created_at_ms = ShellRecord::epoch_millis(&shell.created_at_wall);
        let started_at_ms = shell.started_at.map(|i| shell.epoch_millis_for_instant(i));
        let completed_at_ms = shell
            .completed_at
            .map(|i| shell.epoch_millis_for_instant(i));
        let elapsed_ms = shell.completed_at.map(|completed| {
            completed
                .saturating_duration_since(shell.created_at)
                .as_millis() as u64
        });

        Some(OperationLifecycleSnapshot {
            id: shell.spec.id.clone(),
            kind,
            display_name: shell.spec.display_name.clone(),
            status,
            peer_ready,
            progress_count,
            watcher_count: shell.watchers.len() as u32,
            terminal_outcome,
            child_session_id: shell.spec.child_session_id.clone(),
            peer_handle: shell.peer_handle.clone(),
            created_at_ms,
            started_at_ms,
            completed_at_ms,
            elapsed_ms,
        })
    }

    /// Emit shell-side mechanics for a terminal transition: notify watchers,
    /// push CompletionEntry, arm detached wake, retain in FIFO, evict as
    /// needed. Called AFTER the DSL transition has already persisted the
    /// terminal status + outcome.
    fn finalize_terminal(&mut self, id: &OperationId) {
        let outcome = match self.terminal_outcome(id) {
            Some(o) => o,
            None => return,
        };
        let kind = self.kind(id);

        // Notify watchers and mark completion timestamp.
        if let Some(shell) = self.records.get_mut(id) {
            shell.notify_watchers(&outcome);
            shell.mark_completed();
        }

        // Arm detached wake for BackgroundToolOp terminals.
        if let (Some(k), Some(wake)) = (kind, self.detached_wake.as_ref())
            && k == OperationKind::BackgroundToolOp
        {
            wake.pending.store(true, Ordering::Release);
            wake.notify.notify_one();
        }

        // Push completion feed entry.
        let seq = self.next_seq();
        let display_name = self
            .records
            .get(id)
            .map(|r| r.spec.display_name.clone())
            .unwrap_or_default();
        let completed_at_ms = self
            .records
            .get(id)
            .and_then(|r| r.completed_at.map(|i| r.epoch_millis_for_instant(i)));
        let kind_for_entry = kind.unwrap_or(OperationKind::BackgroundToolOp);
        self.feed_buffer.push(CompletionEntry {
            seq,
            operation_id: id.clone(),
            kind: kind_for_entry,
            display_name,
            terminal_outcome: outcome,
            completed_at_ms,
        });

        // FIFO retention + eviction.
        self.completed_order.push_back(id.clone());
        while self.completed_order.len() > self.max_completed {
            if let Some(evicted) = self.completed_order.pop_front() {
                let evicted_key = mm_dsl::OperationId::from_domain(&evicted).0;
                self.dsl.0.state.op_statuses.remove(&evicted_key);
                self.dsl.0.state.op_kinds.remove(&evicted_key);
                self.dsl.0.state.op_peer_ready.remove(&evicted_key);
                self.dsl.0.state.op_progress_counts.remove(&evicted_key);
                self.dsl.0.state.op_terminal_outcomes.remove(&evicted_key);
                self.dsl.0.state.op_terminal_payload.remove(&evicted_key);
                self.dsl.0.state.op_completion_seq.remove(&evicted_key);
                self.records.remove(&evicted);
            }
        }

        // Satisfy a pending wait request if all its ops are now terminal.
        self.maybe_satisfy_wait();
    }

    /// Read barrier membership from DSL state (sole owner).
    fn wait_operation_ids(&self) -> Vec<OperationId> {
        self.dsl
            .0
            .state
            .wait_operation_ids
            .iter()
            .filter_map(|k| serde_json::from_str::<OperationId>(k).ok())
            .collect()
    }

    /// Whether the DSL has a barrier wait active.
    fn wait_active(&self) -> bool {
        self.dsl.0.state.wait_active
    }

    /// Check whether a pending barrier wait is now satisfied and resolve it.
    ///
    /// Barrier membership and the "all members terminal" decision both live
    /// in the DSL: `wait_operation_ids` carries the set, `wait_active`
    /// signals a pending barrier, and `SatisfyWaitAll`'s
    /// `all_members_terminal` guard owns the fixed-point test. The shell
    /// fires `SatisfyWaitAll` idempotently on every terminal transition and
    /// lets the DSL guard reject early firings as a no-op. On acceptance
    /// (transition returns `Ok`), the shell selects the correlated oneshot
    /// and delivers the `WaitAllSatisfied` obligation token.
    ///
    /// `wait_request_id` is the shell-owned oneshot correlation id that
    /// selects which sender to notify; when the DSL barrier satisfies
    /// without a live correlation (post-recovery, or duplicate resolution),
    /// the oneshot simply remains pending.
    fn maybe_satisfy_wait(&mut self) {
        // Capture membership *before* applying — `SatisfyWaitAll`'s update
        // clause resets `wait_operation_ids` to empty, so a post-apply read
        // would lose the member list carried on the obligation token.
        let ids = self.wait_operation_ids();
        if self
            .dsl_apply_raw(mm_dsl::MeerkatMachineInput::SatisfyWaitAll)
            .is_err()
        {
            // Guard rejection (barrier inactive, or members not all terminal
            // yet) is an expected idempotent no-op on the satisfaction fixed
            // point, not a shell/DSL desync. Swallow.
            return;
        }
        let wait_id = match self.wait_request_id.take() {
            Some(id) => id,
            None => return,
        };
        if let Some(pending) = self.pending_wait.take() {
            if pending.wait_request_id == wait_id {
                let _ = pending.sender.send(WaitAllSatisfied {
                    wait_request_id: wait_id,
                    operation_ids: ids,
                });
            } else {
                self.pending_wait = Some(pending);
            }
        }
    }

    /// Queue a persistence snapshot if a persistence channel is wired.
    ///
    /// Called after terminal transitions. Captures authority + entries + cursors
    /// under the write lock (caller already holds it) and queues to the channel.
    fn maybe_persist(&self) {
        let (tx, epoch_id, cursor_state) = match (
            &self.persist_tx,
            &self.persist_epoch_id,
            &self.persist_cursor_state,
        ) {
            (Some(tx), Some(epoch_id), Some(cs)) => (tx, epoch_id, cs),
            _ => return,
        };

        let snapshot = self.capture_snapshot(epoch_id.clone(), cursor_state);

        // Non-blocking send — bounded-loss is the acknowledged contract.
        if tx.try_send(snapshot).is_err() {
            tracing::warn!("ops lifecycle persistence channel full or closed; snapshot dropped");
        }
    }

    /// Capture the full persisted snapshot for the current state.
    fn capture_snapshot(
        &self,
        epoch_id: meerkat_core::RuntimeEpochId,
        cursor_state: &meerkat_core::EpochCursorState,
    ) -> PersistedOpsSnapshot {
        let operation_specs: HashMap<OperationId, OperationSpec> = self
            .records
            .iter()
            .map(|(id, record)| (id.clone(), record.spec.clone()))
            .collect();

        let completion_entries = {
            let inner = self
                .feed_buffer
                .inner
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            inner.entries.iter().cloned().collect()
        };

        let mut operations: HashMap<OperationId, OperationCanonicalState> = HashMap::new();
        for op_id in self.operation_ids() {
            let Some(status) = self.status(&op_id) else {
                continue;
            };
            let Some(kind) = self.kind(&op_id) else {
                continue;
            };
            let peer_ready = self.peer_ready(&op_id).unwrap_or(false);
            let progress_count = self.progress_count(&op_id).unwrap_or(0);
            let terminal_outcome = self.terminal_outcome(&op_id);
            let terminal_buffered = terminal_outcome.is_some();
            let watcher_count = self
                .records
                .get(&op_id)
                .map(|r| r.watchers.len() as u32)
                .unwrap_or(0);
            operations.insert(
                op_id,
                OperationCanonicalState {
                    status,
                    kind,
                    peer_ready,
                    progress_count,
                    watcher_count,
                    terminal_outcome,
                    terminal_buffered,
                },
            );
        }

        let authority_state = RegistryCanonicalState {
            operations,
            completed_order: self.completed_order.clone(),
            max_completed: self.max_completed,
            max_concurrent: self.max_concurrent,
            active_count: self.active_count(),
            wait_request_id: self.wait_request_id.clone(),
            wait_operation_ids: self.wait_operation_ids(),
            next_completion_seq: self.next_completion_seq,
        };

        PersistedOpsSnapshot {
            epoch_id,
            authority_state,
            operation_specs,
            completion_entries,
            cursors: cursor_state.snapshot(),
        }
    }

    fn shell_record_mut(
        &mut self,
        id: &OperationId,
    ) -> Result<&mut ShellRecord, OpsLifecycleError> {
        self.records
            .get_mut(id)
            .ok_or_else(|| OpsLifecycleError::NotFound(id.clone()))
    }

    fn collect_wait_outcomes(
        &self,
        operation_ids: &[OperationId],
    ) -> Result<Vec<(OperationId, OperationTerminalOutcome)>, OpsLifecycleError> {
        operation_ids
            .iter()
            .map(|operation_id| {
                let outcome = self.terminal_outcome(operation_id).ok_or_else(|| {
                    OpsLifecycleError::Internal(format!(
                        "wait_all completed without terminal outcome for {operation_id}"
                    ))
                })?;
                Ok((operation_id.clone(), outcome))
            })
            .collect()
    }
}

impl Default for ShellState {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_COMPLETED, None)
    }
}

// ---------------------------------------------------------------------------
// Public configuration & registry
// ---------------------------------------------------------------------------

/// Configuration for [`RuntimeOpsLifecycleRegistry`].
#[derive(Debug, Clone)]
pub struct OpsLifecycleConfig {
    /// Maximum number of completed operations to retain (default: 256).
    pub max_completed: usize,
    /// Maximum concurrent non-terminal operations (None = unlimited).
    pub max_concurrent: Option<usize>,
}

impl Default for OpsLifecycleConfig {
    fn default() -> Self {
        Self {
            max_completed: DEFAULT_MAX_COMPLETED,
            max_concurrent: None,
        }
    }
}

/// Per-runtime shared registry for async operation lifecycle truth.
///
/// Per-operation canonical lifecycle state is owned by the DSL authority
/// embedded in the shell. This struct manages I/O concerns: watcher
/// channels, timestamps, peer handles, snapshot assembly, FIFO eviction,
/// and the completion feed buffer.
#[derive(Debug)]
pub struct RuntimeOpsLifecycleRegistry {
    state: RwLock<ShellState>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct RuntimeOpsDiagnosticSnapshot {
    pub operation_count: usize,
    pub active_count: usize,
    pub wait_request_id: Option<WaitRequestId>,
    pub pending_wait_present: bool,
    pub pending_wait_request_id: Option<WaitRequestId>,
    pub wait_operation_ids: Vec<OperationId>,
    pub operations: Vec<OperationLifecycleSnapshot>,
}

impl Default for RuntimeOpsLifecycleRegistry {
    fn default() -> Self {
        Self {
            state: RwLock::new(ShellState::default()),
        }
    }
}

impl RuntimeOpsLifecycleRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_config(config: OpsLifecycleConfig) -> Self {
        Self {
            state: RwLock::new(ShellState::new(config.max_completed, config.max_concurrent)),
        }
    }

    /// Wire a persistence channel for durable snapshot writes.
    ///
    /// After this call, terminal transitions (complete/fail/cancel/abort)
    /// capture a snapshot and queue it to the channel. A dedicated
    /// persistence task should drain the channel and write to the store.
    pub fn set_persistence_channel(
        &self,
        tx: crate::tokio::sync::mpsc::Sender<PersistedOpsSnapshot>,
        epoch_id: meerkat_core::RuntimeEpochId,
        cursor_state: Arc<meerkat_core::EpochCursorState>,
    ) {
        if let Ok(mut state) = self.state.write() {
            state.persist_tx = Some(tx);
            state.persist_epoch_id = Some(epoch_id);
            state.persist_cursor_state = Some(cursor_state);
        }
    }

    /// Wire the detached-wake state so that finalize_terminal arms pending
    /// and fires the Notify when a `BackgroundToolOp` reaches terminal.
    pub fn set_detached_wake(&self, wake: Arc<crate::detached_wake::DetachedWakeState>) {
        if let Ok(mut state) = self.state.write() {
            state.detached_wake = Some(wake);
        }
    }

    /// Recover from a persisted snapshot.
    ///
    /// Rebuilds DSL state (stripping non-terminal ops — only terminals
    /// survive recovery), creates fresh shell records from specs, and seeds
    /// the feed buffer with persisted completion entries.
    pub fn from_recovered(snapshot: PersistedOpsSnapshot) -> Self {
        let mut shell = ShellState::new(
            snapshot.authority_state.max_completed,
            snapshot.authority_state.max_concurrent,
        );

        // Only retain terminal operations in the DSL state.
        let mut retained_ids: HashSet<OperationId> = HashSet::new();
        for (op_id, op_state) in snapshot.authority_state.operations {
            if !op_state.status.is_terminal() {
                continue;
            }
            let id_key = mm_dsl::OperationId::from_domain(&op_id).0;
            shell.dsl.0.state.op_statuses.insert(
                id_key.clone(),
                mm_dsl::OperationStatus::from(op_state.status),
            );
            shell
                .dsl
                .0
                .state
                .op_kinds
                .insert(id_key.clone(), mm_dsl::OperationKind::from(op_state.kind));
            shell
                .dsl
                .0
                .state
                .op_peer_ready
                .insert(id_key.clone(), op_state.peer_ready);
            shell
                .dsl
                .0
                .state
                .op_progress_counts
                .insert(id_key.clone(), op_state.progress_count as u64);
            if let Some(outcome) = op_state.terminal_outcome.as_ref() {
                let (kind, payload) = ShellState::split_outcome(outcome);
                shell
                    .dsl
                    .0
                    .state
                    .op_terminal_outcomes
                    .insert(id_key.clone(), kind);
                shell
                    .dsl
                    .0
                    .state
                    .op_terminal_payload
                    .insert(id_key.clone(), payload);
            }
            retained_ids.insert(op_id);
        }
        // active_count is 0 — all retained ops are terminal.
        shell.dsl.0.state.active_op_count = 0;

        // Rebuild completed_order keeping only retained ops.
        shell.completed_order = snapshot
            .authority_state
            .completed_order
            .into_iter()
            .filter(|id| retained_ids.contains(id))
            .collect();

        // Re-seed completion sequence counter so new terminals keep
        // monotonic order relative to persisted entries.
        shell.next_completion_seq = snapshot.authority_state.next_completion_seq;

        // Seed the feed buffer from persisted entries.
        for entry in &snapshot.completion_entries {
            shell.feed_buffer.push(entry.clone());
        }

        // Rebuild shell records from specs (fresh timestamps, no watchers)
        // — only for operations still retained in the DSL state.
        for (op_id, spec) in snapshot.operation_specs {
            if retained_ids.contains(&op_id) {
                shell.records.insert(
                    op_id,
                    ShellRecord {
                        spec,
                        peer_handle: None,
                        watchers: Vec::new(),
                        created_at: Instant::now(),
                        started_at: None,
                        completed_at: None,
                        created_at_wall: SystemTime::now(),
                    },
                );
            }
        }

        Self {
            state: RwLock::new(shell),
        }
    }

    /// Capture a serializable snapshot of the current state for persistence.
    ///
    /// Includes authority state, operation specs, completion entries, and
    /// cursor values. Cursor values may be stale relative to the agent's
    /// true position (monotonic staleness, not atomicity).
    pub fn capture_persistence_snapshot(
        &self,
        epoch_id: meerkat_core::RuntimeEpochId,
        cursor_state: &meerkat_core::EpochCursorState,
    ) -> PersistedOpsSnapshot {
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.capture_snapshot(epoch_id, cursor_state)
    }

    /// Return a read handle to the completion feed.
    pub fn completion_feed_handle(&self) -> Arc<dyn CompletionFeed> {
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::new(RuntimeCompletionFeed {
            buffer: Arc::clone(&state.feed_buffer),
        })
    }

    /// Capture a stable diagnostic snapshot of the canonical ops lifecycle state.
    ///
    /// This is an internal refactor aid for the MeerkatMachine build-out. It is
    /// intentionally additive and does not change lifecycle semantics.
    #[allow(dead_code)]
    pub(crate) fn diagnostic_snapshot(&self) -> RuntimeOpsDiagnosticSnapshot {
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut operations = state
            .operation_ids()
            .into_iter()
            .filter_map(|id| state.snapshot(&id))
            .collect::<Vec<_>>();
        operations.sort_by(|left, right| left.display_name.cmp(&right.display_name));
        RuntimeOpsDiagnosticSnapshot {
            operation_count: state.operation_count(),
            active_count: state.active_count(),
            wait_request_id: state.wait_request_id.clone(),
            pending_wait_present: state.pending_wait.is_some(),
            pending_wait_request_id: state
                .pending_wait
                .as_ref()
                .map(|pending_wait| pending_wait.wait_request_id.clone()),
            wait_operation_ids: state.wait_operation_ids(),
            operations,
        }
    }

    fn read_state(&self) -> Result<RwLockReadGuard<'_, ShellState>, OpsLifecycleError> {
        self.state
            .read()
            .map_err(|_| OpsLifecycleError::Internal("ops lifecycle registry poisoned".into()))
    }

    fn write_state(&self) -> Result<RwLockWriteGuard<'_, ShellState>, OpsLifecycleError> {
        self.state
            .write()
            .map_err(|_| OpsLifecycleError::Internal("ops lifecycle registry poisoned".into()))
    }

    fn cancel_wait_all_internal(
        &self,
        wait_request_id: &WaitRequestId,
    ) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;
        match state.wait_request_id.as_ref() {
            Some(active) if active == wait_request_id => {
                state.wait_request_id = None;
                state.pending_wait = None;
                // Clear the DSL barrier via the dedicated `CancelWaitAll`
                // transition. Unlike `SatisfyWaitAll`, it does not require
                // every member to be terminal (the request was dropped, not
                // resolved) and does not emit the `WaitAllSatisfied`
                // obligation. The `wait_is_active` guard keeps it an
                // idempotent no-op if the barrier was already cleared.
                let _ = state.dsl_apply(
                    mm_dsl::MeerkatMachineInput::CancelWaitAll,
                    "CancelWaitAll(cancel)",
                );
                Ok(())
            }
            _ => {
                state.pending_wait = None;
                Ok(())
            }
        }
    }
}

enum WaitAllFutureState {
    Ready(Option<Result<WaitAllResult, OpsLifecycleError>>),
    Waiting(tokio::sync::oneshot::Receiver<WaitAllSatisfied>),
    Done,
}

struct WaitAllFuture<'a> {
    registry: &'a RuntimeOpsLifecycleRegistry,
    wait_request_id: WaitRequestId,
    operation_ids: Vec<OperationId>,
    state: WaitAllFutureState,
}

impl Future for WaitAllFuture<'_> {
    type Output = Result<WaitAllResult, OpsLifecycleError>;

    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match &mut self.state {
            WaitAllFutureState::Ready(result) => {
                let ready = result.take().unwrap_or_else(|| {
                    Err(OpsLifecycleError::Internal(
                        "wait_all future polled after completion".into(),
                    ))
                });
                self.state = WaitAllFutureState::Done;
                Poll::Ready(ready)
            }
            WaitAllFutureState::Waiting(receiver) => match std::pin::Pin::new(receiver).poll(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(Ok(satisfied)) => {
                    let outcomes = match self.registry.read_state() {
                        Ok(state) => state.collect_wait_outcomes(&self.operation_ids),
                        Err(err) => Err(err),
                    };
                    self.state = WaitAllFutureState::Done;
                    Poll::Ready(outcomes.map(|outcomes| WaitAllResult {
                        outcomes,
                        satisfied,
                    }))
                }
                Poll::Ready(Err(_)) => {
                    self.state = WaitAllFutureState::Done;
                    Poll::Ready(Err(OpsLifecycleError::Internal(
                        "wait_all completion channel dropped".into(),
                    )))
                }
            },
            WaitAllFutureState::Done => Poll::Ready(Err(OpsLifecycleError::Internal(
                "wait_all future polled after completion".into(),
            ))),
        }
    }
}

impl Drop for WaitAllFuture<'_> {
    fn drop(&mut self) {
        if matches!(self.state, WaitAllFutureState::Waiting(_)) {
            let _ = self
                .registry
                .cancel_wait_all_internal(&self.wait_request_id);
        }
    }
}

// ---------------------------------------------------------------------------
// Shell → DSL error classification
// ---------------------------------------------------------------------------
//
// Transition legality (which "from" statuses each op-lifecycle transition is
// legal from) is owned by the MeerkatMachine DSL's `from_status_valid` guards.
// The shell's only job when a DSL transition is rejected is to surface the
// pre-read status back to the caller as [`OpsLifecycleError::InvalidTransition`].
//
// Each op-lifecycle entry point follows the same shape:
//   1. Pre-read `status(id)` under the write lock. `None` → `NotFound`.
//   2. Fire the DSL input via `dsl_apply_raw`.
//   3. On `GuardRejected`, synthesise `InvalidTransition { status, action }`.
//   4. On `NoMatchingTransition`, surface as `Internal` (genuine desync).
//
// The `action` label is a short, human-readable name of the shell entry point
// — matches the names formerly passed to the deleted `require_status`.

/// Classify a kernel rejection from an op-lifecycle DSL apply.
///
/// `GuardRejected` → `InvalidTransition { id, status, action }`. The pre-read
/// `status` and the DSL's guard observation come from the same canonical map
/// under a single write lock, so they cannot diverge.
/// `NoMatchingTransition` → `Internal` (genuine shell/DSL desync).
fn classify_op_rejection(
    err: mm_dsl::MeerkatMachineTransitionError,
    id: &OperationId,
    status: OperationStatus,
    action: &'static str,
) -> OpsLifecycleError {
    match err {
        mm_dsl::MeerkatMachineTransitionError::GuardRejected { .. } => {
            OpsLifecycleError::InvalidTransition {
                id: id.clone(),
                status,
                action,
            }
        }
        other => OpsLifecycleError::Internal(format!(
            "DSL rejected ops transition ({action}): {other:?}"
        )),
    }
}

/// Classify a `PeerReadyOp` DSL rejection back into the public error surface.
///
/// `PeerReadyOp`'s DSL guards layer three distinct rejections onto a single
/// `GuardRejected` variant. The shell distinguishes them by re-reading the
/// canonical DSL state under the same write lock that observed the guard
/// failure — so the classification cannot race with a concurrent mutation.
///
/// Priority mirrors the declared guard order in the DSL transition:
/// 1. `kind_is_mob_member_child` → `PeerNotExpected`
/// 2. `not_already_peer_ready`   → `AlreadyPeerReady`
/// 3. `from_status_valid`        → `InvalidTransition`
fn classify_peer_ready_rejection(
    state: &ShellState,
    err: mm_dsl::MeerkatMachineTransitionError,
    id: &OperationId,
    status: OperationStatus,
) -> OpsLifecycleError {
    match err {
        mm_dsl::MeerkatMachineTransitionError::GuardRejected { .. } => {
            let kind = state.kind(id);
            if kind != Some(OperationKind::MobMemberChild) {
                return OpsLifecycleError::PeerNotExpected(id.clone());
            }
            if state.peer_ready(id).unwrap_or(false) {
                return OpsLifecycleError::AlreadyPeerReady(id.clone());
            }
            OpsLifecycleError::InvalidTransition {
                id: id.clone(),
                status,
                action: "peer_ready",
            }
        }
        other => OpsLifecycleError::Internal(format!(
            "DSL rejected ops transition (peer_ready): {other:?}"
        )),
    }
}

impl OpsLifecycleRegistry for RuntimeOpsLifecycleRegistry {
    fn register_operation(&self, spec: OperationSpec) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;
        let operation_id = spec.id.clone();
        let kind = spec.kind;

        // Pre-check: duplicate.
        if state.contains(&operation_id) {
            return Err(OpsLifecycleError::AlreadyRegistered(operation_id));
        }
        // Pre-check: concurrency limit.
        if let Some(limit) = state.max_concurrent
            && state.active_count() >= limit
        {
            return Err(OpsLifecycleError::MaxConcurrentExceeded {
                limit,
                active: state.active_count(),
            });
        }

        // DSL apply.
        state.dsl_apply(
            mm_dsl::MeerkatMachineInput::RegisterOp {
                operation_id: mm_dsl::OperationId::from_domain(&operation_id).0,
                kind: mm_dsl::OperationKind::from_domain(&kind),
            },
            "RegisterOp",
        )?;

        // Insert shell record.
        state.records.insert(operation_id, ShellRecord::new(spec));
        Ok(())
    }

    fn provisioning_succeeded(&self, id: &OperationId) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let status = state
            .status(id)
            .ok_or_else(|| OpsLifecycleError::NotFound(id.clone()))?;

        if let Err(err) = state.dsl_apply_raw(mm_dsl::MeerkatMachineInput::StartOp {
            operation_id: mm_dsl::OperationId::from_domain(id).0,
        }) {
            return Err(classify_op_rejection(
                err,
                id,
                status,
                "provisioning_succeeded",
            ));
        }

        // Shell concern: record the started timestamp.
        if let Some(shell) = state.records.get_mut(id) {
            shell.started_at = Some(Instant::now());
        }
        Ok(())
    }

    fn provisioning_failed(
        &self,
        id: &OperationId,
        error: String,
    ) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let status = state
            .status(id)
            .ok_or_else(|| OpsLifecycleError::NotFound(id.clone()))?;

        let terminal_outcome = OperationTerminalOutcome::Failed { error };
        let (outcome_kind, outcome_payload) = ShellState::split_outcome(&terminal_outcome);

        if let Err(err) = state.dsl_apply_raw(mm_dsl::MeerkatMachineInput::FailOp {
            operation_id: mm_dsl::OperationId::from_domain(id).0,
            outcome: outcome_kind,
            payload: outcome_payload,
        }) {
            return Err(classify_op_rejection(err, id, status, "fail_operation"));
        }

        state.finalize_terminal(id);
        state.maybe_persist();
        Ok(())
    }

    fn peer_ready(
        &self,
        id: &OperationId,
        peer: OperationPeerHandle,
    ) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let status = state
            .status(id)
            .ok_or_else(|| OpsLifecycleError::NotFound(id.clone()))?;

        // The kind / not-already-peer-ready / status decisions all live in
        // the DSL's `PeerReadyOp` guards. The shell fires unconditionally
        // and classifies a guard rejection by re-reading the state under
        // the same write lock (so no race with the DSL's guard evaluation).
        if let Err(err) = state.dsl_apply_raw(mm_dsl::MeerkatMachineInput::PeerReadyOp {
            operation_id: mm_dsl::OperationId::from_domain(id).0,
        }) {
            return Err(classify_peer_ready_rejection(&state, err, id, status));
        }

        // Shell concern: store the peer handle.
        if let Some(shell) = state.records.get_mut(id) {
            shell.peer_handle = Some(peer);
        }
        Ok(())
    }

    fn register_watcher(
        &self,
        id: &OperationId,
    ) -> Result<OperationCompletionWatch, OpsLifecycleError> {
        let mut state = self.write_state()?;

        if !state.contains(id) {
            return Err(OpsLifecycleError::NotFound(id.clone()));
        }

        // If already terminal, return an already-resolved watch.
        if let Some(outcome) = state.terminal_outcome(id) {
            return Ok(OperationCompletionWatch::already_resolved(outcome));
        }

        // Shell concern: create the channel and store the sender.
        let shell = state.shell_record_mut(id)?;
        let (tx, watch) = OperationCompletionWatch::channel();
        shell.watchers.push(tx);
        Ok(watch)
    }

    fn report_progress(
        &self,
        id: &OperationId,
        _update: OperationProgressUpdate,
    ) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let status = state
            .status(id)
            .ok_or_else(|| OpsLifecycleError::NotFound(id.clone()))?;

        if let Err(err) = state.dsl_apply_raw(mm_dsl::MeerkatMachineInput::ProgressReportedOp {
            operation_id: mm_dsl::OperationId::from_domain(id).0,
        }) {
            return Err(classify_op_rejection(err, id, status, "report_progress"));
        }
        Ok(())
    }

    fn complete_operation(
        &self,
        id: &OperationId,
        result: OperationResult,
    ) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let status = state
            .status(id)
            .ok_or_else(|| OpsLifecycleError::NotFound(id.clone()))?;

        let terminal_outcome = OperationTerminalOutcome::Completed(result);
        let (outcome_kind, outcome_payload) = ShellState::split_outcome(&terminal_outcome);

        if let Err(err) = state.dsl_apply_raw(mm_dsl::MeerkatMachineInput::CompleteOp {
            operation_id: mm_dsl::OperationId::from_domain(id).0,
            outcome: outcome_kind,
            payload: outcome_payload,
        }) {
            return Err(classify_op_rejection(err, id, status, "complete_operation"));
        }

        state.finalize_terminal(id);
        state.maybe_persist();
        Ok(())
    }

    fn fail_operation(&self, id: &OperationId, error: String) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let status = state
            .status(id)
            .ok_or_else(|| OpsLifecycleError::NotFound(id.clone()))?;

        let terminal_outcome = OperationTerminalOutcome::Failed { error };
        let (outcome_kind, outcome_payload) = ShellState::split_outcome(&terminal_outcome);

        if let Err(err) = state.dsl_apply_raw(mm_dsl::MeerkatMachineInput::FailOp {
            operation_id: mm_dsl::OperationId::from_domain(id).0,
            outcome: outcome_kind,
            payload: outcome_payload,
        }) {
            return Err(classify_op_rejection(err, id, status, "fail_operation"));
        }

        state.finalize_terminal(id);
        state.maybe_persist();
        Ok(())
    }

    fn abort_provisioning(
        &self,
        id: &OperationId,
        reason: Option<String>,
    ) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let status = state
            .status(id)
            .ok_or_else(|| OpsLifecycleError::NotFound(id.clone()))?;

        let terminal_outcome = OperationTerminalOutcome::Aborted { reason };
        let (outcome_kind, outcome_payload) = ShellState::split_outcome(&terminal_outcome);

        if let Err(err) = state.dsl_apply_raw(mm_dsl::MeerkatMachineInput::AbortOp {
            operation_id: mm_dsl::OperationId::from_domain(id).0,
            outcome: outcome_kind,
            payload: outcome_payload,
        }) {
            return Err(classify_op_rejection(err, id, status, "abort_provisioning"));
        }

        state.finalize_terminal(id);
        state.maybe_persist();
        Ok(())
    }

    fn cancel_operation(
        &self,
        id: &OperationId,
        reason: Option<String>,
    ) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let status = state
            .status(id)
            .ok_or_else(|| OpsLifecycleError::NotFound(id.clone()))?;

        let terminal_outcome = OperationTerminalOutcome::Cancelled { reason };
        let (outcome_kind, outcome_payload) = ShellState::split_outcome(&terminal_outcome);

        if let Err(err) = state.dsl_apply_raw(mm_dsl::MeerkatMachineInput::CancelOp {
            operation_id: mm_dsl::OperationId::from_domain(id).0,
            outcome: outcome_kind,
            payload: outcome_payload,
        }) {
            return Err(classify_op_rejection(err, id, status, "cancel_operation"));
        }

        state.finalize_terminal(id);
        state.maybe_persist();
        Ok(())
    }

    fn request_retire(&self, id: &OperationId) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let status = state
            .status(id)
            .ok_or_else(|| OpsLifecycleError::NotFound(id.clone()))?;

        if let Err(err) = state.dsl_apply_raw(mm_dsl::MeerkatMachineInput::RetireRequestedOp {
            operation_id: mm_dsl::OperationId::from_domain(id).0,
        }) {
            return Err(classify_op_rejection(err, id, status, "request_retire"));
        }
        Ok(())
    }

    fn mark_retired(&self, id: &OperationId) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let status = state
            .status(id)
            .ok_or_else(|| OpsLifecycleError::NotFound(id.clone()))?;

        let terminal_outcome = OperationTerminalOutcome::Retired;
        let (outcome_kind, outcome_payload) = ShellState::split_outcome(&terminal_outcome);

        if let Err(err) = state.dsl_apply_raw(mm_dsl::MeerkatMachineInput::RetireCompletedOp {
            operation_id: mm_dsl::OperationId::from_domain(id).0,
            outcome: outcome_kind,
            payload: outcome_payload,
        }) {
            return Err(classify_op_rejection(err, id, status, "mark_retired"));
        }

        state.finalize_terminal(id);
        state.maybe_persist();
        Ok(())
    }

    fn snapshot(&self, id: &OperationId) -> Option<OperationLifecycleSnapshot> {
        self.read_state().ok().and_then(|state| state.snapshot(id))
    }

    fn list_operations(&self) -> Vec<OperationLifecycleSnapshot> {
        let mut snapshots = self
            .read_state()
            .map(|state| {
                state
                    .operation_ids()
                    .into_iter()
                    .filter_map(|id| state.snapshot(&id))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        snapshots.sort_by(|left, right| left.display_name.cmp(&right.display_name));
        snapshots
    }

    fn terminate_owner(&self, reason: String) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        // The shell loop is a mechanical cursor over DSL-owned state; the
        // "is this op non-terminal?" decision is expressed as a typed read
        // of the DSL's `op_statuses` map via `OperationStatus::is_terminal`,
        // not a handwritten branch over stringly-typed state. The DSL's
        // `TerminateOp` transition guards on the same non-terminal set, so
        // guard rejection here indicates a genuine state desync rather than
        // the common already-terminal no-op.
        let to_terminate: Vec<(OperationId, OperationStatus)> = state
            .operation_ids()
            .into_iter()
            .filter_map(|id| state.status(&id).map(|s| (id, s)))
            .filter(|(_, s)| !s.is_terminal())
            .collect();

        for (op_id, status) in &to_terminate {
            let terminal_outcome = OperationTerminalOutcome::Terminated {
                reason: reason.clone(),
            };
            let (outcome_kind, outcome_payload) = ShellState::split_outcome(&terminal_outcome);

            if let Err(err) = state.dsl_apply_raw(mm_dsl::MeerkatMachineInput::TerminateOp {
                operation_id: mm_dsl::OperationId::from_domain(op_id).0,
                outcome: outcome_kind,
                payload: outcome_payload,
            }) {
                return Err(classify_op_rejection(
                    err,
                    op_id,
                    *status,
                    "terminate_owner",
                ));
            }

            state.finalize_terminal(op_id);
        }

        if !to_terminate.is_empty() {
            state.maybe_persist();
        }
        Ok(())
    }

    fn collect_completed(
        &self,
    ) -> Result<Vec<(OperationId, OperationTerminalOutcome)>, OpsLifecycleError> {
        let mut state = self.write_state()?;

        let ids: Vec<OperationId> = state.completed_order.drain(..).collect();
        let mut collected = Vec::with_capacity(ids.len());
        for id in ids {
            let outcome = state.terminal_outcome(&id);
            // Remove from DSL state and shell record.
            let id_key = mm_dsl::OperationId::from_domain(&id).0;
            state.dsl.0.state.op_statuses.remove(&id_key);
            state.dsl.0.state.op_kinds.remove(&id_key);
            state.dsl.0.state.op_peer_ready.remove(&id_key);
            state.dsl.0.state.op_progress_counts.remove(&id_key);
            state.dsl.0.state.op_terminal_outcomes.remove(&id_key);
            state.dsl.0.state.op_terminal_payload.remove(&id_key);
            state.dsl.0.state.op_completion_seq.remove(&id_key);
            state.records.remove(&id);
            if let Some(outcome) = outcome {
                collected.push((id, outcome));
            }
        }
        Ok(collected)
    }

    fn completion_feed(&self) -> Option<Arc<dyn CompletionFeed>> {
        Some(self.completion_feed_handle())
    }

    fn wait_all(
        &self,
        _run_id: &RunId,
        ids: &[OperationId],
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<WaitAllResult, OpsLifecycleError>> + Send + '_>,
    > {
        let wait_request_id = WaitRequestId::new();
        let owned_ids = ids.to_vec();

        let state = match self.write_state() {
            Ok(mut state) => {
                // Guard: duplicate ids.
                let mut seen = HashSet::new();
                for op_id in &owned_ids {
                    if !seen.insert(op_id.clone()) {
                        return Box::pin(WaitAllFuture {
                            registry: self,
                            wait_request_id,
                            operation_ids: owned_ids.clone(),
                            state: WaitAllFutureState::Ready(Some(Err(
                                OpsLifecycleError::DuplicateWaitOperation(op_id.clone()),
                            ))),
                        });
                    }
                }
                // Guard: wait already active — check DSL, the sole barrier owner.
                if state.wait_active() {
                    return Box::pin(WaitAllFuture {
                        registry: self,
                        wait_request_id,
                        operation_ids: owned_ids,
                        state: WaitAllFutureState::Ready(Some(Err(
                            OpsLifecycleError::WaitAlreadyActive,
                        ))),
                    });
                }
                // Guard: all ops known.
                for op_id in &owned_ids {
                    if !state.contains(op_id) {
                        return Box::pin(WaitAllFuture {
                            registry: self,
                            wait_request_id,
                            operation_ids: owned_ids.clone(),
                            state: WaitAllFutureState::Ready(Some(Err(
                                OpsLifecycleError::NotFound(op_id.clone()),
                            ))),
                        });
                    }
                }

                // Short-circuit when every target op is already terminal.
                let all_terminal = owned_ids.iter().all(|op_id| {
                    state
                        .status(op_id)
                        .is_some_and(OperationStatus::is_terminal)
                });
                if all_terminal {
                    let outcomes =
                        state
                            .collect_wait_outcomes(&owned_ids)
                            .map(|outcomes| WaitAllResult {
                                outcomes,
                                satisfied: WaitAllSatisfied {
                                    wait_request_id: wait_request_id.clone(),
                                    operation_ids: owned_ids.clone(),
                                },
                            });
                    WaitAllFutureState::Ready(Some(outcomes))
                } else {
                    // Populate DSL barrier membership via RequestWaitAll.
                    // Membership is DSL-owned; shell holds only the oneshot
                    // correlation id for delivering the result to the caller.
                    let dsl_ids: std::collections::BTreeSet<String> = owned_ids
                        .iter()
                        .map(|id| mm_dsl::OperationId::from_domain(id).0)
                        .collect();
                    if let Err(err) = state.dsl_apply(
                        mm_dsl::MeerkatMachineInput::RequestWaitAll {
                            operation_ids: dsl_ids,
                        },
                        "RequestWaitAll",
                    ) {
                        return Box::pin(WaitAllFuture {
                            registry: self,
                            wait_request_id,
                            operation_ids: owned_ids,
                            state: WaitAllFutureState::Ready(Some(Err(err))),
                        });
                    }
                    state.wait_request_id = Some(wait_request_id.clone());

                    if state.pending_wait.is_some() {
                        // Roll back the DSL barrier we just activated so the
                        // registry is not stuck in a wait-active state with
                        // no correlation oneshot to resolve. `CancelWaitAll`
                        // is the no-obligation clearer (members need not be
                        // terminal).
                        state.wait_request_id = None;
                        let _ = state.dsl_apply(
                            mm_dsl::MeerkatMachineInput::CancelWaitAll,
                            "CancelWaitAll(rollback)",
                        );
                        return Box::pin(WaitAllFuture {
                            registry: self,
                            wait_request_id,
                            operation_ids: owned_ids,
                            state: WaitAllFutureState::Ready(Some(Err(
                                OpsLifecycleError::Internal(
                                    "wait_all started while a pending wait sender already existed"
                                        .into(),
                                ),
                            ))),
                        });
                    }
                    let (sender, receiver) = tokio::sync::oneshot::channel();
                    state.pending_wait = Some(PendingWaitState {
                        wait_request_id: wait_request_id.clone(),
                        sender,
                    });
                    WaitAllFutureState::Waiting(receiver)
                }
            }
            Err(err) => WaitAllFutureState::Ready(Some(Err(err))),
        };

        Box::pin(WaitAllFuture {
            registry: self,
            wait_request_id,
            operation_ids: owned_ids,
            state,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::comms::TrustedPeerSpec;
    use meerkat_core::lifecycle::RunId;
    use meerkat_core::ops_lifecycle::{OperationKind, OpsLifecycleRegistry};
    use meerkat_core::types::SessionId;
    use std::sync::atomic::Ordering;
    use uuid::Uuid;

    fn test_run_id() -> RunId {
        RunId(Uuid::from_u128(1))
    }

    fn background_spec(name: &str) -> OperationSpec {
        OperationSpec {
            id: OperationId::new(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: SessionId::new(),
            display_name: name.into(),
            source_label: "test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        }
    }

    #[tokio::test]
    async fn late_watchers_resolve_immediately() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let spec = background_spec("late");
        let op_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&op_id).unwrap();
        registry
            .complete_operation(
                &op_id,
                OperationResult {
                    id: op_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .unwrap();

        let watch = registry.register_watcher(&op_id).unwrap();
        match watch.wait().await {
            OperationTerminalOutcome::Completed(result) => assert_eq!(result.content, "done"),
            other => panic!("expected completed outcome, got {other:?}"),
        }
    }

    #[test]
    fn peer_ready_requires_peer_expectation() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let spec = background_spec("no-peer");
        let op_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&op_id).unwrap();

        let result = registry.peer_ready(
            &op_id,
            OperationPeerHandle {
                peer_name: "peer".into(),
                trusted_peer: TrustedPeerSpec::new("peer", "peer-id", "inproc://peer").unwrap(),
            },
        );
        assert!(matches!(result, Err(OpsLifecycleError::PeerNotExpected(_))));
    }

    #[tokio::test]
    async fn multi_listener_completion() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let spec = background_spec("multi");
        let op_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&op_id).unwrap();

        let watch1 = registry.register_watcher(&op_id).unwrap();
        let watch2 = registry.register_watcher(&op_id).unwrap();
        let watch3 = registry.register_watcher(&op_id).unwrap();

        registry
            .complete_operation(
                &op_id,
                OperationResult {
                    id: op_id.clone(),
                    content: "multi-done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .unwrap();

        for watch in [watch1, watch2, watch3] {
            match watch.wait().await {
                OperationTerminalOutcome::Completed(result) => {
                    assert_eq!(result.content, "multi-done");
                }
                other => panic!("expected completed, got {other:?}"),
            }
        }
    }

    #[test]
    fn background_terminal_sets_detached_wake_pending_without_signaled_latch() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let wake = std::sync::Arc::new(crate::detached_wake::DetachedWakeState::new());
        registry.set_detached_wake(std::sync::Arc::clone(&wake));

        let spec = background_spec("wake");
        let op_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&op_id).unwrap();
        registry
            .complete_operation(
                &op_id,
                OperationResult {
                    id: op_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .unwrap();

        assert!(
            wake.pending.load(Ordering::Acquire),
            "background terminal should arm detached wake pending state"
        );
        assert!(
            !wake.signaled.load(Ordering::Acquire),
            "ops lifecycle should not set the legacy signaled latch directly"
        );
    }

    #[tokio::test]
    async fn wait_all_returns_all_outcomes() {
        let registry = RuntimeOpsLifecycleRegistry::new();

        let spec_a = background_spec("a");
        let id_a = spec_a.id.clone();
        registry.register_operation(spec_a).unwrap();
        registry.provisioning_succeeded(&id_a).unwrap();

        let spec_b = background_spec("b");
        let id_b = spec_b.id.clone();
        registry.register_operation(spec_b).unwrap();
        registry.provisioning_succeeded(&id_b).unwrap();

        registry
            .complete_operation(
                &id_a,
                OperationResult {
                    id: id_a.clone(),
                    content: "a-done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .unwrap();
        registry.fail_operation(&id_b, "b-error".into()).unwrap();

        let wait_result = registry
            .wait_all(&test_run_id(), &[id_a.clone(), id_b.clone()])
            .await
            .unwrap();
        assert_eq!(wait_result.outcomes.len(), 2);
        assert_eq!(wait_result.outcomes[0].0, id_a);
        assert!(matches!(
            wait_result.outcomes[0].1,
            OperationTerminalOutcome::Completed(_)
        ));
        assert_eq!(wait_result.outcomes[1].0, id_b);
        assert!(matches!(
            wait_result.outcomes[1].1,
            OperationTerminalOutcome::Failed { .. }
        ));
        // Obligation carries the awaited IDs
        assert_eq!(wait_result.satisfied.operation_ids.len(), 2);
        assert_ne!(wait_result.satisfied.wait_request_id.to_string(), "");
    }

    /// Exercises the trait `wait_all` path (via `dyn OpsLifecycleRegistry`)
    /// which must submit WaitAll through the DSL for cross-machine handoff.
    #[tokio::test]
    async fn wait_all_trait_path_submits_through_authority() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let spec = background_spec("trait-wait");
        let op_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&op_id).unwrap();
        registry
            .complete_operation(
                &op_id,
                OperationResult {
                    id: op_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .unwrap();

        // Call through trait object to exercise the trait impl, not the inherent method.
        let trait_ref: &dyn OpsLifecycleRegistry = &registry;
        let wait_result = trait_ref
            .wait_all(&test_run_id(), std::slice::from_ref(&op_id))
            .await
            .unwrap();
        assert_eq!(wait_result.outcomes.len(), 1);
        assert!(matches!(
            wait_result.outcomes[0].1,
            OperationTerminalOutcome::Completed(_)
        ));
        // Obligation carries the validated ID
        assert_eq!(wait_result.satisfied.operation_ids, vec![op_id]);
        assert_ne!(wait_result.satisfied.wait_request_id.to_string(), "");
    }

    #[tokio::test]
    async fn wait_all_resolves_from_authority_owned_wait_request() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let run_id = test_run_id();

        let spec = background_spec("pending");
        let op_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&op_id).unwrap();

        let wait_fut = registry.wait_all(&run_id, std::slice::from_ref(&op_id));
        tokio::pin!(wait_fut);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), &mut wait_fut)
                .await
                .is_err()
        );

        let active_wait_request_id = {
            let state = registry.read_state().unwrap();
            let wait_request_id = match state.wait_request_id.clone() {
                Some(wait_request_id) => wait_request_id,
                None => panic!("wait request should be active"),
            };
            assert_eq!(
                state.wait_operation_ids().as_slice(),
                std::slice::from_ref(&op_id)
            );
            wait_request_id
        };

        registry
            .complete_operation(
                &op_id,
                OperationResult {
                    id: op_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .unwrap();

        let wait_result = wait_fut.await.unwrap();
        assert_eq!(
            wait_result.satisfied.wait_request_id,
            active_wait_request_id
        );
        assert_eq!(wait_result.satisfied.operation_ids, vec![op_id.clone()]);
        assert!(matches!(
            wait_result.outcomes.as_slice(),
            [(returned_id, OperationTerminalOutcome::Completed(_))] if *returned_id == op_id
        ));
        assert!(registry.read_state().unwrap().wait_request_id.is_none());
    }

    #[tokio::test]
    async fn dropping_wait_all_future_cancels_active_wait_request() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let run_id = test_run_id();

        let spec = background_spec("cancelled-wait");
        let op_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&op_id).unwrap();

        let wait_fut = registry.wait_all(&run_id, std::slice::from_ref(&op_id));
        drop(wait_fut);

        let state = registry.read_state().unwrap();
        assert!(state.wait_request_id.is_none());
        assert!(state.wait_operation_ids().is_empty());
        assert!(!state.wait_active());
    }

    #[test]
    fn collect_completed_drains_terminal_operations() {
        let registry = RuntimeOpsLifecycleRegistry::new();

        let spec_a = background_spec("a");
        let id_a = spec_a.id.clone();
        registry.register_operation(spec_a).unwrap();
        registry.provisioning_succeeded(&id_a).unwrap();
        registry
            .complete_operation(
                &id_a,
                OperationResult {
                    id: id_a.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .unwrap();

        let spec_b = background_spec("b");
        let id_b = spec_b.id.clone();
        registry.register_operation(spec_b).unwrap();

        let collected = registry.collect_completed().unwrap();
        assert_eq!(collected.len(), 1);
        assert_eq!(collected[0].0, id_a);

        assert!(registry.snapshot(&id_a).is_none());
        assert!(registry.snapshot(&id_b).is_some());

        let collected2 = registry.collect_completed().unwrap();
        assert!(collected2.is_empty());
    }

    #[test]
    fn bounded_completed_retention_evicts_oldest() {
        let registry = RuntimeOpsLifecycleRegistry::with_config(OpsLifecycleConfig {
            max_completed: 3,
            max_concurrent: None,
        });

        let mut ids = Vec::new();
        for i in 0..5 {
            let spec = background_spec(&format!("op-{i}"));
            let id = spec.id.clone();
            registry.register_operation(spec).unwrap();
            registry.provisioning_succeeded(&id).unwrap();
            registry
                .complete_operation(
                    &id,
                    OperationResult {
                        id: id.clone(),
                        content: format!("done-{i}"),
                        is_error: false,
                        duration_ms: 1,
                        tokens_used: 0,
                    },
                )
                .unwrap();
            ids.push(id);
        }

        assert!(registry.snapshot(&ids[0]).is_none());
        assert!(registry.snapshot(&ids[1]).is_none());
        assert!(registry.snapshot(&ids[2]).is_some());
        assert!(registry.snapshot(&ids[3]).is_some());
        assert!(registry.snapshot(&ids[4]).is_some());
    }

    #[test]
    fn max_concurrent_enforcement() {
        let registry = RuntimeOpsLifecycleRegistry::with_config(OpsLifecycleConfig {
            max_completed: DEFAULT_MAX_COMPLETED,
            max_concurrent: Some(2),
        });

        let spec_a = background_spec("a");
        let id_a = spec_a.id.clone();
        registry.register_operation(spec_a).unwrap();

        let spec_b = background_spec("b");
        registry.register_operation(spec_b).unwrap();

        let spec_c = background_spec("c");
        let result = registry.register_operation(spec_c);
        assert!(matches!(
            result,
            Err(OpsLifecycleError::MaxConcurrentExceeded {
                limit: 2,
                active: 2,
            })
        ));

        registry.provisioning_succeeded(&id_a).unwrap();
        registry
            .complete_operation(
                &id_a,
                OperationResult {
                    id: id_a.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .unwrap();

        let spec_d = background_spec("d");
        assert!(registry.register_operation(spec_d).is_ok());
    }

    #[test]
    fn snapshot_includes_timestamps() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let spec = background_spec("timed");
        let op_id = spec.id.clone();
        registry.register_operation(spec).unwrap();

        let snap1 = registry.snapshot(&op_id).unwrap();
        assert!(snap1.created_at_ms > 0);
        assert!(snap1.started_at_ms.is_none());
        assert!(snap1.completed_at_ms.is_none());
        assert!(snap1.elapsed_ms.is_none());

        registry.provisioning_succeeded(&op_id).unwrap();
        let snap2 = registry.snapshot(&op_id).unwrap();
        assert!(snap2.started_at_ms.is_some());
        assert!(snap2.started_at_ms.unwrap() >= snap2.created_at_ms);

        registry
            .complete_operation(
                &op_id,
                OperationResult {
                    id: op_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .unwrap();
        let snap3 = registry.snapshot(&op_id).unwrap();
        assert!(snap3.completed_at_ms.is_some());
        assert!(snap3.elapsed_ms.is_some());
        assert!(snap3.completed_at_ms.unwrap() >= snap3.started_at_ms.unwrap());
    }

    #[test]
    fn snapshot_includes_peer_handle() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let spec = OperationSpec {
            id: OperationId::new(),
            kind: OperationKind::MobMemberChild,
            owner_session_id: SessionId::new(),
            display_name: "peer-test".into(),
            source_label: "test".into(),
            child_session_id: Some(SessionId::new()),
            expect_peer_channel: true,
        };
        let op_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&op_id).unwrap();

        let snap1 = registry.snapshot(&op_id).unwrap();
        assert!(snap1.peer_handle.is_none());

        let handle = OperationPeerHandle {
            peer_name: "member-x".into(),
            trusted_peer: TrustedPeerSpec::new("member-x", "peer-id", "inproc://x").unwrap(),
        };
        registry.peer_ready(&op_id, handle).unwrap();

        let snap2 = registry.snapshot(&op_id).unwrap();
        assert_eq!(snap2.peer_handle.as_ref().unwrap().peer_name, "member-x");
    }
}

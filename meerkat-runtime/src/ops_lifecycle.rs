//! In-memory runtime implementation of the shared async-operation lifecycle seam.
//!
//! Per-operation canonical lifecycle state lives in the MeerkatMachine DSL
//! authority (`op_statuses`, `op_terminal_outcomes`, `op_kinds`,
//! `op_peer_ready`, `op_progress_counts`, `active_op_count`, `wait_active`,
//! `wait_operation_ids`). This shell layer owns pure mechanics: watcher
//! channels, timestamps, peer handles, snapshot assembly, FIFO eviction
//! bookkeeping, the completion feed buffer, and typed delivery of generated
//! admission/rejection feedback.
//!
//! Per-transition legality ("is `CompleteOp` legal on a `Provisioning` op?")
//! is NOT owned by the shell — it lives in the DSL's `from_status_valid`
//! guards on each op-lifecycle transition. The shell's only job on a
//! `GuardRejected` rejection is to ask the generated rejection resolver for
//! the public result class.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
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

#[derive(Debug)]
pub struct OpsLifecyclePersistenceRequest {
    snapshot: PersistedOpsSnapshot,
    result_tx: std::sync::mpsc::SyncSender<Result<(), OpsLifecycleError>>,
}

impl OpsLifecyclePersistenceRequest {
    pub fn snapshot(&self) -> &PersistedOpsSnapshot {
        &self.snapshot
    }

    pub fn complete(self, result: Result<(), OpsLifecycleError>) {
        let _ = self.result_tx.send(result);
    }
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

enum WaitAllAuthorityPlan {
    AlreadySatisfied(WaitAllSatisfied),
    ActivateBarrier,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecoveredOperationRecordDisposition {
    Retain,
    Discard,
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
    /// Shared feed buffer for completion events.
    feed_buffer: Arc<FeedBuffer>,
    /// Persistence channel for durable snapshot writes (set via `set_persistence_channel`).
    persist_tx: Option<crate::tokio::sync::mpsc::UnboundedSender<OpsLifecyclePersistenceRequest>>,
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
    /// `NoMatchingTransition` (a shell/DSL desync). Op-lifecycle entry points
    /// feed guard rejections into generated rejection feedback before surfacing
    /// public result classes.
    fn dsl_apply_raw(
        &mut self,
        input: mm_dsl::MeerkatMachineInput,
    ) -> Result<(), mm_dsl::MeerkatMachineTransitionError> {
        mm_dsl::MeerkatMachineMutator::apply(&mut self.dsl.0, input).map(|_transition| ())
    }

    fn dsl_apply_with_effects(
        &mut self,
        input: mm_dsl::MeerkatMachineInput,
        context: &str,
    ) -> Result<Vec<mm_dsl::MeerkatMachineEffect>, OpsLifecycleError> {
        let transition =
            mm_dsl::MeerkatMachineMutator::apply(&mut self.dsl.0, input).map_err(|err| {
                OpsLifecycleError::Internal(format!(
                    "DSL rejected ops transition ({context}): {err:?}"
                ))
            })?;
        Ok(transition.effects)
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

    /// Read the DSL-minted completion sequence for a terminal operation.
    fn completion_sequence(&self, id: &OperationId) -> Option<CompletionSeq> {
        let id_key = mm_dsl::OperationId::from_domain(id).0;
        self.dsl.0.state.op_completion_seq.get(&id_key).copied()
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
    /// push CompletionEntry, retain in FIFO, evict as needed. Called AFTER the
    /// DSL transition has already persisted the terminal status + outcome.
    fn finalize_terminal(&mut self, id: &OperationId) -> Result<(), OpsLifecycleError> {
        let outcome = match self.terminal_outcome(id) {
            Some(o) => o,
            None => return Ok(()),
        };
        let kind = self.kind(id);

        // Notify watchers and mark completion timestamp.
        if let Some(shell) = self.records.get_mut(id) {
            shell.notify_watchers(&outcome);
            shell.mark_completed();
        }

        // Push completion feed entry.
        let seq = self.completion_sequence(id).ok_or_else(|| {
            OpsLifecycleError::Internal(format!(
                "generated op terminal transition did not mint completion sequence for {id}"
            ))
        })?;
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
                self.dsl_apply(
                    mm_dsl::MeerkatMachineInput::EvictCompletedOp {
                        operation_id: mm_dsl::OperationId::from_domain(&evicted).0,
                    },
                    "EvictCompletedOp",
                )?;
                self.records.remove(&evicted);
            }
        }

        // Satisfy a pending wait request if all its ops are now terminal.
        self.maybe_satisfy_wait();
        Ok(())
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
    #[cfg(test)]
    fn wait_active(&self) -> bool {
        self.dsl.0.state.wait_active
    }

    fn wait_all_satisfied_from_effects(
        effects: &[mm_dsl::MeerkatMachineEffect],
    ) -> Result<Option<WaitAllSatisfied>, OpsLifecycleError> {
        let mut satisfied = None;
        for effect in effects {
            let mm_dsl::MeerkatMachineEffect::WaitAllSatisfied {
                wait_request_id,
                operation_ids,
            } = effect
            else {
                continue;
            };
            if satisfied.is_some() {
                return Err(OpsLifecycleError::Internal(
                    "generated wait_all authority emitted multiple satisfaction effects".into(),
                ));
            }
            let wait_uuid = uuid::Uuid::parse_str(&wait_request_id.0).map_err(|err| {
                OpsLifecycleError::Internal(format!(
                    "generated wait_all authority emitted invalid wait request id '{}': {err}",
                    wait_request_id.0
                ))
            })?;
            let mut ids = Vec::with_capacity(operation_ids.len());
            for operation_id in operation_ids {
                ids.push(
                    serde_json::from_str::<OperationId>(&operation_id.0).map_err(|err| {
                        OpsLifecycleError::Internal(format!(
                            "generated wait_all authority emitted invalid operation id '{}': {err}",
                            operation_id.0
                        ))
                    })?,
                );
            }
            satisfied = Some(WaitAllSatisfied {
                wait_request_id: WaitRequestId::from_uuid(wait_uuid),
                operation_ids: ids,
            });
        }
        Ok(satisfied)
    }

    fn parse_wait_all_operation_id(
        raw: &str,
        context: &str,
    ) -> Result<OperationId, OpsLifecycleError> {
        serde_json::from_str::<OperationId>(raw).map_err(|err| {
            OpsLifecycleError::Internal(format!(
                "generated wait_all authority emitted invalid {context} operation id '{raw}': {err}"
            ))
        })
    }

    fn duplicate_wait_operation_id(operation_ids: &[OperationId]) -> Option<OperationId> {
        let mut seen = HashSet::new();
        operation_ids
            .iter()
            .find(|operation_id| !seen.insert((*operation_id).clone()))
            .cloned()
    }

    fn wait_all_admission_error_from_effects(
        wait_request_id: &WaitRequestId,
        effects: &[mm_dsl::MeerkatMachineEffect],
    ) -> Result<Option<OpsLifecycleError>, OpsLifecycleError> {
        let mut admission = None;
        for effect in effects {
            let mm_dsl::MeerkatMachineEffect::WaitAllAdmissionResolved {
                wait_request_id: resolved_wait_request_id,
                result,
                reject_reason,
                rejected_operation_id,
            } = effect
            else {
                continue;
            };
            if admission.is_some() {
                return Err(OpsLifecycleError::Internal(
                    "generated wait_all authority emitted multiple admission results".into(),
                ));
            }
            let resolved_uuid =
                uuid::Uuid::parse_str(&resolved_wait_request_id.0).map_err(|err| {
                    OpsLifecycleError::Internal(format!(
                        "generated wait_all authority emitted invalid wait request id '{}': {err}",
                        resolved_wait_request_id.0
                    ))
                })?;
            let resolved_wait_request_id = WaitRequestId::from_uuid(resolved_uuid);
            if &resolved_wait_request_id != wait_request_id {
                return Err(OpsLifecycleError::Internal(format!(
                    "generated wait_all authority resolved wait request {resolved_wait_request_id} while shell requested {wait_request_id}"
                )));
            }
            admission = Some(match result {
                mm_dsl::WaitAllAdmissionResultKind::Accept => {
                    if reject_reason.is_some() || rejected_operation_id.is_some() {
                        return Err(OpsLifecycleError::Internal(
                            "generated wait_all authority accepted with rejection payload".into(),
                        ));
                    }
                    None
                }
                mm_dsl::WaitAllAdmissionResultKind::Reject => {
                    let reason = reject_reason.ok_or_else(|| {
                        OpsLifecycleError::Internal(
                            "generated wait_all authority rejected without reason".into(),
                        )
                    })?;
                    let error = match reason {
                        mm_dsl::WaitAllRejectReasonKind::DuplicateOperation => {
                            let raw = rejected_operation_id.as_deref().ok_or_else(|| {
                                OpsLifecycleError::Internal(
                                    "generated wait_all authority rejected duplicate without operation id"
                                        .into(),
                                )
                            })?;
                            OpsLifecycleError::DuplicateWaitOperation(
                                Self::parse_wait_all_operation_id(raw, "duplicate")?,
                            )
                        }
                        mm_dsl::WaitAllRejectReasonKind::WaitAlreadyActive => {
                            if rejected_operation_id.is_some() {
                                return Err(OpsLifecycleError::Internal(
                                    "generated wait_all authority rejected active wait with operation id"
                                        .into(),
                                ));
                            }
                            OpsLifecycleError::WaitAlreadyActive
                        }
                        mm_dsl::WaitAllRejectReasonKind::OperationNotFound => {
                            let raw = rejected_operation_id.as_deref().ok_or_else(|| {
                                OpsLifecycleError::Internal(
                                    "generated wait_all authority rejected missing operation without operation id"
                                        .into(),
                                )
                            })?;
                            OpsLifecycleError::NotFound(Self::parse_wait_all_operation_id(
                                raw, "missing",
                            )?)
                        }
                    };
                    Some(error)
                }
            });
        }
        admission.ok_or_else(|| {
            OpsLifecycleError::Internal(
                "generated wait_all authority emitted no admission result".into(),
            )
        })
    }

    fn resolve_wait_all_admission(
        &mut self,
        wait_request_id: &WaitRequestId,
        operation_ids: &[OperationId],
        dsl_ids: &BTreeSet<String>,
        dsl_id_tokens: &BTreeSet<mm_dsl::OperationId>,
        operation_token_by_id: &BTreeMap<String, mm_dsl::OperationId>,
        operation_id_by_token: &BTreeMap<mm_dsl::OperationId, String>,
    ) -> Result<(), OpsLifecycleError> {
        let duplicate = Self::duplicate_wait_operation_id(operation_ids)
            .map(|operation_id| mm_dsl::OperationId::from_domain(&operation_id).0);
        let not_found = operation_ids
            .iter()
            .find(|operation_id| !self.contains(operation_id))
            .map(|operation_id| mm_dsl::OperationId::from_domain(operation_id).0);
        let dsl_id_sequence: Vec<String> = operation_ids
            .iter()
            .map(|id| mm_dsl::OperationId::from_domain(id).0)
            .collect();
        let effects = self.dsl_apply_with_effects(
            mm_dsl::MeerkatMachineInput::ResolveWaitAllAdmission {
                wait_request_id: mm_dsl::WaitRequestId::from_domain(wait_request_id),
                operation_id_sequence: dsl_id_sequence,
                operation_ids: dsl_ids.clone(),
                operation_id_tokens: dsl_id_tokens.clone(),
                operation_token_by_id: operation_token_by_id.clone(),
                operation_id_by_token: operation_id_by_token.clone(),
                duplicate_operation_id: duplicate,
                not_found_operation_id: not_found,
            },
            "ResolveWaitAllAdmission",
        )?;
        if let Some(error) = Self::wait_all_admission_error_from_effects(wait_request_id, &effects)?
        {
            return Err(error);
        }
        Ok(())
    }

    fn try_satisfy_wait_all_authority(
        &mut self,
    ) -> Result<Option<WaitAllSatisfied>, OpsLifecycleError> {
        let Some(dsl_wait_request_id) = self.dsl.0.state.wait_request_id.clone() else {
            return Ok(None);
        };
        let dsl_operation_id_tokens = self.dsl.0.state.wait_operation_id_tokens.clone();
        let transition = match mm_dsl::MeerkatMachineMutator::apply(
            &mut self.dsl.0,
            mm_dsl::MeerkatMachineInput::SatisfyWaitAll {
                wait_request_id: dsl_wait_request_id,
                operation_id_tokens: dsl_operation_id_tokens,
            },
        ) {
            Ok(transition) => transition,
            Err(mm_dsl::MeerkatMachineTransitionError::GuardRejected { .. }) => return Ok(None),
            Err(err) => {
                return Err(OpsLifecycleError::Internal(format!(
                    "DSL rejected ops transition (SatisfyWaitAll): {err:?}"
                )));
            }
        };
        Self::wait_all_satisfied_from_effects(&transition.effects)?
            .ok_or_else(|| {
                OpsLifecycleError::Internal(
                    "generated wait_all authority accepted satisfaction without effect".into(),
                )
            })
            .map(Some)
    }

    fn begin_wait_all_authority(
        &mut self,
        wait_request_id: &WaitRequestId,
        operation_ids: &[OperationId],
    ) -> Result<WaitAllAuthorityPlan, OpsLifecycleError> {
        let mut dsl_ids = BTreeSet::new();
        let mut dsl_id_tokens = BTreeSet::new();
        let mut operation_token_by_id = BTreeMap::new();
        let mut operation_id_by_token = BTreeMap::new();
        for id in operation_ids {
            let token = mm_dsl::OperationId::from_domain(id);
            let raw_id = token.0.clone();
            dsl_ids.insert(raw_id.clone());
            dsl_id_tokens.insert(token.clone());
            operation_token_by_id.insert(raw_id.clone(), token.clone());
            operation_id_by_token.insert(token, raw_id);
        }
        self.resolve_wait_all_admission(
            wait_request_id,
            operation_ids,
            &dsl_ids,
            &dsl_id_tokens,
            &operation_token_by_id,
            &operation_id_by_token,
        )?;
        self.dsl_apply(
            mm_dsl::MeerkatMachineInput::RequestWaitAll {
                wait_request_id: mm_dsl::WaitRequestId::from_domain(wait_request_id),
                operation_id_sequence: operation_ids
                    .iter()
                    .map(|id| mm_dsl::OperationId::from_domain(id).0)
                    .collect(),
                operation_ids: dsl_ids,
                operation_id_tokens: dsl_id_tokens,
                operation_token_by_id,
                operation_id_by_token,
            },
            "RequestWaitAll",
        )?;
        if let Some(satisfied) = self.try_satisfy_wait_all_authority()? {
            return Ok(WaitAllAuthorityPlan::AlreadySatisfied(satisfied));
        }
        Ok(WaitAllAuthorityPlan::ActivateBarrier)
    }

    fn owner_termination_targets(
        &self,
    ) -> Result<Vec<(OperationId, OperationStatus)>, OpsLifecycleError> {
        let mut targets = Vec::new();
        for id in self.operation_ids() {
            let Some(status) = self.status(&id) else {
                continue;
            };
            if !Self::operation_status_is_terminal(&id, status)? {
                targets.push((id, status));
            }
        }
        Ok(targets)
    }

    fn operation_status_is_terminal(
        operation_id: &OperationId,
        status: OperationStatus,
    ) -> Result<bool, OpsLifecycleError> {
        let operation_id_key = mm_dsl::OperationId::from_domain(operation_id).0;
        let effects = Self::apply_stateless_classifier(
            mm_dsl::MeerkatMachineInput::ClassifyOperationTerminality {
                operation_id: operation_id_key.clone(),
                status: mm_dsl::OperationStatus::from(status),
            },
            "ClassifyOperationTerminality",
        )?;
        let mut terminal = None;
        for effect in effects {
            match effect {
                mm_dsl::MeerkatMachineEffect::OperationTerminal { operation_id }
                    if operation_id == operation_id_key =>
                {
                    terminal = Some(true)
                }
                mm_dsl::MeerkatMachineEffect::OperationNonTerminal { operation_id }
                    if operation_id == operation_id_key =>
                {
                    terminal = Some(false)
                }
                other => {
                    return Err(OpsLifecycleError::Internal(format!(
                        "unexpected generated operation terminality effect: {other:?}"
                    )));
                }
            }
        }
        terminal.ok_or_else(|| {
            OpsLifecycleError::Internal(format!(
                "generated operation terminality authority emitted no effect for {operation_id}"
            ))
        })
    }

    fn recovered_operation_record_disposition(
        operation_id: &OperationId,
        status: OperationStatus,
        terminal_outcome_present: bool,
        terminal_payload_present: bool,
        completion_sequence_present: bool,
    ) -> Result<RecoveredOperationRecordDisposition, OpsLifecycleError> {
        let operation_id_key = mm_dsl::OperationId::from_domain(operation_id).0;
        let effects = Self::apply_stateless_classifier(
            mm_dsl::MeerkatMachineInput::ClassifyRecoveredOperationRecord {
                operation_id: operation_id_key.clone(),
                status: mm_dsl::OperationStatus::from(status),
                terminal_outcome_present,
                terminal_payload_present,
                completion_sequence_present,
            },
            "ClassifyRecoveredOperationRecord",
        )?;
        let mut disposition = None;
        for effect in effects {
            match effect {
                mm_dsl::MeerkatMachineEffect::RetainTerminalRecord { operation_id }
                    if operation_id == operation_id_key =>
                {
                    disposition = Some(RecoveredOperationRecordDisposition::Retain)
                }
                mm_dsl::MeerkatMachineEffect::DiscardRecoveredOperationRecord { operation_id }
                    if operation_id == operation_id_key =>
                {
                    disposition = Some(RecoveredOperationRecordDisposition::Discard)
                }
                other => {
                    return Err(OpsLifecycleError::Internal(format!(
                        "unexpected generated recovered-operation classification effect: {other:?}"
                    )));
                }
            }
        }
        disposition.ok_or_else(|| {
            OpsLifecycleError::Internal(format!(
                "generated recovered-operation classifier emitted no effect for {operation_id}"
            ))
        })
    }

    fn apply_stateless_classifier(
        input: mm_dsl::MeerkatMachineInput,
        label: &'static str,
    ) -> Result<Vec<mm_dsl::MeerkatMachineEffect>, OpsLifecycleError> {
        let mut authority =
            mm_dsl::MeerkatMachineAuthority::from_state(mm_dsl::MeerkatMachineState {
                lifecycle_phase: mm_dsl::MeerkatPhase::Idle,
                ..mm_dsl::MeerkatMachineState::default()
            });
        let transition =
            mm_dsl::MeerkatMachineMutator::apply(&mut authority, input).map_err(|err| {
                OpsLifecycleError::Internal(format!(
                    "DSL rejected ops transition ({label}): {err:?}"
                ))
            })?;
        Ok(transition.effects)
    }

    /// Check whether a pending barrier wait is now satisfied and resolve it.
    ///
    /// Barrier membership and the "all members terminal" decision both live
    /// in the DSL: `wait_operation_ids` carries the set, `wait_active`
    /// signals a pending barrier, and `SatisfyWaitAll`'s
    /// `all_members_terminal` guard owns the fixed-point test. The shell
    /// echoes the DSL-owned request id and typed operation tokens into
    /// `SatisfyWaitAll` so the transition can clear the barrier before
    /// rendering the handoff effect. It still fires idempotently on every
    /// terminal transition and lets the DSL guard reject early firings as a
    /// no-op. On acceptance (transition returns `Ok`), the shell selects the
    /// correlated oneshot and delivers the `WaitAllSatisfied` obligation
    /// token.
    ///
    /// `wait_request_id` is the shell-owned oneshot correlation id that
    /// selects which sender to notify; when the DSL barrier satisfies
    /// without a live correlation (post-recovery, or duplicate resolution),
    /// the oneshot simply remains pending.
    fn maybe_satisfy_wait(&mut self) {
        let satisfied = match self.try_satisfy_wait_all_authority() {
            Ok(Some(satisfied)) => satisfied,
            Ok(None) => return,
            Err(err) => {
                tracing::error!(
                    error = %err,
                    "generated wait_all authority rejected satisfaction unexpectedly"
                );
                return;
            }
        };
        let shell_wait_id = self.wait_request_id.take();
        if shell_wait_id
            .as_ref()
            .is_some_and(|id| id != &satisfied.wait_request_id)
        {
            tracing::error!(
                shell_wait_request_id = ?shell_wait_id,
                authority_wait_request_id = %satisfied.wait_request_id,
                "generated wait_all authority satisfied a different wait request"
            );
        }
        if let Some(pending) = self.pending_wait.take() {
            if pending.wait_request_id == satisfied.wait_request_id {
                let _ = pending.sender.send(satisfied);
            } else if let Some(shell_wait_id) = shell_wait_id {
                tracing::error!(
                    shell_wait_request_id = %shell_wait_id,
                    pending_wait_request_id = %pending.wait_request_id,
                    authority_wait_request_id = %satisfied.wait_request_id,
                    "generated wait_all authority satisfied without a matching pending waiter"
                );
            }
        }
    }

    /// Persist a terminal snapshot if a persistence channel is wired.
    ///
    /// Called after terminal transitions. Captures authority + entries + cursors under the write
    /// lock (caller already holds it), submits a persistence request, and waits for the worker's
    /// durable-store result. Returning success only means the snapshot write itself succeeded.
    fn maybe_persist(&self) -> Result<(), OpsLifecycleError> {
        let (tx, epoch_id, cursor_state) = match (
            &self.persist_tx,
            &self.persist_epoch_id,
            &self.persist_cursor_state,
        ) {
            (Some(tx), Some(epoch_id), Some(cs)) => (tx, epoch_id, cs),
            _ => return Ok(()),
        };

        let snapshot = self.capture_snapshot(epoch_id.clone(), cursor_state);
        let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
        let request = OpsLifecyclePersistenceRequest {
            snapshot,
            result_tx,
        };

        tx.send(request).map_err(|_| {
            OpsLifecycleError::Internal(
                "ops lifecycle persistence channel closed before terminal snapshot could be queued"
                    .into(),
            )
        })?;
        result_rx.recv().map_err(|_| {
            OpsLifecycleError::Internal(
                "ops lifecycle persistence worker dropped terminal snapshot before confirming durability"
                    .into(),
            )
        })?
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
            next_completion_seq: self.dsl.0.state.next_completion_seq,
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
        tx: crate::tokio::sync::mpsc::UnboundedSender<OpsLifecyclePersistenceRequest>,
        epoch_id: meerkat_core::RuntimeEpochId,
        cursor_state: Arc<meerkat_core::EpochCursorState>,
    ) {
        if let Ok(mut state) = self.state.write() {
            state.persist_tx = Some(tx);
            state.persist_epoch_id = Some(epoch_id);
            state.persist_cursor_state = Some(cursor_state);
        }
    }

    /// Recover from a persisted snapshot.
    ///
    /// Rebuilds DSL state (stripping non-terminal ops — only terminals
    /// survive recovery), creates fresh shell records from specs, and seeds
    /// the feed buffer only with completion entries accepted by generated
    /// recovery authority.
    pub fn from_recovered(snapshot: PersistedOpsSnapshot) -> Result<Self, OpsLifecycleError> {
        let PersistedOpsSnapshot {
            authority_state,
            operation_specs,
            completion_entries,
            ..
        } = snapshot;
        let mut shell = ShellState::new(
            authority_state.max_completed,
            authority_state.max_concurrent,
        );
        let mut completion_sequences: HashMap<OperationId, CompletionSeq> = HashMap::new();
        let mut seen_completion_seqs: HashSet<CompletionSeq> = HashSet::new();
        for entry in &completion_entries {
            if completion_sequences
                .insert(entry.operation_id.clone(), entry.seq)
                .is_some()
            {
                return Err(OpsLifecycleError::Internal(format!(
                    "persisted ops snapshot contains duplicate completion feed entry for {}",
                    entry.operation_id
                )));
            }
            if !seen_completion_seqs.insert(entry.seq) {
                return Err(OpsLifecycleError::Internal(format!(
                    "persisted ops snapshot contains duplicate completion sequence {}",
                    entry.seq
                )));
            }
        }

        // Replay every persisted op through generated recovery authority.
        // The transition accepts only terminal records with outcome and
        // completion-sequence witnesses. Volatile non-terminal rows are not
        // recovered; terminal/corrupt rows must fail closed instead of being
        // projected into shell/public feed state.
        let mut retained_ids: HashSet<OperationId> = HashSet::new();
        for (op_id, op_state) in authority_state.operations {
            let (terminal_outcome, terminal_payload) = op_state
                .terminal_outcome
                .as_ref()
                .map(ShellState::split_outcome)
                .map(|(kind, payload)| (Some(kind), Some(payload)))
                .unwrap_or((None, None));
            let disposition = ShellState::recovered_operation_record_disposition(
                &op_id,
                op_state.status,
                terminal_outcome.is_some(),
                terminal_payload.is_some(),
                completion_sequences.contains_key(&op_id),
            )?;
            if disposition == RecoveredOperationRecordDisposition::Discard {
                continue;
            }
            let recovery = mm_dsl::MeerkatMachineInput::RecoverOpRecord {
                operation_id: mm_dsl::OperationId::from_domain(&op_id).0,
                status: mm_dsl::OperationStatus::from(op_state.status),
                kind: mm_dsl::OperationKind::from(op_state.kind),
                peer_ready: op_state.peer_ready,
                progress_count: u64::from(op_state.progress_count),
                terminal_outcome,
                terminal_payload,
                completion_sequence: completion_sequences.get(&op_id).copied(),
            };
            shell.dsl_apply(recovery, "RecoverOpRecord")?;
            let recovered_seq = shell.completion_sequence(&op_id).ok_or_else(|| {
                OpsLifecycleError::Internal(format!(
                    "generated op recovery accepted {op_id} without completion sequence"
                ))
            })?;
            if completion_sequences.get(&op_id).copied() != Some(recovered_seq) {
                return Err(OpsLifecycleError::Internal(format!(
                    "generated op recovery completion sequence mismatch for {op_id}"
                )));
            }
            retained_ids.insert(op_id);
        }
        shell.dsl_apply(
            mm_dsl::MeerkatMachineInput::RecoverOpsCompletionCursor {
                next_completion_seq: authority_state.next_completion_seq,
            },
            "RecoverOpsCompletionCursor",
        )?;

        // Rebuild completed_order from generated completion-sequence truth,
        // never from the persisted shell ordering mirror.
        let mut recovered_completed: Vec<(CompletionSeq, OperationId)> = retained_ids
            .iter()
            .filter_map(|id| shell.completion_sequence(id).map(|seq| (seq, id.clone())))
            .collect();
        recovered_completed.sort_by_key(|(seq, _)| *seq);
        shell.completed_order = recovered_completed.into_iter().map(|(_, id)| id).collect();

        // Seed the feed buffer only for rows accepted by generated recovery,
        // validating the public result facts against the recovered DSL maps.
        let mut recovered_entries = completion_entries;
        recovered_entries.sort_by_key(|entry| entry.seq);
        for entry in recovered_entries {
            let Some(recovered_seq) = shell.completion_sequence(&entry.operation_id) else {
                return Err(OpsLifecycleError::Internal(format!(
                    "persisted completion feed entry for {} has no generated recovered operation",
                    entry.operation_id
                )));
            };
            if recovered_seq != entry.seq {
                return Err(OpsLifecycleError::Internal(format!(
                    "persisted completion feed entry for {} has sequence {} but generated recovery owns {}",
                    entry.operation_id, entry.seq, recovered_seq
                )));
            }
            let Some(recovered_kind) = shell.kind(&entry.operation_id) else {
                return Err(OpsLifecycleError::Internal(format!(
                    "persisted completion feed entry for {} has no generated recovered kind",
                    entry.operation_id
                )));
            };
            if recovered_kind != entry.kind {
                return Err(OpsLifecycleError::Internal(format!(
                    "persisted completion feed entry for {} has kind {:?} but generated recovery owns {:?}",
                    entry.operation_id, entry.kind, recovered_kind
                )));
            }
            let Some(recovered_outcome) = shell.terminal_outcome(&entry.operation_id) else {
                return Err(OpsLifecycleError::Internal(format!(
                    "persisted completion feed entry for {} has no generated recovered terminal outcome",
                    entry.operation_id
                )));
            };
            if recovered_outcome != entry.terminal_outcome {
                return Err(OpsLifecycleError::Internal(format!(
                    "persisted completion feed entry for {} has terminal outcome {:?} but generated recovery owns {:?}",
                    entry.operation_id, entry.terminal_outcome, recovered_outcome
                )));
            }
            shell.feed_buffer.push(entry);
        }

        // Rebuild shell records from specs (fresh timestamps, no watchers)
        // — only for operations still retained in the DSL state.
        for (op_id, spec) in operation_specs {
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

        Ok(Self {
            state: RwLock::new(shell),
        })
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
                // Clear the DSL barrier via the dedicated `CancelWaitAll`
                // transition. Unlike `SatisfyWaitAll`, it does not require
                // every member to be terminal (the request was dropped, not
                // resolved) and does not emit the `WaitAllSatisfied`.
                state.dsl_apply(
                    mm_dsl::MeerkatMachineInput::CancelWaitAll,
                    "CancelWaitAll(cancel)",
                )?;
                state.wait_request_id = None;
                if state
                    .pending_wait
                    .as_ref()
                    .is_some_and(|pending| pending.wait_request_id == *wait_request_id)
                {
                    state.pending_wait = None;
                }
                Ok(())
            }
            _ => {
                if state
                    .pending_wait
                    .as_ref()
                    .is_some_and(|pending| pending.wait_request_id == *wait_request_id)
                {
                    state.pending_wait = None;
                }
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
                        Ok(state) => state.collect_wait_outcomes(&satisfied.operation_ids),
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
            if let Err(err) = self
                .registry
                .cancel_wait_all_internal(&self.wait_request_id)
            {
                tracing::error!(
                    wait_request_id = %self.wait_request_id,
                    error = %err,
                    "generated wait_all authority rejected cancellation during drop"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Generated op lifecycle result-class feedback
// ---------------------------------------------------------------------------

fn op_lifecycle_action_label(action: mm_dsl::OpLifecycleActionKind) -> &'static str {
    match action {
        mm_dsl::OpLifecycleActionKind::Start => "provisioning_succeeded",
        mm_dsl::OpLifecycleActionKind::Fail => "fail_operation",
        mm_dsl::OpLifecycleActionKind::PeerReady => "peer_ready",
        mm_dsl::OpLifecycleActionKind::ProgressReported => "report_progress",
        mm_dsl::OpLifecycleActionKind::Complete => "complete_operation",
        mm_dsl::OpLifecycleActionKind::Abort => "abort_provisioning",
        mm_dsl::OpLifecycleActionKind::Cancel => "cancel_operation",
        mm_dsl::OpLifecycleActionKind::RetireRequested => "request_retire",
        mm_dsl::OpLifecycleActionKind::RetireCompleted => "mark_retired",
        mm_dsl::OpLifecycleActionKind::Terminate => "terminate_owner",
    }
}

fn op_lifecycle_rejection_error_from_effects(
    id: &OperationId,
    requested_action: mm_dsl::OpLifecycleActionKind,
    effects: &[mm_dsl::MeerkatMachineEffect],
) -> Result<OpsLifecycleError, OpsLifecycleError> {
    let expected_id = mm_dsl::OperationId::from_domain(id).0;
    let mut rejection = None;
    for effect in effects {
        let mm_dsl::MeerkatMachineEffect::OpLifecycleTransitionRejected {
            operation_id,
            action,
            reason,
            status,
        } = effect
        else {
            continue;
        };
        if rejection.is_some() {
            return Err(OpsLifecycleError::Internal(
                "generated op lifecycle authority emitted multiple rejection results".into(),
            ));
        }
        if operation_id != &expected_id || *action != requested_action {
            return Err(OpsLifecycleError::Internal(format!(
                "generated op lifecycle authority resolved {operation_id}/{action:?} while shell requested {expected_id}/{requested_action:?}"
            )));
        }
        let error = match reason {
            mm_dsl::OpLifecycleRejectReasonKind::OperationNotFound => {
                if status.is_some() {
                    return Err(OpsLifecycleError::Internal(
                        "generated op lifecycle authority emitted not-found with status".into(),
                    ));
                }
                OpsLifecycleError::NotFound(id.clone())
            }
            mm_dsl::OpLifecycleRejectReasonKind::InvalidTransition => {
                let status = status.ok_or_else(|| {
                    OpsLifecycleError::Internal(
                        "generated op lifecycle authority emitted invalid-transition without status"
                            .into(),
                    )
                })?;
                OpsLifecycleError::InvalidTransition {
                    id: id.clone(),
                    status: OperationStatus::from(status),
                    action: op_lifecycle_action_label(requested_action),
                }
            }
            mm_dsl::OpLifecycleRejectReasonKind::PeerNotExpected => {
                if status.is_none() {
                    return Err(OpsLifecycleError::Internal(
                        "generated op lifecycle authority emitted peer-not-expected without status"
                            .into(),
                    ));
                }
                OpsLifecycleError::PeerNotExpected(id.clone())
            }
            mm_dsl::OpLifecycleRejectReasonKind::AlreadyPeerReady => {
                if status.is_none() {
                    return Err(OpsLifecycleError::Internal(
                        "generated op lifecycle authority emitted already-peer-ready without status"
                            .into(),
                    ));
                }
                OpsLifecycleError::AlreadyPeerReady(id.clone())
            }
        };
        rejection = Some(error);
    }
    rejection.ok_or_else(|| {
        OpsLifecycleError::Internal(
            "generated op lifecycle authority emitted no rejection result".into(),
        )
    })
}

fn classify_generated_op_rejection(
    state: &mut ShellState,
    err: mm_dsl::MeerkatMachineTransitionError,
    id: &OperationId,
    action: mm_dsl::OpLifecycleActionKind,
) -> OpsLifecycleError {
    match err {
        mm_dsl::MeerkatMachineTransitionError::GuardRejected { .. } => {
            match state.dsl_apply_with_effects(
                mm_dsl::MeerkatMachineInput::ResolveOpLifecycleTransitionRejection {
                    operation_id: mm_dsl::OperationId::from_domain(id).0,
                    action,
                },
                "ResolveOpLifecycleTransitionRejection",
            ) {
                Ok(effects) => op_lifecycle_rejection_error_from_effects(id, action, &effects)
                    .unwrap_or_else(|err| err),
                Err(err) => err,
            }
        }
        other => OpsLifecycleError::Internal(format!(
            "DSL rejected ops transition ({}): {other:?}",
            op_lifecycle_action_label(action)
        )),
    }
}

fn apply_op_transition(
    state: &mut ShellState,
    id: &OperationId,
    input: mm_dsl::MeerkatMachineInput,
    action: mm_dsl::OpLifecycleActionKind,
) -> Result<(), OpsLifecycleError> {
    state
        .dsl_apply_raw(input)
        .map_err(|err| classify_generated_op_rejection(state, err, id, action))
}

fn op_registration_error_from_effects(
    id: &OperationId,
    effects: &[mm_dsl::MeerkatMachineEffect],
) -> Result<Option<OpsLifecycleError>, OpsLifecycleError> {
    let expected_id = mm_dsl::OperationId::from_domain(id).0;
    let mut admission = None;
    for effect in effects {
        let mm_dsl::MeerkatMachineEffect::OpRegistrationAdmissionResolved {
            operation_id,
            result,
            reject_reason,
            max_concurrent_limit,
            active_op_count,
        } = effect
        else {
            continue;
        };
        if admission.is_some() {
            return Err(OpsLifecycleError::Internal(
                "generated op registration authority emitted multiple admission results".into(),
            ));
        }
        if operation_id != &expected_id {
            return Err(OpsLifecycleError::Internal(format!(
                "generated op registration authority resolved {operation_id} while shell requested {expected_id}"
            )));
        }
        admission = Some(match result {
            mm_dsl::OpRegistrationAdmissionResultKind::Accept => {
                if reject_reason.is_some() {
                    return Err(OpsLifecycleError::Internal(
                        "generated op registration authority accepted with rejection reason".into(),
                    ));
                }
                None
            }
            mm_dsl::OpRegistrationAdmissionResultKind::Reject => {
                let reason = reject_reason.ok_or_else(|| {
                    OpsLifecycleError::Internal(
                        "generated op registration authority rejected without reason".into(),
                    )
                })?;
                let error = match reason {
                    mm_dsl::OpRegistrationRejectReasonKind::AlreadyRegistered => {
                        OpsLifecycleError::AlreadyRegistered(id.clone())
                    }
                    mm_dsl::OpRegistrationRejectReasonKind::MaxConcurrentExceeded => {
                        let limit = max_concurrent_limit.ok_or_else(|| {
                            OpsLifecycleError::Internal(
                                "generated op registration authority rejected capacity without limit"
                                    .into(),
                            )
                        })?;
                        OpsLifecycleError::MaxConcurrentExceeded {
                            limit: limit as usize,
                            active: *active_op_count as usize,
                        }
                    }
                };
                Some(error)
            }
        });
    }
    admission.ok_or_else(|| {
        OpsLifecycleError::Internal(
            "generated op registration authority emitted no admission result".into(),
        )
    })
}

impl OpsLifecycleRegistry for RuntimeOpsLifecycleRegistry {
    fn register_operation(&self, spec: OperationSpec) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;
        let operation_id = spec.id.clone();
        let kind = spec.kind;
        let max_concurrent = state.max_concurrent.map(|limit| limit as u64);

        let effects = state.dsl_apply_with_effects(
            mm_dsl::MeerkatMachineInput::RegisterOp {
                operation_id: mm_dsl::OperationId::from_domain(&operation_id).0,
                kind: mm_dsl::OperationKind::from_domain(&kind),
                max_concurrent,
            },
            "RegisterOp",
        )?;
        if let Some(error) = op_registration_error_from_effects(&operation_id, &effects)? {
            return Err(error);
        }

        // Insert shell record.
        state.records.insert(operation_id, ShellRecord::new(spec));
        Ok(())
    }

    fn provisioning_succeeded(&self, id: &OperationId) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        apply_op_transition(
            &mut state,
            id,
            mm_dsl::MeerkatMachineInput::StartOp {
                operation_id: mm_dsl::OperationId::from_domain(id).0,
            },
            mm_dsl::OpLifecycleActionKind::Start,
        )?;

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

        let terminal_outcome = OperationTerminalOutcome::Failed { error };
        let (outcome_kind, outcome_payload) = ShellState::split_outcome(&terminal_outcome);

        apply_op_transition(
            &mut state,
            id,
            mm_dsl::MeerkatMachineInput::FailOp {
                operation_id: mm_dsl::OperationId::from_domain(id).0,
                outcome: outcome_kind,
                payload: outcome_payload,
            },
            mm_dsl::OpLifecycleActionKind::Fail,
        )?;

        state.finalize_terminal(id)?;
        state.maybe_persist()?;
        Ok(())
    }

    fn peer_ready(
        &self,
        id: &OperationId,
        peer: OperationPeerHandle,
    ) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        apply_op_transition(
            &mut state,
            id,
            mm_dsl::MeerkatMachineInput::PeerReadyOp {
                operation_id: mm_dsl::OperationId::from_domain(id).0,
            },
            mm_dsl::OpLifecycleActionKind::PeerReady,
        )?;

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

        apply_op_transition(
            &mut state,
            id,
            mm_dsl::MeerkatMachineInput::ProgressReportedOp {
                operation_id: mm_dsl::OperationId::from_domain(id).0,
            },
            mm_dsl::OpLifecycleActionKind::ProgressReported,
        )?;
        Ok(())
    }

    fn complete_operation(
        &self,
        id: &OperationId,
        result: OperationResult,
    ) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let terminal_outcome = OperationTerminalOutcome::Completed(result);
        let (outcome_kind, outcome_payload) = ShellState::split_outcome(&terminal_outcome);

        apply_op_transition(
            &mut state,
            id,
            mm_dsl::MeerkatMachineInput::CompleteOp {
                operation_id: mm_dsl::OperationId::from_domain(id).0,
                outcome: outcome_kind,
                payload: outcome_payload,
            },
            mm_dsl::OpLifecycleActionKind::Complete,
        )?;

        state.finalize_terminal(id)?;
        state.maybe_persist()?;
        Ok(())
    }

    fn fail_operation(&self, id: &OperationId, error: String) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let terminal_outcome = OperationTerminalOutcome::Failed { error };
        let (outcome_kind, outcome_payload) = ShellState::split_outcome(&terminal_outcome);

        apply_op_transition(
            &mut state,
            id,
            mm_dsl::MeerkatMachineInput::FailOp {
                operation_id: mm_dsl::OperationId::from_domain(id).0,
                outcome: outcome_kind,
                payload: outcome_payload,
            },
            mm_dsl::OpLifecycleActionKind::Fail,
        )?;

        state.finalize_terminal(id)?;
        state.maybe_persist()?;
        Ok(())
    }

    fn abort_provisioning(
        &self,
        id: &OperationId,
        reason: Option<String>,
    ) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let terminal_outcome = OperationTerminalOutcome::Aborted { reason };
        let (outcome_kind, outcome_payload) = ShellState::split_outcome(&terminal_outcome);

        apply_op_transition(
            &mut state,
            id,
            mm_dsl::MeerkatMachineInput::AbortOp {
                operation_id: mm_dsl::OperationId::from_domain(id).0,
                outcome: outcome_kind,
                payload: outcome_payload,
            },
            mm_dsl::OpLifecycleActionKind::Abort,
        )?;

        state.finalize_terminal(id)?;
        state.maybe_persist()?;
        Ok(())
    }

    fn cancel_operation(
        &self,
        id: &OperationId,
        reason: Option<String>,
    ) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let terminal_outcome = OperationTerminalOutcome::Cancelled { reason };
        let (outcome_kind, outcome_payload) = ShellState::split_outcome(&terminal_outcome);

        apply_op_transition(
            &mut state,
            id,
            mm_dsl::MeerkatMachineInput::CancelOp {
                operation_id: mm_dsl::OperationId::from_domain(id).0,
                outcome: outcome_kind,
                payload: outcome_payload,
            },
            mm_dsl::OpLifecycleActionKind::Cancel,
        )?;

        state.finalize_terminal(id)?;
        state.maybe_persist()?;
        Ok(())
    }

    fn request_retire(&self, id: &OperationId) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        apply_op_transition(
            &mut state,
            id,
            mm_dsl::MeerkatMachineInput::RetireRequestedOp {
                operation_id: mm_dsl::OperationId::from_domain(id).0,
            },
            mm_dsl::OpLifecycleActionKind::RetireRequested,
        )?;
        Ok(())
    }

    fn mark_retired(&self, id: &OperationId) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let terminal_outcome = OperationTerminalOutcome::Retired;
        let (outcome_kind, outcome_payload) = ShellState::split_outcome(&terminal_outcome);

        apply_op_transition(
            &mut state,
            id,
            mm_dsl::MeerkatMachineInput::RetireCompletedOp {
                operation_id: mm_dsl::OperationId::from_domain(id).0,
                outcome: outcome_kind,
                payload: outcome_payload,
            },
            mm_dsl::OpLifecycleActionKind::RetireCompleted,
        )?;

        state.finalize_terminal(id)?;
        state.maybe_persist()?;
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

        let to_terminate = state.owner_termination_targets()?;

        for (op_id, _status) in &to_terminate {
            let terminal_outcome = OperationTerminalOutcome::Terminated {
                reason: reason.clone(),
            };
            let (outcome_kind, outcome_payload) = ShellState::split_outcome(&terminal_outcome);

            apply_op_transition(
                &mut state,
                op_id,
                mm_dsl::MeerkatMachineInput::TerminateOp {
                    operation_id: mm_dsl::OperationId::from_domain(op_id).0,
                    outcome: outcome_kind,
                    payload: outcome_payload,
                },
                mm_dsl::OpLifecycleActionKind::Terminate,
            )?;

            state.finalize_terminal(op_id)?;
        }

        if !to_terminate.is_empty() {
            state.maybe_persist()?;
        }
        Ok(())
    }

    fn collect_completed(
        &self,
    ) -> Result<Vec<(OperationId, OperationTerminalOutcome)>, OpsLifecycleError> {
        let mut state = self.write_state()?;

        let ids: Vec<OperationId> = state.completed_order.iter().cloned().collect();
        let mut collected = Vec::with_capacity(ids.len());
        for id in ids {
            let outcome = state.terminal_outcome(&id);
            state.dsl_apply(
                mm_dsl::MeerkatMachineInput::CollectCompletedOp {
                    operation_id: mm_dsl::OperationId::from_domain(&id).0,
                },
                "CollectCompletedOp",
            )?;
            state.completed_order.retain(|queued| queued != &id);
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
                match state.begin_wait_all_authority(&wait_request_id, &owned_ids) {
                    Ok(WaitAllAuthorityPlan::AlreadySatisfied(satisfied)) => {
                        let outcomes =
                            state
                                .collect_wait_outcomes(&satisfied.operation_ids)
                                .map(|outcomes| WaitAllResult {
                                    outcomes,
                                    satisfied,
                                });
                        WaitAllFutureState::Ready(Some(outcomes))
                    }
                    Ok(WaitAllAuthorityPlan::ActivateBarrier) => {
                        if state.pending_wait.is_some() {
                            // Roll back the DSL barrier we just activated so the
                            // registry is not stuck in a wait-active state with
                            // no correlation oneshot to resolve. `CancelWaitAll`
                            // is the no-obligation clearer (members need not be
                            // terminal).
                            let rollback = state.dsl_apply(
                                mm_dsl::MeerkatMachineInput::CancelWaitAll,
                                "CancelWaitAll(rollback)",
                            );
                            return Box::pin(WaitAllFuture {
                                registry: self,
                                wait_request_id,
                                state: WaitAllFutureState::Ready(Some(Err(match rollback {
                                    Ok(()) => OpsLifecycleError::Internal(
                                        "wait_all started while a pending wait sender already existed"
                                            .into(),
                                    ),
                                    Err(err) => err,
                                }))),
                            });
                        }
                        state.wait_request_id = Some(wait_request_id.clone());
                        let (sender, receiver) = tokio::sync::oneshot::channel();
                        state.pending_wait = Some(PendingWaitState {
                            wait_request_id: wait_request_id.clone(),
                            sender,
                        });
                        WaitAllFutureState::Waiting(receiver)
                    }
                    Err(err) => WaitAllFutureState::Ready(Some(Err(err))),
                }
            }
            Err(err) => WaitAllFutureState::Ready(Some(Err(err))),
        };

        Box::pin(WaitAllFuture {
            registry: self,
            wait_request_id,
            state,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::comms::{PeerId, TrustedPeerDescriptor};
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
                peer_name: meerkat_core::comms::PeerName::new("peer").unwrap(),
                trusted_peer: TrustedPeerDescriptor::test_only_unsigned_typed(
                    "peer",
                    PeerId::new(),
                    "inproc://peer",
                )
                .unwrap(),
            },
        );
        assert!(matches!(result, Err(OpsLifecycleError::PeerNotExpected(_))));
    }

    #[test]
    fn duplicate_registration_rejection_is_generated() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let spec = background_spec("duplicate");
        let op_id = spec.id.clone();

        registry.register_operation(spec.clone()).unwrap();
        let result = registry.register_operation(spec);

        assert!(matches!(
            result,
            Err(OpsLifecycleError::AlreadyRegistered(id)) if id == op_id
        ));
    }

    #[test]
    fn invalid_transition_rejection_is_generated() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let spec = background_spec("invalid-transition");
        let op_id = spec.id.clone();
        registry.register_operation(spec).unwrap();

        let result = registry.complete_operation(
            &op_id,
            OperationResult {
                id: op_id.clone(),
                content: "too-early".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        );

        assert!(matches!(
            result,
            Err(OpsLifecycleError::InvalidTransition {
                id,
                status: OperationStatus::Provisioning,
                action: "complete_operation",
            }) if id == op_id
        ));
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
        let state = registry.read_state().unwrap();
        assert!(
            !state.wait_active(),
            "already-satisfied wait_all must be cleared by generated satisfaction authority"
        );
        assert!(state.wait_operation_ids().is_empty());
    }

    #[tokio::test]
    async fn wait_all_duplicate_rejection_is_generated() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let spec = background_spec("duplicate-wait");
        let op_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&op_id).unwrap();

        let result = registry
            .wait_all(&test_run_id(), &[op_id.clone(), op_id.clone()])
            .await;

        assert!(matches!(
            result,
            Err(OpsLifecycleError::DuplicateWaitOperation(id)) if id == op_id
        ));
        let state = registry.read_state().unwrap();
        assert!(
            !state.wait_active(),
            "duplicate wait rejection must not create a shell or machine barrier"
        );
        assert!(state.wait_operation_ids().is_empty());
    }

    #[tokio::test]
    async fn wait_all_active_rejection_is_generated() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let spec = background_spec("active-wait");
        let op_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&op_id).unwrap();

        let active_wait = registry.wait_all(&test_run_id(), std::slice::from_ref(&op_id));
        let result = registry
            .wait_all(&test_run_id(), std::slice::from_ref(&op_id))
            .await;

        assert!(matches!(result, Err(OpsLifecycleError::WaitAlreadyActive)));
        drop(active_wait);
        let state = registry.read_state().unwrap();
        assert!(!state.wait_active());
        assert!(state.wait_operation_ids().is_empty());
    }

    #[tokio::test]
    async fn wait_all_unknown_operation_rejection_is_generated() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let op_id = OperationId::new();

        let result = registry
            .wait_all(&test_run_id(), std::slice::from_ref(&op_id))
            .await;

        assert!(matches!(result, Err(OpsLifecycleError::NotFound(id)) if id == op_id));
        let state = registry.read_state().unwrap();
        assert!(!state.wait_active());
        assert!(state.wait_operation_ids().is_empty());
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
    fn terminate_owner_only_targets_non_terminal_operations() {
        let registry = RuntimeOpsLifecycleRegistry::new();

        let running_spec = background_spec("running");
        let running_id = running_spec.id.clone();
        registry.register_operation(running_spec).unwrap();
        registry.provisioning_succeeded(&running_id).unwrap();

        let completed_spec = background_spec("completed");
        let completed_id = completed_spec.id.clone();
        registry.register_operation(completed_spec).unwrap();
        registry.provisioning_succeeded(&completed_id).unwrap();
        registry
            .complete_operation(
                &completed_id,
                OperationResult {
                    id: completed_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .unwrap();

        registry.terminate_owner("shutdown".into()).unwrap();

        assert!(matches!(
            registry.snapshot(&running_id).unwrap().status,
            OperationStatus::Terminated
        ));
        assert!(matches!(
            registry.snapshot(&completed_id).unwrap().status,
            OperationStatus::Completed
        ));
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
    fn recovered_snapshot_retains_only_machine_accepted_terminal_records() {
        let registry = RuntimeOpsLifecycleRegistry::new();

        let completed_spec = background_spec("completed");
        let completed_id = completed_spec.id.clone();
        registry.register_operation(completed_spec).unwrap();
        registry.provisioning_succeeded(&completed_id).unwrap();
        registry
            .complete_operation(
                &completed_id,
                OperationResult {
                    id: completed_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .unwrap();

        let running_spec = background_spec("running");
        let running_id = running_spec.id.clone();
        registry.register_operation(running_spec).unwrap();
        registry.provisioning_succeeded(&running_id).unwrap();

        let cursor_state = meerkat_core::EpochCursorState::new();
        let snapshot = registry
            .capture_persistence_snapshot(meerkat_core::RuntimeEpochId::new(), &cursor_state);
        let recovered = RuntimeOpsLifecycleRegistry::from_recovered(snapshot).unwrap();

        assert!(recovered.snapshot(&completed_id).is_some());
        assert!(recovered.snapshot(&running_id).is_none());

        let collected = recovered.collect_completed().unwrap();
        assert_eq!(collected.len(), 1);
        assert_eq!(collected[0].0, completed_id);
    }

    #[test]
    fn recovered_snapshot_rejects_completion_feed_without_generated_record() {
        let registry = RuntimeOpsLifecycleRegistry::new();

        let running_spec = background_spec("running");
        let running_id = running_spec.id.clone();
        registry.register_operation(running_spec.clone()).unwrap();
        registry.provisioning_succeeded(&running_id).unwrap();

        let cursor_state = meerkat_core::EpochCursorState::new();
        let mut snapshot = registry
            .capture_persistence_snapshot(meerkat_core::RuntimeEpochId::new(), &cursor_state);
        snapshot.completion_entries.push(CompletionEntry {
            seq: 1,
            operation_id: running_id.clone(),
            kind: running_spec.kind,
            display_name: running_spec.display_name,
            terminal_outcome: OperationTerminalOutcome::Completed(OperationResult {
                id: running_id,
                content: "phantom".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            }),
            completed_at_ms: None,
        });

        let err = match RuntimeOpsLifecycleRegistry::from_recovered(snapshot) {
            Ok(_) => panic!("public completion feed must not recover without generated op truth"),
            Err(err) => err,
        };
        assert!(
            matches!(&err, OpsLifecycleError::Internal(message) if message.contains("ClassifyRecoveredOperationRecord")),
            "unexpected recovery error: {err:?}"
        );
    }

    #[test]
    fn recovered_completed_order_uses_generated_completion_sequences() {
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
                    content: "a".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .unwrap();

        let spec_b = background_spec("b");
        let id_b = spec_b.id.clone();
        registry.register_operation(spec_b).unwrap();
        registry.provisioning_succeeded(&id_b).unwrap();
        registry
            .complete_operation(
                &id_b,
                OperationResult {
                    id: id_b.clone(),
                    content: "b".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .unwrap();

        let cursor_state = meerkat_core::EpochCursorState::new();
        let mut snapshot = registry
            .capture_persistence_snapshot(meerkat_core::RuntimeEpochId::new(), &cursor_state);
        snapshot.authority_state.completed_order = VecDeque::from([id_b.clone(), id_a.clone()]);

        let recovered = RuntimeOpsLifecycleRegistry::from_recovered(snapshot).unwrap();
        let collected = recovered.collect_completed().unwrap();

        assert_eq!(collected[0].0, id_a);
        assert_eq!(collected[1].0, id_b);
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
            peer_name: meerkat_core::comms::PeerName::new("member-x").unwrap(),
            trusted_peer: TrustedPeerDescriptor::test_only_unsigned_typed(
                "member-x",
                PeerId::new(),
                "inproc://x",
            )
            .unwrap(),
        };
        registry.peer_ready(&op_id, handle).unwrap();

        let snap2 = registry.snapshot(&op_id).unwrap();
        assert_eq!(
            snap2.peer_handle.as_ref().unwrap().peer_name.as_str(),
            "member-x"
        );
    }
}

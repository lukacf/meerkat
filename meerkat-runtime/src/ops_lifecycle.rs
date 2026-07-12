//! In-memory runtime implementation of the shared async-operation lifecycle seam.
//!
//! Per-operation canonical lifecycle state lives in the MeerkatMachine DSL
//! authority (`op_statuses`, `op_terminal_outcomes`, `op_kinds`,
//! `op_sources`, `op_peer_ready`, `op_progress_counts`, `active_op_count`,
//! `wait_active`, `wait_operation_ids`). This shell layer owns pure mechanics: watcher
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
    CompletionCursorConsumer, DEFAULT_MAX_COMPLETED, OperationCompletionWakeClass,
    OperationCompletionWatch, OperationId, OperationKind, OperationLifecycleAction,
    OperationLifecycleSnapshot, OperationPeerHandle, OperationProgressUpdate,
    OperationPublicResultClass, OperationResult, OperationSource, OperationSpec, OperationStatus,
    OperationTerminalOutcome, OpsLifecycleError, OpsLifecycleRegistry, WaitAllResult,
    WaitAllSatisfied,
};
use meerkat_core::time_compat::{Instant, SystemTime, UNIX_EPOCH};
use meerkat_core::types::SessionId;

use crate::meerkat_machine::dsl as mm_dsl;

// ---------------------------------------------------------------------------
// Serde-only persisted canonical state shells
// ---------------------------------------------------------------------------
//
// These structures preserve the persisted fact shape of `PersistedOpsSnapshot`.
// They are pure serde shells — no methods beyond read-only field accessors,
// no authority behavior. Optional generated facts that can be explicitly
// absent still serialize as present `null` so recovery can distinguish
// generated "none" from a missing persisted fact.

fn deserialize_required_operation_source<'de, D>(
    deserializer: D,
) -> Result<Option<OperationSource>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    <Option<OperationSource> as serde::Deserialize>::deserialize(deserializer)
}

/// Canonical per-operation state as captured in persisted snapshots.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OperationCanonicalState {
    status: OperationStatus,
    kind: OperationKind,
    #[serde(deserialize_with = "deserialize_required_operation_source")]
    operation_source: Option<OperationSource>,
    peer_ready: bool,
    progress_count: u32,
    watcher_count: u32,
    terminal_outcome: Option<OperationTerminalOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    completion_sequence: Option<CompletionSeq>,
    terminal_buffered: bool,
}

/// Generated-owned public completion feed fact captured from DSL authority.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CompletionFeedCanonicalState {
    seq: CompletionSeq,
    kind: OperationKind,
    terminal_outcome: OperationTerminalOutcome,
}

/// Canonical registry-level state as captured in persisted snapshots.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RegistryCanonicalState {
    operations: HashMap<OperationId, OperationCanonicalState>,
    completion_feed_entries: HashMap<OperationId, CompletionFeedCanonicalState>,
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

    /// Number of generated-owned public completion feed entries captured.
    pub fn completion_feed_count(&self) -> usize {
        self.completion_feed_entries.len()
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
    /// Persisted completion feed projection metadata. Canonical feed truth is
    /// captured in `authority_state.completion_feed_entries`.
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

#[derive(Debug)]
struct OperationCompletionNotifier {
    tx: tokio::sync::oneshot::Sender<OperationTerminalOutcome>,
}

impl OperationCompletionNotifier {
    fn new(tx: tokio::sync::oneshot::Sender<OperationTerminalOutcome>) -> Self {
        Self { tx }
    }

    fn notify_after_generated_terminal(self, outcome: &OperationTerminalOutcome) {
        let _ = self.tx.send(outcome.clone());
    }
}

fn operation_completion_watch_from_receiver(
    rx: tokio::sync::oneshot::Receiver<OperationTerminalOutcome>,
) -> OperationCompletionWatch {
    Box::pin(async move {
        rx.await
            .map_err(|_| meerkat_core::ops_lifecycle::OperationCompletionWatchError::ChannelClosed)
    })
}

fn resolved_operation_completion_watch(
    outcome: OperationTerminalOutcome,
) -> OperationCompletionWatch {
    Box::pin(async move { Ok(outcome) })
}

/// Shell-owned data for a single operation. Canonical lifecycle state lives in
/// the DSL authority; this struct holds I/O concerns that the DSL has no
/// knowledge of.
#[derive(Debug)]
struct ShellRecord {
    spec: OperationSpec,
    peer_handle: Option<OperationPeerHandle>,
    /// Private waiter plumbing. Notifiers are drained only from
    /// `finalize_terminal()` after the generated authority has accepted and
    /// stored a terminal outcome.
    watchers: Vec<OperationCompletionNotifier>,
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
            watcher.notify_after_generated_terminal(outcome);
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
    /// Terminal unregister permanently fences any later channel replacement.
    persistence_sealed: bool,
    /// Epoch ID for persistence snapshots.
    persist_epoch_id: Option<meerkat_core::RuntimeEpochId>,
    /// Shared cursor state for persistence snapshots.
    persist_cursor_state: Option<Arc<meerkat_core::EpochCursorState>>,
    /// Mechanical epoch fence closed by canonical owner teardown.
    ///
    /// Generated authority terminalizes each known operation. This bit closes
    /// the shell admission door so detached callbacks that retained an Arc to
    /// the registry cannot create or mutate operations after unregister.
    owner_retired: bool,
}

/// Wrapper around the DSL authority that provides `Debug` output.
///
/// The generated `MeerkatMachineAuthority` does not derive `Debug`, but
/// `ShellState` requires it. This wrapper delegates to the inner state's
/// `Debug` impl.
struct DslAuthority(Box<mm_dsl::MeerkatMachineAuthority>);

impl std::fmt::Debug for DslAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DslAuthority")
            .field("state", self.0.state())
            .finish()
    }
}

/// Create a DSL authority initialized through generated authority. Per-op
/// transitions guard only on `op_statuses.contains_key(operation_id)`, so the
/// phase stays in `Idle` permanently (they all `to Idle`).
fn new_ops_dsl_authority() -> DslAuthority {
    DslAuthority(Box::new(
        crate::meerkat_machine::dsl_authority::new_initialized_authority(
            "ops lifecycle DSL Initialize must be accepted",
        ),
    ))
}

impl ShellState {
    fn new(max_completed: usize, max_concurrent: Option<usize>) -> Self {
        tracing::info!("RuntimeOpsLifecycleRegistry::ShellState creating dsl");
        let dsl = new_ops_dsl_authority();
        tracing::info!("RuntimeOpsLifecycleRegistry::ShellState created dsl");
        let feed_capacity = max_completed.saturating_mul(4).max(1024);
        tracing::info!(
            feed_capacity,
            "RuntimeOpsLifecycleRegistry::ShellState creating feed buffer"
        );
        let feed_buffer = Arc::new(FeedBuffer::new(feed_capacity));
        tracing::info!("RuntimeOpsLifecycleRegistry::ShellState created feed buffer");
        Self {
            dsl,
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
            feed_buffer,
            persist_tx: None,
            persistence_sealed: false,
            persist_epoch_id: None,
            persist_cursor_state: None,
            owner_retired: false,
        }
    }

    fn ensure_owner_active(&self) -> Result<(), OpsLifecycleError> {
        if self.owner_retired {
            Err(OpsLifecycleError::OwnerRetired)
        } else {
            Ok(())
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
        mm_dsl::MeerkatMachineMutator::apply(&mut *self.dsl.0, input).map(|_transition| ())
    }

    fn dsl_apply_with_effects(
        &mut self,
        input: mm_dsl::MeerkatMachineInput,
        context: &str,
    ) -> Result<Vec<mm_dsl::MeerkatMachineEffect>, OpsLifecycleError> {
        let transition =
            mm_dsl::MeerkatMachineMutator::apply(&mut *self.dsl.0, input).map_err(|err| {
                OpsLifecycleError::Internal(format!(
                    "DSL rejected ops transition ({context}): {err:?}"
                ))
            })?;
        Ok(transition.into_effects())
    }

    /// Fail-closed read of a typed terminal payload entry: the payload IS the
    /// domain [`OperationTerminalOutcome`] (K8b fold — no JSON codec), but the
    /// shell still refuses to surface a payload whose variant disagrees with
    /// the recorded discriminant.
    fn checked_terminal_payload(
        kind: mm_dsl::OperationTerminalOutcomeKind,
        payload: &OperationTerminalOutcome,
        authority: &str,
        operation_id: &str,
    ) -> Result<OperationTerminalOutcome, OpsLifecycleError> {
        if mm_dsl::OperationTerminalOutcomeKind::from(payload) != kind {
            return Err(OpsLifecycleError::Internal(format!(
                "{authority} payload variant for {operation_id} does not match terminal outcome discriminant"
            )));
        }
        Ok(payload.clone())
    }

    /// Read the DSL operation status for `id`, or `None` if not registered.
    fn status(&self, id: &OperationId) -> Option<OperationStatus> {
        let id_key = mm_dsl::OperationId::from_domain(id).0;
        self.dsl
            .0
            .state()
            .op_statuses
            .get(&id_key)
            .copied()
            .map(OperationStatus::from)
    }

    fn require_status(&self, id: &OperationId) -> Result<OperationStatus, OpsLifecycleError> {
        self.status(id).ok_or_else(|| {
            OpsLifecycleError::Internal(format!(
                "generated op lifecycle authority missing status for {id}"
            ))
        })
    }

    /// Read the DSL operation kind for `id`, or `None` if not registered.
    fn kind(&self, id: &OperationId) -> Option<OperationKind> {
        let id_key = mm_dsl::OperationId::from_domain(id).0;
        self.dsl
            .0
            .state()
            .op_kinds
            .get(&id_key)
            .copied()
            .map(OperationKind::from)
    }

    fn require_kind(&self, id: &OperationId) -> Result<OperationKind, OpsLifecycleError> {
        self.kind(id).ok_or_else(|| {
            OpsLifecycleError::Internal(format!(
                "generated op lifecycle authority missing kind for {id}"
            ))
        })
    }

    fn operation_source(
        &self,
        id: &OperationId,
    ) -> Result<Option<OperationSource>, OpsLifecycleError> {
        let id_key = mm_dsl::OperationId::from_domain(id).0;
        self.dsl
            .0
            .state()
            .op_sources
            .get(&id_key)
            .map(|source| {
                source.to_domain().map_err(|error| {
                    OpsLifecycleError::Internal(format!(
                        "generated operation source authority has invalid source for {id}: {error}"
                    ))
                })
            })
            .transpose()
    }

    fn child_session_id_from_operation_source(
        operation_source: Option<&OperationSource>,
    ) -> Option<SessionId> {
        match operation_source {
            Some(OperationSource::SessionChild { session_id }) => Some(session_id.clone()),
            Some(OperationSource::BackendPeer { .. }) | None => None,
        }
    }

    fn align_spec_child_session_id_to_source(
        spec: &mut OperationSpec,
        operation_source: Option<&OperationSource>,
    ) {
        spec.child_session_id = Self::child_session_id_from_operation_source(operation_source);
    }

    /// Read the peer-ready flag for `id`.
    fn peer_ready(&self, id: &OperationId) -> Option<bool> {
        let id_key = mm_dsl::OperationId::from_domain(id).0;
        self.dsl.0.state().op_peer_ready.get(&id_key).copied()
    }

    fn require_peer_ready(&self, id: &OperationId) -> Result<bool, OpsLifecycleError> {
        self.peer_ready(id).ok_or_else(|| {
            OpsLifecycleError::Internal(format!(
                "generated op peer wiring authority missing peer-ready fact for {id}"
            ))
        })
    }

    /// Read the progress counter for `id`.
    fn progress_count(&self, id: &OperationId) -> Option<u32> {
        let id_key = mm_dsl::OperationId::from_domain(id).0;
        self.dsl
            .0
            .state()
            .op_progress_counts
            .get(&id_key)
            .map(|v| (*v).min(u32::MAX as u64) as u32)
    }

    fn require_progress_count(&self, id: &OperationId) -> Result<u32, OpsLifecycleError> {
        self.progress_count(id).ok_or_else(|| {
            OpsLifecycleError::Internal(format!(
                "generated op progress authority missing progress count for {id}"
            ))
        })
    }

    /// Read the terminal outcome for `id` by pairing the DSL's typed
    /// discriminant with the companion payload JSON. Returns `None` when the
    /// op has no recorded terminal discriminant.
    fn terminal_outcome(
        &self,
        id: &OperationId,
    ) -> Result<Option<OperationTerminalOutcome>, OpsLifecycleError> {
        let id_key = mm_dsl::OperationId::from_domain(id).0;
        let state = self.dsl.0.state();
        let status = self.status(id);
        let terminal = match status {
            Some(status) => Self::operation_status_is_terminal(id, status)?,
            None => false,
        };
        let kind = state.op_terminal_outcomes.get(&id_key).copied();
        let Some(kind) = kind else {
            if terminal {
                return Err(OpsLifecycleError::Internal(format!(
                    "generated op terminal authority missing terminal outcome for {id}"
                )));
            }
            return Ok(None);
        };
        if !terminal {
            return Err(OpsLifecycleError::Internal(format!(
                "generated op terminal authority has terminal outcome for non-terminal {id}"
            )));
        }
        let payload = state.op_terminal_payload.get(&id_key).ok_or_else(|| {
            OpsLifecycleError::Internal(format!(
                "generated op terminal authority missing terminal payload for {id}"
            ))
        })?;
        Self::checked_terminal_payload(kind, payload, "generated op terminal authority", &id_key)
            .map(Some)
    }

    /// Whether the operation is currently tracked in DSL state.
    fn contains(&self, id: &OperationId) -> bool {
        let id_key = mm_dsl::OperationId::from_domain(id).0;
        self.dsl.0.state().op_statuses.contains_key(&id_key)
    }

    /// Number of non-terminal operations (derived from DSL state).
    fn active_count(&self) -> usize {
        self.dsl.0.state().active_op_count as usize
    }

    /// Number of operations currently tracked (including terminal).
    fn operation_count(&self) -> usize {
        self.dsl.0.state().op_statuses.len()
    }

    /// Iterate over all tracked operation IDs (DSL keys converted to domain).
    fn operation_ids(&self) -> Result<Vec<OperationId>, OpsLifecycleError> {
        let mut ids = BTreeSet::new();
        let state = self.dsl.0.state();
        Self::collect_operation_id_keys(&mut ids, "op_statuses", state.op_statuses.keys())?;
        Self::collect_operation_id_keys(&mut ids, "op_kinds", state.op_kinds.keys())?;
        Self::collect_operation_id_keys(&mut ids, "op_sources", state.op_sources.keys())?;
        Self::collect_operation_id_keys(&mut ids, "op_peer_ready", state.op_peer_ready.keys())?;
        Self::collect_operation_id_keys(
            &mut ids,
            "op_progress_counts",
            state.op_progress_counts.keys(),
        )?;
        Self::collect_operation_id_keys(
            &mut ids,
            "op_terminal_outcomes",
            state.op_terminal_outcomes.keys(),
        )?;
        Self::collect_operation_id_keys(
            &mut ids,
            "op_terminal_payload",
            state.op_terminal_payload.keys(),
        )?;
        Self::collect_operation_id_keys(
            &mut ids,
            "op_completion_seq",
            state.op_completion_seq.keys(),
        )?;
        ids.extend(self.records.keys().cloned());
        Ok(ids.into_iter().collect())
    }

    fn collect_operation_id_keys<'a, I>(
        ids: &mut BTreeSet<OperationId>,
        field: &str,
        keys: I,
    ) -> Result<(), OpsLifecycleError>
    where
        I: IntoIterator<Item = &'a String>,
    {
        for key in keys {
            let id = serde_json::from_str::<OperationId>(key).map_err(|error| {
                OpsLifecycleError::Internal(format!(
                    "generated operation identity authority used invalid operation id key in {field}: {key}: {error}"
                ))
            })?;
            ids.insert(id);
        }
        Ok(())
    }

    fn has_generated_operation_record_fact(&self, id: &OperationId) -> bool {
        let id_key = mm_dsl::OperationId::from_domain(id).0;
        let state = self.dsl.0.state();
        state.op_statuses.contains_key(&id_key)
            || state.op_kinds.contains_key(&id_key)
            || state.op_sources.contains_key(&id_key)
            || state.op_peer_ready.contains_key(&id_key)
            || state.op_progress_counts.contains_key(&id_key)
            || state.op_terminal_outcomes.contains_key(&id_key)
            || state.op_terminal_payload.contains_key(&id_key)
            || state.op_completion_seq.contains_key(&id_key)
    }

    /// Read the DSL-minted completion sequence for a terminal operation.
    fn completion_sequence(&self, id: &OperationId) -> Option<CompletionSeq> {
        let id_key = mm_dsl::OperationId::from_domain(id).0;
        self.dsl.0.state().op_completion_seq.get(&id_key).copied()
    }

    fn completion_feed_authority_entries(
        &self,
    ) -> Result<HashMap<OperationId, CompletionFeedCanonicalState>, OpsLifecycleError> {
        let state = self.dsl.0.state();
        let sequence_keys: BTreeSet<String> =
            state.completion_feed_sequences.keys().cloned().collect();
        let companion_domains: [(&str, BTreeSet<String>); 3] = [
            (
                "completion_feed_kinds",
                state.completion_feed_kinds.keys().cloned().collect(),
            ),
            (
                "completion_feed_terminal_outcomes",
                state
                    .completion_feed_terminal_outcomes
                    .keys()
                    .cloned()
                    .collect(),
            ),
            (
                "completion_feed_terminal_payload",
                state
                    .completion_feed_terminal_payload
                    .keys()
                    .cloned()
                    .collect(),
            ),
        ];
        for (field, keys) in companion_domains {
            if keys != sequence_keys {
                return Err(OpsLifecycleError::Internal(format!(
                    "generated completion feed authority has mismatched {field} domain"
                )));
            }
        }

        let mut entries = HashMap::new();
        for (id_key, seq) in &state.completion_feed_sequences {
            if !state.completion_sequence_claims.contains(seq) {
                return Err(OpsLifecycleError::Internal(format!(
                    "generated completion feed authority sequence {seq} for {id_key} is not claimed"
                )));
            }
            let operation_id = serde_json::from_str::<OperationId>(id_key).map_err(|error| {
                OpsLifecycleError::Internal(format!(
                    "generated completion feed authority used invalid operation id key {id_key}: {error}"
                ))
            })?;
            let kind = state
                .completion_feed_kinds
                .get(id_key)
                .copied()
                .map(OperationKind::from)
                .ok_or_else(|| {
                    OpsLifecycleError::Internal(format!(
                        "generated completion feed authority missing kind for {id_key}"
                    ))
                })?;
            let outcome_kind = state
                .completion_feed_terminal_outcomes
                .get(id_key)
                .copied()
                .ok_or_else(|| {
                    OpsLifecycleError::Internal(format!(
                        "generated completion feed authority missing terminal outcome for {id_key}"
                    ))
                })?;
            let payload = state
                .completion_feed_terminal_payload
                .get(id_key)
                .ok_or_else(|| {
                    OpsLifecycleError::Internal(format!(
                        "generated completion feed authority missing terminal payload for {id_key}"
                    ))
                })?;
            let terminal_outcome = Self::checked_terminal_payload(
                outcome_kind,
                payload,
                "generated completion feed authority",
                id_key,
            )?;
            entries.insert(
                operation_id,
                CompletionFeedCanonicalState {
                    seq: *seq,
                    kind,
                    terminal_outcome,
                },
            );
        }
        Ok(entries)
    }

    fn completion_cursor(&self, consumer: CompletionCursorConsumer) -> CompletionSeq {
        let state = self.dsl.0.state();
        match consumer {
            CompletionCursorConsumer::AgentApplied => state.completion_agent_applied_cursor,
            CompletionCursorConsumer::RuntimeObserved => state.completion_runtime_observed_cursor,
            CompletionCursorConsumer::RuntimeInjected => state.completion_runtime_injected_cursor,
        }
    }

    fn completion_cursor_snapshot(&self) -> meerkat_core::EpochCursorSnapshot {
        meerkat_core::EpochCursorSnapshot {
            agent_applied_cursor: self.completion_cursor(CompletionCursorConsumer::AgentApplied),
            runtime_observed_seq: self.completion_cursor(CompletionCursorConsumer::RuntimeObserved),
            runtime_last_injected_seq: self
                .completion_cursor(CompletionCursorConsumer::RuntimeInjected),
        }
    }

    /// Build a snapshot from DSL state + shell record.
    fn snapshot(
        &self,
        id: &OperationId,
    ) -> Result<Option<OperationLifecycleSnapshot>, OpsLifecycleError> {
        let Some(shell) = self.records.get(id) else {
            if self.has_generated_operation_record_fact(id) {
                return Err(OpsLifecycleError::Internal(format!(
                    "generated op lifecycle authority has operation facts without shell projection record for {id}"
                )));
            }
            return Ok(None);
        };
        let kind = self.require_kind(id)?;
        let status = self.require_status(id)?;
        let terminal = Self::operation_status_is_terminal(id, status)?;
        let public_result_class = Self::operation_public_result_class(id, status)?;
        let peer_ready = self.require_peer_ready(id)?;
        let progress_count = self.require_progress_count(id)?;
        let operation_source = self.operation_source(id)?;
        let terminal_outcome = self.terminal_outcome(id)?;

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

        Ok(Some(OperationLifecycleSnapshot {
            id: shell.spec.id.clone(),
            kind,
            owner_session_id: shell.spec.owner_session_id.clone(),
            display_name: shell.spec.display_name.clone(),
            source_label: shell.spec.source_label.clone(),
            child_session_id: Self::child_session_id_from_operation_source(
                operation_source.as_ref(),
            ),
            operation_source,
            expect_peer_channel: shell.spec.expect_peer_channel,
            status,
            terminal,
            public_result_class,
            peer_ready,
            progress_count,
            watcher_count: shell.watchers.len() as u32,
            terminal_outcome,
            peer_handle: shell.peer_handle.clone(),
            created_at_ms,
            started_at_ms,
            completed_at_ms,
            elapsed_ms,
        }))
    }

    /// Emit shell-side mechanics for a terminal transition: notify watchers,
    /// push generated-authorized CompletionEntry rows, retain in FIFO, evict
    /// as needed. Called AFTER the DSL transition has already persisted the
    /// terminal status + outcome.
    fn finalize_terminal(
        &mut self,
        id: &OperationId,
    ) -> Result<Option<CompletionEntry>, OpsLifecycleError> {
        let outcome = self.terminal_outcome(id)?.ok_or_else(|| {
            OpsLifecycleError::Internal(format!(
                "generated op terminal transition did not mint terminal outcome for {id}"
            ))
        })?;
        let kind = self.require_kind(id)?;

        // Notify watchers and mark completion timestamp.
        if let Some(shell) = self.records.get_mut(id) {
            shell.notify_watchers(&outcome);
            shell.mark_completed();
        }

        if Self::operation_durability_class(id, kind)? == mm_dsl::OperationDurabilityClass::Discard
        {
            self.dsl_apply(
                mm_dsl::MeerkatMachineInput::CollectCompletedOp {
                    operation_id: mm_dsl::OperationId::from_domain(id).0,
                },
                "CollectCompletedOp",
            )?;
            self.records.remove(id);
            self.completed_order.retain(|queued| queued != id);
            return Ok(None);
        }

        let mut completion_entry = None;
        if Self::operation_completion_feed_class(id, kind)?
            == mm_dsl::OperationCompletionFeedClass::Emit
        {
            let feed_authority = self
                .completion_feed_authority_entries()?
                .remove(id)
                .ok_or_else(|| {
                    OpsLifecycleError::Internal(format!(
                        "generated op terminal transition did not mint completion feed authority for {id}"
                    ))
                })?;
            if feed_authority.kind != kind || feed_authority.terminal_outcome != outcome {
                return Err(OpsLifecycleError::Internal(format!(
                    "generated completion feed authority drifted from terminal op authority for {id}"
                )));
            }
            let seq = self.completion_sequence(id).ok_or_else(|| {
                OpsLifecycleError::Internal(format!(
                    "generated op terminal transition did not mint completion sequence for {id}"
                ))
            })?;
            if feed_authority.seq != seq {
                return Err(OpsLifecycleError::Internal(format!(
                    "generated completion feed authority sequence drifted for {id}"
                )));
            }
            let display_name = self
                .records
                .get(id)
                .map(|r| r.spec.display_name.clone())
                .unwrap_or_default();
            let completed_at_ms = self
                .records
                .get(id)
                .and_then(|r| r.completed_at.map(|i| r.epoch_millis_for_instant(i)));
            completion_entry = Some(CompletionEntry {
                seq: feed_authority.seq,
                operation_id: id.clone(),
                kind: feed_authority.kind,
                display_name,
                terminal_outcome: feed_authority.terminal_outcome,
                completed_at_ms,
            });
        }

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

        // Satisfy a pending wait request if all its ops are now terminal. On
        // authority-invariant corruption this propagates the typed fault (and
        // drops the barrier oneshot so the waiter resolves to Err) instead of
        // reporting the op terminal with a silently-hung barrier.
        self.maybe_satisfy_wait()?;
        Ok(completion_entry)
    }

    fn publish_completion_entry(&self, entry: Option<CompletionEntry>) {
        if let Some(entry) = entry {
            self.feed_buffer.push(entry);
        }
    }

    /// Read barrier membership from DSL state (sole owner).
    fn wait_operation_ids(&self) -> Result<Vec<OperationId>, OpsLifecycleError> {
        self.dsl
            .0
            .state()
            .wait_operation_ids
            .iter()
            .map(|key| {
                serde_json::from_str::<OperationId>(key).map_err(|error| {
                    OpsLifecycleError::Internal(format!(
                        "generated wait operation identity authority used invalid operation id key {key}: {error}"
                    ))
                })
            })
            .collect()
    }

    /// Whether the DSL has a barrier wait active.
    #[cfg(test)]
    fn wait_active(&self) -> bool {
        self.dsl.0.state().wait_active
    }

    fn wait_all_satisfied_from_effects(
        effects: &[mm_dsl::MeerkatMachineEffect],
    ) -> Result<Option<WaitAllSatisfied>, OpsLifecycleError> {
        let mut satisfied = None;
        for effect in effects {
            let mm_dsl::MeerkatMachineEffect::WaitAllSatisfied {
                wait_request_id,
                run_id,
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
                run_id: RunId::from_uuid(uuid::Uuid::parse_str(&run_id.0).map_err(|err| {
                    OpsLifecycleError::Internal(format!(
                        "generated wait_all authority emitted invalid run id '{}': {err}",
                        run_id.0
                    ))
                })?),
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
        let Some(dsl_wait_request_id) = self.dsl.0.state().wait_request_id.clone() else {
            return Ok(None);
        };
        let Some(dsl_run_id) = self.dsl.0.state().wait_run_id.clone() else {
            return Err(OpsLifecycleError::Internal(
                "generated wait_all authority has active wait without run id".into(),
            ));
        };
        let dsl_operation_id_tokens = self.dsl.0.state().wait_operation_id_tokens.clone();
        let transition = match mm_dsl::MeerkatMachineMutator::apply(
            &mut *self.dsl.0,
            mm_dsl::MeerkatMachineInput::SatisfyWaitAll {
                wait_request_id: dsl_wait_request_id,
                run_id: dsl_run_id,
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
        Self::wait_all_satisfied_from_effects(transition.effects())?
            .ok_or_else(|| {
                OpsLifecycleError::Internal(
                    "generated wait_all authority accepted satisfaction without effect".into(),
                )
            })
            .map(Some)
    }

    fn begin_wait_all_authority(
        &mut self,
        run_id: &RunId,
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
                run_id: mm_dsl::RunId::from_domain(run_id),
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
        for id in self.operation_ids()? {
            let status = self.require_status(&id)?;
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
                    terminal = Some(true);
                }
                mm_dsl::MeerkatMachineEffect::OperationNonTerminal { operation_id }
                    if operation_id == operation_id_key =>
                {
                    terminal = Some(false);
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

    fn operation_public_result_class(
        operation_id: &OperationId,
        status: OperationStatus,
    ) -> Result<OperationPublicResultClass, OpsLifecycleError> {
        let operation_id_key = mm_dsl::OperationId::from_domain(operation_id).0;
        let effects = Self::apply_stateless_classifier(
            mm_dsl::MeerkatMachineInput::ClassifyOperationPublicResult {
                operation_id: operation_id_key.clone(),
                status: mm_dsl::OperationStatus::from(status),
            },
            "ClassifyOperationPublicResult",
        )?;
        let mut result = None;
        for effect in effects {
            match effect {
                mm_dsl::MeerkatMachineEffect::OperationPublicResultClassified {
                    operation_id,
                    result: classified,
                } if operation_id == operation_id_key => {
                    result = Some(OperationPublicResultClass::from(classified));
                }
                other => {
                    return Err(OpsLifecycleError::Internal(format!(
                        "unexpected generated operation public-result effect: {other:?}"
                    )));
                }
            }
        }
        result.ok_or_else(|| {
            OpsLifecycleError::Internal(format!(
                "generated operation public-result authority emitted no effect for {operation_id}"
            ))
        })
    }

    fn operation_transition_rejection_is_idempotent(
        operation_id: &OperationId,
        action: OperationLifecycleAction,
        status: OperationStatus,
    ) -> Result<bool, OpsLifecycleError> {
        let operation_id_key = mm_dsl::OperationId::from_domain(operation_id).0;
        let action = mm_dsl::OpLifecycleActionKind::from(action);
        let status = mm_dsl::OperationStatus::from(status);
        let effects = Self::apply_stateless_classifier(
            mm_dsl::MeerkatMachineInput::ClassifyOperationTransitionIdempotence {
                operation_id: operation_id_key.clone(),
                action,
                status,
            },
            "ClassifyOperationTransitionIdempotence",
        )?;
        let mut idempotent = None;
        for effect in effects {
            match effect {
                mm_dsl::MeerkatMachineEffect::OperationTransitionIdempotentSuccess {
                    operation_id,
                    action: effect_action,
                    status: effect_status,
                } if operation_id == operation_id_key
                    && effect_action == action
                    && effect_status == status =>
                {
                    idempotent = Some(true);
                }
                mm_dsl::MeerkatMachineEffect::OperationTransitionNotIdempotent {
                    operation_id,
                    action: effect_action,
                    status: effect_status,
                } if operation_id == operation_id_key
                    && effect_action == action
                    && effect_status == status =>
                {
                    idempotent = Some(false);
                }
                other => {
                    return Err(OpsLifecycleError::Internal(format!(
                        "unexpected generated operation transition-idempotence effect: {other:?}"
                    )));
                }
            }
        }
        idempotent.ok_or_else(|| {
            OpsLifecycleError::Internal(format!(
                "generated operation transition-idempotence authority emitted no effect for {operation_id}"
            ))
        })
    }

    fn operation_completion_feed_class(
        operation_id: &OperationId,
        kind: OperationKind,
    ) -> Result<mm_dsl::OperationCompletionFeedClass, OpsLifecycleError> {
        let operation_id_key = mm_dsl::OperationId::from_domain(operation_id).0;
        let kind = mm_dsl::OperationKind::from(kind);
        let effects = Self::apply_stateless_classifier(
            mm_dsl::MeerkatMachineInput::ClassifyOperationCompletionFeed {
                operation_id: operation_id_key.clone(),
                kind,
            },
            "ClassifyOperationCompletionFeed",
        )?;
        let mut class = None;
        for effect in effects {
            match effect {
                mm_dsl::MeerkatMachineEffect::OperationCompletionFeedClassified {
                    operation_id,
                    result,
                } if operation_id == operation_id_key => {
                    class = Some(result);
                }
                other => {
                    return Err(OpsLifecycleError::Internal(format!(
                        "unexpected generated operation completion-feed effect: {other:?}"
                    )));
                }
            }
        }
        class.ok_or_else(|| {
            OpsLifecycleError::Internal(format!(
                "generated operation completion-feed authority emitted no effect for {operation_id}"
            ))
        })
    }

    fn operation_completion_wake_class(
        operation_id: &OperationId,
        kind: OperationKind,
    ) -> Result<OperationCompletionWakeClass, OpsLifecycleError> {
        let operation_id_key = mm_dsl::OperationId::from_domain(operation_id).0;
        let kind = mm_dsl::OperationKind::from(kind);
        let effects = Self::apply_stateless_classifier(
            mm_dsl::MeerkatMachineInput::ClassifyOperationCompletionWake {
                operation_id: operation_id_key.clone(),
                kind,
            },
            "ClassifyOperationCompletionWake",
        )?;
        let mut class = None;
        for effect in effects {
            match effect {
                mm_dsl::MeerkatMachineEffect::OperationCompletionWakeClassified {
                    operation_id,
                    result,
                } if operation_id == operation_id_key => {
                    class = Some(OperationCompletionWakeClass::from(result));
                }
                other => {
                    return Err(OpsLifecycleError::Internal(format!(
                        "unexpected generated operation completion-wake effect: {other:?}"
                    )));
                }
            }
        }
        class.ok_or_else(|| {
            OpsLifecycleError::Internal(format!(
                "generated operation completion-wake authority emitted no effect for {operation_id}"
            ))
        })
    }

    fn operation_durability_class(
        operation_id: &OperationId,
        kind: OperationKind,
    ) -> Result<mm_dsl::OperationDurabilityClass, OpsLifecycleError> {
        let operation_id_key = mm_dsl::OperationId::from_domain(operation_id).0;
        let kind = mm_dsl::OperationKind::from(kind);
        let effects = Self::apply_stateless_classifier(
            mm_dsl::MeerkatMachineInput::ClassifyOperationDurability {
                operation_id: operation_id_key.clone(),
                kind,
            },
            "ClassifyOperationDurability",
        )?;
        let mut class = None;
        for effect in effects {
            match effect {
                mm_dsl::MeerkatMachineEffect::OperationDurabilityClassified {
                    operation_id,
                    result,
                } if operation_id == operation_id_key => {
                    class = Some(result);
                }
                other => {
                    return Err(OpsLifecycleError::Internal(format!(
                        "unexpected generated operation durability effect: {other:?}"
                    )));
                }
            }
        }
        class.ok_or_else(|| {
            OpsLifecycleError::Internal(format!(
                "generated operation durability authority emitted no effect for {operation_id}"
            ))
        })
    }

    fn recovered_operation_record_disposition(
        operation_id: &OperationId,
        status: OperationStatus,
        kind: OperationKind,
        terminal_outcome_present: bool,
        terminal_payload_present: bool,
        completion_sequence_present: bool,
    ) -> Result<RecoveredOperationRecordDisposition, OpsLifecycleError> {
        let operation_id_key = mm_dsl::OperationId::from_domain(operation_id).0;
        let effects = Self::apply_stateless_classifier(
            mm_dsl::MeerkatMachineInput::ClassifyRecoveredOperationRecord {
                operation_id: operation_id_key.clone(),
                status: mm_dsl::OperationStatus::from(status),
                kind: mm_dsl::OperationKind::from(kind),
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
                    disposition = Some(RecoveredOperationRecordDisposition::Retain);
                }
                mm_dsl::MeerkatMachineEffect::DiscardRecoveredOperationRecord { operation_id }
                    if operation_id == operation_id_key =>
                {
                    disposition = Some(RecoveredOperationRecordDisposition::Discard);
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
        let mut authority = crate::meerkat_machine::dsl_authority::new_initialized_authority(
            "ops stateless classifier Initialize must be accepted",
        );
        let transition =
            mm_dsl::MeerkatMachineMutator::apply(&mut authority, input).map_err(|err| {
                OpsLifecycleError::Internal(format!(
                    "DSL rejected ops transition ({label}): {err:?}"
                ))
            })?;
        Ok(transition.into_effects())
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
    /// the oneshot simply remains pending. That benign no-correlation case is
    /// the only path that leaves the oneshot pending: on authority-invariant
    /// corruption the pending sender is dropped so the waiter's
    /// `WaitAllFuture` resolves to `Err` and the typed
    /// [`OpsLifecycleError`] propagates to the terminal transition caller
    /// instead of reporting the op complete with a silently-hung barrier.
    fn maybe_satisfy_wait(&mut self) -> Result<(), OpsLifecycleError> {
        let satisfied = match self.try_satisfy_wait_all_authority() {
            Ok(Some(satisfied)) => satisfied,
            Ok(None) => return Ok(()),
            Err(err) => {
                // Authority-invariant corruption: do NOT report the op complete
                // with a silently-pending barrier oneshot. Drop the sender so
                // the waiter's `WaitAllFuture` resolves to `Err` via its
                // `Poll::Ready(Err(_))` arm (the same mechanism
                // `cancel_wait_all_internal` uses), then propagate the typed
                // fault. The invariant is already broken, so we do not attempt
                // a `CancelWaitAll` rollback on the corrupt machine.
                if let Some(pending) = self.pending_wait.take() {
                    drop(pending.sender);
                }
                self.wait_request_id = None;
                return Err(err);
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
        Ok(())
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

        let snapshot = self.capture_snapshot(epoch_id.clone(), cursor_state)?;
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
        _cursor_state: &meerkat_core::EpochCursorState,
    ) -> Result<PersistedOpsSnapshot, OpsLifecycleError> {
        let mut operations: HashMap<OperationId, OperationCanonicalState> = HashMap::new();
        for op_id in self.operation_ids()? {
            let status = self.require_status(&op_id)?;
            let kind = self.require_kind(&op_id)?;
            if Self::operation_durability_class(&op_id, kind)?
                != mm_dsl::OperationDurabilityClass::Retain
            {
                continue;
            }
            let peer_ready = self.require_peer_ready(&op_id)?;
            let progress_count = self.require_progress_count(&op_id)?;
            let operation_source = self.operation_source(&op_id)?;
            let terminal_outcome = self.terminal_outcome(&op_id)?;
            let completion_sequence = self.completion_sequence(&op_id);
            if terminal_outcome.is_some() && completion_sequence.is_none() {
                return Err(OpsLifecycleError::Internal(format!(
                    "generated op terminal authority missing completion sequence for retained terminal {op_id}"
                )));
            }
            if terminal_outcome.is_none() && completion_sequence.is_some() {
                return Err(OpsLifecycleError::Internal(format!(
                    "generated op terminal authority has completion sequence for non-terminal {op_id}"
                )));
            }
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
                    operation_source,
                    peer_ready,
                    progress_count,
                    watcher_count,
                    terminal_outcome,
                    completion_sequence,
                    terminal_buffered,
                },
            );
        }
        let operation_specs: HashMap<OperationId, OperationSpec> = self
            .records
            .iter()
            .filter(|(id, _)| operations.contains_key(*id))
            .map(|(id, record)| {
                let mut spec = record.spec.clone();
                let operation_source = operations
                    .get(id)
                    .and_then(|state| state.operation_source.as_ref());
                Self::align_spec_child_session_id_to_source(&mut spec, operation_source);
                (id.clone(), spec)
            })
            .collect();
        let completed_order: VecDeque<OperationId> = self
            .completed_order
            .iter()
            .filter(|id| operations.contains_key(*id))
            .cloned()
            .collect();
        let active_count = operations
            .iter()
            .filter(|(id, state)| {
                matches!(
                    Self::operation_status_is_terminal(id, state.status),
                    Ok(false)
                )
            })
            .count();
        let authority_completion_entries = self.completion_feed_authority_entries()?;
        let published_completion_entries_by_id: HashMap<OperationId, CompletionEntry> = {
            let inner = self
                .feed_buffer
                .inner
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut entries = HashMap::new();
            for entry in &inner.entries {
                if !authority_completion_entries.contains_key(&entry.operation_id) {
                    return Err(OpsLifecycleError::Internal(format!(
                        "public completion feed projection for {} has no generated authority",
                        entry.operation_id
                    )));
                }
                if entries
                    .insert(entry.operation_id.clone(), entry.clone())
                    .is_some()
                {
                    return Err(OpsLifecycleError::Internal(format!(
                        "public completion feed projection for {} appeared more than once",
                        entry.operation_id
                    )));
                }
            }
            entries
        };
        let mut completion_entries: Vec<CompletionEntry> = authority_completion_entries
            .iter()
            .map(|(operation_id, authority_entry)| {
                if let Some(projection) = published_completion_entries_by_id.get(operation_id) {
                    if projection.seq != authority_entry.seq
                        || projection.kind != authority_entry.kind
                        || projection.terminal_outcome != authority_entry.terminal_outcome
                    {
                        return Err(OpsLifecycleError::Internal(format!(
                            "public completion feed projection for {operation_id} drifted from generated authority"
                        )));
                    }
                    return Ok(projection.clone());
                }

                let display_name = self
                    .records
                    .get(operation_id)
                    .map(|record| record.spec.display_name.clone())
                    .unwrap_or_default();
                let completed_at_ms = self.records.get(operation_id).and_then(|record| {
                    record
                        .completed_at
                        .map(|completed_at| record.epoch_millis_for_instant(completed_at))
                });

                Ok(CompletionEntry {
                    seq: authority_entry.seq,
                    operation_id: operation_id.clone(),
                    kind: authority_entry.kind,
                    display_name,
                    terminal_outcome: authority_entry.terminal_outcome.clone(),
                    completed_at_ms,
                })
            })
            .collect::<Result<_, _>>()?;
        completion_entries.sort_by_key(|entry| entry.seq);

        let authority_state = RegistryCanonicalState {
            operations,
            completion_feed_entries: authority_completion_entries,
            completed_order,
            max_completed: self.max_completed,
            max_concurrent: self.max_concurrent,
            active_count,
            wait_request_id: self.wait_request_id.clone(),
            wait_operation_ids: self.wait_operation_ids()?,
            next_completion_seq: self.dsl.0.state().next_completion_seq,
        };

        Ok(PersistedOpsSnapshot {
            epoch_id,
            authority_state,
            operation_specs,
            completion_entries,
            cursors: self.completion_cursor_snapshot(),
        })
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
                let outcome = self.terminal_outcome(operation_id)?.ok_or_else(|| {
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
        let dsl = new_ops_dsl_authority();
        let feed_capacity = DEFAULT_MAX_COMPLETED.saturating_mul(4).max(1024);
        let feed_buffer = Arc::new(FeedBuffer::new(feed_capacity));
        Self {
            state: RwLock::new(ShellState {
                dsl,
                records: HashMap::new(),
                pending_wait: None,
                completed_order: VecDeque::new(),
                max_completed: DEFAULT_MAX_COMPLETED,
                max_concurrent: None,
                wait_request_id: None,
                feed_buffer,
                persist_tx: None,
                persistence_sealed: false,
                persist_epoch_id: None,
                persist_cursor_state: None,
                owner_retired: false,
            }),
        }
    }

    pub fn with_config(config: OpsLifecycleConfig) -> Self {
        Self {
            state: RwLock::new(ShellState::new(config.max_completed, config.max_concurrent)),
        }
    }

    fn recover_completion_feed_entry(
        shell: &mut ShellState,
        operation_id: &OperationId,
        entry: &CompletionFeedCanonicalState,
    ) -> Result<(), OpsLifecycleError> {
        let expected_operation_id = mm_dsl::OperationId::from_domain(operation_id).0;
        let terminal_outcome_kind =
            mm_dsl::OperationTerminalOutcomeKind::from(&entry.terminal_outcome);
        let effects = shell.dsl_apply_with_effects(
            mm_dsl::MeerkatMachineInput::RecoverCompletionFeedEntry {
                operation_id: expected_operation_id.clone(),
                kind: mm_dsl::OperationKind::from(entry.kind),
                terminal_outcome: terminal_outcome_kind,
                terminal_payload: entry.terminal_outcome.clone(),
                completion_sequence: entry.seq,
            },
            "RecoverCompletionFeedEntry",
        )?;
        let recovered = effects.iter().find_map(|effect| match effect {
            mm_dsl::MeerkatMachineEffect::CompletionFeedEntryRecovered {
                operation_id,
                seq,
                kind,
                terminal_outcome,
                terminal_payload,
            } => Some((
                operation_id,
                *seq,
                OperationKind::from(*kind),
                *terminal_outcome,
                terminal_payload,
            )),
            _ => None,
        });
        let Some((operation_id, seq, kind, terminal_outcome, terminal_payload)) = recovered else {
            return Err(OpsLifecycleError::Internal(
                "generated completion-feed recovery emitted no recovered entry".into(),
            ));
        };
        if operation_id != &expected_operation_id
            || seq != entry.seq
            || kind != entry.kind
            || terminal_outcome != terminal_outcome_kind
            || terminal_payload != &entry.terminal_outcome
        {
            return Err(OpsLifecycleError::Internal(format!(
                "generated completion-feed recovery drifted for {operation_id}"
            )));
        }
        Ok(())
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
            if state.owner_retired || state.persistence_sealed {
                tracing::warn!(
                    "ignored attempt to rewire a terminally sealed ops persistence channel"
                );
                return;
            }
            state.persist_tx = Some(tx);
            state.persist_epoch_id = Some(epoch_id);
            state.persist_cursor_state = Some(cursor_state);
        }
    }

    /// Terminalize every live operation, persist those generated transitions,
    /// then close both lifecycle admission and the persistence producer.
    ///
    /// The write lock excludes every callback that retained this registry.
    /// Each terminal transition waits for its durable worker acknowledgement;
    /// taking the sender afterward therefore leaves no queued write behind.
    /// The unregister coordinator must join the owned worker before publishing
    /// final lifecycle deletion in the RuntimeStore.
    pub(crate) fn retire_owner_for_unregister(
        &self,
        reason: String,
    ) -> Result<(), OpsLifecycleError> {
        let closed_sender = {
            let mut state = self.write_state()?;
            if state.owner_retired {
                return Ok(());
            }
            terminate_owner_locked(&mut state, &reason)?;
            state.owner_retired = true;
            state.persistence_sealed = true;
            state.persist_epoch_id = None;
            state.persist_cursor_state = None;
            state.persist_tx.take()
        };
        drop(closed_sender);
        Ok(())
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
            cursors,
            ..
        } = snapshot;
        let max_completed = authority_state.max_completed;
        let max_concurrent = authority_state.max_concurrent;
        let next_completion_seq = authority_state.next_completion_seq;
        let authority_completion_entries = authority_state.completion_feed_entries;
        let authority_operations = authority_state.operations;
        let mut shell = ShellState::new(max_completed, max_concurrent);

        // Replay every persisted op through generated recovery authority.
        // The transition accepts only terminal records with outcome and
        // completion-sequence witnesses. Volatile non-terminal rows are not
        // recovered; terminal/corrupt rows must fail closed instead of being
        // projected into shell/public feed state.
        let mut retained_ids: HashSet<OperationId> = HashSet::new();
        for (op_id, op_state) in authority_operations {
            let terminal_outcome = op_state
                .terminal_outcome
                .as_ref()
                .map(mm_dsl::OperationTerminalOutcomeKind::from);
            let terminal_payload = op_state.terminal_outcome.clone();
            let disposition = ShellState::recovered_operation_record_disposition(
                &op_id,
                op_state.status,
                op_state.kind,
                terminal_outcome.is_some(),
                terminal_payload.is_some(),
                op_state.completion_sequence.is_some(),
            )?;
            if disposition == RecoveredOperationRecordDisposition::Discard {
                continue;
            }
            if let Some(spec_source) = operation_specs
                .get(&op_id)
                .and_then(|spec| spec.operation_source.as_ref())
                && op_state.operation_source.as_ref() != Some(spec_source)
            {
                return Err(OpsLifecycleError::Internal(format!(
                    "persisted operation source mirror for {op_id} drifted from generated authority"
                )));
            }
            let recovery = mm_dsl::MeerkatMachineInput::RecoverOpRecord {
                operation_id: mm_dsl::OperationId::from_domain(&op_id).0,
                status: mm_dsl::OperationStatus::from(op_state.status),
                kind: mm_dsl::OperationKind::from(op_state.kind),
                source: op_state
                    .operation_source
                    .as_ref()
                    .map(mm_dsl::OperationSource::from_domain),
                peer_ready: op_state.peer_ready,
                progress_count: u64::from(op_state.progress_count),
                terminal_outcome,
                terminal_payload,
                completion_sequence: op_state.completion_sequence,
            };
            shell.dsl_apply(recovery, "RecoverOpRecord")?;
            let recovered_seq = shell.completion_sequence(&op_id).ok_or_else(|| {
                OpsLifecycleError::Internal(format!(
                    "generated op recovery accepted {op_id} without completion sequence"
                ))
            })?;
            if op_state.completion_sequence != Some(recovered_seq) {
                return Err(OpsLifecycleError::Internal(format!(
                    "generated op recovery completion sequence mismatch for {op_id}"
                )));
            }
            retained_ids.insert(op_id);
        }
        shell.dsl_apply(
            mm_dsl::MeerkatMachineInput::RecoverOpsCompletionCursor {
                next_completion_seq,
            },
            "RecoverOpsCompletionCursor",
        )?;
        shell.dsl_apply(
            mm_dsl::MeerkatMachineInput::RecoverCompletionConsumerCursors {
                agent_applied_cursor: cursors.agent_applied_cursor,
                runtime_observed_cursor: cursors.runtime_observed_seq,
                runtime_injected_cursor: cursors.runtime_last_injected_seq,
            },
            "RecoverCompletionConsumerCursors",
        )?;

        // Rebuild completed_order from generated completion-sequence truth,
        // never from the persisted shell ordering mirror.
        let mut recovered_completed: Vec<(CompletionSeq, OperationId)> = retained_ids
            .iter()
            .filter_map(|id| shell.completion_sequence(id).map(|seq| (seq, id.clone())))
            .collect();
        recovered_completed.sort_by_key(|(seq, _)| *seq);
        shell.completed_order = recovered_completed.into_iter().map(|(_, id)| id).collect();

        // Recover generated-owned feed authority for entries whose operation
        // record is no longer retained. Retained records already wrote their
        // feed authority through RecoverOpRecord above.
        for (operation_id, entry) in &authority_completion_entries {
            if !retained_ids.contains(operation_id) {
                Self::recover_completion_feed_entry(&mut shell, operation_id, entry)?;
            }
        }

        let canonical_feed_entries = shell.completion_feed_authority_entries()?;
        for (operation_id, entry) in &authority_completion_entries {
            let Some(recovered_entry) = canonical_feed_entries.get(operation_id) else {
                return Err(OpsLifecycleError::Internal(format!(
                    "persisted completion feed authority for {operation_id} was not recovered"
                )));
            };
            if recovered_entry != entry {
                return Err(OpsLifecycleError::Internal(format!(
                    "persisted completion feed authority drifted from generated recovery for {operation_id}"
                )));
            }
        }

        // Projection rows may carry display metadata only after generated
        // feed authority has decided the operation id, sequence, kind, and
        // terminal outcome. Any semantic drift fails closed.
        let mut projection_entries_by_id: HashMap<OperationId, CompletionEntry> = HashMap::new();
        for entry in completion_entries {
            let Some(authority_entry) = canonical_feed_entries.get(&entry.operation_id) else {
                return Err(OpsLifecycleError::Internal(format!(
                    "persisted completion feed projection for {} has no generated feed authority",
                    entry.operation_id
                )));
            };
            if authority_entry.seq != entry.seq
                || authority_entry.kind != entry.kind
                || authority_entry.terminal_outcome != entry.terminal_outcome
            {
                return Err(OpsLifecycleError::Internal(format!(
                    "persisted completion feed projection for {} drifted from generated feed authority",
                    entry.operation_id
                )));
            }
            projection_entries_by_id.insert(entry.operation_id.clone(), entry);
        }

        let mut recovered_entries: Vec<(OperationId, CompletionFeedCanonicalState)> =
            canonical_feed_entries.into_iter().collect();
        recovered_entries.sort_by_key(|(_, entry)| entry.seq);
        for (operation_id, entry) in recovered_entries {
            let projection = projection_entries_by_id.get(&operation_id);
            let display_name = operation_specs
                .get(&operation_id)
                .map(|spec| spec.display_name.clone())
                .or_else(|| projection.map(|entry| entry.display_name.clone()))
                .unwrap_or_default();
            let completed_at_ms = projection.and_then(|entry| entry.completed_at_ms);
            shell.feed_buffer.push(CompletionEntry {
                seq: entry.seq,
                operation_id,
                kind: entry.kind,
                display_name,
                terminal_outcome: entry.terminal_outcome,
                completed_at_ms,
            });
        }

        // Rebuild shell records from specs (fresh timestamps, no watchers)
        // — only for operations still retained in the DSL state.
        for (op_id, spec) in operation_specs {
            if retained_ids.contains(&op_id) {
                let mut spec = spec;
                let operation_source = shell.operation_source(&op_id)?;
                ShellState::align_spec_child_session_id_to_source(
                    &mut spec,
                    operation_source.as_ref(),
                );
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
    /// generated completion-consumer cursor values.
    pub fn capture_persistence_snapshot(
        &self,
        epoch_id: meerkat_core::RuntimeEpochId,
        cursor_state: &meerkat_core::EpochCursorState,
    ) -> Result<PersistedOpsSnapshot, OpsLifecycleError> {
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.capture_snapshot(epoch_id, cursor_state)
    }

    /// Snapshot generated completion-consumer cursor state.
    pub fn completion_cursor_snapshot(&self) -> meerkat_core::EpochCursorSnapshot {
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.completion_cursor_snapshot()
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
    pub(crate) fn diagnostic_snapshot(
        &self,
    ) -> Result<RuntimeOpsDiagnosticSnapshot, OpsLifecycleError> {
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut operations = state
            .operation_ids()?
            .into_iter()
            .map(|id| state.snapshot(&id))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        operations.sort_by(|left, right| left.display_name.cmp(&right.display_name));
        Ok(RuntimeOpsDiagnosticSnapshot {
            operation_count: state.operation_count(),
            active_count: state.active_count(),
            wait_request_id: state.wait_request_id.clone(),
            pending_wait_present: state.pending_wait.is_some(),
            pending_wait_request_id: state
                .pending_wait
                .as_ref()
                .map(|pending_wait| pending_wait.wait_request_id.clone()),
            wait_operation_ids: state.wait_operation_ids()?,
            operations,
        })
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
        if matches!(self.state, WaitAllFutureState::Waiting(_))
            && let Err(err) = self
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
    state.ensure_owner_active()?;
    state
        .dsl_apply_raw(input)
        .map_err(|err| classify_generated_op_rejection(state, err, id, action))
}

fn apply_terminal_op_transition_and_persist(
    state: &mut ShellState,
    id: &OperationId,
    input: mm_dsl::MeerkatMachineInput,
    action: mm_dsl::OpLifecycleActionKind,
) -> Result<(), OpsLifecycleError> {
    let previous_snapshot = state.dsl.0.snapshot();
    apply_op_transition(state, id, input, action)?;
    if let Err(err) = state.maybe_persist() {
        state.dsl.0.restore_snapshot(previous_snapshot);
        return Err(err);
    }
    Ok(())
}

fn terminate_owner_locked(state: &mut ShellState, reason: &str) -> Result<(), OpsLifecycleError> {
    state.ensure_owner_active()?;
    let to_terminate = state.owner_termination_targets()?;

    for (op_id, _status) in &to_terminate {
        let terminal_outcome = OperationTerminalOutcome::Terminated {
            reason: reason.to_owned(),
        };
        let outcome_kind = mm_dsl::OperationTerminalOutcomeKind::from(&terminal_outcome);

        apply_terminal_op_transition_and_persist(
            state,
            op_id,
            mm_dsl::MeerkatMachineInput::TerminateOp {
                operation_id: mm_dsl::OperationId::from_domain(op_id).0,
                outcome: outcome_kind,
                payload: terminal_outcome,
            },
            mm_dsl::OpLifecycleActionKind::Terminate,
        )?;

        if let Some(entry) = state.finalize_terminal(op_id)? {
            state.publish_completion_entry(Some(entry));
        }
    }
    Ok(())
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
        self.register_operation_with_admission_limit(spec, None)
    }

    fn register_operation_with_admission_limit(
        &self,
        mut spec: OperationSpec,
        max_concurrent: Option<usize>,
    ) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;
        state.ensure_owner_active()?;
        let operation_id = spec.id.clone();
        let kind = spec.kind;
        let max_concurrent = max_concurrent
            .or(state.max_concurrent)
            .map(|limit| limit as u64);

        let effects = state.dsl_apply_with_effects(
            mm_dsl::MeerkatMachineInput::RegisterOp {
                operation_id: mm_dsl::OperationId::from_domain(&operation_id).0,
                kind: mm_dsl::OperationKind::from_domain(&kind),
                source: spec
                    .operation_source
                    .as_ref()
                    .map(mm_dsl::OperationSource::from_domain),
                max_concurrent,
            },
            "RegisterOp",
        )?;
        if let Some(error) = op_registration_error_from_effects(&operation_id, &effects)? {
            return Err(error);
        }

        let authority_operation_source = state.operation_source(&operation_id)?;
        ShellState::align_spec_child_session_id_to_source(
            &mut spec,
            authority_operation_source.as_ref(),
        );

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
        let outcome_kind = mm_dsl::OperationTerminalOutcomeKind::from(&terminal_outcome);

        apply_terminal_op_transition_and_persist(
            &mut state,
            id,
            mm_dsl::MeerkatMachineInput::FailOp {
                operation_id: mm_dsl::OperationId::from_domain(id).0,
                outcome: outcome_kind,
                payload: terminal_outcome,
            },
            mm_dsl::OpLifecycleActionKind::Fail,
        )?;

        let completion_entry = state.finalize_terminal(id)?;
        state.publish_completion_entry(completion_entry);
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
        if let Some(outcome) = state.terminal_outcome(id)? {
            return Ok(resolved_operation_completion_watch(outcome));
        }

        // Shell concern: create the channel and store the sender.
        let shell = state.shell_record_mut(id)?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        let watch = operation_completion_watch_from_receiver(rx);
        shell.watchers.push(OperationCompletionNotifier::new(tx));
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
        let outcome_kind = mm_dsl::OperationTerminalOutcomeKind::from(&terminal_outcome);

        apply_terminal_op_transition_and_persist(
            &mut state,
            id,
            mm_dsl::MeerkatMachineInput::CompleteOp {
                operation_id: mm_dsl::OperationId::from_domain(id).0,
                outcome: outcome_kind,
                payload: terminal_outcome,
            },
            mm_dsl::OpLifecycleActionKind::Complete,
        )?;

        let completion_entry = state.finalize_terminal(id)?;
        state.publish_completion_entry(completion_entry);
        Ok(())
    }

    fn fail_operation(&self, id: &OperationId, error: String) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let terminal_outcome = OperationTerminalOutcome::Failed { error };
        let outcome_kind = mm_dsl::OperationTerminalOutcomeKind::from(&terminal_outcome);

        apply_terminal_op_transition_and_persist(
            &mut state,
            id,
            mm_dsl::MeerkatMachineInput::FailOp {
                operation_id: mm_dsl::OperationId::from_domain(id).0,
                outcome: outcome_kind,
                payload: terminal_outcome,
            },
            mm_dsl::OpLifecycleActionKind::Fail,
        )?;

        let completion_entry = state.finalize_terminal(id)?;
        state.publish_completion_entry(completion_entry);
        Ok(())
    }

    fn abort_provisioning(
        &self,
        id: &OperationId,
        reason: Option<String>,
    ) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let terminal_outcome = OperationTerminalOutcome::Aborted { reason };
        let outcome_kind = mm_dsl::OperationTerminalOutcomeKind::from(&terminal_outcome);

        apply_terminal_op_transition_and_persist(
            &mut state,
            id,
            mm_dsl::MeerkatMachineInput::AbortOp {
                operation_id: mm_dsl::OperationId::from_domain(id).0,
                outcome: outcome_kind,
                payload: terminal_outcome,
            },
            mm_dsl::OpLifecycleActionKind::Abort,
        )?;

        let completion_entry = state.finalize_terminal(id)?;
        state.publish_completion_entry(completion_entry);
        Ok(())
    }

    fn cancel_operation(
        &self,
        id: &OperationId,
        reason: Option<String>,
    ) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let terminal_outcome = OperationTerminalOutcome::Cancelled { reason };
        let outcome_kind = mm_dsl::OperationTerminalOutcomeKind::from(&terminal_outcome);

        apply_terminal_op_transition_and_persist(
            &mut state,
            id,
            mm_dsl::MeerkatMachineInput::CancelOp {
                operation_id: mm_dsl::OperationId::from_domain(id).0,
                outcome: outcome_kind,
                payload: terminal_outcome,
            },
            mm_dsl::OpLifecycleActionKind::Cancel,
        )?;

        let completion_entry = state.finalize_terminal(id)?;
        state.publish_completion_entry(completion_entry);
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
        let outcome_kind = mm_dsl::OperationTerminalOutcomeKind::from(&terminal_outcome);

        apply_terminal_op_transition_and_persist(
            &mut state,
            id,
            mm_dsl::MeerkatMachineInput::RetireCompletedOp {
                operation_id: mm_dsl::OperationId::from_domain(id).0,
                outcome: outcome_kind,
                payload: terminal_outcome,
            },
            mm_dsl::OpLifecycleActionKind::RetireCompleted,
        )?;

        let completion_entry = state.finalize_terminal(id)?;
        state.publish_completion_entry(completion_entry);
        Ok(())
    }

    fn snapshot(
        &self,
        id: &OperationId,
    ) -> Result<Option<OperationLifecycleSnapshot>, OpsLifecycleError> {
        let state = self.read_state()?;
        state.snapshot(id)
    }

    fn list_operations(&self) -> Result<Vec<OperationLifecycleSnapshot>, OpsLifecycleError> {
        let state = self.read_state()?;
        let mut snapshots = Vec::new();
        for id in state.operation_ids()? {
            let snapshot = state.snapshot(&id)?.ok_or_else(|| {
                OpsLifecycleError::Internal(format!(
                    "operation {id} was present in generated lifecycle authority but produced no public snapshot"
                ))
            })?;
            snapshots.push(snapshot);
        }
        snapshots.sort_by(|left, right| left.display_name.cmp(&right.display_name));
        Ok(snapshots)
    }

    fn classify_operation_terminality(
        &self,
        id: &OperationId,
        status: OperationStatus,
    ) -> Result<bool, OpsLifecycleError> {
        ShellState::operation_status_is_terminal(id, status)
    }

    fn classify_operation_public_result(
        &self,
        id: &OperationId,
    ) -> Result<OperationPublicResultClass, OpsLifecycleError> {
        let state = self.read_state()?;
        let status = match state.status(id) {
            Some(status) => status,
            None if state.records.contains_key(id)
                || state.has_generated_operation_record_fact(id) =>
            {
                return Err(OpsLifecycleError::Internal(format!(
                    "generated op lifecycle authority missing status for {id}"
                )));
            }
            None => OperationStatus::Absent,
        };
        ShellState::operation_public_result_class(id, status)
    }

    fn classify_operation_completion_wake(
        &self,
        id: &OperationId,
        kind: OperationKind,
    ) -> Result<OperationCompletionWakeClass, OpsLifecycleError> {
        ShellState::operation_completion_wake_class(id, kind)
    }

    fn classify_operation_transition_idempotence(
        &self,
        id: &OperationId,
        action: OperationLifecycleAction,
    ) -> Result<bool, OpsLifecycleError> {
        let state = self.read_state()?;
        let status = match state.status(id) {
            Some(status) => status,
            None if state.records.contains_key(id)
                || state.has_generated_operation_record_fact(id) =>
            {
                return Err(OpsLifecycleError::Internal(format!(
                    "generated op lifecycle authority missing status for {id}"
                )));
            }
            None => OperationStatus::Absent,
        };
        ShellState::operation_transition_rejection_is_idempotent(id, action, status)
    }

    fn terminate_owner(&self, reason: String) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;
        if state.owner_retired {
            return Ok(());
        }
        terminate_owner_locked(&mut state, &reason)
    }

    fn collect_completed(
        &self,
    ) -> Result<Vec<(OperationId, OperationTerminalOutcome)>, OpsLifecycleError> {
        let mut state = self.write_state()?;
        state.ensure_owner_active()?;

        let ids: Vec<OperationId> = state.completed_order.iter().cloned().collect();
        let mut collected = Vec::with_capacity(ids.len());
        for id in ids {
            let outcome = state.terminal_outcome(&id)?;
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

    fn completion_cursor(
        &self,
        consumer: CompletionCursorConsumer,
    ) -> Result<Option<CompletionSeq>, OpsLifecycleError> {
        let state = self.read_state()?;
        Ok(Some(state.completion_cursor(consumer)))
    }

    fn advance_completion_cursor(
        &self,
        consumer: CompletionCursorConsumer,
        cursor: CompletionSeq,
        projection: Option<&meerkat_core::EpochCursorState>,
    ) -> Result<CompletionSeq, OpsLifecycleError> {
        let mut state = self.write_state()?;
        state.ensure_owner_active()?;
        let input = match consumer {
            CompletionCursorConsumer::AgentApplied => {
                mm_dsl::MeerkatMachineInput::AdvanceAgentCompletionCursor { cursor }
            }
            CompletionCursorConsumer::RuntimeObserved => {
                mm_dsl::MeerkatMachineInput::AdvanceRuntimeObservedCompletionCursor { cursor }
            }
            CompletionCursorConsumer::RuntimeInjected => {
                mm_dsl::MeerkatMachineInput::AdvanceRuntimeInjectedCompletionCursor { cursor }
            }
        };
        let effects = state.dsl_apply_with_effects(input, "AdvanceCompletionCursor")?;
        let advanced = effects
            .iter()
            .find_map(|effect| match (consumer, effect) {
                (
                    CompletionCursorConsumer::AgentApplied,
                    mm_dsl::MeerkatMachineEffect::AgentCompletionCursorAdvanced { cursor },
                ) => Some(*cursor),
                (
                    CompletionCursorConsumer::RuntimeObserved,
                    mm_dsl::MeerkatMachineEffect::RuntimeObservedCompletionCursorAdvanced {
                        cursor,
                    },
                ) => Some(*cursor),
                (
                    CompletionCursorConsumer::RuntimeInjected,
                    mm_dsl::MeerkatMachineEffect::RuntimeInjectedCompletionCursorAdvanced {
                        cursor,
                    },
                ) => Some(*cursor),
                _ => None,
            })
            .ok_or_else(|| {
                OpsLifecycleError::Internal(format!(
                    "generated completion cursor transition emitted no feedback for {consumer:?}"
                ))
            })?;
        if let Some(projection) = projection {
            projection.project_authorized_completion_cursor(consumer, advanced);
        }
        Ok(advanced)
    }

    fn wait_all(
        &self,
        run_id: &RunId,
        ids: &[OperationId],
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<WaitAllResult, OpsLifecycleError>> + Send + '_>,
    > {
        let wait_request_id = WaitRequestId::new();
        let owned_ids = ids.to_vec();

        let state = match self.write_state() {
            Ok(mut state) => {
                if let Err(error) = state.ensure_owner_active() {
                    return Box::pin(WaitAllFuture {
                        registry: self,
                        wait_request_id,
                        state: WaitAllFutureState::Ready(Some(Err(error))),
                    });
                }
                match state.begin_wait_all_authority(run_id, &wait_request_id, &owned_ids) {
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
            operation_source: None,
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
        match watch
            .await
            .expect("operation completion watch should resolve")
        {
            OperationTerminalOutcome::Completed(result) => assert_eq!(result.content, "done"),
            other => panic!("expected completed outcome, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dropped_watch_sender_is_waiter_error_not_terminal_outcome() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let watch = operation_completion_watch_from_receiver(rx);
        drop(tx);

        assert_eq!(
            watch.await,
            Err(meerkat_core::ops_lifecycle::OperationCompletionWatchError::ChannelClosed)
        );
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

    /// K8b pinning test: the terminal payload carried through the generated
    /// machine IS the typed domain outcome — no JSON codec exists in either
    /// direction. Every variant classifies to its matching discriminant, the
    /// fail-closed read returns the exact payload when the discriminant
    /// matches, and rejects any discriminant/variant disagreement.
    #[test]
    fn typed_terminal_payload_classifies_and_reads_back_each_variant() {
        let op_id = OperationId::new();
        let outcomes = vec![
            (
                OperationTerminalOutcome::Completed(OperationResult {
                    id: op_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 7,
                    tokens_used: 42,
                }),
                mm_dsl::OperationTerminalOutcomeKind::Completed,
            ),
            (
                OperationTerminalOutcome::Failed {
                    error: "boom".into(),
                },
                mm_dsl::OperationTerminalOutcomeKind::Failed,
            ),
            (
                OperationTerminalOutcome::Aborted {
                    reason: Some("user aborted".into()),
                },
                mm_dsl::OperationTerminalOutcomeKind::Aborted,
            ),
            (
                OperationTerminalOutcome::Aborted { reason: None },
                mm_dsl::OperationTerminalOutcomeKind::Aborted,
            ),
            (
                OperationTerminalOutcome::Cancelled {
                    reason: Some("cancelled".into()),
                },
                mm_dsl::OperationTerminalOutcomeKind::Cancelled,
            ),
            (
                OperationTerminalOutcome::Cancelled { reason: None },
                mm_dsl::OperationTerminalOutcomeKind::Cancelled,
            ),
            (
                OperationTerminalOutcome::Retired,
                mm_dsl::OperationTerminalOutcomeKind::Retired,
            ),
            (
                OperationTerminalOutcome::Terminated {
                    reason: "owner stopped".into(),
                },
                mm_dsl::OperationTerminalOutcomeKind::Terminated,
            ),
        ];

        for (outcome, expected_kind) in &outcomes {
            assert_eq!(
                mm_dsl::OperationTerminalOutcomeKind::from(outcome),
                *expected_kind,
                "typed payload {outcome:?} must classify to {expected_kind:?}"
            );
            let read = ShellState::checked_terminal_payload(
                *expected_kind,
                outcome,
                "test authority",
                "test-op",
            )
            .expect("matching discriminant must read back the exact payload");
            assert_eq!(&read, outcome);
        }

        // Discriminant/variant disagreement fails closed.
        let err = ShellState::checked_terminal_payload(
            mm_dsl::OperationTerminalOutcomeKind::Completed,
            &OperationTerminalOutcome::Retired,
            "test authority",
            "test-op",
        )
        .expect_err("variant mismatch must be rejected");
        assert!(matches!(err, OpsLifecycleError::Internal(_)));
    }

    /// K8b machine-ownership pin: a terminal payload whose variant disagrees
    /// with the transition's terminal kind is rejected by the generated
    /// machine guard (`payload_variant_matches_kind`) — not by any shell
    /// decode step. Drives the DSL input directly to prove the guard owns
    /// the invariant.
    #[test]
    fn generated_guard_rejects_terminal_payload_variant_mismatch() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let spec = background_spec("variant-mismatch");
        let op_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&op_id).unwrap();

        let mut state = registry.write_state().unwrap();
        let err = state
            .dsl_apply(
                mm_dsl::MeerkatMachineInput::CompleteOp {
                    operation_id: mm_dsl::OperationId::from_domain(&op_id).0,
                    outcome: mm_dsl::OperationTerminalOutcomeKind::Completed,
                    // Variant mismatch: Completed kind with a Retired payload.
                    payload: OperationTerminalOutcome::Retired,
                },
                "CompleteOp",
            )
            .expect_err("machine must reject payload variant mismatch");
        // The kernel reports guard rejections without naming the guard; the
        // discriminating pin is the pair: identical input EXCEPT the payload
        // variant is guard-rejected here, then accepted below. Status
        // (`Running`) and discriminant (`Completed`) are identical in both
        // calls, so `payload_variant_matches_kind` is the only differing
        // guard.
        assert!(
            matches!(
                &err,
                OpsLifecycleError::Internal(message)
                    if message.contains("GuardRejected") && message.contains("CompleteOp")
            ),
            "expected generated guard rejection for CompleteOp, got: {err:?}"
        );
        drop(state);

        // RetireCompletedOp requires the unit Retired payload; a data-carrying
        // payload is rejected by the same guard shape.
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
            .expect("matching variant must complete");
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
            match watch
                .await
                .expect("operation completion watch should resolve")
            {
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
        assert!(state.wait_operation_ids().unwrap().is_empty());
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
        assert!(state.wait_operation_ids().unwrap().is_empty());
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
        assert!(state.wait_operation_ids().unwrap().is_empty());
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
        assert!(state.wait_operation_ids().unwrap().is_empty());
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
                state.wait_operation_ids().unwrap().as_slice(),
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
    async fn terminal_transition_rolls_back_publication_when_persistence_fails() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let (tx, mut rx) = crate::tokio::sync::mpsc::unbounded_channel();
        registry.set_persistence_channel(
            tx,
            meerkat_core::RuntimeEpochId::new(),
            Arc::new(meerkat_core::EpochCursorState::new()),
        );
        let worker = std::thread::spawn(move || {
            let request = rx.blocking_recv().expect("persistence request");
            let _ = request.result_tx.send(Err(OpsLifecycleError::Internal(
                "injected persistence failure".to_string(),
            )));
        });

        let spec = background_spec("feed-after-persist");
        let op_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&op_id).unwrap();
        let watch = registry.register_watcher(&op_id).unwrap();

        let err = registry
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
            .expect_err("persistence failure must fail the terminal transition");
        assert!(
            matches!(&err, OpsLifecycleError::Internal(message) if message.contains("injected persistence failure")),
            "unexpected persistence error: {err:?}"
        );
        worker.join().expect("persistence worker");

        let feed = registry.completion_feed_handle();
        let batch = feed.list_since(0);
        assert!(
            batch.entries.is_empty(),
            "completion feed must not publish entries before durable snapshot success"
        );
        assert_eq!(batch.watermark, 0);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), watch)
                .await
                .is_err(),
            "watchers must not resolve before durable terminal snapshot success"
        );
        assert_eq!(
            registry.snapshot(&op_id).unwrap().unwrap().status,
            OperationStatus::Running,
            "failed persistence must roll the generated terminal transition back"
        );

        let (tx, mut rx) = crate::tokio::sync::mpsc::unbounded_channel();
        registry.set_persistence_channel(
            tx,
            meerkat_core::RuntimeEpochId::new(),
            Arc::new(meerkat_core::EpochCursorState::new()),
        );
        let worker = std::thread::spawn(move || {
            let request = rx.blocking_recv().expect("second persistence request");
            let _ = request.result_tx.send(Ok(()));
        });
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
            .expect("rolled-back terminal transition should remain retryable");
        worker.join().expect("second persistence worker");

        let batch = feed.list_since(0);
        assert_eq!(batch.entries.len(), 1);
        assert_eq!(batch.entries[0].operation_id, op_id);
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
        assert!(state.wait_operation_ids().unwrap().is_empty());
        assert!(!state.wait_active());
    }

    /// id 101: an authority-invariant corruption surfaced during a terminal
    /// transition must NOT report the op terminal with a silently-hung barrier.
    /// The terminal call must return the typed `Internal` fault, AND the awaited
    /// `wait_all` future must resolve to `Err` (via the dropped-sender arm)
    /// instead of hanging forever.
    #[tokio::test]
    async fn satisfy_wait_authority_fault_fails_terminal_and_unblocks_waiter() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let run_id = test_run_id();

        let spec = background_spec("corrupt-barrier");
        let op_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&op_id).unwrap();

        // Activate a barrier whose waiter is still pending.
        let wait_fut = registry.wait_all(&run_id, std::slice::from_ref(&op_id));
        tokio::pin!(wait_fut);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), &mut wait_fut)
                .await
                .is_err(),
            "barrier waiter must still be pending before corruption"
        );
        assert!(registry.read_state().unwrap().wait_request_id.is_some());

        // Corrupt the generated wait authority: an active wait_request_id with
        // no wait_run_id forces `try_satisfy_wait_all_authority` down its
        // non-GuardRejected `Internal` arm during the terminal transition.
        {
            let mut state = registry.write_state().unwrap();
            let mut machine_state = state.dsl.0.state().clone();
            machine_state.wait_run_id = None;
            state.dsl = DslAuthority(Box::new(
                mm_dsl::MeerkatMachineAuthority::recover_from_state(machine_state).unwrap(),
            ));
        }

        // The terminal transition must FAIL with the typed authority fault, not
        // report the op complete.
        let err = registry
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
            .expect_err("corrupt wait authority must fail the terminal transition");
        assert!(
            matches!(&err, OpsLifecycleError::Internal(message) if message.contains("active wait without run id")),
            "unexpected terminal error: {err:?}"
        );

        // The waiter must resolve to Err (dropped sender), not hang.
        let waiter_result = tokio::time::timeout(std::time::Duration::from_secs(1), &mut wait_fut)
            .await
            .expect("waiter must resolve, not hang, after authority corruption");
        match waiter_result {
            Err(OpsLifecycleError::Internal(message)) => assert!(
                message.contains("wait_all completion channel dropped"),
                "unexpected waiter error message: {message}"
            ),
            other => panic!("expected dropped-channel Internal error, got {other:?}"),
        }
    }

    /// id 97: a poisoned registry lock must surface as a typed `Internal` fault
    /// from `completion_cursor`, NOT laundered into `Ok(None)` (which means "no
    /// generated cursor authority") or a bare `None`.
    #[test]
    fn completion_cursor_propagates_poison_not_none() {
        let registry = std::sync::Arc::new(RuntimeOpsLifecycleRegistry::new());

        // Poison the registry RwLock by panicking while holding the write guard.
        let poison_registry = std::sync::Arc::clone(&registry);
        let join = std::thread::spawn(move || {
            let _guard = poison_registry.write_state().unwrap();
            panic!("intentional panic to poison ops lifecycle registry lock");
        });
        assert!(
            join.join().is_err(),
            "poisoning thread must have panicked while holding the write guard"
        );

        let trait_ref: &dyn OpsLifecycleRegistry = registry.as_ref();
        let result = trait_ref.completion_cursor(CompletionCursorConsumer::AgentApplied);
        match result {
            Err(OpsLifecycleError::Internal(message)) => assert!(
                message.contains("ops lifecycle registry poisoned"),
                "unexpected cursor error message: {message}"
            ),
            other => panic!("poisoned registry must surface typed Internal fault, got {other:?}"),
        }
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
            registry.snapshot(&running_id).unwrap().unwrap().status,
            OperationStatus::Terminated
        ));
        assert!(matches!(
            registry.snapshot(&completed_id).unwrap().unwrap().status,
            OperationStatus::Completed
        ));
    }

    #[test]
    fn unregister_retirement_fences_detached_late_callbacks() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let running_spec = background_spec("detached-running");
        let running_id = running_spec.id.clone();
        registry.register_operation(running_spec).unwrap();
        registry.provisioning_succeeded(&running_id).unwrap();

        registry
            .retire_owner_for_unregister("owner unregistered".into())
            .unwrap();
        assert!(matches!(
            registry.snapshot(&running_id).unwrap().unwrap().status,
            OperationStatus::Terminated
        ));

        let late_result = OperationResult {
            id: running_id.clone(),
            content: "late detached completion".into(),
            is_error: false,
            duration_ms: 1,
            tokens_used: 0,
        };
        assert_eq!(
            registry.complete_operation(&running_id, late_result),
            Err(OpsLifecycleError::OwnerRetired)
        );
        assert_eq!(
            registry.register_operation(background_spec("late-new-op")),
            Err(OpsLifecycleError::OwnerRetired)
        );
        assert_eq!(
            registry.report_progress(
                &running_id,
                OperationProgressUpdate {
                    message: "late progress".into(),
                    percent: None,
                },
            ),
            Err(OpsLifecycleError::OwnerRetired)
        );
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

        assert!(registry.snapshot(&id_a).unwrap().is_none());
        assert!(registry.snapshot(&id_b).unwrap().is_some());

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

        assert!(registry.snapshot(&ids[0]).unwrap().is_none());
        assert!(registry.snapshot(&ids[1]).unwrap().is_none());
        assert!(registry.snapshot(&ids[2]).unwrap().is_some());
        assert!(registry.snapshot(&ids[3]).unwrap().is_some());
        assert!(registry.snapshot(&ids[4]).unwrap().is_some());
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
            .capture_persistence_snapshot(meerkat_core::RuntimeEpochId::new(), &cursor_state)
            .unwrap();
        let recovered = RuntimeOpsLifecycleRegistry::from_recovered(snapshot).unwrap();

        assert!(recovered.snapshot(&completed_id).unwrap().is_some());
        assert!(recovered.snapshot(&running_id).unwrap().is_none());

        let collected = recovered.collect_completed().unwrap();
        assert_eq!(collected.len(), 1);
        assert_eq!(collected[0].0, completed_id);
    }

    #[test]
    fn capacity_slot_terminal_is_not_persisted_or_recovered() {
        let registry = RuntimeOpsLifecycleRegistry::new();

        let mut spec = background_spec("capacity");
        spec.kind = OperationKind::BackgroundToolCapacitySlot;
        let operation_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&operation_id).unwrap();
        registry.mark_retired(&operation_id).unwrap();

        assert!(registry.snapshot(&operation_id).unwrap().is_none());

        let cursor_state = meerkat_core::EpochCursorState::new();
        let snapshot = registry
            .capture_persistence_snapshot(meerkat_core::RuntimeEpochId::new(), &cursor_state)
            .unwrap();
        assert!(
            !snapshot
                .authority_state
                .operations
                .contains_key(&operation_id)
        );
        assert!(!snapshot.operation_specs.contains_key(&operation_id));
        assert!(snapshot.completion_entries.is_empty());

        let recovered = RuntimeOpsLifecycleRegistry::from_recovered(snapshot).unwrap();
        assert!(recovered.snapshot(&operation_id).unwrap().is_none());
    }

    #[test]
    fn recovered_snapshot_uses_authority_operation_source() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let child_session_id = SessionId::new();
        let operation_source = OperationSource::session_child(child_session_id.clone());
        let spec = OperationSpec {
            id: OperationId::new(),
            kind: OperationKind::MobMemberChild,
            owner_session_id: SessionId::new(),
            display_name: "source-recovery".into(),
            source_label: "test".into(),
            operation_source: Some(operation_source.clone()),
            child_session_id: Some(child_session_id),
            expect_peer_channel: true,
        };
        let operation_id = spec.id.clone();

        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&operation_id).unwrap();
        registry.mark_retired(&operation_id).unwrap();

        let cursor_state = meerkat_core::EpochCursorState::new();
        let mut snapshot = registry
            .capture_persistence_snapshot(meerkat_core::RuntimeEpochId::new(), &cursor_state)
            .unwrap();
        assert_eq!(
            snapshot
                .authority_state
                .operations
                .get(&operation_id)
                .and_then(|state| state.operation_source.as_ref()),
            Some(&operation_source)
        );

        snapshot
            .operation_specs
            .get_mut(&operation_id)
            .expect("persisted spec")
            .operation_source = None;
        let recovered = RuntimeOpsLifecycleRegistry::from_recovered(snapshot).unwrap();
        assert_eq!(
            recovered
                .snapshot(&operation_id)
                .unwrap()
                .unwrap()
                .operation_source,
            Some(operation_source)
        );
    }

    #[test]
    fn recovered_snapshot_rejects_operation_source_mirror_drift() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let child_session_id = SessionId::new();
        let operation_source = OperationSource::session_child(child_session_id.clone());
        let spec = OperationSpec {
            id: OperationId::new(),
            kind: OperationKind::MobMemberChild,
            owner_session_id: SessionId::new(),
            display_name: "source-drift".into(),
            source_label: "test".into(),
            operation_source: Some(operation_source),
            child_session_id: Some(child_session_id),
            expect_peer_channel: true,
        };
        let operation_id = spec.id.clone();

        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&operation_id).unwrap();
        registry.mark_retired(&operation_id).unwrap();

        let cursor_state = meerkat_core::EpochCursorState::new();
        let mut snapshot = registry
            .capture_persistence_snapshot(meerkat_core::RuntimeEpochId::new(), &cursor_state)
            .unwrap();
        snapshot
            .operation_specs
            .get_mut(&operation_id)
            .expect("persisted spec")
            .operation_source = Some(OperationSource::session_child(SessionId::new()));

        let err = RuntimeOpsLifecycleRegistry::from_recovered(snapshot)
            .expect_err("source mirror drift must fail recovery");
        assert!(
            matches!(&err, OpsLifecycleError::Internal(message) if message.contains("operation source mirror")),
            "unexpected recovery error: {err:?}"
        );
    }

    #[test]
    fn persisted_authority_state_serializes_explicit_no_operation_source() {
        let registry = RuntimeOpsLifecycleRegistry::new();

        let spec = background_spec("explicit-no-source");
        let operation_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&operation_id).unwrap();

        let cursor_state = meerkat_core::EpochCursorState::new();
        let snapshot = registry
            .capture_persistence_snapshot(meerkat_core::RuntimeEpochId::new(), &cursor_state)
            .unwrap();
        let value = serde_json::to_value(&snapshot).unwrap();
        let operations = value
            .get("authority_state")
            .and_then(|state| state.get("operations"))
            .and_then(serde_json::Value::as_object)
            .expect("serialized authority operations");
        let persisted_state = operations
            .values()
            .next()
            .and_then(serde_json::Value::as_object)
            .expect("serialized operation state");

        assert!(
            persisted_state
                .get("operation_source")
                .is_some_and(serde_json::Value::is_null),
            "generated explicit no-source fact must be serialized as present null: {persisted_state:?}"
        );

        let recovered_snapshot = serde_json::from_value::<PersistedOpsSnapshot>(value).unwrap();
        assert_eq!(
            recovered_snapshot
                .authority_state
                .operations
                .get(&operation_id)
                .expect("round-tripped operation")
                .operation_source,
            None
        );
    }

    #[test]
    fn persisted_authority_state_rejects_missing_operation_source_fact() {
        let registry = RuntimeOpsLifecycleRegistry::new();

        let spec = background_spec("missing-source-fact");
        registry.register_operation(spec).unwrap();

        let cursor_state = meerkat_core::EpochCursorState::new();
        let snapshot = registry
            .capture_persistence_snapshot(meerkat_core::RuntimeEpochId::new(), &cursor_state)
            .unwrap();
        let mut value = serde_json::to_value(&snapshot).unwrap();
        let operations = value
            .get_mut("authority_state")
            .and_then(|state| state.get_mut("operations"))
            .and_then(serde_json::Value::as_object_mut)
            .expect("serialized authority operations");
        let operation_state = operations
            .values_mut()
            .next()
            .and_then(serde_json::Value::as_object_mut)
            .expect("serialized operation state");
        assert!(operation_state.remove("operation_source").is_some());

        let err = serde_json::from_value::<PersistedOpsSnapshot>(value)
            .expect_err("missing generated source fact must fail recovery snapshot decoding");
        assert!(
            err.to_string().contains("operation_source"),
            "unexpected decode error: {err}"
        );
    }

    #[test]
    fn persisted_authority_state_rejects_missing_completion_feed_authority() {
        let registry = RuntimeOpsLifecycleRegistry::new();

        let spec = background_spec("missing-feed-authority");
        let operation_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&operation_id).unwrap();
        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .unwrap();

        let cursor_state = meerkat_core::EpochCursorState::new();
        let snapshot = registry
            .capture_persistence_snapshot(meerkat_core::RuntimeEpochId::new(), &cursor_state)
            .unwrap();
        let mut value = serde_json::to_value(&snapshot).unwrap();
        let authority_state = value
            .get_mut("authority_state")
            .and_then(serde_json::Value::as_object_mut)
            .expect("serialized authority state");
        assert!(authority_state.remove("completion_feed_entries").is_some());

        let err = serde_json::from_value::<PersistedOpsSnapshot>(value)
            .expect_err("missing generated feed authority must fail recovery snapshot decoding");
        assert!(
            err.to_string().contains("completion_feed_entries"),
            "unexpected decode error: {err}"
        );
    }

    #[test]
    fn public_child_session_projection_uses_authority_operation_source() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let authority_child_session_id = SessionId::new();
        let stale_shell_child_session_id = SessionId::new();
        let operation_source = OperationSource::session_child(authority_child_session_id.clone());
        let spec = OperationSpec {
            id: OperationId::new(),
            kind: OperationKind::MobMemberChild,
            owner_session_id: SessionId::new(),
            display_name: "child-projection".into(),
            source_label: "test".into(),
            operation_source: Some(operation_source),
            child_session_id: Some(stale_shell_child_session_id),
            expect_peer_channel: true,
        };
        let operation_id = spec.id.clone();

        registry.register_operation(spec).unwrap();

        assert_eq!(
            registry
                .snapshot(&operation_id)
                .unwrap()
                .unwrap()
                .child_session_id,
            Some(authority_child_session_id.clone())
        );

        let cursor_state = meerkat_core::EpochCursorState::new();
        let snapshot = registry
            .capture_persistence_snapshot(meerkat_core::RuntimeEpochId::new(), &cursor_state)
            .unwrap();
        assert_eq!(
            snapshot
                .operation_specs
                .get(&operation_id)
                .expect("persisted spec")
                .child_session_id,
            Some(authority_child_session_id)
        );
    }

    #[test]
    fn generated_terminal_payload_projection_fails_closed() {
        let registry = RuntimeOpsLifecycleRegistry::new();

        let spec = background_spec("terminal-payload-drift");
        let operation_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&operation_id).unwrap();
        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .unwrap();

        {
            let mut state = registry.write_state().unwrap();
            let mut machine_state = state.dsl.0.state().clone();
            let operation_id_key = mm_dsl::OperationId::from_domain(&operation_id).0;
            machine_state
                .op_terminal_payload
                .insert(operation_id_key, OperationTerminalOutcome::Retired);
            state.dsl = DslAuthority(Box::new(
                mm_dsl::MeerkatMachineAuthority::recover_from_state(machine_state).unwrap(),
            ));
        }

        let err = match registry.register_watcher(&operation_id) {
            Ok(_) => panic!("invalid generated terminal payload must reject watcher projection"),
            Err(err) => err,
        };
        assert!(
            matches!(&err, OpsLifecycleError::Internal(message) if message.contains("payload variant") && message.contains("does not match terminal outcome discriminant")),
            "unexpected watcher error: {err:?}"
        );
        let err = registry
            .snapshot(&operation_id)
            .expect_err("invalid generated terminal payload must reject public snapshot");
        assert!(
            matches!(&err, OpsLifecycleError::Internal(message) if message.contains("does not match terminal outcome discriminant")),
            "unexpected public snapshot error: {err:?}"
        );

        let cursor_state = meerkat_core::EpochCursorState::new();
        let err = match registry
            .capture_persistence_snapshot(meerkat_core::RuntimeEpochId::new(), &cursor_state)
        {
            Ok(_) => panic!("invalid generated terminal payload must reject persistence snapshot"),
            Err(err) => err,
        };
        assert!(
            matches!(&err, OpsLifecycleError::Internal(message) if message.contains("payload variant") && message.contains("does not match terminal outcome discriminant")),
            "unexpected snapshot error: {err:?}"
        );

        let err = match registry.collect_completed() {
            Ok(_) => panic!("invalid generated terminal payload must reject collection"),
            Err(err) => err,
        };
        assert!(
            matches!(&err, OpsLifecycleError::Internal(message) if message.contains("payload variant") && message.contains("does not match terminal outcome discriminant")),
            "unexpected collection error: {err:?}"
        );
    }

    #[test]
    fn generated_terminal_payload_missing_projection_fails_closed() {
        let registry = RuntimeOpsLifecycleRegistry::new();

        let spec = background_spec("terminal-payload-missing");
        let operation_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&operation_id).unwrap();
        registry
            .fail_operation(&operation_id, "boom".into())
            .unwrap();

        {
            let mut state = registry.write_state().unwrap();
            let mut machine_state = state.dsl.0.state().clone();
            let operation_id_key = mm_dsl::OperationId::from_domain(&operation_id).0;
            machine_state.op_terminal_payload.remove(&operation_id_key);
            state.dsl = DslAuthority(Box::new(
                mm_dsl::MeerkatMachineAuthority::recover_from_state(machine_state).unwrap(),
            ));
        }

        let err = match registry.register_watcher(&operation_id) {
            Ok(_) => panic!("missing generated terminal payload must reject watcher projection"),
            Err(err) => err,
        };
        assert!(
            matches!(&err, OpsLifecycleError::Internal(message) if message.contains("missing terminal payload")),
            "unexpected watcher error: {err:?}"
        );
        let err = registry
            .snapshot(&operation_id)
            .expect_err("missing generated terminal payload must reject public snapshot");
        assert!(
            matches!(&err, OpsLifecycleError::Internal(message) if message.contains("missing terminal payload")),
            "unexpected public snapshot error: {err:?}"
        );
    }

    #[test]
    fn generated_terminal_status_without_outcome_fails_closed() {
        let registry = RuntimeOpsLifecycleRegistry::new();

        let spec = background_spec("terminal-outcome-missing");
        let operation_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&operation_id).unwrap();
        registry
            .fail_operation(&operation_id, "boom".into())
            .unwrap();

        {
            let mut state = registry.write_state().unwrap();
            let mut machine_state = state.dsl.0.state().clone();
            let operation_id_key = mm_dsl::OperationId::from_domain(&operation_id).0;
            machine_state.op_terminal_outcomes.remove(&operation_id_key);
            state.dsl = DslAuthority(Box::new(
                mm_dsl::MeerkatMachineAuthority::recover_from_state(machine_state).unwrap(),
            ));
        }

        let err = registry
            .snapshot(&operation_id)
            .expect_err("terminal status without outcome must reject public snapshot");
        assert!(
            matches!(&err, OpsLifecycleError::Internal(message) if message.contains("missing terminal outcome")),
            "unexpected public snapshot error: {err:?}"
        );

        let err = match registry.collect_completed() {
            Ok(_) => panic!("terminal status without outcome must reject collection"),
            Err(err) => err,
        };
        assert!(
            matches!(&err, OpsLifecycleError::Internal(message) if message.contains("missing terminal outcome")),
            "unexpected collection error: {err:?}"
        );
    }

    #[test]
    fn generated_operation_source_projection_fails_closed() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let child_session_id = SessionId::new();
        let operation_source = OperationSource::session_child(child_session_id.clone());
        let spec = OperationSpec {
            id: OperationId::new(),
            kind: OperationKind::MobMemberChild,
            owner_session_id: SessionId::new(),
            display_name: "source-authority-drift".into(),
            source_label: "test".into(),
            operation_source: Some(operation_source),
            child_session_id: Some(child_session_id),
            expect_peer_channel: true,
        };
        let operation_id = spec.id.clone();

        registry.register_operation(spec).unwrap();

        {
            let mut state = registry.write_state().unwrap();
            let mut machine_state = state.dsl.0.state().clone();
            let operation_id_key = mm_dsl::OperationId::from_domain(&operation_id).0;
            machine_state
                .op_sources
                .get_mut(&operation_id_key)
                .expect("generated operation source")
                .session_id = None;
            state.dsl = DslAuthority(Box::new(
                mm_dsl::MeerkatMachineAuthority::recover_from_state(machine_state).unwrap(),
            ));
        }

        let err = registry
            .snapshot(&operation_id)
            .expect_err("invalid generated operation source must reject public snapshot");
        assert!(
            matches!(&err, OpsLifecycleError::Internal(message) if message.contains("generated operation source authority has invalid source")),
            "unexpected public snapshot error: {err:?}"
        );
        let err = registry
            .list_operations()
            .expect_err("invalid generated operation source must reject public operation list");
        assert!(
            matches!(&err, OpsLifecycleError::Internal(message) if message.contains("generated operation source authority has invalid source")),
            "unexpected operation list error: {err:?}"
        );

        let cursor_state = meerkat_core::EpochCursorState::new();
        let err = match registry
            .capture_persistence_snapshot(meerkat_core::RuntimeEpochId::new(), &cursor_state)
        {
            Ok(_) => panic!("invalid generated operation source must reject persistence snapshot"),
            Err(err) => err,
        };
        assert!(
            matches!(&err, OpsLifecycleError::Internal(message) if message.contains("generated operation source authority has invalid source")),
            "unexpected snapshot error: {err:?}"
        );
    }

    #[test]
    fn generated_operation_id_projection_fails_closed() {
        let registry = RuntimeOpsLifecycleRegistry::new();

        {
            let mut state = registry.write_state().unwrap();
            let mut machine_state = state.dsl.0.state().clone();
            machine_state.op_statuses.insert(
                "not-json-operation-id".into(),
                mm_dsl::OperationStatus::Running,
            );
            state.dsl = DslAuthority(Box::new(
                mm_dsl::MeerkatMachineAuthority::recover_from_state(machine_state).unwrap(),
            ));
        }

        let err = registry
            .list_operations()
            .expect_err("invalid generated operation id must reject public operation list");
        assert!(
            matches!(&err, OpsLifecycleError::Internal(message) if message.contains("invalid operation id key")),
            "unexpected operation list error: {err:?}"
        );

        let cursor_state = meerkat_core::EpochCursorState::new();
        let err = match registry
            .capture_persistence_snapshot(meerkat_core::RuntimeEpochId::new(), &cursor_state)
        {
            Ok(_) => panic!("invalid generated operation id must reject persistence snapshot"),
            Err(err) => err,
        };
        assert!(
            matches!(&err, OpsLifecycleError::Internal(message) if message.contains("invalid operation id key")),
            "unexpected persistence snapshot error: {err:?}"
        );
    }

    #[test]
    fn generated_missing_kind_projection_fails_closed() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let spec = background_spec("missing-kind");
        let operation_id = spec.id.clone();
        registry.register_operation(spec).unwrap();

        {
            let mut state = registry.write_state().unwrap();
            let mut machine_state = state.dsl.0.state().clone();
            let operation_id_key = mm_dsl::OperationId::from_domain(&operation_id).0;
            machine_state.op_kinds.remove(&operation_id_key);
            state.dsl = DslAuthority(Box::new(
                mm_dsl::MeerkatMachineAuthority::recover_from_state(machine_state).unwrap(),
            ));
        }

        let err = registry
            .snapshot(&operation_id)
            .expect_err("missing generated kind must reject public snapshot");
        assert!(
            matches!(&err, OpsLifecycleError::Internal(message) if message.contains("missing kind")),
            "unexpected public snapshot error: {err:?}"
        );
        let err = registry
            .list_operations()
            .expect_err("missing generated kind must reject public list");
        assert!(
            matches!(&err, OpsLifecycleError::Internal(message) if message.contains("missing kind")),
            "unexpected public list error: {err:?}"
        );

        let cursor_state = meerkat_core::EpochCursorState::new();
        let err = registry
            .capture_persistence_snapshot(meerkat_core::RuntimeEpochId::new(), &cursor_state)
            .expect_err("missing generated kind must reject persistence snapshot");
        assert!(
            matches!(&err, OpsLifecycleError::Internal(message) if message.contains("missing kind")),
            "unexpected persistence snapshot error: {err:?}"
        );
    }

    #[test]
    fn generated_missing_status_projection_fails_closed() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let spec = background_spec("missing-status");
        let operation_id = spec.id.clone();
        registry.register_operation(spec).unwrap();

        {
            let mut state = registry.write_state().unwrap();
            let mut machine_state = state.dsl.0.state().clone();
            let operation_id_key = mm_dsl::OperationId::from_domain(&operation_id).0;
            machine_state.op_statuses.remove(&operation_id_key);
            state.dsl = DslAuthority(Box::new(
                mm_dsl::MeerkatMachineAuthority::recover_from_state(machine_state).unwrap(),
            ));
        }

        let err = registry
            .snapshot(&operation_id)
            .expect_err("missing generated status must reject public snapshot");
        assert!(
            matches!(&err, OpsLifecycleError::Internal(message) if message.contains("missing status")),
            "unexpected public snapshot error: {err:?}"
        );
        let err = registry
            .list_operations()
            .expect_err("missing generated status must reject public list");
        assert!(
            matches!(&err, OpsLifecycleError::Internal(message) if message.contains("missing status")),
            "unexpected public list error: {err:?}"
        );
        let err = registry
            .classify_operation_public_result(&operation_id)
            .expect_err("missing generated status must reject public-result classification");
        assert!(
            matches!(&err, OpsLifecycleError::Internal(message) if message.contains("missing status")),
            "unexpected public-result error: {err:?}"
        );
    }

    #[test]
    fn generated_retiring_public_result_remains_running_until_terminal() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let spec = background_spec("retiring-public-result");
        let operation_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&operation_id).unwrap();
        registry.request_retire(&operation_id).unwrap();

        let snapshot = registry.snapshot(&operation_id).unwrap().unwrap();
        assert_eq!(snapshot.status, OperationStatus::Retiring);
        assert!(snapshot.terminal_outcome.is_none());
        assert!(!snapshot.terminal);
        assert_eq!(
            snapshot.public_result_class,
            OperationPublicResultClass::Running
        );
        assert_eq!(
            registry
                .classify_operation_public_result(&operation_id)
                .unwrap(),
            OperationPublicResultClass::Running
        );
    }

    #[test]
    fn generated_missing_peer_ready_projection_fails_closed() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let spec = background_spec("missing-peer-ready");
        let operation_id = spec.id.clone();
        registry.register_operation(spec).unwrap();

        {
            let mut state = registry.write_state().unwrap();
            let mut machine_state = state.dsl.0.state().clone();
            let operation_id_key = mm_dsl::OperationId::from_domain(&operation_id).0;
            machine_state.op_peer_ready.remove(&operation_id_key);
            state.dsl = DslAuthority(Box::new(
                mm_dsl::MeerkatMachineAuthority::recover_from_state(machine_state).unwrap(),
            ));
        }

        let err = registry
            .snapshot(&operation_id)
            .expect_err("missing generated peer-ready fact must reject public snapshot");
        assert!(
            matches!(&err, OpsLifecycleError::Internal(message) if message.contains("missing peer-ready")),
            "unexpected public snapshot error: {err:?}"
        );

        let cursor_state = meerkat_core::EpochCursorState::new();
        let err = registry
            .capture_persistence_snapshot(meerkat_core::RuntimeEpochId::new(), &cursor_state)
            .expect_err("missing generated peer-ready fact must reject persistence snapshot");
        assert!(
            matches!(&err, OpsLifecycleError::Internal(message) if message.contains("missing peer-ready")),
            "unexpected persistence snapshot error: {err:?}"
        );
    }

    #[test]
    fn generated_missing_progress_count_projection_fails_closed() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let spec = background_spec("missing-progress-count");
        let operation_id = spec.id.clone();
        registry.register_operation(spec).unwrap();

        {
            let mut state = registry.write_state().unwrap();
            let mut machine_state = state.dsl.0.state().clone();
            let operation_id_key = mm_dsl::OperationId::from_domain(&operation_id).0;
            machine_state.op_progress_counts.remove(&operation_id_key);
            state.dsl = DslAuthority(Box::new(
                mm_dsl::MeerkatMachineAuthority::recover_from_state(machine_state).unwrap(),
            ));
        }

        let err = registry
            .snapshot(&operation_id)
            .expect_err("missing generated progress count must reject public snapshot");
        assert!(
            matches!(&err, OpsLifecycleError::Internal(message) if message.contains("missing progress count")),
            "unexpected public snapshot error: {err:?}"
        );

        let cursor_state = meerkat_core::EpochCursorState::new();
        let err = registry
            .capture_persistence_snapshot(meerkat_core::RuntimeEpochId::new(), &cursor_state)
            .expect_err("missing generated progress count must reject persistence snapshot");
        assert!(
            matches!(&err, OpsLifecycleError::Internal(message) if message.contains("missing progress count")),
            "unexpected persistence snapshot error: {err:?}"
        );
    }

    #[test]
    fn generated_terminal_sequence_missing_persistence_fails_closed() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let spec = background_spec("terminal-sequence-missing");
        let operation_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&operation_id).unwrap();
        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .unwrap();

        {
            let mut state = registry.write_state().unwrap();
            let mut machine_state = state.dsl.0.state().clone();
            let operation_id_key = mm_dsl::OperationId::from_domain(&operation_id).0;
            machine_state.op_completion_seq.remove(&operation_id_key);
            state.dsl = DslAuthority(Box::new(
                mm_dsl::MeerkatMachineAuthority::recover_from_state(machine_state).unwrap(),
            ));
        }

        let cursor_state = meerkat_core::EpochCursorState::new();
        let err = registry
            .capture_persistence_snapshot(meerkat_core::RuntimeEpochId::new(), &cursor_state)
            .expect_err("missing generated terminal sequence must reject persistence snapshot");
        assert!(
            matches!(&err, OpsLifecycleError::Internal(message) if message.contains("missing completion sequence")),
            "unexpected persistence snapshot error: {err:?}"
        );
    }

    #[test]
    fn generated_record_without_shell_projection_fails_closed() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let spec = background_spec("missing-shell-record");
        let operation_id = spec.id.clone();
        registry.register_operation(spec).unwrap();

        {
            let mut state = registry.write_state().unwrap();
            state.records.remove(&operation_id);
        }

        let err = registry
            .snapshot(&operation_id)
            .expect_err("generated operation without shell record must reject public snapshot");
        assert!(
            matches!(&err, OpsLifecycleError::Internal(message) if message.contains("without shell projection record")),
            "unexpected public snapshot error: {err:?}"
        );
        let err = registry
            .list_operations()
            .expect_err("generated operation without shell record must reject public list");
        assert!(
            matches!(&err, OpsLifecycleError::Internal(message) if message.contains("without shell projection record")),
            "unexpected public list error: {err:?}"
        );
    }

    #[test]
    fn recovered_snapshot_rebuilds_child_session_mirror_from_authority() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let child_session_id = SessionId::new();
        let operation_source = OperationSource::session_child(child_session_id.clone());
        let spec = OperationSpec {
            id: OperationId::new(),
            kind: OperationKind::MobMemberChild,
            owner_session_id: SessionId::new(),
            display_name: "child-drift".into(),
            source_label: "test".into(),
            operation_source: Some(operation_source),
            child_session_id: Some(child_session_id.clone()),
            expect_peer_channel: true,
        };
        let operation_id = spec.id.clone();

        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&operation_id).unwrap();
        registry.mark_retired(&operation_id).unwrap();

        let cursor_state = meerkat_core::EpochCursorState::new();
        let mut snapshot = registry
            .capture_persistence_snapshot(meerkat_core::RuntimeEpochId::new(), &cursor_state)
            .unwrap();
        snapshot
            .operation_specs
            .get_mut(&operation_id)
            .expect("persisted spec")
            .child_session_id = Some(SessionId::new());

        let recovered = RuntimeOpsLifecycleRegistry::from_recovered(snapshot).unwrap();
        assert_eq!(
            recovered
                .snapshot(&operation_id)
                .unwrap()
                .unwrap()
                .child_session_id,
            Some(child_session_id)
        );
    }

    #[test]
    fn completion_wake_class_is_generated_by_operation_kind() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let operation_id = OperationId::new();

        assert_eq!(
            registry
                .classify_operation_completion_wake(&operation_id, OperationKind::BackgroundToolOp)
                .unwrap(),
            OperationCompletionWakeClass::Wake
        );
        assert_eq!(
            registry
                .classify_operation_completion_wake(&operation_id, OperationKind::MobMemberChild)
                .unwrap(),
            OperationCompletionWakeClass::Ignore
        );
        assert_eq!(
            registry
                .classify_operation_completion_wake(
                    &operation_id,
                    OperationKind::BackgroundToolCapacitySlot,
                )
                .unwrap(),
            OperationCompletionWakeClass::Ignore
        );
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
            .capture_persistence_snapshot(meerkat_core::RuntimeEpochId::new(), &cursor_state)
            .unwrap();
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
            matches!(&err, OpsLifecycleError::Internal(message) if message.contains("no generated feed authority")),
            "unexpected recovery error: {err:?}"
        );
    }

    #[test]
    fn captured_snapshot_rejects_completion_feed_projection_without_generated_authority() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let phantom_id = OperationId::new();
        {
            let state = registry.read_state().unwrap();
            state.feed_buffer.push(CompletionEntry {
                seq: 1,
                operation_id: phantom_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                display_name: "phantom".into(),
                terminal_outcome: OperationTerminalOutcome::Completed(OperationResult {
                    id: phantom_id,
                    content: "phantom".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                }),
                completed_at_ms: None,
            });
        }

        let cursor_state = meerkat_core::EpochCursorState::new();
        let err = match registry
            .capture_persistence_snapshot(meerkat_core::RuntimeEpochId::new(), &cursor_state)
        {
            Ok(_) => panic!("phantom public completion projection must fail snapshot capture"),
            Err(err) => err,
        };
        assert!(
            matches!(&err, OpsLifecycleError::Internal(message) if message.contains("no generated authority")),
            "unexpected capture error: {err:?}"
        );
    }

    #[test]
    fn recovered_snapshot_rejects_feed_authority_beyond_completion_cursor() {
        let registry = RuntimeOpsLifecycleRegistry::new();

        let spec = background_spec("terminal");
        let operation_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&operation_id).unwrap();
        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .unwrap();

        let cursor_state = meerkat_core::EpochCursorState::new();
        let mut snapshot = registry
            .capture_persistence_snapshot(meerkat_core::RuntimeEpochId::new(), &cursor_state)
            .unwrap();
        let phantom_id = OperationId::new();
        let phantom_result = OperationResult {
            id: phantom_id.clone(),
            content: "phantom".into(),
            is_error: false,
            duration_ms: 1,
            tokens_used: 0,
        };
        let phantom_entry = CompletionFeedCanonicalState {
            seq: snapshot.authority_state.next_completion_seq + 1,
            kind: OperationKind::BackgroundToolOp,
            terminal_outcome: OperationTerminalOutcome::Completed(phantom_result.clone()),
        };
        snapshot
            .authority_state
            .completion_feed_entries
            .insert(phantom_id.clone(), phantom_entry);
        snapshot.completion_entries.push(CompletionEntry {
            seq: snapshot.authority_state.next_completion_seq + 1,
            operation_id: phantom_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            display_name: "phantom".into(),
            terminal_outcome: OperationTerminalOutcome::Completed(phantom_result),
            completed_at_ms: None,
        });

        let err = match RuntimeOpsLifecycleRegistry::from_recovered(snapshot) {
            Ok(_) => panic!("feed authority must not advance the recovered completion cursor"),
            Err(err) => err,
        };
        assert!(
            matches!(&err, OpsLifecycleError::Internal(message) if message.contains("RecoverCompletionFeedEntry")),
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
            .capture_persistence_snapshot(meerkat_core::RuntimeEpochId::new(), &cursor_state)
            .unwrap();
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

        let snap1 = registry.snapshot(&op_id).unwrap().unwrap();
        assert!(snap1.created_at_ms > 0);
        assert!(snap1.started_at_ms.is_none());
        assert!(snap1.completed_at_ms.is_none());
        assert!(snap1.elapsed_ms.is_none());

        registry.provisioning_succeeded(&op_id).unwrap();
        let snap2 = registry.snapshot(&op_id).unwrap().unwrap();
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
        let snap3 = registry.snapshot(&op_id).unwrap().unwrap();
        assert!(snap3.completed_at_ms.is_some());
        assert!(snap3.elapsed_ms.is_some());
        assert!(snap3.completed_at_ms.unwrap() >= snap3.started_at_ms.unwrap());
    }

    #[test]
    fn snapshot_includes_peer_handle() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let child_session_id = SessionId::new();
        let spec = OperationSpec {
            id: OperationId::new(),
            kind: OperationKind::MobMemberChild,
            owner_session_id: SessionId::new(),
            display_name: "peer-test".into(),
            source_label: "test".into(),
            operation_source: Some(OperationSource::session_child(child_session_id.clone())),
            child_session_id: Some(child_session_id),
            expect_peer_channel: true,
        };
        let op_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&op_id).unwrap();

        let snap1 = registry.snapshot(&op_id).unwrap().unwrap();
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

        let snap2 = registry.snapshot(&op_id).unwrap().unwrap();
        assert_eq!(
            snap2.peer_handle.as_ref().unwrap().peer_name.as_str(),
            "member-x"
        );
    }
}

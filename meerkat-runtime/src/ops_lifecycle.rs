//! In-memory runtime implementation of the shared async-operation lifecycle seam.
//!
//! All canonical lifecycle state mutations are delegated to
//! [`OpsLifecycleAuthority`] via [`OpsLifecycleMutator::apply`]. This shell
//! layer owns I/O concerns: watcher channels, timestamps, peer handles, and
//! snapshot assembly.

use std::collections::{HashMap, VecDeque};
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
    OperationSpec, OperationTerminalOutcome, OpsLifecycleError, OpsLifecycleRegistry,
    WaitAllResult, WaitAllSatisfied,
};
use meerkat_core::time_compat::{Instant, SystemTime, UNIX_EPOCH};

use crate::meerkat_machine::dsl as mm_dsl;
use crate::ops_lifecycle_authority::{
    OpsLifecycleAuthority, OpsLifecycleEffect, OpsLifecycleInput, OpsLifecycleMutator,
};

// ---------------------------------------------------------------------------
// Serializable snapshot for persistence
// ---------------------------------------------------------------------------

/// Serializable snapshot of the ops lifecycle registry state.
///
/// Captured on terminal transitions for durable persistence. Contains
/// canonical authority state, operation specs, persisted completion feed
/// entries, and consumer cursor values.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PersistedOpsSnapshot {
    /// Epoch identity at capture time.
    pub epoch_id: meerkat_core::RuntimeEpochId,
    /// Canonical machine-owned authority state.
    pub authority_state: crate::ops_lifecycle_authority::RegistryCanonicalState,
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
/// the authority; this struct holds I/O concerns that the authority has no
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
// Combined shell state: authority + shell records
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct ShellState {
    authority: OpsLifecycleAuthority,
    records: HashMap<OperationId, ShellRecord>,
    pending_wait: Option<PendingWaitState>,
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
    /// DSL shadow authority for ops lifecycle validation.
    ///
    /// Tracks ops-related state fields in parallel with the handwritten
    /// authority. On each successful authority transition, the corresponding
    /// DSL input is applied here; disagreements are logged as errors.
    dsl_shadow: DslShadow,
}

/// Wrapper around the DSL authority that provides `Debug` output.
///
/// The generated `MeerkatMachineAuthority` does not derive `Debug`, but
/// `ShellState` requires it. This wrapper delegates to the inner state's
/// `Debug` impl.
struct DslShadow(mm_dsl::MeerkatMachineAuthority);

impl std::fmt::Debug for DslShadow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DslShadow")
            .field("state", &self.0.state)
            .finish()
    }
}

/// Create a DSL shadow authority initialized with `lifecycle_phase: Idle`
/// and all ops-related fields at their defaults.
///
/// Ops lifecycle DSL transitions use `per_phase [Idle, Attached, Running, Retired, Stopped]`
/// and always transition back to `Idle`, so the shadow stays in `Idle` permanently.
/// Non-ops fields are left at DSL defaults since ops transitions never guard on them.
fn new_ops_dsl_shadow() -> DslShadow {
    let state = mm_dsl::MeerkatMachineState {
        lifecycle_phase: mm_dsl::MeerkatPhase::Idle,
        ..mm_dsl::MeerkatMachineState::default()
    };
    DslShadow(mm_dsl::MeerkatMachineAuthority::from_state(state))
}

impl ShellState {
    fn new(max_completed: usize, max_concurrent: Option<usize>) -> Self {
        Self {
            authority: OpsLifecycleAuthority::new(max_completed, max_concurrent),
            records: HashMap::new(),
            pending_wait: None,
            detached_wake: None,
            // Feed buffer is larger than authority retention to absorb bursts.
            // Entries are only evicted by buffer capacity, not by consumer cursor,
            // so the buffer must be large enough that consumers drain before
            // the oldest entry is evicted.
            feed_buffer: Arc::new(FeedBuffer::new(max_completed.saturating_mul(4).max(1024))),
            persist_tx: None,
            persist_epoch_id: None,
            persist_cursor_state: None,
            dsl_shadow: new_ops_dsl_shadow(),
        }
    }

    /// Build a snapshot by combining authority canonical state with shell data.
    fn snapshot(&self, id: &OperationId) -> Option<OperationLifecycleSnapshot> {
        let canonical = self.authority.operation(id)?;
        let shell = self.records.get(id)?;

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
            kind: canonical.kind(),
            display_name: shell.spec.display_name.clone(),
            status: canonical.status(),
            peer_ready: canonical.peer_ready(),
            progress_count: canonical.progress_count(),
            watcher_count: shell.watchers.len() as u32,
            terminal_outcome: canonical.terminal_outcome().cloned(),
            child_session_id: shell.spec.child_session_id.clone(),
            peer_handle: shell.peer_handle.clone(),
            created_at_ms,
            started_at_ms,
            completed_at_ms,
            elapsed_ms,
        })
    }

    /// Execute authority effects on shell state.
    ///
    /// **Important:** callers must patch the real terminal outcome on the
    /// authority (via `patch_terminal_outcome`) *before* calling this method.
    /// `NotifyOpWatcher` effects read the patched outcome from the authority
    /// rather than using the placeholder embedded in the effect.
    fn execute_effects(&mut self, effects: &[OpsLifecycleEffect]) {
        for effect in effects {
            match effect {
                OpsLifecycleEffect::NotifyOpWatcher { operation_id, .. } => {
                    // Read the real (patched) outcome from the authority.
                    let outcome = self
                        .authority
                        .operation(operation_id)
                        .and_then(|op| op.terminal_outcome().cloned());
                    if let Some(outcome) = outcome
                        && let Some(shell) = self.records.get_mut(operation_id)
                    {
                        let watcher_count = shell.watchers.len() as u32;
                        shell.notify_watchers(&outcome);
                        shell.mark_completed();
                        self.authority.watchers_drained(operation_id, watcher_count);
                    }
                    // Arm + signal detached-op wake if this is a BackgroundToolOp terminal.
                    if let Some(ref wake) = self.detached_wake
                        && self
                            .authority
                            .operation(operation_id)
                            .is_some_and(|op| op.kind() == OperationKind::BackgroundToolOp)
                    {
                        wake.pending.store(true, Ordering::Release);
                        wake.notify.notify_one(); // wake the waker task directly
                    }
                }
                OpsLifecycleEffect::ExposeOperationPeer { .. } => {
                    // Peer handle is stored in shell record by the calling method
                    // after authority.apply() succeeds. Nothing else to do here.
                }
                OpsLifecycleEffect::RetainTerminalRecord { .. } => {
                    // The authority handles completed_order tracking internally.
                    // Shell record stays in place until evicted.
                }
                OpsLifecycleEffect::EvictCompletedRecord { operation_id } => {
                    self.records.remove(operation_id);
                    self.authority.remove_operation(operation_id);
                }
                OpsLifecycleEffect::CompletionProduced {
                    seq,
                    operation_id,
                    kind,
                } => {
                    // Build a CompletionEntry from authority + shell data and
                    // push to the shared feed buffer.
                    let (display_name, terminal_outcome, completed_at_ms) =
                        if let Some(canonical) = self.authority.operation(operation_id) {
                            let outcome = canonical.terminal_outcome().cloned().unwrap_or(
                                OperationTerminalOutcome::Terminated {
                                    reason: "missing outcome".into(),
                                },
                            );
                            let completed_ms = self.records.get(operation_id).and_then(|r| {
                                r.completed_at.map(|i| r.epoch_millis_for_instant(i))
                            });
                            let name = self
                                .records
                                .get(operation_id)
                                .map(|r| r.spec.display_name.clone())
                                .unwrap_or_default();
                            (name, outcome, completed_ms)
                        } else {
                            (
                                String::new(),
                                OperationTerminalOutcome::Terminated {
                                    reason: "unknown operation".into(),
                                },
                                None,
                            )
                        };

                    self.feed_buffer.push(CompletionEntry {
                        seq: *seq,
                        operation_id: operation_id.clone(),
                        kind: *kind,
                        display_name,
                        terminal_outcome,
                        completed_at_ms,
                    });
                }
                OpsLifecycleEffect::SubmitOpEvent { .. } => {
                    // Future: emit observability events. Currently a no-op.
                }
                OpsLifecycleEffect::WaitAllSatisfied {
                    wait_request_id,
                    operation_ids,
                } => {
                    if let Some(pending_wait) = self.pending_wait.take() {
                        if pending_wait.wait_request_id == *wait_request_id {
                            let _ = pending_wait.sender.send(WaitAllSatisfied {
                                wait_request_id: wait_request_id.clone(),
                                operation_ids: operation_ids.clone(),
                            });
                        } else {
                            self.pending_wait = Some(pending_wait);
                        }
                    }
                }
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

        let operation_specs: HashMap<OperationId, meerkat_core::ops_lifecycle::OperationSpec> =
            self.records
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

        let snapshot = PersistedOpsSnapshot {
            epoch_id: epoch_id.clone(),
            authority_state: self.authority.canonical_state().clone(),
            operation_specs,
            completion_entries,
            cursors: cursor_state.snapshot(),
        };

        // Non-blocking send — bounded-loss is the acknowledged contract.
        if tx.try_send(snapshot).is_err() {
            tracing::warn!("ops lifecycle persistence channel full or closed; snapshot dropped");
        }
    }

    /// Shadow-validate a DSL input against the ops-local DSL authority.
    ///
    /// Called after each successful handwritten authority transition.
    /// On disagreement, logs `tracing::error!` but does NOT fail the
    /// operation — the handwritten authority remains the real owner.
    fn shadow_validate_dsl(&mut self, input: mm_dsl::MeerkatMachineInput, context: &str) {
        match mm_dsl::MeerkatMachineMutator::apply(&mut self.dsl_shadow.0, input) {
            Ok(transition) => {
                self.dsl_shadow.0.state.lifecycle_phase = transition.to_phase;
            }
            Err(err) => {
                tracing::error!(
                    context = context,
                    error = %err,
                    "DSL shadow DISAGREEMENT in ops lifecycle: authority accepted but DSL rejected"
                );
            }
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
                let outcome = self
                    .authority
                    .operation(operation_id)
                    .and_then(|op| op.terminal_outcome().cloned())
                    .ok_or_else(|| {
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
/// All canonical lifecycle state mutations are delegated to
/// [`OpsLifecycleAuthority`]. This shell manages I/O concerns: watcher
/// channels, timestamps, peer handles, and snapshot assembly.
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

    /// Wire the detached-wake state so that `execute_effects` arms pending
    /// and fires the Notify when a `BackgroundToolOp` reaches terminal.
    pub fn set_detached_wake(&self, wake: Arc<crate::detached_wake::DetachedWakeState>) {
        if let Ok(mut state) = self.state.write() {
            state.detached_wake = Some(wake);
        }
    }

    /// Recover from a persisted snapshot.
    ///
    /// Rebuilds the authority (stripping non-terminal ops), creates fresh
    /// shell records from specs, and seeds the feed buffer with persisted
    /// completion entries.
    pub fn from_recovered(snapshot: PersistedOpsSnapshot) -> Self {
        let authority = OpsLifecycleAuthority::from_recovered(snapshot.authority_state);

        // Seed the feed buffer from persisted entries
        let max_retained = authority
            .canonical_state()
            .max_completed()
            .max(256)
            .saturating_mul(4)
            .max(1024);
        let feed_buffer = Arc::new(FeedBuffer::new(max_retained));
        for entry in &snapshot.completion_entries {
            feed_buffer.push(entry.clone());
        }

        // Rebuild shell records from specs (fresh timestamps, no watchers)
        let mut records = HashMap::new();
        for (op_id, spec) in &snapshot.operation_specs {
            // Only rebuild records for operations still in the authority
            if authority.operation(op_id).is_some() {
                records.insert(
                    op_id.clone(),
                    ShellRecord {
                        spec: spec.clone(),
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

        let state = ShellState {
            authority,
            records,
            pending_wait: None,
            detached_wake: None,
            feed_buffer,
            persist_tx: None,
            persist_epoch_id: None,
            persist_cursor_state: None,
            dsl_shadow: new_ops_dsl_shadow(),
        };

        Self {
            state: RwLock::new(state),
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

        let operation_specs: HashMap<OperationId, meerkat_core::ops_lifecycle::OperationSpec> =
            state
                .records
                .iter()
                .map(|(id, record)| (id.clone(), record.spec.clone()))
                .collect();

        let completion_entries = {
            let inner = state
                .feed_buffer
                .inner
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            inner.entries.iter().cloned().collect()
        };

        let cursors = cursor_state.snapshot();

        PersistedOpsSnapshot {
            epoch_id,
            authority_state: state.authority.canonical_state().clone(),
            operation_specs,
            completion_entries,
            cursors,
        }
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
            .authority
            .operations()
            .filter_map(|(id, _)| state.snapshot(id))
            .collect::<Vec<_>>();
        operations.sort_by(|left, right| left.display_name.cmp(&right.display_name));
        RuntimeOpsDiagnosticSnapshot {
            operation_count: state.authority.operation_count(),
            active_count: state.authority.active_count(),
            wait_request_id: state.authority.wait_request_id().cloned(),
            pending_wait_present: state.pending_wait.is_some(),
            pending_wait_request_id: state
                .pending_wait
                .as_ref()
                .map(|pending_wait| pending_wait.wait_request_id.clone()),
            wait_operation_ids: state.authority.wait_operation_ids().to_vec(),
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
        match state.authority.apply(OpsLifecycleInput::CancelWaitAll {
            wait_request_id: wait_request_id.clone(),
        }) {
            Ok(_) => {
                state.pending_wait = None;
                Ok(())
            }
            Err(OpsLifecycleError::WaitNotActive(_)) => {
                state.pending_wait = None;
                Ok(())
            }
            Err(err) => Err(err),
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

impl OpsLifecycleRegistry for RuntimeOpsLifecycleRegistry {
    fn register_operation(&self, spec: OperationSpec) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;
        let operation_id = spec.id.clone();
        let kind = spec.kind;

        // Delegate to authority for guard checks and canonical state insertion.
        let transition = state
            .authority
            .apply(OpsLifecycleInput::RegisterOperation {
                operation_id: operation_id.clone(),
                kind,
            })?;

        // DSL shadow validation.
        state.shadow_validate_dsl(
            mm_dsl::MeerkatMachineInput::RegisterOp {
                operation_id: mm_dsl::OperationId::from_domain(&operation_id).0,
                kind: mm_dsl::OperationKind::from_domain(&kind).0,
            },
            "RegisterOp",
        );

        // Insert shell record.
        state.records.insert(operation_id, ShellRecord::new(spec));

        // Execute effects (none expected for register, but be correct).
        state.execute_effects(&transition.effects);
        Ok(())
    }

    fn provisioning_succeeded(&self, id: &OperationId) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let transition = state
            .authority
            .apply(OpsLifecycleInput::ProvisioningSucceeded {
                operation_id: id.clone(),
            })?;

        // DSL shadow validation.
        state.shadow_validate_dsl(
            mm_dsl::MeerkatMachineInput::StartOp {
                operation_id: mm_dsl::OperationId::from_domain(id).0,
            },
            "StartOp (provisioning_succeeded)",
        );

        // Shell concern: record the started timestamp.
        if let Some(shell) = state.records.get_mut(id) {
            shell.started_at = Some(Instant::now());
        }

        state.execute_effects(&transition.effects);
        Ok(())
    }

    fn provisioning_failed(
        &self,
        id: &OperationId,
        error: String,
    ) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let transition = state
            .authority
            .apply(OpsLifecycleInput::ProvisioningFailed {
                operation_id: id.clone(),
            })?;

        // DSL shadow validation.
        state.shadow_validate_dsl(
            mm_dsl::MeerkatMachineInput::FailOp {
                operation_id: mm_dsl::OperationId::from_domain(id).0,
            },
            "FailOp (provisioning_failed)",
        );

        // Patch the real terminal outcome (authority uses placeholder).
        state
            .authority
            .patch_terminal_outcome(id, OperationTerminalOutcome::Failed { error });

        state.execute_effects(&transition.effects);
        Ok(())
    }

    fn peer_ready(
        &self,
        id: &OperationId,
        peer: OperationPeerHandle,
    ) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let transition = state.authority.apply(OpsLifecycleInput::PeerReady {
            operation_id: id.clone(),
        })?;

        // Shell concern: store the peer handle.
        if let Some(shell) = state.records.get_mut(id) {
            shell.peer_handle = Some(peer);
        }

        state.execute_effects(&transition.effects);
        Ok(())
    }

    fn register_watcher(
        &self,
        id: &OperationId,
    ) -> Result<OperationCompletionWatch, OpsLifecycleError> {
        let mut state = self.write_state()?;

        // Check authority for terminal outcome first (already-resolved path).
        let canonical = state
            .authority
            .operation(id)
            .ok_or_else(|| OpsLifecycleError::NotFound(id.clone()))?;

        if let Some(outcome) = canonical.terminal_outcome() {
            return Ok(OperationCompletionWatch::already_resolved(outcome.clone()));
        }

        // Shell concern: create the channel and store the sender.
        // Watcher tracking lives in shell.watchers Vec — no authority
        // bookkeeping needed.
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

        let transition = state.authority.apply(OpsLifecycleInput::ProgressReported {
            operation_id: id.clone(),
        })?;

        state.execute_effects(&transition.effects);
        Ok(())
    }

    fn complete_operation(
        &self,
        id: &OperationId,
        result: OperationResult,
    ) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let transition = state
            .authority
            .apply(OpsLifecycleInput::CompleteOperation {
                operation_id: id.clone(),
            })?;

        // DSL shadow validation.
        state.shadow_validate_dsl(
            mm_dsl::MeerkatMachineInput::CompleteOp {
                operation_id: mm_dsl::OperationId::from_domain(id).0,
            },
            "CompleteOp",
        );

        // Patch the real terminal outcome (authority uses placeholder).
        state
            .authority
            .patch_terminal_outcome(id, OperationTerminalOutcome::Completed(result));

        state.execute_effects(&transition.effects);
        state.maybe_persist();
        Ok(())
    }

    fn fail_operation(&self, id: &OperationId, error: String) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let transition = state.authority.apply(OpsLifecycleInput::FailOperation {
            operation_id: id.clone(),
        })?;

        // DSL shadow validation.
        state.shadow_validate_dsl(
            mm_dsl::MeerkatMachineInput::FailOp {
                operation_id: mm_dsl::OperationId::from_domain(id).0,
            },
            "FailOp",
        );

        // Patch the real terminal outcome.
        state
            .authority
            .patch_terminal_outcome(id, OperationTerminalOutcome::Failed { error });

        state.execute_effects(&transition.effects);
        state.maybe_persist();
        Ok(())
    }

    fn abort_provisioning(
        &self,
        id: &OperationId,
        reason: Option<String>,
    ) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let transition = state
            .authority
            .apply(OpsLifecycleInput::AbortProvisioning {
                operation_id: id.clone(),
            })?;

        // DSL shadow validation.
        state.shadow_validate_dsl(
            mm_dsl::MeerkatMachineInput::AbortOp {
                operation_id: mm_dsl::OperationId::from_domain(id).0,
            },
            "AbortOp",
        );

        state
            .authority
            .patch_terminal_outcome(id, OperationTerminalOutcome::Aborted { reason });

        state.execute_effects(&transition.effects);
        state.maybe_persist();
        Ok(())
    }

    fn cancel_operation(
        &self,
        id: &OperationId,
        reason: Option<String>,
    ) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let transition = state.authority.apply(OpsLifecycleInput::CancelOperation {
            operation_id: id.clone(),
        })?;

        // DSL shadow validation.
        state.shadow_validate_dsl(
            mm_dsl::MeerkatMachineInput::CancelOp {
                operation_id: mm_dsl::OperationId::from_domain(id).0,
            },
            "CancelOp",
        );

        // Patch the real terminal outcome.
        state
            .authority
            .patch_terminal_outcome(id, OperationTerminalOutcome::Cancelled { reason });

        state.execute_effects(&transition.effects);
        state.maybe_persist();
        Ok(())
    }

    fn request_retire(&self, id: &OperationId) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let transition = state.authority.apply(OpsLifecycleInput::RetireRequested {
            operation_id: id.clone(),
        })?;

        state.execute_effects(&transition.effects);
        Ok(())
    }

    fn mark_retired(&self, id: &OperationId) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let transition = state.authority.apply(OpsLifecycleInput::RetireCompleted {
            operation_id: id.clone(),
        })?;

        // Patch the real terminal outcome.
        state
            .authority
            .patch_terminal_outcome(id, OperationTerminalOutcome::Retired);

        state.execute_effects(&transition.effects);
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
                    .authority
                    .operations()
                    .filter_map(|(id, _)| state.snapshot(id))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        snapshots.sort_by(|left, right| left.display_name.cmp(&right.display_name));
        snapshots
    }

    fn terminate_owner(&self, reason: String) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let transition = state.authority.apply(OpsLifecycleInput::OwnerTerminated)?;

        // Patch all terminal outcomes with the real reason.
        // The authority set placeholder empty-string reasons; we patch the real
        // reason into each newly-terminated operation.
        for effect in &transition.effects {
            if let OpsLifecycleEffect::NotifyOpWatcher { operation_id, .. } = effect {
                state.authority.patch_terminal_outcome(
                    operation_id,
                    OperationTerminalOutcome::Terminated {
                        reason: reason.clone(),
                    },
                );
            }
        }

        state.execute_effects(&transition.effects);
        Ok(())
    }

    fn collect_completed(
        &self,
    ) -> Result<Vec<(OperationId, OperationTerminalOutcome)>, OpsLifecycleError> {
        let mut state = self.write_state()?;

        let collected = state.authority.drain_completed();

        // Remove corresponding shell records.
        for (id, _) in &collected {
            state.records.remove(id);
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
                let transition = match state.authority.apply(OpsLifecycleInput::BeginWaitAll {
                    wait_request_id: wait_request_id.clone(),
                    operation_ids: owned_ids.clone(),
                }) {
                    Ok(transition) => transition,
                    Err(err) => {
                        return Box::pin(WaitAllFuture {
                            registry: self,
                            wait_request_id,
                            operation_ids: owned_ids,
                            state: WaitAllFutureState::Ready(Some(Err(err))),
                        });
                    }
                };

                let satisfied = transition.effects.iter().find_map(|effect| match effect {
                    OpsLifecycleEffect::WaitAllSatisfied {
                        wait_request_id,
                        operation_ids,
                    } => Some(WaitAllSatisfied {
                        wait_request_id: wait_request_id.clone(),
                        operation_ids: operation_ids.clone(),
                    }),
                    _ => None,
                });

                state.execute_effects(&transition.effects);

                if let Some(satisfied) = satisfied {
                    WaitAllFutureState::Ready(Some(state.collect_wait_outcomes(&owned_ids).map(
                        |outcomes| WaitAllResult {
                            outcomes,
                            satisfied,
                        },
                    )))
                } else {
                    if state.pending_wait.is_some() {
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
        // Authority-derived obligation carries the awaited IDs
        assert_eq!(wait_result.satisfied.operation_ids.len(), 2);
        assert_ne!(wait_result.satisfied.wait_request_id.to_string(), "");
    }

    /// Exercises the trait `wait_all` path (via `dyn OpsLifecycleRegistry`)
    /// which must submit WaitAll through the authority for cross-machine handoff.
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
            let wait_request_id = match state.authority.wait_request_id().cloned() {
                Some(wait_request_id) => wait_request_id,
                None => panic!("wait request should be active"),
            };
            assert_eq!(
                state.authority.wait_operation_ids(),
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
        assert!(
            registry
                .read_state()
                .unwrap()
                .authority
                .wait_request_id()
                .is_none()
        );
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
        assert!(state.authority.wait_request_id().is_none());
        assert!(state.authority.wait_operation_ids().is_empty());
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

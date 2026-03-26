//! Generated-authority module for the OpsLifecycle machine.
//!
//! This module provides typed enums and a sealed mutator trait that enforces
//! all per-operation lifecycle state mutations flow through the machine authority.
//! Handwritten shell code calls [`OpsLifecycleAuthority::apply`] and executes
//! returned effects; it cannot mutate canonical state directly.
//!
//! The transition table encoded here is the single source of truth, matching
//! the machine schema in `meerkat-machine-schema/src/catalog/ops_lifecycle.rs`:
//!
//! - 1 phase (Active) — the registry is always active; per-operation status is
//!   the real state dimension
//! - Per-operation statuses: Absent, Provisioning, Running, Retiring,
//!   Completed, Failed, Aborted, Cancelled, Retired, Terminated
//! - 16 inputs: RegisterOperation, ProvisioningSucceeded, ProvisioningFailed,
//!   AbortProvisioning, PeerReady, RegisterWatcher, ProgressReported,
//!   CompleteOperation, FailOperation, CancelOperation, RetireRequested,
//!   RetireCompleted, CollectTerminal, OwnerTerminated, BeginWaitAll,
//!   CancelWaitAll
//! - 8 effects: SubmitOpEvent, NotifyOpWatcher, ExposeOperationPeer,
//!   RetainTerminalRecord, EvictCompletedRecord, WaitAllSatisfied,
//!   CollectCompletedResult, ConcurrencyLimitExceeded

use meerkat_core::lifecycle::WaitRequestId;
use meerkat_core::ops_lifecycle::{
    OperationId, OperationKind, OperationStatus, OperationTerminalOutcome, OpsLifecycleError,
};

// ---------------------------------------------------------------------------
// Typed input enum — mirrors the machine schema's input variants
// ---------------------------------------------------------------------------

/// Typed inputs for per-operation lifecycle transitions.
///
/// Shell code classifies raw commands into these typed inputs, then calls
/// [`OpsLifecycleAuthority::apply`]. The authority decides transition legality.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OpsLifecycleInput {
    /// Register a new operation (Absent -> Provisioning).
    RegisterOperation {
        operation_id: OperationId,
        kind: OperationKind,
    },
    /// Provisioning succeeded (Provisioning -> Running).
    ProvisioningSucceeded { operation_id: OperationId },
    /// Provisioning failed (Provisioning -> Failed).
    ProvisioningFailed { operation_id: OperationId },
    /// Provisioning was intentionally aborted (Provisioning -> Aborted).
    AbortProvisioning { operation_id: OperationId },
    /// Peer is ready for a mob-member-child operation (Running|Retiring, must be MobMemberChild).
    PeerReady { operation_id: OperationId },
    /// Register a completion watcher (operation must exist).
    RegisterWatcher { operation_id: OperationId },
    /// Progress reported (Running|Retiring).
    ProgressReported { operation_id: OperationId },
    /// Complete the operation (Running|Retiring -> Completed).
    CompleteOperation { operation_id: OperationId },
    /// Fail the operation (Provisioning|Running|Retiring -> Failed).
    FailOperation { operation_id: OperationId },
    /// Cancel the operation (Provisioning|Running|Retiring -> Cancelled).
    CancelOperation { operation_id: OperationId },
    /// Request retire (Running -> Retiring).
    RetireRequested { operation_id: OperationId },
    /// Retire completed (Retiring -> Retired).
    RetireCompleted { operation_id: OperationId },
    /// Collect a terminal operation's buffered outcome.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "schema-aligned authority input retained even when current shell paths do not construct it"
        )
    )]
    #[cfg_attr(test, allow(dead_code))]
    CollectTerminal { operation_id: OperationId },
    /// Terminate all non-terminal operations.
    OwnerTerminated,
    /// Begin an authority-owned barrier wait for the specified operations.
    ///
    /// Emits `WaitAllSatisfied` immediately if all operations are already
    /// terminal; otherwise stores the outstanding wait until later terminal
    /// transitions satisfy it.
    BeginWaitAll {
        wait_request_id: WaitRequestId,
        operation_ids: Vec<OperationId>,
    },
    /// Cancel the currently active wait request.
    CancelWaitAll { wait_request_id: WaitRequestId },
}

// ---------------------------------------------------------------------------
// Typed effect enum — mirrors the machine schema's effect variants
// ---------------------------------------------------------------------------

/// Effects emitted by OpsLifecycle transitions.
///
/// Shell code receives these from [`OpsLifecycleAuthority::apply`] and is
/// responsible for executing the side effects (e.g. sending watcher
/// notifications, managing channels).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OpsLifecycleEffect {
    /// Emit an operation event (status change notification).
    SubmitOpEvent {
        operation_id: OperationId,
        event_kind: OpEventKind,
    },
    /// Notify all watchers with a terminal outcome.
    NotifyOpWatcher {
        operation_id: OperationId,
        terminal_outcome: OperationTerminalOutcome,
    },
    /// Expose a peer connection for a mob-member-child operation.
    ExposeOperationPeer { operation_id: OperationId },
    /// Record a terminal outcome in the completed buffer.
    RetainTerminalRecord {
        operation_id: OperationId,
        terminal_outcome: OperationTerminalOutcome,
    },
    /// Evict a completed operation from retention.
    EvictCompletedRecord { operation_id: OperationId },
    /// All specified operations have reached terminal status.
    ///
    /// Handoff protocol: `ops_barrier_satisfaction` — the shell routes this
    /// to TurnExecution's `OpsBarrierSatisfied` input.
    WaitAllSatisfied {
        wait_request_id: WaitRequestId,
        operation_ids: Vec<OperationId>,
    },
}

/// Event kind for SubmitOpEvent effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpEventKind {
    Started,
    Progress,
    Completed,
    Failed,
    Aborted,
    Cancelled,
    Retired,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "schema-aligned operation event retained for future authority wiring"
        )
    )]
    #[cfg_attr(test, allow(dead_code))]
    Terminated,
}

// ---------------------------------------------------------------------------
// Transition result
// ---------------------------------------------------------------------------

/// Successful transition outcome from the OpsLifecycle authority.
#[derive(Debug)]
pub(crate) struct OpsLifecycleTransition {
    /// Effects to be executed by shell code.
    pub effects: Vec<OpsLifecycleEffect>,
}

// ---------------------------------------------------------------------------
// Per-operation canonical state
// ---------------------------------------------------------------------------

/// Canonical machine-owned state for a single operation.
///
/// This is exclusively owned by the authority. Shell code may read via
/// accessor methods but cannot mutate directly.
#[derive(Debug, Clone)]
pub(crate) struct OperationCanonicalState {
    /// Current lifecycle status.
    status: OperationStatus,
    /// Kind of operation (needed for peer_ready guard).
    kind: OperationKind,
    /// Whether a peer channel has been exposed.
    peer_ready: bool,
    /// Cumulative progress report count.
    progress_count: u32,
    /// Number of active watchers.
    watcher_count: u32,
    /// Terminal outcome (set when status becomes terminal).
    terminal_outcome: Option<OperationTerminalOutcome>,
    /// Whether the terminal outcome is buffered for collection.
    terminal_buffered: bool,
}

impl OperationCanonicalState {
    /// Current status.
    pub(crate) fn status(&self) -> OperationStatus {
        self.status
    }

    /// Operation kind.
    pub(crate) fn kind(&self) -> OperationKind {
        self.kind
    }

    /// Whether a peer channel has been exposed.
    pub(crate) fn peer_ready(&self) -> bool {
        self.peer_ready
    }

    /// Cumulative progress report count.
    pub(crate) fn progress_count(&self) -> u32 {
        self.progress_count
    }

    /// Number of active watchers.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "authority inspection helper retained for future waiter wiring"
        )
    )]
    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn watcher_count(&self) -> u32 {
        self.watcher_count
    }

    /// Terminal outcome, if in terminal state.
    pub(crate) fn terminal_outcome(&self) -> Option<&OperationTerminalOutcome> {
        self.terminal_outcome.as_ref()
    }

    /// Whether the terminal outcome is buffered for collection.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "authority inspection helper retained for future buffered terminal collection"
        )
    )]
    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn terminal_buffered(&self) -> bool {
        self.terminal_buffered
    }
}

// ---------------------------------------------------------------------------
// Registry-level canonical state
// ---------------------------------------------------------------------------

/// Canonical machine-owned state for the ops lifecycle registry.
///
/// Tracks per-operation state plus registry-level bookkeeping (completed
/// ordering, concurrency limits). Only mutated through the sealed authority.
#[derive(Debug, Clone)]
struct RegistryCanonicalState {
    /// Per-operation canonical state, keyed by OperationId.
    operations: std::collections::HashMap<OperationId, OperationCanonicalState>,
    /// FIFO ordering of completed operation IDs for bounded eviction.
    completed_order: std::collections::VecDeque<OperationId>,
    /// Maximum completed operations to retain.
    max_completed: usize,
    /// Maximum concurrent non-terminal operations (None = unlimited).
    max_concurrent: Option<usize>,
    /// Count of currently non-terminal operations.
    active_count: usize,
    /// Currently active barrier wait request, if any.
    wait_request_id: Option<WaitRequestId>,
    /// Operation IDs tracked by the active barrier wait.
    wait_operation_ids: Vec<OperationId>,
}

impl RegistryCanonicalState {
    fn new(max_completed: usize, max_concurrent: Option<usize>) -> Self {
        Self {
            operations: std::collections::HashMap::new(),
            completed_order: std::collections::VecDeque::new(),
            max_completed,
            max_concurrent,
            active_count: 0,
            wait_request_id: None,
            wait_operation_ids: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Sealed mutator trait — only the authority implements this
// ---------------------------------------------------------------------------

mod sealed {
    pub trait Sealed {}
}

/// Sealed trait for OpsLifecycle state mutation.
///
/// Only [`OpsLifecycleAuthority`] implements this. Handwritten code cannot
/// create alternative implementations, ensuring single-source-of-truth
/// semantics for operation lifecycle state.
pub(crate) trait OpsLifecycleMutator: sealed::Sealed {
    /// Apply a typed input to the current machine state.
    ///
    /// Returns the transition result including effects to execute,
    /// or an error if the transition is not legal.
    fn apply(
        &mut self,
        input: OpsLifecycleInput,
    ) -> Result<OpsLifecycleTransition, OpsLifecycleError>;
}

// ---------------------------------------------------------------------------
// Authority implementation
// ---------------------------------------------------------------------------

/// The canonical authority for OpsLifecycle state.
///
/// Holds registry-level canonical state (per-operation states, completed
/// ordering, concurrency limits) and delegates all transitions through
/// the encoded transition table.
#[derive(Debug)]
pub(crate) struct OpsLifecycleAuthority {
    state: RegistryCanonicalState,
}

impl sealed::Sealed for OpsLifecycleAuthority {}

impl OpsLifecycleAuthority {
    /// Create a new authority with the given configuration.
    pub(crate) fn new(max_completed: usize, max_concurrent: Option<usize>) -> Self {
        Self {
            state: RegistryCanonicalState::new(max_completed, max_concurrent),
        }
    }

    /// Get canonical state for an operation (read-only).
    pub(crate) fn operation(&self, id: &OperationId) -> Option<&OperationCanonicalState> {
        self.state.operations.get(id)
    }

    /// Check if an operation exists.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "authority helper retained for schema-aligned registry inspection"
        )
    )]
    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn contains(&self, id: &OperationId) -> bool {
        self.state.operations.contains_key(id)
    }

    /// Number of tracked operations (including terminal).
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "authority helper retained for schema-aligned registry inspection"
        )
    )]
    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn operation_count(&self) -> usize {
        self.state.operations.len()
    }

    /// Number of non-terminal operations.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "authority helper retained for schema-aligned registry inspection"
        )
    )]
    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn active_count(&self) -> usize {
        self.state.active_count
    }

    /// Currently active barrier wait request, if any.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "authority helper retained for schema-aligned registry inspection"
        )
    )]
    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn wait_request_id(&self) -> Option<&WaitRequestId> {
        self.state.wait_request_id.as_ref()
    }

    /// Operation IDs tracked by the currently active barrier wait.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "authority helper retained for schema-aligned registry inspection"
        )
    )]
    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn wait_operation_ids(&self) -> &[OperationId] {
        &self.state.wait_operation_ids
    }

    /// Iterate over all operations' canonical state.
    pub(crate) fn operations(
        &self,
    ) -> impl Iterator<Item = (&OperationId, &OperationCanonicalState)> {
        self.state.operations.iter()
    }

    /// Notify the authority that a watcher was drained (after terminal
    /// notification). This decrements the watcher count.
    ///
    /// Called by shell code after executing NotifyOpWatcher effects.
    pub(crate) fn watchers_drained(&mut self, id: &OperationId, count: u32) {
        if let Some(op) = self.state.operations.get_mut(id) {
            op.watcher_count = op.watcher_count.saturating_sub(count);
        }
    }

    /// Remove an operation from tracking entirely.
    ///
    /// Called by shell code after executing EvictCompletedRecord or
    /// CollectTerminal effects to actually remove the record.
    pub(crate) fn remove_operation(&mut self, id: &OperationId) {
        self.state.operations.remove(id);
    }

    /// Patch the terminal outcome of a completed operation.
    ///
    /// The authority uses placeholder terminal outcomes (empty strings) because
    /// the typed input enum doesn't carry the full caller-provided data
    /// (OperationResult, error string, reason). After a successful terminal
    /// transition, the shell patches the real outcome here.
    ///
    /// This is NOT a state transition — it does not change status or guards.
    /// It only enriches the opaque outcome data.
    pub(crate) fn patch_terminal_outcome(
        &mut self,
        id: &OperationId,
        outcome: OperationTerminalOutcome,
    ) {
        if let Some(op) = self.state.operations.get_mut(id) {
            op.terminal_outcome = Some(outcome);
        }
    }

    /// Drain completed_order and remove corresponding terminal operations.
    ///
    /// Returns the IDs and terminal outcomes of drained operations.
    /// Called by shell code to implement collect_completed.
    pub(crate) fn drain_completed(&mut self) -> Vec<(OperationId, OperationTerminalOutcome)> {
        let mut collected = Vec::new();
        let ids: Vec<OperationId> = self.state.completed_order.drain(..).collect();
        for id in ids {
            if let Some(op) = self.state.operations.remove(&id)
                && let Some(outcome) = op.terminal_outcome
            {
                collected.push((id, outcome));
            }
        }
        collected
    }

    fn wait_active(&self) -> bool {
        self.state.wait_request_id.is_some()
    }

    fn all_operations_known(&self, operation_ids: &[OperationId]) -> Result<(), OpsLifecycleError> {
        for operation_id in operation_ids {
            if !self.state.operations.contains_key(operation_id) {
                return Err(OpsLifecycleError::NotFound(operation_id.clone()));
            }
        }
        Ok(())
    }

    fn ensure_no_duplicate_operation_ids(
        &self,
        operation_ids: &[OperationId],
    ) -> Result<(), OpsLifecycleError> {
        let mut seen = std::collections::HashSet::new();
        for operation_id in operation_ids {
            if !seen.insert(operation_id.clone()) {
                return Err(OpsLifecycleError::DuplicateWaitOperation(
                    operation_id.clone(),
                ));
            }
        }
        Ok(())
    }

    fn all_operations_terminal(&self, operation_ids: &[OperationId]) -> bool {
        operation_ids.iter().all(|operation_id| {
            self.state
                .operations
                .get(operation_id)
                .is_some_and(|op| op.status.is_terminal())
        })
    }

    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "authority helper retained for schema-aligned registry inspection"
        )
    )]
    #[cfg_attr(test, allow(dead_code))]
    fn wait_tracks_operation(&self, operation_id: &OperationId) -> bool {
        self.state.wait_operation_ids.contains(operation_id)
    }

    fn maybe_complete_wait(&mut self, effects: &mut Vec<OpsLifecycleEffect>) {
        if !self.wait_active() || !self.all_operations_terminal(&self.state.wait_operation_ids) {
            return;
        }

        let Some(wait_request_id) = self.state.wait_request_id.take() else {
            return;
        };
        let operation_ids = std::mem::take(&mut self.state.wait_operation_ids);
        effects.push(OpsLifecycleEffect::WaitAllSatisfied {
            wait_request_id,
            operation_ids,
        });
    }

    /// Evaluate and apply a transition for a terminal operation input.
    ///
    /// Shared logic for CompleteOperation, FailOperation, CancelOperation,
    /// ProvisioningFailed, RetireCompleted.
    fn apply_terminal(
        &mut self,
        operation_id: &OperationId,
        allowed_from: &[OperationStatus],
        terminal_status: OperationStatus,
        terminal_outcome: OperationTerminalOutcome,
        event_kind: OpEventKind,
    ) -> Result<OpsLifecycleTransition, OpsLifecycleError> {
        let op = self
            .state
            .operations
            .get(operation_id)
            .ok_or_else(|| OpsLifecycleError::NotFound(operation_id.clone()))?;

        if !allowed_from.contains(&op.status) {
            return Err(OpsLifecycleError::InvalidTransition {
                id: operation_id.clone(),
                status: op.status,
                action: terminal_status_action(terminal_status),
            });
        }

        let mut effects = vec![
            OpsLifecycleEffect::SubmitOpEvent {
                operation_id: operation_id.clone(),
                event_kind,
            },
            OpsLifecycleEffect::NotifyOpWatcher {
                operation_id: operation_id.clone(),
                terminal_outcome: terminal_outcome.clone(),
            },
            OpsLifecycleEffect::RetainTerminalRecord {
                operation_id: operation_id.clone(),
                terminal_outcome: terminal_outcome.clone(),
            },
        ];

        // Commit state changes.
        let op = self
            .state
            .operations
            .get_mut(operation_id)
            .ok_or_else(|| OpsLifecycleError::NotFound(operation_id.clone()))?;
        op.status = terminal_status;
        op.terminal_outcome = Some(terminal_outcome);
        op.terminal_buffered = true;

        // Decrement active count.
        self.state.active_count = self.state.active_count.saturating_sub(1);

        // Append to completed ordering.
        self.state.completed_order.push_back(operation_id.clone());

        self.maybe_complete_wait(&mut effects);

        // Evict oldest completed if over limit.
        self.evict_completed(&mut effects);

        Ok(OpsLifecycleTransition { effects })
    }

    /// Evict completed operations that exceed max_completed, appending
    /// EvictCompletedRecord effects for each evicted operation.
    fn evict_completed(&mut self, effects: &mut Vec<OpsLifecycleEffect>) {
        while self.state.completed_order.len() > self.state.max_completed {
            if let Some(evicted_id) = self.state.completed_order.pop_front()
                && self
                    .state
                    .operations
                    .get(&evicted_id)
                    .is_some_and(|op| op.status.is_terminal())
            {
                self.state.operations.remove(&evicted_id);
                effects.push(OpsLifecycleEffect::EvictCompletedRecord {
                    operation_id: evicted_id,
                });
            }
        }
    }
}

impl OpsLifecycleMutator for OpsLifecycleAuthority {
    fn apply(
        &mut self,
        input: OpsLifecycleInput,
    ) -> Result<OpsLifecycleTransition, OpsLifecycleError> {
        match input {
            OpsLifecycleInput::RegisterOperation { operation_id, kind } => {
                // Guard: operation must be absent.
                if self.state.operations.contains_key(&operation_id) {
                    return Err(OpsLifecycleError::AlreadyRegistered(operation_id));
                }

                // Guard: concurrency limit.
                if let Some(limit) = self.state.max_concurrent
                    && self.state.active_count >= limit
                {
                    return Err(OpsLifecycleError::MaxConcurrentExceeded {
                        limit,
                        active: self.state.active_count,
                    });
                }

                // Commit: insert new operation in Provisioning.
                self.state.operations.insert(
                    operation_id,
                    OperationCanonicalState {
                        status: OperationStatus::Provisioning,
                        kind,
                        peer_ready: false,
                        progress_count: 0,
                        watcher_count: 0,
                        terminal_outcome: None,
                        terminal_buffered: false,
                    },
                );
                self.state.active_count += 1;

                Ok(OpsLifecycleTransition {
                    effects: Vec::new(),
                })
            }

            OpsLifecycleInput::ProvisioningSucceeded { operation_id } => {
                let op = self
                    .state
                    .operations
                    .get(&operation_id)
                    .ok_or_else(|| OpsLifecycleError::NotFound(operation_id.clone()))?;

                if op.status != OperationStatus::Provisioning {
                    return Err(OpsLifecycleError::InvalidTransition {
                        id: operation_id.clone(),
                        status: op.status,
                        action: "provisioning_succeeded",
                    });
                }

                // Commit.
                let op = self
                    .state
                    .operations
                    .get_mut(&operation_id)
                    .ok_or_else(|| OpsLifecycleError::NotFound(operation_id.clone()))?;
                op.status = OperationStatus::Running;

                Ok(OpsLifecycleTransition {
                    effects: vec![OpsLifecycleEffect::SubmitOpEvent {
                        operation_id,
                        event_kind: OpEventKind::Started,
                    }],
                })
            }

            OpsLifecycleInput::ProvisioningFailed { operation_id } => self.apply_terminal(
                &operation_id,
                &[OperationStatus::Provisioning],
                OperationStatus::Failed,
                OperationTerminalOutcome::Failed {
                    error: String::new(),
                },
                OpEventKind::Failed,
            ),

            OpsLifecycleInput::AbortProvisioning { operation_id } => self.apply_terminal(
                &operation_id,
                &[OperationStatus::Provisioning],
                OperationStatus::Aborted,
                OperationTerminalOutcome::Aborted { reason: None },
                OpEventKind::Aborted,
            ),

            OpsLifecycleInput::PeerReady { operation_id } => {
                let op = self
                    .state
                    .operations
                    .get(&operation_id)
                    .ok_or_else(|| OpsLifecycleError::NotFound(operation_id.clone()))?;

                // Guard: must be MobMemberChild.
                if op.kind != OperationKind::MobMemberChild {
                    return Err(OpsLifecycleError::PeerNotExpected(operation_id));
                }

                // Guard: must not already be peer-ready.
                if op.peer_ready {
                    return Err(OpsLifecycleError::AlreadyPeerReady(operation_id));
                }

                // Guard: must be Running or Retiring.
                if !matches!(
                    op.status,
                    OperationStatus::Running | OperationStatus::Retiring
                ) {
                    return Err(OpsLifecycleError::InvalidTransition {
                        id: operation_id.clone(),
                        status: op.status,
                        action: "peer_ready",
                    });
                }

                // Commit.
                let op = self
                    .state
                    .operations
                    .get_mut(&operation_id)
                    .ok_or_else(|| OpsLifecycleError::NotFound(operation_id.clone()))?;
                op.peer_ready = true;

                Ok(OpsLifecycleTransition {
                    effects: vec![OpsLifecycleEffect::ExposeOperationPeer { operation_id }],
                })
            }

            OpsLifecycleInput::RegisterWatcher { operation_id } => {
                let op = self
                    .state
                    .operations
                    .get(&operation_id)
                    .ok_or_else(|| OpsLifecycleError::NotFound(operation_id.clone()))?;

                // If already terminal, the shell will return an already-resolved watch.
                // We still increment watcher_count so the snapshot is accurate until
                // the shell drains it.
                let _ = op; // existence check passed

                let op = self
                    .state
                    .operations
                    .get_mut(&operation_id)
                    .ok_or_else(|| OpsLifecycleError::NotFound(operation_id.clone()))?;
                op.watcher_count = op.watcher_count.saturating_add(1);

                Ok(OpsLifecycleTransition {
                    effects: Vec::new(),
                })
            }

            OpsLifecycleInput::ProgressReported { operation_id } => {
                let op = self
                    .state
                    .operations
                    .get(&operation_id)
                    .ok_or_else(|| OpsLifecycleError::NotFound(operation_id.clone()))?;

                if !matches!(
                    op.status,
                    OperationStatus::Running | OperationStatus::Retiring
                ) {
                    return Err(OpsLifecycleError::InvalidTransition {
                        id: operation_id.clone(),
                        status: op.status,
                        action: "report_progress",
                    });
                }

                // Commit.
                let op = self
                    .state
                    .operations
                    .get_mut(&operation_id)
                    .ok_or_else(|| OpsLifecycleError::NotFound(operation_id.clone()))?;
                op.progress_count = op.progress_count.saturating_add(1);

                Ok(OpsLifecycleTransition {
                    effects: vec![OpsLifecycleEffect::SubmitOpEvent {
                        operation_id,
                        event_kind: OpEventKind::Progress,
                    }],
                })
            }

            OpsLifecycleInput::CompleteOperation { operation_id } => self.apply_terminal(
                &operation_id,
                &[OperationStatus::Running, OperationStatus::Retiring],
                OperationStatus::Completed,
                OperationTerminalOutcome::Completed(meerkat_core::ops_lifecycle::OperationResult {
                    id: operation_id.clone(),
                    content: String::new(),
                    is_error: false,
                    duration_ms: 0,
                    tokens_used: 0,
                }),
                OpEventKind::Completed,
            ),

            OpsLifecycleInput::FailOperation { operation_id } => self.apply_terminal(
                &operation_id,
                &[
                    OperationStatus::Provisioning,
                    OperationStatus::Running,
                    OperationStatus::Retiring,
                ],
                OperationStatus::Failed,
                OperationTerminalOutcome::Failed {
                    error: String::new(),
                },
                OpEventKind::Failed,
            ),

            OpsLifecycleInput::CancelOperation { operation_id } => self.apply_terminal(
                &operation_id,
                &[
                    OperationStatus::Provisioning,
                    OperationStatus::Running,
                    OperationStatus::Retiring,
                ],
                OperationStatus::Cancelled,
                OperationTerminalOutcome::Cancelled { reason: None },
                OpEventKind::Cancelled,
            ),

            OpsLifecycleInput::RetireRequested { operation_id } => {
                let op = self
                    .state
                    .operations
                    .get(&operation_id)
                    .ok_or_else(|| OpsLifecycleError::NotFound(operation_id.clone()))?;

                if op.status != OperationStatus::Running {
                    return Err(OpsLifecycleError::InvalidTransition {
                        id: operation_id.clone(),
                        status: op.status,
                        action: "request_retire",
                    });
                }

                // Commit.
                let op = self
                    .state
                    .operations
                    .get_mut(&operation_id)
                    .ok_or_else(|| OpsLifecycleError::NotFound(operation_id.clone()))?;
                op.status = OperationStatus::Retiring;

                Ok(OpsLifecycleTransition {
                    effects: Vec::new(),
                })
            }

            OpsLifecycleInput::RetireCompleted { operation_id } => self.apply_terminal(
                &operation_id,
                &[OperationStatus::Running, OperationStatus::Retiring],
                OperationStatus::Retired,
                OperationTerminalOutcome::Retired,
                OpEventKind::Retired,
            ),

            OpsLifecycleInput::CollectTerminal { operation_id } => {
                let op = self
                    .state
                    .operations
                    .get(&operation_id)
                    .ok_or_else(|| OpsLifecycleError::NotFound(operation_id.clone()))?;

                if !op.status.is_terminal() || !op.terminal_buffered {
                    return Err(OpsLifecycleError::InvalidTransition {
                        id: operation_id.clone(),
                        status: op.status,
                        action: "collect_terminal",
                    });
                }

                // Commit: mark as no longer buffered.
                let op = self
                    .state
                    .operations
                    .get_mut(&operation_id)
                    .ok_or_else(|| OpsLifecycleError::NotFound(operation_id.clone()))?;
                op.terminal_buffered = false;

                Ok(OpsLifecycleTransition {
                    effects: Vec::new(),
                })
            }

            OpsLifecycleInput::OwnerTerminated => {
                let mut effects = Vec::new();
                let to_terminate: Vec<OperationId> = self
                    .state
                    .operations
                    .iter()
                    .filter(|(_, op)| !op.status.is_terminal())
                    .map(|(id, _)| id.clone())
                    .collect();

                for id in &to_terminate {
                    if let Some(op) = self.state.operations.get_mut(id) {
                        let outcome = OperationTerminalOutcome::Terminated {
                            reason: String::new(),
                        };
                        op.status = OperationStatus::Terminated;
                        op.terminal_outcome = Some(outcome.clone());
                        op.terminal_buffered = true;

                        effects.push(OpsLifecycleEffect::NotifyOpWatcher {
                            operation_id: id.clone(),
                            terminal_outcome: outcome.clone(),
                        });
                        effects.push(OpsLifecycleEffect::RetainTerminalRecord {
                            operation_id: id.clone(),
                            terminal_outcome: outcome,
                        });
                    }
                    self.state.completed_order.push_back(id.clone());
                }

                self.state.active_count = 0;
                self.maybe_complete_wait(&mut effects);
                self.evict_completed(&mut effects);

                Ok(OpsLifecycleTransition { effects })
            }

            OpsLifecycleInput::BeginWaitAll {
                wait_request_id,
                operation_ids,
            } => {
                if self.wait_active() {
                    return Err(OpsLifecycleError::WaitAlreadyActive);
                }

                self.ensure_no_duplicate_operation_ids(&operation_ids)?;
                self.all_operations_known(&operation_ids)?;

                if self.all_operations_terminal(&operation_ids) {
                    return Ok(OpsLifecycleTransition {
                        effects: vec![OpsLifecycleEffect::WaitAllSatisfied {
                            wait_request_id,
                            operation_ids,
                        }],
                    });
                }

                self.state.wait_request_id = Some(wait_request_id);
                self.state.wait_operation_ids = operation_ids;

                Ok(OpsLifecycleTransition {
                    effects: Vec::new(),
                })
            }

            OpsLifecycleInput::CancelWaitAll { wait_request_id } => {
                match self.state.wait_request_id.as_ref() {
                    Some(active_wait_request_id) if *active_wait_request_id == wait_request_id => {
                        self.state.wait_request_id = None;
                        self.state.wait_operation_ids.clear();
                        Ok(OpsLifecycleTransition {
                            effects: Vec::new(),
                        })
                    }
                    _ => Err(OpsLifecycleError::WaitNotActive(wait_request_id)),
                }
            }
        }
    }
}

/// Map terminal status to action string for error reporting.
fn terminal_status_action(status: OperationStatus) -> &'static str {
    match status {
        OperationStatus::Completed => "complete_operation",
        OperationStatus::Failed => "fail_operation",
        OperationStatus::Aborted => "abort_provisioning",
        OperationStatus::Cancelled => "cancel_operation",
        OperationStatus::Retired => "mark_retired",
        OperationStatus::Terminated => "terminate_owner",
        _ => "unknown_terminal",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::lifecycle::WaitRequestId;
    use meerkat_core::ops_lifecycle::DEFAULT_MAX_COMPLETED;
    use uuid::Uuid;

    fn test_wait_request_id() -> WaitRequestId {
        WaitRequestId(Uuid::from_u128(1))
    }

    fn make_authority() -> OpsLifecycleAuthority {
        OpsLifecycleAuthority::new(DEFAULT_MAX_COMPLETED, None)
    }

    fn register(auth: &mut OpsLifecycleAuthority, id: &OperationId, kind: OperationKind) {
        auth.apply(OpsLifecycleInput::RegisterOperation {
            operation_id: id.clone(),
            kind,
        })
        .unwrap();
    }

    fn provision_succeed(auth: &mut OpsLifecycleAuthority, id: &OperationId) {
        auth.apply(OpsLifecycleInput::ProvisioningSucceeded {
            operation_id: id.clone(),
        })
        .unwrap();
    }

    // ---- RegisterOperation ----

    #[test]
    fn register_operation_creates_provisioning_state() {
        let mut auth = make_authority();
        let id = OperationId::new();
        register(&mut auth, &id, OperationKind::BackgroundToolOp);

        let op = auth.operation(&id).unwrap();
        assert_eq!(op.status(), OperationStatus::Provisioning);
        assert_eq!(op.kind(), OperationKind::BackgroundToolOp);
        assert!(!op.peer_ready());
        assert_eq!(op.progress_count(), 0);
        assert_eq!(op.watcher_count(), 0);
        assert!(op.terminal_outcome().is_none());
        assert!(!op.terminal_buffered());
        assert_eq!(auth.active_count(), 1);
    }

    #[test]
    fn register_operation_rejects_duplicate() {
        let mut auth = make_authority();
        let id = OperationId::new();
        register(&mut auth, &id, OperationKind::BackgroundToolOp);

        let result = auth.apply(OpsLifecycleInput::RegisterOperation {
            operation_id: id.clone(),
            kind: OperationKind::BackgroundToolOp,
        });
        assert!(matches!(
            result,
            Err(OpsLifecycleError::AlreadyRegistered(_))
        ));
    }

    #[test]
    fn register_operation_enforces_concurrency_limit() {
        let mut auth = OpsLifecycleAuthority::new(DEFAULT_MAX_COMPLETED, Some(2));

        let id1 = OperationId::new();
        let id2 = OperationId::new();
        register(&mut auth, &id1, OperationKind::BackgroundToolOp);
        register(&mut auth, &id2, OperationKind::BackgroundToolOp);

        let id3 = OperationId::new();
        let result = auth.apply(OpsLifecycleInput::RegisterOperation {
            operation_id: id3,
            kind: OperationKind::BackgroundToolOp,
        });
        assert!(matches!(
            result,
            Err(OpsLifecycleError::MaxConcurrentExceeded {
                limit: 2,
                active: 2
            })
        ));
    }

    // ---- ProvisioningSucceeded ----

    #[test]
    fn provisioning_succeeded_transitions_to_running() {
        let mut auth = make_authority();
        let id = OperationId::new();
        register(&mut auth, &id, OperationKind::BackgroundToolOp);

        let t = auth
            .apply(OpsLifecycleInput::ProvisioningSucceeded {
                operation_id: id.clone(),
            })
            .unwrap();

        assert_eq!(
            auth.operation(&id).unwrap().status(),
            OperationStatus::Running
        );
        assert!(t.effects.iter().any(|e| matches!(
            e,
            OpsLifecycleEffect::SubmitOpEvent {
                event_kind: OpEventKind::Started,
                ..
            }
        )));
    }

    #[test]
    fn provisioning_succeeded_rejects_non_provisioning() {
        let mut auth = make_authority();
        let id = OperationId::new();
        register(&mut auth, &id, OperationKind::BackgroundToolOp);
        provision_succeed(&mut auth, &id);

        let result = auth.apply(OpsLifecycleInput::ProvisioningSucceeded { operation_id: id });
        assert!(matches!(
            result,
            Err(OpsLifecycleError::InvalidTransition { .. })
        ));
    }

    // ---- ProvisioningFailed ----

    #[test]
    fn provisioning_failed_transitions_to_failed() {
        let mut auth = make_authority();
        let id = OperationId::new();
        register(&mut auth, &id, OperationKind::BackgroundToolOp);

        let t = auth
            .apply(OpsLifecycleInput::ProvisioningFailed {
                operation_id: id.clone(),
            })
            .unwrap();

        assert_eq!(
            auth.operation(&id).unwrap().status(),
            OperationStatus::Failed
        );
        assert!(
            t.effects
                .iter()
                .any(|e| matches!(e, OpsLifecycleEffect::NotifyOpWatcher { .. }))
        );
    }

    // ---- PeerReady ----

    #[test]
    fn peer_ready_sets_peer_ready_for_mob_member() {
        let mut auth = make_authority();
        let id = OperationId::new();
        register(&mut auth, &id, OperationKind::MobMemberChild);
        provision_succeed(&mut auth, &id);

        let t = auth
            .apply(OpsLifecycleInput::PeerReady {
                operation_id: id.clone(),
            })
            .unwrap();

        assert!(auth.operation(&id).unwrap().peer_ready());
        assert!(
            t.effects
                .iter()
                .any(|e| matches!(e, OpsLifecycleEffect::ExposeOperationPeer { .. }))
        );
    }

    #[test]
    fn peer_ready_rejects_non_mob_member() {
        let mut auth = make_authority();
        let id = OperationId::new();
        register(&mut auth, &id, OperationKind::BackgroundToolOp);
        provision_succeed(&mut auth, &id);

        let result = auth.apply(OpsLifecycleInput::PeerReady { operation_id: id });
        assert!(matches!(result, Err(OpsLifecycleError::PeerNotExpected(_))));
    }

    #[test]
    fn peer_ready_rejects_already_ready() {
        let mut auth = make_authority();
        let id = OperationId::new();
        register(&mut auth, &id, OperationKind::MobMemberChild);
        provision_succeed(&mut auth, &id);
        auth.apply(OpsLifecycleInput::PeerReady {
            operation_id: id.clone(),
        })
        .unwrap();

        let result = auth.apply(OpsLifecycleInput::PeerReady { operation_id: id });
        assert!(matches!(
            result,
            Err(OpsLifecycleError::AlreadyPeerReady(_))
        ));
    }

    #[test]
    fn peer_ready_rejects_non_running() {
        let mut auth = make_authority();
        let id = OperationId::new();
        register(&mut auth, &id, OperationKind::MobMemberChild);
        // Still Provisioning — not Running.

        let result = auth.apply(OpsLifecycleInput::PeerReady { operation_id: id });
        assert!(matches!(
            result,
            Err(OpsLifecycleError::InvalidTransition { .. })
        ));
    }

    // ---- RegisterWatcher ----

    #[test]
    fn register_watcher_increments_count() {
        let mut auth = make_authority();
        let id = OperationId::new();
        register(&mut auth, &id, OperationKind::BackgroundToolOp);

        auth.apply(OpsLifecycleInput::RegisterWatcher {
            operation_id: id.clone(),
        })
        .unwrap();
        auth.apply(OpsLifecycleInput::RegisterWatcher {
            operation_id: id.clone(),
        })
        .unwrap();

        assert_eq!(auth.operation(&id).unwrap().watcher_count(), 2);
    }

    #[test]
    fn register_watcher_rejects_unknown() {
        let mut auth = make_authority();
        let id = OperationId::new();
        let result = auth.apply(OpsLifecycleInput::RegisterWatcher { operation_id: id });
        assert!(matches!(result, Err(OpsLifecycleError::NotFound(_))));
    }

    // ---- ProgressReported ----

    #[test]
    fn progress_reported_increments_count() {
        let mut auth = make_authority();
        let id = OperationId::new();
        register(&mut auth, &id, OperationKind::BackgroundToolOp);
        provision_succeed(&mut auth, &id);

        auth.apply(OpsLifecycleInput::ProgressReported {
            operation_id: id.clone(),
        })
        .unwrap();
        auth.apply(OpsLifecycleInput::ProgressReported {
            operation_id: id.clone(),
        })
        .unwrap();

        assert_eq!(auth.operation(&id).unwrap().progress_count(), 2);
    }

    #[test]
    fn progress_reported_rejects_non_running() {
        let mut auth = make_authority();
        let id = OperationId::new();
        register(&mut auth, &id, OperationKind::BackgroundToolOp);
        // Still Provisioning.

        let result = auth.apply(OpsLifecycleInput::ProgressReported { operation_id: id });
        assert!(matches!(
            result,
            Err(OpsLifecycleError::InvalidTransition { .. })
        ));
    }

    // ---- CompleteOperation ----

    #[test]
    fn complete_operation_transitions_to_completed() {
        let mut auth = make_authority();
        let id = OperationId::new();
        register(&mut auth, &id, OperationKind::BackgroundToolOp);
        provision_succeed(&mut auth, &id);

        let t = auth
            .apply(OpsLifecycleInput::CompleteOperation {
                operation_id: id.clone(),
            })
            .unwrap();

        let op = auth.operation(&id).unwrap();
        assert_eq!(op.status(), OperationStatus::Completed);
        assert!(op.terminal_outcome().is_some());
        assert!(op.terminal_buffered());
        assert_eq!(auth.active_count(), 0);

        assert!(
            t.effects
                .iter()
                .any(|e| matches!(e, OpsLifecycleEffect::NotifyOpWatcher { .. }))
        );
        assert!(
            t.effects
                .iter()
                .any(|e| matches!(e, OpsLifecycleEffect::RetainTerminalRecord { .. }))
        );
    }

    #[test]
    fn complete_operation_rejects_provisioning() {
        let mut auth = make_authority();
        let id = OperationId::new();
        register(&mut auth, &id, OperationKind::BackgroundToolOp);

        let result = auth.apply(OpsLifecycleInput::CompleteOperation { operation_id: id });
        assert!(matches!(
            result,
            Err(OpsLifecycleError::InvalidTransition { .. })
        ));
    }

    // ---- FailOperation ----

    #[test]
    fn fail_operation_from_provisioning() {
        let mut auth = make_authority();
        let id = OperationId::new();
        register(&mut auth, &id, OperationKind::BackgroundToolOp);

        auth.apply(OpsLifecycleInput::FailOperation {
            operation_id: id.clone(),
        })
        .unwrap();

        assert_eq!(
            auth.operation(&id).unwrap().status(),
            OperationStatus::Failed
        );
    }

    #[test]
    fn fail_operation_from_running() {
        let mut auth = make_authority();
        let id = OperationId::new();
        register(&mut auth, &id, OperationKind::BackgroundToolOp);
        provision_succeed(&mut auth, &id);

        auth.apply(OpsLifecycleInput::FailOperation {
            operation_id: id.clone(),
        })
        .unwrap();

        assert_eq!(
            auth.operation(&id).unwrap().status(),
            OperationStatus::Failed
        );
    }

    // ---- CancelOperation ----

    #[test]
    fn cancel_operation_transitions_to_cancelled() {
        let mut auth = make_authority();
        let id = OperationId::new();
        register(&mut auth, &id, OperationKind::BackgroundToolOp);
        provision_succeed(&mut auth, &id);

        auth.apply(OpsLifecycleInput::CancelOperation {
            operation_id: id.clone(),
        })
        .unwrap();

        assert_eq!(
            auth.operation(&id).unwrap().status(),
            OperationStatus::Cancelled
        );
    }

    // ---- AbortProvisioning ----

    #[test]
    fn abort_provisioning_transitions_to_aborted() {
        let mut auth = make_authority();
        let id = OperationId::new();
        register(&mut auth, &id, OperationKind::BackgroundToolOp);

        auth.apply(OpsLifecycleInput::AbortProvisioning {
            operation_id: id.clone(),
        })
        .unwrap();

        assert_eq!(
            auth.operation(&id).unwrap().status(),
            OperationStatus::Aborted
        );
    }

    #[test]
    fn abort_provisioning_rejects_non_provisioning() {
        let mut auth = make_authority();
        let id = OperationId::new();
        register(&mut auth, &id, OperationKind::BackgroundToolOp);
        provision_succeed(&mut auth, &id);

        let result = auth.apply(OpsLifecycleInput::AbortProvisioning { operation_id: id });
        assert!(matches!(
            result,
            Err(OpsLifecycleError::InvalidTransition { .. })
        ));
    }

    // ---- RetireRequested ----

    #[test]
    fn retire_requested_transitions_to_retiring() {
        let mut auth = make_authority();
        let id = OperationId::new();
        register(&mut auth, &id, OperationKind::BackgroundToolOp);
        provision_succeed(&mut auth, &id);

        auth.apply(OpsLifecycleInput::RetireRequested {
            operation_id: id.clone(),
        })
        .unwrap();

        assert_eq!(
            auth.operation(&id).unwrap().status(),
            OperationStatus::Retiring
        );
    }

    #[test]
    fn retire_requested_rejects_non_running() {
        let mut auth = make_authority();
        let id = OperationId::new();
        register(&mut auth, &id, OperationKind::BackgroundToolOp);
        // Provisioning, not Running.

        let result = auth.apply(OpsLifecycleInput::RetireRequested { operation_id: id });
        assert!(matches!(
            result,
            Err(OpsLifecycleError::InvalidTransition { .. })
        ));
    }

    // ---- RetireCompleted ----

    #[test]
    fn retire_completed_transitions_to_retired() {
        let mut auth = make_authority();
        let id = OperationId::new();
        register(&mut auth, &id, OperationKind::BackgroundToolOp);
        provision_succeed(&mut auth, &id);
        auth.apply(OpsLifecycleInput::RetireRequested {
            operation_id: id.clone(),
        })
        .unwrap();

        auth.apply(OpsLifecycleInput::RetireCompleted {
            operation_id: id.clone(),
        })
        .unwrap();

        assert_eq!(
            auth.operation(&id).unwrap().status(),
            OperationStatus::Retired
        );
    }

    #[test]
    fn mark_retired_from_running_succeeds() {
        let mut auth = make_authority();
        let id = OperationId::new();
        register(&mut auth, &id, OperationKind::BackgroundToolOp);
        provision_succeed(&mut auth, &id);

        auth.apply(OpsLifecycleInput::RetireCompleted {
            operation_id: id.clone(),
        })
        .unwrap();

        assert_eq!(
            auth.operation(&id).unwrap().status(),
            OperationStatus::Retired
        );
    }

    // ---- OwnerTerminated ----

    #[test]
    fn owner_terminated_terminates_all_non_terminal() {
        let mut auth = make_authority();

        let id1 = OperationId::new();
        let id2 = OperationId::new();
        let id3 = OperationId::new();

        register(&mut auth, &id1, OperationKind::BackgroundToolOp);
        register(&mut auth, &id2, OperationKind::BackgroundToolOp);
        provision_succeed(&mut auth, &id2);
        register(&mut auth, &id3, OperationKind::BackgroundToolOp);
        provision_succeed(&mut auth, &id3);
        // Complete id3 so it's already terminal.
        auth.apply(OpsLifecycleInput::CompleteOperation {
            operation_id: id3.clone(),
        })
        .unwrap();

        let t = auth.apply(OpsLifecycleInput::OwnerTerminated).unwrap();

        assert_eq!(
            auth.operation(&id1).unwrap().status(),
            OperationStatus::Terminated
        );
        assert_eq!(
            auth.operation(&id2).unwrap().status(),
            OperationStatus::Terminated
        );
        // id3 was already Completed, should remain Completed.
        assert_eq!(
            auth.operation(&id3).unwrap().status(),
            OperationStatus::Completed
        );
        assert_eq!(auth.active_count(), 0);

        // Should have NotifyOpWatcher for id1 and id2 (not id3).
        let notify_count = t
            .effects
            .iter()
            .filter(|e| matches!(e, OpsLifecycleEffect::NotifyOpWatcher { .. }))
            .count();
        assert_eq!(notify_count, 2);
    }

    // ---- Eviction ----

    #[test]
    fn bounded_completed_evicts_oldest() {
        let mut auth = OpsLifecycleAuthority::new(3, None);

        let mut ids = Vec::new();
        for _ in 0..5 {
            let id = OperationId::new();
            register(&mut auth, &id, OperationKind::BackgroundToolOp);
            provision_succeed(&mut auth, &id);
            auth.apply(OpsLifecycleInput::CompleteOperation {
                operation_id: id.clone(),
            })
            .unwrap();
            ids.push(id);
        }

        // Oldest two should be evicted.
        assert!(auth.operation(&ids[0]).is_none());
        assert!(auth.operation(&ids[1]).is_none());
        assert!(auth.operation(&ids[2]).is_some());
        assert!(auth.operation(&ids[3]).is_some());
        assert!(auth.operation(&ids[4]).is_some());
    }

    // ---- CollectTerminal ----

    #[test]
    fn collect_terminal_clears_buffered_flag() {
        let mut auth = make_authority();
        let id = OperationId::new();
        register(&mut auth, &id, OperationKind::BackgroundToolOp);
        provision_succeed(&mut auth, &id);
        auth.apply(OpsLifecycleInput::CompleteOperation {
            operation_id: id.clone(),
        })
        .unwrap();

        assert!(auth.operation(&id).unwrap().terminal_buffered());

        auth.apply(OpsLifecycleInput::CollectTerminal {
            operation_id: id.clone(),
        })
        .unwrap();

        assert!(!auth.operation(&id).unwrap().terminal_buffered());
    }

    #[test]
    fn collect_terminal_rejects_non_terminal() {
        let mut auth = make_authority();
        let id = OperationId::new();
        register(&mut auth, &id, OperationKind::BackgroundToolOp);
        provision_succeed(&mut auth, &id);

        let result = auth.apply(OpsLifecycleInput::CollectTerminal { operation_id: id });
        assert!(matches!(
            result,
            Err(OpsLifecycleError::InvalidTransition { .. })
        ));
    }

    // ---- watchers_drained ----

    #[test]
    fn watchers_drained_decrements_count() {
        let mut auth = make_authority();
        let id = OperationId::new();
        register(&mut auth, &id, OperationKind::BackgroundToolOp);
        auth.apply(OpsLifecycleInput::RegisterWatcher {
            operation_id: id.clone(),
        })
        .unwrap();
        auth.apply(OpsLifecycleInput::RegisterWatcher {
            operation_id: id.clone(),
        })
        .unwrap();
        auth.apply(OpsLifecycleInput::RegisterWatcher {
            operation_id: id.clone(),
        })
        .unwrap();

        auth.watchers_drained(&id, 3);
        assert_eq!(auth.operation(&id).unwrap().watcher_count(), 0);
    }

    // ---- Phase unchanged on failure ----

    #[test]
    fn phase_unchanged_on_rejected_transition() {
        let mut auth = make_authority();
        let id = OperationId::new();
        register(&mut auth, &id, OperationKind::BackgroundToolOp);

        // Provisioning -> CompleteOperation should fail.
        let _ = auth.apply(OpsLifecycleInput::CompleteOperation {
            operation_id: id.clone(),
        });

        assert_eq!(
            auth.operation(&id).unwrap().status(),
            OperationStatus::Provisioning
        );
    }

    // ---- Progress allowed during Retiring ----

    #[test]
    fn progress_allowed_during_retiring() {
        let mut auth = make_authority();
        let id = OperationId::new();
        register(&mut auth, &id, OperationKind::BackgroundToolOp);
        provision_succeed(&mut auth, &id);
        auth.apply(OpsLifecycleInput::RetireRequested {
            operation_id: id.clone(),
        })
        .unwrap();

        auth.apply(OpsLifecycleInput::ProgressReported {
            operation_id: id.clone(),
        })
        .unwrap();

        assert_eq!(auth.operation(&id).unwrap().progress_count(), 1);
    }

    // ---- PeerReady during Retiring ----

    #[test]
    fn peer_ready_during_retiring() {
        let mut auth = make_authority();
        let id = OperationId::new();
        register(&mut auth, &id, OperationKind::MobMemberChild);
        provision_succeed(&mut auth, &id);
        auth.apply(OpsLifecycleInput::RetireRequested {
            operation_id: id.clone(),
        })
        .unwrap();

        auth.apply(OpsLifecycleInput::PeerReady {
            operation_id: id.clone(),
        })
        .unwrap();

        assert!(auth.operation(&id).unwrap().peer_ready());
    }

    // ---- Complete from Retiring ----

    #[test]
    fn complete_from_retiring() {
        let mut auth = make_authority();
        let id = OperationId::new();
        register(&mut auth, &id, OperationKind::BackgroundToolOp);
        provision_succeed(&mut auth, &id);
        auth.apply(OpsLifecycleInput::RetireRequested {
            operation_id: id.clone(),
        })
        .unwrap();

        auth.apply(OpsLifecycleInput::CompleteOperation {
            operation_id: id.clone(),
        })
        .unwrap();

        assert_eq!(
            auth.operation(&id).unwrap().status(),
            OperationStatus::Completed
        );
    }

    // ---- Terminal rejects further transitions ----

    #[test]
    fn completed_rejects_all_status_transitions() {
        let mut auth = make_authority();
        let id = OperationId::new();
        register(&mut auth, &id, OperationKind::BackgroundToolOp);
        provision_succeed(&mut auth, &id);
        auth.apply(OpsLifecycleInput::CompleteOperation {
            operation_id: id.clone(),
        })
        .unwrap();

        // All further transitions should fail.
        assert!(
            auth.apply(OpsLifecycleInput::ProvisioningSucceeded {
                operation_id: id.clone()
            })
            .is_err()
        );
        assert!(
            auth.apply(OpsLifecycleInput::CompleteOperation {
                operation_id: id.clone()
            })
            .is_err()
        );
        assert!(
            auth.apply(OpsLifecycleInput::FailOperation {
                operation_id: id.clone()
            })
            .is_err()
        );
        assert!(
            auth.apply(OpsLifecycleInput::CancelOperation {
                operation_id: id.clone()
            })
            .is_err()
        );
        assert!(
            auth.apply(OpsLifecycleInput::RetireRequested {
                operation_id: id.clone()
            })
            .is_err()
        );
        assert!(
            auth.apply(OpsLifecycleInput::ProgressReported {
                operation_id: id.clone()
            })
            .is_err()
        );
    }

    // ---- Active count tracks correctly through lifecycle ----

    #[test]
    fn active_count_tracks_lifecycle() {
        let mut auth = make_authority();
        assert_eq!(auth.active_count(), 0);

        let id1 = OperationId::new();
        let id2 = OperationId::new();
        register(&mut auth, &id1, OperationKind::BackgroundToolOp);
        assert_eq!(auth.active_count(), 1);

        register(&mut auth, &id2, OperationKind::BackgroundToolOp);
        assert_eq!(auth.active_count(), 2);

        provision_succeed(&mut auth, &id1);
        auth.apply(OpsLifecycleInput::CompleteOperation { operation_id: id1 })
            .unwrap();
        assert_eq!(auth.active_count(), 1);

        auth.apply(OpsLifecycleInput::FailOperation { operation_id: id2 })
            .unwrap();
        assert_eq!(auth.active_count(), 0);
    }

    // ---- BeginWaitAll / CancelWaitAll ----

    #[test]
    fn begin_wait_all_emits_wait_all_satisfied_with_operation_ids() {
        let mut auth = make_authority();
        let id1 = OperationId::new();
        let id2 = OperationId::new();
        register(&mut auth, &id1, OperationKind::BackgroundToolOp);
        register(&mut auth, &id2, OperationKind::BackgroundToolOp);
        provision_succeed(&mut auth, &id1);
        provision_succeed(&mut auth, &id2);

        // Complete both operations
        auth.apply(OpsLifecycleInput::CompleteOperation {
            operation_id: id1.clone(),
        })
        .unwrap();
        auth.apply(OpsLifecycleInput::CompleteOperation {
            operation_id: id2.clone(),
        })
        .unwrap();

        // BeginWaitAll should emit WaitAllSatisfied immediately
        let ids = vec![id1.clone(), id2.clone()];
        let transition = auth
            .apply(OpsLifecycleInput::BeginWaitAll {
                wait_request_id: test_wait_request_id(),
                operation_ids: ids.clone(),
            })
            .unwrap();

        assert_eq!(transition.effects.len(), 1);
        match &transition.effects[0] {
            OpsLifecycleEffect::WaitAllSatisfied {
                wait_request_id,
                operation_ids,
            } => {
                assert_eq!(*wait_request_id, test_wait_request_id());
                assert_eq!(*operation_ids, ids);
            }
            other => panic!("expected WaitAllSatisfied, got {other:?}"),
        }
    }

    #[test]
    fn begin_wait_all_tracks_pending_wait_until_terminal_completion() {
        let mut auth = make_authority();
        let wait_request_id = test_wait_request_id();
        let id = OperationId::new();
        register(&mut auth, &id, OperationKind::BackgroundToolOp);
        provision_succeed(&mut auth, &id);

        let start = auth
            .apply(OpsLifecycleInput::BeginWaitAll {
                wait_request_id: wait_request_id.clone(),
                operation_ids: vec![id.clone()],
            })
            .unwrap();
        assert!(start.effects.is_empty());
        assert_eq!(auth.wait_request_id(), Some(&wait_request_id));
        assert_eq!(auth.wait_operation_ids(), std::slice::from_ref(&id));

        let completion = auth
            .apply(OpsLifecycleInput::CompleteOperation {
                operation_id: id.clone(),
            })
            .unwrap();
        assert!(completion.effects.iter().any(|effect| matches!(
            effect,
            OpsLifecycleEffect::WaitAllSatisfied {
                wait_request_id: effect_wait_request_id,
                operation_ids,
            } if *effect_wait_request_id == wait_request_id && *operation_ids == vec![id.clone()]
        )));
        assert!(auth.wait_request_id().is_none());
        assert!(auth.wait_operation_ids().is_empty());
    }

    #[test]
    fn cancel_wait_all_clears_pending_wait() {
        let mut auth = make_authority();
        let wait_request_id = test_wait_request_id();
        let id = OperationId::new();
        register(&mut auth, &id, OperationKind::BackgroundToolOp);
        provision_succeed(&mut auth, &id);

        auth.apply(OpsLifecycleInput::BeginWaitAll {
            wait_request_id: wait_request_id.clone(),
            operation_ids: vec![id],
        })
        .unwrap();
        assert_eq!(auth.wait_request_id(), Some(&wait_request_id));

        auth.apply(OpsLifecycleInput::CancelWaitAll { wait_request_id })
            .unwrap();
        assert!(auth.wait_request_id().is_none());
        assert!(auth.wait_operation_ids().is_empty());
    }
}
